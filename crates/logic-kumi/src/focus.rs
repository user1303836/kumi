//! `focus`: what the producer looks at, answered from a cache the run loop refreshes every 150 ms with targeted reads
//! (the selected header, the selected region, the control bar's playhead and transport, which editor is open), never
//! from a walk of the whole window. The shape is the host simulator's: `epoch`, `view`, `editor`, `selection`,
//! `playhead`, `transport`, `changed`, `ageMs`, with `busy` added while Logic can't take work, and the selection
//! carrying the selected track's name and kind beside its ref, as the runtime reads them.

use std::time::Instant;

use serde_json::{json, Value};

use crate::ax::paths::{self, TracksArea, Window};
use crate::ax::strings::Strings;
use crate::ax::Node;
use crate::rows::{self, Header, Region};

/// How often the run loop refreshes the cache.
pub const REFRESH_MS: u64 = 150;

/// The transport's state, from the control bar.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transport {
    pub playing: bool,
    pub recording: bool,
    pub cycle: bool,
    pub bar: u32,
    pub beat: u32,
    pub tempo: f64,
    pub time_signature: String,
}

impl Transport {
    /// Read from the control bar's buttons and LCD.
    pub fn of(control_bar: &Node, strings: &Strings) -> Transport {
        let on = |label: &str| paths::transport_button(control_bar, strings, label).is_some_and(Node::is_on);
        let lcd = paths::lcd(control_bar, strings);
        let playhead = lcd.and_then(|lcd| paths::playhead(lcd, strings));
        let slider = |label: &str| {
            playhead
                .and_then(|group| group.children.iter().find(|c| c.role == "AXSlider" && strings.is(&c.desc, label)))
                .and_then(Node::number)
                .map(|n| n.max(0.0) as u32)
        };
        // Without a label per slider, the first two are the bar and the beat.
        let nth = |index: usize| {
            playhead
                .and_then(|group| group.children.iter().filter(|c| c.role == "AXSlider").nth(index))
                .and_then(Node::number)
                .map(|n| n.max(0.0) as u32)
        };
        Transport {
            playing: on("Play"),
            recording: on("Record"),
            cycle: on("Cycle"),
            bar: slider("bar").or_else(|| nth(0)).unwrap_or(1),
            beat: slider("beat").or_else(|| nth(1)).unwrap_or(1),
            tempo: lcd.and_then(|lcd| paths::tempo(lcd, strings)).and_then(Node::number).unwrap_or(0.0),
            time_signature: lcd.and_then(|lcd| paths::time_signature(lcd, strings)).map(Node::value_text).unwrap_or_default(),
        }
    }

    pub fn beats_per_bar(&self) -> u32 {
        rows::beats_per_bar_of(&self.time_signature)
    }

    /// The playhead in beats from the project's start.
    pub fn beats(&self) -> f64 {
        ((self.bar.max(1) - 1) * self.beats_per_bar() + (self.beat.max(1) - 1)) as f64
    }

    /// As the snapshot carries it.
    pub fn to_json(&self) -> Value {
        json!({
            "playing": self.playing, "recording": self.recording, "cycle": self.cycle,
            "playhead": {"bar": self.bar, "beat": self.beat},
            "tempo": self.tempo, "timeSignature": self.time_signature,
        })
    }
}

/// Which editor is open over the Tracks area, as the focus names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editor {
    PianoRoll,
    EventList,
    StepSequencer,
    Mixer,
    Plugin,
}

impl Editor {
    pub fn name(self) -> &'static str {
        match self {
            Editor::PianoRoll => "pianoRoll",
            Editor::EventList => "eventList",
            Editor::StepSequencer => "stepSequencer",
            Editor::Mixer => "mixer",
            Editor::Plugin => "plugin",
        }
    }
}

/// The selection: the selected track's place and header, the selected region's place through the project and its
/// row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    pub track: Option<(usize, Header)>,
    pub track_kind: Option<String>,
    pub region: Option<(usize, Region)>,
}

impl Selection {
    pub fn to_json(&self, epoch: u32) -> Value {
        let mut selection = json!({
            "trackRef": self.track.as_ref().map(|(n, _)| format!("{epoch}:track:{n}")),
            "regionRef": self.region.as_ref().map(|(n, _)| format!("{epoch}:region:{n}")),
        });
        if let Some((_, header)) = &self.track {
            selection["track"] = json!({"name": header.name, "kind": self.track_kind.clone().unwrap_or_else(|| "audio".into())});
        }
        if let Some((_, region)) = &self.region {
            selection["regionName"] = region.name.clone().into();
        }
        selection
    }
}

/// The cache: what the last refresh saw, when, and how often it changed.
#[derive(Debug, Clone)]
pub struct Focus {
    pub epoch: u32,
    pub view: &'static str,
    pub editor: Option<Editor>,
    pub selection: Selection,
    pub transport: Transport,
    pub changed: u64,
    pub refreshed: Option<Instant>,
    /// Why Logic can't take work, if it can't: the status's `busy` code.
    pub busy: Option<&'static str>,
}

impl Default for Focus {
    fn default() -> Self {
        Focus {
            epoch: 1,
            view: "tracks",
            editor: None,
            selection: Selection::default(),
            transport: Transport::default(),
            changed: 0,
            refreshed: None,
            busy: None,
        }
    }
}

impl Focus {
    /// The focus as the wire answers it.
    pub fn to_json(&self) -> Value {
        let mut focus = json!({
            "epoch": self.epoch.to_string(),
            "view": self.view,
            "editor": self.editor.map(Editor::name),
            "selection": self.selection.to_json(self.epoch),
            "playhead": {"bar": self.transport.bar, "beat": self.transport.beat},
            "transport": {
                "playing": self.transport.playing, "recording": self.transport.recording, "cycle": self.transport.cycle,
                "tempo": self.transport.tempo, "beat": self.transport.beats(), "beatsPerBar": self.transport.beats_per_bar(),
            },
            "changed": self.changed,
            "ageMs": self.refreshed.map_or(0, |at| at.elapsed().as_millis() as u64),
        });
        if let Some(busy) = self.busy {
            focus["busy"] = busy.into();
        }
        focus
    }

    /// What the host compares between two answers: everything but the age.
    fn seen(&self) -> Value {
        let mut seen = self.to_json();
        seen.as_object_mut().map(|o| o.remove("ageMs"));
        seen
    }

    /// Takes a fresh look: whether anything the focus says changed.
    pub fn update(&mut self, look: Look) -> bool {
        let before = self.seen();
        self.epoch = look.epoch;
        self.view = look.view;
        self.editor = look.editor;
        self.selection = look.selection;
        self.transport = look.transport;
        self.busy = look.busy;
        self.refreshed = Some(Instant::now());
        let changed = self.seen() != before;
        if changed {
            self.changed += 1;
        }
        changed
    }
}

/// One refresh's findings.
#[derive(Debug, Clone, Default)]
pub struct Look {
    pub epoch: u32,
    pub view: &'static str,
    pub editor: Option<Editor>,
    pub selection: Selection,
    pub transport: Transport,
    pub busy: Option<&'static str>,
}

/// The selected header among the Tracks area's headers, with its place (from 1).
pub fn selected_header(area: &TracksArea, strings: &Strings) -> Option<(usize, Header)> {
    let headers = area.headers?;
    headers
        .children
        .iter()
        .filter(|c| c.role == "AXLayoutItem")
        .enumerate()
        .find(|(_, item)| item.is_selected())
        .map(|(place, item)| (place + 1, Header::of(item, place, strings)))
}

/// The regions of the Tracks area in the order they are numbered: track by track, each track's in the order Logic
/// lists them, with the track's place (from 1).
pub fn regions(area: &TracksArea, strings: &Strings) -> Vec<(usize, Region)> {
    let Some(contents) = area.contents else { return vec![] };
    let mut out = vec![];
    for (place, lane) in contents.children.iter().filter(|c| c.role == "AXLayoutArea").enumerate() {
        for item in lane.children.iter().filter(|c| c.role == "AXLayoutItem") {
            if let Some(region) = Region::of(item, place + 1, strings) {
                out.push((place + 1, region));
            }
        }
    }
    out
}

/// The selected region, with its place through the project (from 1).
pub fn selected_region(area: &TracksArea, strings: &Strings) -> Option<(usize, Region)> {
    regions(area, strings).into_iter().enumerate().find(|(_, (_, region))| region.selected).map(|(index, (_, region))| (index + 1, region))
}

/// Which editor the window shows: the List Editors' Event tab counts as the Event List, the Mixer pane as the Mixer;
/// the editors pane's own kind comes from its label.
pub fn editor_of(window: &Window, strings: &Strings, editors_pane: Option<&Node>) -> Option<Editor> {
    if let Some(pane) = editors_pane {
        if let Some(editor) = editor_in_pane(pane, strings) {
            return Some(editor);
        }
    }
    if window.list_tabs.is_some_and(|tabs| paths::list_tab_on(tabs, strings) == Some("Event")) && window.list.is_some() {
        return Some(Editor::EventList);
    }
    None
}

/// The editor an editors pane shows, by the label of its group.
pub fn editor_in_pane(pane: &Node, strings: &Strings) -> Option<Editor> {
    let label = &pane.desc;
    if strings.is(label, "Piano Roll") {
        Some(Editor::PianoRoll)
    } else if strings.is(label, "Step Sequencer") {
        Some(Editor::StepSequencer)
    } else if strings.is(label, "Mixer") {
        Some(Editor::Mixer)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(value: Value) -> Node {
        Node::from_json(&value)
    }

    #[test]
    fn the_transport_is_read_from_the_control_bar_and_the_focus_counts_its_changes() {
        let strings = Strings::english();
        let bar = node(json!({"role": "AXGroup", "desc": "Control Bar", "c": [
            {"role": "AXGroup", "desc": "Control Bar", "c": [
                {"role": "AXGroup", "desc": "Playhead Position", "c": [{"role": "AXSlider", "desc": "bar", "value": 9}, {"role": "AXSlider", "desc": "beat", "value": 3}]},
                {"role": "AXSlider", "desc": "Tempo", "value": 160},
                {"role": "AXPopUpButton", "desc": "Time Signature", "value": "4/4"},
                {"role": "AXPopUpButton", "desc": "Key Signature", "value": "C Major"}
            ]},
            {"role": "AXCheckBox", "title": "Cycle", "desc": "Cycle", "value": 1},
            {"role": "AXCheckBox", "title": "Record", "desc": "Record", "value": 0},
            {"role": "AXCheckBox", "title": "Play", "desc": "Play", "value": 1},
            {"role": "AXButton", "title": "Stop", "desc": "Stop", "value": 0}
        ]}));
        let transport = Transport::of(&bar, &strings);
        assert_eq!(
            transport,
            Transport { playing: true, recording: false, cycle: true, bar: 9, beat: 3, tempo: 160.0, time_signature: "4/4".into() }
        );
        assert_eq!(transport.beats(), 34.0);
        assert_eq!(transport.to_json()["playhead"], json!({"bar": 9, "beat": 3}));
        let mut focus = Focus::default();
        let look = Look { epoch: 1, view: "tracks", editor: Some(Editor::PianoRoll), transport: transport.clone(), ..Look::default() };
        assert!(focus.update(look.clone()));
        assert!(!focus.update(look.clone()), "the same look again changes nothing");
        assert_eq!(focus.changed, 1);
        let json = focus.to_json();
        assert_eq!(
            (json["view"].as_str(), json["editor"].as_str(), json["epoch"].as_str()),
            (Some("tracks"), Some("pianoRoll"), Some("1"))
        );
        assert_eq!(
            json["transport"],
            json!({"playing": true, "recording": false, "cycle": true, "tempo": 160.0, "beat": 34.0, "beatsPerBar": 4})
        );
        assert_eq!(json["playhead"], json!({"bar": 9, "beat": 3}));
        assert_eq!(json["selection"], json!({"trackRef": null, "regionRef": null}));
        assert!(json.get("busy").is_none());
        assert!(json["ageMs"].as_u64().unwrap() < 1000);
        let busy = Look { busy: Some("modal"), ..look };
        assert!(focus.update(busy));
        assert_eq!(focus.to_json()["busy"], "modal");
    }

    #[test]
    fn the_selection_names_the_selected_header_and_region_with_their_places() {
        let strings = Strings::english();
        let tracks = node(json!({"role": "AXGroup", "desc": "Tracks", "c": [{"role": "AXSplitGroup", "c": [
            {"role": "AXScrollArea", "c": [{"role": "AXGroup", "c": [
                {"role": "AXLayoutItem", "desc": "Track 1 “Vocal”", "selected": false, "c": [{"role": "AXTextField", "desc": "Vocal"}]},
                {"role": "AXLayoutItem", "desc": "Track 2 “Keys”", "selected": true, "c": [{"role": "AXTextField", "desc": "Keys"}]}
            ]}]},
            {"role": "AXScrollArea", "c": [{"role": "AXGroup", "c": [
                {"role": "AXLayoutArea", "desc": "Track 1 “Vocal”", "c": [{"role": "AXLayoutItem", "desc": "Take", "help": "Region starts at 3 bars  and ends at 4 bars , audio region."}]},
                {"role": "AXLayoutArea", "desc": "Track 2 “Keys”", "c": [{"role": "AXLayoutItem", "desc": "Keys", "selected": true, "help": "Region starts at 1 bar  and ends at 5 bars , MIDI region."}]},
                {"role": "AXLayoutArea"}
            ]}]}
        ]}]}));
        let area = TracksArea::of(&tracks);
        let (place, header) = selected_header(&area, &strings).unwrap();
        assert_eq!((place, header.name.as_str()), (2, "Keys"));
        let listed = regions(&area, &strings);
        assert_eq!(listed.iter().map(|(track, region)| (*track, region.name.as_str())).collect::<Vec<_>>(), [(1, "Take"), (2, "Keys")]);
        let (place, region) = selected_region(&area, &strings).unwrap();
        assert_eq!((place, region.name.as_str()), (2, "Keys"));
        let selection = Selection { track: Some((2, header)), track_kind: Some("instrument".into()), region: Some((2, region)) };
        assert_eq!(
            selection.to_json(3),
            json!({"trackRef": "3:track:2", "regionRef": "3:region:2", "track": {"name": "Keys", "kind": "instrument"}, "regionName": "Keys"})
        );
        assert_eq!(editor_in_pane(&node(json!({"role": "AXGroup", "desc": "Piano Roll"})), &strings), Some(Editor::PianoRoll));
        assert_eq!(Editor::EventList.name(), "eventList");
    }
}
