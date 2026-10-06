//! What earlier Kumis kept in JSON files (notes, techniques, lessons, gaps), read into Kumi's database
//! and kept in step with those files while an older Kumi may still use them. Each notes, techniques or
//! lessons file has a base, the file as Kumi last read or wrote it, so what changes in a file since is
//! brought in by a three-way merge (`kumi_store::sync`). Before a rollback, what the database keeps is
//! written back to the files for the older Kumi (`write_back`). Reading in never writes the files.

use super::{
    contracts::{MemoryScope, MemoryStore},
    errors::RuntimeError,
    memory::{create_memory_store, parse_notes, MemoryStoreOptions},
    playbook::{create_playbook_store, parse_lessons, PlaybookStore},
    store_client::StoreClient,
    store_rows::{lesson_of, lesson_row, note_of, note_row, technique_of, technique_row},
    techniques::{create_technique_store, parse_techniques, TechniqueStore},
};
use kumi_store::{
    gaps,
    imports::{self, Source},
    lessons, notes, techniques as stored, Connection, Kept, Scope, Store, StoreError,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Where the files are: each path as the producer's settings name it (`KUMI_MEMORY_FILE` and the rest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonFiles {
    pub memory: PathBuf,
    pub projects: PathBuf,
    pub techniques: PathBuf,
    pub playbook: PathBuf,
    pub gaps: PathBuf,
}

/// What reading in did: files new to the database or changed since, the rows read in from files it
/// hadn't read before, and what changed in files it had.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Imported {
    pub files: usize,
    pub rows: usize,
    pub brought_in: BroughtIn,
}
impl std::ops::AddAssign for Imported {
    fn add_assign(&mut self, other: Imported) {
        self.files += other.files;
        self.rows += other.rows;
        self.brought_in.notes += other.brought_in.notes;
        self.brought_in.techniques += other.brought_in.techniques;
        self.brought_in.lessons += other.brought_in.lessons;
    }
}

/// Changes made in the files with an older Kumi (after a rollback, or one still running), brought in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BroughtIn {
    pub notes: Kept,
    pub techniques: Kept,
    pub lessons: Kept,
}
impl BroughtIn {
    /// The one line Kumi says at startup when there was anything: "Brought in changes made with the
    /// older Kumi: 2 notes edited, 1 note removed, 1 technique added."
    pub fn sentence(&self) -> Option<String> {
        let mut parts = vec![];
        for (kept, one) in [(self.notes, "note"), (self.techniques, "technique"), (self.lessons, "lesson")] {
            for (count, what) in [(kept.added, "added"), (kept.changed, "edited"), (kept.archived, "removed")] {
                if count > 0 {
                    parts.push(format!("{count} {one}{} {what}", if count == 1 { "" } else { "s" }));
                }
            }
        }
        (!parts.is_empty()).then(|| format!("Brought in changes made with the older Kumi: {}.", parts.join(", ")))
    }
}

/// A file's rows, as the database keeps them.
enum Rows {
    Notes(Scope, Vec<notes::Note>),
    Techniques(Vec<stored::Technique>),
    Lessons(Vec<lessons::Lesson>),
    Gaps(Vec<gaps::Gap>),
}
/// What a file holds.
#[derive(Clone)]
enum Kind {
    Notes(Scope),
    Techniques,
    Lessons,
    Gaps,
}

/// The file at `path` as it is now, read and parsed; None when there's no such file, or when it isn't a
/// whole notes, techniques or lessons file (one being written by hand, say), which then waits until it
/// is: read as empty, it would set aside everything it had held.
fn read_file(path: &Path, kind: &Kind) -> Result<Option<(Source, Rows)>, StoreError> {
    let name = match kind {
        Kind::Notes(_) => "notes",
        Kind::Techniques => "techniques",
        Kind::Lessons => "lessons",
        Kind::Gaps => "gaps",
    };
    let Some((source, bytes)) = imports::read(path, name)? else { return Ok(None) };
    if !matches!(kind, Kind::Gaps) && !whole(&bytes, name) {
        return Ok(None);
    }
    let rows = match kind {
        Kind::Notes(scope) => {
            let prefix = if *scope == Scope::Global { 'p' } else { 's' };
            Rows::Notes(scope.clone(), parse_notes(&bytes, prefix).iter().map(note_row).collect())
        }
        Kind::Techniques => Rows::Techniques(parse_techniques(&bytes).iter().map(technique_row).collect()),
        Kind::Lessons => Rows::Lessons(parse_lessons(&bytes).iter().map(lesson_row).collect()),
        Kind::Gaps => Rows::Gaps(parse_gaps(&String::from_utf8_lossy(&bytes))),
    };
    Ok(Some((source, rows)))
}

/// Whether `bytes` are a whole file of version 1 with its list under `key`.
fn whole(bytes: &[u8], key: &str) -> bool {
    serde_json::from_slice::<Value>(bytes).is_ok_and(|file| file["version"].as_f64() == Some(1.0) && file[key].is_array())
}

/// Every file Kumi reads in: the producer's notes, each saved Set's, techniques, lessons and gaps.
fn every_file(files: &JsonFiles) -> Vec<(PathBuf, Kind)> {
    let mut every = vec![(files.memory.clone(), Kind::Notes(Scope::Global))];
    let mut projects: Vec<String> = std::fs::read_dir(&files.projects)
        .map(|entries| entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()).filter(|name| project_id(name)).collect())
        .unwrap_or_default();
    projects.sort();
    for project in projects {
        every.push((files.projects.join(&project).join("memory.json"), Kind::Notes(Scope::Project(project))));
    }
    every.push((files.techniques.clone(), Kind::Techniques));
    every.push((files.playbook.clone(), Kind::Lessons));
    every.push((files.gaps.clone(), Kind::Gaps));
    every
}

/// Every file Kumi reads in, as `every_file` lists them.
pub(crate) fn every_path(files: &JsonFiles) -> Vec<PathBuf> {
    every_file(files).into_iter().map(|(path, _)| path).collect()
}

/// Read the files into the database. A file read for the first time adds the rows the database doesn't
/// have, never one forgotten there, and deletes nothing. A file changed since it was read (or written
/// back) brings in what changed in it since its base (`kumi_store::sync::merge`). Each file goes in a
/// transaction of its own, which first checks the file hasn't been read as it is now (another Kumi may
/// have just done it), so a change comes in once. Blocking (it reads the files and waits for the
/// writes), so it runs on a blocking thread before the first read. Every file is read before any is
/// written, so a file that can't be read leaves the database as it was.
pub fn import_json(store: &Store, files: &JsonFiles, now: i64) -> Result<Imported, StoreError> {
    let mut read = vec![];
    for (path, kind) in every_file(files) {
        read.extend(read_file(&path, &kind)?);
    }
    let mut imported = Imported::default();
    for (source, rows) in read {
        imported += store.write_wait(move |c| sync(c, &source, &rows, now))?;
    }
    Ok(imported)
}

/// One file into the database, as `import_json` says.
fn sync(c: &Connection, source: &Source, rows: &Rows, now: i64) -> Result<Imported, StoreError> {
    let record = imports::record_of(c, &source.path)?;
    if record.as_ref().is_some_and(|record| record.blake3 == source.blake3) {
        return Ok(Imported::default());
    }
    let base = record.and_then(|record| record.base);
    let mut done = Imported { files: 1, ..Imported::default() };
    let next = match (rows, base) {
        (Rows::Gaps(rows), _) => {
            for row in rows {
                done.rows += gaps::import(c, row)? as usize;
            }
            None
        }
        (Rows::Notes(scope, rows), Some(base)) => {
            let (next, kept) = notes::merge(c, scope, &base, rows, now)?;
            done.brought_in.notes = kept;
            Some(next)
        }
        (Rows::Techniques(rows), Some(base)) => {
            let (next, kept) = stored::merge(c, &base, rows, now)?;
            done.brought_in.techniques = kept;
            Some(next)
        }
        (Rows::Lessons(rows), Some(base)) => {
            let (next, kept) = lessons::merge(c, &base, rows, now)?;
            done.brought_in.lessons = kept;
            Some(next)
        }
        (rows, None) => {
            let (next, added) = match rows {
                Rows::Notes(scope, rows) => notes::read_in(c, scope, rows)?,
                Rows::Techniques(rows) => stored::read_in(c, rows)?,
                Rows::Lessons(rows) => lessons::read_in(c, rows)?,
                Rows::Gaps(_) => unreachable!("gaps are read in above"),
            };
            done.rows = added;
            Some(next)
        }
    };
    imports::record(c, source, done.rows, next.as_deref(), now)?;
    Ok(done)
}

/// A project folder's name: the 32 lowercase hex digits Kumi names a Set's folder with.
fn project_id(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The gaps a gaps log holds: a line that isn't a whole entry (time and what was missing) is left out.
pub(crate) fn parse_gaps(text: &str) -> Vec<gaps::Gap> {
    text.lines()
        .filter_map(|line| {
            let raw: Value = serde_json::from_str(line).ok()?;
            let at = chrono::DateTime::parse_from_rfc3339(raw["at"].as_str()?).ok()?.timestamp_millis();
            let missing = raw["missing"].as_str().filter(|s| !s.is_empty())?.to_string();
            let text = |key: &str| raw[key].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            Some(gaps::Gap {
                kumi_version: raw["kumi"].as_str().unwrap_or("").into(),
                missing,
                asked: text("asked"),
                workaround: text("workaround"),
                at,
            })
        })
        .collect()
}

/// Before a rollback: the notes, techniques and lessons the database has in use, written to the files an
/// older Kumi reads, in its format (each file written whole, then renamed into place), and each file's
/// base recorded, so what the older Kumi then changes comes back in. Every Set with notes, or with a
/// notes file, gets its list, an empty one too, so a note forgotten since the update doesn't come back
/// there. Nothing to do without a database.
pub async fn write_back(db: PathBuf, files: JsonFiles, now: i64) -> Result<(), RuntimeError> {
    if !db.exists() {
        return Ok(());
    }
    let failed = |why: StoreError| RuntimeError::plain(why.to_string());
    let joined = |why: tokio::task::JoinError| RuntimeError::plain(why.to_string());
    let store = tokio::task::spawn_blocking(move || Store::open(&db)).await.map_err(joined)?.map_err(failed)?;
    let client = StoreClient::new(store);
    let mut with_files: Vec<String> = std::fs::read_dir(&files.projects)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| project_id(name) && files.projects.join(name).join("memory.json").is_file())
                .collect()
        })
        .unwrap_or_default();
    let (producer, sets, kept_techniques, kept_lessons) = client
        .read(move |c| {
            let mut projects: Vec<String> = c
                .prepare("SELECT DISTINCT scope_id FROM notes WHERE scope_kind = 'project'")?
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            projects.append(&mut with_files);
            projects.sort();
            projects.dedup();
            let sets = projects
                .into_iter()
                .filter(|project| project_id(project))
                .map(|project| Ok((project.clone(), notes::in_use(c, &Scope::Project(project))?)))
                .collect::<Result<Vec<_>, StoreError>>()?;
            Ok((notes::in_use(c, &Scope::Global)?, sets, stored::in_use(c)?, lessons::in_use(c)?))
        })
        .await
        .map_err(failed)?;
    let memory = create_memory_store(MemoryStoreOptions { projects_dir: files.projects.clone(), producer_file: files.memory.clone() });
    memory.save(MemoryScope::Producer, None, &producer.into_iter().map(note_of).collect::<Vec<_>>()).await?;
    let mut written = vec![(files.memory.clone(), Kind::Notes(Scope::Global))];
    for (project, rows) in sets {
        memory.save(MemoryScope::Set, Some(&project), &rows.into_iter().map(note_of).collect::<Vec<_>>()).await?;
        written.push((files.projects.join(&project).join("memory.json"), Kind::Notes(Scope::Project(project))));
    }
    create_technique_store(files.techniques.clone()).save(&kept_techniques.into_iter().map(technique_of).collect::<Vec<_>>()).await?;
    written.push((files.techniques.clone(), Kind::Techniques));
    create_playbook_store(files.playbook.clone()).save(&kept_lessons.into_iter().map(lesson_of).collect::<Vec<_>>()).await?;
    written.push((files.playbook.clone(), Kind::Lessons));
    // What each file holds now is its base: each of its rows is the row in use with its label.
    let read = tokio::task::spawn_blocking(move || {
        written.iter().map(|(path, kind)| read_file(path, kind)).collect::<Result<Vec<_>, StoreError>>()
    })
    .await
    .map_err(joined)?
    .map_err(failed)?;
    client
        .write(move |c| {
            for (source, rows) in read.iter().flatten() {
                let base = match rows {
                    Rows::Notes(scope, rows) => notes::base_of(c, scope, rows)?,
                    Rows::Techniques(rows) => stored::base_of(c, rows)?,
                    Rows::Lessons(rows) => lessons::base_of(c, rows)?,
                    Rows::Gaps(_) => continue,
                };
                imports::record(c, source, 0, Some(&base), now)?;
            }
            Ok(())
        })
        .await
        .map_err(failed)
}
