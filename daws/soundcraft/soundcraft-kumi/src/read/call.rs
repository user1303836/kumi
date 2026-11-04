//! `read {kind: "call", command, params?}`: one of SoundCraft's engine commands by its id, as REAPER's `call` reaches
//! REAPER's API functions, for what the rows don't show. Only the commands on [`READ_ONLY`] run, each one SoundCraft
//! 0.17.0's source shows changes nothing. Anything else is refused before it's sent: a command that changes the session
//! or the app (or only opens a window or a dialog) as one Kumi can't run yet, a method of the control channel as one
//! `call` doesn't reach, and an id SoundCraft hasn't got with the ids near it, from the app's own list.
//!
//! A command goes as `engine.execute {command, params}`, which the app runs through `SoundApp::run`
//! (crates/ui-egui/src/app/commands.rs:5): its UI commands first (menus.rs:511), none of which is on the list, then
//! `Engine::execute` (crates/engine/src/lib.rs:392). A `query` command (cmd.rs:120: not journaled, not undoable, not a
//! view command) pushes no undo step and adds nothing to the command journal; with the document as it was, the dirty
//! flag and the last selection stay as they were (lib.rs:423-436). A command the app refuses still puts its words in
//! the app's status line (commands.rs:42), as a refused read does.
//!
//! Refs, never ids, cross to Kumi. A param that names an object takes the object's ref, checked as `read` checks one
//! (with its epoch; stale or ended, it's refused), and goes to the app as the object's id. The ids in an answer whose
//! [`Shape`] names them go back as refs, under keys that hold refs. An answer past the bytes a read may take is cut,
//! and says where under `truncated`.
use super::{asked, bad, gone, lane_id, slot_of, text, wait_within, Reads, Refusal, ANSWER_BYTES};
use crate::control::Error;
use crate::refs::{Id, Kind, Lives, Ref};
use daw_bridge::wire::code;
use serde_json::{json, Map, Value};
use std::time::Instant;

/// What a param that names objects takes instead of their ids: one ref, or a list of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Takes {
    One(Kind),
    Many(Kind),
}

/// The shape of a command's answer, for the ids in it to go back as refs. Each is as `crate::read`'s rows name them:
/// - `Session`: `session.inspect`'s (crates/engine/src/inspect.rs:73): its tracks as `Track`, its busses', markers',
///   groups' and sources' `id` as `ref`, a group's `members` as `memberRefs`, the selection's `tracks` and `clips` as
///   `trackRefs` and `clipRefs`.
/// - `Track`: a track's (inspect.rs:35): its `id` as `ref`, an input or output to a bus as `inputRef` or `outputRef`,
///   each insert in slots a to j and each send with its `ref`, a send to a bus as `targetRef`, each automation lane its
///   ref can spell with its `ref`, and its clips as `Clip`.
/// - `Clip`: a clip's (inspect.rs:8, and :99 with its track): its `id` as `ref`, an audio clip's `source` as
///   `sourceRef`, and its `track` as `trackRef`.
/// - `Health`: `session.audio_health`'s issues (crates/engine/src/cmd/health.rs:33): `track_id`, `clip_id` and
///   `source_id` as `trackRef`, `clipRef` and `sourceRef`.
/// - `Strip`: `edit.strip_silence_preview`'s tracks (strip_silence.rs:46): `track` as `trackRef`.
///
/// An id the bridge's lives don't know (an object made since its last look) goes back as a null ref, or out of a list.
/// What no row names stays as SoundCraft writes it: a clip's `video` and `video.list`'s movie ids (no ref names a
/// movie), and the track ids inside settings' names, `view.flags`' `object.<track>` and `event.midi_ops_window`'s `rtp`
/// keys, `<track>.<property>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// No object's id in it.
    Plain,
    Session,
    Track,
    Clip,
    Health,
    Strip,
}

/// A command `call` runs.
pub struct Command {
    pub id: &'static str,
    /// The params that name objects by SoundCraft's id, and what each takes from Kumi instead.
    pub objects: &'static [(&'static str, Takes)],
    pub shape: Shape,
    /// Why it changes nothing.
    pub why: &'static str,
}

const fn read_only(id: &'static str, objects: &'static [(&'static str, Takes)], shape: Shape, why: &'static str) -> Command {
    Command { id, objects, shape, why }
}

/// The commands `call` runs: SoundCraft 0.17.0's `query` commands (paths from its crates/engine/src/cmd/), each of which
/// reads the engine or the session and changes neither, nor anything else of the app's. Their menu paths, where they
/// have one, are where the app's menu opens a window showing what they answer; the command itself opens nothing.
///
/// Commands that look like reads and aren't on it:
/// - `transport.status` (transport.rs:42) and `setup.preferences` (options.rs:81): `noundo`, so journaled (lib.rs:433):
///   each call takes a place in the engine's command journal, a ring of 10,000 that drops its oldest (lib.rs:33). What
///   they answer, `session.inspect`'s `transport` and the edit settings, `session.inspect` and `view.flags` read.
/// - `view.clip_gain_info` and `view.clip_effects_status` (view_more.rs:149-150): View > Clip toggles (view_more.rs:13,
///   more_util.rs:12), which turn their flag over when given no `value`.
/// - `file.export_session_text` (file.rs:114): writes its text to `path` when given one; journaled.
/// - `file.score_setup` (file.rs:155), `event.midi_track_offsets` (event_more.rs:118), `setup.io` (setup_more.rs:66) and
///   the other Setup commands: they answer what's set when given nothing, set what their params name, and are journaled
///   or undoable.
/// - `setup.playback_engine` and `setup.hardware`: called, the app applies the open device (ui-egui menus.rs:523).
/// - `ui.theme` (menus.rs:687): answers the theme without `mode`, and changes it with one.
/// - `window.undo_history`, `window.session_info` and `window.audio_health` (menus.rs:561-569): they toggle the windows
///   that show what `engine.history`, `file.get_info` and `session.audio_health` answer.
/// - `engine.clap_plugins` (clap.rs:11) and `engine.vst3_plugins` (vst3.rs:11): `query` commands, but their first scan
///   (and any `rescan`) loads every plug-in binary inside SoundCraft's own process (clap-host's lib.rs:149), so it can run
///   past a read's deadline or take the app down. Nothing of Kumi's scans plug-ins in the app: plug-in lists come from a
///   worker of their own.
pub const READ_ONLY: [Command; 18] = [
    // query.rs:9
    read_only("session.inspect", &[], Shape::Session, "the session as inspect.rs:73 shows it, from &Engine"),
    // query.rs:10
    read_only("track.inspect", &[("track", Takes::One(Kind::Track))], Shape::Track, "one track as inspect.rs:95 shows it"),
    // query.rs:14
    read_only("clip.inspect", &[("clip", Takes::One(Kind::Clip))], Shape::Clip, "one clip, with its notes, as inspect.rs:99 shows it"),
    // query.rs:18
    read_only("engine.commands", &[], Shape::Plain, "the engine's command table (cmd.rs:129), described"),
    // query.rs:27
    read_only("engine.history", &[], Shape::Plain, "the undo history's labels and whether there's a redo (lib.rs:333)"),
    // query.rs:28
    read_only("engine.plugins", &[], Shape::Plain, "the built-in plug-ins' static descriptions"),
    // query.rs:32
    read_only("engine.parity", &[], Shape::Plain, "the menu catalog's parity report (catalog.rs:109), built from static tables"),
    // query.rs:33
    read_only("session.format_time", &[], Shape::Plain, "a position as text in a counter format, from the session"),
    // health.rs:11
    read_only("session.audio_health", &[], Shape::Health, "its audio clips' loaded audio and bounds, from the session (health.rs:64)"),
    // file.rs:196
    read_only("file.get_info", &[], Shape::Session, "the session's summary, as session.inspect's (inspect.rs:73)"),
    // midi.rs:12
    read_only("midi.notes", &[("clip", Takes::One(Kind::Clip))], Shape::Plain, "a MIDI clip's notes, from the session (midi.rs:38)"),
    // event_more.rs:30
    read_only("event.time_ops_window", &[], Shape::Plain, "the tempo and meter maps, song start and selection (event_more.rs:159)"),
    // event_more.rs:31
    read_only("event.tempo_ops_window", &[], Shape::Plain, "the same as event.time_ops_window"),
    // event_more.rs:32
    read_only("event.midi_ops_window", &[], Shape::Plain, "input quantize, stored performances and real-time properties"),
    // setup_more.rs:87
    read_only("setup.keyboard_shortcuts", &[], Shape::Plain, "the command table's shortcuts"),
    // video.rs:32
    read_only("video.list", &[], Shape::Plain, "the session's movies, and whether each file is found (a look, no write)"),
    // view_more.rs:190
    read_only("view.flags", &[], Shape::Plain, "the view and option flags that are on, and the rulers"),
    // strip_silence.rs:11
    read_only(
        "edit.strip_silence_preview",
        &[("tracks", Takes::Many(Kind::Track)), ("track", Takes::One(Kind::Track))],
        Shape::Strip,
        "the ranges Strip Silence would keep, scanned from a copy of the session (strip_silence.rs:38)",
    ),
];

/// The control channel's own methods (crates/ui-egui/src/control.rs:446-566), which `call` never reaches: it sends
/// `engine.execute`. (`session.inspect`, `engine.commands` and `engine.parity` are engine commands too, and on the list.)
const CHANNEL: [&str; 19] = [
    "engine.execute",
    "command",
    "engine.commands",
    "engine.parity",
    "session.inspect",
    "document.inspect",
    "ui.inspect",
    "ui.menu.list",
    "ui.menu.invoke",
    "ui.set",
    "ui.move",
    "ui.click",
    "ui.drag",
    "ui.scroll",
    "ui.zoom",
    "ui.key",
    "ui.text",
    "ui.screenshot",
    "app.quit",
];

/// The plug-in scans, `query` commands kept off the list: see [`READ_ONLY`].
const SCANS: [&str; 2] = ["engine.clap_plugins", "engine.vst3_plugins"];

impl Reads {
    /// One `call`: `{command, params?, epoch?}`, with the epoch the refs in `params` came from. Not started after
    /// `until`.
    pub(super) async fn call(&mut self, params: &Value, until: Instant) -> Result<Value, Refusal> {
        let command = text(params, "command")?
            .ok_or_else(|| bad("command: the id of one of SoundCraft's read-only commands, as engine.commands lists them".into()))?;
        let epoch = text(params, "epoch")?;
        let mut sent = match params.get("params") {
            None | Some(Value::Null) => json!({}),
            Some(given @ Value::Object(_)) => given.clone(),
            Some(_) => return Err(bad("params: an object, the command's params".into())),
        };
        let Some(read_only) = READ_ONLY.iter().find(|read_only| read_only.id == command) else {
            return Err(self.not_run(&command, until).await);
        };
        let named = named(read_only, &sent)?;
        if !named.is_empty() && epoch.is_none() {
            return Err(bad("epoch: the epoch of the answer the refs came from (SoundCraft gives ids out again)".into()));
        }
        // (the lives as the app shows them now, for the refs in and the ids out)
        if !named.is_empty() || !matches!(read_only.shape, Shape::Plain | Shape::Session) {
            self.inspect(until).await?;
        }
        if let Some(epoch) = epoch.filter(|epoch| epoch != self.epoch()) {
            return Err((
                code::STALE,
                format!(
                    "those refs are from another epoch ({epoch}; now {}): another session is open, or the bridge started again: read again",
                    self.epoch()
                ),
            ));
        }
        for (pointer, id) in named {
            let resolved = self.lives.resolve(&id).map_err(|missing| gone(missing, &Ref::Object(id)))?;
            if let Some(slot) = sent.pointer_mut(&pointer) {
                *slot = resolved.into();
            }
        }
        let mut result = self.control.execute(&command, sent, wait_within(until)?).await.map_err(|error| match error {
            Error::Refused(said) => (code::FAILED, format!("{command}: {said}")),
            other => asked(other),
        })?;
        if read_only.shape == Shape::Session {
            // (an inspection of the session is a look, which the lives learn from)
            self.saw(&result);
        }
        name_ids(read_only.shape, &mut result, &self.lives);
        let truncated = cut(&mut result, ANSWER_BYTES)?;
        let mut answer = json!({"command": command, "result": result, "epoch": self.epoch()});
        if !truncated.is_empty() {
            answer["truncated"] = truncated.into();
        }
        Ok(answer)
    }

    /// Why a command off the list isn't run: a channel method, a plug-in scan, a command that changes something (or only
    /// opens a window or a dialog), or none the app has, with the ids near it. The app's list is asked for once (until it
    /// lists some).
    async fn not_run(&mut self, command: &str, until: Instant) -> Refusal {
        if CHANNEL.contains(&command) {
            return bad(format!(
                "{command} is a method of SoundCraft's control channel, which call doesn't reach: call runs SoundCraft's read-only engine commands"
            ));
        }
        if SCANS.contains(&command) {
            return bad(format!(
                "{command} scans plug-ins inside SoundCraft's own process, which can outlast a read or take the app down: Kumi doesn't ask it to"
            ));
        }
        if self.commands.is_none() {
            let wait = match wait_within(until) {
                Ok(wait) => wait,
                Err(refused) => return refused,
            };
            match self.control.call("engine.commands", json!({}), wait).await {
                Ok(listed) => self.commands = Some(list_ids(&listed)).filter(|ids| !ids.is_empty()),
                Err(error) => return asked(error),
            }
        }
        let known = self.commands.as_deref().unwrap_or_default();
        if known.iter().any(|id| id == command) {
            return bad(format!("Kumi can't change SoundCraft yet: {command} {}", what_it_does(command)));
        }
        let near = near(command, known);
        if near.is_empty() {
            bad(format!("no such command in SoundCraft: {command} (engine.commands lists them)"))
        } else {
            bad(format!("no such command in SoundCraft: {command}; perhaps {}", near.join(", ")))
        }
    }
}

/// The ids in `engine.commands`'s answer.
fn list_ids(listed: &Value) -> Vec<String> {
    listed.as_array().into_iter().flatten().filter_map(|command| command["id"].as_str().map(str::to_string)).collect()
}

/// What a command off the list does, in its refusal: only opens or closes a window or a dialog (the app's UI commands,
/// crates/ui-egui/src/menus.rs:511), or changes the session or the app.
fn what_it_does(command: &str) -> &'static str {
    match command {
        "ui.new_tracks_dialog" | "ui.bounce_dialog" => "only opens a dialog",
        "window.config_new" | "window.config_update" | "window.config_auto_update" => "changes the session or the app",
        _ if command.starts_with("window.arrange") => "changes the session or the app",
        _ if command.starts_with("window.") => "only opens or closes a window",
        _ => "changes the session or the app",
    }
}

/// The ids that share the longest start with `command`, then those that hold it: five at most, as REAPER's bridge
/// suggests its functions (reaper-kumi's generic.rs).
fn near<'a>(command: &str, known: &'a [String]) -> Vec<&'a str> {
    let lower = command.to_ascii_lowercase();
    let shared = |id: &str| id.to_ascii_lowercase().bytes().zip(lower.bytes()).take_while(|(a, b)| a == b).count();
    let mut near: Vec<(usize, &str)> = known
        .iter()
        .map(|id| (shared(id).max(if id.to_ascii_lowercase().contains(&lower) { lower.len() } else { 0 }), id.as_str()))
        .filter(|(score, _)| *score >= 4.min(lower.len()).max(1))
        .collect();
    near.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().abs_diff(command.len()).cmp(&b.1.len().abs_diff(command.len()))));
    near.into_iter().take(5).map(|(_, id)| id).collect()
}

/// The refs in a command's params, where it takes objects: each object's place in `params` (a JSON pointer) and its
/// life, for its id to go there once checked. An id given where a ref goes, or a ref anywhere else, is refused.
fn named(command: &Command, params: &Value) -> Result<Vec<(String, Id)>, Refusal> {
    let mut named = vec![];
    let fields = params.as_object().into_iter().flatten();
    for (key, value) in fields {
        let pointer = format!("/{}", key.replace('~', "~0").replace('/', "~1"));
        match command.objects.iter().find(|(object, _)| object == key).map(|(_, takes)| *takes) {
            Some(Takes::One(kind)) => named.push((pointer, object(key, kind, value)?)),
            Some(Takes::Many(kind)) => {
                let Value::Array(items) = value else {
                    return Err(bad(format!("{key}: a list of {} refs (s:{}:…)", kind.word(), kind.word())));
                };
                for (index, item) in items.iter().enumerate() {
                    named.push((format!("{pointer}/{index}"), object(key, kind, item)?));
                }
            }
            None => {
                if let Some(found) = a_ref_in(value) {
                    return Err(bad(format!("{key}: {} takes no object there, so not {found}", command.id)));
                }
            }
        }
    }
    Ok(named)
}

/// The object a param names: a ref to one of `kind`. SoundCraft's own id is refused, as it gives ids out again.
fn object(key: &str, kind: Kind, value: &Value) -> Result<Id, Refusal> {
    let word = kind.word();
    match value {
        Value::Number(id) => Err(bad(format!(
            "{key}: pass a ref from read (s:{word}:…), not SoundCraft's id {id}: it gives ids out again, so an id can name another {word}"
        ))),
        Value::String(text) => match text.parse::<Ref>() {
            Ok(Ref::Object(id)) if id.kind == kind => Ok(id),
            Ok(other) => Err(bad(format!("{key}: a {word} ref (s:{word}:…), not {other}"))),
            Err(why) => Err(bad(format!("{key}: pass a ref from read: {why}"))),
        },
        other => Err(bad(format!("{key}: pass a ref from read (s:{word}:…), not {other}"))),
    }
}

/// A ref anywhere in a value.
fn a_ref_in(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => text.parse::<Ref>().is_ok().then(|| text.clone()),
        Value::Array(items) => items.iter().find_map(a_ref_in),
        Value::Object(fields) => fields.values().find_map(a_ref_in),
        _ => None,
    }
}

/// The ids in an answer of `shape` as refs.
fn name_ids(shape: Shape, answer: &mut Value, lives: &Lives) {
    match shape {
        Shape::Plain => {}
        Shape::Session => session(answer, lives),
        Shape::Track => track(answer, lives),
        Shape::Clip => clip(answer, lives),
        Shape::Health => {
            for issue in items(answer, "issues") {
                swap(issue, "track_id", "trackRef", Kind::Track, lives);
                swap(issue, "clip_id", "clipRef", Kind::Clip, lives);
                swap(issue, "source_id", "sourceRef", Kind::Source, lives);
            }
        }
        Shape::Strip => {
            for track in items(answer, "tracks") {
                swap(track, "track", "trackRef", Kind::Track, lives);
            }
        }
    }
}

fn session(session: &mut Value, lives: &Lives) {
    for track in items(session, "tracks") {
        self::track(track, lives);
    }
    for (key, kind) in [("busses", Kind::Bus), ("markers", Kind::Marker), ("groups", Kind::Group), ("sources", Kind::Source)] {
        for item in items(session, key) {
            swap(item, "id", "ref", kind, lives);
            if kind == Kind::Group {
                swap_all(item, "members", "memberRefs", Kind::Track, lives);
            }
        }
    }
    if let Some(selection) = session.get_mut("selection") {
        swap_all(selection, "tracks", "trackRefs", Kind::Track, lives);
        swap_all(selection, "clips", "clipRefs", Kind::Clip, lives);
    }
}

fn track(track: &mut Value, lives: &Lives) {
    let id = track["id"].as_u64().and_then(|id| lives.now(Kind::Track, id));
    swap(track, "id", "ref", Kind::Track, lives);
    route(track, "input", lives);
    route(track, "output", lives);
    for insert in items(track, "inserts") {
        let slot = slot_of(insert);
        insert["ref"] = id.zip(slot).map_or(Value::Null, |(track, slot)| Ref::Insert { track, slot }.to_string().into());
    }
    for send in items(track, "sends") {
        let slot = send["slot"].as_u64().and_then(|slot| u32::try_from(slot).ok()).filter(|slot| *slot < 10);
        send["ref"] = id.zip(slot).map_or(Value::Null, |(track, slot)| Ref::Send { track, slot }.to_string().into());
        route(send, "target", lives);
    }
    for lane in items(track, "automation") {
        let param = lane["param"].as_str().and_then(lane_id);
        lane["ref"] = id.zip(param).map_or(Value::Null, |(track, param)| Ref::Lane { track, param }.to_string().into());
    }
    for clip in items(track, "clips") {
        self::clip(clip, lives);
    }
}

fn clip(clip: &mut Value, lives: &Lives) {
    swap(clip, "id", "ref", Kind::Clip, lives);
    swap(clip, "source", "sourceRef", Kind::Source, lives);
    swap(clip, "track", "trackRef", Kind::Track, lives);
}

/// The objects in a list under `key`, to name.
fn items<'a>(of: &'a mut Value, key: &str) -> impl Iterator<Item = &'a mut Value> {
    of.get_mut(key).and_then(Value::as_array_mut).into_iter().flatten()
}

/// An object's id under `key` as its ref under `to` (null when the lives don't know it).
fn swap(item: &mut Value, key: &str, to: &str, kind: Kind, lives: &Lives) {
    let Some(fields) = item.as_object_mut() else { return };
    let Some(id) = fields.remove(key) else { return };
    let named = id.as_u64().and_then(|id| lives.now(kind, id)).map_or(Value::Null, |id| Ref::Object(id).to_string().into());
    fields.insert(to.into(), named);
}

/// A list of objects' ids under `key` as their refs under `to`, those the lives know.
fn swap_all(item: &mut Value, key: &str, to: &str, kind: Kind, lives: &Lives) {
    let Some(fields) = item.as_object_mut() else { return };
    let Some(ids) = fields.remove(key) else { return };
    let named: Vec<Value> = ids
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| Some(Ref::Object(lives.now(kind, id.as_u64()?)?).to_string().into()))
        .collect();
    fields.insert(to.into(), named.into());
}

/// A route to a bus (`{"Bus": 1}`) under `key` as the bus's ref under `<key>Ref`; any other stays.
fn route(item: &mut Value, key: &str, lives: &Lives) {
    let Some(bus) = item.get(key).and_then(|route| route.get("Bus")).and_then(Value::as_u64) else { return };
    let Some(fields) = item.as_object_mut() else { return };
    fields.remove(key);
    let named = lives.now(Kind::Bus, bus).map_or(Value::Null, |id| Ref::Object(id).to_string().into());
    fields.insert(format!("{key}Ref"), named);
}

/// Cuts an answer to `most` bytes, and says where: under each place cut (a JSON pointer into the read's answer), a
/// list's `total` items and those `shown`, or a text's `totalBytes` and `shownBytes`. Each turn cuts the smallest list
/// or text whose cutting alone can bring the answer within the bound (the one to blame, as deep as it is: a lane's
/// points before the track's lanes), else the biggest; a list keeps the items from its first that fit, a text its
/// start. An answer with nothing left to cut that's still past the bound is refused.
fn cut(answer: &mut Value, most: usize) -> Result<Map<String, Value>, Refusal> {
    let mut told = Map::new();
    loop {
        let size = size_of(answer);
        let Some(over) = size.checked_sub(most).filter(|over| *over > 0) else { return Ok(told) };
        let mut found = Found::default();
        sizes(answer, &mut String::new(), over, &mut found);
        let Some((pointer, its)) = found.alone.or(found.biggest) else {
            return Err((
                code::FAILED,
                format!("the answer is {} KiB, past the {} KiB a read may take, with no list or text to cut", size / 1024, most / 1024),
            ));
        };
        let room = its - over;
        let place = format!("/result{pointer}");
        let before = told.get(&place).cloned().unwrap_or(Value::Null);
        match answer.pointer_mut(&pointer) {
            Some(Value::Array(items)) => {
                let total = before["total"].as_u64().unwrap_or(items.len() as u64);
                let (mut used, mut keep) = (2, 0);
                for item in items.iter() {
                    used += size_of(item) + usize::from(keep > 0);
                    if used > room {
                        break;
                    }
                    keep += 1;
                }
                items.truncate(keep);
                told.insert(place, json!({"total": total, "shown": keep}));
            }
            Some(Value::String(text)) => {
                let total = before["totalBytes"].as_u64().unwrap_or(text.len() as u64);
                // (its quotes aside; a text whose escapes make it longer is cut again next turn)
                let mut end = room.saturating_sub(2).min(text.len() - 1);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                told.insert(place, json!({"totalBytes": total, "shownBytes": end}));
            }
            _ => unreachable!("found a list or a text there"),
        }
    }
}

/// The lists and texts (not empty) an answer `over` its bound could be cut in: where, and their bytes.
#[derive(Default)]
struct Found {
    /// The smallest whose cutting alone can bring the answer within its bound.
    alone: Option<(String, usize)>,
    biggest: Option<(String, usize)>,
}

/// A value's bytes as compact JSON, as `serde_json` writes it, with what in it could be cut found on the way. `pointer`
/// is where the value is.
fn sizes(value: &Value, pointer: &mut String, over: usize, found: &mut Found) -> usize {
    let size = match value {
        Value::Array(items) => {
            let mut size = 2 + items.len().saturating_sub(1);
            for (index, item) in items.iter().enumerate() {
                let len = pointer.len();
                pointer.push_str(&format!("/{index}"));
                size += sizes(item, pointer, over, found);
                pointer.truncate(len);
            }
            size
        }
        Value::Object(fields) => {
            let mut size = 2 + fields.len().saturating_sub(1);
            for (key, field) in fields {
                let len = pointer.len();
                pointer.push('/');
                pointer.push_str(&key.replace('~', "~0").replace('/', "~1"));
                size += size_of(&Value::String(key.clone())) + 1 + sizes(field, pointer, over, found);
                pointer.truncate(len);
            }
            size
        }
        other => size_of(other),
    };
    let cuttable = match value {
        Value::Array(items) => !items.is_empty(),
        Value::String(text) => !text.is_empty(),
        _ => false,
    };
    if cuttable {
        // (emptied, a list or a text keeps its two brackets or quotes)
        if size - 2 >= over && found.alone.as_ref().is_none_or(|(_, smallest)| size < *smallest) {
            found.alone = Some((pointer.clone(), size));
        }
        if found.biggest.as_ref().is_none_or(|(_, biggest)| size > *biggest) {
            found.biggest = Some((pointer.clone(), size));
        }
    }
    size
}

fn size_of(value: &Value) -> usize {
    serde_json::to_string(value).map_or(0, |text| text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ids SoundCraft 0.17.0 lists (`engine.commands`, recorded in tests/vectors).
    fn recorded() -> Vec<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/soundcraft-0.17.0.ndjson");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["method"], "engine.commands");
        list_ids(&first["reply"]["result"])
    }

    #[test]
    fn every_command_call_runs_is_one_soundcraft_lists() {
        let known = recorded();
        assert_eq!(known.len(), 551);
        for command in &READ_ONLY {
            assert!(known.iter().any(|id| id == command.id), "{} isn't a command of SoundCraft 0.17.0's", command.id);
            assert!(!command.why.is_empty());
        }
        let mut ids: Vec<&str> = READ_ONLY.iter().map(|command| command.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), READ_ONLY.len(), "each once");
        for scan in SCANS {
            assert!(known.iter().any(|id| id == scan) && !ids.contains(&scan), "{scan}: a command of 0.17.0's, kept off the list");
        }
    }
}
