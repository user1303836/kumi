//! The producer's Live Sets, read without opening Live: tracks, chains, clips, samples and settings.
use super::{
    sources::{dirname, is_absolute, join},
    xml::{attribute, scan_xml_file, ScanOptions, TagHandler},
};
use crate::core::errors::RuntimeError;
use indexmap::{IndexMap, IndexSet};
use kumi_common::{
    abort::Signal,
    js::number::{parse, round, to_string},
};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::LazyLock};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    #[default]
    Audio,
    Midi,
    Group,
    Return,
    Main,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DeviceRole {
    Instrument,
    #[default]
    Audio,
    Midi,
    Rack,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SetDevice {
    pub name: String,
    pub role: DeviceRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<Vec<String>>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Clips {
    pub session: usize,
    pub arrangement: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SetTrack {
    pub name: String,
    pub kind: TrackKind,
    pub devices: Vec<SetDevice>,
    pub clips: Clips,
    pub samples: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SetSummary {
    pub name: String,
    pub tracks: Vec<SetTrack>,
    pub returns: Vec<SetTrack>,
    pub scenes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<SetTrack>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangement_beats: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<String>,
}
static DEVICE_NAMES: LazyLock<std::collections::HashMap<String, String>> =
    LazyLock::new(|| serde_json::from_str(include_str!("device-names.json")).expect("device names"));
const INSTRUMENTS: &[&str] = &[
    "OriginalSimpler",
    "MultiSampler",
    "Operator",
    "UltraAnalog",
    "InstrumentVector",
    "Drift",
    "InstrumentMeld",
    "Collision",
    "LoungeLizard",
    "StringStudio",
    "InstrumentImpulse",
    "DrumCell",
    "ProxyInstrumentDevice",
    "MxDeviceInstrument",
];
const RACKS: &[&str] = &["DrumGroupDevice", "InstrumentGroupDevice", "AudioEffectGroupDevice", "MidiEffectGroupDevice"];
const PLUGINS: &[&str] = &["PluginDevice", "AuPluginDevice"];
pub fn device_name(tag: &str) -> String {
    if let Some(name) = DEVICE_NAMES.get(tag) {
        return name.clone();
    }
    static MX: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^Mx(Device)?").unwrap());
    static WORDS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"([a-z])([A-Z])").unwrap());
    static NUMBER: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"([^0-9])([0-9]+)$").unwrap());
    NUMBER.replace(&WORDS.replace_all(&MX.replace(tag, ""), "$1 $2"), "$1").into_owned()
}
const SCALES: &[&str] = &[
    "Major",
    "Minor",
    "Dorian",
    "Mixolydian",
    "Lydian",
    "Phrygian",
    "Locrian",
    "Whole Tone",
    "Half-whole Dim.",
    "Whole-half Dim.",
    "Minor Blues",
    "Minor Pentatonic",
    "Major Pentatonic",
    "Harmonic Minor",
    "Harmonic Major",
    "Dorian #4",
    "Phrygian Dominant",
    "Melodic Minor",
    "Lydian Augmented",
    "Lydian Dominant",
    "Super Locrian",
    "8-Tone Spanish",
    "Bhairav",
    "Hungarian Minor",
    "Hirajoshi",
    "In-Sen",
    "Iwato",
    "Kumoi",
    "Pelog Selisir",
    "Pelog Tembung",
    "Messiaen 3",
    "Messiaen 4",
    "Messiaen 5",
    "Messiaen 6",
    "Messiaen 7",
];
const NOTES: &[&str] = &["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
pub fn time_signature(value: f64) -> Option<String> {
    if !value.is_finite() || value.fract() != 0. || value < 0. {
        return None;
    }
    let numerator = value % 99. + 1.;
    let denominator = 2_f64.powf((value / 99.).floor());
    (denominator <= 64.).then(|| format!("{}/{}", to_string(numerator), to_string(denominator)))
}
fn track_kind(tag: &str) -> Option<TrackKind> {
    match tag {
        "AudioTrack" => Some(TrackKind::Audio),
        "MidiTrack" => Some(TrackKind::Midi),
        "GroupTrack" => Some(TrackKind::Group),
        "ReturnTrack" => Some(TrackKind::Return),
        "MainTrack" | "MasterTrack" => Some(TrackKind::Main),
        _ => None,
    }
}
fn number(value: Option<String>) -> f64 {
    value.as_deref().and_then(parse).unwrap_or(f64::NAN)
}
struct OpenDevice {
    tag: String,
    depth: usize,
    device: SetDevice,
    top: bool,
    names: IndexSet<String>,
}
struct OpenTrack {
    depth: usize,
    track: SetTrack,
    id: Option<String>,
    group_id: Option<String>,
    name: Option<String>,
}
struct Clip {
    arrangement: bool,
    depth: usize,
}
#[derive(Default)]
struct Sample {
    path: Option<String>,
    relative: Option<String>,
}
#[derive(Default)]
struct Scale {
    root: Option<f64>,
    name: Option<String>,
    on: Option<bool>,
}
struct Reader {
    stack: Vec<String>,
    set: SetSummary,
    saw_set: bool,
    current: Option<OpenTrack>,
    devices: Vec<OpenDevice>,
    groups: IndexMap<String, String>,
    members: Vec<(TrackKind, usize, String)>,
    clip: Option<Clip>,
    sample: Option<Sample>,
    scale: Scale,
    folder: String,
}
impl TagHandler for Reader {
    fn open(&mut self, name: &str, attrs: &str, _empty: bool) {
        let depth = self.stack.len();
        let parent = self.stack.last().map(String::as_str).unwrap_or("");
        let value = || attribute(attrs, "Value");
        if depth == 0 && name == "Ableton" {
            if let Some(creator) = attribute(attrs, "Creator").filter(|s| !s.is_empty()) {
                self.set.live = Some(creator);
            }
        }
        if depth == 1 && name == "LiveSet" {
            self.saw_set = true;
        }
        if self.current.is_none() && self.stack.get(1).is_some_and(|s| s == "LiveSet") {
            if depth == 3 && parent == "Scenes" {
                self.set.scenes += 1;
            }
            if depth == 3 && parent == "ScaleInformation" {
                if matches!(name, "Root" | "RootNote") {
                    self.scale.root = Some(number(value()));
                }
                if name == "Name" {
                    self.scale.name = Some(value().unwrap_or_default());
                }
            }
            if depth == 2 && name == "InKey" {
                self.scale.on = Some(value().as_deref() == Some("true"));
            }
        }
        let kind = track_kind(name);
        if self.current.is_none()
            && kind.is_some()
            && self.stack.get(1).is_some_and(|s| s == "LiveSet")
            && (parent == "Tracks" || depth == 2)
        {
            self.current = Some(OpenTrack {
                depth,
                track: SetTrack { kind: kind.unwrap(), ..Default::default() },
                id: attribute(attrs, "Id").filter(|s| !s.is_empty()),
                group_id: None,
                name: None,
            });
        } else if let Some(current) = self.current.as_mut() {
            let at = depth - current.depth;
            if at == 2 && name == "EffectiveName" && parent == "Name" {
                current.name = Some(value().unwrap_or_default());
            }
            if at == 1 && matches!(name, "Color" | "ColorIndex") {
                let color = number(value());
                if color.is_finite() && color.fract() == 0. && color >= 0. {
                    current.track.color = Some(color);
                }
            }
            if at == 1 && name == "TrackGroupId" {
                if let Some(id) = value().filter(|s| !s.is_empty() && s != "-1") {
                    current.group_id = Some(id);
                }
            }
            if at == 1 && name == "Freeze" && value().as_deref() == Some("true") {
                current.track.frozen = Some(true);
            }
            if current.track.kind == TrackKind::Main
                && self.stack.get(current.depth + 2).is_some_and(|s| s == "Mixer")
                && name == "Manual"
                && at == 4
            {
                if parent == "Tempo" {
                    let tempo = number(value());
                    if tempo > 0. {
                        self.set.tempo = Some(round(tempo * 100.) / 100.);
                    }
                }
                if parent == "TimeSignature" {
                    if let Some(signature) = time_signature(number(value())) {
                        self.set.signature = Some(signature);
                    }
                }
            }
            if parent == "Devices" {
                let top = at == 4
                    && self.stack.get(current.depth + 1).is_some_and(|s| s == "DeviceChain")
                    && self.stack.get(current.depth + 2).is_some_and(|s| s == "DeviceChain");
                let role = if RACKS.contains(&name) {
                    DeviceRole::Rack
                } else if name.starts_with("Midi") || name == "MxDeviceMidiEffect" {
                    DeviceRole::Midi
                } else if INSTRUMENTS.contains(&name) {
                    DeviceRole::Instrument
                } else {
                    DeviceRole::Audio
                };
                self.devices.push(OpenDevice {
                    tag: name.into(),
                    depth,
                    device: SetDevice {
                        name: device_name(name),
                        role,
                        plugin: name.starts_with("MxDevice").then(|| "Max for Live".into()),
                        ..Default::default()
                    },
                    top,
                    names: IndexSet::new(),
                });
            }
            if let Some(inner) = self.devices.last_mut() {
                let within = depth - inner.depth;
                if within == 1 && name == "UserName" {
                    if let Some(named) = value().filter(|s| !s.is_empty() && *s != inner.device.name) {
                        inner.device.preset = Some(named);
                    }
                }
                if PLUGINS.contains(&inner.tag.as_str()) && self.stack.get(inner.depth + 1).is_some_and(|s| s == "PluginDesc") {
                    let info = self.stack.get(inner.depth + 2).map(String::as_str).unwrap_or("");
                    if within == 3
                        && ((name == "PlugName" && info == "VstPluginInfo")
                            || (name == "Name" && matches!(info, "Vst3PluginInfo" | "AuPluginInfo")))
                    {
                        if let Some(plugin) = value().filter(|s| !s.is_empty()) {
                            inner.device.name = plugin;
                            inner.device.plugin = Some(
                                match info {
                                    "VstPluginInfo" => "VST",
                                    "Vst3PluginInfo" => "VST3",
                                    _ => "AU",
                                }
                                .into(),
                            );
                        }
                    }
                    if within == 3 && name == "Manufacturer" && info == "AuPluginInfo" {
                        if let Some(maker) = value().filter(|s| !s.is_empty()) {
                            inner.device.plugin = Some(format!("AU · {maker}"));
                        }
                    }
                    if within == 3 && name == "ComponentType" && info == "AuPluginInfo" && value().as_deref() == Some("1635085685") {
                        inner.device.role = DeviceRole::Instrument;
                    }
                }
                if inner.tag.starts_with("MxDevice")
                    && matches!(name, "Path" | "RelativePath")
                    && inner.device.name == device_name(&inner.tag)
                {
                    let file = value().unwrap_or_default();
                    if file.to_ascii_lowercase().ends_with(".amxd") {
                        let basename = file.rsplit(['\\', '/']).next().unwrap();
                        inner.device.name = basename[..basename.len() - 5].into();
                    }
                }
            }
            if matches!(name, "AudioClip" | "MidiClip") && self.clip.is_none() && !self.stack.iter().any(|s| s == "FreezeSequencer") {
                let arrangement = self.stack.iter().any(|s| s == "ArrangerAutomation") && self.stack.iter().any(|s| s == "MainSequencer");
                let session = self.stack.iter().any(|s| s == "ClipSlotList");
                if arrangement || session {
                    self.clip = Some(Clip { arrangement, depth });
                    if arrangement {
                        current.track.clips.arrangement += 1;
                    } else {
                        current.track.clips.session += 1;
                    }
                }
            }
            if self.clip.as_ref().is_some_and(|clip| clip.arrangement && depth == clip.depth + 1) && name == "CurrentEnd" {
                let end = number(value());
                if end.is_finite() && end > self.set.arrangement_beats.unwrap_or(0.) {
                    self.set.arrangement_beats = Some(round(end * 100.) / 100.);
                }
            }
            if name == "SampleRef" {
                self.sample = Some(Sample::default());
            }
            if parent == "FileRef" && self.stack.get(depth.saturating_sub(2)).is_some_and(|s| s == "SampleRef") {
                if let Some(sample) = self.sample.as_mut() {
                    if let Some(file) = value().filter(|s| !s.is_empty()) {
                        if name == "Path" && is_absolute(&file) {
                            sample.path = Some(file.clone());
                        }
                        if name == "RelativePath" && !is_absolute(&file) {
                            sample.relative = Some(join(&self.folder, &file));
                        }
                    }
                }
            }
        }
        self.stack.push(name.into());
    }
    fn close(&mut self, name: &str) {
        if self.stack.is_empty() {
            return;
        }
        let depth = self.stack.len() - 1;
        self.stack.pop();
        if self.clip.as_ref().is_some_and(|clip| depth == clip.depth) {
            self.clip = None;
        }
        if self.devices.last().is_some_and(|inner| depth == inner.depth) {
            let mut inner = self.devices.pop().unwrap();
            for open in &mut self.devices {
                open.names.insert(inner.device.name.clone());
            }
            if let Some(current) = self.current.as_mut() {
                if inner.device.plugin.as_ref().is_some_and(|s| s != "Max for Live") {
                    let plugins = current.track.plugins.get_or_insert_with(Vec::new);
                    if !plugins.contains(&inner.device.name) {
                        plugins.push(inner.device.name.clone());
                    }
                }
                if !inner.names.is_empty() {
                    inner.device.inside = Some(inner.names.into_iter().take(16).collect());
                }
                if inner.top {
                    current.track.devices.push(inner.device);
                }
            }
        }
        if name == "SampleRef" && self.current.is_some() && self.sample.is_some() {
            let sample = self.sample.take().unwrap();
            let file = sample.path.or_else(|| sample.relative.filter(|s| Path::new(s).exists()));
            if let Some(file) = file {
                let current = self.current.as_mut().unwrap();
                if current.track.samples.len() < 32 && !current.track.samples.contains(&file) {
                    current.track.samples.push(file);
                }
            }
        }
        if self.current.as_ref().is_some_and(|current| depth == current.depth) {
            let mut current = self.current.take().unwrap();
            current.track.name = current.name.unwrap_or_default();
            if current.track.kind == TrackKind::Group {
                if let Some(id) = current.id {
                    self.groups.insert(id, current.track.name.clone());
                }
            }
            let kind = current.track.kind;
            let index = match kind {
                TrackKind::Return => self.set.returns.len(),
                TrackKind::Main => 0,
                _ => self.set.tracks.len(),
            };
            if let Some(group) = current.group_id {
                self.members.push((kind, index, group));
            }
            match kind {
                TrackKind::Return => self.set.returns.push(current.track),
                TrackKind::Main => self.set.main = Some(current.track),
                _ => self.set.tracks.push(current.track),
            }
        }
    }
}
/// Read a Set's file. Rejects files that aren't Live Sets.
pub async fn read_set(path: &str, signal: Option<Signal>) -> Result<SetSummary, RuntimeError> {
    let filename = path.rsplit(['\\', '/']).next().unwrap_or("");
    let name = if filename.to_ascii_lowercase().ends_with(".als") { &filename[..filename.len() - 4] } else { filename };
    let mut reader = Reader {
        stack: vec![],
        set: SetSummary { name: name.into(), ..Default::default() },
        saw_set: false,
        current: None,
        devices: vec![],
        groups: IndexMap::new(),
        members: vec![],
        clip: None,
        sample: None,
        scale: Scale::default(),
        folder: dirname(path),
    };
    scan_xml_file(Path::new(path), &mut reader, ScanOptions { signal, ..Default::default() }).await.map_err(|e| match e {
        super::xml::XmlError::Aborted(_) => RuntimeError::Aborted,
        e => RuntimeError::plain(e.to_string()),
    })?;
    if !reader.saw_set {
        return Err(RuntimeError::plain("That isn't a Live Set."));
    }
    for (kind, index, id) in reader.members {
        if let Some(group) = reader.groups.get(&id).filter(|g| !g.is_empty()) {
            let track = match kind {
                TrackKind::Return => reader.set.returns.get_mut(index),
                TrackKind::Main => reader.set.main.as_mut(),
                _ => reader.set.tracks.get_mut(index),
            };
            if let Some(track) = track {
                track.group = Some(group.clone());
            }
        }
    }
    let scale = reader.scale;
    let chosen = !(scale.root == Some(0.) && matches!(scale.name.as_deref(), Some("0" | "Major")));
    if chosen && scale.on != Some(false) {
        if let (Some(root), Some(name)) = (scale.root, scale.name) {
            if root.is_finite() && root.fract() == 0. && (0.0..12.).contains(&root) {
                let named = if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
                    name.parse::<usize>().ok().and_then(|n| SCALES.get(n)).map(|s| (*s).to_string())
                } else {
                    Some(name)
                };
                if let Some(named) = named.filter(|s| !s.is_empty()) {
                    reader.set.key = Some(format!(
                        "{} {}",
                        NOTES[root as usize],
                        if matches!(named.as_str(), "Major" | "Minor") { named.to_lowercase() } else { named }
                    ));
                }
            }
        }
    }
    Ok(reader.set)
}
