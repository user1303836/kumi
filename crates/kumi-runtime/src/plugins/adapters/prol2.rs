use std::sync::LazyLock;

use crate::plugins::adapter::{
    pattern, OsPaths, PluginAdapter, PluginFolders, PluginKind, PluginParameterHint, PluginRecipe, PluginSection,
};

// Names as FabFilter labels its controls ("Gain", "Output Level", "Style", "Lookahead", "Channel Link Transients");
// fairly sure of the main ones, less of the switches'.
pub static PRO_L2: LazyLock<PluginAdapter> = LazyLock::new(|| {
    PluginAdapter {
    id: "prol2", name: "Pro-L 2", vendor: "FabFilter", kind: PluginKind::Effect,
    matcher: pattern(r"(?i)\bPro-?L ?2\b"),
    overview: "FabFilter's limiter. Gain drives the input into it, Output Level is the ceiling, Style picks the algorithm, Lookahead, Attack and Release time it, and Channel Link sets how much left and right limit together. True Peak Limiting and oversampling stop overs between samples. Loudness meters in its window; Unity Gain plays it back at the input level, so you judge the sound, not the volume.",
    sections: vec![
        PluginSection { name: "Limiter", about: "What shapes the sound.",
            parameters: vec![
                PluginParameterHint { role: "gain", names: pattern(r"(?i)^(Input )?Gain$"), about: "dB into the limiter: this sets the loudness." },
                PluginParameterHint { role: "output level", names: pattern(r"(?i)^Output( Level)?$"), about: "The ceiling, dB: -1.0 for streaming, with True Peak on." },
                PluginParameterHint { role: "style", names: pattern(r"(?i)^(Style|Algorithm|Mode)$"), about: "Transparent (clean at light limiting), Punchy and Dynamic (keep transients), Allround and Modern (most masters), Aggressive (loudest, can crunch), Bus (gentle), Safe (never distorts, can pump)." },
                PluginParameterHint { role: "lookahead", names: pattern(r"(?i)^Look ?ahead$"), about: "ms, 0–5. More is smoother and adds latency; little or none keeps punch but can crackle." },
                PluginParameterHint { role: "attack and release", names: pattern(r"(?i)^(Attack|Release)$"), about: "How fast it reacts and recovers. Short release is louder and grainier; long is cleaner but pumps." },
                PluginParameterHint { role: "channel link", names: pattern(r"(?i)^Channel Link"), about: "Transients and Release, %: 100% limits both sides alike (a steady image); less lets each limit alone (louder, wider, can wander)." },
            ] },
        PluginSection { name: "Output", about: "Peaks, quality, level matching, export.",
            parameters: vec![
                PluginParameterHint { role: "true peak", names: pattern(r"(?i)^True Peak"), about: "On: the ceiling holds between samples too." },
                PluginParameterHint { role: "oversampling", names: pattern(r"(?i)^Oversampling$"), about: "Off, or 2x up to 32x: cleaner, truer peaks for more CPU; 4x suits a master." },
                PluginParameterHint { role: "unity gain", names: pattern(r"(?i)^Unity Gain$"), about: "On: plays back at the input level, so you hear only what the limiting does. Off before export." },
                PluginParameterHint { role: "dither", names: pattern(r"(?i)^(Dither(ing)?( Bits)?|Noise Shaping)$"), about: "Only for a final 16- or 24-bit export, never mid-chain." },
                PluginParameterHint { role: "dc filter", names: pattern(r"(?i)^DC (Offset )?Filter$"), about: "Removes DC offset before limiting." },
            ] },
    ],
    recipes: vec![
        PluginRecipe { name: "Streaming master", how: "Style Modern (or Allround), Output Level -1.0 dB, True Peak on, Oversampling 4x, Lookahead 1–2 ms. Raise Gain until the loudest bars limit 2–4 dB. Aim for -14 to -9 LUFS integrated by genre; check with Unity Gain." },
        PluginRecipe { name: "Loud club or bass master", how: "Style Aggressive or Modern, Gain for 4–6 dB of limiting, Lookahead 0.5–1 ms, short Release, Channel Link Transients 50–75%, Oversampling 8x. If it crackles, take a dB of Gain off, or clip before it (Saturn, a clipper) so Pro-L does less." },
        PluginRecipe { name: "Bus peaks", how: "Style Bus or Punchy, Gain 1–3 dB, Lookahead 0–0.5 ms, Output Level -0.3 dB: only the peaks." },
        PluginRecipe { name: "Safety limiter", how: "Style Safe or Transparent, Gain 0 dB, Output Level -1.0 dB, True Peak on: only overs get touched." },
    ],
    beyond: "Everything that shapes the sound is a parameter. In its window only: the loudness meters and their targets, display options, and presets (.ffp files, its preset menu).",
    folders: Some(PluginFolders { presets: Some(OsPaths { mac: Some("~/Documents/FabFilter/Presets/Pro-L 2"), windows: Some("~/Documents/FabFilter/Presets/Pro-L 2") }), wavetables: None }),
    wavetable: None,
}
});
