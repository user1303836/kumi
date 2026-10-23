//! FOCUS's bodies from Kumi's host's `focus_body` answers (the M2 contract, §5), the same for every DAW whose bridge is
//! Kumi's own: a device tree from nodes, a take's notes, the timeline with its lanes, a tracker grid, the sequence, an
//! instrument's keyzones, a track's routing.
use crate::core::{
    contracts::{ClipNote, JsonObject},
    view::*,
};
use serde_json::Value;

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
fn color(value: &Value) -> Option<String> {
    value.get("color").and_then(Value::as_str).filter(|c| c.starts_with('#')).map(str::to_owned)
}

/// A device tree from `nodes` (REAPER's FX chain with its input FX, a take's FX chain; Renoise's DSP chain): a
/// container's children are its one chain (drawn as its devices), or one chain per lane where it has lanes (Renoise's
/// Splitter: `lanes` names them, each child's `lane` says where it runs). What the DAW says of a device beside its name
/// is its note: bypassed, offline, or running in parallel with the one before (REAPER's flag).
pub fn tree(track_ref: &str, body: &Value) -> DeviceTree {
    fn node(value: &Value, parent: &str) -> DeviceNode {
        let container = flag(value, "container");
        // (a device inside a container may have no ref of its own: it's named by its place in the container)
        let reference = match value.get("ref").and_then(Value::as_str).filter(|r| !r.is_empty()) {
            Some(reference) => reference.to_owned(),
            None => {
                let at: Vec<String> = list(value, "at").filter_map(Value::as_i64).map(|n| n.to_string()).collect();
                format!("{parent}#{}", at.join("."))
            }
        };
        let lanes: Vec<String> = list(value, "lanes").filter_map(Value::as_str).map(str::to_owned).collect();
        let mut chains: Vec<ChainNode> = if lanes.is_empty() {
            vec![ChainNode { r#ref: format!("{reference}#1"), name: String::new(), devices: Some(vec![]) }]
        } else {
            lanes
                .iter()
                .enumerate()
                .map(|(at, name)| ChainNode { r#ref: format!("{reference}#{}", at + 1), name: name.clone(), devices: Some(vec![]) })
                .collect()
        };
        for child in list(value, "childNodes") {
            let lane =
                number(child, "lane").filter(|n| *n >= 1. && !lanes.is_empty()).map_or(0, |n| (n as usize - 1).min(chains.len() - 1));
            if let Some(devices) = chains[lane].devices.as_mut() {
                devices.push(node(child, &reference));
            }
        }
        // (a container with nothing in it shows as empty, not with an empty chain)
        if lanes.is_empty() && chains[0].devices.as_ref().is_some_and(Vec::is_empty) {
            chains.clear();
        }
        let mut notes = vec![];
        if value.get("enabled") == Some(&Value::Bool(false)) {
            notes.push("bypassed");
        }
        if flag(value, "offline") {
            notes.push("offline");
        }
        if flag(value, "parallel") && lanes.is_empty() && value.get("lane").is_none() {
            notes.push("parallel");
        }
        let form = if container {
            DeviceForm::Container
        } else if flag(value, "plugin") {
            DeviceForm::Plugin
        } else {
            DeviceForm::Native
        };
        DeviceNode {
            r#ref: reference,
            name: text(value, "name"),
            class_name: None,
            can_have_chains: container.then_some(true),
            can_have_drum_pads: None,
            device_type: None,
            chains: container.then_some(chains),
            role: Some(DeviceRole { signal: DeviceSignal::Unknown, form }),
            note: (!notes.is_empty()).then(|| notes.join(" · ")),
        }
    }
    let mut devices: Vec<DeviceNode> = list(body, "nodes").map(|value| node(value, track_ref)).collect();
    // REAPER's input FX run before the track's: one branch of their own, first.
    let input: Vec<DeviceNode> = list(body, "inputNodes").map(|value| node(value, track_ref)).collect();
    if !input.is_empty() {
        let reference = format!("{track_ref}#input");
        devices.insert(
            0,
            DeviceNode {
                r#ref: reference.clone(),
                name: "Input FX".into(),
                class_name: None,
                can_have_chains: Some(true),
                can_have_drum_pads: None,
                device_type: None,
                chains: Some(vec![ChainNode { r#ref: format!("{reference}#1"), name: String::new(), devices: Some(input) }]),
                role: Some(DeviceRole { signal: DeviceSignal::Unknown, form: DeviceForm::Container }),
                note: None,
            },
        );
    }
    let owner = (text(body, "owner") == "take").then(|| format!("take “{}”", text(body, "ownerName")));
    DeviceTree { track_ref: track_ref.into(), devices, owner, layout: body.get("layout").and_then(Value::as_u64) }
}

/// A controller lane's name in REAPER's words ("CC 1", "Pitch", "Program", "Channel pressure", "Poly aftertouch") and the
/// lowest value it says: pitch bend is centred on 0, as REAPER says it (−8192 to 8191; the bridge sends 0 to 16383).
fn lane_name(cc: &Value) -> Option<(String, f64)> {
    Some(match cc {
        Value::Number(n) => (format!("CC {}", n.as_i64()?), 0.),
        Value::String(word) => match word.as_str() {
            "pitchbend" => ("Pitch".into(), -8192.),
            "program" => ("Program".into(), 0.),
            "pressure" => ("Channel pressure".into(), 0.),
            "polypressure" => ("Poly aftertouch".into(), 0.),
            other => (other.to_owned(), 0.),
        },
        _ => return None,
    })
}

/// A take's notes (REAPER's MIDI editor), in beats from the item's start.
pub fn notes(item_ref: &str, body: &Value) -> NoteView {
    let notes = list(body, "notes")
        .take(512)
        .filter_map(|note| {
            Some(ClipViewNote {
                note: ClipNote {
                    pitch: number(note, "pitch")?,
                    start: number(note, "start")?,
                    duration: number(note, "length")?,
                    velocity: number(note, "velocity").unwrap_or(100.),
                },
                selected: flag(note, "selected").then_some(true),
            })
        })
        .collect();
    ClipView {
        slot_ref: item_ref.into(),
        name: text(body, "name"),
        length: number(body, "length").unwrap_or(0.),
        notes,
        beats_per_bar: number(body, "beatsPerBar"),
        note_count: number(body, "noteCount").map(|n| n.max(0.) as usize),
        lanes: list(body, "ccLanes")
            .take(16)
            .filter_map(|lane| {
                let (name, min) = lane_name(lane.get("cc")?)?;
                let points = list(lane, "points")
                    .filter_map(|point| Some([point.get(0)?.as_f64()?, point.get(1)?.as_f64()? + min]))
                    .filter(|[at, value]| at.is_finite() && value.is_finite())
                    .collect();
                Some(ControllerLane { name, min, max: if min < 0. { -min - 1. } else { 127. }, points })
            })
            .collect(),
    }
}

/// The timeline with each track's items as bars (REAPER's arrange view), in beats.
pub fn timeline(body: &Value) -> TimelineStrip {
    // The depth of a folder collapsed all the way: the tracks under it are hidden, as REAPER hides them.
    let mut under: Option<u32> = None;
    let lanes = list(body, "tracks")
        .map(|track| {
            let depth = number(track, "depth").unwrap_or(0.).max(0.) as u32;
            let inside = under.is_some_and(|folder| depth > folder);
            if !inside {
                under = None;
            }
            let collapsed = number(track, "folder") == Some(1.) && text(track, "folderState") == "fully-collapsed";
            if collapsed && !inside {
                under = Some(depth);
            }
            (track, depth, inside || track.get("shown") == Some(&Value::Bool(false)), collapsed)
        })
        .map(|(track, depth, hidden, collapsed)| TimelineLane {
            name: text(track, "name"),
            track_ref: track.get("ref").and_then(Value::as_str).map(str::to_owned),
            color: color(track),
            depth,
            hidden,
            collapsed,
            items: list(track, "items")
                .filter_map(|item| {
                    Some(TimelineItem {
                        start: number(item, "position")?,
                        length: number(item, "length")?,
                        name: text(item, "name"),
                        muted: flag(item, "muted"),
                        selected: flag(item, "selected"),
                    })
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let mut locators: Vec<Locator> = list(body, "markers")
        .chain(list(body, "regions"))
        .filter_map(|mark| Some(Locator { name: text(mark, "name"), position: number(mark, "position")? }))
        .collect();
    locators.sort_by(|a, b| a.position.total_cmp(&b.position));
    let r#loop = body.get("loop").and_then(|span| {
        let (start, end) = (number(span, "start")?, number(span, "end")?);
        (end > start).then(|| ArrangementLoop { start, length: end - start, enabled: flag(span, "on") })
    });
    TimelineStrip {
        length: number(body, "length").unwrap_or(0.),
        position: number(body, "playhead").unwrap_or(0.),
        playing: flag(body, "playing"),
        r#loop,
        locators,
        beats_per_bar: number(body, "beatsPerBar"),
        lanes: Some(lanes),
        track_count: number(body, "trackCount").map(|n| n.max(0.) as usize),
        lanes_from: number(body, "from").map(|n| n.max(0.) as usize),
        meter: list(body, "meter")
            .filter_map(|mark| {
                Some(MeterMark {
                    position: number(mark, "position")?,
                    bar: number(mark, "bar")?,
                    beat: number(mark, "beat").unwrap_or(0.),
                    beats_per_bar: number(mark, "beatsPerBar").filter(|beats| *beats > 0.)?,
                })
            })
            .collect(),
        time_selection: body.get("timeSelection").and_then(|span| {
            let (start, end) = (number(span, "start")?, number(span, "end")?);
            (end > start).then_some([start, end])
        }),
    }
}

/// A tracker track's columns: its note columns (with a sample effect column when any note has one) and its effect
/// columns.
struct Columns {
    notes: usize,
    fx: bool,
    effects: usize,
}

/// The column a note or effect sits in (from 1), or its place in the list when it doesn't say.
fn column(value: &Value, at: usize) -> usize {
    number(value, "col").filter(|n| *n >= 1.).map_or(at + 1, |n| n as usize)
}

/// One line of a tracker track as the DAW writes it: each note column ("C-4 01 80 .. .."), then each effect column
/// ("0A10"), in the DAW's own text; an empty line or column is drawn as the DAW draws it ("--- .. .. .. ..", "....").
fn tracker_cell(line: Option<&Value>, columns: &Columns) -> String {
    let field = |value: Option<&Value>, key: &str, empty: &str| {
        value.and_then(|value| value.get(key)).and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(empty).to_owned()
    };
    let at = |key: &str, col: usize| {
        line.and_then(|line| line.get(key))
            .and_then(Value::as_array)
            .and_then(|items| items.iter().enumerate().find(|(index, item)| column(item, *index) == col).map(|(_, item)| item))
    };
    let mut cells: Vec<String> = (1..=columns.notes)
        .map(|col| {
            let note = at("notes", col);
            let mut fields = vec![
                field(note, "note", "---"),
                field(note, "instrument", ".."),
                field(note, "volume", ".."),
                field(note, "panning", ".."),
                field(note, "delay", ".."),
            ];
            if columns.fx {
                fields.push(field(note, "fx", "...."));
            }
            fields.join(" ")
        })
        .collect();
    cells.extend((1..=columns.effects).map(|col| {
        let effect = at("effects", col);
        format!("{}{}", field(effect, "number", ".."), field(effect, "amount", ".."))
    }));
    cells.join(" ")
}

/// A tracker's pattern or phrase around its cursor (Renoise), lines counted from 1 as the DAW shows them. The DAW
/// sends only the lines with something in them; every line of the window is drawn.
pub fn tracker(body: &Value) -> TrackerGrid {
    let from = number(body, "from").unwrap_or(1.).max(1.) as usize;
    let lines = number(body, "lines").unwrap_or(0.).max(0.) as usize;
    let line = |key: &str| number(body, key).filter(|n| *n >= 1.).map(|n| n as usize - 1);
    let listed = |track: &Value| list(track, "lines").filter_map(|row| number(row, "line")).map(|n| n as usize).max().unwrap_or(0);
    let last = body
        .get("window")
        .and_then(|window| number(window, "to"))
        .map(|to| to as usize)
        .unwrap_or_else(|| list(body, "tracks").map(listed).max().unwrap_or(0));
    let last = if lines > 0 { last.min(lines) } else { last };
    TrackerGrid {
        item_ref: text(body, "ref"),
        name: text(body, "name"),
        lines,
        from: from - 1,
        cursor: line("cursor"),
        playing: line("playing"),
        lpb: number(body, "lpb"),
        tracks: list(body, "tracks")
            .map(|track| {
                let rows: Vec<&Value> = list(track, "lines").collect();
                let widest = |key: &str| {
                    rows.iter().flat_map(|row| list(row, key).enumerate().map(|(index, item)| column(item, index))).max().unwrap_or(0)
                };
                let columns = Columns {
                    notes: (number(track, "noteColumns").unwrap_or(1.).max(0.) as usize).max(widest("notes")),
                    fx: rows
                        .iter()
                        .any(|row| list(row, "notes").any(|note| note.get("fx").and_then(Value::as_str).is_some_and(|fx| !fx.is_empty()))),
                    effects: (number(track, "effectColumns").unwrap_or(0.).max(0.) as usize).max(widest("effects")),
                };
                let at = |n: usize| rows.iter().copied().find(|row| number(row, "line") == Some(n as f64));
                TrackerTrack {
                    name: text(track, "name"),
                    color: color(track),
                    cells: (from..=last).map(|n| tracker_cell(at(n), &columns)).collect(),
                }
            })
            .collect(),
        heading: None,
        facts: vec![],
    }
}

/// A tracker's sequence (Renoise's pattern matrix). A long one comes as the slots around the current one, from slot
/// `from` (counted from 1, as `current` is).
pub fn matrix(body: &Value) -> SequenceMatrix {
    let first = number(body, "from").filter(|n| *n >= 1.).map_or(0, |n| n as usize - 1);
    SequenceMatrix {
        tracks: list(body, "tracks").filter_map(Value::as_str).map(str::to_owned).collect(),
        slots: list(body, "slots")
            .map(|slot| SequenceSlot {
                // (a pattern by Renoise's number, from 0)
                pattern: number(slot, "pattern").unwrap_or(1.).max(1.) as usize - 1,
                section: slot.get("section").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned),
                muted: list(slot, "mutedTracks").filter_map(Value::as_f64).filter(|n| *n >= 1.).map(|n| n as usize - 1).collect(),
                name: slot.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned),
                empty: list(slot, "emptyTracks").filter_map(Value::as_f64).filter(|n| *n >= 1.).map(|n| n as usize - 1).collect(),
                // (by track number, from 1; the pattern a Lua index, shown from 0 as the slot's own)
                aliases: by_track(slot, "aliases")
                    .filter_map(|(track, alias)| Some((track, alias.as_f64().filter(|n| *n >= 1.)? as usize - 1)))
                    .collect(),
                colors: by_track(slot, "colors")
                    .filter_map(|(track, color)| Some((track, color.as_str().filter(|c| c.starts_with('#'))?.to_owned())))
                    .collect(),
            })
            .collect(),
        current: number(body, "current").filter(|n| *n >= 1.).and_then(|n| (n as usize - 1).checked_sub(first)),
        first,
    }
}

/// An object keyed by track number (from 1, as text): each track's position (from 0) and value, in track order.
fn by_track<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = (usize, &'a Value)> {
    let mut found: Vec<(usize, &Value)> = value
        .get(key)
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(track, value)| Some((track.parse::<usize>().ok().filter(|n| *n >= 1)? - 1, value)))
        .collect();
    found.sort_by_key(|(track, _)| *track);
    found.into_iter()
}

/// An instrument's samples on the keyboard (Renoise's keyzones).
pub fn keyzones(item_ref: &str, body: &Value) -> KeyzoneMap {
    // A range as [low, high], in either order; `or` when it isn't one.
    let range = |value: Option<&Value>, or: (u8, u8)| -> (u8, u8) {
        let ends: Vec<u8> =
            value.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_f64).map(|n| n.clamp(0., 127.) as u8).collect();
        match ends.as_slice() {
            [a, b] => (*a.min(b), *a.max(b)),
            _ => or,
        }
    };
    let word = |key: &str| body.get(key).and_then(Value::as_str).filter(|word| !word.is_empty()).map(str::to_owned);
    let phrases: Vec<String> = list(body, "phrases").filter_map(Value::as_str).map(str::to_owned).collect();
    KeyzoneMap {
        item_ref: item_ref.into(),
        name: text(body, "name"),
        zones: list(body, "keyzones")
            .map(|zone| Keyzone {
                sample: text(zone, "sample"),
                notes: range(zone.get("noteRange"), (0, 119)),
                velocities: range(zone.get("velocityRange"), (0, 127)),
                base: number(zone, "baseNote").unwrap_or(48.).clamp(0., 127.) as u8,
                note_off: text(zone, "layer") == "note-off",
                mute_group: number(zone, "muteGroup").filter(|n| *n >= 1.).map(|n| n.min(15.) as u8),
                slice: flag(zone, "slice"),
            })
            .collect(),
        macros: list(body, "macros").filter_map(|m| Some((text(m, "name"), number(m, "value")?))).collect(),
        // (a phrase with no keys has none: null)
        phrase_keys: list(body, "phraseRanges").map(|keys| keys.is_array().then(|| range(Some(keys), (0, 119)))).collect(),
        program: number(body, "phraseProgram").filter(|n| *n >= 1. && (*n as usize) <= phrases.len()).map(|n| n as usize - 1),
        phrases,
        plays_through: word("kind").filter(|kind| kind != "sample"),
        overlap: word("overlap"),
        phrase_playback: word("phrasePlayback"),
        modsets: number(body, "modsets").map(|n| n.max(0.) as usize),
        chains: number(body, "chains").map(|n| n.max(0.) as usize),
    }
}

/// A track's sends and receives (REAPER's routing).
pub fn routing(track_ref: &str, body: &Value) -> RoutingView {
    let routes = list(body, "routes")
        .map(|route| {
            let category = number(route, "category").unwrap_or(0.);
            let kind = if category < 0. {
                RouteKind::Receive
            } else if category > 0. {
                RouteKind::Hardware
            } else {
                RouteKind::Send
            };
            let tap = number(route, "mode").and_then(|mode| match mode as i64 {
                0 => Some("post-fader".to_owned()),
                1 => Some("pre-FX".to_owned()),
                3 => Some("pre-fader".to_owned()),
                8 => Some("pre-receive".to_owned()),
                _ => None,
            });
            Route { kind, other: text(route, "otherName"), volume_db: number(route, "volumeDb"), muted: flag(route, "mute"), tap }
        })
        .collect();
    // (REAPER's MIDI to a hardware output: its device, and channel 0 for all of them)
    let midi_out = body.get("midiOut").filter(|out| out.is_object()).map(|out| {
        let device = out
            .get("name")
            .and_then(Value::as_str)
            .map_or_else(|| format!("device {}", number(out, "device").unwrap_or(0.)), str::to_owned);
        match number(out, "channel").unwrap_or(0.) as i64 {
            0 => format!("{device} · all channels"),
            channel => format!("{device} · channel {channel}"),
        }
    });
    let name = body.get("name").and_then(Value::as_str).map(str::to_owned);
    RoutingView { track_ref: track_ref.into(), name, routes, parent_send: body.get("parentSend").and_then(Value::as_bool), midi_out }
}

/// The mixer's strips (REAPER's with their meters; Renoise's without).
pub fn mixer(body: &Value) -> MixerView {
    let strip = |row: &Value| MixerStrip {
        name: text(row, "name"),
        track_ref: row.get("ref").and_then(Value::as_str).map(str::to_owned),
        color: color(row),
        kind: if flag(row, "master") { Some(TrackKind::Main) } else { row.get("type").and_then(Value::as_str).and_then(track_kind) },
        volume_db: number(row, "volumeDb"),
        pan: number(row, "pan"),
        // (REAPER's mute is on or off; Renoise's is a state: active, off or muted)
        mute: match row.get("mute") {
            Some(Value::Bool(true)) => Some("muted".into()),
            Some(Value::String(state)) if state != "active" => Some(state.clone()),
            _ => None,
        },
        solo: flag(row, "solo"),
        armed: flag(row, "arm"),
        selected: flag(row, "selected"),
        peak_db: row.get("peakDb").and_then(Value::as_array).and_then(|both| Some([both.first()?.as_f64()?, both.get(1)?.as_f64()?])),
    };
    let strips: Vec<MixerStrip> = list(body, "strips").take(80).map(strip).collect();
    MixerView {
        levels: body.get("levels") != Some(&Value::Bool(false)) && strips.iter().any(|strip| strip.peak_db.is_some()),
        strips,
        from: number(body, "from").map_or(0, |n| n.max(0.) as usize),
        track_count: number(body, "trackCount").map(|n| n.max(0.) as usize),
    }
}

/// The track kind FOCUS's chip shows, from a focus's `track.type`.
pub fn track_kind(kind: &str) -> Option<TrackKind> {
    Some(match kind {
        "midi" => TrackKind::Midi,
        "audio" => TrackKind::Audio,
        "folder" | "group" => TrackKind::Group,
        "master" => TrackKind::Main,
        "send" => TrackKind::Return,
        "sequencer" => TrackKind::Sequencer,
        _ => return None,
    })
}

/// The track a focus names, with its chip.
pub fn focus_track(selection: &Value) -> Option<FocusTrack> {
    let track = selection.get("track")?;
    Some(FocusTrack {
        name: text(track, "name"),
        color: color(track),
        kind: track.get("type").and_then(Value::as_str).and_then(track_kind),
    })
}

/// A view by its kind and the DAW's word for it.
pub fn view(kind: ViewKind, label: &str) -> FocusView {
    FocusView { kind, label: label.into() }
}

/// The selection's refs, by their contract keys.
pub fn selected(selection: &Value, key: &str) -> Option<String> {
    selection.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// The transport a focus carries, for FOCUS's beat light: `at` is when it was read (performance time). Its beats are the
/// DAW's own (REAPER's quarter notes through its tempo map, Renoise's beats through its lines per beat), and its bar is
/// the time signature where the playhead is (no 4/4 assumed): `meter` says it in the DAW's words.
pub fn transport(focus: &Value, at: f64) -> Option<Transport> {
    let transport = focus.get("transport")?;
    let signature = transport.get("timeSignature").and_then(Value::as_array).and_then(|parts| match parts.as_slice() {
        [num, den] => Some(format!("{}/{}", num.as_i64()?, den.as_i64()?)),
        _ => None,
    });
    let lpb = number(transport, "lpb").filter(|n| *n >= 1.).map(|n| format!("LPB {n}"));
    Some(Transport {
        playing: flag(transport, "playing"),
        tempo: number(transport, "tempo"),
        beat: number(transport, "beat"),
        at,
        beats_per_bar: number(transport, "beatsPerBar"),
        meter: signature.or(lpb),
        // (Renoise's: lines by its own numbers, from 0)
        line: number(transport, "line").filter(|n| *n >= 0. && flag(transport, "playing")).map(|n| n as usize),
    })
}

/// A body's request as the host's `focus_body` arguments.
pub fn body_args(kind: &str, reference: Option<&str>) -> JsonObject {
    let mut args = JsonObject::new();
    args.insert("kind".into(), kind.into());
    if let Some(reference) = reference {
        args.insert("ref".into(), reference.into());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_routes_tap_is_named_for_every_send_mode_reaper_has() {
        // A send in pre-receive mode (8) showed no tap (review U15).
        let body = json!({"routes": [
            {"category": 0, "otherName": "Bus", "mode": 0},
            {"category": 0, "otherName": "Verb", "mode": 1},
            {"category": 0, "otherName": "Comp", "mode": 3},
            {"category": 0, "otherName": "Sampler", "mode": 8},
        ]});
        let taps: Vec<Option<String>> = routing("r:track:{A}", &body).routes.into_iter().map(|route| route.tap).collect();
        assert_eq!(taps, ["post-fader", "pre-FX", "pre-fader", "pre-receive"].map(|tap| Some(tap.to_owned())));
    }

    #[test]
    fn a_timeline_keeps_hidden_tracks_marked_and_says_where_its_page_starts() {
        let body = json!({"from": 40, "trackCount": 200, "tracks": [
            {"ref": "r:track:{D}", "name": "Drums", "depth": 0, "folder": 1, "folderState": "fully-collapsed", "items": []},
            {"ref": "r:track:{K}", "name": "Kick", "depth": 1, "folder": 0, "items": []},
            {"ref": "r:track:{S}", "name": "Snare", "depth": 1, "folder": -1, "items": []},
            {"ref": "r:track:{B}", "name": "Bass", "depth": 0, "folder": 0, "items": []},
            {"ref": "r:track:{G}", "name": "Guide", "depth": 0, "folder": 0, "shown": false, "items": []},
            {"ref": "r:track:{P}", "name": "Pads", "depth": 0, "folder": 1, "folderState": "collapsed", "items": []},
            {"ref": "r:track:{P1}", "name": "Pad 1", "depth": 1, "folder": -1, "items": []}
        ]});
        let strip = timeline(&body);
        let lanes = strip.lanes.unwrap();
        let seen: Vec<(&str, bool, bool)> = lanes.iter().map(|lane| (lane.name.as_str(), lane.hidden, lane.collapsed)).collect();
        assert_eq!(
            seen,
            [
                ("Drums", false, true),
                ("Kick", true, false),
                ("Snare", true, false),
                ("Bass", false, false),
                ("Guide", true, false),
                ("Pads", false, false),
                ("Pad 1", false, false)
            ],
            "a fully collapsed folder hides its tracks, a collapsed one doesn't, and the producer's hidden track is hidden"
        );
        assert_eq!(lanes[3].track_ref.as_deref(), Some("r:track:{B}"));
        assert_eq!((strip.lanes_from, strip.track_count), (Some(40), Some(200)));
    }

    #[test]
    fn a_trackers_transport_says_the_line_playing_while_it_plays() {
        // (review U7) FOCUS moves a pattern's playing line between reads by it.
        let line = |playing: bool| transport(&json!({"transport": {"playing": playing, "line": 12, "lpb": 4}}), 0.).unwrap().line;
        assert_eq!((line(true), line(false)), (Some(12), None));
        assert_eq!(transport(&json!({"transport": {"playing": true, "beat": 3.0}}), 0.).unwrap().line, None, "REAPER's has none");
    }

    #[test]
    fn a_timeline_counts_bars_by_reapers_meter_and_keeps_the_time_selection() {
        // (review U4) 3/4 from the start, 7/8 from quarter note 24 (bar 9): bars were counted in 4/4.
        let body = json!({"length": 31.0, "beatsPerBar": 3.0, "tracks": [], "timeSelection": {"start": 6.0, "end": 12.0}, "meter": [
            {"position": 0.0, "bar": 1, "beat": 0.0, "beatsPerBar": 3.0},
            {"position": 24.0, "bar": 9, "beat": 0.0, "beatsPerBar": 3.5}
        ]});
        let strip = timeline(&body);
        let bars: Vec<f64> = [0., 2.9, 3., 16., 23.999_999_9, 24., 27.5, 31.].into_iter().map(|beats| strip.bar_at(beats)).collect();
        assert_eq!(bars, [1., 1., 2., 6., 9., 9., 10., 11.]);
        assert_eq!(strip.time_selection, Some([6., 12.]));
        // With no marks, a bar's beats count them; with neither, four.
        let plain = timeline(&json!({"beatsPerBar": 3.0, "tracks": []}));
        assert_eq!((plain.bar_at(16.), timeline(&json!({"tracks": []})).bar_at(16.)), (6., 5.));
    }

    #[test]
    fn a_chain_tree_keeps_reapers_input_fx_take_owner_and_notes_and_renoises_lanes() {
        let fx = |reference: &str, name: &str, more: Value| {
            let mut node = json!({"ref": reference, "name": name, "enabled": true, "offline": false, "plugin": true, "container": false});
            for (key, value) in more.as_object().unwrap() {
                node[key] = value.clone();
            }
            node
        };
        let body = json!({"owner": "take", "ownerName": "kick.wav", "nodes": [
            fx("r:fx:{A}", "ReaEQ", json!({})),
            fx("r:fx:{B}", "ReaComp", json!({"enabled": false, "parallel": true})),
            fx("r:fx:{C}", "Container", json!({"container": true, "plugin": false, "childNodes": [fx("r:fx:{D}", "ReaDelay", json!({"offline": true}))]})),
        ], "inputNodes": [fx("r:fx:{I}", "ReaTune", json!({}))]});
        let tree = tree("r:take:{T}", &body);
        assert_eq!(tree.owner.as_deref(), Some("take “kick.wav”"));
        let names: Vec<&str> = tree.devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Input FX", "ReaEQ", "ReaComp", "Container"], "the input FX first, as a branch of their own");
        assert_eq!(tree.devices[2].note.as_deref(), Some("bypassed · parallel"));
        let inside = tree.devices[3].chains.as_ref().unwrap();
        assert_eq!((inside.len(), inside[0].name.as_str()), (1, ""), "a container's one chain, unnamed");
        assert_eq!(inside[0].devices.as_ref().unwrap()[0].note.as_deref(), Some("offline"));
        // Renoise's Splitter: its lanes by name, each device in its lane (an empty lane stays), inner devices named by place.
        let splitter = json!({"nodes": [{"ref": "3:device:2.4", "name": "Splitter", "container": true, "lanes": ["Low", "High"], "childNodes": [
            {"at": [1, 1], "name": "Gainer", "lane": 1, "enabled": true},
            {"at": [1, 2], "name": "Cabinet", "lane": 1, "enabled": false},
        ]}]});
        let tree = super::tree("3:track:2", &splitter);
        let lanes = tree.devices[0].chains.as_ref().unwrap();
        let shown: Vec<(&str, usize)> = lanes.iter().map(|lane| (lane.name.as_str(), lane.devices.as_ref().unwrap().len())).collect();
        assert_eq!(shown, [("Low", 2), ("High", 0)]);
        let gainer = &lanes[0].devices.as_ref().unwrap()[0];
        assert_eq!((gainer.r#ref.as_str(), gainer.note.as_deref()), ("3:device:2.4#1.1", None));
        assert_eq!(lanes[0].devices.as_ref().unwrap()[1].note.as_deref(), Some("bypassed"));
    }

    #[test]
    fn a_mixers_strips_say_their_levels_where_the_daw_meters_them() {
        let reaper = json!({"strips": [
            {"ref": "r:track:master", "name": "MASTER", "master": true, "volumeDb": 0.0, "pan": 0.0, "mute": false, "solo": false, "peakDb": [-3.0, -4.5]},
            {"ref": "r:track:{K}", "name": "Kick", "volumeDb": -6.02, "pan": -0.25, "mute": true, "solo": false, "arm": true, "selected": true, "peakDb": [-150.0, -150.0]},
        ], "trackCount": 1, "from": 0});
        let view = mixer(&reaper);
        assert!(view.levels);
        assert_eq!(
            (view.strips[0].kind, view.strips[1].mute.as_deref(), view.strips[1].armed),
            (Some(TrackKind::Main), Some("muted"), true)
        );
        let renoise = json!({"strips": [
            {"ref": "e1:track:1", "name": "Kick", "type": "sequencer", "volumeDb": 0.0, "pan": 0.0, "mute": "off", "solo": true},
            {"ref": "e1:track:2", "name": "Bass", "type": "sequencer", "volumeDb": -3.0, "pan": 0.5, "mute": "active", "solo": false},
        ], "trackCount": 2, "from": 0, "levels": false});
        let view = mixer(&renoise);
        assert!(!view.levels, "Renoise has no meters");
        assert_eq!((view.strips[0].mute.as_deref(), view.strips[0].solo, view.strips[1].mute.as_deref()), (Some("off"), true, None));
    }

    #[test]
    fn a_takes_controller_lanes_are_named_in_reapers_words() {
        let body = json!({"name": "Bass", "length": 16.0, "notes": [], "ccLanes": [
            {"cc": 1, "total": 3, "points": [[0.0, 0], [4.0, 64], [8.0, 127]]},
            {"cc": "pitchbend", "total": 1, "points": [[2.0, 8192]]},
        ]});
        let view = notes("r:take:{T}", &body);
        let lanes: Vec<(&str, f64, usize)> = view.lanes.iter().map(|lane| (lane.name.as_str(), lane.max, lane.points.len())).collect();
        assert_eq!(lanes, [("CC 1", 127., 3), ("Pitch", 8191., 1)]);
    }

    #[test]
    fn a_matrix_cut_to_a_page_keeps_its_slot_numbers_and_finds_the_current_one() {
        let slot = |pattern: u32| json!({"pattern": pattern, "mutedTracks": []});
        let body = json!({"tracks": ["Kick"], "slots": [slot(7), slot(8), slot(9)], "current": 42, "from": 41, "slotCount": 300});
        let matrix = matrix(&body);
        assert_eq!((matrix.first, matrix.current), (40, Some(1)), "slot 42 is the second of those shown from slot 41");
        assert_eq!(matrix.slots[matrix.current.unwrap()].pattern, 7, "the bridge's pattern 8 (a Lua index) is Renoise's 07");
        let whole = super::matrix(&json!({"tracks": ["Kick"], "slots": [slot(1), slot(2)], "current": 2}));
        assert_eq!((whole.first, whole.current), (0, Some(1)));
    }

    #[test]
    fn a_matrix_slot_keeps_each_tracks_part_its_aliases_colours_and_emptiness() {
        // (review U9) By track number from 1; an alias's pattern by Renoise's number, as the slot's own.
        let body = json!({"tracks": ["Kick", "Bass", "Pad"], "slots": [{"pattern": 3, "name": "Drop", "mutedTracks": [2],
            "emptyTracks": [3], "aliases": {"2": 1}, "colors": {"1": "#e04040", "3": "nope"}}]});
        let slot = &matrix(&body).slots[0];
        assert_eq!((slot.pattern, slot.name.as_deref(), &slot.muted, &slot.empty), (2, Some("Drop"), &vec![1], &vec![2]));
        assert_eq!((&slot.aliases, &slot.colors), (&vec![(1, 0)], &vec![(0, "#e04040".to_owned())]));
    }
}
