//! A Lua chunk run in REAPER as a ReaScript, as a change: the chunk runs once, on the main thread, inside one undo block
//! named by the host, so REAPER's undo takes back whatever it did. What it did is REAPER's to know, not Kumi's, so Kumi's
//! own undo is refused (`daw-only`).
//!
//! The chunk is wrapped (see `wrapper`): it is compiled first, run with a time limit where REAPER's Lua has a debug hook,
//! `print` and the console are kept, and what it returns comes back as JSON (a track, item or take as its ref). The
//! wrapper leaves its answer for the extension in an ext state that it reads and deletes. The preview runs nothing in
//! REAPER (a script run moves REAPER's state count, and right after REAPER's undo it can wipe the redo list): a chunk
//! that doesn't compile is refused at the apply, with nothing run.

use serde_json::{json, Value};

use super::{Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};

const NOTE: &str = "Only REAPER's own undo can take back what it does (Edit > Undo).";
/// Where the wrapper leaves its answer.
const SECTION: &str = "kumi_run";
/// How long a chunk may run when the request names no limit, and the most it may name: REAPER's main thread waits.
const TIMEOUT_MS: u64 = 2_000;
const MOST_TIMEOUT_MS: u64 = 10_000;
/// The most of a chunk's answer kept (its result and what it printed).
const MOST_ANSWER: usize = 4 * 1024 * 1024;
/// The most of what a chunk prints that the wrapper keeps: lines, and bytes in all.
const MOST_LINES: usize = 2_000;
const MOST_PRINTED: usize = 1024 * 1024;
/// The most values of a chunk's result the wrapper writes (a result like `_G` is cut there, not encoded for minutes).
const MOST_NODES: usize = 100_000;
/// The most of an error's text the wrapper keeps.
const MOST_ERROR: usize = 64 * 1024;

fn code_of(input: &Value) -> Done<&str> {
    input
        .get("code")
        .and_then(Value::as_str)
        .filter(|code| !code.trim().is_empty())
        .ok_or_else(|| bad("code: the Lua to run (ReaScript: the reaper table and Lua's own libraries)"))
}

fn timeout_of(input: &Value) -> Done<u64> {
    match input.get("timeoutMs") {
        None | Some(Value::Null) => Ok(TIMEOUT_MS),
        Some(v) => v
            .as_u64()
            .filter(|ms| (1..=MOST_TIMEOUT_MS).contains(ms))
            .ok_or_else(|| bad(format!("timeoutMs must be a whole number of milliseconds from 1 to {MOST_TIMEOUT_MS}"))),
    }
}

/// "Ran Lua: " and the chunk's first line that says something.
fn title(code: &str) -> String {
    let line = code.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("--")).unwrap_or_else(|| code.trim());
    let clipped: String = line.chars().take(60).collect();
    let more = if line.chars().count() > 60 { "…" } else { "" };
    format!("Ran Lua: {clipped}{more}")
}

/// A long bracket that the code doesn't close: `[==[` … `]==]` with enough `=`, a space before the closing one (a chunk
/// ending in `]`, as `return t[1]` does, would otherwise close a `]]` with its own last character; a space, not a line,
/// keeps the line an error at the end names).
fn long_string(code: &str) -> String {
    let mut level = 0;
    while code.contains(&format!("]{}]", "=".repeat(level))) {
        level += 1;
    }
    let eq = "=".repeat(level);
    format!("[{eq}[\n{code} ]{eq}]")
}

/// The ReaScript that runs `code` and leaves its answer in the ext state `token`.
pub fn wrapper(code: &str, token: &str, timeout_ms: u64) -> String {
    format!(
        r##"-- Kumi: a Lua chunk run once, then taken away.
local TOKEN, LIMIT_MS = "{token}", {timeout_ms}
local code = {code}
local sethook, getinfo = debug and debug.sethook, debug and debug.getinfo
-- What the wrapper itself calls, taken before the chunk runs: a chunk that replaces any of them (error, the clock, a
-- string function) can't stop the time limit, keep a refusal from raising, or loop where no limit holds.
local raise, pcall, xpcall, select, type, next, rawget, tostring, ipairs = error, pcall, xpcall, select, type, next, rawget, tostring, ipairs
local gsub, format, byte, match, sub, concat = string.gsub, string.format, string.byte, string.match, string.sub, table.concat
local floor, abs, huge = math.floor, math.abs, math.huge
local clock, setext = reaper.time_precise, reaper.SetExtState
local validate, master, item_string, take_string, track_guid =
  reaper.ValidatePtr2, reaper.GetMasterTrack, reaper.GetSetMediaItemInfo_String, reaper.GetSetMediaItemTakeInfo_String, reaper.GetTrackGUID
-- The wrapper's own functions, by identity: past the time limit everything else raises (the chunk, code it loads with
-- load, dofile or require, callbacks it hands in), never these, so the wrapper's own bookkeeping is never cut in half.
local OWN = {{}}
local function own(f) OWN[f] = true return f end
if getinfo then own(getinfo(1, "f").func) end
local STOP = "stopped after " .. LIMIT_MS .. " ms (the chunk's time limit)"
local PAST = "(an answer past what Kumi keeps)"
local function quote(s)
  return '"' .. gsub(s, '[%c"\\]', function(c)
    if c == '"' then return '\\"' elseif c == '\\' then return '\\\\' elseif c == '\n' then return '\\n'
    elseif c == '\r' then return '\\r' elseif c == '\t' then return '\\t' end
    return format('\\u%04x', byte(c))
  end) .. '"'
end
local function ref_of(v)
  local ok, kind = pcall(function()
    if validate(0, v, "MediaTrack*") then
      if v == master(0) then return "r:track:master" end
      return "r:track:" .. track_guid(v)
    elseif validate(0, v, "MediaItem*") then
      local _, guid = item_string(v, "GUID", "", false)
      return "r:item:" .. guid
    elseif validate(0, v, "MediaItem_Take*") then
      local _, guid = take_string(v, "GUID", "", false)
      return "r:take:" .. guid
    end
  end)
  if ok and type(kind) == "string" then return kind end
  return "userdata"
end
-- What the chunk answered, as JSON: tables read raw (no metamethod runs the chunk's code), each table written once (a
-- table met again, or a cycle, is "(seen above)"), within a budget of values and bytes.
local function encode(v)
  local nodes, bytes, seen, out = 0, 0, {{}}, {{}}
  local function put(text)
    bytes = bytes + #text
    if bytes > {most_bytes} then raise(PAST, 0) end
    out[#out + 1] = text
  end
  local function key(k)
    if type(k) == "string" then return k elseif type(k) == "number" then return format("%.17g", k) end
    return "(" .. type(k) .. ")"
  end
  local function walk(v, depth)
    nodes = nodes + 1
    if nodes > {most_nodes} then raise(PAST, 0) end
    local t = type(v)
    if v == nil then put("null")
    elseif t == "boolean" then put(v and "true" or "false")
    elseif t == "number" then
      if v ~= v or v == huge or v == -huge then put("null")
      elseif v == floor(v) and abs(v) < 2^53 then put(format("%d", v))
      else put(format("%.17g", v)) end
    elseif t == "string" then put(quote(v))
    elseif t == "userdata" then put(quote(ref_of(v)))
    elseif t == "table" then
      if seen[v] then put(quote("(seen above)")) return end
      if depth > 32 then put(quote("(nested too deep)")) return end
      seen[v] = true
      local count, n = 0, 0
      for _ in next, v do count = count + 1 end
      while rawget(v, n + 1) ~= nil do n = n + 1 end
      if count == 0 then put("[]")
      elseif count == n then
        put("[")
        for i = 1, n do
          if i > 1 then put(",") end
          walk(rawget(v, i), depth + 1)
        end
        put("]")
      else
        put("{{")
        local first = true
        for k, value in next, v do
          if not first then put(",") end
          first = false
          put(quote(key(k)) .. ":")
          walk(value, depth + 1)
        end
        put("}}")
      end
    else put(quote(t)) end
  end
  walk(v, 0)
  return concat(out)
end
-- An error as JSON: its text, cut at {most_error} bytes, and its line in the chunk. An error that isn't text or a number
-- is named by its type: its __tostring would run the chunk's code where no time limit holds.
local function where(message)
  local t = type(message)
  if t == "number" then message = tostring(message) elseif t ~= "string" then message = "(a " .. t .. " raised as the error)" end
  if #message > {most_error} then message = sub(message, 1, {most_error}) .. " (cut)" end
  local line, rest = match(message, '^chunk:(%d+): (.*)$')
  if line then return '{{"message":' .. quote(rest) .. ',"line":' .. line .. '}}' end
  return '{{"message":' .. quote(message) .. '}}'
end
local function answer(text) setext("{section}", TOKEN, text, false) end
local chunk, err = load(code, "=chunk", "t")
if not chunk then answer('{{"compiled":false,"error":' .. where(err) .. '}}') return end
-- What it prints, up to {most_lines} lines and {most_printed} bytes (a print loop would fill REAPER's memory and ext state).
local printed, held, dropped = {{}}, 0, 0
local hold = own(function(text)
  if #printed >= {most_lines} or held + #text > {most_printed} then
    dropped = dropped + 1
    return
  end
  held = held + #text
  printed[#printed + 1] = text
end)
local keep = own(function(...)
  local parts = {{}}
  for i = 1, select("#", ...) do parts[#parts + 1] = tostring((select(i, ...))) end
  hold(concat(parts, "\t"))
end)
print = keep
reaper.ShowConsoleMsg = own(function(text) hold(tostring(text)) end)
-- A dialog would wait for someone while REAPER's main thread waits for the chunk (no hook runs inside it).
for _, name in ipairs({{"MB", "ShowMessageBox", "GetUserInputs", "GetUserFileNameForRead", "ShowPopupMenu", "ShowActionList", "PromptForAction"}}) do
  if reaper[name] then
    reaper[name] = own(function() raise("reaper." .. name .. " opens a dialog, which would hold REAPER until someone answers it: Kumi refuses it (print shows a result)", 2) end)
  end
end
-- The chunk runs once: code it hands to REAPER for later (defer, runloop, atexit) would run after it, outside Kumi's undo
-- block and its time limit, so its edits would be no change of Kumi's and a loop there would hold REAPER.
for _, name in ipairs({{"defer", "runloop", "atexit"}}) do
  if reaper[name] then
    reaper[name] = own(function() raise("reaper." .. name .. " would run code after the chunk, outside Kumi's undo block and time limit: run-lua runs a chunk once, so do the work in it", 2) end)
  end
end
-- The chunk runs inside Kumi's undo block: blocks it opens are counted, and closed for it when it leaves them open (an end
-- past its own would close Kumi's); REAPER's undo and redo would move the history under Kumi's HISTORY; what it holds
-- back of the screen's refresh is let go after it.
local orig = {{}}
for _, name in ipairs({{"Undo_BeginBlock", "Undo_BeginBlock2", "Undo_EndBlock", "Undo_EndBlock2", "PreventUIRefresh", "Main_OnCommand", "Main_OnCommandEx"}}) do
  orig[name] = reaper[name]
end
local blocks, held_back = 0, 0
for _, name in ipairs({{"Undo_BeginBlock", "Undo_BeginBlock2"}}) do
  if orig[name] then reaper[name] = own(function(...) blocks = blocks + 1 return orig[name](...) end) end
end
for _, name in ipairs({{"Undo_EndBlock", "Undo_EndBlock2"}}) do
  if orig[name] then
    reaper[name] = own(function(...)
      if blocks == 0 then raise("reaper." .. name .. " with no Undo_BeginBlock of the chunk's own would end Kumi's undo block around the chunk: Kumi refuses it", 2) end
      blocks = blocks - 1
      return orig[name](...)
    end)
  end
end
if orig.PreventUIRefresh then
  reaper.PreventUIRefresh = own(function(n) held_back = held_back + (tonumber(n) or 0) return orig.PreventUIRefresh(n) end)
end
for _, name in ipairs({{"Undo_DoUndo2", "Undo_DoRedo2"}}) do
  if reaper[name] then
    reaper[name] = own(function() raise("reaper." .. name .. " would move REAPER's history under Kumi's HISTORY: Kumi refuses it", 2) end)
  end
end
for _, name in ipairs({{"Main_OnCommand", "Main_OnCommandEx"}}) do
  if orig[name] then
    reaper[name] = own(function(command, ...)
      if command == 40029 or command == 40030 then
        raise("REAPER's undo and redo (action " .. command .. ") would move REAPER's history under Kumi's HISTORY: Kumi refuses them", 2)
      end
      return orig[name](command, ...)
    end)
  end
end
-- The time limit: a hook checks the clock; past it, every line of code that isn't the wrapper's own raises, so a chunk
-- that catches errors (a loop of pcall), or runs code it loaded, stops too. Coroutines the chunk makes get the hook as
-- well; the chunk can't take it off.
local started = clock()
local stopped = false
local hook
hook = own(function()
  if not stopped and (clock() - started) * 1000 <= LIMIT_MS then return end
  if not stopped then
    stopped = true
    sethook(hook, "", 1)
  end
  local info = getinfo(2, "f")
  if info and not OWN[info.func] then raise(STOP, 0) end
end)
if sethook then
  local create, resume = coroutine.create, coroutine.resume
  coroutine.create = own(function(f)
    local co = create(f)
    sethook(co, hook, "", stopped and 1 or 1000)
    return co
  end)
  local pass = own(function(ok, ...)
    if not ok then raise((...), 0) end
    return ...
  end)
  coroutine.wrap = own(function(f)
    local co = coroutine.create(f)
    return own(function(...) return pass(resume(co, ...)) end)
  end)
  debug.sethook = own(function() raise("debug.sethook keeps Kumi's time limit while the chunk runs", 2) end)
  sethook(hook, "", 1000)
end
local results = {{ n = 0 }}
local pack = own(function(...) return {{ n = select("#", ...), ... }} end)
local ok, failure = xpcall(own(function() results = pack(chunk()) end), own(function(e) return e end))
if sethook then sethook() end
local closed = blocks
for _ = 1, blocks do
  if orig.Undo_EndBlock2 then orig.Undo_EndBlock2(0, "", -1) elseif orig.Undo_EndBlock then orig.Undo_EndBlock("", -1) end
end
if held_back ~= 0 and orig.PreventUIRefresh then orig.PreventUIRefresh(-held_back) end
local tail = closed > 0 and (',"closedBlocks":' .. closed) or ""
-- The answer, within its own second (and its budget of values and bytes).
local encoded, result
if sethook then
  local until_ = clock() + 1
  sethook(function() if clock() > until_ then raise(PAST, 0) end end, "", 1000)
end
encoded, result = pcall(function()
  if results.n == 1 then return encode(results[1]) elseif results.n > 1 then
    local parts = {{}}
    for i = 1, results.n do parts[i] = encode(results[i]) end
    return "[" .. concat(parts, ",") .. "]"
  end
  return "null"
end)
if sethook then sethook() end
if not encoded then result = quote(PAST) end
local lines = {{}}
for i, line in ipairs(printed) do lines[i] = quote(line) end
if dropped > 0 then lines[#lines + 1] = quote("(" .. dropped .. " more printed lines not kept)") end
if ok then
  answer('{{"compiled":true,"ok":true,"result":' .. result .. ',"printed":[' .. concat(lines, ",") .. ']' .. tail .. '}}')
else
  answer('{{"compiled":true,"ok":false,"error":' .. where(failure) .. ',"printed":[' .. concat(lines, ",") .. ']' .. tail .. '}}')
end
"##,
        code = long_string(code),
        section = SECTION,
        most_bytes = MOST_ANSWER / 2,
        most_nodes = MOST_NODES,
        most_lines = MOST_LINES,
        most_printed = MOST_PRINTED,
        most_error = MOST_ERROR,
    )
}

static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn token() -> String {
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("run{}-{n}", std::process::id())
}

/// Runs the wrapper and reads what it left: None when it left nothing (REAPER didn't run it, or it was cut short).
fn run(ctx: &Ctx, code: &str, timeout_ms: u64) -> Done<Option<Value>> {
    let token = token();
    ctx.reaper.run_script(ctx.proj, &wrapper(code, &token, timeout_ms)).map_err(failed)?;
    let Some(text) = ctx.reaper.take_ext_state(SECTION, &token) else { return Ok(None) };
    if text.len() > MOST_ANSWER {
        return Ok(Some(too_big(&text)));
    }
    serde_json::from_str(&text).map(Some).map_err(|e| failed(format!("the chunk's answer couldn't be read: {e}")))
}

/// An answer past what Kumi keeps: how the chunk ended, read from the answer's head (the wrapper writes `compiled`, `ok`
/// and, for a chunk that stopped with an error, the error, cut short, before what it printed).
fn too_big(text: &str) -> Value {
    let past = format!("(an answer of {} MB, more than Kumi keeps)", text.len() >> 20);
    let Some(rest) = text.strip_prefix(r#"{"compiled":true,"ok":false,"error":"#) else {
        return json!({"compiled": true, "ok": true, "result": past, "printed": []});
    };
    let error = serde_json::Deserializer::from_str(rest)
        .into_iter::<Value>()
        .next()
        .and_then(Result::ok)
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({"message": "(an error past what Kumi keeps)"}));
    json!({"compiled": true, "ok": false, "error": error, "printed": [past]})
}

fn compile_error(error: &Value) -> String {
    match error["line"].as_i64() {
        Some(line) => format!("the chunk doesn't compile: line {line}: {}", error["message"].as_str().unwrap_or("")),
        None => format!("the chunk doesn't compile: {}", error["message"].as_str().unwrap_or("")),
    }
}

pub fn preview(_ctx: &Ctx, input: &Value) -> Done<Preview> {
    let code = code_of(input)?;
    timeout_of(input)?;
    Ok(Preview {
        target: json!({"kind": "lua"}),
        prior: json!({}),
        fence: json!({"chars": code.chars().count()}),
        display: json!({"title": title(code), "note": NOTE}),
    })
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let code = code_of(input)?;
    let timeout_ms = timeout_of(input)?;
    if fence["chars"] != code.chars().count() {
        return Err(bad("the chunk isn't the one previewed"));
    }
    let ran = run(ctx, code, timeout_ms)?;
    let Some(ran) = ran else {
        // (REAPER was asked to run it and nothing came back: whether it ran, and what it did, can't be told)
        return Err((
            "uncertain".into(),
            "REAPER was asked to run the chunk and it gave no answer (if REAPER showed an error in the script around it, that is \
             Kumi's own wrapper failing, not the chunk): whatever it did, REAPER's undo takes back"
                .into(),
        ));
    };
    if ran["compiled"] != true {
        return Err(bad(compile_error(&ran["error"])));
    }
    let printed = ran.get("printed").cloned().unwrap_or(json!([]));
    let mut display = json!({"title": title(code), "note": NOTE});
    // (REAPER counts a block's changes only as it ends, so whether the chunk changed anything isn't known here: the block
    // is always closed as a write, REAPER's undo covers whatever it did, and the answer's undo.entry says whether REAPER
    // made a point of it)
    if let Some(closed) = ran["closedBlocks"].as_u64() {
        display["note"] = format!("{NOTE} It left {closed} undo block(s) of its own open: Kumi closed them.").into();
    }
    let applied = if ran["ok"] == true {
        json!({"result": ran["result"], "printed": printed})
    } else {
        let error = &ran["error"];
        let at = error["line"].as_i64().map(|line| format!(" at line {line}")).unwrap_or_default();
        let stays = " Anything it changed before the error stays: REAPER's undo takes it back.";
        display["note"] = format!("It stopped with an error{at}: {}.{stays}", error["message"].as_str().unwrap_or("")).into();
        json!({"failed": error, "printed": printed})
    };
    Ok(Applied { applied, display, ..Default::default() })
}

pub fn restore(_ctx: &Ctx, _restore: &Restore) -> Done<Restored> {
    Err(("daw-only".into(), "Kumi can't take back what a Lua chunk did: REAPER's own undo can (Edit > Undo)".into()))
}
