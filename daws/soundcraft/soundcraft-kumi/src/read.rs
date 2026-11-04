//! `read`: SoundCraft's session as rows, a page at a time, each object named by its ref ([`crate::refs`]).
//!
//! The session's objects come from `session.inspect {"detail":"full"}`, asked again at most once a second and shared by the
//! reads in between, and a clip's notes from `midi.notes`. Every answer carries the session's epoch: a new one starts when
//! another session is open (its name or path changes), and refs or cursors from an earlier one are stale. A page ends at
//! the limit asked for (50, at most 200) or at the bytes an answer may take. Its cursor names the position and the object
//! there, so a list that changed between pages says so rather than skipping or repeating rows.
use crate::control::{Control, Error};
use crate::refs::{note_index, Id, Kind, Lives, Missing, NoteMissing, Ref, Slot};
use daw_bridge::wire::code;
use serde_json::{json, Map, Value};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 200;
/// What `read` takes as a kind (`note` with a clip as its parent).
pub const KINDS: [&str; 10] = ["session", "track", "clip", "insert", "send", "bus", "marker", "group", "source", "note"];
/// The bytes a page's rows may take: a frame's bound is 256 KiB, and the rest is the envelope.
const ANSWER_BYTES: usize = 224 * 1024;
/// How long one inspection serves the reads after it.
const FRESH: Duration = Duration::from_secs(1);
/// The longest a read waits for the app.
const READ_WAIT: Duration = Duration::from_secs(5);

/// Why a read has no answer: its kumi-daw/1 code, and the words.
pub type Refusal = (&'static str, String);

/// The bridge's reads of one copy of SoundCraft: the lives of its objects, the session's epoch, and the last inspection.
pub struct Reads {
    control: Control,
    lives: Lives,
    started: u64,
    serial: u32,
    /// The session the epoch is for: its name and path.
    session: Option<(Value, Value)>,
    inspected: Option<(Instant, Value)>,
}

impl Reads {
    pub fn new(control: Control) -> Self {
        let started = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64);
        Self { control, lives: Lives::default(), started, serial: 1, session: None, inspected: None }
    }

    /// The session's epoch now: `<start, hex>-<serial>`.
    pub fn epoch(&self) -> String {
        format!("{:x}-{}", self.started, self.serial)
    }

    /// One `read`: `{kind, parent?, cursor?, limit?}` for a list, or `{ref}` for one object, with the `epoch` the refs
    /// came from. Not started after `until` (the request's deadline).
    pub async fn read(&mut self, params: &Value, until: Instant) -> Result<Value, Refusal> {
        let (kind, one, parent, cursor, epoch) =
            (text(params, "kind")?, text(params, "ref")?, text(params, "parent")?, text(params, "cursor")?, text(params, "epoch")?);
        let limit = limit(params)?;
        if kind.is_none() && one.is_none() {
            return Err(bad(format!("kind: what to read, one of {} (or ref: one object)", KINDS.join(", "))));
        }
        let session = self.inspect(until).await?;
        if let Some(epoch) = epoch.filter(|epoch| *epoch != self.epoch()) {
            return Err((
                code::STALE,
                format!("those refs are from an earlier epoch ({epoch}; now {}): another session is open, read again", self.epoch()),
            ));
        }
        if let Some(one) = one {
            let row = self.one(&session, &one.parse::<Ref>().map_err(bad)?, until).await?;
            return Ok(json!({"items": [row], "epoch": self.epoch()}));
        }
        let kind = kind.expect("a kind or a ref");
        let parent = parent.map(|parent| parent.parse::<Ref>().map_err(bad)).transpose()?;
        // (each row with its ref, which a cursor names it by)
        let rows: Vec<(String, Value)> = match kind.as_str() {
            "session" => vec![("session".to_string(), session_row(&session, &self.lives))],
            "track" => {
                tracks(&session).iter().enumerate().filter_map(|(index, track)| track_row(&session, track, index, &self.lives)).collect()
            }
            "clip" => self.under(&session, parent.as_ref(), "clips", |track, clip, lives| clip_row(&session, track, clip, lives))?,
            "insert" => self.under(&session, parent.as_ref(), "inserts", insert_row)?,
            "send" => self.under(&session, parent.as_ref(), "sends", send_row)?,
            "bus" => list(&session, "busses")
                .iter()
                .filter_map(|bus| object_row(Kind::Bus, bus, &self.lives, |bus| json!({"name": bus["name"], "format": bus["format"]})))
                .collect(),
            "marker" => list(&session, "markers").iter().filter_map(|marker| marker_row(&session, marker, &self.lives)).collect(),
            "group" => list(&session, "groups").iter().filter_map(|group| group_row(group, &self.lives)).collect(),
            "source" => list(&session, "sources").iter().filter_map(|source| source_row(&session, source, &self.lives)).collect(),
            "note" => {
                let Some(Ref::Object(clip)) = parent.filter(|parent| matches!(parent, Ref::Object(Id { kind: Kind::Clip, .. }))) else {
                    return Err(bad("parent: the clip whose notes to read (s:clip:…)".into()));
                };
                self.notes(&clip, until).await?
            }
            other => return Err(bad(format!("kind: one of {}, not {other}", KINDS.join(", ")))),
        };
        self.page(rows, cursor.as_deref(), limit)
    }

    /// The session as the app shows it now, from the last inspection when it's fresh enough. A new session starts a new
    /// epoch, and the lives of its objects start again.
    async fn inspect(&mut self, until: Instant) -> Result<Value, Refusal> {
        if let Some((at, session)) = &self.inspected {
            if at.elapsed() < FRESH {
                return Ok(session.clone());
            }
        }
        let wait = READ_WAIT.min(until.saturating_duration_since(Instant::now()));
        if wait.is_zero() {
            return Err((code::DEADLINE, "the read's deadline passed before it started: nothing was read".into()));
        }
        let session = self.control.call("session.inspect", json!({"detail": "full"}), wait).await.map_err(asked)?;
        let named = (session["name"].clone(), session["path"].clone());
        if self.session.as_ref().is_some_and(|before| *before != named) {
            self.serial += 1;
            self.lives = Lives::default();
        }
        self.session = Some(named);
        self.lives.saw_session(&session);
        self.inspected = Some((Instant::now(), session.clone()));
        Ok(session)
    }

    /// The rows of one kind of a track's things (clips, inserts, sends): one track's, as `parent` names, else every track's.
    fn under(
        &self,
        session: &Value,
        parent: Option<&Ref>,
        key: &str,
        row: impl Fn(&Value, &Value, &Lives) -> Option<(String, Value)>,
    ) -> Result<Vec<(String, Value)>, Refusal> {
        let from: Vec<&Value> = match parent {
            None => tracks(session).iter().collect(),
            Some(Ref::Object(id @ Id { kind: Kind::Track, .. })) => vec![self.track(session, id)?],
            Some(other) => return Err(bad(format!("parent: a track (s:track:…), not {other}"))),
        };
        Ok(from
            .into_iter()
            .flat_map(|track| list(track, key).iter().filter_map(|item| row(track, item, &self.lives)).collect::<Vec<_>>())
            .collect())
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
                    Kind::Bus => object_row(Kind::Bus, item, &self.lives, |bus| json!({"name": bus["name"], "format": bus["format"]})),
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
                let insert = list(track, "inserts").iter().find(|insert| slot_of(insert) == Some(*slot)).ok_or_else(missing)?;
                insert_row(track, insert, &self.lives).map(|(_, row)| row).ok_or_else(missing)
            }
            Ref::Send { track, slot } => {
                let track = self.track(session, track)?;
                let send = list(track, "sends").iter().find(|send| send["slot"].as_u64() == Some((*slot).into())).ok_or_else(missing)?;
                send_row(track, send, &self.lives).map(|(_, row)| row).ok_or_else(missing)
            }
            Ref::Note { clip, start, pitch, channel } => {
                let notes = self.notes(clip, until).await?;
                let at = note_index(&notes.iter().map(|(_, row)| row.clone()).collect::<Vec<_>>(), *start, *pitch, *channel).map_err(
                    |missing| match missing {
                        NoteMissing::Gone => gone(Missing::Gone, named),
                        NoteMissing::Tie(count) => (
                            code::FAILED,
                            format!("{count} notes of that clip have that start, pitch and channel: read its notes, which mark the tie"),
                        ),
                    },
                )?;
                Ok(notes[at].1.clone())
            }
            Ref::Param { .. } | Ref::Lane { .. } | Ref::Point { .. } | Ref::Playlist { .. } => {
                Err(bad(format!("{named}: SoundCraft's bridge doesn't read parameters, automation or playlists yet")))
            }
        }
    }

    /// A clip's notes as rows (`midi.notes`), in SoundCraft's order: start, pitch, length. Notes alike (the same start,
    /// pitch and channel, which SoundCraft keeps both of) are marked `tie`: one ref can't tell them apart.
    async fn notes(&mut self, clip: &Id, until: Instant) -> Result<Vec<(String, Value)>, Refusal> {
        let named = Ref::Object(*clip);
        let id = self.lives.resolve(clip).map_err(|missing| gone(missing, &named))?;
        let wait = READ_WAIT.min(until.saturating_duration_since(Instant::now()));
        if wait.is_zero() {
            return Err((code::DEADLINE, "the read's deadline passed before it started: nothing was read".into()));
        }
        let answer = self.control.execute("midi.notes", json!({"clip": id}), wait).await.map_err(|error| match error {
            Error::Refused(said) => (code::FAILED, format!("{named}: {said}")),
            other => asked(other),
        })?;
        let notes = answer["notes"].as_array().cloned().unwrap_or_default();
        let field = |note: &Value, key: &str| note.get(key).and_then(Value::as_i64).unwrap_or_default();
        Ok(notes
            .iter()
            .map(|note| {
                let (start, pitch, channel) = (field(note, "start"), field(note, "pitch"), field(note, "channel"));
                let alike = notes
                    .iter()
                    .filter(|other| field(other, "start") == start && field(other, "pitch") == pitch && field(other, "channel") == channel)
                    .count();
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
                if alike > 1 {
                    row["tie"] = alike.into();
                }
                (key, row)
            })
            .collect())
    }

    /// A page of rows from the cursor's place: up to `limit`, and no more bytes than an answer may take.
    fn page(&self, rows: Vec<(String, Value)>, cursor: Option<&str>, limit: usize) -> Result<Value, Refusal> {
        let from = match cursor {
            None => 0,
            Some(cursor) => self.resume(cursor, &rows)?,
        };
        let (mut items, mut bytes, mut next) = (vec![], 0, None);
        for (position, (key, row)) in rows.iter().enumerate().skip(from) {
            let size = serde_json::to_string(row).map_or(0, |text| text.len() + 1);
            if items.len() == limit || (!items.is_empty() && bytes + size > ANSWER_BYTES) {
                next = Some(format!("c1.{}.{position}~{key}", self.epoch()));
                break;
            }
            bytes += size;
            items.push(row.clone());
        }
        let mut answer = json!({"items": items, "epoch": self.epoch(), "total": rows.len(), "indexFrom": 0});
        if let Some(next) = next {
            answer["nextCursor"] = next.into();
        }
        Ok(answer)
    }

    /// Where a cursor's page starts: at the object it names, wherever that is now.
    fn resume(&self, cursor: &str, rows: &[(String, Value)]) -> Result<usize, Refusal> {
        let unknown = || bad("cursor: pass the nextCursor an earlier read gave, unchanged".into());
        let (epoch, rest) = cursor.strip_prefix("c1.").and_then(|rest| rest.split_once('.')).ok_or_else(unknown)?;
        let (position, key) = rest.split_once('~').ok_or_else(unknown)?;
        let position: usize = position.parse().map_err(|_| unknown())?;
        if epoch != self.epoch() {
            return Err((code::STALE, "that cursor is from an earlier epoch: read again from the start".into()));
        }
        if rows.get(position).is_some_and(|(at, _)| at == key) {
            return Ok(position);
        }
        rows.iter()
            .position(|(at, _)| at == key)
            .ok_or((code::CHANGED, "the list changed since that cursor: read it again from the start".into()))
    }
}

fn text(params: &Value, key: &str) -> Result<Option<String>, Refusal> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.is_empty() => Ok(Some(text.clone())),
        Some(_) => Err(bad(format!("{key} must be a non-empty string"))),
    }
}

fn limit(params: &Value) -> Result<usize, Refusal> {
    match params.get("limit") {
        None | Some(Value::Null) => Ok(DEFAULT_LIMIT),
        Some(limit) => limit
            .as_u64()
            .filter(|limit| (1..=MAX_LIMIT as u64).contains(limit))
            .map(|limit| limit as usize)
            .ok_or_else(|| bad(format!("limit must be a whole number from 1 to {MAX_LIMIT}"))),
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

fn session_row(session: &Value, lives: &Lives) -> Value {
    let selection = &session["selection"];
    let refs = |key: &str, kind: Kind| {
        list(selection, key).iter().filter_map(|id| Some(Ref::Object(lives.now(kind, id.as_u64()?)?).to_string())).collect::<Vec<_>>()
    };
    let meter = list(session, "meter").first().map(|meter| format!("{}/{}", meter["numerator"], meter["denominator"]));
    json!({
        "name": session["name"],
        "path": session["path"],
        "dirty": session["dirty"],
        "sampleRate": session["sample_rate"],
        "bitDepth": session["bit_depth"],
        "mainFormat": session["main_format"],
        "lengthSeconds": seconds(session, &session["length_samples"]),
        "tempo": list(session, "tempo").first().map(|tempo| tempo["bpm"].clone()),
        "meter": meter,
        "editMode": session["edit_mode"],
        "grid": session["grid"],
        "trackCount": tracks(session).len(),
        "markerCount": list(session, "markers").len(),
        "selection": {
            "trackRefs": refs("tracks", Kind::Track),
            "clipRefs": refs("clips", Kind::Clip),
            "startSeconds": seconds(session, &selection["start"]),
            "endSeconds": seconds(session, &selection["end"]),
        },
        "transport": {
            "playing": session["transport"]["playing"],
            "recording": session["transport"]["recording"],
            "seconds": seconds(session, &session["transport"]["position"]),
        },
    })
}

fn track_row(session: &Value, track: &Value, index: usize, lives: &Lives) -> Option<(String, Value)> {
    let id = now(lives, Kind::Track, track)?;
    let key = Ref::Object(id).to_string();
    let selected = list(&session["selection"], "tracks").iter().any(|selected| selected.as_u64() == Some(id.id));
    let mut row = json!({
        "ref": key,
        "index": index,
        "name": track["name"],
        "kind": track["kind"],
        "format": track["format"],
        "mute": track["mute"],
        "solo": track["solo"],
        "recordArm": track["record_arm"],
        "volumeDb": track["volume_db"],
        "pan": track["pan"],
        "hidden": track["hidden"],
        "inactive": track["inactive"],
        "automationMode": track["automation_mode"],
        "insertCount": list(track, "inserts").len(),
        "sendCount": list(track, "sends").len(),
        "clipCount": list(track, "clips").len(),
        "playlist": track["playlist"],
        "playlists": track["playlists"],
        "selected": selected,
    });
    if let Some(instrument) = track.get("instrument").filter(|instrument| !instrument.is_null()) {
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
    match clip.get("source").and_then(Value::as_u64).and_then(|source| lives.now(Kind::Source, source)) {
        Some(source) => {
            row["sourceRef"] = Ref::Object(source).to_string().into();
            row["offset"] = clip["offset"].clone();
        }
        None => row["notes"] = clip["notes"].clone(),
    }
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
