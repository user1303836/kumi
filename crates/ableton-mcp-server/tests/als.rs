use ableton_mcp_server::{
    als::*,
    project_semantic::{canonical_semantic_json, semantic_project_name},
};
use serde::Deserialize;
use serde_json::{json, Value};
fn fixture() -> Value {
    {
        let mut d = serde_json::Deserializer::from_str(include_str!("support/als_oracle.json"));
        d.disable_recursion_limit();
        Value::deserialize(&mut d).unwrap()
    }
}
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(o) => {
            let mut keys: Vec<_> = o.keys().collect();
            keys.sort();
            format!(
                "{{{}}}",
                keys.into_iter()
                    .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical(&o[k])))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        _ => kumi_common::js::json::stringify(value),
    }
}
fn equal(actual: Value, expected: &Value, label: &str) {
    assert_eq!(canonical(&actual), canonical(expected), "{label}");
}
#[test]
fn source_xml_model_midi_lint_oracle() {
    let fixture = fixture();
    for row in fixture["xml"].as_array().unwrap() {
        let actual = match parse_als_xml(row["xml"].as_str().unwrap()) {
            Ok(node) => json!({"ok":node}),
            Err(e) => json!({"error":e.to_string()}),
        };
        equal(actual, &row["result"], row["xml"].as_str().unwrap());
    }
    for row in fixture["cases"].as_array().unwrap() {
        let model = parse_als_xml(row["xml"].as_str().unwrap()).and_then(|root| model_from_als_xml(&root, "Fixture"));
        let label = row["label"].as_str().unwrap();
        match model {
            Ok(model) => {
                equal(json!({"ok":model}), &row["model"], label);
                equal(extract_als_midi(&model, None), &row["midi"], label);
                equal(extract_als_midi(&model, Some("strict")), &row["strictMidi"], label);
                equal(lint_als_model(&model, &AlsLintOptions::default()).unwrap(), &row["lint"], label);
            }
            Err(e) => equal(json!({"error":e.to_string()}), &row["model"], label),
        }
    }
}
#[test]
fn text_split_by_many_comments_parses_in_one_pass() {
    // 100,000 pieces of one text node: counting the text so far again for each made this about 5e9 steps.
    let xml = format!("<Ableton><Name>{}</Name></Ableton>", "x<!---->".repeat(100_000));
    let started = std::time::Instant::now();
    let root = parse_als_xml(&xml).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    assert_eq!(root.children[0].text.len(), 100_000);
    // The bound still holds: past 1 Mi units, the node is refused.
    let over = format!("<Ableton><Name>{}</Name></Ableton>", "xxxxxxxxxxxxxxxx<!---->".repeat(65_537));
    assert!(parse_als_xml(&over).unwrap_err().to_string().contains("text node exceeds the bounded size"));
}
#[test]
fn source_privacy_and_canonical_oracle() {
    let fixture = fixture();
    for row in fixture["names"].as_array().unwrap() {
        assert_eq!(
            semantic_project_name(row["profile"].as_str().unwrap(), "track", &row["value"]),
            row["result"].as_str().unwrap(),
            "{row}"
        );
    }
    for row in fixture["canonical"].as_array().unwrap() {
        let actual = match canonical_semantic_json(&row["value"]) {
            Ok(value) => json!({"ok":value}),
            Err(e) => json!({"error":e.to_string()}),
        };
        assert_eq!(actual, row["result"]);
    }
}
#[test]
fn offline_read_and_authorized_media_checks() {
    use flate2::{write::GzEncoder, Compression};
    use std::{fs, io::Write};
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    fs::create_dir(&allowed).unwrap();
    let outside = dir.path().join("allowed-sibling");
    fs::create_dir(&outside).unwrap();
    let xml =
        fixture()["cases"].as_array().unwrap().iter().find(|row| row["label"] == "audio").unwrap()["xml"].as_str().unwrap().to_owned();
    let path = allowed.join("Test.als");
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(xml.as_bytes()).unwrap();
    fs::write(&path, gzip.finish().unwrap()).unwrap();
    let (source, mut model) = read_als_model(path.to_str().unwrap()).unwrap();
    assert_eq!(source.xml, xml);
    assert_eq!(model.set_name, "Test");
    let options = AlsLintOptions {
        allowed_root: Some(allowed.to_string_lossy().into_owned()),
        set_directory: Some(allowed.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let missing = lint_als_model(&model, &options).unwrap();
    assert!(missing["findings"].as_array().unwrap().iter().any(|v| v["check"] == "missing-sample-reference"));
    model.tracks[0].clips[0].sample_path = Some(outside.join("missing.wav").to_string_lossy().into_owned());
    let sibling = lint_als_model(&model, &options).unwrap();
    assert!(!sibling["findings"].as_array().unwrap().iter().any(|v| v["check"] == "missing-sample-reference"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, allowed.join("linked")).unwrap();
        model.tracks[0].clips[0].sample_path = Some("linked/missing.wav".into());
        let links = lint_als_model(&model, &options).unwrap();
        assert!(!links["findings"].as_array().unwrap().iter().any(|v| v["check"] == "missing-sample-reference"));
    }
    let bounded = lint_als_model(&model, &AlsLintOptions { max_findings: Some(0.), ..Default::default() }).unwrap();
    assert_eq!(bounded, json!({"findings":[],"truncated":true}));
}
#[test]
fn xml_large_bounds() {
    assert_eq!(
        parse_als_xml(&format!("<Ableton>{}<X/>{}</Ableton>", "x".repeat(600_000), "y".repeat(600_000))).unwrap_err().to_string(),
        "Live Set XML text node exceeds the bounded size"
    );
    assert_eq!(
        parse_als_xml(&format!("<Ableton a=\"{}\"/>", "x".repeat(1_048_577))).unwrap_err().to_string(),
        "Live Set XML attribute exceeds the bounded size"
    );
    assert_eq!(
        parse_als_xml(&format!("<Ableton>{}</Ableton>", "<X/>".repeat(400_000))).unwrap_err().to_string(),
        "Live Set XML exceeds the bounded node count"
    );
}
