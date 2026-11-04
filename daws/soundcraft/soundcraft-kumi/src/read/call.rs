//! `read {kind: "call", command, params?}`: one of SoundCraft's engine commands by its id, as REAPER's `call` reaches
//! REAPER's API functions, for what the rows don't show. Only the commands on [`READ_ONLY`] run, with the params each
//! takes, and only on the SoundCraft its list was checked against ([`CHECKED_FOR`]): each one SoundCraft 0.17.0's source
//! shows changes nothing. Anything else is refused before it's sent, with the ids that run: a look-alike as one Kumi
//! doesn't read, and why ([`LOOK_ALIKES`], [`SCANS`]), a command that changes the session or the app (or only opens a
//! window or a dialog) as one Kumi can't run yet, a method of the control channel as one `call` doesn't reach, and an
//! id SoundCraft hasn't got (the app's own list says) with the ones that run near it.
//!
//! A command goes as `engine.execute {command, params}`, which the app runs through `SoundApp::run`
//! (crates/ui-egui/src/app/commands.rs:5): its UI commands first (menus.rs:511), none of which is on the list, then
//! `Engine::execute` (crates/engine/src/lib.rs:392). A `query` command (cmd.rs:120: not journaled, not undoable, not a
//! view command) pushes no undo step and adds nothing to the command journal; with the document as it was, the dirty
//! flag and the last selection stay as they were (lib.rs:423-436). The app puts a refused command's words in its status
//! line (commands.rs:42), so what the session `call` looks at shows is checked first, as `read` checks it: an object
//! param is required and there now, `midi.notes`'s clip is MIDI, and `session.format_time`'s `at` is a number.
//!
//! Refs, never ids, cross to Kumi. A param that names an object takes the object's ref, checked as `read` checks one
//! (with its epoch; stale or ended, it's refused), and goes to the app as the object's id. The ids in an answer whose
//! [`Shape`] names them go back as refs, under keys that hold refs. An answer past what a model takes whole
//! ([`CALL_ANSWER_BYTES`]) is cut, and says where under `truncated`.
use super::{asked, bad, gone, lane_id, list, slot_of, text, tracks, wait_within, Reads, Refusal};
use crate::control::Error;
use crate::refs::{Id, Kind, Lives, Missing, Ref};
use daw_bridge::wire::code;
use serde_json::{json, Map, Value};
use std::time::Instant;

/// The SoundCraft whose commands [`READ_ONLY`] was checked against: 0.17.x. `call` runs on no other.
pub const CHECKED_FOR: &str = "0.17";

/// The bytes a call's answer (its `result`) may take: what a model takes whole. Kumi's kernel hands the model at most
/// 64 KB of a tool's text (kumi-agent's kernel/agent.rs, `MAX_TOOL_RESULT`), and its read pages aim at 60 KiB
/// (kumi-daw-own's bridge.rs, `PAGE_TEXT_BYTES`). A call has no pages, so an answer cut only at the 224 KiB a frame may
/// take would reach the model cut again mid-entry, without the `truncated` that says where. Cut here, under that
/// budget, it arrives whole with its `truncated`, and Kumi needs no cutter of its own.
pub const CALL_ANSWER_BYTES: usize = 56 * 1024;

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
}

/// A command `call` runs.
pub struct Command {
    pub id: &'static str,
    /// The params it takes, by SoundCraft's names (its handler's reads of them): any other is refused.
    pub params: &'static [&'static str],
    /// Those of them that name an object by SoundCraft's id, required, and the kind of ref each takes instead.
    pub objects: &'static [(&'static str, Kind)],
    pub shape: Shape,
    /// Why it changes nothing.
    pub why: &'static str,
}

const fn read_only(
    id: &'static str,
    params: &'static [&'static str],
    objects: &'static [(&'static str, Kind)],
    shape: Shape,
    why: &'static str,
) -> Command {
    Command { id, params, objects, shape, why }
}

/// The commands `call` runs: SoundCraft 0.17.0's `query` commands (paths from its crates/engine/src/cmd/), each of which
/// reads the engine or the session and changes neither, nor anything else of the app's, with the params its handler
/// reads. Their menu paths, where they have one, are where the app's menu opens a window showing what they answer; the
/// command itself opens nothing. The commands that look like reads and aren't on it are [`LOOK_ALIKES`] and [`SCANS`].
pub const READ_ONLY: [Command; 17] = [
    // query.rs:9 (`str_param(p, "detail")`)
    read_only("session.inspect", &["detail"], &[], Shape::Session, "the session as inspect.rs:73 shows it, from &Engine"),
    // query.rs:10-13 (`track_param(…, "track")`)
    read_only("track.inspect", &["track"], &[("track", Kind::Track)], Shape::Track, "one track as inspect.rs:95 shows it"),
    // query.rs:14-17 (`p.get("clip")`)
    read_only("clip.inspect", &["clip"], &[("clip", Kind::Clip)], Shape::Clip, "one clip, with its notes, as inspect.rs:99 shows it"),
    // query.rs:18-26 (`str_param(p, "filter")`)
    read_only(
        "engine.commands",
        &["filter"],
        &[],
        Shape::Plain,
        "the engine's command table (cmd.rs:129), described; its `filter` keeps the answer small (the whole table is 100-110 KB in 0.17.0)",
    ),
    // query.rs:27
    read_only("engine.history", &[], &[], Shape::Plain, "the undo history's labels and whether there's a redo (lib.rs:333)"),
    // query.rs:28-31
    read_only("engine.plugins", &[], &[], Shape::Plain, "the built-in plug-ins' static descriptions"),
    // query.rs:32
    read_only("engine.parity", &[], &[], Shape::Plain, "the menu catalog's parity report (catalog.rs:109), built from static tables"),
    // query.rs:33-38 (`position_param(…, "at")`, `str_param(p, "format")`)
    read_only("session.format_time", &["at", "format"], &[], Shape::Plain, "a position as text in a counter format, from the session"),
    // health.rs:11, :64 (its params unread)
    read_only("session.audio_health", &[], &[], Shape::Health, "its audio clips' loaded audio and bounds, from the session"),
    // file.rs:196
    read_only("file.get_info", &[], &[], Shape::Session, "the session's summary, as session.inspect's (inspect.rs:73)"),
    // midi.rs:12, :38 (`clip_id(p, …)`, cmd.rs:344)
    read_only("midi.notes", &["clip"], &[("clip", Kind::Clip)], Shape::Plain, "a MIDI clip's notes, from the session"),
    // event_more.rs:30
    read_only("event.time_ops_window", &[], &[], Shape::Plain, "the tempo and meter maps, song start and selection (event_more.rs:159)"),
    // event_more.rs:31
    read_only("event.tempo_ops_window", &[], &[], Shape::Plain, "the same as event.time_ops_window"),
    // event_more.rs:32-35
    read_only("event.midi_ops_window", &[], &[], Shape::Plain, "input quantize, stored performances and real-time properties"),
    // setup_more.rs:87-96 (`str_param(p, "filter")`)
    read_only("setup.keyboard_shortcuts", &["filter"], &[], Shape::Plain, "the command table's shortcuts"),
    // video.rs:32-46
    read_only("video.list", &[], &[], Shape::Plain, "the session's movies, and whether each file is found (a look, no write)"),
    // view_more.rs:190-194 (`str_param(p, "prefix")`)
    read_only("view.flags", &["prefix"], &[], Shape::Plain, "the view and option flags that are on, and the rulers"),
];

/// Commands that look like reads and aren't on [`READ_ONLY`], and why, in their refusal's words (SoundCraft 0.17.0;
/// paths from crates/engine/src/cmd/ unless named).
pub const LOOK_ALIKES: [(&[&str], &str); 8] = [
    // transport.rs:42 and options.rs:81: `noundo`, so journaled (lib.rs:433), in a ring of 10,000 that drops its oldest
    // (lib.rs:33)
    (
        &["transport.status", "setup.preferences"],
        "each call takes a place in SoundCraft's command journal, as a change does; session.inspect and view.flags read what it answers",
    ),
    // view_more.rs:13, :149-150; more_util.rs:12
    (&["view.clip_gain_info", "view.clip_effects_status"], "it's a View > Clip toggle, which turns its flag over when given no value"),
    // file.rs:114
    (&["file.export_session_text"], "it writes a file when given a path; file.get_info reads the session"),
    // file.rs:155, event_more.rs:90, :118, setup_more.rs:63-66, :84-86, :97-119: every param optional, journaled (undoable
    // too, for file.score_setup and setup.io)
    (
        &[
            "file.score_setup",
            "event.midi_track_offsets",
            "event.input_quantize",
            "setup.io",
            "setup.disk_allocation",
            "setup.peripherals",
            "setup.external_timecode_offset",
            "setup.video_sync_offset",
            "setup.click_countoff",
            "setup.midi_studio",
            "setup.midi_beat_clock",
            "setup.midi_input_filter",
            "setup.midi_input_devices",
            "setup.transcription_settings",
        ],
        "it sets what its params name, and each call is journaled as a change, even with none",
    ),
    // ui-egui menus.rs:523
    (&["setup.playback_engine", "setup.hardware"], "called, the app applies the audio device that's open"),
    // ui-egui menus.rs:687
    (&["ui.theme"], "given a mode, it changes the app's appearance"),
    // ui-egui menus.rs:561-569
    (
        &["window.undo_history", "window.session_info", "window.audio_health"],
        "it opens or closes the window that shows what engine.history, file.get_info or session.audio_health reads",
    ),
    // strip_silence.rs:11, :41-45 (a stop flag never set); io/strip.rs:197, :222-239; mix/src/offline.rs:135
    (
        &["edit.strip_silence_preview"],
        "it renders the audio of every clip in its range on SoundCraft's window thread, unbounded (SoundCraft itself runs that scan on a worker)",
    ),
];

/// The plug-in scans, `query` commands kept off [`READ_ONLY`]: `engine.clap_plugins` (clap.rs:11) and
/// `engine.vst3_plugins` (vst3.rs:11). Their first scan (and any `rescan`) loads every plug-in binary inside SoundCraft's
/// own process (clap-host's lib.rs:149), so it can run past a read's deadline or take the app down. Nothing of Kumi's
/// scans plug-ins in the app: plug-in lists come from a worker of their own.
pub const SCANS: [&str; 2] = ["engine.clap_plugins", "engine.vst3_plugins"];

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

impl Reads {
    /// One `call`: `{command, params?, epoch?}`, with the epoch the refs in `params` came from. Not started after
    /// `until`.
    pub(super) async fn call(&mut self, params: &Value, until: Instant) -> Result<Value, Refusal> {
        if let Some(refused) = self.unchecked() {
            return Err(refused);
        }
        let command =
            text(params, "command")?.ok_or_else(|| bad(format!("command: the id of one of the commands Kumi reads: {}", reads())))?;
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
        // (a position the app can't read would leave its words in the status line)
        if let Some(at) = sent.get("at").filter(|at| read_only.id == "session.format_time" && !at.is_number()) {
            return Err(bad(format!("at: a position in samples, a number, not {at}")));
        }
        if !named.is_empty() && epoch.is_none() {
            return Err(bad("epoch: the epoch of the answer the refs came from (SoundCraft gives ids out again)".into()));
        }
        // (the lives as the app shows them now, for the refs in and the ids out)
        let session = match !named.is_empty() || !matches!(read_only.shape, Shape::Plain | Shape::Session) {
            true => Some(self.inspect(until).await?),
            false => None,
        };
        if let Some(epoch) = epoch.filter(|epoch| epoch != self.epoch()) {
            return Err((
                code::STALE,
                format!(
                    "those refs are from another epoch ({epoch}; now {}): another session is open, or the bridge started again: read again",
                    self.epoch()
                ),
            ));
        }
        for (key, id) in named {
            let named = Ref::Object(id);
            let resolved = self.lives.resolve(&id).map_err(|missing| gone(missing, &named))?;
            if read_only.id == "midi.notes" {
                // (as `read` checks a clip before its notes: the app would say so in its status line)
                let clip = tracks(session.as_deref().unwrap_or(&Value::Null))
                    .iter()
                    .flat_map(|track| list(track, "clips"))
                    .find(|clip| clip["id"].as_u64() == Some(resolved))
                    .ok_or_else(|| gone(Missing::Gone, &named))?;
                if clip.get("notes").is_none() {
                    return Err(bad(format!("{key}: {named} isn't a MIDI clip, so it has no notes")));
                }
            }
            sent[key] = resolved.into();
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
        let truncated = cut(&mut result, CALL_ANSWER_BYTES)?;
        let mut answer = json!({"command": command, "result": result, "epoch": self.epoch()});
        if !truncated.is_empty() {
            answer["truncated"] = truncated.into();
        }
        Ok(answer)
    }

    /// Why `call` doesn't run on this SoundCraft, when it isn't the one its list was checked against.
    fn unchecked(&self) -> Option<Refusal> {
        let checked = |version: &str| version.strip_prefix(CHECKED_FOR).is_some_and(|rest| rest.is_empty() || rest.starts_with('.'));
        let this = match self.version.as_deref() {
            Some(version) if checked(version) => return None,
            Some(version) => format!("this is SoundCraft {version}"),
            None => "this SoundCraft's version isn't known".into(),
        };
        Some((
            code::FAILED,
            format!("call runs only on SoundCraft {CHECKED_FOR}.x, the version the commands Kumi reads were checked against, and {this}"),
        ))
    }

    /// Why a command off the list isn't run: a channel method, a look-alike or a plug-in scan, a command that changes
    /// something (or only opens a window or a dialog), or none the app has, with those on the list near it. Each names
    /// the commands that run. The app's list is asked for once (until it lists some).
    async fn not_run(&mut self, command: &str, until: Instant) -> Refusal {
        let why = if CHANNEL.contains(&command) {
            format!("{command} is a method of SoundCraft's control channel, which call doesn't reach")
        } else if SCANS.contains(&command) {
            format!("{command} scans plug-ins inside SoundCraft's own process, which can outlast a read or take the app down: Kumi doesn't ask it to")
        } else if let Some((_, why)) = LOOK_ALIKES.iter().find(|(ids, _)| ids.contains(&command)) {
            format!("{command} isn't one of the commands Kumi reads: {why}")
        } else {
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
            if self.commands.as_deref().unwrap_or_default().iter().any(|id| id == command) {
                format!("Kumi can't change SoundCraft yet: {command} {}", what_it_does(command))
            } else {
                match near(command).as_slice() {
                    [] => format!("no such command in SoundCraft: {command}"),
                    near => format!("no such command in SoundCraft: {command}; perhaps {}", near.join(", ")),
                }
            }
        };
        bad(format!("{why}. The commands Kumi reads: {}", reads()))
    }
}

/// The commands `call` runs, for a refusal to name, with how to keep `engine.commands`'s answer whole.
fn reads() -> String {
    let ids = READ_ONLY.iter().map(|command| command.id).collect::<Vec<_>>().join(", ");
    format!("{ids} (engine.commands' filter keeps its answer small: all of SoundCraft 0.17.0's commands come to 100-110 KB)")
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

/// The commands `call` runs that share the longest start with `command`, then those that hold it: five at most, as
/// REAPER's bridge suggests its functions (reaper-kumi's generic.rs).
fn near(command: &str) -> Vec<&'static str> {
    let lower = command.to_ascii_lowercase();
    let shared = |id: &str| id.bytes().zip(lower.bytes()).take_while(|(a, b)| a == b).count();
    let mut near: Vec<(usize, &str)> = READ_ONLY
        .iter()
        .map(|read_only| read_only.id)
        .map(|id| (shared(id).max(if id.contains(&lower) { lower.len() } else { 0 }), id))
        .filter(|(score, _)| *score >= 4.min(lower.len()).max(1))
        .collect();
    near.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().abs_diff(command.len()).cmp(&b.1.len().abs_diff(command.len()))));
    near.into_iter().take(5).map(|(_, id)| id).collect()
}

/// The refs in a command's params, by the param that holds each: every object it takes, required, as its ref. A param
/// it doesn't take, an id given where a ref goes, or a ref anywhere else, is refused.
fn named(command: &Command, params: &Value) -> Result<Vec<(&'static str, Id)>, Refusal> {
    let fields = params.as_object().into_iter().flatten();
    for (key, value) in fields {
        if !command.params.contains(&key.as_str()) {
            return Err(bad(match command.params {
                [] => format!("{key}: {} takes no params", command.id),
                taken => format!("{key}: {} takes only {}", command.id, taken.join(", ")),
            }));
        }
        if let Some(found) = a_ref_in(value).filter(|_| !command.objects.iter().any(|(object, _)| object == key)) {
            return Err(bad(format!("{key}: {} takes no object there, so not {found}", command.id)));
        }
    }
    command
        .objects
        .iter()
        .map(|(key, kind)| {
            let word = kind.word();
            let value = params
                .get(*key)
                .ok_or_else(|| bad(format!("{key}: {} reads the {word} it names, as a ref from read (s:{word}:…)", command.id)))?;
            Ok((*key, object(key, *kind, value)?))
        })
        .collect()
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
/// points before the track's lanes), else empties the biggest and measures again; a list keeps the items from its
/// first that fit, a text its start. An answer with nothing left to cut that's still past the bound is refused.
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
                format!("the answer is {} KiB, past the {} KiB a call may answer, with no list or text to cut", size / 1024, most / 1024),
            ));
        };
        // (none alone can do: the biggest is emptied, and the next turn goes on)
        let room = its.saturating_sub(over);
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
    fn every_command_call_runs_is_one_soundcraft_lists_and_those_kept_off_are_too() {
        let known = recorded();
        assert_eq!(known.len(), 551);
        let mut ids: Vec<&str> = READ_ONLY.iter().map(|command| command.id).collect();
        ids.sort_unstable();
        // (the one list, which Kumi's call words name too: a change there is a change to what Kumi may run in SoundCraft)
        assert_eq!(ids, daw_bridge::contract::SOUNDCRAFT_CALLS, "the commands call runs are the contract's, in its order");
        for command in &READ_ONLY {
            assert!(known.iter().any(|id| id == command.id), "{} isn't a command of SoundCraft 0.17.0's", command.id);
            assert!(!command.why.is_empty());
            for (object, _) in command.objects {
                assert!(command.params.contains(object), "{}: {object} is one of its params", command.id);
            }
        }
        let kept_off = LOOK_ALIKES.iter().flat_map(|(ids, _)| ids.iter()).chain(SCANS.iter());
        for id in kept_off {
            assert!(known.iter().any(|known| known == id), "{id} isn't a command of SoundCraft 0.17.0's");
            assert!(!ids.contains(id), "{id} is kept off the list");
        }
    }
}
