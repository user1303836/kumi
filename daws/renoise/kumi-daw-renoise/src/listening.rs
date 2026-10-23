//! What Kumi knows of Renoise's listening, for the listening tools Kumi's own bridges share.
use kumi_daw_own::listening::Table;

/// Renoise's parameters that are places, as the tool keeps them ([lab] M3 checks): a meta device's and #Sidechain's "Dest.
/// Track/Effect/Parameter" and *Hydra's and *XY Pad's "OutN Track/Effect/Parameter", a #Send's or #Multiband Send's
/// Receiver.
fn renoise_aims(device: &str, parameter: &str) -> bool {
    if device.starts_with("#Send") || device.starts_with("#Multiband Send") {
        return parameter.starts_with("Receiver");
    }
    (device.starts_with('*') || device.starts_with("#Sidechain"))
        && ["Track", "Effect", "Parameter"].iter().any(|end| parameter.ends_with(end))
}

/// Renoise (the tool): one solo render per track, through its fader and the Master track's devices; the Maximizer and
/// the Gainer for the gain stage; EQ 10 (or EQ 5) for a fit ([lab] m5-renoise).
pub const LISTENING: Table = Table {
    post_fader: true,
    no_before_chain: Some("Renoise renders the mix after its Master track's devices"),
    master_fader: false,
    master_ref: None,
    analyzers: &["Analyzer", "Scope", "Spectrum"],
    gains: &[("Gainer", "Gain")],
    linear: &["Gainer", "EQ", "Filter", "DC Offset", "Stereo Expander", "Mixer", "TrackVolPan", "Track Vol/Pan"],
    limiters: &[("Maximizer", "Boost", false)],
    gain_device: ("Audio/Effects/Native/Gainer", "Gainer"),
    safety_limiter: ("Audio/Effects/Native/Maximizer", "Maximizer"),
    device_kind: "device",
    param_tool: "device_param",
    params_tool: "device_params",
    add_tool: "device_add",
    delete_tool: "device_delete",
    add_field: "path",
    raw_field: "value",
    text_field: "valueString",
    normalized: false,
    no_items: Some("Renoise's tracks hold no audio of their own: hear a sample's audio with listen (its ref)"),
    scratch_field: "value",
    eq: "EQ 10",
    eq_shelves_at_ends: true,
    captures: false,
    plugin_prefixes: &[],
    stop_notes: &["Renoise stopped playback to render"],
    pseudo_params: &[],
    aims: renoise_aims,
};
