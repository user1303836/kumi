//! The routing tool, the same in every DAW: look (every way each track reaches the master, and what's likely wrong, in words
//! with the tracks' names) and build (a sidechain, a parallel bus or stems, as one plan), the graph read again after to check
//! the build did what it should. The DAW's backend reads the graph and writes the plan's steps.
use super::backend::{Build, RoutingBackend, Steps};
use super::graph::{Graph, Route};
use crate::core::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use crate::core::errors::RuntimeError;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use serde_json::{json, Value};
use std::rc::Rc;

pub const TOOL: &str = "routing";

pub struct Routing<B: RoutingBackend> {
    backend: B,
    description: String,
}

impl<B: RoutingBackend + 'static> Routing<B> {
    pub fn new(backend: B) -> Rc<Self> {
        let description = description(backend.words());
        Rc::new(Routing { backend, description })
    }

    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(RoutingTool { owner: self.clone() })
    }

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        match input.get("action").and_then(Value::as_str).unwrap_or("look") {
            "look" => self.look(&input, &signal).await,
            "build" => self.build(&input, &signal).await,
            _ => Err("action is look (the default) or build".into()),
        }
    }

    async fn look(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let graph = self.backend.graph(signal).await?;
        let lints = graph.lints();
        let mut reply = json!({
            "tracks": graph.nodes.len().saturating_sub(1),
            "routes": graph.edges.iter().filter(|edge| edge.route != Route::Parent).count(),
            "findings": lints.iter().map(|lint| json!({"kind": lint.kind, "says": lint.words, "refs": lint.tracks})).collect::<Vec<_>>(),
        });
        if let Some(track) = input.get("ref").and_then(Value::as_str) {
            let id = graph
                .nodes
                .keys()
                .find(|id| id.as_str() == track)
                .cloned()
                .ok_or_else(|| format!("{track} isn't a track of this project: read again"))?;
            let (paths, looped) = graph.paths(&id);
            reply["ways"] = json!(paths.iter().map(|path| graph.path_words(path)).collect::<Vec<_>>());
            if looped.is_some() {
                reply["loop"] = json!(true);
            }
        }
        reply["next"] = json!(if lints.iter().any(|lint| lint.kind != "parallel") {
            "say which way is meant and which is the accident, from the names and what the producer said; fix it with the send and parent-send changes, or build"
        } else {
            "nothing looks wrong: every track reaches the master once, or through an effect bus as meant"
        });
        Ok(reply)
    }

    async fn build(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let build = build_of(input)?;
        let before = self.backend.graph(signal).await?;
        let Steps { steps, mut said } = self.backend.steps(&before, &build, signal).await?;
        if steps.is_empty() {
            return Ok(json!({"made": null, "steps": 0, "said": said}));
        }
        let count = steps.len();
        let made = self.backend.run(steps, signal).await?;
        let after = self.backend.graph(signal).await?;
        let checked = check(&before, &after, &build);
        if shares_effects(&after, &build) {
            said.push("the effect returns (a shared reverb, a delay) stay shared: in no stem unless one is a stem of its own".into());
        }
        said.push(format!("one plan: {}'s own undo takes it all back", self.backend.words().app));
        let mut reply = json!({"made": made, "steps": count, "checked": checked, "said": said});
        if checked.iter().any(|line| line.ends_with(": no") || line.ends_with("too few")) {
            reply["next"] = json!("a check failed: read the routing (action look with ref) and say what's missing before anything else");
        }
        Ok(reply)
    }
}

fn build_of(input: &JsonObject) -> Result<Build, String> {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
    let list = |key: &str| -> Vec<String> {
        input.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
    };
    match input.get("kind").and_then(Value::as_str) {
        Some("sidechain") => Ok(Build::Sidechain {
            trigger: text("trigger").ok_or("trigger: the track whose sound drives the compressor (the kick)")?,
            target: text("target").ok_or("target: the track the compressor ducks (the bass)")?,
            compressor: text("compressor"),
        }),
        Some("parallel") => {
            let tracks = list("tracks");
            if tracks.is_empty() {
                return Err("tracks: the tracks sent to the parallel bus".into());
            }
            Ok(Build::Parallel {
                tracks,
                name: text("name").unwrap_or_else(|| "Parallel".into()),
                fx: text("fx"),
                pre_fader: input.get("preFader").and_then(Value::as_bool).unwrap_or(false),
            })
        }
        Some("stems") => {
            let groups: Vec<(String, Vec<String>)> = input
                .get("groups")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|group| {
                    let name = group.get("name")?.as_str()?.to_string();
                    let tracks: Vec<String> =
                        group.get("tracks")?.as_array()?.iter().filter_map(Value::as_str).map(str::to_owned).collect();
                    (!tracks.is_empty()).then_some((name, tracks))
                })
                .collect();
            if groups.is_empty() {
                return Err("groups: each stem's name and tracks".into());
            }
            Ok(Build::Stems { groups })
        }
        _ => Err("kind is sidechain, parallel or stems".into()),
    }
}

/// What the graph says after a build: each thing it should have, in words, and whether it holds.
fn check(before: &Graph, after: &Graph, build: &Build) -> Vec<String> {
    let name = |id: &str| after.nodes.get(id).map_or_else(|| id.to_string(), |node| format!("\u{201c}{}\u{201d}", node.name));
    match build {
        Build::Sidechain { trigger, target, .. } => {
            let routed =
                after.edges.iter().any(|edge| edge.from == *trigger && edge.to.as_deref() == Some(target) && edge.dst >= 3 && !edge.muted);
            let channels = after.nodes.get(target).map_or(0, |node| node.channels);
            let heard_before = before.paths(trigger).0.len();
            let heard_after = after.paths(trigger).0.len();
            vec![
                format!("{} reaches {}'s channels 3/4: {}", name(trigger), name(target), if routed { "yes" } else { "no" }),
                format!("{} has {channels} channels: {}", name(target), if channels >= 4 { "enough" } else { "too few" }),
                format!(
                    "{} is heard as before, the sidechain adding no way to the master: {}",
                    name(trigger),
                    if heard_after == heard_before { "yes" } else { "no" }
                ),
            ]
        }
        Build::Parallel { tracks, .. } => tracks
            .iter()
            .map(|track| {
                let more = after.paths(track).0.len() > before.paths(track).0.len();
                format!("{} reaches the master through the new bus too: {}", name(track), if more { "yes" } else { "no" })
            })
            .collect(),
        Build::Stems { groups } => groups
            .iter()
            .flat_map(|(stem, tracks)| {
                let bus = stem_of(before, after, stem, tracks);
                tracks.iter().map(move |track| {
                    let (paths, _) = after.paths(track);
                    // (a way through an effect return is shared, in no stem: only the others count)
                    let own: Vec<&Vec<usize>> = paths.iter().filter(|path| !after.through_effect(path)).collect();
                    let through =
                        own.iter().filter(|path| path.iter().any(|&k| after.edges[k].to.is_some() && after.edges[k].to == bus)).count();
                    format!(
                        "{} reaches the master once (its effect sends aside), through “{stem}”: {}",
                        name(track),
                        if own.len() == 1 && through == 1 { "yes" } else { "no" }
                    )
                })
            })
            .collect(),
    }
}

/// A stem's track after a build: the new track by its name (else one there by it), else the folder all of its tracks are in
/// (a group that was a folder already).
fn stem_of(before: &Graph, after: &Graph, stem: &str, tracks: &[String]) -> Option<String> {
    let named = |node: &&super::graph::Node| node.name == stem;
    let new = after.nodes.values().filter(named).find(|node| !before.nodes.contains_key(&node.id));
    let parent = tracks.first().and_then(|track| after.nodes.get(track)).and_then(|node| node.parent.clone());
    let folder =
        parent.filter(|parent| tracks.iter().all(|track| after.nodes.get(track).is_some_and(|node| node.parent.as_ref() == Some(parent))));
    new.or_else(|| after.nodes.values().find(named)).map(|node| node.id.clone()).or(folder)
}

/// Whether a stems build leaves effect returns shared: some track of a group still reaches the master through one.
fn shares_effects(after: &Graph, build: &Build) -> bool {
    let Build::Stems { groups } = build else { return false };
    groups.iter().flat_map(|(_, tracks)| tracks).any(|track| after.paths(track).0.iter().any(|path| after.through_effect(path)))
}

fn description(words: &super::backend::Words) -> String {
    format!(
        "Read {app}'s routing as a whole and build it. action look (the default) says every way each track reaches the master ({parent}, sends, hardware outputs) and what's likely wrong: a track heard twice through buses that only sum (louder than its fader says), a loop, a track heard nowhere or only through a hardware output, a sidechain landing on channels its track doesn't have, a duplicate with its polarity inverted (they cancel); with ref, that track's every way in words. Decide which way is meant and which is the accident from the names and what the producer said, and explain it with the tracks' names. action build makes one of these as one plan ({app}'s one undo takes it back) and reads the routing again to check it: kind sidechain (trigger: the kick, target: the bass, {compressor}; the target gets 4 channels and the trigger a send to its 3/4 that adds no way to the master), parallel (tracks sent to a new bus, name, fx to put on it, preFader), stems (groups: [{{name, tracks}}]: tracks of one folder next to each other become a folder by that name, others reach the master only through a bus by that name).",
        app = words.app,
        parent = words.parent,
        compressor = words.compressor
    )
}

fn schema() -> JsonObject {
    let value = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "action": {"type": "string", "enum": ["look", "build"]},
            "ref": {"type": "string", "description": "look: a track whose every way is said"},
            "kind": {"type": "string", "enum": ["sidechain", "parallel", "stems"]},
            "trigger": {"type": "string"},
            "target": {"type": "string"},
            "compressor": {"type": "string", "description": "An FX of the target's to drive"},
            "tracks": {"type": "array", "items": {"type": "string"}, "minItems": 1},
            "name": {"type": "string", "maxLength": 256},
            "fx": {"type": "string", "maxLength": 256},
            "preFader": {"type": "boolean"},
            "groups": {"type": "array", "minItems": 1, "items": {"type": "object", "additionalProperties": false, "required": ["name", "tracks"], "properties": {
                "name": {"type": "string", "minLength": 1, "maxLength": 256},
                "tracks": {"type": "array", "minItems": 1, "items": {"type": "string"}}
            }}}
        }
    });
    value.as_object().cloned().unwrap_or_default()
}

struct RoutingTool<B: RoutingBackend> {
    owner: Rc<Routing<B>>,
}

#[async_trait(?Send)]
impl<B: RoutingBackend + 'static> KernelTool for RoutingTool<B> {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        &self.owner.description
    }

    fn input_schema(&self) -> JsonObject {
        schema()
    }

    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(match self.owner.run(input, signal).await {
            Ok(reply) => ToolResult { text: stringify(&reply), ..Default::default() },
            Err(why) => ToolResult { text: why, is_error: true, ..Default::default() },
        })
    }

    fn reach(&self) -> Reach {
        Reach::DawWrite
    }

    fn deferred(&self) -> bool {
        true
    }
}
