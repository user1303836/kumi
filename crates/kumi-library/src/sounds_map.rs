//! The sounds' map, made again after learning: every measured sound's place, laid out (`kumi_layout`) from its numbers
//! as the search compares them (standardized, each group weighted), so sounds that sound alike sit together. Written to
//! sounds-map.json (`map`), the sounds in order by path so the same sounds make the same map: when there's none, or the
//! sounds measured grew or shrank by a tenth since it was made.

use crate::{
    features::VECTOR_LENGTH,
    learn::{SoundEntry, LOG_VERSION},
    map::{read_sounds_map, SoundsMap, SOUNDS_MAP, SOUNDS_MAP_VERSION},
    search::SoundIndex,
    sources::Source,
    store::{write_compact_json, LogReader},
};
use kumi_common::time::now_ms;
use kumi_layout::{layout, Layout};
use std::{io, path::Path};

/// Whether the map is due with `measured` sounds, `mapped` on it as it was made: there's none, or they grew or shrank
/// by a tenth of it (one at least).
pub fn due(mapped: Option<usize>, measured: usize) -> bool {
    match mapped {
        None => measured > 0,
        Some(mapped) => measured.abs_diff(mapped) >= (mapped / 10).max(1),
    }
}

/// The sounds' map made again from the library's sounds log, when it's due: how many sounds it placed (None: not due).
pub async fn lay_out_sounds(library_dir: &Path, sources: &[Source], how: &Layout) -> io::Result<Option<usize>> {
    let mut reader = LogReader::<SoundEntry>::new(library_dir.join("sounds.jsonl"), "sounds", LOG_VERSION);
    reader.refresh().await?;
    let entries: Vec<SoundEntry> = reader.entries.into_values().collect();
    let (dir, sources, how) = (library_dir.to_path_buf(), sources.to_vec(), how.clone());
    let made = tokio::task::spawn_blocking(move || {
        let index = SoundIndex::new(entries, &sources);
        let mut measured: Vec<(&str, &[f32])> =
            index.compared().map(|(row, numbers)| (index.entries[row].path.as_str(), numbers)).collect();
        measured.sort_by(|a, b| a.0.cmp(b.0));
        if !due(read_sounds_map(&dir).map(|map| map.points.len()), measured.len()) {
            return None;
        }
        let places = layout(measured.len(), VECTOR_LENGTH, &|at, buffer| buffer.copy_from_slice(measured[at].1), &how);
        // Five decimals: a hundred-thousandth of the map is finer than any screen draws it.
        let short = |value: f32| (value as f64 * 1e5).round() / 1e5;
        let points = measured.iter().zip(places).map(|((path, _), [x, y])| (path.to_string(), short(x), short(y))).collect();
        Some(SoundsMap { version: SOUNDS_MAP_VERSION, made: now_ms(), points })
    })
    .await
    .map_err(io::Error::other)?;
    let Some(map) = made else { return Ok(None) };
    write_compact_json(&library_dir.join(SOUNDS_MAP), &map).await?;
    Ok(Some(map.points.len()))
}
