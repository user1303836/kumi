//! The note_gap case.
use kumi_common::abort;
use kumi_runtime::core::gaps::*;
use serde_json::{json, Value};
#[tokio::test]
async fn note_gap_logs_missing_capabilities_privately_and_bounded_and_never_a_secret() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("gaps.jsonl");
    let tools = gap_tools(&file);
    let tool = &tools[0];
    assert_eq!(tool.name(), GAP_TOOL);
    let result = tool
        .execute(
            json!({"missing":"setting Operator's voice count","asked":"make the Reese mono","workaround":"Utility at 0% width"})
                .as_object()
                .unwrap()
                .clone(),
            abort::never(),
        )
        .await
        .unwrap();
    assert_eq!(result.reply.as_deref(), Some(""));
    let entry: Value = serde_json::from_str(std::fs::read_to_string(&file).unwrap().trim()).unwrap();
    assert_eq!(entry["missing"], "setting Operator's voice count");
    assert_eq!(entry["workaround"], "Utility at 0% width");
    assert!(regex::Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+").unwrap().is_match(entry["kumi"].as_str().unwrap()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(
        tool.execute(json!({"missing":"token: sk-abcdefghijklmnopqrstuvwxyz0123456789"}).as_object().unwrap().clone(), abort::never())
            .await
            .unwrap()
            .is_error
    );
    let lines = (0..1200).map(|i| json!({"missing":format!("gap {i} {}","x".repeat(250))}).to_string()).collect::<Vec<_>>().join("\n");
    std::fs::write(&file, format!("{lines}\n")).unwrap();
    tool.execute(json!({"missing":"the last one"}).as_object().unwrap().clone(), abort::never()).await.unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<_> = text.trim().split('\n').collect();
    assert_eq!(lines.len(), 500);
    assert!(lines.last().unwrap().contains("the last one"));
}
