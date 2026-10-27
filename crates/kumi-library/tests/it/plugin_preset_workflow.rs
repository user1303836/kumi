use kumi_library::plugins::formats::juce_xml::XmlState;
use kumi_library::plugins::{
    formats::{
        cbor, minimal_audio::MinimalAudioPreset, ozone12, pulsar_modular::PulsarModularPreset, serum2, softube, structure,
        superior_drummer, valhalla::ValhallaPreset, vstpreset::VstPreset, zynaptiq,
    },
    preset::{self, Edit, WriteOptions},
    templates,
};
use serde_json::{json, Value};

fn options(plugin: &str) -> WriteOptions {
    WriteOptions {
        version: structure::builtin(plugin).unwrap().verified.writes.first().unwrap().version.clone(),
        class_id: None,
        name: None,
    }
}

fn change(path: &str, value: Value) -> Edit {
    Edit { path: path.into(), value }
}

fn evidence(plugin: &str) -> Value {
    let path = std::path::Path::new(env!("KUMI_WORKSPACE")).join("plugin-formats").join(plugin).join("format-1/verified.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn vendor_products_create_edit_and_keep_unrelated_chunks_through_one_api() {
    let mut count = 0;
    for format in structure::builtins().iter().filter(|f| matches!(f.info.family.as_str(), "valhalla" | "minimal-audio" | "pulsar-modular"))
    {
        let plugin = &format.info.plugin;
        count += 1;
        let mut opts = options(plugin);
        let initial = templates::initial(plugin).unwrap();
        let exact = preset::edit(plugin, initial, &opts, &[]).unwrap();
        assert_eq!(exact.write(), initial, "{plugin}: a no-op changed the initialized state");
        opts.name = Some("Kumi local patch".into());
        let record = evidence(plugin);
        let recorded = &record["checks"][0];
        let path = recorded["path"].as_str().unwrap();
        let edit = change(path, recorded["value"].clone());
        let mut generated = preset::create(plugin, &opts, &[edit]).unwrap();
        assert!(format.has_id(&generated.class_id));
        let inspected = preset::inspect(plugin, &generated.write(), None).unwrap();
        assert_eq!(inspected["data"].pointer(&format!("/{path}")).unwrap(), &recorded["value"], "{plugin}");
        generated.chunks.push(("Priv".into(), b"private host information".to_vec()));
        let controller = generated.controller().map(Vec::from);
        opts.name = Some("Retained host data".into());
        let edited = preset::edit(plugin, &generated.write(), &opts, &[]).unwrap();
        assert_eq!(edited.chunk("Priv"), Some(b"private host information".as_slice()), "{plugin}");
        assert_eq!(edited.controller(), controller.as_deref(), "{plugin}");
    }
    assert_eq!(count, 39);
}

#[test]
fn version_identity_and_ambiguous_containers_are_refused() {
    let plugin = "valhalla-delay";
    let initial = templates::initial(plugin).unwrap();
    let mut opts = options(plugin);
    opts.version = "unverified version".into();
    assert!(preset::edit(plugin, initial, &opts, &[]).is_err());
    opts = options(plugin);
    opts.class_id = Some(structure::builtin("valhalla-room").unwrap().info.ids.vst3[0].id.clone());
    assert!(preset::edit(plugin, initial, &opts, &[]).is_err());
    opts.class_id = None;
    for duplicate in ["Comp", "Cont", "Info"] {
        let mut container = VstPreset::read(initial).unwrap();
        container.chunks.retain(|(id, _)| id != duplicate || id == "Comp");
        if duplicate != "Comp" {
            container.chunks.push((duplicate.into(), Vec::new()));
        }
        let bytes = container.chunk(duplicate).unwrap().to_vec();
        container.chunks.push((duplicate.into(), bytes));
        assert!(preset::edit(plugin, &container.write(), &opts, &[]).is_err(), "accepted duplicate {duplicate}");
    }
    let mut mismatched = VstPreset::read(initial).unwrap();
    mismatched.class_id = structure::builtin("valhalla-room").unwrap().info.ids.vst3[0].id.clone();
    assert!(preset::edit("valhalla-room", &mismatched.write(), &options("valhalla-room"), &[]).is_err());
    assert!(preset::create("softube-weiss-ds1-mk3", &WriteOptions { version: "2.6.18".into(), ..Default::default() }, &[]).is_err());
}

#[test]
fn native_effect_presets_convert_with_the_selected_product_envelope() {
    for plugin in ["valhalla-delay", "minimal-cluster-delay", "pulsar-p44-magnum"] {
        let template = VstPreset::read(templates::initial(plugin).unwrap()).unwrap();
        let component = template.component().unwrap();
        let xml = match plugin {
            "valhalla-delay" => ValhallaPreset::read(component).unwrap().write_preset(),
            "minimal-cluster-delay" => MinimalAudioPreset::read(component).unwrap().write_preset(),
            _ => PulsarModularPreset::read(component).unwrap().write_preset().unwrap(),
        };
        let record = evidence(plugin);
        let edits = [change(record["checks"][0]["path"].as_str().unwrap(), record["checks"][0]["value"].clone())];
        let output = preset::edit(plugin, &xml, &options(plugin), &edits).unwrap();
        assert_eq!(output.class_id, template.class_id);
        assert_eq!(output.controller(), template.controller());
        assert!(preset::inspect(plugin, &output.write(), None).is_ok());
        if plugin == "valhalla-delay" {
            assert!(preset::edit("valhalla-room", &xml, &options("valhalla-room"), &[]).is_err());
        }
    }
}

#[test]
fn batches_refuse_unknown_fields_and_invalid_parameter_values() {
    let initial = templates::initial("valhalla-delay").unwrap();
    let opts = options("valhalla-delay");
    let original = initial.to_vec();
    assert!(preset::edit(
        "valhalla-delay",
        initial,
        &opts,
        &[change("parameters/Mix", json!(0.25)), change("parameters/Invented", json!(1))]
    )
    .is_err());
    assert!(preset::edit("valhalla-delay", initial, &opts, &[change("parameters/Mix", json!(1.1))]).is_err());
    assert_eq!(initial, original);
    let mut invalid_name = opts;
    invalid_name.name = Some("a\nb".into());
    assert!(preset::edit("valhalla-delay", initial, &invalid_name, &[]).is_err());
}

fn put(tree: &mut cbor::Value, name: &str, value: cbor::Value) {
    let cbor::Value::Map(fields, _) = tree else { panic!("expected a map") };
    fields.push((cbor::Value::text(name), value));
}

#[test]
fn serum_state_edits_keep_unknown_processor_and_controller_data_and_check_hashes() {
    let opts = options("serum-2");
    let bytes = templates::initial("serum-2").unwrap();
    assert_eq!(preset::edit("serum-2", bytes, &opts, &[]).unwrap().write(), bytes);
    let mut container = VstPreset::read(bytes).unwrap();
    let mut state = serum2::SerumState::read(container.component().unwrap(), container.controller()).unwrap();
    put(&mut state.processor.body, "futureProcessor", cbor::Value::text("keep this"));
    put(&mut state.controller.as_mut().unwrap().body, "futureController", cbor::Value::text("keep this too"));
    for (id, data) in &mut container.chunks {
        if id == "Comp" {
            *data = state.processor.write().unwrap();
        }
        if id == "Cont" {
            *data = state.controller.as_ref().unwrap().write().unwrap();
        }
    }
    let edited = preset::edit(
        "serum-2",
        &container.write(),
        &opts,
        &[
            change("Oscillator1/plainParams/kParamEnable", json!(1)),
            change("Oscillator1/plainParams/kParamCoarsePit", json!(12)),
            change("$add_distortion", json!(0)),
        ],
    )
    .unwrap();
    let state = serum2::SerumState::read(edited.component().unwrap(), edited.controller()).unwrap();
    assert_eq!(state.processor.body.get("futureProcessor").and_then(cbor::Value::as_str), Some("keep this"));
    assert_eq!(state.controller.as_ref().unwrap().body.get("futureController").and_then(cbor::Value::as_str), Some("keep this too"));
    assert_eq!(state.processor.hash_ok, Some(true));
    assert_eq!(state.controller.as_ref().unwrap().hash_ok, Some(true));
    assert_eq!(state.processor.body.at("Oscillator1/plainParams/kParamCoarsePit").and_then(cbor::Value::as_f64), Some(12.0));
    let mut corrupted = container;
    let data = &mut corrupted.chunks.iter_mut().find(|(id, _)| id == "Comp").unwrap().1;
    let hash = data.windows(8).position(|w| w == b"\"hash\":\"").unwrap() + 8;
    data[hash] = if data[hash] == b'0' { b'1' } else { b'0' };
    assert!(preset::edit("serum-2", &corrupted.write(), &opts, &[]).is_err());
}

#[test]
fn ozone_suite_and_every_module_keep_their_own_processor_identity() {
    for variant in ozone12::VARIANTS {
        let mut opts = options("ozone-12");
        opts.class_id = Some(variant.class_id.into());
        opts.name = Some("Kumi Ozone".into());
        let created = preset::create("ozone-12", &opts, &[]).unwrap();
        assert_eq!(created.class_id, variant.class_id);
        let inspected = preset::inspect("ozone-12", &created.write(), opts.class_id.as_deref()).unwrap();
        assert_eq!(inspected["classId"], variant.class_id);
    }
    let opts = options("ozone-12");
    let edited = preset::create(
        "ozone-12",
        &opts,
        &[change("$chain", json!(["Equalizer", "Stereo Imager", "Maximizer"])), change("parameters/Maximizer/Gain", json!(3.0))],
    )
    .unwrap();
    let inspected = preset::inspect("ozone-12", &edited.write(), None).unwrap();
    assert_eq!(inspected["data"]["parameters"]["Maximizer"]["Gain"], 3.0);
    assert_eq!(inspected["data"]["chain"], json!(["Equalizer", "Stereo Imager", "Maximizer"]));
    let mut mismatched = edited;
    mismatched.class_id = ozone12::VARIANTS[1].class_id.into();
    assert!(preset::inspect("ozone-12", &mismatched.write(), None).is_err());
}

#[test]
fn verified_dynamics_and_sampler_workflows_write_their_native_units() {
    for (plugin, path, value) in [
        ("zynaptiq-punch-1", "inputParams/@p4", 3.0),
        ("zynaptiq-unmix-drums", "inputParams/@p1", 0.7),
        ("superior-drummer-3", "SAMPLER[0]/HQ[0]/mastervol[0]", 0.25),
    ] {
        let output = preset::create(plugin, &options(plugin), &[change(path, json!(value))]).unwrap();
        let inspected = preset::inspect_fields(plugin, &output.write(), None, Some(path), 10).unwrap();
        assert_eq!(inspected["fields"][0]["path"], path);
        assert_eq!(inspected["fields"][0]["value"], value);
        let native = if plugin == "superior-drummer-3" {
            superior_drummer::State::read(output.component().unwrap()).unwrap().preset.unwrap().write()
        } else {
            zynaptiq::State::read(output.component().unwrap()).unwrap().preset.write().unwrap()
        };
        let converted = preset::edit(plugin, &native, &options(plugin), &[]).unwrap();
        let restored = preset::inspect_fields(plugin, &converted.write(), None, Some(path), 10).unwrap();
        assert_eq!(restored["fields"][0]["value"], value);
    }
    let old = superior_drummer::Preset::create("3.3.0").unwrap().write();
    assert!(preset::edit("superior-drummer-3", &old, &options("superior-drummer-3"), &[]).is_err());
    let mut old = VstPreset::read(templates::initial("superior-drummer-3").unwrap()).unwrap();
    let bytes = &mut old.chunks.iter_mut().find(|(id, _)| id == "Comp").unwrap().1;
    let bank = 8 + u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    bytes[bank + 20..bank + 24].copy_from_slice(&0x0003_0300u32.to_be_bytes());
    assert!(preset::edit("superior-drummer-3", &old.write(), &options("superior-drummer-3"), &[]).is_err());
}

#[test]
fn agent_inspection_is_bounded_and_returns_concrete_edit_paths() {
    let input = templates::initial("minimal-current").unwrap();
    let short = preset::inspect_fields("minimal-current", input, None, Some("parameters/"), 3).unwrap();
    assert_eq!(short["fields"].as_array().unwrap().len(), 3);
    assert!(short["more"].as_u64().unwrap() > 100);
    for field in short["fields"].as_array().unwrap() {
        assert!(field["path"].as_str().unwrap().starts_with("parameters/"));
    }
    let serum = preset::create("serum-2", &options("serum-2"), &[change("Oscillator1/plainParams/kParamCoarsePit", json!(12))]).unwrap();
    let concrete = preset::inspect_fields("serum-2", &serum.write(), None, Some("Oscillator1/plainParams/kParamCoarsePit"), 5).unwrap();
    assert_eq!(concrete["fields"][0]["path"], "Oscillator1/plainParams/kParamCoarsePit");
    assert_eq!(concrete["fields"][0]["value"], 12.0);
}

#[test]
fn native_softube_templates_require_the_exact_product_and_full_parameter_layout() {
    for format in structure::builtins().iter().filter(|format| format.info.family == "softube") {
        let identity = format.structure.files["preset"].constants["nativePluginId"].as_str().unwrap();
        let parameters: Vec<_> = format
            .structure
            .fields
            .keys()
            .filter_map(|path| {
                path.strip_prefix("presets[]/parameters/")
                    .and_then(|id| id.parse::<u32>().ok())
                    .map(|id| (id, 0.5, format!("Synthetic {id}")))
            })
            .collect();
        let mut template = softube::Preset::create(identity, &format.info.name, "2.4.44", "Synthetic test", &parameters).unwrap();
        template.document["SharedData"] = json!({"unrecognized":"keep"});
        template.document["PresetList"][0]["Unknown"] = json!("retain this");
        let second = template.document["PresetList"][0].clone();
        template.document["PresetList"].as_array_mut().unwrap().push(second);
        let bytes = template.write().unwrap();
        assert_eq!(preset::edit_native(&format.info.plugin, &bytes, None, &[]).unwrap().bytes, bytes);
        let created = preset::create_native_from_template(
            &format.info.plugin,
            &bytes,
            1,
            "My new preset",
            &[change(&format!("presets/0/parameters/{}", parameters[0].0), json!(0.25))],
        )
        .unwrap();
        assert!(!created.runtime_verified);
        assert_eq!(created.extension, ".softubepreset");
        assert_eq!(created.source_version.as_deref(), Some("2.4.44"));
        assert!(!created.bytes.starts_with(b"VST3"));
        let output = softube::Preset::read(&created.bytes).unwrap();
        assert_eq!(output.document["PresetList"].as_array().unwrap().len(), 1);
        assert_eq!(output.document["PresetList"][0]["PresetName"], "My new preset");
        assert_eq!(output.document["PresetList"][0]["Unknown"], "retain this");
        assert_eq!(output.document["SharedData"], template.document["SharedData"]);
        assert_ne!(output.document["PresetList"][0]["UUID"], template.document["PresetList"][1]["UUID"]);
        assert!(preset::edit_native(&format.info.plugin, &bytes, Some("Ambiguous bundle rename"), &[]).is_err());
        template.document["PresetList"][0]["PluginID"] = json!("wrong native product");
        assert!(preset::edit_native(&format.info.plugin, &template.write().unwrap(), None, &[]).is_err());
        let mut partial = output;
        partial.document["PresetList"][0]["ParameterData"]["Parameters"].as_array_mut().unwrap().pop();
        assert!(preset::edit_native(&format.info.plugin, &partial.write().unwrap(), None, &[]).is_err());
    }
}

#[test]
fn native_zynaptiq_actions_preserve_context_without_claiming_vst3_identity() {
    let plugin = "zynaptiq-wide-1";
    let format = structure::builtin(plugin).unwrap();
    let count = format.structure.survey["parameterCounts"][0].as_u64().unwrap();
    let parameters = (0..count).map(|id| format!("p{id}=\"0.25\"")).collect::<Vec<_>>().join(" ");
    let input = format!(
        "<Preset version=\"4\" progName=\"Synthetic\"><inputParams count=\"{count}\" {parameters}/><Unknown value=\"preserve\"/></Preset>"
    );
    let edits = [change("inputParams/@p1", json!(0.4))];
    let output = preset::create_native_from_template(plugin, input.as_bytes(), 0, "A wider preset", &edits).unwrap();
    assert!(!output.runtime_verified);
    assert_eq!(output.extension, ".preset");
    assert!(output.source_version.is_none());
    assert!(output.identity_note.contains("does not identify its product"));
    let decoded = zynaptiq::Preset::read(&output.bytes).unwrap();
    assert_eq!(decoded.parameter(1), Some(0.4));
    assert_eq!(decoded.document.root.attribute("progName"), Some("A wider preset"));
    assert_eq!(decoded.document.root.child("Unknown").unwrap().attribute("value"), Some("preserve"));
    let inspected = preset::inspect_fields(plugin, &output.bytes, None, Some("inputParams/@p1"), 1).unwrap();
    assert!(inspected["classId"].is_null());
    assert_eq!(inspected["fields"][0]["path"], "inputParams/@p1");
    assert_eq!(inspected["fields"][0]["value"], 0.4);
    assert!(preset::create_native_from_template(plugin, input.as_bytes(), 1, "Wrong index", &[]).is_err());
    assert!(preset::edit_native(plugin, input.replace("version=\"4\"", "version=\"3\"").as_bytes(), None, &[]).is_err());
    assert!(preset::edit_native(plugin, input.as_bytes(), None, &[change(&format!("inputParams/@p{count}"), json!(1))]).is_err());
    assert!(preset::edit(plugin, &output.bytes, &WriteOptions { version: "1.0.0".into(), ..Default::default() }, &[]).is_err());
}

#[test]
fn serum_requires_both_source_version_markers_before_writing_native_or_host_state() {
    let opts = options("serum-2");
    let mutate = |file: &mut serum2::XferFile, marker: &str, in_body: bool| {
        if in_body {
            let cbor::Value::Map(entries, _) = &mut file.body else { panic!("map expected") };
            entries.iter_mut().find(|(key, _)| key.as_str() == Some(marker)).unwrap().1 = if marker == "productVersion" {
                cbor::Value::text("2.0.11")
            } else {
                cbor::Value::Float(cbor::Float::Double(12.0f64.to_bits()))
            };
        } else {
            let mut header = file.header_json().unwrap();
            header[marker] = if marker == "productVersion" { json!("2.0.11") } else { json!(12) };
            file.header = serde_json::to_vec(&header).unwrap();
        }
    };
    for marker in ["productVersion", "version"] {
        for in_body in [false, true] {
            let mut native = serum2::SerumPreset::init("Version check").unwrap();
            mutate(&mut native.file, marker, in_body);
            assert!(preset::edit("serum-2", &native.file.write().unwrap(), &opts, &[]).is_err(), "native {marker} body={in_body}");
            for component in ["Comp", "Cont"] {
                let mut state = VstPreset::read(templates::initial("serum-2").unwrap()).unwrap();
                let bytes = &mut state.chunks.iter_mut().find(|(id, _)| id == component).unwrap().1;
                let mut file = serum2::XferFile::read(bytes).unwrap();
                mutate(&mut file, marker, in_body);
                *bytes = file.write().unwrap();
                assert!(preset::edit("serum-2", &state.write(), &opts, &[]).is_err(), "{component} {marker} body={in_body}");
            }
        }
    }
    assert!(preset::create("serum-2", &opts, &[change("productVersion", json!("2.0.11"))]).is_err());
    assert!(preset::create("serum-2", &opts, &[change("version", json!(12))]).is_err());
}

#[test]
fn vendor_source_versions_are_checked_without_disabling_read_only_inspection() {
    for plugin in ["valhalla-delay", "minimal-cluster-delay"] {
        let mut container = VstPreset::read(templates::initial(plugin).unwrap()).unwrap();
        let mut state = XmlState::read(container.component().unwrap()).unwrap();
        let (element, marker) = if plugin == "valhalla-delay" {
            (&mut state.document.root, "pluginVersion")
        } else {
            (state.document.root.children.iter_mut().find(|child| child.name == "Meta").unwrap(), "VERSION")
        };
        element.attributes.iter_mut().find(|(key, _)| key == marker).unwrap().1 = "0.0.1".into();
        let native = state.write_xml();
        container.chunks.iter_mut().find(|(id, _)| id == "Comp").unwrap().1 = state.write().unwrap();
        assert!(preset::inspect(plugin, &native, None).is_ok());
        assert!(preset::inspect(plugin, &container.write(), None).is_ok());
        assert!(preset::edit(plugin, &native, &options(plugin), &[]).is_err());
        assert!(preset::edit(plugin, &container.write(), &options(plugin), &[]).is_err());
    }
    let plugin = "valhalla-freq-echo";
    let mut old = VstPreset::read(templates::initial(plugin).unwrap()).unwrap();
    let bytes = &mut old.chunks.iter_mut().find(|(id, _)| id == "Comp").unwrap().1;
    let bank = 8 + u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    bytes[bank + 20..bank + 24].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    assert!(preset::inspect(plugin, &old.write(), None).is_ok());
    assert!(preset::edit(plugin, &old.write(), &options(plugin), &[]).is_err());
    assert!(preset::create("minimal-cluster-delay", &options("minimal-cluster-delay"), &[change("state/Meta/@VERSION", json!("0.0.1"))])
        .is_err());
}
