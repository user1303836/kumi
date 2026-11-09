//! Where the library's sounds sit on a map: x and y in [0, 1] for each sound by its path, as the layout last made them
//! from the sounds' vectors, kept in sounds-map.json in the library's folder. /map draws them, joined with what the
//! library knows of each sound. What /map draws is here too (`MapData`: the sounds' map and the songs'), as plain data
//! the app fills.

use super::{
    classify::{SoundClass, SoundKind},
    search::SoundIndex,
};
use futures::future::LocalBoxFuture;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path, rc::Rc};

/// The map's file, in the library's folder.
pub const SOUNDS_MAP: &str = "sounds-map.json";
/// The form of the file this build reads and writes.
pub const SOUNDS_MAP_VERSION: u32 = 1;

/// sounds-map.json: `{"version": 1, "made": <ms>, "shape": 2, "points": [["<path>", x, y], …]}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SoundsMap {
    pub version: u32,
    /// When the layout made it, in ms since 1970.
    pub made: i64,
    /// The shape the layout gave it (`kumi_layout::SHAPE`; 0 before maps kept it).
    #[serde(default)]
    pub shape: u32,
    /// Each sound's path, and where it sits (x and y in [0, 1]).
    pub points: Vec<(String, f64, f64)>,
}

/// The map in the library's folder: None when there's none yet, or it's in another form (a newer Kumi's). A point
/// outside [0, 1] is left out.
pub fn read_sounds_map(library_dir: &Path) -> Option<SoundsMap> {
    let bytes = std::fs::read(library_dir.join(SOUNDS_MAP)).ok()?;
    let mut map: SoundsMap = serde_json::from_slice(&bytes).ok()?;
    if map.version != SOUNDS_MAP_VERSION {
        return None;
    }
    map.points.retain(|(_, x, y)| (0. ..=1.).contains(x) && (0. ..=1.).contains(y));
    Some(map)
}

/// A sound on the map, with what the map shows of it and what its arrangements read (`crate::arrange`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapSound {
    pub path: String,
    pub x: f64,
    pub y: f64,
    /// Its file's name, without the extension.
    pub name: String,
    /// Its source's label and the folders down to it ("Splice / Drums / Kicks").
    pub place: String,
    pub class: Option<SoundClass>,
    pub kind: Option<SoundKind>,
    pub seconds: Option<f64>,
    pub bpm: Option<f64>,
    /// As the index has them ("A minor", "C3"); the arrangements read only what parses.
    pub key: Option<String>,
    pub note: Option<String>,
    /// Spectral centroid, Hz.
    pub brightness: Option<f64>,
    /// Spectral flatness, 0 (tonal) to 1 (noisy).
    pub flatness: Option<f64>,
    /// Attack, ms.
    pub attack: Option<f64>,
    /// Onsets a second.
    pub onsets: Option<f64>,
    /// The shares of its energy in the lows and the highs.
    pub low: Option<f64>,
    pub high: Option<f64>,
}

/// The sounds `index` offers, where the map puts them, in the map's order: a sound the index doesn't offer (gone, or
/// in another DAW's library) is left out, and so is one the map doesn't have yet.
pub fn sounds_on_map(map: &SoundsMap, index: &SoundIndex) -> Vec<MapSound> {
    let rows = rows(index, 0..index.entries.len());
    map.points.iter().filter_map(|point| on_map(index, &rows, point)).collect()
}

/// The same, a slice at a time, the thread's other work going on between the slices: Kumi's own thread reads a map of
/// 300,000 sounds without its keys and screen waiting on it.
pub async fn sounds_on_map_by_turns(map: &SoundsMap, index: &SoundIndex) -> Vec<MapSound> {
    const TURN: usize = 4096;
    let mut known = HashMap::with_capacity(index.entries.len());
    for start in (0..index.entries.len()).step_by(TURN) {
        known.extend(rows(index, start..(start + TURN).min(index.entries.len())));
        tokio::task::yield_now().await;
    }
    let mut sounds = Vec::with_capacity(map.points.len());
    for points in map.points.chunks(TURN) {
        sounds.extend(points.iter().filter_map(|point| on_map(index, &known, point)));
        tokio::task::yield_now().await;
    }
    sounds
}

/// The rows of the index's sounds the library still has, by path.
fn rows(index: &SoundIndex, of: std::ops::Range<usize>) -> HashMap<&str, usize> {
    of.filter(|row| index.entries[*row].gone != Some(true)).map(|row| (index.entries[row].path.as_str(), row)).collect()
}

/// A point of the map with what the index knows of its sound: none when it doesn't have the sound.
fn on_map(index: &SoundIndex, rows: &HashMap<&str, usize>, (path, x, y): &(String, f64, f64)) -> Option<MapSound> {
    let row = *rows.get(path.as_str())?;
    let entry = &index.entries[row];
    Some(MapSound {
        path: path.clone(),
        x: *x,
        y: *y,
        name: index.name(row).to_string(),
        place: index.place(row).to_string(),
        class: entry.r#class,
        kind: entry.kind,
        seconds: entry.seconds,
        bpm: entry.bpm,
        key: entry.key.clone(),
        note: entry.note.clone(),
        brightness: entry.brightness,
        flatness: entry.flatness,
        attack: entry.attack,
        onsets: entry.onsets,
        low: entry.low,
        high: entry.high,
    })
}

/// The two maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sounds,
    Songs,
}

/// How /map reads its data, each time it opens (the app makes it, from the library and the producer's music).
pub type LoadMap = Rc<dyn Fn() -> LocalBoxFuture<'static, MapData>>;
/// What the maps would be read from now, as a key: the same key, the same maps (`/map` keeps one it made ready while the
/// key holds). Cheap: what files are there and when they changed, not what's in them.
pub type MapKey = Rc<dyn Fn() -> LocalBoxFuture<'static, String>>;

/// Both maps, as read when /map opened.
#[derive(Clone, Debug)]
pub struct MapData {
    pub sounds: Layer,
    pub songs: Layer,
}

/// One map: its points, their groups, and the folders and tags the points name by number.
#[derive(Clone, Debug)]
pub struct Layer {
    pub kind: Kind,
    pub points: Vec<Point>,
    pub groups: Vec<Group>,
    pub places: Vec<String>,
    pub tags: Vec<String>,
    /// Why there's nothing on it yet, said to the producer.
    pub empty: Option<String>,
    /// The other ways its points are laid out, in `ARRANGEMENTS`' order (the sounds' map's; none on the songs'). The
    /// k-th one's place of each point is the point's `arranged[k]`.
    pub arrangements: Vec<Arrangement>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    /// What a page asks for it by: a sound's place in its layer, a song's id in music.db.
    pub id: i64,
    pub x: f32,
    pub y: f32,
    pub group: u16,
    pub name: String,
    /// Its folder, by its number in the layer's places.
    pub place: u32,
    /// What else the filter reads (a sound's class and kind, a song's genre tags), by its number in the layer's tags.
    pub tags: u32,
    pub path: String,
    /// What of it plays: from where, and for how long (seconds).
    pub from: f32,
    pub seconds: f32,
    /// Where each of the layer's arrangements puts it, `[x, y]` in [0, 1] (y down, like `x`, `y`), in their order.
    pub arranged: Vec<[f32; 2]>,
}

impl Point {
    /// Where it sits in an arrangement (by its place in the layer's), or by similarity (`x`, `y`) for None or one the
    /// point doesn't have.
    pub fn place(&self, arrangement: Option<usize>) -> (f32, f32) {
        match arrangement.and_then(|k| self.arranged.get(k)) {
            Some([x, y]) => (*x, *y),
            None => (self.x, self.y),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// What the legend and the filter call it: a class ("kick"), or a style ("c1 · dark rainy").
    pub name: String,
    pub color: [u8; 3],
    pub count: usize,
    /// Points that stand for it (a style's own examples, else those nearest its middle), by their place in the layer.
    pub examples: Vec<u32>,
}

/// The arrangements' ids, in the order a layer and a point keep them: what each shows is `crate::arrange`'s.
pub const ARRANGEMENTS: [&str; 7] = ["character", "texture", "keys", "tempo", "families", "length", "folders"];

/// Another way to lay a layer out than similarity: its id (one of `ARRANGEMENTS`) and the data words it writes on
/// the map where they belong (class ids, keys, BPM, seconds, folder names; axis words are the UI's).
#[derive(Clone, Debug, PartialEq)]
pub struct Arrangement {
    pub id: &'static str,
    pub labels: Vec<Label>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub text: String,
    pub x: f32,
    pub y: f32,
}

impl Layer {
    pub fn empty(kind: Kind, why: &str) -> Layer {
        Layer { kind, points: vec![], groups: vec![], places: vec![], tags: vec![], empty: Some(why.into()), arrangements: vec![] }
    }
}
