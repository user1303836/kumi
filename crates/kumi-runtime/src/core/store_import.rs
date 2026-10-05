//! What earlier Kumis kept in JSON files (notes, techniques, lessons, gaps), read into Kumi's database
//! once per version of each file. The files are never changed, so an older Kumi still finds them.

use super::{
    memory::parse_notes,
    playbook::parse_lessons,
    store_rows::{lesson_row, note_row, technique_row},
    techniques::parse_techniques,
};
use kumi_store::{gaps, imports, lessons, notes, techniques as stored, Scope, Store, StoreError};
use serde_json::Value;
use std::path::PathBuf;

/// Where the files are: each path as the producer's settings name it (`KUMI_MEMORY_FILE` and the rest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonFiles {
    pub memory: PathBuf,
    pub projects: PathBuf,
    pub techniques: PathBuf,
    pub playbook: PathBuf,
    pub gaps: PathBuf,
}

/// What an import read in: files new to the database (or changed since), and the rows they added.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Imported {
    pub files: usize,
    pub rows: usize,
}

enum Rows {
    Notes(Scope, Vec<notes::Note>),
    Techniques(Vec<stored::Technique>),
    Lessons(Vec<lessons::Lesson>),
    Gaps(Vec<gaps::Gap>),
}

/// Read the files into the database. A file not read in as it is now adds the rows the database doesn't
/// have, and never one forgotten there; nothing is deleted. Blocking (it reads the files and waits for
/// the write), so it runs on a blocking thread before the first read. Each file is checked inside the
/// write, so two Kumis starting at once import it once.
pub fn import_json(store: &Store, files: &JsonFiles, now: i64) -> Result<Imported, StoreError> {
    let mut sources = vec![];
    if let Some((source, bytes)) = imports::read(&files.memory, "notes")? {
        sources.push((source, Rows::Notes(Scope::Global, parse_notes(&bytes, 'p').iter().map(note_row).collect())));
    }
    let mut projects: Vec<String> = std::fs::read_dir(&files.projects)
        .map(|entries| entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()).filter(|name| project_id(name)).collect())
        .unwrap_or_default();
    projects.sort();
    for project in projects {
        if let Some((source, bytes)) = imports::read(&files.projects.join(&project).join("memory.json"), "notes")? {
            sources.push((source, Rows::Notes(Scope::Project(project), parse_notes(&bytes, 's').iter().map(note_row).collect())));
        }
    }
    if let Some((source, bytes)) = imports::read(&files.techniques, "techniques")? {
        sources.push((source, Rows::Techniques(parse_techniques(&bytes).iter().map(technique_row).collect())));
    }
    if let Some((source, bytes)) = imports::read(&files.playbook, "lessons")? {
        sources.push((source, Rows::Lessons(parse_lessons(&bytes).iter().map(lesson_row).collect())));
    }
    if let Some((source, bytes)) = imports::read(&files.gaps, "gaps")? {
        sources.push((source, Rows::Gaps(parse_gaps(&String::from_utf8_lossy(&bytes)))));
    }
    store.write_wait(move |c| {
        let mut imported = Imported::default();
        for (source, rows) in &sources {
            if imports::imported(c, source)? {
                continue;
            }
            let mut added = 0;
            match rows {
                Rows::Notes(scope, rows) => {
                    for row in rows {
                        added += notes::import(c, scope, row)? as usize;
                    }
                }
                Rows::Techniques(rows) => {
                    for row in rows {
                        added += stored::import(c, row)? as usize;
                    }
                }
                Rows::Lessons(rows) => {
                    for row in rows {
                        added += lessons::import(c, row)? as usize;
                    }
                }
                Rows::Gaps(rows) => {
                    for row in rows {
                        added += gaps::import(c, row)? as usize;
                    }
                }
            }
            imports::record(c, source, added, now)?;
            imported.files += 1;
            imported.rows += added;
        }
        Ok(imported)
    })
}

/// A project folder's name: the 32 lowercase hex digits Kumi names a Set's folder with.
fn project_id(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The gaps a gaps log holds: a line that isn't a whole entry (time and what was missing) is left out.
fn parse_gaps(text: &str) -> Vec<gaps::Gap> {
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
