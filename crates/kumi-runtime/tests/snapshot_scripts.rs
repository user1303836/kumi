//! Kumi's snapshot script (history v0) run as Live's Python runs it, against a fake Live that behaves as probes of
//! Live 12.4.15b5 found (support/snapshot-live.py): clips a change deleted or cut come back whole; an audio clip comes
//! back at its length without cutting its neighbours; a Session clip's automation, groove and follow actions come
//! back; what Live won't put back is named; places are checked by track and scene; a big clip's notes come in more
//! calls, the last checking them all; nothing changes when a check fails, and a failure says how far it got.
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn clips_are_captured_and_made_again_in_a_fake_live() {
    let body = include_str!("../src/integrations/ableton/assets/snapshots.py");
    let source = format!("BODY = {}\n{}", serde_json::to_string(body).unwrap(), include_str!("support/snapshot-live.py"));
    let mut child = Command::new("python3").arg("-").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(source.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let scenarios: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(scenarios.len(), 15, "{scenarios:?}");
    for scenario in &scenarios {
        assert_eq!(scenario["problems"], json!([]), "{}", scenario["scenario"]);
    }
}

#[test]
fn the_script_carries_its_args_as_live_python_reads_them() {
    let args = json!({"op":"capture","clips":["1:arrangement_clip:0:0"]});
    let code = kumi_runtime::integrations::ableton::snapshots::script(&args);
    assert!(code.starts_with("# kumi:snapshots\nimport json\nARGS = json.loads("), "{}", &code[..80]);
    assert!(code.contains(r#"\"op\":\"capture\""#));
    assert!(code.ends_with(include_str!("../src/integrations/ableton/assets/snapshots.py")));
    // Within what python.run takes, with room for notes.
    assert!(code.len() < 28 * 1024, "{}", code.len());
}
