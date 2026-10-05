use std::sync::LazyLock;

use crate::plugins::adapter::{
    pattern, OsPaths, PluginAdapter, PluginFolders, PluginKind, PluginParameterHint, PluginRecipe, PluginSection,
};

// Names: Pro-Q 3's host names ("Band 1 Used", "Band 1 Frequency", "Band 1 Dynamic Range"), which Pro-Q 4 is taken
// to keep; its spectral and character controls are matched loosely.
pub static PRO_Q4: LazyLock<PluginAdapter> = LazyLock::new(|| {
    PluginAdapter {
    id: "proq4", name: "Pro-Q 4", vendor: "FabFilter", kind: PluginKind::Effect,
    matcher: pattern(r"(?i)\bPro-?Q ?4\b"),
    overview: "FabFilter's EQ: up to 24 bands, each a group of parameters (Band 1 … Band 24). A band does nothing until it's Used. Each has a shape, frequency, gain, Q, slope and stereo placement, and can be dynamic (its gain moves with its range's level, or a side chain's) or spectral (new in 4: it acts on single resonances inside its range). Globally: processing mode, Character, output. Hundreds of parameters, so Live turns only the configured ones: configure the bands you use.",
    sections: vec![
        PluginSection { name: "Bands", about: "Band N's controls. Turn Used on first.",
            parameters: vec![
                PluginParameterHint { role: "used", names: pattern(r"(?i)^Band ?[0-9]+ Used$"), about: "On creates the band, off removes it. Nothing else on the band works while it's off." },
                PluginParameterHint { role: "enabled", names: pattern(r"(?i)^Band ?[0-9]+ Enabled$"), about: "Bypasses the band, keeping its settings." },
                PluginParameterHint { role: "frequency", names: pattern(r"(?i)^Band ?[0-9]+ Freq(uency)?$"), about: "Hz, 10 Hz–30 kHz." },
                PluginParameterHint { role: "gain", names: pattern(r"(?i)^Band ?[0-9]+ Gain$"), about: "dB, ±30 (ignored by cuts, notches and band-pass)." },
                PluginParameterHint { role: "q", names: pattern(r"(?i)^Band ?[0-9]+ Q$"), about: "0.025–40, higher is narrower; on cuts and shelves, the corner's resonance." },
                PluginParameterHint { role: "shape", names: pattern(r"(?i)^Band ?[0-9]+ Shape$"), about: "Bell, Low Shelf, Low Cut, High Shelf, High Cut, Notch, Band Pass, Tilt Shelf, Flat Tilt (and Pro-Q 4's newer ones)." },
                PluginParameterHint { role: "slope", names: pattern(r"(?i)^Band ?[0-9]+ Slope$"), about: "dB/oct for cuts and shelves: 6 to 96, or Brickwall." },
                PluginParameterHint { role: "stereo placement", names: pattern(r"(?i)^Band ?[0-9]+ (Stereo )?Placement$"), about: "Stereo, Left, Right, Mid or Side." },
            ] },
        PluginSection { name: "Dynamics and spectral", about: "Per band. A dynamic band's gain moves from Gain toward Gain + Range as its level goes past Threshold.",
            parameters: vec![
                PluginParameterHint { role: "dynamics on", names: pattern(r"(?i)^Band ?[0-9]+ Dynamics? (Enabled|On)$"), about: "Makes the band dynamic." },
                PluginParameterHint { role: "range", names: pattern(r"(?i)^Band ?[0-9]+ (Dynamic )?Range$"), about: "dB. Negative cuts when loud, positive boosts when loud. Gain 0 with Range -6: up to 6 dB off, only when it's loud." },
                PluginParameterHint { role: "threshold", names: pattern(r"(?i)^Band ?[0-9]+ (Threshold|Dynamics? Auto|Auto Threshold)$"), about: "dB; Auto sets it from the music." },
                PluginParameterHint { role: "timing", names: pattern(r"(?i)^Band ?[0-9]+ (Attack|Release)$"), about: "How fast the gain follows." },
                PluginParameterHint { role: "side chain", names: pattern(r"(?i)^Band ?[0-9]+ Side ?Chain"), about: "The band follows the side-chain input instead of itself: duck the bass's lows when the kick hits." },
                PluginParameterHint { role: "spectral", names: pattern(r"(?i)^Band ?[0-9]+ Spectral"), about: "Pro-Q 4's spectral dynamics: acts per frequency inside the band, taming single resonances." },
            ] },
        PluginSection { name: "Global", about: "The whole EQ.",
            parameters: vec![
                PluginParameterHint { role: "processing mode", names: pattern(r"(?i)^Processing (Mode|Resolution)$"), about: "Zero Latency (default), Natural Phase (analog-like phase, small latency), Linear Phase (no phase shift; latency and pre-ringing, their length set by Resolution)." },
                PluginParameterHint { role: "character", names: pattern(r"(?i)^Character( Mode)?$"), about: "Clean, or analog-style saturation (Subtle, Warm)." },
                PluginParameterHint { role: "output", names: pattern(r"(?i)^Output (Level|Gain|Pan)$"), about: "Output level (dB) and pan." },
                PluginParameterHint { role: "gain scale and auto gain", names: pattern(r"(?i)^(Gain ?Scale|Auto ?Gain)$"), about: "Gain Scale scales every band's gain (100% is as set). Auto Gain keeps loudness even." },
            ] },
    ],
    recipes: vec![
        PluginRecipe { name: "Clean lows", how: "Used on, Shape Low Cut, Slope 24 or 48 dB/oct: 25–35 Hz on kick and bass, 100–200 Hz on pads, leads and vocals that don't need lows." },
        PluginRecipe { name: "Harshness only when it's there", how: "Bell at 2.5–4 kHz, Q 2–3, Gain 0, dynamics on, Range -4 to -6 dB, Auto threshold. For many narrow resonances, spectral instead." },
        PluginRecipe { name: "Kick and bass", how: "On the bass: Bell or Low Shelf at the kick's fundamental (50–80 Hz), dynamic, following the side chain (the kick), Range -4 to -8 dB, fast attack and release." },
        PluginRecipe { name: "Vocal air and de-ess", how: "High Shelf at 10–12 kHz, +2 dB, Q 0.7. A dynamic Bell at 6–8 kHz, Range -3 to -6 dB." },
        PluginRecipe { name: "Mid/side on a master", how: "Low Cut on Side at 100–150 Hz (mono lows), Bell on Mid at 250–350 Hz -1 dB, High Shelf on Side at 8–10 kHz +1 to +1.5 dB. Natural or Linear Phase." },
    ],
    beyond: "Not parameters: EQ Match (match a reference's spectrum), EQ Sketch, Spectrum Grab, the analyzer, the instance list (the Set's other Pro-Q 4s) and soloing a band to listen: in Pro-Q's window (Kumi can open it), or the producer's to do. The side-chain source is chosen in Live's plug-in device (its Sidechain section). Presets are .ffp files, saved from Pro-Q's preset menu.",
    folders: Some(PluginFolders { presets: Some(OsPaths { mac: Some("~/Documents/FabFilter/Presets/Pro-Q 4"), windows: Some("~/Documents/FabFilter/Presets/Pro-Q 4") }), wavetables: None }),
    wavetable: None,
}
});
