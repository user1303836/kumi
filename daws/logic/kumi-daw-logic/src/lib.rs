//! Logic Pro, through Kumi's own bridge beside it (`logic-kumi`, which reads Logic through its Accessibility helper)
//! and Kumi's host: its refs (positions under the project's epoch, as Renoise's), its read tool, FOCUS's bodies from
//! Logic's own shapes (a channel strip, a region's notes, the Tracks area, a strip's routing), its words for a Logic
//! that answers late or lacks Accessibility, and its instructions. The integration itself is the one every bridge of
//! Kumi's own shares (`own_bridge`).
pub mod changes;
mod inactive;
pub mod library;
mod provider;

pub use provider::{detect_logic, LogicInstall, LOGIC_BRIDGE_SOURCE_VAR, LOGIC_BUNDLES, PROVIDER};

use kumi_agent::core::{
    contracts::Integration,
    presentation::{ToolActivity, ToolPresentation},
    refs::{moved_index, Moved, RefScheme, Shift},
    view::*,
};
use kumi_agent::mcp::client::BridgeDescriptor;
use kumi_daw_own::bridge::{create_own_integration, OwnBridge, OwnDaw, OwnOptions, OwnTools};
use kumi_daw_own::views::{body_args, selected, view};
use kumi_types::contracts::{ClipNote, JsonObject};
use kumi_types::daw::DawProfile;
use regex::Regex;
use serde_json::Value;
use std::{rc::Rc, sync::LazyLock};

/// The instructions Logic's turns carry (M7 brings the full instruction pack).
const INSTRUCTIONS: &str = "You work in Logic Pro through Kumi's bridge beside it, which reads Logic through macOS Accessibility. Read the project with logic_read before you describe it: its tracks (audio, software instrument, external MIDI, aux and output channel strips), their regions, a region's notes as the Event List lists them, each strip's plug-ins and sends, the markers, and the tempo and signatures. A ref such as track:3 is this turn's: pass it back as parent (the track for its regions, plug-ins and sends; a region for its notes) to read what's inside. Kumi reads Logic Pro here; changing it comes later.";

pub const DAW: OwnDaw = OwnDaw {
    profile: &DawProfile::LOGIC,
    descriptor: BridgeDescriptor {
        program: "daw-mcp-server",
        policy_variable: "KUMI_DAW_TOOL_POLICY",
        allow_variable: "KUMI_DAW_TOOL_ALLOW",
        events: "notifications/daw_event",
        daw: "Logic Pro",
    },
    host_flag: "logic",
    bridge: OwnBridge::Beside,
    instructions: INSTRUCTIONS,
    mechanical: "two equal routes to the Stereo Out, a duplicated send, a send to a bus no aux takes, a muted track or a bypassed plug-in",
    read_tool: "logic_read",
    read_description: "Read Logic Pro: the project, its tracks, and what's on them (regions, a region's notes, a channel strip's plug-ins and sends, markers, the tempo and signatures), a page at a time. Rows carry refs to pass back as parent; a page with nextCursor has more. Tracks come in the Tracks area's order: kind is audio, instrument, external, aux or output; volumeDb is the fader, pan runs -64 to 63, output names the bus or Stereo Out. A region's startBeats and endBeats count from the project's start. A note's position is Logic's bar, beat, division (a sixteenth) and tick (240 to a division), its lengthTicks in ticks of 1/960 of a beat, its channel 1-16. A plug-in's slot counts the strip's inserts from 1 (kind instrument or effect, bypassed when off); a send names its destination bus and its level in dB.",
    read_kinds: &["project", "track", "region", "note", "plugin", "send", "marker", "tempo"],
    project_kind: "project",
    fill_focus,
    bodies: &[BodyKind::Tree, BodyKind::Notes, BodyKind::Timeline, BodyKind::Routing],
    body_request,
    to_body,
    busy_words,
    permission_words,
    changes: &changes::CHANGES,
    open_project: Some(provider::open_project),
    // Required descriptions stay inactive: the profile offers no changes, catch-up, phrases, scripts or listening.
    catch_up: &inactive::CATCH_UP,
    phrases: &inactive::PHRASES,
    scripts: &inactive::SCRIPTS,
    spec: &inactive::SPEC,
    listening: &inactive::LISTENING,
    outside_code: "code run in Logic Pro",
};

/// Why Logic answers late, from the bridge's `busy` (the contract's `modal`, `away` and `loading`).
fn busy_words(code: &str) -> String {
    match code {
        "modal" => "Logic Pro is showing a dialog".into(),
        "away" => "Logic Pro's window is on another desktop".into(),
        "loading" => "Logic Pro is still opening the project".into(),
        other => other.into(),
    }
}

/// What the bridge's helper lacks (`accessibility` until the producer allows it), and how to grant it.
fn permission_words(permission: &str) -> String {
    match permission {
        "accessibility" => "Kumi's helper needs Accessibility to read Logic Pro: allow it under System Settings > Privacy & Security > Accessibility (macOS lists the app Kumi runs in)".into(),
        other => format!("Kumi's helper lacks the {other} permission"),
    }
}

/// The track kind FOCUS's chip shows, from Logic's track kinds.
fn logic_track_kind(kind: &str) -> Option<TrackKind> {
    Some(match kind {
        "audio" => TrackKind::Audio,
        "instrument" | "external" => TrackKind::Midi,
        "aux" => TrackKind::Return,
        "output" => TrackKind::Main,
        "folder" | "stack" => TrackKind::Group,
        _ => return None,
    })
}

/// What the producer looks at in Logic: the Tracks area (or the Mixer), with the Piano Roll, the Event List or a
/// plug-in window over it when open; the selected track, region and plug-in.
fn fill_focus(focus: &Value) -> DawFocus {
    let selection = focus.get("selection").cloned().unwrap_or(Value::Null);
    let name = |key: &str| selection.get(key).and_then(Value::as_str).filter(|name| !name.is_empty()).map(str::to_owned);
    let main = match focus.get("view").and_then(Value::as_str) {
        Some("mixer") => view(ViewKind::Mixer, "Mixer"),
        _ => view(ViewKind::Timeline, "Tracks"),
    };
    let editor = match focus.get("editor").and_then(Value::as_str) {
        Some("pianoRoll") => Some(view(ViewKind::NoteEditor, "Piano Roll")),
        Some("eventList") => Some(view(ViewKind::NoteEditor, "Event List")),
        Some("stepSequencer") => Some(view(ViewKind::PatternEditor, "Step Sequencer")),
        Some("mixer") => Some(view(ViewKind::Mixer, "Mixer")),
        Some("plugin") => Some(view(ViewKind::DeviceChain, "Plug-in")),
        _ => None,
    };
    let track = selection.get("track").map(|track| FocusTrack {
        name: track.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
        color: track.get("color").and_then(Value::as_str).filter(|c| c.starts_with('#')).map(str::to_owned),
        kind: track.get("kind").or_else(|| track.get("type")).and_then(Value::as_str).and_then(logic_track_kind),
    });
    let item = selected(&selection, "regionRef").map(|reference| FocusItem {
        kind: ItemKind::Item,
        item_ref: Some(reference),
        name: name("regionName"),
    });
    DawFocus {
        track,
        track_ref: selected(&selection, "trackRef"),
        device_ref: selected(&selection, "pluginRef"),
        device: name("pluginName"),
        main_view: Some(main),
        editor_view: editor,
        item,
        ..Default::default()
    }
}

/// FOCUS draws a screenful of lanes, so it asks for that many tracks, not a page of every track in the project.
const TRACKS_SHOWN: u32 = 24;

fn body_request(request: &BodyRequest) -> Option<JsonObject> {
    Some(match request {
        BodyRequest::Tree { track_ref } => body_args("tree", Some(track_ref)),
        BodyRequest::Notes { item_ref } => body_args("notes", Some(item_ref)),
        BodyRequest::Timeline => {
            let mut args = body_args("timeline", None);
            args.insert("limit".into(), TRACKS_SHOWN.into());
            args
        }
        BodyRequest::Routing { track_ref } => body_args("routing", Some(track_ref)),
        _ => return None,
    })
}

fn to_body(request: &BodyRequest, body: &Value) -> Option<FocusBody> {
    Some(match request {
        BodyRequest::Tree { track_ref } => FocusBody::Tree(strip_tree(track_ref, body)),
        BodyRequest::Notes { item_ref } => FocusBody::Notes(region_notes(item_ref, body)),
        BodyRequest::Timeline => FocusBody::Timeline(lanes_timeline(body)),
        BodyRequest::Routing { track_ref } => FocusBody::Routing(strip_routing(track_ref, body)),
        _ => return None,
    })
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}
fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64).filter(|n| n.is_finite())
}
fn flag(value: &Value, key: &str) -> bool {
    value.get(key) == Some(&Value::Bool(true))
}
fn list<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value.get(key).and_then(Value::as_array).into_iter().flatten()
}

/// A channel strip's inserts as FOCUS's device tree: each plug-in a node, in slot order (Logic chains inserts in
/// series; parallel paths are busses, which routing shows).
fn strip_tree(track_ref: &str, body: &Value) -> DeviceTree {
    let strip = body.get("strip").unwrap_or(body);
    let devices = list(strip, "inserts")
        .map(|insert| {
            let signal = match insert.get("kind").and_then(Value::as_str) {
                Some("instrument") => DeviceSignal::Instrument,
                Some("effect") => DeviceSignal::Audio,
                Some("midi") => DeviceSignal::Midi,
                _ => DeviceSignal::Unknown,
            };
            DeviceNode {
                r#ref: insert.get("pluginRef").or_else(|| insert.get("ref")).and_then(Value::as_str).unwrap_or_default().to_owned(),
                name: text(insert, "name"),
                class_name: None,
                can_have_chains: None,
                can_have_drum_pads: None,
                device_type: None,
                chains: None,
                role: Some(DeviceRole { signal, form: DeviceForm::Plugin }),
                note: None,
            }
        })
        .collect();
    DeviceTree { track_ref: track_ref.into(), devices, owner: None, layout: None }
}

/// Logic's resolution: 960 ticks to a beat (a quarter note), a division being a sixteenth of 240 ticks.
const TICKS_PER_BEAT: f64 = 960.;
const DIVISIONS_PER_BEAT: f64 = 4.;

/// A position as Logic shows it (bar, beat, division and tick, each from 1), in beats from the project's start.
fn beats_of(position: &Value, beats_per_bar: f64) -> Option<f64> {
    let (bar, beat) = (number(position, "bar")?, number(position, "beat")?);
    let division = number(position, "division").unwrap_or(1.);
    let tick = number(position, "tick").unwrap_or(1.);
    Some((bar - 1.) * beats_per_bar + (beat - 1.) + (division - 1.) / DIVISIONS_PER_BEAT + (tick - 1.) / TICKS_PER_BEAT)
}

/// A bar's beats from a body: `beatsPerBar`, else the numerator of its `timeSignature` ("4/4"); four when it doesn't say.
fn beats_per_bar(body: &Value) -> f64 {
    number(body, "beatsPerBar")
        .or_else(|| {
            body.get("timeSignature").and_then(Value::as_str).and_then(|signature| signature.split('/').next()?.trim().parse().ok())
        })
        .filter(|n| *n > 0.)
        .unwrap_or(4.)
}

/// A region's notes as the Event List lists them, in beats from the region's start (Logic positions them in the
/// project), each as long as its ticks say.
fn region_notes(item_ref: &str, body: &Value) -> NoteView {
    let beats = beats_per_bar(body);
    let start = number(body, "startBeats").unwrap_or(0.);
    let end = number(body, "endBeats").unwrap_or(start);
    let notes = list(body, "notes")
        .take(512)
        .filter_map(|note| {
            Some(ClipViewNote {
                note: ClipNote {
                    pitch: number(note, "pitch")?,
                    start: (beats_of(note.get("position")?, beats)? - start).max(0.),
                    duration: number(note, "lengthTicks")? / TICKS_PER_BEAT,
                    velocity: number(note, "velocity").unwrap_or(100.),
                },
                selected: flag(note, "selected").then_some(true),
            })
        })
        .collect();
    ClipView {
        slot_ref: item_ref.into(),
        name: text(body, "name"),
        length: (end - start).max(0.),
        notes,
        beats_per_bar: Some(beats),
        lanes: vec![],
        note_count: None,
    }
}

/// The Tracks area: each track a lane, its regions as bars, in beats.
fn lanes_timeline(body: &Value) -> TimelineStrip {
    let lanes: Vec<TimelineLane> = list(body, "lanes")
        .map(|lane| TimelineLane {
            name: text(lane, "name"),
            track_ref: lane.get("trackRef").and_then(Value::as_str).map(str::to_owned),
            color: lane.get("color").and_then(Value::as_str).filter(|c| c.starts_with('#')).map(str::to_owned),
            depth: number(lane, "depth").unwrap_or(0.).max(0.) as u32,
            items: list(lane, "regions")
                .filter_map(|region| {
                    let (start, end) = (number(region, "startBeats")?, number(region, "endBeats")?);
                    Some(TimelineItem {
                        start,
                        length: (end - start).max(0.),
                        name: text(region, "name"),
                        muted: flag(region, "muted"),
                        selected: flag(region, "selected"),
                    })
                })
                .collect(),
            hidden: flag(lane, "hidden"),
            collapsed: flag(lane, "collapsed"),
        })
        .collect();
    let furthest = lanes.iter().flat_map(|lane| lane.items.iter().map(|item| item.start + item.length)).fold(0., f64::max);
    let window_end = body.get("window").and_then(|window| number(window, "to")).unwrap_or(0.);
    let mut locators: Vec<Locator> = list(body, "markers")
        .filter_map(|marker| {
            let position = number(marker, "positionBeats").or_else(|| beats_of(marker.get("position")?, beats_per_bar(body)))?;
            Some(Locator { name: text(marker, "name"), position })
        })
        .collect();
    locators.sort_by(|a, b| a.position.total_cmp(&b.position));
    TimelineStrip {
        length: number(body, "length").unwrap_or(furthest.max(window_end)),
        position: number(body, "playheadBeats").unwrap_or(0.),
        playing: flag(body, "playing"),
        r#loop: body.get("cycle").and_then(|cycle| {
            let (start, end) = (number(cycle, "start")?, number(cycle, "end")?);
            (end > start).then(|| ArrangementLoop { start, length: end - start, enabled: flag(cycle, "on") })
        }),
        locators,
        beats_per_bar: number(body, "beatsPerBar"),
        lanes: Some(lanes),
        meter: vec![],
        time_selection: None,
        track_count: number(body, "trackCount").map(|n| n.max(0.) as usize),
        lanes_from: number(body, "from").map(|n| n.max(0.) as usize),
    }
}

/// A channel strip's routing: its inputs, its sends (each to a bus, at its level) and its output.
fn strip_routing(track_ref: &str, body: &Value) -> RoutingView {
    let mut routes: Vec<Route> = list(body, "inputs")
        .filter_map(Value::as_str)
        .filter(|input| !input.is_empty())
        .map(|input| Route { kind: RouteKind::Receive, other: input.into(), volume_db: None, muted: false, tap: None })
        .collect();
    routes.extend(list(body, "sends").map(|send| Route {
        kind: RouteKind::Send,
        other: text(send, "destination"),
        volume_db: number(send, "level"),
        muted: flag(send, "muted"),
        tap: send.get("tap").and_then(Value::as_str).filter(|tap| !tap.is_empty()).map(str::to_owned),
    }));
    if let Some(output) = body.get("output").and_then(Value::as_str).filter(|output| !output.is_empty()) {
        routes.push(Route { kind: RouteKind::Hardware, other: output.into(), volume_db: None, muted: false, tap: None });
    }
    RoutingView {
        track_ref: track_ref.into(),
        name: body.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).map(str::to_owned),
        routes,
        parent_send: None,
        midi_out: None,
    }
}

/// Logic's refs: `<epoch>:<kind>:<numbers>`, positions as the bridge counts them (from 1), children under their
/// parent's position (`1:plugin:2.1` is track 2's first insert). A track inserted or deleted moves the tracks after it
/// and what's on them; the epoch moves on every track or region change, retiring every ref of the epoch before.
#[derive(Default)]
pub struct LogicRefs;

/// A ref's parts: its epoch, kind and the numbers after it.
fn parts(reference: &str) -> Option<(&str, &str, Vec<usize>)> {
    let mut parts = reference.splitn(3, ':');
    let (epoch, kind, numbers) = (parts.next()?, parts.next()?, parts.next()?);
    if epoch.is_empty() || kind.is_empty() {
        return None;
    }
    let numbers: Option<Vec<usize>> = numbers.split('.').map(|n| n.parse().ok()).collect();
    Some((epoch, kind, numbers?))
}

/// Whether a ref's first number is its track's position: a track's own, a plug-in's or a send's always; a region's
/// or a note's only when the bridge nests it under its track (`region:2.1`, `note:2.1.3`), not when it counts regions
/// through the project (`region:1`).
fn on_track(kind: &str, numbers: &[usize]) -> bool {
    match kind {
        "track" | "plugin" | "send" => !numbers.is_empty(),
        "region" => numbers.len() >= 2,
        "note" => numbers.len() >= 3,
        _ => false,
    }
}

impl RefScheme for LogicRefs {
    fn positional(&self) -> bool {
        true
    }
    fn kind_of<'a>(&self, reference: &'a str) -> Option<&'a str> {
        parts(reference).map(|(_, kind, _)| kind)
    }
    fn moved(&self, shift: &Shift, reference: &str) -> Moved {
        let Some((epoch, kind, mut numbers)) = parts(reference) else { return Moved::Same };
        if !on_track(kind, &numbers) {
            return Moved::Same;
        }
        match moved_index(numbers[0], &shift.tracks_gone, &shift.tracks_made) {
            Some(now) if now == numbers[0] => Moved::Same,
            Some(now) => {
                numbers[0] = now;
                let numbers: Vec<String> = numbers.iter().map(usize::to_string).collect();
                Moved::To(format!("{epoch}:{kind}:{}", numbers.join(".")))
            }
            None => Moved::Gone,
        }
    }
    fn place_of(&self, reference: &str) -> String {
        reference.into()
    }
    fn refs_in<'a>(&self, text: &'a str) -> Vec<&'a str> {
        static REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9A-Za-z]+:[a-z]+:[0-9]+(?:\.[0-9]+)*").unwrap());
        REF.find_iter(text).map(|found| found.as_str()).collect()
    }
    fn reference_fields(&self) -> &[String] {
        &[]
    }
    fn parents(&self, kind: &str) -> Option<Vec<String>> {
        let parents: &[&str] = match kind {
            "region" | "plugin" | "send" => &["track"],
            "note" => &["region"],
            _ => return None,
        };
        Some(parents.iter().map(|parent| parent.to_string()).collect())
    }
    fn parent_hint(&self, kind: &str, _parent: &str) -> &'static str {
        match kind {
            "note" => "read a region's notes with its ref as parent",
            _ => "",
        }
    }
    fn row_refs(&self, _kind: &str, _row: &JsonObject) -> Vec<(String, String, bool)> {
        vec![]
    }
    fn track_of(&self, reference: &str) -> Option<String> {
        let (epoch, kind, numbers) = parts(reference)?;
        on_track(kind, &numbers).then(|| format!("{epoch}:track:{}", numbers[0]))
    }
}

/// Logic's tools as the app shows them (M1 reads; nothing acts).
pub const PRESENTATION: ToolPresentation = ToolPresentation {
    label: |tool| match tool {
        "logic_read" => Some("looked at your project"),
        _ => None,
    },
    doing: |tool| match tool {
        "logic_read" => Some("looking at your project"),
        _ => None,
    },
    activity: |tool| (tool == "logic_read").then_some(ToolActivity::Read),
    plan: |tool| tool == "make_changes",
    acts: |_| false,
    read_prefix: Some("logic_"),
};

/// How the app reads Logic's focus and strips: the focus comes neutral, and a region in the Piano Roll shows its notes.
pub const VIEW: ViewPresentation = ViewPresentation {
    heading: |kind| match kind {
        BodyKind::Tree => "Channel Strip",
        BodyKind::Notes => "Piano Roll",
        BodyKind::Timeline => "Tracks",
        BodyKind::Routing => "Routing",
        other => (ViewPresentation::NONE.heading)(other),
    },
    timeline_notes: true,
    ..ViewPresentation::NONE
};

/// Logic's integration.
pub fn create_logic_integration(options: OwnOptions) -> Rc<dyn Integration> {
    create_own_integration::<LogicRefs>(&DAW, options, |_| OwnTools::default())
}
