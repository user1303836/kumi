//! The routing tool against a fake DAW (no DAW): what's likely wrong said with the tracks' names and refs, a track's every way,
//! a build run as one plan and checked on the graph read again after.
use super::backend::{Build, RoutingBackend, Steps, Words};
use super::graph::{Edge, Graph, Node, Route, MASTER};
use super::tool::Routing;
use crate::core::contracts::KernelTool;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

static WORDS: Words = Words { app: "Fake", parent: "its folder", compressor: "compressor: the target's" };

/// A project in a fake DAW: its graph, and what a run does to it (the routes it adds, the channels it gives).
#[derive(Default)]
struct Fake {
    graph: RefCell<Graph>,
    runs: RefCell<Vec<Vec<Value>>>,
    /// What a run adds to the graph.
    adds: RefCell<Vec<Edge>>,
    channels: RefCell<Option<(String, u32)>>,
}

#[async_trait(?Send)]
impl RoutingBackend for Rc<Fake> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn graph(&self, _signal: &Signal) -> Result<Graph, String> {
        Ok(self.graph.borrow().clone())
    }

    async fn steps(&self, _graph: &Graph, build: &Build, _signal: &Signal) -> Result<Steps, String> {
        Ok(match build {
            Build::Sidechain { trigger, target, .. } => Steps {
                steps: vec![json!({"tool": "send_add", "input": {"ref": trigger, "toRef": target, "dstChannels": {"offset": 3}}})],
                said: vec![],
            },
            Build::Stems { .. } => Steps { steps: vec![], said: vec!["the folder is that stem already".into()] },
            Build::Parallel { .. } => Steps { steps: vec![json!({"tool": "track_add", "input": {}})], said: vec![] },
        })
    }

    async fn run(&self, steps: Vec<Value>, _signal: &Signal) -> Result<Value, String> {
        self.runs.borrow_mut().push(steps);
        let mut graph = self.graph.borrow_mut();
        graph.edges.extend(self.adds.borrow().iter().cloned());
        if let Some((id, channels)) = self.channels.borrow().clone() {
            graph.nodes.get_mut(&id).unwrap().channels = channels;
        }
        Ok(json!({"changes": ["c1"]}))
    }
}

fn node(id: &str, name: &str) -> Node {
    Node { id: id.into(), name: name.into(), channels: 2, sounds: true, ..Node::default() }
}

fn route(from: &str, to: &str, kind: Route) -> Edge {
    Edge {
        from: from.into(),
        to: Some(to.into()),
        route: kind,
        dst: 1,
        muted: false,
        inverted: false,
        volume_db: -6.0,
        mode: "post-fader".into(),
        reference: None,
        midi_only: false,
    }
}

/// A kick in the Drums folder and sent to a Drum Bus that only sums (heard twice), a bass, a pad heard nowhere.
fn project() -> Rc<Fake> {
    let mut graph = Graph::default();
    for (id, name) in [("t1", "Drums"), ("t2", "Kick"), ("t3", "Drum Bus"), ("t4", "Bass"), ("t5", "Pad")] {
        graph.nodes.insert(id.into(), node(id, name));
    }
    graph.nodes.get_mut("t1").unwrap().sounds = false;
    graph.nodes.get_mut("t3").unwrap().sounds = false;
    graph.nodes.insert(MASTER.into(), Node { id: MASTER.into(), name: "Master".into(), channels: 2, ..Node::default() });
    graph.edges = vec![
        route("t2", "t1", Route::Parent),
        route("t1", MASTER, Route::Parent),
        route("t2", "t3", Route::Send),
        route("t3", MASTER, Route::Parent),
        route("t4", MASTER, Route::Parent),
    ];
    Rc::new(Fake { graph: RefCell::new(graph), ..Fake::default() })
}

async fn run(tool: &Rc<dyn KernelTool>, input: Value) -> Result<Value, String> {
    let done = tool.execute(input.as_object().cloned().unwrap(), Signal::new()).await.unwrap();
    if done.is_error {
        return Err(done.text);
    }
    Ok(serde_json::from_str(&done.text).unwrap())
}

#[tokio::test(flavor = "current_thread")]
async fn a_look_says_whats_likely_wrong_by_name_with_refs_and_a_tracks_every_way() {
    let fake = project();
    let tool = Routing::new(fake.clone()).tool();
    let look = run(&tool, json!({})).await.unwrap();
    let findings = look["findings"].as_array().unwrap();
    let kick = findings.iter().find(|f| f["kind"] == "double-path").unwrap();
    assert_eq!(kick["refs"], json!(["t2"]));
    assert!(kick["says"].as_str().unwrap().starts_with("\u{201c}Kick\u{201d} reaches the master 2 ways"), "{kick}");
    let pad = findings.iter().find(|f| f["kind"] == "silent").unwrap();
    assert_eq!(pad["refs"], json!(["t5"]));
    assert!(look["next"].as_str().unwrap().starts_with("say which way is meant"));
    assert_eq!(look["tracks"], 5);
    // A track's every way, in words.
    let ways = run(&tool, json!({"ref": "t2"})).await.unwrap();
    let ways: Vec<&str> = ways["ways"].as_array().unwrap().iter().map(|w| w.as_str().unwrap()).collect();
    assert!(ways.contains(&"into its folder \u{201c}Drums\u{201d}, then straight to the master"), "{ways:?}");
    assert!(ways.iter().any(|w| w.starts_with("by a send (-6 dB, post-fader) to \u{201c}Drum Bus\u{201d}")), "{ways:?}");
    assert!(run(&tool, json!({"ref": "t9"})).await.unwrap_err().contains("isn't a track"));
    assert!(fake.runs.borrow().is_empty(), "a look changes nothing");
}

#[tokio::test(flavor = "current_thread")]
async fn a_build_is_one_plan_checked_on_the_routing_read_again() {
    let fake = project();
    let tool = Routing::new(fake.clone()).tool();
    // The fake's run adds the sidechain send on 3/4 and gives the bass 4 channels: every check holds.
    fake.adds.borrow_mut().push(Edge { dst: 3, ..route("t2", "t4", Route::Send) });
    *fake.channels.borrow_mut() = Some(("t4".into(), 4));
    let built = run(&tool, json!({"action": "build", "kind": "sidechain", "trigger": "t2", "target": "t4"})).await.unwrap();
    assert_eq!(fake.runs.borrow().len(), 1, "one plan");
    assert_eq!(built["steps"], 1);
    let checked: Vec<&str> = built["checked"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
    assert_eq!(
        checked,
        [
            "\u{201c}Kick\u{201d} reaches \u{201c}Bass\u{201d}'s channels 3/4: yes",
            "\u{201c}Bass\u{201d} has 4 channels: enough",
            "\u{201c}Kick\u{201d} is heard as before, the sidechain adding no way to the master: yes",
        ]
    );
    assert!(built.get("next").is_none(), "{built}");
    assert!(built["said"].to_string().contains("Fake's own undo takes it all back"));

    // A run that left the bass with 2 channels fails its check, and says to look.
    let fake = project();
    let tool = Routing::new(fake.clone()).tool();
    fake.adds.borrow_mut().push(Edge { dst: 3, ..route("t2", "t4", Route::Send) });
    let built = run(&tool, json!({"action": "build", "kind": "sidechain", "trigger": "t2", "target": "t4"})).await.unwrap();
    assert!(built["checked"][1].as_str().unwrap().ends_with("too few"), "{built}");
    assert!(built["next"].as_str().unwrap().starts_with("a check failed"));

    // Nothing to build: no plan at all.
    let stems = run(&tool, json!({"action": "build", "kind": "stems", "groups": [{"name": "Drums", "tracks": ["t2"]}]})).await.unwrap();
    assert_eq!(stems["steps"], 0);
    assert_eq!(fake.runs.borrow().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_build_asks_for_what_it_needs() {
    let tool = Routing::new(project()).tool();
    let ask = |input: Value| {
        let tool = tool.clone();
        async move { run(&tool, input).await.unwrap_err() }
    };
    assert!(ask(json!({"action": "build"})).await.starts_with("kind is"));
    assert!(ask(json!({"action": "build", "kind": "sidechain", "target": "t4"})).await.starts_with("trigger:"));
    assert!(ask(json!({"action": "build", "kind": "parallel"})).await.starts_with("tracks:"));
    assert!(ask(json!({"action": "build", "kind": "stems", "groups": []})).await.starts_with("groups:"));
    assert!(ask(json!({"action": "rewire"})).await.starts_with("action is"));
}
