//! A track's settings beyond its fader (M4, r-tracks): polarity, record arm, record input, monitoring and record mode,
//! the send to its parent, the FX chain switch, pan mode, width, dual pan and pan law, locked controls, solo defeat, where
//! it shows, a folder's collapse and its MIDI hardware output.
//!
//! Each setting is read back after it is written, and put back by value: only the fields the change touched, so a field
//! the producer changed beside it since stays theirs. The fence is the whole setting as the preview read it.

use serde_json::{json, Map, Value};

use super::{track_ref_of, Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::Track;
use crate::refs::{self, TrackId};

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

/// A track setting Kumi changes. A state is an object of REAPER's numbers (and booleans) by name; `wanted` names only the
/// fields the change asks for, and only those are compared, written and put back.
pub struct Prop {
    pub kind: &'static str,
    /// The master track has it too.
    master: bool,
    read: fn(&Ctx, Track) -> Value,
    /// The fields the input asks for, with the track as it is now (its pan mode, its parent, the devices REAPER has).
    wanted: fn(&Ctx, Track, &Value) -> Done<Value>,
    write: fn(&Ctx, Track, &Value) -> bool,
    /// The change's title, with the track's name: "Kick polarity inverted".
    title: fn(&Ctx, &str, &Value) -> String,
    /// What the producer should know beside it (the pan mode it switched to).
    note: fn(&Ctx, Track, &Value, &Value) -> Option<String>,
    noun: &'static str,
}

fn no_note(_: &Ctx, _: Track, _: &Value, _: &Value) -> Option<String> {
    None
}

fn num(state: &Value, key: &str) -> f64 {
    state.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

fn int(state: &Value, key: &str) -> i64 {
    state.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn bit(on: bool) -> f64 {
    if on {
        1.0
    } else {
        0.0
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// Whether `a` holds what `b` says, for every field `b` names.
fn same(a: &Value, b: &Value) -> bool {
    let Some(fields) = b.as_object() else { return false };
    fields.iter().all(|(key, want)| match (a.get(key), want) {
        (Some(have), Value::Number(want)) => have.as_f64().zip(want.as_f64()).is_some_and(|(x, y)| close(x, y)),
        (Some(have), want) => have == want,
        (None, _) => false,
    })
}

/// `state`'s values of the fields `of` names.
fn touched(state: &Value, of: &Value) -> Value {
    let mut fields = Map::new();
    for key in of.as_object().into_iter().flatten().map(|(key, _)| key) {
        if let Some(value) = state.get(key) {
            fields.insert(key.clone(), value.clone());
        }
    }
    Value::Object(fields)
}

fn boolean(input: &Value, key: &str) -> Done<bool> {
    input.get(key).and_then(Value::as_bool).ok_or_else(|| bad(format!("{key} must be true or false")))
}

fn maybe_bool(input: &Value, key: &str) -> Done<Option<bool>> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad(format!("{key} must be true or false"))),
    }
}

fn word<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

fn set(ctx: &Ctx, track: Track, parm: &str, value: f64) -> bool {
    ctx.reaper.set_track_value(track, parm, value)
}

fn value(ctx: &Ctx, track: Track, parm: &str) -> f64 {
    ctx.reaper.track_value(track, parm)
}

fn percent(value: f64) -> String {
    format!("{}%", (value * 100.0).round() as i64)
}

// ----- the record input, as the header encodes it -----

/// `I_RECINPUT` in words: "no input", "audio input 1", "audio inputs 1/2", "MIDI from all devices, channel 3".
fn input_words(ctx: &Ctx, code: i64) -> String {
    if code < 0 {
        return "no input".into();
    }
    if code & 4096 != 0 {
        let channel = code & 31;
        let device = (code >> 5) & 63;
        let from = match device {
            63 => "all MIDI devices".to_string(),
            62 => "the virtual MIDI keyboard".to_string(),
            n => {
                let name = ctx.reaper.midi_inputs().into_iter().find(|(dev, _)| *dev == n as i32).map(|(_, name)| name);
                name.map_or(format!("MIDI device {n}"), |name| super::quoted(&name))
            }
        };
        let channel = if channel == 0 { "all channels".to_string() } else { format!("channel {channel}") };
        return format!("{from}, {channel}");
    }
    let first = (code & 1023) + 1;
    if code & 2048 != 0 {
        format!("audio inputs from {first} (as many as the track has channels)")
    } else if code & 1024 != 0 {
        format!("audio inputs {first}/{}", first + 1)
    } else {
        format!("audio input {first}")
    }
}

/// The audio inputs REAPER has, in words, for a refusal.
fn audio_inputs_words(ctx: &Ctx) -> String {
    match ctx.reaper.audio_inputs() {
        0 => "REAPER's audio device has no inputs (or none is open)".to_string(),
        n => {
            let names: Vec<String> =
                (0..n.min(8)).map(|i| format!("{} {}", i + 1, super::quoted(&ctx.reaper.audio_input_name(i)))).collect();
            let more = if n > 8 { format!(" and {} more", n - 8) } else { String::new() };
            format!("REAPER's audio device has {n} input(s): {}{more}", names.join(", "))
        }
    }
}

fn midi_inputs_words(ctx: &Ctx) -> String {
    let present: Vec<String> = ctx
        .reaper
        .midi_inputs()
        .into_iter()
        .filter(|(dev, _)| *dev < 62)
        .map(|(dev, name)| format!("{dev} {}", super::quoted(&name)))
        .collect();
    if present.is_empty() {
        "REAPER has no MIDI input devices here (\"all\" and \"virtual-keyboard\" are always there)".to_string()
    } else {
        format!("REAPER's MIDI inputs: {}; or \"all\", or \"virtual-keyboard\"", present.join(", "))
    }
}

fn record_input_code(ctx: &Ctx, track: Track, input: &Value) -> Done<i64> {
    let shape = "input must be {kind: \"none\"}, {kind: \"audio\", first (from 1), layout?: \"mono\" | \"stereo\" | \"multichannel\"} or {kind: \"midi\", device?: \"all\" | \"virtual-keyboard\" | a device number, channel?: 0 (all) to 16}";
    let input = input.get("input").filter(|v| v.is_object()).ok_or_else(|| bad(shape))?;
    match word(input, "kind") {
        Some("none") => Ok(-1),
        Some("audio") => {
            let first = input.get("first").and_then(Value::as_i64).filter(|n| (1..=512).contains(n)).ok_or_else(|| bad(shape))?;
            let (flag, count) = match word(input, "layout").unwrap_or("mono") {
                "mono" => (0, 1),
                "stereo" => (1024, 2),
                "multichannel" => (2048, value(ctx, track, "I_NCHAN").max(2.0) as i64),
                _ => return Err(bad(shape)),
            };
            let inputs = ctx.reaper.audio_inputs() as i64;
            if first - 1 + count > inputs {
                return Err(bad(format!("{}: there is no audio input {}", audio_inputs_words(ctx), first - 1 + count)));
            }
            Ok((first - 1) | flag)
        }
        Some("midi") => {
            let device = match input.get("device") {
                None | Some(Value::Null) => 63,
                Some(v) if v == "all" => 63,
                Some(v) if v == "virtual-keyboard" => 62,
                Some(v) => {
                    let n = v.as_i64().filter(|n| (0..62).contains(n)).ok_or_else(|| bad(shape))?;
                    if !ctx.reaper.midi_inputs().iter().any(|(dev, _)| *dev as i64 == n) {
                        return Err(bad(format!("there is no MIDI input {n}: {}", midi_inputs_words(ctx))));
                    }
                    n
                }
            };
            let channel = match input.get("channel") {
                None | Some(Value::Null) => 0,
                Some(v) => v.as_i64().filter(|n| (0..=16).contains(n)).ok_or_else(|| bad(shape))?,
            };
            Ok(4096 | channel | (device << 5))
        }
        _ => Err(bad(shape)),
    }
}

// ----- names of REAPER's numbers -----

const MONITORING: [(&str, i64, &str); 3] =
    [("off", 0, "monitoring off"), ("on", 1, "monitoring on"), ("auto", 2, "monitoring on when not playing (tape style)")];

const RECORD_MODES: [(&str, i64, &str); 9] = [
    ("input", 0, "records its input"),
    ("stereo-out", 1, "records its output (stereo)"),
    ("none", 2, "records nothing (monitoring only)"),
    ("stereo-out-compensated", 3, "records its output (stereo, latency compensated)"),
    ("midi-out", 4, "records its MIDI output"),
    ("mono-out", 5, "records its output (mono)"),
    ("mono-out-compensated", 6, "records its output (mono, latency compensated)"),
    ("midi-overdub", 7, "records MIDI overdubbing"),
    ("midi-replace", 8, "records MIDI replacing"),
];

const TAPS: [(&str, i64); 3] = [("post-fader", 0), ("pre-fx", 1), ("post-fx", 2)];

const PAN_MODES: [(&str, i64, &str); 5] = [
    ("default", -1, "the project's default pan mode"),
    ("classic", 0, "classic (REAPER 3.x) balance mode"),
    ("balance", 3, "balance mode"),
    ("stereo", 5, "stereo pan mode (pan and width)"),
    ("dual", 6, "dual pan mode"),
];

const TAPERS: [(&str, i64); 3] = [("sine", 0), ("hybrid", 3), ("linear", 2)];

fn named(table: &[(&str, i64, &str)], input: &Value, key: &str) -> Done<Option<i64>> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) => table.iter().find(|(n, _, _)| n == name).map(|(_, v, _)| Some(*v)).ok_or_else(|| {
            bad(format!("{key} must be one of {}", table.iter().map(|(n, _, _)| format!("\"{n}\"")).collect::<Vec<_>>().join(", ")))
        }),
        Some(_) => Err(bad(format!("{key} must be a name"))),
    }
}

fn words_of(table: &[(&str, i64, &str)], code: i64) -> String {
    table.iter().find(|(_, v, _)| *v == code).map_or_else(|| format!("REAPER's setting {code}"), |(_, _, words)| words.to_string())
}

fn tap(input: &Value) -> Done<Option<i64>> {
    match input.get("tap") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => TAPS
            .iter()
            .find(|(name, _)| v == name)
            .map(|(_, code)| Some(*code))
            .ok_or_else(|| bad("tap must be \"post-fader\", \"pre-fx\" or \"post-fx\"")),
    }
}

/// The pan mode the track pans in: its own, or the project's default when it follows that (-1).
fn effective_pan_mode(ctx: &Ctx, track: Track) -> i64 {
    match value(ctx, track, "I_PANMODE") as i64 {
        -1 => ctx.reaper.config_text("panmode").and_then(|text| text.trim().parse::<i64>().ok()).unwrap_or(3),
        mode => mode,
    }
}

fn mode_name(mode: i64) -> &'static str {
    PAN_MODES.iter().find(|(_, v, _)| *v == mode).map_or("other", |(name, _, _)| name)
}

/// The pan mode a width or dual pan needs: the track's already (no change), or the one the input switches it to.
fn needed_mode(ctx: &Ctx, track: Track, input: &Value, needs: i64, what: &str) -> Done<Option<i64>> {
    if effective_pan_mode(ctx, track) == needs {
        return Ok(None);
    }
    let name = mode_name(needs);
    match word(input, "panMode") {
        Some(asked) if asked == name => Ok(Some(needs)),
        Some(_) => Err(bad(format!("panMode must be \"{name}\": {what} is a setting of {}", words_of(&PAN_MODES, needs)))),
        None => Err(bad(format!(
            "{} pans in {}, which has no {what}: pass panMode \"{name}\" to switch it to {} in the same change, or set its pan mode first (track_pan_mode)",
            super::quoted(&super::track_name(ctx.reaper, track)),
            words_of(&PAN_MODES, effective_pan_mode(ctx, track)),
            words_of(&PAN_MODES, needs),
        ))),
    }
}

fn pan_mode_note(_: &Ctx, _: Track, now: &Value, wanted: &Value) -> Option<String> {
    let mode = wanted.get("panMode")?.as_i64()?;
    (now.get("panMode").and_then(Value::as_i64) != Some(mode))
        .then(|| format!("It now pans in {}; its pan law is unchanged.", words_of(&PAN_MODES, mode)))
}

/// The pan law, as the producer says it: "−3 dB", "−3 dB with gain compensation", "the project's default".
fn pan_law_words(law: f64) -> String {
    if law < 0.0 {
        return "the project's default".into();
    }
    if law > 1.0 + 1e-9 {
        format!("{} dB with gain compensation", super::tracks::db_text(-20.0 * law.log10()))
    } else {
        format!("{} dB", super::tracks::db_text(20.0 * law.log10()))
    }
}

/// What a track sends to its parent reaches: (the parent's name, its channel count).
fn parent_of(ctx: &Ctx, track: Track) -> Option<(String, i64)> {
    let parent = ctx.reaper.track_parent(track).or_else(|| ctx.reaper.master_track(ctx.proj))?;
    Some((super::track_name(ctx.reaper, parent), value(ctx, parent, "I_NCHAN").max(2.0) as i64))
}

fn span(first: i64, count: i64) -> String {
    match count {
        1 => format!("{first}"),
        2 => format!("{first}/{}", first + 1),
        n => format!("{first}\u{2013}{}", first + n - 1),
    }
}

/// The tracks selected now (by id, the master among them), to put back after an action that works on a selection.
fn selection(ctx: &Ctx) -> (Vec<String>, bool) {
    let reaper = ctx.reaper;
    let tracks = (0..reaper.track_count(ctx.proj))
        .filter_map(|at| reaper.track_at(ctx.proj, at))
        .filter(|t| reaper.track_value(*t, "I_SELECTED") != 0.0)
        .map(|t| reaper.track_guid(t))
        .collect();
    let master = reaper.master_track(ctx.proj).is_some_and(|m| reaper.track_value(m, "I_SELECTED") != 0.0);
    (tracks, master)
}

fn put_selection(ctx: &Ctx, (tracks, master): &(Vec<String>, bool)) {
    let reaper = ctx.reaper;
    for at in 0..reaper.track_count(ctx.proj) {
        if let Some(t) = reaper.track_at(ctx.proj, at) {
            reaper.set_track_selected(t, tracks.contains(&reaper.track_guid(t)));
        }
    }
    if let Some(m) = reaper.master_track(ctx.proj) {
        reaper.set_track_selected(m, *master);
    }
}

/// Runs a main-section action on this one track, the producer's selection put back after.
fn on_track(ctx: &Ctx, track: Track, command: i32) -> bool {
    let kept = selection(ctx);
    ctx.reaper.select_only(track);
    let ran = ctx.reaper.run_action(command);
    put_selection(ctx, &kept);
    ran
}

/// Whether a track's controls are locked: the `LOCK 1` line of its state (in the lighter state REAPER keeps for undo too:
/// 7.82 lab).
fn locked(ctx: &Ctx, track: Track) -> bool {
    let Some(chunk) = ctx.reaper.track_undo_chunk(track) else { return false };
    let mut depth = 0i32;
    for line in chunk.lines() {
        let line = line.trim();
        if line.starts_with('<') {
            depth += 1;
            continue;
        }
        if line == ">" {
            depth -= 1;
            continue;
        }
        if depth == 1 {
            if let Some(rest) = line.strip_prefix("LOCK ") {
                return rest.split_whitespace().next().is_some_and(|flag| flag.parse::<i64>().is_ok_and(|n| n & 1 != 0));
            }
        }
    }
    false
}

/// Track: Lock track controls, and unlock.
const LOCK_CONTROLS: i32 = 41312;
const UNLOCK_CONTROLS: i32 = 41313;

// ----- the settings -----

pub static PROPS: [Prop; 15] = [
    Prop {
        kind: "track-phase",
        master: true,
        read: |ctx, track| json!({"inverted": value(ctx, track, "B_PHASE") != 0.0}),
        wanted: |_, _, input| Ok(json!({"inverted": boolean(input, "inverted")?})),
        write: |ctx, track, state| state.get("inverted").is_none_or(|v| set(ctx, track, "B_PHASE", bit(v == true))),
        title: |_, name, to| format!("{name} polarity {}", if to["inverted"] == true { "inverted" } else { "back to normal" }),
        note: no_note,
        noun: "polarity",
    },
    Prop {
        kind: "track-arm",
        master: false,
        read: |ctx, track| json!({"arm": value(ctx, track, "I_RECARM") != 0.0}),
        wanted: |_, _, input| Ok(json!({"arm": boolean(input, "arm")?})),
        write: |ctx, track, state| state.get("arm").is_none_or(|v| set(ctx, track, "I_RECARM", bit(v == true))),
        title: |_, name, to| format!("{name} {}", if to["arm"] == true { "armed for recording" } else { "record arm off" }),
        note: no_note,
        noun: "record arm",
    },
    Prop {
        kind: "track-record-input",
        master: false,
        read: |ctx, track| json!({"input": value(ctx, track, "I_RECINPUT") as i64}),
        wanted: |ctx, track, input| Ok(json!({"input": record_input_code(ctx, track, input)?})),
        write: |ctx, track, state| state.get("input").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "I_RECINPUT", v)),
        title: |ctx, name, to| format!("{name} records from {}", input_words(ctx, int(to, "input"))),
        note: no_note,
        noun: "record input",
    },
    Prop {
        kind: "track-monitoring",
        master: false,
        read: |ctx, track| json!({"monitoring": value(ctx, track, "I_RECMON") as i64, "items": value(ctx, track, "I_RECMONITEMS") != 0.0}),
        wanted: |_, _, input| {
            let mut wanted = json!({});
            if let Some(mode) = named(&MONITORING, input, "monitoring")? {
                wanted["monitoring"] = mode.into();
            }
            if let Some(items) = maybe_bool(input, "items")? {
                wanted["items"] = items.into();
            }
            if wanted.as_object().is_some_and(Map::is_empty) {
                return Err(bad("give monitoring (\"off\", \"on\" or \"auto\") or items (true or false)"));
            }
            Ok(wanted)
        },
        write: |ctx, track, state| {
            state.get("monitoring").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "I_RECMON", v))
                && state.get("items").is_none_or(|v| set(ctx, track, "I_RECMONITEMS", bit(v == true)))
        },
        title: |_, name, to| {
            let mut parts = Vec::new();
            if let Some(mode) = to.get("monitoring").and_then(Value::as_i64) {
                parts.push(words_of(&MONITORING, mode));
            }
            if let Some(items) = to.get("items").and_then(Value::as_bool) {
                parts.push(if items { "monitors items while recording".into() } else { "doesn't monitor items while recording".into() });
            }
            format!("{name} {}", parts.join(", "))
        },
        note: no_note,
        noun: "monitoring",
    },
    Prop {
        kind: "track-record-mode",
        master: false,
        read: |ctx, track| json!({"mode": value(ctx, track, "I_RECMODE") as i64, "tap": value(ctx, track, "I_RECMODE_FLAGS") as i64 & 3}),
        wanted: |_, _, input| {
            let mode = named(&RECORD_MODES, input, "mode")?;
            let tap = tap(input)?;
            let mut wanted = json!({});
            if let Some(mode) = mode {
                wanted["mode"] = mode.into();
            }
            if let Some(tap) = tap {
                wanted["tap"] = tap.into();
            }
            if wanted.as_object().is_some_and(Map::is_empty) {
                return Err(bad("give mode (\"input\", \"stereo-out\", …) or tap (where an output is recorded)"));
            }
            Ok(wanted)
        },
        write: |ctx, track, state| {
            let mode = state.get("mode").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "I_RECMODE", v));
            // The tap is the low two bits of the flags; the rest are left as they are.
            let tap = state.get("tap").and_then(Value::as_i64).is_none_or(|tap| {
                let flags = value(ctx, track, "I_RECMODE_FLAGS") as i64;
                set(ctx, track, "I_RECMODE_FLAGS", ((flags & !3) | (tap & 3)) as f64)
            });
            mode && tap
        },
        title: |_, name, to| {
            let mut words = to.get("mode").and_then(Value::as_i64).map(|mode| words_of(&RECORD_MODES, mode)).unwrap_or_default();
            if let Some(tap) = to.get("tap").and_then(Value::as_i64) {
                let tap = TAPS.iter().find(|(_, code)| *code == tap).map_or("", |(name, _)| *name);
                words = if words.is_empty() { format!("records outputs {tap}") } else { format!("{words}, {tap}") };
            }
            format!("{name} {words}")
        },
        note: no_note,
        noun: "record mode",
    },
    Prop {
        kind: "track-main-send",
        master: false,
        read: |ctx, track| {
            json!({
                "on": value(ctx, track, "B_MAINSEND") != 0.0,
                "offset": value(ctx, track, "C_MAINSEND_OFFS") as i64,
                "channels": value(ctx, track, "C_MAINSEND_NCH") as i64,
            })
        },
        wanted: |ctx, track, input| {
            let mut wanted = json!({});
            if let Some(on) = maybe_bool(input, "on")? {
                wanted["on"] = on.into();
            }
            match input.get("offset") {
                None | Some(Value::Null) => {}
                Some(v) => {
                    let first = v
                        .as_i64()
                        .filter(|n| (1..=128).contains(n))
                        .ok_or_else(|| bad("offset must be the parent's first channel, from 1 to 128"))?;
                    wanted["offset"] = (first - 1).into();
                }
            }
            match input.get("channels") {
                None | Some(Value::Null) => {}
                Some(v) if v == "all" => wanted["channels"] = 0.into(),
                Some(v) => {
                    let count = v
                        .as_i64()
                        .filter(|n| *n == 1 || (2..=128).contains(n) && n % 2 == 0)
                        .ok_or_else(|| bad("channels must be \"all\", 1, or an even number from 2 to 128"))?;
                    wanted["channels"] = count.into();
                }
            }
            if wanted.as_object().is_some_and(Map::is_empty) {
                return Err(bad("give on (true or false), offset (the parent's first channel) or channels (\"all\" or a count)"));
            }
            // The parent must have the channels the send reaches (REAPER drops what goes past them, unheard).
            if wanted.get("offset").is_some() || wanted.get("channels").is_some() {
                let offset = wanted.get("offset").and_then(Value::as_i64).unwrap_or(value(ctx, track, "C_MAINSEND_OFFS") as i64);
                let count = match wanted.get("channels").and_then(Value::as_i64).unwrap_or(value(ctx, track, "C_MAINSEND_NCH") as i64) {
                    0 => value(ctx, track, "I_NCHAN").max(2.0) as i64,
                    n => n,
                };
                if let Some((parent, channels)) = parent_of(ctx, track) {
                    if offset + count > channels {
                        return Err(bad(format!(
                            "{} has {channels} channels, and the send to it would reach channel {}: raise its channels first (track_channels)",
                            super::quoted(&parent),
                            offset + count
                        )));
                    }
                }
            }
            Ok(wanted)
        },
        write: |ctx, track, state| {
            state.get("on").is_none_or(|v| set(ctx, track, "B_MAINSEND", bit(v == true)))
                && state.get("offset").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "C_MAINSEND_OFFS", v))
                && state.get("channels").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "C_MAINSEND_NCH", v))
        },
        title: |_, name, to| {
            let mut parts = Vec::new();
            match to.get("on").and_then(Value::as_bool) {
                Some(true) => parts.push("sends to its parent".to_string()),
                Some(false) => parts.push("no longer sends to its parent".to_string()),
                None => {}
            }
            if to.get("offset").is_some() || to.get("channels").is_some() {
                let first = to.get("offset").and_then(Value::as_i64).map(|o| o + 1);
                let count = to.get("channels").and_then(Value::as_i64);
                let what = match count {
                    Some(0) => "all its channels".to_string(),
                    Some(n) => format!("{n} channel(s)"),
                    None => "its send".to_string(),
                };
                let to_where = first.map_or(String::new(), |first| match count {
                    Some(n) if n > 0 => format!(" to the parent's channels {}", span(first, n)),
                    _ => format!(" from the parent's channel {first}"),
                });
                parts.push(format!("sends {what}{to_where}"));
            }
            format!("{name} {}", parts.join(", "))
        },
        note: no_note,
        noun: "parent send",
    },
    Prop {
        kind: "track-fx-enabled",
        master: true,
        read: |ctx, track| json!({"enabled": value(ctx, track, "I_FXEN") != 0.0}),
        wanted: |_, _, input| Ok(json!({"enabled": boolean(input, "enabled")?})),
        write: |ctx, track, state| state.get("enabled").is_none_or(|v| set(ctx, track, "I_FXEN", bit(v == true))),
        title: |_, name, to| format!("{name} FX chain {}", if to["enabled"] == true { "on" } else { "bypassed" }),
        note: no_note,
        noun: "FX chain switch",
    },
    Prop {
        kind: "track-pan-mode",
        master: true,
        read: |ctx, track| json!({"panMode": value(ctx, track, "I_PANMODE") as i64}),
        wanted: |_, _, input| Ok(json!({"panMode": named(&PAN_MODES, input, "mode")?.ok_or_else(|| bad("mode must be a pan mode"))?})),
        write: |ctx, track, state| state.get("panMode").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "I_PANMODE", v)),
        title: |_, name, to| format!("{name} pans in {}", words_of(&PAN_MODES, int(to, "panMode"))),
        note: |_, _, now, wanted| {
            (now.get("panMode") != wanted.get("panMode"))
                .then(|| "Its pan law is unchanged; its pan values keep their numbers and mean what they mean in this mode.".to_string())
        },
        noun: "pan mode",
    },
    Prop {
        kind: "track-width",
        master: true,
        // (the pan mode it pans in now, the project's when it follows that, is fenced: it decides what the change writes)
        read: |ctx, track| json!({"panMode": value(ctx, track, "I_PANMODE") as i64, "panModeNow": effective_pan_mode(ctx, track), "width": value(ctx, track, "D_WIDTH")}),
        wanted: |ctx, track, input| {
            let width = input
                .get("width")
                .and_then(Value::as_f64)
                .filter(|w| (-1.0..=1.0).contains(w))
                .ok_or_else(|| bad("width must be a number from -1 to 1 (1 is the full width, 0 mono, -1 swapped)"))?;
            let mut wanted = json!({"width": width});
            if let Some(mode) = needed_mode(ctx, track, input, 5, "width")? {
                wanted["panMode"] = mode.into();
            }
            Ok(wanted)
        },
        write: |ctx, track, state| {
            state.get("panMode").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "I_PANMODE", v))
                && state.get("width").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "D_WIDTH", v))
        },
        title: |_, name, to| format!("{name} width {}", percent(num(to, "width"))),
        note: pan_mode_note,
        noun: "width",
    },
    Prop {
        kind: "track-dual-pan",
        master: true,
        read: |ctx, track| json!({"panMode": value(ctx, track, "I_PANMODE") as i64, "panModeNow": effective_pan_mode(ctx, track), "left": value(ctx, track, "D_DUALPANL"), "right": value(ctx, track, "D_DUALPANR")}),
        wanted: |ctx, track, input| {
            let side = |key: &str| -> Done<Option<f64>> {
                match input.get(key) {
                    None | Some(Value::Null) => Ok(None),
                    Some(v) => v
                        .as_f64()
                        .filter(|p| (-1.0..=1.0).contains(p))
                        .map(Some)
                        .ok_or_else(|| bad(format!("{key} must be a pan from -1 to 1"))),
                }
            };
            let mut wanted = json!({});
            if let Some(left) = side("left")? {
                wanted["left"] = left.into();
            }
            if let Some(right) = side("right")? {
                wanted["right"] = right.into();
            }
            if wanted.as_object().is_some_and(Map::is_empty) {
                return Err(bad("give left, right or both: where each channel pans, -1 to 1"));
            }
            if let Some(mode) = needed_mode(ctx, track, input, 6, "dual pan")? {
                wanted["panMode"] = mode.into();
            }
            Ok(wanted)
        },
        write: |ctx, track, state| {
            state.get("panMode").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "I_PANMODE", v))
                && state.get("left").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "D_DUALPANL", v))
                && state.get("right").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "D_DUALPANR", v))
        },
        title: |_, name, to| {
            let mut parts = Vec::new();
            if let Some(left) = to.get("left").and_then(Value::as_f64) {
                parts.push(format!("left {}", super::tracks::pan_text(left)));
            }
            if let Some(right) = to.get("right").and_then(Value::as_f64) {
                parts.push(format!("right {}", super::tracks::pan_text(right)));
            }
            format!("{name} dual pan {}", parts.join(", "))
        },
        note: pan_mode_note,
        noun: "dual pan",
    },
    Prop {
        kind: "track-pan-law",
        master: true,
        read: |ctx, track| json!({"panLaw": value(ctx, track, "D_PANLAW"), "taper": value(ctx, track, "I_PANLAW_FLAGS") as i64}),
        wanted: |_, _, input| {
            let mut wanted = json!({});
            match input.get("law") {
                None | Some(Value::Null) => {}
                Some(v) if v == "default" => wanted["panLaw"] = (-1.0).into(),
                Some(v) => {
                    let db = v.as_f64().filter(|db| (-12.0..=0.0).contains(db)).ok_or_else(|| {
                        bad("law must be \"default\" or the level at the centre in dB, from -12 to 0 (0, -2.5, -3, -4.5, -6)")
                    })?;
                    let compensated = maybe_bool(input, "compensated")?.unwrap_or(false);
                    wanted["panLaw"] = if compensated { 10f64.powf(-db / 20.0) } else { 10f64.powf(db / 20.0) }.into();
                }
            }
            if let Some(taper) = input.get("taper").filter(|v| !v.is_null()) {
                let code = TAPERS
                    .iter()
                    .find(|(name, _)| taper == name)
                    .map(|(_, code)| *code)
                    .ok_or_else(|| bad("taper must be \"sine\", \"hybrid\" or \"linear\""))?;
                wanted["taper"] = code.into();
            }
            if wanted.as_object().is_some_and(Map::is_empty) {
                return Err(bad("give law (\"default\" or dB at the centre, with compensated) or taper"));
            }
            Ok(wanted)
        },
        write: |ctx, track, state| {
            state.get("panLaw").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "D_PANLAW", v))
                && state.get("taper").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "I_PANLAW_FLAGS", v))
        },
        title: |_, name, to| {
            let mut parts = Vec::new();
            if let Some(law) = to.get("panLaw").and_then(Value::as_f64) {
                parts.push(format!("pan law {}", pan_law_words(law)));
            }
            if let Some(taper) = to.get("taper").and_then(Value::as_i64) {
                parts.push(format!("{} taper", TAPERS.iter().find(|(_, code)| *code == taper).map_or("other", |(name, _)| *name)));
            }
            format!("{name} {}", parts.join(", "))
        },
        note: no_note,
        noun: "pan law",
    },
    Prop {
        kind: "track-lock",
        master: false,
        read: |ctx, track| json!({"locked": locked(ctx, track), "arm": value(ctx, track, "I_RECARM") != 0.0}),
        // REAPER's lock disarms the track for recording, and a locked track takes no arm (7.82 lab): locking an armed track
        // disarms it as part of the change, so its undo unlocks it and arms it again.
        wanted: |ctx, track, input| {
            let lock = boolean(input, "locked")?;
            let mut wanted = json!({"locked": lock});
            if lock && !locked(ctx, track) && value(ctx, track, "I_RECARM") != 0.0 {
                wanted["arm"] = false.into();
            }
            Ok(wanted)
        },
        write: |ctx, track, state| {
            let arm = |ok: bool| ok && state.get("arm").and_then(Value::as_bool).is_none_or(|on| set(ctx, track, "I_RECARM", bit(on)));
            match state.get("locked").and_then(Value::as_bool) {
                Some(true) => arm(true) && on_track(ctx, track, LOCK_CONTROLS),
                Some(false) => arm(on_track(ctx, track, UNLOCK_CONTROLS)),
                None => arm(true),
            }
        },
        title: |_, name, to| {
            let lock = if to["locked"] == true { "locked" } else { "unlocked" };
            match to.get("arm").and_then(Value::as_bool) {
                Some(false) => format!("{name} controls {lock}, record arm off"),
                Some(true) => format!("{name} controls {lock}, armed for recording"),
                None => format!("{name} controls {lock}"),
            }
        },
        note: |_, _, now, wanted| {
            (wanted.get("arm") == Some(&Value::Bool(false)) && now["arm"] == true)
                .then(|| "REAPER's lock disarms the track for recording; undo unlocks it and arms it again.".to_string())
        },
        noun: "control lock",
    },
    Prop {
        kind: "track-solo-defeat",
        master: false,
        read: |ctx, track| json!({"defeat": value(ctx, track, "B_SOLO_DEFEAT") != 0.0}),
        wanted: |_, _, input| Ok(json!({"defeat": boolean(input, "defeat")?})),
        write: |ctx, track, state| state.get("defeat").is_none_or(|v| set(ctx, track, "B_SOLO_DEFEAT", bit(v == true))),
        title: |_, name, to| format!("{name} solo defeat {}", if to["defeat"] == true { "on" } else { "off" }),
        note: no_note,
        noun: "solo defeat",
    },
    Prop {
        kind: "track-visible",
        master: false,
        read: |ctx, track| json!({"arrange": value(ctx, track, "B_SHOWINTCP") != 0.0, "mixer": value(ctx, track, "B_SHOWINMIXER") != 0.0}),
        wanted: |_, _, input| {
            let mut wanted = json!({});
            if let Some(arrange) = maybe_bool(input, "arrange")? {
                wanted["arrange"] = arrange.into();
            }
            if let Some(mixer) = maybe_bool(input, "mixer")? {
                wanted["mixer"] = mixer.into();
            }
            if wanted.as_object().is_some_and(Map::is_empty) {
                return Err(bad("give arrange, mixer or both (true shows the track there)"));
            }
            Ok(wanted)
        },
        write: |ctx, track, state| {
            state.get("arrange").is_none_or(|v| set(ctx, track, "B_SHOWINTCP", bit(v == true)))
                && state.get("mixer").is_none_or(|v| set(ctx, track, "B_SHOWINMIXER", bit(v == true)))
        },
        title: |_, name, to| {
            let place = |key: &str, words: &str| {
                to.get(key).and_then(Value::as_bool).map(|shown| format!("{} in the {words}", if shown { "shown" } else { "hidden" }))
            };
            let parts: Vec<String> = [place("arrange", "arrange view"), place("mixer", "mixer")].into_iter().flatten().collect();
            format!("{name} {}", parts.join(", "))
        },
        note: no_note,
        noun: "visibility",
    },
    Prop {
        kind: "track-folder-state",
        master: false,
        read: |ctx, track| json!({"folder": value(ctx, track, "I_FOLDERDEPTH") as i64 == 1, "collapsed": value(ctx, track, "I_FOLDERCOMPACT") as i64}),
        wanted: |ctx, track, input| {
            if value(ctx, track, "I_FOLDERDEPTH") as i64 != 1 {
                return Err(bad(format!(
                    "{} isn't a folder: only a folder opens and collapses",
                    super::quoted(&super::track_name(ctx.reaper, track))
                )));
            }
            let state = match word(input, "state") {
                Some("open") => 0,
                Some("collapsed") => 1,
                Some("fully-collapsed") => 2,
                _ => return Err(bad("state must be \"open\", \"collapsed\" or \"fully-collapsed\"")),
            };
            Ok(json!({"collapsed": state}))
        },
        write: |ctx, track, state| state.get("collapsed").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "I_FOLDERCOMPACT", v)),
        title: |_, name, to| {
            let words = match int(to, "collapsed") {
                0 => "open",
                1 => "collapsed",
                _ => "fully collapsed",
            };
            format!("Folder {} {words}", super::quoted(name))
        },
        note: no_note,
        noun: "folder state",
    },
];

/// The track's MIDI hardware output: kept apart, it is routing (see `routing.rs`'s words for hardware outputs).
pub static MIDI_OUT: Prop = Prop {
    kind: "track-midi-out",
    master: false,
    read: |ctx, track| json!({"midiOut": value(ctx, track, "I_MIDIHWOUT") as i64}),
    wanted: |ctx, _, input| {
        let device = match input.get("device") {
            Some(v) if v == "none" => return Ok(json!({"midiOut": -1})),
            Some(v) => {
                v.as_i64().filter(|n| (0..32).contains(n)).ok_or_else(|| bad("device must be \"none\" or a MIDI output's number"))?
            }
            None => return Err(bad("device must be \"none\" or a MIDI output's number")),
        };
        let outputs = ctx.reaper.midi_outputs();
        if !outputs.iter().any(|(dev, _)| *dev as i64 == device) {
            let present: Vec<String> = outputs.iter().map(|(dev, name)| format!("{dev} {}", super::quoted(name))).collect();
            let words = if present.is_empty() {
                "REAPER has no MIDI output devices here".to_string()
            } else {
                format!("REAPER's MIDI outputs: {}", present.join(", "))
            };
            return Err(bad(format!("there is no MIDI output {device}: {words}")));
        }
        let channel = match input.get("channel") {
            None | Some(Value::Null) => 0,
            Some(v) => v.as_i64().filter(|n| (0..=16).contains(n)).ok_or_else(|| bad("channel must be 0 (as played) or 1 to 16"))?,
        };
        Ok(json!({"midiOut": (device << 5) | channel}))
    },
    write: |ctx, track, state| state.get("midiOut").and_then(Value::as_f64).is_none_or(|v| set(ctx, track, "I_MIDIHWOUT", v)),
    title: |ctx, name, to| {
        let code = int(to, "midiOut");
        if code < 0 {
            return format!("{name} sends no MIDI to hardware");
        }
        let device = (code >> 5) & 31;
        let label = ctx
            .reaper
            .midi_outputs()
            .into_iter()
            .find(|(dev, _)| *dev as i64 == device)
            .map_or(format!("MIDI output {device}"), |(_, n)| super::quoted(&n));
        let channel = match code & 31 {
            0 => "as played".to_string(),
            n => format!("on channel {n}"),
        };
        format!("{name} sends MIDI to {label} {channel}")
    },
    note: no_note,
    noun: "MIDI hardware output",
};

/// The setting a kind names.
pub fn prop(kind: &str) -> Option<&'static Prop> {
    PROPS.iter().chain(std::iter::once(&MIDI_OUT)).find(|prop| prop.kind == kind)
}

struct Found {
    track: Track,
    id: TrackId,
}

fn find(ctx: &Ctx, input: &Value, prop: &Prop) -> Done<Found> {
    let id = track_ref_of(input, prop.kind)?;
    let track = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &id)?;
    if id == TrackId::Master && !prop.master {
        return Err(bad(format!("the master track has no {}", prop.noun)));
    }
    Ok(Found { track, id })
}

fn label(ctx: &Ctx, found: &Found) -> String {
    super::track_name(ctx.reaper, found.track)
}

fn target(id: &TrackId) -> Value {
    json!({"ref": refs::track_ref(id), "kind": "track"})
}

pub fn preview(ctx: &Ctx, prop: &Prop, input: &Value) -> Done<Preview> {
    let found = find(ctx, input, prop)?;
    let now = (prop.read)(ctx, found.track);
    let wanted = (prop.wanted)(ctx, found.track, input)?;
    let name = label(ctx, &found);
    let mut display = json!({"title": (prop.title)(ctx, &name, &wanted), "before": touched(&now, &wanted), "after": wanted});
    if same(&now, &wanted) {
        display["unchanged"] = true.into();
    }
    if let Some(note) = (prop.note)(ctx, found.track, &now, &wanted) {
        display["note"] = note.into();
    }
    Ok(Preview { target: target(&found.id), prior: touched(&now, &wanted), fence: now, display })
}

pub fn apply(ctx: &Ctx, prop: &Prop, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find(ctx, input, prop).map_err(|fault| {
        if fault.0 == "failed" {
            changed(format!("{}: it went since the preview", fault.1))
        } else {
            fault
        }
    })?;
    let name = label(ctx, &found);
    let now = (prop.read)(ctx, found.track);
    if !same(&now, fence) {
        return Err(changed(format!("{name}'s {} changed since the preview", prop.noun)));
    }
    let wanted = (prop.wanted)(ctx, found.track, input)?;
    let unchanged = same(&now, &wanted);
    if !unchanged && !(prop.write)(ctx, found.track, &wanted) {
        return Err(failed(format!("REAPER wouldn't take the {}", prop.noun)));
    }
    let after = (prop.read)(ctx, found.track);
    if !same(&after, &wanted) {
        // What was written is put back, inside this call: nothing half-done stays.
        let back = (prop.write)(ctx, found.track, &touched(&now, &wanted)) && same(&(prop.read)(ctx, found.track), &touched(&now, &wanted));
        let why = if prop.kind != "track-lock" && locked(ctx, found.track) {
            " (its controls are locked in REAPER: unlock them first)"
        } else {
            ""
        };
        return Err(if back {
            failed(format!("REAPER kept {name}'s {} as it was{why}", prop.noun))
        } else {
            (
                "uncertain".into(),
                format!("REAPER took {name}'s {} otherwise than asked, and Kumi could not put it back: read the track", prop.noun),
            )
        });
    }
    let mut display =
        json!({"title": (prop.title)(ctx, &name, &wanted), "before": touched(&now, &wanted), "after": touched(&after, &wanted)});
    if let Some(note) = (prop.note)(ctx, found.track, &now, &wanted) {
        display["note"] = note.into();
    }
    Ok(Applied { applied: touched(&after, &wanted), created: None, material: None, display, unwritten: unchanged })
}

pub fn restore(ctx: &Ctx, prop: &Prop, restore: &Restore) -> Done<Restored> {
    let reference = restore.target.get("ref").cloned().unwrap_or(Value::Null);
    let found = find(ctx, &json!({"ref": reference}), prop).map_err(|(code, message)| {
        if code == "failed" {
            ("gone".to_string(), message)
        } else {
            (code, message)
        }
    })?;
    let name = label(ctx, &found);
    let now = (prop.read)(ctx, found.track);
    // Already as it was: a retry of a restore that went through, or the producer put it back.
    let already = same(&now, restore.prior);
    if !already {
        if !restore.discard && !same(&now, restore.applied) {
            return Err(changed(format!("{name}'s {} changed in REAPER since Kumi set it", prop.noun)));
        }
        if !(prop.write)(ctx, found.track, restore.prior) {
            return Err(failed(format!("REAPER wouldn't put back the {}", prop.noun)));
        }
        if !same(&(prop.read)(ctx, found.track), restore.prior) {
            return Err(failed(format!("REAPER kept {name}'s {} as it was", prop.noun)));
        }
    }
    let after = touched(&(prop.read)(ctx, found.track), restore.prior);
    Ok(Restored {
        restored: after.clone(),
        display: json!({"title": format!("{} (put back)", (prop.title)(ctx, &name, restore.prior)), "before": touched(&now, restore.prior), "after": after}),
        unwritten: already,
    })
}
