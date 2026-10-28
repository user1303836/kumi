use kumi_library::plugins::formats::{
    juce_xml::{self, XmlState},
    minimal_audio::MinimalAudioPreset,
    pulsar_modular::PulsarModularPreset,
    structure,
    valhalla::ValhallaPreset,
    vstpreset::VstPreset,
};
use serde_json::{json, Value};

fn fixture(plugin: &str) -> VstPreset {
    let source = structure::builtin(plugin).unwrap().info.format_directory();
    let path = std::path::Path::new(env!("KUMI_WORKSPACE")).join("plugin-formats").join(source).join("fixtures/init.vstpreset");
    VstPreset::read(&std::fs::read(path).unwrap()).unwrap()
}

fn binary_xml(xml: &str) -> Vec<u8> {
    let mut bytes = b"VC2!".to_vec();
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(xml.as_bytes());
    bytes.extend_from_slice(b"\0opaque private data\0\xff");
    bytes
}

#[test]
fn every_vendor_product_has_an_exact_init_roundtrip_and_a_recorded_runtime_edit() {
    let mut count = 0;
    for format in structure::builtins()
        .iter()
        .filter(|f| f.info.plugin.starts_with("valhalla-") || f.info.plugin.starts_with("minimal-") || f.info.plugin.starts_with("pulsar-"))
    {
        count += 1;
        let id = &format.info.plugin;
        let preset = fixture(id);
        assert!(format.has_id(&preset.class_id), "{id}: fixture belongs to another processor");
        assert!(format.info.ids.vst2.is_empty() && format.info.ids.au.is_empty() && format.info.ids.clap.is_empty());
        let path =
            std::path::Path::new(env!("KUMI_WORKSPACE")).join("plugin-formats").join(format.info.format_directory()).join("verified.json");
        let evidence: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let check = &evidence["checks"][0];
        let path = check["path"].as_str().unwrap();
        let value = &check["value"];
        let original = preset.component().unwrap();
        let (tree, changed, reread) = if id.starts_with("valhalla-") {
            let mut state = ValhallaPreset::read_state(original).unwrap();
            assert_eq!(state.write_state().unwrap(), original, "{id}");
            let tree = state.to_node();
            state.set(path, value).unwrap();
            let bytes = state.write_state().unwrap();
            let reread = ValhallaPreset::read_state(&bytes).unwrap().to_node();
            (tree, bytes, reread)
        } else if id.starts_with("minimal-") {
            let mut state = MinimalAudioPreset::read_state(original).unwrap();
            assert_eq!(state.write_state().unwrap(), original, "{id}");
            let tree = state.to_node();
            state.set(path, value).unwrap();
            let bytes = state.write_state().unwrap();
            let reread = MinimalAudioPreset::read_state(&bytes).unwrap().to_node();
            (tree, bytes, reread)
        } else {
            let mut state = PulsarModularPreset::read_state(original).unwrap();
            assert_eq!(state.write_state().unwrap(), original, "{id}");
            let tree = state.to_node();
            state.set(path, value).unwrap();
            let bytes = state.write_state().unwrap();
            let reread = PulsarModularPreset::read_state(&bytes).unwrap().to_node();
            (tree, bytes, reread)
        };
        assert!(format.structure.check(&tree).is_clean(), "{id}: fixture does not fit its structure");
        assert_ne!(changed, original, "{id}: the edit must change saved bytes");
        let field = path.strip_prefix("parameters/").unwrap();
        assert_eq!(reread.get("parameters").unwrap().get(field).unwrap().as_f64(), value.as_f64(), "{id}");
        assert!(!format.verified.writes.is_empty(), "{id}: missing runtime verification");
    }
    assert_eq!(count, 39);
}

#[test]
fn binary_xml_retains_private_tail_and_unknown_properties_after_editing() {
    let original = binary_xml("<ValhallaDelay Mix=\"0.5\" mystery=\"preserve &amp; keep\"><Unknown blob=\"data\"/></ValhallaDelay>");
    let mut state = ValhallaPreset::read_state(&original).unwrap();
    state.set("parameters/Mix", &json!(0.25)).unwrap();
    let output = state.write_state().unwrap();
    assert!(output.ends_with(b"\0opaque private data\0\xff"));
    let xml = XmlState::read(&output).unwrap();
    assert_eq!(xml.document.root.attribute("mystery"), Some("preserve & keep"));
    assert_eq!(xml.document.root.child("Unknown").unwrap().attribute("blob"), Some("data"));
    assert_eq!(xml.document.root.attribute("Mix"), Some("0.25"));
    let mut native = binary_xml("<ValhallaDelay Mix=\"0.5\"/>");
    native.extend_from_slice(b"native private payload");
    let mut native = XmlState::read(&native).unwrap();
    juce_xml::set_attribute(&mut native.document.root, "@Mix", "0.75").unwrap();
    let transplanted = native.to_state(&binary_xml("<ValhallaDelay Mix=\"0.5\"/>")).unwrap();
    assert!(transplanted.ends_with(b"native private payload"));
}

#[test]
fn binary_xml_rejects_truncation_and_inconsistent_lengths() {
    let original = binary_xml("<ValhallaDelay Mix=\"0.5\"/>");
    let declared = u32::from_le_bytes(original[4..8].try_into().unwrap()) as usize;
    for end in 0..8 + declared + 1 {
        assert!(XmlState::read(&original[..end]).is_err(), "accepted truncation at {end}");
    }
    let mut oversized = original.clone();
    oversized[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(XmlState::read(&oversized).is_err());
    let mut compatibility = fixture("valhalla-delay").component().unwrap().to_vec();
    compatibility[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(XmlState::read(&compatibility).is_err());
}

#[test]
fn minimal_audio_edits_the_authoritative_physical_value() {
    let xml = b"<ClusterDelay><App_State><Parameters><MKUClusterDelayNode_Main_InputGain unmapped_value=\"0.5\" mapped_value=\"-24\"/></Parameters><Other value=\"keep\"/></App_State></ClusterDelay>";
    let mut state = MinimalAudioPreset::read(xml).unwrap();
    state.set("parameters/MKUClusterDelayNode_Main_InputGain", &json!(-6)).unwrap();
    let output = XmlState::read(&state.write().unwrap()).unwrap();
    let gain =
        output.document.root.child("App_State").unwrap().child("Parameters").unwrap().child("MKUClusterDelayNode_Main_InputGain").unwrap();
    assert_eq!(gain.attribute("mapped_value"), Some("-6"));
    assert_eq!(gain.attribute("unmapped_value"), Some("0.5"));
    assert!(state.set("state/App_State/Parameters/MKUClusterDelayNode_Main_InputGain/@unmapped_value", &json!(0.8)).is_err());
    assert!(state.set_parameter("invented", 0.5).is_err());
    assert!(state.set_parameter("MKUClusterDelayNode_Main_InputGain", f64::NAN).is_err());
}

#[test]
fn pulsar_rejects_obsolete_control_ids_and_preserves_them() {
    let preset = fixture("pulsar-p11-abyss");
    let mut state = PulsarModularPreset::read_state(preset.component().unwrap()).unwrap();
    let original = XmlState::read(preset.component().unwrap()).unwrap();
    assert!(state.set_parameter("parammix", 0.3).is_err());
    state.set_parameter("mix", 0.26).unwrap();
    let updated = XmlState::read(&state.write_state().unwrap()).unwrap();
    let legacy =
        |state: &XmlState| state.document.root.children.iter().find(|element| element.attribute("id") == Some("parammix")).cloned();
    assert_eq!(legacy(&original), legacy(&updated));
}

#[test]
fn explicit_xml_paths_refuse_ambiguous_repeated_children_and_new_attributes() {
    let mut document = XmlState::read(b"<Root><Item value=\"1\"/><Item value=\"2\"/></Root>").unwrap();
    assert!(juce_xml::set_attribute(&mut document.document.root, "Item/@value", "3").is_err());
    assert!(juce_xml::set_attribute(&mut document.document.root, "Item[0]/@new", "3").is_err());
    juce_xml::set_attribute(&mut document.document.root, "Item[1]/@value", "3").unwrap();
    assert_eq!(document.document.root.children[0].attribute("value"), Some("1"));
    assert_eq!(document.document.root.children[1].attribute("value"), Some("3"));
    assert!(ValhallaPreset::read(b"<Unrelated Mix=\"0.5\"/>").is_err());
    let mut valhalla = ValhallaPreset::read(b"<ValhallaDelay Mix=\"0.5\"/>").unwrap();
    assert!(valhalla.set_parameter("Mix", 2.0).is_err());
    assert!(valhalla.set_parameter("nonexistent", 0.2).is_err());
}

#[test]
fn native_xml_converts_to_the_matching_product_component_envelope() {
    let init = fixture("valhalla-delay");
    let initialized = ValhallaPreset::read_state(init.component().unwrap()).unwrap();
    let mut native = ValhallaPreset::read(&initialized.write_preset()).unwrap();
    native.set_parameter("Mode", 0.125).unwrap();
    let component = native.to_state(init.component().unwrap()).unwrap();
    assert!(component.starts_with(b"VstW"));
    assert_eq!(ValhallaPreset::read_state(&component).unwrap().parameters().find(|(key, _)| *key == "Mode").unwrap().1, 0.125);
    let shimmer = fixture("valhalla-shimmer");
    let freq_echo = fixture("valhalla-freq-echo");
    let shimmer = ValhallaPreset::read_state(shimmer.component().unwrap()).unwrap();
    assert!(shimmer.to_state(freq_echo.component().unwrap()).is_err());
}

#[test]
fn decoded_tree_budgets_reject_small_inputs_with_excessive_expansion() {
    use kumi_library::plugins::formats::{bounded_json, cbor, xml, MAX_DECODED_DEPTH, MAX_DECODED_ITEMS};

    let json = format!("[{}0]", "0,".repeat(MAX_DECODED_ITEMS));
    assert!(bounded_json::parse(json.as_bytes()).unwrap_err().to_string().contains("item budget"));
    let mut cbor = vec![0x9f];
    cbor.extend(std::iter::repeat_n(0, MAX_DECODED_ITEMS));
    cbor.push(0xff);
    assert!(cbor::decode(&cbor).unwrap_err().to_string().contains("item budget"));
    let attributes = (0..MAX_DECODED_ITEMS / 2).map(|i| format!(" p{i}=\"0\"")).collect::<String>();
    assert!(xml::parse(&format!("<Preset{attributes}/>")).unwrap_err().to_string().contains("item budget"));
    let nodes = format!("<Root>{}</Root>", "<x/>".repeat(MAX_DECODED_ITEMS));
    assert!(xml::parse(&nodes).unwrap_err().to_string().contains("item budget"));
    let deep = format!("{}0{}", "[".repeat(MAX_DECODED_DEPTH + 2), "]".repeat(MAX_DECODED_DEPTH + 2));
    assert!(bounded_json::parse(deep.as_bytes()).is_err());
    assert!(bounded_json::parse(br#"{"same":1,"same":2}"#).is_err());
    assert!(xml::parse("<Root same=\"1\" same=\"2\"/>").is_err());
}

#[test]
fn numeric_writes_need_verified_intervals_and_cannot_use_raw_text_attributes() {
    let mut minimal = MinimalAudioPreset::read(
        b"<ClusterDelay><App_State><Parameters><FutureControl mapped_value=\"0.5\"/></Parameters></App_State></ClusterDelay>",
    )
    .unwrap();
    assert!(minimal.set_parameter("FutureControl", 0.75).unwrap_err().to_string().contains("Unknown numeric field"));
    assert!(minimal.set("state/App_State/Parameters/FutureControl/@mapped_value", &json!("999999999")).is_err());
    let mut pulsar = PulsarModularPreset::read(fixture("pulsar-p11-abyss").component().unwrap()).unwrap();
    assert!(pulsar.set("state/PARAM[0]/@value", &json!("999999999")).is_err());
    let unverified = structure::builtins()
        .iter()
        .flat_map(|format| format.structure.fields.iter().map(move |(path, field)| (format, path, field)))
        .find(|(_, _, field)| field.seen.is_some() && field.range_status != Some(structure::Status::Verified))
        .unwrap();
    assert!(structure::validate_number(&unverified.0.info.plugin, unverified.1, unverified.2.seen.unwrap()[0]).is_err());
}

fn range_evidence(plugin: &str, depth: usize) -> Value {
    assert!(depth < 8, "cyclic shared range evidence");
    let source = structure::builtin(plugin).unwrap().info.format_directory();
    let path = std::path::Path::new(env!("KUMI_WORKSPACE")).join("plugin-formats").join(source).join("range-verification.json");
    let mut value: Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))).unwrap();
    if let Some(parent) = value.as_object_mut().unwrap().remove("extends") {
        let mut base = range_evidence(parent.as_str().unwrap(), depth + 1);
        structure::merge_format_patch(&mut base, &value);
        value = base;
    }
    value
}

fn endpoint_bounds(record: &Value) -> [f64; 2] {
    assert!(record["parameterId"].as_u64().is_some(), "missing host control ID: {record}");
    let stored = record["stored"].as_array().expect("stored endpoint witnesses");
    assert_eq!(stored.len(), 3);
    assert!(stored.iter().all(|value| value.as_f64().is_some_and(f64::is_finite)));
    let mut bounds = [stored[0].as_f64().unwrap(), stored[1].as_f64().unwrap()];
    if bounds[0] > bounds[1] {
        bounds.swap(0, 1);
    }
    bounds
}

fn capture_covers_product(evidence: &Value, record: &Value) -> bool {
    record["captureIds"].as_array().is_some_and(|ids| {
        ids.iter().any(|id| {
            let Some(id) = id.as_str() else { return false };
            let capture = &evidence["captures"][id];
            capture["plugin"] == evidence["plugin"]
                && capture["version"] == evidence["version"]
                && capture["binarySha256"] == evidence["binarySha256"]
                && capture["binarySha256"]
                    .as_str()
                    .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        })
    })
}

#[test]
fn every_verified_numeric_interval_has_committed_endpoint_evidence() {
    let mut witnessed = 0;
    for format in structure::builtins() {
        let fields: Vec<_> =
            format.structure.fields.iter().filter(|(_, field)| field.range_status == Some(structure::Status::Verified)).collect();
        if fields.is_empty() {
            continue;
        }
        let plugin = &format.info.plugin;
        let evidence = range_evidence(plugin, 0);
        for (path, field) in fields {
            let bounds = field.range.expect("verified range needs bounds");
            assert!(bounds.iter().all(|n| n.is_finite()) && bounds[0] <= bounds[1]);
            match format.info.family.as_str() {
                "valhalla" | "minimal-audio" | "pulsar-modular" => {
                    let record = &evidence["fields"][path];
                    assert_eq!(endpoint_bounds(record), bounds, "{plugin}/{path}");
                    assert_eq!(evidence["plugin"], *plugin);
                    assert!(format.verified.writes.iter().any(|version| evidence["version"] == version.version));
                    if format.structure.range_scope_required {
                        assert!(field.range_products.contains(plugin), "{plugin}/{path}: missing product scope");
                        assert!(capture_covers_product(&evidence, record), "{plugin}/{path}: missing binary capture");
                    }
                    for (index, endpoint) in [0.0, 1.0].into_iter().enumerate() {
                        let readback = record["readback"][index].as_f64().expect("normalized host readback");
                        assert!((readback - endpoint).abs() <= 1e-6, "{plugin}/{path}: endpoint readback {readback}");
                    }
                    let id = record["parameterId"].as_u64().unwrap();
                    assert!(format.structure.parameters.iter().any(|(name, link)| link.field == *path
                        && link.status == structure::Status::Verified
                        && name.ends_with(&format!("[VST3 {id}]"))));
                }
                "serum-2" => {
                    let links: Vec<_> = format
                        .structure
                        .parameters
                        .values()
                        .filter(|link| link.field == *path && link.status == structure::Status::Verified)
                        .collect();
                    assert!(!links.is_empty(), "Serum range has no concrete verified control");
                    for link in links {
                        let record = &evidence["fields"][structure::Structure::location(link)];
                        assert_eq!(endpoint_bounds(record), bounds);
                        assert_eq!(record["readback"][0], 0.0);
                        assert_eq!(record["readback"][1], 1.0);
                        assert!(link.note.as_ref().unwrap().contains(&format!("parameter {}", record["parameterId"])));
                    }
                }
                "ozone-12" => {
                    let record = &evidence["fields"][path];
                    let observations = std::iter::once(record).chain(record["variants"].as_array().into_iter().flatten());
                    for observed in observations {
                        let endpoints = endpoint_bounds(observed);
                        assert!(
                            bounds[0] >= endpoints[0] && bounds[1] <= endpoints[1],
                            "{plugin}/{path}: interval exceeds a checked variant"
                        );
                        let normalized: Vec<_> =
                            observed["normalized"].as_array().unwrap().iter().map(|value| value.as_f64().unwrap()).collect();
                        assert_eq!(normalized, [0.0, 1.0, 0.25]);
                        assert!(format.has_id(observed["classId"].as_str().unwrap()));
                    }
                }
                "zynaptiq" => {
                    let record = evidence["controls"].as_array().unwrap().iter().find(|record| record["path"] == *path).unwrap();
                    assert_eq!(record["range"], json!(bounds));
                    assert_eq!(record["passed"], true);
                    assert_eq!(record["nativeEndpointsVerified"], true);
                    assert_eq!(record["nativeEndpointReadbacks"], json!(bounds));
                    assert_eq!(record["parameterId"].as_u64(), path.strip_prefix("inputParams/@p").and_then(|id| id.parse().ok()));
                }
                "superior-drummer" => {
                    for endpoint in bounds {
                        assert!(evidence["controls"].as_array().unwrap().iter().any(|record| record["path"] == *path
                            && record["requested"].as_f64() == Some(endpoint)
                            && record["readback"].as_f64() == Some(endpoint)
                            && record["passed"] == true));
                    }
                }
                family => panic!("verified range in {family} needs an evidence adapter"),
            }
            witnessed += 1;
        }
    }
    assert!(witnessed > 6000);
}

#[test]
fn a_parent_only_range_cannot_authorize_an_inheriting_product() {
    for (parent, child) in [("minimal-current", "minimal-eprom-memory-rites"), ("minimal-evoke", "minimal-lucid")] {
        let mut schema = serde_json::to_value(&structure::builtin(parent).unwrap().structure).unwrap();
        let mut proof = range_evidence(parent, 0);
        let path = "parameters/FutureParentOnlyControl";
        let capture = parent.strip_prefix("minimal-").unwrap();
        schema["fields"][path] = json!({
            "type": "float", "status": "guessed", "range": [0, 1], "range_status": "verified", "range_products": [parent]
        });
        proof["fields"][path] = json!({"parameterId": 123, "stored": [0, 1, 0.25], "readback": [0, 1, 0.25], "captureIds": [capture]});
        let parent_schema: structure::Structure = serde_json::from_value(schema.clone()).unwrap();
        assert!(parent_schema.validate_number(path, 0.5).is_ok());
        assert!(capture_covers_product(&proof, &proof["fields"][path]));

        // The same merge used for real shared formats preserves the new base
        // field, but it must not turn the parent's capture into child evidence.
        structure::merge_format_patch(&mut schema, &json!({"plugin": child}));
        let child_proof = range_evidence(child, 0);
        structure::merge_format_patch(
            &mut proof,
            &json!({
                "plugin": child, "version": child_proof["version"], "binarySha256": child_proof["binarySha256"]
            }),
        );
        let inherited: structure::Structure = serde_json::from_value(schema.clone()).unwrap();
        assert!(inherited.fields.contains_key(path));
        assert!(inherited.validate_number(path, 0.5).is_err());
        assert!(!capture_covers_product(&proof, &proof["fields"][path]));

        // Omitting coverage entirely must also fail closed for a shared schema.
        schema["fields"][path].as_object_mut().unwrap().remove("range_products");
        let missing: structure::Structure = serde_json::from_value(schema).unwrap();
        assert!(missing.validate_number(path, 0.5).is_err());
    }
}

#[test]
fn shared_minimal_formats_keep_exact_product_templates_and_zero_session_ids() {
    use kumi_library::plugins::templates;
    use sha2::{Digest, Sha256};
    for (parent, variant) in [("minimal-current", "minimal-eprom-memory-rites"), ("minimal-evoke", "minimal-lucid")] {
        let source = structure::builtin(variant).unwrap().info.format_directory();
        assert!(source.starts_with(&format!("{parent}/format-1/variants/")));
        assert_ne!(fixture(parent).class_id, fixture(variant).class_id);
    }
    for format in structure::builtins().iter().filter(|format| format.info.family == "minimal-audio") {
        let plugin = &format.info.plugin;
        let path = std::path::Path::new(env!("KUMI_WORKSPACE")).join("plugin-formats").join(format.info.format_directory());
        let bytes = std::fs::read(path.join("fixtures/init.vstpreset")).unwrap();
        assert_eq!(templates::initial(plugin).unwrap(), bytes, "build compression changed {plugin}");
        let proof: Value = serde_json::from_slice(&std::fs::read(path.join("fixture-verification.json")).unwrap()).unwrap();
        assert_eq!(proof["fixtureSha256"], format!("{:x}", Sha256::digest(&bytes)));
        let container = VstPreset::read(&bytes).unwrap();
        assert!(format.has_id(&container.class_id));
        let state = XmlState::read(container.component().unwrap()).unwrap();
        fn session_ids(element: &kumi_library::plugins::formats::xml::Element, count: &mut usize) {
            for (key, value) in &element.attributes {
                if key == "SESSION_ID" {
                    assert_eq!(value, "00000000000000000000000000000000");
                    *count += 1;
                }
            }
            for child in &element.children {
                session_ids(child, count);
            }
        }
        let mut count = 0;
        session_ids(&state.document.root, &mut count);
        assert_eq!(count, 1, "{plugin}");
        let native = MinimalAudioPreset::read(container.component().unwrap()).unwrap();
        for (name, value) in native.parameters() {
            assert_eq!(format.structure.fields[&format!("parameters/{name}")].default, Some(value), "{plugin}/{name}");
        }
    }
}
