//! A MIDI device's code, run here before it's made: the same frame and code the device runs in Max,
//! with Max's globals stood in for (outlet, Task, inlet, messagename, post) and a clock Kumi moves,
//! so a test is exact and instant. The device's own tests run, and Kumi's checks: it runs, it
//! throws nothing, it leaves no note hanging, and it goes quiet once every note is released.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::process::Stdio;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use kumi_common::js::json::stringify;
use kumi_common::js::number::{parse as js_parse, round, to_string as num};
use kumi_common::js::string::{head, trim};
use regex::Regex;
use rquickjs::context::EvalOptions;
use rquickjs::function::IntoArgs;
use rquickjs::prelude::Coerced;
use rquickjs::{Context, Ctx, Error as JsError, FromJs, Function, Runtime, Value as JsValue};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::midi::midi_device_code;
use super::spec::{Control, MidiEvent, MidiEventType, MidiSpec};

/// The most memory the device's code may take, in bytes; 0 for the engine's own limit. The check's own
/// process sets it (TS: the child's `--max-old-space-size=128`).
pub(crate) static MEMORY_LIMIT: AtomicUsize = AtomicUsize::new(0);

/// One run of the device: what it sent (each event at the time its last byte went), what it said, and whether timers were still running.
struct Run {
    output: Vec<MidiEvent>,
    /// What was thrown: an error's message, or None when a thrown value had none (TS: `undefined`).
    errors: Vec<Option<String>>,
    pending: usize,
}

/// JavaScript's ToInt32, for its bit operations on numbers.
fn to_int32(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let wrapped = value.trunc().rem_euclid(4_294_967_296.0);
    if wrapped >= 2_147_483_648.0 {
        (wrapped - 4_294_967_296.0) as i32
    } else {
        wrapped as i32
    }
}

fn bytes_of(event: &MidiEvent) -> Vec<f64> {
    let channel = round(event.channel.unwrap_or(1.0)).clamp(1.0, 16.0) - 1.0;
    let channel = to_int32(channel);
    let b7 = |value: Option<f64>, fallback: f64| round(value.unwrap_or(fallback)).clamp(0.0, 127.0);
    let status = |kind: i32| (kind | channel) as f64;
    match event.kind {
        // A note-on of velocity 0 goes as one: MIDI's other way to say note-off, which a device must handle.
        MidiEventType::NoteOn => vec![status(0x90), b7(event.pitch, 0.0), b7(event.velocity, 100.0)],
        MidiEventType::NoteOff => vec![status(0x80), b7(event.pitch, 0.0), b7(event.velocity, 0.0)],
        MidiEventType::Cc => vec![status(0xB0), b7(event.controller, 0.0), b7(event.value, 0.0)],
        MidiEventType::PitchBend => {
            let value = to_int32(round(event.value.unwrap_or(8192.0)).clamp(0.0, 16383.0));
            vec![status(0xE0), (value & 127) as f64, (value >> 7) as f64]
        }
        MidiEventType::Aftertouch => vec![status(0xD0), b7(event.value, 0.0)],
        MidiEventType::Polytouch => vec![status(0xA0), b7(event.pitch, 0.0), b7(event.value, 0.0)],
        MidiEventType::Program => vec![status(0xC0), b7(event.value, 0.0)],
    }
}

/// Bytes the device sent, as events (each at the time its last byte went).
fn events_of(bytes: &[(f64, f64)]) -> Vec<MidiEvent> {
    let mut events: Vec<MidiEvent> = Vec::new();
    let mut status: f64 = 0.0;
    let mut data: Vec<f64> = Vec::new();
    for &(at, byte) in bytes {
        if byte >= 0xF0 as f64 {
            status = 0.0;
            data.clear();
            continue;
        }
        if byte >= 0x80 as f64 {
            status = byte;
            data.clear();
            continue;
        }
        if status == 0.0 || status.is_nan() {
            continue;
        }
        data.push(byte);
        let kind = to_int32(status) & 0xF0;
        let channel = ((to_int32(status) & 0x0F) + 1) as f64;
        if data.len() < (if kind == 0xC0 || kind == 0xD0 { 1 } else { 2 }) {
            continue;
        }
        let a = data.first().copied().unwrap_or(0.0);
        let b = data.get(1).copied().unwrap_or(0.0);
        data.clear();
        let event = |kind: MidiEventType| MidiEvent { kind, channel: Some(channel), at: Some(at), ..Default::default() };
        if kind == 0x90 && b > 0.0 {
            events.push(MidiEvent { pitch: Some(a), velocity: Some(b), ..event(MidiEventType::NoteOn) });
        } else if kind == 0x80 || kind == 0x90 {
            events.push(MidiEvent { pitch: Some(a), velocity: Some(if kind == 0x80 { b } else { 0.0 }), ..event(MidiEventType::NoteOff) });
        } else if kind == 0xB0 {
            events.push(MidiEvent { controller: Some(a), value: Some(b), ..event(MidiEventType::Cc) });
        } else if kind == 0xE0 {
            events.push(MidiEvent { value: Some((to_int32(a) | (to_int32(b) << 7)) as f64), ..event(MidiEventType::PitchBend) });
        } else if kind == 0xD0 {
            events.push(MidiEvent { value: Some(a), ..event(MidiEventType::Aftertouch) });
        } else if kind == 0xA0 {
            events.push(MidiEvent { pitch: Some(a), value: Some(b), ..event(MidiEventType::Polytouch) });
        } else if kind == 0xC0 {
            events.push(MidiEvent { value: Some(a), ..event(MidiEventType::Program) });
        }
    }
    events
}

/// A timer the device set: when it's due, its place in line, and what it runs.
struct Entry<'js> {
    due: f64,
    order: u64,
    run: Function<'js>,
}

/// The clock Kumi moves, what the device sent and said, and its timers.
struct Shared<'js> {
    clock: f64,
    sent: Vec<(f64, f64)>,
    errors: Vec<Option<String>>,
    queue: Vec<Entry<'js>>,
    order: u64,
}

/// Max's Task and a Date on Kumi's clock, from the functions Kumi gives it. The frame itself stops code being made
/// from strings, as it does in Live.
const PRELUDE: &str = r#"(function (schedule, cancel, clock) {
  const RealDate = Date;
  globalThis.Task = class Task {
    constructor(fn) { this.fn = fn; this.entry = undefined; }
    schedule(ms) { this.entry = schedule(this.fn, ms, this.entry); }
    cancel() { if (this.entry !== undefined) cancel(this.entry); this.entry = undefined; }
  };
  globalThis.Date = class Date extends RealDate { static now() { return clock(); } };
})"#;

static KUMI_DEVICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Kumi device:").unwrap());
static KUMI_DEVICE_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Kumi device:\s*").unwrap());

/// `(error as Error).message`: what was thrown, if it says.
fn message_of(ctx: &Ctx<'_>, error: JsError) -> Option<String> {
    match error {
        JsError::Exception => {
            let thrown = ctx.catch();
            let message =
                thrown.as_object().and_then(|object| object.get::<_, JsValue>("message").ok()).filter(|value| !value.is_undefined())?;
            Some(Coerced::<String>::from_js(ctx, message).map(|text| text.0).unwrap_or_default())
        }
        other => Some(other.to_string()),
    }
}

/// Calls the frame's `name` with `inlet` set, keeping what it throws.
fn call<'js>(ctx: &Ctx<'js>, shared: &Rc<RefCell<Shared<'js>>>, name: &str, inlet: i32, args: impl IntoArgs<'js>) {
    let globals = ctx.globals();
    let result =
        globals.set("inlet", inlet).and_then(|_| globals.get::<_, Function>(name)).and_then(|function| function.call::<_, ()>(args));
    if let Err(error) = result {
        let message = message_of(ctx, error);
        shared.borrow_mut().errors.push(message);
    }
}

/// Moves the clock to `time`, running every timer due by then in order.
fn until<'js>(ctx: &Ctx<'js>, shared: &Rc<RefCell<Shared<'js>>>, time: f64) {
    for _guard in 0..100_000 {
        let next = {
            let mut state = shared.borrow_mut();
            let mut best: Option<usize> = None;
            for (index, entry) in state.queue.iter().enumerate() {
                if entry.due <= time
                    && best.is_none_or(|at| {
                        entry.due < state.queue[at].due || (entry.due == state.queue[at].due && entry.order < state.queue[at].order)
                    })
                {
                    best = Some(index);
                }
            }
            best.map(|index| state.queue.remove(index))
        };
        let Some(next) = next else { break };
        shared.borrow_mut().clock = next.due;
        if let Err(error) = next.run.call::<_, ()>(()) {
            // TS: a timer that threw past the frame's own guard would have ended the check.
            let message = message_of(ctx, error);
            shared.borrow_mut().errors.push(message);
        }
    }
    let mut state = shared.borrow_mut();
    state.clock = state.clock.max(time);
}

/// `String(value)` for a JSON value.
pub(crate) fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => num(number.as_f64().unwrap_or(f64::NAN)),
        Value::String(text) => text.clone(),
        Value::Array(items) => {
            items.iter().map(|item| if item.is_null() { String::new() } else { js_string(item) }).collect::<Vec<_>>().join(",")
        }
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// `Number(value)` for a JSON value.
pub(crate) fn js_number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(flag) => {
            if *flag {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => js_parse(text).unwrap_or(f64::NAN),
        Value::Array(_) => js_parse(&js_string(value)).unwrap_or(f64::NAN),
        Value::Object(_) => f64::NAN,
    }
}

/// Whether a JSON value is truthy.
pub(crate) fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Runs the device on `input` (control values `set` first), then `settle` ms more for its timers.
fn run(controls: &[Control], code: &str, input: &[MidiEvent], set: Option<&Map<String, Value>>, settle: f64) -> Run {
    match try_run(controls, code, input, set, settle) {
        Ok(run) => run,
        Err(error) => Run { output: Vec::new(), errors: vec![Some(format!("the code doesn't run: {error}"))], pending: 0 },
    }
}

fn try_run(controls: &[Control], code: &str, input: &[MidiEvent], set: Option<&Map<String, Value>>, settle: f64) -> rquickjs::Result<Run> {
    let runtime = Runtime::new()?;
    let limit = MEMORY_LIMIT.load(Ordering::Relaxed);
    if limit > 0 {
        runtime.set_memory_limit(limit);
    }
    // The 2 s the frame and the device's own top level may take to run (TS: the script's `timeout`).
    let deadline: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let timed_out: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    runtime.set_interrupt_handler(Some(Box::new({
        let deadline = deadline.clone();
        let timed_out = timed_out.clone();
        move || {
            if deadline.get().is_some_and(|at| Instant::now() > at) {
                timed_out.set(true);
                return true;
            }
            false
        }
    })));
    let context = Context::full(&runtime)?;
    context.with(|ctx| run_in(ctx, &deadline, &timed_out, controls, code, input, set, settle))
}

#[allow(clippy::too_many_arguments)]
fn run_in<'js>(
    ctx: Ctx<'js>,
    deadline: &Rc<Cell<Option<Instant>>>,
    timed_out: &Rc<Cell<bool>>,
    controls: &[Control],
    code: &str,
    input: &[MidiEvent],
    set: Option<&Map<String, Value>>,
    settle: f64,
) -> rquickjs::Result<Run> {
    let shared: Rc<RefCell<Shared<'js>>> =
        Rc::new(RefCell::new(Shared { clock: 0.0, sent: Vec::new(), errors: Vec::new(), queue: Vec::new(), order: 0 }));
    let globals = ctx.globals();
    globals.set(
        "outlet",
        Function::new(ctx.clone(), {
            let shared = shared.clone();
            move |_index: JsValue<'js>, byte: Coerced<f64>| {
                let mut state = shared.borrow_mut();
                let at = state.clock;
                state.sent.push((at, byte.0));
            }
        })?,
    )?;
    globals.set(
        "post",
        Function::new(ctx.clone(), {
            let shared = shared.clone();
            move |text: Coerced<String>| {
                if KUMI_DEVICE.is_match(&text.0) {
                    let said = trim(&KUMI_DEVICE_PREFIX.replace(&text.0, "")).to_string();
                    shared.borrow_mut().errors.push(Some(said));
                }
            }
        })?,
    )?;
    let schedule = Function::new(ctx.clone(), {
        let shared = shared.clone();
        move |run: Function<'js>, ms: Coerced<f64>, previous: JsValue<'js>| -> f64 {
            let mut state = shared.borrow_mut();
            if let Some(previous) = previous.as_number() {
                state.queue.retain(|entry| entry.order as f64 != previous);
            }
            let ms = if ms.0.is_nan() { 0.0 } else { ms.0 };
            let due = state.clock + ms.max(0.0);
            let order = state.order;
            state.order += 1;
            state.queue.push(Entry { due, order, run });
            order as f64
        }
    })?;
    let cancel = Function::new(ctx.clone(), {
        let shared = shared.clone();
        move |previous: Coerced<f64>| {
            shared.borrow_mut().queue.retain(|entry| entry.order as f64 != previous.0);
        }
    })?;
    // Date.now() reads as it would in Live (today, in ms), moved by Kumi's clock: code that asks the time behaves
    // here as it will there.
    let epoch = kumi_common::time::now_ms() as f64;
    let clock = Function::new(ctx.clone(), {
        let shared = shared.clone();
        move || -> f64 { epoch + shared.borrow().clock }
    })?;
    let prelude: Function = ctx.eval(PRELUDE)?;
    prelude.call::<_, ()>((schedule, cancel, clock))?;
    globals.set("inlet", 0)?;
    globals.set("messagename", "")?;
    let mut options = EvalOptions::default();
    options.strict = false;
    deadline.set(Some(Instant::now() + Duration::from_millis(2_000)));
    timed_out.set(false);
    let evaluated = ctx.eval_with_options::<(), _>(midi_device_code(controls, code), options);
    deadline.set(None);
    if let Err(error) = evaluated {
        // TS: Node's own words for a script stopped at its timeout.
        let message = if timed_out.get() { Some("Script execution timed out after 2000ms".to_string()) } else { message_of(&ctx, error) };
        return Ok(Run {
            output: Vec::new(),
            errors: vec![Some(format!("the code doesn't run: {}", message.unwrap_or_else(|| "undefined".to_string())))],
            pending: 0,
        });
    }
    for (name, value) in set.into_iter().flatten() {
        let Some((index, control)) = controls.iter().enumerate().find(|(_, control)| control.name() == name) else {
            shared.borrow_mut().errors.push(Some(format!("the test sets \"{name}\", which isn't one of the controls")));
            continue;
        };
        globals.set("messagename", format!("c{}", index + 1))?;
        let given: f64 = match control {
            Control::Choice(choice) => {
                choice.options.iter().position(|option| *option == js_string(value)).map(|at| at as f64).unwrap_or(-1.0).max(0.0)
            }
            Control::Switch(_) => {
                if js_truthy(value) {
                    1.0
                } else {
                    0.0
                }
            }
            Control::Number(_) | Control::Integer(_) => js_number(value),
        };
        call(&ctx, &shared, "anything", 1, (given,));
    }
    let mut timeline: Vec<(usize, f64, &MidiEvent)> =
        input.iter().enumerate().map(|(index, event)| (index, event.at.unwrap_or(0.0), event)).collect();
    timeline.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    for &(_, at, event) in &timeline {
        until(&ctx, &shared, at);
        for byte in bytes_of(event) {
            call(&ctx, &shared, "msg_int", 0, (byte,));
        }
    }
    until(&ctx, &shared, timeline.last().map(|entry| entry.1).unwrap_or(0.0) + settle);
    let mut state = shared.borrow_mut();
    let pending = state.queue.len();
    state.queue.clear();
    Ok(Run { output: events_of(&state.sent), errors: std::mem::take(&mut state.errors), pending })
}

fn describe(event: &MidiEvent) -> String {
    let parts: [String; 7] = [
        event.kind.as_str().to_string(),
        event.pitch.map(|pitch| format!("pitch {}", num(pitch))).unwrap_or_default(),
        event
            .velocity
            .filter(|_| event.kind == MidiEventType::NoteOn)
            .map(|velocity| format!("velocity {}", num(velocity)))
            .unwrap_or_default(),
        event.controller.map(|controller| format!("cc {}", num(controller))).unwrap_or_default(),
        event.value.map(|value| format!("value {}", num(value))).unwrap_or_default(),
        event.channel.filter(|channel| *channel != 1.0).map(|channel| format!("channel {}", num(channel))).unwrap_or_default(),
        event.at.map(|at| format!("at {} ms", num(round(at)))).unwrap_or_default(),
    ];
    parts.iter().filter(|part| !part.is_empty()).cloned().collect::<Vec<_>>().join(" ")
}

/// Whether `actual` is what `expected` says: the fields it names, and its time within 3 ms.
fn matches(expected: &MidiEvent, actual: &MidiEvent) -> bool {
    if expected.kind != actual.kind {
        return false;
    }
    for (wanted, got) in [
        (expected.pitch, actual.pitch),
        (expected.velocity, actual.velocity),
        (expected.controller, actual.controller),
        (expected.value, actual.value),
    ] {
        if let Some(wanted) = wanted {
            if got != Some(wanted) {
                return false;
            }
        }
    }
    if expected.channel.unwrap_or(1.0) != actual.channel.unwrap_or(1.0) {
        return false;
    }
    match (expected.at, actual.at) {
        (None, _) => true,
        (Some(wanted), Some(got)) => (wanted - got).abs() <= 3.0,
        (Some(_), None) => false,
    }
}

/// Notes the device turned on and never off.
fn hanging(output: &[MidiEvent]) -> Vec<String> {
    let mut on: Vec<(String, i64)> = Vec::new();
    for event in output {
        let key = format!("{}:{}", num(event.channel.unwrap_or(1.0)), event.pitch.map(num).unwrap_or_else(|| "undefined".to_string()));
        let count = on.iter().position(|(seen, _)| *seen == key);
        if event.kind == MidiEventType::NoteOn {
            match count {
                Some(at) => on[at].1 += 1,
                None => on.push((key, 1)),
            }
        } else if event.kind == MidiEventType::NoteOff {
            if let Some(at) = count.filter(|at| on[*at].1 != 0) {
                on[at].1 -= 1;
            }
        }
    }
    on.into_iter().filter(|(_, count)| *count > 0).map(|(key, _)| key.split(':').nth(1).unwrap_or("").to_string()).collect()
}

/// Kumi's own check: a chord, a single note, a controller, a bend and aftertouch, each released.
static PROBE: LazyLock<Vec<MidiEvent>> = LazyLock::new(|| {
    let note = |kind: MidiEventType, pitch: f64, velocity: Option<f64>, at: f64| MidiEvent {
        kind,
        pitch: Some(pitch),
        velocity,
        at: Some(at),
        ..Default::default()
    };
    vec![
        note(MidiEventType::NoteOn, 60.0, Some(100.0), 0.0),
        note(MidiEventType::NoteOn, 64.0, Some(90.0), 4.0),
        note(MidiEventType::NoteOn, 67.0, Some(80.0), 8.0),
        note(MidiEventType::NoteOff, 60.0, None, 400.0),
        note(MidiEventType::NoteOff, 64.0, None, 402.0),
        note(MidiEventType::NoteOff, 67.0, None, 404.0),
        note(MidiEventType::NoteOn, 72.0, Some(110.0), 1_000.0),
        note(MidiEventType::NoteOff, 72.0, None, 1_300.0),
        MidiEvent { kind: MidiEventType::Cc, controller: Some(1.0), value: Some(64.0), at: Some(1_500.0), ..Default::default() },
        MidiEvent { kind: MidiEventType::PitchBend, value: Some(9_000.0), at: Some(1_600.0), ..Default::default() },
        MidiEvent { kind: MidiEventType::Aftertouch, value: Some(50.0), at: Some(1_700.0), ..Default::default() },
    ]
});

/// How a device's check went: its tests passed, of how many, and what's wrong.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub passed: usize,
    pub of: usize,
    pub problems: Vec<String>,
}

/// More events than this out of Kumi's probe (11 in, over about 4 s) is a runaway, not a busy device.
const FLOOD: usize = 2_000;

/// What was thrown, as a message says it: `undefined` for a value without one, "" when joined.
fn thrown(error: &Option<String>, joined: bool) -> String {
    match error {
        Some(message) => message.clone(),
        None if joined => String::new(),
        None => "undefined".to_string(),
    }
}

/// The device's tests and Kumi's checks; problems say what went wrong, for the model to fix. A device
/// that runs free (an LFO, a clock, a generator) keeps sending once every note is released, so it's
/// held only to running without errors and not flooding; an all-notes-off still silences it.
pub fn check_midi_device(spec: &MidiSpec) -> Checked {
    let mut problems: Vec<String> = Vec::new();
    let probe = run(&spec.controls, &spec.code, &PROBE, None, 2_000.0);
    if !probe.errors.is_empty() {
        let mut unique: Vec<&Option<String>> = Vec::new();
        for error in &probe.errors {
            if !unique.contains(&error) {
                unique.push(error);
            }
        }
        problems.push(format!(
            "Kumi's check: it threw: {}",
            unique.iter().take(3).map(|error| thrown(error, true)).collect::<Vec<_>>().join("; ")
        ));
    }
    let loose = if spec.runs_free { Vec::new() } else { hanging(&probe.output) };
    if !loose.is_empty() {
        problems.push(format!(
            "Kumi's check: once every note is released, it leaves notes hanging (pitch {}); send a noteoff for every noteon it sent.",
            loose.join(", ")
        ));
    }
    if probe.pending > 0 && !spec.runs_free {
        problems.push("Kumi's check: its timers keep running after every note is released; stop them (cancel) when nothing is held, or give runs_free: true if it's meant to keep sending on its own.".to_string());
    }
    if probe.output.len() > FLOOD {
        problems.push(format!("Kumi's check: it sent {} events for 11 in, in about 4 s; something sends without end.", probe.output.len()));
    }
    let mut passed = 0;
    for test in &spec.tests {
        let result = run(&spec.controls, &spec.code, &test.input, test.set.as_ref(), 2_000.0);
        let ok = result.errors.is_empty()
            && result.output.len() == test.expect.len()
            && test.expect.iter().zip(&result.output).all(|(expected, actual)| matches(expected, actual));
        if ok {
            passed += 1;
            continue;
        }
        let expected = test.expect.iter().map(describe).collect::<Vec<_>>().join(", ");
        let got = result.output.iter().map(describe).collect::<Vec<_>>().join(", ");
        let threw = result.errors.first().map(|error| format!(" (it threw: {})", thrown(error, false))).unwrap_or_default();
        problems.push(format!(
            "test \"{}\": expected {}; got {}{threw}.",
            test.name,
            if expected.is_empty() { "nothing" } else { &expected },
            if got.is_empty() { "nothing" } else { &got }
        ));
    }
    Checked { passed, of: spec.tests.len(), problems }
}

/// How long a device's check may take before its process is stopped: code that loops forever says so, not hangs Kumi.
pub const CHECK_TIMEOUT_MS: u64 = 10_000;

/// How [`check_midi_device_isolated`] runs: its deadline, when not [`CHECK_TIMEOUT_MS`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IsolatedOptions {
    pub timeout_ms: Option<u64>,
}

/// The check's own program: `KUMI_HARNESS_BIN`, or `kumi-harness` beside this one.
fn harness_binary() -> PathBuf {
    if let Some(given) = std::env::var_os("KUMI_HARNESS_BIN").filter(|path| !path.is_empty()) {
        return PathBuf::from(given);
    }
    let name = if cfg!(windows) { "kumi-harness.exe" } else { "kumi-harness" };
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(|folder| folder.join(name))).unwrap_or_else(|| PathBuf::from(name))
}

/// checkMidiDevice in a process of its own, for code the model wrote: it can read Kumi's code and nothing
/// else (no files written, no programs started, no workers), it gets no environment (no keys), code
/// can't be made from strings (an escape from the device's frame can't compile anything), and it's
/// stopped at the deadline. Only plain data goes in and comes out.
pub async fn check_midi_device_isolated(spec: &MidiSpec, options: IsolatedOptions) -> Checked {
    let of = spec.tests.len();
    let timeout_ms = options.timeout_ms.unwrap_or(CHECK_TIMEOUT_MS);
    let failed = |problems: Vec<String>| Checked { passed: 0, of, problems };
    let mut command = Command::new(harness_binary());
    command.env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut worker = match command.spawn() {
        Ok(worker) => worker,
        Err(error) => return failed(vec![format!("Kumi's check couldn't start: {}", head(&error.to_string(), 200))]),
    };
    let input = stringify(&json!({ "controls": spec.controls, "code": spec.code, "tests": spec.tests, "runsFree": spec.runs_free }));
    let mut stdin = worker.stdin.take();
    let mut stdout = worker.stdout.take();
    let outcome = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let write = async {
            if let Some(mut stdin) = stdin.take() {
                let _ = stdin.write_all(input.as_bytes()).await;
                let _ = stdin.shutdown().await;
            }
        };
        let read = async {
            let mut out: Vec<u8> = Vec::new();
            let Some(stdout) = stdout.as_mut() else { return Some(out) };
            let mut buffer = [0u8; 16_384];
            loop {
                match stdout.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        out.extend_from_slice(&buffer[..count]);
                        if out.len() > 1_000_000 {
                            return None;
                        }
                    }
                }
            }
            Some(out)
        };
        let (_, out) = tokio::join!(write, read);
        if out.is_some() {
            let _ = worker.wait().await;
        }
        out
    })
    .await;
    let _ = worker.start_kill();
    match outcome {
        Err(_) => failed(vec![format!(
            "Kumi's check: it didn't finish within {} s; something loops forever (a while loop, or a timer that reschedules itself at once).",
            num(round(timeout_ms as f64 / 1000.0))
        )]),
        Ok(None) => failed(vec!["Kumi's check: it said far too much.".to_string()]),
        Ok(Some(out)) => match serde_json::from_slice::<Value>(&out) {
            Ok(Value::Object(result)) => {
                let count = |value: Option<&Value>| {
                    let number = value.map(js_number).unwrap_or(f64::NAN);
                    if number.is_nan() || number <= 0.0 {
                        0
                    } else {
                        number as usize
                    }
                };
                let problems = match result.get("problems") {
                    Some(Value::Array(items)) => items.iter().take(20).map(js_string).collect(),
                    _ => Vec::new(),
                };
                Checked { passed: count(result.get("passed")), of: count(result.get("of")), problems }
            }
            Ok(Value::Null) | Err(_) => failed(vec!["Kumi's check: it stopped before saying how it went (the code may have ended its process).".to_string()]),
            Ok(_) => Checked { passed: 0, of: 0, problems: Vec::new() },
        },
    }
}
