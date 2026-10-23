//! Max for Live's device file (.amxd): the container (kumi-types, which the web tool reads too) and a device's
//! patcher, as Live's device builder lays it out.

use super::patch::layout::arrange;
use super::patch::{NoFiles, Patcher};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use kumi_types::amxd::{decode_amxd, encode_amxd, patcher_chunk, DecodedAmxd, DeviceType};

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
/// face), `width` pixels wide, with `description` as its info text. Its boxes are laid out by Kumi's layout, so the
/// patcher keeps Kumi's standard for Max patchers (`daws/ableton/max-standard/`).
pub fn device_patcher(kind: DeviceType, options: DevicePatcherOptions) -> Value {
    let amxdtype = u32::from_be_bytes(kind.code().as_bytes().try_into().expect("a four-letter code"));
    let project = json!({
        "version": 1, "creationdate": 3590052786u64, "modificationdate": 3590052786u64, "viewrect": [0.0, 0.0, 300.0, 500.0], "autoorganize": 1, "hideprojectwindow": 1,
        "showdependencies": 1, "autolocalize": 0, "contents": { "patchers": {} }, "layout": {}, "searchpath": {}, "detailsvisible": 0, "amxdtype": amxdtype, "readonly": 0, "devpathtype": 0, "devpath": ".",
        "sortmode": 0, "viewmode": 0,
    });
    let document = json!({
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
    });
    let mut patcher = Patcher::read(&document, &NoFiles);
    arrange(&mut patcher);
    patcher.to_document()
}
