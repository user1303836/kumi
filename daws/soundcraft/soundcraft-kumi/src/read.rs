//! `read`: SoundCraft's session as rows, a page at a time, each object named by its ref ([`crate::refs`]).
//!
//! The session's objects come from `session.inspect {"detail":"full"}`, asked again at most once a second and shared by the
//! reads in between; a clip's notes from `midi.notes`; and the built-in plug-ins' parameters' names, ranges and units from
//! `engine.plugins`, asked once. Automation lanes are named as SoundCraft's automation commands name them. The lives of the
//! objects learn from every look at the session:
//! a read's, and the look loop's every few seconds. Every answer carries the epoch of the table of lives its refs are
//! from. A new one starts when the bridge starts again, and when a look sees another session open ([`Reads::saw`]): by
//! another name or path unless its objects hold what they held (a save, a Save As, a rename), by an undo history emptied
//! with nothing to redo (only a session opened, made or closed does that), or by none of the objects it saw before with no
//! step to take them. SoundCraft gives ids out again, so a ref or parent comes with the epoch it came from, and one
//! from another epoch is stale. What a read can be refused for without the app is refused before it's asked. A page ends
//! at the limit asked for (50, at most 200) or at the bytes an answer may take. Its cursor names the object it starts at,
//! so a list that changed between pages says so rather than skipping or repeating rows. FOCUS's bodies ([`bodies`]) are
//! read the same way, and `call` runs one of SoundCraft's read-only commands by its id ([`call`]).
mod bodies;
pub mod call;

use crate::control::{Control, Error};
use crate::refs::{fingerprints, lane, Id, Kind, Lives, Missing, Ref, Slot};
use crate::timebase::Timebase;
use daw_bridge::auth::sha256_hex;
use daw_bridge::wire::code;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 200;
/// What `read` takes as a kind (`note` with a clip as its parent, `param` with an insert, `point` with a lane; `call`
/// with a command).
pub const KINDS: [&str; 14] =
    ["session", "track", "clip", "insert", "send", "bus", "marker", "group", "source", "note", "param", "env", "point", "call"];
/// The bytes a page's rows may take: the host takes answers of up to 256 KiB, and the rest is the envelope.
const ANSWER_BYTES: usize = 224 * 1024;
/// How long one inspection serves the reads after it.
const FRESH: Duration = Duration::from_secs(1);
/// The longest a read waits for the app.
const READ_WAIT: Duration = Duration::from_secs(5);
/// What a snapshot shows of the tracks and markers, from the first: so many rows in so many bytes. `read` pages on.
const SNAPSHOT_TRACKS: usize = 200;
const SNAPSHOT_TRACK_BYTES: usize = 150 * 1024;
const SNAPSHOT_MARKERS: usize = 100;
/// The selected tracks and clips a selection names by ref, as REAPER's does (it counts them all).
const SELECTED_TRACKS: usize = 50;
const SELECTED_CLIPS: usize = 20;

/// Why a read has no answer: its kumi-daw/1 code, and the words.
pub type Refusal = (&'static str, String);

/// What a look saw of the session, to tell another session from it: its name and path, the tops of its undo and redo
/// histories, who its objects were, and what they held (a source by who it is, as a save rewrites where its file is).
struct Look {
    named: (Value, Value),
    history: (Value, Value),
    who: HashSet<(Kind, u64, u64)>,
    held: HashSet<(Kind, u64, u64)>,
}

/// The bridge's reads of one copy of SoundCraft: the lives of its objects (and their epoch), the last look, the last
/// full inspection, and the session the last look of either kind saw.
pub struct Reads {
    control: Control,
    lives: Lives,
    looked: Option<Look>,
    /// The last full inspection, when it came, and its content ([`content`]).
    inspected: Option<(Instant, Arc<Value>, u64)>,
    /// The session the last look saw (a summary, or a full inspection), and when it came: what FOCUS's bodies are drawn
    /// from, as they read nothing only the full inspection has.
    summary: Option<(Instant, Arc<Value>)>,
    /// The epoch as it changes, for whoever can't wait for a read to end.
    told: watch::Sender<String>,
    /// The built-in plug-ins as `engine.plugins` describes them, asked once: they come with the app's version.
    plugins: Option<Vec<Value>>,
    /// The ids of the app's commands (`engine.commands`), asked once, for what `call` refuses.
    commands: Option<Vec<String>>,
    /// The app's version, when it says one: `call` runs only on the one its list was checked against.
    version: Option<String>,
}

impl Reads {
    /// The reads of the SoundCraft `control` reaches, of `version` (the identity's `daw_version`) when known.
    pub fn new(control: Control, version: Option<String>) -> Self {
        let lives = Lives::new();
        let (told, _) = watch::channel(lives.epoch().to_string());
        Self { control, lives, looked: None, inspected: None, summary: None, told, plugins: None, commands: None, version }
    }

    /// The epoch as it changes, readable while a read holds the reads.
    pub fn epochs(&self) -> watch::Receiver<String> {
        self.told.subscribe()
    }

    /// The epoch now: its table's.
    pub fn epoch(&self) -> &str {
        self.lives.epoch()
    }

    /// What the producer has selected, in the refs of the epoch now ([`Reads::saw`] the session first).
    pub fn selection(&self, session: &Value) -> Value {
        selection_row(session, &self.lives)
    }

    /// The plug-in a window of SoundCraft's shows, which it names `[track, slot]` (the instrument's slot is the largest
    /// number there is): its insert's `fxRef`, and its `fxName`, a built-in's as `engine.plugins` names it; and its track's
    /// ref now (`fxTrackRef`, `read`'s) and name (`fxTrackName`), the selected track or not. Nothing for a track or slot
    /// the session doesn't have.
    pub async fn plugin_window(&mut self, session: &Value, window: &Value, until: Instant) -> Option<Value> {
        let (track, slot) = (window[0].as_u64()?, window[1].as_u64()?);
        let track = tracks(session).iter().find(|row| row["id"].as_u64() == Some(track))?;
        let (key, row) = if slot == u64::MAX {
            instrument_row(track, &self.lives)?
        } else {
            let insert = list(track, "inserts").iter().find(|insert| insert["slot"].as_u64() == Some(slot))?;
            insert_row(track, insert, &self.lives)?
        };
        // (a plug-in's name only fills in its id: one the app won't describe stays the id)
        let _ = self.know_plugins(until).await;
        Some(json!({
            "fxRef": key,
            "fxName": self.plugin_name(&row["plugin"]),
            "fxTrackRef": row["trackRef"],
            "fxTrackName": track["name"],
        }))
    }

    /// A plug-in's name, as `engine.plugins` gives a built-in's ([`Reads::know_plugins`] first); else its id.
    fn plugin_name(&self, plugin: &Value) -> Value {
        let known = self.plugins.as_deref().unwrap_or_default().iter().find(|known| known["id"] == *plugin);
        known.map_or(plugin, |known| &known["name"]).clone()
    }

    /// What a look saw of the session (`session.inspect`, in full or not): the lives learn from it, and another session
    /// starts a new epoch, with new lives. Another session is:
    /// - one under another name or path, unless its objects hold all they held and either its history reads as it did (a
    ///   save or Save As, which names the session after its file) or its path is the same (Setup > Session's rename);
    /// - one whose undo history was emptied;
    /// - one with none of the objects before though no step took them (File > New on a session with nothing to undo).
    ///
    /// A save the looks see only after an edit that followed it counts as another session. The last full inspection
    /// serves no more reads, so their rows and the lives come from one look.
    pub fn saw(&mut self, session: &Value) {
        let prints = fingerprints(session);
        let each = |print: fn(Kind, &crate::refs::Print) -> u64| {
            prints.iter().flat_map(|(kind, objects)| objects.iter().map(move |(id, held)| (*kind, *id, print(*kind, held)))).collect()
        };
        let now = Look {
            named: (session["name"].clone(), session["path"].clone()),
            history: (session["undo"].clone(), session["redo"].clone()),
            who: each(|_, print| print.who),
            held: each(|kind, print| if kind == Kind::Source { print.who } else { print.all }),
        };
        let emptied = |history: &(Value, Value)| history.0.is_null() && history.1.is_null();
        let another = self.looked.as_ref().is_some_and(|before| {
            let (kept_history, kept_objects) = (before.history == now.history, before.held == now.held);
            let (saved, renamed) = (kept_objects && kept_history, kept_objects && before.named.1 == now.named.1);
            (before.named != now.named && !(saved || renamed))
                // (an undo leaves something to redo, and a redo or a step something to undo: only another session empties
                // a history)
                || (!emptied(&before.history) && emptied(&now.history))
                // (a step that took every object moved the history)
                || (kept_history && !before.who.is_empty() && before.who.is_disjoint(&now.who))
        });
        if another {
            self.lives = Lives::new();
            self.told.send_replace(self.lives.epoch().to_string());
        }
        for (kind, objects) in prints {
            self.lives.saw(kind, objects);
        }
        self.looked = Some(now);
        self.inspected = None;
        self.summary = None;
    }

    /// A look at the session's summary, whose content the look took ([`content`]): the lives learn from it as from any
    /// look ([`Reads::saw`]), and the reads' last full inspection goes on serving them while it shows that content still,
    /// in the same epoch.
    pub fn looked(&mut self, summary: &Value, content: u64) {
        let (kept, epoch) = (self.inspected.take(), self.lives.epoch().to_string());
        self.saw(summary);
        let same = self.lives.epoch() == epoch;
        self.inspected = kept.filter(|(_, _, held)| same && *held == content);
    }

    /// A look at the session's summary, as [`Reads::looked`], which FOCUS's bodies are drawn from while it's fresh.
    pub fn looked_at(&mut self, summary: Arc<Value>, content: u64) {
        self.looked(&summary, content);
        self.summary = Some((Instant::now(), summary));
    }

    /// One `read`: `{kind, parent?, cursor?, limit?}` for a list, `{ref}` for one object, or `{kind: "call", command,
    /// params?}` for a read-only command's answer ([`call`]), with the `epoch` the refs came from. Not started after
    /// `until` (the request's deadline).
    pub async fn read(&mut self, params: &Value, until: Instant) -> Result<Value, Refusal> {
        if params.get("kind").and_then(Value::as_str) == Some("call") {
            return self.call(params, until).await;
        }
        let (kind, one, parent, cursor, epoch) =
            (text(params, "kind")?, text(params, "ref")?, text(params, "parent")?, text(params, "cursor")?, text(params, "epoch")?);
        let limit = limit(params, DEFAULT_LIMIT, MAX_LIMIT)?;
        let one = one.map(|one| one.parse::<Ref>().map_err(bad)).transpose()?;
        if let Some(refused) = one.as_ref().and_then(not_yet) {
            return Err(refused);
        }
        let parent = parent.map(|parent| parent.parse::<Ref>().map_err(bad)).transpose()?;
        let cursor = cursor.as_deref().map(Cursor::parse).transpose()?;
        if (one.is_some() || parent.is_some()) && epoch.is_none() {
            return Err(bad("epoch: the epoch of the answer the ref or parent came from (SoundCraft gives ids out again)".into()));
        }
        let listing = match (&one, kind) {
            (Some(_), _) => None,
            (None, Some(kind)) => Some(Listing::of(&kind, parent)?),
            (None, None) => return Err(bad(format!("kind: what to read, one of {} (or ref: one object)", KINDS.join(", ")))),
        };
        let session = self.inspect(until).await?;
        self.same_epoch(epoch)?;
        let Some(listing) = listing else {
            let row = self.one(&session, &one.expect("a ref, without a kind"), until).await?;
            return Ok(json!({"items": [row], "epoch": self.epoch()}));
        };
        // (each row with its ref, which a cursor names it by)
        let rows: Vec<(String, Value)> = match listing {
            Listing::Notes(clip) => self.notes(&session, &clip, until).await?,
            Listing::Params(track, slot) => {
                self.know_plugins(until).await?;
                self.params(&session, track, slot)?
            }
            listing => self.rows(&session, listing)?,
        };
        self.page(&rows, cursor.as_ref(), limit)
    }

    /// Refs or a parent from `epoch` (the answer's they came from) are read only in that epoch: SoundCraft gives ids out
    /// again.
    fn same_epoch(&self, epoch: Option<String>) -> Result<(), Refusal> {
        match epoch.filter(|epoch| epoch != self.epoch()) {
            None => Ok(()),
            Some(epoch) => Err((
                code::STALE,
                format!(
                    "those refs are from another epoch ({epoch}; now {}): another session is open, or the bridge started again: read again",
                    self.epoch()
                ),
            )),
        }
    }

    /// The session on one screen, kumi-daw/1's `snapshot`: the project, the transport, the selection, and the first
    /// tracks (200, in 150 KiB) and markers (100). What's left out is named under `truncated`, with the cursor `read`
    /// goes on from. Not started after `until`.
    pub async fn snapshot(&mut self, until: Instant) -> Result<Value, Refusal> {
        let session = self.inspect(until).await?;
        let (session, lives, epoch) = (&*session, &self.lives, self.lives.epoch());
        let playhead = &session["transport"];
        let tracks: Vec<(String, Value)> =
            tracks(session).iter().enumerate().filter_map(|(index, track)| track_row(session, track, index, lives)).collect();
        let markers: Vec<(String, Value)> =
            list(session, "markers").iter().filter_map(|marker| marker_row(session, marker, lives)).collect();
        let shown = [
            ("tracks", &tracks, first(&tracks, SNAPSHOT_TRACKS, SNAPSHOT_TRACK_BYTES)),
            ("markers", &markers, first(&markers, SNAPSHOT_MARKERS, ANSWER_BYTES - SNAPSHOT_TRACK_BYTES - 16 * 1024)),
        ];
        let rows =
            |(_, rows, shown): &(&str, &Vec<(String, Value)>, usize)| rows[..*shown].iter().map(|(_, row)| row.clone()).collect::<Vec<_>>();
        let mut answer = json!({
            "epoch": epoch,
            "project": project_row(session, epoch),
            "transport": transport(session, &playhead["position"], &playhead["playing"], &playhead["recording"]),
            "tracks": rows(&shown[0]),
            "selection": selection_row(session, lives),
            "markers": rows(&shown[1]),
        });
        let truncated: Map<String, Value> = shown
            .iter()
            .filter(|(_, rows, shown)| *shown < rows.len())
            .map(|(key, rows, shown)| {
                (key.to_string(), json!({"total": rows.len(), "nextCursor": Cursor::at(epoch, rows, *shown).to_string()}))
            })
            .collect();
        if !truncated.is_empty() {
            answer["truncated"] = truncated.into();
        }
        Ok(answer)
    }

    /// The session as the app shows it now, from the last inspection when it's fresh enough.
    async fn inspect(&mut self, until: Instant) -> Result<Arc<Value>, Refusal> {
        if let Some((at, session, _)) = &self.inspected {
            if at.elapsed() < FRESH {
                return Ok(Arc::clone(session));
            }
        }
        let session = self.control.call("session.inspect", json!({"detail": "full"}), wait_within(until)?).await.map_err(asked)?;
        self.saw(&session);
        let held = content(&session);
        let session = Arc::new(session);
        self.inspected = Some((Instant::now(), Arc::clone(&session), held));
        self.summary = Some((Instant::now(), Arc::clone(&session)));
        Ok(session)
    }

    /// The rows of a listing the session alone answers: every listing but a clip's notes and an insert's parameters,
    /// which need the app's word too.
    fn rows(&self, session: &Value, listing: Listing) -> Result<Vec<(String, Value)>, Refusal> {
        let lives = &self.lives;
        Ok(match listing {
            Listing::Session => vec![("session".to_string(), session_row(session, lives, self.epoch()))],
            Listing::Tracks => {
                tracks(session).iter().enumerate().filter_map(|(index, track)| track_row(session, track, index, lives)).collect()
            }
            Listing::Clips(of) => self
                .tracks_of(session, of)?
                .into_iter()
                .flat_map(|track| list(track, "clips").iter().filter_map(move |clip| clip_row(session, track, clip, lives)))
                .collect(),
            Listing::Inserts(of) => self.tracks_of(session, of)?.into_iter().flat_map(|track| insert_rows(track, lives)).collect(),
            Listing::Sends(of) => self
                .tracks_of(session, of)?
                .into_iter()
                .flat_map(|track| list(track, "sends").iter().filter_map(move |send| send_row(track, send, lives)))
                .collect(),
            Listing::Busses => list(session, "busses").iter().filter_map(|bus| bus_row(bus, lives)).collect(),
            Listing::Markers => list(session, "markers").iter().filter_map(|marker| marker_row(session, marker, lives)).collect(),
            Listing::Groups => list(session, "groups").iter().filter_map(|group| group_row(group, lives)).collect(),
            Listing::Sources => list(session, "sources").iter().filter_map(|source| source_row(session, source, lives)).collect(),
            Listing::Lanes(of) => self
                .tracks_of(session, of)?
                .into_iter()
                .flat_map(|track| list(track, "automation").iter().filter_map(move |lane| lane_row(track, lane, lives)))
                .collect(),
            Listing::Points(track, id) => {
                let lane = lane_of(self.track(session, &track)?, &id)
                    .ok_or_else(|| gone(Missing::Gone, &Ref::Lane { track, param: id.clone() }))?;
                point_rows(session, track, &id, lane)
            }
            Listing::Notes(_) | Listing::Params(..) => unreachable!("read with the app's word"),
        })
    }

    /// The tracks whose things a read lists: the one `of` names, else every track.
    fn tracks_of<'a>(&self, session: &'a Value, of: Option<Id>) -> Result<Vec<&'a Value>, Refusal> {
        Ok(match of {
            None => tracks(session).iter().collect(),
            Some(id) => vec![self.track(session, &id)?],
        })
    }

    /// The track a ref names, if it's the one there now.
    fn track<'a>(&self, session: &'a Value, id: &Id) -> Result<&'a Value, Refusal> {
        let id = self.lives.resolve(id).map_err(|missing| gone(missing, &Ref::Object(*id)))?;
        tracks(session)
            .iter()
            .find(|track| track["id"].as_u64() == Some(id))
            .ok_or_else(|| gone(Missing::Gone, &Ref::Object(Id { kind: Kind::Track, id, life: 1 })))
    }

    /// One object's row, by its ref.
    async fn one(&mut self, session: &Value, named: &Ref, until: Instant) -> Result<Value, Refusal> {
        let missing = || gone(Missing::Gone, named);
        match named {
            Ref::Object(id) => {
                let found = self.lives.resolve(id).map_err(|missing| gone(missing, named))?;
                let key = match id.kind {
                    Kind::Track => "tracks",
                    Kind::Bus => "busses",
                    Kind::Group => "groups",
                    Kind::Marker => "markers",
                    Kind::Source => "sources",
                    Kind::Clip => {
                        let (track, clip) = tracks(session)
                            .iter()
                            .find_map(|track| {
                                list(track, "clips").iter().find(|clip| clip["id"].as_u64() == Some(found)).map(|clip| (track, clip))
                            })
                            .ok_or_else(missing)?;
                        return clip_row(session, track, clip, &self.lives).map(|(_, row)| row).ok_or_else(missing);
                    }
                };
                let (index, item) =
                    list(session, key).iter().enumerate().find(|(_, item)| item["id"].as_u64() == Some(found)).ok_or_else(missing)?;
                match id.kind {
                    Kind::Track => track_row(session, item, index, &self.lives),
                    Kind::Bus => bus_row(item, &self.lives),
                    Kind::Group => group_row(item, &self.lives),
                    Kind::Marker => marker_row(session, item, &self.lives),
                    Kind::Source => source_row(session, item, &self.lives),
                    Kind::Clip => unreachable!("answered above"),
                }
                .map(|(_, row)| row)
                .ok_or_else(missing)
            }
            Ref::Insert { track, slot } => {
                let track = self.track(session, track)?;
                match slot {
                    Slot::Instrument => instrument_row(track, &self.lives),
                    Slot::Insert(_) => list(track, "inserts")
                        .iter()
                        .find(|insert| slot_of(insert) == Some(*slot))
                        .and_then(|insert| insert_row(track, insert, &self.lives)),
                }
                .map(|(_, row)| row)
                .ok_or_else(missing)
            }
            Ref::Send { track, slot } => {
                let track = self.track(session, track)?;
                let send = list(track, "sends").iter().find(|send| send["slot"].as_u64() == Some((*slot).into())).ok_or_else(missing)?;
                send_row(track, send, &self.lives).map(|(_, row)| row).ok_or_else(missing)
            }
            Ref::Note { clip, .. } => {
                let key = named.to_string();
                let (_, row) = self.notes(session, clip, until).await?.into_iter().find(|(at, _)| *at == key).ok_or_else(missing)?;
                match row.get("tie") {
                    Some(count) => Err((
                        code::FAILED,
                        format!("{count} notes of that clip have that start, pitch and channel: read its notes, which mark the tie"),
                    )),
                    None => Ok(row),
                }
            }
            Ref::Param { track, slot, .. } => {
                self.know_plugins(until).await?;
                let key = named.to_string();
                self.params(session, *track, *slot)?.into_iter().find(|(at, _)| *at == key).map(|(_, row)| row).ok_or_else(missing)
            }
            Ref::Lane { track, param } => {
                let in_track = self.track(session, track)?;
                lane_of(in_track, param).and_then(|lane| lane_row(in_track, lane, &self.lives)).map(|(_, row)| row).ok_or_else(missing)
            }
            Ref::Point { track, param, .. } => {
                let lane = lane_of(self.track(session, track)?, param).ok_or_else(missing)?;
                let key = named.to_string();
                let (_, row) = point_rows(session, *track, param, lane).into_iter().find(|(at, _)| *at == key).ok_or_else(missing)?;
                match row.get("tie") {
                    Some(count) => {
                        Err((code::FAILED, format!("{count} points of that lane are at that sample: read its points, which mark the tie")))
                    }
                    None => Ok(row),
                }
            }
            Ref::Playlist { .. } => unreachable!("refused before the app is asked"),
        }
    }

    /// A MIDI clip's notes as rows (`midi.notes`), in the clip's order (edits keep it by start, pitch, channel, length and
    /// velocity). Notes alike (the same start, pitch and channel, which SoundCraft keeps both of) are marked `tie`: one ref
    /// can't tell them apart. A clip that isn't MIDI is refused before the app is asked, which would say so in its status
    /// line.
    async fn notes(&mut self, session: &Value, clip: &Id, until: Instant) -> Result<Vec<(String, Value)>, Refusal> {
        let named = Ref::Object(*clip);
        let id = self.lives.resolve(clip).map_err(|missing| gone(missing, &named))?;
        let shown = tracks(session)
            .iter()
            .flat_map(|track| list(track, "clips"))
            .find(|shown| shown["id"].as_u64() == Some(id))
            .ok_or_else(|| gone(Missing::Gone, &named))?;
        if shown.get("notes").is_none() {
            return Err(bad(format!("parent: {named} isn't a MIDI clip, so it has no notes")));
        }
        let mut answer =
            self.control.execute("midi.notes", json!({"clip": id}), wait_within(until)?).await.map_err(|error| match error {
                Error::Refused(said) => (code::FAILED, format!("{named}: {said}")),
                other => asked(other),
            })?;
        let notes = match answer.get_mut("notes").map(Value::take) {
            Some(Value::Array(notes)) => notes,
            _ => Vec::new(),
        };
        let field = |note: &Value, key: &str| note.get(key).and_then(Value::as_i64).unwrap_or_default();
        let place = |note: &Value| (field(note, "start"), field(note, "pitch"), field(note, "channel"));
        let mut alike: HashMap<(i64, i64, i64), usize> = HashMap::with_capacity(notes.len());
        for note in &notes {
            *alike.entry(place(note)).or_default() += 1;
        }
        Ok(notes
            .iter()
            .map(|note| {
                let (start, pitch, channel) = place(note);
                let key = Ref::Note { clip: *clip, start, pitch: pitch as u8, channel: channel as u8 }.to_string();
                let mut row = json!({
                    "ref": key,
                    "start": start,
                    "length": field(note, "length"),
                    "pitch": pitch,
                    "velocity": field(note, "velocity"),
                    "releaseVelocity": field(note, "release_velocity"),
                    "channel": channel,
                });
                if let Some(count) = alike.get(&(start, pitch, channel)).filter(|count| **count > 1) {
                    row["tie"] = (*count).into();
                }
                (key, row)
            })
            .collect())
    }

    /// An insert's parameters, as the built-in plug-ins' descriptions name them ([`Reads::know_plugins`] first).
    fn params(&self, session: &Value, track: Id, slot: Slot) -> Result<Vec<(String, Value)>, Refusal> {
        let named = Ref::Insert { track, slot };
        let insert = list(self.track(session, &track)?, "inserts")
            .iter()
            .find(|insert| slot_of(insert) == Some(slot))
            .ok_or_else(|| gone(Missing::Gone, &named))?;
        Ok(param_rows(track, slot, insert, self.plugins.as_deref().unwrap_or_default()))
    }

    /// Asks the app for its built-in plug-ins' descriptions the first time a parameter is read: they come with its
    /// version, so one answer serves the bridge's run.
    async fn know_plugins(&mut self, until: Instant) -> Result<(), Refusal> {
        if self.plugins.is_none() {
            let answer = self.control.execute("engine.plugins", json!({}), wait_within(until)?).await.map_err(asked)?;
            self.plugins = Some(match answer {
                Value::Array(plugins) => plugins,
                _ => Vec::new(),
            });
        }
        Ok(())
    }

    /// A page of rows from the cursor's row: up to `limit`, and no more bytes than an answer may take.
    fn page(&self, rows: &[(String, Value)], cursor: Option<&Cursor>, limit: usize) -> Result<Value, Refusal> {
        let from = match cursor {
            None => 0,
            Some(cursor) => self.resume(cursor, rows)?,
        };
        let (items, more) = fill(rows[from..].iter().map(|(_, row)| row.clone()), limit, ANSWER_BYTES);
        let mut answer = json!({"items": items, "epoch": self.epoch(), "total": rows.len(), "indexFrom": 0});
        if more {
            answer["nextCursor"] = Cursor::at(self.epoch(), rows, from + items.len()).to_string().into();
        }
        Ok(answer)
    }

    /// Where a cursor's page starts: at the row it names, wherever that is now.
    fn resume<T>(&self, cursor: &Cursor, rows: &[(String, T)]) -> Result<usize, Refusal> {
        if cursor.epoch != self.epoch() {
            return Err((code::STALE, "that cursor is from another epoch: read again from the start".into()));
        }
        rows.iter()
            .enumerate()
            .filter(|(_, (key, _))| *key == cursor.key)
            .nth(cursor.alike - 1)
            .map(|(position, _)| position)
            .ok_or((code::CHANGED, "the list changed since that cursor: read it again from the start".into()))
    }
}

/// Rows from the first, up to `limit` and no more bytes than `bytes` (one at least): those taken, and whether any was left.
fn fill(rows: impl Iterator<Item = Value>, limit: usize, bytes: usize) -> (Vec<Value>, bool) {
    let (mut items, mut used) = (vec![], 0);
    for row in rows {
        let size = serde_json::to_string(&row).map_or(0, |text| text.len() + 1);
        if items.len() == limit || (!items.is_empty() && used + size > bytes) {
            return (items, true);
        }
        used += size;
        items.push(row);
    }
    (items, false)
}

/// What a read lists, by kind, and what its rows are under: checked before the app is asked.
enum Listing {
    Session,
    Tracks,
    /// A track's clips, inserts or sends: one track's, or every track's.
    Clips(Option<Id>),
    Inserts(Option<Id>),
    Sends(Option<Id>),
    Busses,
    Markers,
    Groups,
    Sources,
    /// A MIDI clip's notes.
    Notes(Id),
    /// An insert's parameters (a slot's: an instrument's aren't read yet).
    Params(Id, Slot),
    /// A track's automation lanes, or every track's.
    Lanes(Option<Id>),
    /// A lane's points: a track's, and what the lane moves.
    Points(Id, String),
}

impl Listing {
    fn of(kind: &str, parent: Option<Ref>) -> Result<Listing, Refusal> {
        let track = |parent: Option<Ref>| match parent {
            None => Ok(None),
            Some(Ref::Object(id @ Id { kind: Kind::Track, .. })) => Ok(Some(id)),
            Some(other) => Err(bad(format!("parent: a track (s:track:…), not {other}"))),
        };
        let listing = match kind {
            "clip" => return Ok(Listing::Clips(track(parent)?)),
            "insert" => return Ok(Listing::Inserts(track(parent)?)),
            "send" => return Ok(Listing::Sends(track(parent)?)),
            "note" => {
                return match parent {
                    Some(Ref::Object(clip @ Id { kind: Kind::Clip, .. })) => Ok(Listing::Notes(clip)),
                    _ => Err(bad("parent: the clip whose notes to read (s:clip:…)".into())),
                }
            }
            "param" => {
                return match parent {
                    Some(Ref::Insert { track, slot: Slot::Instrument }) => Err(bad(format!(
                        "parent: {}'s parameters are only in the session file, which SoundCraft's bridge doesn't read yet",
                        Ref::Insert { track, slot: Slot::Instrument }
                    ))),
                    Some(Ref::Insert { track, slot }) => Ok(Listing::Params(track, slot)),
                    _ => Err(bad("parent: the insert whose parameters to read (s:insert:…)".into())),
                }
            }
            "env" => return Ok(Listing::Lanes(track(parent)?)),
            "point" => {
                return match parent {
                    Some(Ref::Lane { track, param }) => Ok(Listing::Points(track, param)),
                    _ => Err(bad("parent: the lane whose points to read (s:env:…)".into())),
                }
            }
            "session" => Listing::Session,
            "track" => Listing::Tracks,
            "bus" => Listing::Busses,
            "marker" => Listing::Markers,
            "group" => Listing::Groups,
            "source" => Listing::Sources,
            other => return Err(bad(format!("kind: one of {}, not {other}", KINDS.join(", ")))),
        };
        match parent {
            None => Ok(listing),
            Some(parent) => Err(bad(format!("parent: {kind} rows aren't read under anything, so not under {parent}"))),
        }
    }
}

/// Where a page starts: `c1.<epoch>~<ref>` names the row with that ref, and `c1.<epoch>~<ref>#<n>` the nth of the rows
/// with it (notes alike share a ref).
struct Cursor {
    epoch: String,
    key: String,
    alike: usize,
}

impl Cursor {
    fn parse(text: &str) -> Result<Cursor, Refusal> {
        let unknown = || bad("cursor: pass the nextCursor an earlier read gave, unchanged".into());
        let (epoch, rest) = text.strip_prefix("c1.").and_then(|rest| rest.split_once('~')).ok_or_else(unknown)?;
        let (key, alike) = match rest.rsplit_once('#') {
            Some((key, alike)) => (key, alike.parse().ok().filter(|alike: &usize| *alike > 1).ok_or_else(unknown)?),
            None => (rest, 1),
        };
        (!epoch.is_empty() && !key.is_empty()).then_some(()).ok_or_else(unknown)?;
        Ok(Cursor { epoch: epoch.into(), key: key.into(), alike })
    }

    /// The cursor naming the row at `position`.
    fn at<T>(epoch: &str, rows: &[(String, T)], position: usize) -> Cursor {
        let key = &rows[position].0;
        let alike = rows[..=position].iter().filter(|(at, _)| at == key).count();
        Cursor { epoch: epoch.into(), key: key.clone(), alike }
    }
}

impl std::fmt::Display for Cursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "c1.{}~{}", self.epoch, self.key)?;
        if self.alike > 1 {
            write!(f, "#{}", self.alike)?;
        }
        Ok(())
    }
}

/// How long the app may take to answer, within the read's deadline.
fn wait_within(until: Instant) -> Result<Duration, Refusal> {
    match READ_WAIT.min(until.saturating_duration_since(Instant::now())) {
        wait if wait.is_zero() => Err((code::DEADLINE, "the read's deadline passed before it started: nothing was read".into())),
        wait => Ok(wait),
    }
}

/// What the bridge doesn't read yet, refused before the app is asked: playlists, and an instrument's parameters, which are
/// only in the session file.
fn not_yet(named: &Ref) -> Option<Refusal> {
    match named {
        Ref::Playlist { .. } => Some(bad(format!("{named}: SoundCraft's bridge doesn't read playlists yet"))),
        Ref::Param { slot: Slot::Instrument, .. } => Some(bad(format!(
            "{named}: an instrument's parameters are only in the session file, which SoundCraft's bridge doesn't read yet"
        ))),
        _ => None,
    }
}

fn text(params: &Value, key: &str) -> Result<Option<String>, Refusal> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.is_empty() => Ok(Some(text.clone())),
        Some(_) => Err(bad(format!("{key} must be a non-empty string"))),
    }
}

fn limit(params: &Value, default: usize, most: usize) -> Result<usize, Refusal> {
    match params.get("limit") {
        None | Some(Value::Null) => Ok(default),
        Some(limit) => limit
            .as_u64()
            .filter(|limit| (1..=most as u64).contains(limit))
            .map(|limit| limit as usize)
            .ok_or_else(|| bad(format!("limit must be a whole number from 1 to {most}"))),
    }
}

fn bad(why: String) -> Refusal {
    (code::BAD_PARAMS, why)
}

/// What a ref that names nothing there now gets.
fn gone(missing: Missing, named: &Ref) -> Refusal {
    match missing {
        Missing::Stale => (code::STALE, format!("{named} names an earlier object on that id: read again for its ref now")),
        Missing::Unknown => (code::STALE, format!("{named} isn't from this session: read again")),
        Missing::Gone => (code::FAILED, format!("{named} isn't in the session now (deleted, or undone)")),
    }
}

/// What a request the app didn't answer gets: busy (nothing read) or failed.
fn asked(error: Error) -> Refusal {
    if matches!(error, Error::Busy | Error::Deadline | Error::Stalled | Error::Crowded) {
        (code::BUSY, format!("{error}: nothing was read"))
    } else {
        (code::FAILED, error.to_string())
    }
}

fn list<'a>(of: &'a Value, key: &str) -> &'a [Value] {
    of.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

fn tracks(session: &Value) -> &[Value] {
    list(session, "tracks")
}

fn seconds(session: &Value, samples: &Value) -> Value {
    match (samples.as_f64(), session["sample_rate"].as_f64().filter(|rate| *rate > 0.)) {
        (Some(samples), Some(rate)) => json!((samples / rate * 1000.).round() / 1000.),
        _ => Value::Null,
    }
}

fn now(lives: &Lives, kind: Kind, item: &Value) -> Option<Id> {
    lives.now(kind, item["id"].as_u64()?)
}

/// A route (`Main`, `None`, `{"Bus": 1}`): a bus as its ref under `<key>Ref`, anything else as words under `<key>`.
fn route(row: &mut Value, key: &str, route: &Value, lives: &Lives) {
    match route.get("Bus").and_then(Value::as_u64) {
        Some(bus) => match lives.now(Kind::Bus, bus) {
            Some(id) => row[format!("{key}Ref")] = Ref::Object(id).to_string().into(),
            None => row[key] = format!("bus {bus}").into(),
        },
        None => row[key] = route.clone(),
    }
}

fn slot_of(insert: &Value) -> Option<Slot> {
    insert["slot"].as_u64().filter(|slot| *slot < 10).map(|slot| Slot::Insert(slot as u8))
}

/// The session as kumi-daw/1 names a project: its name and path; its id, which SoundCraft keeps none of, as a saved
/// session's place ([`place_id`]); and its identity, the open session's: its epoch, which only another session changes (a
/// save or Save As keeps it), so Kumi's conversation goes on into the place a save gives it.
fn project_row(session: &Value, epoch: &str) -> Value {
    let path = session["path"].as_str().filter(|path| !path.is_empty());
    json!({
        "id": path.and_then(place_id),
        "identity": epoch,
        "name": session["name"],
        "path": path,
        "saved": path.is_some(),
        "dirty": session["dirty"],
        "sampleRate": session["sample_rate"],
        "lengthSeconds": seconds(session, &session["length_samples"]),
        "editMode": session["edit_mode"],
        "grid": session["grid"],
    })
}

/// A saved session's place: its file's path made canonical (through links, in the case the file system keeps) and hashed
/// to the 32 hex digits a place takes, so its conversations stay with the file. A path that doesn't resolve now is hashed
/// as SoundCraft holds it; a relative one gives none, as SoundCraft resolves it from a folder the bridge can't know. The
/// place follows the file's path, not the session: a Save As, or the file moved or renamed outside SoundCraft, is another
/// place (REAPER's and Renoise's ids travel inside their files).
fn place_id(path: &str) -> Option<String> {
    let path = std::path::Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let canonical = std::fs::canonicalize(path).ok();
    let named = canonical.as_deref().unwrap_or(path).to_string_lossy();
    Some(sha256_hex(named.as_bytes())[..32].to_string())
}

/// A fingerprint of what an edit changes in the session (`session.inspect`), the same from its summary as from its full
/// inspection: all of it but what moves without an edit (the selection, the transport, zoom, the tool, whether it's
/// saved, the undo history's labels, which sources have loaded into memory) and what only the full inspection holds
/// (plug-ins' parameters; automation points, which count instead). For this run of the bridge only.
pub fn content(session: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut seen = session.clone();
    if let Some(fields) = seen.as_object_mut() {
        for moving in ["selection", "transport", "zoom", "tool", "dirty", "undo", "redo"] {
            fields.remove(moving);
        }
    }
    let each = |seen: &mut Value, key: &str| seen.get_mut(key).and_then(Value::as_array_mut).map(std::mem::take).unwrap_or_default();
    let mut sources = each(&mut seen, "sources");
    for source in sources.iter_mut().filter_map(Value::as_object_mut) {
        source.remove("loaded");
    }
    let mut tracks = each(&mut seen, "tracks");
    for track in &mut tracks {
        for insert in track.get_mut("inserts").and_then(Value::as_array_mut).into_iter().flatten().filter_map(Value::as_object_mut) {
            insert.remove("params");
        }
        for lane in track.get_mut("automation").and_then(Value::as_array_mut).into_iter().flatten() {
            if let Some(points) = lane["points"].as_array().map(Vec::len) {
                lane["points"] = points.into();
            }
        }
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (seen.to_string(), Value::Array(sources).to_string(), Value::Array(tracks).to_string()).hash(&mut hasher);
    hasher.finish()
}

fn session_row(session: &Value, lives: &Lives, epoch: &str) -> Value {
    let playhead = &session["transport"];
    let mut row = project_row(session, epoch);
    row["bitDepth"] = session["bit_depth"].clone();
    row["mainFormat"] = session["main_format"].clone();
    row["tempo"] = list(session, "tempo").first().map_or(Value::Null, |tempo| tempo["bpm"].clone());
    row["meter"] =
        list(session, "meter").first().map_or(Value::Null, |meter| format!("{}/{}", meter["numerator"], meter["denominator"]).into());
    row["trackCount"] = tracks(session).len().into();
    row["markerCount"] = list(session, "markers").len().into();
    row["selection"] = selection_row(session, lives);
    row["transport"] = transport(session, &playhead["position"], &playhead["playing"], &playhead["recording"]);
    row
}

/// The transport at `position` samples: whether it plays or records, seconds, and through the tempo map (when SoundCraft's
/// maps are steps) quarter notes from the start, the bar (from 1) and the quarter notes into it (from 1, with their
/// fraction, as kumi-daw/1 counts them in any meter), the tempo and the meter there.
pub fn transport(session: &Value, position: &Value, playing: &Value, recording: &Value) -> Value {
    let state = match (recording.as_bool(), playing.as_bool()) {
        (Some(true), _) => "recording",
        (_, Some(true)) => "playing",
        _ => "stopped",
    };
    let mut row = json!({"playing": playing, "recording": recording, "state": state, "seconds": seconds(session, position)});
    if let (Some(timebase), Some(samples)) = (Timebase::of(session), position.as_f64()) {
        let beats = timebase.beats(samples);
        let (numerator, denominator) = timebase.time_signature(beats);
        row["beat"] = thousandths(beats).into();
        row["bar"] = timebase.bar_beat(beats).bar.into();
        row["beatInBar"] = thousandths(1.0 + timebase.into_bar(beats)).into();
        row["tempo"] = timebase.tempo(beats).into();
        row["timeSignature"] = json!([numerator, denominator]);
        row["beatsPerBar"] = timebase.beats_a_bar(beats).into();
    }
    row
}

/// A beat or a position, to a thousandth.
fn thousandths(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// What the producer has selected: tracks (the first, `trackRef`, is the one Kumi follows), clips, and the time range,
/// whose start and end alike are the cursor. It names the first 50 tracks and 20 clips by ref, as REAPER does, and counts
/// them all, so a big selection can't outgrow the snapshot.
fn selection_row(session: &Value, lives: &Lives) -> Value {
    let selection = &session["selection"];
    let refs = |key: &str, kind: Kind, most: usize| {
        let named = list(selection, key).iter().filter_map(|id| Some(Ref::Object(lives.now(kind, id.as_u64()?)?).to_string()));
        named.take(most).collect::<Vec<_>>()
    };
    let track_refs = refs("tracks", Kind::Track, SELECTED_TRACKS);
    let mut row = json!({
        "trackRef": track_refs.first(),
        "trackRefs": track_refs,
        "trackCount": list(selection, "tracks").len(),
        "clipRefs": refs("clips", Kind::Clip, SELECTED_CLIPS),
        "clipCount": list(selection, "clips").len(),
        "startSeconds": seconds(session, &selection["start"]),
        "endSeconds": seconds(session, &selection["end"]),
    });
    let first = list(selection, "tracks").first().and_then(Value::as_u64);
    if let Some(track) = first.and_then(|id| tracks(session).iter().find(|track| track["id"].as_u64() == Some(id))) {
        row["track"] = json!({"name": track["name"], "type": chip(&track["kind"])});
    }
    row
}

/// A track's kind in the words Kumi's FOCUS chip shows: an instrument track plays MIDI, an aux track is a return.
fn chip(kind: &Value) -> Value {
    match kind.as_str() {
        Some("midi" | "instrument") => "midi".into(),
        Some("aux") => "send".into(),
        Some("vca") => "group".into(),
        _ => kind.clone(),
    }
}

/// How many of `rows`, from the first, fit `limit` rows and `bytes`: one at least.
fn first(rows: &[(String, Value)], limit: usize, bytes: usize) -> usize {
    let mut used = 0;
    let fit = rows
        .iter()
        .take(limit)
        .take_while(|(_, row)| {
            used += serde_json::to_string(row).map_or(0, |text| text.len() + 1);
            used <= bytes
        })
        .count();
    fit.max(rows.len().min(1))
}

fn track_row(session: &Value, track: &Value, index: usize, lives: &Lives) -> Option<(String, Value)> {
    let id = now(lives, Kind::Track, track)?;
    let key = Ref::Object(id).to_string();
    let selected = list(&session["selection"], "tracks").iter().any(|selected| selected.as_u64() == Some(id.id));
    let instrument = track.get("instrument").filter(|instrument| !instrument.is_null());
    let mut row = json!({
        "ref": key,
        "index": index,
        "name": track["name"],
        "type": track["kind"],
        "format": track["format"],
        "mute": track["mute"],
        "solo": track["solo"],
        "recordArm": track["record_arm"],
        "volumeDb": track["volume_db"],
        "pan": track["pan"],
        "hidden": track["hidden"],
        "inactive": track["inactive"],
        "automationMode": track["automation_mode"],
        "deviceCount": list(track, "inserts").len() + usize::from(instrument.is_some()),
        "sendCount": list(track, "sends").len(),
        "clipCount": list(track, "clips").len(),
        "playlist": track["playlist"],
        "playlists": track["playlists"],
        "selected": selected,
    });
    if let Some(instrument) = instrument {
        row["instrument"] = instrument.clone();
    }
    route(&mut row, "input", &track["input"], lives);
    route(&mut row, "output", &track["output"], lives);
    Some((key, row))
}

fn clip_row(session: &Value, track: &Value, clip: &Value, lives: &Lives) -> Option<(String, Value)> {
    let id = now(lives, Kind::Clip, clip)?;
    let key = Ref::Object(id).to_string();
    let selected = list(&session["selection"], "clips").iter().any(|selected| selected.as_u64() == Some(id.id));
    let mut row = json!({
        "ref": key,
        "trackRef": Ref::Object(now(lives, Kind::Track, track)?).to_string(),
        "name": clip["name"],
        "start": clip["start"],
        "length": clip["length"],
        "startSeconds": seconds(session, &clip["start"]),
        "lengthSeconds": seconds(session, &clip["length"]),
        "gainDb": clip["gain_db"],
        "muted": clip["muted"],
        "locked": clip["locked"],
        "fadeIn": clip["fade_in"],
        "fadeOut": clip["fade_out"],
        "selected": selected,
    });
    // (what it plays: a source's audio, its notes, or a video)
    if let Some(source) = clip["source"].as_u64() {
        if let Some(source) = lives.now(Kind::Source, source) {
            row["sourceRef"] = Ref::Object(source).to_string().into();
        }
        row["offset"] = clip["offset"].clone();
    } else if clip.get("notes").is_some() {
        row["notes"] = clip["notes"].clone();
    } else if clip.get("video").is_some() {
        row["video"] = clip["video"].clone();
        row["offset"] = clip["offset"].clone();
    }
    Some((key, row))
}

/// A track's inserts: its instrument first, when it has one, then its slots a to j.
fn insert_rows(track: &Value, lives: &Lives) -> Vec<(String, Value)> {
    instrument_row(track, lives)
        .into_iter()
        .chain(list(track, "inserts").iter().filter_map(|insert| insert_row(track, insert, lives)))
        .collect()
}

/// An instrument track's instrument, as the insert `s:insert:<track>:instrument`: its plug-in (its parameters are only in
/// the session file).
fn instrument_row(track: &Value, lives: &Lives) -> Option<(String, Value)> {
    let plugin = track.get("instrument").filter(|plugin| !plugin.is_null())?;
    let track_id = now(lives, Kind::Track, track)?;
    let key = Ref::Insert { track: track_id, slot: Slot::Instrument }.to_string();
    let row = json!({"ref": key, "trackRef": Ref::Object(track_id).to_string(), "slot": "instrument", "plugin": plugin});
    Some((key, row))
}

fn insert_row(track: &Value, insert: &Value, lives: &Lives) -> Option<(String, Value)> {
    let track_id = now(lives, Kind::Track, track)?;
    let slot = slot_of(insert)?;
    let key = Ref::Insert { track: track_id, slot }.to_string();
    let params = insert["params"].as_object().map(Map::len);
    let row = json!({
        "ref": key,
        "trackRef": Ref::Object(track_id).to_string(),
        "slot": key.rsplit(':').next(),
        "plugin": insert["plugin"],
        "bypass": insert["bypass"],
        "paramCount": params,
    });
    Some((key, row))
}

fn bus_row(bus: &Value, lives: &Lives) -> Option<(String, Value)> {
    object_row(Kind::Bus, bus, lives, |bus| json!({"name": bus["name"], "format": bus["format"]}))
}

/// An insert's parameters. A built-in's are as `engine.plugins` describes them (name, range, default, unit, taper and
/// choices), with their values now; a hosted plug-in's are by number, with only their values: SoundCraft names no
/// hosted parameter yet.
fn param_rows(track: Id, slot: Slot, insert: &Value, plugins: &[Value]) -> Vec<(String, Value)> {
    let values = insert["params"].as_object();
    let insert_ref = Ref::Insert { track, slot }.to_string();
    let param_ref = |id: &str| Ref::Param { track, slot, param: id.into() }.to_string();
    match plugins.iter().find(|plugin| plugin["id"] == insert["plugin"]).and_then(|plugin| plugin["params"].as_array()) {
        Some(described) => described
            .iter()
            .filter_map(|param| {
                let id = param["id"].as_str()?;
                let key = param_ref(id);
                let value = values.and_then(|values| values.get(id)).cloned().unwrap_or(Value::Null);
                let mut row = json!({
                    "ref": key,
                    "insertRef": insert_ref,
                    "id": id,
                    "name": param["name"],
                    "value": value,
                    "min": param["min"],
                    "max": param["max"],
                    "default": param["default"],
                    "unit": param["unit"],
                    "taper": param["taper"],
                });
                if let Some(choices) = param["choices"].as_array().filter(|choices| !choices.is_empty()) {
                    // (a choice's value is its index)
                    row["choice"] = value.as_f64().and_then(|at| choices.get(at.round() as usize)).cloned().unwrap_or(Value::Null);
                    row["choices"] = choices.clone().into();
                }
                Some((key, row))
            })
            .collect(),
        None => {
            let mut rows: Vec<(String, Value)> = values
                .into_iter()
                .flatten()
                .map(|(id, value)| {
                    let key = param_ref(id);
                    (key.clone(), json!({"ref": key, "insertRef": insert_ref, "id": id, "value": value}))
                })
                .collect();
            // (SoundCraft keeps them by their text, 0, 1, 10, 11 …: by number, as the plug-in numbers them)
            let number = |row: &Value| row["id"].as_str().and_then(|id| id.parse::<u32>().ok());
            if rows.iter().all(|(_, row)| number(row).is_some()) {
                rows.sort_by_key(|(_, row)| number(row));
            }
            rows
        }
    }
}

/// SoundCraft's automation id for the lane inspect labels `label`, as its UI writes it and its automation commands take
/// it: `insert A hpf_freq` is `plugin:a:hpf_freq`, `send B level` is `send_b_level`, `pan 2` is `pan2`, and the trim lane,
/// `insert Z trim` (its slot is past the letters), is `plugin:255:trim`. A lane no ref can spell has none: a command can
/// make a plug-in lane of any text (`plugin:a:hpf freq`), which SoundCraft's UI never writes.
fn lane_id(label: &str) -> Option<String> {
    spelled_lane(label).filter(|id| lane(id))
}

/// How inspect's label spells SoundCraft's id, before [`lane_id`] keeps only what a ref takes (the slots, the send
/// words and the parameter's characters are the ref grammar's to judge).
fn spelled_lane(label: &str) -> Option<String> {
    if label == "insert Z trim" {
        return Some("plugin:255:trim".into());
    }
    if let Some(rest) = label.strip_prefix("insert ") {
        let (letter, param) = rest.split_once(' ')?;
        return Some(format!("plugin:{}:{param}", letter.to_ascii_lowercase()));
    }
    if let Some(rest) = label.strip_prefix("send ") {
        let (letter, what) = rest.split_once(' ')?;
        return Some(format!("send_{}_{what}", letter.to_ascii_lowercase()));
    }
    Some(if label == "pan 2" { "pan2".into() } else { label.into() })
}

/// The lane on a track that moves `id` (an automation id).
fn lane_of<'a>(track: &'a Value, id: &str) -> Option<&'a Value> {
    list(track, "automation").iter().find(|lane| lane["param"].as_str().and_then(lane_id).as_deref() == Some(id))
}

fn lane_row(track: &Value, lane: &Value, lives: &Lives) -> Option<(String, Value)> {
    let track_id = now(lives, Kind::Track, track)?;
    let label = lane["param"].as_str()?;
    let id = lane_id(label)?;
    let key = Ref::Lane { track: track_id, param: id.clone() }.to_string();
    let row = json!({
        "ref": key,
        "trackRef": Ref::Object(track_id).to_string(),
        "id": id,
        "label": label,
        "points": list(lane, "points").len(),
    });
    Some((key, row))
}

/// A lane's points. SoundCraft's own writes keep one at a sample (`set_point` replaces the point there), but a session it
/// loads can hold two: those are marked `tie`, as notes alike are, since one ref can't tell them apart.
fn point_rows(session: &Value, track: Id, id: &str, lane: &Value) -> Vec<(String, Value)> {
    let lane_ref = Ref::Lane { track, param: id.into() }.to_string();
    let points = list(lane, "points");
    let mut at_sample: HashMap<i64, usize> = HashMap::with_capacity(points.len());
    for at in points.iter().filter_map(|point| point["at"].as_i64()) {
        *at_sample.entry(at).or_default() += 1;
    }
    points
        .iter()
        .filter_map(|point| {
            let at = point["at"].as_i64()?;
            let key = Ref::Point { track, param: id.into(), sample: at }.to_string();
            let mut row =
                json!({"ref": key, "laneRef": lane_ref, "at": at, "seconds": seconds(session, &point["at"]), "value": point["value"]});
            if let Some(count) = at_sample.get(&at).filter(|count| **count > 1) {
                row["tie"] = (*count).into();
            }
            Some((key, row))
        })
        .collect()
}

fn send_row(track: &Value, send: &Value, lives: &Lives) -> Option<(String, Value)> {
    let track_id = now(lives, Kind::Track, track)?;
    let slot = u32::try_from(send["slot"].as_u64()?).ok()?;
    let key = Ref::Send { track: track_id, slot }.to_string();
    let mut row = json!({
        "ref": key,
        "trackRef": Ref::Object(track_id).to_string(),
        "slot": slot,
        "levelDb": send["level_db"],
        "pan": send["pan"],
        "mute": send["mute"],
        "preFader": send["pre_fader"],
    });
    route(&mut row, "target", &send["target"], lives);
    Some((key, row))
}

fn object_row(kind: Kind, item: &Value, lives: &Lives, fields: impl Fn(&Value) -> Value) -> Option<(String, Value)> {
    let key = Ref::Object(now(lives, kind, item)?).to_string();
    let mut row = fields(item);
    row["ref"] = key.clone().into();
    Some((key, row))
}

fn marker_row(session: &Value, marker: &Value, lives: &Lives) -> Option<(String, Value)> {
    object_row(Kind::Marker, marker, lives, |marker| {
        json!({
            "number": marker["number"],
            "name": marker["name"],
            "kind": marker["kind"],
            "start": marker["start"],
            "end": marker["end"],
            "startSeconds": seconds(session, &marker["start"]),
        })
    })
}

fn group_row(group: &Value, lives: &Lives) -> Option<(String, Value)> {
    object_row(Kind::Group, group, lives, |group| {
        let members: Vec<String> = list(group, "members")
            .iter()
            .filter_map(|member| Some(Ref::Object(lives.now(Kind::Track, member.as_u64()?)?).to_string()))
            .collect();
        json!({"name": group["name"], "letter": group["letter"], "active": group["active"], "memberRefs": members})
    })
}

fn source_row(session: &Value, source: &Value, lives: &Lives) -> Option<(String, Value)> {
    object_row(Kind::Source, source, lives, |source| {
        json!({
            "name": source["name"],
            "channels": source["channels"],
            "frames": source["frames"],
            "seconds": seconds(session, &source["frames"]),
            "path": source["path"],
            "loaded": source["loaded"],
        })
    })
}
