use super::{
    sets::device_name,
    xml::{attribute, scan_tags, xml_head, TagHandler},
};
use indexmap::IndexSet;
use kumi_common::js::string::{head, trim};
use kumi_types::daw::DawId;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, SeekFrom},
    path::Path,
    sync::LazyLock,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt};
pub const PRESET_EXTENSIONS: &[&str] = &[".adv", ".adg", ".amxd", ".vstpreset", ".aupreset", ".fxp", ".fxb"];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetCategory {
    #[serde(rename = "instrument")]
    Instrument,
    #[serde(rename = "audio effect")]
    AudioEffect,
    #[serde(rename = "midi effect")]
    MidiEffect,
    #[serde(rename = "drum rack")]
    DrumRack,
    #[serde(rename = "plug-in")]
    Plugin,
    // REAPER's own (its resource folder)
    #[serde(rename = "fx chain")]
    FxChain,
    #[serde(rename = "track template")]
    TrackTemplate,
    #[serde(rename = "project template")]
    ProjectTemplate,
    #[serde(rename = "jsfx")]
    Jsfx,
    /// A plug-in's bank of presets, as REAPER keeps them (`presets/vst-reacomp.ini`).
    #[serde(rename = "fx presets")]
    FxPresets,
    // Renoise's own (its libraries; an instrument is `Instrument`)
    #[serde(rename = "multi-sample")]
    MultiSample,
    #[serde(rename = "effect chain")]
    EffectChain,
    #[serde(rename = "effect preset")]
    EffectPreset,
    #[serde(rename = "phrase")]
    Phrase,
    #[serde(rename = "phrase script")]
    PhraseScript,
    #[serde(rename = "modulation set")]
    ModulationSet,
}

/// The DAWs a preset of this format is offered in (its format, as the library keeps it): the DAWs whose profiles name it
/// (REAPER takes a VST3 preset's path, as Live does), and Live for a format no DAW names (its own files, plug-ins'
/// presets).
pub fn preset_daws(format: &str) -> impl Iterator<Item = DawId> + '_ {
    let names = move |daw: &DawId| daw.profile().vocab.preset_formats.contains(&format);
    let named = DawId::ALL.iter().any(names);
    DawId::ALL.into_iter().filter(move |daw| if named { names(daw) } else { *daw == DawId::Ableton })
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PresetFacts {
    /// Its own name where the file's isn't it (a JSFX's `desc`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What the DAW loads it by, where that isn't its path (a JSFX: "JS: midi/midi_choke").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<PresetCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
}
fn category_of(tag: &str) -> PresetCategory {
    use PresetCategory::*;
    match tag {
        "InstrumentGroupDevice" => Instrument,
        "DrumGroupDevice" => DrumRack,
        "AudioEffectGroupDevice" => AudioEffect,
        "MidiEffectGroupDevice" => MidiEffect,
        "OriginalSimpler"
        | "MultiSampler"
        | "Operator"
        | "UltraAnalog"
        | "InstrumentVector"
        | "Drift"
        | "InstrumentMeld"
        | "Collision"
        | "LoungeLizard"
        | "StringStudio"
        | "InstrumentImpulse"
        | "DrumCell"
        | "ProxyInstrumentDevice" => Instrument,
        tag if tag.starts_with("Midi") => MidiEffect,
        _ => AudioEffect,
    }
}
#[derive(Default)]
struct Reader {
    stack: Vec<String>,
    root: Option<(String, usize)>,
    inside: IndexSet<String>,
    about: Option<String>,
    plugin: Option<String>,
}
impl TagHandler for Reader {
    fn open(&mut self, name: &str, attrs: &str, _: bool) {
        let depth = self.stack.len();
        let parent = self.stack.last().map(String::as_str).unwrap_or("");
        if self.root.is_none()
            && depth >= 1
            && self.stack[0] == "Ableton"
            && !matches!(name, "GroupDevicePreset" | "Device" | "OverwriteProtectionNumber" | "PresetRef")
        {
            self.root = Some((name.into(), depth));
        } else if self.root.is_some() && parent == "Devices" && name != "MacroControls" {
            self.inside.insert(device_name(name));
        }
        if let Some((_, root_depth)) = &self.root {
            if depth == root_depth + 1 && name == "Annotation" && self.about.is_none() {
                self.about = attribute(attrs, "Value").map(|s| head(trim(&s), 200)).filter(|s| !s.is_empty());
            }
            if (name == "PlugName" || (name == "Name" && matches!(parent, "Vst3PluginInfo" | "AuPluginInfo"))) && self.plugin.is_none() {
                self.plugin = attribute(attrs, "Value").filter(|s| !s.is_empty());
            }
        }
        self.stack.push(name.into());
    }
    fn close(&mut self, _: &str) {
        self.stack.pop();
    }
}
/// A Live preset's first device, a rack's devices and its annotation, read from its first bytes.
pub async fn read_live_preset(path: &str) -> std::io::Result<PresetFacts> {
    let text = xml_head(Path::new(path), 24 * 1024).await?;
    let mut reader = Reader::default();
    scan_tags(&text, &mut reader);
    let Some((tag, _)) = reader.root else {
        return Ok(PresetFacts::default());
    };
    if matches!(tag.as_str(), "PluginDevice" | "AuPluginDevice") {
        return Ok(PresetFacts {
            category: Some(PresetCategory::Plugin),
            device: reader.plugin,
            about: reader.about,
            ..Default::default()
        });
    }
    let category = if tag.starts_with("MxDevice") {
        match tag.as_str() {
            "MxDeviceInstrument" => PresetCategory::Instrument,
            "MxDeviceMidiEffect" => PresetCategory::MidiEffect,
            _ => PresetCategory::AudioEffect,
        }
    } else {
        category_of(&tag)
    };
    Ok(PresetFacts {
        device: Some(device_name(&tag)),
        category: Some(category),
        inside: (!reader.inside.is_empty()).then(|| reader.inside.into_iter().take(12).collect()),
        about: reader.about,
        ..Default::default()
    })
}
/// A Max for Live device's kind comes from its `ampf` header.
pub async fn read_max_device(path: &str) -> std::io::Result<PresetFacts> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut head = [0_u8; 12];
    let _ = file.read(&mut head).await?;
    let category = (&head[..4] == b"ampf").then(|| match &head[8..12] {
        b"iiii" => PresetCategory::Instrument,
        b"mmmm" => PresetCategory::MidiEffect,
        _ => PresetCategory::AudioEffect,
    });
    Ok(PresetFacts { device: Some("Max for Live".into()), category, ..Default::default() })
}
pub fn plugin_preset_facts(relative_path: &str) -> PresetFacts {
    let parts: Vec<_> = relative_path.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    let plugin = if parts.len() >= 3 {
        Some(parts[1])
    } else if parts.len() == 2 {
        Some(parts[0])
    } else {
        None
    };
    PresetFacts { category: Some(PresetCategory::Plugin), device: plugin.map(String::from), ..Default::default() }
}

// ---------------------------------------------------------------------------------------------------
// REAPER's and Renoise's own presets: what each holds, read from its first bytes.

/// How much of a text preset is read (a chain or template may hold megabytes of plug-in state after what's read).
const DAW_HEAD: usize = 256 * 1024;

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}

/// Names joined for `about`, cut to its 200 characters.
fn listed(lead: &str, names: &[String]) -> Option<String> {
    (!names.is_empty()).then(|| head(&format!("{lead}{}", names.join(", ")), 200))
}

/// The quoted words and bare words of an RPP line, as REAPER writes them (`<VST "VST: ReaEQ (Cockos)" reaeq.vst 0 ""`).
fn rpp_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut word = String::new();
        if c == '"' || c == '\'' || c == '`' {
            chars.next();
            for next in chars.by_ref() {
                if next == c {
                    break;
                }
                word.push(next);
            }
        } else {
            while let Some(&next) = chars.peek() {
                if next.is_whitespace() {
                    break;
                }
                word.push(next);
                chars.next();
            }
        }
        words.push(word);
    }
    words
}

/// The FX a REAPER chain's text holds (`<VST "VST: ReaComp (Cockos)" …`, `<JS utility/volume ""`, `<CONTAINER …`), by the
/// names REAPER's FX browser shows, in order; and the track names of a template or project (`NAME` right inside `<TRACK`).
fn reaper_text(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut fx, mut tracks) = (Vec::new(), Vec::new());
    // (the open blocks, by their first word)
    let mut stack: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line == ">" {
            stack.pop();
            continue;
        }
        if let Some(rest) = line.strip_prefix('<') {
            let words = rpp_words(rest);
            let first = words.first().cloned().unwrap_or_default();
            match first.as_str() {
                "VST" | "AU" | "CLAP" | "LV2" | "DX" | "VIDEO_EFFECT" => {
                    if let Some(name) = words.get(1).filter(|n| !n.is_empty()) {
                        // "VST: ReaComp (Cockos)" is listed as "ReaComp (Cockos)": the type is the browser's filter.
                        fx.push(name.split_once(": ").map(|(_, n)| n.to_string()).unwrap_or_else(|| name.clone()));
                    }
                }
                "JS" => {
                    if let Some(path) = words.get(1).filter(|n| !n.is_empty()) {
                        fx.push(format!("JS: {path}"));
                    }
                }
                "CONTAINER" => fx.push("Container".into()),
                _ => {}
            }
            stack.push(first);
            continue;
        }
        if stack.last().map(String::as_str) == Some("TRACK") {
            if let Some(name) = line.strip_prefix("NAME ") {
                tracks.push(rpp_words(name).into_iter().next().unwrap_or_default());
            }
        }
    }
    (fx, tracks)
}

/// A JSFX's name (its first `desc:` line), tags and author, from its first lines. A file with no `desc:` isn't an effect
/// REAPER lists (a library it imports, a data file).
fn jsfx_text(text: &str) -> Option<(String, Option<String>)> {
    let mut desc = None;
    let mut tags = None;
    let mut author = None;
    for line in text.lines().take(400).map(str::trim) {
        if let Some(rest) = line.strip_prefix("desc:") {
            desc.get_or_insert_with(|| trim(rest).to_string());
        } else if let Some(rest) = line.strip_prefix("//tags:") {
            tags.get_or_insert_with(|| trim(rest).to_string());
        } else if let Some(rest) = line.strip_prefix("//author:") {
            author.get_or_insert_with(|| trim(rest).to_string());
        } else if line.starts_with('@') {
            break;
        }
    }
    let desc = desc.filter(|d| !d.is_empty())?;
    let about = match (tags.filter(|t| !t.is_empty()), author.filter(|a| !a.is_empty())) {
        (Some(tags), Some(author)) => Some(format!("{tags} (by {author})")),
        (Some(tags), None) => Some(tags),
        (None, Some(author)) => Some(format!("by {author}")),
        (None, None) => None,
    };
    Some((desc, about.map(|a| head(&a, 200))))
}

/// A REAPER preset bank's presets (`Name=` in each `[PresetN]`), in the file's order.
fn preset_bank(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_preset = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_preset = line.starts_with("[Preset");
        } else if in_preset {
            if let Some(name) = line.strip_prefix("Name=") {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// The plug-in a REAPER preset bank is for, from its file name (`vst-reacomp.ini`, `vst3-Serum2.ini`, `js-utility_volume.ini`).
fn bank_plugin(file: &str) -> String {
    let stem = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let stem = stem.strip_suffix(".ini").or_else(|| stem.strip_suffix(".INI")).unwrap_or(stem);
    for prefix in ["vst3-", "vst-", "vsti-", "js-", "au-", "aui-", "clap-", "clapi-", "lv2-", "dx-"] {
        if let Some(rest) = stem.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    stem.to_string()
}

/// Renoise's device names, by the type its files keep (`CompressorDevice`), as its device list shows them.
static RENOISE_DEVICES: LazyLock<std::collections::HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    [
        ("AnalogFilterDevice", "Analog Filter"),
        ("BusCompressorDevice", "Bus Compressor"),
        ("CabinetSimulatorDevice", "Cabinet Simulator"),
        ("Chorus2Device", "Chorus 2"),
        ("Comb2Device", "Comb Filter 2"),
        ("ConvolverDevice", "Convolver"),
        ("DcOffsetDevice", "DC Offset"),
        ("Distortion2Device", "Distortion 2"),
        ("Eq10Device", "EQ 10"),
        ("Eq5Device", "EQ 5"),
        ("Flanger2Device", "Flanger 2"),
        ("FormulaMetaDevice", "*Formula"),
        ("Gate2Device", "Gate 2"),
        ("MidiControlDevice", "*Instr. MIDI Control"),
        ("InstrumentMacroDevice", "*Instr. Macros"),
        ("LfoDevice", "*LFO"),
        ("Lofi2Device", "LofiMat 2"),
        ("MixerEqDevice", "Mixer EQ"),
        ("Reverb3Device", "mpReverb 2"),
        ("Phaser2Device", "Phaser 2"),
        ("RingMod2Device", "RingMod 2"),
        ("FilterDistortionDevice", "Scream Filter"),
        ("SignalFollowerDevice", "*Signal Follower"),
        ("VelocityDevice", "*Velocity Tracker"),
        ("KeyTrackingDevice", "*Key Tracker"),
        ("HydraDevice", "*Hydra"),
        ("MetaMixerDevice", "*Meta Mixer"),
        ("XYPadDevice", "*XY Pad"),
        ("SendDevice", "#Send"),
        ("MultiBandSendDevice", "#Multiband Send"),
        ("SidechainDevice", "#Sidechain"),
    ]
    .into_iter()
    .collect()
});

/// A Renoise device's name from its type: the list's name, or the type in words ("StereoExpanderDevice": "Stereo Expander").
pub fn renoise_device_name(kind: &str) -> String {
    if let Some(name) = RENOISE_DEVICES.get(kind) {
        return name.to_string();
    }
    device_name(kind.strip_suffix("Device").unwrap_or(kind))
}

static TYPE_ATTRIBUTE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r#"<([A-Za-z0-9]+) type="([A-Za-z0-9]+)""#).unwrap());

/// The devices right inside a Renoise chain's `<Devices>` (not those inside a Doofer's own chain), by name, in order.
fn renoise_chain(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut depth = 0i32;
    let mut devices_at = None;
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end.min(rest.len())];
        if tag.starts_with("</") {
            depth -= 1;
            if devices_at == Some(depth) && tag.starts_with("</Devices") {
                devices_at = None;
            }
        } else if !tag.starts_with("<?") && !tag.starts_with("<!") {
            let empty = tag.ends_with('/');
            if devices_at == Some(depth - 1) {
                if let Some(found) = TYPE_ATTRIBUTE.captures(&format!("{tag}>")) {
                    names.push(renoise_device_name(&found[2]));
                }
            }
            if tag.starts_with("<Devices") && !empty && devices_at.is_none() {
                devices_at = Some(depth);
            }
            if !empty {
                depth += 1;
            }
        }
        rest = &rest[end.min(rest.len())..];
        if rest.is_empty() {
            break;
        }
        rest = &rest[1.min(rest.len())..];
    }
    names
}

/// An element's text (`<Name>Set 01</Name>`), the first one.
fn element(text: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&format!("</{name}>"))?;
    let value = trim(&text[start..start + end]).to_string();
    (!value.is_empty()).then_some(value)
}

/// The sample files a Renoise instrument holds, from its zip's directory (`SampleData/Sample00 (Kick).flac`): names only,
/// nothing unpacked. None when it isn't a zip Kumi reads.
async fn xrni_samples(path: &str) -> io::Result<Vec<String>> {
    let mut file = tokio::fs::File::open(path).await?;
    let len = file.metadata().await?.len();
    zip_samples(&mut file, len).await
}

/// The most of a zip's directory Kumi reads (tens of thousands of names).
const MOST_DIRECTORY: u64 = 8 << 20;

/// `xrni_samples` from a zip of `len` bytes: only its end record and its directory are read, never the samples (a
/// multi-sampled instrument can be hundreds of MB, and the learner reads one after another).
async fn zip_samples<R: AsyncRead + AsyncSeek + Unpin>(zip: &mut R, len: u64) -> io::Result<Vec<String>> {
    let mut tail = vec![0; len.min(65_557) as usize];
    zip.seek(SeekFrom::Start(len - tail.len() as u64)).await?;
    zip.read_exact(&mut tail).await?;
    let end = tail.windows(4).rposition(|w| w == b"PK\x05\x06").ok_or_else(|| invalid("not a zip"))?;
    let record = &tail[end..];
    if record.len() < 22 {
        return Err(invalid("not a zip"));
    }
    let count = u16::from_le_bytes([record[10], record[11]]) as usize;
    let size = u32::from_le_bytes([record[12], record[13], record[14], record[15]]) as u64;
    let start = u32::from_le_bytes([record[16], record[17], record[18], record[19]]) as u64;
    if size > MOST_DIRECTORY || start + size > len {
        return Err(invalid("a cut zip"));
    }
    let mut bytes = vec![0; size as usize];
    zip.seek(SeekFrom::Start(start)).await?;
    zip.read_exact(&mut bytes).await?;
    let mut at = 0;
    let mut names = Vec::new();
    for _ in 0..count {
        let header = bytes.get(at..at + 46).ok_or_else(|| invalid("a cut zip"))?;
        if header[..4] != *b"PK\x01\x02" {
            return Err(invalid("a damaged zip"));
        }
        let name_len = u16::from_le_bytes([header[28], header[29]]) as usize;
        let extra = u16::from_le_bytes([header[30], header[31]]) as usize;
        let comment = u16::from_le_bytes([header[32], header[33]]) as usize;
        let name = bytes.get(at + 46..at + 46 + name_len).ok_or_else(|| invalid("a cut zip"))?;
        let name = String::from_utf8_lossy(name);
        if let Some(file) = name.strip_prefix("SampleData/") {
            let stem = file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(file);
            // "Sample00 (Kick)": the sample's own name is in the brackets.
            let own = stem.split_once(" (").and_then(|(_, rest)| rest.strip_suffix(')')).unwrap_or(stem);
            names.push(own.to_string());
        }
        at += 46 + name_len + extra + comment;
    }
    Ok(names)
}

/// What a REAPER or Renoise preset holds, by its format (as the library keeps it); `relative` is its path inside its library.
pub async fn read_daw_preset(format: &str, path: &str, relative: &str) -> io::Result<PresetFacts> {
    use PresetCategory::*;
    let parts: Vec<&str> = relative.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    let text = || xml_head(Path::new(path), DAW_HEAD);
    Ok(match format {
        "rfxchain" => {
            let (fx, _) = reaper_text(&text().await?);
            PresetFacts {
                category: Some(FxChain),
                device: fx.first().cloned(),
                inside: (!fx.is_empty()).then(|| fx.into_iter().take(12).collect()),
                ..Default::default()
            }
        }
        "rtracktemplate" | "rpp" => {
            let (fx, tracks) = reaper_text(&text().await?);
            let lead = if tracks.len() == 1 { "1 track: ".to_string() } else { format!("{} tracks: ", tracks.len()) };
            PresetFacts {
                category: Some(if format == "rpp" { ProjectTemplate } else { TrackTemplate }),
                inside: (!fx.is_empty()).then(|| fx.into_iter().collect::<IndexSet<_>>().into_iter().take(12).collect()),
                about: listed(&lead, &tracks),
                ..Default::default()
            }
        }
        "jsfx" => {
            let (desc, about) = jsfx_text(&text().await?).ok_or_else(|| invalid("not a JSFX (no desc line)"))?;
            // REAPER adds a JSFX by its path inside Effects.
            let inside = parts.iter().skip(1).copied().collect::<Vec<_>>().join("/");
            PresetFacts { name: Some(desc), load: Some(format!("JS: {inside}")), category: Some(Jsfx), about, ..Default::default() }
        }
        "ini" => {
            let presets = preset_bank(&text().await?);
            if presets.is_empty() {
                return Err(invalid("no presets in it"));
            }
            let plugin = bank_plugin(relative);
            PresetFacts {
                name: Some(format!("{plugin} presets")),
                category: Some(FxPresets),
                device: Some(plugin),
                about: Some(format!("{} preset{}", presets.len(), if presets.len() == 1 { "" } else { "s" })),
                inside: Some(presets.into_iter().take(256).collect()),
                ..Default::default()
            }
        }
        "xrni" => {
            let samples = xrni_samples(path).await?;
            let lead = if samples.len() == 1 { "1 sample: ".to_string() } else { format!("{} samples: ", samples.len()) };
            PresetFacts { category: Some(Instrument), about: listed(&lead, &samples), ..Default::default() }
        }
        "sfz" => {
            let text = text().await?;
            let regions = text.matches("<region>").count();
            PresetFacts {
                category: Some(MultiSample),
                about: (regions > 0).then(|| format!("{regions} zone{}", if regions == 1 { "" } else { "s" })),
                ..Default::default()
            }
        }
        "xrnt" => {
            let devices = renoise_chain(&text().await?);
            PresetFacts {
                category: Some(EffectChain),
                inside: (!devices.is_empty()).then(|| devices.into_iter().take(12).collect()),
                ..Default::default()
            }
        }
        "xrdp" => {
            let text = text().await?;
            // In "Effect Presets/<device>/", the folder is the device's name as Renoise lists it.
            let device = if parts.len() >= 3 { Some(parts[parts.len() - 2].to_string()) } else { None }
                .or_else(|| TYPE_ATTRIBUTE.captures(&text).map(|found| renoise_device_name(&found[2])));
            let inside = renoise_chain(&text);
            PresetFacts {
                category: Some(EffectPreset),
                device,
                inside: (!inside.is_empty()).then(|| inside.into_iter().take(12).collect()),
                ..Default::default()
            }
        }
        "xrnz" => {
            let text = text().await?;
            PresetFacts { category: Some(Phrase), about: element(&text, "SelectedPresetName").map(|n| head(&n, 200)), ..Default::default() }
        }
        "lua" => {
            // A phrase script's first comment lines say what it does.
            let text = text().await?;
            let notes: Vec<&str> = text
                .lines()
                .map(str::trim)
                .take_while(|l| l.starts_with("--") || l.is_empty())
                .filter_map(|l| l.strip_prefix("--"))
                .map(|l| l.trim_start_matches('-').trim())
                .filter(|l| !l.is_empty())
                .collect();
            PresetFacts {
                category: Some(PhraseScript),
                about: (!notes.is_empty()).then(|| head(&notes.join(" "), 200)),
                ..Default::default()
            }
        }
        "xrno" => {
            let text = text().await?;
            PresetFacts { category: Some(ModulationSet), about: element(&text, "Name").map(|n| head(&n, 200)), ..Default::default() }
        }
        _ => PresetFacts::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    /// A zip in memory that counts the bytes read from it.
    struct Counted {
        zip: io::Cursor<Vec<u8>>,
        read: usize,
    }
    impl AsyncRead for Counted {
        fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> Poll<io::Result<()>> {
            let before = buf.filled().len();
            let polled = Pin::new(&mut self.zip).poll_read(cx, buf);
            self.read += buf.filled().len() - before;
            polled
        }
    }
    impl AsyncSeek for Counted {
        fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
            Pin::new(&mut self.zip).start_seek(position)
        }
        fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
            Pin::new(&mut self.zip).poll_complete(cx)
        }
    }

    /// A stored zip of these files.
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut out, mut central) = (Vec::new(), Vec::new());
        for (name, data) in files {
            let at = out.len() as u32;
            let header = |signature: &[u8], central: bool| {
                let mut h = signature.to_vec();
                if central {
                    h.extend([20, 0]);
                }
                h.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                h.extend((data.len() as u32).to_le_bytes());
                h.extend((data.len() as u32).to_le_bytes());
                h.extend((name.len() as u16).to_le_bytes());
                h.extend([0, 0]);
                if central {
                    h.extend([0; 10]);
                    h.extend(at.to_le_bytes());
                }
                h.extend(name.as_bytes());
                h
            };
            out.extend(header(b"PK\x03\x04", false));
            out.extend(*data);
            central.extend(header(b"PK\x01\x02", true));
        }
        let start = out.len() as u32;
        out.extend(&central);
        out.extend(b"PK\x05\x06\0\0\0\0");
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((central.len() as u32).to_le_bytes());
        out.extend(start.to_le_bytes());
        out.extend([0, 0]);
        out
    }

    /// (#361 review K32) A Renoise instrument's samples are listed from its zip's end and directory alone: its sample
    /// data, most of the file, is never read.
    #[tokio::test]
    async fn an_instruments_samples_are_listed_without_reading_their_data() {
        let kick = vec![7u8; 4 << 20];
        let bytes = zip(&[
            ("Instrument.xml", b"<RenoiseInstrument/>"),
            ("SampleData/Sample00 (Kick).flac", &kick),
            ("SampleData/Sample01 (Snare).flac", &kick),
        ]);
        let len = bytes.len() as u64;
        let mut counted = Counted { zip: io::Cursor::new(bytes), read: 0 };
        assert_eq!(zip_samples(&mut counted, len).await.unwrap(), ["Kick", "Snare"]);
        assert!(counted.read < 70_000, "read {} of {len} bytes", counted.read);
    }
}
