//! What Kumi knows of REAPER's listening, for the listening tools Kumi's own bridges share.
use kumi_daw_own::listening::Table;

/// REAPER (the extension): pre-fader stems in one pass, the master with and without its FX, ReaLimit and JS Volume
/// Adjustment for the gain stage, ReaEQ for a fit. Device names are what the extension's rows say ([lab] m5-reaper).
pub const LISTENING: Table = Table {
    post_fader: false,
    no_before_chain: None,
    master_fader: true,
    master_ref: Some("r:track:master"),
    analyzers: &["Analyzer", "Spectrum", "Spectrograph", "Meter", "Loudness", "Tuner", "Oscilloscope"],
    gains: &[("Volume Adjustment", "Adjustment (dB)"), ("Volume/Pan", "Volume (dB)")],
    linear: &[],
    limiters: &[("ReaLimit", "Threshold", true), ("Master Limiter", "Threshold", true)],
    gain_device: ("JS: Volume Adjustment", "Volume Adjustment"),
    safety_limiter: ("VST: ReaLimit (Cockos)", "ReaLimit"),
    device_kind: "fx",
    param_tool: "fx_param",
    params_tool: "fx_params",
    add_tool: "fx_add",
    delete_tool: "fx_delete",
    add_field: "name",
    raw_field: "normalized",
    text_field: "value",
    normalized: true,
    no_items: None,
    scratch_field: "normalized",
    eq: "ReaEQ",
    eq_shelves_at_ends: false,
    captures: true,
    plugin_prefixes: &["VST:", "VSTi:", "VST3:", "VST3i:", "AU:", "AUi:", "CLAP:", "CLAPi:", "LV2:", "LV2i:", "DX:", "DXi:"],
    stop_notes: &["The audition playing was stopped", "REAPER stopped playback to render"],
    pseudo_params: &["Bypass", "Wet", "Delta"],
    aims: |_, _| false,
};
