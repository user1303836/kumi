//! M4's acceptance runner for Renoise (a lab check, not a test): every change kind in Kumi's Renoise table, through Kumi's
//! whole stack (its change tools, make_changes, undo_change and HISTORY; the host in process; the tool in Renoise).
//!
//! accept_renoise <kumi-home> [--only <case,...>] [--big] [--out <results.md>] | --lua <probe.lua>
//!
//! - Renoise runs with Kumi's tool installed and connected to `<kumi-home>` (its discovery and secret), stopped. The runner
//!   builds what each case needs in the open song (tracks, instruments, patterns named "K …"): use a scratch song, never a
//!   producer's.
//! - Per kind: setup (Lua through the host) → make_changes (preview + apply) → read back that it took → Kumi's undo → read
//!   back that it's gone → apply again → Renoise's undo (`song:undo()`) → HISTORY says undone → Kumi's undo answers that it's
//!   undone already → Renoise's redo → HISTORY says it's back → Kumi's undo puts it back or refuses honestly. Kinds Renoise's
//!   undo skips: HISTORY's note says so, and Renoise's undo leaves them (a sentinel step before takes the undo).
//! - Then the actions, run_lua, the focus following a selection made in Renoise, and edge cases. `--big` instead builds a
//!   64-track, 100-pattern song in the song Renoise opened and times reads, changes and undos there. One line per case on
//!   stdout; the results table to `--out` or stdout.
use daw_bridge::Daw;
use daw_mcp_server::host::Host;
use futures::future::LocalBoxFuture;
use futures::FutureExt;
use kumi_agent::core::view::DawFocus;
use kumi_agent::core::{context::IntegrationContext, contracts::*};
use kumi_agent::mcp::client::{McpEndpoint, StderrStatus};
use kumi_agent::mcp::types::{CallToolResult, ContentBlock, Implementation, ListToolsResult};
use kumi_common::abort::Signal;
use kumi_daw_own::bridge::{Connect, OwnOptions};
use kumi_daw_renoise::{changes as renoise_changes, create_renoise_integration};
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::{cell::Cell, cell::RefCell, rc::Rc, sync::Arc, time::Duration, time::Instant};

kumi_testkit::in_process!();
use in_process::InProcess;

/// Between a step and the next one Renoise's undo must tell apart (two writes close together are one undo step).
const GAP_MS: u64 = 450;
/// How long HISTORY may take to follow Renoise's undo or redo (the tool tells it on its next timer).
const TOLD_MS: u64 = 3_000;

// ---------------------------------------------------------------------------------------------------------------------
// The host, with a producer's edit slipped in before a chosen call (the `changed` and new-song edge cases)

type Hook = Box<dyn FnOnce() -> LocalBoxFuture<'static, ()>>;

struct Hooked {
    inner: Rc<InProcess>,
    /// (method, calls of it to let pass first, what to do before the next one)
    hooks: RefCell<Vec<(&'static str, usize, Option<Hook>)>>,
}

impl Hooked {
    fn before(&self, method: &'static str, skip: usize, hook: Hook) {
        self.hooks.borrow_mut().push((method, skip, Some(hook)));
    }
    fn armed(&self) -> bool {
        !self.hooks.borrow().is_empty()
    }
}

#[async_trait::async_trait(?Send)]
impl McpEndpoint for Hooked {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, cursor: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        self.inner.list(cursor, signal).await
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        let hook = {
            let mut hooks = self.hooks.borrow_mut();
            let found = hooks.iter().position(|(method, _, _)| *method == name);
            match found {
                Some(k) if hooks[k].1 > 0 => {
                    hooks[k].1 -= 1;
                    None
                }
                Some(k) => hooks.remove(k).2,
                None => None,
            }
        };
        if let Some(hook) = hook {
            hook().await;
        }
        self.inner.call(name, args, signal).await
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
        self.inner.stderr_status()
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.inner.close().await
    }
}

/// A host tool's JSON answer, or why it refused (its words and code).
fn answer(result: &CallToolResult) -> Result<Value, (String, Option<String>)> {
    let text: String = result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
    if result.is_error == Some(true) {
        let why = value.get("error").and_then(Value::as_str).map(str::to_owned).unwrap_or(text);
        return Err((why, value.get("code").and_then(Value::as_str).map(str::to_owned)));
    }
    Ok(value)
}

async fn host_call(host: &InProcess, name: &str, args: Value) -> Result<Value, (String, Option<String>)> {
    match host.call(name, args.as_object().cloned().unwrap_or_default(), Signal::new()).await {
        Ok(result) => answer(&result),
        Err(error) => Err((format!("{error:?}"), None)),
    }
}

/// What Lua run in Renoise through the host answered (its `return`), or why it failed. Not Kumi's: no HISTORY row, and
/// the host forgets it at once.
async fn lua_on(host: &InProcess, code: &str) -> Result<Value, String> {
    let code = format!("{PRELUDE}\n{code}");
    // (the host asks every change for the epoch its refs come from: Lua has none, the song's now is given)
    let song = host_call(host, "read", json!({"kind": "song"})).await.map_err(|(why, _)| format!("read: {why}"))?;
    let preview =
        host_call(host, "change_preview", json!({"kind": "run-lua", "input": {"code": code, "timeoutMs": 800}, "epoch": song["epoch"]}))
            .await
            .map_err(|(why, code)| format!("preview {code:?}: {why}"))?;
    let (Some(id), Some(confirmation)) = (preview["transactionId"].as_str(), preview["confirmation"].as_str()) else {
        return Err(format!("no transaction: {preview}"));
    };
    let key = uuid::Uuid::new_v4().to_string();
    let applied = host_call(host, "change_apply", json!({"transactionId": id, "confirmation": confirmation, "idempotencyKey": key}))
        .await
        .map_err(|(why, code)| format!("apply {code:?}: {why}"))?;
    host_call(host, "change_release", json!({"transactionIds": [id]})).await.ok();
    if let Some(failed) = applied.pointer("/applied/failed") {
        return Err(format!("Lua: {}", failed["message"].as_str().unwrap_or_default()));
    }
    Ok(applied.pointer("/applied/result").cloned().unwrap_or(Value::Null))
}

/// Helpers every setup and probe chunk may call (they run in one callback each, with `song`).
const PRELUDE: &str = r#"
local R = renoise
local function master_index()
  for k = 1, #song.tracks do if song.tracks[k].type == R.Track.TRACK_TYPE_MASTER then return k end end
end
local function new_track(name, devices)
  local m = master_index()
  local tr = song:insert_track_at(m)
  tr.name = name
  for _, path in ipairs(devices or {}) do tr:insert_device_at(path, #tr.devices + 1) end
  return m
end
local function sine(sample, frames, hz)
  local b = sample.sample_buffer
  b:create_sample_data(44100, 16, 1, frames)
  b:prepare_sample_data_changes()
  for f = 1, frames do b:set_sample_data(1, f, 0.4 * math.sin(f * hz * 6.2831853 / 44100)) end
  b:finalize_sample_data_changes()
end
local function new_instrument(name, samples, frames)
  local i = #song.instruments + 1
  local ins = song:insert_instrument_at(i)
  ins.name = name
  for k = 1, samples or 0 do
    local s = ins:insert_sample_at(k)
    s.name = name .. " " .. k
    sine(s, frames or 2205, 110 * k)
  end
  return i
end
local function new_slot(pattern)
  local seq = song.sequencer
  local q = #seq.pattern_sequence + 1
  if pattern then seq:insert_sequence_at(q, pattern) else pattern = seq:insert_new_pattern_at(q) end
  return q, pattern
end
local function put(p, t, l, note, col)
  local c = song:pattern(p):track(t):line(l):note_column(col or 1)
  c.note_string = note
  c.instrument_value = 0
end
local function notes(p, t, n, cols)
  local out, pt = {}, song:pattern(p):track(t)
  for l = 1, n or 16 do
    local row = {}
    for c = 1, cols or 1 do
      local nc = pt:line(l):note_column(c)
      row[c] = nc.note_string .. nc.instrument_string .. nc.volume_string .. nc.delay_string
    end
    out[l] = table.concat(row, " ")
  end
  return out
end
local function names() local out = {} for k, tr in ipairs(song.tracks) do out[k] = tr.name end return out end
local function inames() local out = {} for k, ins in ipairs(song.instruments) do out[k] = ins.name end return out end
local function seq() local out = {} for k, p in ipairs(song.sequencer.pattern_sequence) do out[k] = p end return out end
local function paths(t) local out = {} for k, d in ipairs(song:track(t).devices) do out[k] = d.device_path .. "|" .. d.display_name end return out end
local function values(d) local out = {} for k, p in ipairs(d.parameters) do out[k] = p.value end return out end
local function pdata(d)
  return (d.active_preset_data:gsub("<SelectedPresetIsModified>[^<]*</SelectedPresetIsModified>", ""):gsub("<IsSelected>[^<]*</IsSelected>", ""))
end
local function named_track(name) for k, tr in ipairs(song.tracks) do if tr.name == name then return k end end end
local function named_instrument(name) for k, ins in ipairs(song.instruments) do if ins.name == name then return k end end end
local function moddev(set, kind, target)
  for _, p in ipairs(set.available_devices) do
    if (p:match("([^/]+)$") or p) == kind then return set:insert_device_at(p, target or 1, #set.devices + 1) end
  end
  error("no modulation device " .. kind)
end
local function setnames(i) local out = {} for k, s in ipairs(song:instrument(i).sample_modulation_sets) do local d = {} for j, x in ipairs(s.devices) do d[j] = x.name end out[k] = s.name .. ":" .. table.concat(d, ",") end return out end
local function chainpaths(i, c) local out = {} for k, d in ipairs(song:instrument(i).sample_device_chains[c].devices) do out[k] = d.device_path .. "|" .. tostring(d.is_active) end return out end
local function chainnames(i) local out = {} for k, c in ipairs(song:instrument(i).sample_device_chains) do out[k] = c.name end return out end
local function phnames(i) local out = {} for k, p in ipairs(song:instrument(i).phrases) do out[k] = p.name end return out end
local function phnotes(i, p, n) local out, ph = {}, song:instrument(i):phrase(p) for l = 1, n or 8 do out[l] = ph:line(l):note_column(1).note_string end return out end
local function snames(i) local out = {} for k, s in ipairs(song:instrument(i).samples) do out[k] = s.name .. "/" .. s.sample_buffer.number_of_frames end return out end
"#;

// ---------------------------------------------------------------------------------------------------------------------
// The cases

/// One case: a kind of Kumi's (its tool), what it needs made first, its input, and what it touches, read in Lua.
struct Case {
    tool: &'static str,
    /// Another case of the same kind ("presetData", "path").
    label: &'static str,
    /// Lua returning the names the input and probe use (`$name`); `$E` is the epoch, runner-wide ones are `$dir`, …
    setup: &'static str,
    /// Changes made through the host after the setup (kind, input template): what Lua can't make (a Doofer's inside).
    host: &'static [(&'static str, &'static str)],
    input: &'static str,
    /// Lua returning what the change touches, compared before, after and after each undo.
    probe: &'static str,
    /// Why a result is what it is (a limit, a design decision).
    note: &'static str,
    /// A known limit of Renoise's: failures of the steps after the second apply (Renoise's undo and redo, and Kumi's undo
    /// after them) make the case a limit, with this reason.
    limit: &'static str,
}

const fn case(tool: &'static str, setup: &'static str, input: &'static str, probe: &'static str) -> Case {
    Case { tool, label: "", setup, host: &[], input, probe, note: "", limit: "" }
}

impl Case {
    const fn label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }
    const fn host(mut self, host: &'static [(&'static str, &'static str)]) -> Self {
        self.host = host;
        self
    }
    const fn note(mut self, note: &'static str) -> Self {
        self.note = note;
        self
    }
    const fn limit(mut self, why: &'static str) -> Self {
        self.limit = why;
        self
    }
    fn name(&self) -> String {
        match self.label {
            "" => self.tool.to_owned(),
            label => format!("{} ({label})", self.tool),
        }
    }
}

const TRACK: &str = "return {t = new_track('K track')}";
const GAINER: &str = "return {t = new_track('K gainer', {'Audio/Effects/Native/Gainer'})}";
const EQ5: &str = "return {t = new_track('K eq', {'Audio/Effects/Native/EQ 5'})}";
const DOOFER: &str = "return {t = new_track('K doofer', {'Audio/Effects/Native/Doofer'})}";
const INSTRUMENT: &str = "return {i = new_instrument('K instrument', 1)}";
const TWO_NOTES: &str = "local t = new_track('K notes') put(1, t, 1, 'C-4') put(1, t, 3, 'E-4') return {t = t}";
const PHRASE: &str = "local i = new_instrument('K phrase', 1) song:instrument(i):insert_phrase_at(1).name = 'K ph' return {i = i}";
const CHAIN: &str =
    "local i = new_instrument('K chain', 1) local c = song:instrument(i):insert_sample_device_chain_at(1) c.name = 'K ch' return {i = i}";
const CHAIN_DEVICES: &str = "local i = new_instrument('K chain', 1) local c = song:instrument(i):insert_sample_device_chain_at(1) c:insert_device_at('Audio/Effects/Native/Gainer', 2) c:insert_device_at('Audio/Effects/Native/Delay', 3) return {i = i}";
const PLUGIN: &str = "local i = new_instrument('K plugin', 0) song:instrument(i).plugin_properties:load_plugin('Audio/Generators/AU/aumu:dls :appl') local pp = song:instrument(i).plugin_properties return {i = i, s = not pp.auto_suspend}";
const SLOT: &str = "local q, p = new_slot() return {q = q, p = p}";
/// What a Doofer holds: its devices' types and their gains, as its preset data says.
const INSIDE: &str = r#"local x = pdata(song:track($t):device(2)) local out = {} for k in x:gmatch('type="(%w+)"') do out[#out + 1] = k end for v in x:gmatch('<Volume>%s*<Value>([^<]*)</Value>') do out[#out + 1] = v end return out"#;
/// Why Kumi's undo of lines refuses after Renoise's undo and redo of them.
const LINES_REDO: &str = "Renoise's redo of lines can't be told from its undo bringing back the producer's like lines (review MUST 6, 10): Kumi's undo refuses after it, and HISTORY says Renoise put them back";
/// Why Renoise's undo can't take a plug-in instrument's change back now and then.
const AU_UNDO: &str = "Renoise's undo history goes empty now and then once AU plug-in instruments load and unload (can_undo() false before its undo; as after n-devices' device inventory): its undo takes nothing back, HISTORY stays as it was, Kumi's undo works";

static CASES: &[Case] = &[
    // Mixer, names, structure (M3 kinds, now through the whole stack)
    case("track_volume", TRACK, r#"{"ref":"$E:track:$t","volumeDb":-6}"#, "local tr = song:track($t) return {tr.prefx_volume.value, tr.postfx_volume.value}"),
    case("track_pan", TRACK, r#"{"ref":"$E:track:$t","pan":-0.5}"#, "return song:track($t).postfx_panning.value"),
    case("track_mute", TRACK, r#"{"ref":"$E:track:$t","mute":true}"#, "return song:track($t).mute_state"),
    case("track_solo", TRACK, r#"{"ref":"$E:track:$t","solo":true}"#, "local out = {} for _, tr in ipairs(song.tracks) do if tr.type ~= R.Track.TRACK_TYPE_MASTER then out[#out + 1] = tr.mute_state .. (tr.solo_state and '*' or '') end end return out"),
    case("track_rename", TRACK, r#"{"ref":"$E:track:$t","name":"K renamed"}"#, "return song:track($t).name"),
    case("track_color", TRACK, r##"{"ref":"$E:track:$t","color":"#336699","colorBlend":40}"##, "local tr = song:track($t) return {tr.color[1], tr.color[2], tr.color[3], tr.color_blend}"),
    case("track_add", "return {t = new_track('K add anchor')}", r#"{"type":"sequencer","name":"K added","index":"$t"}"#, "return names()"),
    case("track_move", "local a = new_track('K move A') local b = new_track('K move B') return {a = a, b = b}", r#"{"ref":"$E:track:$a","index":"$b"}"#, "return names()"),
    case("track_group", "local t = new_track('K member') local g = master_index() song:insert_group_at(g).name = 'K group' return {t = t, g = g}", r#"{"ref":"$E:track:$t","groupRef":"$E:track:$g"}"#, "local out = {} for k, tr in ipairs(song.tracks) do out[k] = tr.name .. '<' .. (tr.group_parent and tr.group_parent.name or '') end return out"),
    case("track_delete", "local t = new_track('K delete', {'Audio/Effects/Native/Delay'}) put(1, t, 1, 'C-4') return {t = t}", r#"{"ref":"$E:track:$t"}"#, "local k = named_track('K delete') return {names(), k and paths(k) or false, k and notes(1, k, 4) or false}"),
    case("song_bpm", "return {}", r#"{"bpm":133}"#, "return song.transport.bpm"),
    case("song_lpb", "return {}", r#"{"lpb":8}"#, "return song.transport.lpb"),
    case("song_tpl", "return {}", r#"{"tpl":3}"#, "return song.transport.tpl"),
    case("device_add", TRACK, r#"{"ref":"$E:track:$t","path":"Audio/Effects/Native/Delay"}"#, "return paths($t)"),
    case("device_delete", "return {t = new_track('K device', {'Audio/Effects/Native/Delay'})}", r#"{"ref":"$E:device:$t.2"}"#, "return paths($t)"),
    case("device_move", "return {t = new_track('K device', {'Audio/Effects/Native/Delay', 'Audio/Effects/Native/Gainer'})}", r#"{"ref":"$E:device:$t.2","index":3}"#, "return paths($t)"),
    case("device_bypass", "return {t = new_track('K device', {'Audio/Effects/Native/Delay'})}", r#"{"ref":"$E:device:$t.2","bypass":true}"#, "return song:track($t):device(2).is_active"),
    case("device_param", GAINER, r#"{"ref":"$E:device:$t.2","param":1,"value":0.5}"#, "return values(song:track($t):device(2))"),
    case("notes_write", TRACK, r#"{"patternRef":"$E:pattern:1","trackRef":"$E:track:$t","lines":[{"line":0,"notes":[{"col":1,"note":"C-4","instrument":0}]},{"line":4,"notes":[{"col":1,"note":"E-4","instrument":0}]}]}"#, "return notes(1, $t, 8)").note(LINES_REDO),
    case("notes_clear", TWO_NOTES, r#"{"patternRef":"$E:pattern:1","trackRef":"$E:track:$t"}"#, "return notes(1, $t, 4)").note(LINES_REDO),
    case("slot_insert", "local q, p = new_slot() return {q = q + 1, p = p}", r#"{"index":"$q","patternRef":"$E:pattern:$p"}"#, "return seq()"),
    case("slot_delete", SLOT, r#"{"ref":"$E:seq:$q"}"#, "return {seq(), #song.patterns}"),
    case("slot_pattern", "local q = new_slot() local _, p2 = new_slot() return {q = q, p2 = p2}", r#"{"ref":"$E:seq:$q","patternRef":"$E:pattern:$p2"}"#, "return {seq(), #song.patterns}"),
    case("slot_section", SLOT, r#"{"ref":"$E:seq:$q","start":true,"name":"K section"}"#, "return {song.sequencer:sequence_is_start_of_section($q), song.sequencer:sequence_section_name($q)}"),
    case("slot_mute", TRACK, r#"{"ref":"$E:seq:1","trackRef":"$E:track:$t","mute":true}"#, "return song.sequencer:track_sequence_slot_is_muted($t, 1)"),
    case("loop_range", "if #song.sequencer.pattern_sequence < 2 then new_slot() end return {}", r#"{"first":1,"last":2}"#, "local r = song.transport.loop_sequence_range return {r[1], r[2]}"),
    case("loop_pattern", "return {}", r#"{"on":true}"#, "return song.transport.loop_pattern"),
    case("instrument_rename", INSTRUMENT, r#"{"ref":"$E:instrument:$i","name":"K renamed"}"#, "return song:instrument($i).name"),
    case("instrument_macro", INSTRUMENT, r#"{"ref":"$E:instrument:$i","macro":1,"value":0.8,"name":"K cut"}"#, "local m = song:instrument($i):macro(1) return {m.value, m.name}"),
    case("sample_set", INSTRUMENT, r#"{"ref":"$E:sample:$i.1","volumeDb":-6,"transpose":3}"#, "local s = song:instrument($i):sample(1) return {s.volume, s.transpose}"),
    case("keyzone_set", INSTRUMENT, r#"{"ref":"$E:sample:$i.1","baseNote":60,"noteRange":[36,72]}"#, "local m = song:instrument($i):sample(1).sample_mapping return {m.base_note, m.note_range[1], m.note_range[2]}"),
    case("run_lua", TRACK, r#"{"code":"song:track($t).name = 'K lua named' return 42"}"#, "return song:track($t).name")
        .note("Kumi can't take back what Lua did (it doesn't know what it changed): its undo refuses, Renoise's undo takes it back"),
    // Song settings
    case("song_groove", "return {}", r#"{"on":true,"amounts":[0.5,0.25,0.75,0]}"#, "local tr = song.transport local a = tr.groove_amounts return {tr.groove_enabled, a[1], a[2], a[3], a[4]}"),
    case("song_metronome", "return {}", r#"{"beatsPerBar":3,"linesPerBeat":8,"volumeDb":-9}"#, "local tr = song.transport return {tr.metronome_beats_per_bar, tr.metronome_lines_per_beat, tr.metronome_volume}"),
    case("metronome_switch", "return {}", r#"{"on":true,"precount":2}"#, "local tr = song.transport return {tr.metronome_enabled, tr.metronome_precount_enabled, tr.metronome_precount_bars}"),
    case("edit_settings", "return {}", r#"{"editStep":3,"octave":5,"followPlayer":true}"#, "local tr = song.transport return {tr.edit_step, tr.octave, tr.follow_player}"),
    case("song_info", "return {}", r#"{"name":"K song","artist":"K artist","comments":"one\ntwo"}"#, "return {song.name, song.artist, song.comments}"),
    case("song_headroom", "return {}", r#"{"headroomDb":-3}"#, "return song.transport.track_headroom"),
    // Track settings
    case("track_collapse", TRACK, r#"{"ref":"$E:track:$t","collapsed":true}"#, "return song:track($t).collapsed"),
    case("track_width", TRACK, r#"{"ref":"$E:track:$t","width":50}"#, "return song:track($t).prefx_width.value"),
    case("track_output", "local t = new_track('K out') local r = song:track(t).available_output_routings return {t = t, r = r[#r]}", r#"{"ref":"$E:track:$t","routing":"$r"}"#, "return song:track($t).output_routing"),
    case("track_delay", TRACK, r#"{"ref":"$E:track:$t","delayMs":-5}"#, "return song:track($t).output_delay"),
    case("track_columns", TRACK, r#"{"ref":"$E:track:$t","noteColumns":3,"volume":true}"#, "local tr = song:track($t) return {tr.visible_note_columns, tr.volume_column_visible}"),
    case("track_column", TRACK, r#"{"ref":"$E:track:$t","col":1,"name":"K col","mute":true}"#, "local tr = song:track($t) return {tr:column_name(1), tr:column_is_muted(1)}"),
    // Patterns and the sequence
    case("pattern_name", SLOT, r#"{"ref":"$E:pattern:$p","name":"K pattern"}"#, "return song:pattern($p).name"),
    case("pattern_length", SLOT, r#"{"ref":"$E:pattern:$p","lines":32}"#, "return song:pattern($p).number_of_lines"),
    case("slot_color", "local _, p = new_slot() return {p = p, t = new_track('K slot colour')}", r##"{"ref":"$E:pattern:$p","trackRef":"$E:track:$t","color":"#123456"}"##, "local c = song:pattern($p):track($t).color return c and {c[1], c[2], c[3]} or false"),
    case("slot_clone", "local q, p = new_slot() put(p, 1, 1, 'C-4') return {q = q}", r#"{"ref":"$E:seq:$q"}"#, "return {seq(), #song.patterns}"),
    case("slot_move", "local q = new_slot() local q2 = new_slot() return {q = q, q2 = q2}", r#"{"ref":"$E:seq:$q","index":"$q2"}"#, "return seq()"),
    case("slot_unique", "local q, p = new_slot() local q2 = new_slot(p) put(p, 1, 1, 'D-4') return {q = q, q2 = q2}", r#"{"ref":"$E:seq:$q","lastRef":"$E:seq:$q2"}"#, "return {seq(), #song.patterns}"),
    // M7 (variations)
    case("slot_variation", "local t = new_track('K var') local _, p = new_slot() put(p, t, 1, 'C-4') put(p, t, 5, 'D-4') return {t = t, p = p}", r#"{"patternRef":"$E:pattern:$p","trackRef":"$E:track:$t","first":0,"last":7,"content":[{"line":0,"notes":[{"col":1,"note":"C-4","instrument":0}]},{"line":6,"notes":[{"col":1,"note":"E-4","instrument":0}],"effects":[{"col":1,"number":"0B","amount":0}]}],"name":"K V1","section":"K variations"}"#, "local s = song.sequencer return {seq(), #song.patterns, s:sequence_is_start_of_section(#s.pattern_sequence)}"),
    case("slots_drop", "local q, p = new_slot() local q2, p2 = new_slot() put(p, 1, 1, 'C-4') put(p2, 1, 1, 'D-4') return {q = q, q2 = q2}", r#"{"slotRefs":["$E:seq:$q","$E:seq:$q2"]}"#, "return {seq(), #song.patterns}"),
    case("pattern_alias", "local t = new_track('K alias') local _, p = new_slot() local _, p2 = new_slot() put(p, t, 1, 'C-4') put(p2, t, 3, 'G-4') return {t = t, p = p, p2 = p2}", r#"{"ref":"$E:pattern:$p2","trackRef":"$E:track:$t","aliasRef":"$E:pattern:$p"}"#, "return {song:pattern($p2):track($t).alias_pattern_index, notes($p2, $t, 4)}"),
    case("automation_write", "local t = new_track('K automation', {'Audio/Effects/Native/Gainer'}) local _, p = new_slot() return {t = t, p = p}", r#"{"ref":"$E:device:$t.2","param":1,"patternRef":"$E:pattern:$p","points":[{"line":0,"value":0.2},{"line":8,"value":0.8}]}"#, "local a = song:pattern($p):track($t):find_automation(song:track($t):device(2):parameter(1)) if not a then return false end local out = {a.playmode} for _, pt in ipairs(a.points) do out[#out + 1] = pt.time .. '=' .. pt.value end return out"),
    case("notes_transpose", TWO_NOTES, r#"{"patternRef":"$E:pattern:1","trackRef":"$E:track:$t","semitones":2}"#, "return notes(1, $t, 4)").note(LINES_REDO),
    case("notes_humanize", TWO_NOTES, r#"{"patternRef":"$E:pattern:1","trackRef":"$E:track:$t","delay":16,"volume":10,"seed":7}"#, "return notes(1, $t, 4)").note(LINES_REDO),
    case("lines_copy", TWO_NOTES, r#"{"patternRef":"$E:pattern:1","trackRef":"$E:track:$t","first":0,"last":3,"line":8}"#, "return notes(1, $t, 16)").note(LINES_REDO),
    // Instruments
    case("instrument_set", INSTRUMENT, r#"{"ref":"$E:instrument:$i","volumeDb":-6,"transpose":2}"#, "local ins = song:instrument($i) return {ins.volume, ins.transpose}"),
    // (instruments by Renoise's numbers, from 0: the end is the count)
    case("instrument_add", "return {n = #song.instruments}", r#"{"index":"$n","name":"K added"}"#, "return inames()"),
    case("instrument_delete", "return {i = new_instrument('K delete', 1)}", r#"{"ref":"$E:instrument:$i"}"#, "return inames()"),
    case("instrument_load", "local i = new_instrument('K xrni', 1) song.selected_instrument_index = i renoise.app():save_instrument('$dir/k.xrni') return {n = #song.instruments}", r#"{"path":"$dir/k.xrni","index":"$n"}"#, "return inames()"),
    case("instrument_duplicate", "return {i = new_instrument('K duplicate', 1)}", r#"{"ref":"$E:instrument:$i"}"#, "return inames()"),
    case("instrument_move", "local a = new_instrument('K move A', 1) local b = new_instrument('K move B', 1) return {a = a, b = b - 1}", r#"{"ref":"$E:instrument:$a","index":"$b"}"#, "return inames()"),
    case("keyzone_overlap", "return {i = new_instrument('K overlap', 2)}", r#"{"ref":"$E:instrument:$i","mode":"cycle"}"#, "return song:instrument($i).sample_mapping_overlap_mode"),
    case("keyzone_drumkit", "return {i = new_instrument('K kit', 3)}", r#"{"ref":"$E:instrument:$i"}"#, "local out = {} for k, s in ipairs(song:instrument($i).samples) do local m = s.sample_mapping out[k] = m.note_range[1] .. '-' .. m.note_range[2] .. '@' .. m.base_note end return out"),
    case("sample_load", "local i = new_instrument('K load', 1) song:instrument(i):sample(1).sample_buffer:save_as('$dir/k.wav', 'wav') return {i = i}", r#"{"ref":"$E:instrument:$i","path":"$dir/k.wav"}"#, "return snames($i)"),
    case("sample_delete", "return {i = new_instrument('K delete', 2)}", r#"{"ref":"$E:sample:$i.2"}"#, "return snames($i)"),
    case("sample_slices", "return {i = new_instrument('K slices', 1, 22050)}", r#"{"ref":"$E:sample:$i.1","insert":[5000,11000]}"#, "local s = song:instrument($i):sample(1) local out = {#song:instrument($i).samples} for _, m in ipairs(s.slice_markers) do out[#out + 1] = m end return out"),
    case("modset_add", INSTRUMENT, r#"{"ref":"$E:instrument:$i","name":"K set"}"#, "return setnames($i)"),
    case("modset_delete", "local i = new_instrument('K sets', 1) song:instrument(i):insert_sample_modulation_set_at(2) song:instrument(i).sample_modulation_sets[2].name = 'K set 2' return {i = i}", r#"{"ref":"$E:modset:$i.2"}"#, "return setnames($i)"),
    case("modset_set", INSTRUMENT, r#"{"ref":"$E:modset:$i.1","pitchRange":24,"name":"K set"}"#, "local s = song:instrument($i).sample_modulation_sets[1] return {s.name, s.pitch_range}"),
    case("moddevice_add", INSTRUMENT, r#"{"ref":"$E:modset:$i.1","path":"AHDSR","target":"volume"}"#, "return setnames($i)"),
    case("moddevice_delete", "local i = new_instrument('K mod', 1) moddev(song:instrument(i).sample_modulation_sets[1], 'AHDSR') return {i = i}", r#"{"ref":"$E:moddevice:$i.1.1"}"#, "return setnames($i)"),
    case("moddevice_move", "local i = new_instrument('K mod', 1) local s = song:instrument(i).sample_modulation_sets[1] moddev(s, 'AHDSR') moddev(s, 'LFO') return {i = i}", r#"{"ref":"$E:moddevice:$i.1.1","index":2}"#, "return setnames($i)"),
    case("moddevice_set", "local i = new_instrument('K mod', 1) moddev(song:instrument(i).sample_modulation_sets[1], 'LFO') return {i = i}", r#"{"ref":"$E:moddevice:$i.1.1","name":"K lfo","active":false}"#, "local d = song:instrument($i).sample_modulation_sets[1].devices[1] return {d.display_name, d.is_active}"),
    case("moddevice_param", "local i = new_instrument('K mod', 1) moddev(song:instrument(i).sample_modulation_sets[1], 'AHDSR') return {i = i}", r#"{"ref":"$E:moddevice:$i.1.1","param":"Attack","value":0.3}"#, "return values(song:instrument($i).sample_modulation_sets[1].devices[1])"),
    case("chain_add", INSTRUMENT, r#"{"ref":"$E:instrument:$i","name":"K chain"}"#, "return chainnames($i)"),
    case("chain_delete", CHAIN, r#"{"ref":"$E:chain:$i.1"}"#, "return chainnames($i)"),
    case("chain_set", CHAIN, r#"{"ref":"$E:chain:$i.1","name":"K air"}"#, "return chainnames($i)"),
    case("chaindevice_add", CHAIN, r#"{"ref":"$E:chain:$i.1","path":"Gainer"}"#, "return chainpaths($i, 1)"),
    case("chaindevice_delete", CHAIN_DEVICES, r#"{"ref":"$E:chaindevice:$i.1.2"}"#, "return chainpaths($i, 1)"),
    case("chaindevice_move", CHAIN_DEVICES, r#"{"ref":"$E:chaindevice:$i.1.2","index":3}"#, "return chainpaths($i, 1)"),
    case("chaindevice_bypass", CHAIN_DEVICES, r#"{"ref":"$E:chaindevice:$i.1.2","bypass":true}"#, "return chainpaths($i, 1)"),
    case("chaindevice_param", CHAIN_DEVICES, r#"{"ref":"$E:chaindevice:$i.1.2","param":1,"value":0.5}"#, "return values(song:instrument($i).sample_device_chains[1].devices[2])"),
    case("phrase_add", INSTRUMENT, r#"{"ref":"$E:instrument:$i","name":"K phrase"}"#, "return phnames($i)"),
    case("phrase_add", INSTRUMENT, r#"{"ref":"$E:instrument:$i","name":"K take","lines":8,"content":[{"line":0,"notes":[{"col":1,"note":"C-4","fx":"0S40"}]},{"line":4,"notes":[{"col":1,"note":"E-4","volume":"30"}],"effects":[{"col":1,"number":"0R","amount":3}]}]}"#, "local out = phnames($i) if #song:instrument($i).phrases > 0 then out[#out + 1] = table.concat(phnotes($i, 1, 8), ',') end return out").label("content"),
    case("phrase_delete", PHRASE, r#"{"ref":"$E:phrase:$i.1"}"#, "return phnames($i)"),
    case("phrases_drop", "local i = new_instrument('K drop', 1) local ins = song:instrument(i) for k = 1, 3 do local ph = ins:insert_phrase_at(k) ph.name = 'K p' .. k ph:line(1):note_column(1).note_string = 'C-4' end return {i = i}", r#"{"phraseRefs":["$E:phrase:$i.1","$E:phrase:$i.3"]}"#, "return phnames($i)"),
    // (review W1, W2: neighbours come back in their order, and the program stays on the phrase that stays)
    case("phrases_drop", "local i = new_instrument('K drop', 1) local ins = song:instrument(i) for k = 1, 5 do local ph = ins:insert_phrase_at(k) ph.name = 'K p' .. k ph:line(1):note_column(1).note_string = 'C-4' end ins.phrase_playback_mode = renoise.Instrument.PHRASES_PLAY_SELECTIVE ins.phrase_program = 5 return {i = i}", r#"{"phraseRefs":["$E:phrase:$i.2","$E:phrase:$i.3","$E:phrase:$i.4"]}"#, "local ins = song:instrument($i) local out = phnames($i) out[#out + 1] = 'program ' .. ins:phrase(ins.phrase_program).name return out").label("neighbours, program")
        .limit("Renoise's own undo of phrase deletes puts the phrases back but not the program's number, which deletes below it moved down (measured, 3.5.4): the program then plays another phrase. Kumi's undo puts it back on its phrase"),
    case("phrase_set", PHRASE, r#"{"ref":"$E:phrase:$i.1","lpb":8,"name":"K ph2"}"#, "local ph = song:instrument($i):phrase(1) return {ph.name, ph.lpb}"),
    case("phrase_map", PHRASE, r#"{"ref":"$E:phrase:$i.1","noteRange":[48,59]}"#, "local m = song:instrument($i):phrase(1).mapping return m and {m.note_range[1], m.note_range[2]} or false"),
    case("phrase_playback", PHRASE, r#"{"ref":"$E:instrument:$i","mode":"keymap"}"#, "return song:instrument($i).phrase_playback_mode"),
    case("phrase_write", PHRASE, r#"{"ref":"$E:phrase:$i.1","lines":[{"line":1,"notes":[{"col":1,"note":"C-4"}]}]}"#, "return phnotes($i, 1, 4)").note(LINES_REDO),
    case("phrase_clear", "local i = new_instrument('K phrase', 1) local ph = song:instrument(i):insert_phrase_at(1) ph:line(2):note_column(1).note_string = 'C-4' return {i = i}", r#"{"ref":"$E:phrase:$i.1"}"#, "return phnotes($i, 1, 4)").note(LINES_REDO),
    case("phrase_script", PHRASE, r#"{"ref":"$E:phrase:$i.1","code":"return pattern { unit = \"1/8\", event = { \"c4\", \"e4\" } }"}"#, "local ph = song:instrument($i):phrase(1) return {ph.playback_mode, table.concat(ph.script.paragraphs, '\\n')}"),
    case("phrase_render", "local i = new_instrument('K render', 1) local ph = song:instrument(i):insert_phrase_at(1) ph.playback_mode = R.InstrumentPhrase.PLAY_SCRIPT ph.script.paragraphs = {'return pattern { unit = \"1/8\", event = { \"c4\", \"e4\", \"g4\" } }'} ph.script:commit() return {i = i}", r#"{"ref":"$E:phrase:$i.1"}"#, "local ph = song:instrument($i):phrase(1) return {ph.playback_mode, ph.number_of_lines, phnotes($i, 1, 8)}"),
    case("plugin_load", "return {i = new_instrument('K plugin', 0)}", r#"{"ref":"$E:instrument:$i","path":"DLSMusicDevice"}"#, "local ins = song:instrument($i) local pp = ins.plugin_properties return {pp.plugin_loaded, pp.plugin_device and pp.plugin_device.name or false, ins.name}").limit(AU_UNDO),
    case("plugin_param", PLUGIN, r#"{"ref":"$E:instrument:$i","param":"Volume","value":0.5}"#, "return values(song:instrument($i).plugin_properties.plugin_device)").limit(AU_UNDO),
    case("plugin_preset", PLUGIN, r#"{"ref":"$E:instrument:$i","preset":1}"#, "local d = song:instrument($i).plugin_properties.plugin_device local x, h = d.active_preset_data, 0 for k = 1, #x do h = (h * 31 + x:byte(k)) % 2147483647 end return {d.active_preset, h, values(d)}")
        .limit("This Mac's AU instruments (DLSMusicDevice, AUMIDISynth, AUSampler, MS2000 Emulator) have one program each, which Renoise keeps: the answer is unchanged with no HISTORY row (checked); another program can't be tried here"),
    case("plugin_set", PLUGIN, r#"{"ref":"$E:instrument:$i","channel":2}"#, "return song:instrument($i).plugin_properties.channel").limit(AU_UNDO),
    case("plugin_suspend", PLUGIN, r#"{"ref":"$E:instrument:$i","autoSuspend":"$s"}"#, "return song:instrument($i).plugin_properties.auto_suspend").limit(AU_UNDO),
    case("midi_set", "return {i = new_instrument('K midi', 0)}", r#"{"ref":"$E:instrument:$i","channel":10}"#, "return song:instrument($i).midi_output_properties.channel"),
    // Devices (n-devices)
    case("device_preset", EQ5, r#"{"ref":"$E:device:$t.2","preset":2}"#, "local d = song:track($t):device(2) return {d.active_preset, values(d)}"),
    case("device_preset_data", "local t = new_track('K gainer', {'Audio/Effects/Native/Gainer'}) local p = song:track(t):device(2):parameter(1) p.value = 0.5 local x = song:track(t):device(2).active_preset_data p.value = 1 return {t = t, x = x}", r#"{"ref":"$E:device:$t.2","presetData":"$x"}"#, "return values(song:track($t):device(2))"),
    case("device_preset_data", "return {t = new_track('K compressor', {'Audio/Effects/Native/Compressor'})}", r#"{"ref":"$E:device:$t.2","path":"$preset"}"#, "return values(song:track($t):device(2))").label("path"),
    case("device_set", EQ5, r#"{"ref":"$E:device:$t.2","name":"K device","maximized":false}"#, "local d = song:track($t):device(2) return {d.display_name, d.is_maximized}"),
    case("doofer_macro", DOOFER, r#"{"ref":"$E:device:$t.2","macro":1,"name":"K cut","value":40}"#, "local d = song:track($t):device(2) return {d.parameters[1].name, d.parameters[1].value}"),
    case("doofer_device_add", DOOFER, r#"{"ref":"$E:device:$t.2","path":"Audio/Effects/Native/Gainer"}"#, INSIDE),
    case("doofer_device_delete", DOOFER, r#"{"ref":"$E:device:$t.2","at":[1]}"#, INSIDE)
        .host(&[("doofer-device-add", r#"{"ref":"$E:device:$t.2","path":"Audio/Effects/Native/Gainer"}"#)]),
    case("doofer_device_move", DOOFER, r#"{"ref":"$E:device:$t.2","at":[1],"to":[2]}"#, INSIDE).host(&[
        ("doofer-device-add", r#"{"ref":"$E:device:$t.2","path":"Audio/Effects/Native/Gainer"}"#),
        ("doofer-device-add", r#"{"ref":"$E:device:$t.2","path":"Audio/Effects/Native/Delay"}"#),
    ]),
    case("doofer_device_set", DOOFER, r#"{"ref":"$E:device:$t.2","at":[1],"param":1,"value":0.5}"#, INSIDE)
        .host(&[("doofer-device-add", r#"{"ref":"$E:device:$t.2","path":"Audio/Effects/Native/Gainer"}"#)]),
    case("splitter_set", "return {t = new_track('K splitter', {'Audio/Effects/Native/Splitter'})}", r#"{"ref":"$E:device:$t.2","type":"mid-side"}"#, "return pdata(song:track($t):device(2)):match('<Type>([^<]*)</Type>') or 'Parallel'"),
    case("notepad_write", "return {t = new_track('K notepad', {'Audio/Effects/Native/Notepad'})}", r#"{"ref":"$E:device:$t.2","text":"K line one\nK line two"}"#, "local out = {} for p in pdata(song:track($t):device(2)):gmatch('<Paragraph>([^<]*)</Paragraph>') do out[#out + 1] = p end return out"),
    case("meta_destination", "return {t = new_track('K meta', {'Audio/Effects/Native/*LFO', 'Audio/Effects/Native/Gainer'})}", r#"{"ref":"$E:device:$t.2","targetRef":"$E:device:$t.3","param":1}"#, "return values(song:track($t):device(2))"),
    case("doofer_make", "return {t = new_track('K make', {'Audio/Effects/Native/Gainer', 'Audio/Effects/Native/Delay'})}", r#"{"trackRef":"$E:track:$t","deviceRefs":["$E:device:$t.2","$E:device:$t.3"]}"#, "return paths($t)"),
    case("device_params", EQ5, r#"{"ref":"$E:device:$t.2","values":[{"param":1,"value":-6},{"param":2,"value":3}]}"#, "return values(song:track($t):device(2))"),
    case("device_chain_load", TRACK, r#"{"trackRef":"$E:track:$t","path":"$chain"}"#, "return paths($t)"),
    case("phrase_load", INSTRUMENT, r#"{"ref":"$E:instrument:$i","path":"$phrasefile"}"#, "return phnames($i)"),
    case("modset_load", INSTRUMENT, r#"{"ref":"$E:instrument:$i","path":"$modsetfile"}"#, "return setnames($i)"),
    case("instrument_macro_map", "local i = new_instrument('K macros', 1) moddev(song:instrument(i).sample_modulation_sets[1], 'AHDSR') return {i = i}", r#"{"ref":"$E:instrument:$i","macro":1,"add":[{"modulation":1,"device":1,"param":1}]}"#, "local i = named_instrument('K macros') local out = {#song.instruments} for _, mp in ipairs(song:instrument(i):macro(1).mappings) do out[#out + 1] = mp.parameter.name .. ':' .. mp.parameter_min .. '-' .. mp.parameter_max end return out"),
];

// ---------------------------------------------------------------------------------------------------------------------
// The lab: Kumi's integration over the host, and the calls a case makes

struct Lab {
    host: Rc<InProcess>,
    hooked: Rc<Hooked>,
    daw: Rc<dyn Integration>,
    tools: RefCell<HashMap<String, Rc<dyn KernelTool>>>,
    rows: Rc<RefCell<Vec<ChangeRecord>>>,
    told: Rc<RefCell<Vec<String>>>,
    focus: Rc<RefCell<Vec<(Instant, Option<DawFocus>)>>>,
    /// Runner-wide names: `dir` (a scratch folder), library files.
    vars: BTreeMap<String, Value>,
}

/// One Kumi tool call: its answer, whether it's an error, and how long it took.
struct Said {
    text: String,
    error: bool,
    ms: u128,
}

impl Lab {
    async fn lua(&self, code: &str) -> Result<Value, String> {
        lua_on(&self.host, code).await
    }

    async fn tool(&self, name: &str, input: Value) -> Said {
        let Some(tool) = self.tools.borrow().get(name).cloned() else {
            return Said { text: format!("no tool {name}"), error: true, ms: 0 };
        };
        let t = Instant::now();
        let result = tool.execute(input.as_object().cloned().unwrap_or_default(), Signal::new()).await;
        let ms = t.elapsed().as_millis();
        match result {
            Ok(result) => Said { text: result.text, error: result.is_error, ms },
            Err(error) => Said { text: format!("{error:?}"), error: true, ms },
        }
    }

    /// make_changes with these steps: its answer, and the HISTORY rows it added.
    async fn plan(&self, steps: Value) -> (Said, Vec<ChangeRecord>) {
        let before: Vec<String> = self.rows.borrow().iter().map(|row| row.id.clone()).collect();
        let said = self.tool("make_changes", json!({"steps": steps})).await;
        let added = self.rows.borrow().iter().filter(|row| !before.contains(&row.id)).cloned().collect();
        (said, added)
    }

    async fn undo(&self, id: &str) -> Said {
        self.tool("undo_change", json!({"change": id})).await
    }

    fn row(&self, id: &str) -> Option<ChangeRecord> {
        self.rows.borrow().iter().find(|row| row.id == id).cloned()
    }

    /// Waits up to `ms` for the row to reach `state`: the row as it is then.
    async fn wait_state(&self, id: &str, state: ChangeState, ms: u64) -> Option<ChangeRecord> {
        let t = Instant::now();
        loop {
            let row = self.row(id);
            if row.as_ref().is_some_and(|row| row.state == state) || t.elapsed() > Duration::from_millis(ms) {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The song's epoch now (the host's read), with Kumi's book brought to it (Kumi's read).
    async fn epoch(&self) -> String {
        let read = self.tool("renoise_read", json!({"kind": "song"})).await;
        if read.error {
            println!("   (Kumi's read of the song: {})", short(&read.text, 200));
        }
        let song = host_call(&self.host, "read", json!({"kind": "song"})).await.unwrap_or_default();
        song["epoch"].as_str().unwrap_or_default().to_owned()
    }

    async fn pause(&self) {
        tokio::time::sleep(Duration::from_millis(GAP_MS)).await;
    }
}

fn short(text: &str, n: usize) -> String {
    let flat: String = text.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    match flat.chars().count() > n {
        true => format!("{}…", flat.chars().take(n).collect::<String>()),
        false => flat,
    }
}

/// `$name` in a Lua chunk made the value's Lua text (longest names first, so `$t2` isn't `$t` and a 2).
fn fill_lua(template: &str, vars: &BTreeMap<String, Value>) -> String {
    let mut names: Vec<&String> = vars.keys().collect();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    let mut text = template.to_owned();
    for name in names {
        let value = match &vars[name] {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        text = text.replace(&format!("${name}"), &value);
    }
    text
}

/// `$name` in an input's strings: a string that is only `$name` takes the value as it is (a number, a boolean, text).
fn fill_json(value: &Value, vars: &BTreeMap<String, Value>) -> Value {
    match value {
        Value::String(text) => match text.strip_prefix('$').and_then(|name| vars.get(name)) {
            Some(whole) => whole.clone(),
            None => Value::String(fill_lua(text, vars)),
        },
        Value::Array(items) => Value::Array(items.iter().map(|item| fill_json(item, vars)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(k, v)| (k.clone(), fill_json(v, vars))).collect()),
        other => other.clone(),
    }
}

/// Two probes alike (numbers within a millionth: Renoise keeps floats).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(f64::NAN), y.as_f64().unwrap_or(f64::NAN));
            (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1.0)
        }
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// One case's run

#[derive(Clone, Copy, PartialEq, Debug)]
enum Verdict {
    Pass,
    Fail,
    Limit,
}

struct Outcome {
    name: String,
    apply_ms: Option<u128>,
    steps: Vec<(&'static str, String)>,
    verdict: Verdict,
    note: String,
    /// The probes as read at each step, printed under a case that didn't pass.
    trace: Vec<(&'static str, Value)>,
}

impl Outcome {
    fn new(name: String) -> Self {
        Self { name, apply_ms: None, steps: vec![], verdict: Verdict::Pass, note: String::new(), trace: vec![] }
    }
    fn step(&mut self, step: &'static str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        if !ok {
            self.verdict = Verdict::Fail;
        }
        self.steps.push((
            step,
            if ok {
                if detail.is_empty() {
                    "ok".into()
                } else {
                    detail
                }
            } else {
                format!("FAIL: {detail}")
            },
        ));
    }
    fn info(&mut self, step: &'static str, detail: impl Into<String>) {
        self.steps.push((step, detail.into()));
    }
    fn fail(mut self, step: &'static str, detail: impl Into<String>) -> Self {
        self.step(step, false, detail);
        self
    }
    fn line(&self) -> String {
        let mark = match self.verdict {
            Verdict::Pass => "ok  ",
            Verdict::Fail => "FAIL",
            Verdict::Limit => "lim ",
        };
        let ms = self.apply_ms.map(|ms| format!("{ms} ms")).unwrap_or_else(|| "-".into());
        let steps: Vec<String> = self.steps.iter().map(|(step, detail)| format!("{step} {}", short(detail, 160))).collect();
        let mut line = format!(
            "  {mark} {:<28} {:>7}  {}{}",
            self.name,
            ms,
            steps.join(" · "),
            if self.note.is_empty() { String::new() } else { format!("  [{}]", self.note) }
        );
        if self.verdict != Verdict::Pass {
            for (step, value) in &self.trace {
                line += &format!("\n        {step}: {}", short(&value.to_string(), 1_200));
            }
        }
        line
    }
}

async fn setup(lab: &Lab, case: &Case) -> Result<BTreeMap<String, Value>, String> {
    let mut vars = lab.vars.clone();
    let made = lab.lua(&fill_lua(case.setup, &vars)).await.map_err(|why| format!("setup: {why}"))?;
    if let Value::Object(fields) = made {
        vars.extend(fields);
    }
    for (kind, input) in case.host {
        vars.insert("E".into(), json!(lab.epoch().await));
        let input = fill_json(&serde_json::from_str(input).expect("a host step's input is JSON"), &vars);
        let preview = host_call(&lab.host, "change_preview", json!({"kind": kind, "input": input, "epoch": vars["E"]}))
            .await
            .map_err(|(why, _)| format!("setup {kind}: {why}"))?;
        let key = uuid::Uuid::new_v4().to_string();
        let args = json!({"transactionId": preview["transactionId"], "confirmation": preview["confirmation"], "idempotencyKey": key});
        host_call(&lab.host, "change_apply", args).await.map_err(|(why, _)| format!("setup {kind}: {why}"))?;
        host_call(&lab.host, "change_release", json!({"transactionIds": [preview["transactionId"]]})).await.ok();
    }
    vars.insert("E".into(), json!(lab.epoch().await));
    Ok(vars)
}

async fn probe(lab: &Lab, case: &Case, vars: &BTreeMap<String, Value>) -> Value {
    lab.lua(&fill_lua(case.probe, vars)).await.unwrap_or_else(|why| json!({"probe failed": why}))
}

/// Applies the case's change through make_changes: the row it made, or why not.
async fn apply(lab: &Lab, case: &Case, vars: &mut BTreeMap<String, Value>) -> Result<(ChangeRecord, Said), String> {
    vars.insert("E".into(), json!(lab.epoch().await));
    let input = fill_json(&serde_json::from_str(case.input).expect("a case's input is JSON"), vars);
    let (said, added) = lab.plan(json!([{"tool": case.tool, "input": input}])).await;
    if said.error {
        return Err(short(&said.text, 300));
    }
    match added.last() {
        Some(row) => Ok((row.clone(), said)),
        None if said.text.contains("\"change\":null") => Err(format!("unchanged, no HISTORY row: {}", short(&said.text, 300))),
        None => Err(format!("no HISTORY row: {}", short(&said.text, 300))),
    }
}

async fn run_case(lab: &Lab, case: &Case, outside_undo: bool) -> Outcome {
    let mut out = Outcome::new(case.name());
    out.note = case.note.to_owned();
    let mut vars = match setup(lab, case).await {
        Ok(vars) => vars,
        Err(why) => return out.fail("setup", why),
    };
    let p0 = probe(lab, case, &vars).await;
    out.trace.push(("p0", p0.clone()));
    lab.pause().await;
    // 1. Kumi's change, read back
    let (row, said) = match apply(lab, case, &mut vars).await {
        Ok(applied) => applied,
        // (a case whose kind Renoise answers unchanged here: the limit says why, and the answer is checked)
        Err(why) if !case.limit.is_empty() && why.starts_with("unchanged") => {
            let p1 = probe(lab, case, &vars).await;
            out.step("unchanged", same(&p0, &p1), short(&why, 160));
            if out.verdict == Verdict::Pass {
                out.verdict = Verdict::Limit;
                out.note = case.limit.to_owned();
            }
            return out;
        }
        Err(why) => return out.fail("apply", why),
    };
    out.apply_ms = Some(said.ms);
    let p1 = probe(lab, case, &vars).await;
    out.trace.push(("p1", p1.clone()));
    out.step(
        "took",
        !same(&p0, &p1) && row.state == ChangeState::Applied,
        if same(&p0, &p1) { format!("nothing changed: {p1}") } else { String::new() },
    );
    if outside_undo {
        let note = row.note.clone().unwrap_or_default();
        out.step("note", note.contains("Renoise") && note.contains("Kumi"), format!("“{}”", short(&note, 90)));
    }
    // 2. Kumi's undo, read back
    let undo = lab.undo(&row.id).await;
    let p2 = probe(lab, case, &vars).await;
    out.trace.push(("p2", p2.clone()));
    let state = lab.row(&row.id).map(|r| r.state);
    let back = same(&p0, &p2);
    out.step(
        "undo",
        !undo.error && back && state == Some(ChangeState::Undone),
        if back { format!("{} ms", undo.ms) } else { format!("{:?} {} → {p2} (was {p0})", state, short(&undo.text, 120)) },
    );
    if case.tool == "run_lua" {
        return run_lua_case(lab, case, out, vars, p0).await;
    }
    // 3. Again, then Renoise's own undo and redo
    lab.pause().await;
    if outside_undo {
        // (a step Renoise's undo takes instead: this change makes none)
        if let Err(why) = lab.lua("song.artist = 'K sentinel ' .. tostring(os.clock())").await {
            return out.fail("sentinel", why);
        }
        lab.pause().await;
    }
    let (row, _) = match apply(lab, case, &mut vars).await {
        Ok(applied) => applied,
        Err(why) => return out.fail("again", why),
    };
    let mut unsure = false;
    let p3 = probe(lab, case, &vars).await;
    out.trace.push(("p3", p3.clone()));
    if !same(&p1, &p3) {
        out.info("again", format!("took otherwise: {p3}"));
    }
    lab.pause().await;
    let stack = lab.lua("return {song:can_undo(), song:can_redo()}").await.unwrap_or_default();
    // (Renoise keeps no undo step now and then for a whole session: its undo then takes nothing back, which is no case's doing)
    let kept_none = stack[0] == json!(false);
    out.trace.push(("undo stack", stack));
    if let Err(why) = lab.lua("song:undo()").await {
        return out.fail("daw undo", why);
    }
    if outside_undo {
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        let p4 = probe(lab, case, &vars).await;
        out.trace.push(("p4", p4.clone()));
        let state = lab.row(&row.id).map(|r| r.state);
        out.step("daw undo", same(&p3, &p4) && state == Some(ChangeState::Applied), format!("left it ({state:?})"));
        lab.pause().await;
        lab.lua("song:redo()").await.ok();
    } else {
        let told = lab.wait_state(&row.id, ChangeState::Undone, TOLD_MS).await;
        let p4 = probe(lab, case, &vars).await;
        out.trace.push(("p4", p4.clone()));
        let state = told.as_ref().map(|r| r.state);
        let note = told.as_ref().and_then(|r| r.note.clone()).unwrap_or_default();
        out.step(
            "daw undo",
            same(&p0, &p4) && state == Some(ChangeState::Undone),
            match (state == Some(ChangeState::Undone), same(&p0, &p4), same(&p3, &p4)) {
                (true, true, _) => format!("told “{}”", short(&note, 40)),
                (false, true, _) => format!("undone in Renoise, not told ({state:?})"),
                (_, false, true) => format!("Renoise's undo did nothing ({state:?})"),
                _ => format!("{state:?} → {p4}"),
            },
        );
        // Kumi's undo now: undone already, nothing written
        let again = lab.undo(&row.id).await;
        let p4b = probe(lab, case, &vars).await;
        out.trace.push(("p4b", p4b.clone()));
        let state = lab.row(&row.id).map(|r| r.state);
        out.step("undo after", same(&p4, &p4b) && state == Some(ChangeState::Undone), format!("“{}”", short(&again.text, 70)));
        lab.pause().await;
        if let Err(why) = lab.lua("song:redo()").await {
            return out.fail("daw redo", why);
        }
        let told = lab.wait_state(&row.id, ChangeState::Applied, TOLD_MS).await;
        let p5 = probe(lab, case, &vars).await;
        out.trace.push(("p5", p5.clone()));
        let state = told.as_ref().map(|r| r.state);
        // (lines: Renoise doesn't say which step its redo took, so the row says it may have redone it)
        unsure = state == Some(ChangeState::Unsure);
        let told = match state {
            Some(ChangeState::Applied) => "told".to_owned(),
            Some(ChangeState::Unsure) => format!("unsure “{}”", short(&told.and_then(|r| r.note).unwrap_or_default(), 50)),
            _ => format!("{state:?}"),
        };
        let ok = same(&p3, &p5) && matches!(state, Some(ChangeState::Applied | ChangeState::Unsure));
        out.step("daw redo", ok, if same(&p3, &p5) { told } else { format!("{told} → {p5}") });
    }
    // 4. Kumi's undo once more: puts it back, or refuses and says why
    let last = lab.undo(&row.id).await;
    let p6 = probe(lab, case, &vars).await;
    out.trace.push(("p6", p6.clone()));
    let row = lab.row(&row.id);
    let state = row.as_ref().map(|r| r.state);
    match state {
        Some(ChangeState::Undone) => {
            out.step("last undo", same(&p0, &p6), if same(&p0, &p6) { String::new() } else { format!("→ {p6} (was {p0})") })
        }
        // Refused after Renoise's undo and redo: listed as a limit, with what HISTORY says (a case decides whether it's one)
        Some(ChangeState::Kept) => {
            let note = row.and_then(|r| r.note).unwrap_or_default();
            out.step(
                "last undo",
                !note.is_empty(),
                format!("refused{}: “{}”", if unsure { " (after an unsure redo)" } else { "" }, short(&note, 80)),
            );
            // (the bridge's own words, under HISTORY's)
            out.trace.push(("refused", Value::String(last.text.clone())));
            if out.verdict == Verdict::Pass {
                out.verdict = Verdict::Limit;
            }
        }
        other => out.step("last undo", false, format!("{other:?}: {}", short(&last.text, 120))),
    }
    // A known limit of Renoise's fails only the steps after the second apply: the case is that limit.
    let late = ["daw undo", "undo after", "daw redo", "last undo"];
    let failed = |(step, detail): &(&str, String)| detail.starts_with("FAIL") && !late.contains(step);
    if !case.limit.is_empty() && out.verdict == Verdict::Fail && !out.steps.iter().any(failed) {
        out.verdict = Verdict::Limit;
        out.note = case.limit.to_owned();
    } else if kept_none && out.verdict == Verdict::Fail && !out.steps.iter().any(failed) {
        out.verdict = Verdict::Limit;
        out.note = "Renoise kept no undo step for it (can_undo() false before its undo): its undo took nothing back".into();
    }
    out
}

/// run_lua: Kumi's undo refuses (it can't know what Lua did); Renoise's undo takes it back.
async fn run_lua_case(lab: &Lab, case: &Case, mut out: Outcome, mut vars: BTreeMap<String, Value>, p0: Value) -> Outcome {
    // (the undo above was refused: the step's check is redone here)
    if let Some((_, detail)) = out.steps.iter_mut().find(|(step, _)| *step == "undo") {
        let row = lab.rows.borrow().last().cloned();
        let kept = row.as_ref().is_some_and(|r| r.state == ChangeState::Kept);
        *detail =
            format!("refused{}: “{}”", if kept { "" } else { " (row not Kept)" }, short(&row.and_then(|r| r.note).unwrap_or_default(), 80));
    }
    out.verdict = Verdict::Pass;
    lab.pause().await;
    if let Err(why) = lab.lua("song:undo()").await {
        return out.fail("daw undo", why);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let p = probe(lab, case, &vars).await;
    out.step("daw undo", same(&p0, &p), format!("{p}"));
    vars.insert("E".into(), json!(lab.epoch().await));
    out
}

// ---------------------------------------------------------------------------------------------------------------------

fn library_file(root: &Path, folder: &str, extension: &str, prefer: &str) -> Option<String> {
    fn walk(dir: &Path, extension: &str, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, extension, found);
            } else if path.extension().and_then(|e| e.to_str()) == Some(extension) {
                found.push(path);
            }
        }
    }
    let mut found = vec![];
    walk(&root.join(folder), extension, &mut found);
    found.sort();
    let pick = found.iter().find(|p| p.to_string_lossy().contains(prefer)).or(found.first())?;
    Some(pick.to_string_lossy().into_owned())
}

fn markdown(outcomes: &[Outcome]) -> String {
    let mut text = String::from("| Case | Apply | Steps | Result | Note |\n|---|---|---|---|---|\n");
    for o in outcomes {
        let steps: Vec<String> =
            o.steps.iter().map(|(step, detail)| format!("{step}: {}", short(detail, 140).replace('|', "\\|"))).collect();
        let verdict = match o.verdict {
            Verdict::Pass => "pass",
            Verdict::Fail => "**FAIL**",
            Verdict::Limit => "limit",
        };
        let ms = o.apply_ms.map(|ms| format!("{ms} ms")).unwrap_or_default();
        text += &format!("| {} | {ms} | {} | {verdict} | {} |\n", o.name, steps.join("; "), o.note.replace('|', "\\|"));
    }
    text
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(home) = argv.first().filter(|a| !a.starts_with("--")) else {
        eprintln!("accept_renoise <kumi-home> [--only <case,...>] [--big] [--out <results.md>]");
        std::process::exit(2);
    };
    let flag = |name: &str| argv.iter().position(|a| a == name).and_then(|k| argv.get(k + 1)).cloned();
    let only: Option<Vec<String>> = flag("--only").map(|list| list.split(',').map(str::to_owned).collect());
    let big = argv.iter().any(|a| a == "--big");
    let out_path = flag("--out");
    let home = PathBuf::from(home);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    tokio::task::LocalSet::new().block_on(&runtime, async {
        let host = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Renoise, &home)), id: Cell::new(0) });
        let hooked = Rc::new(Hooked { inner: host.clone(), hooks: RefCell::default() });
        let endpoint = hooked.clone();
        let connect: Connect = Rc::new(move |_| {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
        });
        let rows: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
        let told: Rc<RefCell<Vec<String>>> = Rc::default();
        let focus: Rc<RefCell<Vec<(Instant, Option<DawFocus>)>>> = Rc::default();
        let (into_rows, into_told, into_focus) = (rows.clone(), told.clone(), focus.clone());
        let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
        context.on_change = Some(Rc::new(move |record| {
            let mut rows = into_rows.borrow_mut();
            match rows.iter_mut().find(|r| r.id == record.id) {
                Some(row) => *row = record,
                None => rows.push(record),
            }
        }));
        context.on_action = Some(Rc::new(move |event: ActionEvent| into_told.borrow_mut().push(event.title)));
        context.on_focus = Some(Rc::new(move |seen| into_focus.borrow_mut().push((Instant::now(), seen))));
        context.focus_drawn = true;
        let options = OwnOptions { context, kumi_home: home.clone(), connect: Some(connect) };
        let daw = create_renoise_integration(options);
        let t = Instant::now();
        if let Err(error) = daw.start(Signal::new()).await {
            eprintln!("no Renoise: {error:?}");
            std::process::exit(1);
        }
        let observed = daw.observe(Signal::new(), None).await.expect("a look at Renoise");
        println!("== connected and observed in {} ms ({} tools)", t.elapsed().as_millis(), observed.tools.len());
        let tools = observed.tools.iter().map(|tool| (tool.name().to_owned(), tool.clone())).collect();
        let scratch = home.join("accept-renoise");
        std::fs::create_dir_all(&scratch).ok();
        let library = dirs_home().join("Library/Application Support/Renoise/V3.5.4/Library");
        let mut vars = BTreeMap::new();
        vars.insert("dir".to_owned(), json!(scratch.to_string_lossy()));
        for (name, folder, extension, prefer) in [
            ("chain", "Effect Chains", "xrnt", "Bright Vox"),
            ("phrasefile", "Phrases", "xrnz", "Rock 05"),
            ("modsetfile", "Modulation Sets", "xrno", "Autopaned"),
            ("preset", "Effect Presets/Compressor", "xrdp", "Drums Hard"),
        ] {
            match library_file(&library, folder, extension, prefer) {
                Some(path) => vars.insert(name.to_owned(), json!(path)),
                None => vars.insert(name.to_owned(), json!(format!("(no {folder} file)"))),
            };
        }
        let lab = Lab { host, hooked, daw: daw.clone(), tools: RefCell::new(tools), rows, told, focus, vars };
        // Renoise keeps no undo step now and then for a whole session (seen from its start too): a run then proves nothing of its
        // undo, so it stops at once and says so.
        if !big {
            lab.lua("song.artist = 'K undo check'").await.ok();
            lab.pause().await;
            let kept = lab.lua("return song:can_undo()").await;
            lab.lua("song:undo()").await.ok();
            if kept != Ok(json!(true)) {
                eprintln!("Renoise keeps no undo step in this session (can_undo() false after a change): quit and start it again");
                lab.daw.close().await.ok();
                std::process::exit(3);
            }
            lab.pause().await;
        }
        let started = Instant::now();
        let mut outcomes = vec![];
        let wanted = |name: &str| only.as_ref().is_none_or(|only| only.iter().any(|o| name == o || name.starts_with(&format!("{o} "))));

        // A lab probe: Lua chunks (split at lines "--- step"), each in its own callback, a pause apart.
        if let Some(file) = flag("--lua") {
            let text = std::fs::read_to_string(&file).expect("the probe file");
            for (k, chunk) in text.split("\n--- step").enumerate() {
                let t = Instant::now();
                let answer = lab.lua(chunk).await;
                println!("step {}: {} ms: {}", k + 1, t.elapsed().as_millis(), short(&format!("{answer:?}"), 2_000));
                lab.pause().await;
            }
            lab.daw.close().await.ok();
            return;
        }
        // (the big song is measured on its own, in the song Renoise opened)
        if big {
            extras::big(&lab, &mut outcomes).await;
        }
        // Every kind of Kumi's table has a case: one without is the runner's failure.
        if !big {
            println!("== kinds ({} in Kumi's Renoise table)", renoise_changes::CHANGES.kinds.len());
        }
        // (plug-in instruments last: after an AU plug-in loads, Renoise's undo now and then stops taking steps back for the
        // rest of the session, which would fail every later case's Renoise-undo step)
        let plugin = |tool: &str| tool.starts_with("plugin_");
        let kinds = renoise_changes::CHANGES
            .kinds
            .iter()
            .filter(|kind| !plugin(kind.tool))
            .chain(renoise_changes::CHANGES.kinds.iter().filter(|kind| plugin(kind.tool)));
        for kind in kinds.filter(|_| !big) {
            let cases: Vec<&Case> = CASES.iter().filter(|case| case.tool == kind.tool).collect();
            if cases.is_empty() {
                let mut out = Outcome::new(kind.tool.to_owned());
                out.step("case", false, "the runner has no case for this kind");
                println!("{}", out.line());
                outcomes.push(out);
                continue;
            }
            for case in cases {
                if !wanted(&case.name()) {
                    continue;
                }
                let outcome = run_case(&lab, case, kind.skipped_by_daw_undo).await;
                println!("{}", outcome.line());
                outcomes.push(outcome);
            }
        }
        for case in CASES.iter().filter(|case| !renoise_changes::CHANGES.kinds.iter().any(|kind| kind.tool == case.tool)) {
            println!("  (a case for {} names no kind in Kumi's table)", case.tool);
        }
        if !big {
            extras::run(&lab, &wanted, &mut outcomes).await;
        }

        let failed = outcomes.iter().filter(|o| o.verdict == Verdict::Fail).count();
        let limits = outcomes.iter().filter(|o| o.verdict == Verdict::Limit).count();
        let summary = format!(
            "{} cases: {} pass, {failed} fail, {limits} limits, in {} s",
            outcomes.len(),
            outcomes.len() - failed - limits,
            started.elapsed().as_secs()
        );
        println!("== {summary}");
        let table = format!("{summary}\n\n{}", markdown(&outcomes));
        match &out_path {
            Some(path) => std::fs::write(path, table).expect("the results file"),
            None => println!("{table}"),
        }
        let _ = (&lab.told, &lab.daw);
        lab.daw.close().await.ok();
    });
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Actions, run_lua, the focus, edge cases and the big song.
mod extras {
    use super::*;

    /// Lua's answer, or its failure as a value (the step reading it fails there).
    async fn read(lab: &Lab, code: &str) -> Value {
        lab.lua(code).await.unwrap_or_else(|why| json!({"failed": why}))
    }

    async fn fresh(lab: &Lab, code: &str) -> Result<BTreeMap<String, Value>, String> {
        let mut vars = lab.vars.clone();
        if let Value::Object(fields) = lab.lua(&fill_lua(code, &vars)).await? {
            vars.extend(fields);
        }
        vars.insert("E".into(), json!(lab.epoch().await));
        Ok(vars)
    }

    fn input(template: &str, vars: &BTreeMap<String, Value>) -> Value {
        fill_json(&serde_json::from_str(template).expect("JSON"), vars)
    }

    fn done(outcomes: &mut Vec<Outcome>, out: Outcome) {
        println!("{}", out.line());
        outcomes.push(out);
    }

    pub async fn big(lab: &Lab, outcomes: &mut Vec<Outcome>) {
        println!("== big song");
        done(outcomes, big_song(lab).await);
    }

    pub async fn run(lab: &Lab, wanted: &dyn Fn(&str) -> bool, outcomes: &mut Vec<Outcome>) {
        println!("== edge cases before a save");
        if wanted("edge unsaved") {
            done(outcomes, unsaved(lab).await);
        }
        println!("== actions");
        for name in ["play", "stop", "audition_note", "select", "save_song", "save_instrument", "plugin_editor", "save_device_preset"] {
            if wanted(name) {
                done(outcomes, action(lab, name).await);
            }
        }
        println!("== run_lua");
        if wanted("run_lua answers") {
            done(outcomes, run_lua(lab).await);
        }
        println!("== the focus follows Renoise's selection");
        if wanted("selection") {
            done(outcomes, selection(lab).await);
        }
        println!("== edge cases");
        for name in ["edge changed", "edge copy", "edge alias", "edge group", "edge hidden"] {
            if !wanted(name) {
                continue;
            }
            let out = match name {
                "edge changed" => changed(lab).await,
                "edge copy" => copy(lab).await,
                "edge alias" => alias(lab).await,
                "edge group" => group(lab).await,
                _ => hidden(lab).await,
            };
            done(outcomes, out);
        }
        // Last: it leaves another song open.
        if wanted("edge new song") {
            done(outcomes, new_song(lab).await);
        }
    }

    /// An unsaved song: saving without a path is refused, and a delete keeps no copy of a file it hasn't.
    async fn unsaved(lab: &Lab) -> Outcome {
        let mut out = Outcome::new("edge unsaved song".into());
        let file = read(lab, "return song.file_name").await;
        if file.as_str().is_some_and(|f| !f.is_empty()) {
            out.info("song", format!("saved already ({file}): skipped"));
            out.verdict = Verdict::Limit;
            return out;
        }
        let save = lab.tool("save_song", json!({})).await;
        out.step("save refused", save.error, format!("“{}”", short(&save.text, 100)));
        let Ok(vars) = fresh(lab, "return {t = new_track('K unsaved delete')}").await else { return out.fail("setup", "") };
        let (said, added) = lab.plan(json!([{"tool": "track_delete", "input": input(r#"{"ref":"$E:track:$t"}"#, &vars)}])).await;
        out.step("delete, no copy", !said.error && !said.text.contains("\"copy\""), short(&said.text, 100));
        if let Some(row) = added.last() {
            let undo = lab.undo(&row.id).await;
            out.step("undo", !undo.error, short(&undo.text, 60));
        }
        out
    }

    async fn action(lab: &Lab, name: &str) -> Outcome {
        let mut out = Outcome::new(name.into());
        let dir = lab.vars["dir"].as_str().unwrap_or_default().to_owned();
        match name {
            "play" => {
                let said = lab.tool("play", json!({"sequence": 1, "line": 0})).await;
                out.apply_ms = Some(said.ms);
                tokio::time::sleep(Duration::from_millis(300)).await;
                let playing = read(lab, "return song.transport.playing").await;
                out.step("playing", !said.error && playing == json!(true), format!("{} {playing}", short(&said.text, 80)));
            }
            "stop" => {
                let said = lab.tool("stop", json!({})).await;
                out.apply_ms = Some(said.ms);
                tokio::time::sleep(Duration::from_millis(200)).await;
                let playing = read(lab, "return song.transport.playing").await;
                out.step("stopped", !said.error && playing == json!(false), format!("{} {playing}", short(&said.text, 80)));
            }
            "audition_note" => {
                let Ok(vars) = fresh(lab, "return {i = new_instrument('K audition', 1, 22050), t = new_track('K audition')}").await else {
                    return out.fail("setup", "");
                };
                let rows = lab.rows.borrow().len();
                let args = input(
                    r#"{"instrumentRef":"$E:instrument:$i","trackRef":"$E:track:$t","notes":["C-4","E-4","G-4"],"durationMs":400}"#,
                    &vars,
                );
                let said = lab.tool("audition_note", args).await;
                out.apply_ms = Some(said.ms);
                out.step("auditioned", !said.error, short(&said.text, 100));
                tokio::time::sleep(Duration::from_millis(700)).await;
                out.step("no HISTORY row", lab.rows.borrow().len() == rows, "");
            }
            "select" => {
                let Ok(vars) = fresh(lab, "return {t = new_track('K select'), q = #song.sequencer.pattern_sequence}").await else {
                    return out.fail("setup", "");
                };
                // (Renoise's line 04 is its fifth: Lua's index 5)
                let args = input(r#"{"trackRef":"$E:track:$t","sequenceRef":"$E:seq:$q","line":4,"frame":"mixer"}"#, &vars);
                let said = lab.tool("select", args).await;
                out.apply_ms = Some(said.ms);
                tokio::time::sleep(Duration::from_millis(200)).await;
                let now = read(lab, "return {song.selected_track_index, song.selected_sequence_index, song.selected_line_index}").await;
                let want = json!([vars["t"], vars["q"], 5]);
                out.step("selected", !said.error && same(&now, &want), format!("{} {now}", short(&said.text, 80)));
                lab.tool("select", json!({"frame": "pattern"})).await;
            }
            "save_song" => {
                let path = format!("{dir}/accept.xrns");
                std::fs::remove_file(&path).ok();
                let said = lab.tool("save_song", json!({"path": path})).await;
                out.apply_ms = Some(said.ms);
                out.step("saved as", !said.error && Path::new(&path).exists(), short(&said.text, 100));
                tokio::time::sleep(Duration::from_millis(300)).await;
                let file = read(lab, "return song.file_name").await;
                out.step("song is the file", file.as_str() == Some(path.as_str()), format!("{file}"));
                let again = lab.tool("save_song", json!({})).await;
                out.step("saved again", !again.error, short(&again.text, 80));
                let over = lab.tool("save_song", json!({"path": format!("{dir}/k.xrni")})).await;
                out.step("over a file refused", over.error, short(&over.text, 80));
            }
            "save_instrument" => {
                let Ok(vars) = fresh(lab, "return {i = new_instrument('K save', 1)}").await else { return out.fail("setup", "") };
                let path = format!("{dir}/saved.xrni");
                std::fs::remove_file(&path).ok();
                let args = input(r#"{"instrumentRef":"$E:instrument:$i","path":"$dir/saved.xrni"}"#, &vars);
                let said = lab.tool("save_instrument", args.clone()).await;
                out.apply_ms = Some(said.ms);
                out.step("saved", !said.error && Path::new(&path).exists(), short(&said.text, 100));
                let again = lab.tool("save_instrument", args).await;
                out.step("existing refused", again.error, short(&again.text, 80));
            }
            "plugin_editor" => {
                let setup = "local t = new_track('K editor', {'Audio/Effects/Native/Gainer'}) local tr = song:track(t) for _, p in ipairs(tr.available_devices) do if p:find('aufx:dely') then tr:insert_device_at(p, 3) break end end return {t = t, au = #tr.devices == 3}";
                let Ok(vars) = fresh(lab, setup).await else { return out.fail("setup", "") };
                let native = lab.tool("plugin_editor", input(r#"{"ref":"$E:device:$t.2"}"#, &vars)).await;
                out.step("native refused", native.error, short(&native.text, 80));
                if vars["au"] == json!(true) {
                    let shown = lab.tool("plugin_editor", input(r#"{"ref":"$E:device:$t.3","show":true}"#, &vars)).await;
                    out.apply_ms = Some(shown.ms);
                    out.step("shown", !shown.error && shown.text.contains("true"), short(&shown.text, 80));
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let hidden = lab.tool("plugin_editor", input(r#"{"ref":"$E:device:$t.3","show":false}"#, &vars)).await;
                    out.step("hidden", !hidden.error && hidden.text.contains("false"), short(&hidden.text, 80));
                } else {
                    out.info("AU", "no AUDelay here: only the refusal checked");
                }
            }
            _ => {
                let Ok(vars) = fresh(lab, "return {t = new_track('K preset', {'Audio/Effects/Native/Gainer'})}").await else {
                    return out.fail("setup", "");
                };
                let args = input(r#"{"ref":"$E:device:$t.2","name":"Kumi Accept Test"}"#, &vars);
                let said = lab.tool("save_device_preset", args.clone()).await;
                out.apply_ms = Some(said.ms);
                let file = serde_json::from_str::<Value>(&said.text).ok().and_then(|v| v["file"].as_str().map(str::to_owned));
                let exists = file.as_ref().is_some_and(|f| Path::new(f).exists());
                out.step("saved", !said.error && exists, short(&said.text, 120));
                let again = lab.tool("save_device_preset", args).await;
                out.step("existing refused", again.error, short(&again.text, 80));
                if let Some(file) = file.filter(|f| f.contains("Kumi Accept Test")) {
                    std::fs::remove_file(&file).ok();
                }
            }
        }
        out
    }

    /// run_lua's answers: a value and what it printed, an error, a chunk stopped at its time, one that doesn't compile.
    async fn run_lua(lab: &Lab) -> Outcome {
        let mut out = Outcome::new("run_lua (answers)".into());
        let rows = lab.rows.borrow().len();
        let value = lab.tool("run_lua", json!({"code": "print('hello', 1) return {bpm = song.transport.bpm, n = #song.tracks}"})).await;
        out.step("value", !value.error && value.text.contains("bpm") && value.text.contains("hello"), short(&value.text, 100));
        out.step("read: no row", lab.rows.borrow().len() == rows, "");
        let error = lab.tool("run_lua", json!({"code": "error('K boom')"})).await;
        out.step("error", error.text.contains("K boom"), short(&error.text, 100));
        let t = Instant::now();
        let stopped = lab.tool("run_lua", json!({"code": "local n = 0 while true do n = n + 1 end", "timeoutMs": 200})).await;
        let ms = t.elapsed().as_millis();
        out.step("stopped", stopped.text.contains("200 ms") && ms < 3_000, format!("{ms} ms: {}", short(&stopped.text, 80)));
        let compile = lab.tool("run_lua", json!({"code": "this is not lua"})).await;
        out.step("compile", compile.error, short(&compile.text, 80));
        out
    }

    /// The producer selects a track in Renoise: Kumi's focus names it within 200 ms (median of five).
    async fn selection(lab: &Lab) -> Outcome {
        let mut out = Outcome::new("selection → focus".into());
        let Ok(vars) = fresh(lab, "return {a = new_track('K select A'), b = new_track('K select B')}").await else {
            return out.fail("setup", "");
        };
        let mut times = vec![];
        for k in 0..6 {
            let t = if k % 2 == 0 { vars["a"].clone() } else { vars["b"].clone() };
            let wanted = format!("{}:track:{}", vars["E"].as_str().unwrap_or_default(), t);
            let start = Instant::now();
            if let Err(why) = lab.lua(&format!("song.selected_track_index = {t}")).await {
                return out.fail("select", why);
            }
            let mut seen = None;
            while start.elapsed() < Duration::from_millis(2_000) {
                let hit = lab.focus.borrow().iter().rev().find(|(at, _)| *at >= start).and_then(|(at, focus)| {
                    focus.as_ref().filter(|f| f.track_ref.as_deref() == Some(wanted.as_str())).map(|_| at.duration_since(start).as_millis())
                });
                if hit.is_some() {
                    seen = hit;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            match seen {
                Some(ms) if k > 0 => times.push(ms),
                Some(_) => {}
                None => {
                    let last = lab.focus.borrow().last().and_then(|(_, f)| f.as_ref().and_then(|f| f.track_ref.clone()));
                    return out.fail("focus", format!("never named {wanted} (last {last:?})"));
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        times.sort();
        let median = times[times.len() / 2];
        out.step("followed", median <= 200, format!("median {median} ms, all {times:?}"));
        out
    }

    /// The producer changes what a change fenced between Kumi's preview and its apply: refused, nothing written.
    async fn changed(lab: &Lab) -> Outcome {
        let mut out = Outcome::new("edge changed before apply".into());
        let Ok(vars) = fresh(lab, "return {t = new_track('K changed')}").await else { return out.fail("setup", "") };
        let (host, t) = (lab.host.clone(), vars["t"].clone());
        let edit: Hook = Box::new(move || {
            async move {
                lua_on(&host, &format!("song:track({t}).postfx_volume.value = 0.3")).await.ok();
            }
            .boxed_local()
        });
        lab.hooked.before("change_apply", 0, edit);
        let (said, added) =
            lab.plan(json!([{"tool": "track_volume", "input": input(r#"{"ref":"$E:track:$t","volumeDb":-6}"#, &vars)}])).await;
        let now = read(lab, &fill_lua("return song:track($t).postfx_volume.value", &vars)).await;
        out.step("refused", said.error && added.is_empty(), short(&said.text, 140));
        out.step("producer's value kept", same(&now, &json!(0.3)), format!("{now}"));
        out.step("edit made", !lab.hooked.armed(), "");
        out
    }

    /// A delete in a saved song keeps a copy of its file first.
    async fn copy(lab: &Lab) -> Outcome {
        let mut out = Outcome::new("edge saved song: copy before a delete".into());
        let file = read(lab, "return song.file_name").await;
        if file.as_str().is_none_or(str::is_empty) {
            out.info("song", "not saved: skipped (save_song runs first)");
            out.verdict = Verdict::Limit;
            return out;
        }
        // (a new turn looks at Renoise first, and so learns the song's file)
        lab.daw.observe(Signal::new(), None).await.ok();
        let Ok(vars) = fresh(lab, "return {t = new_track('K saved delete')}").await else { return out.fail("setup", "") };
        let said = lab.tool("track_delete", input(r#"{"ref":"$E:track:$t"}"#, &vars)).await;
        let copy = serde_json::from_str::<Value>(&said.text).ok().and_then(|v| v["copy"].as_str().map(str::to_owned));
        out.step("copied", !said.error && copy.as_ref().is_some_and(|c| Path::new(c).exists()), format!("{copy:?}"));
        if let Some(id) = serde_json::from_str::<Value>(&said.text).ok().and_then(|v| v["change"].as_str().map(str::to_owned)) {
            let undo = lab.undo(&id).await;
            out.step("undo", !undo.error, short(&undo.text, 60));
        }
        out
    }

    /// Lines written to an aliased pattern track: Kumi says where they went, and its undo puts them back.
    async fn alias(lab: &Lab) -> Outcome {
        let mut out = Outcome::new("edge aliased pattern".into());
        let setup = "local t = new_track('K alias edge') local _, p = new_slot() local _, p2 = new_slot() put(p, t, 1, 'C-4') song:pattern(p2):track(t).alias_pattern_index = p return {t = t, p = p, p2 = p2}";
        let Ok(vars) = fresh(lab, setup).await else { return out.fail("setup", "") };
        let probe = fill_lua("return {notes($p, $t, 4), song:pattern($p2):track($t).alias_pattern_index}", &vars);
        let before = read(lab, &probe).await;
        lab.pause().await;
        let write = r#"{"patternRef":"$E:pattern:$p2","trackRef":"$E:track:$t","lines":[{"line":2,"notes":[{"col":1,"note":"A-4","instrument":0}]}]}"#;
        let (said, added) = lab.plan(json!([{"tool": "notes_write", "input": input(write, &vars)}])).await;
        let after = read(lab, &probe).await;
        out.info("answer", short(&said.text, 160));
        match added.last() {
            Some(row) => {
                out.step("written", !same(&before, &after), format!("{after}"));
                let undo = lab.undo(&row.id).await;
                let back = read(lab, &probe).await;
                out.step("undo", !undo.error && same(&before, &back), short(&undo.text, 80));
            }
            None => out.step("refused", said.error && same(&before, &after), "nothing written"),
        }
        out
    }

    /// A group track: what only a sequencer track has is refused; a group's volume changes and comes back.
    async fn group(lab: &Lab) -> Outcome {
        let mut out = Outcome::new("edge group track".into());
        let setup = "local t = new_track('K grouped') local g = master_index() song:insert_group_at(g).name = 'K group edge' song:add_track_to_group(t, g) return {g = named_track('K group edge')}";
        let Ok(vars) = fresh(lab, setup).await else { return out.fail("setup", "") };
        let (columns, _) =
            lab.plan(json!([{"tool": "track_columns", "input": input(r#"{"ref":"$E:track:$g","noteColumns":2}"#, &vars)}])).await;
        out.step("note columns refused", columns.error, short(&columns.text, 100));
        let (delete, _) = lab.plan(json!([{"tool": "track_delete", "input": input(r#"{"ref":"$E:track:$g"}"#, &vars)}])).await;
        out.step("delete refused", delete.error, short(&delete.text, 100));
        let probe = fill_lua("return song:track($g).postfx_volume.value", &vars);
        let before = read(lab, &probe).await;
        let (said, added) =
            lab.plan(json!([{"tool": "track_volume", "input": input(r#"{"ref":"$E:track:$g","volumeDb":-4}"#, &vars)}])).await;
        let after = read(lab, &probe).await;
        out.step("volume", !said.error && !same(&before, &after), format!("{after}"));
        if let Some(row) = added.last() {
            lab.undo(&row.id).await;
            let back = read(lab, &probe).await;
            out.step("undo", same(&before, &back), format!("{back}"));
        }
        out
    }

    /// Notes in a column the track hides: a transpose of it shows the column, and its undo hides it again.
    async fn hidden(lab: &Lab) -> Outcome {
        let mut out = Outcome::new("edge hidden columns".into());
        let setup = "local t = new_track('K hidden') song:track(t).visible_note_columns = 2 put(1, t, 1, 'C-4', 2) song:track(t).visible_note_columns = 1 return {t = t}";
        let Ok(vars) = fresh(lab, setup).await else { return out.fail("setup", "") };
        let probe = fill_lua("return {song:track($t).visible_note_columns, notes(1, $t, 2, 2)}", &vars);
        let before = read(lab, &probe).await;
        lab.pause().await;
        let transpose = r#"{"patternRef":"$E:pattern:1","trackRef":"$E:track:$t","semitones":3,"columns":[2]}"#;
        let (said, added) = lab.plan(json!([{"tool": "notes_transpose", "input": input(transpose, &vars)}])).await;
        let after = read(lab, &probe).await;
        out.step("transposed, shown", !said.error && after[0] == json!(2), format!("{after} {}", short(&said.text, 80)));
        if let Some(row) = added.last() {
            let undo = lab.undo(&row.id).await;
            let back = read(lab, &probe).await;
            out.step("undo", !undo.error && same(&before, &back), format!("{back}"));
        }
        out
    }

    /// Renoise opens a new song between two steps of a plan: the second is refused, and the first's undo says the song
    /// changed. (The song is saved first, so the new one replaces it without asking.)
    async fn new_song(lab: &Lab) -> Outcome {
        let mut out = Outcome::new("edge new song mid-plan".into());
        let Ok(vars) = fresh(lab, "return {a = new_track('K plan A'), b = new_track('K plan B')}").await else {
            return out.fail("setup", "");
        };
        let host = lab.host.clone();
        let path = format!("{}/accept-before-new.xrns", lab.vars["dir"].as_str().unwrap_or_default());
        std::fs::remove_file(&path).ok();
        let switch: Hook = Box::new(move || {
            async move {
                lua_on(&host, &format!("renoise.app():save_song_as('{path}')")).await.ok();
                tokio::time::sleep(Duration::from_millis(1_000)).await;
                lua_on(&host, "renoise.app():new_song()").await.ok();
                tokio::time::sleep(Duration::from_millis(2_000)).await;
            }
            .boxed_local()
        });
        lab.hooked.before("change_preview", 1, switch);
        let steps = json!([
            {"tool": "track_rename", "input": input(r#"{"ref":"$E:track:$a","name":"K plan A renamed"}"#, &vars)},
            {"tool": "track_rename", "input": input(r#"{"ref":"$E:track:$b","name":"K plan B renamed"}"#, &vars)},
        ]);
        let (said, added) = lab.plan(steps).await;
        out.info("answer", short(&said.text, 200));
        out.step("first made", added.len() == 1, format!("{} rows", added.len()));
        let tracks = read(lab, "return #song.tracks").await;
        out.info("new song", format!("{tracks} tracks"));
        if let Some(row) = added.first() {
            let undo = lab.undo(&row.id).await;
            let state = lab.row(&row.id).map(|r| r.state);
            out.step("undo refused", state == Some(ChangeState::Kept), format!("{state:?} “{}”", short(&undo.text, 100)));
        }
        out
    }

    /// 64 tracks × 100 patterns: Kumi's look, reads, a few changes and their undos, timed.
    async fn big_song(lab: &Lab) -> Outcome {
        let mut out = Outcome::new("big: 64 tracks × 100 patterns".into());
        let t = Instant::now();
        let build = "local n = 0 while master_index() - 1 < 64 do n = n + 1 song:insert_track_at(master_index()).name = 'K big ' .. n end local seq = song.sequencer while #song.patterns < 100 do seq:insert_new_pattern_at(#seq.pattern_sequence + 1) end return master_index() - 1";
        let first = read(lab, build).await;
        out.info("built", format!("{first} tracks, 100 patterns in {} ms", t.elapsed().as_millis()));
        // 16 notes a pattern track, 10 patterns a callback
        let t = Instant::now();
        for from in (1..=100).step_by(10) {
            let code = format!(
                "for p = {from}, {} do for t = 1, 64 do local pt = song:pattern(p):track(t) for l = 1, 64, 4 do local c = pt:line(l):note_column(1) c.note_value = 36 + (t + l + p) % 48 c.instrument_value = 0 end end end return true",
                from + 9
            );
            if let Err(why) = lab.lua(&code).await {
                return out.fail("notes", why);
            }
        }
        out.info("notes", format!("102,400 notes in {} ms", t.elapsed().as_millis()));
        let t = Instant::now();
        let look = lab.daw.observe(Signal::new(), None).await;
        out.step("look", look.is_ok(), format!("{} ms", t.elapsed().as_millis()));
        let t = Instant::now();
        let (mut pages, mut cursor) = (0, None::<String>);
        loop {
            let mut args = json!({"kind": "track", "limit": 50});
            if let Some(c) = &cursor {
                args["cursor"] = json!(c);
            }
            let page = lab.tool("renoise_read", args).await;
            pages += 1;
            cursor = serde_json::from_str::<Value>(&page.text).ok().and_then(|v| v["nextCursor"].as_str().map(str::to_owned));
            if cursor.is_none() || pages > 20 {
                break;
            }
        }
        out.info("tracks read", format!("{pages} pages in {} ms", t.elapsed().as_millis()));
        let epoch = lab.epoch().await;
        let t = Instant::now();
        let lines = lab
            .tool("renoise_read", json!({"kind": "line", "parent": format!("{epoch}:pattern:50"), "trackRef": format!("{epoch}:track:32")}))
            .await;
        out.step("lines read", !lines.error, format!("{} ms", t.elapsed().as_millis()));
        let t = Instant::now();
        let seq = lab.tool("renoise_read", json!({"kind": "sequence", "limit": 100})).await;
        out.step("sequence read", !seq.error, format!("{} ms", t.elapsed().as_millis()));
        let mut vars = lab.vars.clone();
        for (name, template) in [
            ("volume", r#"{"tool":"track_volume","input":{"ref":"$E:track:60","volumeDb":-3}}"#),
            ("transpose", r#"{"tool":"notes_transpose","input":{"patternRef":"$E:pattern:50","trackRef":"$E:track:32","semitones":1}}"#),
            ("track add", r#"{"tool":"track_add","input":{"index":1,"name":"K big added"}}"#),
            ("clone", r#"{"tool":"slot_clone","input":{"ref":"$E:seq:50"}}"#),
        ] {
            vars.insert("E".into(), json!(lab.epoch().await));
            let (said, added) = lab.plan(json!([input(template, &vars)])).await;
            let Some(row) = added.last() else {
                out.step(name, false, short(&said.text, 100));
                continue;
            };
            let undo = lab.undo(&row.id).await;
            let back = lab.row(&row.id).map(|r| r.state) == Some(ChangeState::Undone);
            out.step(name, !said.error && back, format!("apply {} ms, undo {} ms", said.ms, undo.ms));
        }
        out
    }
}
