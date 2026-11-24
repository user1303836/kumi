//! read_with_code: one script in place of many reads. A question over a big project ("which tracks have a compressor
//! set under −20 dB?") takes a read for each device, every row of each would go to the model, and the answer is a
//! handful of them. Here the model writes JavaScript instead: each of its tools that only read is a function, the
//! script loops, filters and counts, and only what it returns goes back.
//!
//! The script runs in Kumi, in a JavaScript engine with nothing of the computer in it: no files, no network, no
//! modules. All it reaches is the functions it's given, and those are the tools that say all they do is read
//! (`KernelTool::only_reads`: the DAW's readers, the library's presets and earlier projects, Kumi's own records): no
//! change tool, nothing that plays, renders or writes a file, nothing that goes online. So there's nothing to approve
//! and nothing for HISTORY.
//!
//! The engine has a thread of its own. A call is plain there (no `await` to forget; one that's written anyway does no
//! harm, and what a script starts without awaiting still runs before its answer is read): it crosses to the kernel's
//! thread, which runs the tool as it runs the model's own calls, its input checked against its schema, and answers.
//!
//! A script can't hold Kumi up or take it down. It's bounded by what it does, not by a clock: its calls, its steps
//! (the engine asks at each loop turn and function call, and inside a regular expression) and its memory, which the
//! engine is handed piece by piece here (`Heap`). Past a bound it's stopped where it is; one that gets past its own
//! stop (the engine lets a promise's own code swallow one) is left where it stands, its thread taking no more time.

use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
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
use rquickjs::allocator::{Allocator, RustAllocator};
use rquickjs::function::Opt;
use rquickjs::promise::PromiseState;
use rquickjs::{Coerced, Context, Ctx, Error as JsError, FromJs, Function, IntoJs, Object, Persistent, Promise, Runtime, Value as JsValue};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::schema;
use crate::core::hidden::remember_text;
use crate::core::text::cut;

pub const TOOL: &str = "read_with_code";

/// The most of a script (bytes).
const MOST_CODE: usize = 32 * 1024;
/// The most calls one script makes: a project of 200 tracks read device by device is about a thousand.
const MOST_CALLS: usize = 2_000;
/// The most steps one script takes, counted where the engine asks whether to go on (every ten thousand or so loop
/// turns, function calls and steps of a regular expression's match): a loop that never ends stops here, and so does a
/// pattern that backtracks without end, with no clock read. A stopped script that's still asked this often again has
/// got past its stop, and is left where it stands.
const MOST_CHECKS: u32 = 20_000;
/// The most memory a script holds: past it, it's stopped.
const MEMORY: usize = 64 * 1024 * 1024;
/// Once past `MEMORY`, pieces this big are refused, and smaller ones still given until the script has stopped: the
/// engine answers a refused big piece (an array's room, a long text) with an error and carries on, but one it can't
/// have for the error itself, or for a promise's parts, leaves it leaking or reading what it freed.
const BIG: usize = 16 * 1024;
/// The most a script is ever given, in the small pieces that still come past `MEMORY`: there it's left where it stands.
const MEMORY_END: usize = 2 * MEMORY;
/// The deepest the engine's calls nest, and the stack its thread has: room beyond the engine's own bound for what
/// runs between two of its checks.
const STACK: usize = 512 * 1024;
const THREAD_STACK: usize = 4 * STACK;
/// The most of what a script printed that goes back with its result, as `cut` counts (UTF-16 units).
const PRINTED: usize = 4_000;
/// The most of a call's error that a script's own error repeats.
const CALL_ERROR: usize = 1_000;
/// How much of a result's fields an error shows: how deep, how many of an object's, how many tools', how long each.
const OUTLINE_DEPTH: usize = 4;
const OUTLINE_FIELDS: usize = 12;
const OUTLINE_TOOLS: usize = 4;
const OUTLINE: usize = 400;
/// How many of the failures a script left unread are kept, each with the promise it's of (the first is reported; the
/// rest are counted).
const MOST_UNREAD: usize = 16;
/// The same call made this often is a loop that isn't moving on, not a script that reads a lot.
const AGAIN: usize = 3;

/// A name a script can call as it's written.
static IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap());
/// The line of the script an error's stack names first.
static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"eval_script:(\d+)").unwrap());

tokio::task_local! {
    /// A script's call of a tool is under way in this task.
    static CALLING: ();
}

/// Whether a script's call of a tool is under way here: what the tool says it's doing isn't shown, since the step is
/// the script's and the call has no start or end of its own.
pub fn calling() -> bool {
    CALLING.try_with(|_| ()).is_ok()
}

/// Whether a script may call `tool`: all it does is read, whatever it's given.
fn reads(tool: &dyn KernelTool) -> bool {
    let name = tool.name();
    tool.only_reads() && name != TOOL && IDENTIFIER.is_match(name)
}

/// Whether a script's function reads the DAW itself (not Kumi's own library or records).
fn reads_the_daw(tool: &dyn KernelTool) -> bool {
    reads(tool) && tool.reach() == Reach::DawRead
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

/// The tool, over those of `tools` that only read; None where none reads the DAW. The DAW's `profile` gives its name
/// and how its reads answer.
pub fn tool(profile: &DawProfile, tools: &[Rc<dyn KernelTool>]) -> Option<Rc<dyn KernelTool>> {
    let readers: Vec<&Rc<dyn KernelTool>> = tools.iter().filter(|tool| reads(tool.as_ref())).collect();
    // The example's read: one the model has in every request, when there is one.
    let of_the_daw = || readers.iter().filter(|tool| reads_the_daw(tool.as_ref()));
    let first = of_the_daw().find(|tool| !tool.deferred()).or_else(|| of_the_daw().next())?.name().to_string();
    let names: Vec<&str> = readers.iter().map(|tool| tool.name()).collect();
    let app = profile.words.app;
    // Nothing else tells a script's author where a result keeps its rows before a first result does.
    let answers = match profile.vocab.read_answer {
        "" => String::new(),
        answers => format!(" In {app}, {answers}."),
    };
    let description = format!(
        "Reads a lot and keeps only what you need: runs JavaScript in Kumi (not in {app}) where each read tool below is a \
         function. A function takes its tool's input and returns its result (an object when that's JSON), and throws \
         the tool's error; calls are plain, with no await.{answers} Use it when an answer takes more than a few reads, or far \
         less than the reads return: loop over tracks or over a list's pages (follow a cursor while there is one), filter, \
         count and compare in the script, and `return` only the answer, as small as will do (the few rows, their refs, \
         a count). Refs it returns are current. print() adds a line to the result. It changes nothing. E.g. `const \
         found = []; for (const ref of refs) {{ const rows = {first}({{…}}); … }} return found;`. Functions: {}.",
        names.join(", ")
    );
    // What a script calls is the tool itself, not what stands in front of it for the answer's own calls.
    let tools = readers.into_iter().map(|tool| (tool.name().to_string(), tool.inner().unwrap_or_else(|| tool.clone()))).collect();
    Some(Rc::new(ReadWithCode { description, tools, app, place: profile.words.place }))
}

struct ReadWithCode {
    description: String,
    /// The tools a script calls, by the names its functions have.
    tools: IndexMap<String, Rc<dyn KernelTool>>,
    /// The DAW's name and what it calls a project, for what a result says.
    app: &'static str,
    place: &'static str,
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
            std::thread::Builder::new().name("kumi-read-script".into()).stack_size(THREAD_STACK).spawn(move || {
                let ended = run(&code, &names, &asks, &flags);
                let _ = asks.send(Ask::Ended(ended));
            })
        };
        if let Err(error) = script {
            return Ok(ToolResult::error(format!("Kumi couldn't start the script ({error}): read with your tools instead.")));
        }
        let mut reads = Reads { dropped: self.drops(), ..Reads::default() };
        loop {
            // The answer stopped: so is the script, wherever it is.
            let ask = tokio::select! {
                biased;
                _ = signal.cancelled() => return Ok(ToolResult::error("The script was stopped.")),
                ask = asked.recv() => ask,
            };
            match ask {
                Some(Ask::Call { name, input, answer }) => {
                    // A read under way when the answer stops isn't dropped where it stands: it hears the stop itself
                    // and winds down as it does for a call of the answer's own (the kernel never drops one either).
                    let called = self.call(&name, &input, &signal, &mut reads).await;
                    if reads.gone.is_some() {
                        flags.stop(Why::Gone);
                    }
                    let _ = answer.send(called);
                }
                Some(Ask::Ended(ended)) => return Ok(self.result(ended, &reads)),
                None => return Ok(ToolResult::error("The script stopped without an answer: read with your tools instead.")),
            }
        }
    }
}

impl ReadWithCode {
    /// How often the refs its functions' reads return have been dropped, all told.
    fn drops(&self) -> u64 {
        self.tools.values().map(|tool| tool.dropped()).sum()
    }

    /// One call of a script's: the tool run as the kernel runs a call of the model's, its input checked first.
    async fn call(&self, name: &str, input: &str, signal: &Signal, reads: &mut Reads) -> Answer {
        let Some(tool) = self.tools.get(name) else { return Answer::error(format!("{name} isn't one of the script's functions.")) };
        reads.made(name, input);
        let Ok(Value::Object(input)) = serde_json::from_str::<Value>(input) else {
            return Answer::error(format!("{name} takes its tool's input, an object: {name}({{…}})."));
        };
        if let Some(problem) = schema::problem(&Value::Object(tool.input_schema()), &Value::Object(input.clone())) {
            return Answer::error(format!("{name}: {problem}"));
        }
        let answer = match CALLING.scope((), tool.execute(input, signal.clone())).await {
            Ok(result) => {
                // A name that hides text is kept here, as a call of the answer's own keeps it: the kernel sees only
                // what the script makes of it after (a key, a part of a longer line), which it can't put back.
                remember_text(&result.text);
                if result.is_error {
                    Answer::error(format!("{name}: {}", cut(&result.text, CALL_ERROR)))
                } else {
                    reads.last.insert(name.to_string(), result.text.clone());
                    Answer { json: serde_json::from_str::<serde::de::IgnoredAny>(&result.text).is_ok(), ok: true, text: result.text }
                }
            }
            Err(error) => Answer::error(format!("{name}: {}", cut(&error.to_string(), CALL_ERROR))),
        };
        // The refs the script holds in its variables are gone (Live drops a turn's when a read fails, or the Set
        // changes): whatever it read on with them would be refused, and a script that skips a failure would return
        // what it had as if that were all. It ends here.
        if reads.gone.is_none() && self.drops() != reads.dropped {
            reads.gone = Some(if answer.ok { String::new() } else { answer.text.clone() });
        }
        answer
    }

    /// The call's result: what the script returned, or what went wrong and what to do about it.
    fn result(&self, ended: Ended, reads: &Reads) -> ToolResult {
        let calls = reads.calls;
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
            How::Threw { error, line, misread, unread } => {
                let at = line.map(|line| format!(" near line {line}")).unwrap_or_default();
                let error = error.trim_end_matches('.');
                let unread = match unread {
                    0 => "",
                    _ => " That was in work the script started and didn't wait for, so its answer may be short of it: `await` it, or call the functions plainly (they need no await).",
                };
                let deep = if error.contains("Maximum call stack size exceeded") {
                    " Something nests too deep: a function that calls itself without end, or a value nested thousands deep."
                } else {
                    ""
                };
                let fields = if misread { outlines(&reads.last) } else { String::new() };
                ToolResult::error(format!("The script stopped{at}: {error}.{unread}{deep} It had made {made}.{fields}{printed}"))
            }
            How::Waits => ToolResult::error(format!(
                "The script waits for something that never comes ({made} made): its functions answer at once, so call them without promises or timers.{printed}"
            )),
            How::Stopped(Why::Calls) => {
                let why = match &reads.most {
                    Some((name, times)) if *times >= AGAIN => format!(
                        "it made the same {name} call {times} times, so a loop isn't moving on. A list's last page has no nextCursor: follow a cursor only while there is one (`while (cursor)`), and read each thing once."
                    ),
                    _ => "read in bigger pages, or narrow what it reads.".to_string(),
                };
                ToolResult::error(format!("The script was stopped at {MOST_CALLS} calls, the most one makes: {why}{printed}"))
            }
            How::Stopped(Why::Steps) => ToolResult::error(format!(
                "The script was stopped: it ran too long without ending ({made} made). Check that each loop ends, that a cursor is followed only while there is one, and that no regular expression repeats a repeat (`(\\w+\\s?)+`), which can take forever on a long name.{printed}"
            )),
            How::Stopped(Why::Memory) => ToolResult::error(format!(
                "The script was stopped: it held more than {} MB ({made} made). Keep less of what each read returns (the few fields the answer needs, a count), not whole results.{printed}",
                MEMORY / (1024 * 1024)
            )),
            How::Stopped(Why::Gone) => {
                let (app, place) = (self.app, self.place);
                ToolResult::error(match reads.gone.as_deref().unwrap_or("") {
                    "" => format!(
                        "The script was stopped: {app} dropped the refs and cursors this turn had read while it ran (the {place} changed, or {app} went away), so the script couldn't go on with those it held ({made} made). Run it again.{printed}"
                    ),
                    failed => format!(
                        "The script was stopped: {}, and a read that fails in {app} drops the refs and cursors this turn has read, so the script couldn't go on with those it held ({made} made). Read again from discovery, and have the script leave out what a read can't take rather than catch its failure.{printed}",
                        failed.trim_end_matches('.')
                    ),
                })
            }
            How::Stopped(_) => ToolResult::error("The script was stopped."),
        }
    }
}

/// What the kernel's side keeps of a script's calls.
#[derive(Default)]
struct Reads {
    /// The calls it made (a stopped script's later tries aren't calls).
    calls: usize,
    /// What each function last returned: a script that read a field a result doesn't have is shown the fields it has.
    last: IndexMap<String, String>,
    /// How often each call was made (by its function and input), and the one made most: a loop that isn't moving on
    /// makes the same one again and again.
    times: IndexMap<u64, usize>,
    most: Option<(String, usize)>,
    /// How often the tools' refs had been dropped when the script began (`KernelTool::dropped`).
    dropped: u64,
    /// The refs the script had read were dropped: the failed call's words, or none when no call of its own failed.
    gone: Option<String>,
}

impl Reads {
    /// A call was made: counted with those like it.
    fn made(&mut self, name: &str, input: &str) {
        self.calls += 1;
        let mut hasher = DefaultHasher::new();
        (name, input).hash(&mut hasher);
        let times = self.times.entry(hasher.finish()).or_insert(0);
        *times += 1;
        if self.most.as_ref().is_none_or(|(_, most)| *times > *most) {
            self.most = Some((name.to_string(), *times));
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
    /// and each `throw`, so an error between two is given the earlier one's. `misread`: it took a value for what it
    /// wasn't (a field a result doesn't have, a name that isn't there). `unread`: how many failures it left in work
    /// it started and never waited for (this is the first), when its own top level ended well.
    Threw { error: String, line: Option<u32>, misread: bool, unread: usize },
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
    Memory = 4,
    /// The refs it had read were dropped.
    Gone = 5,
}

/// What both threads know of a script as it runs.
#[derive(Default)]
struct Flags {
    stopped: AtomicU8,
    calls: AtomicUsize,
    checks: AtomicU32,
    /// How often the engine has asked since the script was stopped.
    since: AtomicU32,
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
            4 => Why::Memory,
            5 => Why::Gone,
            _ => Why::No,
        }
    }
    fn stopped(&self) -> bool {
        self.why() != Why::No
    }
    /// The engine asks whether to stop: once stopped, at every asking, so nothing the script catches lets it go on.
    fn check(&self) -> bool {
        if self.checks.fetch_add(1, Ordering::Relaxed) >= MOST_CHECKS {
            self.stop(Why::Steps);
        }
        self.stopped()
    }
    /// Whether a stopped script has been asked a script's worth of times since and still runs: its stops don't end it.
    fn passed(&self) -> bool {
        self.since.fetch_add(1, Ordering::Relaxed) >= MOST_CHECKS
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
    /// How long `text` is, as `cut` counts.
    units: usize,
    cut: bool,
}

impl Printed {
    /// Keeps a line, or as much of it as there's room for; false once no more is kept. The room, the cut and the
    /// count are all in `cut`'s units, the line's own newline among them.
    fn add(&mut self, line: &str) -> bool {
        if self.cut {
            return false;
        }
        let newline = usize::from(!self.text.is_empty());
        let Some(room) = PRINTED.checked_sub(self.units + newline) else {
            self.cut = true;
            return false;
        };
        let kept = cut(line, room);
        self.cut = kept.len() < line.len();
        if self.cut && kept.is_empty() {
            return false;
        }
        if newline == 1 {
            self.text.push('\n');
        }
        self.text.push_str(kept);
        self.units += newline + kept.encode_utf16().count();
        !self.cut
    }
    fn said(&self) -> String {
        if self.cut {
            format!("{}\n…", self.text)
        } else {
            self.text.clone()
        }
    }
}

/// Where a script that can't be ended is left. The engine has no way to end one from outside, and a thread can't be
/// taken away: so the kernel is told how it ended, and the thread never runs again. Its memory stays taken until Kumi
/// quits; only a script written to get past its own stop comes here.
struct Leave {
    asks: mpsc::UnboundedSender<Ask>,
    flags: Arc<Flags>,
    printed: Rc<RefCell<Printed>>,
}

impl Leave {
    fn now(&self) -> ! {
        let printed = self.printed.try_borrow().map(|printed| printed.said()).unwrap_or_default();
        let _ = self.asks.send(Ask::Ended(Ended { how: How::Stopped(self.flags.why()), printed }));
        loop {
            std::thread::park();
        }
    }
}

/// The engine's memory, counted as it's handed out (the engine's own limit refuses every piece at once, small ones
/// too, which this engine doesn't always survive: see `BIG`).
struct Heap {
    inner: RustAllocator,
    held: usize,
    flags: Arc<Flags>,
    leave: Rc<Leave>,
}

impl Heap {
    /// Whether the engine may have `more` bytes, in a piece of `piece`.
    fn gives(&mut self, more: usize, piece: usize) -> bool {
        let held = self.held.saturating_add(more);
        if held <= MEMORY {
            return true;
        }
        self.flags.stop(Why::Memory);
        if held > MEMORY_END {
            self.leave.now();
        }
        piece < BIG
    }
}

// SAFETY: every piece comes from, and goes back to, `RustAllocator`, whose own contract this passes through: a null
// or a piece of at least the size asked, aligned as it aligns, and `usable_size` as it says.
unsafe impl Allocator for Heap {
    fn alloc(&mut self, size: usize) -> *mut u8 {
        if !self.gives(size, size) {
            return std::ptr::null_mut();
        }
        let piece = self.inner.alloc(size);
        if !piece.is_null() {
            // SAFETY: `piece` was just given by `RustAllocator`.
            self.held += unsafe { RustAllocator::usable_size(piece) };
        }
        piece
    }
    fn calloc(&mut self, count: usize, size: usize) -> *mut u8 {
        let Some(all) = count.checked_mul(size) else { return std::ptr::null_mut() };
        if !self.gives(all, all) {
            return std::ptr::null_mut();
        }
        let piece = self.inner.calloc(count, size);
        if !piece.is_null() {
            // SAFETY: `piece` was just given by `RustAllocator`.
            self.held += unsafe { RustAllocator::usable_size(piece) };
        }
        piece
    }
    unsafe fn dealloc(&mut self, piece: *mut u8) {
        if piece.is_null() {
            return;
        }
        // SAFETY: the engine gives back only what this gave it (the trait's contract).
        unsafe {
            self.held = self.held.saturating_sub(RustAllocator::usable_size(piece));
            self.inner.dealloc(piece);
        }
    }
    unsafe fn realloc(&mut self, piece: *mut u8, size: usize) -> *mut u8 {
        if piece.is_null() {
            return self.alloc(size);
        }
        if size == 0 {
            // SAFETY: as in `dealloc`.
            unsafe { self.dealloc(piece) };
            return std::ptr::null_mut();
        }
        // SAFETY: `piece` is one this gave (the trait's contract); a refused or failed resize leaves it as it was.
        unsafe {
            let had = RustAllocator::usable_size(piece);
            if size > had && !self.gives(size - had, size) {
                return std::ptr::null_mut();
            }
            let resized = self.inner.realloc(piece, size);
            if !resized.is_null() {
                self.held = self.held.saturating_sub(had) + RustAllocator::usable_size(resized);
            }
            resized
        }
    }
    unsafe fn usable_size(piece: *mut u8) -> usize {
        // SAFETY: the caller's contract is `RustAllocator`'s own.
        unsafe { RustAllocator::usable_size(piece) }
    }
}

/// A failure the script left unread: a promise that was rejected with nothing waiting on it (an async function it
/// called and didn't await). Kept with its promise, which a handler added later takes off again.
struct Unread {
    promise: Persistent<JsValue<'static>>,
    error: String,
    line: Option<u32>,
    misread: bool,
}

/// The failures left unread, and how many more there were than are kept.
#[derive(Default)]
struct Unreads {
    kept: Vec<Unread>,
    more: usize,
}

/// The script, run to its end on this thread.
fn run(code: &str, names: &[String], asks: &mpsc::UnboundedSender<Ask>, flags: &Arc<Flags>) -> Ended {
    let printed = Rc::new(RefCell::new(Printed::default()));
    let leave = Rc::new(Leave { asks: asks.clone(), flags: flags.clone(), printed: printed.clone() });
    let how = (|| -> rquickjs::Result<How> {
        let runtime = Runtime::new_with_alloc(Heap { inner: RustAllocator, held: 0, flags: flags.clone(), leave: leave.clone() })?;
        runtime.set_max_stack_size(STACK);
        runtime.set_interrupt_handler(Some(Box::new({
            let (flags, leave) = (flags.clone(), leave.clone());
            move || {
                if !flags.check() {
                    return false;
                }
                if flags.passed() {
                    leave.now();
                }
                true
            }
        })));
        let unreads = Rc::new(RefCell::new(Unreads::default()));
        runtime.set_host_promise_rejection_tracker(Some(Box::new({
            let unreads = unreads.clone();
            move |ctx, promise, reason, handled| {
                let promise = Persistent::save(&ctx, promise);
                let mut unreads = unreads.borrow_mut();
                if handled {
                    // Something waits on it after all: it isn't unread. (One past those kept is only counted.)
                    match unreads.kept.iter().position(|unread| unread.promise == promise) {
                        Some(at) => drop(unreads.kept.remove(at)),
                        None => unreads.more = unreads.more.saturating_sub(1),
                    }
                } else if unreads.kept.len() < MOST_UNREAD {
                    let (error, line, misread) = said(&ctx, reason);
                    unreads.kept.push(Unread { promise, error, line, misread });
                } else {
                    unreads.more += 1;
                }
            }
        })));
        let context = Context::full(&runtime)?;
        let how = context.with(|ctx| {
            let how = within(&ctx, code, names, asks, flags, &printed, &unreads);
            // The promises kept go back before the engine does: it can't be freed while one is held.
            unreads.borrow_mut().kept.clear();
            how
        });
        runtime.set_host_promise_rejection_tracker(None);
        how
    })()
    .unwrap_or_else(|error| How::Threw { error: error.to_string(), line: None, misread: false, unread: 0 });
    // Stopped is stopped, whatever the script made of it.
    let how = match flags.why() {
        Why::No => how,
        why => How::Stopped(why),
    };
    let printed = printed.borrow().said();
    Ended { how, printed }
}

/// What a script's own glue is, in the script's language: print (and console's), which shows a value as its author
/// reads it (text and numbers as they are, anything else as JSON), and how a value is written as JSON for the result.
/// Both give what JSON has no form for a form: an error its words, a Map its entries, a Set its members; a promise
/// nobody waited for is said, not sent as `{}`. It's written here and not on this side because the engine can stop
/// its own code anywhere and `try` doesn't catch a stop, where glue on this side that tries one way and then another
/// would have to tell a stop from a failure each time (and once took one for the other).
const GLUE: &str = r#"(function (say, most) {
  const {Map, Set, Error, Promise, RegExp, String} = globalThis;
  const {stringify} = JSON, {from} = Array, {fromEntries} = Object;
  const whole = String.prototype.toWellFormed ? (text) => text.toWellFormed() : (text) => text;
  const words = (error) => {
    const {name, message} = error;
    return !name || name === "Error" ? String(message) : name + ": " + message;
  };
  const plain = function (key, value) {
    if (value instanceof Map) return fromEntries(value);
    if (value instanceof Set) return from(value);
    if (value instanceof Error) return words(value);
    if (value instanceof Promise) throw new Error("the answer holds a promise that wasn't awaited: `await` it (`await Promise.all(…)` for a list of them), or call the functions plainly, since they need no await");
    return value;
  };
  const written = (value) => stringify(value, plain);
  const shown = (value) => {
    if (typeof value === "string") return value;
    if (typeof value !== "object" || value === null || value instanceof RegExp) return String(value);
    if (value instanceof Error) return words(value);
    try {
      const text = written(value);
      if (text !== undefined) return text;
    } catch (error) {}
    try {
      return String(value);
    } catch (error) {
      return "[a value that can't be printed]";
    }
  };
  let open = true;
  const print = (...values) => {
    if (open) open = say(whole(values.map(shown).join(" ").slice(0, most + 2)));
  };
  return {print, written};
})"#;

fn within<'js>(
    ctx: &Ctx<'js>,
    code: &str,
    names: &[String],
    asks: &mpsc::UnboundedSender<Ask>,
    flags: &Arc<Flags>,
    printed: &Rc<RefCell<Printed>>,
    unreads: &Rc<RefCell<Unreads>>,
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
            if flags.stopped() {
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
            // (The kernel's side stops it with the answer it sends, when the refs it had read are gone.)
            if flags.stopped() {
                return throw("stopped");
            }
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
    let say = Function::new(ctx.clone(), {
        let printed = printed.clone();
        move |line: String| printed.borrow_mut().add(&line)
    })?;
    let glue: Function<'js> = ctx.eval(GLUE)?;
    let glue: Object<'js> = glue.call((say, PRINTED as i32))?;
    let (print, written): (Function<'js>, Function<'js>) = (glue.get("print")?, glue.get("written")?);
    globals.set("print", print.clone())?;
    let console = Object::new(ctx.clone())?;
    for level in ["log", "info", "warn", "error", "debug"] {
        console.set(level, print.clone())?;
    }
    globals.set("console", console)?;
    // The body starts on the wrapper's own line, so an error's line is the script's.
    let promise: Promise<'js> = match ctx.eval(format!("(async function(){{{code}\n}})()")) {
        Ok(promise) => promise,
        Err(error) => return Ok(thrown(ctx, error)),
    };
    // The script's queued work, a job at a time: what it awaits, and then, its top level over, what it started and
    // never waited for (a forEach(async …)), so its answer holds what that added, as it would under any other host. A
    // stop ends it between jobs: a job that fails is still a job run, and would otherwise start the next.
    let mut state = promise.state();
    while state == PromiseState::Pending && !flags.stopped() && ctx.execute_pending_job() {
        state = promise.state();
    }
    if state == PromiseState::Resolved {
        while !flags.stopped() && ctx.execute_pending_job() {}
    }
    if flags.stopped() {
        return Ok(How::Stopped(flags.why()));
    }
    let value = match (state, promise.result::<JsValue<'js>>()) {
        (PromiseState::Pending, _) | (_, None) => return Ok(How::Waits),
        (_, Some(Err(error))) => return Ok(thrown(ctx, error)),
        (_, Some(Ok(value))) => value,
    };
    // A failure nothing waited on, in work the script started: its answer may be short of what that would have added.
    {
        let unreads = unreads.borrow();
        if let Some(first) = unreads.kept.first() {
            let unread = unreads.kept.len() + unreads.more;
            return Ok(How::Threw { error: first.error.clone(), line: first.line, misread: first.misread, unread });
        }
    }
    if value.is_undefined() {
        return Ok(How::Returned(None));
    }
    Ok(match written.call::<_, Option<rquickjs::String<'js>>>((value,)) {
        Ok(Some(text)) => How::Returned(Some(text.to_string()?)),
        Ok(None) => How::Returned(None),
        Err(error) => thrown(ctx, error),
    })
}

/// What a script threw, in its own words, with the line it was near.
fn thrown<'js>(ctx: &Ctx<'js>, error: JsError) -> How {
    let JsError::Exception = error else { return How::Threw { error: error.to_string(), line: None, misread: false, unread: 0 } };
    let (error, line, misread) = said(ctx, ctx.catch());
    How::Threw { error, line, misread, unread: 0 }
}

/// What a thrown value says: its words, the script's line it was near, and whether it's a value taken for what it
/// wasn't.
fn said<'js>(ctx: &Ctx<'js>, thrown: JsValue<'js>) -> (String, Option<u32>, bool) {
    let text = |value: JsValue<'js>| Coerced::<String>::from_js(ctx, value).map(|text| text.0).unwrap_or_default();
    let Some(object) = thrown.as_object().filter(|object| object.is_error()).cloned() else {
        return (text(thrown), None, false);
    };
    let field = |name: &str| object.get::<_, JsValue<'js>>(name).ok().filter(|value| !value.is_undefined()).map(&text).unwrap_or_default();
    let (kind, message, stack) = (field("name"), field("message"), field("stack"));
    let line = LINE.captures(&stack).and_then(|found| found[1].parse().ok());
    // A plain Error is a call's (the tool's own words, which name it) or the script's own `throw`: its kind adds nothing.
    let error = if kind.is_empty() || kind == "Error" { message } else { format!("{kind}: {message}") };
    (error, line, matches!(kind.as_str(), "TypeError" | "ReferenceError"))
}

/// A result's fields without their values, as a script's author needs them: `{items: [{ref, name}], nextCursor}`.
fn outline(value: &Value, depth: usize) -> String {
    match value {
        Value::Object(_) | Value::Array(_) if depth == 0 => "…".into(),
        Value::Object(fields) => {
            let mut shown: Vec<String> = fields
                .iter()
                .take(OUTLINE_FIELDS)
                .map(|(name, value)| match value {
                    Value::Object(_) | Value::Array(_) => format!("{name}: {}", outline(value, depth - 1)),
                    _ => name.clone(),
                })
                .collect();
            if fields.len() > OUTLINE_FIELDS {
                shown.push("…".into());
            }
            format!("{{{}}}", shown.join(", "))
        }
        Value::Array(items) => format!("[{}]", items.first().map(|first| outline(first, depth - 1)).unwrap_or_default()),
        Value::String(_) => "text".into(),
        Value::Number(_) => "number".into(),
        Value::Bool(_) => "true or false".into(),
        Value::Null => "null".into(),
    }
}

/// What the script's reads returned, by their fields: the last result of each function it called.
fn outlines(read: &IndexMap<String, String>) -> String {
    if read.is_empty() {
        return String::new();
    }
    let mut shown: Vec<String> = read
        .iter()
        .take(OUTLINE_TOOLS)
        .map(|(name, text)| {
            let fields = serde_json::from_str::<Value>(text).map_or("text".into(), |value| outline(&value, OUTLINE_DEPTH));
            let kept = cut(&fields, OUTLINE);
            let more = if kept.len() < fields.len() { "…" } else { "" };
            format!("{name}: {kept}{more}")
        })
        .collect();
    if read.len() > OUTLINE_TOOLS {
        shown.push("…".into());
    }
    format!(" Its reads returned {}.", shown.join("; "))
}
