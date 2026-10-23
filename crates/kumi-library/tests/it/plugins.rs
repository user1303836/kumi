//! The guide. The plugin tool's test (reading a
//! plug-in's names through the synthetic bridge and making a wavetable) waits for the integration.

use kumi_runtime::plugins::adapter::{
    pattern, PluginAdapter, PluginKind, PluginParameterHint, PluginRecipe, PluginSection, PluginWavetable, WavetableFormat,
};
use kumi_runtime::plugins::registry::{adapter_for, group_names, plugin_guide, ExposedParameter};
use serde_json::{json, Value};

fn synth() -> PluginAdapter {
    PluginAdapter {
        id: "fixture-synth",
        name: "Fixture Synth",
        vendor: "Kumi",
        kind: PluginKind::Instrument,
        matcher: pattern(r"(?i)^fixture ?synth"),
        overview: "Two oscillators into a filter.",
        sections: vec![PluginSection {
            name: "Filter",
            about: "One low-pass.",
            parameters: vec![
                PluginParameterHint { role: "cutoff", names: pattern(r"(?i)^(fil|filter)\s*cutoff$"), about: "Where it closes" },
                PluginParameterHint { role: "drive", names: pattern(r"(?i)drive"), about: "Grit" },
            ],
        }],
        recipes: vec![PluginRecipe { name: "Reese", how: "Two saws, 12 cents apart." }],
        beyond: "Its wavetables aren't parameters.",
        folders: None,
        wavetable: Some(PluginWavetable { frame: 2048, max_frames: 256, format: WavetableFormat::Clm }),
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn a_plug_ins_guide_sets_kumis_notes_against_its_real_parameters_what_kumi_can_turn_and_the_rest_grouped() {
    let synth = synth();
    let adapters = [&synth];
    assert_eq!(adapter_for("Fixture Synth", Some(&adapters)).map(|adapter| adapter.id), Some("fixture-synth"));
    assert_eq!(adapter_for("FixtureSynth (VST3)", Some(&adapters)).map(|adapter| adapter.id), Some("fixture-synth"));
    assert!(adapter_for("Operator", Some(&adapters)).is_none());
    let names = strings(&["Fil Cutoff", "Fil Reso", "A Level", "A Pan", "A Octave", "B Level", "Drive"]);
    let exposed = [ExposedParameter { name: "Fil Cutoff".into(), reference: "7:parameter:1".into(), display: Some("812 Hz".into()) }];
    let guide = Value::Object(plugin_guide("Fixture Synth", Some(&synth), &names, &exposed));
    assert_eq!(guide["plugin"], json!("Fixture Synth (Kumi)"));
    assert_eq!(guide["canTurn"], json!([{ "name": "Fil Cutoff", "ref": "7:parameter:1", "now": "812 Hz" }]));
    assert_eq!(guide["sections"][0]["parameters"][0]["live"], json!(["Fil Cutoff (Kumi can turn it)"]));
    assert_eq!(guide["sections"][0]["parameters"][1]["live"], json!(["Drive"]));
    assert_eq!(guide["parameters"]["notConfigured"], json!(6));
    assert!(guide["toTurnMore"].as_str().unwrap().contains("Configure"));
    let groups: Vec<(String, usize)> = group_names(&strings(&["A Level", "A Pan", "B Level", "LFO 1 Rate", "LFO 1 Depth"]), 60)
        .into_iter()
        .map(|group| (group.group, group.count))
        .collect();
    assert_eq!(groups, vec![("A".to_string(), 2), ("B".to_string(), 1), ("LFO 1".to_string(), 2)]);
    let unknown = Value::Object(plugin_guide("Mystery Box", None, &strings(&["Knob 1"]), &[]));
    assert!(unknown["note"].as_str().unwrap().contains("no notes on this plug-in"));
}
