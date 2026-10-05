use std::sync::LazyLock;

use crate::plugins::adapter::{
    pattern, OsPaths, PluginAdapter, PluginFolders, PluginKind, PluginParameterHint, PluginRecipe, PluginSection,
};

// Names in FabFilter's per-band form ("Band 1 Drive", "Band 1 Style"); medium confidence, the global ones loosest.
pub static SATURN2: LazyLock<PluginAdapter> = LazyLock::new(|| {
    PluginAdapter {
    id: "saturn2", name: "Saturn 2", vendor: "FabFilter", kind: PluginKind::Effect,
    matcher: pattern(r"(?i)\bSaturn ?2\b"),
    overview: "FabFilter's multiband saturation: up to six bands split by crossovers, each with its own Style (28: tube, tape, transformer, amp and effect types), Drive, Feedback, Dynamics, Tone, Mix and Level; then a global Mix and the output. Modulation sources (XLFOs, envelope generators and followers, MIDI, XY pads, sliders) can move any knob. Parameters come per band (Band 1, Band 2…), so Live turns only the configured ones.",
    sections: vec![
        PluginSection { name: "Bands", about: "Band N's controls. A band exists only once it's made in the window.",
            parameters: vec![
                PluginParameterHint { role: "style", names: pattern(r"(?i)^Band ?[0-9]+ Style$"), about: "The distortion type: Clean and Warm Tube, tapes, transformers, amps, and effects like Rectify and Smudge." },
                PluginParameterHint { role: "drive", names: pattern(r"(?i)^Band ?[0-9]+ Drive$"), about: "How hard the band hits its style." },
                PluginParameterHint { role: "dynamics", names: pattern(r"(?i)^Band ?[0-9]+ Dynamics$"), about: "Off center, it compresses or expands around the distortion (expanding brings back punch). Try both sides by ear." },
                PluginParameterHint { role: "tone", names: pattern(r"(?i)^Band ?[0-9]+ Tone$"), about: "Darker or brighter distortion." },
                PluginParameterHint { role: "feedback", names: pattern(r"(?i)^Band ?[0-9]+ Feedback( Freq(uency)?)?$"), about: "Resonant feedback, from warmth to howl, ringing at its frequency (Hz)." },
                PluginParameterHint { role: "mix and level", names: pattern(r"(?i)^Band ?[0-9]+ (Mix|Level|Gain|Pan)$"), about: "The band's dry/wet, output level (dB) and pan." },
                PluginParameterHint { role: "band switches", names: pattern(r"(?i)^Band ?[0-9]+ (Enabled|Bypass|Mute|Solo)$"), about: "Bypass, mute and solo, for listening." },
            ] },
        PluginSection { name: "Crossovers and global", about: "Split points and the whole plug-in.",
            parameters: vec![
                PluginParameterHint { role: "crossover", names: pattern(r"(?i)Crossover|^Band ?[0-9]+ (Low |High )?Freq(uency)?$"), about: "Where bands split, Hz." },
                PluginParameterHint { role: "global mix", names: pattern(r"(?i)^(Global )?Mix$"), about: "Dry/wet of everything: parallel saturation." },
                PluginParameterHint { role: "in and out", names: pattern(r"(?i)^(Input|Output) (Level|Gain|Pan)$"), about: "dB in and out; output pan." },
                PluginParameterHint { role: "quality", names: pattern(r"(?i)^(High Quality|HQ|Oversampling)"), about: "Oversampling: less aliasing on bright, driven sounds, more CPU." },
                PluginParameterHint { role: "modulation", names: pattern(r"(?i)XLFO|\bEG ?[0-9]|Env(elope)? ?Fol|Slider ?[0-9]|\bXY ?[0-9]"), about: "The modulation sources' own settings; a source moves only what it's connected to." },
            ] },
    ],
    recipes: vec![
        PluginRecipe { name: "Bass: grit up top, clean lows", how: "Two bands split at 120–200 Hz. Low band clean (no drive, or Clean Tube lightly). Upper band Warm Tube or a tape style, Drive about halfway, Tone a little bright, Mix 60–80%; level-match with its Level." },
        PluginRecipe { name: "Drum bus glue", how: "One band, a tape style, Drive a quarter of the way, Dynamics nudged to its compressing side, Mix 40–60%; Output down to match." },
        PluginRecipe { name: "Vocal warmth", how: "One band, Warm Tube, Drive low, Tone slightly dark, Mix 30–40%." },
        PluginRecipe { name: "Broken lo-fi", how: "An effect style (Rectify, Smudge), Drive high, Feedback up with its frequency at 1–3 kHz, an XLFO slowly moving Drive; global Mix 50%." },
        PluginRecipe { name: "Parallel on the mix bus", how: "Global Mix 15–25%, a tape style, Drive moderate, High Quality on: density without losing transients." },
    ],
    beyond: "In Saturn's window (Kumi can open it), or the producer's to do: adding and removing bands (click in the band area, drag crossovers), connecting modulation (drag a source's handle onto a knob), XLFO steps and envelope shapes, MIDI learn, presets (.ffp files, its preset menu). A band that hasn't been made does nothing, whatever its parameters say.",
    folders: Some(PluginFolders { presets: Some(OsPaths { mac: Some("~/Documents/FabFilter/Presets/Saturn 2"), windows: Some("~/Documents/FabFilter/Presets/Saturn 2") }), wavetables: None }),
    wavetable: None,
}
});
