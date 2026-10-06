//! A model of the open Set, kept in memory and built each turn from the pages the observation already reads,
//! so it asks Live nothing of its own. It holds the Set's tracks (returns and main included, as the observation
//! lists them) with their devices one rack level down. It finds them by Kumi's own track id (item 3b) or their
//! current ref, and by name. It tells what changed between two builds: each track keeps the generation it last
//! changed in.
use crate::core::contracts::JsonObject;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// A chain inside a rack: its ref and name.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainNode {
    pub reference: String,
    pub name: String,
    pub devices: Vec<DeviceNode>,
}

/// A device: its ref, name, class (when it isn't the name) and, for a rack, its chains.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceNode {
    pub reference: String,
    pub name: String,
    pub class: Option<String>,
    pub chains: Vec<ChainNode>,
}

/// A track: Kumi's id for it (when Live reports one), where it is now (ref and index), and what it is.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackNode {
    pub id: Option<String>,
    pub reference: String,
    pub index: usize,
    pub name: String,
    pub kind: Option<String>,
    pub media: Option<String>,
    /// The ref of the group track it's in.
    pub group: Option<String>,
    pub devices: Vec<DeviceNode>,
    /// A digest of what it is (not where): its name, kind, media, group and devices.
    pub hash: String,
    /// The build it last changed in.
    pub generation: u64,
}

impl TrackNode {
    /// How the model knows the track from one build to the next: Kumi's id for it, else its ref.
    pub fn key(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.reference)
    }
}

/// What changed between two builds, by track key.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Diff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// (key, name before, name now)
    pub renamed: Vec<(String, String, String)>,
    /// (key, index before, index now)
    pub moved: Vec<(String, usize, usize)>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.renamed.is_empty() && self.moved.is_empty()
    }
}

/// The Set as the last observation read it.
#[derive(Debug, Default, Clone)]
pub struct SetModel {
    pub generation: u64,
    pub tracks: Vec<TrackNode>,
    /// Whether every track and device was read (none of their pages was cut short).
    pub complete: bool,
    by_key: HashMap<String, usize>,
    by_name: HashMap<String, Vec<usize>>,
}

/// A name as lookups compare it: lowercase letters and digits only, so "BASS!", "bass" and " Bass " match.
pub fn normalize(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn text(row: &JsonObject, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_owned)
}

impl SetModel {
    /// The next build after `previous`, from the observation's track rows and device rows (`complete` when no
    /// page was cut short). A track whose key and digest are unchanged keeps its generation.
    pub fn next(previous: &SetModel, tracks: &[JsonObject], devices: &[JsonObject], complete: bool) -> SetModel {
        let generation = previous.generation + 1;
        let mut model = SetModel { generation, complete, ..Default::default() };
        let mut on: HashMap<String, Vec<DeviceNode>> = HashMap::new();
        nest(devices, &mut on);
        for (index, row) in tracks.iter().enumerate() {
            // A row without a ref or a name isn't a track as Live lists it.
            let (Some(reference), Some(name)) = (text(row, "ref"), text(row, "name")) else { continue };
            let devices = on.remove(&reference).unwrap_or_default();
            let mut track = TrackNode {
                id: text(row, "kumiTrack").filter(|id| super::track_ids::track_id(id)),
                reference,
                index,
                name,
                kind: text(row, "kind"),
                media: text(row, "mediaKind"),
                group: text(row, "groupTrackRef"),
                devices,
                hash: String::new(),
                generation,
            };
            track.hash = digest(&track);
            if let Some(before) = previous.track(track.key()).filter(|before| before.hash == track.hash) {
                track.generation = before.generation;
            }
            model.tracks.push(track);
        }
        model.index();
        model
    }
    fn index(&mut self) {
        for (at, track) in self.tracks.iter().enumerate() {
            self.by_key.insert(track.key().to_owned(), at);
            if track.id.is_some() {
                self.by_key.entry(track.reference.clone()).or_insert(at);
            }
            self.by_name.entry(normalize(&track.name)).or_default().push(at);
        }
    }
    /// The track with this key: Kumi's id for it, or its current ref.
    pub fn track(&self, key: &str) -> Option<&TrackNode> {
        self.by_key.get(key).map(|&at| &self.tracks[at])
    }
    /// The tracks with this name, compared as `normalize` does, in the Set's order.
    pub fn tracks_named(&self, name: &str) -> Vec<&TrackNode> {
        self.by_name.get(&normalize(name)).into_iter().flatten().map(|&at| &self.tracks[at]).collect()
    }
    /// The tracks that changed in a build after `generation`.
    pub fn changed_since(&self, generation: u64) -> Vec<&TrackNode> {
        self.tracks.iter().filter(|track| track.generation > generation).collect()
    }
    /// What changed from `self` to `newer`: tracks added, removed, renamed and moved, by key.
    pub fn diff(&self, newer: &SetModel) -> Diff {
        let mut diff = Diff::default();
        for track in &newer.tracks {
            match self.track(track.key()).filter(|before| before.key() == track.key()) {
                None => diff.added.push(track.key().to_owned()),
                Some(before) => {
                    if before.name != track.name {
                        diff.renamed.push((track.key().to_owned(), before.name.clone(), track.name.clone()));
                    }
                    if before.index != track.index {
                        diff.moved.push((track.key().to_owned(), before.index, track.index));
                    }
                }
            }
        }
        for track in &self.tracks {
            if newer.track(track.key()).filter(|now| now.key() == track.key()).is_none() {
                diff.removed.push(track.key().to_owned());
            }
        }
        diff
    }
}

/// Devices placed under what holds them: a track's own, and in each rack's chains, one level down.
fn nest(rows: &[JsonObject], on: &mut HashMap<String, Vec<DeviceNode>>) {
    let device = |row: &JsonObject| {
        let name = text(row, "name").unwrap_or_default();
        DeviceNode {
            reference: text(row, "ref").unwrap_or_default(),
            class: text(row, "className").filter(|class| *class != name),
            chains: row
                .get("chainList")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|chain| {
                    Some(ChainNode {
                        reference: chain.get("ref")?.as_str()?.to_owned(),
                        name: chain.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        devices: vec![],
                    })
                })
                .collect(),
            name,
        }
    };
    let mut in_chains: HashMap<String, Vec<DeviceNode>> = HashMap::new();
    let mut top: Vec<(String, DeviceNode)> = vec![];
    for row in rows {
        let Some(parent) = text(row, "parentRef") else { continue };
        if parent.contains(":chain:") {
            in_chains.entry(parent).or_default().push(device(row));
        } else {
            top.push((parent, device(row)));
        }
    }
    for (parent, mut device) in top {
        for chain in &mut device.chains {
            chain.devices = in_chains.remove(&chain.reference).unwrap_or_default();
        }
        on.entry(parent).or_default().push(device);
    }
}

fn digest(track: &TrackNode) -> String {
    let mut hasher = Sha256::new();
    let mut part = |text: &str| {
        hasher.update((text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    };
    part(&track.name);
    part(track.kind.as_deref().unwrap_or(""));
    part(track.media.as_deref().unwrap_or(""));
    part(track.group.as_deref().unwrap_or(""));
    fn walk(list: &[DeviceNode], part: &mut dyn FnMut(&str)) {
        part(&list.len().to_string());
        for device in list {
            part(&device.name);
            part(device.class.as_deref().unwrap_or(""));
            part(&device.chains.len().to_string());
            for chain in &device.chains {
                part(&chain.name);
                walk(&chain.devices, part);
            }
        }
    }
    walk(&track.devices, &mut part);
    hex::encode(hasher.finalize())
}
