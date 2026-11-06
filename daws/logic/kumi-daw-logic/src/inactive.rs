//! Required shared tables for capabilities the M2 Logic profile leaves off. These offer no tools.
use kumi_agent::core::describe::ProjectWords;
use kumi_daw_own::catchup::{CatchUpWords, RecipeKinds};
use kumi_daw_own::phrases::{OwnPhrases, Phrased};
use kumi_types::contracts::JsonObject;

/// Its profile has no delivery to a spec: these complete the DAW's description.
pub static SPEC: kumi_daw_own::spec::OwnSpecFacts = kumi_daw_own::spec::OwnSpecFacts {
    words: kumi_agent::core::spec::backend::Words { app: "Logic Pro", fader_most: 6., limiter: "", gainer: "" },
    is_master: |_| false,
    fader: "volumeDb",
};

/// Its profile keeps no scripts: these complete the DAW's description.
pub static SCRIPTS: kumi_agent::core::scripts::backend::Words =
    kumi_agent::core::scripts::backend::Words { app: "Logic Pro", language: "", action: "", places: &[] };

/// What changed in a Logic Pro session, in its words (its profile doesn't catch up yet, and with no change kinds it has
/// no watch_me).
pub const CATCH_UP: CatchUpWords = CatchUpWords {
    words: ProjectWords {
        kinds: &[("track", "tracks"), ("plugin", "plug-ins"), ("region", "regions"), ("marker", "markers")],
        project: "project",
        rank: &["project", "track", "plugin", "region", "marker"],
        opaque: &["plugin"],
    },
    facts: |_| Vec::new(),
    recipe: RecipeKinds {
        track_add: "",
        track_types: &[],
        effect: "plugin",
        effect_add: |_, _| None,
        effect_values: |_| ("", JsonObject::new()),
        mixer: &[],
    },
    watch_description: "",
};

/// No fixed phrases yet: each goes to the model.
pub const PHRASES: OwnPhrases = OwnPhrases { plan: |_, _| Phrased::Model, loops_by_slot: false };

use kumi_daw_own::listening::Table;

const NO_RENDER: &str = "Kumi does not listen or render in Logic Pro at this stage";

pub static LISTENING: Table = Table {
    post_fader: true,
    no_before_chain: Some(NO_RENDER),
    master_fader: true,
    master_ref: None,
    analyzers: &[],
    gains: &[],
    linear: &[],
    limiters: &[],
    gain_device: ("", ""),
    safety_limiter: ("", ""),
    device_kind: "plugin",
    param_tool: "",
    params_tool: "",
    add_tool: "",
    delete_tool: "",
    add_field: "plugin",
    raw_field: "value",
    text_field: "",
    normalized: false,
    no_items: Some(NO_RENDER),
    scratch_field: "value",
    eq: "",
    eq_shelves_at_ends: false,
    captures: false,
    plugin_prefixes: &[],
    stop_notes: &[],
    pseudo_params: &[],
    aims: |_, _| false,
};
