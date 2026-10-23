//! Scratch copies (`scratch`): copies of a track that a tune search, a probe or a goal hears side by side, each with its
//! own knob values, rendered as stems and never played out loud.
//!
//! A copy is the source's state chunk with every GUID in it made new (REAPER keeps the GUIDs a chunk names, so a copy made
//! as text would share them), inserted after the last track at the top level, out of every folder and group, sending
//! nowhere (no master or parent send, no hardware output), unarmed, unsoloed, unmuted and unselected. A folder's copy gets
//! its tracks' sound through sends from each of them, which go when the copy goes. Copies are made and removed as bare
//! edits, with no undo point: REAPER's undo of a later point may take them away, which drop then finds.
//!
//! Each make is written to Kumi's folder (`listening/scratch.json`) before its first copy is made: copies a crash left
//! behind are removed when the bridge loads or their project opens again (found by GUID, and only while they still carry
//! Kumi's mark and name). A drop keeps the copies it removed there too: as bare edits inside the producer's undo points,
//! REAPER's undo or redo may bring them back, so they're swept whenever an undo or redo moves the project, until their
//! saved project is seen without them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::{bad, failed, gone, stale};
use crate::queue::Answer;
use crate::read::Reads;
use crate::reaper::{Fx, FxOwner, Project, ProjectInfo, Reaper, Track, INPUT_FX};
use crate::refs::{self, Ref, TrackId};

pub const MAX_COPIES: u64 = 16;
/// The most copies standing at once, over every search.
pub const MAX_STANDING: usize = 64;
/// The name every copy has: `Kumi · try <tag> <n>`.
pub const NAME: &str = "Kumi \u{b7} try";
/// The mark every copy carries (`P_EXT`, saved with the track): `<scratchId> <n>`.
pub const MARK: &str = "P_EXT:kumi_scratch";
const JOURNAL: &str = "scratch.json";
/// The biggest track Kumi copies.
const MAX_CHUNK: usize = 32 << 20;
/// A make left in the journal this long is forgotten.
const FORGET_AFTER_MS: u64 = 30 * 24 * 3600 * 1000;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

// ----- the chunk of a copy -----

/// Whether `text` at `at` is a GUID as REAPER writes them: `{8-4-4-4-12}` in hex.
fn guid_at(text: &[u8], at: usize) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    if text.get(at) != Some(&b'{') {
        return false;
    }
    let mut i = at + 1;
    for (n, len) in GROUPS.iter().enumerate() {
        for _ in 0..*len {
            if !text.get(i).is_some_and(u8::is_ascii_hexdigit) {
                return false;
            }
            i += 1;
        }
        let want = if n == 4 { b'}' } else { b'-' };
        if text.get(i) != Some(&want) {
            return false;
        }
        i += 1;
    }
    true
}

const GUID_LEN: usize = 38;

/// Replaces the GUIDs in a line, outside quoted strings (a file's path, a name): the same old GUID always becomes the same
/// new one, so what the chunk says of itself still agrees.
fn fresh_line(line: &str, map: &mut HashMap<String, String>, fresh: &mut dyn FnMut() -> String) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut quote: Option<u8> = None;
    let mut at = 0;
    let mut start_of_token = true;
    while at < bytes.len() {
        let byte = bytes[at];
        match quote {
            Some(q) => {
                if byte == q {
                    quote = None;
                }
            }
            None if start_of_token && matches!(byte, b'"' | b'\'' | b'`') => quote = Some(byte),
            None if guid_at(bytes, at) => {
                let old = &line[at..at + GUID_LEN];
                let new = map.entry(old.to_string()).or_insert_with(&mut *fresh).clone();
                out.push_str(&new);
                at += GUID_LEN;
                start_of_token = false;
                continue;
            }
            None => {}
        }
        start_of_token = byte == b' ' || byte == b'\t';
        out.push(byte as char);
        at += 1;
    }
    // (only ASCII is pushed byte by byte above; take the original text when it has more)
    if !line.is_ascii() {
        return fresh_line_unicode(line, map, fresh);
    }
    out
}

/// As `fresh_line`, for a line with text outside ASCII (a name in quotes, usually).
fn fresh_line_unicode(line: &str, map: &mut HashMap<String, String>, fresh: &mut dyn FnMut() -> String) -> String {
    let bytes = line.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut quote: Option<u8> = None;
    let mut at = 0;
    let mut start_of_token = true;
    while at < bytes.len() {
        let byte = bytes[at];
        match quote {
            Some(q) if byte == q => quote = None,
            Some(_) => {}
            None if start_of_token && matches!(byte, b'"' | b'\'' | b'`') => quote = Some(byte),
            None if guid_at(bytes, at) => {
                let old = &line[at..at + GUID_LEN];
                let new = map.entry(old.to_string()).or_insert_with(&mut *fresh).clone();
                out.extend_from_slice(new.as_bytes());
                at += GUID_LEN;
                start_of_token = false;
                continue;
            }
            None => {}
        }
        start_of_token = byte == b' ' || byte == b'\t';
        out.push(byte);
        at += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| line.to_string())
}

/// What a copy's chunk is made from the source's: every GUID new, no hardware outputs and no track groups, not a folder,
/// not sending to the master or its parent, unarmed, unsoloed, unmuted, unselected, not writing automation. `shift` moves
/// the tracks its receives come from (when copies go in above them). Returns the chunk and the GUIDs it changed.
pub fn copy_chunk(chunk: &str, shift: i64, fresh: &mut dyn FnMut() -> String) -> (String, HashMap<String, String>) {
    let mut map = HashMap::new();
    let mut out = String::with_capacity(chunk.len() + 64);
    let mut depth = 0usize;
    let mut notes_at: Option<usize> = None;
    for line in chunk.lines() {
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        let mut words = trimmed.split_whitespace();
        let key = words.next().unwrap_or_default();
        // Text blocks (an item's notes) are the producer's words: left as they are.
        if notes_at.is_some() {
            if trimmed == ">" && Some(depth) == notes_at {
                notes_at = None;
                depth = depth.saturating_sub(1);
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if key.starts_with('<') {
            depth += 1;
            if key == "<NOTES" {
                notes_at = Some(depth);
                out.push_str(line);
                out.push('\n');
                continue;
            }
        } else if trimmed == ">" {
            depth = depth.saturating_sub(1);
        }
        // At the track's own level (depth 1): the lines that would make it reach the producer.
        let rest: Vec<&str> = words.collect();
        let own = depth == 1;
        let rewritten = match key {
            "HWOUT" if own => continue,
            "GROUP_FLAGS" | "GROUP_FLAGS_HIGH" if own => continue,
            "ISBUS" if own => "ISBUS 0 0".to_string(),
            "MAINSEND" if own => format!("MAINSEND 0 {}", rest.get(1).copied().unwrap_or("0")),
            "SEL" if own => "SEL 0".to_string(),
            "MUTESOLO" if own => format!("MUTESOLO 0 0 {}", rest.get(2).copied().unwrap_or("0")),
            "REC" if own && !rest.is_empty() => format!("REC 0 {}", rest[1..].join(" ")),
            "AUTOMODE" if own && rest.first().and_then(|m| m.parse::<i32>().ok()).is_some_and(|m| m >= 2) => "AUTOMODE 0".to_string(),
            "AUXRECV" if own && shift != 0 => match rest.first().and_then(|n| n.parse::<i64>().ok()) {
                Some(from) => format!("AUXRECV {} {}", from + shift, rest[1..].join(" ")),
                None => trimmed.to_string(),
            },
            _ => trimmed.to_string(),
        };
        out.push_str(indent);
        out.push_str(&fresh_line(&rewritten, &mut map, fresh));
        out.push('\n');
    }
    (out, map)
}

/// A GUID made here, for a REAPER that can't make one.
fn random_guid() -> String {
    let hex = daw_bridge::auth::random_hex16();
    format!("{{{}-{}-{}-{}-{}}}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32]).to_uppercase()
}

// ----- the journal -----

/// One make as the journal keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    /// The project's file when it was made (empty while unsaved) and its identity in that REAPER.
    pub path: String,
    pub identity: String,
    pub source: String,
    pub source_name: String,
    /// Each copy's GUID and name.
    pub copies: Vec<(String, String)>,
    pub made_ms: u64,
}

impl Entry {
    fn to_json(&self) -> Value {
        json!({
            "scratchId": self.id,
            "project": {"path": self.path, "identity": self.identity},
            "source": {"guid": self.source, "name": self.source_name},
            "copies": self.copies.iter().map(|(guid, name)| json!({"guid": guid, "name": name})).collect::<Vec<_>>(),
            "madeMs": self.made_ms,
        })
    }

    fn from_json(value: &Value) -> Option<Entry> {
        let text = |v: &Value| v.as_str().map(str::to_string);
        Some(Entry {
            id: text(&value["scratchId"])?,
            path: text(&value["project"]["path"]).unwrap_or_default(),
            identity: text(&value["project"]["identity"]).unwrap_or_default(),
            source: text(&value["source"]["guid"]).unwrap_or_default(),
            source_name: text(&value["source"]["name"]).unwrap_or_default(),
            copies: value["copies"].as_array()?.iter().filter_map(|c| Some((text(&c["guid"])?, text(&c["name"])?))).collect(),
            made_ms: value["madeMs"].as_u64().unwrap_or(0),
        })
    }
}

fn journal_path(folder: &Path) -> PathBuf {
    folder.join(JOURNAL)
}

/// The makes the journal holds; none when it is missing or unreadable (a broken one is set aside, never trusted).
pub fn read_journal(folder: &Path) -> Vec<Entry> {
    let path = journal_path(folder);
    let Ok(text) = std::fs::read_to_string(&path) else { return vec![] };
    match serde_json::from_str::<Value>(&text) {
        Ok(value) => value["makes"].as_array().map(|makes| makes.iter().filter_map(Entry::from_json).collect()).unwrap_or_default(),
        Err(_) => {
            let _ = std::fs::rename(&path, folder.join(format!("{JOURNAL}.broken")));
            vec![]
        }
    }
}

fn write_journal(folder: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let path = journal_path(folder);
    if entries.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let text = json!({"version": 1, "makes": entries.iter().map(Entry::to_json).collect::<Vec<_>>()});
    daw_bridge::owner::write_atomic(&path, text.to_string().as_bytes())
}

// ----- the copies standing now -----

/// One copy as Kumi made it.
#[derive(Debug, Clone)]
struct Copy {
    guid: String,
    name: String,
    mark: String,
    /// Its FX by GUID, in the order the source's chain was read (main chain, then input FX).
    fx: Vec<String>,
}

#[derive(Debug, Clone)]
struct Make {
    entry: Entry,
    proj: Project,
    copies: Vec<Copy>,
}

/// The scratch copies standing now, and what the journal still holds of earlier sessions.
#[derive(Default)]
pub struct Scratch {
    makes: Vec<Make>,
    /// Makes from before this bridge loaded, not yet swept.
    leftovers: Vec<Entry>,
    loaded: bool,
    /// `DIRTY` before the first of the copies standing now was made, and the state count after Kumi's last own edit of
    /// them: the drop that takes the last ones away puts `DIRTY` back only when nothing else changed the project since.
    dirty_before: Option<(Project, f64)>,
    kumi_count: Option<(Project, i32)>,
}

/// The tag in a make's names: the scratch id's last four characters.
fn tag(id: &str) -> String {
    let tail: String = id.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    tail.to_lowercase()
}

/// A track of a project by GUID, by looking (the registry knows only the tab in front).
fn find(reaper: &Reaper, proj: Project, guid: &str) -> Option<Track> {
    (0..reaper.track_count(proj)).filter_map(|i| reaper.track_at(proj, i)).find(|t| reaper.track_guid(*t) == guid)
}

/// The track's FX in the order a read lists them: the main chain, then the input chain.
fn chain(reaper: &Reaper, track: Track) -> Vec<Fx> {
    let owner = FxOwner::Track(track);
    (0..reaper.fx_count(owner))
        .map(|index| Fx { owner, index })
        .chain((0..reaper.input_fx_count(track)).map(|index| Fx { owner, index: INPUT_FX | index }))
        .collect()
}

/// Whether the producer has made the copy theirs: renamed it, put it in a folder, or sent it somewhere. (Where it sits
/// among the tracks says nothing: a copy kept with scratch-keep, or a track Kumi added, may come after the others.)
fn taken(reaper: &Reaper, track: Track, copy: &Copy) -> Option<String> {
    let name = reaper.track_name(track);
    if name != copy.name {
        return Some(format!("{} was renamed {}", crate::changes::quoted(&copy.name), crate::changes::quoted(&name)));
    }
    if reaper.track_parent(track).is_some() {
        return Some(format!("{} was moved into a folder", crate::changes::quoted(&copy.name)));
    }
    if reaper.track_value(track, "B_MAINSEND") != 0.0 || reaper.send_count(track, 0) > 0 || reaper.send_count(track, 1) > 0 {
        return Some(format!("{} sends somewhere now", crate::changes::quoted(&copy.name)));
    }
    None
}

impl Scratch {
    /// Whether the journal still names copies to sweep (an earlier session's, or dropped ones an undo may bring back).
    pub fn has_leftovers(&self) -> bool {
        !self.leftovers.is_empty()
    }

    /// Copies standing now.
    pub fn standing(&self) -> usize {
        self.makes.iter().map(|make| make.copies.len()).sum()
    }

    fn load(&mut self, folder: &Path) {
        if !self.loaded {
            self.loaded = true;
            self.leftovers = read_journal(folder);
        }
    }

    fn save(&self, folder: &Path) -> std::io::Result<()> {
        let entries: Vec<Entry> = self.leftovers.iter().cloned().chain(self.makes.iter().map(|m| m.entry.clone())).collect();
        write_journal(folder, &entries)
    }

    fn noted(&mut self, reaper: &Reaper, proj: Project) {
        self.kumi_count = reaper.state_change_count(proj).map(|count| (proj, count));
    }

    /// `scratch {action, …}`.
    pub fn answer(&mut self, reaper: &Reaper, reads: &Reads, folder: &Path, params: &Value) -> Answer {
        self.load(folder);
        match params.get("action").and_then(Value::as_str) {
            Some("make") => self.make(reaper, reads, folder, params),
            Some("set") => self.set(reaper, params),
            Some("drop") => self.drop_copies(reaper, folder, params),
            _ => Err(bad("action: make, set or drop")),
        }
    }

    fn make(&mut self, reaper: &Reaper, reads: &Reads, folder: &Path, params: &Value) -> Answer {
        let id = params.get("scratchId").and_then(Value::as_str).unwrap_or_default();
        if !super::render::id_ok(id, 40) {
            return Err(bad("scratchId: the host's id for these copies"));
        }
        if self.makes.iter().any(|make| make.entry.id == id) {
            return Err(bad(format!("copies {id} are made already")));
        }
        let count =
            params.get("count").and_then(Value::as_u64).filter(|n| (1..=MAX_COPIES).contains(n)).ok_or_else(|| bad("count: 1 to 16"))?;
        let label = params.get("label").and_then(Value::as_str).unwrap_or_default();
        if label.is_empty() || label.chars().count() > 80 {
            return Err(bad("label: what the copies are for, in a few words"));
        }
        let info = reaper.current_project().ok_or_else(|| failed("REAPER has no project open"))?;
        let proj = info.ptr;
        let epoch = params.get("epoch").and_then(Value::as_str).ok_or_else(|| bad("epoch: the epoch the ref was read in"))?;
        if epoch != reads.epoch() && !reads.epoch_is_of(epoch, proj) {
            return Err(stale(format!(
                "those refs are from an earlier epoch ({epoch}; now {}): the project in the tab changed, read again",
                reads.epoch()
            )));
        }
        if self.standing() + count as usize > MAX_STANDING {
            return Err(failed(format!("{} scratch copies stand already: drop some first", self.standing())));
        }
        let reference = params.get("ref").and_then(Value::as_str).ok_or_else(|| bad("ref: the track to copy"))?;
        let source_guid = match Ref::parse(reference).map_err(bad)? {
            Ref::Track(TrackId::Guid(guid)) => guid,
            Ref::Track(TrackId::Master) => return Err(bad("the master can't be copied: copy the track whose sound is wanted")),
            other => return Err(bad(format!("ref must be a track ref, not r:{}", other.kind()))),
        };
        let source = reads
            .track_by_id(reaper, proj, &TrackId::Guid(source_guid.clone()))
            .map_err(|_| gone("that track isn't in the project any more: read again"))?;
        let chunk = reaper.track_chunk(source).ok_or_else(|| failed("REAPER wouldn't give the track's state"))?;
        if chunk.len() > MAX_CHUNK {
            return Err(failed(format!("the track's state is {} MB: too big for Kumi to copy", chunk.len() >> 20)));
        }
        let source_name = crate::changes::track_name(reaper, source);
        // The tracks a folder's copy gets its sound from: those that feed it now.
        let feeders: Vec<Track> = (0..reaper.track_count(proj))
            .filter_map(|i| reaper.track_at(proj, i))
            .filter(|t| reaper.track_parent(*t) == Some(source) && reaper.track_value(*t, "B_MAINSEND") != 0.0)
            .collect();
        // Copies go after the last track, unless it leaves a folder open (they would land in it): then above the first.
        let open_at_end: i64 = (0..reaper.track_count(proj))
            .filter_map(|i| reaper.track_at(proj, i))
            .map(|t| reaper.track_value(t, "I_FOLDERDEPTH") as i64)
            .sum();
        let on_top = open_at_end > 0;
        let tag = tag(id);
        let mut fresh = random_guid;
        let mut planned: Vec<(String, String, String)> = Vec::new();
        for n in 1..=count {
            let shift = if on_top { n as i64 } else { 0 };
            let (text, map) = copy_chunk(&chunk, shift, &mut fresh);
            let guid = map.get(&source_guid).cloned().unwrap_or_else(random_guid);
            planned.push((guid, format!("{NAME} {tag} {n}"), text));
        }
        let entry = Entry {
            id: id.into(),
            path: info.path.clone(),
            identity: reads.identity(proj),
            source: source_guid.clone(),
            source_name: source_name.clone(),
            copies: planned.iter().map(|(guid, name, _)| (guid.clone(), name.clone())).collect(),
            made_ms: now_ms(),
        };
        // Noted before the first copy is made: a crash from here on leaves them findable.
        let mut make = Make { entry, proj, copies: vec![] };
        self.makes.push(make.clone());
        if let Err(error) = self.save(folder) {
            self.makes.pop();
            return Err(failed(format!("Kumi couldn't note its copies in its folder ({error}), so it made none")));
        }
        if self.standing() == 0 && self.dirty_before.is_none_or(|(p, _)| p != proj) {
            self.dirty_before = reaper.project_number(proj, "DIRTY").map(|d| (proj, d));
        }
        let source_fx: Vec<String> = chain(reaper, source).into_iter().map(|fx| reaper.fx_guid(fx)).collect();
        let mut made: Vec<Track> = Vec::new();
        let mut failure: Option<String> = None;
        for (n, (guid, name, text)) in planned.iter().enumerate() {
            let at = if on_top { n as i32 } else { reaper.track_count(proj) };
            let Some(track) = reaper.insert_track(proj, at) else {
                failure = Some("REAPER wouldn't add a track".into());
                break;
            };
            made.push(track);
            if !reaper.set_track_chunk(track, text) {
                failure = Some("REAPER wouldn't take the copy's state".into());
                break;
            }
            let copy_guid = reaper.track_guid(track);
            let copy_fx: Vec<String> = chain(reaper, track).into_iter().map(|fx| reaper.fx_guid(fx)).collect();
            if copy_guid != *guid || copy_guid == source_guid || copy_fx.iter().any(|fx| source_fx.contains(fx)) {
                failure = Some("REAPER kept an id of the track it copied, so the copy would be mistaken for it".into());
                break;
            }
            reaper.set_track_string(track, "P_NAME", name);
            reaper.set_track_string(track, MARK, &format!("{id} {}", n + 1));
            for (parm, value) in
                [("B_MAINSEND", 0.0), ("I_RECARM", 0.0), ("I_SOLO", 0.0), ("B_MUTE", 0.0), ("I_SELECTED", 0.0), ("I_FOLDERDEPTH", 0.0)]
            {
                if reaper.track_value(track, parm) != value {
                    reaper.set_track_value(track, parm, value);
                }
            }
            if reaper.track_value(track, "I_AUTOMODE") >= 2.0 {
                reaper.set_track_value(track, "I_AUTOMODE", 0.0);
            }
            while reaper.send_count(track, 1) > 0 {
                if !reaper.remove_send(track, 1, 0) {
                    break;
                }
            }
            for feeder in &feeders {
                if let Some(index) = reaper.create_send(*feeder, Some(track)) {
                    reaper.set_send_value(*feeder, 0, index, "I_SENDMODE", 0.0);
                    reaper.set_send_value(*feeder, 0, index, "I_DSTCHAN", reaper.track_value(*feeder, "C_MAINSEND_OFFS").max(0.0));
                }
            }
            make.copies.push(Copy { guid: copy_guid, name: name.clone(), mark: format!("{id} {}", n + 1), fx: copy_fx });
        }
        if let Some(why) = failure {
            for track in made.iter().rev() {
                if reaper.track_valid(proj, *track) {
                    reaper.delete_track(*track);
                }
            }
            self.makes.retain(|m| m.entry.id != id);
            let _ = self.save(folder);
            return Err(failed(format!("{why}: no copies were made")));
        }
        reaper.refresh_after_change();
        self.noted(reaper, proj);
        let answer = json!({
            "scratchId": id,
            "copies": make.copies.iter().map(|c| json!({"ref": refs::track_ref(&TrackId::Guid(c.guid.clone())), "name": c.name})).collect::<Vec<_>>(),
            "epoch": reads.epoch(),
            "undo": {"steps": 0},
        });
        let mut answer = answer;
        let mut notes = vec![];
        if !feeders.is_empty() {
            notes.push(format!(
                "{} is a folder: each copy hears its tracks through sends from them, which go with the copies",
                crate::changes::quoted(&source_name)
            ));
        }
        if on_top {
            notes.push("The last track leaves a folder open, so the copies are above the first track instead".to_string());
        }
        if !notes.is_empty() {
            answer["notes"] = json!(notes);
        }
        if let Some(standing) = self.makes.iter_mut().find(|m| m.entry.id == id) {
            *standing = make;
        }
        Ok(answer)
    }

    fn set(&mut self, reaper: &Reaper, params: &Value) -> Answer {
        let id = params.get("scratchId").and_then(Value::as_str).unwrap_or_default();
        let make = self.makes.iter().find(|make| make.entry.id == id).ok_or_else(|| bad(format!("Kumi has no scratch copies {id:?}")))?;
        let values = params
            .get("values")
            .and_then(Value::as_array)
            .filter(|v| !v.is_empty() && v.len() <= 4096)
            .ok_or_else(|| bad("values: 1 to 4096 of them"))?;
        if let Some(busy) = reaper.busy() {
            return Err(("busy".into(), format!("{}; nothing changed", busy.reason())));
        }
        // (a project whose tab was closed is never asked anything: its pointer is gone)
        if !reaper.project_valid(make.proj) {
            return Err(gone("the project these copies were made in isn't open any more"));
        }
        // All checked first: a set is written whole or not at all.
        let mut writes: Vec<(Fx, i32, f64)> = Vec::new();
        for value in values {
            let reference = value.get("ref").and_then(Value::as_str).unwrap_or_default();
            let guid = match Ref::parse(reference) {
                Ok(Ref::Track(TrackId::Guid(guid))) => guid,
                _ => return Err(bad(format!("{reference:?} isn't a copy's track ref"))),
            };
            let copy = make.copies.iter().find(|c| c.guid == guid).ok_or_else(|| bad(format!("{reference} isn't one of {id}'s copies")))?;
            let track = find(reaper, make.proj, &guid).ok_or_else(|| {
                gone(format!("{} isn't there any more (REAPER's undo may have taken it)", crate::changes::quoted(&copy.name)))
            })?;
            let device =
                value.get("device").and_then(Value::as_u64).ok_or_else(|| bad("device: the FX's place in the track's chain, from 0"))?
                    as usize;
            let param = value.get("param").and_then(Value::as_u64).ok_or_else(|| bad("param: the parameter's index, from 0"))? as i32;
            let normalized = match (value.get("normalized").and_then(Value::as_f64), value.get("value")) {
                (Some(n), _) if (0.0..=1.0).contains(&n) => n,
                (Some(_), _) => return Err(bad("normalized: 0 to 1")),
                (None, Some(_)) => return Err(bad("REAPER takes normalized (0 to 1), not value")),
                (None, None) => return Err(bad("normalized: 0 to 1")),
            };
            let fxs = chain(reaper, track);
            let fx = *fxs
                .get(device)
                .ok_or_else(|| bad(format!("{} has {} FX: device {device} isn't one", crate::changes::quoted(&copy.name), fxs.len())))?;
            if copy.fx.get(device).is_none_or(|made| *made != reaper.fx_guid(fx)) {
                return Err(("changed".into(), format!("the FX of {} changed since Kumi made it", crate::changes::quoted(&copy.name))));
            }
            if param < 0 || param >= reaper.fx_param_count(fx) {
                return Err(bad(format!("that FX has {} parameters: param {param} isn't one", reaper.fx_param_count(fx))));
            }
            writes.push((fx, param, normalized));
        }
        for (fx, param, normalized) in &writes {
            if let FxOwner::Track(track) = fx.owner {
                reaper.set_fx_param_normalized(track, fx.index, *param, *normalized);
            }
        }
        let proj = make.proj;
        self.noted(reaper, proj);
        Ok(json!({"set": writes.len(), "undo": {"steps": 0}}))
    }

    fn drop_copies(&mut self, reaper: &Reaper, folder: &Path, params: &Value) -> Answer {
        let id = params.get("scratchId").and_then(Value::as_str).unwrap_or_default();
        let Some(at) = self.makes.iter().position(|make| make.entry.id == id) else {
            // Asked again (a retry), or never made: nothing stands.
            return Ok(json!({"dropped": 0, "left": 0, "undo": {"steps": 0}}));
        };
        let make = self.makes[at].clone();
        let proj = make.proj;
        let alive = reaper.project_valid(proj);
        // Nothing but Kumi changed the project since its last edit of the copies: DIRTY may go back once the last is gone.
        let untouched = alive && self.kumi_count.is_some_and(|(p, count)| p == proj && reaper.state_change_count(proj) == Some(count));
        let (mut dropped, mut gone_count, mut kept) = (0usize, 0usize, 0usize);
        let mut notes = Vec::new();
        let mut left = Vec::new();
        // (the copies Kumi removed, or found gone: what an undo or redo may bring back)
        let mut removed: Vec<String> = Vec::new();
        for copy in &make.copies {
            let Some(track) = alive.then(|| find(reaper, proj, &copy.guid)).flatten() else {
                gone_count += 1;
                removed.push(copy.guid.clone());
                continue;
            };
            let mark = reaper.track_string(track, MARK).unwrap_or_default();
            if mark != copy.mark {
                // Kept with scratch-keep (its mark taken off): a track of the producer's now.
                kept += 1;
                continue;
            }
            if let Some(why) = taken(reaper, track, copy) {
                left.push(why);
                continue;
            }
            if reaper.delete_track(track) {
                dropped += 1;
                removed.push(copy.guid.clone());
            } else {
                left.push(format!("REAPER wouldn't delete {}", crate::changes::quoted(&copy.name)));
            }
        }
        if gone_count > 0 {
            notes.push(format!("{gone_count} of the copies were gone already (REAPER's undo may have taken them)"));
        }
        for why in &left {
            notes.push(format!("{why}: Kumi left it as yours"));
        }
        // The copies Kumi removed (and those REAPER's undo had) were bare edits inside the producer's undo points: an undo or
        // redo may bring them back. The journal keeps them, to be swept, until their saved project is seen without them.
        let mut entry = self.makes.remove(at).entry;
        entry.copies.retain(|(guid, _)| removed.iter().any(|done| done == guid));
        if !entry.copies.is_empty() {
            self.leftovers.push(entry);
        }
        let _ = self.save(folder);
        if dropped > 0 {
            reaper.refresh_after_change();
        }
        if self.standing() == 0 {
            if let Some((p, dirty)) = self.dirty_before.take() {
                if untouched && p == proj && kept == 0 && left.is_empty() && reaper.project_number(proj, "DIRTY") != Some(dirty) {
                    reaper.set_project_number(proj, "DIRTY", dirty);
                }
            }
        }
        if alive {
            self.noted(reaper, proj);
        }
        let mut answer = json!({"dropped": dropped + gone_count, "left": left.len(), "undo": {"steps": 0}});
        if !notes.is_empty() {
            answer["notes"] = json!(notes);
        }
        Ok(answer)
    }

    /// Removes copies an earlier session left in these projects (by GUID, while they still carry Kumi's mark and name),
    /// and marks each project it removed some from as changed (its file still holds them until it is saved). A make is
    /// forgotten once none of its copies is left to look for (a copy the producer took is theirs, one Kumi removed may
    /// still be in the saved file), its saved project is open, unchanged and holds none of them (the file is clean), or it
    /// is a month old. Returns the names removed.
    pub fn sweep(&mut self, reaper: &Reaper, folder: &Path, projects: &[ProjectInfo]) -> Vec<String> {
        self.load(folder);
        if self.leftovers.is_empty() {
            return vec![];
        }
        let mut names = Vec::new();
        let now = now_ms();
        let mut changed = false;
        let mut keep = Vec::new();
        let mut swept_in: Vec<crate::reaper::Project> = Vec::new();
        for mut entry in std::mem::take(&mut self.leftovers) {
            let mut found = false;
            // A copy the producer took (renamed, the mark gone) is theirs: no longer looked for.
            let mut taken = vec![false; entry.copies.len()];
            for project in projects {
                for (n, (guid, name)) in entry.copies.iter().enumerate() {
                    let Some(track) = find(reaper, project.ptr, guid) else { continue };
                    found = true;
                    let mark = reaper.track_string(track, MARK).unwrap_or_default();
                    if mark.split(' ').next() == Some(entry.id.as_str()) && reaper.track_name(track) == *name {
                        if reaper.delete_track(track) {
                            names.push(name.clone());
                            if !swept_in.contains(&project.ptr) {
                                swept_in.push(project.ptr);
                            }
                        }
                    } else {
                        taken[n] = true;
                    }
                }
            }
            let clean_file = !found && !entry.path.is_empty() && projects.iter().any(|p| p.path == entry.path && !reaper.is_dirty(p.ptr));
            let old = now.saturating_sub(entry.made_ms) > FORGET_AFTER_MS;
            let before = entry.copies.len();
            entry.copies = entry.copies.into_iter().zip(taken).filter(|(_, t)| !t).map(|(c, _)| c).collect();
            changed |= entry.copies.len() != before;
            if entry.copies.is_empty() || clean_file || old {
                changed = true;
                continue;
            }
            keep.push(entry);
        }
        self.leftovers = keep;
        if changed {
            let _ = self.save(folder);
        }
        for proj in &swept_in {
            reaper.set_project_number(*proj, "DIRTY", 1.0);
        }
        if !names.is_empty() {
            reaper.refresh_after_change();
        }
        names
    }

    /// Whether a track is a copy standing now (not kept), and its name.
    pub fn copy_named(&self, guid: &str) -> Option<String> {
        self.makes.iter().flat_map(|m| &m.copies).find(|c| c.guid == guid).map(|c| c.name.clone())
    }

    /// The bridge is going: copies stay (the journal sweeps them next time), the in-memory makes are forgotten.
    pub fn forget(&mut self) {
        self.makes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter() -> impl FnMut() -> String {
        let mut n = 0;
        move || {
            n += 1;
            format!("{{00000000-0000-0000-0000-{n:012}}}")
        }
    }

    /// A track as REAPER 7 writes it: a folder parent armed, soloed, selected, in a group, with a hardware output, a
    /// receive, an item whose take and MIDI source have GUIDs, notes with a GUID-shaped text, and an FX with an envelope.
    const CHUNK: &str = r#"<TRACK {8E4E2D9C-AAAA-4C7A-9E0E-0123456789AB}
  NAME "Lead {5AB1C2D3-1111-2222-3333-444455556666} 'quoted'"
  PEAKCOL 16576
  BEAT -1
  AUTOMODE 3
  VOLPAN 0.5 0 -1 -1 1
  MUTESOLO 1 2 0
  IPHASE 0
  ISBUS 1 1
  SEL 1
  REC 1 0 1 0 0 0 0 0
  GROUP_FLAGS 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
  TRACKID {8E4E2D9C-AAAA-4C7A-9E0E-0123456789AB}
  MAINSEND 1 2
  AUXRECV 3 0 1 0 0 0 0 0 0 -1:U 0 -1 ''
  HWOUT 0 0 1 0 0 0 0 -1:U -1
  <FXCHAIN
    BYPASS 0 0 0
    <VST "VST3: ReaEQ (Cockos)" reaeq.vst3 0 "" 1919247729{56535452656571726561657100000000} ""
      cmVhZXEAAAAAAAAAAgAAAAEAAAAAAAAAAgAAAAAAAAACAAAAAQAAAAAAAAACAAAAAAAAAAMAAAABAAAAAAAAAAIAAAAAAAAA
    >
    FXID {11111111-2222-3333-4444-555555555555}
    <PARMENV 1 0 1 0.5
      EGUID {22222222-3333-4444-5555-666666666666}
      PT 0 0.5 0
    >
    WAK 0 0
  >
  <ITEM
    POSITION 2
    LENGTH 4
    <NOTES
      |remember {33333333-4444-5555-6666-777777777777}
    >
    IGUID {44444444-5555-6666-7777-888888888888}
    NAME "take"
    GUID {55555555-6666-7777-8888-999999999999}
    <SOURCE MIDI
      HASDATA 1 960 QN
      POOLEDEVTS {66666666-7777-8888-9999-AAAAAAAAAAAA}
      E 0 90 3c 60
      GUID {77777777-8888-9999-AAAA-BBBBBBBBBBBB}
    >
  >
  <ITEM
    POSITION 8
    <SOURCE WAVE
      FILE "/audio/{99999999-8888-7777-6666-555555555555}.wav"
    >
    IGUID {88888888-9999-AAAA-BBBB-CCCCCCCCCCCC}
    GUID {55555555-6666-7777-8888-999999999999}
  >
>
"#;

    #[test]
    fn a_copys_chunk_has_every_guid_new_and_reaches_nothing() {
        let (copy, map) = copy_chunk(CHUNK, 0, &mut counter());
        let track = "{8E4E2D9C-AAAA-4C7A-9E0E-0123456789AB}";
        // Every GUID outside quotes and notes changed, the same old one to the same new one.
        for old in [
            track,
            "{11111111-2222-3333-4444-555555555555}",
            "{22222222-3333-4444-5555-666666666666}",
            "{44444444-5555-6666-7777-888888888888}",
            "{55555555-6666-7777-8888-999999999999}",
            "{66666666-7777-8888-9999-AAAAAAAAAAAA}",
            "{77777777-8888-9999-AAAA-BBBBBBBBBBBB}",
            "{88888888-9999-AAAA-BBBB-CCCCCCCCCCCC}",
        ] {
            assert!(!copy.contains(old), "{old} is still in the copy");
            assert!(map.contains_key(old), "{old} wasn't mapped");
        }
        let new_track = &map[track];
        assert!(copy.starts_with(&format!("<TRACK {new_track}\n")));
        assert!(copy.contains(&format!("  TRACKID {new_track}\n")), "the header and TRACKID agree");
        // The same take GUID in two items maps to one new GUID (REAPER would have refused the original anyway).
        assert_eq!(copy.matches(map["{55555555-6666-7777-8888-999999999999}"].as_str()).count(), 2);
        // Quoted text and notes are the producer's words: a name, a file's path, an item's notes stay as they were.
        assert!(copy.contains(r#"NAME "Lead {5AB1C2D3-1111-2222-3333-444455556666} 'quoted'""#));
        assert!(copy.contains(r#"FILE "/audio/{99999999-8888-7777-6666-555555555555}.wav""#));
        assert!(copy.contains("|remember {33333333-4444-5555-6666-777777777777}"));
        // A VST3's class id (no dashes) isn't a GUID.
        assert!(copy.contains("1919247729{56535452656571726561657100000000}"));
        // It reaches nothing of the producer's: no hardware output, no group, no folder, no master send; unarmed, unsoloed,
        // unmuted, unselected, reading automation without writing it. Receives stay (they feed it as they feed the source).
        assert!(!copy.contains("HWOUT") && !copy.contains("GROUP_FLAGS"));
        for line in [
            "  ISBUS 0 0\n",
            "  MAINSEND 0 2\n",
            "  SEL 0\n",
            "  MUTESOLO 0 0 0\n",
            "  REC 0 0 1 0 0 0 0 0\n",
            "  AUTOMODE 0\n",
            "  AUXRECV 3 0 1 0 0 0 0 0 0 -1:U 0 -1 ''\n",
        ] {
            assert!(copy.contains(line), "{line:?} missing from:\n{copy}");
        }
        // FX state and envelope points are copied as they are.
        assert!(copy.contains("      cmVhZXEAAAAAAAAAAgAAAAEAAAAAAAAAAgAAAAAAAAACAAAAAQAAAAAAAAACAAAAAAAAAAMAAAABAAAAAAAAAAIAAAAAAAAA\n"));
        assert!(copy.contains("      PT 0 0.5 0\n"));
        // Copies placed above the tracks their receives come from move those numbers.
        let (above, _) = copy_chunk(CHUNK, 2, &mut counter());
        assert!(above.contains("  AUXRECV 5 0 1 0 0 0 0 0 0 -1:U 0 -1 ''\n"));
        // Two copies made with one counter share nothing.
        let mut fresh = counter();
        let (_, first) = copy_chunk(CHUNK, 0, &mut fresh);
        let (_, second) = copy_chunk(CHUNK, 0, &mut fresh);
        assert!(first.values().all(|guid| !second.values().any(|other| other == guid)));
    }

    #[test]
    fn a_guid_is_only_the_exact_shape() {
        let text = b"{8E4E2D9C-AAAA-4C7A-9E0E-0123456789AB} {8E4E2D9C-AAAA-4C7A-9E0E-0123456789A} {8E4E2D9CAAAA4C7A9E0E0123456789AB}";
        assert!(guid_at(text, 0));
        assert!(!guid_at(text, 39));
        assert!(!guid_at(text, 77));
        assert_eq!(tag("s1x9ab"), "x9ab");
        assert_eq!(tag("s7"), "s7");
        assert!(guid_at(random_guid().as_bytes(), 0));
    }

    #[test]
    fn the_journal_round_trips_and_a_broken_one_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_journal(dir.path()).is_empty());
        let entry = Entry {
            id: "s1".into(),
            path: "/p/Song.rpp".into(),
            identity: "1:1".into(),
            source: "{A}".into(),
            source_name: "Lead".into(),
            copies: vec![("{C1}".into(), format!("{NAME} s1 1"))],
            made_ms: 5,
        };
        write_journal(dir.path(), std::slice::from_ref(&entry)).unwrap();
        assert_eq!(read_journal(dir.path()), [entry]);
        write_journal(dir.path(), &[]).unwrap();
        assert!(!journal_path(dir.path()).exists());
        std::fs::write(journal_path(dir.path()), "{not json").unwrap();
        assert!(read_journal(dir.path()).is_empty());
        assert!(dir.path().join("scratch.json.broken").exists());
    }
}
