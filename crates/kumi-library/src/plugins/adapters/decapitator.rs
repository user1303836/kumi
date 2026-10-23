use std::sync::LazyLock;

use crate::plugins::adapter::{pattern, PluginAdapter, PluginKind, PluginParameterHint, PluginRecipe, PluginSection};

// Names as on its face ("Drive", "Style", "Low Cut", "High Cut", "Thump", "Steep", "Tone", "Punish", "Mix", "Output",
// "Auto"); the patterns allow joined words ("LowCut").
pub static DECAPITATOR: LazyLock<PluginAdapter> = LazyLock::new(|| {
    PluginAdapter {
    id: "decapitator", name: "Decapitator", vendor: "Soundtoys", kind: PluginKind::Effect,
    matcher: pattern(r"(?i)\bDecapitator\b"),
    overview: "Soundtoys' analog saturator. Five Styles model hardware: A (Ampex 350 tape preamp), E (EMI/Chandler TG channel), N (Neve 1057 input), T and P (Thermionic Culture Vulture, triode and pentode). Drive pushes the circuit; Low Cut and High Cut shape it (Thump adds a bump at the low cut, Steep steepens the high cut); Tone tilts dark to bright; Punish adds 20 dB of drive; Mix blends in the dry; Output sets the level, and Auto ties it to Drive. Small: every control is a parameter.",
    sections: vec![
        PluginSection { name: "Controls", about: "As on its face.",
            parameters: vec![
                PluginParameterHint { role: "drive", names: pattern(r"(?i)^Drive$"), about: "0–10: how hard the circuit is pushed." },
                PluginParameterHint { role: "style", names: pattern(r"(?i)^Style$"), about: "A full and smooth, E bright and crisp, N thick in the mids, T and P aggressive (P the most)." },
                PluginParameterHint { role: "filters", names: pattern(r"(?i)^(Low|High) ?Cut$"), about: "Hz: what's cut from the lows and the top." },
                PluginParameterHint { role: "filter switches", names: pattern(r"(?i)^(Thump|Steep)$"), about: "Thump: a resonant bump at the low cut. Steep: a steeper high cut." },
                PluginParameterHint { role: "tone", names: pattern(r"(?i)^Tone$"), about: "Dark to bright; the center is neutral." },
                PluginParameterHint { role: "punish", names: pattern(r"(?i)^Punish$"), about: "On: 20 dB more drive, for destruction." },
                PluginParameterHint { role: "mix", names: pattern(r"(?i)^Mix$"), about: "Dry/wet, 0–100%: parallel saturation in place." },
                PluginParameterHint { role: "output", names: pattern(r"(?i)^(Output|Auto)$"), about: "Output in dB; Auto lowers it as Drive rises, for fair comparisons." },
            ] },
    ],
    recipes: vec![
        PluginRecipe { name: "Drum bus smash", how: "Style N or E, Drive 6–8, Mix 25–35%, High Cut 10 kHz; Punish on for more. Auto on to judge it fairly." },
        PluginRecipe { name: "Bass growl, clean sub", how: "Style T or P, Drive 4–6, Low Cut 80–120 Hz with Thump, Tone a little bright, Mix 40–60%: the dry keeps the sub, the wet growls." },
        PluginRecipe { name: "Vocal grit", how: "Style A, Drive 3–5, High Cut 8–10 kHz with Steep, Mix 30–40%, Auto on." },
        PluginRecipe { name: "Synth warmth", how: "Style A or N, Drive 2–3, Mix 100%, Tone a step toward dark." },
        PluginRecipe { name: "Telephone", how: "Style E, Drive 7, Low Cut 400 Hz, High Cut 3 kHz with Steep, Punish on, Mix 100%." },
    ],
    beyond: "Every control is a parameter; presets are in its window's preset menu.",
    folders: None,
    wavetable: None,
}
});
