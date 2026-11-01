use kumi_library::plugins::{
    formats::{structure, vstpreset::VstPreset, xml},
    preset::{self, Edit, WriteOptions},
    templates,
};
use serde_json::{json, Value};

fn record(format: &structure::PluginFormat) -> Value {
    let path =
        std::path::Path::new(env!("KUMI_WORKSPACE")).join("plugin-formats").join(format.info.format_directory()).join("verified.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn word(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
}

fn assert_name(preset: &VstPreset, expected: &str) {
    let info = xml::parse(std::str::from_utf8(preset.chunk("Info").expect("VST3 display metadata")).unwrap()).unwrap();
    let name = info.root.children.iter().find(|child| child.name == "Attribute" && child.attribute("id") == Some("Name")).unwrap();
    assert_eq!(name.attribute("value"), Some(expected), "VST3 Info name");
    let component = preset.component().unwrap();
    let state = if component.starts_with(b"FabF") { component } else { preset.controller().expect("FFBS controller display name") };
    assert_eq!(std::str::from_utf8(&state[12..12 + word(state, 8)]).unwrap(), expected, "plug-in's internal preset name");
}

fn controller_metadata(controller: &[u8]) -> std::collections::BTreeMap<String, String> {
    let mut position = 20 + word(controller, 8);
    for _ in 0..2 {
        position += 4 + word(controller, position);
    }
    assert_eq!(&controller[position..position + 8], b"CuSV\x01\0\0\0");
    let count = word(controller, position + 8);
    position += 12;
    let mut entries = std::collections::BTreeMap::new();
    for _ in 0..count {
        let mut read = || {
            let length = word(controller, position);
            position += 4;
            let text = std::str::from_utf8(&controller[position..position + length]).unwrap().to_owned();
            position += length;
            text
        };
        entries.insert(read(), read());
    }
    entries
}

#[test]
fn fabfilter_products_create_edit_inspect_and_preserve_host_state() {
    let mut products = 0;
    for format in structure::builtins().iter().filter(|format| format.info.family == "fabfilter") {
        let plugin = &format.info.plugin;
        let evidence = record(format);
        let check = &evidence["checks"][0];
        let path = check["path"].as_str().expect("native readback edit path");
        let edit = Edit { path: path.into(), value: check["value"].clone() };
        for version in &format.verified.writes {
            let mut options = WriteOptions { version: version.version.clone(), ..Default::default() };
            let initial = templates::initial_for_version(plugin, &options.version).or_else(|| templates::initial(plugin)).unwrap();
            assert_eq!(preset::edit(plugin, initial, &options, &[]).unwrap().write(), initial, "{plugin} {} no-op", options.version);
            options.name = Some("Kumi FabFilter 测试".into());
            let generated = preset::create(plugin, &options, std::slice::from_ref(&edit)).unwrap();
            assert!(format.has_id(&generated.class_id));
            assert_name(&generated, options.name.as_deref().unwrap());
            let inspected = preset::inspect(plugin, &generated.write(), None).unwrap();
            let name = path.strip_prefix("parameters/").unwrap();
            let value = inspected["data"]["parameters"][name].as_f64().unwrap();
            let expected = edit.value.as_f64().unwrap();
            assert!((value - expected).abs() <= 1e-5 * expected.abs().max(1.0), "{plugin} {} readback", options.version);
            let mut existing = VstPreset::read(initial).unwrap();
            existing.chunks.push(("Priv".into(), b"host-private data".to_vec()));
            options.name = None;
            let component = existing.component().unwrap();
            let offset = if component.starts_with(b"FabF") { 20 + word(component, 8) } else { 12 };
            let at = offset + 4 * check["stored_index"].as_u64().unwrap() as usize;
            let mut expected_component = component.to_vec();
            expected_component[at..at + 4].copy_from_slice(&(expected as f32).to_le_bytes());
            assert_ne!(component, expected_component, "{plugin} {} must exercise a real edit", options.version);
            let updated = preset::edit(plugin, &existing.write(), &options, std::slice::from_ref(&edit)).unwrap();
            assert_eq!(updated.component(), Some(expected_component.as_slice()), "{plugin}: more than the intended float slot changed");
            assert_eq!(updated.chunk("Priv"), Some(b"host-private data".as_slice()));
            for (id, bytes) in existing.chunks.iter().filter(|(id, _)| id != "Comp") {
                assert_eq!(updated.chunk(id), Some(bytes.as_slice()), "{plugin}: an unrelated edit changed {id}");
            }
            assert!(preset::edit(plugin, generated.component().unwrap(), &options, &[]).is_err(), "bare component accepted: {plugin}");
            assert!(preset::inspect(plugin, generated.component().unwrap(), None).is_err());
            let invalid = [edit.clone(), Edit { path: "parameters/Invented setting".into(), value: json!(0.5) }];
            assert!(preset::edit(plugin, initial, &options, &invalid).is_err());
            options.version = "999.0.0.0".into();
            assert!(preset::create(plugin, &options, &[]).is_err());
        }
        products += 1;
    }
    assert_eq!(products, 25, "the current and legacy VST3 product families must all be covered");
}

#[test]
fn native_imports_use_the_requested_name_and_source_author_through_the_public_workflow() {
    for format in structure::builtins().iter().filter(|format| format.info.family == "fabfilter") {
        let plugin = &format.info.plugin;
        let path = std::path::Path::new(env!("KUMI_WORKSPACE"))
            .join("plugin-formats")
            .join(format.info.format_directory())
            .join("fixtures/init.ffp");
        let bytes = std::fs::read(path).unwrap();
        let options = WriteOptions {
            version: format.verified.writes[0].version.clone(),
            name: Some("Imported Kumi 音".into()),
            ..Default::default()
        };
        let imported = preset::edit(plugin, &bytes, &options, &[]).unwrap();
        assert_name(&imported, options.name.as_deref().unwrap());
        if imported.component().unwrap().starts_with(b"FFBS") {
            let template = VstPreset::read(templates::initial(plugin).unwrap()).unwrap();
            let before = controller_metadata(template.controller().unwrap());
            let after = controller_metadata(imported.controller().unwrap());
            assert_eq!(after.get("AUTHOR").map(String::as_str), Some("Kumi fixture author"), "{plugin}: source author lost");
            assert_eq!(
                after.get("DESCRIPTION").map(String::as_str),
                Some("Synthetic test controls generated by Kumi."),
                "{plugin}: source description lost"
            );
            assert_ne!(before.get("AUTHOR"), after.get("AUTHOR"), "{plugin}: fixture must exercise metadata replacement");
        }
    }
}

#[test]
fn missing_native_text_metadata_does_not_inherit_the_template_author_or_description() {
    let plugin = "fabfilter-pro-q-4";
    let format = structure::builtin(plugin).unwrap();
    let bytes = b"[Preset]\nSignature=FQ4p\nVersion=4\n[Parameters]\nBand 1 Gain=0\n";
    let options =
        WriteOptions { version: format.verified.writes[0].version.clone(), name: Some("Private preset".into()), ..Default::default() };
    let imported = preset::edit(plugin, bytes, &options, &[]).unwrap();
    let metadata = controller_metadata(imported.controller().unwrap());
    assert!(!metadata.contains_key("AUTHOR"));
    assert!(!metadata.contains_key("DESCRIPTION"));
    assert_name(&imported, "Private preset");
}

#[test]
fn older_fabfilter_templates_keep_their_own_parameter_layout() {
    let plugin = "fabfilter-twin-3";
    let older = templates::initial_for_version(plugin, "3.0.5.0").expect("captured older Twin 3");
    let current = templates::initial(plugin).unwrap();
    let old_container = VstPreset::read(older).unwrap();
    let new_container = VstPreset::read(current).unwrap();
    assert_ne!(&old_container.component().unwrap()[8..12], &new_container.component().unwrap()[8..12]);
    let options = WriteOptions { version: "3.0.5.0".into(), ..Default::default() };
    assert_eq!(preset::create(plugin, &options, &[]).unwrap().component(), old_container.component());
    assert!(preset::edit(plugin, current, &options, &[]).is_err(), "newer layout reached the older host");
    assert!(preset::inspect(plugin, older, None).is_ok());
}

#[test]
fn fabfilter_writes_reject_mismatched_classes_and_controllers() {
    let plugin = "fabfilter-pro-q-4";
    let format = structure::builtin(plugin).unwrap();
    let options = WriteOptions { version: format.verified.writes[0].version.clone(), ..Default::default() };
    let original = templates::initial(plugin).unwrap();
    let mut wrong_class = VstPreset::read(original).unwrap();
    wrong_class.class_id = structure::builtin("fabfilter-pro-r-2").unwrap().info.ids.vst3[0].id.clone();
    assert!(preset::edit(plugin, &wrong_class.write(), &options, &[]).is_err());
    let mut wrong_controller = VstPreset::read(original).unwrap();
    let (_, controller) = wrong_controller.chunks.iter_mut().find(|(id, _)| id == "Cont").unwrap();
    controller[..4].copy_from_slice(b"NOPE");
    assert!(preset::edit(plugin, &wrong_controller.write(), &options, &[]).is_err());
}

#[test]
fn an_older_native_preset_can_edit_controls_added_by_the_installed_version() {
    let plugin = "fabfilter-one";
    let format = structure::builtin(plugin).unwrap();
    let native = include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/fabfilter-one/format-1/fixtures/init-v0.ffp"));
    let path = "parameters/Polyphony";
    let value = format.structure.field(path).unwrap().range.unwrap()[1];
    let options = WriteOptions { version: format.verified.writes[0].version.clone(), ..Default::default() };
    let preset = preset::edit(plugin, native, &options, &[Edit { path: path.into(), value: json!(value) }]).unwrap();
    let readback = preset::inspect(plugin, &preset.write(), None).unwrap();
    assert_eq!(readback["data"]["parameters"]["Polyphony"], json!(value));
}
