//! The producer's own music, learned on their computer: the songs in the folders they agreed Kumi may learn from
//! (`musicFolders`), each known first by its tags, then heard in the background, so Kumi can lean on their taste when
//! a request leaves the style open. Kept in `music` in Kumi's folder; never sent anywhere.

pub mod clusters;
pub mod describe;
pub mod folders;
pub mod hear;
pub mod learning;
pub mod map;
pub mod scan;
pub mod store;
pub mod tags;
pub mod tool;
pub mod vectors;
pub mod worker;

use std::path::{Path, PathBuf};

/// Where Kumi keeps what it learned of the producer's music: `music` in Kumi's folder.
pub fn music_dir(kumi_dir: &Path) -> PathBuf {
    kumi_dir.join("music")
}

/// The songs' database in the music folder.
pub fn database(music_dir: &Path) -> PathBuf {
    music_dir.join("music.db")
}
