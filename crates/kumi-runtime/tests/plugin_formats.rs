//! The plug-in format readers against `plugin-formats/`: the data Kumi ships parses and agrees with the readers,
//! and every fixture (made from each plug-in's init patch) decodes, round-trips and fits its structure. The
//! states were saved by Live 12.4 for the same patches as the preset files, so they also show how a preset and
//! a host's state relate.

use std::path::PathBuf;

use kumi_runtime::plugins::formats::cbor::{self, Value};
use kumi_runtime::plugins::formats::ozone12::{self, OzonePreset, OzoneState};
use kumi_runtime::plugins::formats::serum2::{self, SerumPreset, SerumState, XferFile};
use kumi_runtime::plugins::formats::structure::{builtin, builtins, for_id, Structure};
use kumi_runtime::plugins::formats::tree::{Node, PathRule};
use kumi_runtime::plugins::formats::vital::VitalPreset;
use kumi_runtime::plugins::formats::vstpreset::{vst2_chunk, VstPreset};

fn fixture(plugin: &str, name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugin-formats").join(plugin).join("format-1/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn structure(plugin: &str) -> &'static Structure {
    &builtin(plugin).unwrap_or_else(|| panic!("{plugin} ships")).structure
}

/// Two values alike, a map's entries in any order.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Map(x, _), Value::Map(y, _)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && same(v, v2)))
        }
        _ => a == b,
    }
}

/// Every path in a tree as written, numbers and all ("settings/osc_1_level").
fn concrete_paths(tree: &Node) -> std::collections::BTreeSet<String> {
    let mut paths = std::collections::BTreeSet::new();
    tree.walk(PathRule::Exact, &mut |path, _| {
        paths.insert(path.to_string());
    });
    paths
}

/// Each linked parameter's location is a value the tree has (for formats that write every value).
fn links_resolve(plugin: &str, tree: &Node, which: impl Fn(&str) -> bool) -> usize {
    let paths = concrete_paths(tree);
    let mut resolved = 0;
    for (name, link) in structure(plugin).parameters.iter().filter(|(name, _)| which(name)) {
        let location = Structure::location(link);
        assert!(paths.contains(&location), "{plugin}: {name} links to {location}, which the tree doesn't have");
        resolved += 1;
    }
    resolved
}

/// The tree fits its structure: no field it doesn't list, none of another type.
fn fits(plugin: &str, tree: &Node) {
    let check = structure(plugin).check(tree);
    assert!(check.is_clean(), "{plugin}: unknown {:?}, mismatched {:?}", check.unknown, check.mismatched);
}

#[test]
fn the_shipped_formats_parse_and_agree_with_the_readers() {
    assert_eq!(builtins().len(), 3);
    for format in builtins() {
        let plugin = &format.info.plugin;
        assert_eq!(&format.structure.plugin, plugin);
        assert_eq!(&format.verified.plugin, plugin);
        assert!(format.info.formats.contains(&format.structure.format), "{plugin}");
        for (name, link) in &format.structure.parameters {
            assert!(format.structure.fields.contains_key(&link.field), "{plugin}: {name} links to an unknown field {}", link.field);
            assert_eq!(link.at.len(), link.field.matches("{n}").count(), "{plugin}: {name} fills {} with {:?}", link.field, link.at);
        }
    }
    let serum = &structure("serum-2").files["preset"].constants;
    assert_eq!(serum["magic"].as_str().map(str::as_bytes), Some(serum2::MAGIC));
    assert_eq!(serum["encoding"].as_u64(), Some(serum2::ZSTD as u64));
    let ozone = &structure("ozone-12").files["state"].constants;
    assert_eq!(ozone["magic"].as_u64(), Some(ozone12::STATE_MAGIC as u64));
    assert_eq!(for_id("56534558667350736572756D20320000").map(|f| f.info.plugin.as_str()), Some("serum-2"));
    assert_eq!(for_id("1449751649").map(|f| f.info.plugin.as_str()), Some("vital"));
    assert_eq!(for_id("5653545A425A4D4F7A6F6E652050726F").map(|f| f.info.plugin.as_str()), Some("ozone-12"));
    // A link's location fills its path.
    let (link, _) = structure("serum-2").parameter("Env 1 Attack").expect("linked");
    assert_eq!(Structure::location(link), "Env0/plainParams/kParamAttack");
}

#[test]
fn serum_2_preset_and_state_are_one_tree_in_two_containers() {
    let bytes = fixture("serum-2", "init.SerumPreset");
    let (file, decoded) = XferFile::read_with_body(&bytes).unwrap();
    assert_eq!(file.hash_ok, Some(true));
    assert_eq!(cbor::encode(&file.body), decoded, "the body re-encodes byte for byte");
    let preset = SerumPreset::read(&bytes).unwrap();
    assert_eq!(preset.product_version().as_deref(), Some("2.1.5"));
    assert_eq!(preset.format_version(), Some(11.0));
    let again = SerumPreset::read(&preset.file.write().unwrap()).unwrap();
    assert_eq!(again.file.body, preset.file.body);
    assert_eq!(again.file.hash_ok, Some(true));
    fits("serum-2", &preset.file.body.to_node());

    // Live's state of the same, unedited patch: a processor and a controller container whose trees, merged,
    // are the preset's.
    let live = VstPreset::read(&fixture("serum-2", "init.vstpreset")).unwrap();
    assert_eq!(live.class_id, "56534558667350736572756D20320000");
    let state = SerumState::read(live.component().unwrap(), live.controller()).unwrap();
    assert_eq!(state.preset_name().as_deref(), Some("Kumi Init"));
    fits("serum-2", &state.processor.body.to_node());
    fits("serum-2", &state.controller.as_ref().unwrap().body.to_node());
    let merged = state.merged();
    let mut sections = 0;
    for (key, value) in preset.file.body.entries() {
        if key == "fileType" {
            assert!(merged.get(key).is_none());
            continue;
        }
        assert!(merged.get(key).is_some_and(|merged| same(merged, value)), "section {key}");
        sections += 1;
    }
    assert_eq!(sections, 175);
    let state_only: Vec<&str> = merged.entries().map(|(k, _)| k).filter(|k| preset.file.body.get(k).is_none()).collect();
    assert_eq!(state_only, ["component", "modMatrixLocked", "presetHasBeenEdited", "scalarCurvesLocked", "selectedPresetPath"]);
    assert_eq!(merged.get("presetHasBeenEdited").and_then(Value::as_bool), Some(false));

    // A Distortion unit added to the main FX rack.
    let with = VstPreset::read(&fixture("serum-2", "init-distortion.vstpreset")).unwrap();
    let with = SerumState::read(with.component().unwrap(), with.controller()).unwrap();
    let rack = |state: &SerumState| state.processor.body.at("FXRack0/FX").and_then(Value::as_array).map(<[Value]>::len).unwrap_or(0);
    assert_eq!(rack(&with), rack(&state) + 1);
    assert!(with.processor.body.at("FXRack0/FX/0/FXDistortion").is_some());
}

#[test]
fn vital_state_is_its_preset_with_a_tuning() {
    let bytes = fixture("vital", "init.vital");
    let preset = VitalPreset::read(&bytes).unwrap();
    assert_eq!(preset.synth_version(), Some("1.0.7"));
    assert_eq!(preset.name(), Some("Kumi Init"));
    assert_eq!(VitalPreset::read(&preset.write()).unwrap(), preset);
    assert_eq!(preset.wavetable_frames(0).unwrap().iter().map(Vec::len).collect::<Vec<_>>(), [2048]);
    assert!(preset.tuning().is_none());
    fits("vital", &preset.to_node());
    // Vital writes every control, so every parameter Live lists lands on a value in the file.
    assert_eq!(links_resolve("vital", &preset.to_node(), |_| true), 772);

    let live = VstPreset::read(&fixture("vital", "init.vstpreset")).unwrap();
    assert_eq!(live.class_id, "56535456697461766974616C00000000");
    let chunk = vst2_chunk(live.component().unwrap()).unwrap().expect("the VST3 build wraps its VST2 chunk");
    assert_eq!((&chunk.plugin, chunk.plugin_version, &chunk.kind), (b"Vita", 0x0001_0007, b"FBCh"));
    let state = VitalPreset::read_state(chunk.data).unwrap();
    assert!(state.tuning().is_some());
    assert_eq!(state.settings(), preset.settings());
    let without_tuning = |json: &serde_json::Value| {
        let mut json = json.clone();
        json.as_object_mut().unwrap().remove("tuning");
        json
    };
    assert_eq!(without_tuning(&state.json), preset.json);
    fits("vital", &state.to_node());
}

#[test]
fn ozone_12_presets_are_sparse_xml_and_states_typed_json() {
    for (name, root, chain) in [("init.xml", "OzoneMS", vec![]), ("init-maximizer.xml", "OzoneMaximizer", vec!["Maximizer"])] {
        let preset = OzonePreset::read(&fixture("ozone-12", name)).unwrap();
        assert_eq!(preset.root(), root);
        assert_eq!((preset.attribute("PresetVer"), preset.attribute("PluginVer")), (Some("6"), Some("120100")));
        assert_eq!(OzonePreset::read(&preset.write()).unwrap(), preset);
        assert!(preset.modules().iter().all(|m| m.params.is_empty()), "an init preset leaves every default out");
        assert_eq!(preset.chain().unwrap().unwrap_or_default().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), chain);
        fits("ozone-12", &preset.to_node());
    }
    for (name, chain) in
        [("init.vstpreset", vec![]), ("init-exciter.vstpreset", vec!["Exciter"]), ("init-maximizer.vstpreset", vec!["Maximizer"])]
    {
        let live = VstPreset::read(&fixture("ozone-12", name)).unwrap();
        assert!(live.controller().unwrap_or_default().is_empty());
        let state = OzoneState::read(live.component().unwrap()).unwrap();
        assert_eq!(state.version, 4);
        assert_eq!(state.chain().unwrap().unwrap_or_default().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), chain, "{name}");
        assert!(state.value("Maximizer", "Character").and_then(serde_json::Value::as_f64).is_some(), "{name}");
        fits("ozone-12", &state.to_node());
    }
    // The state is complete where the preset is sparse: the init state's values are the fields' defaults, and
    // every linked parameter of the main plug-in (and of the Maximizer, from its own state) lands on a value.
    let state = OzoneState::read(VstPreset::read(&fixture("ozone-12", "init.vstpreset")).unwrap().component().unwrap()).unwrap();
    assert!(links_resolve("ozone-12", &state.to_node(), |name| !name.contains(" / ")) > 100);
    let maximizer =
        OzoneState::read(VstPreset::read(&fixture("ozone-12", "init-maximizer.vstpreset")).unwrap().component().unwrap()).unwrap();
    assert_eq!(links_resolve("ozone-12", &maximizer.to_node(), |name| name.starts_with("Ozone 12 Maximizer / ")), 13);
    let character = structure("ozone-12").field("DSP State/DSP Elements/Maximizer/Character").unwrap();
    assert_eq!(character.default, state.value("Maximizer", "Character").and_then(serde_json::Value::as_f64));
}
