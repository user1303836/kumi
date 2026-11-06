//! SoundCraft, through Kumi's host and the bridge beside SoundCraft (`soundcraft-kumi`, which the host starts): its refs
//! (SoundCraft's own ids, under the session's epoch), its read tool and instructions, and what FOCUS shows of it. The
//! integration itself is the one every bridge of Kumi's own shares (`kumi_daw_own::bridge`). Kumi reads SoundCraft's
//! sessions and changes nothing in them yet; its provider is experimental, so only `--daw soundcraft` reaches it.
pub mod changes;
pub mod library;
mod listening;
mod provider;

use kumi_agent::core::{
    contracts::Integration,
    describe::ProjectWords,
    presentation::{ToolActivity, ToolPresentation},
    refs::{Moved, RefScheme, Shift},
    view::{BodyKind, DawFocus, FocusItem, ItemKind, ViewKind, ViewPresentation},
};
use kumi_agent::mcp::client::BridgeDescriptor;
use kumi_daw_own::bridge::{create_own_integration, OwnBridge, OwnDaw, OwnOptions, OwnTools};
use kumi_daw_own::catchup::{CatchUpWords, RecipeKinds};
use kumi_daw_own::phrases::{OwnPhrases, Phrased};
use kumi_daw_own::views::{focus_track, selected, view};
use kumi_types::contracts::JsonObject;
use kumi_types::daw::DawProfile;
use regex::Regex;
use serde_json::Value;
use std::{rc::Rc, sync::LazyLock};

pub use provider::PROVIDER;

/// The instructions SoundCraft's turns carry: reading, and what its refs are.
const INSTRUCTIONS: &str = concat!(
    "You work in SoundCraft through Kumi's bridge. Read the session with soundcraft_read before you describe it: its tracks (audio, MIDI, instrument, aux and VCA) with their clips, inserts, sends and automation lanes, its busses, markers, groups and sources, a MIDI clip's notes and an insert's parameters.",
    " A ref such as s:track:3 is SoundCraft's own id for something, in the epoch of the answer it came in: pass it back as ref or parent with that epoch. SoundCraft gives ids out again, so a ref from another epoch is refused as stale: read again."
);

/// SoundCraft's host: Kumi's own (`daw-mcp-server --daw soundcraft`), which starts the bridge beside SoundCraft.
const DESCRIPTOR: BridgeDescriptor = BridgeDescriptor {
    program: "daw-mcp-server",
    policy_variable: "KUMI_DAW_TOOL_POLICY",
    allow_variable: "KUMI_DAW_TOOL_ALLOW",
    events: "notifications/daw_event",
    daw: "SoundCraft",
};

/// What Kumi reads in SoundCraft, and how.
pub const DAW: OwnDaw = OwnDaw {
    profile: &DawProfile::SOUNDCRAFT,
    descriptor: DESCRIPTOR,
    host_flag: "soundcraft",
    call_ms: kumi_daw_own::bridge::CALL_MS,
    bridge: OwnBridge::Beside,
    instructions: INSTRUCTIONS,
    mechanical: "a muted or inactive track in the path, an insert bypassed, a send to a bus nothing returns, two equal sends to one bus",
    read_tool: "soundcraft_read",
    read_description: "Read SoundCraft's session: a kind lists its rows (clip, insert, send and env under a track as parent, note under a MIDI clip, param under an insert, point under an automation lane), and ref reads one. Pass a ref or parent with the epoch of the answer it came in. A page with nextCursor has more.",
    read_kinds: &["session", "track", "clip", "insert", "send", "bus", "marker", "group", "source", "note", "param", "env", "point"],
    project_kind: "session",
    fill_focus,
    bodies: &[],
    body_request: |_| None,
    to_body: |_, _| None,
    changes: &changes::CHANGES,
    catch_up: &CATCH_UP,
    phrases: &PHRASES,
    scripts: &SCRIPTS,
    spec: &SPEC,
    listening: &listening::LISTENING,
    outside_code: "a command run in SoundCraft",
};

/// Its profile has no delivery to a spec: these complete the DAW's description.
static SPEC: kumi_daw_own::spec::OwnSpecFacts = kumi_daw_own::spec::OwnSpecFacts {
    words: kumi_agent::core::spec::backend::Words { app: "SoundCraft", fader_most: 6., limiter: "", gainer: "" },
    is_master: |_| false,
    fader: "volumeDb",
};

/// Its profile keeps no scripts: these complete the DAW's description.
static SCRIPTS: kumi_agent::core::scripts::backend::Words =
    kumi_agent::core::scripts::backend::Words { app: "SoundCraft", language: "", action: "", places: &[] };

/// What changed in a SoundCraft session, in its words (its profile doesn't catch up yet, and with no change kinds it has
/// no watch_me).
pub const CATCH_UP: CatchUpWords = CatchUpWords {
    words: ProjectWords {
        kinds: &[("track", "tracks"), ("insert", "inserts"), ("clip", "clips"), ("marker", "markers")],
        project: "session",
        rank: &["session", "track", "insert", "clip", "marker"],
        opaque: &["insert"],
    },
    facts: |_| Vec::new(),
    recipe: RecipeKinds {
        track_add: "",
        track_types: &[],
        effect: "insert",
        effect_add: |_, _| None,
        effect_values: |_| ("", JsonObject::new()),
        mixer: &[],
    },
    watch_description: "",
};

/// No fixed phrases yet: each goes to the model.
const PHRASES: OwnPhrases = OwnPhrases { plan: |_, _| Phrased::Model, loops_by_slot: false };

/// What the producer looks at in SoundCraft: the Edit window always, with the MIDI Editor, a plug-in's window or the Mix
/// window over it when the bridge says so; the first selected track, the first selected clip, and the plug-in whose
/// window is open.
fn fill_focus(focus: &Value) -> DawFocus {
    let selection = focus.get("selection").cloned().unwrap_or(Value::Null);
    let editor = match focus.get("view").and_then(Value::as_str) {
        Some("midi") => Some(view(ViewKind::NoteEditor, "MIDI Editor")),
        Some("fx") => Some(view(ViewKind::DeviceChain, "Plug-in")),
        Some("mixer") => Some(view(ViewKind::Mixer, "Mix")),
        _ => None,
    };
    let clip = selection.get("clipRefs").and_then(Value::as_array).and_then(|clips| clips.first()).and_then(Value::as_str);
    DawFocus {
        track: focus_track(&selection),
        track_ref: selected(&selection, "trackRef"),
        device_ref: selected(&selection, "fxRef"),
        device: selected(&selection, "fxName"),
        main_view: Some(view(ViewKind::Timeline, "Edit")),
        editor_view: editor,
        item: clip.map(|reference| FocusItem { kind: ItemKind::Clip, item_ref: Some(reference.to_owned()), name: None }),
        ..Default::default()
    }
}

/// SoundCraft's refs: `s:<kind>:<id>` (a reused id's later life `<id>.<life>`), and what's in a track by the track's
/// (`s:insert:<track>:<slot>`). They hold within an epoch, so a restructure moves none.
#[derive(Default)]
pub struct SoundCraftRefs;

impl RefScheme for SoundCraftRefs {
    fn positional(&self) -> bool {
        false
    }
    fn kind_of<'a>(&self, reference: &'a str) -> Option<&'a str> {
        let mut parts = reference.splitn(3, ':');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("s"), Some(kind), Some(rest)) if !kind.is_empty() && !rest.is_empty() => Some(kind),
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
        // (a ref starts a word and ends with a part: never the colon of "s:clip:40: …" or the period after it, and a dot
        // only inside, as a later life's `3.2`)
        static REF: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\bs:[a-z]+(?::[0-9A-Za-z{}_-]+)+(?:\.[0-9A-Za-z{}_-]+(?::[0-9A-Za-z{}_-]+)*)*").unwrap());
        REF.find_iter(text).map(|found| found.as_str()).collect()
    }
    fn reference_fields(&self) -> &[String] {
        &[]
    }
    fn parents(&self, kind: &str) -> Option<Vec<String>> {
        let parent = match kind {
            "clip" | "insert" | "send" | "env" => "track",
            "note" => "clip",
            "param" => "insert",
            "point" => "env",
            _ => return None,
        };
        Some(vec![parent.to_string()])
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

/// SoundCraft's tools as the app shows them.
pub const PRESENTATION: ToolPresentation = ToolPresentation {
    label: |tool| match tool {
        "soundcraft_read" => Some("looked at your session"),
        _ => None,
    },
    doing: |tool| match tool {
        "soundcraft_read" => Some("looking at your session"),
        _ => None,
    },
    activity: |tool| (tool == "soundcraft_read").then_some(ToolActivity::Read),
    plan: |_| false,
    acts: |_| false,
    read_prefix: Some("soundcraft_"),
};

/// How the app reads SoundCraft's focus: it comes neutral, and FOCUS draws no bodies of it yet.
pub const VIEW: ViewPresentation = ViewPresentation {
    heading: |kind| match kind {
        BodyKind::Timeline => "Edit",
        BodyKind::Mixer => "Mix",
        BodyKind::Notes => "MIDI",
        other => (ViewPresentation::NONE.heading)(other),
    },
    ..ViewPresentation::NONE
};

/// SoundCraft's integration: its reads, FOCUS and catch-up of the look; no change tools while it has no change kinds.
pub fn create_soundcraft_integration(options: OwnOptions) -> Rc<dyn Integration> {
    create_own_integration::<SoundCraftRefs>(&DAW, options, |_| OwnTools::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_soundcraft_ref_is_its_kind_and_soundcrafts_id() {
        let refs = SoundCraftRefs;
        assert_eq!(refs.kind_of("s:track:3"), Some("track"));
        assert_eq!(refs.kind_of("s:track:3.2"), Some("track"), "a reused id's later life");
        assert_eq!(refs.kind_of("s:insert:3:a"), Some("insert"));
        assert_eq!(refs.kind_of("s:param:3:a:high_shelf_gain"), Some("param"));
        assert_eq!(refs.kind_of("r:track:{AB}"), None, "REAPER's");
        assert_eq!(refs.kind_of("s:track:"), None);
        assert_eq!(
            refs.refs_in("moved s:clip:25 to s:track:3.2, then s:param:3:a:high_shelf_gain."),
            ["s:clip:25", "s:track:3.2", "s:param:3:a:high_shelf_gain"]
        );
        assert_eq!(refs.refs_in("s:clip:40: isn't a MIDI clip"), ["s:clip:40"], "the bridge's refusal: no colon in the ref");
        assert_eq!(refs.refs_in("s:note:40:-5.60.0 and s:point:3:volume:-2."), ["s:note:40:-5.60.0", "s:point:3:volume:-2"]);
        assert!(refs.refs_in("Drums:kick:1").is_empty(), "inside a word, no ref");
        assert_eq!(refs.parents("note"), Some(vec!["clip".to_string()]));
        assert_eq!(refs.parents("marker"), None, "read under nothing");
        assert_eq!(refs.track_of("s:track:7"), Some("s:track:7".into()));
        assert_eq!(refs.track_of("s:insert:7:a"), None);
    }

    #[test]
    fn focus_shows_the_window_the_producer_looks_at_and_the_first_selected_track_and_clip() {
        let focus = json!({
            "view": "midi",
            "selection": {"trackRef": "s:track:3", "track": {"name": "Bass", "type": "midi"}, "clipRefs": ["s:clip:25", "s:clip:26"]},
        });
        let filled = fill_focus(&focus);
        assert_eq!(filled.editor_view.map(|view| view.kind), Some(ViewKind::NoteEditor));
        assert_eq!(filled.main_view.map(|view| view.kind), Some(ViewKind::Timeline));
        assert_eq!(filled.track_ref.as_deref(), Some("s:track:3"));
        assert_eq!(filled.track.map(|track| track.name).as_deref(), Some("Bass"));
        let item = filled.item.unwrap();
        assert_eq!((item.kind, item.item_ref.as_deref()), (ItemKind::Clip, Some("s:clip:25")));

        // A plug-in's window names its insert; the Edit window alone has nothing over it.
        let fx = fill_focus(&json!({"view": "fx", "selection": {"fxRef": "s:insert:3:a", "fxName": "Compressor"}}));
        assert_eq!(fx.editor_view.map(|view| view.kind), Some(ViewKind::DeviceChain));
        assert_eq!((fx.device_ref.as_deref(), fx.device.as_deref()), (Some("s:insert:3:a"), Some("Compressor")));
        let edit = fill_focus(&json!({"view": "edit", "selection": {"trackRefs": [], "clipRefs": []}}));
        assert!(edit.editor_view.is_none() && edit.item.is_none() && edit.track_ref.is_none());
    }

    #[test]
    fn kumi_reads_soundcraft_and_has_the_producer_make_a_fix() {
        assert!(DAW.changes.changes_nothing());
        // The provider says so before any integration is made: the session builds nothing from its start.
        assert!(kumi_daw::provider::DawProvider::reads_only(&PROVIDER));
        let instructions = kumi_daw_own::bridge::instructions_of(&DAW);
        assert!(instructions.contains("can't change them yet"), "{instructions}");
        assert!(instructions.contains("how to make that fix in SoundCraft"), "{instructions}");
        for promise in ["Kumi's undo", "each undoable", "Describe each change", "The look's setup"] {
            assert!(!instructions.contains(promise), "a DAW Kumi can't change promises {promise:?}: {instructions}");
        }
    }
}
