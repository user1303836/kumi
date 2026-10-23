//! Refs and the lookup REAPER lacks. A ref is `r:<kind>:<id>` with the object's GUID, so it holds across reloads; REAPER
//! has no object lookup by GUID (only for takes), so the registry keeps GUID to handle maps, checks each hit with
//! ValidatePtr2 and the object's own GUID, and rebuilds a map when a hit goes stale or a GUID isn't found.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::reaper::{Fx, FxOwner, Item, Project, Reaper, Take, Track, CONTAINER_FX, INPUT_FX};

pub const PREFIX: &str = "r";
pub const MASTER: &str = "master";

/// A track: the master, or by GUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackId {
    Master,
    Guid(String),
}

/// Which envelope of a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeId {
    /// The envelope's own GUID: what every envelope is named by.
    Guid(String),
    /// A track's built-in one by its chunk name (`VOLENV2`): an alias, read back as the GUID's ref.
    Chunk(String),
    /// An FX parameter's: an alias too.
    Param { fx: String, index: i32 },
}

/// What is at the other end of a send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendEnd {
    Track(TrackId),
    /// A hardware output, by the output it goes to: REAPER's `I_DSTCHAN` (the first channel, plus 1024 for a mono one).
    /// Never by its place among the track's outputs, which removing another output moves.
    Hardware(i32),
}

impl SendEnd {
    pub fn text(&self) -> String {
        match self {
            SendEnd::Track(id) => id.text().to_string(),
            SendEnd::Hardware(n) if n & 1024 != 0 => format!("hw{}m", n & 1023),
            SendEnd::Hardware(n) => format!("hw{}", n & 1023),
        }
    }

    fn parse_hardware(text: &str) -> Option<SendEnd> {
        let digits = text.strip_prefix("hw")?;
        let (digits, mono) = digits.strip_suffix('m').map_or((digits, 0), |digits| (digits, 1024));
        let first = digits.parse::<i32>().ok().filter(|first| (0..1024).contains(first) && digits.bytes().all(|b| b.is_ascii_digit()))?;
        Some(SendEnd::Hardware(first | mono))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ref {
    Track(TrackId),
    Item(String),
    Take(String),
    Fx(String),
    Param {
        fx: String,
        index: i32,
    },
    /// A send or receive of `track`, named by the track at its other end, or a hardware output by the output it goes to:
    /// never by position, which a reorder moves. Two or more to the same place are told apart by which of them this is
    /// (`repeat`, from 1) among how many there are (`of`): the ref stops reading anything once that count changes,
    /// since REAPER gives a send no identity of its own to follow. A send of two or more to one track also carries the
    /// state of those sends as the ref was read (`seen`): a change through it is refused once the producer has changed
    /// them, since the place may name another send by then.
    Send {
        track: TrackId,
        category: i32,
        other: SendEnd,
        repeat: u32,
        of: u32,
        seen: Option<String>,
    },
    Marker(String),
    Region(String),
    Envelope {
        track: TrackId,
        which: EnvelopeId,
    },
    // m4-r-fxenv: a take's envelopes and automation items.
    /// A take's envelope (its volume, pan, mute or pitch, or a take FX parameter's), by the take's GUID and its own.
    TakeEnvelope {
        take: String,
        guid: String,
    },
    /// An automation item of a track's envelope. REAPER gives one no id of its own (its pool's name and data are shared by
    /// every copy), so it is named by its pool and where it starts; two of one pool at one place name neither.
    AutoItem {
        track: TrackId,
        envelope: String,
        pool: i64,
        /// Seconds, as REAPER's own number prints: exactly what `D_POSITION` reads.
        position: String,
    },
}

impl TrackId {
    fn parse(text: &str) -> Result<TrackId, String> {
        match text {
            MASTER => Ok(TrackId::Master),
            guid if looks_like_guid(guid) => Ok(TrackId::Guid(guid.to_string())),
            other => Err(format!("{other:?} isn't a track id: expected a GUID in braces, or master")),
        }
    }

    pub fn text(&self) -> &str {
        match self {
            TrackId::Master => MASTER,
            TrackId::Guid(guid) => guid,
        }
    }
}

/// REAPER writes a GUID as `{8-4-4-4-12 hex}`; the bridge only checks the braces, which is all a ref's grammar needs.
fn looks_like_guid(text: &str) -> bool {
    text.len() >= 3 && text.len() <= 64 && text.starts_with('{') && text.ends_with('}') && !text.contains(':')
}

fn number(text: &str, what: &str) -> Result<i32, String> {
    text.parse::<i32>().map_err(|_| format!("{text:?} isn't {what}"))
}

fn guid_of(text: &str, what: &str) -> Result<String, String> {
    if looks_like_guid(text) {
        Ok(text.to_string())
    } else {
        Err(format!("{text:?} isn't {what}: expected a GUID in braces"))
    }
}

impl Ref {
    pub fn parse(text: &str) -> Result<Ref, String> {
        let mut parts = text.split(':');
        if parts.next() != Some(PREFIX) {
            return Err(format!("{text:?} isn't a REAPER ref: they look like r:track:{{GUID}}"));
        }
        let kind = parts.next().ok_or_else(|| format!("{text:?} names no kind"))?;
        let rest: Vec<&str> = parts.collect();
        let bad = || format!("{text:?} isn't a well-formed r:{kind} ref");
        Ok(match (kind, rest.as_slice()) {
            ("track", [id]) => Ref::Track(TrackId::parse(id)?),
            ("item", [guid]) => Ref::Item(guid_of(guid, "an item id")?),
            ("take", [guid]) => Ref::Take(guid_of(guid, "a take id")?),
            ("fx", [guid]) => Ref::Fx(guid_of(guid, "an FX id")?),
            ("param", [guid, index]) => Ref::Param { fx: guid_of(guid, "an FX id")?, index: number(index, "a parameter index")? },
            ("send", [track, category, other]) | ("send", [track, category, other, _]) => {
                let category = number(category, "a send category")?;
                let (repeat, of, seen) = match rest.get(3) {
                    None => (1, 1, None),
                    Some(text) => {
                        let (count, seen) = match text.split_once('.') {
                            Some((count, seen)) => (count, Some(seen)),
                            None => (*text, None),
                        };
                        let (repeat, of) = count
                            .split_once("of")
                            .and_then(|(k, n)| Some((k.parse::<u32>().ok()?, n.parse::<u32>().ok()?)))
                            .filter(|&(k, n)| n >= 2 && (1..=n).contains(&k))
                            .filter(|_| seen.is_none_or(|seen| !seen.is_empty() && seen.chars().all(|c| c.is_ascii_hexdigit())))
                            .ok_or_else(|| {
                                format!("{text:?} isn't a repeat such as 2of3: which of the sends to the same place, of how many")
                            })?;
                        (repeat, of, seen.map(str::to_string))
                    }
                };
                let other = match other.strip_prefix("hw") {
                    Some(_) if category == 1 => SendEnd::parse_hardware(other).ok_or_else(bad)?,
                    Some(_) => return Err(bad()),
                    None if matches!(category, -1 | 0) => SendEnd::Track(TrackId::parse(other)?),
                    None => return Err(bad()),
                };
                Ref::Send { track: TrackId::parse(track)?, category, other, repeat, of, seen }
            }
            ("marker", [guid]) => Ref::Marker(guid_of(guid, "a marker id")?),
            ("region", [guid]) => Ref::Region(guid_of(guid, "a region id")?),
            // m4-r-fxenv
            ("env", ["take", take, guid]) => {
                Ref::TakeEnvelope { take: guid_of(take, "a take id")?, guid: guid_of(guid, "an envelope id")? }
            }
            ("env", ["take", ..]) => return Err(bad()),
            ("aitem", [track, envelope, pool, position]) => {
                let pool = pool.parse::<i64>().ok().filter(|pool| *pool >= 0).ok_or_else(bad)?;
                let at = position.parse::<f64>().ok().filter(|at| at.is_finite() && *at >= 0.0).ok_or_else(bad)?;
                Ref::AutoItem {
                    track: TrackId::parse(track)?,
                    envelope: guid_of(envelope, "an envelope id")?,
                    pool,
                    position: position_text(at),
                }
            }
            ("env", [track, name]) => {
                let which = if looks_like_guid(name) {
                    EnvelopeId::Guid(name.to_string())
                } else if !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
                    EnvelopeId::Chunk(name.to_string())
                } else {
                    return Err(bad());
                };
                Ref::Envelope { track: TrackId::parse(track)?, which }
            }
            ("env", [track, fx, index]) => Ref::Envelope {
                track: TrackId::parse(track)?,
                which: EnvelopeId::Param { fx: guid_of(fx, "an FX id")?, index: number(index, "a parameter index")? },
            },
            ("track" | "item" | "take" | "fx" | "param" | "send" | "marker" | "region" | "env" | "aitem", _) => return Err(bad()),
            _ => return Err(format!("{text:?}: REAPER has no ref kind {kind:?}")),
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Ref::Track(_) => "track",
            Ref::Item(_) => "item",
            Ref::Take(_) => "take",
            Ref::Fx(_) => "fx",
            Ref::Param { .. } => "param",
            Ref::Send { .. } => "send",
            Ref::Marker(_) => "marker",
            Ref::Region(_) => "region",
            Ref::Envelope { .. } | Ref::TakeEnvelope { .. } => "env",
            Ref::AutoItem { .. } => "aitem",
        }
    }

    pub fn text(&self) -> String {
        match self {
            Ref::Track(id) => track_ref(id),
            Ref::Item(guid) => item_ref(guid),
            Ref::Take(guid) => take_ref(guid),
            Ref::Fx(guid) => fx_ref(guid),
            Ref::Param { fx, index } => param_ref(fx, *index),
            Ref::Send { track, category, other, repeat, of, seen } => send_ref(track, *category, other, *repeat, *of, seen.as_deref()),
            Ref::Marker(guid) => marker_ref(guid),
            Ref::Region(guid) => region_ref(guid),
            Ref::Envelope { track, which } => envelope_ref(track, which),
            Ref::TakeEnvelope { take, guid } => take_envelope_ref(take, guid),
            Ref::AutoItem { track, envelope, pool, position } => format!("{PREFIX}:aitem:{}:{envelope}:{pool}:{position}", track.text()),
        }
    }
}

pub fn track_ref(id: &TrackId) -> String {
    format!("{PREFIX}:track:{}", id.text())
}
pub fn item_ref(guid: &str) -> String {
    format!("{PREFIX}:item:{guid}")
}
pub fn take_ref(guid: &str) -> String {
    format!("{PREFIX}:take:{guid}")
}
pub fn fx_ref(guid: &str) -> String {
    format!("{PREFIX}:fx:{guid}")
}
pub fn param_ref(fx_guid: &str, index: i32) -> String {
    format!("{PREFIX}:param:{fx_guid}:{index}")
}
pub fn send_ref(track: &TrackId, category: i32, other: &SendEnd, repeat: u32, of: u32, seen: Option<&str>) -> String {
    let base = format!("{PREFIX}:send:{}:{category}:{}", track.text(), other.text());
    match seen {
        _ if of < 2 => base,
        Some(seen) => format!("{base}:{repeat}of{of}.{seen}"),
        None => format!("{base}:{repeat}of{of}"),
    }
}
pub fn marker_ref(guid: &str) -> String {
    format!("{PREFIX}:marker:{guid}")
}
pub fn region_ref(guid: &str) -> String {
    format!("{PREFIX}:region:{guid}")
}
pub fn envelope_ref(track: &TrackId, which: &EnvelopeId) -> String {
    match which {
        EnvelopeId::Guid(guid) => format!("{PREFIX}:env:{}:{guid}", track.text()),
        EnvelopeId::Chunk(name) => format!("{PREFIX}:env:{}:{name}", track.text()),
        EnvelopeId::Param { fx, index } => format!("{PREFIX}:env:{}:{fx}:{index}", track.text()),
    }
}

// m4-r-fxenv
pub fn take_envelope_ref(take: &str, guid: &str) -> String {
    format!("{PREFIX}:env:take:{take}:{guid}")
}
pub fn auto_item_ref(track: &TrackId, envelope: &str, pool: i64, position: f64) -> String {
    format!("{PREFIX}:aitem:{}:{envelope}:{pool}:{}", track.text(), position_text(position))
}
/// A position as an automation item's ref holds it: the shortest text that reads back as the same number.
pub fn position_text(position: f64) -> String {
    // To a nanosecond: REAPER's undo and redo bring a position back from text with its last bits changed, and the ref
    // should still read the same.
    let text = format!("{:.9}", if position == 0.0 { 0.0 } else { position });
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.to_string()
    }
}

/// How long a map that was just rebuilt without finding a GUID is believed: a ref that isn't there stays missing until
/// the project's state changes or this passes, so a caller polling a dead ref doesn't walk the whole project each time.
const MISS_MEMORY: Duration = Duration::from_millis(500);

/// GUID to handle, per project tab.
#[derive(Default)]
pub struct Registry {
    project: usize,
    tracks: HashMap<String, Track>,
    items: HashMap<String, Item>,
    fx: HashMap<String, Fx>,
    /// GUIDs that two tracks, items or FX of this project have: REAPER keeps a GUID through a copy made as text (a script
    /// setting one track's chunk on another), so a ref to one can't say which it means. Found when a map is rebuilt, and
    /// forgotten with it.
    shared_tracks: HashSet<String>,
    shared_items: HashSet<String>,
    shared_fx: HashSet<String>,
    /// When the item map and the FX map were last rebuilt, and the project's state change count then.
    items_built: Option<(i32, Instant)>,
    fx_built: Option<(i32, Instant)>,
}

impl Registry {
    /// Forgets everything: a tab switch, or a new project in the tab.
    pub fn clear(&mut self) {
        *self = Registry::default();
    }

    fn follow(&mut self, proj: Project) {
        if self.project != proj.addr() {
            self.clear();
            self.project = proj.addr();
        }
    }

    fn recently_missed(built: Option<(i32, Instant)>, count: i32) -> bool {
        built.is_some_and(|(at, when)| at == count && when.elapsed() < MISS_MEMORY)
    }

    /// Whether two objects of a kind were found with this GUID: it names neither.
    pub fn is_shared(&self, guid: &str) -> bool {
        self.shared_tracks.contains(guid) || self.shared_items.contains(guid) || self.shared_fx.contains(guid)
    }

    /// The track for an id; the master by name, any other by GUID.
    pub fn track(&mut self, reaper: &Reaper, proj: Project, id: &TrackId) -> Option<Track> {
        let guid = match id {
            TrackId::Master => return reaper.master_track(proj),
            TrackId::Guid(guid) => guid,
        };
        self.follow(proj);
        // A GUID that was found on two tracks is looked for again each time: the copy may be gone.
        if let Some(&track) = self.tracks.get(guid).filter(|_| !self.shared_tracks.contains(guid)) {
            if reaper.track_valid(proj, track) && reaper.track_guid(track) == *guid {
                return Some(track);
            }
        }
        // A project's tracks are few: walk them all on a miss rather than trust a count that may not have moved.
        self.tracks.clear();
        self.shared_tracks.clear();
        for index in 0..reaper.track_count(proj) {
            if let Some(track) = reaper.track_at(proj, index) {
                let own = reaper.track_guid(track);
                if self.tracks.insert(own.clone(), track).is_some() {
                    self.shared_tracks.insert(own);
                }
            }
        }
        if self.shared_tracks.contains(guid) {
            return None;
        }
        self.tracks.get(guid).copied()
    }

    /// A track's id: the master by name, any other by GUID. Remembers the track.
    pub fn track_id(&mut self, reaper: &Reaper, proj: Project, track: Track) -> TrackId {
        if reaper.track_value(track, "IP_TRACKNUMBER") < 0.0 {
            return TrackId::Master;
        }
        self.follow(proj);
        let guid = reaper.track_guid(track);
        self.tracks.insert(guid.clone(), track);
        TrackId::Guid(guid)
    }

    /// The item for a GUID.
    pub fn item(&mut self, reaper: &Reaper, proj: Project, guid: &str) -> Option<Item> {
        self.follow(proj);
        if let Some(&item) = self.items.get(guid).filter(|_| !self.shared_items.contains(guid)) {
            if reaper.item_valid(proj, item) && reaper.item_guid(item) == guid {
                return Some(item);
            }
        }
        let count = reaper.state_change_count(proj).unwrap_or(0);
        // A GUID the map holds with a pointer that no longer names it means the map is out of date, not a recent miss.
        let stale = self.items.contains_key(guid);
        if !stale && Self::recently_missed(self.items_built, count) && !self.shared_items.contains(guid) {
            return None;
        }
        self.items.clear();
        self.shared_items.clear();
        for track in (0..reaper.track_count(proj)).filter_map(|i| reaper.track_at(proj, i)).chain(reaper.master_track(proj)) {
            for index in 0..reaper.track_item_count(track) {
                if let Some(item) = reaper.track_item(track, index) {
                    let own = reaper.item_guid(item);
                    if self.items.insert(own.clone(), item).is_some() {
                        self.shared_items.insert(own);
                    }
                }
            }
        }
        self.items_built = Some((count, Instant::now()));
        if self.shared_items.contains(guid) {
            return None;
        }
        self.items.get(guid).copied()
    }

    /// Remembers an item REAPER just handed out, so a ref to it resolves without a walk.
    pub fn note_item(&mut self, proj: Project, guid: &str, item: Item) {
        self.follow(proj);
        self.items.insert(guid.to_string(), item);
    }

    /// The take for a GUID: REAPER looks these up itself.
    pub fn take(&mut self, reaper: &Reaper, proj: Project, guid: &str) -> Option<Take> {
        reaper.take_by_guid(proj, guid).filter(|&take| reaper.take_valid(proj, take))
    }

    /// The FX for a GUID, wherever it is: a track's chain, its input chain, a container, or a take's chain.
    pub fn fx(&mut self, reaper: &Reaper, proj: Project, guid: &str) -> Option<Fx> {
        self.follow(proj);
        if let Some(&fx) = self.fx.get(guid).filter(|_| !self.shared_fx.contains(guid)) {
            if self.fx_matches(reaper, proj, fx, guid) {
                return Some(fx);
            }
            // The chain was reordered: the same owner's chain is the first place it can have gone, while REAPER still has
            // that owner. One REAPER replaced (its undo puts a track back as a new object) is never read: its chain is
            // freed memory (a crash in the lab, 2026-10-19: REAPER's undo and redo of a change whose FX the watch followed).
            if self.owner_valid(reaper, proj, fx.owner) {
                self.index_chain(reaper, fx.owner);
                if let Some(&fx) = self.fx.get(guid) {
                    if self.fx_matches(reaper, proj, fx, guid) {
                        return Some(fx);
                    }
                }
            }
        }
        let count = reaper.state_change_count(proj).unwrap_or(0);
        if Self::recently_missed(self.fx_built, count) && !self.shared_fx.contains(guid) {
            return None;
        }
        self.fx.clear();
        self.shared_fx.clear();
        // Tracks first: nearly every FX is on one. Takes only when the track chains didn't have it.
        let tracks: Vec<Track> =
            (0..reaper.track_count(proj)).filter_map(|i| reaper.track_at(proj, i)).chain(reaper.master_track(proj)).collect();
        for &track in &tracks {
            self.index_chain(reaper, FxOwner::Track(track));
        }
        let every_chain = !self.fx.contains_key(guid);
        if every_chain {
            for &track in &tracks {
                for index in 0..reaper.track_item_count(track) {
                    let Some(item) = reaper.track_item(track, index) else { continue };
                    for n in 0..reaper.take_count(item) {
                        if let Some(take) = reaper.take_at(item, n) {
                            self.index_chain(reaper, FxOwner::Take(take));
                        }
                    }
                }
            }
        }
        // M5 listening (m5-reaper): a miss is remembered only after every chain was looked at; when a track had the FX the
        // takes weren't, and a take's FX asked for next (same state count) would be missed without a look.
        self.fx_built = every_chain.then(|| (count, Instant::now()));
        if self.shared_fx.contains(guid) {
            return None;
        }
        self.fx.get(guid).copied()
    }

    fn owner_valid(&self, reaper: &Reaper, proj: Project, owner: FxOwner) -> bool {
        match owner {
            FxOwner::Track(track) => reaper.track_valid(proj, track),
            FxOwner::Take(take) => reaper.take_valid(proj, take),
        }
    }

    fn fx_matches(&self, reaper: &Reaper, proj: Project, fx: Fx, guid: &str) -> bool {
        self.owner_valid(reaper, proj, fx.owner) && reaper.fx_guid(fx) == guid
    }

    /// Remembers every FX in an owner's chain, containers' children and a track's input chain included.
    pub fn index_chain(&mut self, reaper: &Reaper, owner: FxOwner) {
        for index in 0..reaper.fx_count(owner) {
            self.index_fx(reaper, Fx { owner, index });
        }
        if let FxOwner::Track(track) = owner {
            for index in 0..reaper.input_fx_count(track) {
                self.index_fx(reaper, Fx { owner, index: INPUT_FX | index });
            }
        }
    }

    fn index_fx(&mut self, reaper: &Reaper, fx: Fx) {
        let guid = reaper.fx_guid(fx);
        if guid.is_empty() {
            return;
        }
        // The same FX met again while indexing is a reorder re-reading its chain; another owner's is a shared GUID.
        if let Some(before) = self.fx.insert(guid.clone(), fx) {
            if before.owner != fx.owner {
                self.shared_fx.insert(guid);
            }
        }
        for child in reaper.fx_children(fx) {
            debug_assert!(child.index & CONTAINER_FX != 0);
            self.index_fx(reaper, child);
        }
    }

    /// Remembers an FX REAPER just handed out.
    pub fn note_fx(&mut self, proj: Project, guid: &str, fx: Fx) {
        self.follow(proj);
        self.fx.insert(guid.to_string(), fx);
    }
}

/// The epoch: changes when refs stop meaning what they meant (a tab switch, a new project in the tab). Distinct for each
/// run of REAPER, so a cursor from before a restart is refused.
pub struct Epoch {
    session: u64,
    serial: u64,
}

impl Epoch {
    pub fn new() -> Epoch {
        let session = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        Epoch { session, serial: 1 }
    }

    pub fn text(&self) -> String {
        format!("{:x}-{}", self.session, self.serial)
    }

    pub fn bump(&mut self) {
        self.serial += 1;
    }
}

impl Default for Epoch {
    fn default() -> Self {
        Epoch::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_round_trip_through_their_text() {
        let guid = "{1A2B3C4D-0000-1111-2222-333344445555}";
        let fx = "{AAAAAAAA-0000-1111-2222-333344445555}";
        let track = TrackId::Guid(guid.into());
        for reference in [
            Ref::Track(TrackId::Master),
            Ref::Track(track.clone()),
            Ref::Item(guid.into()),
            Ref::Take(guid.into()),
            Ref::Fx(fx.into()),
            Ref::Param { fx: fx.into(), index: 7 },
            Ref::Send { track: track.clone(), category: -1, other: SendEnd::Track(TrackId::Guid(fx.into())), repeat: 1, of: 1, seen: None },
            Ref::Send {
                track: track.clone(),
                category: 0,
                other: SendEnd::Track(TrackId::Master),
                repeat: 3,
                of: 3,
                seen: Some("0a1b2c3d".into()),
            },
            Ref::Send { track: track.clone(), category: 0, other: SendEnd::Track(TrackId::Master), repeat: 1, of: 2, seen: None },
            Ref::Send { track: track.clone(), category: 1, other: SendEnd::Hardware(2), repeat: 1, of: 1, seen: None },
            Ref::Send { track: track.clone(), category: 1, other: SendEnd::Hardware(2 | 1024), repeat: 2, of: 2, seen: None },
            Ref::Marker(guid.into()),
            Ref::Region(fx.into()),
            Ref::Envelope { track: track.clone(), which: EnvelopeId::Guid(guid.into()) },
            Ref::Envelope { track: track.clone(), which: EnvelopeId::Chunk("VOLENV2".into()) },
            Ref::Envelope { track: track.clone(), which: EnvelopeId::Param { fx: fx.into(), index: 4 } },
            Ref::TakeEnvelope { take: guid.into(), guid: fx.into() },
            Ref::AutoItem { track: track.clone(), envelope: fx.into(), pool: 3, position: "12.5".into() },
            Ref::AutoItem { track, envelope: fx.into(), pool: 0, position: "0".into() },
        ] {
            let text = reference.text();
            assert_eq!(Ref::parse(&text).as_ref(), Ok(&reference), "{text}");
            // The second part of the ref is its kind, as Kumi's book reads it.
            assert_eq!(text.split(':').nth(1), Some(reference.kind()));
        }
    }

    #[test]
    fn malformed_refs_say_what_is_wrong() {
        for (text, needle) in [
            ("track:{A}", "isn't a REAPER ref"),
            ("r:", "kind"),
            ("r:track", "well-formed"),
            ("r:track:nope", "isn't a track id"),
            ("r:item:42", "GUID"),
            ("r:param:{A}:x", "parameter index"),
            ("r:send:{A}:0", "well-formed"),
            // A repeat says which one of how many: "2" alone, or one that is no more than the count, is not one.
            ("r:send:{A}:0:{B}:1", "isn't a repeat"),
            ("r:send:{A}:0:{B}:2", "isn't a repeat"),
            ("r:send:{A}:0:{B}:1of1", "isn't a repeat"),
            ("r:send:{A}:0:{B}:3of2", "isn't a repeat"),
            ("r:send:{A}:0:{B}:0of2", "isn't a repeat"),
            ("r:send:{A}:0:hw1", "well-formed"),
            ("r:send:{A}:1:{B}", "well-formed"),
            ("r:send:{A}:1:hw1:2", "isn't a repeat"),
            ("r:send:{A}:1:hw", "well-formed"),
            ("r:send:{A}:1:hw1x", "well-formed"),
            ("r:send:{A}:1:hw2000", "well-formed"),
            ("r:marker:3", "GUID"),
            ("r:pattern:3", "no ref kind"),
            ("r:env:{A}:", "well-formed"),
            ("r:env:{A}:VOL ENV", "well-formed"),
            // A position in the envelope list names a different envelope once the list changes: no ref has one.
            ("r:env:{A}:i3", "well-formed"),
            ("r:env:take:{A}", "well-formed"),
            ("r:env:take:{A}:x", "envelope id"),
            ("r:aitem:{A}:{B}:1", "well-formed"),
            ("r:aitem:{A}:{B}:-1:2", "well-formed"),
            ("r:aitem:{A}:{B}:1:x", "well-formed"),
            ("r:aitem:{A}:{B}:1:-2", "well-formed"),
            ("r:aitem:{A}:B:1:2", "envelope id"),
        ] {
            let error = Ref::parse(text).unwrap_err();
            assert!(error.contains(needle), "{text}: {error}");
        }
    }

    #[test]
    fn the_epoch_changes_on_a_bump_and_differs_between_runs() {
        let mut epoch = Epoch::new();
        let first = epoch.text();
        epoch.bump();
        assert_ne!(epoch.text(), first);
        assert!(epoch.text().starts_with(first.split('-').next().unwrap()));
        let mut other = Epoch { session: epoch.session + 1, serial: 1 };
        other.bump();
        assert_ne!(other.text(), epoch.text());
    }
}
