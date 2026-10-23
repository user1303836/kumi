//! Where the library's sounds sit on a map: x and y in [0, 1] for each sound by its path, as the layout last made them
//! from the sounds' vectors, kept in sounds-map.json in the library's folder. /map draws them.

use serde::{Deserialize, Serialize};
use std::path::Path;

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
