//! The scan of the producer's music folders. Its first pass reads every song's tags, 300,000 songs in minutes: it
//! touches only the files that changed since the last walk (by size and time changed), marks the ones gone that are
//! gone, and finds duplicates again. Then the songs are heard, in the order that makes taste useful soonest: the songs
//! taste used, a spread sample across artists in full, every song lightly (newest first), then every song in full.

use super::{
    clusters::{clusters_for, kmeans, SEED},
    folders::{folders_to_walk, walk, Walked},
    store::{folded, MusicError, MusicStore, Song, Stage, BATCH, LIVE},
    tags::{self, Tags},
    vectors::{VectorReader, DIMS},
};
use kumi_common::abort::Signal;
use std::{
    collections::HashSet,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

/// What a tags pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TagsPassed {
    /// Song files found.
    pub walked: usize,
    pub new: usize,
    pub changed: usize,
    pub gone: usize,
    /// Files gone before and found again as they were.
    pub back: usize,
    /// Rows written (none when nothing changed).
    pub written: usize,
}

/// How far a tags pass has got.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TagsProgress {
    pub walked: usize,
    pub read: usize,
    pub to_read: usize,
}

/// How a file's tags are read: `tags::read`, or a test's own.
pub type ReadTags = dyn Fn(&Path, &Path) -> Tags + Sync;

/// What a tags pass is told.
pub struct TagsPass<'a> {
    pub folders: &'a [String],
    /// Folders left out (the producer's sample folders, the library's places).
    pub skip: &'a [String],
    pub signal: &'a Signal,
    /// Threads reading tags at once.
    pub readers: usize,
    pub progress: &'a dyn Fn(TagsProgress),
}

/// Walk the folders and bring music.db up to date with them. Stopped by the signal, what was written stays and nothing
/// is marked gone (the walk wasn't whole).
pub fn tags_pass(store: &mut MusicStore, pass: &TagsPass) -> Result<TagsPassed, MusicError> {
    let folders = folders_to_walk(pass.folders);
    let mut walked = vec![];
    let readable = walk(&folders, pass.skip, &mut |song| {
        walked.push(song);
        if walked.len() % 5_000 == 0 {
            (pass.progress)(TagsProgress { walked: walked.len(), ..Default::default() });
        }
        !pass.signal.is_cancelled()
    });
    if pass.signal.is_cancelled() {
        return Err(stopped());
    }
    let whole: Vec<String> = folders.iter().zip(&readable).filter(|(_, read)| **read).map(|(folder, _)| folder.clone()).collect();
    apply_walk(store, &folders, &whole, walked, &tags::read, pass)
}

/// A walk's files written to music.db: new and changed ones with their tags (read on `pass.readers` threads, written
/// `BATCH` at a time), the ones gone marked gone (only under a folder walked whole, or one no longer listed), and the
/// duplicates found again.
pub fn apply_walk(
    store: &mut MusicStore,
    folders: &[String],
    whole: &[String],
    walked: Vec<Walked>,
    read: &ReadTags,
    pass: &TagsPass,
) -> Result<TagsPassed, MusicError> {
    let stamps = store.stamps()?;
    let mut passed = TagsPassed { walked: walked.len(), ..Default::default() };
    let mut seen = HashSet::with_capacity(walked.len());
    let mut back = vec![];
    let mut to_read = vec![];
    for song in walked {
        match stamps.get(&song.path) {
            Some(stamp) if stamp.size == song.size && stamp.mtime == song.mtime && stamp.folder == song.folder => {
                if stamp.gone {
                    back.push(stamp.id);
                }
            }
            Some(_) => {
                passed.changed += 1;
                to_read.push(song.clone());
            }
            None => {
                passed.new += 1;
                to_read.push(song.clone());
            }
        }
        seen.insert(song.path);
    }
    let progress = |read: usize| (pass.progress)(TagsProgress { walked: passed.walked, read, to_read: to_read.len() });
    progress(0);
    let next = AtomicUsize::new(0);
    let (sender, receiver) = std::sync::mpsc::sync_channel::<(usize, Tags)>(BATCH * 2);
    let written = std::thread::scope(|scope| -> Result<usize, MusicError> {
        for _ in 0..pass.readers.max(1) {
            let sender = sender.clone();
            let (next, to_read, signal) = (&next, &to_read, pass.signal);
            scope.spawn(move || loop {
                let at = next.fetch_add(1, Ordering::Relaxed);
                if at >= to_read.len() || signal.is_cancelled() {
                    break;
                }
                let song = &to_read[at];
                if sender.send((at, read(Path::new(&song.path), Path::new(&song.folder)))).is_err() {
                    break;
                }
            });
        }
        drop(sender);
        let (mut batch, mut written) = (Vec::with_capacity(BATCH), 0);
        for (at, tags) in receiver {
            batch.push((to_read[at].clone(), tags));
            if batch.len() >= BATCH {
                written += store.write_songs(&batch)?;
                batch.clear();
                progress(written);
            }
        }
        written += store.write_songs(&batch)?;
        progress(written);
        Ok(written)
    });
    // A reader stopped early: the readers' channel ends, and so does writing.
    let written = written?;
    if pass.signal.is_cancelled() {
        return Err(stopped());
    }
    let gone: Vec<i64> = stamps
        .iter()
        .filter(|(path, stamp)| !stamp.gone && !seen.contains(*path) && (!folders.contains(&stamp.folder) || whole.contains(&stamp.folder)))
        .map(|(_, stamp)| stamp.id)
        .collect();
    passed.gone = store.set_gone(&gone, true)?;
    passed.back = store.set_gone(&back, false)?;
    let duplicates = store.resolve_duplicates()?;
    passed.written = written + passed.gone + passed.back + duplicates;
    Ok(passed)
}

fn stopped() -> MusicError {
    MusicError("stopped".into())
}

/// The spread sample's size: songs heard in full first, so the first hour of hearing covers the whole library.
pub const SPREAD: usize = 2_000;

/// Up to `size` songs taken in turn from each artist (or, for songs named by their path, each top folder), newest
/// first within each.
pub fn spread_sample(store: &MusicStore, size: usize) -> Result<Vec<i64>, MusicError> {
    let mut groups: indexmap::IndexMap<String, std::collections::VecDeque<i64>> = indexmap::IndexMap::new();
    let mut statement = store
        .connection()
        .prepare(&format!("SELECT id, artist, album_artist, names_from, path, folder FROM songs WHERE {LIVE} ORDER BY mtime DESC, id"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let (id, artist, album_artist, names_from): (i64, Option<String>, Option<String>, String) =
            (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
        let (path, folder): (String, String) = (row.get(4)?, row.get(5)?);
        let key = match album_artist.or(artist).filter(|_| names_from == "tags") {
            Some(artist) => format!("artist:{}", folded(&artist)),
            None => {
                let relative = Path::new(&path).strip_prefix(&folder).unwrap_or(Path::new(&path));
                let top = relative.components().next().filter(|_| relative.components().count() > 1);
                format!("folder:{}", top.map(|top| folded(&top.as_os_str().to_string_lossy())).unwrap_or_default())
            }
        };
        groups.entry(key).or_default().push_back(id);
    }
    let mut sample = vec![];
    while sample.len() < size && groups.values().any(|group| !group.is_empty()) {
        for group in groups.values_mut() {
            if let Some(id) = group.pop_front() {
                sample.push(id);
                if sample.len() >= size {
                    break;
                }
            }
        }
    }
    Ok(sample)
}

/// The spread sample kept in music.db: made when there's none, and again once the songs have grown by a tenth.
pub fn spread(store: &MusicStore) -> Result<Vec<i64>, MusicError> {
    let songs = store.counts()?.songs;
    let kept = store.meta("spread")?.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    if let Some(kept) = kept.filter(|kept| songs * 10 <= kept["songs"].as_u64().unwrap_or(0) as usize * 11) {
        return Ok(kept["ids"].as_array().into_iter().flatten().filter_map(|id| id.as_i64()).collect());
    }
    let sample = spread_sample(store, SPREAD)?;
    store.set_meta("spread", &serde_json::json!({"songs": songs, "ids": sample}).to_string())?;
    Ok(sample)
}

/// What to hear next.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// A song to hear at a stage.
    Hear(Song, Stage),
    /// A song heard by another style model, to hear again by this one (its windows only).
    Restyle(Song, Stage),
}

impl Next {
    pub fn song(&self) -> &Song {
        match self {
            Next::Hear(song, _) | Next::Restyle(song, _) => song,
        }
    }
}

/// The next song to hear, best first: one taste used (in full), one this style model hasn't heard yet (its windows
/// only), the spread sample (in full), every song lightly (newest first), then every song in full (newest first).
/// `busy` are being heard now, `missing` waits for this style model, and `spread_at` is how far into the spread sample
/// every song is heard in full.
pub fn next(
    store: &MusicStore,
    busy: &HashSet<i64>,
    missing: &mut std::collections::VecDeque<i64>,
    spread: &[i64],
    spread_at: &mut usize,
) -> Result<Option<Next>, MusicError> {
    let not_busy = match busy.is_empty() {
        true => "1".to_string(),
        false => format!("id NOT IN ({})", busy.iter().map(i64::to_string).collect::<Vec<_>>().join(",")),
    };
    let one = |condition: &str, order: &str| -> Result<Option<Song>, MusicError> {
        Ok(store.songs_where(&format!("({condition}) AND {not_busy}"), order, 1)?.into_iter().next())
    };
    if let Some(song) = one("used_at IS NOT NULL AND stage < 2", "used_at DESC, id")? {
        return Ok(Some(Next::Hear(song, Stage::Full)));
    }
    while let Some(id) = missing.front().copied() {
        missing.pop_front();
        if let Some(song) = store.songs_by_id(&[id])?.into_iter().find(|song| song.stage >= 1 && !busy.contains(&song.id)) {
            let stage = if song.stage >= 2 { Stage::Full } else { Stage::Light };
            return Ok(Some(Next::Restyle(song, stage)));
        }
    }
    while *spread_at < spread.len() {
        let chunk: Vec<i64> = spread[*spread_at..(*spread_at + 200).min(spread.len())].to_vec();
        let mut waiting: Vec<Song> =
            store.songs_by_id(&chunk)?.into_iter().filter(|song| song.stage < 2 && !busy.contains(&song.id)).collect();
        if waiting.is_empty() {
            *spread_at += chunk.len();
            continue;
        }
        waiting.sort_by_key(|song| chunk.iter().position(|id| *id == song.id));
        return Ok(Some(Next::Hear(waiting.remove(0), Stage::Full)));
    }
    if let Some(song) = one("stage = 0", "mtime DESC, id")? {
        return Ok(Some(Next::Hear(song, Stage::Light)));
    }
    Ok(one("stage = 1", "mtime DESC, id")?.map(|song| Next::Hear(song, Stage::Full)))
}

/// The clusters made again from the songs this style model heard: each song's, and their centres in `meta`.
pub fn recluster(store: &mut MusicStore, music_dir: &Path, model: &str) -> Result<usize, MusicError> {
    let reader = VectorReader::open(music_dir, store, model)?;
    let ids: Vec<i64> = store.heard_ids()?.into_iter().filter(|id| reader.has(*id)).collect();
    let read = |at: usize, buffer: &mut [f32]| {
        if let Some(vector) = reader.get(ids[at]) {
            buffer.copy_from_slice(&vector[..DIMS]);
        }
    };
    let (centres, assigned) = kmeans(ids.len(), DIMS, clusters_for(ids.len()), &read, SEED);
    store.set_clusters(&ids.iter().copied().zip(assigned.iter().copied()).collect::<Vec<_>>())?;
    // Each cluster's songs nearest its centre (the examples taste names) and its two commonest genre tags.
    let genres: std::collections::HashMap<i64, String> = {
        let mut statement =
            store.connection().prepare(&format!("SELECT id, genre FROM songs WHERE {LIVE} AND stage >= 1 AND genre IS NOT NULL"))?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let clusters: Vec<serde_json::Value> = centres
        .iter()
        .enumerate()
        .map(|(cluster, centre)| {
            let members: Vec<usize> = (0..ids.len()).filter(|at| assigned[*at] == cluster).collect();
            let mut near: Vec<(f64, i64)> = members.iter().filter_map(|at| Some((reader.distance(ids[*at], centre)?, ids[*at]))).collect();
            near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            let examples: Vec<i64> = near.iter().take(EXAMPLES).map(|(_, id)| *id).collect();
            let tags = members.iter().filter_map(|at| genres.get(&ids[*at])).flat_map(|genre| genre.split("; "));
            serde_json::json!({"songs": members.len(), "examples": examples, "genres": commonest(tags, 2)})
        })
        .collect();
    let kept = serde_json::json!({"model": model, "heard": ids.len(), "centres": centres, "clusters": clusters});
    store.set_meta("clusters", &kept.to_string())?;
    Ok(ids.len())
}

/// The songs nearest each cluster's centre kept with it.
pub const EXAMPLES: usize = 10;

/// The `most` commonest tags, any case (each as most of its songs spell it), the commonest first.
pub fn commonest<'a>(tags: impl Iterator<Item = &'a str>, most: usize) -> Vec<String> {
    let mut counts: indexmap::IndexMap<String, (usize, std::collections::HashMap<&'a str, usize>)> = indexmap::IndexMap::new();
    for tag in tags.map(str::trim).filter(|tag| !tag.is_empty()) {
        let (count, spellings) = counts.entry(folded(tag)).or_default();
        *count += 1;
        *spellings.entry(tag).or_default() += 1;
    }
    let mut ranked: Vec<(String, usize, String)> = counts
        .into_iter()
        .map(|(key, (count, spellings))| {
            let spelling = spellings.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0))).map(|(tag, _)| tag.to_string()).unwrap();
            (key, count, spelling)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.into_iter().take(most).map(|(_, _, spelling)| spelling).collect()
}

/// How many songs the clusters were made from (by this style model), from `meta`.
pub fn clustered(store: &MusicStore, model: &str) -> Result<usize, MusicError> {
    let kept = store.meta("clusters")?.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    Ok(kept.filter(|kept| kept["model"] == model).and_then(|kept| kept["heard"].as_u64()).unwrap_or(0) as usize)
}
