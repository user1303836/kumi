use kumi::tui::{icons::IconKind, tree::*};
use kumi_runtime::core::contracts::DeviceTree;
use serde_json::{json, Value};
fn fx(reference: &str, name: &str) -> Value {
    json!({"ref":reference,"name":name,"className":regex::Regex::new(r"\W").unwrap().replace_all(name,""),"deviceType":"audio_effect"})
}
fn audio() -> DeviceTree {
    serde_json::from_value(json!({"trackRef":"1:track:3","devices":[fx("d0","Chorus-Ensemble"),fx("d1","Compressor"),{"ref":"d2","name":"Audio Effect Rack","className":"AudioEffectGroupDevice","canHaveChains":true,"chains":[{"ref":"c0","name":"Chain 1","devices":[fx("d2a","Saturator"),fx("d2b","EQ Eight")]},{"ref":"c1","name":"Chain 2","devices":[fx("d2c","Saturator"),fx("d2d","Utility")]}]},fx("d3","Gate")]})).unwrap()
}
fn focus(device: &str, chain: Option<&str>) -> TreeFocus {
    TreeFocus { device: Some(device.into()), chain: chain.map(str::to_string), device_ref: None, open_containers: false }
}
fn lines(tree: &DeviceTree, focus: &TreeFocus) -> Vec<String> {
    tree_rows(tree, focus)
        .iter()
        .map(|row| {
            format!(
                "{}{}{}{}",
                row.prefix,
                row.name,
                row.count.map(|n| format!(" ({n})")).unwrap_or_default(),
                match row.role {
                    TreeRole::Focus => " ◀",
                    TreeRole::Path => " ·",
                    TreeRole::Other => "",
                }
            )
        })
        .collect()
}
#[test]
fn selected_path_open_others_folded() {
    let tree = audio();
    assert_eq!(
        lines(&tree, &focus("Saturator", Some("Chain 1"))),
        [
            "├ Chorus-Ensemble",
            "├ Compressor",
            "├ Audio Effect Rack ·",
            "│ ├ Chain 1 ·",
            "│ │ ├ Saturator ◀",
            "│ │ └ EQ Eight",
            "│ └ Chain 2 (2)",
            "└ Gate"
        ]
    );
    assert_eq!(lines(&tree, &focus("Gate", None)), ["├ Chorus-Ensemble", "├ Compressor", "├ Audio Effect Rack (2)", "└ Gate ◀"]);
    assert_eq!(&lines(&tree, &focus("Audio Effect Rack", None))[2..5], ["├ Audio Effect Rack ◀", "│ ├ Chain 1 (2)", "│ └ Chain 2 (2)"]);
}
#[test]
fn duplicate_names_disambiguated_by_selected_chain() {
    let tree = audio();
    for (device, chain, expected) in [
        ("Saturator", Some("Chain 2"), vec!["d2", "c1", "d2c"]),
        ("Saturator", Some("Chain 1"), vec!["d2", "c0", "d2a"]),
        ("Saturator", None, vec!["d2", "c0", "d2a"]),
        ("Reverb", Some("Chain 2"), vec!["d2", "c1"]),
    ] {
        assert_eq!(focus_path_refs(&tree, Some(device), chain, None).unwrap(), expected);
    }
    assert!(focus_path_refs(&tree, Some("Reverb"), None, None).is_none());
    assert_eq!(&lines(&tree, &focus("Reverb", Some("Chain 2")))[2..5], ["├ Audio Effect Rack ·", "│ ├ Chain 1 (2)", "│ └ Chain 2 ◀"]);
}
#[test]
fn rows_carry_kind_trail_and_neighbors() {
    let rows = tree_rows(&audio(), &focus("Saturator", Some("Chain 1")));
    let sat = rows.iter().find(|r| r.r#ref == "d2a").unwrap();
    assert_eq!(sat.kind, IconKind::AudioEffect);
    assert_eq!(sat.trail, ["Audio Effect Rack", "Chain 1"]);
    assert_eq!(sat.siblings, ["EQ Eight"]);
    assert_eq!(rows.iter().find(|r| r.r#ref == "d2").unwrap().kind, IconKind::AudioRack);
    assert_eq!(rows.iter().find(|r| r.r#ref == "c0").unwrap().kind, IconKind::Chain);
}
#[test]
fn drum_pads_nested_racks_and_focus_window() {
    let drums:DeviceTree=serde_json::from_value(json!({"trackRef":"1:track:0","devices":[{"ref":"r","name":"Drum Rack","className":"DrumGroupDevice","canHaveChains":true,"canHaveDrumPads":true,"chains":(0..16).map(|i|json!({"ref":format!("p{i}"),"name":format!("Pad {}",i+1)})).collect::<Vec<_>>()},{"ref":"i","name":"Instrument Rack","className":"InstrumentGroupDevice","canHaveChains":true,"chains":[{"ref":"ic","name":"Layer","devices":[{"ref":"ia","name":"Audio Effect Rack","className":"AudioEffectGroupDevice","canHaveChains":true,"chains":[{"ref":"iac","name":"Chain","devices":[fx("deep","Erosion")]}]}]}]}]})).unwrap();
    assert_eq!(
        lines(&drums, &focus("Erosion", None)),
        ["├ Drum Rack (16)", "└ Instrument Rack ·", "  └ Layer ·", "    └ Audio Effect Rack ·", "      └ Chain ·", "        └ Erosion ◀"]
    );
    let rows = tree_rows(&drums, &focus("Drum Rack", None));
    assert_eq!(rows.iter().filter(|r| r.kind == IconKind::DrumPad).count(), 16);
    let window = tree_window(&rows, 12, 12);
    assert_eq!(window.rows.len(), 12);
    assert_eq!(window.above + window.below + 12, rows.len());
    assert!(window.rows.contains(&rows[12]));
    assert_eq!(tree_window(&rows[..5], 12, 0), TreeWindow { rows: rows[..5].to_vec(), above: 0, below: 0 });
}
#[test]
fn exact_reference_wins_over_names() {
    let tree = audio();
    assert_eq!(focus_path_refs(&tree, Some("Saturator"), Some("Chain 1"), Some("d2c")).unwrap(), ["d2", "c1", "d2c"]);
    assert_eq!(focus_path_refs(&tree, Some("Saturator"), None, Some("gone")).unwrap(), ["d2", "c0", "d2a"]);
}
