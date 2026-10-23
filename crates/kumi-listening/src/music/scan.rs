//! The scan of the producer's music folders. Its first pass reads every song's tags, 300,000 songs in minutes: it
//! touches only the files that changed since the last walk (by size and time changed), marks the ones gone that are
//! gone, and finds duplicates again. The songs are heard after it.

use super::{
    folders::{folders_to_walk, walk, Walked},
    store::{MusicError, MusicStore, BATCH},
    tags::{self, Tags},
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
