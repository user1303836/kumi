//! `script` (CC-19): code the producer approved, run once in the DAW: Python in Live (its integration's `run_python`,
//! in `exec` mode), Lua in REAPER and Renoise (their `run_lua` kind, as one step of the DAW's plan tool).
//!
//! It isn't a `change` step: `change` keeps its typed steps and their allowlist (`DawProvider::plugin_steps`, which
//! refuses `run_lua`), while a script's approval is of its code, by its SHA-256. [`check`] runs in the preview the
//! host's approval shows and again at the dispatch, so what runs is what was approved, and nothing the checks refuse is
//! ever asked about. A run is one HISTORY row (family `script`) that Kumi never undoes: the DAW's own undo may.

use super::change::{offered_steps, one_line};
use super::connection;
use super::ctx::{Call, Ctx, Effect, JsonObject, OpError, OpFuture, Operation};
use super::history::{CallIdentity, ScriptRun};
use kumi_types::contracts::{KernelTool, ToolResult};
use kumi_types::daw::DawId;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::rc::Rc;

/// The most lines and bytes a script has (CC-19's r4 and r8): one cap, everywhere.
pub const MOST_LINES: usize = 400;
pub const MOST_BYTES: usize = 32_768;
/// The most characters of each field of what a run said (its result, what it printed, its error).
const OUTPUT: usize = 4_000;
/// The most characters of a script's purpose, as the preview and its HISTORY row say it.
const PURPOSE: usize = 200;

/// How long a run may take before its DAW stops it (CC-19's r9), in ms: Live's `python.run` (its tracer) and REAPER's
/// run_lua (its count hook) are given 10 s; Renoise's tool stops a chunk at 800 ms at most (its own count hook), which
/// is stricter and stays. Scripts run on the DAW's main thread: the bridge's own bound is what keeps one from hanging it.
fn deadline_ms(daw: DawId) -> u64 {
    match daw {
        DawId::Renoise => 800,
        _ => 10_000,
    }
}

/// The deadline in seconds, as the preview gives it and the stop's words say it: `10`, `0.8`.
fn deadline_seconds(daw: DawId) -> Value {
    let ms = deadline_ms(daw);
    if ms % 1000 == 0 {
        json!(ms / 1000)
    } else {
        json!(ms as f64 / 1000.0)
    }
}

/// What each bridge says when it stopped a script at its deadline: Live's `python.run`, REAPER's wrapper, Renoise's tool.
const STOPPED: &[&str] = &["Python exceeded timeoutMs", "ms (the chunk's time limit)", "Kumi stopped it: it ran longer than"];

pub static SCRIPT: Operation = Operation {
    name: "script",
    description: "Runs a script in the DAW, once, under its write lease: Python in Live, Lua in REAPER and Renoise, for \
                  what Kumi's change steps can't do. It runs with the DAW's access to the computer, and Kumi can't undo \
                  it (the DAW's own undo may). `preview` checks it and gives its SHA-256 without running anything; the \
                  run names that SHA-256 (expected_sha256) and the project key an observe gave.",
    schema: script_schema,
    effect: script_effect,
    run: run_script,
};

fn script_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["language", "purpose", "code", "expected_project_key"], "properties": {
        "language": {"type": "string", "enum": ["python", "lua"], "description": "Live runs Python; REAPER and Renoise run Lua"},
        "purpose": {"type": "string", "minLength": 1, "maxLength": PURPOSE, "pattern": "\\S", "description": "What the script does, in a few words"},
        "code": {"type": "string", "description": "The whole script: at most 400 lines and 32 KB"},
        "expected_project_key": {"type": "string", "minLength": 1, "maxLength": 64},
        "expected_sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$", "description": "The run's: the SHA-256 its preview gave"},
        "preview": {"type": "boolean", "description": "Check the script and give its SHA-256 without running it (the host's approval)"}
    }})
}

/// A preview reads; the run writes, as a `change` does.
fn script_effect(args: &JsonObject) -> Effect {
    if previewing(args) {
        Effect::Read
    } else {
        Effect::Write
    }
}

fn previewing(args: &JsonObject) -> bool {
    args.get("preview").and_then(Value::as_bool).unwrap_or(false)
}

fn run_script(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(script(ctx, call))
}

/// What [`check`] found of a script it lets through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The SHA-256 of the code's UTF-8 bytes, in lowercase hex.
    pub sha256: String,
    pub lines: usize,
    pub bytes: usize,
}

/// Why [`check`] refused a script: its code and the producer's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub code: &'static str,
    pub reason: String,
}

fn refuse(code: &'static str, reason: impl Into<String>) -> Refused {
    Refused { code, reason: reason.into() }
}

/// What a script may not name, in Lua and in Python, matched as whole names.
///
/// This is a safeguard, not the boundary: it's bypassable by design (`getattr(__import__("o" + "s"), …)`,
/// `load(string.char(…))`, a module that imports another). The producer's approval of the code is the boundary.
const LUA_DENIED: &[&str] = &["os.execute", "io.popen", "socket"];
const PYTHON_DENIED: &[&str] = &["subprocess", "os.system", "socket", "urllib", "ctypes"];

/// The script `code` in `language` for `daw`, checked as CC-19 says, in its order: something to run in the DAW's own
/// language, within the size cap, with nothing in it that shows otherwise than it runs, and naming nothing on the
/// denylist. Pure: the preview and the dispatch both run it.
pub fn check(code: &str, language: &str, daw: DawId) -> Result<Checked, Refused> {
    let takes = match daw {
        DawId::Ableton => "python",
        DawId::Reaper | DawId::Renoise => "lua",
        DawId::Logic | DawId::SoundCraft => return Err(refuse("unsupported", "Kumi runs scripts only in Live, REAPER and Renoise.")),
    };
    if code.trim().is_empty() {
        return Err(refuse("invalid", "The script has no code."));
    }
    if language != takes {
        let named = match language {
            "lua" => "Lua",
            "python" => "Python",
            other => other,
        };
        return Err(refuse("invalid", format!("Live runs Python scripts, and REAPER and Renoise run Lua: this one is {named}.")));
    }
    // Lines as `split('\n')` counts them, a single newline at the end not counted.
    let lines = code.split('\n').count() - usize::from(code.ends_with('\n'));
    let bytes = code.len();
    if lines > MOST_LINES || bytes > MOST_BYTES {
        return Err(refuse("too_long", "The script is over 400 lines or 32 KB, so Kumi won't run it: split it, or use a typed step."));
    }
    if let Some((hidden, line)) = first_hidden(code) {
        return Err(refuse(
            "hidden_characters",
            format!(
                "The script has a hidden or direction-changing character (U+{:04X} on line {line}), which can make code read differently than it runs, so Kumi won't run it.",
                u32::from(hidden)
            ),
        ));
    }
    let denied = if takes == "lua" { LUA_DENIED } else { PYTHON_DENIED };
    if let Some(entry) = denied.iter().filter_map(|entry| named_at(code, entry).map(|at| (at, *entry))).min().map(|(_, entry)| entry) {
        return Err(refuse(
            "not_allowed",
            format!("Kumi refuses scripts that use `{entry}`: it reaches outside the DAW. (This check is a safeguard, not a guarantee.)"),
        ));
    }
    Ok(Checked { sha256: sha256(code), lines, bytes })
}

/// The first character in `code` that shows otherwise than it runs, with its line (from 1): a bidi control, a
/// zero-width or invisible one, a line or paragraph separator, or a control character but tab, a newline and a
/// carriage return before one (a lone `\r` breaks a line in some readers and not others, so "line N" could name other
/// code in each).
fn first_hidden(code: &str) -> Option<(char, usize)> {
    let mut line = 1;
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        let shown = match c {
            '\n' | '\t' => true,
            '\r' => chars.peek() == Some(&'\n'),
            c if hides(c) => false,
            '\u{0000}'..='\u{001F}' | '\u{007F}'..='\u{009F}' => false,
            _ => true,
        };
        if !shown {
            return Some((c, line));
        }
        if c == '\n' {
            line += 1;
        }
    }
    None
}

/// r7's characters that hide or reorder text without being control characters: bidi controls, zero-width and invisible
/// ones, and the line and paragraph separators.
fn hides(c: char) -> bool {
    matches!(
        c,
        '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{061C}'
            | '\u{200B}'..='\u{200D}'
            | '\u{2060}'
            | '\u{FEFF}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// Where `code` first names `entry` as a whole name (or dotted name): never inside a longer one (`mysocket_name`,
/// `osystem`).
fn named_at(code: &str, entry: &str) -> Option<usize> {
    let name = |c: char| c.is_alphanumeric() || c == '_';
    code.match_indices(entry)
        .find(|(at, _)| {
            let before = code[..*at].chars().next_back();
            let after = code[at + entry.len()..].chars().next();
            !before.is_some_and(name) && !after.is_some_and(name)
        })
        .map(|(at, _)| at)
}

fn sha256(code: &str) -> String {
    Sha256::digest(code.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Refused before anything ran.
fn refused(code: &str, reason: impl Into<String>) -> Value {
    json!({"state": "refused", "code": code, "reason": reason.into()})
}

fn refusal(error: OpError) -> Value {
    refused(error.code(), error.reason())
}

/// What runs a script in this DAW: Live's `run_python`, or REAPER's and Renoise's plan tool with its `run_lua` step.
enum Runner {
    Python(Rc<dyn KernelTool>),
    Lua(Rc<dyn KernelTool>),
}

/// The DAW's runner, as the last look offered it.
fn runner(ctx: &Ctx) -> Result<Runner, Value> {
    let app = ctx.daw().profile().words.app;
    if ctx.daw() == DawId::Ableton {
        return ctx
            .link()
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "run_python")
            .map(Runner::Python)
            .ok_or_else(|| refused("unsupported", format!("Kumi's part in {app} doesn't run Python here: set Kumi up in {app} again.")));
    }
    ctx.link()
        .plan_tool()
        .filter(|plan| offered_steps(&plan.input_schema()).iter().any(|step| step == "run_lua"))
        .map(Runner::Lua)
        .ok_or_else(|| refused("unsupported", format!("Kumi's part in {app} doesn't run Lua here: set Kumi up in {app} again.")))
}

async fn script(ctx: &Ctx, call: Call) -> Result<Value, OpError> {
    let args = &call.args;
    let text = |name: &str| args.get(name).and_then(Value::as_str).unwrap_or_default();
    let (language, code) = (text("language"), text("code"));
    let checked = match check(code, language, ctx.daw()) {
        Ok(checked) => checked,
        Err(why) => return Ok(refused(why.code, why.reason)),
    };
    if let Err(error) = ctx.connection() {
        return Ok(refusal(error));
    }
    let project = match ctx.link().expect_project(text("expected_project_key")) {
        Ok(project) => project,
        Err(error) => return Ok(refusal(error)),
    };
    let runner = match runner(ctx) {
        Ok(runner) => runner,
        Err(refusal) => return Ok(refusal),
    };
    let purpose = one_line(text("purpose"), PURPOSE);
    if previewing(args) {
        return Ok(json!({
            "state": "preview",
            "daw": ctx.daw().as_str(),
            "language": language,
            "purpose": purpose,
            "sha256": checked.sha256,
            "lines": checked.lines,
            "bytes": checked.bytes,
            "deadline_seconds": deadline_seconds(ctx.daw()),
            "project": {"key": project.key, "epoch": project.epoch},
        }));
    }
    match args.get("expected_sha256").and_then(Value::as_str) {
        Some(approved) if approved == checked.sha256 => {}
        Some(_) => return Ok(refused("stale", "The script changed since it was approved.")),
        None => return Ok(refused("invalid", "A script runs only with the SHA-256 its preview gave (expected_sha256).")),
    }
    let run = ScriptRun { sha256: checked.sha256, language: language.to_string() };
    keep_code(ctx, &run, code)?;
    let title = if purpose.is_empty() { "Script".to_string() } else { format!("Script: {purpose}") };
    let asked = super::mcp::digest(SCRIPT.name, args);
    let identity = CallIdentity::of(&call.host, &asked);
    let history = ctx.history();
    history.begin_script(&title, &run);
    let app = ctx.daw().profile().words.app;
    // The bridge stops a run at the deadline (r9), so an accidental hang (a loop that never ends) can't hold the DAW.
    // It bounds accidents, not intent: in Live a script that calls `sys.settrace(None)` takes off python.run's tracer,
    // and Python can't be stubbed reliably against that; the approval of the code is the boundary for it. A single
    // long call into C isn't stopped either, nor is memory capped.
    let deadline = deadline_ms(ctx.daw());
    let ran = match &runner {
        Runner::Python(tool) => {
            let input: Map<String, Value> =
                [("code".to_string(), json!(code)), ("mode".to_string(), json!("exec")), ("timeoutMs".to_string(), json!(deadline))]
                    .into_iter()
                    .collect();
            python_ran(app, tool.execute(input, call.signal.clone()).await)
        }
        Runner::Lua(plan) => {
            let steps = json!([{"tool": "run_lua", "input": {"code": code, "timeoutMs": deadline}}]);
            let input: Map<String, Value> = [("steps".to_string(), steps)].into_iter().collect();
            lua_ran(app, plan.execute(input, call.signal.clone()).await)
        }
    }
    .stopped_at_deadline(ctx.daw());
    let (row_state, unsure) = match ran.state {
        "ran" => ("applied", None),
        "failed" => ("failed", None),
        _ => ("unsure", Some(ran.error.clone().and_then(|error| error.as_str().map(str::to_string)).unwrap_or_default())),
    };
    // Every script row says why Kumi can't undo it: a readout takes an applied row it can't undo, with no note, for one
    // from before Kumi restarted.
    let words = cant_undo(ctx.daw());
    let note = match (&unsure, ran.state) {
        (Some(reason), _) if !reason.is_empty() => format!("{} {words}", reason.trim_end()),
        // A stop at the deadline says so on the row, as in the answer.
        (_, "failed") if ran.error == Some(json!(stopped_words(ctx.daw()))) => format!("{} {words}", stopped_words(ctx.daw())),
        _ => words,
    };
    let row = history.end_script(identity, &project.key, row_state, note, ctx.snapshots());
    history.call_came_to(std::slice::from_ref(&row.id), ran.state, ctx.snapshots());
    if let Some(reason) = &unsure {
        ctx.link().note_unsure(SCRIPT.name, reason, "unsure");
    }
    // Whatever it did, the project may have changed: the next plan is made on a fresh look.
    ctx.link().advance();
    connection::publish_focus(&ctx.handle());
    let now = ctx.link().project().unwrap_or(project);
    super::core::publish_status(ctx, false).await;
    Ok(json!({
        "state": ran.state,
        "history_id": row.id,
        // As every write answers its rows: so an unsure run isn't audited a second time.
        "history_ids": [row.id],
        "output": ran.output(),
        "project": {"key": now.key, "epoch": now.epoch},
    }))
}

/// The code a run is about to run, kept beside HISTORY by its SHA-256 (`<state>/scripts/<sha256>.py|.lua`, the owner's
/// alone; the plugin may have written the same bytes there first, for its approval). Refused, with nothing run, when
/// it can't be kept. The folder is shared with the plugin (a pending approval's code, and other kinds of file), so the
/// only code taken away is a script's that no kept row names and that's older than [`STALE_CODE`].
fn keep_code(ctx: &Ctx, run: &ScriptRun, code: &str) -> Result<(), OpError> {
    let folder = ctx.paths().state_dir.join("scripts");
    let extension = if run.language == "python" { "py" } else { "lua" };
    let kept = super::ctx::make_private_dir(&folder).and_then(|()| {
        match super::inflight::write_synced(&folder.join(format!("{}.{extension}", run.sha256)), code.as_bytes()) {
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            other => other,
        }
    });
    if let Err(error) = kept {
        return Err(OpError::Failed {
            reason: format!("Kumi couldn't keep the script's code before running it, so it didn't run: {error}"),
        });
    }
    let named: Vec<String> =
        ctx.history().rows(super::history::KEPT).into_iter().filter_map(|row| row.script.map(|script| script.sha256)).collect();
    let now = std::time::SystemTime::now();
    for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if !matches!(path.extension().and_then(|extension| extension.to_str()), Some("py" | "lua")) {
            continue;
        }
        let sha = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
        if sha == run.sha256 || named.iter().any(|named| named == sha) {
            continue;
        }
        // A file whose age can't be read is kept: it may be another chat's pending approval.
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age > STALE_CODE));
        if old {
            if let Err(error) = std::fs::remove_file(&path) {
                super::say(&format!("kumi serve: an old script's code not removed: {error}"));
            }
        }
    }
    Ok(())
}

/// How old a script's code no kept row names must be before a run takes it away: a pending approval's is younger.
const STALE_CODE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// What came of a run: `ran`, `failed` (the script raised, or returned an error) or `unsure` (the DAW didn't say), and
/// what it said.
struct Ran {
    state: &'static str,
    result: Option<Value>,
    stdout: Option<Value>,
    error: Option<Value>,
}

impl Ran {
    fn failed(error: impl Into<Value>) -> Ran {
        Ran { state: "failed", result: None, stdout: None, error: Some(error.into()) }
    }

    fn unsure(reason: String) -> Ran {
        Ran { state: "unsure", result: None, stdout: None, error: Some(json!(reason)) }
    }

    /// A run its DAW's bridge stopped at the deadline (what [`STOPPED`] lists, in its error) is `failed`, with Kumi's
    /// words for it.
    fn stopped_at_deadline(mut self, daw: DawId) -> Ran {
        let said = self.error.as_ref().map(Value::to_string).unwrap_or_default();
        if self.state == "failed" && STOPPED.iter().any(|stop| said.contains(stop)) {
            self.error = Some(json!(stopped_words(daw)));
        }
        self
    }

    /// `{result?, stdout?, error?}`: each a text of at most [`OUTPUT`] characters, with no control character but tab and
    /// newline.
    fn output(&self) -> Value {
        let mut output = Map::new();
        for (name, value) in [("result", &self.result), ("stdout", &self.stdout), ("error", &self.error)] {
            if let Some(text) = value.as_ref().and_then(said) {
                output.insert(name.to_string(), json!(text));
            }
        }
        Value::Object(output)
    }
}

/// What a run stopped at its deadline says: "Kumi stopped the script after 10 seconds."
fn stopped_words(daw: DawId) -> String {
    format!("Kumi stopped the script after {} seconds.", deadline_seconds(daw))
}

/// A field of what a run said, as its output gives it: a text as it is, lines printed one to a line, any other value as
/// JSON; nothing when it's empty.
fn said(value: &Value) -> Option<String> {
    let text = match value {
        Value::Null => return None,
        Value::String(text) => text.clone(),
        Value::Array(lines) if lines.iter().all(Value::is_string) => lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
        other => other.to_string(),
    };
    // What a script said is shown to the model and the producer: nothing in it may hide text either (r7).
    let text: String = text.chars().filter(|c| !hides(*c) && (!c.is_control() || matches!(c, '\t' | '\n'))).collect();
    if text.is_empty() {
        return None;
    }
    Some(match text.char_indices().nth(OUTPUT - 1) {
        Some((at, _)) if text.chars().count() > OUTPUT => format!("{}…", &text[..at]),
        _ => text,
    })
}

/// Live's `run_python` answer (the bridge's `{ok, result, stdout, error}`): `ran` when it's ok, `failed` when the script
/// raised. An error of the bridge's own, or no answer, is unsure: a script it sent may have changed the Set.
fn python_ran(app: &str, ran: Result<ToolResult, RuntimeError>) -> Ran {
    let answer = match ran {
        Ok(answer) => answer,
        Err(error) => {
            return Ran::unsure(format!(
                "{app} didn't say how the script ended ({error}), so it may have changed the Set: look at it before another change."
            ))
        }
    };
    match serde_json::from_str::<Value>(&answer.text) {
        Ok(Value::Object(done)) if done.contains_key("ok") => {
            let ok = done.get("ok") == Some(&Value::Bool(true));
            Ran {
                state: if ok { "ran" } else { "failed" },
                result: done.get("result").cloned(),
                stdout: done.get("stdout").cloned(),
                error: done.get("error").filter(|error| !error.is_null()).map(python_error),
            }
        }
        _ if answer.is_error => Ran::unsure(answer.text),
        _ => Ran { state: "ran", result: Some(json!(answer.text)), stdout: None, error: None },
    }
}

/// Live's error (`{type, message, traceback}`) as one text: what it is first, so a cut output keeps it.
fn python_error(error: &Value) -> Value {
    let Value::Object(fields) = error else { return error.clone() };
    let message = fields.get("message").and_then(Value::as_str).unwrap_or_default();
    let head = match fields.get("type").and_then(Value::as_str) {
        Some(kind) => format!("{kind}: {message}"),
        None => message.to_string(),
    };
    match fields.get("traceback").and_then(Value::as_str).filter(|trace| !trace.is_empty()) {
        Some(trace) => json!(format!("{head}\n{trace}")),
        None => json!(head),
    }
}

/// The plan tool's answer to its one `run_lua` step: `ran` when the chunk ran to its end, `failed` when it stopped at
/// an error (what it changed before stays) or the DAW refused it before it ran, and `unsure` when the plan stopped (the
/// DAW didn't confirm the step) or didn't answer.
fn lua_ran(app: &str, ran: Result<ToolResult, RuntimeError>) -> Ran {
    let answer = match ran {
        Ok(answer) => answer,
        Err(error) => {
            return Ran::unsure(format!(
                "{app} didn't say how the script ended ({error}), so it may have changed the project: look at it before another change."
            ))
        }
    };
    let Ok(Value::Object(said)) = serde_json::from_str::<Value>(&answer.text) else {
        return match answer.is_error {
            true => Ran::failed(answer.text),
            false => Ran { state: "ran", result: Some(json!(answer.text)), stdout: None, error: None },
        };
    };
    if let Some(stopped) = said.get("stopped").filter(|stopped| !stopped.is_null()) {
        let reason = stopped.get("error").and_then(Value::as_str).unwrap_or("The DAW didn't confirm the script ran.");
        return Ran::unsure(reason.to_string());
    }
    let first = |name: &str| said.get(name).and_then(Value::as_array).and_then(|steps| steps.first()).cloned();
    if let Some(done) = first("done") {
        let failed = done.get("failed").filter(|failed| !failed.is_null()).cloned();
        // (Lua's JSON writes an empty table, nothing printed, as `{}`)
        let printed = done.get("printed").filter(|printed| !matches!(printed, Value::Object(empty) if empty.is_empty())).cloned();
        return Ran {
            state: if failed.is_some() { "failed" } else { "ran" },
            result: done.get("result").cloned(),
            stdout: printed,
            error: failed,
        };
    }
    if let Some(refused) = first("refused") {
        return Ran::failed(refused.get("error").cloned().unwrap_or_else(|| json!(format!("{app} refused the script."))));
    }
    Ran::unsure(format!("{app} didn't say whether the script ran: look at the project before another change."))
}

/// `undo` of a script's row (CC-19): Kumi can't, and says so in the preview and the undo alike, without asking the DAW.
pub fn not_undoable(ctx: &Ctx, previewing: bool) -> Value {
    let note = cant_undo(ctx.daw());
    if previewing {
        json!({"state": "preview", "undo_available": false, "reason": "not_undoable", "note": note})
    } else {
        json!({"state": "refused", "code": "not_undoable", "reason": note})
    }
}

/// Why Kumi can't undo a script in `daw`: the undo's words, and every script row's note.
fn cant_undo(daw: DawId) -> String {
    format!("Kumi can't undo a script: {}'s own Undo may, if its step is still the latest there.", daw.profile().words.app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::ctx::{Host, Progress};
    use crate::serve::Args;
    use async_trait::async_trait;
    use kumi_agent::core::contracts::{Integration, Observation};
    use kumi_common::abort::Signal;
    use kumi_types::contracts::{ChangeFamily, ChangeRecord, ChangeState};
    use kumi_types::system::Env;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::path::Path;
    use std::rc::Weak;

    fn refused_as(code: &str, language: &str, daw: DawId) -> Refused {
        check(code, language, daw).expect_err(code)
    }

    #[test]
    fn a_script_is_refused_for_each_thing_the_checks_find_and_in_their_order() {
        assert_eq!(refused_as("", "python", DawId::Ableton), refuse("invalid", "The script has no code."));
        assert_eq!(refused_as(" \n\t\r\n ", "lua", DawId::Reaper), refuse("invalid", "The script has no code."));
        for daw in [DawId::Logic, DawId::SoundCraft] {
            assert_eq!(refused_as("x = 1", "python", daw).code, "unsupported");
        }
        // Each class of hidden character, with the first one named and its line.
        for (hidden, class) in [
            ('\u{202E}', "bidi override"),
            ('\u{2066}', "bidi isolate"),
            ('\u{200E}', "left-to-right mark"),
            ('\u{061C}', "Arabic letter mark"),
            ('\u{200B}', "zero-width space"),
            ('\u{2060}', "word joiner"),
            ('\u{FEFF}', "byte-order mark"),
            ('\u{2028}', "line separator"),
            ('\u{2029}', "paragraph separator"),
            ('\u{0000}', "NUL"),
            ('\u{001B}', "escape"),
            ('\u{000B}', "vertical tab"),
            ('\u{007F}', "DEL"),
            ('\u{0085}', "NEL"),
            ('\u{009F}', "C1"),
        ] {
            let code = format!("x = 1\ny = 2\nok = \"a{hidden}b\"\n");
            assert_eq!(
                refused_as(&code, "python", DawId::Ableton),
                refuse(
                    "hidden_characters",
                    format!("The script has a hidden or direction-changing character (U+{:04X} on line 3), which can make code read differently than it runs, so Kumi won't run it.", u32::from(hidden))
                ),
                "{class}"
            );
        }
        // A precompiled chunk (Lua's bytecode starts with ESC, "\27Lua") is refused here, before Renoise's tool, whose
        // loadstring would take it, could see it.
        let bytecode = "\u{1b}LuaQ\u{0}\u{1}\u{4}\u{8}";
        assert_eq!(
            refused_as(bytecode, "lua", DawId::Renoise),
            refuse(
                "hidden_characters",
                "The script has a hidden or direction-changing character (U+001B on line 1), which can make code read differently than it runs, so Kumi won't run it."
            )
        );
        let lone = refused_as("local a = 1\rlocal b = 2", "lua", DawId::Renoise);
        assert_eq!((lone.code, lone.reason.contains("U+000D on line 1")), ("hidden_characters", true), "a lone carriage return");
        assert!(check("local a = 1\r\n\tlocal b = 2\r\n", "lua", DawId::Renoise).is_ok(), "tabs and CRLF line ends are fine");
        // The size cap comes before the characters, the characters before the denylist.
        let long = format!("{}\u{202E}import socket", "x = 1\n".repeat(MOST_LINES));
        assert_eq!(refused_as(&long, "python", DawId::Ableton).code, "too_long");
        assert_eq!(refused_as("import socket\u{200B}", "python", DawId::Ableton).code, "hidden_characters");
    }

    #[test]
    fn each_denylist_entry_is_refused_by_name_and_a_longer_name_that_holds_one_isnt() {
        let reason = |entry: &str| {
            refuse(
                "not_allowed",
                format!(
                    "Kumi refuses scripts that use `{entry}`: it reaches outside the DAW. (This check is a safeguard, not a guarantee.)"
                ),
            )
        };
        for (code, entry) in [
            ("os.execute('rm -rf ~')", "os.execute"),
            ("local f = io.popen('ls')", "io.popen"),
            ("local s = require(\"socket\")", "socket"),
            ("local http = require 'socket.http'", "socket"),
        ] {
            assert_eq!(refused_as(code, "lua", DawId::Reaper), reason(entry), "{code}");
            assert_eq!(refused_as(code, "lua", DawId::Renoise), reason(entry), "{code}");
        }
        for (code, entry) in [
            ("import subprocess\nsubprocess.run(['ls'])", "subprocess"),
            ("import os\nos.system('ls')", "os.system"),
            ("import socket", "socket"),
            ("from urllib.request import urlopen", "urllib"),
            ("import ctypes", "ctypes"),
            // Several: the first one in the code is named.
            ("import ctypes, subprocess", "ctypes"),
        ] {
            assert_eq!(refused_as(code, "python", DawId::Ableton), reason(entry), "{code}");
        }
        // Inside a longer name, it's another name.
        for near in
            ["osystem = 1", "mysocket_name = 'a'", "sockets = []", "not_subprocess = 0", "urllib3_like = None", "pos.system_name = 1"]
        {
            assert!(check(near, "python", DawId::Ableton).is_ok(), "{near}");
        }
        for near in ["myos.execute = 1", "local mysocket_name = 1", "local io_popen = 2", "t.io.popen_safe = 1"] {
            assert!(check(near, "lua", DawId::Reaper).is_ok(), "{near}");
        }
    }

    #[test]
    fn the_caps_are_400_lines_and_32768_bytes_and_one_more_is_refused() {
        let too_long = refuse("too_long", "The script is over 400 lines or 32 KB, so Kumi won't run it: split it, or use a typed step.");
        let lines = |n: usize| (0..n).map(|line| format!("x{line} = {line}")).collect::<Vec<_>>().join("\n");
        assert_eq!(check(&lines(MOST_LINES), "python", DawId::Ableton).unwrap().lines, 400);
        let ended = format!("{}\n", lines(MOST_LINES));
        assert_eq!(check(&ended, "python", DawId::Ableton).unwrap().lines, 400, "one newline at the end isn't a line");
        assert_eq!(refused_as(&format!("{ended}\n"), "python", DawId::Ableton), too_long, "a second one is");
        assert_eq!(refused_as(&lines(MOST_LINES + 1), "python", DawId::Ableton), too_long);
        let full = format!("-- {}", "a".repeat(MOST_BYTES - 3));
        assert_eq!(check(&full, "lua", DawId::Renoise).unwrap().bytes, 32_768);
        assert_eq!(refused_as(&format!("{full}a"), "lua", DawId::Renoise), too_long);
        // Bytes, not characters: 16,385 two-byte characters are over.
        assert_eq!(refused_as(&format!("-- {}", "é".repeat(16_383)), "lua", DawId::Renoise), too_long);
    }

    #[test]
    fn a_script_in_the_other_language_is_refused_for_the_daw() {
        let reason =
            |named: &str| refuse("invalid", format!("Live runs Python scripts, and REAPER and Renoise run Lua: this one is {named}."));
        assert_eq!(refused_as("print('hi')", "lua", DawId::Ableton), reason("Lua"));
        assert_eq!(refused_as("print('hi')", "python", DawId::Reaper), reason("Python"));
        assert_eq!(refused_as("print('hi')", "python", DawId::Renoise), reason("Python"));
        assert!(check("print('hi')", "python", DawId::Ableton).is_ok());
        assert!(check("print('hi')", "lua", DawId::Reaper).is_ok());
    }

    #[test]
    fn what_a_run_says_is_capped_and_has_no_control_characters_but_tab_and_newline() {
        let ran = Ran {
            state: "ran",
            result: Some(json!({"tracks": 3})),
            stdout: Some(json!(["one\tA", "two\u{1b}[31m", "x".repeat(5_000)])),
            error: Some(Value::Null),
        };
        let output = ran.output();
        assert_eq!(output["result"], "{\"tracks\":3}");
        let stdout = output["stdout"].as_str().unwrap();
        assert!(stdout.starts_with("one\tA\ntwo[31m\nxxx"), "{stdout}");
        assert_eq!(stdout.chars().count(), OUTPUT);
        assert!(stdout.ends_with('…'));
        assert!(output.get("error").is_none(), "nothing said, no field");
        // Nothing that hides or reorders text either (r7's classes): what a script said reads as it is.
        let hidden = Ran {
            state: "ran",
            result: Some(json!("ok\u{202E}gnp.exe\u{2066}\u{200B}\u{2060}\u{FEFF}\u{061C}")),
            stdout: Some(json!("line one\u{2028}line two\u{2029}\u{200E}end")),
            error: None,
        };
        assert_eq!(hidden.output(), json!({"result": "okgnp.exe", "stdout": "line oneline twoend"}));
        let live =
            python_error(&json!({"type": "NameError", "message": "name 'x' is not defined", "traceback": "Traceback…\nNameError: …"}));
        assert_eq!(live, json!("NameError: name 'x' is not defined\nTraceback…\nNameError: …"));
    }

    /// A DAW that's there: nothing it's asked but the tools its look offers.
    struct Answering;

    #[async_trait(?Send)]
    impl Integration for Answering {
        async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
            Ok(())
        }
        async fn observe(&self, _: Signal, _: Option<kumi_agent::core::contracts::ObserveHints>) -> Result<Observation, RuntimeError> {
            unreachable!("the tests give the link their look")
        }
        async fn close(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    /// Live's `run_python`, answering as the bridge does, or not at all; with what it was given.
    struct Python {
        answer: Option<Value>,
        given: Rc<RefCell<Vec<Value>>>,
    }

    #[async_trait(?Send)]
    impl KernelTool for Python {
        fn name(&self) -> &str {
            "run_python"
        }
        fn description(&self) -> &str {
            "Run Python inside Live."
        }
        fn input_schema(&self) -> JsonObject {
            json!({"type": "object", "required": ["code"], "properties": {"code": {"type": "string"}, "mode": {"type": "string"}}})
                .as_object()
                .unwrap()
                .clone()
        }
        async fn execute(&self, input: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
            self.given.borrow_mut().push(Value::Object(input));
            match &self.answer {
                Some(done) => Ok(ToolResult { text: done.to_string(), is_error: done["ok"] != true, ..Default::default() }),
                None => Err(RuntimeError::plain("the bridge went away")),
            }
        }
    }

    /// REAPER's plan tool with its `run_lua` step: it keeps Kumi's change record for the chunk, as REAPER's executor
    /// does (through the runtime's `on_change`), and answers its step.
    struct Plan {
        ctx: Weak<Ctx>,
        given: Rc<RefCell<Vec<Value>>>,
    }

    #[async_trait(?Send)]
    impl KernelTool for Plan {
        fn name(&self) -> &str {
            "make_changes"
        }
        fn description(&self) -> &str {
            "Make changes in one call."
        }
        fn input_schema(&self) -> JsonObject {
            json!({"type": "object", "required": ["steps"], "properties": {"steps": {"type": "array", "items": {
                "type": "object", "properties": {"tool": {"type": "string", "enum": ["run_lua", "track_volume"]}, "input": {"type": "object"}}
            }}}})
            .as_object()
            .unwrap()
            .clone()
        }
        async fn execute(&self, input: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
            self.given.borrow_mut().push(Value::Object(input));
            let ctx = self.ctx.upgrade().unwrap();
            let key = ctx.link().project().unwrap().key;
            ctx.history().record(&lua_record(ChangeState::Applied), &key, ctx.snapshots());
            let done = json!({"done": [{"step": 1, "changed": "Ran Lua", "change": "c7", "result": 3, "printed": ["made 3 tracks"]}]});
            Ok(ToolResult::text(done.to_string()))
        }
    }

    /// Kumi's change record for the chunk REAPER ran, as its executor keeps it.
    fn lua_record(state: ChangeState) -> ChangeRecord {
        ChangeRecord {
            id: "c7".into(),
            family: ChangeFamily::Structure,
            title: "Ran Lua".into(),
            track: None,
            from: None,
            to: None,
            range: None,
            colors: None,
            clip: None,
            devices: None,
            state,
            score: None,
            note: None,
            at: 1_700_000_000_000,
            picture: None,
            batch: None,
        }
    }

    fn runtime(state: &Path, daw: DawId) -> Rc<Ctx> {
        let args = Args {
            state_dir: state.to_path_buf(),
            kumi_home: state.join("home"),
            hermes_home: None,
            hermes_root: None,
            daw,
            library_roots: vec![],
            media_roots: vec![],
            tools: BTreeMap::new(),
        };
        Ctx::new(args, Env::new(), Default::default()).unwrap()
    }

    /// The runtime connected, with a look that offered `tool`: the project's key.
    fn looked(ctx: &Ctx, tool: Rc<dyn KernelTool>) -> String {
        let observation = Observation {
            key: "reaper:0123456789abcdef".into(),
            revision: None,
            label: "Night Drive".into(),
            context: String::new(),
            instructions: String::new(),
            tools: vec![tool],
            project: None,
            set: Some("4242:1".into()),
            tracks: None,
            saved_at: None,
            identity: None,
            native_epoch: None,
        };
        ctx.link().connected_for_test(Rc::new(Answering), &observation).key
    }

    async fn call(ctx: &Ctx, operation: &Operation, args: Value) -> Value {
        let Value::Object(args) = args else { unreachable!() };
        let host = Host { session_id: Some("chat-1".into()), tool_call_id: Some("call-1".into()), ..Default::default() };
        (operation.run)(ctx, Call { args, host, signal: Signal::new(), progress: Progress::none() }).await.unwrap()
    }

    async fn local<F: Future<Output = ()>>(test: F) {
        tokio::task::LocalSet::new().run_until(test).await;
    }

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[tokio::test]
    async fn a_preview_says_what_would_run_by_its_sha256_and_runs_nothing() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path(), DawId::Ableton);
            let given = Rc::new(RefCell::new(Vec::new()));
            let args = |key: &str| json!({"language": "python", "purpose": "Name\u{7}\nthe drums", "code": "abc", "expected_project_key": key, "preview": true});
            assert_eq!(call(&ctx, &SCRIPT, args("p-0")).await["code"], "not_connected");
            let key = looked(&ctx, Rc::new(Python { answer: Some(json!({"ok": true})), given: given.clone() }));
            assert_eq!(call(&ctx, &SCRIPT, args("p-0000000000000000")).await["code"], "stale", "another project's");
            let preview = call(&ctx, &SCRIPT, args(&key)).await;
            let epoch = ctx.link().epoch();
            assert_eq!(
                preview,
                json!({"state": "preview", "daw": "ableton", "language": "python", "purpose": "Name the drums", "sha256": ABC,
                       "lines": 1, "bytes": 3, "deadline_seconds": 10, "project": {"key": key, "epoch": epoch}})
            );
            assert!(given.borrow().is_empty(), "nothing ran");
            assert!(ctx.history().rows(10).is_empty(), "nothing in HISTORY");
            assert_eq!(script_effect(args(&key).as_object().unwrap()), Effect::Read);
            assert_eq!(script_effect(&JsonObject::new()), Effect::Write, "the run writes");
        })
        .await;
    }

    #[tokio::test]
    async fn a_run_of_other_code_than_approved_is_stale_and_runs_nothing() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path(), DawId::Ableton);
            let given = Rc::new(RefCell::new(Vec::new()));
            let key = looked(&ctx, Rc::new(Python { answer: Some(json!({"ok": true})), given: given.clone() }));
            let run = |sha: Option<&str>| {
                let mut args = json!({"language": "python", "purpose": "Name the drums", "code": "abc", "expected_project_key": key});
                if let Some(sha) = sha {
                    args["expected_sha256"] = json!(sha);
                }
                args
            };
            let other = "0".repeat(64);
            assert_eq!(
                call(&ctx, &SCRIPT, run(Some(&other))).await,
                json!({"state": "refused", "code": "stale", "reason": "The script changed since it was approved."})
            );
            assert_eq!(call(&ctx, &SCRIPT, run(None)).await["code"], "invalid");
            // The checks hold at the dispatch too.
            let mut hidden = run(Some(ABC));
            hidden["code"] = json!("abc\u{202E}");
            assert_eq!(call(&ctx, &SCRIPT, hidden).await["code"], "hidden_characters");
            assert!(given.borrow().is_empty() && ctx.history().rows(10).is_empty(), "nothing ran");
        })
        .await;
    }

    #[tokio::test]
    async fn a_live_run_is_one_script_row_kumi_never_undoes_with_its_code_kept() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path(), DawId::Ableton);
            let given = Rc::new(RefCell::new(Vec::new()));
            let done = json!({"ok": true, "result": {"named": 4}, "stdout": "renamed 4 tracks\n", "error": null});
            let key = looked(&ctx, Rc::new(Python { answer: Some(done), given: given.clone() }));
            let before = ctx.link().epoch();
            // The folder is the plugin's too: another chat's pending code, and files of other kinds, old or not.
            let scripts = ctx.paths().state_dir.join("scripts");
            std::fs::create_dir_all(&scripts).unwrap();
            let two_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 60 * 60);
            let put = |name: &str, old: bool| {
                let file = std::fs::File::create(scripts.join(name)).unwrap();
                if old {
                    file.set_modified(two_days_ago).unwrap();
                }
            };
            let pending = format!("{}.py", "1".repeat(64));
            put(&pending, false);
            put(&format!("{}.py", "2".repeat(64)), true);
            put(&format!("{}.lua", "3".repeat(64)), true);
            put(&format!("{}.device.json", "4".repeat(64)), true);
            put(&format!("{}.genexpr", "5".repeat(64)), true);
            let args = json!({"language": "python", "purpose": "Name the drums", "code": "abc", "expected_project_key": key, "expected_sha256": ABC});
            let ran = call(&ctx, &SCRIPT, args).await;
            assert_eq!(*given.borrow(), [json!({"code": "abc", "mode": "exec", "timeoutMs": 10_000})], "once, in exec mode, stopped at 10 s");
            let id = ran["history_id"].as_str().unwrap().to_string();
            assert_eq!(
                ran,
                json!({"state": "ran", "history_id": id, "history_ids": [id], "output": {"result": "{\"named\":4}", "stdout": "renamed 4 tracks\n"},
                       "project": {"key": key, "epoch": before + 1}})
            );
            let row = ctx.history().row(&id).unwrap();
            assert_eq!(
                (row.title.as_str(), row.family.as_str(), row.state.as_str(), row.undo_available),
                ("Script: Name the drums", "script", "applied", false)
            );
            assert_eq!(row.script, Some(ScriptRun { sha256: ABC.into(), language: "python".into() }));
            assert_eq!((row.tool_call_id.as_deref(), row.call_outcome.as_deref()), (Some("call-1"), Some("ran")));
            ctx.snapshots().flush();
            let published: Value = serde_json::from_slice(&std::fs::read(dir.path().join("history.json")).unwrap()).unwrap();
            assert_eq!(published["items"][0]["script"], json!({"sha256": ABC, "language": "python"}));
            assert_eq!(
                published["items"][0]["note"],
                "Kumi can't undo a script: Live's own Undo may, if its step is still the latest there.",
                "so it isn't taken for a change from before a restart"
            );
            assert!(!published.to_string().contains("\"abc\""), "the code itself isn't published");
            let kept = std::fs::read_to_string(ctx.paths().state_dir.join("scripts").join(format!("{ABC}.py"))).unwrap();
            assert_eq!(kept, "abc");
            let mut left: Vec<String> =
                std::fs::read_dir(&scripts).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
            left.sort();
            assert_eq!(
                left,
                [pending, format!("{}.device.json", "4".repeat(64)), format!("{}.genexpr", "5".repeat(64)), format!("{ABC}.py")],
                "only old code no row names goes; a pending approval's and other kinds of file stay"
            );
            // Undo: Kumi can't, and says the DAW's own undo may, in the preview and the undo alike.
            let note = "Kumi can't undo a script: Live's own Undo may, if its step is still the latest there.";
            assert_eq!(
                call(&ctx, &crate::serve::change::UNDO, json!({"history_id": id, "preview": true})).await,
                json!({"state": "preview", "undo_available": false, "reason": "not_undoable", "note": note})
            );
            assert_eq!(
                call(&ctx, &crate::serve::change::UNDO, json!({"history_id": id})).await,
                json!({"state": "refused", "code": "not_undoable", "reason": note})
            );
            // A whole-change undo (#1129; the plugin's "undo that") meets the same refusal first, previewed or sent, so
            // nothing is asked of the DAW and nothing is undone.
            assert_eq!(
                call(&ctx, &crate::serve::change::UNDO, json!({"history_id": id, "whole_change": true, "preview": true})).await,
                json!({"state": "preview", "undo_available": false, "reason": "not_undoable", "note": note})
            );
            assert_eq!(
                call(&ctx, &crate::serve::change::UNDO, json!({"history_id": id, "whole_change": true, "expect_rows": "0".repeat(64)}))
                    .await,
                json!({"state": "refused", "code": "not_undoable", "reason": note})
            );
            assert_eq!(ctx.history().row(&id).unwrap().state, "applied", "nothing undone");
            assert_eq!(given.borrow().len(), 1, "the DAW was asked nothing more");
        })
        .await;
    }

    #[tokio::test]
    async fn a_script_that_raises_failed_and_one_live_didnt_answer_is_unsure_each_one_row() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path(), DawId::Ableton);
            let given = Rc::new(RefCell::new(Vec::new()));
            let raised = json!({"ok": false, "result": null, "stdout": "", "error": {"type": "NameError", "message": "name 'x' is not defined", "traceback": ""}});
            let key = looked(&ctx, Rc::new(Python { answer: Some(raised), given: given.clone() }));
            let args = json!({"language": "python", "purpose": "Name the drums", "code": "abc", "expected_project_key": key, "expected_sha256": ABC});
            let failed = call(&ctx, &SCRIPT, args.clone()).await;
            assert_eq!((failed["state"].clone(), failed["output"].clone()), (json!("failed"), json!({"error": "NameError: name 'x' is not defined"})));
            assert_eq!(ctx.history().row(failed["history_id"].as_str().unwrap()).unwrap().state, "failed");
            looked(&ctx, Rc::new(Python { answer: None, given: given.clone() }));
            let unsure = call(&ctx, &SCRIPT, args).await;
            assert_eq!(unsure["state"], "unsure");
            assert_eq!(unsure["history_ids"], json!([unsure["history_id"].clone()]), "its row, so it isn't audited again");
            let rows = ctx.history().rows(10);
            assert_eq!(rows.len(), 2, "one row a run");
            assert_eq!((rows[0].state.as_str(), rows[0].family.as_str()), ("unsure", "script"));
            assert!(ctx.link().unsure().is_some(), "said in status until the next look");
        })
        .await;
    }

    #[tokio::test]
    async fn a_lua_run_is_one_run_lua_step_and_the_executors_row_is_the_scripts_own() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path(), DawId::Reaper);
            let given = Rc::new(RefCell::new(Vec::new()));
            let key = looked(&ctx, Rc::new(Plan { ctx: ctx.handle(), given: given.clone() }));
            let code = "for i = 1, 3 do reaper.InsertTrackAtIndex(0, true) end\nreturn 3\n";
            let sha = sha256(code);
            let args = json!({"language": "lua", "purpose": "Add three tracks", "code": code, "expected_project_key": key, "expected_sha256": sha});
            let ran = call(&ctx, &SCRIPT, args).await;
            assert_eq!(
                *given.borrow(),
                [json!({"steps": [{"tool": "run_lua", "input": {"code": code, "timeoutMs": 10_000}}]})],
                "one step, outside the allowlist, stopped at 10 s"
            );
            assert_eq!((ran["state"].clone(), ran["output"].clone()), (json!("ran"), json!({"result": "3", "stdout": "made 3 tracks"})));
            let rows = ctx.history().rows(10);
            assert_eq!(rows.len(), 1, "no second row");
            assert_eq!(ran["history_id"], rows[0].id, "the executor's row is the script's");
            assert_eq!(
                (rows[0].title.as_str(), rows[0].family.as_str(), rows[0].undo_available, rows[0].script.clone()),
                ("Script: Add three tracks", "script", false, Some(ScriptRun { sha256: sha, language: "lua".into() }))
            );
            // The executor's later word on it (REAPER's own undo took it back) keeps it the script's, never Kumi's to undo.
            let mut record = lua_record(ChangeState::Applied);
            ctx.history().record(&record, &key, ctx.snapshots());
            record.state = ChangeState::Undone;
            let after = ctx.history().record(&record, &key, ctx.snapshots());
            assert_eq!((after.title.as_str(), after.family.as_str(), after.state.as_str(), after.undo_available), ("Script: Add three tracks", "script", "undone", false));
            let note = "Kumi can't undo a script: REAPER's own Undo may, if its step is still the latest there.";
            assert_eq!(call(&ctx, &crate::serve::change::UNDO, json!({"history_id": after.id, "preview": true})).await["note"], note);
            // Python isn't REAPER's.
            let python = json!({"language": "python", "purpose": "x", "code": "x = 1", "expected_project_key": key, "preview": true});
            assert_eq!(call(&ctx, &SCRIPT, python).await["code"], "invalid");
        })
        .await;
    }

    /// Each DAW's deadline (r9) as the preview gives it, the bridge is given it, and its stop is said: 10 s for Live and
    /// REAPER, Renoise's tool's own 800 ms.
    #[tokio::test]
    async fn each_daws_deadline_is_in_the_preview_and_its_stop_is_a_failed_run_in_kumis_words() {
        local(async {
            assert_eq!((deadline_seconds(DawId::Ableton), deadline_seconds(DawId::Reaper)), (json!(10), json!(10)));
            assert_eq!((deadline_ms(DawId::Renoise), deadline_seconds(DawId::Renoise)), (800, json!(0.8)));
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path(), DawId::Renoise);
            let given = Rc::new(RefCell::new(Vec::new()));
            let key = looked(&ctx, Rc::new(Plan { ctx: ctx.handle(), given }));
            let preview = json!({"language": "lua", "purpose": "x", "code": "return 1", "expected_project_key": key, "preview": true});
            assert_eq!(call(&ctx, &SCRIPT, preview).await["deadline_seconds"], 0.8);
            // Each bridge's stop: Live's python.run (raised in the script, or after it ran), REAPER's wrapper, Renoise's tool.
            let stopped = |daw, error: Value| Ran::failed(error).stopped_at_deadline(daw).error;
            let live = python_error(&json!({"type": "TimeoutError", "message": "Python exceeded timeoutMs (10000 ms)", "traceback": "…"}));
            let ten = Some(json!("Kumi stopped the script after 10 seconds."));
            assert_eq!(stopped(DawId::Ableton, live), ten);
            let after = json!({"type": "PythonRan", "message": "The code ran, so Live may have changed, but then TimeoutError: Python exceeded timeoutMs (10000 ms)"});
            assert_eq!(stopped(DawId::Ableton, python_error(&after)), ten);
            assert_eq!(stopped(DawId::Reaper, json!({"message": "stopped after 10000 ms (the chunk's time limit)"})), ten);
            let renoise = json!({"message": "kumi:3: Kumi stopped it: it ran longer than 800 ms"});
            assert_eq!(stopped(DawId::Renoise, renoise), Some(json!("Kumi stopped the script after 0.8 seconds.")));
            // Any other error is the script's own.
            assert_eq!(stopped(DawId::Reaper, json!({"message": "attempt to call a nil value", "line": 2})), Some(json!({"message": "attempt to call a nil value", "line": 2})));
            let ran = Ran { state: "ran", result: None, stdout: Some(json!("Python exceeded timeoutMs")), error: None };
            assert_eq!(ran.stopped_at_deadline(DawId::Ableton).state, "ran", "a script that printed the words wasn't stopped");
            // Lua that kept nothing still says what it said, as the plan answers for it (lab, Renoise 3.5.4): the endless loop
            // stopped at its time is failed in Kumi's words, a read gives its value, and nothing printed (Lua's empty table,
            // `{}`) is no stdout.
            let plan = |step: Value| Ok(ToolResult::text(json!({"done": [step]}).to_string()));
            let looped = lua_ran("Renoise", plan(json!({"step": 1, "changed": "Ran Lua: while true do end: Only Renoise's own undo can take back what it did (Edit > Undo); Kumi can't", "change": null, "printed": {}, "failed": {"message": "kumi:1: Kumi stopped it: it ran longer than 800 ms"}})))
                .stopped_at_deadline(DawId::Renoise);
            assert_eq!((looped.state, looped.output()), ("failed", json!({"error": "Kumi stopped the script after 0.8 seconds."})));
            let read = lua_ran("Renoise", plan(json!({"step": 1, "changed": "Ran Lua: return song.transport.bpm: …", "change": null, "printed": {}, "result": 80})));
            assert_eq!((read.state, read.output()), ("ran", json!({"result": "80"})));
        })
        .await;
    }

    #[tokio::test]
    async fn a_live_script_stopped_at_its_deadline_failed_and_its_row_says_so() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path(), DawId::Ableton);
            let given = Rc::new(RefCell::new(Vec::new()));
            let error = json!({"type": "TimeoutError", "message": "Python exceeded timeoutMs (10000 ms)", "traceback": "Traceback…"});
            let key = looked(&ctx, Rc::new(Python { answer: Some(json!({"ok": false, "result": null, "stdout": "", "error": error})), given }));
            let args = json!({"language": "python", "purpose": "Loop", "code": "abc", "expected_project_key": key, "expected_sha256": ABC});
            let ran = call(&ctx, &SCRIPT, args).await;
            let words = "Kumi stopped the script after 10 seconds.";
            assert_eq!((ran["state"].clone(), ran["output"].clone()), (json!("failed"), json!({"error": words})));
            let row = ctx.history().row(ran["history_id"].as_str().unwrap()).unwrap();
            assert_eq!(row.state, "failed");
            assert_eq!(
                row.note.as_deref(),
                Some("Kumi stopped the script after 10 seconds. Kumi can't undo a script: Live's own Undo may, if its step is still the latest there.")
            );
        })
        .await;
    }
}
