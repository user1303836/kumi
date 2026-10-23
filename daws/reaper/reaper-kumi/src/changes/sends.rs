//! Sends between tracks: adding and removing one, and changing its volume, pan, mute and mode. REAPER gives a send no
//! identity of its own, so a send is found by its ref, which holds only while the same number of sends go to the same
//! track (see `refs`); a restore finds it the same way and refuses `changed` if that number moved.

use serde_json::{json, Value};

use super::{Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Reaper, Track};
use crate::refs::{self, Ref, SendEnd, TrackId};
use crate::rows::{self, SendKey};

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

/// A send of the track a ref names (the sending side; REAPER's category 0).
struct Found {
    track: Track,
    index: i32,
    receiver: Track,
    from: String,
    to: String,
}

/// A send as the titles say it: `“Bass” → “Reverb”`.
fn route_text(from: &str, to: &str) -> String {
    format!("{} → {}", super::quoted(from), super::quoted(to))
}

/// A send as HISTORY draws it (for the display only: no undo reads it): its ends, whether Kumi made, removed or changed it,
/// and each setting it moved (all of them for one made or removed), before and after, in REAPER's words.
pub(super) fn drawn_route(from: &str, to: &str, before: Option<&Value>, after: Option<&Value>) -> Value {
    let change = match (before, after) {
        (None, _) => "made",
        (_, None) => "removed",
        _ => "changed",
    };
    let number = |state: Option<&Value>, key: &str| state.and_then(|state| state.get(key)).and_then(Value::as_f64);
    let flag = |state: Option<&Value>, key: &str| match state.and_then(|state| state.get(key)) {
        Some(Value::Bool(on)) => Some(*on),
        Some(value) => value.as_f64().map(|n| n != 0.0),
        None => None,
    };
    let moved = |key: &str| change != "changed" || before.and_then(|b| b.get(key)) != after.and_then(|a| a.get(key));
    let mut values = vec![];
    if moved("volume") {
        let db = |state: Option<&Value>| number(state, "volume").map(rows::db);
        let (from, to) = (db(before), db(after));
        if from.is_some() || to.is_some() {
            let text = |db: Option<f64>| db.map(|db| format!("{} dB", super::tracks::db_text(db)));
            values.push(json!({"name": "Volume", "from": from.map(|db| db.max(-60.0)), "to": to.map(|db| db.max(-60.0)), "min": -60.0, "max": 12.0, "fromText": text(from), "toText": text(to)}));
        }
    }
    if moved("pan") {
        let (from, to) = (number(before, "pan"), number(after, "pan"));
        if from.is_some() || to.is_some() {
            values.push(json!({"name": "Pan", "from": from, "to": to, "min": -1.0, "max": 1.0, "fromText": from.map(super::tracks::pan_text), "toText": to.map(super::tracks::pan_text)}));
        }
    }
    if moved("mode") {
        let mode = |state: Option<&Value>| number(state, "mode").map(|mode| rows::send_mode_name(mode as i64));
        if let Some(to) = mode(after).or_else(|| mode(before)) {
            values.push(json!({"name": "Taps", "fromText": mode(before).filter(|_| after.is_some()), "toText": to}));
        }
    }
    for (key, name, on, off) in
        [("mute", "Mute", "muted", "playing"), ("phase", "Phase", "inverted", "normal"), ("mono", "Mono", "mono", "stereo")]
    {
        let words = |state: Option<&Value>| flag(state, key).map(|set| if set { on } else { off });
        if moved(key) && (change == "changed" || flag(after.or(before), key) == Some(true)) {
            if let Some(to) = words(after).or_else(|| words(before)) {
                values.push(json!({"name": name, "fromText": words(before).filter(|_| after.is_some()), "toText": to}));
            }
        }
    }
    json!({"from": from, "to": to, "change": change, "values": values})
}

fn name_of(reaper: &Reaper, track: Track) -> String {
    let name = rows::clip(&reaper.track_name(track));
    if name.is_empty() {
        format!("Track {}", reaper.track_value(track, "IP_TRACKNUMBER") as i64)
    } else {
        name
    }
}

fn track_by_id(ctx: &Ctx, id: &TrackId) -> Option<Track> {
    ctx.reads.track_by_id(ctx.reaper, ctx.proj, id).ok()
}

/// (r-tracks) A receive is the send seen from the receiving track (REAPER keeps one object for both; the k-th receive of a
/// track from another is the k-th send between them, 7.82 lab): its ref is turned into that send's, which every change
/// works on, so its stamp and seal are the send's. A hardware output has kinds of its own.
pub(super) fn canonical(reference: &str) -> Done<String> {
    match Ref::parse(reference).map_err(bad)? {
        Ref::Send { track, category: -1, other: SendEnd::Track(sender), repeat, of, seen } => {
            Ok(refs::send_ref(&sender, 0, &SendEnd::Track(track), repeat, of, seen.as_deref()))
        }
        Ref::Send { category: 0, .. } => Ok(reference.to_string()),
        Ref::Send { .. } => Err(bad("that is a hardware output: change it with the hw_out kinds")),
        other => Err(bad(format!("ref must be a send or receive ref (r:send:…), not r:{}", other.kind()))),
    }
}

/// Finds the send a ref names: its track, and where it is in that track's sends now. A ref to one of two or more sends to
/// the same track names it only while those sends are as the ref saw them, or as Kumi's own writes have made them since
/// (`check`): once the producer has added, taken away or changed one, the place may name another send.
fn find(ctx: &Ctx, reference: &str, check: bool) -> Done<Found> {
    let reference = &canonical(reference)?;
    let Ref::Send { track, category, other, repeat, of, seen } = Ref::parse(reference).map_err(bad)? else {
        return Err(bad("ref must be a send ref (r:send:…)"));
    };
    if category != 0 {
        return Err(bad("pass the send from the sending track's side (r:send:{sender}:0:{receiver}), not a receive or a hardware output"));
    }
    let SendEnd::Track(other) = other else { return Err(bad("ref must be a send to a track")) };
    let reaper = ctx.reaper;
    let owner = ctx.reads.track_by_id(reaper, ctx.proj, &track)?;
    let key = SendKey { other: SendEnd::Track(other.clone()), repeat, of };
    let keys = rows::send_keys(reaper, owner, 0);
    let Some(index) = keys.iter().position(|k| *k == key) else {
        let now = keys.iter().filter(|k| k.other == key.other).count();
        return Err(failed(match now {
            0 => "that send isn't there (deleted, or it went elsewhere)".to_string(),
            n => format!("the track has {n} such send(s) now, and this ref was made when there were {of}: read them again"),
        }));
    };
    let receiver = reaper.send_peer(owner, 0, index as i32).ok_or_else(|| failed("that send has no receiving track"))?;
    let found = Found { track: owner, index: index as i32, receiver, from: name_of(reaper, owner), to: name_of(reaper, receiver) };
    if check && of >= 2 && !seen.is_some_and(|seen| written_since(reaper, owner, receiver, &seen)) {
        return Err(changed(format!(
            "the sends {} changed in REAPER since that ref was read (one added, taken away or changed): read them again",
            route_text(&found.from, &found.to)
        )));
    }
    Ok(found)
}

// ----- the sends to one track as Kumi left them -----

/// How many of Kumi's own writes are kept for a pair of tracks, and for how many pairs.
const WRITES_KEPT: usize = 64;
const PAIRS_KEPT: usize = 256;

/// The sender's and the receiver's ids → the pair's state before and after each of Kumi's writes, as refs carry it, oldest
/// first.
type Written = std::collections::HashMap<(String, String), Vec<(String, String)>>;

thread_local! {
    /// What Kumi's own writes did to the sends between two tracks: a ref read before them still names its send when only
    /// Kumi has written since.
    static WRITTEN: std::cell::RefCell<Written> = Default::default();
}

/// The pair's state as a ref carries it.
fn pair_seen(reaper: &Reaper, owner: Track, receiver: Track) -> String {
    rows::pair_hash(reaper, owner, receiver)[..8].to_string()
}

/// Kumi wrote to the sends from `owner` to `receiver`, which were `before`.
fn note_written(reaper: &Reaper, owner: Track, receiver: Track, before: String) {
    let after = pair_seen(reaper, owner, receiver);
    if after == before {
        return;
    }
    let pair = (reaper.track_guid(owner), reaper.track_guid(receiver));
    WRITTEN.with(|written| {
        let mut written = written.borrow_mut();
        if written.len() >= PAIRS_KEPT && !written.contains_key(&pair) {
            written.clear();
        }
        let writes = written.entry(pair).or_default();
        writes.push((before, after));
        if writes.len() > WRITES_KEPT {
            writes.remove(0);
        }
    });
}

/// Runs a write Kumi makes to send `index` of `owner` elsewhere (its automation mode, its envelopes shown or hidden) and
/// notes what it did to the sends between the two tracks, as this module's own writes are: a ref to one of several
/// sends between them, read before, still names its send after.
pub(super) fn noting_writes<T>(reaper: &Reaper, owner: Track, index: i32, write: impl FnOnce() -> T) -> T {
    let Some(receiver) = reaper.send_peer(owner, 0, index) else { return write() };
    let before = pair_seen(reaper, owner, receiver);
    let done = write();
    note_written(reaper, owner, receiver, before);
    done
}

/// Whether a ref to one of the sends from `owner` to `receiver` still names the send it was read as: those sends are as the ref
/// saw them (`seen`), or as Kumi's own writes made them since. A ref to one of two or more without `seen` names none.
pub(crate) fn seen_holds(reaper: &Reaper, owner: Track, receiver: Track, seen: Option<&str>) -> bool {
    seen.is_some_and(|seen| written_since(reaper, owner, receiver, seen))
}

/// Whether the sends from `owner` to `receiver` are as `seen`, or became what they are now from it by Kumi's writes only.
fn written_since(reaper: &Reaper, owner: Track, receiver: Track, seen: &str) -> bool {
    let now = pair_seen(reaper, owner, receiver);
    if now == seen {
        return true;
    }
    let pair = (reaper.track_guid(owner), reaper.track_guid(receiver));
    WRITTEN.with(|written| {
        let written = written.borrow();
        let Some(writes) = written.get(&pair) else { return false };
        let mut reached = vec![seen.to_string()];
        let mut at = 0;
        while at < reached.len() {
            for (before, after) in writes {
                if *before == reached[at] && !reached.contains(after) {
                    reached.push(after.clone());
                }
            }
            at += 1;
        }
        reached.contains(&now)
    })
}

// ----- which send it is -----

/// Where Kumi writes its mark on a send it makes: REAPER gives a send no identity of its own, and a ref is only a place
/// among the sends to one track.
pub(super) const STAMP: &str = "P_EXT:kumi_id";

fn stamp_of(reaper: &Reaper, track: Track, index: i32) -> Option<String> {
    reaper.send_text(track, 0, index, STAMP)
}

/// The send from `owner` to `receiver` with Kumi's mark: `None` when there is none. Only a send to that track is taken (the
/// producer's copy of the receiving track brings a copy of the mark with it), and two with the mark are refused `changed`:
/// which of them is Kumi's can't be told.
fn find_by_stamp(reaper: &Reaper, owner: Track, receiver: Track, stamp: &str) -> Done<Option<i32>> {
    let mut marked = (0..reaper.send_count(owner, 0))
        .filter(|&index| reaper.send_peer(owner, 0, index) == Some(receiver) && stamp_of(reaper, owner, index).as_deref() == Some(stamp));
    let first = marked.next();
    if first.is_some() && marked.next().is_some() {
        let route = route_text(&name_of(reaper, owner), &name_of(reaper, receiver));
        return Err(changed(format!("two sends {route} carry the mark of Kumi's send (one was copied): Kumi can't tell which is its own")));
    }
    Ok(first)
}

pub(super) fn fresh_stamp() -> String {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let count = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{:x}{:04x}{:x}", nanos, std::process::id() & 0xffff, count)
}

fn pair_hash(reaper: &Reaper, owner: Track, receiver: Track) -> String {
    rows::pair_hash(reaper, owner, receiver)
}

/// The states of the other sends from one track to the same one, as one hash: with the send's own channels, what tells
/// the send at a place from another when its own values may have been changed.
fn others_hash(reaper: &Reaper, owner: Track, receiver: Track, index: i32) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for other in 0..reaper.send_count(owner, 0) {
        if other == index || reaper.send_peer(owner, 0, other) != Some(receiver) {
            continue;
        }
        for parm in rows::SEND_STATE {
            for byte in reaper.send_value(owner, 0, other, parm).to_bits().to_le_bytes() {
                hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        for byte in rows::send_envelope_print(reaper, owner, other).bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash = (hash ^ 0xff).wrapping_mul(0x0000_0100_0000_01b3);
    }
    for parm in ["I_SRCCHAN", "I_DSTCHAN", "I_MIDIFLAGS"] {
        for byte in reaper.send_value(owner, 0, index, parm).to_bits().to_le_bytes() {
            hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// How many sends go from one track to another.
fn pair_count(reaper: &Reaper, owner: Track, receiver: Track) -> usize {
    (0..reaper.send_count(owner, 0)).filter(|&index| reaper.send_peer(owner, 0, index) == Some(receiver)).count()
}

/// What tells this send from every other: the mark Kumi made it with, or else its whole state and the states of the
/// sends beside it. A look at a send takes one; a later write or undo holds the send to it.
fn seal(reaper: &Reaper, found: &Found) -> Value {
    json!({
        "stamp": stamp_of(reaper, found.track, found.index),
        "state": send_state(reaper, found.track, found.index),
        "pair": pair_hash(reaper, found.track, found.receiver),
        "others": others_hash(reaper, found.track, found.receiver, found.index),
    })
}

/// The send a ref names, and held to a seal: one with a stamp is found by it wherever the producer has moved it among the
/// sends; any other is the one at the place the ref says only while it is in the state the seal has and has the same
/// sends beside it (`check`), else it may be another send, which is not written.
fn find_sealed(ctx: &Ctx, reference: &str, seal: &Value, check: bool) -> Done<Found> {
    let reaper = ctx.reaper;
    let reference = &canonical(reference)?;
    if let Some(stamp) = seal["stamp"].as_str() {
        let Ref::Send { track, other, .. } = Ref::parse(reference).map_err(bad)? else {
            return Err(bad("ref must be a send ref (r:send:…)"));
        };
        let owner = ctx.reads.track_by_id(reaper, ctx.proj, &track)?;
        let gone = || failed("that send isn't there (deleted, or it went elsewhere)");
        let SendEnd::Track(other) = other else { return Err(bad("ref must be a send to a track")) };
        let receiver = track_by_id(ctx, &other).ok_or_else(gone)?;
        let index = find_by_stamp(reaper, owner, receiver, stamp)?.ok_or_else(gone)?;
        return Ok(Found { track: owner, index, receiver, from: name_of(reaper, owner), to: name_of(reaper, receiver) });
    }
    // (A seal holds the sends as the preview saw them, after the ref was checked there: it is the stricter check.)
    let found = find(ctx, reference, check && !seal.is_object())?;
    // (an undo told to put the values back whatever they are now still writes only to the send it changed: the others to that
    // track, and the send's own channels, are as they were)
    if !check && seal.is_object() && seal.get("stamp").is_none_or(Value::is_null) {
        let same = seal["others"].as_str().is_some_and(|others| others == others_hash(reaper, found.track, found.receiver, found.index));
        if !same {
            return Err(changed(format!(
                "the send {} at that place isn't the one Kumi changed (the sends between those tracks changed since): Kumi won't write to another send, even to undo",
                route_text(&found.from, &found.to)
            )));
        }
    }
    if check && seal.is_object() {
        let route = route_text(&found.from, &found.to);
        let now = send_state(reaper, found.track, found.index);
        // (its envelopes tell it from a send with the same settings put in its place: where the seal has them)
        let envelopes_moved = seal["state"].get("envelopes").is_some_and(|was| *was != now["envelopes"]);
        if !same_state(&now, &seal["state"]) || envelopes_moved {
            return Err(changed(format!("the send {route} is not as it was: it may be another send now")));
        }
        if pair_hash(reaper, found.track, found.receiver) != seal["pair"] {
            return Err(changed(format!(
                "the sends {route} have changed (one added, taken away or changed): the ref may name another send now"
            )));
        }
    }
    Ok(found)
}

fn find_for_apply(ctx: &Ctx, reference: &str, seal: &Value) -> Done<Found> {
    find_sealed(ctx, reference, seal, true).map_err(|fault| {
        if fault.0 == "failed" {
            changed(format!("{}: it changed since the preview", fault.1))
        } else {
            fault
        }
    })
}

fn find_for_restore(ctx: &Ctx, reference: &str, seal: &Value, check: bool) -> Done<Found> {
    find_sealed(ctx, reference, seal, check).map_err(|fault| if fault.0 == "failed" { ("gone".to_string(), fault.1) } else { fault })
}

fn target(reference: &str) -> Value {
    json!({"ref": reference, "kind": "send"})
}

// ----- a send's attributes -----

/// A check of the wanted state against the send's tracks: (sender, receiver).
type Check = fn(&Reaper, Track, Track, &Value) -> Done<()>;

pub struct Prop {
    pub kind: &'static str,
    read: fn(&Reaper, Track, i32) -> Value,
    wanted: fn(&Value) -> Done<Value>,
    write: fn(&Reaper, Track, i32, &Value) -> bool,
    same: fn(&Value, &Value) -> bool,
    title: fn(&str, &Value) -> String,
    noun: &'static str,
    /// What the tracks at both ends must allow for the wanted state (the channels a send uses).
    check: Option<Check>,
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// (r-tracks) Pre-receive (8, REAPER 7.78): the track's media and monitoring before its receives, so MIDI can go to a sampler
/// and its audio come back to the same track with no feedback loop.
pub(super) const MODES: [(&str, i64); 4] = [("post-fader", 0), ("pre-fader", 3), ("pre-fx", 1), ("pre-receive", 8)];

fn mode_state(mode: i64) -> Value {
    json!({"mode": mode, "modeName": rows::send_mode_name(mode)})
}

pub static PROPS: [Prop; 7] = [
    Prop {
        kind: "send-volume",
        read: |reaper, track, index| {
            let gain = reaper.send_value(track, 0, index, "D_VOL");
            json!({"volume": gain, "volumeDb": rows::db(gain)})
        },
        wanted: |input| {
            let db = input
                .get("volumeDb")
                .and_then(Value::as_f64)
                .filter(|db| db.is_finite())
                .ok_or_else(|| bad("volumeDb must be a number"))?;
            if !(-150.0..=24.0).contains(&db) {
                return Err(bad("volumeDb must be from -150 (silent) to 24"));
            }
            let gain = if db <= -150.0 { 0.0 } else { 10f64.powf(db / 20.0) };
            Ok(json!({"volume": gain, "volumeDb": rows::db(gain)}))
        },
        write: |reaper, track, index, state| reaper.set_send_value(track, 0, index, "D_VOL", number(state, "volume")),
        same: |a, b| close(number(a, "volume"), number(b, "volume")),
        title: |route, to| format!("Send {route} {} dB", super::tracks::db_text(number(to, "volumeDb"))),
        noun: "volume",
        check: None,
    },
    Prop {
        kind: "send-pan",
        read: |reaper, track, index| json!({"pan": reaper.send_value(track, 0, index, "D_PAN")}),
        wanted: |input| {
            let pan = input
                .get("pan")
                .and_then(Value::as_f64)
                .filter(|pan| (-1.0..=1.0).contains(pan))
                .ok_or_else(|| bad("pan must be a number from -1 (left) to 1 (right)"))?;
            Ok(json!({"pan": pan}))
        },
        write: |reaper, track, index, state| reaper.set_send_value(track, 0, index, "D_PAN", number(state, "pan")),
        same: |a, b| close(number(a, "pan"), number(b, "pan")),
        title: |route, to| format!("Send {route} pan {}", super::tracks::pan_text(number(to, "pan"))),
        noun: "pan",
        check: None,
    },
    Prop {
        kind: "send-mute",
        read: |reaper, track, index| json!({"mute": reaper.send_value(track, 0, index, "B_MUTE") != 0.0}),
        wanted: |input| {
            let mute = input.get("mute").and_then(Value::as_bool).ok_or_else(|| bad("mute must be true or false"))?;
            Ok(json!({"mute": mute}))
        },
        write: |reaper, track, index, state| {
            reaper.set_send_value(track, 0, index, "B_MUTE", if state["mute"] == true { 1.0 } else { 0.0 })
        },
        same: |a, b| a["mute"] == b["mute"],
        title: |route, to| format!("Send {route} {}", if to["mute"] == true { "muted" } else { "unmuted" }),
        noun: "mute",
        check: None,
    },
    Prop {
        kind: "send-mode",
        read: |reaper, track, index| mode_state(reaper.send_value(track, 0, index, "I_SENDMODE") as i64),
        wanted: |input| {
            let modes = "mode must be post-fader, pre-fader, pre-fx or pre-receive";
            let word = input.get("mode").and_then(Value::as_str).ok_or_else(|| bad(modes))?;
            let (_, mode) = MODES.iter().find(|(name, _)| *name == word).ok_or_else(|| bad(modes))?;
            Ok(mode_state(*mode))
        },
        write: |reaper, track, index, state| reaper.set_send_value(track, 0, index, "I_SENDMODE", number(state, "mode")),
        // 2 is the deprecated spelling of 3.
        same: |a, b| {
            (a["mode"].as_i64().unwrap_or(0) == 2 && b["mode"] == 3)
                || (b["mode"].as_i64().unwrap_or(0) == 2 && a["mode"] == 3)
                || a["mode"] == b["mode"]
        },
        title: |route, to| format!("Send {route} {}", to["modeName"].as_str().unwrap_or("")),
        noun: "mode",
        check: None,
    },
    Prop {
        kind: "send-channels",
        read: |reaper, track, index| channels_state(reaper, track, index),
        wanted: |input| {
            let mut wanted = json!({});
            if let Some(src) = input.get("srcChannels").filter(|v| !v.is_null()) {
                wanted["srcChan"] = src_code(src)?.into();
            }
            if let Some(dst) = input.get("dstChannels").filter(|v| !v.is_null()) {
                wanted["dstChan"] = dst_code(dst)?.into();
            }
            if let Some(midi) = input.get("midi").filter(|v| !v.is_null()) {
                wanted["midiFlags"] = midi_code(midi)?.into();
            }
            if wanted.as_object().is_some_and(|w| w.is_empty()) {
                return Err(bad("give srcChannels, dstChannels or midi"));
            }
            Ok(with_words(wanted))
        },
        write: |reaper, track, index, state| {
            CHANNEL_KEYS.iter().all(|(parm, key)| state[key].as_f64().is_none_or(|v| reaper.set_send_value(track, 0, index, parm, v)))
        },
        // (only what both say is compared: a change of the destination alone leaves the source as it is)
        same: |a, b| CHANNEL_KEYS.iter().all(|(_, key)| a[key].is_null() || b[key].is_null() || a[key].as_i64() == b[key].as_i64()),
        title: |route, to| format!("Send {route} {}", channels_text(to)),
        noun: "channels",
        check: Some(fits),
    },
    // (r-tracks) Its polarity, and whether it sums to mono.
    Prop {
        kind: "send-phase",
        read: |reaper, track, index| json!({"inverted": reaper.send_value(track, 0, index, "B_PHASE") != 0.0}),
        wanted: |input| {
            let inverted = input.get("inverted").and_then(Value::as_bool).ok_or_else(|| bad("inverted must be true or false"))?;
            Ok(json!({"inverted": inverted}))
        },
        write: |reaper, track, index, state| {
            reaper.set_send_value(track, 0, index, "B_PHASE", if state["inverted"] == true { 1.0 } else { 0.0 })
        },
        same: |a, b| a["inverted"] == b["inverted"],
        title: |route, to| format!("Send {route} polarity {}", if to["inverted"] == true { "inverted" } else { "back to normal" }),
        noun: "polarity",
        check: None,
    },
    Prop {
        kind: "send-mono",
        read: |reaper, track, index| json!({"mono": reaper.send_value(track, 0, index, "B_MONO") != 0.0}),
        wanted: |input| {
            let mono = input.get("mono").and_then(Value::as_bool).ok_or_else(|| bad("mono must be true or false"))?;
            Ok(json!({"mono": mono}))
        },
        write: |reaper, track, index, state| {
            reaper.set_send_value(track, 0, index, "B_MONO", if state["mono"] == true { 1.0 } else { 0.0 })
        },
        same: |a, b| a["mono"] == b["mono"],
        title: |route, to| format!("Send {route} {}", if to["mono"] == true { "summed to mono" } else { "in stereo" }),
        noun: "mono",
        check: None,
    },
];

// ----- a send's channels -----

const CHANNEL_KEYS: [(&str, &str); 3] = [("I_SRCCHAN", "srcChan"), ("I_DSTCHAN", "dstChan"), ("I_MIDIFLAGS", "midiFlags")];

/// The channels a send takes from and gives to, and its MIDI, as REAPER's numbers and in words.
fn channels_state(reaper: &Reaper, track: Track, index: i32) -> Value {
    let value = |parm: &str| reaper.send_value(track, 0, index, parm) as i64;
    with_words(json!({"srcChan": value("I_SRCCHAN"), "dstChan": value("I_DSTCHAN"), "midiFlags": value("I_MIDIFLAGS")}))
}

fn with_words(mut state: Value) -> Value {
    if let Some(src) = state["srcChan"].as_i64() {
        state["srcChannels"] = src_words(src);
    }
    if let Some(dst) = state["dstChan"].as_i64() {
        state["dstChannels"] = dst_words(dst);
    }
    if let Some(midi) = state["midiFlags"].as_i64() {
        state["midi"] = midi_words(midi);
    }
    state
}

/// How many channels a source code takes: (code >> 10) is 0 for stereo, 1 for mono, n for 2n channels; -1 is none.
pub(super) fn src_count(code: i64) -> i64 {
    match code {
        c if c < 0 => 0,
        c => match c >> 10 {
            0 => 2,
            1 => 1,
            n => n * 2,
        },
    }
}

pub(super) fn src_words(code: i64) -> Value {
    if code < 0 {
        return "none".into();
    }
    json!({"offset": (code & 1023) + 1, "count": src_count(code)})
}

pub(super) fn dst_words(code: i64) -> Value {
    json!({"offset": (code & 1023) + 1, "mono": code & 1024 != 0})
}

fn midi_words(code: i64) -> Value {
    let (from, to) = (code & 31, (code >> 5) & 31);
    let from: Value = match from {
        31 => "none".into(),
        0 => "all".into(),
        n => n.into(),
    };
    let to: Value = if to == 0 { "same".into() } else { to.into() };
    let mut words = json!({"from": from, "to": to});
    // (r-tracks) The buses (128 a track) and the faders sending MIDI volume and pan, when set.
    match (code >> 14) & 255 {
        0 => {}
        bus => words["fromBus"] = bus.into(),
    }
    match (code >> 22) & 255 {
        0 => {}
        bus => words["toBus"] = bus.into(),
    }
    if code & 1024 != 0 {
        words["faders"] = true.into();
    }
    words
}

pub(super) fn whole(value: &Value, key: &str, low: i64, high: i64) -> Done<i64> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .filter(|n| (low..=high).contains(n))
        .ok_or_else(|| bad(format!("{key} must be a whole number from {low} to {high}")))
}

/// REAPER's source code for {offset, count} (channels counted from 1; count 1, 2 or an even number up to 64), or "none"
/// (no audio: a MIDI-only send).
pub(super) fn src_code(value: &Value) -> Done<i64> {
    if value == "none" {
        return Ok(-1);
    }
    let offset = whole(value, "offset", 1, 128)?;
    let count = value.get("count").map_or(Ok(2), |_| whole(value, "count", 1, 64))?;
    if count > 2 && count % 2 != 0 {
        return Err(bad("srcChannels.count must be 1, 2 or an even number up to 64"));
    }
    if offset + count - 1 > 128 {
        return Err(bad("srcChannels go past channel 128"));
    }
    let code = match count {
        2 => 0,
        1 => 1,
        n => n / 2,
    };
    Ok((offset - 1) | (code << 10))
}

/// REAPER's destination code for {offset, mono?} (counted from 1).
pub(super) fn dst_code(value: &Value) -> Done<i64> {
    let offset = whole(value, "offset", 1, 128)?;
    let mono = match value.get("mono") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("dstChannels.mono must be true or false")),
    };
    Ok((offset - 1) | if mono { 1024 } else { 0 })
}

/// REAPER's MIDI flags for {from: "all" | 1-16 | "none", to: "same" | 1-16}.
fn midi_code(value: &Value) -> Done<i64> {
    let from = match value.get("from") {
        None | Some(Value::Null) => 0,
        Some(v) if v == "all" => 0,
        Some(v) if v == "none" => 31,
        Some(v) => {
            v.as_i64().filter(|n| (1..=16).contains(n)).ok_or_else(|| bad("midi.from must be \"all\", 1 to 16, or \"none\" (no MIDI)"))?
        }
    };
    let to = match value.get("to") {
        None | Some(Value::Null) => 0,
        Some(v) if v == "same" => 0,
        Some(v) => v.as_i64().filter(|n| (1..=16).contains(n)).ok_or_else(|| bad("midi.to must be \"same\" or 1 to 16"))?,
    };
    // (r-tracks) Buses: from "all" (0) or a bus 1 to 16 (REAPER's routing window has 16), to "same" (0) or a bus; faders:
    // the send's faders send MIDI volume and pan.
    let bus = |key: &str, any: &str| -> Done<i64> {
        match value.get(key) {
            None | Some(Value::Null) => Ok(0),
            Some(v) if v == any => Ok(0),
            Some(v) => v
                .as_i64()
                .filter(|n| (1..=16).contains(n))
                .ok_or_else(|| bad(format!("midi.{key} must be \"{any}\" or a bus from 1 to 16"))),
        }
    };
    let (from_bus, to_bus) = (bus("fromBus", "all")?, bus("toBus", "same")?);
    let faders = match value.get("faders") {
        None | Some(Value::Null) => 0,
        Some(Value::Bool(on)) => i64::from(*on) << 10,
        Some(_) => return Err(bad("midi.faders must be true or false")),
    };
    Ok(from | (to << 5) | faders | (from_bus << 14) | (to_bus << 22))
}

pub(super) fn span_text(first: i64, count: i64) -> String {
    match count {
        1 => format!("{first}"),
        2 => format!("{first}/{}", first + 1),
        n => format!("{first}\u{2013}{}", first + n - 1),
    }
}

/// "channels 1/2 → 3/4, MIDI all → same", from what a state says.
fn channels_text(state: &Value) -> String {
    let mut parts = Vec::new();
    let src = state["srcChan"].as_i64();
    let from = match src {
        Some(c) if c < 0 => Some("no audio".to_string()),
        Some(c) => Some(span_text((c & 1023) + 1, src_count(c))),
        None => None,
    };
    let to = state["dstChan"].as_i64().map(|d| {
        let count = src.map_or(2, src_count).max(1);
        let mono = if d & 1024 != 0 { " (mono)" } else { "" };
        format!("{}{mono}", span_text((d & 1023) + 1, if d & 1024 != 0 { 1 } else { count }))
    });
    match (from, to) {
        (Some(from), Some(to)) => parts.push(format!("channels {from} \u{2192} {to}")),
        (Some(from), None) => parts.push(format!("from channels {from}")),
        (None, Some(to)) => parts.push(format!("to channels {to}")),
        (None, None) => {}
    }
    if let Some(midi) = state["midiFlags"].as_i64() {
        let words = midi_words(midi);
        let from = words["from"].as_str().map_or_else(|| words["from"].to_string(), str::to_string);
        if from == "none" {
            parts.push("no MIDI".to_string());
        } else {
            let to = words["to"].as_str().map_or_else(|| words["to"].to_string(), str::to_string);
            parts.push(format!("MIDI {from} \u{2192} {to}"));
            // (r-tracks)
            if words.get("fromBus").is_some() || words.get("toBus").is_some() {
                let bus = |key: &str, any: &str| words.get(key).map_or(any.to_string(), |bus| format!("bus {bus}"));
                parts.push(format!("{} \u{2192} {}", bus("fromBus", "all buses"), bus("toBus", "the same bus")));
            }
            if words.get("faders").is_some() {
                parts.push("faders send MIDI volume and pan".to_string());
            }
        }
    }
    parts.join(", ")
}

/// Whether the tracks at both ends have the channels the wanted state uses: a send can't take from or give to channels a
/// track doesn't have (REAPER keeps them, unheard).
/// The fields asked for over the send's own: a change of one end is checked with the other end it has.
fn over(now: &Value, wanted: &Value) -> Value {
    let mut merged = now.clone();
    for (key, value) in wanted.as_object().into_iter().flatten() {
        merged[key] = value.clone();
    }
    merged
}

fn fits(reaper: &Reaper, from: Track, to: Track, wanted: &Value) -> Done<()> {
    let channels = |track: Track| reaper.track_value(track, "I_NCHAN").max(2.0) as i64;
    let src = wanted["srcChan"].as_i64();
    if let Some(src) = src.filter(|c| *c >= 0) {
        let last = (src & 1023) + src_count(src);
        if last > channels(from) {
            return Err(bad(format!(
                "{} has {} channels, and the send would take from channel {last}: raise its channels first (track_channels)",
                super::quoted(&name_of(reaper, from)),
                channels(from)
            )));
        }
    }
    if let Some(dst) = wanted["dstChan"].as_i64() {
        let count = if dst & 1024 != 0 { 1 } else { src.map_or(2, src_count).max(1) };
        let last = (dst & 1023) + count;
        if last > channels(to) {
            return Err(bad(format!(
                "{} has {} channels, and the send would reach channel {last}: raise its channels first (track_channels)",
                super::quoted(&name_of(reaper, to)),
                channels(to)
            )));
        }
    }
    Ok(())
}

pub fn prop_preview(ctx: &Ctx, prop: &Prop, input: &Value) -> Done<Preview> {
    let reference = &canonical(input["ref"].as_str().unwrap_or_default())?;
    let found = find(ctx, reference, true)?;
    let now = (prop.read)(ctx.reaper, found.track, found.index);
    let wanted = (prop.wanted)(input)?;
    if let Some(check) = prop.check {
        check(ctx.reaper, found.track, found.receiver, &over(&now, &wanted))?;
    }
    let route = route_text(&found.from, &found.to);
    let mut display = json!({"title": (prop.title)(&route, &wanted), "before": now, "after": wanted});
    if (prop.same)(&now, &wanted) {
        display["unchanged"] = true.into();
    }
    let fence = json!({"field": now, "send": seal(ctx.reaper, &found)});
    Ok(Preview { target: target(reference), prior: now, fence, display })
}

pub fn prop_apply(ctx: &Ctx, prop: &Prop, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, input["ref"].as_str().unwrap_or_default(), &fence["send"])?;
    let route = route_text(&found.from, &found.to);
    let now = (prop.read)(ctx.reaper, found.track, found.index);
    if !(prop.same)(&now, &fence["field"]) {
        return Err(changed(format!("{route}'s {} changed since the preview", prop.noun)));
    }
    let wanted = (prop.wanted)(input)?;
    if let Some(check) = prop.check {
        check(ctx.reaper, found.track, found.receiver, &over(&now, &wanted))?;
    }
    let before = pair_seen(ctx.reaper, found.track, found.receiver);
    if !(prop.same)(&now, &wanted) && !(prop.write)(ctx.reaper, found.track, found.index, &wanted) {
        return Err(failed(format!("REAPER wouldn't take the send's {}", prop.noun)));
    }
    let after = (prop.read)(ctx.reaper, found.track, found.index);
    if !(prop.same)(&after, &wanted) {
        // (r-tracks) What REAPER took otherwise than asked is put back, inside this call: nothing half-done stays.
        let back = (prop.write)(ctx.reaper, found.track, found.index, &now)
            && (prop.same)(&(prop.read)(ctx.reaper, found.track, found.index), &now);
        return Err(if back {
            failed(format!("REAPER kept the send's {} as it was", prop.noun))
        } else {
            (
                "uncertain".into(),
                format!("REAPER took the send's {} otherwise than asked, and Kumi could not put it back: read the routing", prop.noun),
            )
        });
    }
    // A send Kumi changes gets Kumi's mark, in the same step, if it has none: its undo then finds it by that and by no value
    // (two sends at REAPER's defaults are alike in every value). Only once REAPER took the value: a refused change leaves
    // no mark behind.
    if !(prop.same)(&now, &wanted) && stamp_of(ctx.reaper, found.track, found.index).is_none() {
        ctx.reaper.set_send_text(found.track, 0, found.index, STAMP, &fresh_stamp());
    }
    note_written(ctx.reaper, found.track, found.receiver, before);
    let mut applied = after.clone();
    applied["send"] = seal(ctx.reaper, &found);
    Ok(Applied {
        applied,
        created: None,
        material: None,
        display: json!({
            "title": (prop.title)(&route, &after),
            "before": now,
            "after": after,
            "route": drawn_route(&found.from, &found.to, Some(&now), Some(&after)),
        }),
        unwritten: (prop.same)(&now, &wanted),
    })
}

pub fn prop_restore(ctx: &Ctx, prop: &Prop, restore: &Restore) -> Done<Restored> {
    // (a send Kumi marked when it changed it is the one with the mark: none there means it was removed or replaced since, or
    // REAPER's own undo took the change back with its mark, which leaves the send at its place as it was before)
    let marked = restore.applied["send"]["stamp"].is_string();
    let reference = restore.target["ref"].as_str().unwrap_or_default();
    let found = find_for_restore(ctx, reference, &restore.applied["send"], !restore.discard).map_err(|fault| {
        if !(marked && fault.0 == "gone") {
            return fault;
        }
        let back = find(ctx, reference, false).is_ok_and(|at| (prop.same)(&(prop.read)(ctx.reaper, at.track, at.index), restore.prior));
        if back {
            changed("the send Kumi changed has lost its mark and is as it was before: it may have been undone in REAPER (Edit > Undo)")
        } else {
            changed("the send Kumi changed isn't there with its mark any more: it was removed or replaced in REAPER since")
        }
    })?;
    let route = route_text(&found.from, &found.to);
    let now = (prop.read)(ctx.reaper, found.track, found.index);
    let already = (prop.same)(&now, restore.prior);
    if !already {
        if !restore.discard && !(prop.same)(&now, restore.applied) {
            return Err(changed(format!("{route}'s {} changed in REAPER since Kumi set it", prop.noun)));
        }
        let before = pair_seen(ctx.reaper, found.track, found.receiver);
        if !(prop.write)(ctx.reaper, found.track, found.index, restore.prior) {
            return Err(failed(format!("REAPER wouldn't put back the send's {}", prop.noun)));
        }
        note_written(ctx.reaper, found.track, found.receiver, before);
    }
    let after = (prop.read)(ctx.reaper, found.track, found.index);
    if !(prop.same)(&after, restore.prior) {
        return Err(failed(format!("REAPER kept the send's {} as it was", prop.noun)));
    }
    Ok(Restored {
        restored: after.clone(),
        display: json!({"title": format!("{} (put back)", (prop.title)(&route, &after)), "before": now, "after": after}),
        unwritten: already,
    })
}

// ----- adding a send -----

/// All of a send's settings, as a state to compare and to make again.
fn send_state(reaper: &Reaper, track: Track, index: i32) -> Value {
    let value = |parm: &str| reaper.send_value(track, 0, index, parm);
    json!({
        "volume": value("D_VOL"), "pan": value("D_PAN"), "mute": value("B_MUTE"), "mode": value("I_SENDMODE"),
        "srcChan": value("I_SRCCHAN"), "dstChan": value("I_DSTCHAN"), "midiFlags": value("I_MIDIFLAGS"),
        "phase": value("B_PHASE"), "mono": value("B_MONO"), "autoMode": value("I_AUTOMODE"),
        "envelopes": rows::send_envelope_print(reaper, track, index),
    })
}

const STATE_KEYS: [(&str, &str); 10] = [
    ("D_VOL", "volume"),
    ("D_PAN", "pan"),
    ("B_MUTE", "mute"),
    ("I_SENDMODE", "mode"),
    ("I_SRCCHAN", "srcChan"),
    ("I_DSTCHAN", "dstChan"),
    ("I_MIDIFLAGS", "midiFlags"),
    ("B_PHASE", "phase"),
    ("B_MONO", "mono"),
    ("I_AUTOMODE", "autoMode"),
];

fn write_state(reaper: &Reaper, track: Track, index: i32, state: &Value) {
    for (parm, key) in STATE_KEYS {
        if let Some(value) = state[key].as_f64() {
            reaper.set_send_value(track, 0, index, parm, value);
        }
    }
}

fn same_state(a: &Value, b: &Value) -> bool {
    STATE_KEYS.iter().all(|(_, key)| close(number(a, key), number(b, key)))
}

struct Add {
    from: Track,
    from_id: TrackId,
    to: Track,
    to_id: TrackId,
    changes: Vec<(&'static str, f64)>,
}

fn add_plan(ctx: &Ctx, input: &Value) -> Done<Add> {
    let id = |what: &str| -> Done<TrackId> {
        match Ref::parse(input[what].as_str().unwrap_or_default()).map_err(bad)? {
            Ref::Track(id) => Ok(id),
            other => Err(bad(format!("{what}: a track ref, not r:{}", other.kind()))),
        }
    };
    let (from_id, to_id) = (id("ref")?, id("toRef")?);
    if from_id == TrackId::Master || to_id == TrackId::Master {
        return Err(bad("the master track has no sends of this kind: a send goes from one track to another"));
    }
    if from_id == to_id {
        return Err(bad("a track can't send to itself"));
    }
    let from = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &from_id)?;
    let to = ctx.reads.track_by_id(ctx.reaper, ctx.proj, &to_id)?;
    let mut changes = Vec::new();
    if input.get("volumeDb").is_some_and(|v| !v.is_null()) {
        changes.push(("D_VOL", number(&(PROPS[0].wanted)(input)?, "volume")));
    }
    if input.get("pan").is_some_and(|v| !v.is_null()) {
        changes.push(("D_PAN", number(&(PROPS[1].wanted)(input)?, "pan")));
    }
    if input.get("mode").is_some_and(|v| !v.is_null()) {
        changes.push(("I_SENDMODE", number(&(PROPS[3].wanted)(input)?, "mode")));
    }
    // A sidechain: the channels it takes from and gives to, and its MIDI.
    if ["srcChannels", "dstChannels", "midi"].iter().any(|key| input.get(*key).is_some_and(|v| !v.is_null())) {
        let wanted = (PROPS[4].wanted)(input)?;
        fits(ctx.reaper, from, to, &wanted)?;
        for (parm, key) in CHANNEL_KEYS {
            if let Some(value) = wanted[key].as_i64() {
                changes.push((parm, value as f64));
            }
        }
    }
    Ok(Add { from, from_id, to, to_id, changes })
}

/// ", channels 1/2 → 3/4" for a send added as a sidechain.
fn sidechain_text(plan: &Add) -> String {
    let mut state = json!({});
    for (parm, key) in CHANNEL_KEYS {
        if let Some((_, value)) = plan.changes.iter().find(|(p, _)| *p == parm) {
            state[key] = (*value as i64).into();
        }
    }
    let text = channels_text(&state);
    if text.is_empty() {
        String::new()
    } else {
        format!(", {text}")
    }
}

fn add_fence(ctx: &Ctx, plan: &Add) -> Value {
    json!({"sends": ctx.reaper.send_count(plan.from, 0), "toGuid": ctx.reaper.track_guid(plan.to)})
}

pub fn add_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let plan = add_plan(ctx, input)?;
    let route = route_text(&name_of(ctx.reaper, plan.from), &name_of(ctx.reaper, plan.to));
    let existing = rows::send_keys(ctx.reaper, plan.from, 0).iter().filter(|key| key.other == SendEnd::Track(plan.to_id.clone())).count();
    let mut display = json!({"title": format!("Added send {route}{}", sidechain_text(&plan))});
    if existing > 0 {
        display["note"] = format!("There are {existing} send(s) between them already.").into();
    }
    Ok(Preview {
        target: json!({"ref": refs::track_ref(&plan.from_id), "kind": "track"}),
        prior: json!({"sends": ctx.reaper.send_count(plan.from, 0)}),
        fence: add_fence(ctx, &plan),
        display,
    })
}

pub fn add_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let plan = add_plan(ctx, input).map_err(|fault| {
        if fault.0 == "failed" {
            changed(format!("{}: it went since the preview", fault.1))
        } else {
            fault
        }
    })?;
    if &add_fence(ctx, &plan) != fence {
        return Err(changed("the sends of that track changed since the preview"));
    }
    let reaper = ctx.reaper;
    let before = pair_seen(reaper, plan.from, plan.to);
    let index = reaper.create_send(plan.from, Some(plan.to)).ok_or_else(|| failed("REAPER wouldn't make the send"))?;
    // Marked as Kumi's own, so that an undo finds this send and no other, whatever the producer does to the ones beside it.
    let stamp = fresh_stamp();
    reaper.set_send_text(plan.from, 0, index, STAMP, &stamp);
    for (parm, value) in &plan.changes {
        reaper.set_send_value(plan.from, 0, index, parm, *value);
    }
    note_written(reaper, plan.from, plan.to, before);
    let keys = rows::send_keys(reaper, plan.from, 0);
    let key = keys.get(index as usize).ok_or_else(|| failed("the new send isn't in the track's sends"))?;
    let seen = rows::pair_seen(reaper, plan.from, 0, index, key);
    let reference = refs::send_ref(&plan.from_id, 0, &key.other, key.repeat, key.of, seen.as_deref());
    let route = route_text(&name_of(reaper, plan.from), &name_of(reaper, plan.to));
    let found = Found { track: plan.from, index, receiver: plan.to, from: name_of(reaper, plan.from), to: name_of(reaper, plan.to) };
    let mut created = target(&reference);
    if stamp_of(reaper, plan.from, index).as_deref() == Some(stamp.as_str()) {
        created["stamp"] = stamp.into();
    }
    let mut applied = send_state(reaper, plan.from, index);
    let drawn = drawn_route(&found.from, &found.to, None, Some(&applied));
    applied["send"] = seal(reaper, &found);
    Ok(Applied {
        applied,
        created: Some(created),
        material: None,
        display: json!({
            "title": format!("Added send {route}{}", sidechain_text(&plan)),
            "route": drawn,
        }),
        ..Default::default()
    })
}

pub fn add_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the send that was added"))?;
    let reference = created["ref"].as_str().ok_or_else(|| bad("created: the send's ref"))?;
    let found = find_for_restore(ctx, reference, &restore.applied["send"], !restore.discard)?;
    let now = send_state(ctx.reaper, found.track, found.index);
    if !restore.discard && !same_state(&now, restore.applied) {
        return Err(changed(format!("the send {} has been changed in REAPER since Kumi added it", route_text(&found.from, &found.to))));
    }
    // Its volume, pan and mute envelopes go with it, and nothing brings them back: drawn since, they are the producer's.
    let drawn = drawn_envelopes(ctx.reaper, found.track, found.index);
    if !restore.discard && drawn > 0 {
        return Err(changed(format!(
            "the send {} has {drawn} envelope(s) the producer added since Kumi made it, which would go with it",
            route_text(&found.from, &found.to)
        )));
    }
    let before = pair_seen(ctx.reaper, found.track, found.receiver);
    if !ctx.reaper.remove_send(found.track, 0, found.index) {
        return Err(failed("REAPER wouldn't remove the send"));
    }
    note_written(ctx.reaper, found.track, found.receiver, before);
    Ok(Restored {
        restored: json!({"removed": reference}),
        display: json!({"title": format!("Send {} taken away again", route_text(&found.from, &found.to))}),
        ..Default::default()
    })
}

// ----- a receive made from the receiving side (r-tracks) -----

/// A receive made on the receiving track is the send from the other one, as send-add makes it: `ref` the receiving track,
/// `fromRef` the sending one.
fn as_send_add(input: &Value) -> Done<Value> {
    let from = input.get("fromRef").filter(|v| v.is_string()).cloned().ok_or_else(|| bad("fromRef: the track it receives from"))?;
    let mut send = input.clone();
    send["toRef"] = input.get("ref").cloned().unwrap_or(Value::Null);
    send["ref"] = from;
    if let Some(fields) = send.as_object_mut() {
        fields.remove("fromRef");
    }
    Ok(send)
}

pub fn receive_add_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    add_preview(ctx, &as_send_add(input)?)
}

pub fn receive_add_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    add_apply(ctx, &as_send_add(input)?, fence)
}

// ----- removing a send -----

pub fn remove_preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let reference = &canonical(input["ref"].as_str().unwrap_or_default())?;
    let found = find(ctx, reference, true)?;
    let now = send_state(ctx.reaper, found.track, found.index);
    let mut display = json!({"title": format!("Removed send {}", route_text(&found.from, &found.to)), "before": now});
    let envelopes = drawn_envelopes(ctx.reaper, found.track, found.index);
    if envelopes > 0 {
        display["note"] = format!("Its {envelopes} envelope(s) can't be put back by undo.").into();
    }
    let fence = json!({"state": now, "send": seal(ctx.reaper, &found)});
    Ok(Preview { target: target(reference), prior: now, fence, display })
}

/// The send's volume, pan and mute envelopes that are on or hold points (one bypassed is still drawn): REAPER takes them
/// with the send, and nothing brings them back.
fn drawn_envelopes(reaper: &Reaper, track: Track, index: i32) -> usize {
    ["<VOLENV", "<PANENV", "<MUTEENV"]
        .into_iter()
        .filter_map(|chunk| reaper.send_envelope(track, index, chunk))
        .filter(|envelope| reaper.envelope_flag(*envelope, "ACTIVE") || reaper.envelope_point_count(*envelope) > 0)
        .count()
}

pub fn remove_apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let reference = input["ref"].as_str().unwrap_or_default();
    let found = find_for_apply(ctx, reference, &fence["send"])?;
    let reaper = ctx.reaper;
    let now = send_state(reaper, found.track, found.index);
    if !same_state(&now, &fence["state"]) {
        return Err(changed(format!("the send {} changed since the preview", route_text(&found.from, &found.to))));
    }
    let receiver = reaper
        .send_peer(found.track, 0, found.index)
        .map(|t| reaper.track_guid(t))
        .ok_or_else(|| failed("that send has no receiving track"))?;
    let lost = drawn_envelopes(reaper, found.track, found.index);
    let (stamp, together) = (stamp_of(reaper, found.track, found.index), pair_count(reaper, found.track, found.receiver));
    let before = pair_seen(reaper, found.track, found.receiver);
    if !reaper.remove_send(found.track, 0, found.index) {
        return Err(failed("REAPER wouldn't remove the send"));
    }
    note_written(reaper, found.track, found.receiver, before);
    let material = json!({"state": now, "receiver": receiver, "lostEnvelopes": lost, "stamp": stamp, "together": together});
    Ok(Applied {
        applied: json!({"removed": true}),
        created: None,
        material: Some(material),
        display: json!({
            "title": format!("Removed send {}", route_text(&found.from, &found.to)),
            "route": drawn_route(&found.from, &found.to, Some(&now), None),
        }),
        ..Default::default()
    })
}

pub fn remove_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the removal took away"))?;
    let reference = restore.target["ref"].as_str().unwrap_or_default();
    let Ref::Send { track, .. } = Ref::parse(reference).map_err(bad)? else { return Err(bad("target: a send ref")) };
    let reaper = ctx.reaper;
    let from = ctx.reads.track_by_id(reaper, ctx.proj, &track).map_err(|(_, message)| ("gone".to_string(), message))?;
    let receiver = TrackId::Guid(material["receiver"].as_str().unwrap_or_default().to_string());
    let to =
        track_by_id(ctx, &receiver).ok_or_else(|| ("gone".to_string(), "the receiving track isn't in the project any more".to_string()))?;
    // The send is back already (a retry, or the producer made it): nothing is made twice. One with the mark is Kumi's send;
    // else there are as many to that track as there were before it went, and one has its state.
    let stamp = material["stamp"].as_str();
    let marked = match stamp {
        Some(stamp) => find_by_stamp(reaper, from, to, stamp)?,
        None => None,
    };
    let back = match marked {
        Some(index) => Some(index),
        None if stamp.is_none() && pair_count(reaper, from, to) >= material["together"].as_u64().unwrap_or(1) as usize => {
            (0..reaper.send_count(from, 0)).find(|&index| {
                reaper.send_peer(from, 0, index) == Some(to) && same_state(&send_state(reaper, from, index), &material["state"])
            })
        }
        None => None,
    };
    if let Some(index) = back {
        return Ok(Restored {
            restored: send_state(reaper, from, index),
            display: json!({"title": "The send is back already"}),
            unwritten: true,
        });
    }
    let before = pair_seen(reaper, from, to);
    let index = reaper.create_send(from, Some(to)).ok_or_else(|| failed("REAPER wouldn't make the send again"))?;
    write_state(reaper, from, index, &material["state"]);
    if let Some(stamp) = stamp {
        reaper.set_send_text(from, 0, index, STAMP, stamp);
    }
    note_written(reaper, from, to, before);
    let keys = rows::send_keys(reaper, from, 0);
    let key = keys.get(index as usize).ok_or_else(|| failed("the send made again isn't in the track's sends"))?;
    let seen = rows::pair_seen(reaper, from, 0, index, key);
    let mut display = json!({"title": format!("Send {} put back", route_text(&name_of(reaper, from), &name_of(reaper, to)))});
    if let Some(lost) = material["lostEnvelopes"].as_u64().filter(|lost| *lost > 0) {
        display["note"] = format!("Its {lost} envelope(s) were not put back.").into();
    }
    Ok(Restored {
        restored: json!({
            "ref": refs::send_ref(&track, 0, &key.other, key.repeat, key.of, seen.as_deref()),
            "state": send_state(reaper, from, index),
        }),
        display,
        ..Default::default()
    })
}
