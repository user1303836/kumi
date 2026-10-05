use std::collections::HashSet;

use kumi_runtime::plugins::adapter::{matches, Pattern, PluginAdapter, PluginKind};
use kumi_runtime::plugins::adapters::ADAPTERS;
use kumi_runtime::plugins::registry::adapter_for;

/// The names Live shows for each plug-in's device.
const LIVE_NAMES: &[(&str, &[&str])] = &[
    ("serum2", &["Serum 2", "Serum2"]),
    ("vital", &["Vital"]),
    ("ozone12", &["Ozone 12", "Ozone 12 Advanced", "iZotope Ozone 12", "Ozone 12 Maximizer"]),
    ("proq4", &["FabFilter Pro-Q 4", "Pro-Q 4"]),
    ("prol2", &["FabFilter Pro-L 2", "Pro-L 2"]),
    ("saturn2", &["FabFilter Saturn 2", "Saturn 2"]),
    ("ott", &["OTT"]),
    ("supermassive", &["ValhallaSupermassive", "Valhalla Supermassive"]),
    ("decapitator", &["Decapitator"]),
    ("pigments", &["Pigments", "Pigments 6"]),
];
/// Other versions, and devices with names close to these, that no adapter is for.
const NOT_THESE: &[&str] = &[
    "Serum",
    "SerumFX",
    "Serum 2 FX",
    "Ozone 11",
    "Ozone Imager",
    "Pro-Q 3",
    "Pro-L",
    "Saturn",
    "Vitalizer",
    "Operator",
    "EQ Eight",
    "Multiband Dynamics",
];

/// Parameter names in the forms each adapter's patterns are written for (each file says how sure those forms are).
fn sample_names(id: &str) -> &'static [&'static str] {
    match id {
        "serum2" => &[
            "A Vol",
            "Osc B Level",
            "A Pan",
            "A Octave",
            "A CoarsePit",
            "A Unison",
            "A UniDet",
            "B Uni Detune",
            "A UniBlend",
            "A WTPos",
            "Osc C WT Pos",
            "A Warp",
            "B Warp 2",
            "A RandPhase",
            "Sub Osc Level",
            "SubOscShape",
            "Noise Level",
            "Noise Pitch",
            "Fil Cutoff",
            "Filter 2 Cutoff",
            "Fil Reso",
            "Fil Driv",
            "Fil Var",
            "Fil Mix",
            "Fil Pan",
            "Env1 Atk",
            "Env 1 Release",
            "Env2 Dec",
            "LFO1 Rate",
            "LFO 10 Rate",
            "Macro 8",
            "MasterVol",
            "Porta Time",
            "Hyp Wet",
            "Dly Wet",
            "Main Reverb Mix",
            "Dist Drv",
            "Dly Feed",
            "Dly TimL",
            "Dly BPM_Sync",
            "VerbSize",
            "VerbLoCt",
            "Cmp Thr",
            "CmpGain",
            "Compressor Ratio",
        ],
        "vital" => &[
            "Oscillator 1 Level",
            "Osc 2 Pan",
            "Oscillator 1 Transpose",
            "Oscillator 1 Wave Frame",
            "Oscillator 1 Unison Voices",
            "Oscillator 2 Stereo Spread",
            "Oscillator 1 Spectral Morph Amount",
            "Oscillator 1 Distortion Amount",
            "Oscillator 1 Phase Randomization",
            "Sample Level",
            "Filter 1 Cutoff",
            "Filter FX Cutoff",
            "Filter 2 Resonance",
            "Filter 1 Drive",
            "Filter 1 Blend",
            "Filter 1 Key Track",
            "Filter 1 Formant X",
            "Envelope 1 Attack",
            "Envelope 2 Decay",
            "Envelope 2 Decay Power",
            "LFO 1 Frequency",
            "LFO 1 Tempo",
            "Macro 1",
            "Modulation 1 Amount",
            "Chorus Switch",
            "Reverb Mix",
            "Distortion Drive",
            "Compressor Low Gain",
            "Delay Feedback",
            "Reverb Decay Time",
            "Volume",
            "Polyphony",
            "Portamento Time",
        ],
        "ozone12" => &[
            "Maximizer: Threshold",
            "Maximizer: Ceiling",
            "Maximizer: Character",
            "Maximizer: IRC Mode",
            "Maximizer: Transient Emphasis",
            "Maximizer: True Peak",
            "Equalizer 1: Band 1 Gain",
            "EQ Band 2 Frequency",
            "Equalizer 1: Band 1 Q",
            "Dynamic EQ: Band 1 Threshold",
            "Dynamics: Band 1 Threshold",
            "Dynamics: Crossover 1",
            "Imager: Band 1 Width",
            "Exciter: Band 1 Amount",
            "Low End Focus: Contrast",
            "Stabilizer: Amount",
            "Master Rebalance: Vocal Gain",
            "Unlimiter: Amount",
            "Global: Output Gain",
        ],
        "proq4" => &[
            "Band 1 Used",
            "Band 1 Enabled",
            "Band 1 Frequency",
            "Band 12 Gain",
            "Band 1 Q",
            "Band 1 Shape",
            "Band 1 Slope",
            "Band 1 Stereo Placement",
            "Band 1 Dynamics Enabled",
            "Band 1 Dynamic Range",
            "Band 1 Threshold",
            "Band 1 Attack",
            "Band 1 Side Chain",
            "Band 1 Spectral Enabled",
            "Processing Mode",
            "Character",
            "Output Level",
            "Gain Scale",
        ],
        "prol2" => &[
            "Gain",
            "Output Level",
            "Style",
            "Lookahead",
            "Release",
            "Channel Link Transients",
            "True Peak Limiting",
            "Oversampling",
            "Unity Gain",
            "Dithering",
            "DC Offset Filter",
        ],
        "saturn2" => &[
            "Band 1 Style",
            "Band 1 Drive",
            "Band 1 Dynamics",
            "Band 1 Tone",
            "Band 1 Feedback Frequency",
            "Band 1 Mix",
            "Band 1 Mute",
            "Crossover 1 Frequency",
            "Mix",
            "Output Level",
            "High Quality",
            "XLFO 1 Rate",
        ],
        "ott" => &["Depth", "Time", "In Gain", "Out Gain", "Upwd %", "Downwd %", "H Gain", "L Gain"],
        "supermassive" => {
            &["Mix", "Delay_Ms", "Delay_Note", "Warp", "Feedback", "Density", "Width", "LowCut", "HighCut", "ModRate", "ModDepth", "Mode"]
        }
        "decapitator" => &["Drive", "Style", "Low Cut", "HighCut", "Thump", "Steep", "Tone", "Punish", "Mix", "Output", "Auto"],
        "pigments" => &[
            "Engine 1 Volume",
            "Engine 1 Coarse Tune",
            "Engine 1 Wavetable Position",
            "Engine 2 Unison Detune",
            "Engine 1 Filter Mix",
            "Filter 1 Cutoff",
            "Filter 2 Resonance",
            "Filter 1 Drive",
            "Filter Routing",
            "VCA Env Release",
            "Env 2 Decay",
            "LFO 1 Rate",
            "Macro 1",
            "Aux Send",
            "Master Volume",
        ],
        other => panic!("no sample names for {other}"),
    }
}

/// What the model reads of an adapter.
fn reading(adapter: &PluginAdapter) -> String {
    let mut lines: Vec<&str> = vec![adapter.name, adapter.vendor, adapter.overview, adapter.beyond];
    for section in &adapter.sections {
        lines.push(section.name);
        lines.push(section.about);
        for hint in &section.parameters {
            lines.push(hint.role);
            lines.push(hint.about);
        }
    }
    for recipe in &adapter.recipes {
        lines.push(recipe.name);
        lines.push(recipe.how);
    }
    lines.join("\n")
}

fn patterns(adapter: &PluginAdapter) -> Vec<&Pattern> {
    std::iter::once(&adapter.matcher).chain(adapter.hints().map(|hint| &hint.names)).collect()
}

#[test]
fn ten_adapters_in_their_order_each_with_its_own_id() {
    let ids: Vec<&str> = ADAPTERS.iter().map(|adapter| adapter.id).collect();
    let expected: Vec<&str> = LIVE_NAMES.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, expected);
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ADAPTERS.len());
}

#[test]
fn each_adapter_is_picked_by_its_plug_ins_names_in_live_and_by_no_others() {
    for (id, names) in LIVE_NAMES {
        for name in *names {
            let picked: Vec<&str> = ADAPTERS.iter().filter(|adapter| adapter.matches(name)).map(|adapter| adapter.id).collect();
            assert_eq!(picked, vec![*id], "{name}");
            assert_eq!(adapter_for(name, None).map(|adapter| adapter.id), Some(*id), "{name}");
        }
    }
    for name in NOT_THESE {
        assert!(adapter_for(name, None).is_none(), "{name}");
    }
}

#[test]
fn every_section_has_parameters_each_described_and_no_pattern_keeps_state_between_names() {
    for adapter in ADAPTERS.iter() {
        assert!(!adapter.sections.is_empty() && !adapter.overview.is_empty() && !adapter.beyond.is_empty(), "{}", adapter.id);
        for section in &adapter.sections {
            assert!(!section.parameters.is_empty(), "{}: {}", adapter.id, section.name);
            for hint in &section.parameters {
                assert!(!hint.role.is_empty() && !hint.about.is_empty(), "{}: {}", adapter.id, hint.role);
            }
        }
        // TS: also that no pattern has the global or sticky flag; a Rust pattern has neither.
        for pattern in patterns(adapter) {
            assert!(!matches(pattern, ""), "{}: {} takes any name", adapter.id, pattern.as_str());
        }
    }
}

#[test]
fn every_hint_takes_names_in_the_forms_its_written_for_and_each_of_those_names_lands_under_one_hint() {
    for adapter in ADAPTERS.iter() {
        let names = sample_names(adapter.id);
        let hints: Vec<_> = adapter.hints().collect();
        for hint in &hints {
            assert!(names.iter().any(|name| hint.matches(name)), "{}: {} takes none of its sample names", adapter.id, hint.role);
        }
        for name in names {
            assert_eq!(hints.iter().filter(|hint| hint.matches(name)).count(), 1, "{}: {}", adapter.id, name);
        }
    }
    let eq = |role: &str| ADAPTERS.iter().find(|adapter| adapter.id == "ozone12").unwrap().hints().find(|hint| hint.role == role).unwrap();
    for name in ["Dynamic EQ: Band 1 Gain", "Match EQ: Band 1 Gain", "Stem EQ: Band 1 Gain"] {
        assert!(!eq("eq gain").matches(name), "{name}");
    }
}

#[test]
fn each_adapter_reads_in_well_under_8_kb_with_4_8_recipes_for_a_synth_and_3_6_for_an_effect_and_folders_under_the_home_folder() {
    let home_folder = regex::Regex::new(r"^~/[^\\]+$").unwrap();
    for adapter in ADAPTERS.iter() {
        assert!(reading(adapter).len() < 7 * 1024, "{}: {} bytes", adapter.id, reading(adapter).len());
        let (least, most) = if adapter.kind == PluginKind::Instrument { (4, 8) } else { (3, 6) };
        assert!(adapter.recipes.len() >= least && adapter.recipes.len() <= most, "{}: {} recipes", adapter.id, adapter.recipes.len());
        for paths in [adapter.folders.and_then(|folders| folders.presets), adapter.folders.and_then(|folders| folders.wavetables)] {
            for path in [paths.and_then(|paths| paths.mac), paths.and_then(|paths| paths.windows)].into_iter().flatten() {
                assert!(home_folder.is_match(path), "{}: {}", adapter.id, path);
            }
        }
    }
}
