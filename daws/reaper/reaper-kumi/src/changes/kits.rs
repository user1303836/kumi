//! A sampler kit made from sample files (M6, m6-library): `drum-kit`, one change and one point of REAPER's undo. Each pad
//! is a ReaSamplOmatic5000 holding one file (`FILE0`, then `DONE`), playing it as it is (`MODE` 1) on its one note.
//!
//! - `tracks` (the default): a folder named after the kit, a MIDI track in it that holds the notes, and a track per pad
//!   fed by a MIDI-only send from it: each pad has its own fader, FX and sends.
//! - `container`: one track, its pads in one FX container: a compact kit that saves as one chain.
//!
//! Hi-hats choke as a drummer's do: JS: MIDI Choke ahead of the pads ends an open hat's note when a closed or pedal hat
//! plays, and the open hats obey note-offs so the end cuts them. REAPER can't name a pad's file in an undo point, so the
//! tracks made are the change's (`applied.tracks`, by GUID), as a track template's are: its undo deletes them only while
//! each is as the kit left them (its items, mix, routing, plug-ins and their parameters, and each pad's file) and no track
//! of the producer's has gone into the folder.

use std::path::Path;

use serde_json::{json, Value};

use super::fxchain::{self, Holder};
use super::templates;
use super::{Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Fx, FxOwner, Reaper, Track};
use crate::refs::{self, TrackId};

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

/// The most pads one kit holds, and the most a pad's file may be (RS5k holds it in memory).
const MOST_PADS: usize = 64;
const MOST_FILE: u64 = 256 << 20;
/// The sampler and the choke, by the names REAPER's FX browser finds them by.
const SAMPLER: &str = "ReaSamplOmatic5000 (Cockos)";
const CHOKE: &str = "JS: midi/midi_choke";
/// JS: MIDI Choke's sliders, from 0: its channel, the choke range (start, count), the range it cuts (start, count), what a
/// note-on in the cut range does while a choke note is held, and four more choke notes.
const CHOKE_CHANNEL: i32 = 0;
const CHOKE_START: i32 = 1;
const CHOKE_COUNT: i32 = 2;
const CUT_START: i32 = 3;
const CUT_COUNT: i32 = 4;
const CHOKE_EXTRA: [i32; 4] = [6, 7, 8, 9];

/// Sound files REAPER's sampler reads, and how each one's first bytes start.
const AUDIO: &[(&str, &[&[u8]])] = &[
    ("wav", &[b"RIFF", b"RIFX", b"RF64"]),
    ("wave", &[b"RIFF", b"RIFX", b"RF64"]),
    ("aif", &[b"FORM"]),
    ("aiff", &[b"FORM"]),
    ("flac", &[b"fLaC"]),
    ("ogg", &[b"OggS"]),
    ("mp3", &[b"ID3", b"\xff\xfb", b"\xff\xf3", b"\xff\xf2", b"\xff\xfa"]),
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Layout {
    Tracks,
    Container,
}

/// A pad's part in the choke: its note cuts the others, or it is what they cut.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Choke {
    None,
    Closes,
    Open,
}

#[derive(Clone, Debug)]
struct Pad {
    path: String,
    name: String,
    note: i64,
    choke: Choke,
    obey: bool,
    volume: Option<f64>,
    /// What the file was at the preview (its size and modified time): a file changed after it refuses the apply.
    stamp: Value,
}

#[derive(Debug)]
struct Kit {
    pads: Vec<Pad>,
    layout: Layout,
    name: String,
    at: i64,
    channel: i64,
}

/// A note as REAPER names it (middle C, 60, is C4).
fn note_name(note: i64) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[note.rem_euclid(12) as usize], note.div_euclid(12) - 1)
}

/// A hi-hat, by its name ("Closed Hat", "HH open 2", "hihat_open"), and whether it is an open one.
fn hat(name: &str) -> Option<bool> {
    let lower = name.to_lowercase().replace(['_', '-', '.'], " ");
    let words: Vec<&str> = lower.split_whitespace().collect();
    let is_hat = words.iter().any(|w| w.contains("hat") || *w == "hh" || w.starts_with("hh"));
    is_hat.then(|| words.iter().any(|w| w.contains("open") || *w == "oh"))
}

fn text_field(value: Option<&Value>, what: &str) -> Done<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.chars().count() <= 256 && !text.chars().any(char::is_control) => {
            Ok(Some(text.trim().to_string()).filter(|t| !t.is_empty()))
        }
        Some(_) => Err(bad(format!("{what} must be one line of at most 256 characters"))),
    }
}

/// A sound file the sampler can read: whole path, there, of a kind it reads and starting as that kind does.
fn audio_file(path: &str) -> Done<(String, Value)> {
    let file = Path::new(path);
    if !file.is_absolute() || file.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(bad("a pad's path must be a whole path to a sound file, with no \"..\" in it"));
    }
    let extension = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let Some((_, starts)) = AUDIO.iter().find(|(kind, _)| *kind == extension) else {
        return Err(bad(format!("{path} isn't a sound file the sampler reads (.wav, .aif, .flac, .ogg or .mp3)")));
    };
    let meta = std::fs::metadata(file).map_err(|_| bad(format!("there's no file at {path}")))?;
    if !meta.is_file() || meta.len() > MOST_FILE {
        return Err(bad(format!("{path} isn't a sound file Kumi loads (a file of at most {} MB)", MOST_FILE >> 20)));
    }
    let mut head = [0u8; 4];
    let read = std::fs::File::open(file).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
    if !starts.iter().any(|start| head[..read].starts_with(start)) {
        return Err(bad(format!("{path} isn't a .{extension} file (its contents say otherwise)")));
    }
    let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64);
    Ok((path.to_string(), json!({"size": meta.len(), "modified": modified})))
}

fn file_title(path: &str) -> String {
    Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn kit_of(ctx: &Ctx, input: &Value) -> Done<Kit> {
    let list = input
        .get("pads")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| bad("pads: the sound files, as [{path, name?, ...}], at least one"))?;
    if list.len() > MOST_PADS {
        return Err(bad(format!("a kit holds at most {MOST_PADS} pads")));
    }
    let layout = match input.get("layout").and_then(Value::as_str) {
        None | Some("tracks") => Layout::Tracks,
        Some("container") => Layout::Container,
        Some(_) => return Err(bad("layout is tracks (a folder with a track per pad) or container (one track, its pads in one container)")),
    };
    let first = match input.get("first") {
        None | Some(Value::Null) => 36,
        Some(v) => v.as_i64().filter(|n| (0..=127).contains(n)).ok_or_else(|| bad("first is a note from 0 to 127 (C2, 36, by default)"))?,
    };
    let channel = match input.get("channel") {
        None | Some(Value::Null) => 1,
        Some(v) => {
            v.as_i64().filter(|n| (1..=16).contains(n)).ok_or_else(|| bad("channel is the MIDI channel the hi-hats choke on, 1 to 16"))?
        }
    };
    let chokes = input.get("chokeHats").map(|v| v.as_bool().ok_or_else(|| bad("chokeHats is true or false"))).transpose()?.unwrap_or(true);
    let mut pads = Vec::new();
    let mut named_choke = false;
    for given in list {
        let path = given.get("path").and_then(Value::as_str).ok_or_else(|| bad("each pad has a path: a sound file"))?;
        let (path, stamp) = audio_file(path)?;
        let name = text_field(given.get("name"), "a pad's name")?.unwrap_or_else(|| file_title(&path));
        let note = match given.get("note") {
            None | Some(Value::Null) => -1,
            Some(v) => v.as_i64().filter(|n| (0..=127).contains(n)).ok_or_else(|| bad("a pad's note is from 0 to 127 (C4 is 60)"))?,
        };
        let choke = match given.get("choke").and_then(Value::as_str) {
            None if given.get("choke").is_none_or(Value::is_null) => Choke::None,
            Some("closes") => Choke::Closes,
            Some("open") => Choke::Open,
            Some("none") => Choke::None,
            _ => return Err(bad("a pad's choke is closes (its note cuts the open ones), open (cut by them) or none")),
        };
        named_choke |= given.get("choke").is_some_and(|v| !v.is_null());
        let obey = match given.get("obeyNoteOffs") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(_) => return Err(bad("obeyNoteOffs is true or false")),
        };
        let volume = match given.get("volumeDb") {
            None | Some(Value::Null) => None,
            Some(v) => {
                Some(v.as_f64().filter(|db| db.is_finite() && *db <= 24.0).ok_or_else(|| bad("a pad's volumeDb is a number up to +24"))?)
            }
        };
        if volume.is_some() && layout == Layout::Container {
            return Err(bad(
                "a pad's volumeDb is its track's fader: the tracks layout has one per pad (in a container, set the sampler's Volume after)",
            ));
        }
        pads.push(Pad { path, name, note, choke, obey: obey.unwrap_or(false), volume, stamp });
    }
    // Notes: a pad's own, then the next free one from `first` up.
    let taken: Vec<i64> = pads.iter().map(|p| p.note).filter(|n| *n >= 0).collect();
    let mut next = first;
    for pad in pads.iter_mut().filter(|p| p.note < 0) {
        while taken.contains(&next) {
            next += 1;
        }
        if next > 127 {
            return Err(bad(format!("{} pads from {} run past the top note (127)", list.len(), note_name(first))));
        }
        pad.note = next;
        next += 1;
    }
    // The hi-hats choke unless a pad says how, or the request turns it off: an open hat is cut by the others.
    if chokes && !named_choke {
        for pad in pads.iter_mut() {
            if let Some(open) = hat(&pad.name) {
                pad.choke = if open { Choke::Open } else { Choke::Closes };
            }
        }
    }
    let closes = pads.iter().filter(|p| p.choke == Choke::Closes).count();
    let open = pads.iter().filter(|p| p.choke == Choke::Open).count();
    if closes == 0 || open == 0 {
        for pad in pads.iter_mut() {
            pad.choke = Choke::None;
        }
    }
    for pad in pads.iter_mut().filter(|p| p.choke == Choke::Open) {
        // (a cut pad must stop at the note-off the choke sends)
        pad.obey = true;
    }
    let name = text_field(input.get("name"), "name")?.unwrap_or_else(|| "Drums".into());
    let count = ctx.reaper.track_count(ctx.proj) as i64;
    let at = match input.get("index") {
        None | Some(Value::Null) => count,
        Some(v) => v.as_i64().filter(|i| (1..=count + 1).contains(i)).map(|i| i - 1).ok_or_else(|| {
            bad(format!("index is the kit's first track's place, from 1 to {} (counted from 1, as a read shows)", count + 1))
        })?,
    };
    let kit = Kit { pads, layout, name, at, channel };
    chokes_of(&kit)?;
    Ok(kit)
}

/// The JS: MIDI Choke instances a kit needs: the notes that choke (one run and up to four more), and each run of open-hat
/// notes it cuts (one instance a run).
struct ChokePlan {
    closes: Vec<i64>,
    cuts: Vec<(i64, i64)>,
}

fn chokes_of(kit: &Kit) -> Done<Option<ChokePlan>> {
    let mut closes: Vec<i64> = kit.pads.iter().filter(|p| p.choke == Choke::Closes).map(|p| p.note).collect();
    let mut open: Vec<i64> = kit.pads.iter().filter(|p| p.choke == Choke::Open).map(|p| p.note).collect();
    if closes.is_empty() || open.is_empty() {
        return Ok(None);
    }
    closes.sort();
    closes.dedup();
    open.sort();
    open.dedup();
    let run = closes.windows(2).take_while(|w| w[1] == w[0] + 1).count() + 1;
    if closes.len() - run > CHOKE_EXTRA.len() {
        return Err(bad(
            "too many pads choke the open hats for one JS: MIDI Choke: give at most five closing notes, or put them next to each other",
        ));
    }
    let mut cuts: Vec<(i64, i64)> = Vec::new();
    for note in open {
        match cuts.last_mut() {
            Some((start, count)) if *start + *count == note && *count < 16 => *count += 1,
            _ => cuts.push((note, 1)),
        }
    }
    Ok(Some(ChokePlan { closes, cuts }))
}

fn fence_of(ctx: &Ctx, kit: &Kit) -> Value {
    let reaper = ctx.reaper;
    json!({
        "tracks": reaper.track_count(ctx.proj),
        "at": reaper.track_at(ctx.proj, kit.at as i32).map(|t| reaper.track_guid(t)),
        "files": kit.pads.iter().map(|p| p.stamp.clone()).collect::<Vec<_>>(),
    })
}

fn title_of(kit: &Kit) -> String {
    let what = if kit.layout == Layout::Tracks { "drum kit" } else { "drum kit container" };
    let mut title =
        format!("Made the {what} {} with {} pad{}", super::quoted(&kit.name), kit.pads.len(), if kit.pads.len() == 1 { "" } else { "s" });
    if kit.pads.iter().any(|p| p.choke != Choke::None) {
        title += "; its hi-hats choke each other";
    }
    title
}

fn note_of(kit: &Kit, choke_found: bool) -> String {
    let pads: Vec<String> = kit.pads.iter().map(|p| format!("{} {}", note_name(p.note), p.name)).collect();
    let midi = match kit.layout {
        Layout::Tracks => format!("Notes go on {} (the track this made)", super::quoted(&format!("{} MIDI", kit.name))),
        Layout::Container => format!("Notes go on {} (the track this made)", super::quoted(&kit.name)),
    };
    let mut note = format!("{midi}; pads: {}.", pads.join(", "));
    if kit.pads.iter().any(|p| p.choke != Choke::None) && !choke_found {
        note += " REAPER has no JS: MIDI Choke here, so the hi-hats don't choke.";
    }
    note
}

/// The kit's note, and that its files in Kumi's own folder play from copies in the project's media folder.
fn note_with_copies(ctx: &Ctx, kit: &Kit, given: &[String], choke_found: bool) -> String {
    let mut note = note_of(kit, choke_found);
    if given.iter().any(|path| super::kumi_files::is_kumis(ctx, path)) {
        note += " Pads whose files are in Kumi's own folder, which Kumi clears, play copies in the project's media folder.";
    }
    note
}

pub fn preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let kit = kit_of(ctx, input)?;
    let given: Vec<String> = kit.pads.iter().map(|pad| pad.path.clone()).collect();
    Ok(Preview {
        target: json!({"ref": null, "kind": "track", "index": kit.at}),
        prior: json!({"tracks": ctx.reaper.track_count(ctx.proj)}),
        fence: fence_of(ctx, &kit),
        display: json!({"title": title_of(&kit), "note": note_with_copies(ctx, &kit, &given, true)}),
    })
}

/// A parameter of the sampler by its name (REAPER's names; its place in 7.82 when the name isn't found).
fn param_named(reaper: &Reaper, fx: Fx, name: &str, place: i32) -> i32 {
    (0..reaper.fx_param_count(fx)).find(|p| reaper.param_name(fx, *p).eq_ignore_ascii_case(name)).unwrap_or(place)
}

/// One pad's sampler, set: its file, played as it is, on its one note, obeying note-offs or not.
fn set_pad(reaper: &Reaper, fx: Fx, pad: &Pad) -> Done<()> {
    // (parameter writes: REAPER would record them under a global override in touch, latch or write)
    let track = match fx.owner {
        FxOwner::Track(track) => Some(track),
        FxOwner::Take(_) => None,
    };
    let _quiet = fxchain::QuietAutomation::around(reaper, track);
    reaper.fx_set_config(fx, "FILE0", &pad.path);
    reaper.fx_set_config(fx, "DONE", "");
    reaper.fx_set_config(fx, "MODE", "1");
    let note = pad.note as f64 / 127.0;
    reaper.fx_set_param(fx, param_named(reaper, fx, "Note range start", 3), note);
    reaper.fx_set_param(fx, param_named(reaper, fx, "Note range end", 4), note);
    reaper.fx_set_param(fx, param_named(reaper, fx, "Obey note-offs", 11), if pad.obey { 1.0 } else { 0.0 });
    // (REAPER answers whether a file took only by reading it back)
    if reaper.fx_config(fx, "FILE0").is_none_or(|file| file.trim() != pad.path) {
        return Err(failed(format!("the sampler wouldn't take {}", pad.path)));
    }
    Ok(())
}

/// The choke ahead of the pads in a chain, at `place` on: one JS: MIDI Choke a run of open-hat notes. False when REAPER
/// has no such JS (the kit is made without the choke, and says so).
fn add_chokes(reaper: &Reaper, track: Track, place: i32, plan: &ChokePlan, channel: i64) -> bool {
    let _quiet = fxchain::QuietAutomation::around(reaper, Some(track));
    let owner = FxOwner::Track(track);
    for (n, (start, count)) in plan.cuts.iter().enumerate() {
        let at = place + n as i32;
        if reaper.fx_add(owner, CHOKE, false, Some(at)) < 0 {
            return false;
        }
        let fx = Fx { owner, index: at };
        let run = plan.closes.windows(2).take_while(|w| w[1] == w[0] + 1).count() + 1;
        reaper.fx_set_param_raw(fx, CHOKE_CHANNEL, channel as f64);
        reaper.fx_set_param_raw(fx, CHOKE_START, plan.closes[0] as f64);
        reaper.fx_set_param_raw(fx, CHOKE_COUNT, run as f64);
        reaper.fx_set_param_raw(fx, CUT_START, *start as f64);
        reaper.fx_set_param_raw(fx, CUT_COUNT, *count as f64);
        for (slider, note) in CHOKE_EXTRA.iter().zip(plan.closes.iter().skip(run)) {
            reaper.fx_set_param_raw(fx, *slider, *note as f64);
        }
    }
    true
}

fn by_guid(ctx: &Ctx, guid: &str) -> Option<Track> {
    ctx.reads.track_by_id(ctx.reaper, ctx.proj, &TrackId::Guid(guid.to_string())).ok()
}

/// Takes away what a kit that went wrong made, last first, and puts the other tracks' folders back as they were (`depths`);
/// `uncertain` when REAPER wouldn't.
fn take_back(ctx: &Ctx, made: &[String], depths: &[(String, i64)], why: Fault) -> Fault {
    for guid in made.iter().rev() {
        if let Some(track) = by_guid(ctx, guid) {
            ctx.reaper.delete_track(track);
        }
    }
    super::tracks::put_folder_depths_back(ctx, depths);
    if made.iter().any(|guid| by_guid(ctx, guid).is_some()) {
        ("uncertain".into(), format!("{}, and Kumi couldn't take away the tracks it made: read the tracks", why.1))
    } else {
        (why.0, format!("{}; what Kumi made is taken away again", why.1))
    }
}

/// A new track at `at`, named.
fn new_track(ctx: &Ctx, at: i64, name: &str, made: &mut Vec<String>) -> Done<Track> {
    let track = ctx.reaper.insert_track(ctx.proj, at as i32).ok_or_else(|| failed("REAPER wouldn't add a track"))?;
    made.push(ctx.reaper.track_guid(track));
    ctx.reaper.set_track_string(track, "P_NAME", name);
    Ok(track)
}

/// What a kit's build made: the track the notes go on, each pad's sampler (its GUID and file), and whether the choke is in.
struct Built {
    notes: String,
    samplers: Vec<(String, String)>,
    choke: bool,
}

fn build(ctx: &Ctx, kit: &Kit, made: &mut Vec<String>) -> Done<Built> {
    let reaper = ctx.reaper;
    let plan = chokes_of(kit)?;
    let mut samplers = Vec::new();
    let mut choke = plan.is_none();
    match kit.layout {
        Layout::Tracks => {
            let folder = new_track(ctx, kit.at, &kit.name, made)?;
            reaper.set_track_value(folder, "I_FOLDERDEPTH", 1.0);
            let midi = new_track(ctx, kit.at + 1, &format!("{} MIDI", kit.name), made)?;
            let mut pads = Vec::new();
            for (n, pad) in kit.pads.iter().enumerate() {
                let track = new_track(ctx, kit.at + 2 + n as i64, &pad.name, made)?;
                if let Some(db) = pad.volume {
                    reaper.set_track_value(track, "D_VOL", 10f64.powf(db / 20.0));
                }
                let owner = FxOwner::Track(track);
                if reaper.fx_add(owner, SAMPLER, false, Some(0)) < 0 {
                    return Err(failed("REAPER has no ReaSamplOmatic5000 here"));
                }
                let fx = Fx { owner, index: 0 };
                set_pad(reaper, fx, pad)?;
                samplers.push((reaper.fx_guid(fx), pad.path.clone()));
                pads.push(track);
            }
            if let Some(last) = pads.last() {
                reaper.set_track_value(*last, "I_FOLDERDEPTH", -1.0);
            }
            // MIDI only, every channel as it is, from the MIDI track to each pad (after its FX: the choke's note-offs too).
            for pad in &pads {
                let index = reaper.create_send(midi, Some(*pad)).ok_or_else(|| failed("REAPER wouldn't send MIDI to a pad"))?;
                reaper.set_send_value(midi, 0, index, "I_SRCCHAN", -1.0);
                reaper.set_send_value(midi, 0, index, "I_MIDIFLAGS", 0.0);
            }
            if let Some(plan) = &plan {
                choke = add_chokes(reaper, midi, 0, plan, kit.channel);
                if !choke {
                    remove_chokes(reaper, midi);
                }
            }
            for pad in &kit.pads {
                reaper.set_note_name(ctx.proj, midi, pad.note as i32, -1, &pad.name);
            }
            Ok(Built { notes: reaper.track_guid(midi), samplers, choke })
        }
        Layout::Container => {
            let track = new_track(ctx, kit.at, &kit.name, made)?;
            let owner = FxOwner::Track(track);
            let chokes = match &plan {
                Some(plan) => {
                    choke = add_chokes(reaper, track, 0, plan, kit.channel);
                    if !choke {
                        remove_chokes(reaper, track);
                    }
                    reaper.fx_count(owner)
                }
                None => 0,
            };
            let holder = Holder::Track { track, input: false };
            reaper.fx_add(owner, "Container", false, Some(chokes));
            let guids = fxchain::guids(reaper, holder);
            let guid = guids
                .get(chokes as usize)
                .cloned()
                .filter(|_| guids.len() == chokes as usize + 1)
                .ok_or_else(|| failed("REAPER wouldn't add the container"))?;
            let container = fxchain::adopt(ctx, holder, &guid)?.fx;
            if !fxchain::is_container(reaper, container) {
                return Err(failed("REAPER added something other than a container"));
            }
            reaper.fx_set_config(container, "renamed_name", &kit.name);
            for (n, pad) in kit.pads.iter().enumerate() {
                let (address, _) = fxchain::add_place(reaper, Holder::Container(container), n)?;
                reaper.fx_add(owner, SAMPLER, false, Some(address));
                let children = reaper.fx_children(container);
                let fx = *children
                    .get(n)
                    .filter(|_| children.len() == n + 1)
                    .ok_or_else(|| failed("REAPER wouldn't put a sampler in the container"))?;
                set_pad(reaper, fx, pad)?;
                samplers.push((reaper.fx_guid(fx), pad.path.clone()));
            }
            for pad in &kit.pads {
                reaper.set_note_name(ctx.proj, track, pad.note as i32, -1, &pad.name);
            }
            Ok(Built { notes: reaper.track_guid(track), samplers, choke })
        }
    }
}

/// Takes off the chokes a chain got before REAPER refused one (the kit goes on without them).
fn remove_chokes(reaper: &Reaper, track: Track) {
    let owner = FxOwner::Track(track);
    for index in (0..reaper.fx_count(owner)).rev() {
        let fx = Fx { owner, index };
        if reaper.fx_name(fx).to_lowercase().contains("midi_choke") || reaper.fx_name(fx).contains("MIDI Choke") {
            reaper.fx_delete(fx);
        }
    }
}

/// Where each pad's sampler is now and the file it holds, by its GUID: None when it isn't in a track the kit made.
fn sampler_file(ctx: &Ctx, guid: &str) -> Option<String> {
    let fx = ctx.reads.fx_by_guid(ctx.reaper, ctx.proj, guid).ok()?;
    ctx.reaper.fx_config(fx, "FILE0").map(|f| f.trim().to_string())
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    // (the same input passed the preview: what fails now changed on disk or in the project since)
    let kit = kit_of(ctx, input).map_err(|fault| {
        if fault.0 == "bad-params" {
            changed(format!("{}: it changed since the preview", fault.1))
        } else {
            fault
        }
    })?;
    if &fence_of(ctx, &kit) != fence {
        return Err(changed("the tracks, or a pad's file, changed since the preview"));
    }
    // (a pad's file in Kumi's own folder plays from a copy in the project's media folder)
    let given: Vec<String> = kit.pads.iter().map(|pad| pad.path.clone()).collect();
    let mut kit = kit;
    for pad in &mut kit.pads {
        pad.path = super::kumi_files::playable(ctx, &pad.path)?;
    }
    let mut made = Vec::new();
    let depths = super::tracks::folder_depths(ctx);
    let built = match build(ctx, &kit, &mut made) {
        Ok(built) => built,
        Err(why) => return Err(take_back(ctx, &made, &depths, why)),
    };
    let reaper = ctx.reaper;
    let placed: Vec<i64> =
        made.iter().filter_map(|g| by_guid(ctx, g)).map(|t| reaper.track_value(t, "IP_TRACKNUMBER") as i64 - 1).collect();
    if placed.len() != made.len() || placed.iter().enumerate().any(|(n, p)| *p != kit.at + n as i64) {
        return Err(take_back(ctx, &made, &depths, failed("REAPER put the kit's tracks elsewhere than asked")));
    }
    let tracks: Vec<Value> = made
        .iter()
        .filter_map(|guid| by_guid(ctx, guid).map(|t| json!({"guid": guid, "signature": templates::signature(ctx, t)})))
        .collect();
    let track_refs: Vec<String> = made.iter().map(|guid| refs::track_ref(&TrackId::Guid(guid.clone()))).collect();
    let notes_ref = refs::track_ref(&TrackId::Guid(built.notes.clone()));
    let pads: Vec<Value> = built.samplers.iter().map(|(guid, file)| json!({"fx": guid, "file": file})).collect();
    Ok(Applied {
        applied: json!({"index": kit.at, "layout": if kit.layout == Layout::Tracks { "tracks" } else { "container" }, "tracks": tracks, "pads": pads, "choke": built.choke}),
        created: Some(json!({"ref": notes_ref, "kind": "track", "trackRefs": track_refs})),
        material: None,
        display: json!({"title": title_of(&kit), "note": note_with_copies(ctx, &kit, &given, built.choke)}),
        ..Default::default()
    })
}

pub fn restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let reaper = ctx.reaper;
    let tracks = restore.applied["tracks"].as_array().ok_or_else(|| bad("applied: the tracks the kit made"))?;
    let made: Vec<String> = tracks.iter().filter_map(|t| t["guid"].as_str().map(str::to_string)).collect();
    let present: Vec<(String, Track)> = made.iter().filter_map(|guid| by_guid(ctx, guid).map(|t| (guid.clone(), t))).collect();
    if present.is_empty() {
        return Err(("gone".into(), "the kit's tracks aren't in the project any more".into()));
    }
    if !restore.discard {
        if present.len() != made.len() {
            return Err(changed("some of the kit's tracks were deleted in REAPER since: the others stay"));
        }
        // A pad's file swapped in its sampler is the producer's (REAPER keeps it in the plug-in's state, which the signature doesn't read).
        for pad in restore.applied["pads"].as_array().into_iter().flatten() {
            let (Some(guid), Some(file)) = (pad["fx"].as_str(), pad["file"].as_str()) else { continue };
            if sampler_file(ctx, guid).as_deref() != Some(file) {
                return Err(changed("a pad's sampler holds another sound now (or went): the producer's"));
            }
        }
        for entry in tracks {
            let Some((_, track)) = present.iter().find(|(guid, _)| entry["guid"].as_str() == Some(guid.as_str())) else { continue };
            if !templates::as_left(ctx, *track, &entry["signature"]) {
                return Err(changed(format!(
                    "{} has been changed in REAPER since the kit made it (its items, mix, routing or plug-ins)",
                    super::quoted(&super::track_name(reaper, *track))
                )));
            }
        }
        let places: Vec<i64> = present.iter().map(|(_, t)| reaper.track_value(*t, "IP_TRACKNUMBER") as i64 - 1).collect();
        if places.windows(2).any(|w| w[1] != w[0] + 1) {
            return Err(changed("the kit's tracks aren't together any more: other tracks were moved among them"));
        }
    }
    // No track of the producer's inside the kit's folder, even told to undo anyway: deleting the folder would move it.
    let inside = (0..reaper.track_count(ctx.proj)).filter_map(|at| reaper.track_at(ctx.proj, at)).any(|other| {
        !present.iter().any(|(_, t)| *t == other)
            && reaper.track_parent(other).is_some_and(|parent| present.iter().any(|(_, t)| *t == parent))
    });
    if inside {
        return Err(changed("a track of the producer's is inside the kit's folder now: Kumi won't delete the folder around it"));
    }
    let name = present.first().map(|(_, t)| super::track_name(reaper, *t)).unwrap_or_default();
    // Every other track stays in the folder it is in (a kit at the end of a folder closes it, which a bare delete loses).
    let kit: Vec<Track> = present.iter().map(|(_, t)| *t).collect();
    super::tracks::delete_keeping_folders(ctx, &kit, Some(&[]))?;
    Ok(Restored {
        restored: json!({"deleted": present.len()}),
        display: json!({"title": format!("Took away the drum kit {} ({} track{})", super::quoted(&name), present.len(), if present.len() == 1 { "" } else { "s" })}),
        ..Default::default()
    })
}

#[cfg(test)]
mod words {
    use super::*;

    #[test]
    fn notes_and_hats_are_read_as_reaper_and_drummers_name_them() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(36), "C2");
        assert_eq!(note_name(42), "F#2");
        assert_eq!(hat("Closed Hat"), Some(false));
        assert_eq!(hat("HH Open 2"), Some(true));
        assert_eq!(hat("hihat_open"), Some(true));
        assert_eq!(hat("Kick"), None);
        assert_eq!(hat("Shaker"), None);
    }
}
