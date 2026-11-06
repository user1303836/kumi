//! Where Logic keeps what the bridge reads, found by role and label rather than by fixed index wherever Logic allows,
//! so a moved element is a re-match, not a rewrite. The labels are Logic's English ones, matched through
//! [`Strings`] in the producer's language; what Logic doesn't label (the track headers' and contents' groups) is
//! found by its shape. Measured on Logic Pro 12.3.1: the main window's children are the control bar (`AXGroup`
//! "Control Bar", with the LCD as a group of the same name inside it), the Inspector (`AXGroup` "Inspector"), the
//! Tracks area (`AXGroup` "Tracks": a toolbar group and the area, a split group of the ruler row and the headers
//! beside the contents) and, when the List Editors are open, an `AXRadioGroup` of their tabs beside the list's group.

use super::strings::Strings;
use super::Node;

/// The main window's parts, as far as the window's tree was read.
#[derive(Debug, Default)]
pub struct Window<'a> {
    pub control_bar: Option<&'a Node>,
    pub inspector: Option<&'a Node>,
    pub tracks: Option<&'a Node>,
    /// The List Editors' tabs (Event, Marker, Tempo, Signature) and the list beside them, when the pane is open.
    pub list_tabs: Option<&'a Node>,
    pub list: Option<&'a Node>,
}

impl<'a> Window<'a> {
    /// The window's top-level areas by role and label.
    pub fn of(window: &'a Node, strings: &Strings) -> Window<'a> {
        let mut found = Window::default();
        for (index, child) in window.children.iter().enumerate() {
            match child.role.as_str() {
                "AXGroup" if strings.is(&child.desc, "Control Bar") && found.control_bar.is_none() => found.control_bar = Some(child),
                "AXGroup" if strings.is(&child.desc, "Inspector") && found.inspector.is_none() => found.inspector = Some(child),
                "AXGroup" if strings.is(&child.desc, "Tracks") && found.tracks.is_none() => found.tracks = Some(child),
                "AXRadioGroup" if is_list_tabs(child, strings) => {
                    found.list_tabs = Some(child);
                    // The list is the group right after its tabs.
                    found.list = window.children.get(index + 1).filter(|next| next.role == "AXGroup");
                }
                _ => {}
            }
        }
        found
    }
}

/// The List Editors' tab group: radio buttons named Event, Marker, Tempo and Signature.
fn is_list_tabs(node: &Node, strings: &Strings) -> bool {
    let tabs: Vec<&Node> = node.children.iter().filter(|c| c.role == "AXRadioButton").collect();
    tabs.len() >= 3 && tabs.iter().any(|tab| strings.is(&tab.desc, "Marker")) && tabs.iter().any(|tab| strings.is(&tab.desc, "Event"))
}

/// The control bar's LCD: the group inside the control bar that holds the tempo slider.
pub fn lcd<'a>(control_bar: &'a Node, strings: &Strings) -> Option<&'a Node> {
    control_bar.children.iter().find(|group| group.role == "AXGroup" && tempo(group, strings).is_some())
}

/// The LCD's tempo slider.
pub fn tempo<'a>(lcd: &'a Node, strings: &Strings) -> Option<&'a Node> {
    lcd.children.iter().find(|n| n.role == "AXSlider" && strings.is(&n.desc, "Tempo"))
}

/// The LCD's time signature and key signature pop-ups.
pub fn time_signature<'a>(lcd: &'a Node, strings: &Strings) -> Option<&'a Node> {
    lcd.children.iter().find(|n| n.role == "AXPopUpButton" && strings.is(&n.desc, "Time Signature"))
}

pub fn key_signature<'a>(lcd: &'a Node, strings: &Strings) -> Option<&'a Node> {
    lcd.children.iter().find(|n| n.role == "AXPopUpButton" && strings.is(&n.desc, "Key Signature"))
}

/// The LCD's playhead position group, holding a slider per unit shown (bar, beat, and division and tick when the
/// display shows them).
pub fn playhead<'a>(lcd: &'a Node, strings: &Strings) -> Option<&'a Node> {
    lcd.children.iter().find(|n| n.role == "AXGroup" && strings.is(&n.desc, "Playhead Position"))
}

/// A transport button of the control bar (Play, Record, Stop, Cycle) by its label.
pub fn transport_button<'a>(control_bar: &'a Node, strings: &Strings, label: &str) -> Option<&'a Node> {
    control_bar
        .children
        .iter()
        .find(|n| matches!(n.role.as_str(), "AXCheckBox" | "AXButton") && (strings.is(&n.desc, label) || strings.is(&n.title, label)))
}

/// A view button of the control bar (Editors, List Editors, Mixer, Inspector…) by its label: a checkbox whose value
/// says whether the pane is open.
pub fn view_button<'a>(control_bar: &'a Node, strings: &Strings, label: &str) -> Option<&'a Node> {
    transport_button(control_bar, strings, label)
}

/// The Tracks area's parts: the groups that hold the track headers and the tracks' contents, and the time ruler.
#[derive(Debug, Default)]
pub struct TracksArea<'a> {
    pub headers: Option<&'a Node>,
    pub contents: Option<&'a Node>,
    pub ruler: Option<&'a Node>,
}

impl<'a> TracksArea<'a> {
    /// Found by shape under the Tracks group: the split group whose scroll areas hold a group of `AXLayoutItem`s
    /// (the headers) and a group of `AXLayoutArea`s (the contents); the ruler is the `AXLayoutArea` of the row above.
    pub fn of(tracks: &'a Node) -> TracksArea<'a> {
        let mut area = TracksArea::default();
        let mut splits = vec![];
        tracks.find_all(&|n| n.role == "AXSplitGroup" && n.children.iter().filter(|c| c.role == "AXScrollArea").count() >= 2, &mut splits);
        for split in splits {
            let scrolls: Vec<&Node> = split.children.iter().filter(|c| c.role == "AXScrollArea").collect();
            for scroll in &scrolls {
                let Some(group) = scroll.child_where(|c| c.role == "AXGroup") else { continue };
                if group.children.iter().any(|c| c.role == "AXLayoutItem") && area.headers.is_none() {
                    area.headers = Some(group);
                } else if group.children.iter().any(|c| c.role == "AXLayoutArea") && area.contents.is_none() {
                    area.contents = Some(group);
                }
            }
            // Without a track there is nothing to tell them apart by: the headers come first.
            if area.headers.is_none() && area.contents.is_none() && scrolls.len() >= 2 {
                area.headers = scrolls[0].child_where(|c| c.role == "AXGroup");
                area.contents = scrolls[1].child_where(|c| c.role == "AXGroup");
            }
        }
        area.ruler = tracks.find(&|n| n.role == "AXLayoutArea" && n.children.iter().any(|c| c.subrole == "AXTimeline"));
        area
    }
}

/// The Inspector's parts.
#[derive(Debug, Default)]
pub struct Inspector<'a> {
    /// The selected track's name field (the "Track:" row).
    pub track_name: Option<&'a Node>,
    /// The channel strips shown: the selected track's, then its output's.
    pub strips: Vec<&'a Node>,
}

impl<'a> Inspector<'a> {
    pub fn of(inspector: &'a Node, strings: &Strings) -> Inspector<'a> {
        let mut found = Inspector::default();
        let mut groups = vec![];
        inspector.find_all(
            &|n| n.role == "AXGroup" && n.children.iter().any(|c| c.role == "AXTextField" || c.role == "AXLayoutArea"),
            &mut groups,
        );
        for group in groups {
            if let Some(field) = group.child_where(|c| c.role == "AXTextField") {
                if found.track_name.is_none()
                    && group.children.iter().any(|c| c.role == "AXStaticText" && strings.starts_with(&c.value_text(), "Track"))
                {
                    found.track_name = Some(field);
                }
            }
            if let Some(area) = group.child_where(|c| c.role == "AXLayoutArea" && strings.is(&c.desc, "Mixer")) {
                found.strips = area.children.iter().filter(|c| c.role == "AXLayoutItem").collect();
            }
        }
        found
    }
}

/// The tab of the List Editors that is on (Event, Marker, Tempo or Signature), as its English label.
pub fn list_tab_on(list_tabs: &Node, strings: &Strings) -> Option<&'static str> {
    list_tabs
        .children
        .iter()
        .filter(|c| c.role == "AXRadioButton" && c.is_on())
        .find_map(|tab| ["Event", "Marker", "Tempo", "Signature"].into_iter().find(|label| strings.is(&tab.desc, label)))
}

/// A list editor's table: the `AXTable` inside the list's group.
pub fn list_table(list: &Node) -> Option<&Node> {
    list.find(&|n| n.role == "AXTable")
}

/// A table's columns, by their identifiers (Logic names them in English whatever the language: `Position`, `Name`,
/// `Track`, `Length`, `Tempo`…), each with its index among the row's cells.
pub fn columns(table: &Node) -> Vec<(String, usize)> {
    table.children.iter().filter(|c| c.role == "AXColumn").enumerate().map(|(index, column)| (column.id.clone(), index)).collect()
}

/// A table's rows.
pub fn rows(table: &Node) -> Vec<&Node> {
    table.children.iter().filter(|c| c.role == "AXRow").collect()
}

/// The list's "Number of Items" text ("4 Events", "1 Marker"), for the count without reading the rows.
pub fn item_count(list: &Node, strings: &Strings) -> Option<usize> {
    let text = list.find(&|n| n.role == "AXStaticText" && strings.is(&n.desc, "Number of Items"))?;
    text.value_text().split_whitespace().next()?.parse().ok()
}

/// The channel strip's parts, by the labels of its controls (the Mixer's, which the Inspector's strips share). Logic
/// lists a strip's controls from the bottom up: the name first, then the buttons, the fader, the output and send
/// slots, the inserts from the lowest slot to the highest, the instrument slot, the EQ display and the setting button
/// last; the slots are kept here in that order, so the first insert here is the strip's last. An occupied slot is an
/// `AXGroup` named after the plug-in or the bus, holding its `bypass` checkbox; an empty one a button named for what
/// it takes ("audio plug-in", "send button").
#[derive(Debug, Default)]
pub struct Strip<'a> {
    pub name: Option<&'a Node>,
    pub mute: Option<&'a Node>,
    pub solo: Option<&'a Node>,
    pub record: Option<&'a Node>,
    pub monitoring: Option<&'a Node>,
    pub volume: Option<&'a Node>,
    pub pan: Option<&'a Node>,
    pub output: Option<&'a Node>,
    pub inputs: Vec<&'a Node>,
    pub instrument: Option<&'a Node>,
    /// The send slots, bottom-up, empty ones included.
    pub sends: Vec<&'a Node>,
    /// The insert slots, bottom-up, empty ones included.
    pub inserts: Vec<&'a Node>,
    pub eq: Option<&'a Node>,
    pub bounce: Option<&'a Node>,
    pub channel_mode: Option<&'a Node>,
    /// Whether a MIDI effect slot was listed after the inserts: the group just before it is the instrument's.
    had_midi_slot: bool,
}

/// Whether a slot's node is an occupied one (a group with a bypass checkbox) rather than an empty slot's button.
pub fn occupied(slot: &Node) -> bool {
    slot.role == "AXGroup"
}

impl<'a> Strip<'a> {
    pub fn of(strip: &'a Node, strings: &Strings) -> Strip<'a> {
        let mut found = Strip::default();
        // Occupied slots are groups between the empty slots' buttons; which kind of slot a group is follows from the
        // kind of the empty slot before it (an insert bar sits between inserts).
        let mut last_slot = "";
        for child in &strip.children {
            let desc = child.desc.as_str();
            let help = child.help.as_str();
            match child.role.as_str() {
                "AXTextField" if strings.is(desc, "name") => found.name = Some(child),
                "AXButton" if strings.is(desc, "mute") => found.mute = Some(child),
                "AXButton" if strings.is(desc, "solo") => found.solo = Some(child),
                "AXButton" if strings.is(desc, "record") => found.record = Some(child),
                "AXButton" if strings.is(desc, "monitoring") => found.monitoring = Some(child),
                "AXButton" if strings.is(desc, "bounce") => found.bounce = Some(child),
                "AXButton" if strings.is(desc, "channel mode") => found.channel_mode = Some(child),
                "AXSlider" if strings.is(desc, "volume fader") => found.volume = Some(child),
                "AXSlider" if strings.is(desc, "pan") => found.pan = Some(child),
                "AXButton" if strings.is(desc, "EQ") => found.eq = Some(child),
                "AXButton" if strings.is(desc, "insert bar") => {}
                "AXButton" if strings.starts_with(help, "Output slot") => found.output = Some(child),
                "AXButton" if strings.starts_with(help, "Input slot") => found.inputs.push(child),
                "AXButton" if strings.starts_with(help, "Instrument slot") => {
                    found.instrument = Some(child);
                    last_slot = "instrument";
                }
                "AXButton" if strings.starts_with(help, "Send slot") || strings.is(desc, "send button") => {
                    found.sends.push(child);
                    last_slot = "send";
                }
                "AXButton" if strings.starts_with(help, "Audio Effect slot") || strings.is(desc, "audio plug-in") => {
                    found.inserts.push(child);
                    last_slot = "insert";
                }
                "AXButton" if strings.starts_with(help, "MIDI Effect slot") || strings.is(desc, "MIDI plug-in") => {
                    last_slot = "midi";
                    found.had_midi_slot = true;
                }
                "AXGroup" if child.children.iter().any(|c| c.role == "AXCheckBox" && strings.is(&c.desc, "bypass")) => match last_slot {
                    "send" => found.sends.push(child),
                    "insert" => found.inserts.push(child),
                    _ => {}
                },
                _ => {}
            }
        }
        found.settle_instrument();
        found
    }

    /// The group above the highest insert that is followed by the MIDI effect slot (or nothing) is the instrument's:
    /// Logic lists the instrument slot right after the inserts, and the inserts hold no instrument.
    fn settle_instrument(&mut self) {
        if self.instrument.is_some() {
            return;
        }
        // Among the inserts, a group whose next sibling in the strip is the MIDI plug-in slot is the instrument.
        let Some(last) = self.inserts.last().copied() else { return };
        if occupied(last) && self.had_midi_slot {
            self.inserts.pop();
            self.instrument = Some(last);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(value: serde_json::Value) -> Node {
        Node::from_json(&value)
    }

    #[test]
    fn the_windows_areas_are_found_by_role_and_label_in_any_order() {
        let window = node(json!({"role": "AXWindow", "c": [
            {"role": "AXButton", "subrole": "AXCloseButton"},
            {"role": "AXGroup", "desc": "Spuren", "c": [{"role": "AXGroup", "desc": "Spuren"}]},
            {"role": "AXGroup", "desc": "Steuerungsleiste", "c": [
                {"role": "AXGroup", "desc": "Steuerungsleiste", "c": [
                    {"role": "AXGroup", "desc": "Position der Abspielposition", "c": [{"role": "AXSlider", "desc": "Takt", "value": 9}, {"role": "AXSlider", "desc": "Schlag", "value": 1}]},
                    {"role": "AXSlider", "desc": "Tempo", "value": 160},
                    {"role": "AXPopUpButton", "desc": "Taktart", "value": "4/4"},
                    {"role": "AXPopUpButton", "desc": "Tonart", "value": "C-Dur"}
                ]},
                {"role": "AXCheckBox", "title": "Wiedergabe", "desc": "Wiedergabe", "value": 1},
                {"role": "AXCheckBox", "title": "Mixer", "desc": "Mixer", "value": 0}
            ]},
            {"role": "AXGroup", "desc": "Informationen"},
            {"role": "AXRadioGroup", "c": [{"role": "AXRadioButton", "desc": "Ereignis", "value": 0}, {"role": "AXRadioButton", "desc": "Marker", "value": 1}, {"role": "AXRadioButton", "desc": "Tempo"}, {"role": "AXRadioButton", "desc": "Taktart"}]},
            {"role": "AXGroup", "c": [{"role": "AXGroup", "desc": "Marker", "c": [{"role": "AXStaticText", "desc": "Anzahl", "value": "2 Marker"}, {"role": "AXTable", "c": [{"role": "AXRow"}, {"role": "AXRow"}, {"role": "AXColumn", "id": "Position"}, {"role": "AXColumn", "id": "Name"}]}]}]}
        ]}));
        let strings = Strings::with(
            "de",
            &[
                ("Control Bar#acc", "Steuerungsleiste"),
                ("Inspector#acc", "Informationen"),
                ("StrTabBtnLabel|||Tracks", "Spuren"),
                ("Playhead Position", "Position der Abspielposition"),
                ("Time Signature", "Taktart"),
                ("Key Signature", "Tonart"),
                ("Play", "Wiedergabe"),
                ("StrTabBtnLabel|||Event", "Ereignis"),
                ("StrTabBtnLabel|||Signature", "Taktart"),
                ("Number of Items", "Anzahl"),
            ],
        );
        let found = Window::of(&window, &strings);
        let bar = found.control_bar.expect("the control bar");
        assert_eq!(found.inspector.map(|n| n.desc.as_str()), Some("Informationen"));
        assert!(found.tracks.is_some());
        let lcd = lcd(bar, &strings).expect("the LCD");
        assert_eq!(tempo(lcd, &strings).and_then(Node::number), Some(160.0));
        assert_eq!(time_signature(lcd, &strings).map(Node::value_text), Some("4/4".into()));
        assert_eq!(key_signature(lcd, &strings).map(Node::value_text), Some("C-Dur".into()));
        assert_eq!(playhead(lcd, &strings).map(|n| n.children.len()), Some(2));
        assert!(transport_button(bar, &strings, "Play").unwrap().is_on());
        assert!(!view_button(bar, &strings, "Mixer").unwrap().is_on());
        assert_eq!(transport_button(bar, &strings, "Record"), None);
        let tabs = found.list_tabs.expect("the list tabs");
        assert_eq!(list_tab_on(tabs, &strings), Some("Marker"));
        let list = found.list.expect("the list");
        let table = list_table(list).unwrap();
        assert_eq!(rows(table).len(), 2);
        assert_eq!(columns(table), [("Position".to_string(), 0), ("Name".to_string(), 1)]);
        assert_eq!(item_count(list, &strings), Some(2));
    }

    #[test]
    fn the_tracks_area_is_found_by_its_shape_and_the_inspector_by_its_labels() {
        let tracks = node(json!({"role": "AXGroup", "desc": "Tracks", "c": [
            {"role": "AXGroup", "desc": "Tracks", "c": [{"role": "AXButton", "desc": "Leave Folder"}]},
            {"role": "AXGroup", "desc": "Tracks", "c": [{"role": "AXSplitGroup", "c": [
                {"role": "AXSplitGroup", "c": [{"role": "AXGroup", "desc": "Tracks legend"}, {"role": "AXScrollArea", "c": [{"role": "AXLayoutArea", "desc": "Tracks time ruler", "c": [{"role": "AXValueIndicator", "subrole": "AXTimeline", "desc": "Playhead thumb"}]}]}]},
                {"role": "AXSplitGroup", "c": [
                    {"role": "AXScrollArea", "c": [{"role": "AXGroup", "desc": "Tracks header", "c": [{"role": "AXLayoutItem", "desc": "Track 1 “Vocal”"}]}]},
                    {"role": "AXScrollArea", "c": [{"role": "AXGroup", "desc": "Tracks contents", "c": [{"role": "AXLayoutArea", "desc": "Track 1 “Vocal”"}]}]}
                ]}
            ]}]}
        ]}));
        let area = TracksArea::of(&tracks);
        assert_eq!(area.headers.map(|n| n.desc.as_str()), Some("Tracks header"));
        assert_eq!(area.contents.map(|n| n.desc.as_str()), Some("Tracks contents"));
        assert_eq!(area.ruler.map(|n| n.desc.as_str()), Some("Tracks time ruler"));
        // Without a track, the order tells them apart.
        let empty = node(json!({"role": "AXGroup", "c": [{"role": "AXSplitGroup", "c": [
            {"role": "AXScrollArea", "c": [{"role": "AXGroup", "desc": "h"}]}, {"role": "AXScrollArea", "c": [{"role": "AXGroup", "desc": "c"}]}
        ]}]}));
        let area = TracksArea::of(&empty);
        assert_eq!((area.headers.map(|n| n.desc.as_str()), area.contents.map(|n| n.desc.as_str())), (Some("h"), Some("c")));

        let inspector = node(json!({"role": "AXGroup", "desc": "Inspector", "c": [{"role": "AXList", "c": [
            {"role": "AXGroup", "c": [{"role": "AXScrollArea"}, {"role": "AXStaticText", "value": "Region:"}, {"role": "AXStaticText", "value": "Audio Defaults"}]},
            {"role": "AXGroup", "c": [{"role": "AXDisclosureTriangle"}, {"role": "AXStaticText", "value": "Track:"}, {"role": "AXTextField", "value": "Vocal"}]},
            {"role": "AXGroup", "c": [{"role": "AXLayoutArea", "desc": "Mixer", "c": [
                {"role": "AXLayoutItem", "desc": "Audio 1", "c": [
                    {"role": "AXTextField", "desc": "name", "value": "Vocal"},
                    {"role": "AXButton", "subrole": "AXSwitch", "desc": "mute", "value": "off"},
                    {"role": "AXSlider", "desc": "volume fader", "value": 173, "valueDesc": "0.0 dB"},
                    {"role": "AXSlider", "desc": "pan", "value": 0, "valueDesc": "0"},
                    {"role": "AXButton", "desc": "Stereo Output", "help": "Output slot. Click and hold to choose the channel strip output destination."},
                    {"role": "AXButton", "desc": "send button", "help": "Send slot. Route the signal to an aux channel strip."},
                    {"role": "AXButton", "desc": "audio plug-in", "help": "Audio Effect slot. Insert an audio effect."},
                    {"role": "AXButton", "desc": "Input 1", "help": "Input slot. Choose the channel strip input source."},
                    {"role": "AXButton", "desc": "EQ", "value": "off"}
                ]},
                {"role": "AXLayoutItem", "desc": "Stereo Out", "c": [{"role": "AXTextField", "desc": "name", "value": "Stereo Out"}, {"role": "AXButton", "subrole": "AXSwitch", "desc": "bounce", "value": "off"}]}
            ]}]}
        ]}]}));
        let strings = Strings::english();
        let found = Inspector::of(&inspector, &strings);
        assert_eq!(found.track_name.map(Node::value_text), Some("Vocal".into()));
        assert_eq!(found.strips.len(), 2);
        let strip = Strip::of(found.strips[0], &strings);
        assert_eq!(strip.name.map(Node::value_text), Some("Vocal".into()));
        assert_eq!(strip.volume.map(|n| n.value_desc.as_str()), Some("0.0 dB"));
        assert_eq!(strip.output.map(|n| n.desc.as_str()), Some("Stereo Output"));
        assert_eq!(strip.inputs.iter().map(|n| n.desc.as_str()).collect::<Vec<_>>(), ["Input 1"]);
        assert_eq!((strip.sends.len(), strip.inserts.len()), (1, 1));
        assert!(strip.eq.is_some() && strip.bounce.is_none() && strip.instrument.is_none());
        assert!(Strip::of(found.strips[1], &strings).bounce.is_some());
        // An instrument strip as the Mixer lists it, bottom-up: the empty insert, the occupied ones with insert bars
        // between, the instrument's group, then the MIDI effect slot.
        let keys = node(json!({"role": "AXLayoutItem", "desc": "Keys", "c": [
            {"role": "AXTextField", "desc": "name", "value": "Keys"},
            {"role": "AXButton", "desc": "Stereo Output", "help": "Output slot. Click and hold."},
            {"role": "AXButton", "desc": "send button", "help": "Send slot. Route the signal."},
            {"role": "AXButton", "desc": "audio plug-in", "help": "Audio Effect slot. Insert an audio effect."},
            {"role": "AXButton", "desc": "insert bar"},
            {"role": "AXGroup", "desc": "ChromaVerb", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": false}]},
            {"role": "AXButton", "desc": "insert bar"},
            {"role": "AXGroup", "desc": "Channel EQ", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": true}]},
            {"role": "AXGroup", "desc": "Piano", "c": [{"role": "AXCheckBox", "desc": "bypass", "value": false}]},
            {"role": "AXButton", "desc": "MIDI plug-in", "help": "MIDI Effect slot. Insert a MIDI effect."},
            {"role": "AXButton", "desc": "EQ", "value": "on"}
        ]}));
        let strip = Strip::of(&keys, &strings);
        assert_eq!(strip.instrument.map(|n| n.desc.as_str()), Some("Piano"));
        assert_eq!(strip.inserts.iter().map(|n| n.desc.as_str()).collect::<Vec<_>>(), ["audio plug-in", "ChromaVerb", "Channel EQ"]);
        assert!(occupied(strip.inserts[1]) && !occupied(strip.inserts[0]));
        assert!(strip.inputs.is_empty());
    }
}
