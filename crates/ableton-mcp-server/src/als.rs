//! Bounded offline Live Set XML parsing, structural extraction, MIDI, and findings-only lint.
use crate::{
    project::{decode_xml_attribute, read_set_source, ProjectError, SetSourceRead},
    project_semantic::{semantic_project_name, JS_SPACE},
};
use kumi_common::js::{json as js_json, number, string};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::LazyLock,
};
fn fail(s: impl Into<String>) -> ProjectError {
    ProjectError(s.into())
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlsXmlNode {
    pub tag: String,
    pub attrs: BTreeMap<String, String>,
    pub children: Vec<AlsXmlNode>,
    pub text: String,
}
fn decode_xml_text(value: &str) -> Result<String, ProjectError> {
    static ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^&(?:amp;|lt;|gt;|quot;|apos;|#[0-9]+;|#x[0-9a-fA-F]+;)").unwrap());
    for (i, _) in value.match_indices('&') {
        if !ENTITY.is_match(&value[i..]) {
            return Err(fail("Live Set XML contains an unsupported entity"));
        }
    }
    decode_xml_attribute(value)
}
pub fn parse_als_xml(xml: &str) -> Result<AlsXmlNode, ProjectError> {
    static FORBIDDEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<!DOCTYPE|<!ENTITY").unwrap());
    static TAG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            &r#"^<(/?)([A-Za-z_][A-Za-z0-9_.-]*)((?:\s+[A-Za-z_][A-Za-z0-9_.-]*\s*=\s*(?:"[^"<]*"|'[^'<]*'))*)\s*(/?)>"#
                .replace(r"\s", JS_SPACE),
        )
        .unwrap()
    });
    static ATTR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(&r#"([A-Za-z_][A-Za-z0-9_.-]*)\s*=\s*(?:"([^"]*)"|'([^']*)')"#.replace(r"\s", JS_SPACE)).unwrap());
    if FORBIDDEN.is_match(xml) {
        return Err(fail("Live Set XML must not contain DOCTYPE or ENTITY declarations"));
    }
    if string::utf16_len(xml) > 64 * 1024 * 1024 {
        return Err(fail("Live Set XML exceeds the bounded size"));
    }
    let mut stack: Vec<AlsXmlNode> = vec![];
    // Each open element's text length in UTF-16 units so far: text split by comments or CDATA is measured as it
    // comes, not counted again from its start for every piece.
    let mut lengths: Vec<usize> = vec![];
    let mut root = None;
    let mut nodes = 0;
    let mut cursor = 0;
    fn push_text(stack: &mut [AlsXmlNode], lengths: &mut [usize], raw: &str, cdata: bool) -> Result<(), ProjectError> {
        if string::trim(raw).is_empty() {
            return Ok(());
        }
        let parent = stack.last_mut().ok_or_else(|| fail("Live Set XML is malformed (text outside the root)"))?;
        let Some(length) = lengths.last_mut() else { return Err(fail("Live Set XML is malformed (text outside the root)")) };
        if string::utf16_len(raw) + *length > 1024 * 1024 {
            return Err(fail("Live Set XML text node exceeds the bounded size"));
        }
        let text = if cdata { raw.to_owned() } else { decode_xml_text(raw)? };
        *length += string::utf16_len(&text);
        parent.text.push_str(&text);
        Ok(())
    }
    fn append(stack: &mut [AlsXmlNode], root: &mut Option<AlsXmlNode>, node: AlsXmlNode) -> Result<(), ProjectError> {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(node);
        } else if root.is_none() {
            *root = Some(node);
        } else {
            return Err(fail("Live Set XML has multiple roots"));
        }
        Ok(())
    }
    while cursor < xml.len() {
        let rest = &xml[cursor..];
        if !rest.starts_with('<') {
            let end = rest.find('<').map_or(xml.len(), |n| cursor + n);
            push_text(&mut stack, &mut lengths, &xml[cursor..end], false)?;
            cursor = end;
            continue;
        }
        if rest.starts_with("<!--") {
            let end = rest[4..].find("-->").map(|n| cursor + 4 + n).ok_or_else(|| fail("Live Set XML is malformed (comment)"))?;
            if xml[cursor + 4..end].contains("--") {
                return Err(fail("Live Set XML is malformed (comment)"));
            }
            cursor = end + 3;
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            let end = rest[9..].find("]]>").map(|n| cursor + 9 + n).ok_or_else(|| fail("Live Set XML is malformed (CDATA)"))?;
            push_text(&mut stack, &mut lengths, &xml[cursor + 9..end], true)?;
            cursor = end + 3;
            continue;
        }
        if rest.starts_with("<?xml ") && root.is_none() && stack.is_empty() && string::trim(&xml[..cursor]).is_empty() {
            let end = rest[6..].find("?>").map(|n| cursor + 6 + n).ok_or_else(|| fail("Live Set XML is malformed (declaration)"))?;
            cursor = end + 2;
            continue;
        }
        let m = TAG.captures(rest).ok_or_else(|| fail("Live Set XML is malformed or contains unsupported markup"))?;
        cursor += m.get(0).unwrap().len();
        let closing = &m[1];
        let tag = &m[2];
        let attrs = &m[3];
        let self_closing = &m[4];
        if closing == "/" {
            if !string::trim(attrs).is_empty() || !self_closing.is_empty() {
                return Err(fail("Live Set XML is malformed (closing tag)"));
            }
            let node = stack
                .pop()
                .filter(|node| node.tag == tag)
                .ok_or_else(|| fail(format!("Live Set XML is malformed (unexpected </{tag}>)")))?;
            lengths.pop();
            append(&mut stack, &mut root, node)?;
            continue;
        }
        if nodes >= 400_000 {
            return Err(fail("Live Set XML exceeds the bounded node count"));
        }
        nodes += 1;
        if stack.len() >= 64 {
            return Err(fail("Live Set XML exceeds the bounded depth"));
        }
        let pairs: Vec<_> = ATTR.captures_iter(attrs).collect();
        if pairs.len() > 64 {
            return Err(fail("Live Set XML element exceeds the bounded attribute count"));
        }
        let mut attributes = BTreeMap::new();
        for pair in pairs {
            let name = pair[1].to_owned();
            let value = pair.get(2).or_else(|| pair.get(3)).unwrap().as_str();
            if attributes.contains_key(&name) {
                return Err(fail("Live Set XML is malformed (duplicate attribute)"));
            }
            if string::utf16_len(value) > 1024 * 1024 {
                return Err(fail("Live Set XML attribute exceeds the bounded size"));
            }
            attributes.insert(name, decode_xml_text(value)?);
        }
        let node = AlsXmlNode { tag: tag.into(), attrs: attributes, children: vec![], text: String::new() };
        if self_closing == "/" {
            append(&mut stack, &mut root, node)?;
        } else {
            stack.push(node);
            lengths.push(0);
        }
    }
    if !stack.is_empty() {
        return Err(fail("Live Set XML is malformed (unclosed elements)"));
    }
    root.ok_or_else(|| fail("Live Set XML has no root element"))
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AlsClipModel {
    pub name: String,
    pub kind: String,
    pub start: f64,
    pub length: Option<f64>,
    pub loop_start: Option<f64>,
    pub loop_end: Option<f64>,
    pub looping: Option<bool>,
    pub muted: Option<bool>,
    pub warping: Option<bool>,
    pub sample_path: Option<String>,
    pub sample_length_beats: Option<f64>,
    pub warp_marker_count: usize,
    pub notes: Vec<Value>,
    pub lane: String,
    pub scene_index: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AlsTrackModel {
    pub name: String,
    pub kind: String,
    pub color_index: Option<f64>,
    pub volume: Option<f64>,
    pub pan: Option<f64>,
    pub devices: Vec<AlsDeviceModel>,
    pub clips: Vec<AlsClipModel>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AlsDeviceModel {
    pub name: String,
    pub class_name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlsSceneModel {
    pub name: String,
    pub tempo: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlsLocatorModel {
    pub time: f64,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AlsModel {
    pub set_name: String,
    pub tempo: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub major_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minor_version: Option<String>,
    pub tracks: Vec<AlsTrackModel>,
    pub scenes: Vec<AlsSceneModel>,
    pub locators: Vec<AlsLocatorModel>,
    pub parse_notes: Vec<String>,
}
fn attr_number(node: &AlsXmlNode, name: &str) -> Option<f64> {
    node.attrs.get(name).and_then(|s| number::parse(s)).filter(|n| n.is_finite())
}
fn child<'a>(node: &'a AlsXmlNode, tag: &str) -> Option<&'a AlsXmlNode> {
    node.children.iter().find(|n| n.tag == tag)
}
fn child_value<'a>(node: &'a AlsXmlNode, tag: &str) -> Option<&'a str> {
    let child = child(node, tag)?;
    let value = child.attrs.get("Value").map(String::as_str).unwrap_or(&child.text);
    (!value.is_empty()).then_some(value)
}
fn child_number(node: &AlsXmlNode, tag: &str) -> Option<f64> {
    child_value(node, tag).filter(|s| !string::trim(s).is_empty()).and_then(number::parse).filter(|n| n.is_finite())
}
fn descendants<'a>(node: &'a AlsXmlNode, tag: &str) -> Vec<&'a AlsXmlNode> {
    fn visit<'a>(node: &'a AlsXmlNode, tag: &str, into: &mut Vec<&'a AlsXmlNode>) {
        for n in &node.children {
            if n.tag == tag {
                into.push(n);
            }
            visit(n, tag, into);
        }
    }
    let mut into = vec![];
    visit(node, tag, &mut into);
    into
}
fn first<'a>(node: &'a AlsXmlNode, tag: &str) -> Option<&'a AlsXmlNode> {
    for n in &node.children {
        if n.tag == tag {
            return Some(n);
        }
        if let Some(found) = first(n, tag) {
            return Some(found);
        }
    }
    None
}
fn parse_note(event: &AlsXmlNode, key: &AlsXmlNode) -> Option<Value> {
    let pitch = attr_number(key, "MidiKey").or_else(|| child(key, "MidiKey").and_then(|n| attr_number(n, "Value")))?;
    let start = attr_number(event, "Time")?;
    let duration = attr_number(event, "Duration")?;
    let velocity = attr_number(event, "Velocity")?;
    if pitch.fract() != 0. || !(0. ..=127.).contains(&pitch) || start < 0. || duration <= 0. {
        return None;
    }
    let probability = attr_number(event, "Probability").filter(|p| (0. ..=1.).contains(p)).unwrap_or(1.);
    Some(
        json!({"pitch":pitch,"start":start,"duration":duration,"velocity":number::round(velocity).clamp(1.,127.),"channel":1,"mute":event.attrs.get("IsEnabled").is_some_and(|s|s=="false"),"probability":probability,"velocityDeviation":attr_number(event,"VelocityDeviation").unwrap_or(0.),"releaseVelocity":attr_number(event,"OffVelocity").map(|v|number::round(v).clamp(0.,127.)).unwrap_or(64.)}),
    )
}
fn parse_clip(clip: &AlsXmlNode, lane: &str, scene_index: Option<usize>, parse_notes: &mut Vec<String>) -> AlsClipModel {
    let midi = clip.tag == "MidiClip";
    let start = attr_number(clip, "Time").or_else(|| child_number(clip, "CurrentStart")).unwrap_or(0.);
    let current_start = child_number(clip, "CurrentStart").unwrap_or(start);
    let current_end = child_number(clip, "CurrentEnd").or_else(|| attr_number(clip, "CurrentEnd"));
    let length = attr_number(clip, "Length")
        .or_else(|| child_number(clip, "Length"))
        .or_else(|| current_end.map(|end| end - current_start))
        .filter(|n| *n >= 0.);
    if length.is_none() {
        parse_notes.push("clip length is unavailable in this XML shape".into());
    }
    let mut notes = vec![];
    if midi {
        let mut dropped = 0;
        for key in descendants(clip, "KeyTrack") {
            let mut events = descendants(key, "MidiNoteEvent");
            events.extend(descendants(key, "NoteEvent"));
            let remaining = 10_000usize.saturating_sub(notes.len());
            if events.len() > remaining {
                dropped += events.len() - remaining;
                events.truncate(remaining);
            }
            for event in events {
                if let Some(note) = parse_note(event, key) {
                    notes.push(note);
                } else {
                    dropped += 1;
                }
            }
        }
        if dropped > 0 {
            parse_notes.push(format!("{dropped} malformed or overflow note event(s) dropped from a clip"));
        }
    }
    let sample_path =
        first(clip, "FileRef").and_then(|n| child_value(n, "Path").or_else(|| child_value(n, "RelativePath"))).map(str::to_owned);
    let loop_node = child(clip, "Loop");
    let loop_on = loop_node.and_then(|n| child_value(n, "LoopOn"));
    let warping = child_value(clip, "IsWarped").or_else(|| child_value(clip, "Warping"));
    AlsClipModel {
        name: child_value(clip, "Name").unwrap_or("").into(),
        kind: if midi { "midi" } else { "audio" }.into(),
        start,
        length,
        loop_start: loop_node.and_then(|n| child_number(n, "LoopStart").or_else(|| attr_number(n, "LoopStart"))),
        loop_end: loop_node.and_then(|n| child_number(n, "LoopEnd").or_else(|| attr_number(n, "LoopEnd"))),
        looping: loop_on.map(|s| s == "true" || s == "1"),
        muted: child_value(clip, "Disabled").or_else(|| clip.attrs.get("Disabled").map(String::as_str)).map(|s| s == "true"),
        warping: warping.map(|s| s == "true" || s == "1"),
        sample_path,
        sample_length_beats: attr_number(clip, "SampleLength"),
        warp_marker_count: descendants(clip, "WarpMarker").len(),
        notes,
        lane: lane.into(),
        scene_index,
    }
}
pub fn model_from_als_xml(root: &AlsXmlNode, fallback_name: &str) -> Result<AlsModel, ProjectError> {
    if root.tag != "Ableton" {
        return Err(fail("Live Set XML root is not <Ableton>"));
    }
    let live_set = child(root, "LiveSet").unwrap_or(root);
    let mut parents: HashMap<*const AlsXmlNode, &AlsXmlNode> = HashMap::new();
    fn index<'a>(node: &'a AlsXmlNode, map: &mut HashMap<*const AlsXmlNode, &'a AlsXmlNode>) {
        for child in &node.children {
            map.insert(child as *const _, node);
            index(child, map);
        }
    }
    index(live_set, &mut parents);
    let mut parse_notes = vec![];
    let mut tracks = vec![];
    let elements = first(live_set, "Tracks")
        .into_iter()
        .flat_map(|n| n.children.iter())
        .chain(live_set.children.iter().filter(|n| n.tag == "MasterTrack" || n.tag == "MainTrack"));
    for element in elements {
        let kind = match element.tag.as_str() {
            "MidiTrack" => "midi",
            "AudioTrack" => "audio",
            "GroupTrack" => "group",
            "ReturnTrack" => "return",
            "MasterTrack" | "MainTrack" => "main",
            _ => continue,
        };
        let name_node = child(element, "Name").unwrap_or(element);
        let name = child_value(name_node, "EffectiveName").or_else(|| child_value(name_node, "UserName")).unwrap_or("").to_owned();
        let color = attr_number(child(element, "Color").unwrap_or(element), "Value").filter(|n| (0. ..=69.).contains(n));
        let mixer = first(element, "Mixer");
        let mixer_number = |tag| mixer.and_then(|n| first(n, tag)).and_then(|n| attr_number(child(n, "Manual").unwrap_or(n), "Value"));
        let devices = first(element, "Devices")
            .into_iter()
            .flat_map(|n| n.children.iter())
            .take(256)
            .map(|n| AlsDeviceModel {
                class_name: n.tag.clone(),
                name: child_value(n, "UserName").or_else(|| child_value(n, "EffectiveName")).unwrap_or(&n.tag).to_owned(),
            })
            .collect();
        let mut clips = vec![];
        for (order, slot) in descendants(element, "ClipSlot")
            .into_iter()
            .filter(|slot| parents.get(&(*slot as *const _)).is_none_or(|n| n.tag != "ClipSlot"))
            .enumerate()
        {
            if let Some(clip) = slot
                .children
                .iter()
                .find(|n| n.tag == "MidiClip" || n.tag == "AudioClip")
                .or_else(|| first(slot, "MidiClip"))
                .or_else(|| first(slot, "AudioClip"))
            {
                clips.push(parse_clip(clip, "session", Some(order), &mut parse_notes));
            }
        }
        for clip in descendants(element, "MidiClip").into_iter().chain(descendants(element, "AudioClip")) {
            let mut ancestor = parents.get(&(clip as *const _)).copied();
            let mut slot = false;
            let mut arrangement = false;
            while let Some(n) = ancestor {
                slot |= n.tag == "ClipSlot";
                arrangement |= matches!(n.tag.as_str(), "ArrangerAutomation" | "Events" | "Arrangement");
                ancestor = parents.get(&(n as *const _)).copied();
            }
            if !slot && arrangement {
                clips.push(parse_clip(clip, "arrangement", None, &mut parse_notes));
            }
        }
        if tracks.len() >= 512 {
            parse_notes.push("track collection truncated at the 512-track bound".into());
            break;
        }
        tracks.push(AlsTrackModel {
            name,
            kind: kind.into(),
            color_index: color,
            volume: mixer_number("Volume"),
            pan: mixer_number("Pan"),
            devices,
            clips,
        });
    }
    let scenes = descendants(live_set, "Scene")
        .into_iter()
        .take(1024)
        .map(|scene| AlsSceneModel {
            name: child_value(scene, "Name").unwrap_or("").into(),
            tempo: first(scene, "Tempo").and_then(|n| attr_number(child(n, "Manual").unwrap_or(n), "Value")),
        })
        .collect();
    let mut locators: Vec<_> = descendants(live_set, "Locator")
        .into_iter()
        .take(1024)
        .filter_map(|n| {
            child_number(n, "Time")
                .or_else(|| attr_number(n, "Time"))
                .filter(|t| *t >= 0.)
                .map(|time| AlsLocatorModel { time, name: child_value(n, "Name").unwrap_or("").into() })
        })
        .collect();
    locators.sort_by(|a, b| a.time.total_cmp(&b.time));
    let main = live_set.children.iter().find(|n| n.tag == "MasterTrack" || n.tag == "MainTrack");
    let mixer = main.and_then(|n| first(n, "Mixer"));
    let tempo_node = if let Some(mixer) = mixer { child(mixer, "Tempo") } else { child(live_set, "Tempo") };
    let tempo = tempo_node.and_then(|n| attr_number(child(n, "Manual").unwrap_or(n), "Value")).filter(|n| (20. ..=999.).contains(n));
    Ok(AlsModel {
        set_name: fallback_name.into(),
        tempo,
        creator: root.attrs.get("Creator").cloned(),
        major_version: root.attrs.get("MajorVersion").cloned(),
        minor_version: root.attrs.get("MinorVersion").cloned(),
        tracks,
        scenes,
        locators,
        parse_notes,
    })
}
#[derive(Debug, Clone, Default)]
pub struct AlsLintOptions {
    pub allowed_root: Option<String>,
    pub set_directory: Option<String>,
    pub max_findings: Option<f64>,
}
fn resolve(path: impl AsRef<Path>) -> Result<PathBuf, ProjectError> {
    crate::command::resolve(path).map_err(|e| fail(e.message()))
}
/// What a path is, its own link not followed: kept for the rest of one lint, whose references share folders.
#[derive(Clone, Copy)]
enum Entry {
    Directory,
    Other,
    Link,
    Missing,
    Unreadable,
}
fn entry(path: &Path, seen: &mut HashMap<PathBuf, Entry>) -> Entry {
    if let Some(found) = seen.get(path) {
        return *found;
    }
    let found = match std::fs::symlink_metadata(path) {
        Ok(stats) if stats.is_symlink() => Entry::Link,
        Ok(stats) if stats.is_dir() => Entry::Directory,
        Ok(_) => Entry::Other,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Entry::Missing,
        Err(_) => Entry::Unreadable,
    };
    seen.insert(path.to_owned(), found);
    found
}
fn media_exists_without_links(root: &Path, candidate: &Path, seen: &mut HashMap<PathBuf, Entry>) -> Option<bool> {
    let mut current = root.to_owned();
    let components: Vec<_> = candidate.strip_prefix(root).ok()?.components().collect();
    for index in 0..=components.len() {
        if index > 0 {
            current.push(components[index - 1]);
        }
        match entry(&current, seen) {
            Entry::Link | Entry::Unreadable => return None,
            Entry::Missing => return Some(false),
            Entry::Other if index < components.len() => return None,
            Entry::Directory | Entry::Other => {}
        }
    }
    Some(true)
}
pub fn lint_als_model(model: &AlsModel, options: &AlsLintOptions) -> Result<Value, ProjectError> {
    static WINDOWS_DRIVE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z]:[\\/]").unwrap());
    let mut findings = vec![];
    let mut truncated = false;
    let mut push = |severity: &str, check: &str, message: String, kind: &str, name: &str, index: Option<usize>| {
        if findings.len() as f64 >= options.max_findings.unwrap_or(512.) {
            truncated = true;
            return;
        }
        findings.push(json!({"severity":severity,"check":check,"message":message,"object":{"kind":kind,"name":name,"index":index}}));
    };
    let last_locator = model.locators.last().map(|l| l.time);
    let media_root = options.allowed_root.as_ref().map(resolve).transpose()?;
    let mut seen = HashMap::new();
    let mut names: HashMap<&str, usize> = HashMap::new();
    for track in &model.tracks {
        if !track.name.is_empty() {
            *names.entry(&track.name).or_default() += 1;
        }
    }
    let arrangement_count = model.tracks.iter().flat_map(|t| &t.clips).filter(|c| c.lane == "arrangement").count();
    if arrangement_count > 500 {
        push(
            "warning",
            "oversized-arrangement",
            format!("{arrangement_count} arrangement clips exceed the 500-clip review bound"),
            "set",
            &model.set_name,
            None,
        );
    }
    for (track_index, track) in model.tracks.iter().enumerate() {
        if !track.name.is_empty() && names[track.name.as_str()] > 1 {
            push(
                "info",
                "duplicate-track-name",
                format!("track name \"{}\" appears {} times", track.name, names[track.name.as_str()]),
                "track",
                &track.name,
                Some(track_index),
            );
        }
        if track.clips.is_empty() && track.devices.is_empty() {
            push(
                "info",
                "empty-track",
                format!("track \"{}\" has no clips and no devices", track.name),
                "track",
                &track.name,
                Some(track_index),
            );
        }
        for (clip_index, clip) in track.clips.iter().enumerate() {
            if let Some(last) = last_locator.filter(|last| clip.lane == "arrangement" && clip.start > *last) {
                push(
                    "info",
                    "clip-beyond-last-locator",
                    format!(
                        "clip \"{}\" starts at {} beats, beyond the last locator at {}",
                        clip.name,
                        js_json::stringify(&json!(clip.start)),
                        js_json::stringify(&json!(last))
                    ),
                    "clip",
                    &clip.name,
                    Some(clip_index),
                );
            }
            if clip.kind == "audio" && clip.warping == Some(false) {
                if let Some(length) = clip.sample_length_beats.filter(|n| *n > 60.) {
                    push(
                        "warning",
                        "unwarped-long-sample",
                        format!("audio clip \"{}\" is not warped over a {}-beat sample", clip.name, js_json::stringify(&json!(length))),
                        "clip",
                        &clip.name,
                        Some(clip_index),
                    );
                }
            }
            if let (Some(path), Some(root)) = (clip.sample_path.as_ref().filter(|s| !s.is_empty()), media_root.as_ref()) {
                if path.contains('\0')
                    || path.chars().take(2).filter(|c| *c == '/' || *c == '\\').count() == 2
                    || (!Path::new(path).is_absolute() && WINDOWS_DRIVE.is_match(path))
                {
                    continue;
                }
                let candidate = resolve(options.set_directory.as_ref().map(Path::new).unwrap_or(root).join(path))?;
                if candidate.starts_with(root) && media_exists_without_links(root, &candidate, &mut seen) == Some(false) {
                    push(
                        "error",
                        "missing-sample-reference",
                        format!("referenced sample is missing: {}", candidate.file_name().unwrap_or_default().to_string_lossy()),
                        "clip",
                        &clip.name,
                        Some(clip_index),
                    );
                }
            }
        }
    }
    Ok(json!({"findings":findings,"truncated":truncated}))
}
pub fn extract_als_midi(model: &AlsModel, profile: Option<&str>) -> Value {
    let profile = profile.unwrap_or("collaboration");
    let mut rows = vec![];
    for (track_index, track) in model.tracks.iter().enumerate() {
        for clip in &track.clips {
            if clip.kind != "midi" {
                continue;
            }
            let mut revisions: Vec<String> = clip
                .notes
                .iter()
                .map(|note| {
                    js_json::stringify(&json!([
                        note["pitch"],
                        note["start"],
                        note["duration"],
                        note["velocity"],
                        note["channel"],
                        note["mute"],
                        note["probability"],
                        note["velocityDeviation"],
                        note["releaseVelocity"]
                    ]))
                })
                .collect();
            revisions.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            rows.push(json!({"track":semantic_project_name(profile,"track",&json!(track.name)),"trackIndex":track_index,"clip":semantic_project_name(profile,"clip",&json!(clip.name)),"lane":clip.lane,"sceneIndex":clip.scene_index,"start":clip.start,"notes":clip.notes,"notesRevision":hex::encode(Sha256::digest(js_json::stringify(&json!(revisions))))}));
        }
    }
    json!(rows)
}
pub fn read_als_model(path: &str) -> Result<(SetSourceRead, AlsModel), ProjectError> {
    let source = read_set_source(path)?;
    let name = Path::new(&source.path).file_name().unwrap_or_default().to_string_lossy();
    let model = model_from_als_xml(&parse_als_xml(&source.xml)?, name.strip_suffix(".als").unwrap_or(&name))?;
    Ok((source, model))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineAlsArtifactOptions {
    pub profile: Option<String>,
    pub exporter_version: String,
    pub max_records: Option<f64>,
}
fn snapshot_clip(track: usize, index: usize, clip: &AlsClipModel) -> Value {
    json!({"ref":format!("offline:clip:{track}:{}:{}:{}",clip.lane,clip.scene_index.unwrap_or(index),js_json::stringify(&json!(clip.start))),"name":clip.name,"kind":clip.kind,"start":clip.start,"length":clip.length,"notes":clip.notes,"warp":clip.warping,"takes":[],"automation":[],"loopStart":clip.loop_start,"loopEnd":clip.loop_end,"looping":clip.looping,"muted":clip.muted,"filePath":clip.sample_path,"sampleLength":clip.sample_length_beats})
}
pub fn create_offline_als_artifact(
    source: &SetSourceRead,
    model: &AlsModel,
    options: &OfflineAlsArtifactOptions,
) -> Result<Value, ProjectError> {
    use crate::{
        project::{
            AbletonRootAttributes, ObservedKind, ProjectManifest, ProjectReference, ProjectSourceEvidence, ReferenceBounds,
            ReferenceResolution,
        },
        project_semantic::{create_semantic_project_snapshot, CreateSemanticProjectOptions, SemanticUnavailable},
    };
    if model.tracks.iter().any(|track| track.clips.iter().any(|clip| clip.length.is_none())) {
        return Err(fail("offline clip length is unavailable; semantic export refused"));
    }
    let tracks:Vec<_>=model.tracks.iter().enumerate().map(|(i,track)|{let clips:Vec<_>=track.clips.iter().filter(|c|c.lane=="session").enumerate().map(|(j,c)|snapshot_clip(i,j,c)).collect();let slots:Vec<_>=track.clips.iter().filter(|c|c.lane=="session").enumerate().map(|(j,c)|json!({"ref":format!("offline:slot:{i}:{}",c.scene_index.unwrap_or(j)),"parentRef":format!("offline:track:{i}"),"sceneIndex":c.scene_index.unwrap_or(j),"clipRef":clips[j]["ref"],"empty":false})).collect();let devices:Vec<_>=track.devices.iter().enumerate().map(|(j,d)|json!({"ref":format!("offline:device:{i}:{j}"),"name":d.name,"className":d.class_name,"kind":"device","enabled":null,"parameters":[]})).collect();json!({"ref":format!("offline:track:{i}"),"name":track.name,"kind":track.kind,"volume":track.volume,"pan":track.pan,"mute":null,"solo":null,"armed":null,"clips":clips,"clipSlots":slots,"devices":devices,"sends":[]})}).collect();
    let arrangement: Vec<_> = model
        .tracks
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            t.clips
                .iter()
                .filter(|c| c.lane == "arrangement")
                .map(move |c| json!({"trackRef":format!("offline:track:{i}"),"clip":snapshot_clip(i,0,c)}))
        })
        .collect();
    let snapshot = json!({"set":{"ref":"offline:set","name":model.set_name,"tempo":model.tempo},"tracks":tracks,"scenes":model.scenes.iter().enumerate().map(|(i,s)|json!({"ref":format!("offline:scene:{i}"),"name":s.name,"index":i,"colorIndex":null,"tempo":s.tempo})).collect::<Vec<_>>(),"arrangement":{"length":model.locators.last().map_or(0.,|l|l.time),"locators":model.locators.iter().enumerate().map(|(i,l)|json!({"ref":format!("offline:locator:{i}"),"name":l.name,"position":l.time})).collect::<Vec<_>>()},"arrangementClips":arrangement});
    let mut seen = std::collections::HashSet::new();
    let mut references = vec![];
    let mut complete = model.parse_notes.is_empty();
    'outer: for track in &model.tracks {
        for clip in &track.clips {
            let Some(path) = clip.sample_path.as_ref().filter(|s| !s.is_empty() && !seen.contains(*s)) else {
                continue;
            };
            if seen.len() >= 4096 {
                complete = false;
                break 'outer;
            }
            seen.insert(path.clone());
            references.push(ProjectReference {
                value: path.clone(),
                resolved_path: None,
                exists: None,
                project_local: None,
                resolution: ReferenceResolution::Unresolved,
            });
        }
    }
    let evidence = ProjectSourceEvidence {
        manifest: ProjectManifest {
            path: source.path.clone(),
            size: source.size,
            mtime_ms: source.mtime_ms,
            sha256: source.sha256.clone(),
            tracks: model.tracks.len(),
            scenes: model.scenes.len(),
            media_refs: references.len(),
        },
        ableton: AbletonRootAttributes {
            creator: model.creator.clone(),
            major_version: model.major_version.clone(),
            minor_version: model.minor_version.clone(),
            schema_change_count: None,
        },
        reference_bounds: ReferenceBounds {
            observed: references.len(),
            observed_kind: if complete { ObservedKind::Exact } else { ObservedKind::LowerBound },
            included: references.len(),
            omitted: usize::from(!complete),
            complete,
        },
        references,
    };
    let mut extra = vec![];
    for (field, reason) in [
        ("media-existence", "offline reads do not probe referenced media; lint checks metadata only under its allowed root"),
        (
            "live-playback",
            "playback, armed/monitoring, meters, and performance state exist only in a running Live and are absent from the file",
        ),
        ("take-lanes", "take-lane and comp structure is not reconstructed by the offline parser"),
        ("groove-pool", "groove pool contents are not reconstructed by the offline parser"),
        ("tuning", "tuning system and song scale are not reconstructed by the offline parser"),
    ] {
        extra.push(SemanticUnavailable { field: field.into(), reason: reason.into(), source_name: "offline-parse".into() });
    }
    for note in &model.parse_notes {
        extra.push(SemanticUnavailable { field: "parse-truncation".into(), reason: note.clone(), source_name: "offline-parse".into() });
    }
    create_semantic_project_snapshot(
        &snapshot,
        &CreateSemanticProjectOptions {
            profile: options.profile.clone(),
            exporter_version: options.exporter_version.clone(),
            max_records: options.max_records,
            live: json!({"protocol":"als-file/v1","adapter":"offline-file","provenance":"unknown"}),
            project_path: None,
            source_evidence: Some(evidence),
            source_kind: Some("offline-file".into()),
            extra_unavailable: extra,
        },
    )
}
