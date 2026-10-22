use ableton_mcp_server::{project_semantic::*, project_semantic_diff::*};
use serde::Deserialize;
use serde_json::{json, Value};
fn fixture() -> Value {
    let mut d = serde_json::Deserializer::from_str(include_str!("support/project_semantic_diff_oracle.json"));
    d.disable_recursion_limit();
    Value::deserialize(&mut d).unwrap()
}
fn result(value: Result<Value, ableton_mcp_server::project::ProjectError>) -> Value {
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
            a.chars().skip(prefix.saturating_sub(70)).take(250).collect::<String>(),
            e.chars().skip(prefix.saturating_sub(70)).take(250).collect::<String>()
        );
    }
}
#[test]
fn source_diff_workflows() {
    let f = fixture();
    for row in f["cases"].as_array().unwrap() {
        equal(&result(diff_semantic_project_snapshots(&row["before"], &row["after"])), &row["result"], row["label"].as_str().unwrap());
    }
}
#[test]
fn a_rename_to_a_name_with_a_spaced_slash_is_diffed_both_ways() {
    // "Kick / Snare" isn't a path: the exporter keeps it, so a diff that carries it passes the same test.
    let mut d = serde_json::Deserializer::from_str(include_str!("support/project_semantic_oracle.json"));
    d.disable_recursion_limit();
    let cases = Value::deserialize(&mut d).unwrap();
    let options: CreateSemanticProjectOptions = serde_json::from_value(cases["cases"][0]["options"].clone()).unwrap();
    let snapshot = cases["cases"][0]["snapshot"].clone();
    let mut renamed = snapshot.clone();
    renamed["tracks"][0]["name"] = json!("Kick / Snare");
    let before = create_semantic_project_snapshot(&snapshot, &options).unwrap();
    let after = create_semantic_project_snapshot(&renamed, &options).unwrap();
    for (from, to) in [(&before, &after), (&after, &before)] {
        let diff = diff_semantic_project_snapshots(from, to).unwrap();
        assert_eq!(diff["summary"]["changed"], true);
        assert!(canonical_semantic_json(&diff).unwrap().contains("Kick / Snare"), "{diff}");
    }
}
#[test]
fn source_diff_paging_and_validation() {
    let f = fixture();
    let diff = &f["cases"].as_array().unwrap().iter().find(|r| r["label"] == "many changes").unwrap()["result"]["ok"];
    for row in f["paging"].as_array().unwrap() {
        let mut options = SemanticPageOptions { limit: row["limit"].as_f64(), cursor: None };
        let mut pages = vec![];
        let out = loop {
            match page_semantic_project_diff(diff, &options) {
                Err(e) => break result(Err(e)),
                Ok(page) => {
                    options.cursor = page["page"]["nextCursor"].as_str().map(str::to_owned);
                    pages.push(page);
                    if options.cursor.is_none() {
                        break json!({"ok":pages});
                    }
                }
            }
        };
        equal(&out, &row["result"], "paging");
    }
    for row in f["cursors"].as_array().unwrap() {
        equal(
            &result(page_semantic_project_diff(
                diff,
                &SemanticPageOptions { limit: Some(3.), cursor: row["cursor"].as_str().map(str::to_owned) },
            )),
            &row["result"],
            "cursor",
        );
    }
    for row in f["mutations"].as_array().unwrap() {
        equal(
            &result(page_semantic_project_diff(&row["diff"], &SemanticPageOptions { limit: Some(3.), cursor: None })),
            &row["result"],
            row["label"].as_str().unwrap(),
        );
    }
}
#[test]
fn maximum_duplicate_ambiguity_source_oracle() {
    let f = fixture();
    let semantic: Value = serde_json::from_str(include_str!("support/project_semantic_oracle.json")).unwrap();
    let mut snapshot = semantic["cases"][0]["snapshot"].clone();
    let base = snapshot["tracks"][0].clone();
    let count = f["large"]["count"].as_u64().unwrap() as usize;
    snapshot["tracks"] = json!((0..count)
        .map(|i| {
            let mut track = base.clone();
            track["ref"] = json!(format!("track:{i}"));
            track["objectIdentity"] = json!(format!("identity:{i}"));
            track["name"] = json!("Duplicate");
            for key in ["clips", "clipSlots", "devices"] {
                track[key] = json!([]);
            }
            track
        })
        .collect::<Vec<_>>());
    let mut options: CreateSemanticProjectOptions = serde_json::from_value(semantic["cases"][0]["options"].clone()).unwrap();
    options.live.as_object_mut().unwrap().remove("registryHash");
    let artifact = create_semantic_project_snapshot(&snapshot, &options).unwrap();
    equal(&artifact["artifact"], &f["large"]["artifact"], "large artifact");
    // Independently allocated inputs exercise the same validation work as loaded artifacts.
    let after = artifact.clone();
    let start = std::time::Instant::now();
    let diff = diff_semantic_project_snapshots(&artifact, &after).unwrap();
    equal(&diff, &f["large"]["result"], "maximum duplicate ambiguity");
    let page = page_semantic_project_diff(&diff, &SemanticPageOptions { limit: Some(200.), cursor: None }).unwrap();
    assert!(canonical_semantic_json(&page).unwrap().len() < 512 * 1024);
    let elapsed = start.elapsed();
    // The source's 10-second bound is for an optimized build. A debug build on a busy CI runner gets twice that,
    // which still catches comparisons that stop using their index.
    let bound = if cfg!(debug_assertions) { 20. } else { 10. };
    assert!(elapsed.as_secs_f64() < bound, "indexed 11,995-record comparison exceeded its {bound}-second bound: {elapsed:?}");
}
