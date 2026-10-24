//! Where each heard song sits on the map: the style vectors of the songs the style model heard, laid out together
//! (`kumi_layout`, so songs that sound alike sit together), kept in music.db as each song's `x` and `y` (from 0 to 1),
//! with what the map was made from in `meta` ("map"). Made again when the songs heard grow by a tenth, and at the end
//! of a scan when the songs on it changed at all (more or fewer, or others: a folder moved, a file changed).

use super::{
    store::{MusicError, MusicStore, LIVE},
    vectors::{VectorReader, DIMS},
};
use kumi_common::time::now_ms;
use kumi_layout::{layout_until, Layout};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// What the map was made from: the style model, how many songs, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Made {
    pub model: String,
    pub songs: usize,
    pub made: i64,
}

/// The map as it was last made: None before the first.
pub fn made(store: &MusicStore) -> Result<Option<Made>, MusicError> {
    Ok(store.meta("map")?.and_then(|text| serde_json::from_str(&text).ok()))
}

/// How many songs the map holds, when `model` made it.
pub fn mapped(store: &MusicStore, model: &str) -> Result<Option<usize>, MusicError> {
    Ok(made(store)?.filter(|made| made.model == model).map(|made| made.songs))
}

/// Whether the map is due with `heard` songs heard, `mapped` of them on the map the style model made last: there's
/// none, the songs heard grew by a tenth (ten at least), or, at a scan's end (`ended`), changed at all.
pub fn due(mapped: Option<usize>, heard: usize, ended: bool) -> bool {
    heard > 0
        && match mapped {
            None => true,
            Some(songs) if ended => heard != songs,
            Some(songs) => heard >= songs + (songs / 10).max(10),
        }
}

/// Whether the songs the clusters and the map were made from are still the ones the style model heard: a song heard
/// (with its vector) without a cluster or a place, or one with them that's gone, set aside or has no vector now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Changed {
    pub clusters: bool,
    pub places: bool,
}

/// What changed among the songs `reader`'s style model heard since the clusters and the map were made.
pub fn changed(store: &MusicStore, reader: &VectorReader) -> Result<Changed, MusicError> {
    let heard = format!("({LIVE} AND stage >= 1)");
    let mut statement = store.connection().prepare(&format!(
        "SELECT id, {heard}, cluster IS NOT NULL, x IS NOT NULL AND y IS NOT NULL FROM songs WHERE {heard} OR cluster IS NOT NULL OR x IS NOT NULL"
    ))?;
    let mut rows = statement.query([])?;
    let mut changed = Changed::default();
    while let Some(row) = rows.next()? {
        let (id, heard, clustered, placed): (i64, bool, bool, bool) = (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
        let wanted = heard && reader.has(id);
        changed.clusters |= wanted != clustered;
        changed.places |= wanted != placed;
        if changed.clusters && changed.places {
            break;
        }
    }
    Ok(changed)
}

/// The map made again from the songs `model` heard: each one's place, and what it was made from. How many it placed.
pub fn lay_out(store: &mut MusicStore, music_dir: &Path, model: &str, how: &Layout) -> Result<usize, MusicError> {
    Ok(lay_out_until(store, music_dir, model, how, &|| true)?.unwrap_or(0))
}

/// `lay_out`, asking `go_on` as the layout works (`kumi_layout::layout_until`): it may wait there (the scan waits while
/// Kumi listens), and false ends it with nothing written (None).
pub fn lay_out_until(
    store: &mut MusicStore,
    music_dir: &Path,
    model: &str,
    how: &Layout,
    go_on: &(dyn Fn() -> bool + Sync),
) -> Result<Option<usize>, MusicError> {
    let reader = VectorReader::open(music_dir, store, model)?;
    // In the songs' order (by id), so the same songs make the same map.
    let mut ids: Vec<i64> = store.heard_ids()?.into_iter().filter(|id| reader.has(*id)).collect();
    ids.sort_unstable();
    let read = |at: usize, buffer: &mut [f32]| match reader.get(ids[at]) {
        Some(vector) => buffer.copy_from_slice(&vector[..DIMS]),
        None => buffer.fill(0.),
    };
    let Some(places) = layout_until(ids.len(), DIMS, &read, how, go_on) else { return Ok(None) };
    store.set_places(&ids.iter().copied().zip(places).collect::<Vec<_>>())?;
    let made = Made { model: model.to_string(), songs: ids.len(), made: now_ms() };
    store.set_meta("map", &serde_json::to_string(&made).map_err(|error| MusicError(error.to_string()))?)?;
    Ok(Some(ids.len()))
}

#[cfg(test)]
mod tests {
    use super::due;

    #[test]
    fn the_map_is_due_when_there_is_none_or_the_songs_heard_grew_by_a_tenth() {
        assert!(!due(None, 0, false) && due(None, 1, false));
        assert!(!due(Some(100), 109, false) && due(Some(100), 110, false));
        assert!(!due(Some(20), 29, false) && due(Some(20), 30, false), "ten at least");
        assert!(due(Some(100), 101, true) && due(Some(100), 99, true) && !due(Some(100), 100, true), "at a scan's end, any change");
    }
}
