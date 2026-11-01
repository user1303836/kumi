use kumi_library::plugins::{
    formats::{structure, vstpreset::VstPreset},
    preset::{self, Edit, WriteOptions},
    templates,
};
use serde_json::{json, Value};

fn record(format: &structure::PluginFormat) -> Value {
    let path =
        std::path::Path::new(env!("KUMI_WORKSPACE")).join("plugin-formats").join(format.info.format_directory()).join("verified.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
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
            let mut generated = preset::create(plugin, &options, std::slice::from_ref(&edit)).unwrap();
            assert!(format.has_id(&generated.class_id));
            let inspected = preset::inspect(plugin, &generated.write(), None).unwrap();
            let name = path.strip_prefix("parameters/").unwrap();
            let value = inspected["data"]["parameters"][name].as_f64().unwrap();
            let expected = edit.value.as_f64().unwrap();
            assert!((value - expected).abs() <= 1e-5 * expected.abs().max(1.0), "{plugin} {} readback", options.version);
            generated.chunks.push(("Priv".into(), b"host-private data".to_vec()));
            options.name = None;
            let controller = generated.controller().map(Vec::from);
            let updated = preset::edit(plugin, &generated.write(), &options, std::slice::from_ref(&edit)).unwrap();
            assert_eq!(updated.chunk("Priv"), Some(b"host-private data".as_slice()));
            assert_eq!(updated.controller(), controller.as_deref(), "an unrelated edit changed controller metadata: {plugin}");
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
