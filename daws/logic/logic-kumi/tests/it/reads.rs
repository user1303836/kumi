//! The original M2 native-reader replay over four recorded Logic scenes. Its 26 answers are compared against the
//! byte-preserved vector from d821e669, with no blessing or simulator substitution.

use std::path::PathBuf;

use logic_kumi::ax::strings::Strings;
use logic_kumi::ax::{paths, Node};
use logic_kumi::fixtures::{Scene, SceneHands};
use logic_kumi::helper::{Helper, HelperError, Launch};
use logic_kumi::scenes;
use serde_json::{json, Value};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn vectors_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logic-reads.json")
}

/// Only pane buttons within the Control Bar and the four tabs of a List Editors radio group can be pressed by
/// readers. Derive their addresses from this recording's labelled trees: Editors and list tabs move between scenes.
fn navigation_controls(scene: &Scene) -> Vec<(Value, Value, String)> {
    fn paths_in(node: &Node, strings: &Strings, out: &mut Vec<String>) {
        if node.role == "AXGroup" && strings.is(&node.desc, "Control Bar") {
            for label in ["Mixer", "Editors", "List Editors"] {
                if let Some(button) = paths::view_button(node, strings, label) {
                    out.push(button.path.clone());
                }
            }
        }
        let tabs: Vec<_> = node.children.iter().filter(|child| child.role == "AXRadioButton").collect();
        let labels = ["Event", "Marker", "Tempo", "Signature"];
        if node.role == "AXRadioGroup"
            && tabs.len() == labels.len()
            && labels.iter().all(|label| tabs.iter().filter(|tab| strings.is(&tab.desc, label)).count() == 1)
        {
            out.extend(tabs.iter().map(|tab| tab.path.clone()));
        }
        for child in &node.children {
            paths_in(child, strings, out);
        }
    }
    let strings = Strings::english();
    let mut controls = vec![];
    for call in scene.calls.iter().filter(|call| call.op == "tree") {
        let Some(node) = call.reply.get("node") else { continue };
        let mut paths = vec![];
        paths_in(&Node::from_json(node), &strings, &mut paths);
        for path in paths {
            let control = (call.params["pid"].clone(), call.params["root"].clone(), path);
            if !controls.contains(&control) {
                controls.push(control);
            }
        }
    }
    assert!(!controls.is_empty(), "{} records its pane and tab controls", scene.name);
    controls
}

fn assert_read_calls(calls: &[(String, Value)], controls: &[(Value, Value, String)]) {
    for (op, params) in calls {
        assert!(["tree", "get", "windows", "action"].contains(&op.as_str()), "a native read wrote through {op}: {params}");
        if op == "action" {
            assert_eq!(params["name"], "AXPress");
            let target = (params["pid"].clone(), params["root"].clone(), params["path"].as_str().unwrap().to_string());
            assert!(controls.contains(&target), "a read pressed outside the recording's labelled pane/tab controls: {params}");
        }
    }
}

/// The fixed request set over a scene: the cases, and the requests the scene couldn't answer.
fn run(name: &str) -> (Vec<Value>, Vec<(String, Value)>) {
    let dir = fixtures().join(name);
    let scene = Scene::load(&dir).unwrap_or_else(|error| panic!("no scene {name}: {error}; record it with the record_fixtures example"));
    let pid = scene.logic.get("pid").and_then(Value::as_u64).unwrap_or(1) as u32;
    let language = scene.logic.get("language").and_then(Value::as_str).unwrap_or("en");
    assert_eq!(language, "en", "the scenes are recorded from Logic in English");
    let project = scene.logic.get("project").cloned().unwrap_or(Value::Null);
    let (id, path, pname) = (
        project.get("id").and_then(Value::as_str).map(str::to_string),
        project.get("path").and_then(Value::as_str).map(str::to_string),
        project.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
    );
    let status = json!({"project": project, "busy": false, "permission": null});
    let hands = SceneHands::new(&scene);
    let mut helper = Launch::launch(&mut hands.clone()).unwrap();
    let cases = scenes::run(helper.as_mut(), pid, Strings::english(), status, (id.as_deref(), path.as_deref(), &pname));
    assert_read_calls(&hands.calls(), &navigation_controls(&scene));
    (cases, hands.unrecorded())
}

fn result(case: &Value) -> &Value {
    &case["response"]["result"]
}

/// Only the producer's recorded region-selection responses change in these derived scenarios. The original
/// recording and all navigation replies remain intact, so the test still refuses unrecorded native helper calls.
fn with_region_selection(reply: &mut Value, selected: bool, multiple: bool) {
    let lanes = reply["node"]["c"].as_array_mut().unwrap();
    let mut changed = 0;
    for lane in lanes {
        for region in lane.get_mut("c").and_then(Value::as_array_mut).into_iter().flatten() {
            if region["role"] == "AXLayoutItem" && region["help"].as_str().is_some_and(|help| help.contains("Region starts")) {
                assert_eq!(region["selected"], true, "the original recording's sole region is already selected");
                region["selected"] = selected.into();
                changed += 1;
            }
        }
        if multiple {
            if let Some(regions) = lane.get_mut("c").and_then(Value::as_array_mut).filter(|regions| !regions.is_empty()) {
                let mut extra = regions[0].clone();
                extra["i"] = "7.1.0.1.1.0.1.1".into();
                extra["desc"] = "Another region".into();
                regions.push(extra);
            }
        }
    }
    assert_eq!(changed, 1, "only the recorded region selection is adapted");
}

struct StoppedList {
    inner: Box<dyn Helper>,
    error: Option<HelperError>,
    scenario: &'static str,
    contents_reads: usize,
}

impl Helper for StoppedList {
    fn call(&mut self, op: &str, params: Value) -> Result<Value, HelperError> {
        if op == "tree" && params["path"] == "9" && params["depth"] == 7 {
            if let Some(error) = self.error.take() {
                return Err(error);
            }
        }
        let contents = op == "tree" && params["path"] == "7.1.0.1.1.0" && params["depth"] == 3;
        let mut reply = self.inner.call(op, params)?;
        if contents && self.scenario != "prime" {
            self.contents_reads += 1;
            with_region_selection(
                &mut reply,
                self.scenario != "unselected" && (self.scenario != "changed" || self.contents_reads < 4),
                self.scenario == "multiple",
            );
        }
        Ok(reply)
    }
    fn event(&mut self, wait: std::time::Duration) -> Option<Value> {
        self.inner.event(wait)
    }
    fn alive(&self) -> bool {
        self.inner.alive()
    }
    fn quit(&mut self) {
        self.inner.quit();
    }
}

#[test]
fn notes_never_select_a_region_on_success_refusal_helper_failure_or_cancellation() {
    use logic_kumi::{project::Project, serve};
    let original = Scene::load(&fixtures().join("scratch")).unwrap();
    let controls = navigation_controls(&original);
    for scenario in ["selected", "unselected", "multiple", "changed", "failed", "cancelled"] {
        for (method, params) in [
            ("read", json!({"kind":"note", "parent":"1:region:1"})),
            ("read", json!({"ref":"1:note:1.1"})),
            ("focus_body", json!({"kind":"notes", "ref":"1:region:1"})),
            ("focus_body", json!({"kind":"notes"})),
        ] {
            let pid = original.logic["pid"].as_u64().unwrap() as u32;
            let hands = SceneHands::new(&original);
            let inner = Launch::launch(&mut hands.clone()).unwrap();
            let error = match scenario {
                "failed" => Some(HelperError::Timeout),
                "cancelled" => Some(HelperError::Gone("the bridge is stopping".into())),
                _ => None,
            };
            // The multiple-selection scenario already has both regions when first observed. Adding a new region
            // after priming would correctly advance the epoch and make the explicit target ref stale instead.
            let mut helper =
                StoppedList { inner, error: None, scenario: if scenario == "multiple" { "multiple" } else { "prime" }, contents_reads: 0 };
            let mut project = Project::new(Strings::english());
            // Follow the original fixed request sequence through its first note read: the Mixer and list pane
            // recordings then describe the same state. This is not a substitute simulator for navigation.
            for (method, params) in scenes::fixed_requests().into_iter().take(8).skip(1) {
                let answer = serve::answer(&mut project, &mut helper, pid, method, &params);
                if params == json!({"kind":"note"}) {
                    assert_eq!(answer.unwrap_err().0, "bad-params");
                } else {
                    assert!(answer.is_ok(), "priming {method} {params}: {answer:?}");
                }
            }
            let primed_calls = hands.calls().len();
            helper.error = error;
            helper.scenario = scenario;
            helper.contents_reads = 0;
            let answer = serve::answer(&mut project, &mut helper, pid, method, &params);
            match scenario {
                "selected" => assert!(answer.is_ok(), "{method} {params}: {answer:?}"),
                "failed" | "cancelled" => assert!(answer.is_err(), "{scenario}: {answer:?}"),
                _ => {
                    let (code, message) = answer.unwrap_err();
                    assert_eq!(code, "failed", "{scenario} {method} {params}: {message}");
                    assert!(message.contains("select only that region") && message.contains("selection unchanged"), "{message}");
                }
            }
            let calls = hands.calls();
            assert_read_calls(&calls, &controls);
            // The only AX writes reachable by reads press the recorded pane/tab buttons. None targets a track,
            // region or event selection. Errors and stop cannot invoke a selection-restoration write either.
            if matches!(scenario, "unselected" | "multiple") {
                assert!(calls[primed_calls..].iter().all(|(op, _)| op != "action"), "an unselected notes refusal doesn't navigate panes");
            }
            assert!(hands.unrecorded().is_empty(), "{scenario} {method}: {:?}", hands.unrecorded());
        }
    }
}

#[test]
fn the_fixed_request_set_over_scratch_answers_in_the_simulators_shapes_and_is_the_vectors() {
    let (cases, unrecorded) = run("scratch");
    assert!(unrecorded.is_empty(), "the scene didn't record what the readers ask now; record it again: {unrecorded:?}");
    assert_eq!(cases.len(), 26);
    let by_request: Vec<(String, Value, Value)> = cases
        .iter()
        .map(|c| (c["request"]["method"].as_str().unwrap().to_string(), c["request"]["params"].clone(), c["response"].clone()))
        .collect();
    let find = |method: &str, params: Value| -> Value {
        by_request
            .iter()
            .find(|(m, p, _)| m == method && *p == params)
            .map(|(_, _, r)| r.clone())
            .unwrap_or_else(|| panic!("no case {method} {params}"))
    };

    let status = find("status", json!({}));
    assert_eq!(status["result"].as_object().unwrap().keys().collect::<Vec<_>>(), ["project", "busy", "permission"]);

    let project = find("read", json!({"kind": "project"}));
    let row = &project["result"]["items"][0];
    for key in ["id", "identity", "path", "tempo", "timeSignature", "keySignature", "sampleRate", "trackCount"] {
        assert!(row.get(key).is_some(), "project row lacks {key}");
    }
    assert_eq!(
        (row["identity"].as_str(), row["tempo"].as_f64(), row["timeSignature"].as_str()),
        (Some("Scratch"), Some(160.0), Some("4/4"))
    );
    assert_eq!((project["result"]["total"].as_u64(), project["result"]["epoch"].as_str()), (Some(1), Some("1")));

    let tracks = find("read", json!({"kind": "track"}));
    let items = tracks["result"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for key in
        ["ref", "index", "name", "kind", "mute", "solo", "armed", "monitoring", "volumeDb", "pan", "color", "output", "inputs", "selected"]
    {
        assert!(items[0].get(key).is_some(), "track row lacks {key}");
    }
    assert_eq!(
        (items[0]["ref"].as_str(), items[0]["name"].as_str(), items[0]["kind"].as_str(), items[0]["selected"].as_bool()),
        (Some("1:track:1"), Some("Vocal"), Some("audio"), Some(true))
    );
    assert_eq!(
        (items[1]["ref"].as_str(), items[1]["kind"].as_str(), items[1]["selected"].as_bool()),
        (Some("1:track:2"), Some("instrument"), Some(false))
    );
    let first = find("read", json!({"kind": "track", "limit": 1}));
    assert_eq!((first["result"]["items"].as_array().unwrap().len(), first["result"]["nextCursor"].as_str()), (1, Some("1.1")));
    let second = find("read", json!({"kind": "track", "cursor": "1.1"}));
    assert_eq!((second["result"]["items"][0]["ref"].as_str(), second["result"].get("nextCursor")), (Some("1:track:2"), None));

    let regions = find("read", json!({"kind": "region"}));
    let region = &regions["result"]["items"][0];
    for key in ["ref", "trackRef", "name", "startBeats", "endBeats", "kind", "selected", "looped"] {
        assert!(region.get(key).is_some(), "region row lacks {key}");
    }
    assert_eq!(
        (region["ref"].as_str(), region["trackRef"].as_str(), region["name"].as_str(), region["kind"].as_str()),
        (Some("1:region:1"), Some("1:track:2"), Some("Keys"), Some("midi"))
    );
    assert_eq!((region["startBeats"].as_f64(), region["endBeats"].as_f64()), (Some(0.0), Some(16.0)));
    assert_eq!(find("read", json!({"kind": "region", "parent": "1:track:2"}))["result"]["total"], 1);

    let refused = find("read", json!({"kind": "note"}));
    assert_eq!(refused, json!({"ok": false, "error": {"code": "bad-params", "message": "note: pass the parent region"}}));
    let notes = find("read", json!({"kind": "note", "parent": "1:region:1"}));
    let notes = notes["result"]["items"].as_array().unwrap();
    assert_eq!(notes.len(), 4);
    for key in ["ref", "regionRef", "position", "pitch", "velocity", "lengthTicks", "channel"] {
        assert!(notes[0].get(key).is_some(), "note row lacks {key}");
    }
    let seen: Vec<(u64, u64, u64, u64)> = notes
        .iter()
        .map(|n| {
            (
                n["position"]["beat"].as_u64().unwrap(),
                n["pitch"].as_u64().unwrap(),
                n["velocity"].as_u64().unwrap(),
                n["lengthTicks"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(seen, [(1, 60, 100, 240), (2, 64, 96, 240), (3, 67, 92, 240), (4, 72, 110, 480)]);
    assert_eq!(notes[0]["ref"], "1:note:1.1");

    let plugins = find("read", json!({"kind": "plugin", "parent": "1:track:1"}));
    let plugin = &plugins["result"]["items"][0];
    assert_eq!(
        (plugin["ref"].as_str(), plugin["slot"].as_u64(), plugin["name"].as_str(), plugin["bypassed"].as_bool(), plugin["kind"].as_str()),
        (Some("1:plugin:1.1"), Some(1), Some("Channel EQ"), Some(false), Some("effect"))
    );
    let sends = find("read", json!({"kind": "send", "parent": "1:track:1"}));
    assert!(sends["result"]["items"].is_array());

    let markers = find("read", json!({"kind": "marker"}));
    let marker = &markers["result"]["items"][0];
    for key in ["ref", "position", "name", "length", "text"] {
        assert!(marker.get(key).is_some(), "marker row lacks {key}");
    }
    assert_eq!((marker["position"]["bar"].as_u64(), marker["length"].as_f64()), (Some(9), Some(32.0)));

    let tempo = find("read", json!({"kind": "tempo"}));
    assert_eq!(tempo["result"]["items"][0]["bpm"], 160.0);
    assert_eq!(tempo["result"]["items"][0]["signatures"][0]["time"], "4/4");

    assert_eq!(find("read", json!({"ref": "1:track:2"}))["result"]["items"][0]["name"], "Keys");
    assert_eq!(
        find("read", json!({"ref": "1:track:9"})),
        json!({"ok": false, "error": {"code": "failed", "message": "1:track:9 isn't in the project (deleted, or another project)"}})
    );
    assert_eq!(find("read", json!({"kind": "bogus"}))["error"]["message"], "Logic Pro has no kind \"bogus\" to read");
    assert_eq!(
        find("read", json!({"kind": "region", "parent": "0:track:2"})),
        json!({"ok": false, "error": {"code": "stale", "message": "that ref is from an earlier epoch (0; now 1): read again"}})
    );

    let snapshot = find("snapshot", json!({}));
    assert_eq!(
        snapshot["result"].as_object().unwrap().keys().collect::<Vec<_>>(),
        ["epoch", "project", "transport", "tracks", "selection", "markers"]
    );
    assert_eq!(snapshot["result"]["selection"]["trackRef"], "1:track:1");
    let focus = find("focus", json!({}));
    for key in ["epoch", "view", "editor", "selection", "playhead", "transport", "changed", "ageMs"] {
        assert!(focus["result"].get(key).is_some(), "focus lacks {key}");
    }
    assert_eq!(
        (focus["result"]["view"].as_str(), focus["result"]["selection"]["trackRef"].as_str(), focus["result"]["ageMs"].as_u64()),
        (Some("tracks"), Some("1:track:1"), Some(0))
    );

    let tree = find("focus_body", json!({"kind": "tree", "ref": "1:track:1"}));
    assert_eq!(tree["result"]["strip"]["inserts"][0]["name"], "Channel EQ");
    assert_eq!(find("focus_body", json!({"kind": "tree"}))["result"]["trackRef"], "1:track:1");
    let body_notes = find("focus_body", json!({"kind": "notes", "ref": "1:region:1"}));
    assert_eq!((body_notes["result"]["total"].as_u64(), body_notes["result"]["notes"].as_array().unwrap().len()), (Some(4), 4));
    assert_eq!(find("focus_body", json!({"kind": "notes"}))["result"]["regionRef"], "1:region:1");
    let timeline = find("focus_body", json!({"kind": "timeline"}));
    assert_eq!(timeline["result"]["lanes"].as_array().unwrap().len(), 2);
    let around = find("focus_body", json!({"kind": "timeline", "around": "1:track:2", "window": {"from": 0, "to": 16}, "limit": 1}));
    assert_eq!((around["result"]["lanes"].as_array().unwrap().len(), around["result"]["lanes"][0]["name"].as_str()), (1, Some("Keys")));
    let routing = find("focus_body", json!({"kind": "routing", "ref": "1:track:1"}));
    assert_eq!(routing["result"]["inputs"], json!(["Input 1"]));

    let text = scenes::vectors_text(&cases);
    let current = std::fs::read_to_string(vectors_path()).expect("the original M2 vector is checked in");
    assert_eq!(current, text, "native reader answers differ from the original M2 vector; do not regenerate it");
}

#[test]
fn the_other_scenes_change_the_selection_the_transport_and_the_busy_state() {
    for (scene, check) in [
        (
            "scratch-keys",
            (|focus: &Value| {
                assert_eq!(focus["selection"]["trackRef"], "1:track:2", "{focus}");
                assert_eq!(focus["selection"]["regionRef"], "1:region:1", "{focus}");
                assert_eq!(focus["editor"], "pianoRoll", "{focus}");
            }) as fn(&Value),
        ),
        ("scratch-playing", |focus: &Value| {
            assert_eq!(focus["transport"]["playing"], true, "{focus}");
        }),
    ] {
        let (cases, unrecorded) = run(scene);
        assert!(unrecorded.is_empty(), "{scene}: {unrecorded:?}");
        let focus = cases.iter().find(|c| c["request"]["method"] == "focus").map(result).unwrap().clone();
        check(&focus);
    }
}

#[test]
fn the_original_project_chooser_is_refused_before_any_controls_behind_it_are_read() {
    use logic_kumi::{ax::Root, fixtures::Recorder, project::Project, serve};
    use std::sync::{Arc, Mutex};

    let scene = Scene::load(&fixtures().join("scratch-dialog")).unwrap();
    let pid = scene.logic["pid"].as_u64().unwrap() as u32;
    for (root, cached) in
        [(Root::Main, false), (Root::Title("Scratch.logicx - Tracks".into()), false), (Root::Title("Scratch.logicx - Tracks".into()), true)]
    {
        for (method, params) in [("read", json!({"kind":"project"})), ("snapshot", json!({})), ("focus_body", json!({"kind":"tree"}))] {
            let hands = SceneHands::new(&scene);
            let helper = Launch::launch(&mut hands.clone()).unwrap();
            let trace = Arc::new(Mutex::new(Scene::new("chooser requests")));
            let mut helper = Recorder::new(helper, trace.clone());
            let mut project = Project::new(Strings::english());
            project.set_root(root.clone());
            if cached {
                project.layout.control_bar = Some("the previous project's control bar".into());
            }
            let (code, message) = serve::answer(&mut project, &mut helper, pid, method, &params).unwrap_err();
            assert_eq!(code, "busy", "{method}, {root:?}, cached={cached}");
            assert!(message.contains("Choose a Project") && message.contains("nothing changed"), "{message}");
            let trace = trace.lock().unwrap();
            assert_eq!(trace.calls.len(), 1, "only the identifying main-window read ran: {:?}", trace.counts());
            assert_eq!(trace.calls[0].op, "tree");
            assert_eq!(trace.calls[0].params.get("root"), Some(&json!("main")));
            assert!(hands.unrecorded().is_empty(), "the refusal uses the original native recording unchanged");
        }
    }
}

#[test]
fn the_native_recordings_and_vector_keep_the_original_m2_source_bytes() {
    let source: Value = serde_json::from_str(include_str!("../fixtures/sources.json")).unwrap();
    assert_eq!(source["commit"], "d821e6690c06f9b983ca63ac98e37ba40eb50c7e");
    let files = source["files"].as_array().unwrap();
    assert_eq!(files.len(), 5);
    for file in files {
        let path = fixtures().join(file["path"].as_str().unwrap());
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(daw_bridge::auth::sha256_hex(&bytes), file["sha256"], "{}", path.display());
    }
}
