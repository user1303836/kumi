use ableton_mcp_server::{als::*, project::SetSourceRead, project_semantic::*};
use serde::Deserialize;
use serde_json::{json, Value};
fn fixture() -> Value {
    let mut d = serde_json::Deserializer::from_str(include_str!("support/project_semantic_oracle.json"));
    d.disable_recursion_limit();
    Value::deserialize(&mut d).unwrap()
}
fn result<T: serde::Serialize>(value: Result<T, ableton_mcp_server::project::ProjectError>) -> Value {
    match value {
        Ok(v) => json!({"ok":v}),
        Err(e) => json!({"error":e.to_string()}),
    }
}
fn equal(actual: &Value, expected: &Value, label: &str) {
    let a = canonical_semantic_json(actual).unwrap();
    let e = canonical_semantic_json(expected).unwrap();
    if a != e {
        let prefix = a.bytes().zip(e.bytes()).take_while(|(a, b)| a == b).count();
        panic!(
            "{label}: first difference near byte {prefix}\nactual {}\nexpected {}",
            a.chars().skip(prefix.saturating_sub(70)).take(200).collect::<String>(),
            e.chars().skip(prefix.saturating_sub(70)).take(200).collect::<String>()
        );
    }
}
#[test]
fn source_snapshot_and_offline_artifact_oracle() {
    let f = fixture();
    for row in f["cases"].as_array().unwrap() {
        let options: CreateSemanticProjectOptions = serde_json::from_value(row["options"].clone()).unwrap();
        let actual = result(create_semantic_project_snapshot(&row["snapshot"], &options));
        equal(&actual, &row["result"], row["label"].as_str().unwrap());
    }
    for row in f["offline"].as_array().unwrap() {
        let xml = row["xml"].as_str().unwrap();
        let model = model_from_als_xml(&parse_als_xml(xml).unwrap(), "Offline").unwrap();
        let source = SetSourceRead {
            path: "/tmp/Offline.als".into(),
            raw: b"fixture".to_vec(),
            xml: xml.into(),
            size: 7,
            mtime_ms: 0.,
            sha256: "c".repeat(64),
        };
        let options: OfflineAlsArtifactOptions = serde_json::from_value(row["options"].clone()).unwrap();
        equal(&result(create_offline_als_artifact(&source, &model, &options)), &row["result"], "offline");
    }
}
fn apply(value: &mut Value, path: &[Value], replacement: &Value, remove: bool) {
    let mut target = value;
    for key in &path[..path.len() - 1] {
        target = if let Some(index) = key.as_u64() { &mut target[index as usize] } else { &mut target[key.as_str().unwrap()] };
    }
    let key = &path[path.len() - 1];
    if remove {
        target.as_object_mut().unwrap().remove(key.as_str().unwrap());
    } else if let Some(index) = key.as_u64() {
        target[index as usize] = replacement.clone();
    } else {
        target[key.as_str().unwrap()] = replacement.clone();
    }
}
#[test]
fn source_validation_and_page_oracle() {
    let f = fixture();
    let artifact = &f["cases"][0]["result"]["ok"];
    for row in f["mutations"].as_array().unwrap() {
        let mut copy = artifact.clone();
        apply(&mut copy, row["path"].as_array().unwrap(), &row["value"], row["remove"].as_bool().unwrap());
        equal(&result(validate_semantic_project_artifact(&copy)), &row["result"], row["label"].as_str().unwrap());
    }
    for row in f["pages"].as_array().unwrap() {
        let mut options = SemanticPageOptions { limit: row["limit"].as_f64(), cursor: None };
        let mut pages = vec![];
        let out = loop {
            match page_semantic_project_snapshot(artifact, &options) {
                Err(e) => break result::<Value>(Err(e)),
                Ok(page) => {
                    options.cursor = page["page"]["nextCursor"].as_str().map(str::to_owned);
                    pages.push(page);
                    if options.cursor.is_none() {
                        break json!({"ok":pages});
                    }
                }
            }
        };
        equal(&out, &row["result"], "pages");
        if let Some(pages) = out["ok"].as_array() {
            equal(&assemble_semantic_project_pages(pages).unwrap(), artifact, "assembled");
        }
    }
    for row in f["pageMutations"].as_array().unwrap() {
        equal(&result(assemble_semantic_project_pages(row["pages"].as_array().unwrap())), &row["result"], row["label"].as_str().unwrap());
    }
    for row in f["cursors"].as_array().unwrap() {
        let cursor = row["cursor"].as_str().unwrap();
        assert_eq!(json!(semantic_cursor_artifact_id(cursor)), row["id"]);
        equal(
            &result(page_semantic_project_snapshot(artifact, &SemanticPageOptions { limit: Some(5.), cursor: Some(cursor.into()) })),
            &row["result"],
            cursor,
        );
    }
}
#[test]
fn rehashed_adversarial_and_large_paging_source_oracle() {
    let f = fixture();
    for row in f["rehashed"].as_array().unwrap() {
        equal(&result(validate_semantic_project_artifact(&row["artifact"])), &row["result"], row["label"].as_str().unwrap());
    }
    for row in f["large"].as_array().unwrap() {
        let mut snapshot = f["cases"][0]["snapshot"].clone();
        let base = snapshot["tracks"][0].clone();
        let count = row["count"].as_u64().unwrap() as usize;
        let long = row["long"].as_bool().unwrap();
        let tracks:Vec<_>=(0..count).map(|i|{let mut track=base.clone();track["ref"]=json!(format!("track:large-{i}"));track["name"]=json!(format!("Track {i}{}",if long{"x".repeat(500)}else{String::new()}));for key in ["clips","clipSlots","devices"]{track[key]=json!([]);}if long{track["routing"]=json!({"inputType":"a".repeat(512),"inputSubRouting":"b".repeat(512),"outputType":"c".repeat(512),"outputSubRouting":"d".repeat(512)});}track}).collect();
        snapshot["tracks"] = json!(tracks);
        let mut options: CreateSemanticProjectOptions = serde_json::from_value(f["cases"][0]["options"].clone()).unwrap();
        options.max_records = row["maxRecords"].as_f64();
        let artifact = create_semantic_project_snapshot(&snapshot, &options).unwrap();
        equal(&artifact["artifact"], &row["identity"], "large identity");
        equal(&artifact["manifest"], &row["manifest"], "large manifest");
        equal(
            &result(page_semantic_project_snapshot(&artifact, &SemanticPageOptions { limit: row["limit"].as_f64(), cursor: None })),
            &row["first"],
            "large page",
        );
    }
}

#[test]
fn canonical_boundaries_keep_source_validation_order() {
    let f = fixture();
    let cases: Vec<Value> = serde_json::from_str(include_str!("support/project_semantic_bounds_oracle.json")).unwrap();
    for row in cases {
        let count = row["count"].as_u64().unwrap() as usize;
        let value = match row["kind"].as_str().unwrap() {
            "ascii" => json!("x".repeat(count)),
            "astral" => json!("🎹".repeat(count)),
            "key" => json!({"x".repeat(count):true}),
            "object" => Value::Object((0..count).map(|i| (format!("field{i}"), json!(true))).collect()),
            "depth" => (0..count).fold(Value::Null, |value, _| json!([value])),
            _ => unreachable!(),
        };
        let mut artifact = f["cases"][0]["result"]["ok"].clone();
        artifact["records"][0]["data"]["extra"] = value;
        assert_eq!(json!(validate_semantic_project_artifact(&artifact).unwrap_err().to_string()), row["error"], "{row}");
    }
}

#[test]
fn source_platform_path_classification_and_locators() {
    let f = fixture();
    let paths: Value = serde_json::from_str(include_str!("support/project_semantic_paths_oracle.json")).unwrap();
    let platform = if cfg!(windows) { "win32" } else { "posix" };
    let options: CreateSemanticProjectOptions = serde_json::from_value(f["cases"][0]["options"].clone()).unwrap();
    for row in paths[platform].as_array().unwrap() {
        let mut snapshot = f["cases"][0]["snapshot"].clone();
        snapshot["tracks"][0]["clips"][0]["filePath"] = row["path"].clone();
        let actual = create_semantic_project_snapshot(&snapshot, &options).unwrap();
        let dependencies: Vec<_> = actual["records"].as_array().unwrap().iter().filter(|row| row["kind"] == "dependency").collect();
        let label = row["path"].as_str().unwrap();
        equal(&json!(dependencies), &row["dependencies"], label);
        equal(&actual["artifact"], &row["identity"], label);
    }
}
