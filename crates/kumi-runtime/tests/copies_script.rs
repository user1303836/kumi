//! Kumi's scratch-copies script run as Live's Python runs it, against a fake Live: a search's own drop takes its copies
//! (one never renamed too) and leaves the copied track; a later sweep goes by the prefix alone and names a same-named
//! newcomer instead of taking it, in the same run of Live and after a restart (when every identity is new); a make
//! that fails takes back everything it added.
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
};

/// Runs the script with `args` against tracks named `names` with identities `ids` (each duplicate of the copied
/// track adds `per_copy` tracks): the names left, its result, and the error it raised.
fn live(names: &[&str], ids: &[&str], per_copy: usize, args: Value) -> Value {
    let fake = format!(
        r#"
import json
class Device:
    def __init__(self):
        self.canonical_parent = None
class Track:
    count = 0
    def __init__(self, name, ident):
        self.name, self.ident, self.is_foldable = name, ident, False
        self.devices = [Device()]
        self.devices[0].canonical_parent = self
class Song:
    def __init__(self, tracks):
        self.tracks = tracks
    def delete_track(self, index):
        del self.tracks[index]
    def duplicate_track(self, index):
        for k in range({per_copy}):
            Track.count += 1
            self.tracks.insert(index + 1, Track(self.tracks[index].name, 'copy%d' % Track.count))
class Bridge:
    def _capture_object_identity(self, thing):
        return thing.ident
song = Song([Track(n, i) for n, i in zip({names}, {ids})])
g = {{'song': song, 'bridge': Bridge(), 'obj': song.tracks[1].devices[0], 'ARGS': json.loads({args})}}
error = None
try:
    exec({body}, g)
except Exception as raised:
    error = str(raised)
print(json.dumps({{'left': [str(t.name) for t in song.tracks], 'result': g.get('result'), 'error': error}}))
"#,
        per_copy = per_copy,
        names = serde_json::to_string(names).unwrap(),
        ids = serde_json::to_string(ids).unwrap(),
        args = serde_json::to_string(&args.to_string()).unwrap(),
        body = serde_json::to_string(include_str!("../src/integrations/ableton/assets/tune-copies.py")).unwrap(),
    );
    let mut child = Command::new("python3").arg("-").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(fake.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn a_drop_takes_the_copies_and_never_the_producers_tracks() {
    let drop = |by_name: bool| json!({"action":"drop","prefix":"Kumi · try ab12","before":["t1","t2"],"source":"Bass","by_name":by_name});
    let tracks = ["Drums", "Bass", "Kumi · try ab12 1", "Bass"];
    // A search's own drop: a copy never renamed (it still reads "Bass") goes too; the original stays by its identity.
    let own = live(&tracks, &["t1", "t2", "t3", "t4"], 1, drop(true));
    assert_eq!(own["left"], json!(["Drums", "Bass"]));
    // A later sweep while Live runs: only the named copy goes, and the newcomer called "Bass" is named, not taken.
    let swept = live(&tracks, &["t1", "t2", "t3", "t4"], 1, drop(false));
    assert_eq!(swept["left"], json!(["Drums", "Bass", "Bass"]));
    assert_eq!(swept["result"]["newcomers"], json!(["Bass"]));
    // After Live restarted every identity is new: only the named copies go, and nothing is said of "Bass".
    let restarted = live(&["Drums", "Bass", "Kumi · try ab12 1"], &["n1", "n2", "n3"], 1, drop(false));
    assert_eq!(restarted["left"], json!(["Drums", "Bass"]));
    assert_eq!(restarted["result"]["newcomers"], json!([]));
}

#[test]
fn a_make_that_fails_takes_back_everything_it_added() {
    // A duplicate that comes out as two tracks: the make stops, and neither is left behind.
    let made = live(&["Drums", "Bass"], &["t1", "t2"], 2, json!({"action":"make","count":3,"prefix":"Kumi · try cd34","budget":12}));
    assert!(made["error"].as_str().is_some_and(|error| error.contains("2 tracks")), "{made}");
    assert_eq!(made["left"], json!(["Drums", "Bass"]));
    // A make that works names each copy after the prefix.
    let made = live(&["Drums", "Bass"], &["t1", "t2"], 1, json!({"action":"make","count":2,"prefix":"Kumi · try cd34","budget":12}));
    assert_eq!(made["result"]["names"], json!(["Kumi · try cd34 1", "Kumi · try cd34 2"]));
}
