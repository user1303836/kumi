//! Refs: the names the bridge gives SoundCraft's objects, and the lives behind them.
//!
//! A ref is `s:<kind>:<parts>`, with SoundCraft's own ids where it has them, and positional parts spelled out, to be
//! resolved again at each write: `s:track:12`, `s:clip:40`, `s:insert:12:a`, `s:param:12:a:high_shelf_gain`,
//! `s:note:40:480.60.0`. Undo puts SoundCraft's next id back, so an id can come back on another object (tracks "First"
//! and "Second" both got id 1). The bridge keeps the lives of the ids it has seen in a session: an id that comes back
//! with another fingerprint is a new life, and its ref takes the life's number, `s:track:1.2`. A ref from an earlier life
//! is stale. A Kumi change's object that an undo crossed is retired at once: if its id comes back, it's a new life.
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// The kinds of object with an id of SoundCraft's own, and so a life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Track,
    Clip,
    Bus,
    Group,
    Marker,
    Source,
}

impl Kind {
    pub const ALL: [Kind; 6] = [Kind::Track, Kind::Clip, Kind::Bus, Kind::Group, Kind::Marker, Kind::Source];

    /// Its word in a ref.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Track => "track",
            Kind::Clip => "clip",
            Kind::Bus => "bus",
            Kind::Group => "group",
            Kind::Marker => "marker",
            Kind::Source => "source",
        }
    }

    fn from_word(word: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.word() == word)
    }
}

/// One life of an object with an id: the id, and which life of it (1 for the first, never written).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id {
    pub kind: Kind,
    pub id: u64,
    pub life: u32,
}

impl Id {
    /// The id's part of a ref: `12`, or `12.2` for its second life.
    fn part(&self) -> String {
        if self.life > 1 {
            format!("{}.{}", self.id, self.life)
        } else {
            self.id.to_string()
        }
    }

    fn parse_part(kind: Kind, part: &str) -> Option<Id> {
        let (id, life) = match part.split_once('.') {
            Some((id, life)) => (id, life.parse().ok().filter(|life: &u32| *life > 1)?),
            None => (part, 1),
        };
        Some(Id { kind, id: number(id)?, life })
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "s:{}:{}", self.kind.word(), self.part())
    }
}

/// An insert's place on its track: slots a to j, or the instrument of an instrument track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    /// 0 to 9: a to j.
    Insert(u8),
    Instrument,
}

impl Slot {
    fn part(self) -> String {
        match self {
            Slot::Insert(slot) => char::from(b'a' + slot).to_string(),
            Slot::Instrument => "instrument".into(),
        }
    }

    fn parse(part: &str) -> Option<Slot> {
        match part.as_bytes() {
            b"instrument" => Some(Slot::Instrument),
            [letter @ b'a'..=b'j'] => Some(Slot::Insert(letter - b'a')),
            _ => None,
        }
    }
}

/// What a ref names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ref {
    /// A track, clip, bus, group, marker or source.
    Object(Id),
    Insert {
        track: Id,
        slot: Slot,
    },
    /// A send, by its place in the track's sends (from 0).
    Send {
        track: Id,
        slot: u32,
    },
    /// A plug-in parameter, by its id: a built-in's name (`high_shelf_gain`), or a hosted plug-in's number.
    Param {
        track: Id,
        slot: Slot,
        param: String,
    },
    /// An automation lane, by what it moves, as SoundCraft's automation commands name it: `volume`, `pan`, `pan2`, `mute`,
    /// `send_a_level`, or a plug-in's parameter, `plugin:a:high_shelf_gain`.
    Lane {
        track: Id,
        param: String,
    },
    /// A point on a lane, by its sample.
    Point {
        track: Id,
        param: String,
        sample: i64,
    },
    /// A note, by its start (ticks from its clip's start), pitch and channel.
    Note {
        clip: Id,
        start: i64,
        pitch: u8,
        channel: u8,
    },
    /// A playlist, by its place in the track's playlists (from 0).
    Playlist {
        track: Id,
        index: u32,
    },
}

impl std::fmt::Display for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ref::Object(id) => write!(f, "{id}"),
            Ref::Insert { track, slot } => write!(f, "s:insert:{}:{}", track.part(), slot.part()),
            Ref::Send { track, slot } => write!(f, "s:send:{}:{slot}", track.part()),
            Ref::Param { track, slot, param } => write!(f, "s:param:{}:{}:{param}", track.part(), slot.part()),
            Ref::Lane { track, param } => write!(f, "s:env:{}:{param}", track.part()),
            Ref::Point { track, param, sample } => write!(f, "s:point:{}:{param}:{sample}", track.part()),
            Ref::Note { clip, start, pitch, channel } => write!(f, "s:note:{}:{start}.{pitch}.{channel}", clip.part()),
            Ref::Playlist { track, index } => write!(f, "s:playlist:{}:{index}", track.part()),
        }
    }
}

impl std::str::FromStr for Ref {
    type Err = String;

    /// A ref as the bridge spells it; anything else is refused with what it is.
    fn from_str(text: &str) -> Result<Ref, String> {
        parse(text).ok_or_else(|| format!("{text} isn't a SoundCraft ref (s:<kind>:<parts>, as a read gave it)"))
    }
}

fn parse(text: &str) -> Option<Ref> {
    let (word, parts) = text.strip_prefix("s:")?.split_once(':')?;
    let track = |part: &str| Id::parse_part(Kind::Track, part);
    match word {
        "insert" => {
            let (id, slot) = parts.split_once(':')?;
            Some(Ref::Insert { track: track(id)?, slot: Slot::parse(slot)? })
        }
        "send" => {
            let (id, slot) = parts.split_once(':')?;
            Some(Ref::Send { track: track(id)?, slot: number(slot)? })
        }
        "param" => {
            let mut split = parts.splitn(3, ':');
            let (id, slot, param) = (split.next()?, split.next()?, split.next()?);
            param_id(param).then_some(())?;
            Some(Ref::Param { track: track(id)?, slot: Slot::parse(slot)?, param: param.into() })
        }
        "env" => {
            let (id, param) = parts.split_once(':')?;
            lane(param).then_some(())?;
            Some(Ref::Lane { track: track(id)?, param: param.into() })
        }
        "point" => {
            let (id, rest) = parts.split_once(':')?;
            let (param, sample) = rest.rsplit_once(':')?;
            lane(param).then_some(())?;
            Some(Ref::Point { track: track(id)?, param: param.into(), sample: signed(sample)? })
        }
        "note" => {
            let (id, note) = parts.split_once(':')?;
            let mut fields = note.split('.');
            let (start, pitch, channel) = (fields.next()?, fields.next()?, fields.next()?);
            fields.next().is_none().then_some(())?;
            let pitch = number::<u8>(pitch).filter(|pitch| *pitch <= 127)?;
            let channel = number::<u8>(channel).filter(|channel| *channel <= 15)?;
            Some(Ref::Note { clip: Id::parse_part(Kind::Clip, id)?, start: signed(start)?, pitch, channel })
        }
        "playlist" => {
            let (id, index) = parts.split_once(':')?;
            Some(Ref::Playlist { track: track(id)?, index: number(index)? })
        }
        word => Id::parse_part(Kind::from_word(word)?, parts).map(Ref::Object),
    }
}

/// A plain number: digits only, no sign, no leading `+`.
fn number<T: std::str::FromStr>(text: &str) -> Option<T> {
    (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())).then(|| text.parse().ok()).flatten()
}

/// A number that may be negative.
fn signed(text: &str) -> Option<i64> {
    match text.strip_prefix('-') {
        Some(digits) => number::<i64>(digits).map(|n| -n),
        None => number(text),
    }
}

/// A parameter's id: a built-in's name or a hosted plug-in's number.
fn param_id(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// What a lane moves, in SoundCraft's automation ids: a word of the track's (`volume`, `pan2`, `send_a_level`), or
/// `plugin:<slot>:<parameter>`.
fn lane(text: &str) -> bool {
    match text.strip_prefix("plugin:").and_then(|plugin| plugin.split_once(':')) {
        Some((slot, param)) => matches!(Slot::parse(slot), Some(Slot::Insert(_))) && param_id(param),
        None => param_id(text),
    }
}

/// Why a ref names nothing there now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// It names an earlier life: its id is on another object now, or the object was a Kumi change's that an undo
    /// crossed.
    Stale,
    /// Its object isn't in the session now.
    Gone,
    /// Its id was never seen in this session (a ref from another session, or from before the bridge started).
    Unknown,
}

/// One id's life now.
#[derive(Debug, Clone, Copy)]
struct Life {
    number: u32,
    fingerprint: u64,
    present: bool,
    retired: bool,
}

/// The lives of the ids the bridge has seen in one session. A new session starts a new table.
#[derive(Debug, Default)]
pub struct Lives {
    seen: HashMap<(Kind, u64), Life>,
}

impl Lives {
    /// What a look saw of one kind: each id there now, with its fingerprint. An id that isn't there has gone (for now).
    /// One there already keeps its life, however it changed. One back with the fingerprint it went with keeps its life
    /// too (an undo put it back). One back with another, or retired, is a new life.
    pub fn saw(&mut self, kind: Kind, now: impl IntoIterator<Item = (u64, u64)>) {
        let now: HashMap<u64, u64> = now.into_iter().collect();
        for (&(seen_kind, id), life) in &mut self.seen {
            if seen_kind == kind && !now.contains_key(&id) {
                life.present = false;
            }
        }
        for (id, fingerprint) in now {
            let fresh = Life { number: 1, fingerprint, present: true, retired: false };
            let life = self.seen.entry((kind, id)).or_insert(fresh);
            if life.retired || (!life.present && life.fingerprint != fingerprint) {
                *life = Life { number: life.number + 1, ..fresh };
            } else {
                life.present = true;
                life.fingerprint = fingerprint;
            }
        }
    }

    /// What `session.inspect {"detail":"full"}` shows of every kind.
    pub fn saw_session(&mut self, inspect: &Value) {
        for (kind, now) in fingerprints(inspect) {
            self.saw(kind, now);
        }
    }

    /// Ends the life of a Kumi change's object that an undo crossed: refs to it are stale from now on, and its id, should it
    /// come back, is a new life.
    pub fn retire(&mut self, kind: Kind, id: u64) {
        if let Some(life) = self.seen.get_mut(&(kind, id)) {
            life.retired = true;
        }
    }

    /// The id's ref now: its life, while it's there.
    pub fn now(&self, kind: Kind, id: u64) -> Option<Id> {
        self.seen.get(&(kind, id)).filter(|life| life.present && !life.retired).map(|life| Id { kind, id, life: life.number })
    }

    /// The object a ref's id names, if it's the one there now.
    pub fn resolve(&self, id: &Id) -> Result<u64, Missing> {
        let life = self.seen.get(&(id.kind, id.id)).ok_or(Missing::Unknown)?;
        if life.number != id.life || life.retired {
            return Err(if id.life > life.number { Missing::Unknown } else { Missing::Stale });
        }
        if life.present {
            Ok(id.id)
        } else {
            Err(Missing::Gone)
        }
    }
}

/// What tells objects apart on the same id, of each kind, from `session.inspect {"detail":"full"}`: a track's kind,
/// format, name, clips and inserts; a clip's track, start, length and source (or note count); a bus's name and format;
/// a group's name and members; a marker's name and start; a source's name, channels and frames.
pub fn fingerprints(inspect: &Value) -> Vec<(Kind, Vec<(u64, u64)>)> {
    let list = |key: &str| inspect.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let id = |item: &Value| item.get("id").and_then(Value::as_u64);
    let of = |item: &Value, keys: &[&str]| hash(keys.iter().map(|key| item.get(*key).cloned().unwrap_or(Value::Null)).collect::<Vec<_>>());
    let mut tracks = vec![];
    let mut clips = vec![];
    for track in list("tracks") {
        let Some(track_id) = id(track) else { continue };
        let track_clips = track.get("clips").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let inserts = track.get("inserts").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let clip_shape: Vec<Value> =
            track_clips.iter().map(|clip| Value::Array(vec![clip["id"].clone(), clip["start"].clone(), clip["length"].clone()])).collect();
        let insert_shape: Vec<Value> =
            inserts.iter().map(|insert| Value::Array(vec![insert["slot"].clone(), insert["plugin"].clone()])).collect();
        tracks.push((track_id, hash((track["kind"].clone(), track["format"].clone(), track["name"].clone(), clip_shape, insert_shape))));
        for clip in track_clips {
            let Some(clip_id) = id(clip) else { continue };
            let made_of =
                clip.get("source").filter(|source| !source.is_null()).or_else(|| clip.get("notes")).cloned().unwrap_or(Value::Null);
            clips.push((clip_id, hash((track_id, clip["start"].clone(), clip["length"].clone(), made_of))));
        }
    }
    let each = |key: &str, keys: &[&str]| list(key).iter().filter_map(|item| Some((id(item)?, of(item, keys)))).collect::<Vec<_>>();
    vec![
        (Kind::Track, tracks),
        (Kind::Clip, clips),
        (Kind::Bus, each("busses", &["name", "format"])),
        (Kind::Group, each("groups", &["name", "members"])),
        (Kind::Marker, each("markers", &["name", "start"])),
        (Kind::Source, each("sources", &["name", "channels", "frames"])),
    ]
}

/// A fingerprint: a hash of JSON values, for this run of the bridge only.
fn hash(value: impl serde::Serialize) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&value).unwrap_or_default().hash(&mut hasher);
    hasher.finish()
}

/// Why a note ref finds no single note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteMissing {
    /// No note of the clip's has that start, pitch and channel.
    Gone,
    /// Several have (SoundCraft keeps both of two such notes): a ref can't tell which.
    Tie(usize),
}

/// The index of the note a ref names among its clip's notes (`midi.notes`, which SoundCraft addresses by index): the one
/// note with that start, pitch and channel.
pub fn note_index(notes: &[Value], start: i64, pitch: u8, channel: u8) -> Result<usize, NoteMissing> {
    let field = |note: &Value, key: &str| note.get(key).and_then(Value::as_i64);
    let mut found = notes.iter().enumerate().filter(|(_, note)| {
        field(note, "start") == Some(start) && field(note, "pitch") == Some(pitch.into()) && field(note, "channel") == Some(channel.into())
    });
    match (found.next(), found.count()) {
        (Some((index, _)), 0) => Ok(index),
        (Some(_), more) => Err(NoteMissing::Tie(more + 1)),
        (None, _) => Err(NoteMissing::Gone),
    }
}
