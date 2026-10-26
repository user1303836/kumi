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
//!
//! Kept across digests: the parameter values the fingerprints and `values` are made of, the bulk of a digest's REAPER
//! calls, per FX, while the tab's epoch and REAPER's count of the project's state changes stay what they were (watch_me's
//! next look at a project nobody touched): a parameter write moves the count, in REAPER or through its API, inside an
//! undo block or not (7.82). Nothing else is kept: a bare API write outside an undo block (an extension's: a track's mute,
//! name or volume set, an item or a marker moved, an envelope point) moves neither the count nor the undo history, so the
//! rest is read every time. (What the producer does in REAPER, a mute or solo included, moves the count.) The kept values
//! are read again after `KEPT_FOR` whatever the count says: a parameter moved by what makes no undo point (a control a
//! plug-in's own hand moves, say) shows within it.
//!
//! Kept too: the pages of the last complete digest of each kind (`values`, `content`), as they were answered, while the
//! epoch, the count and this extension's own count of writes (`reaper::writes`: its listening's scratch work writes
//! outside undo blocks, and a plan's changes write before its block ends) stay what they were when it was read, and for
//! `KEPT_FOR` at most. watch_me's next look at a project nobody touched is then no REAPER calls but the count's.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::{active, bad, Reads};
use crate::queue::Answer;
use crate::reaper::{Fx, FxOwner, Marker, Project, Reaper, Track, INPUT_FX};
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
/// Parameter values kept across digests, at most (about 1.6 MB): past it, the rest is read each time.
const PARAMS_KEPT: usize = 200_000;
/// How long parameter values and pages are kept at most: read again after this, whatever the count says.
const KEPT_FOR: Duration = Duration::from_secs(30);
/// Pages kept of one digest, at most (as JSON): past it, the digest is read every time.
const PAGES_KEPT: usize = 4 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    /// A test's own page size, item and marker limits: (page bytes, most items, most markers).
    pub static TEST_LIMITS: std::cell::Cell<Option<(usize, usize, usize)>> = const { std::cell::Cell::new(None) };
    /// A test's page share of the main thread as a number of points and items read, in place of `PAGE_TIME`.
    pub static TEST_READS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    /// A test's own bound on the parameter values kept, in place of `PARAMS_KEPT`.
    pub static TEST_PARAMS_KEPT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    /// A test's own time bound on the parameter values kept, in place of `KEPT_FOR`.
    pub static TEST_KEPT_FOR: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
    /// A test's own bound on the pages kept, in place of `PAGES_KEPT`.
    pub static TEST_PAGES_KEPT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn pages_kept() -> usize {
    #[cfg(test)]
    if let Some(most) = TEST_PAGES_KEPT.with(|most| most.get()) {
        return most;
    }
    PAGES_KEPT
}

fn params_kept() -> usize {
    #[cfg(test)]
    if let Some(most) = TEST_PARAMS_KEPT.with(|most| most.get()) {
        return most;
    }
    PARAMS_KEPT
}

fn kept_for() -> Duration {
    #[cfg(test)]
    if let Some(bound) = TEST_KEPT_FOR.with(|bound| bound.get()) {
        return bound;
    }
    KEPT_FOR
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

thread_local! {
    /// The text `Print::number` writes a number into, one buffer for them all (REAPER's main thread prints): a digest
    /// prints hundreds of thousands of numbers.
    static NUMBER: RefCell<String> = const { RefCell::new(String::new()) };
}

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
        NUMBER.with_borrow_mut(|text| {
            text.clear();
            let _ = write!(text, "{:.*}", places as usize, if value.is_finite() { value } else { 0.0 });
            // What reads as zero is zero, from either side: −0, or the −3.5e-18 a pitch nudged up three cents and down
            // three is left at (REAPER's own nudges, lab 7.82; down and up leaves +3.5e-18).
            let zero = text.strip_prefix('-').filter(|digits| digits.bytes().all(|b| b == b'0' || b == b'.'));
            self.text(zero.unwrap_or(text));
        });
        self
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

/// What a digest reads under: REAPER and the project in front, the tab's epoch and REAPER's count of the project's state
/// changes, which the parameter values kept across digests are good for (none when REAPER has no count: nothing is kept).
struct Under<'a, 'r> {
    reaper: &'a Reaper<'r>,
    proj: Project,
    epoch: &'a str,
    changed: Option<i32>,
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

/// Parameter values as they were read under one epoch and state change count, by FX GUID (their first `PARAMS_PER_FX`,
/// normalized): a parameter edit, in REAPER or through its API, moves the count (7.82), so what was read under it
/// stands until it moves, or `KEPT_FOR` passes since the first was kept.
#[derive(Default)]
struct KeptParams {
    under: Option<(String, i32, Instant)>,
    by_fx: HashMap<String, Vec<f64>>,
    total: usize,
}

impl KeptParams {
    fn is_under(&self, under: &Under, changed: i32) -> bool {
        self.under.as_ref().is_some_and(|(epoch, count, since)| epoch == under.epoch && *count == changed && since.elapsed() < kept_for())
    }
}

/// What a digest was read under: the tab's epoch, REAPER's count of the project's state changes and this extension's
/// count of its own writes. While all three stay, the project is as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Standing {
    epoch: String,
    changed: i32,
    writes: u64,
}

/// The pages of one digest as they were answered, by the cursor each was asked with (None for the first), while it is
/// read and once complete: a complete one is answered again, page for page, while what it was read under stands and
/// `KEPT_FOR` hasn't passed.
struct KeptPages {
    standing: Standing,
    /// The digest's serial: a page of another digest of the same kind isn't one of these.
    digest: u64,
    since: Instant,
    pages: HashMap<Option<String>, Value>,
    bytes: usize,
    complete: bool,
}

/// The markers and regions a digest's first page read, by their index, kept for the pages that list them.
struct KeptMarkers {
    /// The digest, and the project's change count they were read under.
    key: (u64, Option<i32>),
    markers: Vec<Option<Marker>>,
}

#[derive(Default)]
struct Kept {
    /// The serial the next digest takes.
    next: u64,
    works: VecDeque<Work>,
    markers: VecDeque<KeptMarkers>,
    params: KeptParams,
    /// The last digest of each kind (`values`, `content`).
    pages: HashMap<(bool, bool), KeptPages>,
}

thread_local! {
    /// The tracks read part way, or read for their later pages, and the parameter values kept (REAPER's main thread reads
    /// them all).
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

/// An FX's first `read` parameters' values as they were read under this epoch and count, if kept (a plug-in whose
/// parameters changed in number is read again).
fn kept_params(under: &Under, guid: &str, read: usize) -> Option<Vec<f64>> {
    let changed = under.changed?;
    KEPT.with(|kept| {
        let kept = kept.borrow();
        if !kept.params.is_under(under, changed) {
            return None;
        }
        kept.params.by_fx.get(guid).filter(|values| values.len() == read).cloned()
    })
}

/// Keeps an FX's parameter values as read under this epoch and count, up to `PARAMS_KEPT` of them in all: a count (or an
/// epoch) that moved, or `KEPT_FOR` gone by, forgets what was kept under the last.
fn keep_params(under: &Under, guid: &str, values: &[f64]) {
    let Some(changed) = under.changed else { return };
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        if !kept.params.is_under(under, changed) {
            kept.params = KeptParams { under: Some((under.epoch.to_string(), changed, Instant::now())), ..KeptParams::default() };
        }
        let params = &mut kept.params;
        let before = params.by_fx.get(guid).map_or(0, Vec::len);
        if params.total - before + values.len() > params_kept() {
            return;
        }
        params.total = params.total - before + values.len();
        params.by_fx.insert(guid.to_string(), values.to_vec());
    })
}

/// Forgets the parameter values and the pages kept: the epoch moved (a tab switch, another project in the tab).
pub(super) fn forget() {
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        kept.params = KeptParams::default();
        kept.pages.clear();
    });
}

/// A page of the complete digest of this kind kept, answered as it was, when what it was read under stands and the bound
/// hasn't passed: for the first page (no cursor) or a page its own cursors lead to.
fn kept_page(kind: (bool, bool), standing: &Standing, cursor: Option<&str>) -> Option<Value> {
    KEPT.with(|kept| {
        let kept = kept.borrow();
        let pages = kept.pages.get(&kind)?;
        if !pages.complete || pages.standing != *standing || pages.since.elapsed() >= kept_for() {
            return None;
        }
        pages.pages.get(&cursor.map(str::to_owned)).cloned()
    })
}

/// Keeps a page of the digest being read: the first page begins a new keeping of its kind (the one before, complete or
/// not, goes); a later page of the same digest under the same standing joins it; `last` completes it. A page of another
/// digest, under another standing (the project moved between pages), or past `PAGES_KEPT` of them ends the keeping.
fn keep_page(kind: (bool, bool), standing: &Standing, digest: u64, cursor: Option<String>, answer: &Value, bytes: usize, last: bool) {
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        if cursor.is_none() {
            let fresh =
                KeptPages { standing: standing.clone(), digest, since: Instant::now(), pages: HashMap::new(), bytes: 0, complete: false };
            kept.pages.insert(kind, fresh);
        }
        let Some(pages) = kept.pages.get_mut(&kind) else { return };
        if pages.complete || pages.digest != digest || pages.standing != *standing {
            kept.pages.remove(&kind);
            return;
        }
        pages.bytes += bytes;
        if pages.bytes > pages_kept() {
            kept.pages.remove(&kind);
            return;
        }
        pages.pages.insert(cursor, answer.clone());
        pages.complete = last;
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

/// Keeps the markers and regions a digest read, for its next page.
fn keep_markers(key: (u64, Option<i32>), markers: Vec<Option<Marker>>) {
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        kept.markers.push_back(KeptMarkers { key, markers });
        while kept.markers.len() > KEPT_WORK {
            kept.markers.pop_front();
        }
    })
}

/// The markers and regions kept for this digest, taken out, if they were read under this change count.
fn take_markers(key: (u64, Option<i32>)) -> Option<Vec<Option<Marker>>> {
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        let at = kept.markers.iter().position(|kept| kept.key.0 == key.0)?;
        kept.markers.remove(at).filter(|kept| kept.key == key).map(|kept| kept.markers)
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
        let cursor_text = match params.get("cursor") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => return Err(bad("cursor must be a string")),
        };
        let kind = (values, content);
        let changed = reaper.state_change_count(proj);
        // Read under what: nothing moved since the last complete digest of this kind, and its page is answered again.
        let standing = changed.map(|changed| Standing { epoch: epoch.clone(), changed, writes: crate::reaper::writes() });
        if let Some(page) = standing.as_ref().and_then(|standing| kept_page(kind, standing, cursor_text.as_deref())) {
            return Ok(page);
        }
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
        // The markers and regions, read once a digest: its first page fingerprints them in the facts, and the pages that
        // list them take what it read while the change count stays (it moved: they're read again).
        let markers = if first {
            Some((0..marker_total.min(MOST_MARKERS * 10)).map(|index| reaper.marker(proj, index as i32)).collect::<Vec<_>>())
        } else {
            take_markers((cursor.digest, changed))
        };
        let mut clock = Clock { started: Instant::now(), reads: 0 };
        let under = Under { reaper, proj, epoch: &epoch, changed };
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
            if !self.read_track(&under, track, cursor.n, &mut work, &mut clock) {
                keep_work(work);
                next = Some(cursor.clone());
                break 'tracks;
            }
            // How many of the track's records this page has room for; then they move into it (a track read whole into one
            // page, the usual case, has nothing cloned), or, for a track that goes on into the next page, are copied out
            // and the rest stay with its work.
            let stream = work.records.take().unwrap_or_default();
            let mut fit = 0;
            let mut full = false;
            for record in stream.iter().skip(cursor.sub) {
                let size = super::json_len(record) + 1;
                // (a page's first record always goes in)
                if (!records.is_empty() || fit > 0) && (bytes + size > page_bytes || clock.over()) {
                    full = true;
                    break;
                }
                bytes += size;
                fit += 1;
            }
            if full {
                records.extend(stream.iter().skip(cursor.sub).take(fit).cloned());
                cursor.sub += fit;
                work.records = Some(stream);
                keep_work(work);
                next = Some(cursor.clone());
                break 'tracks;
            }
            records.extend(stream.into_iter().skip(cursor.sub));
            cursor.n += 1;
            cursor.sub = 0;
            (cursor.params, cursor.points, cursor.values) = (work.budget.params, work.budget.points, work.budget.value_count);
        }
        // Then the markers and regions, one record each.
        if next.is_none() && cursor.markers {
            while cursor.n - track_total < marker_total {
                let index = cursor.n - track_total;
                let read;
                let marker = match markers.as_ref().and_then(|markers| markers.get(index)) {
                    Some(marker) => marker.as_ref(),
                    None => {
                        read = reaper.marker(proj, index as i32);
                        read.as_ref()
                    }
                };
                if let Some(marker) = marker {
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
            answer["facts"] = facts(reaper, proj, marker_total, markers.as_deref().unwrap_or_default());
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
        if let Some(standing) = &standing {
            keep_page(kind, standing, cursor.digest, cursor_text, &answer, bytes, next.is_none());
        }
        // (for the pages still to come that list them)
        if let Some(markers) = markers.filter(|_| next.is_some() && cursor.markers) {
            keep_markers((cursor.digest, changed), markers);
        }
        Ok(answer)
    }

    /// Reads a track's records into `work` as far as the page's share of time goes: its envelopes' points, its items,
    /// then its own record, its FX (containers' children after them) and its items' records. True once they're read.
    fn read_track(&self, under: &Under, track: Track, position: usize, work: &mut Work, clock: &mut Clock) -> bool {
        let reaper = under.reaper;
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
            let fx_guid = self.fx_record(under, &place, automated, &mut work.budget, &mut work.fx_records);
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
    fn fx_record(&self, under: &Under, place: &FxPlace, automated: &Automated, budget: &mut Budget, records: &mut Vec<Value>) -> String {
        let (reaper, proj) = (under.reaper, under.proj);
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
            // The sweep that costs: kept while the count stays what it was, read again once it moved. (An FX REAPER gives
            // no GUID for is read every time: one key for them all would hand one's values to another.)
            let kept = if fx_guid.is_empty() { None } else { kept_params(under, &fx_guid, read as usize) };
            let normalized = kept.unwrap_or_else(|| {
                let normalized: Vec<f64> = (0..read).map(|param| reaper.param_normalized(fx, param)).collect();
                if !fx_guid.is_empty() {
                    keep_params(under, &fx_guid, &normalized);
                }
                normalized
            });
            for (param, &normalized) in (0..).zip(&normalized) {
                // (an automated parameter follows its envelope, which has its own fingerprint)
                let skip = automated.params.contains(&(fx.index, param));
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

/// The project's own values: its tempo and time signature at the start, the tempo map, and (summed up) its markers, as
/// the digest read them.
fn facts(reaper: &Reaper, proj: Project, marker_total: usize, markers: &[Option<Marker>]) -> Value {
    let start = reaper.time_signature_at(proj, 0.0);
    let mut tempo_map = Print::new();
    let tempo_count = reaper.tempo_marker_count(proj).max(0);
    for index in 0..tempo_count {
        if let Some(marker) = reaper.tempo_marker(proj, index) {
            tempo_map.number(marker.time, 6).number(marker.bpm, 4).number(marker.num as f64, 0).number(marker.denom as f64, 0);
            tempo_map.text(if marker.linear { "l" } else { "" });
        }
    }
    let mut print = Print::new();
    for marker in markers.iter().flatten() {
        print.text(&marker.guid).text(&marker.name).number(marker.start, 6).number(marker.end, 6).number(marker.color as f64, 0);
    }
    json!({
        "tempo": start.map(|s| json!(round(s.bpm, 3))).unwrap_or(Value::Null),
        "timeSignature": start.map(|s| json!([s.num, s.denom])).unwrap_or(Value::Null),
        "tempoMap": {"markers": tempo_count, "tempoMapHash": tempo_map.hex()},
        "markers": {"count": marker_total, "markersHash": print.hex()},
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
    fn a_value_that_reads_as_zero_prints_as_zero_from_either_side() {
        fake::lab_project();
        let reads = Reads::default();
        // The second kick's take pitched (the count moves, so the digest is read again): its content's fingerprint.
        let pitched = |cents: f64| {
            fake::with(|s| {
                s.tracks[1].items[1].takes[0].extra.insert("D_PITCH".into(), cents);
                s.state_count += 1;
            });
            let (_, records) = whole(&reads, false, true);
            find(&records, "{ITEM-K2}")["f"]["content"]["contentHash"].clone()
        };
        let zero = pitched(0.0);
        // Up three cents and down three, and down and up, with REAPER's own nudges (lab 7.82); and −0.
        for cents in [-3.469_446_951_953_614e-18, 3.469_446_951_953_614e-18, -0.0] {
            assert_eq!(pitched(cents), zero, "{cents:e}");
        }
        // A pitch that reads otherwise still tells.
        assert_ne!(pitched(-0.000001), zero);
    }

    // Second sweep RP2: a number's text is written into one buffer, and what's hashed is what it was: the fingerprints the
    // host keeps in its baselines still match (this is the hash these numbers had as `format!` printed each).
    #[test]
    fn numbers_hash_as_their_text_did() {
        let mut print = Print::new();
        let values = [0.0, -0.0, 1.0, 0.5, -3.469_446_951_953_614e-18, 3.469_446_951_953_614e-18, 123.456789, -2.5, 4e-7, 1e300];
        for value in values.into_iter().chain([f64::NAN, f64::INFINITY, -1e-7, 48000.0]) {
            for places in [0, 4, 6] {
                print.number(value, places);
            }
        }
        assert_eq!(print.hex(), "ab5d778a1d18ca69");
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
        // (the project as it was, the pages kept would be answered again as they were: a count moved reads anew)
        let moved = || fake::with(|s| s.state_count += 1);
        moved();
        TEST_LIMITS.with(|limits| limits.set(Some((400, MOST_ITEMS, MOST_MARKERS))));
        let (first, paged) = whole(&reads, true, true);
        TEST_LIMITS.with(|limits| limits.set(None));
        assert!(first["pages"].as_u64().unwrap() > 5, "{}", first["pages"]);
        assert_eq!(paged, whole_records);
        let cursor = page(&reads, json!({})).unwrap()["nextCursor"].clone();
        assert!(cursor.is_null(), "the lab project fits one page");
        moved();
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

    // Second sweep RP3: a digest reads each marker and region once, for the facts' fingerprint and for its record,
    // whether the records come in the first page or in later ones.
    #[test]
    fn a_digest_reads_each_marker_once_for_its_fingerprint_and_its_record() {
        use crate::reaper::fake::FakeMarker;
        let digest = |markers: usize, page_bytes: usize| {
            fake::lab_project();
            fake::with(|s| {
                s.markers = (0..markers)
                    .map(|n| FakeMarker {
                        guid: format!("{{M{n:04}}}"),
                        is_region: n % 2 == 1,
                        start: n as f64,
                        end: n as f64 + 0.5,
                        name: format!("Line {n}"),
                        number: n as i32 + 1,
                        color: 0,
                    })
                    .collect()
            });
            forget();
            TEST_LIMITS.with(|limits| limits.set(Some((page_bytes, MOST_ITEMS, MOST_MARKERS))));
            let before = fake::with(|s| s.calls);
            let (first, records) = whole(&Reads::default(), false, true);
            TEST_LIMITS.with(|limits| limits.set(None));
            (fake::with(|s| s.calls) - before, first, records)
        };
        let (_, one_page, all) = digest(400, PAGE_BYTES);
        let listed = |records: &[Value]| records.iter().filter(|r| ["marker", "region"].contains(&r["k"].as_str().unwrap())).count();
        assert_eq!((one_page["pages"].as_u64(), listed(&all)), (Some(1), 400));
        // (in pages of 2 KB, the first page ends among the tracks: the markers' records come in later ones)
        for (page_bytes, in_first) in [(PAGE_BYTES, 400), (2048, 0)] {
            let (none, _, _) = digest(0, page_bytes);
            let (calls, first, records) = digest(400, page_bytes);
            assert_eq!(listed(first["records"].as_array().unwrap()), in_first);
            assert_eq!(records, all, "the same records in pages of {page_bytes}");
            assert_eq!(first["facts"]["markers"], one_page["facts"]["markers"]);
            // A marker's read is 8 REAPER calls, a region's 9: the marker, its values, its GUID and its name.
            let pages = first["pages"].as_u64().unwrap();
            assert!(calls - none < 400 * 9 * 3 / 2, "{} REAPER calls for 400 markers and regions in {pages} pages", calls - none);
        }
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
        // (the project as it was, the pages kept would be answered again as they were: a count moved reads anew)
        let moved = || fake::with(|s| s.state_count += 1);
        moved();
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
        moved();
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
        // (read anew: the parameters kept from the digest before would make these pages cheaper than a first read's)
        fake::with(|s| s.state_count += 1);
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
            // (a parameter edit moves REAPER's state change count, 7.82; the fake's value alone doesn't)
            s.state_count += 1;
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

    // The optimization audit's R1 (2026-10-24): watch_me looks at the project every second or so, and most looks find it
    // as it was; the parameter sweeps are the bulk of a digest's REAPER calls, and a parameter edit moves REAPER's state
    // change count (lab, 7.82). Under one count, they're read once.
    #[test]
    fn parameter_values_are_kept_while_the_state_count_stays_and_read_again_once_it_moved() {
        big_project();
        let reads = Reads::default();
        // (pages cut by their bytes alone, not the clock: the same digest makes the same calls each time)
        TEST_READS.with(|reads| reads.set(Some(usize::MAX)));
        let calls = || fake::with(|s| s.calls);
        let before = calls();
        let (_, first) = whole(&reads, true, false);
        let once = calls() - before;
        let before = calls();
        let (_, again) = whole(&reads, true, false);
        let kept = calls() - before;
        assert_eq!(again, first);
        assert!(kept * 4 < once, "{kept} REAPER calls for the look again, {once} for the first: 64,000 parameters were kept");
        // The producer moves a parameter: REAPER moves its count, and the parameters read anew.
        fake::with(|s| {
            s.state_count += 1;
            s.tracks[3].fx[1].params[2].value = 0.9;
        });
        let before = calls();
        let (_, after) = whole(&reads, true, false);
        let moved = calls() - before;
        assert!(moved * 2 > once, "{moved} REAPER calls once the count moved, {once} for the first");
        let (was, is) = (find(&first, "{F0003-1}"), find(&after, "{F0003-1}"));
        assert_eq!(is["v"][2], 0.9);
        assert_ne!(was["f"]["parameters"]["parametersHash"], is["f"]["parameters"]["parametersHash"]);
        assert_eq!(after.iter().filter(|record| !first.contains(record)).count(), 1, "one FX changed");
        TEST_READS.with(|reads| reads.set(None));
    }

    // What REAPER moves neither its count nor its history for (lab, 7.82: a bare API write outside an undo block, another
    // extension's, of a track's mute or name, an item's place or a marker) stays out of the pages kept until the bound
    // passes, as it did out of the parameter values kept; this extension's own writes are counted and show at once
    // (`a_complete_digests_pages_are_answered_again_while_nothing_moved`).
    #[test]
    fn a_bare_write_of_anothers_that_moved_no_count_shows_once_the_bound_passed() {
        fake::lab_project();
        let reads = Reads::default();
        let (_, before) = whole(&reads, true, true);
        fake::with(|s| {
            s.tracks[1].mute = true;
            s.tracks[2].name = "Snare Top".into();
            s.tracks[1].items[1].position = 3.0;
            s.markers[0].name = "Drop 2".into();
        });
        let (_, kept) = whole(&reads, true, true);
        assert_eq!(kept, before, "the pages kept stand: nothing says the project moved");
        TEST_KEPT_FOR.with(|bound| bound.set(Some(Duration::ZERO)));
        let (_, after) = whole(&reads, true, true);
        TEST_KEPT_FOR.with(|bound| bound.set(None));
        let moved: Vec<&str> =
            before.iter().zip(&after).filter(|(old, new)| old != new).map(|(_, new)| new["id"].as_str().unwrap()).collect();
        assert_eq!(moved, ["{BBBB}", "{ITEM-K2}", "{CCCC}", "{MARK-1}"]);
        assert_eq!(find(&after, "{BBBB}")["f"]["mixer"]["mute"], true);
    }

    #[test]
    fn kept_parameter_values_are_forgotten_with_the_epoch_and_bounded() {
        big_project();
        let reads = Reads::default();
        TEST_READS.with(|reads| reads.set(Some(usize::MAX)));
        // (the parameter values kept on their own: no pages kept here)
        TEST_PAGES_KEPT.with(|most| most.set(Some(0)));
        let calls = || fake::with(|s| s.calls);
        let before = calls();
        whole(&reads, true, false);
        let once = calls() - before;
        // Another project in the tab: nothing kept is of it.
        reads.bump_epoch();
        let before = calls();
        whole(&reads, true, false);
        assert_eq!(calls() - before, once, "read as a first digest is");
        // Kept up to a bound: past it, the rest is read each time.
        TEST_PARAMS_KEPT.with(|most| most.set(Some(640)));
        fake::with(|s| s.state_count += 1);
        let before = calls();
        whole(&reads, true, false);
        let fresh = calls() - before;
        let before = calls();
        whole(&reads, true, false);
        let kept = calls() - before;
        TEST_PARAMS_KEPT.with(|most| most.set(None));
        TEST_PAGES_KEPT.with(|most| most.set(None));
        TEST_READS.with(|reads| reads.set(None));
        assert_eq!(fresh - kept, 640, "ten FX of 64 parameters kept, the rest read again");
    }

    // Review: a parameter moved by what makes no undo point (a MIDI-learned control, a plug-in's own hand) would stay as
    // the last read had it until the count moved; after `KEPT_FOR` the values are read again whatever the count says.
    #[test]
    fn kept_parameter_values_are_read_again_after_a_while() {
        fake::lab_project();
        let reads = Reads::default();
        TEST_READS.with(|reads| reads.set(Some(usize::MAX)));
        let calls = || fake::with(|s| s.calls);
        let before = calls();
        let (_, first) = whole(&reads, true, false);
        let once = calls() - before;
        let before = calls();
        whole(&reads, true, false);
        assert!(calls() - before < once, "the EQ's parameters were kept");
        // A control the plug-in's own hand moved, with the count where it was: seen once the bound passes.
        fake::with(|s| s.tracks[0].fx[0].params[0].value = 0.75);
        TEST_KEPT_FOR.with(|bound| bound.set(Some(Duration::ZERO)));
        let before = calls();
        let (_, later) = whole(&reads, true, false);
        TEST_KEPT_FOR.with(|bound| bound.set(None));
        TEST_READS.with(|reads| reads.set(None));
        assert_eq!(calls() - before, once, "read whole again");
        assert_eq!(find(&first, "{FX-EQ}")["v"][0], 0.25);
        assert_eq!(find(&later, "{FX-EQ}")["v"][0], 0.75);
    }

    // Review: REAPER gives no GUID for an FX whose address holds none (or without `guidToString`); two such FX must never
    // share one kept set of values.
    #[test]
    fn an_fx_without_a_guid_is_read_every_time() {
        use crate::reaper::fake::{FakeFx, FakeParam};
        fake::lab_project();
        fake::with(|s| {
            let mut a = FakeFx::new("", "VST: Nameless (Kumi)");
            a.params = vec![FakeParam::new("Mix", 0.1, "10 %")];
            let mut b = FakeFx::new("", "VST: Nameless (Kumi)");
            b.params = vec![FakeParam::new("Mix", 0.9, "90 %")];
            s.tracks[2].fx = vec![a, b];
        });
        let reads = Reads::default();
        let values = |records: &[Value]| -> Vec<Value> {
            records.iter().filter(|r| r["p"] == "{CCCC}" && r["k"] == "FX").map(|r| r["v"][0].clone()).collect()
        };
        let (_, first) = whole(&reads, true, false);
        assert_eq!(values(&first), [json!(0.1), json!(0.9)]);
        // (a parameter write moves REAPER's count, bare or not: 7.82)
        fake::with(|s| {
            s.tracks[2].fx[1].params[0].value = 0.5;
            s.state_count += 1;
        });
        let (_, again) = whole(&reads, true, false);
        assert_eq!(values(&again), [json!(0.1), json!(0.5)], "each read anew, neither handed the other's values");
    }

    // The optimization audit's R7 and R1's plan (2026-10-24): watch_me looks at the project every second or so, and nearly
    // every look finds it as it was; with the parameter values kept (#511) a look still read every track, FX and send.
    // The pages of a complete digest are kept and answered again, with no REAPER call but the count's, while the epoch,
    // the count and this extension's own writes stay.
    #[test]
    fn a_complete_digests_pages_are_answered_again_while_nothing_moved() {
        big_project();
        let reads = Reads::default();
        TEST_READS.with(|reads| reads.set(Some(usize::MAX)));
        let calls = || fake::with(|s| s.calls);
        let before = calls();
        let (first, records) = whole(&reads, true, false);
        let once = calls() - before;
        let pages = first["pages"].as_u64().unwrap();
        assert!(pages >= 3, "{pages} pages");
        let before = calls();
        let (again, again_records) = whole(&reads, true, false);
        let kept = calls() - before;
        assert_eq!(again, first);
        assert_eq!(again_records, records);
        // (the project in front and the count, per page)
        assert!(kept <= 3 * pages, "{kept} REAPER calls for {pages} pages answered again, {once} for the read");
        // The producer edits a track (an undo point: the count moves): the digest is read anew, and kept anew.
        fake::with(|s| {
            s.state_count += 1;
            s.tracks[3].name = "Snare Top".into();
        });
        let before = calls();
        let (_, after) = whole(&reads, true, false);
        assert!(calls() - before > once / 2, "read anew");
        assert_eq!(find(&after, "{T0003}")["n"], "Snare Top");
        let before = calls();
        whole(&reads, true, false);
        assert!(calls() - before <= 3 * pages, "kept anew");
        // This extension writes something itself, outside an undo block (its listening's scratch work): REAPER's count
        // stays, the pages don't.
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let track = reaper.track_at(Project::first(), 4).unwrap();
        assert!(reaper.set_track_value(track, "B_MUTE", 1.0));
        let before = calls();
        let (_, written) = whole(&reads, true, false);
        assert!(calls() - before > 3 * pages, "read anew after a write of our own (the parameter values kept still)");
        assert_eq!(find(&written, "{T0004}")["f"]["mixer"]["mute"], true);
        TEST_READS.with(|reads| reads.set(None));
    }

    #[test]
    fn kept_pages_are_of_one_kind_complete_bounded_forgotten_with_the_epoch_and_read_again_after_a_while() {
        fake::lab_project();
        let reads = Reads::default();
        let calls = || fake::with(|s| s.calls);
        let cost = |values: bool, content: bool| {
            let before = calls();
            let (first, records) = whole(&reads, values, content);
            (calls() - before, first, records)
        };
        let (once, _, records) = cost(true, false);
        let (kept, _, again) = cost(true, false);
        assert_eq!(again, records);
        assert!(kept * 10 < once, "{kept} calls kept, {once} read");
        // Another kind of digest (catch-up's, with content) is read for itself, and kept for itself.
        let (content_once, _, _) = cost(false, true);
        assert!(content_once * 10 > once, "{content_once} calls: read, not the other kind's pages");
        let (content_kept, _, _) = cost(false, true);
        assert!(content_kept * 10 < content_once);
        let (still_kept, _, _) = cost(true, false);
        assert!(still_kept * 10 < once, "the first kind's pages are still kept");
        // After the bound, read again whatever the count says.
        TEST_KEPT_FOR.with(|bound| bound.set(Some(Duration::ZERO)));
        let (later, _, _) = cost(true, false);
        TEST_KEPT_FOR.with(|bound| bound.set(None));
        assert!(later * 10 > once, "{later} calls after the bound");
        // Another project in the tab: nothing kept is of it.
        let (_, _, _) = cost(true, false);
        reads.bump_epoch();
        let (fresh, _, _) = cost(true, false);
        assert!(fresh * 10 > once, "{fresh} calls after the epoch moved");
        // Only a complete digest is answered again: a first page alone isn't one.
        TEST_LIMITS.with(|limits| limits.set(Some((400, MOST_ITEMS, MOST_MARKERS))));
        fake::with(|s| s.state_count += 1);
        let first = page(&reads, json!({"values": true, "content": false})).unwrap();
        assert!(first["nextCursor"].is_string(), "more than one page");
        let before = calls();
        let first_again = page(&reads, json!({"values": true, "content": false})).unwrap();
        assert!(calls() - before > 3, "read again: the digest before wasn't walked to its end");
        assert_ne!(first_again["nextCursor"], first["nextCursor"], "a digest of its own");
        TEST_LIMITS.with(|limits| limits.set(None));
        // Kept up to a bound of bytes: past it, the digest is read every time.
        TEST_PAGES_KEPT.with(|most| most.set(Some(100)));
        fake::with(|s| s.state_count += 1);
        cost(true, false);
        let (unkept, _, _) = cost(true, false);
        TEST_PAGES_KEPT.with(|most| most.set(None));
        assert!(unkept * 10 > once, "{unkept} calls: too big to keep");
    }

    // The extension's writes are counted through every wrapper, inside an undo block or not, and through the generic call.
    #[test]
    fn every_write_of_the_extensions_own_is_counted() {
        fake::lab_project();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let proj = Project::first();
        let track = reaper.track_at(proj, 1).unwrap();
        let before = crate::reaper::writes();
        reaper.set_track_value(track, "D_VOL", 0.5);
        reaper.set_track_string(track, "P_NAME", "Kick 2");
        reaper.set_project_text(proj, "TRACK_GROUP_NAME:1", "Drums");
        reaper.run_action(40001);
        assert_eq!(crate::reaper::writes() - before, 4);
        // The FX, envelope and automation-item wrappers, a track selected, the lanes updated: writes too (review: the
        // first sweep went by the wrappers' names and missed these).
        let before = crate::reaper::writes();
        let drums = reaper.track_at(proj, 0).unwrap();
        let eq = Fx { owner: FxOwner::Track(drums), index: 0 };
        reaper.fx_set_param(eq, 0, 0.5);
        reaper.fx_set_config(eq, "renamed_name", "EQ");
        reaper.fx_set_enabled(eq, false);
        let envelope = reaper.track_envelope_by_chunk_name(drums, "<VOLENV2").unwrap();
        reaper.env_insert_point(
            envelope,
            -1,
            crate::reaper::EnvPoint { time: 1.0, value: 0.5, shape: 0, tension: 0.0, selected: false },
            true,
        );
        reaper.select_only(track);
        reaper.update_item_lanes(proj);
        assert_eq!(crate::reaper::writes() - before, 6);
        // A chunk of Lua run for Kumi is a write whatever it did (review): counted whether or not REAPER took it.
        let before = crate::reaper::writes();
        let _ = reaper.run_script(proj, "reaper.SetMediaTrackInfo_Value(reaper.GetTrack(0, 0), 'B_MUTE', 1)");
        assert_eq!(crate::reaper::writes() - before, 1);
        // Reads count for nothing.
        let before = crate::reaper::writes();
        reaper.track_value(track, "D_VOL");
        reaper.track_name(track);
        reaper.state_change_count(proj);
        assert_eq!(crate::reaper::writes(), before);
    }

    #[test]
    fn a_plug_in_whose_parameters_changed_in_number_is_read_again() {
        fake::lab_project();
        let reads = Reads::default();
        let (_, before) = whole(&reads, true, false);
        assert_eq!(find(&before, "{FX-EQ}")["v"], json!([0.25, null, 1.0]));
        // (a preset loaded, which is how a plug-in comes to more parameters, moves REAPER's count: 7.82)
        fake::with(|s| {
            s.tracks[0].fx[0].params.push(fake::FakeParam::new("Band 2 Gain", 0.75, "+3 dB"));
            s.state_count += 1;
        });
        let (_, after) = whole(&reads, true, false);
        assert_eq!(find(&after, "{FX-EQ}")["v"], json!([0.25, null, 1.0, 0.75]));
        assert_eq!(find(&after, "{FX-EQ}")["f"]["parameters"]["count"], 4);
    }
}
