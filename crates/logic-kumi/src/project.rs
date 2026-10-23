//! The project as the bridge reads it from Logic's window: the readers that fill the [`Catalog`] and refresh the
//! focus, over the walker, the paths and the rows. Each reader takes a small subtree (the control bar, the track
//! headers, the tracks' contents, the Inspector's strips, the Mixer's strips, a list editor's table), never the whole
//! window. What a reader needs Logic to show, it asks for and puts back: the Mixer pane for every strip, the List
//! Editors for the markers, the tempo, the signatures and a region's notes (which also needs the region selected,
//! since the Event List shows the selected region's events). The epoch moves on every track or region change seen
//! between polls: a header appeared, vanished, was renamed or moved, a region added, removed or moved.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::ax::paths::{self, Inspector, TracksArea, Window};
use crate::ax::strings::Strings;
use crate::ax::{Ax, AxError, Node, Root, BRIEF, CHEAP};
use crate::focus::{self, Editor, Look, Selection, Transport};
use crate::read::Catalog;
use crate::refs::{self, failed, Fault, Ref};
use crate::rows::{self, Header, Marker, Note, Region, SignatureEvent, StripState};

/// How long the strips read from the Mixer stay good for: their kinds, inserts, sends and routing change rarely, and
/// every read of them opens the Mixer pane.
pub const STRIPS_FOR: Duration = Duration::from_secs(5);
/// How long a list editor's rows stay good for.
pub const LISTS_FOR: Duration = Duration::from_secs(2);
/// How long a setting read from Logic's preferences (middle C) stays good for.
pub const PREFERENCES_FOR: Duration = Duration::from_secs(5);
/// The preferences' key of "Display Middle C As" (on: C4, Roland; off or unset: C3, Yamaha).
const MIDDLE_C: &str = "MiddleC";
/// How long Logic gets to show a pane or a tab after the bridge asks.
const SETTLE_MS: u64 = 250;

/// Where the window's parts are, as paths below the main window, found by one shallow walk and kept until a reader
/// finds one missing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    pub control_bar: Option<String>,
    pub inspector: Option<String>,
    pub tracks: Option<String>,
    pub list_tabs: Option<String>,
    pub list: Option<String>,
    /// The Mixer pane's layout area of strips, when the pane is open.
    pub mixer: Option<String>,
    /// The editors pane (Piano Roll, Step Sequencer…), when open: its group and its label.
    pub editor: Option<(String, String)>,
    /// Within the Tracks area: the headers' group, the contents' group, the ruler.
    pub headers: Option<String>,
    pub contents: Option<String>,
}

/// What the Tracks area looked like at the last poll, for the epoch: the headers' names in order, and the regions'
/// track, name and bounds in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shape {
    pub headers: Vec<String>,
    pub regions: Vec<(usize, String, u32, u32, u32, u32)>,
}

/// A List Editors tab shown for a run of reads and writes: whether the pane was opened for it, and the tab that was
/// on before, if another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListOpen {
    pub opened: bool,
    pub was_on: Option<&'static str>,
}

/// Where each track and region of one epoch stands in the next: a restore names a track or region by the place it
/// had when the change was previewed, and the bridge carries it to where it is now. Found from the shapes before and
/// after an epoch move: one header more is an insertion at the first place that differs, one less a deletion there,
/// the same names in another order go by name, another name at the same count is a rename that keeps every place,
/// anything else matches names where they are unique; a region follows its track by its name and bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carry {
    /// The epoch the places are from (they lead to the next).
    pub from: u32,
    pub tracks: Vec<Option<usize>>,
    pub regions: Vec<Option<usize>>,
}

/// The most epoch moves carried: a restore from further back is `gone`.
pub const CARRIES: usize = 64;

impl Carry {
    fn between(from: u32, before: &Shape, after: &Shape) -> Carry {
        let (b, a) = (&before.headers, &after.headers);
        let mut sorted_a = a.clone();
        let mut sorted_b = b.clone();
        sorted_a.sort();
        sorted_b.sort();
        let by_name = |name: &String| {
            let unique = b.iter().filter(|n| *n == name).count() == 1 && a.iter().filter(|n| *n == name).count() == 1;
            unique.then(|| a.iter().position(|n| n == name).map(|i| i + 1)).flatten()
        };
        let tracks: Vec<Option<usize>> = if a.len() == b.len() && sorted_a == sorted_b {
            // The same names in another order: each by its name (the same name twice stays where it is).
            b.iter().enumerate().map(|(i, name)| by_name(name).or(Some(i + 1))).collect()
        } else if a.len() == b.len() {
            // A rename: every place holds.
            (1..=b.len()).map(Some).collect()
        } else if a.len() == b.len() + 1 {
            let at = (0..b.len()).find(|i| a[*i] != b[*i]).unwrap_or(b.len());
            (0..b.len()).map(|i| Some(if i < at { i + 1 } else { i + 2 })).collect()
        } else if a.len() + 1 == b.len() {
            let at = (0..a.len()).find(|i| a[*i] != b[*i]).unwrap_or(a.len());
            (0..b.len())
                .map(|i| {
                    if i < at {
                        Some(i + 1)
                    } else if i == at {
                        None
                    } else {
                        Some(i)
                    }
                })
                .collect()
        } else {
            b.iter().map(by_name).collect()
        };
        let regions = before
            .regions
            .iter()
            .map(|(track, name, start_bar, start_beat, end_bar, end_beat)| {
                let track_now = tracks.get(track.wrapping_sub(1)).copied().flatten()?;
                after
                    .regions
                    .iter()
                    .position(|r| {
                        r.0 == track_now && r.1 == *name && (r.2, r.3, r.4, r.5) == (*start_bar, *start_beat, *end_bar, *end_beat)
                    })
                    .map(|i| i + 1)
            })
            .collect();
        Carry { from, tracks, regions }
    }
}

/// What a request needs read beyond the Tracks area and the control bar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Needs {
    /// The Mixer's strips: the tracks' kinds, plug-ins, sends and routing.
    pub strips: bool,
    pub markers: bool,
    /// The Tempo and Signature lists.
    pub tempo: bool,
    /// A region's notes, by its place through the project.
    pub notes_of: Option<usize>,
    /// The lists (markers, tempo, signatures) read in one showing of the List Editors, each tab in turn, as the bulk
    /// read does; the paged reads show the pane once per list.
    pub together: bool,
}

impl Needs {
    /// What a `read` of a kind needs.
    pub fn for_kind(kind: &str, notes_of: Option<usize>) -> Needs {
        match kind {
            "track" | "plugin" | "send" => Needs { strips: true, ..Needs::default() },
            "marker" => Needs { markers: true, ..Needs::default() },
            "tempo" => Needs { tempo: true, ..Needs::default() },
            "note" => Needs { notes_of, ..Needs::default() },
            _ => Needs::default(),
        }
    }

    /// What `snapshot` needs: the tracks with their kinds, and the markers.
    pub fn snapshot() -> Needs {
        Needs { strips: true, markers: true, ..Needs::default() }
    }

    /// What the bulk read of these kinds needs: the strips for tracks, plug-ins and sends, the lists for markers and
    /// the tempo, all in one go.
    pub fn for_kinds(kinds: &[&str]) -> Needs {
        Needs {
            strips: kinds.iter().any(|kind| matches!(*kind, "track" | "plugin" | "send")),
            markers: kinds.contains(&"marker"),
            tempo: kinds.contains(&"tempo"),
            notes_of: None,
            together: true,
        }
    }

    /// What a body needs.
    pub fn for_body(kind: &str, notes_of: Option<usize>) -> Needs {
        match kind {
            "tree" | "routing" => Needs { strips: true, ..Needs::default() },
            "notes" => Needs { notes_of, ..Needs::default() },
            _ => Needs::default(),
        }
    }
}

/// The bridge's picture of the project.
pub struct Project {
    pub strings: Strings,
    /// Where the project window is addressed: the main window, or the project's window by title when another window
    /// is main.
    pub root: Root,
    pub layout: Layout,
    pub epoch: u32,
    pub shape: Option<Shape>,
    /// Where the tracks and regions of each earlier epoch stand now, one carry per epoch move, oldest first.
    pub carries: Vec<Carry>,
    /// The strips as the Mixer last showed them, by the track's place (from 1), with when.
    strips: Option<(Instant, Vec<StripState>)>,
    /// The Mixer's output strips (the Stereo Out first), read with the tracks' strips.
    outputs: Vec<StripState>,
    /// The list editors' rows read last, by tab, with when.
    lists: HashMap<&'static str, (Instant, Vec<Value>)>,
    /// The notes read last, by region, with when.
    notes: HashMap<usize, (Instant, Vec<Value>)>,
    /// The project's identity as the model knows it (`read project` needs the path and id).
    pub id: Option<String>,
    pub path: Option<String>,
    pub identity: String,
    /// The last transport and selection seen (the focus's), kept for the catalog.
    pub transport: Transport,
    pub selection: Selection,
    pub key_signature: String,
    pub sample_rate: Option<u32>,
    /// How many helper calls the last full-window walk took and how long: the lab's number.
    pub last_walk_ms: Option<u64>,
    /// The octave of middle C as last read from Logic's preferences, with when.
    middle_c: Option<(Instant, u8)>,
    /// Whether the playhead stands exactly on a beat (its division and tick 1), which the LCD can't show: false once
    /// Logic has played or stopped since, or the Mackie Control's timecode shows a division or tick past 1. Placing
    /// the playhead goes to the project's start first when it isn't, since the LCD's bar and beat sliders keep the
    /// division and tick it had. A project read fresh is taken as exact; the bridge starts it unsure.
    pub playhead_exact: bool,
}

impl Project {
    pub fn new(strings: Strings) -> Project {
        Project {
            strings,
            root: Root::Main,
            layout: Layout::default(),
            epoch: 1,
            shape: None,
            carries: Vec::new(),
            strips: None,
            outputs: Vec::new(),
            lists: HashMap::new(),
            notes: HashMap::new(),
            id: None,
            path: None,
            identity: String::new(),
            transport: Transport::default(),
            selection: Selection::default(),
            key_signature: String::new(),
            sample_rate: None,
            last_walk_ms: None,
            middle_c: None,
            playhead_exact: true,
        }
    }

    /// The project's identity, from the model's project: its id, path and name.
    pub fn set_identity(&mut self, id: Option<&str>, path: Option<&str>, name: &str) {
        let changed = self.path.as_deref() != path || self.identity != name;
        self.id = id.map(str::to_string);
        self.path = path.map(str::to_string);
        self.identity = name.to_string();
        if changed {
            self.forget();
            self.sample_rate = path.and_then(sample_rate_of);
        }
    }

    /// Addresses the project window by this root from now on; a different one means the layout is found again.
    pub fn set_root(&mut self, root: Root) {
        if self.root != root {
            self.root = root;
            self.layout = Layout::default();
        }
    }

    /// Everything read of the project is stale: another project, or the epoch moved.
    pub fn forget(&mut self) {
        self.strips = None;
        self.lists.clear();
        self.notes.clear();
    }

    /// Where the window's parts are: one walk of the main window two levels down (some forty nodes).
    pub fn find_layout(&mut self, ax: &mut Ax) -> Result<(), AxError> {
        let window = ax.tree(&self.root, "", 2, 60, Some(BRIEF))?;
        let parts = Window::of(&window, &self.strings);
        let mut layout = Layout {
            control_bar: parts.control_bar.map(|n| n.path.clone()),
            inspector: parts.inspector.map(|n| n.path.clone()),
            tracks: parts.tracks.map(|n| n.path.clone()),
            list_tabs: parts.list_tabs.map(|n| n.path.clone()),
            list: parts.list.map(|n| n.path.clone()),
            ..Layout::default()
        };
        for child in &window.children {
            if child.role == "AXGroup" && self.strings.is(&child.desc, "Mixer") {
                layout.mixer = child
                    .child_where(|c| c.role == "AXLayoutArea")
                    .map(|c| c.path.clone())
                    .or_else(|| Some(crate::ax::join(&child.path, 1)));
            } else if child.role == "AXGroup" && focus::editor_in_pane(child, &self.strings).is_some() {
                layout.editor = Some((child.path.clone(), child.desc.clone()));
            }
        }
        if let Some(tracks) = layout.tracks.clone() {
            let area = ax.tree(&self.root, &tracks, 5, 40, Some(&["role", "subrole", "desc"]))?;
            let parts = TracksArea::of(&area);
            layout.headers = parts.headers.map(|n| n.path.clone());
            layout.contents = parts.contents.map(|n| n.path.clone());
        }
        self.layout = layout;
        Ok(())
    }

    fn need(&self, part: &Option<String>, what: &str) -> Result<String, AxError> {
        part.clone().ok_or_else(|| AxError::Missing(what.into()))
    }

    /// The control bar with its LCD, three levels down.
    pub fn control_bar(&self, ax: &mut Ax) -> Result<Node, AxError> {
        let path = self.need(&self.layout.control_bar, "its control bar")?;
        ax.tree(&self.root, &path, 3, 40, Some(BRIEF))
    }

    /// The track headers with their controls.
    pub fn headers(&self, ax: &mut Ax) -> Result<Node, AxError> {
        let path = self.need(&self.layout.headers, "its track headers")?;
        ax.tree(&self.root, &path, 2, 256, Some(BRIEF))
    }

    /// The tracks' contents: each track's lane with its regions (their help text says where they are).
    pub fn contents(&self, ax: &mut Ax) -> Result<Node, AxError> {
        let path = self.need(&self.layout.contents, "its regions")?;
        ax.tree(&self.root, &path, 3, 256, Some(CHEAP))
    }

    /// The Inspector's strips: the selected track's and its output's.
    pub fn inspector(&self, ax: &mut Ax) -> Result<Node, AxError> {
        let path = self.need(&self.layout.inspector, "its Inspector")?;
        ax.tree(&self.root, &path, 6, 40, Some(CHEAP))
    }

    /// The track headers as rows, in order.
    pub fn read_headers(&self, ax: &mut Ax) -> Result<Vec<Header>, AxError> {
        let headers = self.headers(ax)?;
        Ok(headers
            .children
            .iter()
            .filter(|c| c.role == "AXLayoutItem")
            .enumerate()
            .map(|(place, item)| Header::of(item, place, &self.strings))
            .collect())
    }

    /// The regions, track by track, with their track's place (from 1).
    pub fn read_regions(&self, ax: &mut Ax) -> Result<Vec<(usize, Region)>, AxError> {
        let contents = self.contents(ax)?;
        let area = TracksArea { contents: Some(&contents), ..TracksArea::default() };
        Ok(focus::regions(&area, &self.strings))
    }

    /// The Tracks area's shape, for the epoch.
    fn shape_of(headers: &[Header], regions: &[(usize, Region)]) -> Shape {
        Shape {
            headers: headers.iter().map(|h| h.name.clone()).collect(),
            regions: regions.iter().map(|(track, r)| (*track, r.name.clone(), r.start.bar, r.start.beat, r.end.bar, r.end.beat)).collect(),
        }
    }

    /// Notes a poll's headers and regions: whether the epoch moved (and where the tracks and regions went).
    pub fn observe(&mut self, headers: &[Header], regions: &[(usize, Region)]) -> bool {
        let shape = Self::shape_of(headers, regions);
        let moved = self.shape.as_ref().is_some_and(|before| *before != shape);
        if moved {
            if let Some(before) = &self.shape {
                self.carries.push(Carry::between(self.epoch, before, &shape));
                if self.carries.len() > CARRIES {
                    self.carries.remove(0);
                }
            }
            self.epoch += 1;
            self.forget();
        }
        self.shape = Some(shape);
        moved
    }

    /// Another project, or the same one loaded again: the places of earlier epochs mean nothing now.
    pub fn new_project(&mut self) {
        self.carries.clear();
        self.shape = None;
        self.layout = Layout::default();
    }

    /// Where a track of an earlier epoch stands now (its place from 1): None when the epoch is too far back, or the
    /// track went.
    pub fn carry_track(&self, epoch: u32, place: usize) -> Option<usize> {
        let mut place = place;
        for carry in self.carries.iter().filter(|carry| carry.from >= epoch) {
            place = carry.tracks.get(place.checked_sub(1)?).copied().flatten()?;
        }
        (self.carries.iter().any(|carry| carry.from == epoch) || epoch == self.epoch).then_some(place)
    }

    /// Where a region of an earlier epoch stands now.
    pub fn carry_region(&self, epoch: u32, place: usize) -> Option<usize> {
        let mut place = place;
        for carry in self.carries.iter().filter(|carry| carry.from >= epoch) {
            place = carry.regions.get(place.checked_sub(1)?).copied().flatten()?;
        }
        (self.carries.iter().any(|carry| carry.from == epoch) || epoch == self.epoch).then_some(place)
    }

    /// A view pane's button in the control bar, by its label: its path and whether the pane is open.
    fn view_button(&self, ax: &mut Ax, label: &str) -> Result<(String, bool), AxError> {
        let bar = self.control_bar(ax)?;
        let button = paths::view_button(&bar, &self.strings, label).ok_or_else(|| AxError::Missing(format!("the {label} button")))?;
        Ok((button.path.clone(), button.is_on()))
    }

    /// Opens a pane the reader needs if it is closed; whether it was opened (so the reader closes it after).
    pub(crate) fn open_pane(&mut self, ax: &mut Ax, label: &str) -> Result<bool, AxError> {
        let (path, open) = self.view_button(ax, label)?;
        if open {
            return Ok(false);
        }
        ax.action(&self.root, &path, "AXPress", SETTLE_MS)?;
        self.find_layout(ax)?;
        Ok(true)
    }

    pub(crate) fn close_pane(&mut self, ax: &mut Ax, label: &str) -> Result<(), AxError> {
        let (path, open) = self.view_button(ax, label)?;
        if open {
            ax.action(&self.root, &path, "AXPress", SETTLE_MS)?;
            self.find_layout(ax)?;
        }
        Ok(())
    }

    /// Every channel strip from the Mixer pane, opened for the read when closed and closed again after: the track
    /// strips first, in the Tracks area's order, then the busses, the outputs and the master. The Mixer and the
    /// editors share the bottom of the window, and closing the Mixer closes that area: an editor that was open (the
    /// Piano Roll of the selected region) is opened again after.
    pub fn read_mixer(&mut self, ax: &mut Ax) -> Result<Vec<StripState>, AxError> {
        let editors_were_open = self.view_button(ax, "Editors").map(|(_, open)| open).unwrap_or(false);
        let opened = self.open_pane(ax, "Mixer")?;
        let result = (|| {
            let path = self.need(&self.layout.mixer, "its Mixer")?;
            let area = ax.tree(&self.root, &path, 3, 256, Some(CHEAP))?;
            Ok(area.children.iter().filter(|c| c.role == "AXLayoutItem").map(|strip| StripState::of(strip, &self.strings)).collect())
        })();
        if opened {
            self.close_pane(ax, "Mixer")?;
            if editors_were_open {
                let _ = self.open_pane(ax, "Editors");
            }
        }
        result
    }

    /// The Mixer pane's strips as nodes, with their controls' paths: the pane must be open (`open_pane("Mixer")`),
    /// and the layout found after it opened.
    pub fn mixer_strip_nodes(&mut self, ax: &mut Ax) -> Result<Vec<Node>, AxError> {
        let path = self.need(&self.layout.mixer, "its Mixer")?;
        let area = ax.tree(&self.root, &path, 3, 256, Some(CHEAP))?;
        Ok(area.children.into_iter().filter(|c| c.role == "AXLayoutItem").collect())
    }

    /// The strips of the tracks, by place (from 1), from the Mixer unless read a moment ago.
    pub fn strips(&mut self, ax: &mut Ax, headers: &[Header]) -> Result<Vec<StripState>, AxError> {
        if let Some((at, strips)) = &self.strips {
            if at.elapsed() < STRIPS_FOR {
                return Ok(strips.clone());
            }
        }
        let all = self.read_mixer(ax)?;
        self.outputs = all.iter().filter(|strip| strip.kind == "output").cloned().collect();
        // The Mixer lists the tracks' strips in the Tracks area's order, then what no track has (busses, outputs, the
        // master): the strips are matched to the headers by name, in order, so a renamed strip still finds its track.
        let mut strips = vec![];
        let mut used = vec![false; all.len()];
        for header in headers {
            let found = all.iter().enumerate().find(|(i, strip)| !used[*i] && strip.name == header.name && strip.kind != "output");
            match found {
                Some((i, strip)) => {
                    used[i] = true;
                    strips.push(strip.clone());
                }
                None => strips.push(StripState {
                    name: header.name.clone(),
                    kind: "audio",
                    mute: header.mute,
                    solo: header.solo,
                    armed: header.armed,
                    monitoring: header.monitoring,
                    volume_db: header.volume_db,
                    pan: header.pan,
                    output: String::new(),
                    inputs: vec![],
                    inserts: vec![],
                    sends: vec![],
                    drawn: true,
                }),
            }
        }
        self.strips = Some((Instant::now(), strips.clone()));
        Ok(strips)
    }

    /// The selected track's strip from the Inspector, which shows it without opening anything.
    pub fn inspector_strip(&self, ax: &mut Ax) -> Result<Option<StripState>, AxError> {
        let inspector = self.inspector(ax)?;
        let parts = Inspector::of(&inspector, &self.strings);
        Ok(parts.strips.first().map(|strip| StripState::of(strip, &self.strings)))
    }

    /// Shows a List Editors tab (opening the pane when closed) and reads its table's rows; puts the pane back after.
    fn read_list(&mut self, ax: &mut Ax, tab: &'static str) -> Result<(Vec<(String, usize)>, Node), AxError> {
        let open = self.show_list(ax, tab)?;
        let result = self.list_table(ax, tab);
        self.hide_list(ax, open)?;
        result
    }

    /// Shows a List Editors tab, opening the pane when it is closed, and keeps it shown for a run of reads and
    /// writes; [`Project::hide_list`] puts the tab and the pane back.
    pub fn show_list(&mut self, ax: &mut Ax, tab: &'static str) -> Result<ListOpen, AxError> {
        let opened = self.open_pane(ax, "List Editors")?;
        let tabs_path = self.need(&self.layout.list_tabs, "its List Editors")?;
        let tabs = ax.tree(&self.root, &tabs_path, 1, 8, Some(BRIEF))?;
        let was_on = paths::list_tab_on(&tabs, &self.strings);
        if was_on != Some(tab) {
            let button = tabs
                .children
                .iter()
                .find(|c| c.role == "AXRadioButton" && self.strings.is(&c.desc, tab))
                .ok_or_else(|| AxError::Missing(format!("the {tab} tab")))?;
            ax.action(&self.root, &button.path, "AXPress", SETTLE_MS)?;
        }
        Ok(ListOpen { opened, was_on: was_on.filter(|was| *was != tab) })
    }

    /// The shown list's table, with its columns: read again after each write to it.
    pub fn list_table(&mut self, ax: &mut Ax, tab: &str) -> Result<(Vec<(String, usize)>, Node), AxError> {
        let list_path = self.need(&self.layout.list, "its list")?;
        let list = ax.tree(&self.root, &list_path, 7, 256, Some(CHEAP))?;
        let table = paths::list_table(&list).ok_or_else(|| AxError::Missing(format!("the {tab} list")))?.clone();
        let columns = paths::columns(&table);
        Ok((columns, table))
    }

    /// The list's group itself (its buttons: Create new Marker, Create new Event, the item count).
    pub fn list_group(&mut self, ax: &mut Ax) -> Result<Node, AxError> {
        let list_path = self.need(&self.layout.list, "its list")?;
        ax.tree(&self.root, &list_path, 2, 64, Some(BRIEF))
    }

    /// The list's header a level deeper: the segments of its Edit, Functions and View menu buttons.
    pub fn list_header(&mut self, ax: &mut Ax) -> Result<Node, AxError> {
        let list_path = self.need(&self.layout.list, "its list")?;
        ax.tree(&self.root, &list_path, 3, 64, Some(BRIEF))
    }

    /// Puts the List Editors back as they were before [`Project::show_list`].
    pub fn hide_list(&mut self, ax: &mut Ax, open: ListOpen) -> Result<(), AxError> {
        if let Some(was_on) = open.was_on {
            let tabs_path = self.need(&self.layout.list_tabs, "its List Editors")?;
            let tabs = ax.tree(&self.root, &tabs_path, 1, 8, Some(BRIEF))?;
            if let Some(button) = tabs.children.iter().find(|c| c.role == "AXRadioButton" && self.strings.is(&c.desc, was_on)) {
                let _ = ax.action(&self.root, &button.path, "AXPress", 50);
            }
        }
        if open.opened {
            self.close_pane(ax, "List Editors")?;
        }
        Ok(())
    }

    /// The markers from the Marker List.
    pub fn read_markers(&mut self, ax: &mut Ax, beats_per_bar: u32) -> Result<Vec<Marker>, AxError> {
        let (columns, table) = self.read_list(ax, "Marker")?;
        Ok(paths::rows(&table).iter().filter_map(|row| Marker::of(row, &columns, beats_per_bar)).collect())
    }

    /// The tempo events from the Tempo List: the first is the project's tempo.
    pub fn read_tempo(&mut self, ax: &mut Ax) -> Result<Vec<(rows::Position, f64)>, AxError> {
        let (columns, table) = self.read_list(ax, "Tempo")?;
        Ok(paths::rows(&table).iter().filter_map(|row| rows::tempo_of(row, &columns)).collect())
    }

    pub fn read_signatures(&mut self, ax: &mut Ax) -> Result<Vec<SignatureEvent>, AxError> {
        let (columns, table) = self.read_list(ax, "Signature")?;
        Ok(paths::rows(&table).iter().filter_map(|row| rows::signature_of(row, &columns, &self.strings)).collect())
    }

    /// The notes of a region: the Event List shows the selected region's events, so the region is selected first
    /// (through its item in the Tracks area) when it isn't.
    pub fn read_notes(&mut self, ax: &mut Ax, region_place: usize, beats_per_bar: u32) -> Result<Vec<Note>, AxError> {
        let contents = self.contents(ax)?;
        let area = TracksArea { contents: Some(&contents), ..TracksArea::default() };
        let regions = focus::regions(&area, &self.strings);
        let (_, region) = regions.get(region_place - 1).ok_or_else(|| AxError::Missing("that region".into()))?;
        if !region.selected {
            let items: Vec<&Node> = contents
                .children
                .iter()
                .filter(|c| c.role == "AXLayoutArea")
                .flat_map(|lane| lane.children.iter().filter(|c| c.role == "AXLayoutItem"))
                .collect();
            let item = items.get(region_place - 1).ok_or_else(|| AxError::Missing("that region".into()))?;
            ax.set(&self.root, &item.path, "AXSelected", json!(true), SETTLE_MS)?;
        }
        let (columns, table) = self.read_list(ax, "Event")?;
        Ok(paths::rows(&table).iter().filter_map(|row| Note::of(row, &columns, beats_per_bar, &self.strings)).collect())
    }

    /// Selects a track (by its place, from 1) as a producer would: a click on its header's name field, which selects
    /// that track alone (the headers' selected state isn't settable, and a selected header keeps a multiple
    /// selection on a click elsewhere in it). Through the helper's `click`, at the system's event tap.
    pub fn select_track(&mut self, ax: &mut Ax, place: usize) -> Result<(), AxError> {
        if self.layout.headers.is_none() {
            self.find_layout(ax)?;
        }
        let headers = self.headers(ax)?;
        let item = headers
            .children
            .iter()
            .filter(|c| c.role == "AXLayoutItem")
            .nth(place.checked_sub(1).ok_or_else(|| AxError::Missing("that track".into()))?)
            .ok_or_else(|| AxError::Missing("that track".into()))?;
        let attrs = ax.get(&self.root, &item.path, &["AXChildren"])?;
        let _ = attrs;
        let name = item.child_where(|c| c.role == "AXTextField").ok_or_else(|| AxError::Missing("the track's name field".into()))?;
        let frame = ax.get(&self.root, &name.path, &["AXPosition", "AXSize"])?;
        let (x, y) = (frame["AXPosition"]["x"].as_f64().unwrap_or(0.0), frame["AXPosition"]["y"].as_f64().unwrap_or(0.0));
        let (w, h) = (frame["AXSize"]["w"].as_f64().unwrap_or(0.0), frame["AXSize"]["h"].as_f64().unwrap_or(0.0));
        ax.helper.call("click", json!({"pid": ax.pid, "x": x + (w / 2.0).min(60.0), "y": y + h / 2.0}))?;
        Ok(())
    }

    /// Adds the track at `place` to the selection: a Shift-click on its header's name field.
    pub fn extend_track_selection(&mut self, ax: &mut Ax, place: usize) -> Result<(), AxError> {
        if self.layout.headers.is_none() {
            self.find_layout(ax)?;
        }
        let headers = self.headers(ax)?;
        let item = headers
            .children
            .iter()
            .filter(|c| c.role == "AXLayoutItem")
            .nth(place.checked_sub(1).ok_or_else(|| AxError::Missing("that track".into()))?)
            .ok_or_else(|| AxError::Missing("that track".into()))?;
        let name = item.child_where(|c| c.role == "AXTextField").ok_or_else(|| AxError::Missing("the track's name field".into()))?;
        let (x, y, w, h) = ax.frame(&self.root, &name.path)?;
        ax.click_with(x + (w / 2.0).min(60.0), y + h / 2.0, &["shift"])
    }

    /// Selects a region (by its place through the project, from 1) through its item's `AXSelected`, which Logic
    /// takes as a toggle whatever value is set: so only when the region isn't selected. The track selection stays as
    /// it is.
    pub fn select_region(&mut self, ax: &mut Ax, place: usize) -> Result<(), AxError> {
        if self.layout.contents.is_none() {
            self.find_layout(ax)?;
        }
        let contents = self.contents(ax)?;
        let items: Vec<&Node> = contents
            .children
            .iter()
            .filter(|c| c.role == "AXLayoutArea")
            .flat_map(|lane| lane.children.iter().filter(|c| c.role == "AXLayoutItem"))
            .collect();
        let item = items
            .get(place.checked_sub(1).ok_or_else(|| AxError::Missing("that region".into()))?)
            .ok_or_else(|| AxError::Missing("that region".into()))?;
        if !item.is_selected() {
            ax.set(&self.root, &item.path, "AXSelected", json!(true), SETTLE_MS)?;
        }
        Ok(())
    }

    /// One focus refresh: the transport from the control bar, the selected header and region, which panes are open.
    pub fn look(&mut self, ax: &mut Ax, busy: Option<&'static str>) -> Result<(Look, Vec<Header>, Vec<(usize, Region)>), AxError> {
        if self.layout.control_bar.is_none() {
            self.find_layout(ax)?;
        }
        let bar = self.control_bar(ax)?;
        let transport = Transport::of(&bar, &self.strings);
        if let Some(lcd) = paths::lcd(&bar, &self.strings) {
            if let Some(key) = paths::key_signature(lcd, &self.strings) {
                self.key_signature = key.value_text();
            }
        }
        let headers = self.read_headers(ax)?;
        let regions = self.read_regions(ax)?;
        let moved = self.observe(&headers, &regions);
        let _ = moved;
        let selected_track = headers.iter().enumerate().find(|(_, h)| h.selected).map(|(i, h)| (i + 1, h.clone()));
        let selected_region = regions.iter().enumerate().find(|(_, (_, r))| r.selected).map(|(i, (_, r))| (i + 1, r.clone()));
        let track_kind = match (&selected_track, &self.strips) {
            (Some((place, _)), Some((_, strips))) => strips.get(place - 1).map(|s| s.kind.to_string()),
            _ => None,
        };
        let mixer_open = paths::view_button(&bar, &self.strings, "Mixer").is_some_and(Node::is_on);
        let lists_open = paths::view_button(&bar, &self.strings, "List Editors").is_some_and(Node::is_on);
        let editors_open = paths::view_button(&bar, &self.strings, "Editors").is_some_and(Node::is_on);
        let editor = if editors_open && self.layout.editor.is_some() {
            self.layout
                .editor
                .as_ref()
                .and_then(|(_, label)| focus::editor_in_pane(&Node { desc: label.clone(), ..Node::default() }, &self.strings))
        } else if mixer_open {
            Some(Editor::Mixer)
        } else if lists_open && self.layout.list_tabs.is_some() {
            let tabs_path = self.layout.list_tabs.clone().unwrap_or_default();
            let tabs = ax.tree(&self.root, &tabs_path, 1, 8, Some(BRIEF))?;
            (paths::list_tab_on(&tabs, &self.strings) == Some("Event")).then_some(Editor::EventList)
        } else {
            None
        };
        let selection = Selection { track: selected_track, track_kind, region: selected_region };
        self.transport = transport.clone();
        self.selection = selection.clone();
        Ok((Look { epoch: self.epoch, view: "tracks", editor, selection, transport, busy }, headers, regions))
    }

    /// Fills a catalog with what a request needs: always the project, the tracks and the regions (one refresh of the
    /// focus gives them), and on request the strips (the Mixer, for the tracks' kinds, plug-ins and sends), the
    /// markers (the Marker List), the tempo and signatures (their lists) and a region's notes (the Event List). What a
    /// request doesn't need isn't read, so nothing opens a pane for nothing.
    pub fn catalog(&mut self, ax: &mut Ax, needs: &Needs) -> Result<Catalog, AxError> {
        let (look, headers, regions) = self.look(ax, None)?;
        let epoch = self.epoch;
        let beats_per_bar = look.transport.beats_per_bar();
        let strips = if needs.strips {
            self.strips(ax, &headers)?
        } else {
            self.strips.as_ref().map(|(_, strips)| strips.clone()).unwrap_or_default()
        };
        let r = |kind: &str, n: String| format!("{epoch}:{kind}:{n}");
        let tracks: Vec<Value> = headers
            .iter()
            .enumerate()
            .map(|(i, header)| rows::track_row(&r("track", (i + 1).to_string()), header, strips.get(i)))
            .collect();
        let region_rows: Vec<Value> = regions
            .iter()
            .enumerate()
            .map(|(i, (track, region))| {
                rows::region_row(&r("region", (i + 1).to_string()), &r("track", track.to_string()), region, beats_per_bar)
            })
            .collect();
        let mut plugins = vec![];
        let mut sends = vec![];
        for (i, strip) in strips.iter().enumerate() {
            let track_ref = r("track", (i + 1).to_string());
            for (slot, insert) in strip.inserts.iter().enumerate() {
                plugins.push(rows::plugin_row(&r("plugin", format!("{}.{}", i + 1, slot + 1)), &track_ref, slot + 1, insert));
            }
            for (slot, send) in strip.sends.iter().enumerate() {
                sends.push(rows::send_row(&r("send", format!("{}.{}", i + 1, slot + 1)), &track_ref, slot + 1, send));
            }
        }
        // The outputs' plug-ins (the Stereo Out's limiter or gain): under `<epoch>:output:<n>`, as `0.<n>.<slot>`.
        if needs.strips {
            for (i, strip) in self.outputs.iter().enumerate() {
                let output_ref = r("output", (i + 1).to_string());
                for (slot, insert) in strip.inserts.iter().enumerate() {
                    plugins.push(rows::plugin_row(&r("plugin", format!("0.{}.{}", i + 1, slot + 1)), &output_ref, slot + 1, insert));
                }
            }
        }
        let together =
            if needs.together && (needs.markers || needs.tempo) { Some(self.lists_together(ax, needs, beats_per_bar)?) } else { None };
        let markers = if let Some((markers, _, _)) = &together {
            markers.clone()
        } else if needs.markers {
            self.cached_list(ax, "Marker", beats_per_bar)?
        } else {
            self.lists.get("Marker").map(|(_, rows)| rows.clone()).unwrap_or_default()
        };
        let (bpm, tempo_events, signatures) = if let Some((_, tempo_events, signature_events)) = together.filter(|_| needs.tempo) {
            let bpm = tempo_events.first().map_or(look.transport.tempo, |(_, bpm)| *bpm);
            (bpm, tempo_events, rows::signatures(&signature_events, &look.transport.time_signature, &self.key_signature))
        } else if needs.tempo {
            let tempo_events = self.read_tempo(ax)?;
            let signature_events = self.read_signatures(ax)?;
            let bpm = tempo_events.first().map_or(look.transport.tempo, |(_, bpm)| *bpm);
            (bpm, tempo_events, rows::signatures(&signature_events, &look.transport.time_signature, &self.key_signature))
        } else {
            (look.transport.tempo, vec![], rows::signatures(&[], &look.transport.time_signature, &self.key_signature))
        };
        let tempo = rows::tempo_row(bpm, &tempo_events, &signatures);
        let mut notes = vec![];
        if let Some(place) = needs.notes_of {
            notes.push((place, self.notes_rows(ax, place, beats_per_bar)?));
        }
        let middle_c = self.middle_c();
        let project = rows::project_row(
            self.id.as_deref(),
            &self.identity,
            self.path.as_deref(),
            bpm,
            &look.transport.time_signature,
            &self.key_signature,
            self.sample_rate,
            headers.len(),
            middle_c,
        );
        let marker_rows: Vec<Value> = markers
            .iter()
            .enumerate()
            .map(|(i, marker)| rows::marker_row(&r("marker", (i + 1).to_string()), &Marker::from_row(marker)))
            .collect();
        Ok(Catalog {
            epoch,
            project: Some(project),
            tracks,
            regions: region_rows,
            notes,
            plugins,
            sends,
            markers: marker_rows,
            tempo: Some(tempo),
            selection: look.selection.to_json(epoch),
            transport: look.transport.to_json(),
        })
    }

    /// The octave Logic names middle C in (3 or 4), from its preferences unless read a moment ago: the producer can
    /// change it in Logic's Settings while the project is open.
    pub fn middle_c(&mut self) -> u8 {
        if let Some((at, octave)) = self.middle_c {
            if at.elapsed() < PREFERENCES_FOR {
                return octave;
            }
        }
        let octave = rows::middle_c_of(crate::disk::preference(MIDDLE_C).as_deref());
        self.middle_c = Some((Instant::now(), octave));
        octave
    }

    /// The lists a bulk read needs, in one showing of the List Editors: the Marker List (unless read a moment ago), the
    /// Tempo List and the Signature List, each tab shown in turn, then the tab the pane had and the pane put back.
    #[allow(clippy::type_complexity)]
    fn lists_together(
        &mut self,
        ax: &mut Ax,
        needs: &Needs,
        beats_per_bar: u32,
    ) -> Result<(Vec<Value>, Vec<(rows::Position, f64)>, Vec<SignatureEvent>), AxError> {
        let cached = self.lists.get("Marker").filter(|(at, _)| at.elapsed() < LISTS_FOR).map(|(_, rows)| rows.clone());
        let mut tabs: Vec<&'static str> = vec![];
        if needs.markers && cached.is_none() {
            tabs.push("Marker");
        }
        if needs.tempo {
            tabs.extend(["Tempo", "Signature"]);
        }
        let mut tables = vec![];
        let mut first: Option<ListOpen> = None;
        let mut failure = None;
        for tab in &tabs {
            match self.show_list(ax, tab).and_then(|open| Ok((open, self.list_table(ax, tab)?))) {
                Ok((open, table)) => {
                    first.get_or_insert(open);
                    tables.push((*tab, table));
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        if let (Some(first), Some(last)) = (first, tabs.last()) {
            // The tab the pane showed before the first was shown, pressed again unless it is the one showing now.
            let original = first.was_on.unwrap_or(tabs[0]);
            self.hide_list(ax, ListOpen { opened: first.opened, was_on: (original != *last).then_some(original) })?;
        }
        if let Some(error) = failure {
            return Err(error);
        }
        let table = |name: &str| tables.iter().find(|(tab, _)| *tab == name).map(|(_, table)| table);
        let markers = match (cached, table("Marker")) {
            (Some(rows), _) => rows,
            (None, Some((columns, table))) => {
                let rows: Vec<Value> =
                    paths::rows(table).iter().filter_map(|row| Marker::of(row, columns, beats_per_bar)).map(|m| m.to_row()).collect();
                self.lists.insert("Marker", (Instant::now(), rows.clone()));
                rows
            }
            (None, None) => vec![],
        };
        let tempo =
            table("Tempo").map(|(columns, table)| paths::rows(table).iter().filter_map(|row| rows::tempo_of(row, columns)).collect());
        let signatures = table("Signature")
            .map(|(columns, table)| paths::rows(table).iter().filter_map(|row| rows::signature_of(row, columns, &self.strings)).collect());
        Ok((markers, tempo.unwrap_or_default(), signatures.unwrap_or_default()))
    }

    /// The marker rows, read unless read a moment ago.
    fn cached_list(&mut self, ax: &mut Ax, tab: &'static str, beats_per_bar: u32) -> Result<Vec<Value>, AxError> {
        if let Some((at, rows)) = self.lists.get(tab) {
            if at.elapsed() < LISTS_FOR {
                return Ok(rows.clone());
            }
        }
        let rows: Vec<Value> = match tab {
            "Marker" => self.read_markers(ax, beats_per_bar)?.iter().map(Marker::to_row).collect(),
            _ => vec![],
        };
        self.lists.insert(tab, (Instant::now(), rows.clone()));
        Ok(rows)
    }

    /// A region's note rows, read unless read a moment ago.
    pub fn notes_rows(&mut self, ax: &mut Ax, region_place: usize, beats_per_bar: u32) -> Result<Vec<Value>, AxError> {
        if let Some((at, rows)) = self.notes.get(&region_place) {
            if at.elapsed() < LISTS_FOR {
                return Ok(rows.clone());
            }
        }
        let epoch = self.epoch;
        let region_ref = format!("{epoch}:region:{region_place}");
        let notes = self.read_notes(ax, region_place, beats_per_bar)?;
        let rows: Vec<Value> = notes
            .iter()
            .enumerate()
            .map(|(i, note)| rows::note_row(&format!("{epoch}:note:{region_place}.{}", i + 1), &region_ref, note))
            .collect();
        self.notes.insert(region_place, (Instant::now(), rows.clone()));
        Ok(rows)
    }

    /// The region a `note` read wants, checked: its place through the project.
    pub fn region_place(&self, parent: &Option<Ref>, raw: &str) -> Result<usize, Fault> {
        match parent {
            Some(reference) if reference.kind == "region" && reference.numbers.len() == 1 && reference.first() >= 1 => {
                Ok(reference.first())
            }
            _ => Err(refs::not_in_project(raw)),
        }
    }

    /// A read's own failure, in the wire's words.
    pub fn fault(error: &AxError) -> Fault {
        if crate::ax::untrusted(error) {
            return failed("Kumi's helper isn't allowed under Accessibility, so nothing of Logic Pro can be read");
        }
        failed(format!("couldn't read Logic Pro: {error}"))
    }
}

impl Marker {
    /// The marker as a row keeps it, and back: the lists cache rows.
    pub fn to_row(&self) -> Value {
        json!({"position": self.position.to_json(), "name": self.name, "length": self.length_beats, "text": self.text})
    }

    pub fn from_row(row: &Value) -> Marker {
        let p = &row["position"];
        let at = |key: &str| p.get(key).and_then(Value::as_u64).unwrap_or(1) as u32;
        Marker {
            position: rows::Position { bar: at("bar"), beat: at("beat"), division: at("division"), tick: at("tick") },
            name: row["name"].as_str().unwrap_or_default().to_string(),
            length_beats: row["length"].as_f64().unwrap_or(0.0),
            text: row["text"].as_str().unwrap_or_default().to_string(),
        }
    }
}

/// The project's sample rate from its package's `MetaData.plist` (`SampleRate`), when the package can be read: macOS
/// keeps `~/Music/Logic` from processes without the Files and Folders permission, and then it is unknown.
pub fn sample_rate_of(path: &str) -> Option<u32> {
    let package = Path::new(path);
    let alternatives = package.join("Alternatives");
    let mut candidates = vec![package.join("Alternatives/000/MetaData.plist")];
    if let Ok(entries) = std::fs::read_dir(&alternatives) {
        for entry in entries.flatten() {
            candidates.push(entry.path().join("MetaData.plist"));
        }
    }
    for candidate in candidates {
        let Ok(output) =
            std::process::Command::new(crate::tools::PLUTIL).args(["-extract", "SampleRate", "raw", "-o", "-"]).arg(&candidate).output()
        else {
            continue;
        };
        if output.status.success() {
            if let Ok(rate) = String::from_utf8_lossy(&output.stdout).trim().parse::<f64>() {
                return Some(rate as u32);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_moves_when_a_header_or_region_appears_vanishes_moves_or_is_renamed() {
        let mut project = Project::new(Strings::english());
        let header = |name: &str| Header {
            index: 1,
            name: name.into(),
            mute: false,
            solo: false,
            armed: false,
            monitoring: false,
            volume_db: None,
            pan: 0,
            selected: false,
            stack: false,
        };
        let region = |track: usize, name: &str, bar: u32| {
            (
                track,
                Region {
                    track_index: track,
                    name: name.into(),
                    start: rows::Position { bar, ..rows::Position::START },
                    end: rows::Position { bar: bar + 4, ..rows::Position::START },
                    midi: true,
                    selected: false,
                    looped: false,
                },
            )
        };
        assert!(!project.observe(&[header("Vocal")], &[]), "the first look sets the shape");
        assert_eq!(project.epoch, 1);
        assert!(!project.observe(&[header("Vocal")], &[]));
        assert!(project.observe(&[header("Vocal"), header("Keys")], &[]), "a header appeared");
        assert_eq!(project.epoch, 2);
        assert!(project.observe(&[header("Vocal"), header("Keys")], &[region(2, "Keys", 1)]), "a region appeared");
        assert!(!project.observe(&[header("Vocal"), header("Keys")], &[region(2, "Keys", 1)]));
        assert!(project.observe(&[header("Vocal"), header("Keys")], &[region(2, "Keys", 3)]), "a region moved");
        assert!(project.observe(&[header("Keys"), header("Vocal")], &[region(2, "Keys", 3)]), "the headers moved");
        assert!(project.observe(&[header("Keys"), header("Vox")], &[region(2, "Keys", 3)]), "a header was renamed");
        assert_eq!(project.epoch, 6);
        // A mute or a volume change moves nothing.
        let mut muted = header("Keys");
        muted.mute = true;
        assert!(!project.observe(&[muted, header("Vox")], &[region(2, "Keys", 3)]));
        // The places of earlier epochs are carried: epoch 2's "Keys" (place 2) is place 1 since the headers moved.
        assert_eq!(project.carry_track(2, 2), Some(1));
        assert_eq!(project.carry_track(2, 1), Some(2));
        assert_eq!(project.carry_track(6, 2), Some(2));
        assert_eq!(project.carry_track(3, 1), Some(2), "a region appearing moved nothing");
        assert_eq!(project.carry_region(3, 1), None, "a region that moved itself isn't followed");
        assert_eq!(project.carry_region(5, 1), Some(1), "a rename keeps the region");
        assert_eq!(project.carry_track(0, 1), None, "before the first shape");
        // A track inserted shifts the ones after it; one deleted takes its place with it.
        assert!(project.observe(&[header("Keys"), header("Bass"), header("Vox")], &[region(3, "Keys", 3)]));
        assert_eq!((project.carry_track(6, 1), project.carry_track(6, 2)), (Some(1), Some(3)));
        assert_eq!(project.carry_region(6, 1), Some(1), "the region followed its track");
        assert!(project.observe(&[header("Keys"), header("Vox")], &[region(2, "Keys", 3)]));
        assert_eq!((project.carry_track(7, 2), project.carry_track(7, 3), project.carry_track(6, 2)), (None, Some(2), Some(2)));
        project.new_project();
        assert_eq!(project.carry_track(7, 1), None);
        assert_eq!(project.region_place(&Ref::parse("6:region:1"), "6:region:1"), Ok(1));
        assert!(project.region_place(&Ref::parse("6:track:1"), "6:track:1").is_err());
        assert_eq!(
            Project::fault(&AxError::Missing("its control bar".into())).1,
            "couldn't read Logic Pro: Logic Pro's window doesn't show its control bar"
        );
        let marker = Marker {
            position: rows::Position { bar: 9, ..rows::Position::START },
            name: "Chorus".into(),
            length_beats: 32.0,
            text: String::new(),
        };
        assert_eq!(Marker::from_row(&marker.to_row()), marker);
        assert_eq!(sample_rate_of("/nowhere/Nothing.logicx"), None);
    }
}
