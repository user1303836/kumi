//! read_with_code: one script in place of many reads. A question over a big project ("which tracks have a compressor
//! set under −20 dB?") takes a read for each device, every row of each would go to the model, and the answer is a
//! handful of them. Here the model writes JavaScript instead: each of its tools that only read is a function, the
//! script loops, filters and counts, and only what it returns goes back.
//!
//! The script runs in Kumi, in a JavaScript engine with nothing of the computer in it: no files, no network, no
//! modules. All it reaches is the functions it's given, and those are the tools that say all they do is read
//! (`KernelTool::only_reads`: the DAW's readers, the library's searches), less the web: no change tool, nothing that
//! plays or writes a file, nothing that leaves the computer. So there's nothing to approve and nothing for HISTORY.
//!
//! The engine has a thread of its own. A call is plain there (no `await` to forget; one that's written anyway does no
//! harm): it crosses to the kernel's thread, which runs the tool as it runs the model's own calls, its input checked
//! against its schema, and answers. A script that never ends can't hold Kumi up, and is bounded by what it does, not
//! by a clock: its calls, its steps and its memory.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use kumi_types::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use kumi_types::daw::DawProfile;
use kumi_types::errors::RuntimeError;
use regex::Regex;
use rquickjs::function::{Opt, Rest};
use rquickjs::{Coerced, Context, Ctx, Error as JsError, FromJs, Function, IntoJs, Promise, Runtime, Value as JsValue};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::schema;
use crate::core::text::cut;

pub const TOOL: &str = "read_with_code";

/// The most of a script (bytes).
const MOST_CODE: usize = 32 * 1024;
/// The most calls one script makes: a project of 200 tracks read device by device is about a thousand.
const MOST_CALLS: usize = 2_000;
/// The most steps one script takes, counted where the engine asks whether to go on (every ten thousand or so loop
/// turns and function calls): a loop that never ends stops here, with no clock read.
const MOST_CHECKS: u32 = 20_000;
/// The most memory the engine takes, and the deepest its calls nest.
const MEMORY: usize = 64 * 1024 * 1024;
const STACK: usize = 512 * 1024;
/// The most of what a script printed that goes back with its result.
const PRINTED: usize = 4_000;
/// The most of a call's error that a script's own error repeats.
const CALL_ERROR: usize = 1_000;
/// Tools that only read, which a script still doesn't call: the web. Nothing a script reads leaves the computer.
const LEFT_OUT: &[&str] = &["search_web", "read_web"];

/// A name a script can call as it's written.
static IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap());
/// The line of the script an error's stack names first.
static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"eval_script:(\d+)").unwrap());

/// Whether a script may call `tool`: all it does is read, on this computer.
fn reads(tool: &dyn KernelTool) -> bool {
    let name = tool.name();
    tool.only_reads() && !LEFT_OUT.contains(&name) && name != TOOL && IDENTIFIER.is_match(name)
}

/// Whether a script's function reads the DAW itself (not the library, whose search may ask the DAW where a clip's
/// file is).
fn reads_the_daw(tool: &dyn KernelTool) -> bool {
    reads(tool) && tool.reach() == Reach::DawRead && !tool.concurrent()
}

/// Whether [`tool`] makes one of `tools`: one of them reads the DAW. (A chat without it has a few searches, which
/// need no script.)
pub fn offered(tools: &[Rc<dyn KernelTool>]) -> bool {
    tools.iter().any(|tool| reads_the_daw(tool.as_ref()))
}

/// What the system text says where the tool is offered: when a script is the way to read. A DAW's own instructions
/// send the model to a script in the DAW for what takes several calls; reading isn't one of those any more.
pub fn guidance(profile: &DawProfile) -> String {
    format!(
        "To read much of the {place} and keep a little of it (something from every track, the whole of a long list), \
         write one {TOOL} script over your read tools, rather than reading call by call or running a script in {app}: \
         only what it returns comes back.",
        place = profile.words.place,
        app = profile.words.app
    )
}

/// The tool, over those of `tools` that only read; None where none reads the DAW. `app` is the DAW as the producer
/// calls it.
pub fn tool(app: &str, tools: &[Rc<dyn KernelTool>]) -> Option<Rc<dyn KernelTool>> {
    let readers: Vec<&Rc<dyn KernelTool>> = tools.iter().filter(|tool| reads(tool.as_ref())).collect();
    // The example's read: one the model has in every request, when there is one.
    let of_the_daw = || readers.iter().filter(|tool| reads_the_daw(tool.as_ref()));
    let first = of_the_daw().find(|tool| !tool.deferred()).or_else(|| of_the_daw().next())?.name().to_string();
    let names: Vec<&str> = readers.iter().map(|tool| tool.name()).collect();
    let description = format!(
        "Reads a lot and keeps only what you need: runs JavaScript in Kumi (not in {app}) where each read tool below is a \
         function. A function takes its tool's input and returns its result (an object when that's JSON), and throws \
         the tool's error; calls are plain, with no await. Use it when an answer takes more than a few reads, or far \
         less than the reads return: loop over tracks or over a list's pages (follow a cursor to its end), filter, \
         count and compare in the script, and `return` only the answer, as small as will do (the few rows, their refs, \
         a count). Refs it returns are current. print() adds a line to the result. It changes nothing. E.g. `const \
         found = []; for (const ref of refs) {{ const rows = {first}({{…}}); … }} return found;`. Functions: {}.",
        names.join(", ")
    );
    // What a script calls is the tool itself, not what stands in front of it for the answer's own calls.
    let tools = readers.into_iter().map(|tool| (tool.name().to_string(), tool.inner().unwrap_or_else(|| tool.clone()))).collect();
    Some(Rc::new(ReadWithCode { description, tools }))
}

struct ReadWithCode {
    description: String,
    /// The tools a script calls, by the names its functions have.
    tools: IndexMap<String, Rc<dyn KernelTool>>,
}

#[async_trait(?Send)]
impl KernelTool for ReadWithCode {
    fn name(&self) -> &str {
        TOOL
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object","additionalProperties":false,"required":["code"],"properties":{
            "code":{"type":"string","minLength":1,"maxLength":MOST_CODE,"description":"The script, as a function's body: call the read functions, then return the answer"}
        }})
        .as_object()
        .expect("schema")
        .clone()
    }
    fn reach(&self) -> Reach {
        Reach::DawRead
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let code = input.get("code").and_then(Value::as_str).unwrap_or("");
        if code.trim().is_empty() || code.len() > MOST_CODE {
            return Ok(ToolResult::error(format!("code: the script, at most {} KB.", MOST_CODE / 1024)));
        }
        let flags = Arc::new(Flags::default());
        // However this ends (its answer, an error, the answer stopped and this dropped), the script ends with it.
        let _ends = Ends(flags.clone());
        let (asks, mut asked) = mpsc::unbounded_channel::<Ask>();
        let (code, names) = (code.to_string(), self.tools.keys().cloned().collect::<Vec<_>>());
        let script = {
            let flags = flags.clone();
            std::thread::Builder::new().name("kumi-read-script".into()).spawn(move || {
                let ended = run(&code, &names, &asks, &flags);
                let _ = asks.send(Ask::Ended(ended));
            })
        };
        if let Err(error) = script {
            return Ok(ToolResult::error(format!("Kumi couldn't start the script ({error}): read with your tools instead.")));
        }
        // The answer stopped: so is the script, wherever it is, and the read it waits on.
        let stopped = || ToolResult::error("The script was stopped.");
        loop {
            let ask = tokio::select! {
                biased;
                _ = signal.cancelled() => return Ok(stopped()),
                ask = asked.recv() => ask,
            };
            match ask {
                Some(Ask::Call { name, input, answer }) => {
                    let called = tokio::select! {
                        biased;
                        _ = signal.cancelled() => return Ok(stopped()),
                        called = self.call(&name, &input, &signal) => called,
                    };
                    let _ = answer.send(called);
                }
                Some(Ask::Ended(ended)) => return Ok(result(ended, flags.calls.load(Ordering::Relaxed).min(MOST_CALLS))),
                None => return Ok(ToolResult::error("The script stopped without an answer: read with your tools instead.")),
            }
        }
    }
}

impl ReadWithCode {
    /// One call of a script's: the tool run as the kernel runs a call of the model's, its input checked first.
    async fn call(&self, name: &str, input: &str, signal: &Signal) -> Answer {
        let Some(tool) = self.tools.get(name) else { return Answer::error(format!("{name} isn't one of the script's functions.")) };
        let Ok(Value::Object(input)) = serde_json::from_str::<Value>(input) else {
            return Answer::error(format!("{name} takes its tool's input, an object: {name}({{…}})."));
        };
        if let Some(problem) = schema::problem(&Value::Object(tool.input_schema()), &Value::Object(input.clone())) {
            return Answer::error(format!("{name}: {problem}"));
        }
        match tool.execute(input, signal.clone()).await {
            Ok(result) if result.is_error => Answer::error(format!("{name}: {}", cut(&result.text, CALL_ERROR))),
            Ok(result) => Answer { json: serde_json::from_str::<serde::de::IgnoredAny>(&result.text).is_ok(), ok: true, text: result.text },
            Err(error) => Answer::error(format!("{name}: {}", cut(&error.to_string(), CALL_ERROR))),
        }
    }
}

/// What the script's thread asks of the kernel's.
enum Ask {
    Call { name: String, input: String, answer: oneshot::Sender<Answer> },
    Ended(Ended),
}

/// A call's answer: the tool's words (`json` when they're JSON), or its error.
struct Answer {
    ok: bool,
    json: bool,
    text: String,
}

impl Answer {
    fn error(text: String) -> Self {
        Self { ok: false, json: false, text }
    }
}

/// How a script ended, and what it printed on the way.
struct Ended {
    how: How,
    printed: String,
}

enum How {
    /// What it returned, as JSON; None when it returned nothing JSON has.
    Returned(Option<String>),
    /// What it threw (or why it couldn't be read as a script), and about where: the engine keeps a line for each call
    /// and each `throw`, so an error between two is given the earlier one's.
    Threw { error: String, line: Option<u32> },
    /// Stopped: by its own bounds, or with the answer.
    Stopped(Why),
    /// It waits on something that never comes.
    Waits,
}

/// Why a script was stopped before it ended.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Why {
    No = 0,
    /// The answer it's part of ended.
    Answer = 1,
    Calls = 2,
    Steps = 3,
}

/// What both threads know of a script as it runs.
#[derive(Default)]
struct Flags {
    stopped: AtomicU8,
    calls: AtomicUsize,
    checks: AtomicU32,
}

impl Flags {
    /// The first reason holds.
    fn stop(&self, why: Why) {
        let _ = self.stopped.compare_exchange(Why::No as u8, why as u8, Ordering::Relaxed, Ordering::Relaxed);
    }
    fn why(&self) -> Why {
        match self.stopped.load(Ordering::Relaxed) {
            1 => Why::Answer,
            2 => Why::Calls,
            3 => Why::Steps,
            _ => Why::No,
        }
    }
    /// The engine asks whether to stop: once stopped, at every asking, so nothing the script catches lets it go on.
    fn check(&self) -> bool {
        if self.checks.fetch_add(1, Ordering::Relaxed) >= MOST_CHECKS {
            self.stop(Why::Steps);
        }
        self.why() != Why::No
    }
}

/// Stops the script when the call that started it is over.
struct Ends(Arc<Flags>);
impl Drop for Ends {
    fn drop(&mut self) {
        self.0.stop(Why::Answer);
    }
}

/// What printed lines are kept, and whether some weren't.
#[derive(Default)]
struct Printed {
    text: String,
    cut: bool,
}

/// The script, run to its end on this thread.
fn run(code: &str, names: &[String], asks: &mpsc::UnboundedSender<Ask>, flags: &Arc<Flags>) -> Ended {
    let printed = Rc::new(RefCell::new(Printed::default()));
    let how = (|| -> rquickjs::Result<How> {
        let runtime = Runtime::new()?;
        runtime.set_memory_limit(MEMORY);
        runtime.set_max_stack_size(STACK);
        runtime.set_interrupt_handler(Some(Box::new({
            let flags = flags.clone();
            move || flags.check()
        })));
        let context = Context::full(&runtime)?;
        context.with(|ctx| within(&ctx, code, names, asks, flags, &printed))
    })()
    .unwrap_or_else(|error| How::Threw { error: error.to_string(), line: None });
    // Stopped is stopped, whatever the script made of it.
    let how = match flags.why() {
        Why::No => how,
        why => How::Stopped(why),
    };
    let printed = printed.borrow();
    Ended { how, printed: if printed.cut { format!("{}\n…", printed.text) } else { printed.text.clone() } }
}

fn within<'js>(
    ctx: &Ctx<'js>,
    code: &str,
    names: &[String],
    asks: &mpsc::UnboundedSender<Ask>,
    flags: &Arc<Flags>,
    printed: &Rc<RefCell<Printed>>,
) -> rquickjs::Result<How> {
    let globals = ctx.globals();
    for name in names {
        let (tool, asks, flags) = (name.clone(), asks.clone(), flags.clone());
        let function = Function::new(ctx.clone(), move |ctx: Ctx<'js>, input: Opt<JsValue<'js>>| -> rquickjs::Result<JsValue<'js>> {
            let throw = |words: &str| Err(rquickjs::Exception::throw_message(&ctx, words));
            let input = match input.0.filter(|input| !input.is_undefined()) {
                Some(input) => ctx.json_stringify(input)?.map(|text| text.to_string()).transpose()?.unwrap_or_default(),
                None => "{}".to_string(),
            };
            if flags.calls.fetch_add(1, Ordering::Relaxed) >= MOST_CALLS {
                flags.stop(Why::Calls);
            }
            if flags.why() != Why::No {
                return throw("stopped");
            }
            let (answer, answered) = oneshot::channel();
            if asks.send(Ask::Call { name: tool.clone(), input, answer }).is_err() {
                flags.stop(Why::Answer);
                return throw("stopped");
            }
            // Waits here for the kernel's thread; an answer that's stopped drops its end, and the wait is over.
            let Ok(answer) = answered.blocking_recv() else {
                flags.stop(Why::Answer);
                return throw("stopped");
            };
            if !answer.ok {
                return throw(&answer.text);
            }
            if answer.json {
                ctx.json_parse(answer.text)
            } else {
                answer.text.into_js(&ctx)
            }
        })?;
        globals.set(name.as_str(), function)?;
    }
    let print = Function::new(ctx.clone(), {
        let printed = printed.clone();
        move |ctx: Ctx<'js>, values: Rest<JsValue<'js>>| {
            let words: Vec<String> = values.0.into_iter().map(|value| shown(&ctx, value)).collect();
            let mut printed = printed.borrow_mut();
            if printed.cut {
                return;
            }
            let line = words.join(" ");
            let room = PRINTED.saturating_sub(printed.text.len());
            if !printed.text.is_empty() {
                printed.text.push('\n');
            }
            printed.text.push_str(cut(&line, room));
            printed.cut = line.len() > room;
        }
    })?;
    globals.set("print", print.clone())?;
    let console = rquickjs::Object::new(ctx.clone())?;
    for level in ["log", "info", "warn", "error", "debug"] {
        console.set(level, print.clone())?;
    }
    globals.set("console", console)?;
    // The body starts on the wrapper's own line, so an error's line is the script's.
    let promise: rquickjs::Result<Promise<'js>> = ctx.eval(format!("(async function(){{{code}\n}})()"));
    let returned = promise.and_then(|promise| promise.finish::<JsValue<'js>>());
    Ok(match returned {
        Ok(value) if value.is_undefined() => How::Returned(None),
        Ok(value) => match ctx.json_stringify(value) {
            Ok(Some(text)) => How::Returned(Some(text.to_string()?)),
            Ok(None) => How::Returned(None),
            Err(error) => thrown(ctx, error),
        },
        Err(JsError::WouldBlock) => How::Waits,
        Err(error) => thrown(ctx, error),
    })
}

/// A printed value as the script's author reads it: text as it is, anything else as JSON.
fn shown<'js>(ctx: &Ctx<'js>, value: JsValue<'js>) -> String {
    if !value.is_string() {
        if let Ok(Some(text)) = ctx.json_stringify(value.clone()) {
            if let Ok(text) = text.to_string() {
                return text;
            }
        }
        // (A value JSON has no form for threw, or gave nothing: its own words do.)
        let _ = ctx.catch();
    }
    Coerced::<String>::from_js(ctx, value).map(|text| text.0).unwrap_or_default()
}

/// What a script threw, in its own words, with the line it was near.
fn thrown<'js>(ctx: &Ctx<'js>, error: JsError) -> How {
    let JsError::Exception = error else { return How::Threw { error: error.to_string(), line: None } };
    let thrown = ctx.catch();
    let text = |value: JsValue<'js>| Coerced::<String>::from_js(ctx, value).map(|text| text.0).unwrap_or_default();
    let Some(object) = thrown.as_object().filter(|object| object.is_error()).cloned() else {
        return How::Threw { error: text(thrown), line: None };
    };
    let field = |name: &str| object.get::<_, JsValue<'js>>(name).ok().filter(|value| !value.is_undefined()).map(&text).unwrap_or_default();
    let (kind, message, stack) = (field("name"), field("message"), field("stack"));
    let line = LINE.captures(&stack).and_then(|found| found[1].parse().ok());
    // A plain Error is a call's (the tool's own words, which name it) or the script's own `throw`: its kind adds nothing.
    let error = if kind.is_empty() || kind == "Error" { message } else { format!("{kind}: {message}") };
    How::Threw { error, line }
}

/// The call's result: what the script returned, or what went wrong and what to do about it.
fn result(ended: Ended, calls: usize) -> ToolResult {
    let made = match calls {
        1 => "1 call".to_string(),
        calls => format!("{calls} calls"),
    };
    let printed = if ended.printed.is_empty() { String::new() } else { format!("\nPrinted:\n{}", ended.printed) };
    match ended.how {
        How::Returned(Some(returned)) => {
            let printed = if ended.printed.is_empty() { String::new() } else { format!(",\"printed\":{}", stringify(&json!(ended.printed))) };
            ToolResult::text(format!("{{\"result\":{returned}{printed},\"calls\":{calls}}}"))
        }
        How::Returned(None) if !ended.printed.is_empty() => ToolResult::text(stringify(&json!({"printed": ended.printed, "calls": calls}))),
        How::Returned(None) => ToolResult::error(format!("The script returned nothing ({made} made): end it with `return` and the answer.")),
        How::Threw { error, line } => {
            let at = line.map(|line| format!(" near line {line}")).unwrap_or_default();
            let error = error.trim_end_matches('.');
            let memory = if error.contains("out of memory") { " A script has 64 MB: keep less of what it reads." } else { "" };
            ToolResult::error(format!("The script stopped{at}: {error}.{memory} It had made {made}.{printed}"))
        }
        How::Waits => ToolResult::error(format!(
            "The script waits for something that never comes ({made} made): its functions answer at once, so call them without promises or timers.{printed}"
        )),
        How::Stopped(Why::Calls) => ToolResult::error(format!(
            "The script was stopped at {MOST_CALLS} calls, the most one makes: read in bigger pages, or narrow what it reads.{printed}"
        )),
        How::Stopped(Why::Steps) => ToolResult::error(format!(
            "The script was stopped: it ran too long without ending ({made} made). Check that each loop ends, and that a cursor is followed only while there is one.{printed}"
        )),
        How::Stopped(_) => ToolResult::error("The script was stopped."),
    }
}
