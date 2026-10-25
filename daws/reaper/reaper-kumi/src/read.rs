//! `read` and `snapshot`: the project as rows, paged. Every answer stays under the frame bound (256 KiB): a page ends at
//! the limit, at the byte budget or at the time budget, and says where to go on with a cursor.

use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::io::Write;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::focus;
use crate::queue::Answer;
use crate::reaper::{Envelope, Fx, FxOwner, Item, Project, Reaper, Take, Track, INPUT_FX};
use crate::refs::{self, EnvelopeId, Epoch, Ref, Registry, TrackId};
use crate::rows;

mod bodies;
pub(crate) mod item_rows;
// m4-r-fxenv: points, automation items and take envelopes.
mod envelopes;
// M6: the project as records for catch-up and watch_me.
mod digest;

pub use bodies::BODY_KINDS;

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 200;
/// The most one answer carries. The bridge's frame bound is 256 KiB; the rest is the envelope around the rows.
pub const ANSWER_BYTES: usize = 224 * 1024;
/// A page stops growing after this long on the main thread, close to what a tick spends on requests: the rest comes with
/// the cursor.
#[cfg(not(test))]
const PAGE_TIME: Duration = Duration::from_millis(10);

#[cfg(not(test))]
fn page_time() -> Duration {
    PAGE_TIME
}

#[cfg(test)]
thread_local! {
    /// A test's own page time, to make a page run out of it without a slow REAPER.
    pub static TEST_PAGE_TIME: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn page_time() -> Duration {
    // Slow builds and busy machines mustn't change what a test sees: a test that wants a short time sets one.
    TEST_PAGE_TIME.with(|time| time.get()).unwrap_or(Duration::from_secs(10))
}
/// The most tracks and markers a snapshot carries; `read` has the rest.
const SNAPSHOT_TRACKS: usize = 200;
const SNAPSHOT_MARKERS: usize = 100;
/// The snapshot's own share of the answer, so the tracks leave room for the markers and the selection.
const SNAPSHOT_TRACK_BYTES: usize = 150 * 1024;
const KINDS: [&str; 15] = [
    "project", "track", "item", "take", "fx", "param", "send", "marker", "region", "note", "event", "envelope", "call", "point", "autoitem",
];

pub fn bad(message: impl Into<String>) -> (String, String) {
    ("bad-params".into(), message.into())
}

pub fn failed(message: impl Into<String>) -> (String, String) {
    ("failed".into(), message.into())
}

/// Two objects of a kind have one GUID: a ref to either can't say which it means.
fn shared_id(kind: &str) -> (String, String) {
    failed(format!(
        "two {kind}s in the project have that id (one was copied as text, which keeps the id): REAPER can't tell them apart, so remove the copy or recreate it"
    ))
}

/// The list under a cursor is not the one it was made for.
pub fn changed(message: impl Into<String>) -> (String, String) {
    (daw_bridge::wire::code::CHANGED.into(), message.into())
}

/// The refs' registry and the epoch they live in.
/// A saved project's id while it has none of its own: made from its path, as Renoise's tool and Live make theirs.
pub fn path_id(path: &str) -> String {
    daw_bridge::auth::sha256_hex(format!("reaper-project:{path}").as_bytes())[..32].to_string()
}

#[derive(Default)]
pub struct Reads {
    registry: RefCell<Registry>,
    epoch: RefCell<Epoch>,
    /// The serial of the project open in each tab: what its identity is made of.
    serials: RefCell<Serials>,
    /// A Kumi project id found in a tab's project while it was unsaved: it came with the template the project was made
    /// from, so it isn't that project's own.
    inherited: RefCell<HashMap<usize, String>>,
    /// What `GetTouchedOrFocusedFX` said (touched, focused) when the tab last changed: it names the track by its number in
    /// whichever project is active, and takes no project, so the same answer after a switch may be another project's.
    stale_fx: RefCell<[Option<crate::reaper::FxHit>; 2]>,
    /// The project each recent epoch was about (its identity), so that a look at another tab and back doesn't make what
    /// was done before it un-undoable: the epoch moved, the project is the same.
    eras: RefCell<std::collections::VecDeque<(String, String)>>,
    /// Kumi's own folder (from the bridge's reference): a file in it is copied into the project before REAPER plays it.
    kumi_folder: RefCell<Option<std::path::PathBuf>>,
}

/// A serial number for each project open in a tab, from this REAPER's own counter: the same while the project stays open
/// (a tab switched away from and back to keeps its number), new when a different project takes the tab's place. The
/// tab's pointer is only the key: File > New in the tab, and a pointer REAPER hands out again, both repeat it.
#[derive(Default)]
struct Serials {
    by_tab: HashMap<usize, u64>,
    last: u64,
}

impl Serials {
    fn of(&mut self, tab: usize) -> u64 {
        if let Some(&serial) = self.by_tab.get(&tab) {
            return serial;
        }
        self.replace(tab)
    }

    fn replace(&mut self, tab: usize) -> u64 {
        self.last += 1;
        self.by_tab.insert(tab, self.last);
        self.last
    }
}

/// The size of a value as JSON, without building the text.
pub fn json_len(value: &Value) -> usize {
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    let _ = serde_json::to_writer(&mut count, value);
    count.0
}

/// Rows gathered for one answer.
pub struct Pager {
    limit: usize,
    budget: usize,
    bytes: usize,
    started: Instant,
    pub rows: Vec<Value>,
}

impl Pager {
    pub fn new(limit: usize, budget: usize) -> Pager {
        Pager { limit, budget, bytes: 0, started: Instant::now(), rows: Vec::new() }
    }

    /// Whether the page has had its time, whether or not it has a row yet: a page that is still looking for its first row
    /// ends empty, with a cursor to go on from, rather than hold REAPER's main thread.
    pub fn spent(&self) -> bool {
        self.started.elapsed() >= page_time()
    }

    /// Whether the page has room for another row.
    pub fn open(&self) -> bool {
        self.rows.len() < self.limit && self.bytes < self.budget && (self.rows.is_empty() || self.started.elapsed() < page_time())
    }

    /// Adds a row; false when it would overflow the byte budget (a first row always goes in).
    pub fn push(&mut self, row: Value) -> bool {
        let size = json_len(&row) + 1;
        if !self.rows.is_empty() && self.bytes + size > self.budget {
            return false;
        }
        self.bytes += size;
        self.rows.push(row);
        true
    }
}

/// What a `read` was asked.
#[derive(Debug, Default)]
struct Request {
    kind: Option<String>,
    reference: Option<String>,
    parent: Option<String>,
    cursor: Option<String>,
    limit: usize,
}

fn text_param(params: &Value, name: &str) -> Result<Option<String>, (String, String)> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.is_empty() => Ok(Some(text.clone())),
        Some(_) => Err(bad(format!("{name} must be a non-empty string"))),
    }
}

fn limit_param(params: &Value) -> Result<usize, (String, String)> {
    match params.get("limit") {
        None | Some(Value::Null) => Ok(DEFAULT_LIMIT),
        Some(value) => match value.as_u64() {
            Some(n) if (1..=MAX_LIMIT as u64).contains(&n) => Ok(n as usize),
            _ => Err(bad(format!("limit must be a whole number from 1 to {MAX_LIMIT}"))),
        },
    }
}

impl Request {
    fn parse(params: &Value) -> Result<Request, (String, String)> {
        let request = Request {
            kind: text_param(params, "kind")?,
            reference: text_param(params, "ref")?,
            parent: text_param(params, "parent")?,
            cursor: text_param(params, "cursor")?,
            limit: limit_param(params)?,
        };
        match (&request.kind, &request.reference) {
            (None, None) => return Err(bad(format!("kind: one of {}", KINDS.join(", ")))),
            (Some(kind), _) if !KINDS.contains(&kind.as_str()) => {
                return Err(bad(format!("REAPER has no kind {kind:?} to read: use one of {}", KINDS.join(", "))))
            }
            _ => {}
        }
        Ok(request)
    }
}

/// The project a read is about: the active tab.
fn active(reaper: &Reaper) -> Result<(Project, crate::reaper::ProjectInfo), (String, String)> {
    let info = reaper.current_project().ok_or_else(|| failed("REAPER has no project open"))?;
    Ok((info.ptr, info))
}

/// What a cursor says: the position, and the key of the row at it.
type CursorParts = (String, Option<String>);

impl Reads {
    /// Kumi's own folder, once the reference names it.
    pub fn set_kumi_folder(&self, folder: std::path::PathBuf) {
        *self.kumi_folder.borrow_mut() = Some(folder);
    }

    pub fn kumi_folder(&self) -> Option<std::path::PathBuf> {
        self.kumi_folder.borrow().clone()
    }

    pub fn epoch(&self) -> String {
        self.epoch.borrow().text()
    }

    /// The FX REAPER says was touched (mode 0) or has the focus (mode 1), unless that is still what it said before the tab
    /// changed: REAPER names the track by its number and says nothing of its project, so it is not known to be this one's.
    /// It counts again once REAPER says something else.
    pub fn touched_fx(&self, reaper: &Reaper, mode: i32) -> Option<crate::reaper::FxHit> {
        let hit = reaper.touched_or_focused_fx(mode)?;
        let mut stale = self.stale_fx.borrow_mut();
        let slot = &mut stale[(mode & 1) as usize];
        if *slot == Some(hit) {
            return None;
        }
        *slot = None;
        Some(hit)
    }

    /// The tab changed: what REAPER says is touched or focused now was said about the project before.
    pub fn forget_touched(&self, reaper: &Reaper) {
        *self.stale_fx.borrow_mut() = [reaper.touched_or_focused_fx(0), reaper.touched_or_focused_fx(1)];
    }

    /// Remembers which project the epoch now is about.
    pub fn note_project(&self, proj: Project) {
        let epoch = self.epoch();
        let mut eras = self.eras.borrow_mut();
        if eras.iter().all(|(known, _)| *known != epoch) {
            eras.push_back((epoch, self.identity(proj)));
            while eras.len() > 64 {
                eras.pop_front();
            }
        }
    }

    /// Whether an epoch, current or past, was about this project.
    pub fn epoch_is_of(&self, epoch: &str, proj: Project) -> bool {
        let identity = self.identity(proj);
        self.eras.borrow().iter().any(|(known, of)| known == epoch && *of == identity)
    }

    /// Refs stop meaning what they meant: a tab switch, or a new project in the tab.
    pub fn bump_epoch(&self) {
        self.epoch.borrow_mut().bump();
        self.registry.borrow_mut().clear();
    }

    /// What names the project open in this tab while it stays open in this REAPER: this process and the project's serial.
    pub fn identity(&self, proj: Project) -> String {
        format!("{}:{}", std::process::id(), self.serials.borrow_mut().of(proj.addr()))
    }

    /// A different project took the place of the one in this tab.
    pub fn project_replaced(&self, proj: Project) {
        self.serials.borrow_mut().replace(proj.addr());
        self.inherited.borrow_mut().remove(&proj.addr());
    }

    /// The Kumi project id the project carries (ProjExtState `kumi`/`project`, saved in the RPP), or None when it has none
    /// of its own. It travels with the file: Save As, a copy of the file opened, a backup, all share it, as Live's Sets
    /// do. A project that is still unsaved has none of its own: one found in it came with the template it was made from,
    /// and stays foreign after the first save, until a different one is written over it.
    pub fn project_id(&self, reaper: &Reaper, proj: Project, info: &crate::reaper::ProjectInfo) -> Option<String> {
        let stored = reaper.project_ext_state(proj, "kumi", "project").filter(|id| !id.is_empty());
        let tab = proj.addr();
        let mut inherited = self.inherited.borrow_mut();
        match (stored, info.path.is_empty()) {
            (Some(id), true) => {
                inherited.insert(tab, id);
                None
            }
            // Saved, with no id of its own yet (none, or its template's): named by its path, so a restart reopening the
            // same file is the same project. The id Kumi writes later (M6) is this one.
            (Some(id), false) if inherited.get(&tab) == Some(&id) => Some(path_id(&info.path)),
            (Some(id), false) => {
                inherited.remove(&tab);
                Some(id)
            }
            (None, true) => {
                inherited.remove(&tab);
                None
            }
            (None, false) => {
                inherited.remove(&tab);
                Some(path_id(&info.path))
            }
        }
    }

    /// (m6-places) A saved project with no id of its own (none, or the template's it came with) gets the id Kumi names it
    /// by, its path's, written into it: REAPER keeps it with the project and saves it with the producer's next save, so a
    /// Save As or a moved file stays the same place. It marks nothing changed (7.82 lab). An unsaved project gets none.
    pub fn keep_project_id(&self, reaper: &Reaper, proj: Project, info: &crate::reaper::ProjectInfo) {
        if info.path.is_empty() {
            return;
        }
        let stored = reaper.project_ext_state(proj, "kumi", "project").filter(|id| !id.is_empty());
        let inherited = self.inherited.borrow().get(&proj.addr()).cloned();
        if stored.is_some() && stored != inherited {
            return;
        }
        if reaper.set_project_ext_state(proj, "kumi", "project", &path_id(&info.path)) {
            self.inherited.borrow_mut().remove(&proj.addr());
        }
    }

    /// The tabs open now: the serial of a tab that closed is forgotten, so a pointer REAPER hands out again starts afresh.
    pub fn keep_tabs(&self, open: &[usize]) {
        self.serials.borrow_mut().by_tab.retain(|tab, _| open.contains(tab));
        self.inherited.borrow_mut().retain(|tab, _| open.contains(tab));
    }

    pub fn registry(&self) -> RefMut<'_, Registry> {
        self.registry.borrow_mut()
    }

    /// A request that says which epoch its refs came from (`epoch`, as an earlier answer gave it) is refused as stale
    /// when the project in the tab has changed since: refs are GUIDs, and a copy of the project in another tab has the
    /// same ones.
    pub fn expect_epoch(&self, params: &Value) -> Result<(), (String, String)> {
        match params.get("epoch") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(epoch)) if *epoch == self.epoch() => Ok(()),
            Some(Value::String(epoch)) => Err((
                daw_bridge::wire::code::STALE.into(),
                format!("those refs are from an earlier epoch ({epoch}; now {}): the project in the tab changed, read again", self.epoch()),
            )),
            Some(_) => Err(bad("epoch: the epoch an earlier answer gave, unchanged")),
        }
    }

    /// A cursor is `c1.<epoch>.<position>`, with `~<key>` after the position when the row it points at has an identity:
    /// a page goes on from that row wherever it is by then, not from a position an edit between two pages has moved.
    fn cursor_text(&self, position: &str) -> String {
        format!("c1.{}.{position}", self.epoch())
    }

    /// The position and, if it has one, the key of the row a cursor points at; None for no cursor.
    fn cursor_parts(&self, cursor: Option<&str>) -> Result<Option<CursorParts>, (String, String)> {
        let Some(cursor) = cursor else { return Ok(None) };
        let mut parts = cursor.splitn(3, '.');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("c1"), Some(epoch), Some(rest)) if !rest.is_empty() => {
                if epoch != self.epoch() {
                    return Err(bad("that cursor is from before the project tab changed: read again from the start"));
                }
                Ok(Some(match rest.split_once('~') {
                    Some((position, key)) if !position.is_empty() && !key.is_empty() => (position.to_string(), Some(key.to_string())),
                    Some(_) => return Err(bad("cursor: pass the nextCursor an earlier read gave, unchanged")),
                    None => (rest.to_string(), None),
                }))
            }
            _ => Err(bad("cursor: pass the nextCursor an earlier read gave, unchanged")),
        }
    }

    /// The position of a cursor into a list of `total` rows that has no identity to follow (a parameter, a note): from
    /// the start without one, and refused when it points past the end, since the list is shorter than it was.
    fn position_within(&self, cursor: Option<&str>, total: usize) -> Result<usize, (String, String)> {
        self.resume_number(cursor, total, |_| None)
    }

    /// Where the next page of a list of `total` rows starts. A cursor made with a key goes on from the row that has it,
    /// where it is now; if no row has it any more the list changed under the cursor. One without a key goes on from its
    /// position. Either way a position past the end is refused.
    fn resume_number(
        &self,
        cursor: Option<&str>,
        total: usize,
        key_at: impl Fn(usize) -> Option<String>,
    ) -> Result<usize, (String, String)> {
        let Some((position, key)) = self.cursor_parts(cursor)? else { return Ok(0) };
        let position: usize = position.parse().map_err(|_| bad("cursor: not one this kind of read gave"))?;
        if position > total {
            return Err(changed("that cursor points past the end of a list that is shorter now: read again from the start"));
        }
        let Some(key) = key else { return Ok(position) };
        if key_at(position).as_deref() == Some(key.as_str()) {
            return Ok(position);
        }
        (0..total)
            .find(|&at| key_at(at).as_deref() == Some(key.as_str()))
            .ok_or_else(|| changed("what that cursor pointed at is gone: the list changed between pages, read again from the start"))
    }

    /// A position and the key of its row, as a cursor carries them.
    fn anchored(position: impl std::fmt::Display, key: Option<String>) -> String {
        match key {
            Some(key) => format!("{position}~{key}"),
            None => position.to_string(),
        }
    }

    /// A page of a list whose rows have keys: its cursor points at the row after the last, and carries that row's key.
    fn page_keyed(&self, pager: Pager, next: Option<usize>, total: Option<usize>, key_at: impl Fn(usize) -> Option<String>) -> Answer {
        self.page_at(pager, next.map(|at| Self::anchored(at, key_at(at))), total)
    }

    fn page(&self, pager: Pager, next: Option<usize>, total: Option<usize>) -> Answer {
        self.page_at(pager, next.map(|n| n.to_string()), total)
    }

    fn page_at(&self, pager: Pager, next: Option<String>, total: Option<usize>) -> Answer {
        let mut answer = json!({"items": pager.rows, "epoch": self.epoch()});
        if let Some(total) = total {
            answer["total"] = total.into();
        }
        if let Some(next) = next {
            answer["nextCursor"] = self.cursor_text(&next).into();
        }
        Ok(answer)
    }

    // ----- resolving refs -----

    pub(crate) fn envelope_by_ref(
        &self,
        reaper: &Reaper,
        proj: Project,
        track: &TrackId,
        which: &EnvelopeId,
    ) -> Result<Envelope, (String, String)> {
        let track = self.track_by_id(reaper, proj, track)?;
        self.envelope_of(reaper, proj, track, which)
    }

    pub(crate) fn item_by_ref(&self, reaper: &Reaper, proj: Project, guid: &str) -> Result<Item, (String, String)> {
        self.item_by_guid(reaper, proj, guid)
    }

    pub(crate) fn take_by_ref(&self, reaper: &Reaper, proj: Project, guid: &str) -> Result<Take, (String, String)> {
        self.take_by_guid(reaper, proj, guid)
    }

    pub(crate) fn track_by_id(&self, reaper: &Reaper, proj: Project, id: &TrackId) -> Result<Track, (String, String)> {
        let mut registry = self.registry.borrow_mut();
        registry.track(reaper, proj, id).ok_or_else(|| match id {
            TrackId::Guid(guid) if registry.is_shared(guid) => shared_id("track"),
            _ => failed("that track isn't in the project (deleted, or another tab)"),
        })
    }

    fn item_by_guid(&self, reaper: &Reaper, proj: Project, guid: &str) -> Result<Item, (String, String)> {
        let mut registry = self.registry.borrow_mut();
        registry.item(reaper, proj, guid).ok_or_else(|| match registry.is_shared(guid) {
            true => shared_id("item"),
            false => failed("that item isn't in the project (deleted, or another tab)"),
        })
    }

    fn take_by_guid(&self, reaper: &Reaper, proj: Project, guid: &str) -> Result<Take, (String, String)> {
        self.registry
            .borrow_mut()
            .take(reaper, proj, guid)
            .ok_or_else(|| failed("that take isn't in the project (deleted, or another tab)"))
    }

    pub(crate) fn fx_by_guid(&self, reaper: &Reaper, proj: Project, guid: &str) -> Result<Fx, (String, String)> {
        let mut registry = self.registry.borrow_mut();
        registry.fx(reaper, proj, guid).ok_or_else(|| match registry.is_shared(guid) {
            true => shared_id("FX"),
            false => failed("that FX isn't in the project (deleted, or another tab)"),
        })
    }

    /// A parent ref that must be a track.
    fn parent_track(&self, reaper: &Reaper, proj: Project, parent: &str) -> Result<Track, (String, String)> {
        match Ref::parse(parent).map_err(bad)? {
            Ref::Track(id) => self.track_by_id(reaper, proj, &id),
            other => Err(bad(format!("parent: a track ref, not r:{}", other.kind()))),
        }
    }

    /// A ref that must be a track, named in the error by the parameter it came in.
    fn parent_track_named(&self, reaper: &Reaper, proj: Project, reference: &str, parameter: &str) -> Result<Track, (String, String)> {
        match Ref::parse(reference).map_err(bad)? {
            Ref::Track(id) => self.track_by_id(reaper, proj, &id),
            other => Err(bad(format!("{parameter}: a track ref, not r:{}", other.kind()))),
        }
    }

    fn parent_fx(&self, reaper: &Reaper, proj: Project, parent: &str) -> Result<Fx, (String, String)> {
        match Ref::parse(parent).map_err(bad)? {
            Ref::Fx(guid) => self.fx_by_guid(reaper, proj, &guid),
            other => Err(bad(format!("parent: an FX ref, not r:{}", other.kind()))),
        }
    }

    // ----- read -----

    pub fn read(&self, reaper: &Reaper, params: &Value) -> Answer {
        // M5 listening (m5-reaper): what plays where.
        if params.get("kind").and_then(Value::as_str) == Some("spans") {
            return self.read_spans(reaper, params);
        }
        let request = Request::parse(params)?;
        self.expect_epoch(params)?;
        let (proj, info) = active(reaper)?;
        // M5 listening (m5-reaper): a parameter's text across its range.
        let grid = crate::listening::reads::grid_points(params)?;
        if let Some(reference) = &request.reference {
            return self.read_one(reaper, proj, reference, grid);
        }
        let kind = request.kind.as_deref().unwrap_or_default();
        if kind == "call" {
            return self.read_call(reaper, proj, params);
        }
        let pager = Pager::new(request.limit, ANSWER_BYTES);
        let cursor = request.cursor.as_deref();
        let parent = request.parent.as_deref();
        let answer = match kind {
            "project" => {
                let mut pager = pager;
                pager.push(rows::project_row(reaper, proj, &info, &self.identity(proj), self.project_id(reaper, proj, &info).as_deref()));
                self.page(pager, None, Some(1))
            }
            "track" => self.read_tracks(reaper, proj, pager, cursor),
            "item" => self.read_items(reaper, proj, pager, parent, cursor),
            "take" => self.read_takes(reaper, proj, pager, parent, cursor),
            "fx" => self.read_fx(reaper, proj, pager, parent, cursor),
            "param" => self.read_params(reaper, proj, pager, parent, cursor, grid),
            "send" => self.read_sends(reaper, proj, pager, parent, cursor),
            "marker" => self.read_markers(reaper, proj, pager, false, cursor),
            "region" => self.read_markers(reaper, proj, pager, true, cursor),
            "note" => self.read_notes(reaper, proj, pager, parent, cursor),
            "event" => self.read_events(reaper, proj, pager, parent, cursor),
            "envelope" => self.read_envelopes(reaper, proj, pager, parent, cursor),
            // m4-r-fxenv
            "point" => self.read_points(reaper, proj, pager, parent, cursor),
            "autoitem" => self.read_auto_items(reaper, proj, pager, parent, cursor),
            other => Err(bad(format!("REAPER has no kind {other:?} to read"))),
        };
        // Where a row's `index` starts counting, which is where the change that takes the same index starts too: a track's
        // place is counted from 1 (as REAPER numbers its tracks), an FX's place, a parameter's and a note's from 0.
        let from = match kind {
            "track" => Some(1),
            "fx" | "param" | "note" | "event" | "point" | "autoitem" => Some(0),
            _ => None,
        };
        answer.map(|mut answer| {
            if let Some(from) = from {
                answer["indexFrom"] = from.into();
            }
            answer
        })
    }

    /// `read {kind: "call", fn, args}`: one of REAPER's API functions that only reads, called by name (see reaper/generic.rs).
    /// One that changes the project is a change: the api-call kind.
    fn read_call(&self, reaper: &Reaper, proj: Project, params: &Value) -> Answer {
        use crate::reaper::generic;
        let name = params.get("fn").and_then(Value::as_str).ok_or_else(|| bad("fn: the name of a REAPER API function"))?;
        let index = generic::function(name).map_err(bad)?;
        let args = params.get("args").cloned().unwrap_or(Value::Array(vec![]));
        if let Some(why) = generic::denied(index, &args) {
            return Err(bad(why));
        }
        if !generic::reads_only(index, &args) {
            return Err(bad(format!("{name} changes the project: make it a change, the api-call kind, which REAPER's undo takes back")));
        }
        let handles = CallRefs { reads: self, reaper, proj };
        let (result, out) = reaper.call(index, &args, &handles, false).map_err(bad)?;
        Ok(json!({"epoch": self.epoch(), "fn": name, "result": result, "out": out}))
    }

    /// `read {ref}`: the one object a ref names.
    fn read_one(&self, reaper: &Reaper, proj: Project, reference: &str, grid: Option<usize>) -> Answer {
        let row = match Ref::parse(reference).map_err(bad)? {
            Ref::Track(id) => rows::track_row_detailed(reaper, self.track_by_id(reaper, proj, &id)?),
            Ref::Item(guid) => rows::item_row(reaper, proj, self.item_by_guid(reaper, proj, &guid)?),
            Ref::Take(guid) => take_row(reaper, self.take_by_guid(reaper, proj, &guid)?, None),
            Ref::Fx(guid) => {
                let fx = self.fx_by_guid(reaper, proj, &guid)?;
                // m4-r-fxenv: one FX read alone also says its state chunk, presets, wet and delta.
                let mut row = rows::fx_row(reaper, fx, chain_position(reaper, fx));
                envelopes::add_fx_details(reaper, fx, &mut row);
                row
            }
            Ref::Param { fx: guid, index } => {
                let fx = self.fx_by_guid(reaper, proj, &guid)?;
                if index < 0 || index >= reaper.fx_param_count(fx) {
                    return Err(failed(format!("that parameter isn't there: the FX has {} parameters", reaper.fx_param_count(fx))));
                }
                let mut row = rows::param_row(reaper, fx, &guid, index);
                if let Some(points) = grid {
                    crate::listening::reads::add_grid(reaper, fx, index, points, &mut row);
                }
                row
            }
            Ref::Send { track, category, other, repeat, of, seen } => {
                let owner = self.track_by_id(reaper, proj, &track)?;
                let keys = rows::send_keys(reaper, owner, category);
                let key = rows::SendKey { other, repeat, of };
                let Some(index) = keys.iter().position(|k| *k == key) else {
                    // Sends to one place have no identity of their own: once their number changes, a ref to one of them
                    // could be any, so it names none.
                    let now = keys.iter().filter(|k| k.other == key.other).count();
                    let sends = if now == 1 { "send" } else { "sends" };
                    let why = match (now, of) {
                        (0, _) => "the track has no such send (deleted, or it went elsewhere)".to_string(),
                        (_, 1) => {
                            format!("the track has {now} such {sends} now, and this ref was made when there was one: read them again")
                        }
                        (_, _) => {
                            format!("the track has {now} such {sends} now, and this ref was made when there were {of}: read them again")
                        }
                    };
                    return Err(failed(format!("that send isn't there: {why}")));
                };
                // One of two or more sends to one track is the send it was read as only while those sends are as the ref saw
                // them (or as Kumi's own writes made them): never another send at that place.
                if category == 0 && of >= 2 {
                    let receiver = reaper.send_peer(owner, 0, index as i32);
                    // (answered in these words, not as a ref of another project: the project is the same, the sends aren't)
                    if !receiver.is_some_and(|receiver| crate::changes::send_seen_holds(reaper, owner, receiver, seen.as_deref())) {
                        return Err(failed("the sends between those tracks changed since that ref was read: read the track's sends again"));
                    }
                }
                rows::send_row(reaper, owner, category, index as i32, &key)
            }
            Ref::Marker(guid) => self.marker_by_guid(reaper, proj, &guid, false)?,
            Ref::Region(guid) => self.marker_by_guid(reaper, proj, &guid, true)?,
            Ref::Envelope { track, which } => {
                let owner = self.track_by_id(reaper, proj, &track)?;
                let envelope = self.envelope_of(reaper, proj, owner, &which)?;
                self.track_envelope_row(reaper, proj, &track, envelope)
            }
            // m4-r-fxenv
            other @ (Ref::TakeEnvelope { .. } | Ref::AutoItem { .. }) => self.read_one_m4(reaper, proj, &other)?,
        };
        Ok(json!({"items": [row], "epoch": self.epoch()}))
    }

    fn marker_by_guid(&self, reaper: &Reaper, proj: Project, guid: &str, region: bool) -> Result<Value, (String, String)> {
        reaper.marker_by_guid(proj, guid).filter(|m| m.is_region == region).map(|m| rows::marker_row(reaper, proj, &m)).ok_or_else(|| {
            failed(format!("that {} isn't in the project (deleted, or another tab)", if region { "region" } else { "marker" }))
        })
    }

    fn envelope_of(&self, reaper: &Reaper, proj: Project, track: Track, which: &EnvelopeId) -> Result<Envelope, (String, String)> {
        let found = match which {
            EnvelopeId::Guid(guid) => reaper
                .track_envelope_by_chunk_name(track, guid)
                .or_else(|| {
                    self.track_envelopes(reaper, track).into_iter().find(|(id, _)| id.as_deref() == Some(guid.as_str())).map(|(_, e)| e)
                })
                .filter(|envelope| reaper.envelope_owner(*envelope).is_some_and(|(owner, _, _)| owner == track)),
            EnvelopeId::Chunk(name) => reaper.track_envelope_by_chunk_name(track, &format!("<{name}")),
            EnvelopeId::Param { fx, index } => {
                let fx = self.fx_by_guid(reaper, proj, fx)?;
                match fx.owner {
                    FxOwner::Track(owner) if owner == track => reaper.fx_param_envelope(track, fx.index, *index),
                    _ => None,
                }
            }
        };
        found.ok_or_else(|| failed("that track has no such envelope (deleted, or on another track)"))
    }

    // ----- tracks -----

    /// Rows of the master (position 0) and the tracks (1 and on).
    pub fn track_page(&self, reaper: &Reaper, proj: Project, pager: &mut Pager, start: usize) -> Option<usize> {
        let count = reaper.track_count(proj) as usize;
        let mut position = start;
        while position <= count {
            if !pager.open() {
                return Some(position);
            }
            let track = if position == 0 { reaper.master_track(proj) } else { reaper.track_at(proj, position as i32 - 1) };
            if let Some(track) = track {
                if !pager.push(rows::track_row(reaper, track)) {
                    return Some(position);
                }
            }
            position += 1;
        }
        None
    }

    /// The key of the row at a position of the master-then-tracks list.
    pub fn track_key(&self, reaper: &Reaper, proj: Project, position: usize) -> Option<String> {
        match position {
            0 => Some("master".into()),
            n => reaper.track_at(proj, n as i32 - 1).map(|track| reaper.track_guid(track)),
        }
    }

    fn read_tracks(&self, reaper: &Reaper, proj: Project, mut pager: Pager, cursor: Option<&str>) -> Answer {
        let total = reaper.track_count(proj) as usize + 1;
        let key = |position: usize| self.track_key(reaper, proj, position);
        let start = self.resume_number(cursor, total, key)?;
        let next = self.track_page(reaper, proj, &mut pager, start);
        self.page_keyed(pager, next, Some(total), key)
    }

    // ----- items and takes -----

    fn read_items(&self, reaper: &Reaper, proj: Project, mut pager: Pager, parent: Option<&str>, cursor: Option<&str>) -> Answer {
        if let Some(parent) = parent {
            let track = self.parent_track(reaper, proj, parent)?;
            let total = reaper.track_item_count(track) as usize;
            let key = |position: usize| reaper.track_item(track, position as i32).map(|item| reaper.item_guid(item));
            let mut position = self.resume_number(cursor, total, key)?;
            let mut next = None;
            while position < total {
                if !pager.open() {
                    next = Some(position);
                    break;
                }
                if let Some(item) = reaper.track_item(track, position as i32) {
                    if !pager.push(rows::item_row(reaper, proj, item)) {
                        next = Some(position);
                        break;
                    }
                }
                position += 1;
            }
            return self.page_keyed(pager, next, Some(total), key);
        }
        // Every item in the project: the cursor is `track.item`, and the GUID of that item.
        let not_a_cursor = || bad("cursor: not one this kind of read gave");
        let tracks = reaper.track_count(proj) as usize;
        let item_key = |track_at: usize, item_at: usize| {
            let item = reaper.track_at(proj, track_at as i32).and_then(|track| reaper.track_item(track, item_at as i32))?;
            Some(reaper.item_guid(item))
        };
        let (mut track_at, mut item_at) = match self.cursor_parts(cursor)? {
            None => (0usize, 0usize),
            Some((position, key)) => {
                let (t, i) = position.split_once('.').ok_or_else(not_a_cursor)?;
                let (t, i): (usize, usize) = (t.parse().map_err(|_| not_a_cursor())?, i.parse().map_err(|_| not_a_cursor())?);
                match key {
                    None if t <= tracks => (t, i),
                    // The item the cursor points at, where it is now (or gone).
                    Some(key) if t < tracks && item_key(t, i).as_deref() == Some(key.as_str()) => (t, i),
                    Some(key) => (0..tracks)
                        .find_map(|t| {
                            let count = reaper.track_at(proj, t as i32).map_or(0, |track| reaper.track_item_count(track)) as usize;
                            (0..count).find(|&i| item_key(t, i).as_deref() == Some(key.as_str())).map(|i| (t, i))
                        })
                        .ok_or_else(|| {
                            changed("the item that cursor pointed at is gone: the items changed between pages, read again from the start")
                        })?,
                    None => return Err(changed("that cursor points past the last track: read again from the start")),
                }
            }
        };
        let mut next = None;
        'tracks: while track_at < tracks {
            if let Some(track) = reaper.track_at(proj, track_at as i32) {
                let count = reaper.track_item_count(track) as usize;
                while item_at < count {
                    if !pager.open() {
                        next = Some(Self::anchored(format!("{track_at}.{item_at}"), item_key(track_at, item_at)));
                        break 'tracks;
                    }
                    if let Some(item) = reaper.track_item(track, item_at as i32) {
                        if !pager.push(rows::item_row(reaper, proj, item)) {
                            next = Some(Self::anchored(format!("{track_at}.{item_at}"), item_key(track_at, item_at)));
                            break 'tracks;
                        }
                    }
                    item_at += 1;
                }
            }
            track_at += 1;
            item_at = 0;
        }
        self.page_at(pager, next, None)
    }

    fn read_takes(&self, reaper: &Reaper, proj: Project, mut pager: Pager, parent: Option<&str>, cursor: Option<&str>) -> Answer {
        let parent = parent.ok_or_else(|| bad("take: pass the item as parent"))?;
        let item = match Ref::parse(parent).map_err(bad)? {
            Ref::Item(guid) => self.item_by_guid(reaper, proj, &guid)?,
            other => return Err(bad(format!("parent: an item ref, not r:{}", other.kind()))),
        };
        let total = reaper.take_count(item) as usize;
        let active = reaper.active_take(item);
        let key = |position: usize| reaper.take_at(item, position as i32).map(|take| reaper.take_guid(take));
        let mut position = self.resume_number(cursor, total, key)?;
        let mut next = None;
        while position < total {
            if !pager.open() {
                next = Some(position);
                break;
            }
            if let Some(take) = reaper.take_at(item, position as i32) {
                if !pager.push(take_row(reaper, take, Some((position, active == Some(take))))) {
                    next = Some(position);
                    break;
                }
            }
            position += 1;
        }
        self.page_keyed(pager, next, Some(total), key)
    }

    // ----- FX and parameters -----

    fn read_fx(&self, reaper: &Reaper, proj: Project, mut pager: Pager, parent: Option<&str>, cursor: Option<&str>) -> Answer {
        let parent = parent.ok_or_else(|| bad("fx: pass the track, take or container as parent"))?;
        // The chain to list: top-level FX plus, on a track, its input chain; or one container's children.
        let chain: Vec<Fx> = match Ref::parse(parent).map_err(bad)? {
            Ref::Track(id) => {
                let track = self.track_by_id(reaper, proj, &id)?;
                let owner = FxOwner::Track(track);
                (0..reaper.fx_count(owner))
                    .map(|index| Fx { owner, index })
                    .chain((0..reaper.input_fx_count(track)).map(|index| Fx { owner, index: INPUT_FX | index }))
                    .collect()
            }
            Ref::Take(guid) => {
                let owner = FxOwner::Take(self.take_by_guid(reaper, proj, &guid)?);
                (0..reaper.fx_count(owner)).map(|index| Fx { owner, index }).collect()
            }
            Ref::Fx(guid) => reaper.fx_children(self.fx_by_guid(reaper, proj, &guid)?),
            other => return Err(bad(format!("parent: a track, take or FX ref, not r:{}", other.kind()))),
        };
        let total = chain.len();
        let key = |position: usize| chain.get(position).map(|fx| reaper.fx_guid(*fx));
        let mut position = self.resume_number(cursor, total, key)?;
        let mut next = None;
        while position < total {
            if !pager.open() {
                next = Some(position);
                break;
            }
            let fx = chain[position];
            self.registry.borrow_mut().note_fx(proj, &reaper.fx_guid(fx), fx);
            if !pager.push(rows::fx_row(reaper, fx, position)) {
                next = Some(position);
                break;
            }
            position += 1;
        }
        self.page_keyed(pager, next, Some(total), key)
    }

    fn read_params(
        &self,
        reaper: &Reaper,
        proj: Project,
        mut pager: Pager,
        parent: Option<&str>,
        cursor: Option<&str>,
        grid: Option<usize>,
    ) -> Answer {
        let parent = parent.ok_or_else(|| bad("param: pass the FX as parent"))?;
        let fx = self.parent_fx(reaper, proj, parent)?;
        let guid = reaper.fx_guid(fx);
        let total = reaper.fx_param_count(fx).max(0) as usize;
        let mut position = self.position_within(cursor, total)?;
        let mut next = None;
        while position < total {
            if !pager.open() {
                next = Some(position);
                break;
            }
            let mut row = rows::param_row(reaper, fx, &guid, position as i32);
            if let Some(points) = grid {
                crate::listening::reads::add_grid(reaper, fx, position as i32, points, &mut row);
            }
            if !pager.push(row) {
                next = Some(position);
                break;
            }
            position += 1;
        }
        self.page(pager, next, Some(total))
    }

    // ----- sends -----

    fn read_sends(&self, reaper: &Reaper, proj: Project, mut pager: Pager, parent: Option<&str>, cursor: Option<&str>) -> Answer {
        let parent = parent.ok_or_else(|| bad("send: pass the track as parent"))?;
        let track = self.parent_track(reaper, proj, parent)?;
        // Sends, then receives, then hardware outputs.
        let all: Vec<(i32, i32, rows::SendKey)> = [0, -1, 1]
            .into_iter()
            .flat_map(|category| {
                rows::send_keys(reaper, track, category).into_iter().enumerate().map(move |(index, key)| (category, index as i32, key))
            })
            .collect();
        let this = rows::track_id(reaper, track);
        let key = |position: usize| {
            all.get(position).map(|(category, _, key)| refs::send_ref(&this, *category, &key.other, key.repeat, key.of, None))
        };
        let mut position = self.resume_number(cursor, all.len(), key)?;
        let mut next = None;
        while position < all.len() {
            if !pager.open() {
                next = Some(position);
                break;
            }
            let (category, index, key) = &all[position];
            if !pager.push(rows::send_row(reaper, track, *category, *index, key)) {
                next = Some(position);
                break;
            }
            position += 1;
        }
        self.page_keyed(pager, next, Some(all.len()), key)
    }

    // ----- markers and regions -----

    fn read_markers(&self, reaper: &Reaper, proj: Project, mut pager: Pager, regions: bool, cursor: Option<&str>) -> Answer {
        let total = reaper.marker_count(proj).max(0) as usize;
        let key = |position: usize| reaper.marker(proj, position as i32).map(|marker| marker.guid);
        let mut position = self.resume_number(cursor, total, key)?;
        let mut next = None;
        while position < total {
            let Some(marker) = reaper.marker(proj, position as i32) else { break };
            if marker.is_region == regions {
                if !pager.open() {
                    next = Some(position);
                    break;
                }
                if !pager.push(rows::marker_row(reaper, proj, &marker)) {
                    next = Some(position);
                    break;
                }
            }
            position += 1;
        }
        self.page_keyed(pager, next, None, key)
    }

    // ----- MIDI notes -----

    fn read_notes(&self, reaper: &Reaper, proj: Project, mut pager: Pager, parent: Option<&str>, cursor: Option<&str>) -> Answer {
        let parent = parent.ok_or_else(|| bad("note: pass the MIDI take as parent"))?;
        let take = match Ref::parse(parent).map_err(bad)? {
            Ref::Take(guid) => self.take_by_guid(reaper, proj, &guid)?,
            Ref::Item(guid) => {
                let item = self.item_by_guid(reaper, proj, &guid)?;
                reaper.active_take(item).ok_or_else(|| failed("that item has no take"))?
            }
            other => return Err(bad(format!("parent: a take ref, not r:{}", other.kind()))),
        };
        if !reaper.take_is_midi(take) {
            return Err(bad("that take isn't MIDI"));
        }
        // The notes as the changes see and write them, from the one buffer REAPER gives the take's MIDI in: `MIDI_GetNote`
        // ends two overlapping notes of one pitch at the first note-off, where the buffer pairs them in order, and a model
        // that reads a length and changes the note must see the length Kumi writes.
        let events = reaper.midi_events(take).ok_or_else(|| failed("REAPER wouldn't give the take's MIDI"))?;
        let notes = crate::midi::notes(&events);
        let item_qn = take_start_qn(reaper, proj, take);
        let mut position = self.position_within(cursor, notes.len())?;
        let mut next = None;
        while let Some(note) = notes.get(position) {
            if !pager.open() || !pager.push(rows::note_row(reaper, take, item_qn, position as i32, &note.midi())) {
                next = Some(position);
                break;
            }
            position += 1;
        }
        self.page(pager, next, Some(notes.len()))
    }

    /// A MIDI take's events that aren't notes (controllers, pitch bend, program changes, pressure, text, sysex), from the
    /// one buffer the changes edit, with the indexes the event changes take (from 0); the source's end marker isn't one.
    fn read_events(&self, reaper: &Reaper, proj: Project, mut pager: Pager, parent: Option<&str>, cursor: Option<&str>) -> Answer {
        let parent = parent.ok_or_else(|| bad("event: pass the MIDI take as parent"))?;
        let take = self.midi_take(reaper, proj, Some(parent))?;
        let buffer = reaper.midi_events(take).ok_or_else(|| failed("REAPER wouldn't give the take's MIDI"))?;
        let events = crate::changes::midi_events_of(&buffer);
        let item_qn = take_start_qn(reaper, proj, take);
        let mut position = self.position_within(cursor, events.len())?;
        let mut next = None;
        while let Some((event, _)) = events.get(position) {
            let beats = reaper.ppq_to_quarter_notes(take, event.pos as f64).map_or(0.0, |qn| qn - item_qn);
            if !pager.open() || !pager.push(crate::changes::midi_event_row(event, position, beats)) {
                next = Some(position);
                break;
            }
            position += 1;
        }
        self.page(pager, next, Some(events.len()))
    }

    // ----- envelopes -----

    /// A track's envelopes, each with the GUID its ref is made of (none if REAPER gives it none): in the order REAPER lists
    /// them, which changes as the producer shows and adds envelopes, so a ref never holds a position.
    fn track_envelopes(&self, reaper: &Reaper, track: Track) -> Vec<(Option<String>, Envelope)> {
        (0..reaper.track_envelope_count(track))
            .filter_map(|position| reaper.track_envelope(track, position))
            .map(|envelope| (reaper.envelope_guid(envelope), envelope))
            .collect()
    }

    fn read_envelopes(&self, reaper: &Reaper, proj: Project, mut pager: Pager, parent: Option<&str>, cursor: Option<&str>) -> Answer {
        let parent = parent.ok_or_else(|| bad("envelope: pass the track, take or FX as parent"))?;
        // m4-r-fxenv: a take's envelopes (and a take FX's).
        let take_fx = |guid: &str| self.fx_by_guid(reaper, proj, guid).is_ok_and(|fx| matches!(fx.owner, FxOwner::Take(_)));
        match Ref::parse(parent).map_err(bad)? {
            Ref::Take(_) | Ref::Item(_) => return self.read_take_envelopes(reaper, proj, pager, parent, cursor),
            Ref::Fx(guid) if take_fx(&guid) => return self.read_take_envelopes(reaper, proj, pager, parent, cursor),
            _ => {}
        }
        let found: Vec<(TrackId, Envelope)> = match Ref::parse(parent).map_err(bad)? {
            Ref::Track(id) => {
                let track = self.track_by_id(reaper, proj, &id)?;
                self.track_envelopes(reaper, track).into_iter().map(|(_, e)| (id.clone(), e)).collect()
            }
            Ref::Fx(guid) => {
                let fx = self.fx_by_guid(reaper, proj, &guid)?;
                let FxOwner::Track(track) = fx.owner else { return Err(bad("take FX envelopes aren't read yet")) };
                let id = rows::track_id(reaper, track);
                // The track's envelopes that are for this FX: asked of each envelope, not of every parameter of the FX.
                self.track_envelopes(reaper, track)
                    .into_iter()
                    .filter(|(_, e)| {
                        reaper
                            .envelope_owner(*e)
                            .is_some_and(|(_, index, _)| index >= 0 && reaper.fx_guid(Fx { owner: fx.owner, index }) == guid)
                    })
                    .map(|(_, e)| (id.clone(), e))
                    .collect()
            }
            other => return Err(bad(format!("parent: a track or FX ref, not r:{}", other.kind()))),
        };
        let key = |position: usize| found.get(position).and_then(|(_, envelope)| reaper.envelope_guid(*envelope));
        let mut position = self.resume_number(cursor, found.len(), key)?;
        let mut next = None;
        while position < found.len() {
            if !pager.open() {
                next = Some(position);
                break;
            }
            let (track, envelope) = &found[position];
            if !pager.push(self.track_envelope_row(reaper, proj, track, *envelope)) {
                next = Some(position);
                break;
            }
            position += 1;
        }
        self.page_keyed(pager, next, Some(found.len()), key)
    }

    // ----- M5 listening (m5-reaper): what plays where -----

    /// `read {kind: "spans", limit?, cursor?}`: the tracks in project order (64 by default, up to 200), each with the
    /// stretches where it plays, in quarter notes from the project start.
    fn read_spans(&self, reaper: &Reaper, params: &Value) -> Answer {
        self.expect_epoch(params)?;
        let (proj, _) = active(reaper)?;
        let limit = match params.get("limit") {
            None | Some(Value::Null) => crate::listening::reads::DEFAULT_LIMIT,
            Some(_) => limit_param(params)?,
        };
        let cursor = text_param(params, "cursor")?;
        let total = reaper.track_count(proj).max(0) as usize;
        let key = |position: usize| reaper.track_at(proj, position as i32).map(|track| reaper.track_guid(track));
        let mut position = self.resume_number(cursor.as_deref(), total, key)?;
        let mut pager = Pager::new(limit, ANSWER_BYTES);
        let mut next = None;
        while position < total {
            if !pager.open() {
                next = Some(position);
                break;
            }
            if let Some(track) = reaper.track_at(proj, position as i32) {
                if !pager.push(crate::listening::reads::spans_row(reaper, proj, track)) {
                    next = Some(position);
                    break;
                }
            }
            position += 1;
        }
        self.page_keyed(pager, next, Some(total), key)
    }

    // ----- snapshot -----

    /// The project on one screen: the project, the transport, the tracks (the first 200), the selection, the markers and
    /// the producer's setup.
    /// What doesn't fit is named in `truncated`, with the cursor for `read`.
    pub fn snapshot(&self, reaper: &Reaper) -> Answer {
        let (proj, info) = active(reaper)?;
        let mut tracks = Pager::new(SNAPSHOT_TRACKS + 1, SNAPSHOT_TRACK_BYTES);
        let next_track = self.track_page(reaper, proj, &mut tracks, 0);
        let mut markers = Pager::new(SNAPSHOT_MARKERS, ANSWER_BYTES - SNAPSHOT_TRACK_BYTES - 16 * 1024);
        let marker_total = reaper.marker_count(proj);
        let mut next_marker = None;
        for index in 0..marker_total {
            let Some(marker) = reaper.marker(proj, index) else { break };
            if !markers.open() || !markers.push(rows::marker_row(reaper, proj, &marker)) {
                next_marker = Some(index as usize);
                break;
            }
        }
        let mut answer = json!({
            "epoch": self.epoch(),
            "project": rows::project_row(reaper, proj, &info, &self.identity(proj), self.project_id(reaper, proj, &info).as_deref()),
            "transport": rows::transport(reaper, proj),
            "tracks": tracks.rows,
            "selection": focus::selection(self, reaper, proj),
            "markers": markers.rows,
            "setup": crate::setup::setup(reaper, proj),
        });
        let mut truncated = serde_json::Map::new();
        if let Some(next) = next_track {
            let cursor = self.cursor_text(&Self::anchored(next, self.track_key(reaper, proj, next)));
            truncated.insert("tracks".into(), json!({"total": reaper.track_count(proj) + 1, "nextCursor": cursor}));
        }
        if let Some(next) = next_marker {
            let key = reaper.marker(proj, next as i32).map(|marker| marker.guid);
            // Markers and regions share one list: read each kind with this cursor to get the rest of it.
            truncated.insert(
                "markers".into(),
                json!({"total": marker_total, "nextCursor": self.cursor_text(&Self::anchored(next, key)), "kinds": ["marker", "region"]}),
            );
        }
        if !truncated.is_empty() {
            answer["truncated"] = Value::Object(truncated);
        }
        Ok(answer)
    }
}

/// The position of the item a take is on, in quarter notes.
pub fn take_start_qn(reaper: &Reaper, proj: Project, take: Take) -> f64 {
    reaper.take_item(take).and_then(|item| reaper.quarter_notes_at(proj, reaper.item_value(item, "D_POSITION"))).unwrap_or(0.0)
}

/// Where an FX sits in its parent's chain (an input FX continues after the main chain).
fn chain_position(reaper: &Reaper, fx: Fx) -> usize {
    let siblings = match reaper.fx_config(fx, "parent_container").and_then(|p| p.trim().parse::<i32>().ok()) {
        Some(parent) => reaper.fx_children(Fx { owner: fx.owner, index: parent }),
        None => {
            let main = reaper.fx_count(fx.owner);
            let base = fx.index & !INPUT_FX;
            return if fx.index & INPUT_FX != 0 { main as usize + base as usize } else { base as usize };
        }
    };
    siblings.iter().position(|s| *s == fx).unwrap_or(0)
}

fn take_row(reaper: &Reaper, take: Take, place: Option<(usize, bool)>) -> Value {
    let mut row = json!({
        "ref": refs::take_ref(&reaper.take_guid(take)),
        "name": rows::clip(&reaper.take_name(take)),
        "source": rows::source_word(reaper, Some(take)),
        "fxCount": reaper.fx_count(FxOwner::Take(take)),
    });
    if let Some((index, active)) = place {
        row["index"] = index.into();
        row["active"] = active.into();
    }
    if let Some((notes, ccs, _)) = reaper.midi_counts(take).filter(|_| reaper.take_is_midi(take)) {
        row["noteCount"] = notes.into();
        row["ccCount"] = ccs.into();
    }
    item_rows::take_details(reaper, take, &mut row, place.is_none());
    row
}

/// The refs of a generic call: a ref in its arguments is the object it names, and an object REAPER gives back is named
/// by its ref.
pub(crate) struct CallRefs<'a, 'r> {
    pub reads: &'a Reads,
    pub reaper: &'a Reaper<'r>,
    pub proj: Project,
}

impl crate::reaper::generic::Handles for CallRefs<'_, '_> {
    fn find(&self, base: &str, reference: &str) -> Result<*mut std::ffi::c_void, String> {
        use crate::reaper::generic::{pointer, Handle};
        let parsed = Ref::parse(reference).map_err(|e| format!("{reference:?} isn't a ref: {e}"))?;
        let (reads, reaper, proj) = (self.reads, self.reaper, self.proj);
        let handle = match (base, parsed) {
            ("MediaTrack", Ref::Track(id)) => Handle::Track(reads.track_by_id(reaper, proj, &id).map_err(|e| e.1)?),
            ("MediaItem", Ref::Item(guid)) => Handle::Item(reads.item_by_ref(reaper, proj, &guid).map_err(|e| e.1)?),
            ("MediaItem_Take", Ref::Take(guid)) => Handle::Take(reads.take_by_ref(reaper, proj, &guid).map_err(|e| e.1)?),
            ("TrackEnvelope", Ref::Envelope { track, which }) => {
                Handle::Envelope(reads.envelope_by_ref(reaper, proj, &track, &which).map_err(|e| e.1)?)
            }
            // m4-r-fxenv
            ("TrackEnvelope", Ref::TakeEnvelope { take, guid }) => {
                let take = reads.take_by_ref(reaper, proj, &take).map_err(|e| e.1)?;
                let envelope = (0..reaper.take_envelope_count(take))
                    .filter_map(|i| reaper.take_envelope(take, i))
                    .find(|e| reaper.envelope_guid(*e).is_some_and(|g| g.eq_ignore_ascii_case(&guid)))
                    .ok_or("that take has no such envelope")?;
                Handle::Envelope(envelope)
            }
            (base, other) => return Err(format!("{reference} is r:{}, where a {base} is wanted", other.kind())),
        };
        Ok(pointer(handle))
    }

    fn name(&self, base: &str, pointer: *mut std::ffi::c_void) -> Value {
        self.reaper.ref_of(self.proj, base, pointer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reaper::fake::{self, FakeTrack};

    /// Reads against the lab project through the fake table.
    struct Lab {
        reads: Reads,
    }

    impl Lab {
        fn new() -> Lab {
            fake::lab_project();
            Lab { reads: Reads::default() }
        }

        fn read(&self, params: Value) -> Answer {
            let api = fake::api();
            self.reads.read(&Reaper::new(&api), &params)
        }

        fn rows(&self, params: Value) -> Vec<Value> {
            self.read(params).unwrap()["items"].as_array().unwrap().clone()
        }
    }

    #[test]
    fn the_project_is_one_row() {
        let lab = Lab::new();
        let answer = lab.read(json!({"kind": "project"})).unwrap();
        assert_eq!(answer["epoch"], lab.reads.epoch());
        let project = &answer["items"][0];
        assert_eq!(project["name"], "Song.rpp");
        assert_eq!(project["path"], "/lab/Projects/Song.rpp");
        assert_eq!(project["saved"], true);
        assert_eq!(project["tempo"], 120.0);
        assert_eq!(project["timeSignature"], json!([4, 4]));
        assert_eq!(project["length"], 64.0);
        assert_eq!(project["playState"], "stopped");
        assert_eq!(project["timeSelection"], Value::Null);
        assert_eq!(project["tabCount"], 1);
        // The Kumi project id the project carries, once it has one (named by its path until then); and what names it
        // while it stays open.
        assert_eq!(project["id"], path_id("/lab/Projects/Song.rpp"));
        assert_eq!(project["identity"], lab.reads.identity(Project::first()));
        assert!(!project["identity"].as_str().unwrap().contains('x'));
        fake::with(|s| {
            s.project_ext.insert(("kumi".into(), "project".into()), "0123456789abcdef0123456789abcdef".into());
        });
        assert_eq!(lab.rows(json!({"kind": "project"}))[0]["id"], "0123456789abcdef0123456789abcdef");
    }

    #[test]
    fn a_project_is_named_by_a_serial_until_another_takes_its_tab() {
        let lab = Lab::new();
        let identity = |lab: &Lab| lab.rows(json!({"kind": "project"}))[0]["identity"].as_str().unwrap().to_string();
        let (first, second) = (Project::first().addr(), Project::first().addr() + 16);
        let before = identity(&lab);
        // This process and a serial: never a pointer.
        assert_eq!(before, format!("{}:1", std::process::id()));
        assert_eq!(identity(&lab), before);
        // A different project takes the tab (File > New, Open): the next serial.
        lab.reads.project_replaced(Project::first());
        let after = identity(&lab);
        assert_eq!(after, format!("{}:2", std::process::id()));
        // Another tab is another project; going back to the first finds it as it was.
        fake::with(|s| {
            s.other_tabs = vec![0];
            s.project_id = 1;
        });
        lab.reads.keep_tabs(&[first, second]);
        assert_eq!(identity(&lab), format!("{}:3", std::process::id()));
        fake::with(|s| {
            s.other_tabs = vec![1];
            s.project_id = 0;
        });
        assert_eq!(identity(&lab), after);
        // A tab that closed is forgotten: its pointer, handed out again, is a new project.
        lab.reads.keep_tabs(&[first]);
        fake::with(|s| {
            s.other_tabs = vec![];
            s.project_id = 1;
        });
        assert_eq!(identity(&lab), format!("{}:4", std::process::id()));
    }

    #[test]
    fn tracks_come_master_first_with_folders_colours_and_counts() {
        let lab = Lab::new();
        fake::with(|s| {
            s.tracks[0].solo = 1;
            s.tracks[1].arm = true;
            s.tracks[2].mute = true;
        });
        fake::select_tracks(&[2]);
        let rows = lab.rows(json!({"kind": "track"}));
        assert_eq!(
            rows.iter().map(|r| r["ref"].as_str().unwrap()).collect::<Vec<_>>(),
            ["r:track:master", "r:track:{AAAA}", "r:track:{BBBB}", "r:track:{CCCC}", "r:track:{DDDD}"]
        );
        assert_eq!(rows[0]["master"], true);
        assert_eq!(rows[0]["index"], 0);
        let drums = &rows[1];
        assert_eq!(drums["index"], 1);
        assert_eq!(drums["name"], "Drums");
        assert_eq!((&drums["depth"], &drums["folder"]), (&json!(0), &json!(1)));
        assert_eq!(drums["color"], "#ff0000");
        assert_eq!(drums["volumeDb"], -6.02);
        assert_eq!(drums["pan"], -0.25);
        assert_eq!(
            (&drums["fxCount"], &drums["sends"], &drums["receives"], &drums["itemCount"]),
            (&json!(2), &json!(1), &json!(0), &json!(0))
        );
        assert_eq!(drums["solo"], true);
        // The children of the folder sit one deeper, and the track that closes it says so.
        assert_eq!(
            (&rows[2]["depth"], &rows[2]["folder"], &rows[2]["arm"], &rows[2]["color"]),
            (&json!(1), &json!(0), &json!(true), &Value::Null)
        );
        assert_eq!(
            (&rows[3]["depth"], &rows[3]["folder"], &rows[3]["mute"], &rows[3]["selected"]),
            (&json!(1), &json!(-1), &json!(true), &json!(true))
        );
        assert_eq!((&rows[4]["depth"], &rows[4]["receives"]), (&json!(0), &json!(1)));
    }

    #[test]
    fn a_page_ends_at_the_limit_and_the_cursor_goes_on_from_there() {
        let lab = Lab::new();
        let first = lab.read(json!({"kind": "track", "limit": 2})).unwrap();
        assert_eq!(first["items"].as_array().unwrap().len(), 2);
        assert_eq!(first["total"], 5);
        let second = lab.read(json!({"kind": "track", "limit": 2, "cursor": first["nextCursor"]})).unwrap();
        assert_eq!(second["items"][0]["ref"], "r:track:{BBBB}");
        let third = lab.read(json!({"kind": "track", "limit": 2, "cursor": second["nextCursor"]})).unwrap();
        assert_eq!(third["items"].as_array().unwrap().len(), 1);
        assert_eq!(third.get("nextCursor"), None);
    }

    #[test]
    fn a_cursor_from_another_epoch_or_made_up_is_refused() {
        let lab = Lab::new();
        let first = lab.read(json!({"kind": "track", "limit": 1})).unwrap();
        let cursor = first["nextCursor"].clone();
        lab.reads.bump_epoch();
        let (code, message) = lab.read(json!({"kind": "track", "cursor": cursor})).unwrap_err();
        assert_eq!(code, "bad-params");
        assert!(message.contains("from before the project tab changed"), "{message}");
        assert!(lab.read(json!({"kind": "track", "cursor": "7"})).unwrap_err().1.contains("nextCursor"));
        assert!(lab.read(json!({"kind": "track", "cursor": format!("c1.{}.x", lab.reads.epoch())})).unwrap_err().1.contains("cursor"));
    }

    /// A copy of the project in another tab has the same GUIDs: a ref that says where it came from is refused there.
    #[test]
    fn a_request_that_names_its_epoch_is_refused_as_stale_when_the_project_in_the_tab_changed() {
        let lab = Lab::new();
        let epoch = lab.reads.epoch();
        let read = |lab: &Lab, epoch: Value| lab.read(json!({"ref": "r:track:{AAAA}", "epoch": epoch}));
        assert_eq!(read(&lab, json!(epoch)).unwrap()["items"][0]["name"], "Drums");
        assert_eq!(read(&lab, Value::Null).unwrap()["items"][0]["name"], "Drums");
        assert_eq!(read(&lab, json!(5)).unwrap_err().0, "bad-params");
        // The producer switched to the Save As copy in another tab, which has the same GUIDs.
        lab.reads.bump_epoch();
        let (code, message) = read(&lab, json!(epoch)).unwrap_err();
        assert_eq!(code, "stale");
        assert!(message.contains(&epoch) && message.contains("read again"), "{message}");
        assert_eq!(lab.read(json!({"kind": "track", "epoch": epoch})).unwrap_err().0, "stale");
        // Without it a read answers about the project now in the tab, and says which epoch that is.
        assert_ne!(lab.read(json!({"ref": "r:track:{AAAA}"})).unwrap()["epoch"], json!(epoch));
        // The bodies are refused the same way.
        let api = fake::api();
        let body =
            |epoch: &str| lab.reads.focus_body(&Reaper::new(&api), &json!({"kind": "routing", "ref": "r:track:{AAAA}", "epoch": epoch}));
        assert_eq!(body(&epoch).unwrap_err().0, "stale");
        assert_eq!(body(&lab.reads.epoch()).unwrap()["kind"], "routing");
    }

    fn names(rows: &Value) -> Vec<String> {
        rows["items"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_string()).collect()
    }

    /// A cursor is a position, so an edit between two pages used to repeat a row or skip one without a word.
    #[test]
    fn a_cursor_goes_on_from_its_row_wherever_it_is_after_tracks_come_and_go() {
        let lab = Lab::new();
        let eight = || (1..=8).map(|n| FakeTrack::new(&format!("{{T{n}}}"), &format!("T{n}"))).collect::<Vec<_>>();
        fake::with(|s| s.tracks = eight());
        // The master, then T1, T2; the cursor points at T3.
        let first = lab.read(json!({"kind": "track", "limit": 3})).unwrap();
        assert_eq!(names(&first), ["MASTER", "T1", "T2"]);
        let cursor = first["nextCursor"].clone();
        let next = |lab: &Lab| lab.read(json!({"kind": "track", "limit": 3, "cursor": cursor.clone()}));
        // A track inserted at the top: T2 isn't read twice, T3 isn't skipped.
        fake::with(|s| s.tracks.insert(0, FakeTrack::new("{NEW}", "New")));
        assert_eq!(names(&next(&lab).unwrap()), ["T3", "T4", "T5"]);
        // The first track deleted instead (put back, then taken out): T3 is still next.
        fake::with(|s| s.tracks = eight());
        fake::with(|s| {
            s.tracks.remove(0);
        });
        assert_eq!(names(&next(&lab).unwrap()), ["T3", "T4", "T5"]);
        // The row the cursor points at is deleted: the list changed under it, and the page says so.
        fake::with(|s| s.tracks.retain(|t| t.guid != "{T3}"));
        let (code, message) = next(&lab).unwrap_err();
        assert_eq!(code, "changed");
        assert!(message.contains("read again from the start"), "{message}");
    }

    #[test]
    fn items_fx_markers_and_sends_go_on_from_their_row_too() {
        use crate::reaper::fake::{FakeFx, FakeItem, FakeMarker, FakeSend};
        let lab = Lab::new();
        fake::with(|s| {
            s.tracks = vec![FakeTrack::new("{A}", "A"), FakeTrack::new("{B}", "B")];
            let item = |n: u32, at: f64| FakeItem::new(&format!("{{I{n}}}"), at, 1.0, vec![]);
            s.tracks[0].items = (1..=4).map(|n| item(n, n as f64)).collect();
            s.tracks[1].items = vec![item(5, 1.0), item(6, 2.0)];
            s.tracks[0].fx = (1..=4).map(|n| FakeFx::new(&format!("{{F{n}}}"), &format!("fx{n}"))).collect();
            s.markers = (1..=4)
                .map(|n| FakeMarker {
                    guid: format!("{{M{n}}}"),
                    is_region: false,
                    start: n as f64,
                    end: n as f64,
                    name: format!("m{n}"),
                    number: n,
                    color: 0,
                })
                .collect();
        });
        let refs =
            |rows: &Value| rows["items"].as_array().unwrap().iter().map(|r| r["ref"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        // The items of the whole project: two on a page, the cursor points at I3. An item added before it, in front of the
        // track's others, doesn't make I2 come again.
        let first = lab.read(json!({"kind": "item", "limit": 2})).unwrap();
        assert_eq!(refs(&first), ["r:item:{I1}", "r:item:{I2}"]);
        fake::with(|s| s.tracks[0].items.insert(0, FakeItem::new("{I0}", 0.0, 1.0, vec![])));
        let second = lab.read(json!({"kind": "item", "limit": 2, "cursor": first["nextCursor"]})).unwrap();
        assert_eq!(refs(&second), ["r:item:{I3}", "r:item:{I4}"]);
        // The same under one track.
        let first = lab.read(json!({"kind": "item", "parent": "r:track:{A}", "limit": 2})).unwrap();
        assert_eq!(refs(&first), ["r:item:{I0}", "r:item:{I1}"]);
        fake::with(|s| {
            s.tracks[0].items.remove(0);
        });
        let second = lab.read(json!({"kind": "item", "parent": "r:track:{A}", "limit": 2, "cursor": first["nextCursor"]})).unwrap();
        assert_eq!(refs(&second), ["r:item:{I2}", "r:item:{I3}"]);
        // An item deleted that the cursor pointed at.
        fake::with(|s| s.tracks[0].items.retain(|i| i.guid != "{I2}"));
        let gone = lab.read(json!({"kind": "item", "parent": "r:track:{A}", "limit": 2, "cursor": first["nextCursor"]})).unwrap_err();
        assert_eq!(gone.0, "changed");
        // FX.
        let first = lab.read(json!({"kind": "fx", "parent": "r:track:{A}", "limit": 2})).unwrap();
        fake::with(|s| s.tracks[0].fx.insert(0, FakeFx::new("{F0}", "fx0")));
        let second = lab.read(json!({"kind": "fx", "parent": "r:track:{A}", "limit": 2, "cursor": first["nextCursor"]})).unwrap();
        assert_eq!(refs(&second), ["r:fx:{F3}", "r:fx:{F4}"]);
        // Markers.
        let first = lab.read(json!({"kind": "marker", "limit": 2})).unwrap();
        fake::with(|s| s.markers.insert(0, FakeMarker { guid: "{M0}".into(), name: "m0".into(), ..s.markers[0].clone() }));
        let second = lab.read(json!({"kind": "marker", "limit": 2, "cursor": first["nextCursor"]})).unwrap();
        assert_eq!(refs(&second), ["r:marker:{M3}", "r:marker:{M4}"]);
        // Sends: two to one track, then one more is made before the cursor.
        fake::with(|s| s.tracks[0].sends = vec![FakeSend::to(1), FakeSend::to(1)]);
        let first = lab.read(json!({"kind": "send", "parent": "r:track:{A}", "limit": 1})).unwrap();
        fake::with(|s| s.tracks[0].sends.push(FakeSend::to(1)));
        // The pair has three now: the cursor named the first of two, which is no row any more.
        let (code, _) = lab.read(json!({"kind": "send", "parent": "r:track:{A}", "limit": 1, "cursor": first["nextCursor"]})).unwrap_err();
        assert_eq!(code, "changed");
    }

    /// A position past the count wrapped to a negative i32 and ran REAPER's main thread through tens of thousands of
    /// calls; a cursor that points past the end of a list is refused instead.
    #[test]
    fn a_cursor_past_the_end_is_refused_not_walked() {
        let lab = Lab::new();
        let epoch = lab.reads.epoch();
        let far = format!("c1.{epoch}.{}", u32::MAX as u64 - 3);
        let farther = format!("c1.{epoch}.{}", 1u64 << 31);
        fake::with(|s| s.calls = 0);
        for cursor in [&far, &farther] {
            for params in [
                json!({"kind": "marker", "cursor": cursor}),
                json!({"kind": "track", "cursor": cursor}),
                json!({"kind": "note", "parent": "r:take:{TAKE-S1}", "cursor": cursor}),
                json!({"kind": "param", "parent": "r:fx:{FX-EQ}", "cursor": cursor}),
            ] {
                let (code, _) = lab.read(params.clone()).unwrap_err();
                assert_eq!(code, "changed", "{params}");
            }
        }
        // The MIDI body too, and the arrange body.
        let api = fake::api();
        let body = |params: Value| lab.reads.focus_body(&Reaper::new(&api), &params);
        assert_eq!(body(json!({"kind": "midi", "ref": "r:take:{TAKE-S1}", "cursor": far})).unwrap_err().0, "changed");
        assert_eq!(body(json!({"kind": "arrange", "cursor": farther})).unwrap_err().0, "changed");
        let calls = fake::with(|s| s.calls);
        assert!(calls < 200, "{calls} calls to refuse a cursor");
    }

    fn set_track(index: usize, values: &[(&str, f64)]) {
        fake::with(|s| {
            for (key, value) in values {
                s.tracks[index].extra.insert((*key).into(), *value);
            }
        });
    }

    /// What a mixer strip shows beyond the fader, and what a power user checks before they change a track.
    #[test]
    fn a_track_row_says_how_it_pans_records_and_is_shown_and_which_folder_it_is_in() {
        let lab = Lab::new();
        let rows = lab.rows(json!({"kind": "track"}));
        let row = |n: usize| &rows[n];
        // The defaults of a new track; Kick is in Drums' folder, Drums is open, and the master has no parent.
        let kick = rows.iter().find(|r| r["name"] == "Kick").unwrap();
        assert_eq!(
            (&kick["channels"], &kick["panMode"], &kick["width"], &kick["panLaw"], &kick["dualPan"]),
            (&json!(2), &json!("balance"), &json!(1.0), &Value::Null, &Value::Null)
        );
        assert_eq!(
            (&kick["phaseInverted"], &kick["fxEnabled"], &kick["shownInArrange"], &kick["shownInMixer"], &kick["folderState"]),
            (&json!(false), &json!(true), &json!(true), &json!(true), &Value::Null)
        );
        assert_eq!(
            (&kick["parentRef"], &kick["recordInput"], &kick["recordMode"], &kick["monitoring"]),
            (&json!("r:track:{AAAA}"), &json!({"kind": "none"}), &json!("input"), &json!("off"))
        );
        assert_eq!((&kick["automationMode"], &kick["itemLayout"], kick.get("lanes")), (&json!("trim"), &json!("normal"), None));
        assert_eq!(rows.iter().find(|r| r["name"] == "Drums").unwrap()["parentRef"], Value::Null);
        assert_eq!(rows.iter().find(|r| r["name"] == "Drums").unwrap()["folderState"], "open");
        assert_eq!(row(0)["master"], true);
        assert_eq!(row(0).get("parentRef"), None);
        // A dual-pan track, narrowed, with a pan law, inverted, its FX chain bypassed, hidden in the arrange view, a
        // collapsed folder, recording MIDI from every device on channel 3, monitoring, overdubbing, in touch mode.
        set_track(
            1,
            &[
                ("I_PANMODE", 6.0),
                ("D_DUALPANL", -1.0),
                ("D_DUALPANR", 0.5),
                ("D_WIDTH", 0.25),
                ("D_PANLAW", 0.5),
                ("B_PHASE", 1.0),
                ("I_FXEN", 0.0),
                ("B_SHOWINTCP", 0.0),
                ("I_NCHAN", 6.0),
                ("I_RECINPUT", (4096 | (63 << 5) | 3) as f64),
                ("I_RECMODE", 7.0),
                ("I_RECMON", 1.0),
                ("I_AUTOMODE", 2.0),
            ],
        );
        set_track(0, &[("I_FOLDERCOMPACT", 2.0)]);
        let rows = lab.rows(json!({"kind": "track"}));
        let kick = rows.iter().find(|r| r["name"] == "Kick").unwrap();
        assert_eq!(
            (&kick["panMode"], &kick["dualPan"], &kick["width"], &kick["panLaw"], &kick["channels"]),
            (&json!("dual"), &json!({"left": -1.0, "right": 0.5}), &json!(0.25), &json!(0.5), &json!(6))
        );
        assert_eq!(
            (&kick["phaseInverted"], &kick["fxEnabled"], &kick["shownInArrange"], &kick["shownInMixer"]),
            (&json!(true), &json!(false), &json!(false), &json!(true))
        );
        assert_eq!(kick["recordInput"], json!({"kind": "midi", "channel": 3, "device": "all"}));
        assert_eq!(
            (&kick["recordMode"], &kick["monitoring"], &kick["automationMode"]),
            (&json!("midi-overdub"), &json!("on"), &json!("touch"))
        );
        assert_eq!(rows.iter().find(|r| r["name"] == "Drums").unwrap()["folderState"], "fully-collapsed");
        // A new track follows the project's default pan mode (REAPER stores -1).
        set_track(1, &[("I_PANMODE", -1.0)]);
        assert_eq!(lab.rows(json!({"ref": "r:track:{BBBB}"}))[0]["panMode"], "default");
        // Audio inputs, mono and stereo and multichannel, and the virtual keyboard.
        for (value, expected) in [
            (512.0, json!({"kind": "audio", "first": 513, "layout": "mono"})),
            ((1024 | 2) as f64, json!({"kind": "audio", "first": 3, "layout": "stereo"})),
            ((2048 | 4) as f64, json!({"kind": "audio", "first": 5, "layout": "multichannel"})),
            ((4096 | (62 << 5)) as f64, json!({"kind": "midi", "channel": 0, "device": "virtual-keyboard"})),
            ((4096 | (3 << 5) | 16) as f64, json!({"kind": "midi", "channel": 16, "device": "3"})),
        ] {
            set_track(1, &[("I_RECINPUT", value)]);
            assert_eq!(lab.rows(json!({"ref": "r:track:{BBBB}"}))[0]["recordInput"], expected, "{value}");
        }
    }

    #[test]
    fn a_track_with_fixed_lanes_names_them_says_which_play_and_its_items_say_if_theirs_does() {
        let lab = Lab::new();
        set_track(
            1,
            &[("I_FREEMODE", 2.0), ("I_NUMFIXEDLANES", 3.0), ("C_LANEPLAYS:0", 0.0), ("C_LANEPLAYS:1", 1.0), ("C_LANEPLAYS:2", 2.0)],
        );
        fake::with(|s| {
            s.tracks[1].lane_names = vec!["Take 1".into(), "Take 2".into(), "Take 3".into()];
            s.tracks[1].items[0].lane = 1;
            s.tracks[1].items[0].extra.insert("C_LANEPLAYS".into(), 1.0);
            s.tracks[1].items[1].extra.insert("C_LANEPLAYS".into(), -1.0);
        });
        let track = &lab.rows(json!({"ref": "r:track:{BBBB}"}))[0];
        assert_eq!(track["itemLayout"], "lanes");
        assert_eq!(
            track["lanes"],
            json!([
                {"index": 0, "name": "Take 1", "plays": "no"},
                {"index": 1, "name": "Take 2", "plays": "only"},
                {"index": 2, "name": "Take 3", "plays": "with-others"},
            ])
        );
        let items = lab.rows(json!({"kind": "item", "parent": "r:track:{BBBB}"}));
        assert_eq!((&items[0]["lane"], &items[0]["lanePlays"], &items[1]["lanePlays"]), (&json!(1), &json!("only"), &json!("hidden")));
        // Items on a track without fixed lanes have no lane to play.
        let drums = lab.rows(json!({"kind": "item", "parent": "r:track:{CCCC}"}));
        assert_eq!(drums[0]["lanePlays"], Value::Null);
    }

    #[test]
    fn a_send_says_its_phase_mono_midi_and_automation_and_links_its_envelopes() {
        use crate::reaper::fake::FakeEnvelope;
        let lab = Lab::new();
        fake::with(|s| {
            let send = &mut s.tracks[0].sends[0];
            send.extra.insert("B_PHASE".into(), 1.0);
            send.extra.insert("B_MONO".into(), 1.0);
            send.extra.insert("I_AUTOMODE".into(), 3.0);
            send.extra.insert("I_MIDIFLAGS".into(), (31) as f64);
            s.tracks[0].envelopes.push(FakeEnvelope::for_send("Send Volume: Reverb", 0, "<VOLENV", 3));
            s.tracks[0].envelopes.push(FakeEnvelope::for_send("Send Mute: Reverb", 0, "<MUTEENV", 1));
            // REAPER hands out a pan envelope for the send too, with no points and not in use: it isn't listed.
            s.tracks[0].envelopes.push(FakeEnvelope { active: false, ..FakeEnvelope::for_send("Send Pan: Reverb", 0, "<PANENV", 0) });
        });
        let sends = lab.rows(json!({"kind": "send", "parent": "r:track:{AAAA}"}));
        let send = &sends[0];
        assert_eq!((&send["phaseInverted"], &send["mono"], &send["automationMode"]), (&json!(true), &json!(true), &json!("write")));
        assert_eq!(send["midi"], json!({"enabled": false, "srcChannel": 0, "dstChannel": 0, "srcBus": 0, "dstBus": 0, "faders": false}));
        assert_eq!(send["envelopeRefs"], json!(["r:env:{AAAA}:{ENV-SEND0-VOLENV}", "r:env:{AAAA}:{ENV-SEND0-MUTEENV}"]));
        // The ref reads the envelope.
        assert_eq!(lab.rows(json!({"ref": "r:env:{AAAA}:{ENV-SEND0-VOLENV}"}))[0]["name"], "Send Volume: Reverb");
        // The defaults of a send: MIDI on, from every channel to the original, the track's own automation mode.
        fake::with(|s| s.tracks[0].sends[0].extra.clear());
        let send = &lab.rows(json!({"kind": "send", "parent": "r:track:{AAAA}"}))[0];
        assert_eq!(send["midi"]["enabled"], true);
        assert_eq!((&send["automationMode"], &send["phaseInverted"], &send["mono"]), (&json!("track"), &json!(false), &json!(false)));
        // Channels 3 to 5, buses 2 and 3, the faders sending MIDI volume and pan.
        let flags = 3 | (5 << 5) | 1024 | (2 << 14) | (3 << 22);
        fake::with(|s| s.tracks[0].sends[0].extra.insert("I_MIDIFLAGS".into(), flags as f64));
        let send = &lab.rows(json!({"kind": "send", "parent": "r:track:{AAAA}"}))[0];
        assert_eq!(send["midi"], json!({"enabled": true, "srcChannel": 3, "dstChannel": 5, "srcBus": 2, "dstBus": 3, "faders": true}));
        // A receive has MIDI flags too; a hardware output has no MIDI to speak of.
        let reverb = lab.rows(json!({"kind": "send", "parent": "r:track:{DDDD}"}));
        assert!(reverb[0].get("midi").is_some() && reverb[1].get("midi").is_none() && reverb[0].get("envelopeRefs").is_none());
    }

    /// A time signature counts its own beats (eighths in 6/8); the transport counts quarter notes throughout.
    #[test]
    fn the_beat_in_a_bar_counts_quarter_notes_in_any_time_signature() {
        let lab = Lab::new();
        fake::with(|s| {
            s.time_signature = (6, 8);
            s.cursor = 0.75; // 1.5 quarter notes in at 120 bpm: three eighths
        });
        let transport = snapshot(&lab).unwrap()["transport"].clone();
        assert_eq!((&transport["bar"], &transport["beatInBar"], &transport["beatsPerBar"]), (&json!(1), &json!(2.5), &json!(3.0)));
    }

    #[test]
    fn errors_do_not_quote_the_refs_they_are_about() {
        let lab = Lab::new();
        for reference in [
            "r:track:{NOPE}",
            "r:item:{NOPE}",
            "r:take:{NOPE}",
            "r:fx:{NOPE}",
            "r:param:{FX-EQ}:99",
            "r:send:{AAAA}:0:{NOPE}",
            "r:marker:{NOPE}",
            "r:region:{NOPE}",
            "r:env:{AAAA}:{NOPE}",
        ] {
            let (_, message) = lab.read(json!({"ref": reference})).unwrap_err();
            assert!(!message.contains("{NOPE}") && !message.contains("r:"), "{reference}: {message}");
        }
    }

    #[test]
    fn an_item_is_drawn_in_its_active_takes_own_colour_when_it_has_one() {
        let lab = Lab::new();
        let red = fake::native_color(255, 0, 0);
        let green = fake::native_color(0, 255, 0);
        fake::with(|s| s.tracks[1].items[0].color = green);
        assert_eq!(lab.rows(json!({"ref": "r:item:{ITEM-K1}"}))[0]["color"], "#00ff00");
        fake::with(|s| {
            s.tracks[1].items[0].takes[0].extra.insert("I_CUSTOMCOLOR".into(), red as f64);
        });
        assert_eq!(lab.rows(json!({"ref": "r:item:{ITEM-K1}"}))[0]["color"], "#ff0000");
        // And in the arrange body.
        let api = fake::api();
        let body = lab.reads.focus_body(&Reaper::new(&api), &json!({"kind": "arrange"})).unwrap();
        let kick = body["tracks"].as_array().unwrap().iter().find(|t| t["name"] == "Kick").unwrap().clone();
        assert_eq!(kick["items"][0]["color"], "#ff0000");
    }

    /// REAPER keeps a GUID through a copy made as text (a script setting one track's chunk on another): the registry kept
    /// the last object it found, and a ref read whichever that was.
    #[test]
    fn a_guid_two_objects_have_names_neither_until_one_is_gone() {
        use crate::reaper::fake::FakeFx;
        let lab = Lab::new();
        // A track copied as text: the same GUID on its track, its items and its FX.
        fake::with(|s| {
            let mut copy = s.tracks[1].clone();
            copy.name = "Kick copy".into();
            s.tracks.push(copy);
        });
        for (reference, kind) in [("r:track:{BBBB}", "track"), ("r:item:{ITEM-K1}", "item")] {
            let (code, message) = lab.read(json!({"ref": reference})).unwrap_err();
            assert_eq!(code, "failed", "{reference}");
            assert!(message.contains(&format!("two {kind}s in the project have that id")), "{message}");
        }
        // The track's own rows still list both, whatever their refs say.
        assert_eq!(lab.rows(json!({"kind": "track"})).iter().filter(|t| t["ref"] == "r:track:{BBBB}").count(), 2);
        // The copy is removed: the refs name the one that is left, and the registry has forgotten the clash.
        fake::with(|s| {
            s.tracks.pop();
        });
        assert_eq!(lab.rows(json!({"ref": "r:track:{BBBB}"}))[0]["name"], "Kick");
        assert_eq!(lab.rows(json!({"ref": "r:item:{ITEM-K1}"}))[0]["name"], "kick.wav");
        // The same FX on two tracks.
        fake::with(|s| s.tracks[1].fx = vec![FakeFx::new("{FX-EQ}", "VST3: ReaEQ (Cockos)")]);
        let (code, message) = lab.read(json!({"ref": "r:fx:{FX-EQ}"})).unwrap_err();
        assert_eq!(code, "failed");
        assert!(message.contains("two FXs in the project have that id"), "{message}");
    }

    #[test]
    fn limits_default_to_fifty_and_stop_at_two_hundred() {
        let lab = Lab::new();
        fake::with(|s| s.tracks = (0..260).map(|i| FakeTrack::new(&format!("{{T{i}}}"), "t")).collect());
        assert_eq!(lab.rows(json!({"kind": "track"})).len(), DEFAULT_LIMIT);
        assert_eq!(lab.rows(json!({"kind": "track", "limit": 200})).len(), MAX_LIMIT);
        // Over the most is refused, not quietly cut, as Renoise's tool does.
        for bad_limit in [json!(0), json!(-1), json!("ten"), json!(1.5), json!(201), json!(500)] {
            assert_eq!(lab.read(json!({"kind": "track", "limit": bad_limit})).unwrap_err().0, "bad-params");
        }
    }

    #[test]
    fn items_of_a_track_and_of_the_whole_project() {
        let lab = Lab::new();
        let kick = lab.rows(json!({"kind": "item", "parent": "r:track:{BBBB}"}));
        assert_eq!(kick.len(), 2);
        assert_eq!(kick[0]["ref"], "r:item:{ITEM-K1}");
        assert_eq!((&kick[0]["position"], &kick[0]["length"], &kick[0]["name"]), (&json!(0.0), &json!(2.0), &json!("kick.wav")));
        assert_eq!(
            (&kick[0]["source"], &kick[0]["takeRef"], &kick[0]["trackRef"]),
            (&json!("wav"), &json!("r:take:{TAKE-K1}"), &json!("r:track:{BBBB}"))
        );
        assert_eq!(
            (&kick[0]["muted"], &kick[0]["lane"], &kick[0]["color"], &kick[0]["selected"]),
            (&json!(false), &json!(0), &Value::Null, &json!(false))
        );
        // Without a parent: every item, in track order, paged by a cursor that names track and item.
        let first = lab.read(json!({"kind": "item", "limit": 2})).unwrap();
        assert_eq!(
            first["items"].as_array().unwrap().iter().map(|r| r["ref"].as_str().unwrap()).collect::<Vec<_>>(),
            ["r:item:{ITEM-K1}", "r:item:{ITEM-K2}"]
        );
        let rest = lab.read(json!({"kind": "item", "limit": 2, "cursor": first["nextCursor"]})).unwrap();
        assert_eq!(rest["items"][0]["ref"], "r:item:{ITEM-S1}");
        assert_eq!(rest["items"][0]["source"], "mid");
        assert_eq!(rest.get("nextCursor"), None);
    }

    #[test]
    fn an_item_without_a_take_is_empty() {
        let lab = Lab::new();
        fake::with(|s| s.tracks[1].items[0].takes.clear());
        let row = &lab.rows(json!({"kind": "item", "parent": "r:track:{BBBB}"}))[0];
        assert_eq!((&row["source"], &row["name"], &row["takeRef"]), (&json!("empty"), &json!(""), &Value::Null));
    }

    #[test]
    fn takes_of_an_item() {
        let lab = Lab::new();
        let takes = lab.rows(json!({"kind": "take", "parent": "r:item:{ITEM-S1}"}));
        assert_eq!(takes.len(), 1);
        assert_eq!(
            (&takes[0]["ref"], &takes[0]["source"], &takes[0]["active"], &takes[0]["noteCount"]),
            (&json!("r:take:{TAKE-S1}"), &json!("mid"), &json!(true), &json!(2))
        );
    }

    #[test]
    fn fx_chains_list_containers_with_their_children_and_containers_list_their_own() {
        let lab = Lab::new();
        let chain = lab.rows(json!({"kind": "fx", "parent": "r:track:{AAAA}"}));
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0]["ref"], "r:fx:{FX-EQ}");
        assert_eq!(
            (&chain[0]["name"], &chain[0]["enabled"], &chain[0]["preset"], &chain[0]["paramCount"], &chain[0]["container"]),
            (&json!("VST3: ReaEQ (Cockos)"), &json!(true), &json!("Warm"), &json!(3), &json!(false))
        );
        assert_eq!(chain[0].get("childRefs"), None);
        assert_eq!((&chain[1]["container"], &chain[1]["childRefs"]), (&json!(true), &json!(["r:fx:{FX-COMP}", "r:fx:{FX-C2}"])));
        let children = lab.rows(json!({"kind": "fx", "parent": "r:fx:{FX-C}"}));
        assert_eq!(children.iter().map(|r| r["ref"].as_str().unwrap()).collect::<Vec<_>>(), ["r:fx:{FX-COMP}", "r:fx:{FX-C2}"]);
        let nested = lab.rows(json!({"kind": "fx", "parent": "r:fx:{FX-C2}"}));
        assert_eq!(nested[0]["ref"], "r:fx:{FX-DLY}");
        assert_eq!(nested[0]["container"], false);
    }

    #[test]
    fn the_fx_lookup_survives_a_reorder_and_says_so_when_the_fx_is_gone() {
        let lab = Lab::new();
        assert_eq!(lab.rows(json!({"kind": "param", "parent": "r:fx:{FX-EQ}"})).len(), 3);
        // The chain is reordered: the cached address no longer holds that FX.
        fake::with(|s| s.tracks[0].fx.reverse());
        assert_eq!(lab.rows(json!({"kind": "param", "parent": "r:fx:{FX-EQ}"}))[0]["ref"], "r:param:{FX-EQ}:0");
        fake::with(|s| s.tracks[0].fx.clear());
        let (code, message) = lab.read(json!({"kind": "param", "parent": "r:fx:{FX-EQ}"})).unwrap_err();
        assert_eq!(code, "failed");
        assert!(message.contains("isn't in the project"), "{message}");
    }

    #[test]
    fn take_fx_are_found_by_guid_too() {
        let lab = Lab::new();
        fake::with(|s| s.tracks[1].items[1].takes[0].fx.push(crate::reaper::fake::FakeFx::new("{FX-TAKE}", "JS: Volume")));
        let rows = lab.rows(json!({"kind": "fx", "parent": "r:take:{TAKE-K2}"}));
        assert_eq!(rows[0]["ref"], "r:fx:{FX-TAKE}");
        assert_eq!(lab.rows(json!({"kind": "param", "parent": "r:fx:{FX-TAKE}"})).len(), 0);
    }

    #[test]
    fn parameters_have_their_value_text_and_range() {
        let lab = Lab::new();
        let params = lab.rows(json!({"kind": "param", "parent": "r:fx:{FX-EQ}"}));
        assert_eq!(
            params[1],
            json!({"ref": "r:param:{FX-EQ}:1", "index": 1, "name": "Band 1 Gain", "value": 0.5, "formatted": "0.0 dB", "min": 0.0, "max": 1.0})
        );
        let page = lab.read(json!({"kind": "param", "parent": "r:fx:{FX-EQ}", "limit": 2})).unwrap();
        assert_eq!((&page["total"], page["nextCursor"].is_string()), (&json!(3), true));
    }

    #[test]
    fn sends_receives_and_hardware_outputs_are_rows_of_the_track() {
        let lab = Lab::new();
        let out = lab.rows(json!({"kind": "send", "parent": "r:track:{AAAA}"}));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["ref"], "r:send:{AAAA}:0:{DDDD}");
        assert_eq!(
            (&out[0]["destRef"], &out[0]["trackRef"], &out[0]["direction"]),
            (&json!("r:track:{DDDD}"), &json!("r:track:{AAAA}"), &json!("send"))
        );
        assert_eq!(
            (&out[0]["volumeDb"], &out[0]["mode"], &out[0]["modeName"], &out[0]["mute"]),
            (&json!(0.0), &json!(0), &json!("post-fader"), &json!(false))
        );
        assert_eq!(out[0]["channels"], json!({"src": {"first": 0, "count": 2}, "dst": {"first": 0, "mono": false}}));
        let reverb = lab.rows(json!({"kind": "send", "parent": "r:track:{DDDD}"}));
        assert_eq!(
            reverb.iter().map(|r| r["ref"].as_str().unwrap()).collect::<Vec<_>>(),
            ["r:send:{DDDD}:-1:{AAAA}", "r:send:{DDDD}:1:hw2"]
        );
        // A receive's other track is where it comes from.
        assert_eq!(
            (&reverb[0]["destRef"], &reverb[0]["trackRef"], &reverb[0]["direction"]),
            (&json!("r:track:{AAAA}"), &json!("r:track:{DDDD}"), &json!("receive"))
        );
        assert_eq!((&reverb[1]["destRef"], &reverb[1]["category"], &reverb[1]["direction"]), (&Value::Null, &json!(1), &json!("hardware")));
        assert_eq!(reverb[1]["channels"]["dst"]["first"], 2);
    }

    #[test]
    fn send_channels_and_modes_are_decoded() {
        let lab = Lab::new();
        fake::with(|s| {
            s.tracks[0].sends[0].mode = 3;
            s.tracks[0].sends[0].src_chan = 2 << 10 | 2;
            s.tracks[0].sends[0].dst_chan = 2 | 1024;
            s.tracks[0].sends[0].volume = 0.25;
        });
        let row = &lab.rows(json!({"kind": "send", "parent": "r:track:{AAAA}"}))[0];
        assert_eq!((&row["mode"], &row["modeName"], &row["volumeDb"]), (&json!(3), &json!("pre-fader"), &json!(-12.04)));
        assert_eq!(row["channels"], json!({"src": {"first": 2, "count": 4}, "dst": {"first": 2, "mono": true}}));
        fake::with(|s| s.tracks[0].sends[0].src_chan = -1);
        assert_eq!(lab.rows(json!({"kind": "send", "parent": "r:track:{AAAA}"}))[0]["channels"]["src"], Value::Null);
    }

    #[test]
    fn markers_and_regions_are_read_apart_with_the_numbers_reaper_shows() {
        let lab = Lab::new();
        let markers = lab.rows(json!({"kind": "marker"}));
        assert_eq!(
            markers,
            [
                json!({"ref": "r:marker:{MARK-1}", "number": 1, "name": "Drop", "position": 4.0, "positionBeats": 8.0, "color": Value::Null, "lane": 0})
            ]
        );
        let regions = lab.rows(json!({"kind": "region"}));
        assert_eq!(
            regions,
            [
                json!({"ref": "r:region:{REG-1}", "number": 1, "name": "Verse", "position": 8.0, "positionBeats": 16.0, "end": 16.0, "endBeats": 32.0, "color": "#00ff00", "lane": 0})
            ]
        );
        // The ruler lane a marker sits in is read (accept-reaper lab: marker-lane's change couldn't be read back).
        fake::with(|s| s.marker_lanes.insert("{MARK-1}".into(), 1));
        assert_eq!(lab.rows(json!({"kind": "marker"}))[0]["lane"], 1);
    }

    #[test]
    fn notes_are_in_quarter_notes_from_the_start_of_the_item() {
        let lab = Lab::new();
        // The item starts at 8 s; at 120 bpm that is quarter note 16.
        let notes = lab.rows(json!({"kind": "note", "parent": "r:take:{TAKE-S1}"}));
        assert_eq!(
            notes[0],
            json!({"index": 0, "pitch": 38, "start": 0.0, "length": 0.5, "velocity": 100, "channel": 1, "selected": false, "muted": false})
        );
        assert_eq!(
            (&notes[1]["start"], &notes[1]["length"], &notes[1]["channel"], &notes[1]["selected"]),
            (&json!(1.0), &json!(0.5), &json!(10), &json!(true))
        );
        // The item works as a parent too (its active take).
        assert_eq!(lab.rows(json!({"kind": "note", "parent": "r:item:{ITEM-S1}"})).len(), 2);
        let (code, message) = lab.read(json!({"kind": "note", "parent": "r:take:{TAKE-K1}"})).unwrap_err();
        assert_eq!(code, "bad-params");
        assert!(message.contains("isn't MIDI"), "{message}");
    }

    #[test]
    fn envelopes_are_named_by_their_guid_and_an_fx_ones_say_which_fx_and_parameter() {
        let lab = Lab::new();
        let envelopes = lab.rows(json!({"kind": "envelope", "parent": "r:track:{AAAA}"}));
        assert_eq!(
            envelopes.iter().map(|r| r["ref"].as_str().unwrap()).collect::<Vec<_>>(),
            ["r:env:{AAAA}:{ENV-VOLENV2}", "r:env:{AAAA}:{ENV-FX-EQ-1}"]
        );
        assert_eq!(
            (&envelopes[0]["name"], &envelopes[0]["pointCount"], &envelopes[0]["armed"], &envelopes[0]["visible"]),
            (&json!("Volume"), &json!(4), &json!(false), &json!(true))
        );
        assert_eq!((&envelopes[0]["fxRef"], &envelopes[0]["param"]), (&Value::Null, &Value::Null));
        assert_eq!((&envelopes[1]["fxRef"], &envelopes[1]["param"]), (&json!("r:fx:{FX-EQ}"), &json!(1)));
        // An FX lists its own.
        let of_fx = lab.rows(json!({"kind": "envelope", "parent": "r:fx:{FX-EQ}"}));
        assert_eq!(of_fx.len(), 1);
        assert_eq!(of_fx[0]["ref"], "r:env:{AAAA}:{ENV-FX-EQ-1}");
        // And a ref reads back to the same row; the chunk name and the FX-and-parameter forms are aliases of it.
        assert_eq!(lab.rows(json!({"ref": "r:env:{AAAA}:{ENV-VOLENV2}"}))[0], envelopes[0]);
        assert_eq!(lab.rows(json!({"ref": "r:env:{AAAA}:{ENV-FX-EQ-1}"}))[0], envelopes[1]);
        assert_eq!(lab.rows(json!({"ref": "r:env:{AAAA}:VOLENV2"}))[0], envelopes[0]);
        assert_eq!(lab.rows(json!({"ref": "r:env:{AAAA}:{FX-EQ}:1"}))[0], envelopes[1]);
        // The GUID is the track's own to name: another track's envelope isn't read through it.
        assert_eq!(lab.read(json!({"ref": "r:env:{BBBB}:{ENV-VOLENV2}"})).unwrap_err().0, "failed");
    }

    /// Send, dual-pan, tempo and the like have no chunk name a ref could use, and an FX envelope was found by probing the
    /// parameters: every envelope was named by its place in the track's list, which others coming and going change.
    #[test]
    fn an_envelope_ref_holds_when_other_envelopes_come_and_go() {
        use crate::reaper::fake::FakeEnvelope;
        let lab = Lab::new();
        let send = |name: &str, guid: &str, points: i32| FakeEnvelope {
            chunk: None,
            fx_param: None,
            guid: guid.into(),
            ..FakeEnvelope::own(name, "", points)
        };
        fake::with(|s| {
            s.tracks[1].envelopes =
                vec![send("Send Volume: Delay", "{ENV-SEND-DELAY}", 3), send("Send Volume: Reverb", "{ENV-SEND-REVERB}", 9)]
        });
        let rows = lab.rows(json!({"kind": "envelope", "parent": "r:track:{BBBB}"}));
        assert_eq!(
            rows.iter().map(|r| r["ref"].as_str().unwrap()).collect::<Vec<_>>(),
            ["r:env:{BBBB}:{ENV-SEND-DELAY}", "r:env:{BBBB}:{ENV-SEND-REVERB}"]
        );
        let name_of = |reference: &str| lab.rows(json!({"ref": reference}))[0]["name"].clone();
        assert_eq!(name_of("r:env:{BBBB}:{ENV-SEND-REVERB}"), json!("Send Volume: Reverb"));
        // The producer shows the track's own Volume envelope, which REAPER lists first.
        fake::with(|s| s.tracks[1].envelopes.insert(0, FakeEnvelope::own("Volume", "<VOLENV2", 0)));
        assert_eq!(name_of("r:env:{BBBB}:{ENV-SEND-REVERB}"), json!("Send Volume: Reverb"));
        // The producer removes the Delay send and its envelope: its ref names nothing, and Reverb's still reads Reverb.
        fake::with(|s| s.tracks[1].envelopes.retain(|e| e.guid != "{ENV-SEND-DELAY}"));
        assert_eq!(lab.read(json!({"ref": "r:env:{BBBB}:{ENV-SEND-DELAY}"})).unwrap_err().0, "failed");
        assert_eq!(name_of("r:env:{BBBB}:{ENV-SEND-REVERB}"), json!("Send Volume: Reverb"));
    }

    #[test]
    fn an_fx_with_thousands_of_parameters_is_not_probed_parameter_by_parameter() {
        use crate::reaper::fake::{FakeEnvelope, FakeFx, FakeParam};
        let lab = Lab::new();
        fake::with(|s| {
            let mut big = FakeFx::new("{FX-BIG}", "VST3: Big (Vendor)");
            big.params = (0..8000).map(|i| FakeParam::new(&format!("Param {i}"), 0.5, "0.5")).collect();
            s.tracks[1].fx = vec![big];
            s.tracks[1].envelopes = vec![FakeEnvelope::for_param("Param 7000", "{FX-BIG}", 7000, 2)];
        });
        fake::with(|s| s.calls = 0);
        let rows = lab.rows(json!({"kind": "envelope", "parent": "r:track:{BBBB}"}));
        assert_eq!(rows[0]["ref"], "r:env:{BBBB}:{ENV-FX-BIG-7000}");
        assert_eq!((&rows[0]["fxRef"], &rows[0]["param"]), (&json!("r:fx:{FX-BIG}"), &json!(7000)));
        let calls = fake::with(|s| s.calls);
        assert!(calls < 100, "{calls} calls to find one envelope");
    }

    // The optimization audit's R5: every track row read fifty group masks (25 roles, low and high masks), nearly always
    // all zero, and the host never read them from a list. The one track asked about still has them.
    #[test]
    fn a_lists_rows_leave_the_groups_out_and_the_one_track_asked_about_has_them() {
        let lab = Lab::new();
        let guid = fake::with(|s| s.tracks[1].guid.clone());
        fake::extra(|e| e.groups.insert((guid, "VOLUME_VCA_FOLLOW".into(), false), 1 | 4));
        let calls = || fake::with(|s| s.calls);
        let before = calls();
        let rows = lab.rows(json!({"kind": "track"}));
        let listed = calls() - before;
        assert!(rows.iter().all(|row| row.get("groups").is_none()), "{rows:?}");
        let kick = rows.iter().find(|row| row["name"] == "Kick").unwrap();
        let before = calls();
        let one = lab.rows(json!({"ref": kick["ref"]}));
        let single = calls() - before;
        assert_eq!(one[0]["groups"], json!([{"group": 1, "roles": ["vca-follow"]}, {"group": 3, "roles": ["vca-follow"]}]));
        assert!(single >= listed / rows.len() as u64 + 50, "{single} calls for the one track, {listed} for {} rows", rows.len());
    }

    #[test]
    fn one_object_by_its_ref() {
        let lab = Lab::new();
        for (reference, field, expected) in [
            ("r:track:{CCCC}", "name", json!("Snare")),
            ("r:track:master", "master", json!(true)),
            ("r:item:{ITEM-K2}", "position", json!(2.0)),
            ("r:take:{TAKE-K1}", "name", json!("kick.wav")),
            ("r:fx:{FX-DLY}", "name", json!("VST: ReaDelay (Cockos)")),
            ("r:param:{FX-EQ}:2", "name", json!("Wet")),
            ("r:send:{DDDD}:-1:{AAAA}", "category", json!(-1)),
            ("r:marker:{MARK-1}", "name", json!("Drop")),
            ("r:region:{REG-1}", "end", json!(16.0)),
        ] {
            let rows = lab.rows(json!({"ref": reference}));
            assert_eq!(rows.len(), 1, "{reference}");
            assert_eq!(rows[0][field], expected, "{reference}");
        }
    }

    #[test]
    fn a_ref_to_something_gone_or_never_there_fails_and_a_malformed_one_is_bad_params() {
        let lab = Lab::new();
        for reference in [
            "r:track:{NOPE}",
            "r:item:{NOPE}",
            "r:take:{NOPE}",
            "r:fx:{NOPE}",
            "r:param:{FX-EQ}:99",
            "r:send:{AAAA}:0:{NOPE}",
            "r:send:{AAAA}:0:{DDDD}:2of2",
            "r:send:{AAAA}:1:hw5",
            "r:marker:{NOPE}",
            "r:region:{NOPE}",
            // A marker's GUID isn't a region's.
            "r:region:{MARK-1}",
            "r:marker:{REG-1}",
            "r:env:{AAAA}:{NOPE}",
        ] {
            let (code, message) = lab.read(json!({"ref": reference})).unwrap_err();
            assert_eq!(code, "failed", "{reference}: {message}");
        }
        // REAPER hands out a track's built-in envelope before it is shown (m4-r-fxenv, lab 7.82): hidden, off, no points.
        let pan = lab.read(json!({"ref": "r:env:{AAAA}:PANENV"})).unwrap();
        assert_eq!(
            (pan["items"][0]["active"].clone(), pan["items"][0]["visible"].clone(), pan["items"][0]["pointCount"].clone()),
            (json!(false), json!(false), json!(0))
        );
        for reference in
            ["track:{AAAA}", "r:pattern:3", "r:track:", "r:param:{FX-EQ}:x", "r:send:{AAAA}:0:5", "r:send:{AAAA}:0", "r:marker:3"]
        {
            assert_eq!(lab.read(json!({"ref": reference})).unwrap_err().0, "bad-params", "{reference}");
        }
    }

    #[test]
    fn requests_that_name_no_kind_or_the_wrong_parent_say_what_to_pass() {
        let lab = Lab::new();
        assert!(lab.read(json!({})).unwrap_err().1.contains("kind"));
        assert!(lab.read(json!({"kind": "pattern"})).unwrap_err().1.contains("no kind"));
        assert!(lab.read(json!({"kind": "param"})).unwrap_err().1.contains("pass the FX as parent"));
        assert!(lab.read(json!({"kind": "fx"})).unwrap_err().1.contains("parent"));
        assert!(lab.read(json!({"kind": "send", "parent": "r:fx:{FX-EQ}"})).unwrap_err().1.contains("track ref"));
        assert!(lab.read(json!({"kind": "item", "parent": "r:take:{TAKE-K1}"})).unwrap_err().1.contains("track ref"));
        assert!(lab.read(json!({"kind": 3})).unwrap_err().1.contains("kind"));
        assert!(lab.read(json!({"kind": "track", "parent": ""})).unwrap_err().1.contains("parent"));
    }

    #[test]
    fn a_track_that_was_deleted_is_not_confused_with_the_one_that_took_its_place() {
        let lab = Lab::new();
        assert_eq!(lab.rows(json!({"ref": "r:track:{CCCC}"}))[0]["name"], "Snare");
        // Deleting Kick moves Snare to index 1: the old handle points at index 2, which is now Reverb.
        fake::with(|s| {
            s.tracks.remove(1);
        });
        assert_eq!(lab.rows(json!({"ref": "r:track:{CCCC}"}))[0]["index"], 2);
        assert_eq!(lab.read(json!({"ref": "r:track:{BBBB}"})).unwrap_err().0, "failed");
        // An item whose track is gone is gone.
        assert_eq!(lab.read(json!({"ref": "r:item:{ITEM-K1}"})).unwrap_err().0, "failed");
    }

    #[test]
    fn a_ref_that_stays_missing_does_not_walk_the_whole_project_each_time() {
        let lab = Lab::new();
        fake::with(|s| s.calls = 0);
        for _ in 0..2 {
            assert!(lab.read(json!({"ref": "r:item:{NOPE}"})).is_err());
        }
        let after_two = fake::with(|s| s.calls);
        for _ in 0..5 {
            assert!(lab.read(json!({"ref": "r:item:{NOPE}"})).is_err());
        }
        let after_seven = fake::with(|s| s.calls);
        // The first miss walked the items; the rest answered from that.
        assert!(after_seven - after_two < 20 * 5, "{after_two} then {after_seven}");
    }

    #[test]
    fn a_page_never_passes_the_frame_bound() {
        let lab = Lab::new();
        // Names at the clip: 200 rows of ~1 KiB, ~220 KiB of rows alone.
        let long = "x".repeat(2000);
        fake::with(|s| s.tracks = (0..300).map(|i| FakeTrack::new(&format!("{{T{i}}}"), &long)).collect());
        let answer = lab.read(json!({"kind": "track", "limit": 200})).unwrap();
        let size = serde_json::to_string(&answer).unwrap().len();
        assert!(size <= 256 * 1024, "{size}");
        let rows = answer["items"].as_array().unwrap().len();
        assert!(rows < 200 && rows > 100, "{rows}");
        // Every track is still reachable by following the cursors.
        let mut seen = rows;
        let mut cursor = answer["nextCursor"].clone();
        while cursor.is_string() {
            let page = lab.read(json!({"kind": "track", "limit": 200, "cursor": cursor})).unwrap();
            assert!(serde_json::to_string(&page).unwrap().len() <= 256 * 1024);
            seen += page["items"].as_array().unwrap().len();
            cursor = page["nextCursor"].clone();
        }
        assert_eq!(seen, 301);
        // A name past the clip is cut with an ellipsis.
        assert!(answer["items"][1]["name"].as_str().unwrap().ends_with('…'));
    }

    #[test]
    fn the_lookups_stay_cheap_in_a_big_project() {
        fake::lab_project();
        fake::with(|s| {
            s.tracks = (0..200)
                .map(|t| {
                    let mut track = FakeTrack::new(&format!("{{T{t}}}"), &format!("Track {t}"));
                    track.items = (0..10)
                        .map(|i| {
                            fake::FakeItem::new(
                                &format!("{{I{t}-{i}}}"),
                                i as f64,
                                1.0,
                                vec![fake::FakeTake::wave(&format!("{{K{t}-{i}}}"), "x")],
                            )
                        })
                        .collect();
                    track
                })
                .collect();
        });
        let reads = Reads::default();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let ask = |reference: &str| reads.read(&reaper, &json!({"ref": reference})).unwrap();
        // The first item lookup walks all 2,000 items; the ones after it are one validation and one GUID read.
        assert_eq!(ask("r:item:{I199-9}")["items"][0]["position"], 9.0);
        fake::with(|s| s.calls = 0);
        for t in [0, 57, 199] {
            ask(&format!("r:item:{{I{t}-3}}"));
        }
        let calls = fake::with(|s| s.calls);
        // (a row reads some 50 attributes, an item's fades and lock among them; a walk would be thousands)
        assert!(calls < 3 * 60, "{calls} calls for three cached items");
    }

    fn snapshot(lab: &Lab) -> Answer {
        let api = fake::api();
        lab.reads.snapshot(&Reaper::new(&api))
    }

    #[test]
    fn the_snapshot_is_the_project_on_one_screen() {
        let lab = Lab::new();
        fake::select_tracks(&[1]);
        fake::with(|s| {
            s.cursor = 2.0;
            s.play_bits = 1;
            s.play_position = 6.0;
            s.tracks[1].items[0].selected = true;
        });
        let snapshot = snapshot(&lab).unwrap();
        assert_eq!(snapshot["epoch"], lab.reads.epoch());
        assert_eq!(snapshot["project"]["name"], "Song.rpp");
        let transport = &snapshot["transport"];
        assert_eq!((&transport["playing"], &transport["seconds"], &transport["beat"]), (&json!(true), &json!(6.0), &json!(12.0)));
        assert_eq!((&transport["bar"], &transport["beatInBar"], &transport["beatsPerBar"]), (&json!(4), &json!(1.0), &json!(4.0)));
        assert_eq!((&transport["tempo"], &transport["timeSignature"]), (&json!(120.0), &json!([4, 4])));
        assert_eq!(transport["loop"], json!({"on": false, "start": 0.0, "end": 0.0}));
        let tracks = snapshot["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 5);
        assert_eq!(tracks[0]["ref"], "r:track:master");
        let selection = &snapshot["selection"];
        assert_eq!(
            (&selection["trackRef"], &selection["itemRef"], &selection["cursor"]),
            (&json!("r:track:{BBBB}"), &json!("r:item:{ITEM-K1}"), &json!(2.0))
        );
        assert_eq!(selection["track"], json!({"name": "Kick", "color": Value::Null, "type": "audio"}));
        assert_eq!(selection["itemName"], "kick.wav");
        let markers: Vec<&str> = snapshot["markers"].as_array().unwrap().iter().map(|m| m["ref"].as_str().unwrap()).collect();
        assert_eq!(markers, ["r:marker:{MARK-1}", "r:region:{REG-1}"]);
        assert_eq!(snapshot.get("truncated"), None);
    }

    #[test]
    fn a_snapshot_of_a_big_project_names_what_it_leaves_for_read() {
        let lab = Lab::new();
        fake::with(|s| {
            s.tracks = (0..320).map(|i| FakeTrack::new(&format!("{{T{i}}}"), "t")).collect();
            s.markers = (0..150)
                .map(|i| fake::FakeMarker {
                    guid: format!("{{M{i}}}"),
                    is_region: false,
                    start: i as f64,
                    end: i as f64,
                    name: "m".into(),
                    number: i + 1,
                    color: 0,
                })
                .collect();
        });
        let snapshot = snapshot(&lab).unwrap();
        assert_eq!(snapshot["tracks"].as_array().unwrap().len(), SNAPSHOT_TRACKS + 1);
        assert_eq!(snapshot["markers"].as_array().unwrap().len(), SNAPSHOT_MARKERS);
        assert_eq!(snapshot["truncated"]["tracks"]["total"], 321);
        assert_eq!(snapshot["truncated"]["markers"]["total"], 150);
        // The cursors go to `read`: the rest of the tracks, and the rest of the markers.
        let rest = lab.read(json!({"kind": "track", "limit": 200, "cursor": snapshot["truncated"]["tracks"]["nextCursor"]})).unwrap();
        assert_eq!(rest["items"].as_array().unwrap().len(), 120);
        assert_eq!(rest["items"][0]["ref"], "r:track:{T200}");
        let more = lab.read(json!({"kind": "marker", "cursor": snapshot["truncated"]["markers"]["nextCursor"]})).unwrap();
        assert_eq!(more["items"].as_array().unwrap().len(), 50);
    }

    #[test]
    fn a_snapshot_stays_inside_the_frame_bound_whatever_the_names() {
        let lab = Lab::new();
        let long = "n".repeat(5000);
        fake::with(|s| {
            s.tracks = (0..260).map(|i| FakeTrack::new(&format!("{{T{i}}}"), &long)).collect();
            s.markers = (0..150)
                .map(|i| fake::FakeMarker {
                    guid: format!("{{M{i}}}"),
                    is_region: true,
                    start: i as f64,
                    end: i as f64 + 1.0,
                    name: long.clone(),
                    number: i + 1,
                    color: 0,
                })
                .collect();
        });
        let snapshot = snapshot(&lab).unwrap();
        let size = serde_json::to_string(&snapshot).unwrap().len();
        assert!(size <= 256 * 1024, "{size}");
        assert!(snapshot["truncated"]["tracks"].is_object() && snapshot["truncated"]["markers"].is_object());
    }

    #[test]
    fn no_project_open_is_a_failure_not_a_panic() {
        let lab = Lab::new();
        fake::with(|s| s.project = None);
        assert_eq!(snapshot(&lab).unwrap_err().0, "failed");
        assert_eq!(lab.read(json!({"kind": "track"})).unwrap_err().0, "failed");
    }

    #[test]
    fn several_sends_to_one_track_are_told_apart_by_which_of_how_many_and_a_reorder_keeps_them() {
        use crate::reaper::fake::FakeSend;
        let lab = Lab::new();
        let mut loud = FakeSend::to(3);
        loud.volume = 0.5;
        let mut louder = FakeSend::to(3);
        louder.volume = 0.25;
        fake::with(|s| s.tracks[0].sends = vec![FakeSend::to(3), FakeSend::to(1), loud, louder]);
        let rows = lab.rows(json!({"kind": "send", "parent": "r:track:{AAAA}"}));
        let refs: Vec<&str> = rows.iter().map(|r| r["ref"].as_str().unwrap()).collect();
        // The three sends to Reverb carry the state of the three as one mark; the send alone to Kick carries none.
        let seen = refs[0].rsplit_once('.').unwrap().1;
        assert_eq!(seen.len(), 8);
        assert_eq!(
            refs,
            [
                format!("r:send:{{AAAA}}:0:{{DDDD}}:1of3.{seen}"),
                "r:send:{AAAA}:0:{BBBB}".into(),
                format!("r:send:{{AAAA}}:0:{{DDDD}}:2of3.{seen}"),
                format!("r:send:{{AAAA}}:0:{{DDDD}}:3of3.{seen}"),
            ]
        );
        let volume = |lab: &Lab, reference: &str| lab.rows(json!({"ref": reference}))[0]["volumeDb"].clone();
        assert_eq!(volume(&lab, refs[3]), json!(-12.04));
        // A send to another track moves to the front: the others keep their refs, and so their sends.
        fake::with(|s| s.tracks[0].sends.swap(0, 1));
        assert_eq!(volume(&lab, refs[2]), json!(-6.02));
        assert_eq!(volume(&lab, refs[3]), json!(-12.04));
        // A ref without the state of the sends it is one of names none of them.
        assert_eq!(lab.read(json!({"ref": "r:send:{AAAA}:0:{DDDD}:3of3"})).unwrap_err().0, "failed");
        // The producer changes one of the sends to Reverb: a ref read before reads none of them, never the send at its place now.
        fake::with(|s| s.tracks[0].sends[1].volume = 0.9);
        let (code, message) = lab.read(json!({"ref": refs[3]})).unwrap_err();
        assert_eq!((code.as_str(), message.contains("read the track's sends again")), ("failed", true));
        assert_eq!(lab.rows(json!({"ref": "r:send:{AAAA}:0:{BBBB}"}))[0]["index"], 0);
    }

    /// REAPER gives a send no identity: once the number of sends to one place changes, a ref to one of them could name any,
    /// so it names none (a position would read the other send, whose own ref would then fail).
    #[test]
    fn removing_one_of_two_sends_to_a_track_leaves_its_ref_and_the_others_naming_nothing() {
        use crate::reaper::fake::FakeSend;
        let lab = Lab::new();
        let mut quiet = FakeSend::to(3);
        quiet.volume = 0.25;
        fake::with(|s| s.tracks[0].sends = vec![FakeSend::to(3), quiet]);
        let rows = lab.rows(json!({"kind": "send", "parent": "r:track:{AAAA}"}));
        let refs: Vec<&str> = rows.iter().map(|r| r["ref"].as_str().unwrap()).collect();
        let plain: Vec<&str> = refs.iter().map(|r| r.rsplit_once('.').unwrap().0).collect();
        assert_eq!(plain, ["r:send:{AAAA}:0:{DDDD}:1of2", "r:send:{AAAA}:0:{DDDD}:2of2"]);
        assert_eq!(lab.rows(json!({"ref": refs[0]}))[0]["volumeDb"], json!(0.0));
        // The producer deletes the 0 dB send. Its ref no longer reads the quiet one, and the quiet one's own ref says why.
        fake::with(|s| {
            s.tracks[0].sends.remove(0);
        });
        for reference in refs {
            let (code, message) = lab.read(json!({"ref": reference})).unwrap_err();
            assert_eq!(code, "failed", "{reference}");
            assert!(message.contains("1 such send now") && message.contains("there were 2"), "{message}");
        }
        // What is read now has the plain ref of a send alone.
        assert_eq!(lab.rows(json!({"kind": "send", "parent": "r:track:{AAAA}"}))[0]["ref"], "r:send:{AAAA}:0:{DDDD}");
        // A send added to a pair that had one: the ref made then reads nothing either.
        fake::with(|s| s.tracks[0].sends.push(FakeSend::to(3)));
        let (_, message) = lab.read(json!({"ref": "r:send:{AAAA}:0:{DDDD}"})).unwrap_err();
        assert!(message.contains("2 such sends now") && message.contains("when there was one"), "{message}");
        // Receives are told apart the same way, with the pair's state as their send's refs carry it (M4).
        let reverb = lab.rows(json!({"kind": "send", "parent": "r:track:{DDDD}"}));
        assert!(reverb[0]["ref"].as_str().unwrap().starts_with("r:send:{DDDD}:-1:{AAAA}:1of2."), "{}", reverb[0]["ref"]);
    }

    /// A hardware output is named by the output it goes to, so removing another output doesn't rename it.
    #[test]
    fn hardware_outputs_are_named_by_the_output_not_by_their_place_in_the_list() {
        use crate::reaper::fake::FakeSend;
        let lab = Lab::new();
        let out = |dst_chan: i32, volume: f64| FakeSend { dst_chan, volume, ..FakeSend::to(0) };
        fake::with(|s| s.tracks[3].hw_outs = vec![out(0, 1.0), out(2, 0.5), out(3 | 1024, 0.25)]);
        let rows = lab.rows(json!({"kind": "send", "parent": "r:track:{DDDD}"}));
        let refs: Vec<&str> = rows.iter().filter(|r| r["category"] == 1).map(|r| r["ref"].as_str().unwrap()).collect();
        assert_eq!(refs, ["r:send:{DDDD}:1:hw0", "r:send:{DDDD}:1:hw2", "r:send:{DDDD}:1:hw3m"]);
        let volume = |reference: &str| lab.rows(json!({"ref": reference}))[0]["volumeDb"].clone();
        assert_eq!(volume("r:send:{DDDD}:1:hw2"), json!(-6.02));
        // The producer deletes Out 1/2. Its ref reads nothing; the others still read their own output.
        fake::with(|s| {
            s.tracks[3].hw_outs.remove(0);
        });
        let (code, _) = lab.read(json!({"ref": "r:send:{DDDD}:1:hw0"})).unwrap_err();
        assert_eq!(code, "failed");
        assert_eq!(volume("r:send:{DDDD}:1:hw2"), json!(-6.02));
        assert_eq!(volume("r:send:{DDDD}:1:hw3m"), json!(-12.04));
        // Two outputs to the same channels are told apart like sends to one track.
        fake::with(|s| s.tracks[3].hw_outs.push(out(2, 0.125)));
        let rows = lab.rows(json!({"kind": "send", "parent": "r:track:{DDDD}"}));
        let refs: Vec<&str> = rows.iter().filter(|r| r["category"] == 1).map(|r| r["ref"].as_str().unwrap()).collect();
        assert_eq!(refs, ["r:send:{DDDD}:1:hw2:1of2", "r:send:{DDDD}:1:hw3m", "r:send:{DDDD}:1:hw2:2of2"]);
        assert_eq!(volume("r:send:{DDDD}:1:hw2:2of2"), json!(-18.06));
    }

    #[test]
    fn markers_with_the_same_number_have_refs_of_their_own() {
        let lab = Lab::new();
        fake::with(|s| {
            s.markers.push(fake::FakeMarker {
                guid: "{MARK-2}".into(),
                is_region: false,
                start: 12.0,
                end: 12.0,
                name: "Another one 1".into(),
                number: 1,
                color: 0,
            });
        });
        let markers = lab.rows(json!({"kind": "marker"}));
        assert_eq!(markers.iter().map(|m| m["ref"].as_str().unwrap()).collect::<Vec<_>>(), ["r:marker:{MARK-1}", "r:marker:{MARK-2}"]);
        assert_eq!(markers[0]["number"], markers[1]["number"]);
        assert_eq!(lab.rows(json!({"ref": "r:marker:{MARK-2}"}))[0]["name"], "Another one 1");
        // The number is editable: the ref doesn't follow it.
        fake::with(|s| s.markers[2].number = 9);
        assert_eq!(lab.rows(json!({"ref": "r:marker:{MARK-2}"}))[0]["number"], 9);
    }

    /// Every answer, run through the contract's naming rule: a ref sits only under `ref`, `parent`, `…Ref` or `…Refs`.
    #[test]
    fn every_answer_names_refs_the_way_the_contract_says() {
        use crate::focus::Focus;
        use daw_bridge::contract::misplaced_refs;
        let lab = Lab::new();
        fake::with(|s| {
            s.tracks[1].items[0].selected = true;
            s.tracks[2].selected = true;
            s.touched_fx = Some((0, -1, -1, 0, 1));
            s.focused_fx = Some((0, -1, -1, 0, 0));
            s.tracks[2].items[0].takes[0].fx.push(crate::reaper::fake::FakeFx::new("{FX-TAKE}", "JS: Volume"));
        });
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let mut answers: Vec<(String, Value)> = Vec::new();
        let expect = |label: String, answer: Answer| (label.clone(), answer.unwrap_or_else(|e| panic!("{label}: {e:?}")));
        // Every kind at every parent that has some, and every row read again by its own ref.
        let mut refs_seen: Vec<String> = Vec::new();
        for kind in ["project", "track", "item", "marker", "region"] {
            let answer = lab.reads.read(&reaper, &json!({"kind": kind}));
            if let Ok(page) = &answer {
                refs_seen.extend(page["items"].as_array().unwrap().iter().filter_map(|r| r["ref"].as_str().map(str::to_string)));
            }
            answers.push(expect(format!("read {kind}"), answer));
        }
        // Each ref found is a parent in its turn: tracks to items and FX, FX to parameters, containers to their children.
        let mut queue: Vec<String> = refs_seen.clone();
        let mut at = 0;
        while at < queue.len() {
            let parent = queue[at].clone();
            at += 1;
            let kinds: &[&str] = match parent.split(':').nth(1) {
                Some("track") => &["item", "fx", "send", "envelope"],
                Some("item") => &["take", "note"],
                Some("take") => &["fx", "note"],
                Some("fx") => &["fx", "param", "envelope"],
                _ => &[],
            };
            for kind in kinds {
                if let Ok(page) = lab.reads.read(&reaper, &json!({"kind": kind, "parent": parent})) {
                    for found in page["items"].as_array().unwrap().iter().filter_map(|r| r["ref"].as_str()) {
                        if !queue.iter().any(|known| known == found) {
                            queue.push(found.to_string());
                        }
                    }
                    answers.push((format!("read {kind} of {parent}"), page));
                }
            }
        }
        refs_seen = queue;
        refs_seen.sort();
        refs_seen.dedup();
        for reference in &refs_seen {
            if let Ok(page) = lab.reads.read(&reaper, &json!({"ref": reference})) {
                answers.push((format!("read {reference}"), page));
            }
        }
        answers.push(expect("snapshot".into(), lab.reads.snapshot(&reaper)));
        let project = reaper.current_project().unwrap().ptr;
        let focus = Focus::default();
        answers.push(("focus (fx)".into(), focus.poll(&lab.reads, &reaper, project, true)));
        fake::with(|s| s.focused_fx = None);
        answers.push(("focus (arrange)".into(), focus.poll(&lab.reads, &reaper, project, true)));
        fake::with(|s| s.midi_editor = Some((2, 0, 0)));
        answers.push(("focus (midi)".into(), focus.poll(&lab.reads, &reaper, project, true)));
        for (kind, reference) in [
            ("arrange", None),
            ("midi", Some("r:take:{TAKE-S1}")),
            ("fx", Some("r:track:{AAAA}")),
            ("fx", Some("r:take:{TAKE-S1}")),
            ("routing", Some("r:track:{DDDD}")),
            ("routing", Some("r:track:{AAAA}")),
        ] {
            let mut params = json!({"kind": kind});
            if let Some(reference) = reference {
                params["ref"] = reference.into();
            }
            answers.push(expect(format!("focus_body {kind} {reference:?}"), lab.reads.focus_body(&reaper, &params)));
        }
        assert!(answers.len() > 50, "only {} answers walked", answers.len());
        for (label, answer) in &answers {
            let misplaced = misplaced_refs(answer);
            assert!(misplaced.is_empty(), "{label}: {misplaced:?}");
        }
        // The walk saw refs of every kind the contract names.
        let kinds: std::collections::BTreeSet<&str> = refs_seen.iter().filter_map(|r| r.split(':').nth(1)).collect();
        for kind in ["track", "item", "take", "fx", "param", "send", "marker", "region", "env"] {
            assert!(kinds.contains(kind), "no {kind} ref walked: {kinds:?}");
        }
    }

    /// REAPER stores a colour even when it isn't used: only the 0x1000000 flag says it is. Without it a track is the default
    /// colour (null), never black.
    #[test]
    fn a_colour_counts_only_with_reapers_set_flag_so_default_tracks_are_not_black() {
        let lab = Lab::new();
        fake::with(|s| {
            s.tracks[0].color = 0; // no custom colour
            s.tracks[1].color = fake::native_color(0, 0, 0); // a custom colour that is black
            s.tracks[2].color = 0x0000FF; // a colour kept but not used (no flag)
            s.tracks[3].color = fake::native_color(255, 0, 0);
            s.tracks[1].items[0].color = 0;
            s.tracks[1].items[1].color = fake::native_color(0, 255, 0);
            s.tracks[2].items[0].color = 0x00FF00; // kept, not used
            s.markers[0].color = 0x0000FF; // kept, not used
            s.markers[1].color = fake::native_color(0, 0, 0);
        });
        let colours = |rows: Vec<Value>| rows.iter().map(|r| r["color"].clone()).collect::<Vec<_>>();
        let tracks = lab.rows(json!({"kind": "track"}));
        assert_eq!(colours(tracks[1..].to_vec()), [Value::Null, json!("#000000"), Value::Null, json!("#ff0000")]);
        assert_eq!(colours(lab.rows(json!({"kind": "item", "parent": "r:track:{BBBB}"}))), [Value::Null, json!("#00ff00")]);
        assert_eq!(colours(lab.rows(json!({"kind": "item", "parent": "r:track:{CCCC}"}))), [Value::Null]);
        assert_eq!(colours(lab.rows(json!({"kind": "marker"}))), [Value::Null]);
        assert_eq!(colours(lab.rows(json!({"kind": "region"}))), [json!("#000000")]);
        // The same in what FOCUS draws: the selection's header and the arrange window.
        fake::select_tracks(&[0]);
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let snapshot = lab.reads.snapshot(&reaper).unwrap();
        assert_eq!(snapshot["selection"]["track"]["color"], Value::Null);
        fake::select_tracks(&[1]);
        assert_eq!(lab.reads.snapshot(&reaper).unwrap()["selection"]["track"]["color"], "#000000");
        let arrange = lab.reads.focus_body(&reaper, &json!({"kind": "arrange"})).unwrap();
        assert_eq!(colours(arrange["tracks"].as_array().unwrap().clone()), [Value::Null, json!("#000000"), Value::Null, json!("#ff0000")]);
        assert_eq!(colours(arrange["tracks"][1]["items"].as_array().unwrap().clone()), [Value::Null, json!("#00ff00")]);
    }
}
