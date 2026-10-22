//! REAPER, through Kumi's extension and host: its refs (REAPER's GUIDs, which hold across reloads), its read tool and
//! its instructions. The integration itself is the one every bridge of Kumi's own shares (`own_bridge`).
use super::own_bridge::{create_own_integration, OwnDaw, OwnOptions};
use super::own_catchup::{as_read, CatchUpWords, Mixer, RecipeKinds};
use super::own_phrases::{bar_span, track, OwnPhrases, PhraseLook, Phrased};
use super::own_views::{self, body_args, focus_track, selected, view};
use crate::core::{
    contracts::{Integration, JsonObject},
    daw::DawProfile,
    describe::ProjectWords,
    phrases::{Intent, Switch},
    presentation::{ToolActivity, ToolPresentation},
    refs::{Moved, RefScheme, Shift},
    view::{BodyKind, BodyRequest, DawFocus, FocusBody, FocusItem, FocusParameter, ItemKind, ViewKind, ViewPresentation},
};
use crate::mcp::client::BridgeDescriptor;
use regex::Regex;
use serde_json::{json, Value};
use std::{rc::Rc, sync::LazyLock};

/// The instructions REAPER's turns carry: reading, the producer's setup, and how REAPER's power users work (M7's pack:
/// the rules the research found that the lab or the research's sources back; render wildcards checked in the lab, KP3).
const INSTRUCTIONS: &str = concat!(
    "You work in REAPER through Kumi's extension. Read the project with reaper_read before you describe it: its tracks (depth and folder show the folders), items and takes, FX and their parameters, sends and receives, markers, regions and envelopes. A ref such as track:3 is this turn's: pass it back as parent to read what's inside.",
    " The project's timebase is in project. In the look's setup, new items get newItemFades: off, or a fade in and out that long. Add no fade an item wouldn't get, and never a fade-in on a drum hit. An item extended past its source repeats it only when it loops (loopSourceOfNewItems for new ones; read an item's own), and with the items timebase time a tempo change leaves items where they are in seconds. Call an extension's functions or named commands (SWS's _SWS_…, ReaPack, ReaImGui, js_ReaScriptAPI) only when setup.extensions says it's there. render is what File > Render does now: say so before you change it.",
    " Work as REAPER's power users do, in REAPER's words (items, takes, regions, FX, actions, named rather than by their keys), and put another DAW's words into REAPER's without comment (Strip Silence is a dynamic split; a Pro Tools region is an item). Tell the job from the project before choosing how: many short regions or items rendered by $region or $track names at 48 kHz is game audio; speech on a time timebase is voice-over or a podcast; hundreds of tracks with offline instruments is a composer's template; anything else is music.",
    " When the producer says this or these, or names nothing to edit: a razor area, else the selected items (within the time selection, if there is one), else the time selection; ask only when none is set.",
    " Names a render pattern uses ($project, $region, $track, $parenttrack, $timelineorder) become file names: keep the project's naming style (such as UCS's CatID_FXName), with no spaces for game audio or a spec that asks it, say so before renaming, and check the render targets (REAPER adds -001, -002 to names that clash). Files for a game or a client go through render_files: plan the assets, make their regions, check the files against the names, then render (into the project's render folder unless the producer names one).",
    " Variations keep their sources: new items or takes with small changes of pitch, playrate, start offset and fades, a region each.",
    " Spoken word works on a time timebase in minutes and seconds, without beat snapping; deliver to a spec only when one is named (ACX: RMS −23 to −18 dB, peaks at most −3 dB, a noise floor at most −60 dB RMS, 0.5–1 s of room tone at the head and 1–5 s at the tail, at most 120 minutes a file, all files mono or all stereo, 192 kbps CBR MP3 at 44.1 kHz).",
    " In a big template never bring offline FX online, show hidden tracks or move folders as a side effect: a new track goes into its section's folder and stem. Reverbs and delays go on shared FX buses fed by sends, or into each stem when the project delivers stems.",
    " Keep the producer's colours and naming; colour tracks by family only in an uncoloured project.",
    " Corrective work takes ReaEQ, ReaComp, ReaLimit and JS; reverb, saturation and instruments come from what the producer has installed first (REAPER's own instruments are ReaSynth, ReaSynDr and ReaSamplOmatic5000). A multi-FX sound to keep goes in a container with macros; match levels before comparing."
);

pub const DAW: OwnDaw = OwnDaw {
    profile: &DawProfile::REAPER,
    descriptor: BridgeDescriptor::REAPER,
    host_flag: "reaper",
    instructions: INSTRUCTIONS,
    mechanical: "two equal routes to the master, a duplicated send, a send on the wrong channels, a muted or bypassed path",
    read_tool: "reaper_read",
    read_description: "Read REAPER: the project, its tracks, and what's in them (items, FX, parameters, sends, markers, regions, MIDI notes, envelopes), a page at a time. Rows carry refs to pass back as parent; a page with nextCursor has more. Tracks come in project order: depth is how deep in folders a track sits (0 at the top), folder 1 starts a folder and -1 or less closes one or more; pan runs -1 to 1. A send's mode is 0 post-fader, 1 pre-FX, 3 pre-fader, 8 pre-receive (modeName says it). Items' position and length are seconds, with positionBeats and lengthBeats in quarter notes from the project's start; markers and regions add positionBeats too; a note's start (from its item's start) and length are quarter notes, its channel 1-16. Kind event lists a MIDI take's events beyond notes (cc, pitchbend, program, pressure, poly-pressure, text, sysex; start in quarter notes from the item's start) with the index the MIDI event changes take; a take read alone lists its stretch and take markers with theirs. Kind call calls one of REAPER's API functions that only reads, by name (fn), with args in order or by the SDK's parameter names (refs for tracks, items, takes and envelopes): its result, and in out the values it writes back. An FX's parent is a track (its input FX follow its main chain, input: true), a take or a container; one FX read by ref also gives its state chunk, presets, wet and delta. Envelopes come with their units; kind point reads an envelope's (or an automation item's) points in those units, kind autoitem an envelope's automation items.",
    read_kinds: &["project", "track", "item", "take", "fx", "param", "send", "marker", "region", "note", "event", "envelope", "call", "point", "autoitem"],
    project_kind: "project",
    fill_focus,
    bodies: &[BodyKind::Tree, BodyKind::Notes, BodyKind::Timeline, BodyKind::Routing, BodyKind::Mixer],
    body_request,
    to_body,
    changes: &super::reaper_changes::CHANGES,
    catch_up: &CATCH_UP,
    phrases: &PHRASES,
};

/// What changed in a REAPER project, in REAPER's words: catch-up's lines, watch_me's changes and its draft recipe.
pub const CATCH_UP: CatchUpWords = CatchUpWords {
    words: ProjectWords {
        kinds: &[("track", "tracks"), ("FX", "FX"), ("item", "items"), ("marker", "markers"), ("region", "regions")],
        project: "project",
        rank: &["project", "track", "FX", "item", "marker", "region"],
        opaque: &["FX"],
    },
    facts: project_facts,
    recipe: RecipeKinds {
        track_add: "track_add",
        track_types: &[],
        effect: "FX",
        // (added again by the plug-in's own name, whatever the producer renamed it)
        effect_add: |made, name| {
            let plugin = made.get("plugin").and_then(Value::as_str).filter(|plugin| !plugin.is_empty()).unwrap_or(name);
            (!plugin.is_empty()).then(|| ("fx_add", serde_json::json!({"name": plugin}).as_object().cloned().unwrap_or_default()))
        },
        effect_values: |values| {
            let values: Vec<Value> = values.iter().map(|(name, value)| serde_json::json!({"param": name, "normalized": value})).collect();
            ("fx_params", serde_json::json!({"values": values}).as_object().cloned().unwrap_or_default())
        },
        mixer: &[
            Mixer { what: "mixer.volume", tool: "track_volume", field: "volumeDb", value: as_read, with: &[] },
            Mixer { what: "mixer.pan", tool: "track_pan", field: "pan", value: as_read, with: &[] },
            Mixer { what: "mixer.width", tool: "track_width", field: "width", value: as_read, with: &[] },
            Mixer { what: "mixer.mute", tool: "track_mute", field: "mute", value: as_read, with: &[] },
            Mixer { what: "mixer.solo", tool: "track_solo", field: "solo", value: as_read, with: &[] },
            Mixer { what: "mixer.phase", tool: "track_phase", field: "inverted", value: as_read, with: &[] },
        ],
    },
    watch_description: "Learn a routine the producer does by hand in REAPER: action start just before they do it (Kumi notes the project as it is; nothing in REAPER changes), then action stop when they say they're done. Stop gives what changed: tracks added (with their mixer, folder and sends), FX added (with the parameters they moved after adding them), items, markers and settings changed, each with its track; and recipeSteps, a draft of the steps that would redo it in your change kinds, with $blanks for the tracks they worked on (the one they selected is $track). Save it at once with save_recipe from those steps, fixing the draft where it's off and leaving out anything clearly unrelated. Then say in a few words what the recipe does. The producer can ask you to change it.",
};

/// The project's own values that changed, as catch-up lines.
fn project_facts(details: &[Value]) -> Vec<String> {
    let text = |value: &Value| kumi_common::js::json::stringify(value);
    let mut lines = vec![];
    for detail in details {
        let (from, to) = (&detail["from"], &detail["to"]);
        match detail["what"].as_str().unwrap_or("") {
            "tempo" if from.is_number() && to.is_number() => lines.push(format!("Tempo {} → {} BPM", text(from), text(to))),
            "timeSignature" => {
                let signature = |value: &Value| value.as_array().map(|parts| parts.iter().map(text).collect::<Vec<_>>().join("/"));
                if let (Some(from), Some(to)) = (signature(from), signature(to)) {
                    lines.push(format!("Time signature {from} → {to}"));
                }
            }
            what if what.starts_with("tempoMap.") => lines.push("Changed the tempo map".into()),
            what if what.starts_with("markers.") => lines.push("Changed the markers and regions".into()),
            _ => {}
        }
    }
    lines.dedup();
    lines
}

/// REAPER's fixed phrases: the tempo of a project without a tempo map, a track's mute, solo and arm, the loop (its range
/// and repeat) and the metronome.
const PHRASES: OwnPhrases = OwnPhrases { plan: phrase, loops_by_slot: false };

fn phrase(intent: &Intent, look: &PhraseLook) -> Phrased {
    let step = |tool: &str, input: Value| json!({"tool": tool, "input": input});
    // Tempo or time signature markers: which one a tempo means, and how long a bar is, depend on where. The model asks.
    let mapped = look.project.get("tempoMarkers").and_then(Value::as_u64).is_some_and(|markers| markers > 0);
    match intent {
        Intent::Tempo { bpm } if *bpm > 960.0 => Phrased::Refused("REAPER's tempo goes up to 960 BPM.".into()),
        Intent::Tempo { .. } if mapped => Phrased::Model,
        Intent::Tempo { bpm } => Phrased::Steps(vec![step("tempo_set", json!({"bpm": bpm}))]),
        Intent::Track { track: pick, switch, on } => match track(pick, look) {
            Some(reference) => Phrased::Steps(vec![match switch {
                Switch::Mute => step("track_mute", json!({"ref": reference, "mute": on})),
                Switch::Solo => step("track_solo", json!({"ref": reference, "solo": on})),
                Switch::Arm => step("track_arm", json!({"ref": reference, "arm": on})),
            }]),
            None => Phrased::Model,
        },
        Intent::Loop { .. } if mapped => Phrased::Model,
        Intent::Loop { from_bar, to_bar } => {
            // A bar in quarter notes, from the time signature: 4/4 is 4, 6/8 is 3.
            let signature = look.project.get("timeSignature").and_then(Value::as_array);
            let bar = match signature.map(|s| (s.first().and_then(Value::as_f64), s.get(1).and_then(Value::as_f64))) {
                Some((Some(num), Some(den))) if num > 0.0 && den > 0.0 => num * 4.0 / den,
                _ => 4.0,
            };
            let (start, end) = bar_span(*from_bar, *to_bar, bar);
            let mut steps = vec![step("loop_range", json!({"startBeats": start, "endBeats": end}))];
            let repeating = [look.transport, look.project].iter().any(|row| row.pointer("/loop/on") == Some(&Value::Bool(true)));
            if !repeating {
                steps.push(step("loop_repeat", json!({"on": true})));
            }
            Phrased::Steps(steps)
        }
        Intent::LoopOff => Phrased::Steps(vec![step("loop_repeat", json!({"on": false}))]),
        Intent::Metronome { on } => Phrased::Steps(vec![step("metronome_switch", json!({"on": on}))]),
    }
}

/// What the producer looks at in REAPER: the arrange view always, with the MIDI editor, an FX chain or a track's
/// routing over it when focused; the selected track, item (by its take, whose notes the MIDI editor shows) and FX.
fn fill_focus(focus: &Value) -> DawFocus {
    let selection = focus.get("selection").cloned().unwrap_or(Value::Null);
    let editor = match focus.get("view").and_then(Value::as_str) {
        Some("midi") => Some(view(ViewKind::NoteEditor, "MIDI editor")),
        Some("fx") => Some(view(ViewKind::DeviceChain, "FX chain")),
        Some("routing") => Some(view(ViewKind::Routing, "Routing")),
        Some("mixer") => Some(view(ViewKind::Mixer, "Mixer")),
        _ => None,
    };
    let item = selected(&selection, "takeRef").or_else(|| selected(&selection, "itemRef")).map(|reference| FocusItem {
        kind: if selected(&selection, "takeRef").is_some() { ItemKind::Take } else { ItemKind::Item },
        item_ref: Some(reference),
        name: selection.get("itemName").and_then(Value::as_str).map(str::to_owned),
    });
    let fx = selection.get("fxName").and_then(Value::as_str).filter(|name| !name.is_empty()).map(str::to_owned);
    let parameter = selection.get("param").and_then(|param| {
        Some(FocusParameter {
            name: param.get("name")?.as_str()?.to_owned(),
            value: param.get("value").map(|value| value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string())),
            owner: fx.clone(),
        })
    });
    // The routing window is one track's, not always the selected one's: FOCUS shows that track's routing (without the
    // selected track's name over it).
    let selected_track = selected(&selection, "trackRef");
    let routing = selected(&selection, "routingTrackRef").filter(|_| focus.get("view").and_then(Value::as_str) == Some("routing"));
    let track = if routing.is_none() || routing == selected_track { focus_track(&selection) } else { None };
    DawFocus {
        track,
        track_ref: routing.or(selected_track),
        device_ref: selected(&selection, "fxRef"),
        device: fx,
        parameter,
        main_view: Some(view(ViewKind::Timeline, "Arrange")),
        editor_view: editor,
        item,
        // (a take's FX chain: FOCUS's tree is the take's, not the track's)
        chain_ref: selected(&selection, "fxChainRef"),
        ..Default::default()
    }
}

/// FOCUS draws a screenful of lanes, so it asks for that many tracks, not a page of every track in the project.
const ARRANGE_LANES: u32 = 24;

fn body_request(request: &BodyRequest) -> Option<JsonObject> {
    Some(match request {
        BodyRequest::Tree { track_ref } => body_args("fx", Some(track_ref)),
        BodyRequest::Notes { item_ref } => body_args("midi", Some(item_ref)),
        BodyRequest::Timeline => {
            let mut args = body_args("arrange", None);
            args.insert("limit".into(), ARRANGE_LANES.into());
            args
        }
        BodyRequest::Routing { track_ref } => body_args("routing", Some(track_ref)),
        BodyRequest::Mixer { track_ref } => {
            let mut args = body_args("mixer", None);
            if let Some(track_ref) = track_ref {
                args.insert("around".into(), track_ref.as_str().into());
            }
            args.insert("limit".into(), ARRANGE_LANES.into());
            args
        }
        _ => return None,
    })
}

fn to_body(request: &BodyRequest, body: &Value) -> Option<FocusBody> {
    Some(match request {
        BodyRequest::Tree { track_ref } => FocusBody::Tree(own_views::tree(track_ref, body)),
        BodyRequest::Notes { item_ref } => FocusBody::Notes(own_views::notes(item_ref, body)),
        BodyRequest::Timeline => FocusBody::Timeline(own_views::timeline(body)),
        BodyRequest::Routing { track_ref } => FocusBody::Routing(own_views::routing(track_ref, body)),
        BodyRequest::Mixer { .. } => FocusBody::Mixer(own_views::mixer(body)),
        _ => return None,
    })
}

/// REAPER's refs: `r:<kind>:<GUID or number>`. They name the same object across reloads, so a restructure moves none.
#[derive(Default)]
pub struct ReaperRefs;

impl RefScheme for ReaperRefs {
    fn positional(&self) -> bool {
        false
    }
    fn kind_of<'a>(&self, reference: &'a str) -> Option<&'a str> {
        let mut parts = reference.splitn(3, ':');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("r"), Some(kind), Some(rest)) if !kind.is_empty() && !rest.is_empty() => Some(kind),
            _ => None,
        }
    }
    fn moved(&self, _shift: &Shift, _reference: &str) -> Moved {
        Moved::Same
    }
    fn place_of(&self, reference: &str) -> String {
        reference.into()
    }
    fn refs_in<'a>(&self, text: &'a str) -> Vec<&'a str> {
        static REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"r:[a-z]+:[0-9A-Za-z{}:.-]+").unwrap());
        REF.find_iter(text).map(|found| found.as_str()).collect()
    }
    fn reference_fields(&self) -> &[String] {
        &[]
    }
    fn parents(&self, kind: &str) -> Option<Vec<String>> {
        let parents: &[&str] = match kind {
            "item" | "send" => &["track"],
            "fx" => &["track", "take"],
            "param" => &["fx"],
            "note" | "event" => &["take"],
            "envelope" => &["track", "fx", "take", "item"],
            // m4-r-fxenv
            "point" => &["env", "aitem"],
            "autoitem" => &["env"],
            _ => return None,
        };
        Some(parents.iter().map(|parent| parent.to_string()).collect())
    }
    fn parent_hint(&self, _kind: &str, _parent: &str) -> &'static str {
        ""
    }
    fn row_refs(&self, _kind: &str, _row: &JsonObject) -> Vec<(String, String, bool)> {
        vec![]
    }
    fn track_of(&self, reference: &str) -> Option<String> {
        (self.kind_of(reference) == Some("track")).then(|| reference.to_owned())
    }
}

/// REAPER's tools as the app shows them.
pub const PRESENTATION: ToolPresentation = ToolPresentation {
    label: |tool| match tool {
        "reaper_read" => Some("looked at your project"),
        _ => None,
    },
    doing: |tool| match tool {
        "reaper_read" => Some("looking at your project"),
        _ => None,
    },
    activity: |tool| (tool == "reaper_read").then_some(ToolActivity::Read),
    plan: |tool| tool == "make_changes",
    acts: |_| false,
    read_prefix: Some("reaper_"),
};

/// How the app reads REAPER's focus and FX: the focus comes neutral, and a take in the MIDI editor shows its notes.
pub const VIEW: ViewPresentation = ViewPresentation {
    heading: |kind| match kind {
        BodyKind::Tree => "FX",
        BodyKind::Notes => "MIDI",
        BodyKind::Timeline => "Arrange",
        BodyKind::Routing => "Routing",
        BodyKind::Mixer => "Mixer",
        other => (ViewPresentation::NONE.heading)(other),
    },
    timeline_notes: true,
    open_containers: true,
    ..ViewPresentation::NONE
};

/// REAPER's integration.
pub fn create_reaper_integration(options: OwnOptions) -> Rc<dyn Integration> {
    create_own_integration::<ReaperRefs>(&DAW, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_read_text_says_what_item_rows_and_send_modes_carry() {
        // Item rows carry positionBeats and lengthBeats, and a send can tap pre-receive (review U15).
        let read = DAW.read_description;
        assert!(read.contains("8 pre-receive"), "{read}");
        let items = &read[read.find("Items' position").unwrap()..];
        assert!(items[..items.find(';').unwrap()].contains("positionBeats and lengthBeats"), "{items}");
    }

    #[test]
    fn a_reaper_ref_is_its_kind_and_its_guid() {
        let refs = ReaperRefs;
        assert_eq!(refs.kind_of("r:track:{0A1B2C3D-0000-4000-8000-000000000001}"), Some("track"));
        assert_eq!(refs.kind_of("r:track:master"), Some("track"));
        assert_eq!(refs.kind_of("r:param:{AB}:3"), Some("param"));
        assert_eq!(refs.kind_of("track:3"), None);
        assert_eq!(refs.kind_of("r:track:"), None);
        assert!(!refs.positional());
        assert_eq!(refs.refs_in("set r:fx:{AB-CD} and r:track:master"), ["r:fx:{AB-CD}", "r:track:master"]);
        // A send is named by its two tracks (and which repeat of the pair), a marker by its GUID: never by a number.
        assert_eq!(refs.kind_of("r:send:{A1}:-1:{B2}:2"), Some("send"));
        assert_eq!(
            refs.refs_in("r:send:{A1}:0:{B2}, r:send:{A1}:1:hw3 and r:marker:{C3}"),
            ["r:send:{A1}:0:{B2}", "r:send:{A1}:1:hw3", "r:marker:{C3}"]
        );
    }

    /// m6-places: each phrase's steps in a project as the look found it.
    #[test]
    fn a_phrase_is_reapers_own_change_and_a_tempo_map_leaves_tempo_and_loops_to_the_model() {
        use crate::core::phrases::{PhraseTrack, TrackPick};
        let tracks = [
            PhraseTrack { reference: "track:1", name: "MASTER", own: false, master: true },
            PhraseTrack { reference: "track:2", name: "Drums", own: true, master: false },
            PhraseTrack { reference: "track:3", name: "Vox", own: true, master: false },
        ];
        let plan = |intent: Intent, project: Value, transport: Value| {
            phrase(&intent, &PhraseLook { tracks: &tracks, project: &project, transport: &transport, slots: None })
        };
        let steps = |intent: Intent, project: Value| match plan(intent, project, json!({})) {
            Phrased::Steps(steps) => Value::Array(steps),
            other => panic!("{other:?}"),
        };
        let pick = |track: TrackPick, switch: Switch| Intent::Track { track, switch, on: true };
        assert_eq!(steps(Intent::Tempo { bpm: 124.0 }, json!({})), json!([{"tool": "tempo_set", "input": {"bpm": 124.0}}]));
        assert_eq!(
            steps(pick(TrackPick::Name("vox".into()), Switch::Arm), json!({})),
            json!([{"tool": "track_arm", "input": {"ref": "track:3", "arm": true}}])
        );
        assert_eq!(
            steps(pick(TrackPick::Position(1), Switch::Solo), json!({})),
            json!([{"tool": "track_solo", "input": {"ref": "track:2", "solo": true}}]),
            "the master doesn't count by place"
        );
        assert_eq!(plan(pick(TrackPick::Name("master".into()), Switch::Mute), json!({}), json!({})), Phrased::Model);
        // Bars 9 to 16 in 6/8: three quarter notes a bar. Repeat already on: the range alone.
        assert_eq!(
            steps(Intent::Loop { from_bar: 9, to_bar: 16 }, json!({"timeSignature": [6, 8], "loop": {"on": true}})),
            json!([{"tool": "loop_range", "input": {"startBeats": 24.0, "endBeats": 48.0}}])
        );
        assert_eq!(
            steps(Intent::Loop { from_bar: 1, to_bar: 4 }, json!({"timeSignature": [4, 4]})),
            json!([{"tool": "loop_range", "input": {"startBeats": 0.0, "endBeats": 16.0}}, {"tool": "loop_repeat", "input": {"on": true}}])
        );
        assert_eq!(steps(Intent::LoopOff, json!({})), json!([{"tool": "loop_repeat", "input": {"on": false}}]));
        assert_eq!(steps(Intent::Metronome { on: false }, json!({})), json!([{"tool": "metronome_switch", "input": {"on": false}}]));
        // A tempo map: which tempo, and how long a bar is, depend on where.
        let mapped = json!({"tempoMarkers": 2, "timeSignature": [4, 4]});
        assert_eq!(plan(Intent::Tempo { bpm: 124.0 }, mapped.clone(), json!({})), Phrased::Model);
        assert_eq!(plan(Intent::Loop { from_bar: 1, to_bar: 2 }, mapped, json!({})), Phrased::Model);
        assert!(matches!(plan(Intent::Tempo { bpm: 990.0 }, json!({}), json!({})), Phrased::Refused(why) if why.contains("960")));
    }

    #[test]
    fn the_routing_window_shows_its_own_tracks_routing() {
        let selection = json!({"trackRef": "r:track:{A}", "track": {"name": "Lead"}, "routingTrackRef": "r:track:{B}"});
        let other = fill_focus(&json!({"view": "routing", "selection": selection}));
        assert_eq!(other.track_ref.as_deref(), Some("r:track:{B}"), "the window's track, not the selected one");
        assert!(other.track.is_none(), "the selected track's name isn't put over another track's routing");
        let same = fill_focus(
            &json!({"view": "routing", "selection": {"trackRef": "r:track:{A}", "track": {"name": "Lead"}, "routingTrackRef": "r:track:{A}"}}),
        );
        assert_eq!((same.track_ref.as_deref(), same.track.map(|t| t.name)), (Some("r:track:{A}"), Some("Lead".into())));
        let arrange = fill_focus(&json!({"view": "arrange", "selection": selection}));
        assert_eq!(arrange.track_ref.as_deref(), Some("r:track:{A}"), "outside the routing view, the selected track");
    }
}
