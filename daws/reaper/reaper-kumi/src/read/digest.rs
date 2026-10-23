//! `digest {cursor?, values?, content?}`: the project as records for catch-up and watch_me to compare (the host keeps and
//! diffs them), a page at a time. Every record is keyed by REAPER's GUID: tracks (the master's is "master") with their
//! mixer, folder, colour, input, sends, items and automation as facets; their FX (container children under their
//! container); their items, unless the project has more than `MOST_ITEMS`; markers and regions, unless more than
//! `MOST_MARKERS`. A facet leaf named `…Hash` is a fingerprint; a null leaf wasn't read (an automated fader or
//! parameter follows its envelope, so it's left to the envelope's fingerprint).
//!
//! Bounded: each page holds about `PAGE_BYTES` and `PAGE_TIME` of work, inside a track too (a track of many items or
//! points is read over several pages, and read once for all its pages); parameters, envelope points and (with `values`)
//! parameter values have budgets for the whole digest, carried in the cursor. `content: false` (watch_me's looks) leaves
//! out items and envelope points.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::{active, bad, Reads};
use crate::queue::Answer;
use crate::reaper::{Fx, FxOwner, Project, Reaper, Track, INPUT_FX};
use crate::refs;
use crate::rows;

/// Items listed one by one up to this many in the project; past it, each track sums its items up.
const MOST_ITEMS: usize = 5_000;
/// Markers and regions listed up to this many; past it, the project sums them up.
const MOST_MARKERS: usize = 2_000;
/// Parameters a digest reads for its fingerprints, and per FX.
const PARAMS_BUDGET: usize = 100_000;
const PARAMS_PER_FX: i32 = 512;
/// Parameter values a digest lists (watch_me): about 1 MB of pages more at most.
const VALUES_BUDGET: usize = 100_000;
/// Envelope points a digest reads for its fingerprints.
const POINTS_BUDGET: usize = 200_000;
/// A page's share of the answer, and of REAPER's main thread.
const PAGE_BYTES: usize = 192 * 1024;
const PAGE_TIME: Duration = Duration::from_millis(20);

/// Tracks read part way (or read, for their later pages) kept at once: a few digests may go on side by side.
const KEPT_WORK: usize = 4;

#[cfg(test)]
thread_local! {
    /// A test's own page size, item and marker limits: (page bytes, most items, most markers).
    pub static TEST_LIMITS: std::cell::Cell<Option<(usize, usize, usize)>> = const { std::cell::Cell::new(None) };
    /// A test's page share of the main thread as a number of points and items read, in place of `PAGE_TIME`.
    pub static TEST_READS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// A page's share of REAPER's main thread.
struct Clock {
    started: Instant,
    reads: usize,
}

impl Clock {
    fn over(&self) -> bool {
        #[cfg(test)]
        if let Some(most) = TEST_READS.with(|reads| reads.get()) {
            return self.reads >= most;
        }
        self.started.elapsed() >= PAGE_TIME
    }

    /// One more point or item read; whether the page's share is used up.
    fn spend(&mut self) -> bool {
        self.reads += 1;
        self.over()
    }
}

fn limits() -> (usize, usize, usize) {
    #[cfg(test)]
    if let Some(limits) = TEST_LIMITS.with(|limits| limits.get()) {
        return limits;
    }
    (PAGE_BYTES, MOST_ITEMS, MOST_MARKERS)
}

/// FNV-1a over what's fed: a fingerprint only this bridge compares with itself.
struct Print(u64);

impl Print {
    fn new() -> Self {
        Print(0xcbf2_9ce4_8422_2325)
    }
    fn text(&mut self, text: &str) -> &mut Self {
        for byte in text.bytes().chain([0x1f]) {
            self.0 ^= byte as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        self
    }
    fn number(&mut self, value: f64, places: i32) -> &mut Self {
        self.text(&format!("{:.*}", places as usize, if value.is_finite() { value } else { 0.0 }))
    }
    fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

fn round(value: f64, places: i32) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// Where a digest goes on from, and what it has used of its budgets by then. Written `g2.<epoch>.<digest>.<n>.<sub>.
/// <params>.<points>.<values>.<items>.<markers>`: `digest` this digest's serial (what it read of a track part way is kept
/// by it); `n` the track (0 the master) or, past the tracks, the marker; `sub` the record within it; the budgets as they
/// were when that track began, so it's read again the same way.
#[derive(Debug, Clone, PartialEq)]
struct Cursor {
    epoch: String,
    digest: u64,
    n: usize,
    sub: usize,
    params: usize,
    points: usize,
    values: usize,
    items: bool,
    markers: bool,
}

impl Cursor {
    fn text(&self) -> String {
        format!(
            "g2.{}.{}.{}.{}.{}.{}.{}.{}.{}",
            self.epoch, self.digest, self.n, self.sub, self.params, self.points, self.values, self.items as u8, self.markers as u8
        )
    }

    fn parse(text: &str) -> Option<Cursor> {
        let parts: Vec<&str> = text.split('.').collect();
        if parts.len() != 10 || parts[0] != "g2" || parts[1].is_empty() {
            return None;
        }
        let number = |at: usize| parts[at].parse::<usize>().ok();
        Some(Cursor {
            epoch: parts[1].into(),
            digest: parts[2].parse().ok()?,
            n: number(3)?,
            sub: number(4)?,
            params: number(5)?,
            points: number(6)?,
            values: number(7)?,
            items: parts[8] == "1",
            markers: parts[9] == "1",
        })
    }
}

/// What a digest asked for, and what it has used.
struct Budget {
    values: bool,
    content: bool,
    items: bool,
    params: usize,
    points: usize,
    value_count: usize,
}

/// The digest, the track, the project's change count and the budgets as the track began.
type WorkKey = (u64, usize, Option<i32>, usize, usize, usize);

/// A track being read, kept between the pages of one digest: where its points' and items' loops are and their prints so
/// far, so a page out of time goes on from there; once read, its records, which its later pages take as they are.
struct Work {
    /// Another digest, track, change count or budgets, and the track is read again.
    key: WorkKey,
    budget: Budget,
    envelope: i32,
    point: i32,
    points: Print,
    points_read: bool,
    points_done: bool,
    item: i32,
    item_print: Print,
    item_records: Vec<Value>,
    items_done: bool,
    /// The FX still to read, the next last (an FX, then its container's children), once begun; their records so far; and
    /// what of the track is automated, which they read by.
    fx_left: Option<Vec<FxPlace>>,
    fx_records: Vec<Value>,
    automated: Option<Automated>,
    records: Option<Vec<Value>>,
}

/// An FX to read: where it sits in its chain, the GUID of what holds it (its track, or its container) and how deep.
struct FxPlace {
    fx: Fx,
    position: usize,
    parent: String,
    depth: usize,
}

impl Work {
    /// Nothing read yet (`content: false` reads no points or items).
    fn new(key: WorkKey, budget: Budget) -> Self {
        let content = budget.content;
        Work {
            key,
            budget,
            envelope: 0,
            point: 0,
            points: Print::new(),
            points_read: content,
            points_done: !content,
            item: 0,
            item_print: Print::new(),
            item_records: vec![],
            items_done: !content,
            fx_left: None,
            fx_records: vec![],
            automated: None,
            records: None,
        }
    }
}

#[derive(Default)]
struct Kept {
    /// The serial the next digest takes.
    next: u64,
    works: VecDeque<Work>,
}

thread_local! {
    /// The tracks read part way, or read for their later pages (REAPER's main thread reads them all).
    static KEPT: RefCell<Kept> = RefCell::new(Kept::default());
}

/// A new digest's serial.
fn serial() -> u64 {
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        kept.next += 1;
        kept.next
    })
}

/// The work kept for a track by its key, taken out (it's kept again if the page ends inside the track).
fn take_work(key: &WorkKey) -> Option<Work> {
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        let at = kept.works.iter().position(|work| work.key == *key)?;
        kept.works.remove(at)
    })
}

fn keep_work(work: Work) {
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        kept.works.push_back(work);
        while kept.works.len() > KEPT_WORK {
            kept.works.pop_front();
        }
    })
}

/// The parameters a track's envelopes automate, by FX (as `TrackFX_` addresses it), and its automated faders.
struct Automated {
    params: HashSet<(i32, i32)>,
    volume: bool,
    pan: bool,
    width: bool,
    mute: bool,
}

fn automated(reaper: &Reaper, track: Track) -> Automated {
    let mut found = Automated { params: HashSet::new(), volume: false, pan: false, width: false, mute: false };
    for index in 0..reaper.track_envelope_count(track) {
        let Some(envelope) = reaper.track_envelope(track, index) else { continue };
        if reaper.envelope_point_count(envelope) == 0 && reaper.ai_count(envelope) == 0 {
            continue;
        }
        match reaper.envelope_owner(envelope) {
            Some((_, fx, param)) if fx >= 0 => {
                found.params.insert((fx, param));
            }
            _ => {}
        }
    }
    let has = |chunk: &str| {
        reaper.track_envelope_by_chunk_name(track, chunk).is_some_and(|e| reaper.envelope_point_count(e) > 0 || reaper.ai_count(e) > 0)
    };
    found.volume = has("<VOLENV2");
    found.pan = has("<PANENV2");
    found.width = has("<WIDTHENV2");
    found.mute = has("<MUTEENV");
    found
}

impl Reads {
    /// `digest`: one page of the project's records.
    pub fn digest(&self, reaper: &Reaper, params: &Value) -> Answer {
        let (proj, info) = active(reaper)?;
        let values = params.get("values") == Some(&Value::Bool(true));
        let content = params.get("content") != Some(&Value::Bool(false));
        let epoch = self.epoch();
        let (page_bytes, most_items, most_markers) = limits();
        let track_total = reaper.track_count(proj).max(0) as usize + 1;
        let marker_total = reaper.marker_count(proj).max(0) as usize;
        let mut cursor = match params.get("cursor") {
            None | Some(Value::Null) => {
                let items: usize = (1..track_total)
                    .filter_map(|n| reaper.track_at(proj, n as i32 - 1))
                    .map(|t| reaper.track_item_count(t).max(0) as usize)
                    .sum();
                Cursor {
                    epoch: epoch.clone(),
                    digest: serial(),
                    n: 0,
                    sub: 0,
                    params: 0,
                    points: 0,
                    values: 0,
                    items: content && items <= most_items,
                    markers: marker_total <= most_markers,
                }
            }
            Some(Value::String(text)) => {
                let cursor = Cursor::parse(text).ok_or_else(|| bad("cursor: the nextCursor an earlier page gave, unchanged"))?;
                if cursor.epoch != epoch {
                    return Err((
                        daw_bridge::wire::code::STALE.into(),
                        "the project in the tab changed since the first page: read again".into(),
                    ));
                }
                cursor
            }
            Some(_) => return Err(bad("cursor must be a string")),
        };
        let first = cursor.n == 0 && cursor.sub == 0;
        let mut clock = Clock { started: Instant::now(), reads: 0 };
        let changed = reaper.state_change_count(proj);
        let mut records: Vec<Value> = vec![];
        let mut bytes = 0;
        let mut next: Option<Cursor> = None;
        // Tracks, each with its FX and items.
        'tracks: while cursor.n < track_total {
            let track = if cursor.n == 0 { reaper.master_track(proj) } else { reaper.track_at(proj, cursor.n as i32 - 1) };
            let Some(track) = track else {
                cursor.n += 1;
                cursor.sub = 0;
                continue;
            };
            let key = (cursor.digest, cursor.n, changed, cursor.params, cursor.points, cursor.values);
            let mut work = take_work(&key).unwrap_or_else(|| {
                let budget = Budget {
                    values,
                    content,
                    items: cursor.items,
                    params: cursor.params,
                    points: cursor.points,
                    value_count: cursor.values,
                };
                Work::new(key, budget)
            });
            if !self.read_track(reaper, proj, track, cursor.n, &mut work, &mut clock) {
                keep_work(work);
                next = Some(cursor.clone());
                break 'tracks;
            }
            let stream = work.records.as_deref().unwrap_or_default();
            let mut full = false;
            for record in stream.iter().skip(cursor.sub) {
                let size = super::json_len(record) + 1;
                if !records.is_empty() && (bytes + size > page_bytes || clock.over()) {
                    full = true;
                    break;
                }
                bytes += size;
                records.push(record.clone());
                cursor.sub += 1;
            }
            if full {
                keep_work(work);
                next = Some(cursor.clone());
                break 'tracks;
            }
            cursor.n += 1;
            cursor.sub = 0;
            (cursor.params, cursor.points, cursor.values) = (work.budget.params, work.budget.points, work.budget.value_count);
        }
        // Then the markers and regions, one record each.
        if next.is_none() && cursor.markers {
            while cursor.n - track_total < marker_total {
                let index = cursor.n - track_total;
                if let Some(marker) = reaper.marker(proj, index as i32) {
                    let record = json!({
                        "k": if marker.is_region { "region" } else { "marker" },
                        "id": marker.guid,
                        "n": rows::clip(&marker.name),
                        "o": index,
                        "f": {
                            "position": round(marker.start, 6),
                            "end": if marker.is_region { json!(round(marker.end, 6)) } else { Value::Null },
                            "number": marker.number,
                            "color": marker.color,
                            "lane": marker.lane,
                        },
                    });
                    let size = super::json_len(&record) + 1;
                    if !records.is_empty() && (bytes + size > page_bytes || clock.over()) {
                        next = Some(cursor.clone());
                        break;
                    }
                    bytes += size;
                    records.push(record);
                }
                cursor.n += 1;
            }
        }
        let mut answer = json!({"epoch": epoch, "records": records});
        // (every page says the change count, so a digest the producer edited in the middle of is read again)
        answer["changed"] = reaper.state_change_count(proj).map_or(Value::Null, Value::from);
        if let Some(next) = &next {
            answer["nextCursor"] = next.text().into();
        }
        if first {
            let identity = self.identity(proj);
            answer["project"] = json!({
                "id": self.project_id(reaper, proj, &info),
                "identity": identity,
                "name": rows::clip(&info.name),
                "path": rows::clip(&info.path),
            });
            answer["facts"] = facts(reaper, proj, marker_total);
            answer["summaries"] = json!({"FX": "FX", "item": "items", "marker": "markers", "region": "markers"});
            let mut bounded = Map::new();
            if !cursor.items && content {
                bounded.insert("item".into(), format!("more than {most_items} items: each track's are summed up").into());
            }
            if !cursor.markers {
                bounded.insert("marker".into(), format!("more than {most_markers} markers and regions").into());
                bounded.insert("region".into(), format!("more than {most_markers} markers and regions").into());
            }
            if !content {
                bounded.insert("item".into(), "not read (a quick look)".into());
            }
            answer["bounded"] = Value::Object(bounded);
        }
        if next.is_none() {
            let mut bounded = Map::new();
            if cursor.params >= PARAMS_BUDGET {
                bounded.insert("parameters".into(), format!("more than {PARAMS_BUDGET} parameters: the rest aren't compared").into());
            }
            if cursor.points >= POINTS_BUDGET {
                bounded.insert("points".into(), format!("more than {POINTS_BUDGET} envelope points: the rest aren't compared").into());
            }
            if !bounded.is_empty() {
                let into = answer.as_object_mut().unwrap().entry("bounded").or_insert_with(|| json!({}));
                if let Some(into) = into.as_object_mut() {
                    into.extend(bounded);
                }
            }
        }
        Ok(answer)
    }

    /// Reads a track's records into `work` as far as the page's share of time goes: its envelopes' points, its items,
    /// then its own record, its FX (containers' children after them) and its items' records. True once they're read.
    fn read_track(&self, reaper: &Reaper, proj: Project, track: Track, position: usize, work: &mut Work, clock: &mut Clock) -> bool {
        if work.records.is_some() {
            return true;
        }
        // The automation: every envelope's points and automation items, within the budget.
        let envelope_count = reaper.track_envelope_count(track);
        while !work.points_done && work.envelope < envelope_count {
            let Some(envelope) = reaper.track_envelope(track, work.envelope) else {
                work.envelope += 1;
                continue;
            };
            let count = reaper.envelope_point_count(envelope).max(0);
            if work.point == 0 {
                if work.budget.points + count as usize > POINTS_BUDGET {
                    work.budget.points = POINTS_BUDGET;
                    work.points_read = false;
                    break;
                }
                work.budget.points += count as usize;
                work.points.text(&reaper.envelope_name(envelope)).number(count as f64, 0);
                work.points.text(if reaper.envelope_flag(envelope, "ACTIVE") { "on" } else { "off" });
            }
            while work.point < count {
                if let Some(p) = reaper.env_point(envelope, -1, work.point) {
                    work.points.number(p.time, 6).number(p.value, 6).number(p.shape as f64, 0).number(p.tension, 4);
                }
                work.point += 1;
                if clock.spend() && work.point < count {
                    return false;
                }
            }
            for item in 0..reaper.ai_count(envelope) {
                for key in ["D_POSITION", "D_LENGTH", "D_POOL_ID", "D_STARTOFFS", "D_PLAYRATE", "D_BASELINE", "D_AMPLITUDE", "D_LOOPSRC"] {
                    work.points.number(reaper.ai_value(envelope, item, key), 6);
                }
            }
            work.envelope += 1;
            work.point = 0;
            if clock.over() && work.envelope < envelope_count {
                return false;
            }
        }
        work.points_done = true;
        // The items: each one's place, length and what it plays, summed up on the track either way.
        let item_count = reaper.track_item_count(track).max(0);
        let guid = if position == 0 { "master".to_string() } else { reaper.track_guid(track) };
        while !work.items_done && work.item < item_count {
            let index = work.item;
            work.item += 1;
            let Some(item) = reaper.track_item(track, index) else { continue };
            let item_guid = reaper.item_guid(item);
            let take = reaper.active_take(item);
            let (at, length) = (round(reaper.item_value(item, "D_POSITION"), 6), round(reaper.item_value(item, "D_LENGTH"), 6));
            let mute = reaper.item_value(item, "B_MUTE") != 0.0;
            let mut what = Print::new();
            for parm in ["D_VOL", "D_FADEINLEN", "D_FADEOUTLEN", "B_LOOPSRC", "I_FIXEDLANE", "D_SNAPOFFSET"] {
                what.number(reaper.item_value(item, parm), 6);
            }
            if let Some(take) = take {
                match reaper.take_is_midi(take) {
                    true => what.text(&reaper.midi_hash(take, false).unwrap_or_default()),
                    false => what.text(&reaper.take_file(take).unwrap_or_default()),
                };
                for parm in ["D_STARTOFFS", "D_PLAYRATE", "D_PITCH", "D_VOL", "D_PAN", "B_PPITCH"] {
                    what.number(reaper.take_value(take, parm), 6);
                }
                what.number(reaper.fx_count(FxOwner::Take(take)) as f64, 0);
            }
            let takes = reaper.take_count(item);
            let content_hash = what.number(takes as f64, 0).hex();
            work.item_print.text(&item_guid).number(at, 6).number(length, 6).text(if mute { "m" } else { "" }).text(&content_hash);
            if work.budget.items {
                let take_name = take.map(|t| reaper.take_name(t)).unwrap_or_default();
                work.item_records.push(json!({
                    "k": "item",
                    "id": item_guid,
                    "n": rows::clip(&take_name),
                    "o": index,
                    "p": guid,
                    "f": {
                        "position": at,
                        "length": length,
                        "mute": mute,
                        "takes": takes,
                        "source": rows::source_word(reaper, take),
                        "content": {"contentHash": content_hash},
                    },
                }));
            }
            if clock.spend() && work.item < item_count {
                return false;
            }
        }
        work.items_done = true;
        // The FX, one at a time, a page's share of time at a time (one plug-in's 512 parameters, a chain of containers):
        // the track's own record and its FX's then follow.
        let automated = work.automated.get_or_insert_with(|| automated(reaper, track));
        let left = work.fx_left.get_or_insert_with(|| {
            let owner = FxOwner::Track(track);
            let chain = (0..reaper.fx_count(owner))
                .map(|index| Fx { owner, index })
                .chain((0..reaper.input_fx_count(track)).map(|index| Fx { owner, index: INPUT_FX | index }));
            let mut left: Vec<FxPlace> =
                chain.enumerate().map(|(position, fx)| FxPlace { fx, position, parent: guid.clone(), depth: 0 }).collect();
            left.reverse();
            left
        });
        while let Some(place) = left.pop() {
            let fx_guid = self.fx_record(reaper, proj, &place, automated, &mut work.budget, &mut work.fx_records);
            if place.depth < 8 && reaper.fx_config(place.fx, "container_count").is_some() {
                let children = reaper.fx_children(place.fx).into_iter().enumerate();
                let children: Vec<FxPlace> =
                    children.map(|(position, fx)| FxPlace { fx, position, parent: fx_guid.clone(), depth: place.depth + 1 }).collect();
                left.extend(children.into_iter().rev());
            }
            if clock.spend() && !left.is_empty() {
                return false;
            }
        }
        work.records = Some(self.track_records(reaper, track, position, work));
        true
    }

    /// A track's records once its points, items and FX are read: its own, then its FX (containers' children after them),
    /// then its items.
    fn track_records(&self, reaper: &Reaper, track: Track, position: usize, work: &mut Work) -> Vec<Value> {
        let Work { budget, points, points_read, item_print, item_records, fx_records, automated, .. } = work;
        let master = position == 0;
        let guid = if master { "master".to_string() } else { reaper.track_guid(track) };
        let own_name = reaper.track_string(track, "P_NAME").unwrap_or_default();
        // (REAPER shows an unnamed track by its number)
        let shown = || Some(reaper.track_name(track)).filter(|name| !name.is_empty()).unwrap_or_else(|| format!("Track {position}"));
        let name = if master {
            "Master".to_string()
        } else if own_name.is_empty() {
            shown()
        } else {
            own_name.clone()
        };
        let automated = automated.get_or_insert_with(|| self::automated(reaper, track));
        let value = |parm: &str| reaper.track_value(track, parm);
        let fader = |on: bool, value: Value| if on { Value::Null } else { value };
        // Sends and hardware outputs: where each goes and how.
        let mut sends = Print::new();
        let mut send_count = 0;
        for category in [0, 1] {
            for index in 0..reaper.send_count(track, category) {
                send_count += 1;
                let peer = reaper.send_peer(track, category, index).map(|peer| reaper.track_guid(peer)).unwrap_or_default();
                sends.text(&format!("{category}:{peer}"));
                for parm in ["D_VOL", "D_PAN", "B_MUTE", "I_SENDMODE", "I_SRCCHAN", "I_DSTCHAN", "B_PHASE", "B_MONO", "I_MIDIFLAGS"] {
                    sends.number(reaper.send_value(track, category, index, parm), 4);
                }
            }
        }
        let envelope_count = reaper.track_envelope_count(track);
        let item_count = reaper.track_item_count(track).max(0);
        let chain_count = reaper.fx_count(FxOwner::Track(track)).max(0) + reaper.input_fx_count(track).max(0);
        let mut track_record = json!({
            "k": "track",
            "id": guid,
            "n": rows::clip(&name),
            "o": position,
            "f": {
                "mixer": {
                    "volume": fader(automated.volume, json!(rows::db(value("D_VOL")))),
                    "pan": fader(automated.pan, json!(round(value("D_PAN"), 3))),
                    "width": fader(automated.width, json!(round(value("D_WIDTH"), 3))),
                    "mute": fader(automated.mute, json!(value("B_MUTE") != 0.0)),
                    "solo": value("I_SOLO") != 0.0,
                    "phase": value("B_PHASE") != 0.0,
                },
                "color": reaper.track_color(track).and_then(|c| reaper.color_hex(c)).unwrap_or_default(),
                "input": {"arm": value("I_RECARM") != 0.0, "monitor": value("I_RECMON") as i64, "recInput": value("I_RECINPUT") as i64},
                "sends": {"count": send_count, "sendsHash": sends.hex()},
                "FX": {"count": chain_count},
                "items": {"count": item_count, "itemsHash": if budget.content { json!(item_print.hex()) } else { Value::Null }},
                "automation": {"count": envelope_count, "pointsHash": if *points_read { json!(points.hex()) } else { Value::Null }},
            },
        });
        if !master {
            track_record["f"]["folder"] = json!({"depth": reaper.track_depth(track), "folder": value("I_FOLDERDEPTH") as i64});
        } else {
            track_record["f"].as_object_mut().unwrap().remove("input");
        }
        if own_name.is_empty() && !master {
            track_record["dn"] = true.into();
        }
        let mut records = vec![track_record];
        records.append(fx_records);
        records.append(item_records);
        records
    }

    /// An FX's record (its container's children come after it, read one by one): its GUID.
    fn fx_record(
        &self,
        reaper: &Reaper,
        proj: Project,
        place: &FxPlace,
        automated: &Automated,
        budget: &mut Budget,
        records: &mut Vec<Value>,
    ) -> String {
        let FxPlace { fx, position, parent, .. } = place;
        let fx = *fx;
        let fx_guid = reaper.fx_guid(fx);
        let renamed = reaper.fx_config(fx, "renamed_name").unwrap_or_default();
        let plugin = reaper.fx_name(fx);
        // (as Kumi's titles name it: "ReaComp", not "VST: ReaComp (Cockos)")
        let name = if renamed.is_empty() { crate::changes::short_name(&plugin) } else { renamed };
        let count = reaper.fx_param_count(fx).max(0);
        let read = count.min(PARAMS_PER_FX);
        let mut print = Print::new();
        let mut values = vec![];
        let hashed = budget.params + read as usize <= PARAMS_BUDGET;
        let listed = budget.values && budget.value_count + read as usize <= VALUES_BUDGET;
        if hashed || listed {
            for param in 0..read {
                // (an automated parameter follows its envelope, which has its own fingerprint)
                let skip = automated.params.contains(&(fx.index, param));
                let normalized = reaper.param_normalized(fx, param);
                if hashed && !skip {
                    print.number(normalized, 4);
                }
                if listed {
                    values.push(if skip { Value::Null } else { json!(round(normalized, 6)) });
                }
            }
        }
        if hashed {
            budget.params += read as usize;
        } else {
            budget.params = PARAMS_BUDGET;
        }
        if listed {
            budget.value_count += read as usize;
        }
        let mut record = json!({
            "k": "FX",
            "id": fx_guid,
            "n": rows::clip(&name),
            "o": position,
            "p": parent,
            "f": {
                // (what to add again to make it: the plug-in's own name, whatever the producer renamed it)
                "plugin": rows::clip(&plugin),
                "enabled": reaper.fx_enabled(fx),
                "offline": reaper.fx_offline(fx),
                "parameters": {"count": count, "parametersHash": if hashed { json!(print.hex()) } else { Value::Null }},
            },
        });
        if fx.index & INPUT_FX != 0 {
            record["f"]["input"] = true.into();
        }
        if listed {
            record["v"] = values.into();
        }
        if budget.values {
            record["r"] = refs::fx_ref(&fx_guid).into();
        }
        self.registry.borrow_mut().note_fx(proj, &fx_guid, fx);
        records.push(record);
        fx_guid
    }
}

/// The project's own values: its tempo and time signature at the start, the tempo map, and (summed up) its markers.
fn facts(reaper: &Reaper, proj: Project, marker_total: usize) -> Value {
    let start = reaper.time_signature_at(proj, 0.0);
    let mut tempo_map = Print::new();
    let tempo_count = reaper.tempo_marker_count(proj).max(0);
    for index in 0..tempo_count {
        if let Some(marker) = reaper.tempo_marker(proj, index) {
            tempo_map.number(marker.time, 6).number(marker.bpm, 4).number(marker.num as f64, 0).number(marker.denom as f64, 0);
            tempo_map.text(if marker.linear { "l" } else { "" });
        }
    }
    let mut markers = Print::new();
    for index in 0..marker_total.min(MOST_MARKERS * 10) as i32 {
        if let Some(marker) = reaper.marker(proj, index) {
            markers.text(&marker.guid).text(&marker.name).number(marker.start, 6).number(marker.end, 6).number(marker.color as f64, 0);
        }
    }
    json!({
        "tempo": start.map(|s| json!(round(s.bpm, 3))).unwrap_or(Value::Null),
        "timeSignature": start.map(|s| json!([s.num, s.denom])).unwrap_or(Value::Null),
        "tempoMap": {"markers": tempo_count, "tempoMapHash": tempo_map.hex()},
        "markers": {"count": marker_total, "markersHash": markers.hex()},
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reaper::fake;

    fn page(reads: &Reads, params: Value) -> Answer {
        let api = fake::api();
        reads.digest(&Reaper::new(&api), &params)
    }

    /// Every page of a digest: the first page, and all the records.
    fn whole(reads: &Reads, values: bool, content: bool) -> (Value, Vec<Value>) {
        let mut first = page(reads, json!({"values": values, "content": content})).unwrap();
        let mut records = first["records"].as_array().unwrap().clone();
        let mut cursor = first["nextCursor"].clone();
        let mut pages = 1;
        while let Some(text) = cursor.as_str() {
            let next = page(reads, json!({"values": values, "content": content, "cursor": text})).unwrap();
            assert!(next.get("project").is_none(), "only the first page names the project");
            records.extend(next["records"].as_array().unwrap().clone());
            cursor = next["nextCursor"].clone();
            pages += 1;
            assert!(pages < 1000);
        }
        first["pages"] = pages.into();
        (first, records)
    }

    fn find<'a>(records: &'a [Value], id: &str) -> &'a Value {
        records.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("{id} in {records:?}"))
    }

    #[test]
    fn a_cursor_reads_back_as_it_was_written() {
        let cursor =
            Cursor { epoch: "1f-3".into(), digest: 4, n: 7, sub: 2, params: 900, points: 12, values: 0, items: true, markers: false };
        assert_eq!(Cursor::parse(&cursor.text()), Some(cursor));
        for bad in [
            "",
            "g2.1f-3.4.7",
            "g1.1f-3.7.2.900.12.0.1.0",
            "g2.1f-3.4.x.2.900.12.0.1.0",
            "g2..4.7.2.900.12.0.1.0",
            "g2.1f-3.x.7.2.900.12.0.1.0",
        ] {
            assert_eq!(Cursor::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_project_is_its_tracks_with_their_fx_and_items_then_its_markers_each_by_guid() {
        fake::lab_project();
        let reads = Reads::default();
        let (first, records) = whole(&reads, false, true);
        assert_eq!(first["project"]["path"], "/lab/Projects/Song.rpp");
        assert_eq!(first["project"]["identity"], reads.identity(Project::first()));
        assert_eq!(first["facts"]["tempo"], 120.0);
        assert_eq!(first["facts"]["timeSignature"], json!([4, 4]));
        assert_eq!(first["facts"]["markers"]["count"], 2);
        assert_eq!(first["summaries"]["item"], "items");
        assert_eq!(first["bounded"], json!({}));
        assert_eq!(first["changed"], 11);
        let ids: Vec<&str> = records.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            [
                "master",
                "{AAAA}",
                "{FX-EQ}",
                "{FX-C}",
                "{FX-COMP}",
                "{FX-C2}",
                "{FX-DLY}",
                "{BBBB}",
                "{ITEM-K1}",
                "{ITEM-K2}",
                "{CCCC}",
                "{ITEM-S1}",
                "{DDDD}",
                "{FX-VERB}",
                "{MARK-1}",
                "{REG-1}"
            ]
        );
        let drums = find(&records, "{AAAA}");
        assert_eq!((drums["k"].clone(), drums["n"].clone(), drums["o"].clone()), (json!("track"), json!("Drums"), json!(1)));
        let eq = find(&records, "{FX-EQ}");
        assert_eq!((eq["n"].clone(), eq["f"]["plugin"].clone()), (json!("ReaEQ"), json!("VST3: ReaEQ (Cockos)")), "named as the titles do");
        // Its volume follows its envelope, so the envelope's fingerprint stands for it.
        assert_eq!(drums["f"]["mixer"]["volume"], Value::Null);
        assert_eq!(drums["f"]["mixer"]["pan"], -0.25);
        assert_eq!(drums["f"]["folder"], json!({"depth": 0, "folder": 1}));
        assert_eq!(drums["f"]["color"], "#ff0000");
        assert_eq!(drums["f"]["sends"]["count"], 1);
        assert_eq!(drums["f"]["FX"]["count"], 2);
        assert!(drums["f"]["automation"]["pointsHash"].is_string());
        assert_eq!(find(&records, "master")["n"], "Master");
        // Containers hold their children.
        assert_eq!(find(&records, "{FX-COMP}")["p"], "{FX-C}");
        assert_eq!(find(&records, "{FX-DLY}")["p"], "{FX-C2}");
        assert_eq!(find(&records, "{FX-C}")["p"], "{AAAA}");
        let kick = find(&records, "{ITEM-K2}");
        assert_eq!(
            (kick["p"].clone(), kick["f"]["position"].clone(), kick["f"]["source"].clone()),
            (json!("{BBBB}"), json!(2.0), json!("wav"))
        );
        let verse = find(&records, "{REG-1}");
        assert_eq!((verse["k"].clone(), verse["n"].clone(), verse["f"]["end"].clone()), (json!("region"), json!("Verse"), json!(16.0)));
        // No values or refs unless asked (watch_me): catch-up's snapshots stay small.
        assert!(records.iter().all(|r| r.get("v").is_none() && r.get("r").is_none()));
    }

    #[test]
    fn watching_lists_parameter_values_and_refs_and_a_quick_look_leaves_items_out() {
        fake::lab_project();
        let reads = Reads::default();
        let (first, records) = whole(&reads, true, false);
        let eq = find(&records, "{FX-EQ}");
        // Band 1 Gain is automated: its value follows the envelope, so it isn't one the producer set.
        assert_eq!(eq["v"], json!([0.25, null, 1.0]));
        assert_eq!(eq["r"], "r:fx:{FX-EQ}");
        assert!(records.iter().all(|r| r["k"] != "item"));
        assert_eq!(find(&records, "{BBBB}")["f"]["items"], json!({"count": 2, "itemsHash": null}));
        assert_eq!(first["bounded"]["item"], "not read (a quick look)");
    }

    #[test]
    fn pages_of_any_size_add_up_to_the_same_digest_and_another_projects_cursor_is_refused() {
        fake::lab_project();
        let reads = Reads::default();
        let (_, whole_records) = whole(&reads, true, true);
        TEST_LIMITS.with(|limits| limits.set(Some((400, MOST_ITEMS, MOST_MARKERS))));
        let (first, paged) = whole(&reads, true, true);
        TEST_LIMITS.with(|limits| limits.set(None));
        assert!(first["pages"].as_u64().unwrap() > 5, "{}", first["pages"]);
        assert_eq!(paged, whole_records);
        let cursor = page(&reads, json!({})).unwrap()["nextCursor"].clone();
        assert!(cursor.is_null(), "the lab project fits one page");
        TEST_LIMITS.with(|limits| limits.set(Some((400, MOST_ITEMS, MOST_MARKERS))));
        let cursor = page(&reads, json!({})).unwrap()["nextCursor"].as_str().unwrap().to_string();
        reads.bump_epoch();
        assert_eq!(page(&reads, json!({"cursor": cursor})).unwrap_err().0, "stale");
        assert_eq!(page(&reads, json!({"cursor": "nonsense"})).unwrap_err().0, "bad-params");
        TEST_LIMITS.with(|limits| limits.set(None));
    }

    #[test]
    fn a_big_project_sums_its_items_and_markers_up_instead_of_listing_them() {
        fake::lab_project();
        let reads = Reads::default();
        TEST_LIMITS.with(|limits| limits.set(Some((PAGE_BYTES, 2, 1))));
        let (first, records) = whole(&reads, false, true);
        TEST_LIMITS.with(|limits| limits.set(None));
        assert!(records.iter().all(|r| !["item", "marker", "region"].contains(&r["k"].as_str().unwrap())), "{records:?}");
        assert!(first["bounded"]["item"].as_str().unwrap().contains("more than 2 items"));
        assert!(first["bounded"]["marker"].is_string() && first["bounded"]["region"].is_string());
        // Summed up all the same: a change to them still shows on their track, and in the project's markers.
        assert!(find(&records, "{BBBB}")["f"]["items"]["itemsHash"].is_string());
        assert_eq!(first["facts"]["markers"]["count"], 2);
    }

    /// 200 tracks of 5 FX of 64 parameters each and 20 items each (4,000 items: listed one by one).
    fn big_project() {
        use crate::reaper::fake::{FakeFx, FakeItem, FakeParam, FakeTake, FakeTrack};
        fake::lab_project();
        fake::with(|s| {
            s.tracks = (0..200)
                .map(|t| {
                    let mut track = FakeTrack::new(&format!("{{T{t:04}}}"), &format!("Track {t}"));
                    track.fx = (0..5)
                        .map(|f| {
                            let mut fx = FakeFx::new(&format!("{{F{t:04}-{f}}}"), "VST: ReaEQ (Cockos)");
                            fx.params = (0..64).map(|p| FakeParam::new(&format!("P{p}"), (p as f64) / 64.0, "x")).collect();
                            fx
                        })
                        .collect();
                    track.items = (0..20)
                        .map(|i| {
                            let take = FakeTake::wave(&format!("{{K{t:04}-{i}}}"), "loop.wav");
                            FakeItem::new(&format!("{{I{t:04}-{i}}}"), i as f64 * 2.0, 2.0, vec![take])
                        })
                        .collect();
                    track
                })
                .collect();
        });
    }

    #[test]
    fn a_big_projects_digest_comes_in_pages_inside_the_frame_bound() {
        big_project();
        let reads = Reads::default();
        let began = Instant::now();
        let mut answer = page(&reads, json!({})).unwrap();
        let (mut pages, mut records, mut most) = (1, 0, 0);
        loop {
            most = most.max(super::super::json_len(&answer));
            records += answer["records"].as_array().unwrap().len();
            let Some(cursor) = answer["nextCursor"].as_str().map(str::to_owned) else { break };
            answer = page(&reads, json!({"cursor": cursor})).unwrap();
            pages += 1;
        }
        eprintln!("digest of 200 tracks, 1,000 FX, 64,000 parameters, 4,000 items: {pages} pages, {records} records, largest page {most} bytes, {:?}", began.elapsed());
        assert_eq!(records, 1 + 200 + 1000 + 4000 + 2, "the master, the tracks, their FX and items, the lab's two markers");
        assert!(most <= super::super::ANSWER_BYTES, "every page inside the frame bound: {most}");
        assert!(pages > 1);
    }

    /// The lab project with Drums holding 600 items and a pan lane of 3,000 points.
    fn one_big_track() {
        use crate::reaper::fake::{FakeEnvelope, FakeItem, FakeTake};
        fake::lab_project();
        fake::with(|s| {
            let drums = &mut s.tracks[0];
            drums.items = (0..600)
                .map(|i| FakeItem::new(&format!("{{I{i:04}}}"), i as f64, 1.0, vec![FakeTake::wave(&format!("{{K{i:04}}}"), "hit.wav")]))
                .collect();
            drums.envelopes.push(FakeEnvelope::own("Pan", "<PANENV2", 3000));
        });
    }

    // Review R9: a track of many items or points is read over several pages, a page's share at a time, and once for all
    // of them.
    #[test]
    fn one_big_track_is_read_a_share_at_a_time_and_once_however_many_pages_it_takes() {
        one_big_track();
        let reads = Reads::default();
        let calls = || fake::with(|s| s.calls);
        let before = calls();
        let (_, whole_records) = whole(&reads, false, true);
        let once = calls() - before;
        // Pages of 2 KB: Drums's records take dozens of pages, and its items and points are read once for them all.
        TEST_LIMITS.with(|limits| limits.set(Some((2048, MOST_ITEMS, MOST_MARKERS))));
        let before = calls();
        let (first, paged) = whole(&reads, false, true);
        let paged_calls = calls() - before;
        TEST_LIMITS.with(|limits| limits.set(None));
        assert!(first["pages"].as_u64().unwrap() > 50, "{}", first["pages"]);
        assert_eq!(paged, whole_records);
        assert!(paged_calls < once * 2, "{paged_calls} REAPER calls in pages, {once} in one go");
        // A page's share of the main thread (here 100 points or items) holds inside the track too.
        TEST_READS.with(|reads| reads.set(Some(100)));
        let mut answer = page(&reads, json!({})).unwrap();
        let (mut pages, mut most, mut records) = (1, 0, answer["records"].as_array().unwrap().clone());
        while let Some(cursor) = answer["nextCursor"].as_str().map(str::to_owned) {
            let before = calls();
            answer = page(&reads, json!({"cursor": cursor})).unwrap();
            most = most.max(calls() - before);
            records.extend(answer["records"].as_array().unwrap().clone());
            pages += 1;
        }
        TEST_READS.with(|reads| reads.set(None));
        assert_eq!(records, whole_records);
        assert!(pages >= 3600 / 100, "{pages} pages");
        assert!(most < once / 4, "the most REAPER calls on one page, {most}, against {once} for the whole track");
    }

    // A track's FX were read in one go, every parameter of every plug-in and container child: a page held REAPER's main
    // thread far past its share.
    #[test]
    fn a_tracks_fx_are_read_a_share_at_a_time_too() {
        use crate::reaper::fake::{FakeFx, FakeParam};
        fake::lab_project();
        fake::with(|s| {
            let params: Vec<FakeParam> = (0..300).map(|p| FakeParam::new(&format!("P{p}"), 0.5, "50")).collect();
            let fx = |n: usize| FakeFx { params: params.clone(), ..FakeFx::new(&format!("{{FX-BIG{n:02}}}"), "VST: Big (Kumi)") };
            let drums = &mut s.tracks[0];
            drums.fx = (0..30).map(fx).collect();
            drums.fx.push(FakeFx::container("{FX-BOX}", (30..40).map(fx).collect()));
        });
        let reads = Reads::default();
        let calls = || fake::with(|s| s.calls);
        let before = calls();
        let (_, whole_records) = whole(&reads, false, true);
        let once = calls() - before;
        // A page's share of the main thread (here 2 points, items or FX) holds inside the FX, a container's children too.
        TEST_READS.with(|reads| reads.set(Some(2)));
        let mut answer = page(&reads, json!({})).unwrap();
        let (mut pages, mut most, mut records) = (1, 0, answer["records"].as_array().unwrap().clone());
        while let Some(cursor) = answer["nextCursor"].as_str().map(str::to_owned) {
            let before = calls();
            answer = page(&reads, json!({"cursor": cursor})).unwrap();
            most = most.max(calls() - before);
            records.extend(answer["records"].as_array().unwrap().clone());
            pages += 1;
        }
        TEST_READS.with(|reads| reads.set(None));
        assert_eq!(records, whole_records, "the same records, in the same order");
        assert!(pages >= 41 / 2, "{pages} pages");
        assert!(most < once / 4, "the most REAPER calls on one page, {most}, against {once} for the whole project");
    }

    #[test]
    fn what_the_producer_changes_moves_only_its_own_facet() {
        fake::lab_project();
        let reads = Reads::default();
        let (_, before) = whole(&reads, false, true);
        fake::with(|s| {
            s.tracks[1].volume = 0.25;
            s.tracks[2].name = "Snare Top".into();
            s.tracks[0].fx[0].params[0].value = 0.5;
            s.tracks[1].items[1].position = 3.0;
            s.markers[0].name = "Drop 2".into();
        });
        let (_, after) = whole(&reads, false, true);
        assert_eq!(before.len(), after.len());
        let mut moved = vec![];
        for (old, new) in before.iter().zip(&after) {
            if old != new {
                moved.push(new["id"].as_str().unwrap().to_string());
            }
        }
        // (the Drums track holds the EQ, but an FX is a record of its own: the track didn't change)
        assert_eq!(moved, ["{FX-EQ}", "{BBBB}", "{ITEM-K2}", "{CCCC}", "{MARK-1}"]);
        let (old, new) = (find(&before, "{BBBB}"), find(&after, "{BBBB}"));
        assert_ne!(old["f"]["mixer"]["volume"], new["f"]["mixer"]["volume"]);
        assert_ne!(old["f"]["items"]["itemsHash"], new["f"]["items"]["itemsHash"], "the track sums its items up too");
        assert_ne!(
            find(&before, "{FX-EQ}")["f"]["parameters"]["parametersHash"],
            find(&after, "{FX-EQ}")["f"]["parameters"]["parametersHash"]
        );
        assert_eq!(find(&after, "{CCCC}")["n"], "Snare Top");
    }
}
