//! Max for Live's device file (.amxd): "ampf", the device's type, a "meta" chunk and a "ptch" chunk
//! holding the Max patcher as JSON (NUL-terminated), as Live's own device templates are laid out.

use kumi_common::js::json::stringify_with_indent;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    MidiEffect,
    AudioEffect,
    Instrument,
}

impl DeviceType {
    pub const ALL: [DeviceType; 3] = [DeviceType::MidiEffect, DeviceType::AudioEffect, DeviceType::Instrument];

    pub fn as_str(self) -> &'static str {
        match self {
            DeviceType::MidiEffect => "midi_effect",
            DeviceType::AudioEffect => "audio_effect",
            DeviceType::Instrument => "instrument",
        }
    }

    pub fn parse(text: &str) -> Option<DeviceType> {
        DeviceType::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    /// The four-letter type code Live reads; the same letters, as a number, name it in the patcher's project.
    fn code(self) -> &'static str {
        match self {
            DeviceType::MidiEffect => "mmmm",
            DeviceType::AudioEffect => "aaaa",
            DeviceType::Instrument => "iiii",
        }
    }

    fn from_code(code: &[u8]) -> Option<DeviceType> {
        DeviceType::ALL.into_iter().find(|kind| kind.code().as_bytes() == code)
    }
}

impl std::fmt::Display for DeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn u32le(value: u32) -> [u8; 4] {
    value.to_le_bytes()
}

/// A device file from its patcher.
pub fn encode_amxd(kind: DeviceType, patcher: &Value) -> Vec<u8> {
    let code = kind.code().as_bytes();
    let mut body = format!("{}\n", stringify_with_indent(patcher, "\t")).into_bytes();
    body.push(0);
    let mut bytes = Vec::with_capacity(32 + body.len());
    bytes.extend_from_slice(b"ampf");
    bytes.extend_from_slice(&u32le(code.len() as u32));
    bytes.extend_from_slice(code);
    bytes.extend_from_slice(b"meta");
    bytes.extend_from_slice(&u32le(4));
    bytes.extend_from_slice(&u32le(0));
    bytes.extend_from_slice(b"ptch");
    bytes.extend_from_slice(&u32le(body.len() as u32));
    bytes.extend_from_slice(&body);
    bytes
}

/// A device file's type and patcher (`{ patcher: {...} }`), as [`decode_amxd`] reads them.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAmxd {
    pub kind: DeviceType,
    pub patcher: Value,
}

/// A device file's type and patcher; None when it isn't one.
pub fn decode_amxd(bytes: &[u8]) -> Option<DecodedAmxd> {
    let (code, chunk) = patcher_chunk(bytes)?;
    let kind = DeviceType::from_code(code)?;
    let text = String::from_utf8_lossy(chunk);
    let text = text.trim_end_matches('\0');
    serde_json::from_str::<Value>(text).ok().map(|patcher| DecodedAmxd { kind, patcher })
}

/// A device file's four-letter type code and its "ptch" chunk as it lies in the file: the patcher's JSON, or a frozen
/// device's container of files (see [`super::patch::frozen`]).
pub fn patcher_chunk(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    if bytes.len() < 12 || &bytes[0..4] != b"ampf" {
        return None;
    }
    let read_u32 = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
    let length = read_u32(4);
    let code = &bytes[8..(8 + length).min(bytes.len())];
    let mut at = 8 + length;
    while at + 8 <= bytes.len() {
        let tag = &bytes[at..at + 4];
        let size = read_u32(at + 4);
        if tag == b"ptch" {
            return Some((code, &bytes[(at + 8).min(bytes.len())..(at + 8 + size).min(bytes.len())]));
        }
        at += 8 + size;
    }
    None
}

/// Live shows a device 169 pixels tall: room for three rows of dials.
const FACE_ROWS: usize = 3;

/// Where each of a device's controls sits on its face, from [`face_layout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaceLayout {
    pub columns: usize,
}

impl FaceLayout {
    /// The position of the control at `index`: x, y.
    pub fn at(&self, index: usize) -> (f64, f64) {
        (8.0 + (index % self.columns) as f64 * 52.0, (index / self.columns) as f64 * 52.0)
    }
}

/// Where each of a device's `count` controls sits on its face: in one row up to eight, as Live's own
/// devices have them, then in up to three rows, as wide as it takes.
pub fn face_layout(count: usize) -> FaceLayout {
    let columns = if count <= 8 { count.max(1) } else { count.div_ceil(FACE_ROWS) };
    FaceLayout { columns }
}

/// A box in a patcher: Max's own JSON for it. (TS: `Box`.)
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PatchBox {
    #[serde(rename = "box")]
    pub item: serde_json::Map<String, Value>,
}

impl PatchBox {
    /// A box from `{ ...fields }` (an object).
    pub fn new(item: Value) -> PatchBox {
        match item {
            Value::Object(item) => PatchBox { item },
            _ => PatchBox { item: serde_json::Map::new() },
        }
    }
}

/// A patch cord: from a box's outlet to a box's inlet.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Line {
    pub patchline: PatchLine,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PatchLine {
    pub source: (String, u32),
    pub destination: (String, u32),
}

impl Line {
    pub fn new(source: &str, outlet: u32, destination: &str, inlet: u32) -> Line {
        Line { patchline: PatchLine { source: (source.to_string(), outlet), destination: (destination.to_string(), inlet) } }
    }
}

/// What a device's top-level patcher is made of.
#[derive(Debug, Clone, PartialEq)]
pub struct DevicePatcherOptions {
    pub title: String,
    pub description: String,
    pub width: f64,
    pub boxes: Vec<PatchBox>,
    pub lines: Vec<Line>,
}

/// A device's top-level patcher, as Live's templates have it: opened in presentation (the device's
/// face), `width` pixels wide, with `description` as its info text.
pub fn device_patcher(kind: DeviceType, options: DevicePatcherOptions) -> Value {
    let amxdtype = u32::from_be_bytes(kind.code().as_bytes().try_into().expect("a four-letter code"));
    let project = json!({
        "version": 1, "creationdate": 3590052786u64, "modificationdate": 3590052786u64, "viewrect": [0.0, 0.0, 300.0, 500.0], "autoorganize": 1, "hideprojectwindow": 1,
        "showdependencies": 1, "autolocalize": 0, "contents": { "patchers": {} }, "layout": {}, "searchpath": {}, "detailsvisible": 0, "amxdtype": amxdtype, "readonly": 0, "devpathtype": 0, "devpath": ".",
        "sortmode": 0, "viewmode": 0,
    });
    json!({
        "patcher": {
            "fileversion": 1,
            "appversion": { "major": 9, "minor": 1, "revision": 5, "architecture": "x64", "modernui": 1 },
            "classnamespace": "box",
            "rect": [100.0, 100.0, 900.0, 600.0],
            "openrect": [0.0, 0.0, options.width, 169.0],
            "bglocked": 0,
            "openinpresentation": 1,
            "default_fontsize": 10.0,
            "default_fontface": 0,
            "default_fontname": "Arial Bold",
            "gridonopen": 1,
            "gridsize": [8.0, 8.0],
            "gridsnaponopen": 1,
            "objectsnaponopen": 1,
            "statusbarvisible": 2,
            "toolbarvisible": 1,
            "boxanimatetime": 500,
            "enablehscroll": 1,
            "enablevscroll": 1,
            "devicewidth": options.width,
            "description": options.description,
            "digest": "",
            "tags": "Kumi",
            "style": "",
            "subpatcher_template": "",
            "title": options.title,
            "boxes": options.boxes,
            "lines": options.lines,
            "dependency_cache": [],
            "latency": 0,
            "project": project,
            "autosave": 0,
        },
    })
}
