//! Where the library's sounds sit on a map: x and y in [0, 1] for each sound by its path, as the layout last made them
//! from the sounds' vectors, kept in sounds-map.json in the library's folder. /map draws them, joined with what the
//! library knows of each sound.

use super::{
    classify::{SoundClass, SoundKind},
    search::SoundIndex,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};

/// The map's file, in the library's folder.
pub const SOUNDS_MAP: &str = "sounds-map.json";
/// The form of the file this build reads and writes.
pub const SOUNDS_MAP_VERSION: u32 = 1;

/// sounds-map.json: `{"version": 1, "made": <ms>, "points": [["<path>", x, y], …]}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SoundsMap {
    pub version: u32,
    /// When the layout made it, in ms since 1970.
    pub made: i64,
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

/// A sound on the map, with what the map shows of it.
#[derive(Debug, Clone, PartialEq)]
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
}

/// The sounds `index` offers, where the map puts them, in the map's order: a sound the index doesn't offer (gone, or
/// in another DAW's library) is left out, and so is one the map doesn't have yet.
pub fn sounds_on_map(map: &SoundsMap, index: &SoundIndex) -> Vec<MapSound> {
    let rows: HashMap<&str, usize> = index
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.gone != Some(true))
        .map(|(row, entry)| (entry.path.as_str(), row))
        .collect();
    map.points
        .iter()
        .filter_map(|(path, x, y)| {
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
            })
        })
        .collect()
}
