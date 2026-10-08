//! Kumi's scratch-copies script run as Live's Python runs it, against a fake Live: a drop takes the copies and leaves
//! the copied track, in the same run of Live and after a restart (when every track's identity is new).
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
};

/// Runs a drop with `args` against tracks named `names`, whose identities are `ids`: the names left.
fn drop(names: &[&str], ids: &[&str], args: Value) -> Vec<String> {
    let fake = format!(
        r#"
import json
class Track:
    def __init__(self, name, ident):
        self.name, self.ident = name, ident
class Song:
    def __init__(self, tracks):
        self.tracks = tracks
    def delete_track(self, index):
        del self.tracks[index]
class Bridge:
    def _capture_object_identity(self, thing):
        return thing.ident
song = Song([Track(n, i) for n, i in zip({names}, {ids})])
bridge = Bridge()
obj = None
ARGS = json.loads({args})
{body}
print(json.dumps([str(t.name) for t in song.tracks]))
"#,
        names = serde_json::to_string(names).unwrap(),
        ids = serde_json::to_string(ids).unwrap(),
        args = serde_json::to_string(&args.to_string()).unwrap(),
        body = include_str!("../src/integrations/ableton/assets/tune-copies.py"),
    );
    let mut child = Command::new("python3").arg("-").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(fake.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn a_drop_takes_the_copies_and_never_the_copied_track() {
    let args = json!({"action":"drop","prefix":"Kumi · try ab12","before":["t1","t2"],"source":"Bass"});
    // The same run of Live: a copy never renamed (it still reads "Bass") goes too; the original stays by its identity.
    let left = drop(&["Drums", "Bass", "Kumi · try ab12 1", "Bass"], &["t1", "t2", "t3", "t4"], args.clone());
    assert_eq!(left, ["Drums", "Bass"]);
    // After Live restarted, every identity is new: only the named copies go, and Bass stays.
    let left = drop(&["Drums", "Bass", "Kumi · try ab12 1"], &["n1", "n2", "n3"], args);
    assert_eq!(left, ["Drums", "Bass"]);
}
