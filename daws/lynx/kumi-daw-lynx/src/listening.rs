//! How Kumi would hear LYNX SHARE: not yet. Its bridge renders nothing, so its status has no `listening` and the shared
//! listening offers no tools; this table only completes the DAW's description.
use kumi_daw_own::listening::Table;

const NO_RENDER: &str = "LYNX SHARE's bridge can't render yet";

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
    device_kind: "insert",
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
