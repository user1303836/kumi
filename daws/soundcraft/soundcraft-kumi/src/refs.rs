//! Refs: the names the bridge gives SoundCraft's objects, and the lives behind them.
//!
//! A ref is `s:<kind>:<parts>`, with SoundCraft's own ids where it has them, and positional parts spelled out, to be
//! resolved again at each write: `s:track:12`, `s:clip:40`, `s:insert:12:a`, `s:param:12:a:high_shelf_gain`,
//! `s:note:40:480.60.0`. Each has one spelling.
//!
//! Undo puts SoundCraft's next id back, so an id can come back on another object (tracks "First" and "Second" both got
//! id 1). The bridge keeps the lives of the ids it has seen, in a table of its own for each session and each run of the
//! bridge, under an epoch that requests carry back. At each look an object keeps its life while it's the same object:
//! while it's there, by who it is (a track's kind, format, name and instrument); back after a look without it, by all it
//! holds (an undo put it back as it was). Otherwise its id starts a new life, and its ref takes the life's number,
//! `s:track:1.2`: a ref from an earlier life is stale. A Kumi change's object that an undo crossed is retired at once.
//!
//! What no look can tell: between two looks, an undo and then a step that makes an object like the undone one (a track
//! of the same kind, format, name and instrument) on the same id. Refs to the undone one find the new one. For an audio
//! clip, alike is wide: while it's there, it's known by its name and what it plays, which every copy of it shares, so a
//! paste undone and a paste elsewhere between two looks is the same clip to the bridge, at its new place. SoundCraft
//! would have to count its undos where a look sees them, or never give an id out again, for the bridge to tell.
use serde_json::{json, Value};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
            Some((id, life)) => (id, number::<u32>(life).filter(|life| *life > 1)?),
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
    /// A send, by its slot: 0 to 9, SoundCraft's sends A to J.
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
    /// `send_a_level`, a plug-in's parameter (`plugin:a:high_shelf_gain`), or the trim lane, `plugin:255:trim`.
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
    /// A note, by its start (ticks from its clip's start), pitch and channel: by where it is. An edit that moves notes (a
    /// transpose) can leave another note there, so a change to a note checks the clip's notes are those it read.
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
            Some(Ref::Send { track: track(id)?, slot: number(slot).filter(|slot: &u32| *slot < 10)? })
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

/// A plain number, in its one spelling: digits only, no sign, and no leading zero but in `0`.
fn number<T: std::str::FromStr>(text: &str) -> Option<T> {
    let plain = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) && (text == "0" || !text.starts_with('0'));
    plain.then(|| text.parse().ok()).flatten()
}

/// A number that may be negative (and never `-0`).
fn signed(text: &str) -> Option<i64> {
    match text.strip_prefix('-') {
        Some(digits) => number::<i64>(digits).filter(|magnitude| *magnitude != 0).map(|magnitude| -magnitude),
        None => number(text),
    }
}

/// A parameter's id: a built-in's name or a hosted plug-in's number.
fn param_id(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// What a lane moves, as SoundCraft's own automation ids spell it (its UI's, which its automation commands take):
/// `volume`, `pan`, `pan2`, `mute`, `send_<a-j>_level`, `_pan` or `_mute`, `plugin:<a-j>:<parameter>`, and the trim
/// lane, `plugin:255:trim`. (The commands take other spellings too, `vol` or `send_A_level`: those aren't refs.)
pub(crate) fn lane(text: &str) -> bool {
    let slot = |letter: &str| matches!(letter.as_bytes(), [b'a'..=b'j']);
    if matches!(text, "volume" | "pan" | "pan2" | "mute" | "plugin:255:trim") {
        return true;
    }
    if let Some(send) = text.strip_prefix("send_") {
        return send.split_once('_').is_some_and(|(letter, what)| slot(letter) && matches!(what, "level" | "pan" | "mute"));
    }
    text.strip_prefix("plugin:").and_then(|plugin| plugin.split_once(':')).is_some_and(|(letter, param)| slot(letter) && param_id(param))
}

/// Why a ref names nothing there now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// It names an earlier life: its id is on another object now, or the object was a Kumi change's that an undo
    /// crossed.
    Stale,
    /// Its object isn't in the session now.
    Gone,
    /// It names a life this table hasn't seen: an id it never saw, or a later life than any it gave. (A ref from another
    /// table can name a life this one has too: so a request carries the epoch its refs came from, for the bridge to
    /// check against this table's.)
    Unknown,
}

/// What tells one object from another on the same id: who it is, compared at each look while it's there, and all it
/// holds, compared when it's back after a look without it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Print {
    pub who: u64,
    pub all: u64,
}

/// One id's life now.
#[derive(Debug, Clone, Copy)]
struct Life {
    number: u32,
    print: Print,
    present: bool,
    retired: bool,
}

/// The lives of the ids the bridge has seen in one session, in one run, under an epoch no other table has.
#[derive(Debug)]
pub struct Lives {
    epoch: String,
    seen: HashMap<(Kind, u64), Life>,
}

impl Default for Lives {
    fn default() -> Self {
        Self::new()
    }
}

impl Lives {
    /// A new table, with a new epoch: the time, and a count of the run's tables.
    pub fn new() -> Self {
        static TABLES: AtomicU64 = AtomicU64::new(0);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64);
        Self { epoch: format!("{now:x}-{}", TABLES.fetch_add(1, Ordering::Relaxed) + 1), seen: HashMap::new() }
    }

    /// The table's epoch: its refs mean something only with it.
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// What a look saw of one kind: each id there now, with its print. An id that isn't there has gone (for now). One
    /// still there keeps its life while it's who it was. One back keeps its life if it holds all it held when it went
    /// (an undo put it back). Any other, or one retired, is a new life.
    pub fn saw(&mut self, kind: Kind, now: impl IntoIterator<Item = (u64, Print)>) {
        let now: HashMap<u64, Print> = now.into_iter().collect();
        for (&(seen_kind, id), life) in &mut self.seen {
            if seen_kind == kind && !now.contains_key(&id) {
                life.present = false;
            }
        }
        for (id, print) in now {
            let fresh = Life { number: 1, print, present: true, retired: false };
            let life = self.seen.entry((kind, id)).or_insert(fresh);
            let same = if life.present { life.print.who == print.who } else { life.print.all == print.all };
            if life.retired || !same {
                *life = Life { number: life.number + 1, ..fresh };
            } else {
                life.present = true;
                life.print = print;
            }
        }
    }

    /// What `session.inspect {"detail":"full"}` shows of every kind.
    pub fn saw_session(&mut self, inspect: &Value) {
        for (kind, now) in fingerprints(inspect) {
            self.saw(kind, now);
        }
    }

    /// Ends the life of a Kumi change's object that an undo crossed, seen by a look or not (a change learns its ids from
    /// its answer): refs to it are stale from now on, and its id, when a look shows it, is a new life.
    pub fn retire(&mut self, kind: Kind, id: u64) {
        let unseen = Life { number: 1, print: Print::default(), present: false, retired: true };
        self.seen.entry((kind, id)).or_insert(unseen).retired = true;
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

/// The prints of each kind's objects in `session.inspect {"detail":"full"}`, with the ids they hold read as what those
/// stand for, since SoundCraft gives ids out again together (an import's source, track and clip).
///
/// Who it is: a track's kind, format, name and instrument; a clip's name and what it plays (its source's name, channels
/// and frames, or MIDI, or video), and a MIDI clip's start and length, which its notes are placed from; a bus's name and
/// format; a group's name; a marker's name and kind; a source's name, channels and frames. All it holds: that, and a
/// track's clips and inserts; a clip's track, place and notes; a group's members; a marker's place; a source's file.
pub fn fingerprints(inspect: &Value) -> Vec<(Kind, Vec<(u64, Print)>)> {
    fn list<'a>(of: &'a Value, key: &str) -> &'a [Value] {
        of.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
    }
    let id = |item: &Value| item.get("id").and_then(Value::as_u64);
    let fields = |item: &Value, keys: &[&str]| keys.iter().map(|key| item.get(*key).cloned().unwrap_or(Value::Null)).collect::<Vec<_>>();
    let print = |who: &Vec<Value>, more: Value| Print { who: hash(who), all: hash((who, more)) };
    let sources: HashMap<u64, Vec<Value>> =
        list(inspect, "sources").iter().filter_map(|source| Some((id(source)?, fields(source, &["name", "channels", "frames"])))).collect();
    let who_tracks: HashMap<u64, Vec<Value>> = list(inspect, "tracks")
        .iter()
        .filter_map(|track| Some((id(track)?, fields(track, &["kind", "format", "name", "instrument"]))))
        .collect();
    let mut tracks = vec![];
    let mut clips = vec![];
    for track in list(inspect, "tracks") {
        let Some((track_id, track_who)) = id(track).and_then(|track_id| Some((track_id, who_tracks.get(&track_id)?))) else { continue };
        let clip_shapes: Vec<Vec<Value>> = list(track, "clips").iter().map(|clip| fields(clip, &["name", "start", "length"])).collect();
        let insert_shapes: Vec<Vec<Value>> = list(track, "inserts").iter().map(|insert| fields(insert, &["slot", "plugin"])).collect();
        tracks.push((track_id, print(track_who, json!([clip_shapes, insert_shapes]))));
        for clip in list(track, "clips") {
            let Some(clip_id) = id(clip) else { continue };
            let midi = clip.get("notes").is_some();
            let plays = match clip["source"].as_u64() {
                Some(source) => json!(sources.get(&source)),
                None if midi => json!("midi"),
                None if clip.get("video").is_some() => json!("video"),
                None => Value::Null,
            };
            let placed = fields(clip, &["start", "length"]);
            let who = vec![clip["name"].clone(), plays, if midi { json!(placed) } else { Value::Null }];
            clips.push((clip_id, print(&who, json!([track_who, placed, fields(clip, &["offset", "notes"])]))));
        }
    }
    let groups = list(inspect, "groups")
        .iter()
        .filter_map(|group| {
            let members: Vec<Option<&Vec<Value>>> =
                list(group, "members").iter().map(|member| member.as_u64().and_then(|member| who_tracks.get(&member))).collect();
            Some((id(group)?, print(&fields(group, &["name"]), json!(members))))
        })
        .collect();
    let each = |key: &str, who: &[&str], more: &[&str]| {
        list(inspect, key)
            .iter()
            .filter_map(|item| Some((id(item)?, print(&fields(item, who), json!(fields(item, more))))))
            .collect::<Vec<_>>()
    };
    vec![
        (Kind::Track, tracks),
        (Kind::Clip, clips),
        (Kind::Bus, each("busses", &["name", "format"], &[])),
        (Kind::Group, groups),
        (Kind::Marker, each("markers", &["name", "kind"], &["start", "end"])),
        (Kind::Source, each("sources", &["name", "channels", "frames"], &["path"])),
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
