//! M4's acceptance runner for REAPER (a lab check, not a test: CI only builds it). Every change kind in Kumi's REAPER
//! table goes through Kumi's whole stack (Kumi's integration, the host in process, Kumi's extension in a real REAPER):
//! made, read back, taken back by Kumi's undo, made again, taken back by REAPER's own undo (HISTORY must follow), Kumi's
//! undo then answering that it's back already, put back by REAPER's redo, and taken back by Kumi once more. Also each
//! action, the escape hatches, selection events, edge cases and (--big) a 200-track project's timings.
//!
//! accept_reaper <kumi-home> --lab <dir> [--only <kind,...|extras>] [--big] [--out <results.md>]
//!
//! - REAPER runs with Kumi's extension connected to `<kumi-home>`, on a new scratch project, with `<dir>/lab.lua` started
//!   at launch: the producer's hand. The runner drops Lua in `<dir>/cmd` and lab.lua answers in `<dir>/done`; it makes
//!   what each case needs (in an undo point of its own), edits as a producer would, and presses REAPER's undo and redo.
//! - A kind of Kumi's REAPER table with no case here is a failure of the runner, so a new kind can't slip past it.
//! - It changes the project it finds: run it on the lab's scratch project, never a producer's.
use async_trait::async_trait;
use daw_bridge::Daw;
use daw_mcp_server::host::Host;
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_agent::core::{context::IntegrationContext, contracts::*};
use kumi_agent::mcp::client::{McpEndpoint, StderrStatus};
use kumi_agent::mcp::types::{CallToolResult, ContentBlock, Implementation, ListToolsResult};
use kumi_common::abort::Signal;
use kumi_daw_own::bridge::{Connect, OwnOptions};
use kumi_daw_reaper::{changes as reaper_changes, create_reaper_integration};
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::{cell::Cell, cell::RefCell, rc::Rc, sync::Arc, time::Duration, time::Instant};

kumi_testkit::in_process!();
use in_process::InProcess;

// ----------------------------------------------------------------------------------------------------------------------
// The producer's hand: Lua run by lab.lua inside the lab REAPER.

#[derive(Clone)]
struct Lab {
    dir: PathBuf,
    next: Rc<Cell<u64>>,
}

impl Lab {
    /// Runs `code` in REAPER (on its main thread, between two of its timer ticks): the lines it printed, or why it failed.
    async fn lua(&self, code: &str) -> Result<Vec<String>, String> {
        let id = self.next.get() + 1;
        self.next.set(id);
        let done = self.dir.join("done");
        let _ = std::fs::remove_file(&done);
        let tmp = self.dir.join("cmd.tmp");
        std::fs::write(&tmp, format!("-- id {id}\n{code}\n")).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.dir.join("cmd")).map_err(|e| e.to_string())?;
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(180) {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let Ok(text) = std::fs::read_to_string(&done) else { continue };
            let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
            if lines.first().map(String::as_str) != Some(&format!("id {id}")) {
                continue;
            }
            let _ = std::fs::remove_file(&done);
            lines.remove(0);
            return match lines.pop() {
                Some(last) if last == "ok" => Ok(lines),
                Some(last) => Err(format!("{last} {}", lines.join(" | "))),
                None => Err("no answer".into()),
            };
        }
        Err("lab.lua didn't answer in 180 s".into())
    }

    /// `code` as one undo point of the producer's (named `what`), as their own edit makes one.
    async fn edit(&self, what: &str, code: &str) -> Result<Vec<String>, String> {
        self.lua(&format!(
            "reaper.Undo_BeginBlock2(0)\nlocal ok, err = pcall(function()\n{code}\nend)\nreaper.Undo_EndBlock2(0, {what:?}, -1)\nif not ok then error(err) end"
        ))
        .await
    }

    /// REAPER's own Edit > Undo or Redo, alone (no other Lua around it: a script after an undo can wipe REAPER's redo list).
    async fn undo(&self) -> Result<Vec<String>, String> {
        self.lua("local what = reaper.Undo_CanUndo2(0) reaper.Main_OnCommand(40029, 0) print(what)").await
    }
    async fn redo(&self) -> Result<Vec<String>, String> {
        self.lua("local what = reaper.Undo_CanRedo2(0) reaper.Main_OnCommand(40030, 0) print(what)").await
    }
}

// ----------------------------------------------------------------------------------------------------------------------
// The host's MCP in process, with the last answers kept and hooks run after a call (a producer's edit between Kumi's
// preview and apply, a tab switch inside a plan).

type Hook = Box<dyn FnOnce() -> LocalBoxFuture<'static, ()>>;

struct Tap {
    inner: InProcess,
    answers: RefCell<HashMap<String, Value>>,
    hooks: RefCell<Vec<(String, Hook)>>,
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A host answer as JSON: its value, or `{refused: code, why}`.
fn answer(result: &CallToolResult) -> Value {
    let text = text_of(result);
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
    if result.is_error == Some(true) {
        let why = value.get("error").and_then(Value::as_str).map(str::to_owned).unwrap_or(text);
        return json!({"refused": value.get("code").cloned().unwrap_or(Value::Null), "why": why});
    }
    value
}

#[async_trait(?Send)]
impl McpEndpoint for Tap {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, name: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        self.inner.list(name, signal).await
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        let result = self.inner.call(name, args, signal).await;
        if let Ok(result) = &result {
            if name.starts_with("change_") || name.starts_with("undo_step") {
                self.answers.borrow_mut().insert(name.to_owned(), answer(result));
            }
        }
        let hook = {
            let mut hooks = self.hooks.borrow_mut();
            hooks.iter().position(|(tool, _)| tool == name).map(|at| hooks.remove(at).1)
        };
        if let Some(hook) = hook {
            hook().await;
        }
        result
    }
    fn on_catalog_changed(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        self.inner.on_catalog_changed(listener)
    }
    fn on_disconnect(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        self.inner.on_disconnect(listener)
    }
    fn on_live_event(&self, listener: Rc<dyn Fn(JsonObject)>) -> Box<dyn Fn()> {
        self.inner.on_live_event(listener)
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.inner.close().await
    }
}

// ----------------------------------------------------------------------------------------------------------------------
// Kumi's REAPER integration, driven as a model would drive it.

struct Session {
    daw: Rc<dyn Integration>,
    tools: RefCell<Vec<Rc<dyn KernelTool>>>,
    rows: Rc<RefCell<Vec<ChangeRecord>>>,
    focus: Rc<RefCell<Vec<(Instant, Option<String>)>>>,
    told: Rc<RefCell<Vec<String>>>,
    /// The host's undo events (REAPER's own undo and redo of Kumi's points), as it passed them on.
    events: Rc<RefCell<Vec<Value>>>,
    tap: Rc<Tap>,
    lab: Lab,
}

struct Ran {
    text: String,
    error: bool,
    ms: u128,
    /// The HISTORY rows it made.
    rows: Vec<String>,
}

impl Session {
    async fn open(home: &Path, lab: Lab) -> Session {
        let tap = Rc::new(Tap {
            inner: InProcess { host: Arc::new(Host::new(Daw::Reaper, home)), id: Cell::new(0) },
            answers: RefCell::default(),
            hooks: RefCell::default(),
        });
        let endpoint = tap.clone();
        let connect: Connect = Rc::new(move |_| {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
        });
        let rows: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
        let focus: Rc<RefCell<Vec<(Instant, Option<String>)>>> = Rc::default();
        let told: Rc<RefCell<Vec<String>>> = Rc::default();
        let (into_rows, into_focus, into_told) = (rows.clone(), focus.clone(), told.clone());
        let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
        context.on_change = Some(Rc::new(move |record| {
            let mut rows = into_rows.borrow_mut();
            match rows.iter_mut().find(|r| r.id == record.id) {
                Some(row) => *row = record,
                None => rows.push(record),
            }
        }));
        context.on_focus = Some(Rc::new(move |focus: Option<DawFocus>| {
            into_focus.borrow_mut().push((Instant::now(), focus.and_then(|f| f.track_ref)));
        }));
        context.on_action = Some(Rc::new(move |event: ActionEvent| into_told.borrow_mut().push(event.title)));
        // As the full screen's FOCUS asks.
        context.focus_drawn = true;
        let daw = create_reaper_integration(OwnOptions { context, kumi_home: home.to_path_buf(), connect: Some(connect) });
        daw.start(Signal::new()).await.expect("Kumi's REAPER integration starts");
        let events: Rc<RefCell<Vec<Value>>> = Rc::default();
        let (into_events, mut from_host) = (events.clone(), tap.inner.host.events());
        tokio::task::spawn_local(async move {
            while let Ok(event) = from_host.recv().await {
                if event["name"] == "undone_in_daw" {
                    into_events.borrow_mut().push(event);
                }
            }
        });
        let session = Session { daw, tools: RefCell::default(), rows, focus, told, events, tap, lab };
        session.observe().await;
        session
    }

    async fn observe(&self) -> Value {
        let observed = self.daw.observe(Signal::new(), None).await.expect("an observation");
        *self.tools.borrow_mut() = observed.tools.clone();
        serde_json::from_str(&observed.context).unwrap_or(Value::Null)
    }

    fn tool(&self, name: &str) -> Option<Rc<dyn KernelTool>> {
        self.tools.borrow().iter().find(|tool| tool.name() == name).cloned()
    }

    async fn execute(&self, name: &str, input: Value) -> Ran {
        let before: Vec<String> = self.rows.borrow().iter().map(|r| r.id.clone()).collect();
        let started = Instant::now();
        let (text, error) = match self.tool(name) {
            Some(tool) => match tool.execute(input.as_object().cloned().unwrap_or_default(), Signal::new()).await {
                Ok(result) => (result.text, result.is_error),
                Err(error) => (format!("runtime error: {error}"), true),
            },
            None => (format!("no {name} tool"), true),
        };
        let ms = started.elapsed().as_millis();
        let rows = self.rows.borrow().iter().map(|r| r.id.clone()).filter(|id| !before.contains(id)).collect();
        Ran { text, error, ms, rows }
    }

    /// One plan of these steps, as the model sends make_changes.
    async fn plan(&self, steps: Vec<(&str, Value)>) -> Ran {
        let steps: Vec<Value> = steps.into_iter().map(|(tool, input)| json!({"tool": tool, "input": input})).collect();
        self.execute("make_changes", json!({"steps": steps})).await
    }

    async fn undo(&self, id: &str, discard: bool) -> Ran {
        let mut input = json!({"change": id});
        if discard {
            input["discard"] = true.into();
        }
        self.execute("undo_change", input).await
    }

    /// The host's read, as Kumi's read tool asks it (long refs).
    async fn read(&self, args: &Value) -> Value {
        let args = args.as_object().cloned().unwrap_or_default();
        match self.tap.inner.call("read", args, Signal::new()).await {
            Ok(result) => answer(&result),
            Err(error) => json!({"refused": "runtime", "why": error.to_string()}),
        }
    }

    fn row(&self, id: &str) -> Option<ChangeRecord> {
        self.rows.borrow().iter().find(|r| r.id == id).cloned()
    }

    /// Waits up to `ms` for a row to reach `state`: how long it took.
    async fn wait_state(&self, id: &str, state: ChangeState, ms: u64) -> Option<u128> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(ms) {
            if self.row(id).is_some_and(|r| r.state == state) {
                return Some(started.elapsed().as_millis());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        None
    }

    fn last(&self, tool: &str) -> Value {
        self.tap.answers.borrow().get(tool).cloned().unwrap_or(Value::Null)
    }

    fn hook(&self, after: &str, code: String, what: &'static str) {
        let lab = self.lab.clone();
        self.tap.hooks.borrow_mut().push((
            after.to_owned(),
            Box::new(move || {
                async move {
                    if let Err(why) = lab.edit(what, &code).await {
                        println!("   hook failed: {why}");
                    }
                }
                .boxed_local()
            }),
        ));
    }
}

// ----------------------------------------------------------------------------------------------------------------------
// Comparing what the reads say.

/// Fields that move without a change: the edit cursor, play state, selection, meters, paging and the epoch.
const VOLATILE: [&str; 11] =
    ["epoch", "layout", "nextCursor", "cursor", "dirty", "selected", "playState", "peakDb", "tabCount", "length", "itemsSelected"];

/// Where `a` and `b` differ (numbers within 1e-6), or None.
fn differ(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for key in x.keys().chain(y.keys()) {
                if VOLATILE.contains(&key.as_str()) {
                    continue;
                }
                if let Some(at) = differ(x.get(key).unwrap_or(&Value::Null), y.get(key).unwrap_or(&Value::Null), &format!("{path}.{key}")) {
                    return Some(at);
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: {} rows vs {}", x.len(), y.len()));
            }
            x.iter().zip(y).enumerate().find_map(|(i, (x, y))| differ(x, y, &format!("{path}[{i}]")))
        }
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.), y.as_f64().unwrap_or(0.));
            ((x - y).abs() > 1e-6 * (1. + x.abs().max(y.abs()))).then(|| format!("{path}: {x} vs {y}"))
        }
        _ if a == b => None,
        (Value::String(x), Value::String(y)) if x.contains('\n') || y.contains('\n') => {
            let (mut xs, mut ys) = (x.lines(), y.lines());
            let mut line = 0;
            loop {
                line += 1;
                match (xs.next(), ys.next()) {
                    (Some(p), Some(q)) if p == q => continue,
                    (None, None) => return None,
                    (p, q) => return Some(format!("{path} line {line}: {:?} vs {:?}", p.unwrap_or(""), q.unwrap_or(""))),
                }
            }
        }
        _ => Some(format!("{path}: {} vs {}", short(&a.to_string(), 120), short(&b.to_string(), 120))),
    }
}

fn differ_all(a: &[Value], b: &[Value]) -> Option<String> {
    a.iter().zip(b).enumerate().find_map(|(i, (a, b))| differ(a, b, &format!("probe{i}")))
}

/// GUIDs (and the hashes of what holds them) out, for comparing two makings of the same thing.
fn unguid(value: &Value) -> Value {
    static GUID: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"\{[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\}").unwrap()
    });
    match value {
        Value::String(text) => Value::String(GUID.replace_all(text, "{G}").into_owned()),
        Value::Array(items) => Value::Array(items.iter().map(unguid).collect()),
        Value::Object(map) => {
            Value::Object(map.iter().filter(|(k, _)| k.as_str() != "hash").map(|(k, v)| (k.clone(), unguid(v))).collect())
        }
        other => other.clone(),
    }
}

fn short(text: &str, n: usize) -> String {
    let flat: String = text.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if flat.chars().count() <= n {
        flat
    } else {
        format!("{}…", flat.chars().take(n).collect::<String>())
    }
}

// ----------------------------------------------------------------------------------------------------------------------
// The cases: one per kind of Kumi's REAPER table.

#[derive(Clone, Copy, PartialEq)]
enum Flow {
    /// Kumi's undo, made again, REAPER's undo and redo, Kumi's undo again.
    Full,
    /// Only REAPER's undo takes it back: Kumi's undo is refused (`daw-only`).
    DawOnly,
    /// REAPER's undo doesn't take it back (`skipped_by_daw_undo`): HISTORY says so; Kumi's undo does.
    Outside,
}

struct Case {
    tool: &'static str,
    /// Lua making what the case needs (one undo point; `CASE` names its tracks); `out(k, v)` names a value for `$k`.
    setup: &'static str,
    /// More values from reads: (key, read args, pick) where pick is `n:field` (row n of `items`), `field=value:field`
    /// (the first row whose field is value) or a JSON pointer (`/state/chunk`).
    resolve: &'static [(&'static str, &'static str, &'static str)],
    /// A Kumi action run before the change (a chain file or template the change loads).
    before: Option<(&'static str, &'static str)>,
    /// The step's input: `$k` is replaced in strings, `"#k"` by the number.
    input: &'static str,
    /// Reads whose answers are compared before, after and after each undo.
    probes: &'static [&'static str],
    flow: Flow,
    /// Why this case is expected to be refused (an API or lab limit), checked to be said honestly.
    limit: Option<&'static str>,
    /// Why making it again reads differently (a new file, a new pool): R isn't compared.
    again: Option<&'static str>,
    /// Why an undo brings it back under new ids (said in its note): undos are compared without GUIDs.
    new_ids: Option<&'static str>,
    /// Why Kumi's undo leaves more than the switch as it was (REAPER lists a parameter's modulation settings until it next loads
    /// the track, though none of them is saved):
    /// Kumi's undo is compared on the `active` probes only.
    switch_only: Option<&'static str>,
}

const fn case(tool: &'static str, setup: &'static str, input: &'static str, probes: &'static [&'static str]) -> Case {
    Case {
        tool,
        setup,
        resolve: &[],
        before: None,
        input,
        probes,
        flow: Flow::Full,
        limit: None,
        again: None,
        new_ids: None,
        switch_only: None,
    }
}

impl Case {
    const fn resolve(mut self, resolve: &'static [(&'static str, &'static str, &'static str)]) -> Self {
        self.resolve = resolve;
        self
    }
    const fn flow(mut self, flow: Flow) -> Self {
        self.flow = flow;
        self
    }
    const fn before(mut self, tool: &'static str, input: &'static str) -> Self {
        self.before = Some((tool, input));
        self
    }
    const fn limit(mut self, why: &'static str) -> Self {
        self.limit = Some(why);
        self
    }
    const fn again(mut self, why: &'static str) -> Self {
        self.again = Some(why);
        self
    }
    const fn new_ids(mut self, why: &'static str) -> Self {
        self.new_ids = Some(why);
        self
    }
    const fn switch_only(mut self, why: &'static str) -> Self {
        self.switch_only = Some(why);
        self
    }
}

// Setups (lab.lua's helpers: T() makes a track named after the case, midi(), audio(), note(), the ref makers).
const ONE: &str = r#"local t = T() out("t", tref(t))"#;
const TWO: &str = r#"local a = T(" A") local b = T(" B") out("a", tref(a)) out("b", tref(b))"#;
const SEND: &str = r#"local a = T(" A") local b = T(" B") out("a", tref(a)) out("b", tref(b)) reaper.CreateTrackSend(a, b)
reaper.SetMediaTrackInfo_Value(b, "I_NCHAN", 4)"#;
const HW: &str =
    r#"local t = T() out("t", tref(t)) local s = reaper.CreateTrackSend(t, nil) reaper.SetTrackSendInfo_Value(t, 1, s, "I_DSTCHAN", 0)"#;
const FX: &str = r#"local t = T() out("t", tref(t)) reaper.TrackFX_AddByName(t, "ReaEQ (Cockos)", false, -1) out("f", fxref(t, 0)) out("p", pref(t, 0, 0))"#;
const FX2: &str = r#"local t = T() out("t", tref(t)) reaper.TrackFX_AddByName(t, "ReaEQ (Cockos)", false, -1) reaper.TrackFX_AddByName(t, "ReaComp (Cockos)", false, -1)
out("f", fxref(t, 0)) out("f2", fxref(t, 1)) out("q", pref(t, 1, 1))"#;
const BOX: &str = r#"local t = T() out("t", tref(t)) reaper.TrackFX_AddByName(t, "Container", false, -1)
reaper.TrackFX_AddByName(t, "ReaEQ (Cockos)", false, -1000 - 0x2000003) out("c", fxref(t, 0)) out("k", "r:fx:" .. reaper.TrackFX_GetFXGUID(t, 0x2000003))"#;
const BOX_MAPPED: &str = r#"local t = T() out("t", tref(t)) reaper.TrackFX_AddByName(t, "Container", false, -1)
reaper.TrackFX_AddByName(t, "ReaEQ (Cockos)", false, -1000 - 0x2000003) out("c", fxref(t, 0))
local _, m = reaper.TrackFX_GetNamedConfigParm(t, 0, "container_map.add.0.0") out("mp", "r:param:" .. reaper.TrackFX_GetFXGUID(t, 0) .. ":" .. m)"#;
const FX_STATE: &str = r#"local t = T() out("t", tref(t)) reaper.TrackFX_AddByName(t, "ReaEQ (Cockos)", false, -1) reaper.TrackFX_AddByName(t, "ReaEQ (Cockos)", false, -1)
reaper.TrackFX_SetParamNormalized(t, 1, 0, 0.9) out("f", fxref(t, 0)) out("f2", fxref(t, 1))"#;
const FX_TO: &str = r#"local t = T() out("t", tref(t)) reaper.TrackFX_AddByName(t, "ReaEQ (Cockos)", false, -1) out("f", fxref(t, 0)) local b = T(" to") out("b", tref(b))"#;
const CHAIN: &str = r#"local a = T(" A") local b = T(" B") out("a", tref(a)) out("b", tref(b))
reaper.TrackFX_AddByName(a, "ReaEQ (Cockos)", false, -1) reaper.TrackFX_AddByName(a, "ReaComp (Cockos)", false, -1)"#;
const MIDI: &str = r#"local t = T() out("t", tref(t)) local it, tk = midi(t, 0, 4) out("i", iref(it)) out("k", kref(tk))"#;
const NOTES: &str = r#"local t = T() out("t", tref(t)) local it, tk = midi(t, 0, 4) out("i", iref(it)) out("k", kref(tk))
note(tk, 0.1, 0.5, 60) note(tk, 1.07, 0.5, 62)"#;
const CCS: &str = r#"local t = T() out("t", tref(t)) local it, tk = midi(t, 0, 4) out("k", kref(tk))
reaper.MIDI_InsertCC(tk, false, false, reaper.MIDI_GetPPQPosFromProjQN(tk, 0.5), 0xB0, 0, 1, 50) reaper.MIDI_Sort(tk)"#;
const AUDIO: &str = r#"local t = T() out("t", tref(t)) local it, tk = audio(t, 0, 1.5) out("i", iref(it)) out("k", kref(tk))"#;
const TAKES: &str = r#"local t = T() out("t", tref(t)) local it, tk = audio(t, 0, 1.5) out("i", iref(it)) out("k", kref(tk))
local tk2 = reaper.AddTakeToMediaItem(it) reaper.SetMediaItemTake_Source(tk2, reaper.PCM_Source_CreateFromFile(WAV)) reaper.SetActiveTake(tk)
reaper.UpdateItemInProject(it) out("k2", kref(tk2))"#;
const STRETCH: &str =
    r#"local t = T() out("t", tref(t)) local it, tk = audio(t, 0, 1.5) out("k", kref(tk)) reaper.SetTakeStretchMarker(tk, -1, 0.5)"#;
const TAKE_MARKS: &str =
    r#"local t = T() out("t", tref(t)) local it, tk = audio(t, 0, 1.5) out("k", kref(tk)) reaper.SetTakeMarker(tk, -1, "accm", 0.3)"#;
const ITEMS2: &str =
    r#"local t = T() out("t", tref(t)) local i = audio(t, 0, 1) local j = audio(t, 1, 1) out("i", iref(i)) out("j", iref(j))"#;
const LANES: &str = r#"local t = T() out("t", tref(t)) reaper.SetMediaTrackInfo_Value(t, "I_FREEMODE", 2)
local i = audio(t, 0, 1.5) local j = audio(t, 0, 1.5) reaper.SetMediaItemInfo_Value(i, "I_FIXEDLANE", 0) reaper.SetMediaItemInfo_Value(j, "I_FIXEDLANE", 1)
reaper.UpdateItemLanes(0) out("i", iref(i)) out("j", iref(j))"#;
const ENV: &str = r#"local t = T() out("t", tref(t)) local env = reaper.GetTrackEnvelopeByChunkName(t, "<VOLENV2")
reaper.GetSetEnvelopeInfo_String(env, "ACTIVE", "1", true) reaper.GetSetEnvelopeInfo_String(env, "VISIBLE", "1", true)
reaper.DeleteEnvelopePointRange(env, -1, 1000)
reaper.InsertEnvelopePoint(env, 1, reaper.ScaleToEnvelopeMode(1, 0.7), 0, 0, false, true)
reaper.InsertEnvelopePoint(env, 3, reaper.ScaleToEnvelopeMode(1, 0.4), 0, 0, false, true) reaper.Envelope_SortPoints(env)"#;
const AITEM: &str = r#"local t = T() out("t", tref(t)) local env = reaper.GetTrackEnvelopeByChunkName(t, "<VOLENV2")
reaper.GetSetEnvelopeInfo_String(env, "ACTIVE", "1", true) reaper.GetSetEnvelopeInfo_String(env, "VISIBLE", "1", true)
reaper.InsertAutomationItem(env, -1, 4, 2)"#;
const FOLDER: &str = r#"local p = T(" parent") local c = T(" child") out("p", tref(p)) out("c", tref(c))
reaper.SetMediaTrackInfo_Value(p, "I_FOLDERDEPTH", 1) reaper.SetMediaTrackInfo_Value(c, "I_FOLDERDEPTH", -1)"#;
const MARKER: &str = r#"local t = T() out("t", tref(t)) reaper.AddProjectMarker2(0, false, 105, 0, "acc marker", 901, 0)"#;
const REGION: &str = r#"local t = T() out("t", tref(t)) reaper.AddProjectMarker2(0, true, 110, 114, "acc region", 902, 0)"#;
const TEMPO_MARK: &str =
    r#"local t = T() out("t", tref(t)) reaper.SetTempoTimeSigMarker(0, -1, 30, -1, -1, 125, 0, 0, false) reaper.UpdateTimeline()"#;
const NOTHING: &str = r#"local t = T() out("t", tref(t))"#;

const MOD_KEPT: &str =
    "REAPER lists a parameter's modulation settings until it next loads the track: Kumi's undo puts them back, off (said)";

const ENV_REF: &[(&str, &str, &str)] = &[("e", r#"{"kind":"envelope","parent":"$t"}"#, "0:ref")];
const AITEM_REF: &[(&str, &str, &str)] =
    &[("e", r#"{"kind":"envelope","parent":"$t"}"#, "0:ref"), ("ai", r#"{"kind":"autoitem","parent":"$e"}"#, "0:ref")];
const SEND_REF: &[(&str, &str, &str)] = &[("s", r#"{"kind":"send","parent":"$a"}"#, "0:ref")];
const HW_REF: &[(&str, &str, &str)] = &[("h", r#"{"kind":"send","parent":"$t"}"#, "0:ref")];
const MARKER_REF: &[(&str, &str, &str)] = &[("m", r#"{"kind":"marker","limit":200}"#, "name=acc marker:ref")];
const REGION_REF: &[(&str, &str, &str)] = &[("r", r#"{"kind":"region","limit":200}"#, "name=acc region:ref")];

// Probes.
const P_T: &[&str] = &[r#"{"ref":"$t"}"#];
const P_SENDS: &[&str] = &[r#"{"kind":"send","parent":"$a"}"#, r#"{"ref":"$a"}"#, r#"{"ref":"$b"}"#];
const P_HW: &[&str] = &[r#"{"kind":"send","parent":"$t"}"#, r#"{"ref":"$t"}"#];
const P_MARKERS: &[&str] = &[r#"{"kind":"marker","limit":200}"#, r#"{"kind":"region","limit":200}"#];
const P_PROJECT: &[&str] = &[r#"{"kind":"project"}"#];
const P_TEMPO: &[&str] = &[
    r#"{"kind":"project"}"#,
    r#"{"kind":"call","fn":"CountTempoTimeSigMarkers","args":[]}"#,
    r#"{"kind":"call","fn":"GetTempoTimeSigMarker","args":[0]}"#,
];
const P_FX: &[&str] = &[r#"{"kind":"fx","parent":"$t"}"#, r#"{"ref":"$f"}"#];
const P_FXLIST: &[&str] = &[r#"{"kind":"fx","parent":"$t"}"#];
// (not the FX's state: ReaEQ's chunk bytes change when a parameter is written and put back, by REAPER's undo too)
const P_PARAMS: &[&str] = &[r#"{"kind":"param","parent":"$f","limit":6}"#];
const P_NOTES: &[&str] = &[r#"{"kind":"note","parent":"$k"}"#];
const P_EVENTS: &[&str] = &[r#"{"kind":"event","parent":"$k"}"#];
const P_ITEMS: &[&str] = &[r#"{"kind":"item","parent":"$t"}"#];
const P_ITEM: &[&str] = &[r#"{"ref":"$i"}"#, r#"{"kind":"item","parent":"$t"}"#];
const P_TAKE: &[&str] = &[r#"{"ref":"$k"}"#];
const P_TAKES: &[&str] = &[r#"{"ref":"$i"}"#, r#"{"ref":"$k"}"#];
const P_TRACKS: &[&str] = &[r#"{"kind":"track","limit":200}"#];
const P_ENV: &[&str] = &[r#"{"kind":"point","parent":"$e"}"#, r#"{"kind":"envelope","parent":"$t"}"#];
const P_ENVS: &[&str] = &[r#"{"kind":"envelope","parent":"$t"}"#];
const P_AITEMS: &[&str] = &[r#"{"kind":"autoitem","parent":"$e"}"#];
const P_LANES: &[&str] = &[
    r#"{"ref":"$t"}"#,
    r#"{"kind":"item","parent":"$t"}"#,
    r#"{"kind":"call","fn":"GetMediaTrackInfo_Value","args":["$t","I_FREEMODE"]}"#,
    r#"{"kind":"call","fn":"GetMediaTrackInfo_Value","args":["$t","C_ALLLANESPLAY"]}"#,
    r#"{"kind":"call","fn":"GetMediaTrackInfo_Value","args":["$t","C_LANEPLAYS:1"]}"#,
    r#"{"kind":"call","fn":"GetSetMediaTrackInfo_String","args":["$t","P_LANENAME:0","",false]}"#,
];

fn mod_probes(family: &str, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .map(|key| format!(r#"{{"kind":"call","fn":"TrackFX_GetNamedConfigParm","args":["$t",0,"param.0.{family}.{key}"]}}"#))
        .collect()
}

fn cases() -> Vec<Case> {
    vec![
        // M3: tracks
        case("track_volume", ONE, r#"{"ref":"$t","volumeDb":-6}"#, P_T),
        case("track_pan", ONE, r#"{"ref":"$t","pan":-0.5}"#, P_T),
        case("track_mute", ONE, r#"{"ref":"$t","mute":true}"#, P_T),
        case("track_solo", ONE, r#"{"ref":"$t","solo":true}"#, P_T),
        case("track_channels", ONE, r#"{"ref":"$t","channels":4}"#, &[r#"{"ref":"$t"}"#, r#"{"kind":"call","fn":"GetMediaTrackInfo_Value","args":["$t","I_NCHAN"]}"#]),
        case("track_rename", ONE, r#"{"ref":"$t","name":"Renamed by Kumi"}"#, P_T),
        case("track_color", ONE, r##"{"ref":"$t","color":"#ff8800"}"##, P_T),
        case("track_add", NOTHING, r#"{"name":"Added by Kumi"}"#, P_TRACKS),
        case("track_move", TWO, r##"{"ref":"$b","index":"#ai"}"##, P_TRACKS).resolve(&[("ai", r#"{"ref":"$a"}"#, "/items/0/index")]),
        case("track_delete", FOLDER, r#"{"ref":"$c"}"#, P_TRACKS),
        // M3: sends
        case("send_add", TWO, r#"{"ref":"$a","toRef":"$b","volumeDb":-6}"#, P_SENDS),
        case("send_remove", SEND, r#"{"ref":"$s"}"#, P_SENDS).resolve(SEND_REF),
        case("send_volume", SEND, r#"{"ref":"$s","volumeDb":-10}"#, P_SENDS).resolve(SEND_REF),
        case("send_pan", SEND, r#"{"ref":"$s","pan":0.3}"#, P_SENDS).resolve(SEND_REF),
        case("send_mute", SEND, r#"{"ref":"$s","mute":true}"#, P_SENDS).resolve(SEND_REF),
        case("send_channels", SEND, r#"{"ref":"$s","dstChannels":{"offset":3}}"#, P_SENDS).resolve(SEND_REF),
        case("send_mode", SEND, r#"{"ref":"$s","mode":"pre-fx"}"#, P_SENDS).resolve(SEND_REF),
        // M3: markers, regions, tempo, loop
        case("marker_add", NOTHING, r#"{"position":101.5,"name":"acc added"}"#, P_MARKERS),
        case("region_add", NOTHING, r#"{"position":102,"end":104,"name":"acc added region"}"#, P_MARKERS),
        case("marker_move", MARKER, r#"{"ref":"$m","position":106}"#, P_MARKERS).resolve(MARKER_REF),
        case("marker_rename", MARKER, r#"{"ref":"$m","name":"acc renamed"}"#, P_MARKERS).resolve(MARKER_REF),
        case("marker_color", MARKER, r##"{"ref":"$m","color":"#00aa44"}"##, P_MARKERS).resolve(MARKER_REF),
        case("marker_delete", MARKER, r#"{"ref":"$m"}"#, P_MARKERS).resolve(MARKER_REF).new_ids("REAPER gives a marker made again a new id (said)"),
        case("tempo_set", NOTHING, r#"{"bpm":97}"#, P_PROJECT),
        case("tempo_marker_set", NOTHING, r#"{"position":20,"bpm":130}"#, P_TEMPO),
        case("tempo_marker_delete", TEMPO_MARK, r#"{"position":30}"#, P_TEMPO),
        case("loop_range", NOTHING, r#"{"start":2,"end":6}"#, P_PROJECT),
        case("loop_repeat", NOTHING, r#"{"on":true}"#, P_PROJECT).flow(Flow::Outside),
        // M3: FX
        case("fx_add", ONE, r#"{"ref":"$t","name":"ReaEQ (Cockos)"}"#, P_FXLIST),
        case("fx_delete", FX, r#"{"ref":"$f"}"#, P_FX),
        case("fx_move", FX2, r#"{"ref":"$f2","index":0}"#, P_FXLIST),
        case("fx_bypass", FX, r#"{"ref":"$f","bypass":true}"#, P_FX),
        case("fx_pins", FX, r#"{"ref":"$f","pins":[{"pin":1,"channels":[2]}]}"#, &[r#"{"kind":"call","fn":"TrackFX_GetPinMappings","args":["$t",0,0,0]}"#]),
        case("fx_param", FX, r#"{"ref":"$p","normalized":0.3}"#, P_PARAMS),
        // M3: notes
        case("notes_add", MIDI, r#"{"ref":"$k","notes":[{"pitch":60,"start":0.5,"length":1,"velocity":100}]}"#, P_NOTES),
        case("notes_change", NOTES, r#"{"ref":"$k","changes":[{"index":0,"pitch":64,"velocity":50}]}"#, P_NOTES),
        case("notes_delete", NOTES, r#"{"ref":"$k","indexes":[0]}"#, P_NOTES),
        // M4: escape hatches
        case(
            "run_lua",
            r#"local t = T() out("t", tref(t)) out("tg", reaper.GetTrackGUID(t))"#,
            r#"{"code":"for i = 0, reaper.CountTracks(0) - 1 do local tr = reaper.GetTrack(0, i) if reaper.GetTrackGUID(tr) == '$tg' then reaper.SetMediaTrackInfo_Value(tr, 'D_VOL', 0.5) end end print('set') return reaper.GetTrack(0, 0)"}"#,
            P_T,
        )
        .flow(Flow::DawOnly),
        case("api_call", ONE, r#"{"fn":"SetMediaTrackInfo_Value","args":["$t","D_VOL",0.25]}"#, P_T).flow(Flow::DawOnly),
        // M4: FX (the rest)
        case("fx_params", FX, r#"{"ref":"$f","values":[{"param":0,"normalized":0.2},{"param":1,"normalized":0.7}]}"#, P_PARAMS),
        case("fx_offline", FX, r#"{"ref":"$f","offline":true}"#, P_FX),
        case("fx_wet", FX, r#"{"ref":"$f","wet":0.5}"#, P_FX),
        case("fx_eq_band", FX, r#"{"ref":"$f","band":1,"type":"notch"}"#, &[r#"{"kind":"call","fn":"TrackFX_GetNamedConfigParm","args":["$t",0,"BANDTYPE1"]}"#])
            .flow(Flow::Outside),
        case("fx_preset", FX, r#"{"ref":"$f","step":1}"#, P_FX),
        case("fx_state", FX_STATE, r#"{"ref":"$f","state":"$st"}"#, P_FX).resolve(&[("st", r#"{"ref":"$f2"}"#, "/items/0/state/chunk")]),
        case("fx_chain_load", CHAIN, r#"{"ref":"$b","file":"accept-chain.RfxChain"}"#, &[r#"{"kind":"fx","parent":"$b"}"#])
            .before("fx_chain_save", r#"{"ref":"$a","name":"accept-chain","overwrite":true}"#),
        case("fx_copy", FX_TO, r#"{"ref":"$f","toRef":"$b"}"#, &[r#"{"kind":"fx","parent":"$b"}"#, r#"{"kind":"fx","parent":"$t"}"#]),
        case("fx_container_add", ONE, r#"{"ref":"$t","name":"acc box"}"#, P_FXLIST),
        case("fx_container_move", BOX, r#"{"ref":"$k","toRef":"$t","index":1}"#, &[r#"{"kind":"fx","parent":"$t"}"#, r#"{"ref":"$c"}"#, r#"{"ref":"$k"}"#]),
        case("fx_parallel", FX2, r#"{"ref":"$f2","parallel":"parallel"}"#, &[r#"{"ref":"$f2"}"#, r#"{"kind":"fx","parent":"$t"}"#]),
        case("fx_container_map", BOX, r#"{"ref":"$c","childRef":"$k","param":0}"#, &[r#"{"kind":"param","parent":"$c","limit":6}"#, r#"{"ref":"$c"}"#]),
        case("fx_container_unmap", BOX_MAPPED, r#"{"ref":"$mp"}"#, &[r#"{"kind":"param","parent":"$c","limit":6}"#, r#"{"ref":"$c"}"#]),
        case("fx_param_mod", FX, r#"{"ref":"$f","param":0,"active":true,"baseline":0.3}"#, &[]).switch_only(MOD_KEPT),
        case("fx_param_lfo", FX, r#"{"ref":"$f","param":0,"active":true,"speed":2,"strength":0.5}"#, &[]).switch_only(MOD_KEPT),
        case("fx_param_acs", FX, r#"{"ref":"$f","param":0,"active":true,"attack":100}"#, &[]).switch_only(MOD_KEPT),
        case("fx_param_link", FX2, r#"{"ref":"$f","param":0,"active":true,"sourceRef":"$q","scale":0.5}"#, &[]).switch_only(MOD_KEPT),
        case("fx_param_learn", FX, r#"{"ref":"$f","param":0,"midiStatus":176,"midiNumber":20}"#, &[]).switch_only(MOD_KEPT),
        // M4: envelopes and automation items
        case("envelope_show", ONE, r#"{"trackRef":"$t","name":"width","visible":true,"active":true}"#, P_ENVS),
        case("envelope_points_add", ENV, r#"{"ref":"$e","points":[{"position":2,"value":-6}]}"#, P_ENV).resolve(ENV_REF),
        case("envelope_points_change", ENV, r#"{"ref":"$e","changes":[{"index":1,"value":-3}]}"#, P_ENV).resolve(ENV_REF),
        case("envelope_points_delete", ENV, r#"{"ref":"$e","indexes":[1]}"#, P_ENV).resolve(ENV_REF),
        case("envelope_range_clear", ENV, r#"{"ref":"$e","start":0.5,"end":3.5}"#, P_ENV).resolve(ENV_REF),
        case("automation_item_add", ENV, r#"{"ref":"$e","position":5,"length":2}"#, P_AITEMS).resolve(ENV_REF).again("each add makes a new pool"),
        case("automation_item_set", AITEM, r#"{"ref":"$ai","mute":true,"baseline":0.3}"#, P_AITEMS).resolve(AITEM_REF),
        case("automation_item_delete", AITEM, r#"{"ref":"$ai"}"#, P_AITEMS).resolve(AITEM_REF),
        case("track_automation_mode", ONE, r#"{"ref":"$t","mode":"touch"}"#, &[r#"{"ref":"$t"}"#, r#"{"kind":"call","fn":"GetMediaTrackInfo_Value","args":["$t","I_AUTOMODE"]}"#]),
        case("send_automation_mode", SEND, r#"{"ref":"$s","mode":"touch"}"#, &[r#"{"kind":"call","fn":"GetTrackSendInfo_Value","args":["$a",0,0,"I_AUTOMODE"]}"#])
            .resolve(SEND_REF),
        case("automation_override", NOTHING, r#"{"mode":"bypass"}"#, &[r#"{"kind":"call","fn":"GetGlobalAutomationOverride","args":[]}"#]).flow(Flow::Outside),
        // M4: items and takes
        case("item_add", ONE, r#"{"ref":"$t","position":1,"length":2}"#, P_ITEMS),
        case("item_import", ONE, r#"{"ref":"$t","path":"$wav","position":0.5}"#, P_ITEMS),
        case("item_move", MIDI, r#"{"ref":"$i","position":3}"#, P_ITEM),
        case("item_split", MIDI, r#"{"ref":"$i","position":2}"#, P_ITEMS),
        case("item_trim", MIDI, r#"{"ref":"$i","start":0.5}"#, P_ITEM),
        case("item_fade", MIDI, r#"{"ref":"$i","fadeIn":{"length":0.5,"shape":1}}"#, P_ITEM),
        case("item_props", MIDI, r#"{"ref":"$i","mute":true,"volumeDb":-3}"#, P_ITEM),
        case("item_group", ITEMS2, r#"{"itemRefs":["$i","$j"],"grouped":true}"#, P_ITEMS),
        case("item_delete", MIDI, r#"{"ref":"$i"}"#, P_ITEMS),
        case("item_duplicate", MIDI, r#"{"ref":"$i","position":5}"#, P_ITEMS),
        case("take_props", AUDIO, r#"{"ref":"$k","volumeDb":-4,"pitch":2}"#, P_TAKE),
        case("take_active", TAKES, r#"{"ref":"$k2"}"#, P_TAKES),
        case("take_add", AUDIO, r#"{"ref":"$i","path":"$wav"}"#, P_TAKES),
        case("take_delete", TAKES, r#"{"ref":"$k2"}"#, P_TAKES),
        case("stretch_markers_add", AUDIO, r#"{"ref":"$k","markers":[{"position":0.5}]}"#, P_TAKE),
        case("stretch_markers_change", STRETCH, r#"{"ref":"$k","changes":[{"index":0,"position":0.6}]}"#, P_TAKE),
        case("stretch_markers_delete", STRETCH, r#"{"ref":"$k","indexes":[0]}"#, P_TAKE),
        case("take_markers_add", AUDIO, r#"{"ref":"$k","markers":[{"sourcePosition":0.3,"name":"acc take marker"}]}"#, P_TAKE),
        case("take_markers_change", TAKE_MARKS, r#"{"ref":"$k","changes":[{"index":0,"name":"acc renamed"}]}"#, P_TAKE),
        case("take_markers_delete", TAKE_MARKS, r#"{"ref":"$k","indexes":[0]}"#, P_TAKE),
        case("track_lanes", AUDIO, r#"{"ref":"$t","mode":"lanes"}"#, P_LANES),
        case("lane_plays", LANES, r#"{"ref":"$t","lane":1,"plays":"only"}"#, P_LANES),
        case("lane_name", LANES, r#"{"ref":"$t","lane":0,"name":"acc lane"}"#, P_LANES),
        case("lane_comp", LANES, r#"{"ref":"$t","lane":1,"start":0.2,"end":1}"#, P_LANES),
        case("razor_set", AUDIO, r#"{"ref":"$t","areas":[{"start":0.5,"end":1}]}"#, P_T),
        case("items_glue", ITEMS2, r#"{"itemRefs":["$i","$j"]}"#, P_ITEMS).again("each glue writes a new file"),
        case("items_render", AUDIO, r#"{"itemRefs":["$i"]}"#, P_TAKES).again("each render writes a new file"),
        case("midi_events_add", MIDI, r#"{"ref":"$k","events":[{"type":"cc","start":0.5,"controller":1,"value":64}]}"#, P_EVENTS),
        case("midi_events_change", CCS, r#"{"ref":"$k","changes":[{"index":0,"value":100}]}"#, P_EVENTS),
        case("midi_events_delete", CCS, r#"{"ref":"$k","indexes":[0]}"#, P_EVENTS),
        case("notes_quantize", NOTES, r#"{"ref":"$k","grid":0.25}"#, P_NOTES),
        case("notes_humanize", NOTES, r#"{"ref":"$k","timing":20,"velocity":10,"seed":7}"#, P_NOTES),
        // M4: track settings
        case("track_phase", ONE, r#"{"ref":"$t","inverted":true}"#, P_T),
        case("track_arm", ONE, r#"{"ref":"$t","arm":true}"#, P_T),
        case("track_record_input", ONE, r#"{"ref":"$t","input":{"kind":"midi","device":"virtual-keyboard","channel":0}}"#, P_T),
        case("track_monitoring", ONE, r#"{"ref":"$t","monitoring":"off"}"#, P_T),
        case("track_record_mode", ONE, r#"{"ref":"$t","mode":"none"}"#, P_T),
        case("track_main_send", ONE, r#"{"ref":"$t","on":false}"#, P_T),
        case("track_fx_enabled", ONE, r#"{"ref":"$t","enabled":false}"#, P_T),
        case("track_pan_mode", ONE, r#"{"ref":"$t","mode":"dual"}"#, P_T),
        case("track_width", ONE, r#"{"ref":"$t","width":0.5,"panMode":"stereo"}"#, P_T),
        case("track_dual_pan", ONE, r#"{"ref":"$t","left":-0.5,"right":0.5,"panMode":"dual"}"#, P_T),
        case("track_pan_law", ONE, r#"{"ref":"$t","law":-3}"#, P_T),
        case("track_lock", ONE, r#"{"ref":"$t","locked":true}"#, &[r#"{"ref":"$t"}"#, r#"{"kind":"call","fn":"GetTrackStateChunk","args":["$t","",false]}"#]),
        case("track_solo_defeat", ONE, r#"{"ref":"$t","defeat":true}"#, P_T),
        case("track_visible", ONE, r#"{"ref":"$t","mixer":false}"#, P_T),
        case("track_folder_state", FOLDER, r#"{"ref":"$p","state":"collapsed"}"#, &[r#"{"ref":"$p"}"#]),
        case("track_midi_out", ONE, r#"{"ref":"$t","device":0,"channel":1}"#, P_T).limit("no MIDI output device in the lab REAPER (said)"),
        // M4: routing
        case("receive_add", TWO, r#"{"ref":"$b","fromRef":"$a"}"#, P_SENDS),
        case("send_phase", SEND, r#"{"ref":"$s","inverted":true}"#, P_SENDS).resolve(SEND_REF),
        case("send_mono", SEND, r#"{"ref":"$s","mono":true}"#, P_SENDS).resolve(SEND_REF),
        case("hw_out_add", ONE, r#"{"ref":"$t","output":{"first":1}}"#, P_HW),
        case("hw_out_remove", HW, r#"{"ref":"$h"}"#, P_HW).resolve(HW_REF),
        case("hw_out_volume", HW, r#"{"ref":"$h","volumeDb":-8}"#, P_HW).resolve(HW_REF),
        case("hw_out_pan", HW, r#"{"ref":"$h","pan":0.4}"#, P_HW).resolve(HW_REF),
        case("hw_out_mute", HW, r#"{"ref":"$h","mute":true}"#, P_HW).resolve(HW_REF),
        case("hw_out_mode", HW, r#"{"ref":"$h","mode":"pre-fx"}"#, P_HW).resolve(HW_REF),
        case("hw_out_channels", HW, r#"{"ref":"$h","output":{"first":2,"mono":true}}"#, P_HW).resolve(HW_REF),
        case("hw_out_phase", HW, r#"{"ref":"$h","inverted":true}"#, P_HW).resolve(HW_REF),
        case("hw_out_mono", HW, r#"{"ref":"$h","mono":true}"#, P_HW).resolve(HW_REF),
        // M4: project
        case("time_selection", NOTHING, r#"{"start":1,"end":3}"#, P_PROJECT),
        case("project_notes", NOTHING, r#"{"notes":"acc notes from Kumi"}"#, P_PROJECT).flow(Flow::Outside),
        case("project_info", NOTHING, r#"{"title":"acc title","author":"acc author"}"#, P_PROJECT).flow(Flow::Outside),
        case("project_grid", NOTHING, r#"{"division":"1/8"}"#, P_PROJECT).flow(Flow::Outside),
        case("project_timebase", NOTHING, r#"{"items":"time"}"#, P_PROJECT).flow(Flow::Outside),
        case("ruler_lane", NOTHING, r#"{"lane":0,"name":"acc lane"}"#, P_PROJECT),
        case("ruler_lane_add", NOTHING, r#"{"name":"acc added lane"}"#, P_PROJECT),
        case("marker_lane", MARKER, r#"{"ref":"$m","lane":0}"#, P_MARKERS).resolve(MARKER_REF),
        case("region_render", REGION, r#"{"ref":"$r","addRefs":["$t"]}"#, &[r#"{"kind":"region","limit":200}"#, r#"{"kind":"call","fn":"EnumRegionRenderMatrix","args":[0,902,0]}"#])
            .resolve(REGION_REF),
        case("track_group", ONE, r#"{"ref":"$t","groupNumber":5,"add":["volume-lead","mute-follow"]}"#, P_T),
        case("track_group_name", NOTHING, r#"{"groupNumber":6,"name":"acc group"}"#, P_PROJECT),
        case("run_action", ONE, r#"{"action":40280,"trackRefs":["$t"]}"#, P_T).flow(Flow::DawOnly),
        case("track_template_insert", ONE, r#"{"name":"accept-template"}"#, P_TRACKS)
            .before("save_track_template", r#"{"trackRefs":["$t"],"name":"accept-template","replace":true}"#),
    ]
}

/// Probes made from a family of an FX parameter's modulation keys (the reads have no field for them).
fn probes_of(case: &Case) -> Vec<String> {
    let modulation: Option<(&str, &[&str])> = match case.tool {
        "fx_param_mod" => Some(("mod", &["active", "baseline"])),
        "fx_param_lfo" => Some(("lfo", &["active", "speed", "strength"])),
        "fx_param_acs" => Some(("acs", &["active", "attack"])),
        "fx_param_link" => Some(("plink", &["active", "effect", "param", "scale"])),
        "fx_param_learn" => Some(("learn", &["midi1", "midi2"])),
        _ => None,
    };
    match modulation {
        Some((family, keys)) => mod_probes(family, keys),
        None => case.probes.iter().map(|p| p.to_string()).collect(),
    }
}

// ----------------------------------------------------------------------------------------------------------------------
// Running a case.

type Env = BTreeMap<String, String>;

/// `template` with `$k` replaced in strings and `"#k"` by the number.
fn fill(template: &str, env: &Env) -> Result<Value, String> {
    static NAME: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| Regex::new(r"\$([a-z][a-z0-9]*)").unwrap());
    let value: Value = serde_json::from_str(template).map_err(|e| format!("template {template}: {e}"))?;
    fn walk(value: &Value, env: &Env, missing: &mut Vec<String>) -> Value {
        match value {
            Value::String(text) if text.starts_with('#') && env.contains_key(&text[1..]) => {
                let text = &env[&text[1..]];
                text.parse::<i64>()
                    .map(|n| json!(n))
                    .or_else(|_| text.parse::<f64>().map(|n| json!(n)))
                    .unwrap_or_else(|_| Value::String(text.clone()))
            }
            Value::String(text) => Value::String(
                NAME.replace_all(text, |c: &regex::Captures| match env.get(&c[1]) {
                    Some(v) => v.clone(),
                    None => {
                        missing.push(c[1].to_owned());
                        c[0].to_owned()
                    }
                })
                .into_owned(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(|v| walk(v, env, missing)).collect()),
            Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), walk(v, env, missing))).collect()),
            other => other.clone(),
        }
    }
    let mut missing = vec![];
    let filled = walk(&value, env, &mut missing);
    if missing.is_empty() {
        Ok(filled)
    } else {
        Err(format!("no value for ${}", missing.join(", $")))
    }
}

fn pick(answer: &Value, how: &str) -> Option<String> {
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if how.starts_with('/') {
        return answer.pointer(how).map(text);
    }
    let (row, field) = how.rsplit_once(':')?;
    let items = answer.get("items")?.as_array()?;
    let found = match row.split_once('=') {
        Some((key, value)) => items.iter().find(|r| r.get(key).and_then(Value::as_str) == Some(value))?,
        None => items.get(row.parse::<usize>().ok()?)?,
    };
    found.get(field).map(text)
}

#[derive(Default)]
struct Outcome {
    tool: String,
    /// Steps in order: (letter, passed, what was seen).
    steps: Vec<(&'static str, bool, String)>,
    apply_ms: Option<u128>,
    undo_ms: Option<u128>,
    title: String,
    note: String,
    limit: Option<String>,
    failed_at: Option<String>,
}

impl Outcome {
    fn step(&mut self, letter: &'static str, passed: bool, seen: impl Into<String>) {
        let seen = seen.into();
        println!("   {letter} {} {}", if passed { "ok" } else { "FAIL" }, short(&seen, 300));
        if !passed && self.failed_at.is_none() {
            self.failed_at = Some(format!("{letter}: {}", short(&seen, 200)));
        }
        self.steps.push((letter, passed, seen));
    }
    fn passed(&self) -> bool {
        self.failed_at.is_none()
    }
}

async fn state(s: &Session, probes: &[Value]) -> Vec<Value> {
    let mut out = Vec::with_capacity(probes.len());
    for probe in probes {
        out.push(quiet_chunks(s.read(probe).await));
    }
    out
}

/// A track chunk without the line REAPER writes into tracks its undo restores (`LANEREC -1 -1 -1 0`, the same as none),
/// as Kumi's own prints leave it out.
fn quiet_chunks(value: Value) -> Value {
    match value {
        Value::String(text) if text.starts_with("<TRACK") => {
            Value::String(text.lines().filter(|line| !line.trim_start().starts_with("LANEREC ")).collect::<Vec<_>>().join("\n"))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(quiet_chunks).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, quiet_chunks(v))).collect()),
        other => other,
    }
}

/// The HISTORY row's words for a change REAPER's undo doesn't take back.
fn says_outside(note: &str) -> bool {
    note.contains("REAPER's undo can't") || note.contains("REAPER's own undo") || note.contains("Edit > Undo")
}

/// What the probes say, reduced to what a step compares: without GUIDs when the case's undo brings things back under new
/// ids; for Kumi's undo of a parameter's modulation, only whether each switch is on (REAPER keeps the settings).
fn seen_as(case: &Case, probes: &[String], answers: &[Value], kumi_undo: bool) -> Vec<Value> {
    answers
        .iter()
        .zip(probes)
        .filter_map(|(answer, probe)| {
            if kumi_undo && case.switch_only.is_some() {
                if !(probe.contains(".active") || probe.contains(".learn.")) {
                    return None;
                }
                let value = answer["out"].as_object().and_then(|out| out.values().find_map(Value::as_str)).unwrap_or("");
                return Some(json!(answer["result"] == true && !matches!(value, "" | "0")));
            }
            Some(if case.new_ids.is_some() { unguid(answer) } else { answer.clone() })
        })
        .collect()
}

async fn resolve(s: &Session, case: &Case, env: &mut Env) -> Result<(), String> {
    for (key, args, how) in case.resolve {
        let args = fill(args, env)?;
        let answer = s.read(&args).await;
        let value = pick(&answer, how).ok_or_else(|| format!("no {how} in {}", short(&answer.to_string(), 400)))?;
        env.insert((*key).into(), value);
    }
    Ok(())
}

/// The bridge's undo events since `from` (what the host told Kumi of REAPER's undo and redo).
fn events_since(s: &Session, from: usize) -> String {
    let events = s.events.borrow();
    let shown: Vec<String> = events[from.min(events.len())..].iter().map(|e| e["data"].to_string()).collect();
    if shown.is_empty() {
        "no undo events".into()
    } else {
        let all = shown.join(" | ");
        println!("   events: {all}");
        all
    }
}

async fn run_case(s: &Session, case: &Case, wav: &str) -> Outcome {
    let mut o = Outcome { tool: case.tool.to_owned(), ..Default::default() };
    println!("== {}", case.tool);
    // Setup: one undo point of its own (REAPER's undo of the change below takes back only the change).
    let made = s.lab.edit("accept setup", &format!("_G.CASE = {:?}\n{}", case.tool.replace('_', " "), case.setup)).await;
    let mut env = Env::new();
    env.insert("wav".into(), wav.into());
    match made {
        Ok(lines) => lines.iter().filter_map(|l| l.split_once('=')).for_each(|(k, v)| {
            env.insert(k.into(), v.into());
        }),
        Err(why) => {
            o.step("setup", false, why);
            return o;
        }
    }
    if let Err(why) = resolve(s, case, &mut env).await {
        o.step("setup", false, why);
        return o;
    }
    if let Some((tool, input)) = case.before {
        let ran = s.execute(tool, fill(input, &env).unwrap_or(Value::Null)).await;
        println!("   before: {tool} {} ms: {}", ran.ms, short(&ran.text, 200));
        if ran.error {
            o.step("setup", false, format!("{tool}: {}", ran.text));
            return o;
        }
    }
    let texts = probes_of(case);
    let (input, probes) = match (fill(case.input, &env), texts.iter().map(|p| fill(p, &env)).collect::<Result<Vec<_>, _>>()) {
        (Ok(input), Ok(probes)) => (input, probes),
        (Err(why), _) | (_, Err(why)) => {
            o.step("setup", false, why);
            return o;
        }
    };
    let back_to =
        |before: &[Value], now: &[Value], kumi: bool| differ_all(&seen_as(case, &texts, before, kumi), &seen_as(case, &texts, now, kumi));
    let s0 = state(s, &probes).await;
    // 1. made through make_changes
    let first = s.plan(vec![(case.tool, input.clone())]).await;
    o.apply_ms = Some(first.ms);
    let applied = s.last("change_apply");
    let step_end = s.last("undo_step_end");
    // (a limit of the lab: refused honestly when it holds; the whole flow when it doesn't, as with a MIDI output another
    // program opened)
    if let Some(why) = case.limit.filter(|_| first.rows.is_empty()) {
        o.limit = Some(why.to_owned());
        o.step("T", first.error || first.text.contains("refused"), format!("refused as expected ({why}): {}", short(&first.text, 300)));
        return o;
    }
    let Some(id) = first.rows.first().cloned() else {
        o.step("T", false, format!("no HISTORY row: {}", short(&first.text, 400)));
        return o;
    };
    if first.error {
        o.step("T", false, format!("error: {}", short(&first.text, 400)));
    }
    let row = s.row(&id).unwrap();
    o.title = row.title.clone();
    o.note = row.note.clone().unwrap_or_default();
    let undo_of = |key: &str| applied.pointer(&format!("/undo/{key}")).and_then(Value::as_bool);
    let s1 = state(s, &probes).await;
    let took = differ_all(&s0, &s1);
    let read_back = took.clone().unwrap_or_else(|| format!("nothing read back changed: {}", short(&json!(s1).to_string(), 300)));
    o.step(
        "T",
        took.is_some() || probes.is_empty(),
        format!("{} ms; {}; undo {}; step {}; read back: {read_back}", first.ms, row.title, applied["undo"], step_end),
    );
    // Outside REAPER's undo by the table, or REAPER keeps no point of it (said in HISTORY).
    let outside = case.flow == Flow::Outside || undo_of("entry") == Some(false) || undo_of("covered") == Some(false);
    if outside {
        o.step("H", says_outside(&o.note), format!("undo {}; HISTORY note: {}", applied["undo"], o.note));
    }
    // 2. Kumi's undo
    let undo = s.undo(&id, false).await;
    o.undo_ms = Some(undo.ms);
    let s2 = state(s, &probes).await;
    let id = if case.flow == Flow::DawOnly {
        o.step("U", undo.error && undo.text.contains("REAPER"), format!("refused: {}", short(&undo.text, 300)));
        id
    } else {
        let back = back_to(&s0, &s2, true);
        let note = case.switch_only.or(case.new_ids).map(|why| format!(" ({why})")).unwrap_or_default();
        o.step(
            "U",
            !undo.error && back.is_none(),
            format!("{} ms: {}; {}{note}", undo.ms, short(&undo.text, 200), back.unwrap_or_else(|| "as before".into())),
        );
        // 3. made again (its refs read again: an undo may bring a thing back under a new id)
        if let Err(why) = resolve(s, case, &mut env).await {
            o.step("R", false, why);
            return o;
        }
        let input = fill(case.input, &env).unwrap_or(input.clone());
        let again = s.plan(vec![(case.tool, input)]).await;
        let Some(id2) = again.rows.first().cloned() else {
            o.step("R", false, format!("no row: {}", short(&again.text, 300)));
            return o;
        };
        let s3 = state(s, &probes).await;
        let same = differ_all(&s1.iter().map(unguid).collect::<Vec<_>>(), &s3.iter().map(unguid).collect::<Vec<_>>());
        let seen = match (case.again, same) {
            (Some(why), _) => format!("{} ms; not compared: {why}", again.ms),
            (None, Some(differs)) => format!("{} ms; {differs}", again.ms),
            (None, None) => format!("{} ms; as the first time", again.ms),
        };
        o.step("R", !again.error && (case.again.is_some() || !seen.contains(" vs ")), seen);
        id2
    };
    let s3 = state(s, &probes).await;
    if outside {
        // Kumi's undo is the only way back: taken back last.
        let last = s.undo(&id, false).await;
        let s6 = state(s, &probes).await;
        let back = back_to(&s0, &s6, true);
        o.step("K", !last.error && back.is_none(), format!("{}; {}", short(&last.text, 200), back.unwrap_or_else(|| "as before".into())));
        teardown(s, &mut o).await;
        return o;
    }
    // 4. REAPER's own undo: the row turns Undone.
    let events = s.events.borrow().len();
    let took_back = match s.lab.undo().await {
        Ok(lines) => lines.join(" "),
        Err(why) => {
            o.step("D", false, format!("REAPER's undo: {why}"));
            return o;
        }
    };
    let seen = s.wait_state(&id, ChangeState::Undone, 4000).await;
    let row = s.row(&id).unwrap();
    let told = if seen.is_none() { format!("; events: {}", events_since(s, events)) } else { String::new() };
    o.step(
        "J",
        seen.is_some(),
        format!("REAPER undid {took_back:?}; HISTORY {:?} after {seen:?} ms: {}{told}", row.state, row.note.clone().unwrap_or_default()),
    );
    let s4 = state(s, &probes).await;
    let back = back_to(&s0, &s4, false);
    o.step("D", back.is_none(), back.map(|d| format!("REAPER's undo left: {d}")).unwrap_or_else(|| "as before".into()));
    // Kumi's undo now: back already.
    let already = s.undo(&id, false).await;
    o.step("A", !already.error && already.text.contains("already"), short(&already.text, 200));
    // 5. REAPER's redo: the row turns Applied.
    let events = s.events.borrow().len();
    let put_back = match s.lab.redo().await {
        Ok(lines) => lines.join(" "),
        Err(why) => {
            o.step("P", false, format!("REAPER's redo: {why}"));
            return o;
        }
    };
    let seen = s.wait_state(&id, ChangeState::Applied, 4000).await;
    let row = s.row(&id).unwrap();
    let s5 = state(s, &probes).await;
    let same = differ_all(&s3, &s5);
    let told = if seen.is_none() { format!("; events: {}", events_since(s, events)) } else { String::new() };
    o.step(
        "P",
        seen.is_some() && same.is_none(),
        format!(
            "REAPER redid {put_back:?}; HISTORY {:?} after {seen:?} ms; {}{told}",
            row.state,
            same.unwrap_or_else(|| "as Kumi made it".into())
        ),
    );
    // 6. Kumi's undo after REAPER's redo: it works, or it's refused honestly.
    let last = s.undo(&id, false).await;
    let s6 = state(s, &probes).await;
    if case.flow == Flow::DawOnly {
        o.step("K", last.error && last.text.contains("REAPER"), format!("refused: {}", short(&last.text, 200)));
        // (taken back with REAPER's undo again, so the next case starts clean)
        let _ = s.lab.undo().await;
        s.wait_state(&id, ChangeState::Undone, 2000).await;
    } else {
        let back = back_to(&s0, &s6, true);
        let honest = last.error && (last.text.contains("changed") || last.text.contains("REAPER"));
        o.step(
            "K",
            (!last.error && back.is_none()) || honest,
            format!("{}; {}", short(&last.text, 200), back.unwrap_or_else(|| "as before".into())),
        );
    }
    teardown(s, &mut o).await;
    o
}

async fn teardown(s: &Session, o: &mut Outcome) {
    if let Err(why) = s.lab.edit("accept teardown", "teardown()").await {
        o.note = format!("{} (teardown: {why})", o.note);
    }
}

// ----------------------------------------------------------------------------------------------------------------------
// Beyond the kinds: actions, the reads' generic call, selection, edge cases, a big project.

struct Extra {
    name: String,
    passed: bool,
    ms: Option<u128>,
    seen: String,
}

fn extra(out: &mut Vec<Extra>, name: &str, passed: bool, ms: Option<u128>, seen: impl Into<String>) {
    let seen = seen.into();
    println!("== {name}: {} {}", if passed { "ok" } else { "FAIL" }, short(&seen, 400));
    out.push(Extra { name: name.into(), passed, ms, seen });
}

async fn one_track(s: &Session, name: &str) -> Option<String> {
    let lines = s.lab.edit("accept setup", &format!("_G.CASE = {name:?}\n{ONE}")).await.ok()?;
    lines.iter().find_map(|l| l.strip_prefix("t=").map(str::to_owned))
}

async fn extras(s: &Session, out: &mut Vec<Extra>) {
    // Unsaved project: a delete keeps no copy (nothing saved to copy), and says nothing false about one.
    let t = one_track(s, "unsaved delete").await.unwrap_or_default();
    let ran = s.plan(vec![("track_delete", json!({"ref": t}))]).await;
    let row = ran.rows.first().and_then(|id| s.row(id));
    extra(
        out,
        "edge: delete in an unsaved project",
        !ran.error && row.is_some(),
        Some(ran.ms),
        format!("{}; note {:?}", short(&ran.text, 200), row.and_then(|r| r.note)),
    );
    if let Some(id) = ran.rows.first() {
        let undo = s.undo(id, false).await;
        extra(out, "edge: its undo", !undo.error, Some(undo.ms), short(&undo.text, 200));
    }
    teardown(s, &mut Outcome::default()).await;

    // Actions: save (the project gets a name, in the folder REAPER saves projects in), then a delete keeps a copy of the
    // saved project beside it.
    let projects = dirs_lab().join("lab/Projects");
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let saved = projects.join(format!("accept-{stamp}.rpp"));
    let ran = s.execute("save_project", json!({"path": saved.to_string_lossy()})).await;
    extra(out, "action: save_project (path)", !ran.error && saved.exists(), Some(ran.ms), short(&ran.text, 200));
    let ran = s.execute("save_project", json!({"version": true})).await;
    extra(out, "action: save_project (version)", !ran.error, Some(ran.ms), short(&ran.text, 200));
    let t = one_track(s, "saved delete").await.unwrap_or_default();
    let ran = s.plan(vec![("track_delete", json!({"ref": t}))]).await;
    let copies: Vec<String> = std::fs::read_dir(&projects)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(&format!("accept-{stamp} (Kumi copy ")))
                .collect()
        })
        .unwrap_or_default();
    extra(
        out,
        "edge: delete in a saved project keeps a copy",
        !ran.error && !copies.is_empty(),
        Some(ran.ms),
        format!("{}; copies {copies:?}", short(&ran.text, 200)),
    );
    if let Some(id) = ran.rows.first() {
        s.undo(id, false).await;
    }
    teardown(s, &mut Outcome::default()).await;

    let t = one_track(s, "actions").await.unwrap_or_default();
    let ran = s.execute("set_cursor", json!({"position": 4})).await;
    let project = s.read(&json!({"kind": "project"})).await["items"][0].clone();
    extra(
        out,
        "action: set_cursor",
        !ran.error && project["cursor"].as_f64().is_some_and(|c| (c - 4.).abs() < 1e-6),
        Some(ran.ms),
        format!("{}; cursor {}", short(&ran.text, 120), project["cursor"]),
    );
    let ran = s.execute("play", json!({})).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let project = s.read(&json!({"kind": "project"})).await["items"][0].clone();
    extra(
        out,
        "action: play",
        !ran.error && project["playState"] == "playing",
        Some(ran.ms),
        format!("{}; {}", short(&ran.text, 120), project["playState"]),
    );
    let ran = s.execute("stop", json!({})).await;
    let project = s.read(&json!({"kind": "project"})).await["items"][0].clone();
    extra(
        out,
        "action: stop",
        !ran.error && project["playState"] == "stopped",
        Some(ran.ms),
        format!("{}; {}", short(&ran.text, 120), project["playState"]),
    );
    let _ = s
        .lab
        .edit(
            "accept setup",
            &format!(
                "local t = by_guid({:?}) reaper.TrackFX_AddByName(t, \"ReaEQ (Cockos)\", false, -1)",
                t.trim_start_matches("r:track:")
            ),
        )
        .await;
    let ran = s.execute("fx_chain_save", json!({"ref": t, "name": "accept-chain-action", "overwrite": true})).await;
    extra(out, "action: fx_chain_save", !ran.error, Some(ran.ms), short(&ran.text, 200));
    let ran = s.execute("save_track_template", json!({"trackRefs": [t], "name": "accept-template-action", "replace": true})).await;
    extra(out, "action: save_track_template", !ran.error, Some(ran.ms), short(&ran.text, 200));
    // The reads' generic call, and Kumi's own read tool (short refs).
    let read = s.read(&json!({"kind": "call", "fn": "GetMediaTrackInfo_Value", "args": [t, "D_VOL"]})).await;
    extra(out, "read: generic call", read.get("result").is_some(), None, short(&read.to_string(), 200));
    let read = s.execute("reaper_read", json!({"kind": "track"})).await;
    extra(out, "read: Kumi's reaper_read", !read.error, Some(read.ms), short(&read.text, 200));
    let refused = s.read(&json!({"kind": "call", "fn": "SetMediaTrackInfo_Value", "args": [t, "D_VOL", 0.5]})).await;
    extra(out, "read: a setter refused", refused.get("refused").is_some(), None, short(&refused.to_string(), 200));

    // A producer's edit between Kumi's preview and apply: refused `changed`, nothing written, no row.
    s.hook(
        "change_preview",
        format!("local t = by_guid({:?}) reaper.SetMediaTrackInfo_Value(t, 'D_VOL', 0.1)", t.trim_start_matches("r:track:")),
        "producer edit",
    );
    let ran = s.plan(vec![("track_volume", json!({"ref": t, "volumeDb": -12}))]).await;
    let vol = s.read(&json!({"kind": "call", "fn": "GetMediaTrackInfo_Value", "args": [t, "D_VOL"]})).await;
    extra(
        out,
        "edge: producer edit between preview and apply",
        ran.rows.is_empty() && ran.text.contains("changed") && (vol["result"].as_f64().unwrap_or(0.) - 0.1).abs() < 1e-9,
        Some(ran.ms),
        format!("{}; D_VOL {}", short(&ran.text, 300), vol["result"]),
    );
    // A producer's edit after Kumi's change: Kumi's undo refuses `changed`; discard puts it back.
    let ran = s.plan(vec![("track_volume", json!({"ref": t, "volumeDb": -12}))]).await;
    let _ = s
        .lab
        .edit(
            "producer edit",
            &format!("local t = by_guid({:?}) reaper.SetMediaTrackInfo_Value(t, 'D_VOL', 0.2)", t.trim_start_matches("r:track:")),
        )
        .await;
    if let Some(id) = ran.rows.first() {
        let undo = s.undo(id, false).await;
        extra(
            out,
            "edge: producer edit after the change, Kumi's undo",
            undo.error && undo.text.contains("changed"),
            Some(undo.ms),
            short(&undo.text, 300),
        );
        let undo = s.undo(id, true).await;
        let vol = s.read(&json!({"kind": "call", "fn": "GetMediaTrackInfo_Value", "args": [t, "D_VOL"]})).await;
        extra(out, "edge: then with discard", !undo.error, Some(undo.ms), format!("{}; D_VOL {}", short(&undo.text, 200), vol["result"]));
    }

    // REAPER tab switch inside a plan: a new tab opens after the first step; the next steps' refs are another project's.
    let tg = t.trim_start_matches("r:track:").to_owned();
    s.lab.lua("_G.ORIG = reaper.EnumProjects(-1)").await.ok();
    s.hook("change_apply", "reaper.Main_OnCommand(40859, 0)".into(), "new tab");
    let ran = s
        .plan(vec![
            ("track_pan", json!({"ref": t, "pan": 0.25})),
            ("track_mute", json!({"ref": t, "mute": true})),
            ("track_solo", json!({"ref": t, "solo": true})),
        ])
        .await;
    extra(
        out,
        "edge: tab switch mid-plan",
        ran.rows.len() == 1 && (ran.text.contains("stale") || ran.text.contains("another")),
        Some(ran.ms),
        format!("rows {:?}: {}", ran.rows, short(&ran.text, 500)),
    );
    s.lab.lua("reaper.Main_OnCommand(40860, 0) reaper.SelectProjectInstance(_G.ORIG)").await.ok();
    tokio::time::sleep(Duration::from_millis(400)).await;
    s.observe().await;
    if let Some(id) = ran.rows.first() {
        let undo = s.undo(id, false).await;
        let pan = s.read(&json!({"kind": "call", "fn": "GetMediaTrackInfo_Value", "args": [t, "D_PAN"]})).await;
        extra(
            out,
            "edge: back in the first tab, Kumi's undo of step 1",
            !undo.error,
            Some(undo.ms),
            format!("{}; D_PAN {}", short(&undo.text, 200), pan["result"]),
        );
    }
    let _ = tg;

    // Selection made in REAPER: Kumi's focus follows (FOCUS polls every 150 ms, as the full screen asks).
    let other = {
        let lines = s.lab.edit("accept setup", "_G.CASE = 'selection'\nlocal b = T(' B') out('b', tref(b))").await.unwrap_or_default();
        lines.iter().find_map(|l| l.strip_prefix("b=").map(str::to_owned)).unwrap_or_default()
    };
    let mut base = vec![];
    for _ in 0..5 {
        let started = Instant::now();
        s.lab.lua("local _ = 1").await.ok();
        base.push(started.elapsed().as_millis());
    }
    let mut lags = vec![];
    for round in 0..8 {
        let target = if round % 2 == 0 { &t } else { &other };
        let started = Instant::now();
        let from = s.focus.borrow().len();
        s.lab
            .lua(&format!(
                "reaper.SetOnlyTrackSelected(by_guid({:?})) reaper.SetMixerScroll(by_guid({:?}))",
                target.trim_start_matches("r:track:"),
                target.trim_start_matches("r:track:")
            ))
            .await
            .ok();
        let mut seen = None;
        while started.elapsed() < Duration::from_millis(1500) {
            if s.focus.borrow()[from..].iter().any(|(_, r)| r.as_deref() == Some(target.as_str())) {
                seen = Some(started.elapsed().as_millis());
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        lags.push(seen);
    }
    base.sort();
    let lua_ms = base[base.len() / 2];
    let worst = lags.iter().map(|l| l.unwrap_or(9999)).max().unwrap_or(9999);
    extra(
        out,
        "selection: Kumi's focus follows REAPER's",
        lags.iter().all(Option::is_some) && worst <= 200 + lua_ms,
        Some(worst),
        format!("from the Lua command: {lags:?} ms (a Lua round trip alone {lua_ms} ms)"),
    );
    teardown(s, &mut Outcome::default()).await;
    let told = s.told.borrow().clone();
    println!("   actions told: {}", short(&told.join(" | "), 400));
}

async fn big(s: &Session, lab_dir: &Path, out: &mut Vec<Extra>) {
    let source = dirs_lab().join("lab/Projects/m2-big.rpp");
    let copy = lab_dir.join("accept-big.rpp");
    if std::fs::copy(&source, &copy).is_err() {
        extra(out, "big: project", false, None, format!("no {}", source.display()));
        return;
    }
    let started = Instant::now();
    let opened = s.lab.lua(&format!("reaper.Main_OnCommand(40859, 0) reaper.Main_openProject('noprompt:' .. {:?}) print(reaper.CountTracks(0), reaper.CountMediaItems(0))", copy.to_string_lossy())).await;
    extra(out, "big: open 200 tracks × 2,000 items", opened.is_ok(), Some(started.elapsed().as_millis()), format!("{opened:?}"));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let started = Instant::now();
    let context = s.observe().await;
    extra(
        out,
        "big: Kumi's look (snapshot)",
        context.is_object(),
        Some(started.elapsed().as_millis()),
        format!("{} tracks listed", context["tracks"].as_array().map_or(0, Vec::len)),
    );
    // (a page at a time, as Kumi reads: the host fits each page into the frame)
    let started = Instant::now();
    let (mut rows, mut pages, mut cursor) = (Vec::new(), 0, None::<String>);
    loop {
        let mut args = json!({"kind": "track", "limit": 200});
        if let Some(cursor) = &cursor {
            args["cursor"] = cursor.as_str().into();
        }
        let page = s.read(&args).await;
        pages += 1;
        rows.extend(page["items"].as_array().cloned().unwrap_or_default());
        cursor = page["nextCursor"].as_str().map(str::to_owned);
        if cursor.is_none() || pages > 20 {
            break;
        }
    }
    extra(
        out,
        "big: read every track row",
        rows.len() >= 200,
        Some(started.elapsed().as_millis()),
        format!("{} rows in {pages} pages", rows.len()),
    );
    let t = rows.get(100).and_then(|r| r["ref"].as_str()).unwrap_or_default().to_owned();
    let started = Instant::now();
    let items = s.read(&json!({"kind": "item", "parent": t})).await;
    let item = items["items"][0]["ref"].as_str().unwrap_or_default().to_owned();
    extra(
        out,
        "big: read a track's items",
        !item.is_empty(),
        Some(started.elapsed().as_millis()),
        format!("{} items", items["items"].as_array().map_or(0, Vec::len)),
    );
    let started = Instant::now();
    let one = s.read(&json!({"ref": item})).await;
    extra(out, "big: read one item", one["items"][0].get("ref").is_some(), Some(started.elapsed().as_millis()), "");
    for (tool, input) in [
        ("track_volume", json!({"ref": t, "volumeDb": -5})),
        ("item_move", json!({"ref": item, "position": 30})),
        ("fx_add", json!({"ref": t, "name": "ReaEQ (Cockos)"})),
        ("track_delete", json!({"ref": t})),
    ] {
        let ran = s.plan(vec![(tool, input)]).await;
        extra(out, &format!("big: {tool}"), !ran.error && !ran.rows.is_empty(), Some(ran.ms), short(&ran.text, 160));
        if let Some(id) = ran.rows.first() {
            let undo = s.undo(id, false).await;
            extra(out, &format!("big: {tool} undone by Kumi"), !undo.error, Some(undo.ms), short(&undo.text, 160));
        }
    }
    let ran = s.plan(vec![("track_volume", json!({"ref": t, "volumeDb": -7}))]).await;
    if let Some(id) = ran.rows.first() {
        let started = Instant::now();
        s.lab.undo().await.ok();
        let seen = s.wait_state(id, ChangeState::Undone, 5000).await;
        extra(
            out,
            "big: REAPER's undo followed in HISTORY",
            seen.is_some(),
            Some(started.elapsed().as_millis()),
            format!("row Undone after {seen:?} ms"),
        );
    }
}

fn dirs_lab() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Development/kumi-daw-lab/reaper")
}

/// A short tone (1.5 s, 440 Hz, mono, 16-bit) for the cases that import or play media.
fn write_tone(path: &Path) {
    let (rate, n) = (44_100u32, 66_150u32);
    let mut bytes = Vec::with_capacity(44 + n as usize * 2);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + n * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(n * 2).to_le_bytes());
    for i in 0..n {
        let v = ((i as f64 * 440. * std::f64::consts::TAU / rate as f64).sin() * 12_000.) as i16;
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, bytes).expect("the tone");
}

// ----------------------------------------------------------------------------------------------------------------------

fn report(outcomes: &[Outcome], extras: &[Extra], missing: &[String], total: Duration) -> String {
    let mut md = String::new();
    let passed = outcomes.iter().filter(|o| o.passed() && o.limit.is_none()).count();
    let limits = outcomes.iter().filter(|o| o.limit.is_some()).count();
    let failed = outcomes.iter().filter(|o| !o.passed()).count();
    md.push_str(&format!(
        "# REAPER M4 acceptance\n\n- Kinds: {} run, {passed} passed, {failed} failed, {limits} limits; {} with no case.\n- Extras: {} run, {} passed.\n- Took {} s.\n- Steps: T made and read back, H HISTORY says REAPER's undo skips it, U Kumi's undo, R made again, J HISTORY follows REAPER's undo, D REAPER's undo read back, A Kumi's undo says back already, P REAPER's redo (HISTORY and read back), K Kumi's undo after the redo.\n\n",
        outcomes.len(),
        missing.len(),
        extras.len(),
        extras.iter().filter(|e| e.passed).count(),
        total.as_secs()
    ));
    if !missing.is_empty() {
        md.push_str(&format!("**Kinds with no case: {}**\n\n", missing.join(", ")));
    }
    md.push_str("| Kind | Result | Apply ms | Undo ms | Steps | Title / first failure / note |\n|---|---|---|---|---|---|\n");
    for o in outcomes {
        let result = match (&o.limit, o.passed()) {
            (Some(_), true) => "limit",
            (_, true) => "pass",
            (_, false) => "FAIL",
        };
        let steps: String =
            o.steps.iter().map(|(l, ok, _)| if *ok { l.to_string() } else { format!("~~{l}~~") }).collect::<Vec<_>>().join(" ");
        let words = match (&o.failed_at, &o.limit) {
            (Some(at), _) => at.clone(),
            (None, Some(why)) => why.clone(),
            (None, None) if o.note.is_empty() => o.title.clone(),
            (None, None) => format!("{} — {}", o.title, o.note),
        };
        let ms = |v: Option<u128>| v.map(|v| v.to_string()).unwrap_or_default();
        md.push_str(&format!(
            "| {} | {result} | {} | {} | {steps} | {} |\n",
            o.tool,
            ms(o.apply_ms),
            ms(o.undo_ms),
            short(&words, 220).replace('|', "\\|")
        ));
    }
    md.push_str("\n| Extra | Result | ms | Seen |\n|---|---|---|---|\n");
    for e in extras {
        md.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            e.name,
            if e.passed { "pass" } else { "FAIL" },
            e.ms.map(|v| v.to_string()).unwrap_or_default(),
            short(&e.seen, 220).replace('|', "\\|")
        ));
    }
    md.push_str("\n- M5/M6 (not here): render, hear, audition, goal, match, catch-up, watch_me, library loaders.\n");
    md
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let usage = "accept_reaper <kumi-home> --lab <dir> [--only <kind,...|extras>] [--big] [--out <results.md>]";
    let Some(home) = argv.first().filter(|a| !a.starts_with("--")) else {
        eprintln!("{usage}");
        std::process::exit(2);
    };
    let flag = |name: &str| argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned();
    let Some(lab_dir) = flag("--lab").map(PathBuf::from) else {
        eprintln!("{usage}");
        std::process::exit(2);
    };
    let only: Option<Vec<String>> = flag("--only").map(|list| list.split(',').map(str::to_owned).collect());
    let big_too = argv.iter().any(|a| a == "--big");
    let out_path = flag("--out");
    let home = PathBuf::from(home);
    let wav = lab_dir.join("tone.wav");
    write_tone(&wav);

    // A kind of the table with no case fails the run.
    let table: Vec<String> = reaper_changes::CHANGES.kinds.iter().map(|k| k.tool.to_owned()).collect();
    let all = cases();
    let missing: Vec<String> = table.iter().filter(|tool| !all.iter().any(|c| c.tool == tool.as_str())).cloned().collect();
    let extra_cases: Vec<&str> = all.iter().map(|c| c.tool).filter(|tool| !table.iter().any(|t| t == tool)).collect();
    if !extra_cases.is_empty() {
        eprintln!("cases for kinds the table doesn't have: {extra_cases:?}");
        std::process::exit(2);
    }

    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let started = Instant::now();
    let (outcomes, extras_seen) = tokio::task::LocalSet::new().block_on(&runtime, async {
        let lab = Lab { dir: lab_dir.clone(), next: Rc::default() };
        let session = Session::open(&home, lab).await;
        println!("== connected and observed in {} ms", started.elapsed().as_millis());
        // A flagged baseline point: REAPER's undo never reaches past it into the bare project.
        session.lab.edit("accept baseline", "reaper.SetMediaTrackInfo_Value(reaper.GetMasterTrack(0), 'D_PAN', 0)").await.ok();
        // A track no case deletes: every case deletes the tracks it made, and a project whose tracks are all new since the
        // extension's last look reads as another project (refs stale).
        session
            .lab
            .edit("accept anchor", "reaper.InsertTrackAtIndex(0, true) reaper.GetSetMediaTrackInfo_String(reaper.GetTrack(0, 0), 'P_NAME', 'accept anchor', true)")
            .await
            .ok();
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut outcomes = vec![];
        let mut seen = vec![];
        let wanted = |name: &str| only.as_ref().is_none_or(|list| list.iter().any(|w| w == name));
        if wanted("extras") {
            extras(&session, &mut seen).await;
        }
        for case in all.iter().filter(|c| wanted(c.tool)) {
            outcomes.push(run_case(&session, case, &wav.to_string_lossy()).await);
        }
        if big_too {
            big(&session, &lab_dir, &mut seen).await;
        }
        session.daw.close().await.ok();
        (outcomes, seen)
    });
    let md = report(&outcomes, &extras_seen, &missing, started.elapsed());
    match out_path {
        Some(path) => std::fs::write(&path, &md).expect("the results"),
        None => println!("{md}"),
    }
    let failed = outcomes.iter().any(|o| !o.passed()) || extras_seen.iter().any(|e| !e.passed) || !missing.is_empty();
    std::process::exit(if failed { 1 } else { 0 });
}
