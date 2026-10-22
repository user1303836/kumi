//! A project's routing as a graph, the same in every DAW: tracks (and the master), the routes between them (a track's send to
//! its parent, its sends, its hardware outputs), every way a track's audio reaches the master, and what's likely wrong (lints):
//! a track heard twice, a loop, a track heard nowhere, a sidechain landing where nothing listens, a duplicate that cancels.
use std::collections::BTreeMap;

/// The master's id in a graph.
pub const MASTER: &str = "master";

/// A track: its id, name, place in the DAW's list (from 1; 0 for the master), the folder it's in, how many channels it has,
/// whether it's muted or its polarity is inverted, the FX on it (names, and the DAW's refs: a bus with a reverb is a send
/// effect, one with none just sums; read where they matter), and whether it holds sound of its own.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub place: usize,
    pub parent: Option<String>,
    pub channels: u32,
    pub muted: bool,
    pub inverted: bool,
    pub fx: Vec<String>,
    pub fx_refs: Vec<String>,
    pub sounds: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The send to its parent (its folder, or the master at the top).
    Parent,
    Send,
    /// A hardware output: leaves the project.
    Hardware,
}

/// A route: from a track to another (none for hardware), the first channel it lands on (from 1), whether it's muted or
/// inverted, its level, and where it taps the track.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: String,
    pub to: Option<String>,
    pub route: Route,
    pub dst: u32,
    pub muted: bool,
    pub inverted: bool,
    pub volume_db: f64,
    pub mode: String,
    /// The DAW's ref of the send (none for a parent send).
    pub reference: Option<String>,
    /// It carries MIDI only.
    pub midi_only: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Graph {
    pub nodes: BTreeMap<String, Node>,
    pub edges: Vec<Edge>,
}

/// One way a track's audio reaches the master: the routes, in order.
pub type Path = Vec<usize>;

/// The most ways counted per track, and the longest way followed.
const MOST_PATHS: usize = 16;
const DEEPEST: usize = 32;

impl Graph {
    fn name(&self, id: &str) -> String {
        if id == MASTER {
            return "the master".into();
        }
        self.nodes.get(id).map_or_else(|| id.to_string(), |node| quoted(&node.name))
    }

    /// Whether a route carries what's heard: unmuted audio, onto the first channels (a sidechain's 3/4 is a detector's, not a
    /// path to the ears).
    fn audible(&self, edge: &Edge) -> bool {
        !edge.muted && !edge.midi_only && edge.route != Route::Hardware && edge.dst <= 2
    }

    /// Every audible way from `from` to the master (at most `MOST_PATHS`), and whether a loop was met on the way.
    pub fn paths(&self, from: &str) -> (Vec<Path>, Option<Vec<String>>) {
        let mut found = Vec::new();
        let mut looped = None;
        let mut stack: Vec<(String, Path, Vec<String>)> = vec![(from.to_string(), Vec::new(), vec![from.to_string()])];
        while let Some((at, path, seen)) = stack.pop() {
            if found.len() >= MOST_PATHS {
                break;
            }
            for (k, edge) in self.edges.iter().enumerate().filter(|(_, edge)| edge.from == at) {
                if !self.audible(edge) {
                    continue;
                }
                let Some(to) = &edge.to else { continue };
                let mut next = path.clone();
                next.push(k);
                if to == MASTER {
                    found.push(next);
                } else if seen.contains(to) {
                    let mut cycle = seen.clone();
                    cycle.push(to.clone());
                    looped.get_or_insert(cycle);
                } else if next.len() < DEEPEST && !self.nodes.get(to).is_some_and(|node| node.muted) {
                    let mut seen = seen.clone();
                    seen.push(to.clone());
                    stack.push((to.clone(), next, seen));
                }
            }
        }
        (found, looped)
    }

    /// A way in words: "through “Drums” (its folder) to the master".
    pub fn path_words(&self, path: &Path) -> String {
        let mut words = Vec::new();
        for &k in path {
            let edge = &self.edges[k];
            let to = edge.to.as_deref().unwrap_or("");
            let how = match edge.route {
                Route::Parent if to == MASTER => "straight to the master".to_string(),
                Route::Parent => format!("into its folder {}", self.name(to)),
                Route::Send => {
                    let fx = self
                        .nodes
                        .get(to)
                        .filter(|node| !node.fx.is_empty())
                        .map(|node| format!(" ({})", node.fx.join(", ")))
                        .unwrap_or_default();
                    format!("by a send ({} dB, {}) to {}{fx}", round(edge.volume_db), edge.mode, self.name(to))
                }
                Route::Hardware => "to a hardware output".to_string(),
            };
            words.push(how);
        }
        words.join(", then ")
    }

    /// What's likely wrong, a line each, the tracks it names first.
    pub fn lints(&self) -> Vec<Lint> {
        let mut out = Vec::new();
        let mut loops_said: Vec<Vec<String>> = Vec::new();
        for node in self.nodes.values().filter(|node| node.id != MASTER) {
            let (paths, looped) = self.paths(&node.id);
            if let Some(cycle) = looped {
                let mut sorted = cycle.clone();
                sorted.sort();
                sorted.dedup();
                if !loops_said.contains(&sorted) {
                    loops_said.push(sorted);
                    let names: Vec<String> = cycle.iter().map(|id| self.name(id)).collect();
                    out.push(Lint { tracks: vec![node.id.clone()], kind: "feedback", words: format!("a loop: {}", names.join(" → ")) });
                }
            }
            let sends_out: Vec<&Edge> = self.edges.iter().filter(|edge| edge.from == node.id).collect();
            if paths.is_empty() && self.sounding(&node.id) && !node.muted {
                let hardware = sends_out.iter().any(|edge| edge.route == Route::Hardware && !edge.muted);
                let midi = sends_out.iter().any(|edge| edge.midi_only && !edge.muted);
                let (ends, through_muted) = self.dead_ends(&node.id);
                // (a way into a track the producer muted is silent as meant; a MIDI send's track sounds elsewhere)
                if !hardware && (midi || through_muted) {
                    continue;
                }
                let mut words = if hardware {
                    format!("{} goes only to a hardware output: the master (and a render of it) never hears it", self.name(&node.id))
                } else if ends.iter().all(|end| *end == node.id) {
                    format!("{} is heard nowhere: its send to its parent is off and no send of it reaches the master", self.name(&node.id))
                } else {
                    let names: Vec<String> = ends.iter().filter(|end| **end != node.id).map(|end| self.name(end)).collect();
                    format!(
                        "{} is heard nowhere: its way ends at {}, which reaches neither its parent nor the master",
                        self.name(&node.id),
                        names.join(", ")
                    )
                };
                // (a bus fed by sends: "my reverb does nothing", the sends into it named)
                let senders = self.senders(&node.id);
                if !senders.is_empty() {
                    let names: Vec<String> = senders.iter().map(|id| self.name(id)).collect();
                    words.push_str(&format!("; the sends of {} into it do nothing", names.join(", ")));
                }
                let mut tracks = vec![node.id.clone()];
                tracks.extend(senders);
                out.push(Lint { tracks, kind: if hardware { "hardware-only" } else { "silent" }, words });
            }
            if paths.len() >= 2 {
                let ways: Vec<String> = paths.iter().map(|path| self.path_words(path)).collect();
                // (a way through a bus with a time effect is a send effect, as meant; ways through buses that only sum are the
                // same sound twice, unless one is a parallel bus meant so)
                let plain = paths.iter().filter(|path| !self.through_effect(path)).count();
                let inverted = paths.iter().any(|path| self.inverted(path)) && paths.iter().any(|path| !self.inverted(path));
                let (kind, words) = if inverted {
                    (
                        "inverted-duplicate",
                        format!(
                            "{} reaches the master {} ways, one with its polarity inverted: they cancel ({})",
                            self.name(&node.id),
                            paths.len(),
                            ways.join("; ")
                        ),
                    )
                } else if plain >= 2 {
                    ("double-path", format!("{} reaches the master {} ways without a send effect between, so it plays louder than its fader says (+{:.0} dB for {plain} at one level), unless one is a parallel bus meant so: {}", self.name(&node.id), paths.len(), 20. * (plain as f64).log10(), ways.join("; ")))
                } else {
                    (
                        "parallel",
                        format!(
                            "{} reaches the master {} ways, through an effect bus: as a send effect is meant to ({})",
                            self.name(&node.id),
                            paths.len(),
                            ways.join("; ")
                        ),
                    )
                };
                out.push(Lint { tracks: vec![node.id.clone()], kind, words });
            }
            for edge in sends_out.iter().filter(|edge| edge.route == Route::Send && !edge.muted && !edge.midi_only) {
                let Some(to) = edge.to.as_deref() else { continue };
                let Some(target) = self.nodes.get(to) else { continue };
                if edge.dst > 2 && edge.dst + 1 > target.channels {
                    out.push(Lint {
                        tracks: vec![node.id.clone(), to.to_string()],
                        kind: "sidechain-channels",
                        words: format!(
                            "{}'s send lands on channels {}/{} of {}, which has {} channels: nothing arrives (give it {} channels)",
                            self.name(&node.id),
                            edge.dst,
                            edge.dst + 1,
                            self.name(to),
                            target.channels,
                            edge.dst + 1
                        ),
                    });
                }
            }
        }
        out
    }

    /// The tracks whose unmuted sends (audio or MIDI) come into this one.
    pub fn senders(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for edge in self.edges.iter().filter(|edge| edge.route == Route::Send && !edge.muted && edge.to.as_deref() == Some(id)) {
            if !out.contains(&edge.from) {
                out.push(edge.from.clone());
            }
        }
        out
    }

    /// Whether a track has sound to lose: its own, or what other tracks send it.
    pub fn sounding(&self, id: &str) -> bool {
        self.nodes.get(id).is_some_and(|node| node.sounds) || !self.senders(id).is_empty()
    }

    /// Where a track's audible ways stop short of the master (it alone when it has none), and whether one runs into a muted
    /// track.
    fn dead_ends(&self, from: &str) -> (Vec<String>, bool) {
        let mut ends = Vec::new();
        let mut through_muted = false;
        let mut seen = vec![from.to_string()];
        let mut stack = vec![from.to_string()];
        while let Some(at) = stack.pop() {
            let mut onward = false;
            for edge in self.edges.iter().filter(|edge| edge.from == at && self.audible(edge)) {
                let Some(to) = &edge.to else { continue };
                onward = true;
                if self.nodes.get(to).is_some_and(|node| node.muted) {
                    through_muted = true;
                } else if to != MASTER && !seen.contains(to) && seen.len() < DEEPEST * MOST_PATHS {
                    seen.push(to.clone());
                    stack.push(to.clone());
                }
            }
            if !onward {
                ends.push(at);
            }
        }
        (ends, through_muted)
    }

    /// Whether a way passes a send effect: a bus with a time effect on it (its return is shared, as meant).
    pub fn through_effect(&self, path: &Path) -> bool {
        path.iter().any(|&k| {
            let edge = &self.edges[k];
            edge.route == Route::Send
                && edge.to.as_deref().and_then(|to| self.nodes.get(to)).is_some_and(|node| node.fx.iter().any(|fx| time_effect(fx)))
        })
    }

    /// Whether a way flips the polarity: an odd count of flips among its sends and the tracks it passes (the source's own
    /// flips every way alike).
    fn inverted(&self, path: &Path) -> bool {
        let flips: usize = path
            .iter()
            .map(|&k| {
                let edge = &self.edges[k];
                let through =
                    edge.to.as_deref().filter(|to| *to != MASTER).and_then(|to| self.nodes.get(to)).is_some_and(|node| node.inverted);
                usize::from(edge.inverted) + usize::from(through)
            })
            .sum();
        flips % 2 == 1
    }
}

/// A finding: the tracks it's about, its kind, and what it says.
#[derive(Debug, Clone, PartialEq)]
pub struct Lint {
    pub tracks: Vec<String>,
    pub kind: &'static str,
    pub words: String,
}

/// Whether an effect is a time effect (what a send effect bus holds): a reverb, a delay, an echo.
pub fn time_effect(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["verb", "delay", "echo", "space", "room", "hall", "plate", "chorus", "reverb"].iter().any(|word| lower.contains(word))
}

fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

fn round(value: f64) -> f64 {
    (value * 10.).round() / 10.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, fx: &[&str]) -> Node {
        Node {
            id: id.into(),
            name: id.into(),
            channels: 2,
            sounds: fx.is_empty(),
            fx: fx.iter().map(|f| f.to_string()).collect(),
            ..Node::default()
        }
    }

    fn parent(from: &str, to: &str) -> Edge {
        Edge {
            from: from.into(),
            to: Some(to.into()),
            route: Route::Parent,
            dst: 1,
            muted: false,
            inverted: false,
            volume_db: 0.,
            mode: "post-fader".into(),
            reference: None,
            midi_only: false,
        }
    }

    fn send(from: &str, to: &str) -> Edge {
        Edge { route: Route::Send, reference: Some(format!("{from}>{to}")), ..parent(from, to) }
    }

    fn graph(nodes: Vec<Node>, edges: Vec<Edge>) -> Graph {
        let mut graph = Graph { nodes: nodes.into_iter().map(|n| (n.id.clone(), n)).collect(), edges };
        graph.nodes.insert(MASTER.into(), Node { id: MASTER.into(), name: "Master".into(), channels: 2, ..Node::default() });
        graph
    }

    #[test]
    fn a_kick_in_its_folder_and_sent_to_a_plain_bus_plays_twice() {
        let mut drums = node("Drums", &[]);
        drums.sounds = false;
        let g = graph(
            vec![node("Kick", &[]), drums, node("Drum Bus", &["ReaComp"]), node("Reverb", &["ReaVerbate"])],
            vec![
                parent("Kick", "Drums"),
                parent("Drums", MASTER),
                send("Kick", "Drum Bus"),
                parent("Drum Bus", MASTER),
                send("Kick", "Reverb"),
                parent("Reverb", MASTER),
            ],
        );
        let (paths, looped) = g.paths("Kick");
        assert_eq!(paths.len(), 3);
        assert!(looped.is_none());
        let lints = g.lints();
        let kick = lints.iter().find(|lint| lint.tracks == ["Kick"]).unwrap();
        assert_eq!(kick.kind, "double-path", "{lints:?}");
        assert!(kick.words.contains("“Drum Bus”") && kick.words.contains("into its folder “Drums”"), "{}", kick.words);
    }

    #[test]
    fn a_send_to_a_reverb_bus_is_a_send_effect_and_a_sidechain_on_3_4_is_no_path() {
        let mut bass = node("Bass", &["ReaComp"]);
        bass.channels = 4;
        let g = graph(
            vec![node("Vox", &[]), node("Reverb", &["ReaVerbate"]), node("Kick", &[]), bass],
            vec![
                parent("Vox", MASTER),
                send("Vox", "Reverb"),
                parent("Reverb", MASTER),
                parent("Kick", MASTER),
                Edge { dst: 3, ..send("Kick", "Bass") },
                parent("Bass", MASTER),
            ],
        );
        let lints = g.lints();
        assert_eq!(lints.iter().find(|lint| lint.tracks == ["Vox"]).unwrap().kind, "parallel");
        assert!(lints.iter().all(|lint| lint.tracks[0] != "Kick"), "a sidechain isn't heard twice: {lints:?}");
        // On a 2-channel bass, the sidechain arrives nowhere.
        let mut two = g.clone();
        two.nodes.get_mut("Bass").unwrap().channels = 2;
        let lint = two.lints().into_iter().find(|lint| lint.kind == "sidechain-channels").unwrap();
        assert!(lint.words.contains("channels 3/4 of “Bass”, which has 2 channels"), "{}", lint.words);
    }

    #[test]
    fn loops_silence_hardware_only_and_inverted_duplicates_are_found() {
        let looped = graph(vec![node("A", &[]), node("B", &[])], vec![send("A", "B"), send("B", "A"), parent("B", MASTER)]);
        assert!(looped.lints().iter().any(|lint| lint.kind == "feedback"));
        let silent = graph(vec![node("Pad", &[])], vec![]);
        assert_eq!(silent.lints()[0].kind, "silent");
        // In a folder that reaches nothing, the folder is named; in a muted one, it's silent as meant.
        let mut folder = node("Keys", &[]);
        folder.sounds = false;
        let stuck = graph(vec![node("Pad", &[]), folder.clone()], vec![parent("Pad", "Keys")]);
        let lint = &stuck.lints()[0];
        assert!(lint.words.contains("its way ends at “Keys”"), "{}", lint.words);
        folder.muted = true;
        let muted_folder = graph(vec![node("Pad", &[]), folder], vec![parent("Pad", "Keys")]);
        assert!(muted_folder.lints().is_empty(), "{:?}", muted_folder.lints());
        let hardware = graph(vec![node("Click", &[])], vec![Edge { to: None, route: Route::Hardware, ..parent("Click", MASTER) }]);
        assert_eq!(hardware.lints()[0].kind, "hardware-only");
        let cancel = graph(
            vec![node("Kick", &[]), node("Bus", &[])],
            vec![parent("Kick", MASTER), Edge { inverted: true, ..send("Kick", "Bus") }, parent("Bus", MASTER)],
        );
        assert_eq!(cancel.lints()[0].kind, "inverted-duplicate");
        // A bus whose own polarity is flipped cancels the same way.
        let mut flipped = node("Bus", &[]);
        flipped.inverted = true;
        let bus = graph(vec![node("Kick", &[]), flipped], vec![parent("Kick", MASTER), send("Kick", "Bus"), parent("Bus", MASTER)]);
        assert_eq!(bus.lints().iter().find(|lint| lint.tracks == ["Kick"]).unwrap().kind, "inverted-duplicate");
        // A reverb that reaches nothing: heard nowhere, the sends into it named.
        let mut verb = node("Reverb", &["ReaVerbate"]);
        verb.sounds = false;
        let dead = graph(vec![node("Vox", &[]), verb], vec![parent("Vox", MASTER), send("Vox", "Reverb")]);
        let lint = dead.lints().into_iter().find(|lint| lint.tracks[0] == "Reverb").unwrap();
        assert_eq!((lint.kind, lint.tracks.clone()), ("silent", vec!["Reverb".to_string(), "Vox".to_string()]));
        assert!(lint.words.ends_with("the sends of “Vox” into it do nothing"), "{}", lint.words);
        // A muted send is no way at all.
        let muted = graph(
            vec![node("Kick", &[]), node("Bus", &[])],
            vec![parent("Kick", MASTER), Edge { muted: true, ..send("Kick", "Bus") }, parent("Bus", MASTER)],
        );
        assert!(muted.lints().is_empty());
    }
}
