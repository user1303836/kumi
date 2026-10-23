use std::sync::LazyLock;

use crate::plugins::adapter::{pattern, PluginAdapter, PluginKind, PluginParameterHint, PluginRecipe, PluginSection};

// Names as on its face ("Mix", "Delay", "Warp", "Feedback", "Density", "Width", "Low Cut", "High Cut", "Mod Rate",
// "Mod Depth", "Mode"); Valhalla may join words ("LowCut", "Delay_Ms"), so the patterns allow that.
pub static SUPERMASSIVE: LazyLock<PluginAdapter> = LazyLock::new(|| {
    PluginAdapter {
    id: "supermassive", name: "Supermassive", vendor: "Valhalla DSP", kind: PluginKind::Effect,
    matcher: pattern(r"(?i)\b(Valhalla ?)?Super ?massive\b"),
    overview: "Valhalla's free delay and reverb: a feedback network of delays. Mode picks the algorithm, from smeared echoes to huge, slow-building reverbs. Delay sets the spacing, Feedback the length, Density how fast echoes blur into a wash, Warp how the network's delays relate. Low Cut and High Cut sit in the feedback, so each repeat loses lows and highs; Mod Rate and Depth chorus the tail. Small: every control is a parameter.",
    sections: vec![
        PluginSection { name: "Controls", about: "Each Mode answers Delay, Warp and Density its own way.",
            parameters: vec![
                PluginParameterHint { role: "mix", names: pattern(r"(?i)^Mix$"), about: "0–100%; 100% on a return track." },
                PluginParameterHint { role: "delay", names: pattern(r"(?i)^Delay([ _]?(Ms|Time))?$"), about: "ms: echo spacing, and in reverb modes the size and pre-delay feel." },
                PluginParameterHint { role: "delay sync", names: pattern(r"(?i)^Delay[ _]?(Note|Sync|Style)"), about: "Free or synced to the tempo, and the note value." },
                PluginParameterHint { role: "warp", names: pattern(r"(?i)^Warp$"), about: "%: reshapes the echo pattern, from distinct to smeared; differs per mode." },
                PluginParameterHint { role: "feedback", names: pattern(r"(?i)^Feedback$"), about: "%: decay length. Above 90% very long; near 100% almost endless." },
                PluginParameterHint { role: "density", names: pattern(r"(?i)^Density$"), about: "%: low keeps echoes distinct, high blurs them into reverb." },
                PluginParameterHint { role: "width", names: pattern(r"(?i)^Width$"), about: "Stereo width." },
                PluginParameterHint { role: "low and high cut", names: pattern(r"(?i)^(Low|High) ?Cut$"), about: "Hz, inside the feedback: each repeat gets thinner or darker." },
                PluginParameterHint { role: "modulation", names: pattern(r"(?i)^Mod ?(Rate|Depth)$"), about: "Rate (Hz) and depth (%) of the tail's chorus." },
                PluginParameterHint { role: "mode", names: pattern(r"(?i)^Mode$"), about: "The algorithm (Gemini, Hydra, Andromeda, Great Annihilator, Lyra, Capricorn and more); try a few by ear." },
            ] },
    ],
    recipes: vec![
        PluginRecipe { name: "Huge pad wash", how: "On a return: Great Annihilator or Andromeda, Mix 100%, Delay 150–300 ms, Feedback 80–90%, Density 60–80%, Warp about 30%, Width full, Low Cut 200 Hz, High Cut 5–7 kHz, Mod Depth 30–50%." },
        PluginRecipe { name: "Echo throws", how: "Gemini or Hydra on a return, Delay synced 1/8 dotted or 1/4, Feedback 40–60%, Density 0–20%, Warp 0–20%, Low Cut 300 Hz, High Cut 4 kHz; automate the send." },
        PluginRecipe { name: "Lead space", how: "As an insert: Mix 15–25%, Delay 80–120 ms, Feedback 50–60%, Density 40%, High Cut 6 kHz." },
        PluginRecipe { name: "Endless drone", how: "Feedback 95–100%, Density high, Mod Depth 50%, Low Cut 150 Hz so lows don't pile up; feed it a chord, then mute the source." },
    ],
    beyond: "Every control is a parameter; presets are in its window. Tails run long: when comparing, listen past the dry sound.",
    folders: None,
    wavetable: None,
}
});
