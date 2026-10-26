//! The sounds' map, made again after learning: every measured sound's place, laid out (`kumi_layout`) from its numbers
//! as the search compares them (standardized, each group weighted), so sounds that sound alike sit together. Written to
//! sounds-map.json (`map`), the sounds in order by path so the same sounds make the same map: when there's none, the
//! sounds measured and the ones on it differ by a tenth since it was made, or the layout gives maps another shape now.
//! The sounds of every folder the library learns from count, there now or not (a drive unplugged a while isn't a library
//! changed): what draws the map hides the ones that aren't there.

use crate::{
    features::VECTOR_LENGTH,
    learn::{SoundEntry, LOG_VERSION},
    map::{read_sounds_map, SoundsMap, SOUNDS_MAP, SOUNDS_MAP_VERSION},
    search::SoundIndex,
    sources::Source,
    store::{write_compact_json, LogReader},
};
use kumi_common::time::now_ms;
use kumi_layout::{layout_until, Layout, SHAPE};
use std::{collections::HashSet, io, path::Path, sync::Arc};

/// Whether the map is due with these sounds measured (their paths), `mapped` on it as it was made: there's none, or the
/// two differ by a tenth of the map (one at least): sounds added, gone, or moved (a path new to it and one gone from it).
pub fn due(mapped: Option<&[&str]>, measured: &[&str]) -> bool {
    let Some(mapped) = mapped else { return !measured.is_empty() };
    let (on_map, now): (HashSet<&str>, HashSet<&str>) = (mapped.iter().copied().collect(), measured.iter().copied().collect());
    now.symmetric_difference(&on_map).count() >= (mapped.len() / 10).max(1)
}

/// The sounds' map made again from the library's sounds log, when it's due: how many sounds it placed (None: not due,
/// or `go_on` ended it). `go_on` is asked as the layout works (`kumi_layout::layout_until`): it may wait there.
pub async fn lay_out_sounds(
    library_dir: &Path,
    sources: &[Source],
    how: &Layout,
    go_on: Arc<dyn Fn() -> bool + Send + Sync>,
) -> io::Result<Option<usize>> {
    let mut reader = LogReader::<SoundEntry>::new(library_dir.join("sounds.jsonl"), "sounds", LOG_VERSION);
    reader.refresh().await?;
    let entries: Vec<SoundEntry> = reader.entries.into_values().collect();
    let (dir, sources, how) = (library_dir.to_path_buf(), sources.to_vec(), how.clone());
    let made = tokio::task::spawn_blocking(move || {
        let index = SoundIndex::of_every_source(entries, &sources);
        let mut measured: Vec<(&str, &[f32])> =
            index.compared().map(|(row, numbers)| (index.entries[row].path.as_str(), numbers)).collect();
        measured.sort_by(|a, b| a.0.cmp(b.0));
        let mapped = read_sounds_map(&dir);
        let on_map: Option<Vec<&str>> = mapped.as_ref().map(|map| map.points.iter().map(|(path, ..)| path.as_str()).collect());
        let paths: Vec<&str> = measured.iter().map(|(path, _)| *path).collect();
        let reshaped = mapped.as_ref().is_some_and(|map| map.shape != SHAPE && !paths.is_empty());
        if !reshaped && !due(on_map.as_deref(), &paths) {
            return None;
        }
        let read = |at: usize, buffer: &mut [f32]| buffer.copy_from_slice(measured[at].1);
        let places = layout_until(measured.len(), VECTOR_LENGTH, &read, &how, &*go_on)?;
        // Five decimals: a hundred-thousandth of the map is finer than any screen draws it.
        let short = |value: f32| (value as f64 * 1e5).round() / 1e5;
        let points = measured.iter().zip(places).map(|((path, _), [x, y])| (path.to_string(), short(x), short(y))).collect();
        Some(SoundsMap { version: SOUNDS_MAP_VERSION, made: now_ms(), shape: SHAPE, points })
    })
    .await
    .map_err(io::Error::other)?;
    let Some(map) = made else { return Ok(None) };
    write_compact_json(&library_dir.join(SOUNDS_MAP), &map).await?;
    Ok(Some(map.points.len()))
}
