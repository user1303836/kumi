//! REAPER's backend for routing (#401, the routing doctor and builder): only REAPER's facts. The graph from its track rows (a
//! track's send to its parent: its folder, or the master at the top) and send rows (read for the tracks with sends, and for
//! the tracks heard nowhere: their hardware outputs), with the FX of the tracks that receive (a bus with a reverb is a send
//! effect); and the steps that build a sidechain (ReaComp's detector on the sidechain), a parallel bus or stems, run as one
//! plan: one step of REAPER's undo. Core's tool does the rest.
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_runtime::core::plan::make_changes;
use kumi_runtime::core::refs::RefScheme;
use kumi_runtime::core::routing::backend::{Build, RoutingBackend, Steps, Words};
use kumi_runtime::core::routing::graph::{Edge, Graph, Node, Route, MASTER};
use kumi_runtime::integrations::own_changes::OwnPlan;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::rc::Rc;

pub static WORDS: Words = Words {
    app: "REAPER",
    parent: "the parent send: to its folder, or the master at the top",
    compressor: "compressor: an FX of the target's to switch to the sidechain (a ReaComp's detector; another compressor's own sidechain switch is the producer's), else a ReaComp is added after its FX",
};

/// Pages of rows read (200 a page).
const PAGES: usize = 20;
/// ReaComp's detector input (param 8, "SignIn"): 0 the main input, anything else the sidechain on 3/4; it reads 0 to 1084
/// over 0 to 1 (the 7.82 lab).
const DETECTOR: i64 = 8;
const SIDECHAIN: f64 = 1.0 / 1084.0;
/// A send this quiet is no way at all (dB).
const SILENT_DB: f64 = -120.0;

pub struct ReaperRouting<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
}

impl<S: RefScheme + 'static> ReaperRouting<S> {
    pub fn new(plan: Rc<OwnPlan<S>>) -> Self {
        ReaperRouting { plan }
    }

    fn short(&self, long: &str) -> String {
        self.plan.link().book.borrow_mut().short_ref(long)
    }

    /// A track's FX that sound (on, online, not its input FX): their names and refs.
    async fn fx_of(&self, long: &str, signal: &Signal) -> Result<(Vec<String>, Vec<String>), String> {
        let rows = self.plan.rows_through(json!({"kind": "fx", "parent": long, "limit": 200}), PAGES, signal).await?;
        Ok(rows
            .iter()
            .filter(|row| row.get("enabled") != Some(&Value::Bool(false)) && row.get("offline") != Some(&Value::Bool(true)))
            .filter(|row| row.get("input") != Some(&Value::Bool(true)))
            .filter_map(|row| Some((row.get("name")?.as_str()?.to_string(), self.short(row.get("ref")?.as_str()?))))
            .unzip())
    }

    /// A track's sends (and hardware outputs) as the graph's routes; only its hardware outputs when `hardware_only`.
    async fn add_sends(&self, graph: &mut Graph, long: &str, hardware_only: bool, signal: &Signal) -> Result<(), String> {
        let rows = self.plan.rows_through(json!({"kind": "send", "parent": long, "limit": 200}), PAGES, signal).await?;
        for edge in rows.iter().filter_map(|row| self.edge_of(row)) {
            if !hardware_only || edge.route == Route::Hardware {
                graph.edges.push(edge);
            }
        }
        Ok(())
    }

    /// A send row as a route (receives are the other track's sends, read there).
    fn edge_of(&self, row: &Value) -> Option<Edge> {
        let route = match row.get("direction")?.as_str()? {
            "send" => Route::Send,
            "hardware" => Route::Hardware,
            _ => return None,
        };
        let volume_db = row.get("volumeDb").and_then(Value::as_f64).unwrap_or(f64::NEG_INFINITY);
        let channels = row.get("channels");
        // (the first channel it lands on, from 0 in the row; none taken from the track: MIDI only)
        let dst = channels.and_then(|c| c.get("dst")).and_then(|d| d.get("first")).and_then(Value::as_u64).unwrap_or(0) as u32 + 1;
        Some(Edge {
            from: self.short(row.get("trackRef")?.as_str()?),
            to: if route == Route::Send { Some(self.short(row.get("destRef")?.as_str()?)) } else { None },
            route,
            dst,
            muted: row.get("mute") == Some(&Value::Bool(true)) || volume_db <= SILENT_DB,
            inverted: row.get("phaseInverted") == Some(&Value::Bool(true)),
            volume_db,
            mode: row.get("modeName").and_then(Value::as_str).unwrap_or("post-fader").to_string(),
            reference: row.get("ref").and_then(Value::as_str).map(|r| self.short(r)),
            midi_only: channels.and_then(|c| c.get("src")).is_some_and(Value::is_null),
        })
    }
}

fn reacomp(name: &str) -> bool {
    name.to_lowercase().contains("reacomp")
}

/// Whether these tracks (in their order) can become one folder: next to each other, of one folder (or none), each folder among
/// them with all its tracks, as REAPER's folder-of takes them.
fn foldable(graph: &Graph, nodes: &[&Node]) -> bool {
    let first = nodes[0].place;
    if nodes.iter().enumerate().any(|(n, node)| node.place != first + n) {
        return false;
    }
    let ids: Vec<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
    let inside = |node: &Node| node.parent.as_deref().is_some_and(|parent| ids.contains(&parent));
    let outer = &nodes[0].parent;
    nodes.iter().all(|node| node.parent == *outer || inside(node))
        && graph.nodes.values().all(|node| !inside(node) || ids.contains(&node.id.as_str()))
}

/// The folder these tracks are already, all of its tracks and no others: its name.
fn folder_already<'g>(graph: &'g Graph, nodes: &[&Node]) -> Option<&'g Node> {
    let parent = nodes[0].parent.as_deref()?;
    let ids: Vec<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
    let children: Vec<&str> =
        graph.nodes.values().filter(|node| node.parent.as_deref() == Some(parent)).map(|node| node.id.as_str()).collect();
    (nodes.iter().all(|node| node.parent.as_deref() == Some(parent)) && children.len() == ids.len())
        .then(|| graph.nodes.get(parent))
        .flatten()
}

/// A track of the graph to route (never the master).
fn track<'g>(graph: &'g Graph, id: &str) -> Result<&'g Node, String> {
    if id == MASTER {
        return Err("the master isn't routed so: name tracks".into());
    }
    graph.nodes.get(id).ok_or_else(|| format!("{id} isn't a track of this project: read again"))
}

/// A sidechain's steps: the target given 4 channels, the trigger sent pre-fader to its 3/4, and the compressor that listens:
/// the one named (`chosen`: its name and ref; a ReaComp's detector switched), else a ReaComp added after the target's FX.
fn sidechain(graph: &Graph, trigger: &str, target: &str, chosen: Option<(String, String)>) -> Result<Steps, String> {
    let (kick, bass) = (track(graph, trigger)?, track(graph, target)?);
    if trigger == target {
        return Err("trigger and target are one track".into());
    }
    let there = graph.edges.iter().any(|edge| {
        edge.from == trigger && edge.to.as_deref() == Some(target) && edge.route == Route::Send && edge.dst >= 3 && !edge.muted
    });
    if there {
        return Err(format!(
            "\u{201c}{}\u{201d} reaches \u{201c}{}\u{201d}'s channels 3/4 already: the sidechain is there",
            kick.name, bass.name
        ));
    }
    let mut out = Steps::default();
    if bass.channels < 4 {
        out.steps.push(json!({"tool": "track_channels", "input": {"ref": target, "channels": 4}}));
    }
    // (pre-fader: the kick's fader doesn't change how much the bass ducks)
    out.steps
        .push(json!({"tool": "send_add", "input": {"ref": trigger, "toRef": target, "mode": "pre-fader", "dstChannels": {"offset": 3}}}));
    match chosen {
        Some((name, fx)) if reacomp(&name) => {
            out.steps.push(json!({"tool": "fx_param", "input": {"ref": fx, "param": DETECTOR, "normalized": SIDECHAIN}}));
            out.said.push(format!(
                "\u{201c}{name}\u{201d} on \u{201c}{}\u{201d} now listens to \u{201c}{}\u{201d}, at the threshold and ratio it had",
                bass.name, kick.name
            ));
        }
        Some((name, _)) => out.said.push(format!(
            "\u{201c}{name}\u{201d} hears the sidechain once its own sidechain (external key) switch is on: the producer's, in its window"
        )),
        None => {
            out.steps.push(json!({"tool": "fx_add", "input": {"ref": target, "name": "ReaComp (Cockos)"}, "as": "comp"}));
            out.steps.push(json!({"tool": "fx_param", "input": {"ref": "@comp", "param": DETECTOR, "normalized": SIDECHAIN}}));
            out.said.push(format!(
                "a ReaComp was added after \u{201c}{}\u{201d}'s FX, its detector on the sidechain: its threshold and ratio set how far it ducks",
                bass.name
            ));
        }
    }
    Ok(out)
}

/// A parallel bus's steps: the bus, its FX, and a send to it from each track.
fn parallel(graph: &Graph, tracks: &[String], name: &str, fx: Option<&str>, pre_fader: bool) -> Result<Steps, String> {
    for id in tracks {
        track(graph, id)?;
    }
    let mut out = Steps::default();
    out.steps.push(json!({"tool": "track_add", "input": {"name": name}, "as": "bus"}));
    if let Some(fx) = fx {
        out.steps.push(json!({"tool": "fx_add", "input": {"ref": "@bus", "name": fx}}));
    }
    let mode = if pre_fader { "pre-fader" } else { "post-fader" };
    for id in tracks {
        out.steps.push(json!({"tool": "send_add", "input": {"ref": id, "toRef": "@bus", "mode": mode}}));
    }
    Ok(out)
}

/// Stems' steps: each group a folder where REAPER's folder-of takes its tracks (nothing when it's a folder already), else a
/// bus its tracks send to, their parent sends off.
fn stems(graph: &Graph, groups: &[(String, Vec<String>)]) -> Result<Steps, String> {
    let mut out = Steps::default();
    let mut seen: Vec<&String> = Vec::new();
    for (k, (stem, tracks)) in groups.iter().enumerate() {
        let mut nodes: Vec<&Node> = tracks.iter().map(|id| track(graph, id)).collect::<Result<_, _>>()?;
        if let Some(twice) = tracks.iter().find(|id| seen.contains(id)) {
            return Err(format!("\u{201c}{}\u{201d} is in two stems: one each", graph.nodes[twice].name));
        }
        seen.extend(tracks.iter());
        nodes.sort_by_key(|node| node.place);
        if let Some(folder) = folder_already(graph, &nodes) {
            out.said.push(format!(
                "\u{201c}{stem}\u{201d} is the folder \u{201c}{}\u{201d} already: nothing to build (rename it if the stem's name matters)",
                folder.name
            ));
        } else if foldable(graph, &nodes) {
            let refs: Vec<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
            out.steps.push(json!({"tool": "track_add", "input": {"name": stem, "folderOfRefs": refs}}));
        } else {
            let bus = format!("stem{}", k + 1);
            out.steps.push(json!({"tool": "track_add", "input": {"name": stem}, "as": bus}));
            for node in &nodes {
                out.steps.push(json!({"tool": "send_add", "input": {"ref": node.id, "toRef": format!("@{bus}"), "mode": "post-fader"}}));
                out.steps.push(json!({"tool": "track_main_send", "input": {"ref": node.id, "on": false}}));
                if let Some(folder) = node.parent.as_ref().and_then(|parent| graph.nodes.get(parent)) {
                    out.said.push(format!(
                        "\u{201c}{}\u{201d} leaves its folder \u{201c}{}\u{201d}: it reaches the master only through \u{201c}{stem}\u{201d}",
                        node.name, folder.name
                    ));
                }
            }
        }
    }
    Ok(out)
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> RoutingBackend for ReaperRouting<S> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn graph(&self, signal: &Signal) -> Result<Graph, String> {
        let rows = self.plan.rows_through(json!({"kind": "track", "limit": 200}), PAGES, signal).await?;
        let mut graph = Graph::default();
        let mut longs: BTreeMap<String, String> = BTreeMap::new();
        let (mut with_sends, mut buses) = (Vec::new(), Vec::new());
        for row in &rows {
            let Some(long) = row.get("ref").and_then(Value::as_str) else { continue };
            let count = |key: &str| row.get(key).and_then(Value::as_u64).unwrap_or(0);
            let channels = row.get("channels").and_then(Value::as_u64).unwrap_or(2) as u32;
            if row.get("master") == Some(&Value::Bool(true)) {
                graph.nodes.insert(MASTER.into(), Node { id: MASTER.into(), name: "Master".into(), channels, ..Node::default() });
                continue;
            }
            let id = self.short(long);
            let parent = row.get("parentRef").and_then(Value::as_str).map(|parent| self.short(parent));
            let main = row.get("mainSend");
            if main.and_then(|main| main.get("on")) == Some(&Value::Bool(true)) {
                graph.edges.push(Edge {
                    from: id.clone(),
                    to: Some(parent.clone().unwrap_or_else(|| MASTER.into())),
                    route: Route::Parent,
                    dst: main.and_then(|main| main.get("offset")).and_then(Value::as_u64).unwrap_or(1) as u32,
                    muted: false,
                    inverted: false,
                    volume_db: 0.0,
                    mode: "post-fader".into(),
                    reference: None,
                    midi_only: false,
                });
            }
            if count("sends") > 0 {
                with_sends.push(id.clone());
            }
            if count("receives") > 0 && count("fxCount") > 0 {
                buses.push(id.clone());
            }
            graph.nodes.insert(
                id.clone(),
                Node {
                    id: id.clone(),
                    name: row.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    place: count("index") as usize,
                    parent,
                    channels,
                    muted: row.get("mute") == Some(&Value::Bool(true)),
                    inverted: row.get("phaseInverted") == Some(&Value::Bool(true)),
                    sounds: count("itemCount") > 0,
                    ..Node::default()
                },
            );
            longs.insert(id, long.to_string());
        }
        graph.nodes.entry(MASTER.into()).or_insert_with(|| Node {
            id: MASTER.into(),
            name: "Master".into(),
            channels: 2,
            ..Node::default()
        });
        for id in &with_sends {
            self.add_sends(&mut graph, &longs[id], false, signal).await?;
        }
        for id in &buses {
            let (fx, refs) = self.fx_of(&longs[id], signal).await?;
            if let Some(node) = graph.nodes.get_mut(id) {
                node.fx = fx;
                node.fx_refs = refs;
            }
        }
        // (a track heard nowhere may go to a hardware output: read only theirs)
        let unheard: Vec<String> = graph
            .nodes
            .values()
            .filter(|node| graph.sounding(&node.id) && !node.muted && longs.contains_key(&node.id) && !with_sends.contains(&node.id))
            .filter(|node| graph.paths(&node.id).0.is_empty())
            .map(|node| node.id.clone())
            .collect();
        for id in &unheard {
            self.add_sends(&mut graph, &longs[id], true, signal).await?;
        }
        Ok(graph)
    }

    async fn steps(&self, graph: &Graph, build: &Build, signal: &Signal) -> Result<Steps, String> {
        match build {
            Build::Sidechain { trigger, target, compressor } => {
                // (a compressor already there levels the target: it's switched only when named, else a new one ducks)
                let chosen = match compressor {
                    Some(given) => {
                        let bass = track(graph, target)?;
                        let (names, refs) = self.fx_of(&self.plan.lengthen(target)?, signal).await?;
                        let at = refs
                            .iter()
                            .position(|fx| fx == given)
                            .ok_or_else(|| format!("{given} isn't an FX of \u{201c}{}\u{201d} that's on: read its FX again", bass.name))?;
                        Some((names[at].clone(), given.clone()))
                    }
                    None => None,
                };
                sidechain(graph, trigger, target, chosen)
            }
            Build::Parallel { tracks, name, fx, pre_fader } => parallel(graph, tracks, name, fx.as_deref(), *pre_fader),
            Build::Stems { groups } => stems(graph, groups),
        }
    }

    async fn run(&self, steps: Vec<Value>, signal: &Signal) -> Result<Value, String> {
        let input = json!({"steps": steps});
        let done = make_changes(&self.plan, input.as_object().cloned().unwrap_or_default(), signal.clone())
            .await
            .map_err(|error| error.to_string())?;
        if done.is_error {
            return Err(done.text);
        }
        Ok(serde_json::from_str(&done.text).unwrap_or_else(|_| json!({"said": done.text})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::CHANGES;
    use kumi_runtime::kernel::schema::problem;

    fn node(id: &str, place: usize, parent: Option<&str>) -> Node {
        Node { id: id.into(), name: id.into(), place, parent: parent.map(str::to_owned), channels: 2, sounds: true, ..Node::default() }
    }

    fn graph(nodes: Vec<Node>) -> Graph {
        Graph { nodes: nodes.into_iter().map(|node| (node.id.clone(), node)).collect(), edges: vec![] }
    }

    #[test]
    fn tracks_next_to_each_other_in_one_folder_fold_and_others_dont() {
        let g = graph(vec![
            node("Drums", 1, None),
            node("Kick", 2, Some("Drums")),
            node("Snare", 3, Some("Drums")),
            node("Bass", 4, None),
            node("Keys", 5, None),
        ]);
        let pick = |ids: &[&str]| -> Vec<&Node> { ids.iter().map(|id| &g.nodes[*id]).collect() };
        assert!(foldable(&g, &pick(&["Bass", "Keys"])));
        assert!(foldable(&g, &pick(&["Drums", "Kick", "Snare"])), "a folder with all its tracks");
        assert!(!foldable(&g, &pick(&["Snare", "Bass"])), "two folders");
        assert!(!foldable(&g, &pick(&["Kick", "Bass"])), "not next to each other");
        assert!(!foldable(&g, &pick(&["Drums", "Kick"])), "a folder without all its tracks");
        assert_eq!(folder_already(&g, &pick(&["Kick", "Snare"])).map(|f| f.name.as_str()), Some("Drums"));
        assert!(folder_already(&g, &pick(&["Kick"])).is_none());
    }

    #[test]
    fn a_sidechain_adds_its_own_reacomp_unless_one_is_named_and_says_what_it_switched() {
        let mut g = graph(vec![node("Kick", 1, None), node("Bass", 2, None)]);
        let added = sidechain(&g, "Kick", "Bass", None).unwrap();
        let tools: Vec<&str> = added.steps.iter().map(|step| step["tool"].as_str().unwrap()).collect();
        assert_eq!(tools, ["track_channels", "send_add", "fx_add", "fx_param"], "the bass's own compressor is left to level it");
        assert!(added.said[0].starts_with("a ReaComp was added after “Bass”'s FX"), "{:?}", added.said);
        let named = sidechain(&g, "Kick", "Bass", Some(("VST: ReaComp (Cockos)".into(), "f7".into()))).unwrap();
        assert_eq!(named.steps.last().unwrap()["input"]["ref"], "f7");
        assert_eq!(named.said, ["“VST: ReaComp (Cockos)” on “Bass” now listens to “Kick”, at the threshold and ratio it had"]);
        let other = sidechain(&g, "Kick", "Bass", Some(("VST3: Pro-C 2 (FabFilter)".into(), "f8".into()))).unwrap();
        assert_eq!(other.steps.len(), 2, "its own switch is the producer's");
        assert!(other.said[0].contains("its own sidechain (external key) switch"));
        // Four channels already: none given; the sidechain there already: refused.
        g.nodes.get_mut("Bass").unwrap().channels = 4;
        assert_eq!(sidechain(&g, "Kick", "Bass", None).unwrap().steps[0]["tool"], "send_add");
        g.edges.push(Edge {
            from: "Kick".into(),
            to: Some("Bass".into()),
            route: Route::Send,
            dst: 3,
            muted: false,
            inverted: false,
            volume_db: 0.0,
            mode: "pre-fader".into(),
            reference: None,
            midi_only: false,
        });
        assert!(sidechain(&g, "Kick", "Bass", None).unwrap_err().contains("already"));
        assert!(sidechain(&g, "Kick", MASTER, None).unwrap_err().contains("the master"));
    }

    #[test]
    fn stems_fold_what_folds_send_the_rest_and_build_nothing_for_a_folder_already() {
        let g = graph(vec![
            node("Drums", 1, None),
            node("Kick", 2, Some("Drums")),
            node("Snare", 3, Some("Drums")),
            node("Bass", 4, None),
            node("Keys", 5, None),
        ]);
        let groups = vec![
            ("Drum Stem".to_string(), vec!["Snare".to_string(), "Kick".to_string()]),
            ("Music".to_string(), vec!["Keys".to_string(), "Bass".to_string()]),
            ("Odd".to_string(), vec!["Snare".to_string()]),
        ];
        assert!(stems(&g, &groups).unwrap_err().contains("in two stems"));
        let built = stems(&g, &groups[..2]).unwrap();
        assert!(built.said[0].starts_with("“Drum Stem” is the folder “Drums” already"), "{:?}", built.said);
        assert_eq!(
            built.steps,
            [json!({"tool": "track_add", "input": {"name": "Music", "folderOfRefs": ["Bass", "Keys"]}})],
            "in their order"
        );
        // Apart, they go through a bus, out of their folder, said.
        let apart = stems(&g, &[("Hits".to_string(), vec!["Kick".to_string(), "Bass".to_string()])]).unwrap();
        let tools: Vec<&str> = apart.steps.iter().map(|step| step["tool"].as_str().unwrap()).collect();
        assert_eq!(tools, ["track_add", "send_add", "track_main_send", "send_add", "track_main_send"]);
        assert_eq!(apart.said, ["“Kick” leaves its folder “Drums”: it reaches the master only through “Hits”"]);
        let bus = parallel(&g, &["Kick".to_string()], "Crush", Some("ReaComp (Cockos)"), true).unwrap();
        assert_eq!(bus.steps[2]["input"], json!({"ref": "Kick", "toRef": "@bus", "mode": "pre-fader"}));
    }

    #[test]
    fn every_recipe_step_is_a_reaper_change_with_an_input_it_takes() {
        // (each step the recipes write, its input as written, checked against the kind's own schema)
        let steps = [
            json!({"tool": "track_channels", "input": {"ref": "t2", "channels": 4}}),
            json!({"tool": "send_add", "input": {"ref": "t1", "toRef": "t2", "mode": "pre-fader", "dstChannels": {"offset": 3}}}),
            json!({"tool": "fx_param", "input": {"ref": "f1", "param": DETECTOR, "normalized": SIDECHAIN}}),
            json!({"tool": "fx_add", "input": {"ref": "t2", "name": "ReaComp (Cockos)"}}),
            json!({"tool": "track_add", "input": {"name": "Parallel"}}),
            json!({"tool": "track_add", "input": {"name": "Drums", "folderOfRefs": ["t1", "t2"]}}),
            json!({"tool": "send_add", "input": {"ref": "t1", "toRef": "@bus", "mode": "post-fader"}}),
            json!({"tool": "track_main_send", "input": {"ref": "t1", "on": false}}),
        ];
        for step in steps {
            let kind = CHANGES.kinds.iter().find(|kind| kind.tool == step["tool"]).unwrap_or_else(|| panic!("{step}"));
            assert_eq!(problem(&Value::Object(kind.input_schema()), &step["input"]), None, "{step}");
        }
    }
}
