use ableton_mcp_server::host::json_diagnostics::syntax_error_units;
use serde_json::Value;
#[test]
fn syntax_diagnostics_preserve_source_utf16_messages_positions_and_context() {
    let rows: Vec<Value> = serde_json::from_str(include_str!("fixtures/json-diagnostics-oracle.json")).unwrap();
    for row in rows {
        let units: Vec<u16> = row["units"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u16).collect();
        let expected = row["error"].as_array().map(|items| items.iter().map(|v| v.as_u64().unwrap() as u16).collect::<Vec<_>>());
        assert_eq!(syntax_error_units(&units), expected, "{}", String::from_utf16_lossy(&units));
    }
}
