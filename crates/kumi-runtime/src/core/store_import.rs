//! What earlier Kumis kept in JSON files (notes, techniques, lessons, gaps), read into Kumi's database
//! and kept in step with those files while an older Kumi may still use them. Each notes, techniques or
//! lessons file has a base, the file as Kumi last read or wrote it, so what changes in a file since is
//! brought in by a three-way merge (`kumi_store::sync`). Before a rollback, what the database keeps is
//! written back to the files for the older Kumi (`write_back`), after bringing in what changed in them.
//! Reading in never writes the files.

use super::{
    contracts::MemoryNote,
    errors::RuntimeError,
    memory::{notes_file, parse_notes},
    playbook::{lessons_file, parse_lessons, Lesson},
    store_rows::{lesson_of, lesson_row, note_of, note_row, technique_of, technique_row},
    techniques::{parse_techniques, techniques_file, Technique},
};
use kumi_store::{
    gaps,
    imports::{self, Source},
    lessons, notes, techniques as stored, Connection, Kept, Scope, Store, StoreError,
};
use serde_json::Value;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

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
/// hadn't read before, what changed in files it had, and the files it left as they are because they
/// aren't whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Imported {
    pub files: usize,
    pub rows: usize,
    pub brought_in: BroughtIn,
    pub not_whole: Vec<PathBuf>,
}
impl std::ops::AddAssign for Imported {
    fn add_assign(&mut self, other: Imported) {
        self.files += other.files;
        self.rows += other.rows;
        self.brought_in.notes += other.brought_in.notes;
        self.brought_in.techniques += other.brought_in.techniques;
        self.brought_in.lessons += other.brought_in.lessons;
        self.not_whole.extend(other.not_whole);
    }
}
impl Imported {
    /// What Kumi says about it, once each: what came in from the older Kumi, and each file left as it is.
    pub fn sentences(&self) -> Vec<String> {
        let left = self.not_whole.iter().map(|path| {
            format!(
                "Kumi left {} as it is: it isn't a whole file Kumi can read. Fix or remove it and Kumi reads it next time.",
                path.display()
            )
        });
        self.brought_in.sentence().into_iter().chain(left).collect()
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
    /// The one line Kumi says when there was anything: "Brought in changes made with the older Kumi: 2
    /// notes edited, 1 note removed, 1 technique edited in both and kept twice."
    pub fn sentence(&self) -> Option<String> {
        let mut parts = vec![];
        for (kept, one) in [(self.notes, "note"), (self.techniques, "technique"), (self.lessons, "lesson")] {
            for (count, what) in
                [(kept.added, "added"), (kept.changed, "edited"), (kept.archived, "removed"), (kept.both, "edited in both and kept twice")]
            {
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
impl Kind {
    fn name(&self) -> &'static str {
        match self {
            Kind::Notes(_) => "notes",
            Kind::Techniques => "techniques",
            Kind::Lessons => "lessons",
            Kind::Gaps => "gaps",
        }
    }
}
/// A file as it was read.
enum Read {
    Whole(Source, Rows),
    /// A notes, techniques or lessons file that isn't whole (cut short, or not version 1), left as it
    /// is until it is: read as empty, it would set aside everything it had held.
    NotWhole(PathBuf),
}

/// The file at `path` as it is now, read and parsed; None when there's no such file.
fn read_file(path: &Path, kind: &Kind) -> Result<Option<Read>, StoreError> {
    let Some((source, bytes)) = imports::read(path, kind.name())? else { return Ok(None) };
    if !matches!(kind, Kind::Gaps) && !whole(&bytes, kind.name()) {
        return Ok(Some(Read::NotWhole(path.to_path_buf())));
    }
    Ok(Some(Read::Whole(source, rows_of(kind, &bytes))))
}

/// The rows `bytes` hold, as a file of this kind.
fn rows_of(kind: &Kind, bytes: &[u8]) -> Rows {
    match kind {
        Kind::Notes(scope) => {
            let prefix = if *scope == Scope::Global { 'p' } else { 's' };
            Rows::Notes(scope.clone(), parse_notes(bytes, prefix).iter().map(note_row).collect())
        }
        Kind::Techniques => Rows::Techniques(parse_techniques(bytes).iter().map(technique_row).collect()),
        Kind::Lessons => Rows::Lessons(parse_lessons(bytes).iter().map(lesson_row).collect()),
        Kind::Gaps => Rows::Gaps(parse_gaps(&String::from_utf8_lossy(bytes))),
    }
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

/// Read the files into the database. A file read for the first time adds the rows the database doesn't
/// have, never one forgotten there, and deletes nothing. A file changed since it was read (or written
/// back) brings in what changed in it since its base (`kumi_store::sync::merge`). Each file goes in a
/// transaction of its own, which first checks the file hasn't been read as it is now (another Kumi may
/// have just done it) and is still as it was read, so a change comes in once and never against a newer
/// base. Blocking (it reads the files and waits for the writes), so it runs on a blocking thread. Every
/// file is read before any is written, so a file that can't be read leaves the database as it was.
pub fn import_json(store: &Store, files: &JsonFiles, now: i64) -> Result<Imported, StoreError> {
    let mut read = vec![];
    let mut imported = Imported::default();
    for (path, kind) in every_file(files) {
        match read_file(&path, &kind)? {
            Some(Read::Whole(source, rows)) => read.push((source, rows)),
            Some(Read::NotWhole(path)) => imported.not_whole.push(path),
            None => {}
        }
    }
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
    // Changed again since it was read: the next look brings it in as it is then.
    if imports::read(Path::new(&source.path), &source.kind)?.map(|(now, _)| now.blake3).as_ref() != Some(&source.blake3) {
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

/// What a write-back puts in one file, as the older Kumi's stores keep it.
enum Contents {
    Notes(Vec<MemoryNote>),
    Techniques(Vec<Technique>),
    Lessons(Vec<Lesson>),
}
impl Contents {
    /// What a file of this kind holds, read as its store reads it.
    fn read(kind: &Kind, bytes: &[u8]) -> Contents {
        match kind {
            Kind::Notes(scope) => Contents::Notes(parse_notes(bytes, if *scope == Scope::Global { 'p' } else { 's' })),
            Kind::Techniques => Contents::Techniques(parse_techniques(bytes)),
            Kind::Lessons | Kind::Gaps => Contents::Lessons(parse_lessons(bytes)),
        }
    }
    fn is_empty(&self) -> bool {
        match self {
            Contents::Notes(rows) => rows.is_empty(),
            Contents::Techniques(rows) => rows.is_empty(),
            Contents::Lessons(rows) => rows.is_empty(),
        }
    }
    /// The file as the store writes it.
    fn text(&self) -> String {
        match self {
            Contents::Notes(rows) => notes_file(rows),
            Contents::Techniques(rows) => techniques_file(rows),
            Contents::Lessons(rows) => lessons_file(rows),
        }
    }
    /// The same rows in label order, written as the store writes them: two files that say the same in
    /// another order or format compare equal.
    fn canonical(&self) -> String {
        match self {
            Contents::Notes(rows) => notes_file(&sorted(rows, |row| row.id.clone())),
            Contents::Techniques(rows) => techniques_file(&sorted(rows, |row| row.id.clone())),
            Contents::Lessons(rows) => lessons_file(&sorted(rows, |row| row.id.clone())),
        }
    }
}
fn sorted<T: Clone>(rows: &[T], label: impl Fn(&T) -> String) -> Vec<T> {
    let mut rows = rows.to_vec();
    rows.sort_by_key(label);
    rows
}

/// Files a write-back left as they were, each with why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WrittenBack {
    pub left: Vec<(PathBuf, String)>,
}
const NOT_WHOLE: &str = "it isn't a whole file Kumi can read";
const CHANGED: &str = "another Kumi changed it just now, so what's new there comes in at your next update";

/// Before a rollback: first what an older Kumi changed in the files since they were read is brought in
/// (nothing is written if that fails); then the notes, techniques and lessons the database has in use
/// are written to the files the older Kumi reads, in its format, each whole and renamed into place only
/// if it's still as it was read, and each file's base is recorded, so what the older Kumi then changes
/// comes back in. Every Set with notes, or with a notes file, gets its list, an empty one too, so a note
/// forgotten since the update doesn't come back there. A file that already says what the database keeps
/// is left as it is, byte for byte, and no file is made to hold nothing. A file that isn't whole, or that
/// changes on the way, is left as it is. Nothing to do without a database.
pub async fn write_back(db: PathBuf, files: JsonFiles, now: i64) -> Result<WrittenBack, RuntimeError> {
    if !db.exists() {
        return Ok(WrittenBack::default());
    }
    tokio::task::spawn_blocking(move || write_back_now(&db, &files, now))
        .await
        .map_err(|why| RuntimeError::plain(why.to_string()))?
        .map_err(|why| RuntimeError::plain(why.to_string()))
}

fn write_back_now(db: &Path, files: &JsonFiles, now: i64) -> Result<WrittenBack, StoreError> {
    let store = Store::open(db)?;
    let imported = import_json(&store, files, now)?;
    let mut left: Vec<(PathBuf, String)> = imported.not_whole.iter().map(|path| (path.clone(), NOT_WHOLE.into())).collect();
    let mut with_files: Vec<String> = std::fs::read_dir(&files.projects)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| project_id(name) && files.projects.join(name).join("memory.json").is_file())
                .collect()
        })
        .unwrap_or_default();
    let (producer, sets, kept_techniques, kept_lessons) = store.read(move |c| {
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
    })?;
    let mut targets =
        vec![(files.memory.clone(), Kind::Notes(Scope::Global), Contents::Notes(producer.into_iter().map(note_of).collect()))];
    for (project, rows) in sets {
        targets.push((
            files.projects.join(&project).join("memory.json"),
            Kind::Notes(Scope::Project(project)),
            Contents::Notes(rows.into_iter().map(note_of).collect()),
        ));
    }
    targets.push((
        files.techniques.clone(),
        Kind::Techniques,
        Contents::Techniques(kept_techniques.into_iter().map(technique_of).collect()),
    ));
    targets.push((files.playbook.clone(), Kind::Lessons, Contents::Lessons(kept_lessons.into_iter().map(lesson_of).collect())));
    let paths: Vec<String> = targets.iter().map(|(path, ..)| path.to_string_lossy().into_owned()).collect();
    let records = store.read(move |c| {
        paths.iter().map(|path| Ok(imports::record_of(c, path)?.map(|record| record.blake3))).collect::<Result<Vec<_>, StoreError>>()
    })?;
    for ((path, kind, contents), merged) in targets.into_iter().zip(records) {
        if left.iter().any(|(skipped, _)| *skipped == path) {
            continue;
        }
        let there = match imports::read(&path, kind.name()) {
            Ok(there) => there,
            Err(why) => {
                left.push((path, format!("Kumi couldn't read it ({why})")));
                continue;
            }
        };
        match &there {
            // Nothing to write, and no file to write over.
            None if contents.is_empty() => continue,
            // Already says what the database keeps (in its own order and format): it stays as it is,
            // byte for byte, with the base the import just recorded.
            Some((_, bytes)) if Contents::read(&kind, bytes).canonical() == contents.canonical() => continue,
            // Not as the import just read it: changed since.
            Some((source, _)) if Some(&source.blake3) != merged.as_ref() => {
                left.push((path, CHANGED.into()));
                continue;
            }
            _ => {}
        }
        let expected = there.map(|(source, _)| source.blake3);
        let text = contents.text();
        match write_file(&path, &text, expected.as_deref()) {
            Ok(true) => {}
            Ok(false) => {
                left.push((path, CHANGED.into()));
                continue;
            }
            Err(why) => {
                left.push((path, format!("Kumi couldn't write it ({why})")));
                continue;
            }
        }
        // Its base is what was written, recorded before the next file: a stop partway leaves every file
        // written with its own base, and a change an older Kumi makes now is a change against it.
        let source = Source {
            path: path.to_string_lossy().into_owned(),
            kind: kind.name().into(),
            size: text.len() as i64,
            mtime: std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |at| at.as_millis() as i64),
            blake3: imports::hash(text.as_bytes()),
        };
        let rows = rows_of(&kind, text.as_bytes());
        store.write_wait(move |c| {
            let base = match &rows {
                Rows::Notes(scope, rows) => notes::base_of(c, scope, rows)?,
                Rows::Techniques(rows) => stored::base_of(c, rows)?,
                Rows::Lessons(rows) => lessons::base_of(c, rows)?,
                Rows::Gaps(_) => return Ok(()),
            };
            imports::record(c, &source, 0, Some(&base), now)
        })?;
    }
    Ok(WrittenBack { left })
}

/// Write `text` to `path` whole: a temporary file beside it (only the producer's), renamed into place
/// only while the file is still as it was read (`expected`, its blake3, or None for no file). Whether it
/// was written.
fn write_file(path: &Path, text: &str, expected: Option<&str>) -> std::io::Result<bool> {
    let folder = path.parent().filter(|folder| !folder.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(folder)?;
    let temporary = folder.join(format!(".kumi-{}", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        drop(file);
        let there = match std::fs::read(path) {
            Ok(bytes) => Some(imports::hash(&bytes)),
            Err(why) if why.kind() == std::io::ErrorKind::NotFound => None,
            Err(why) => return Err(why),
        };
        if there.as_deref() != expected {
            return Ok(false);
        }
        std::fs::rename(&temporary, path)?;
        Ok(true)
    })();
    if !matches!(written, Ok(true)) {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_read_before_a_newer_version_came_in_waits_for_its_next_read() {
        let dir = tempfile::tempdir().unwrap();
        let files = JsonFiles {
            memory: dir.path().join("memory.json"),
            projects: dir.path().join("projects"),
            techniques: dir.path().join("techniques.json"),
            playbook: dir.path().join("playbook.json"),
            gaps: dir.path().join("gaps.jsonl"),
        };
        let write = |notes: &str| std::fs::write(&files.memory, format!(r#"{{"version":1,"notes":[{notes}]}}"#)).unwrap();
        write(r#"{"id":"p1","text":"Likes short reverbs","at":100}"#);
        let store = Store::open(dir.path().join("kumi.db")).unwrap();
        import_json(&store, &files, 1).unwrap();
        // One Kumi reads the file an older Kumi just changed...
        write(r#"{"id":"p1","text":"Likes short reverbs on drums","at":200}"#);
        let Some(Read::Whole(source, rows)) = read_file(&files.memory, &Kind::Notes(Scope::Global)).unwrap() else {
            panic!("a whole file")
        };
        // ...which changes it again, and another Kumi brings that in first.
        write(r#"{"id":"p1","text":"Likes short reverbs on drums","at":200},{"id":"p2","text":"Works at 140","at":300}"#);
        import_json(&store, &files, 2).unwrap();
        // The first one's older read changes nothing: p2 isn't set aside.
        assert_eq!(store.write_wait(move |c| sync(c, &source, &rows, 3)).unwrap(), Imported::default());
        let texts: Vec<String> = store.read(|c| notes::in_use(c, &Scope::Global)).unwrap().into_iter().map(|n| n.text).collect();
        assert_eq!(texts, ["Likes short reverbs on drums", "Works at 140"]);
    }
}
