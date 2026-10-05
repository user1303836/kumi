//! Notes, techniques and lessons kept in Kumi's database, behind the same stores the JSON files served.
//! A row a full list sets aside is archived, not dropped; a forgotten one is deleted, and its id kept so
//! no import brings it back.

use super::{
    contracts::{Memory, MemoryNote, MemoryScope, MemoryStore, NoteChange, Remembering},
    errors::RuntimeError,
    memory::{add_in, checked_note, fit, remember_in},
    playbook::{checked_lesson, Lesson, PlaybookStore, Reaction, MAX_LESSONS},
    store_client::StoreClient,
    store_rows::{lesson_of, lesson_row, note_of, note_row, technique_of, technique_row},
    techniques::{checked_technique, keep_in, Technique, TechniqueDraft, TechniqueStore, MAX_TECHNIQUES},
};
use async_trait::async_trait;
use kumi_common::time::now_ms;
use kumi_store::{lessons, notes, techniques, Scope, StoreError};

fn error(why: StoreError) -> RuntimeError {
    RuntimeError::plain(why.to_string())
}
/// A saved Set's project id: the 32 lowercase hex digits its folder is named with.
fn project_id(project: &str) -> bool {
    project.len() == 32 && project.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn scope_of(scope: MemoryScope, project: Option<&str>) -> Result<Scope, RuntimeError> {
    match scope {
        MemoryScope::Producer => Ok(Scope::Global),
        MemoryScope::Set => {
            project.filter(|p| project_id(p)).map(|p| Scope::Project(p.into())).ok_or_else(|| RuntimeError::plain("invalid project id"))
        }
    }
}

pub struct SqliteMemoryStore {
    client: StoreClient,
}
impl SqliteMemoryStore {
    pub fn new(client: StoreClient) -> SqliteMemoryStore {
        SqliteMemoryStore { client }
    }
}
#[async_trait(?Send)]
impl MemoryStore for SqliteMemoryStore {
    async fn load(&self, project: Option<&str>) -> Result<Memory, RuntimeError> {
        let set = project.filter(|p| project_id(p)).map(|p| Scope::Project(p.into()));
        let (producer, set) = self
            .client
            .read(move |c| {
                Ok((notes::in_use(c, &Scope::Global)?, set.map(|scope| notes::in_use(c, &scope)).transpose()?.unwrap_or_default()))
            })
            .await
            .map_err(error)?;
        let notes = |rows: Vec<notes::Note>, scope| {
            let mut found: Vec<MemoryNote> = rows.into_iter().filter_map(|n| checked_note(note_of(n), scope)).collect();
            fit(&mut found);
            found
        };
        Ok(Memory { producer: notes(producer, MemoryScope::Producer), set: notes(set, MemoryScope::Set) })
    }
    async fn remember(
        &self,
        scope: MemoryScope,
        project: Option<&str>,
        text: &str,
        replaces: Option<&str>,
        at: i64,
    ) -> Result<Remembering, RuntimeError> {
        // Read, decide and write in one transaction, so two Kumis keeping notes at once can't take one
        // label or make room with the same note.
        let (kind, text, replaces) = (scope_of(scope, project)?, text.to_string(), replaces.map(str::to_string));
        self.client
            .write(move |c| {
                let mut list: Vec<MemoryNote> = notes::in_use(c, &kind)?.into_iter().map(note_of).collect();
                let remembering = remember_in(&mut list, scope, &text, replaces.as_deref(), at);
                if let Remembering::Kept { note, replaced } = &remembering {
                    match replaced {
                        Some(old) if old.id == note.id => {
                            notes::replace(c, &kind, &note_row(note))?;
                        }
                        Some(old) => {
                            notes::archive(c, &kind, &old.id, at)?;
                            notes::insert(c, &kind, &note_row(note))?;
                        }
                        None => notes::insert(c, &kind, &note_row(note))?,
                    }
                }
                Ok(remembering)
            })
            .await
            .map_err(error)
    }
    async fn change(
        &self,
        scope: MemoryScope,
        project: Option<&str>,
        id: &str,
        change: NoteChange,
        at: i64,
    ) -> Result<Option<MemoryNote>, RuntimeError> {
        let (kind, label) = (scope_of(scope, project)?, id.to_string());
        self.client
            .write(move |c| {
                let Some(mut note) = notes::in_use(c, &kind)?.into_iter().find(|n| n.label == label) else { return Ok(None) };
                match change {
                    NoteChange::Text(text) => {
                        note.text = text;
                        note.at = at;
                    }
                    NoteChange::Pinned(pinned) => note.pinned = pinned,
                }
                notes::update(c, &kind, &note)?;
                Ok(Some(note_of(note)))
            })
            .await
            .map_err(error)
    }
    async fn add(&self, scope: MemoryScope, project: Option<&str>, texts: &[String], at: i64) -> Result<(), RuntimeError> {
        let (kind, texts) = (scope_of(scope, project)?, texts.to_vec());
        self.client
            .write(move |c| {
                let mut list: Vec<MemoryNote> = notes::in_use(c, &kind)?.into_iter().map(note_of).collect();
                for note in add_in(&mut list, scope, &texts, at) {
                    notes::insert(c, &kind, &note_row(&note))?;
                }
                Ok(())
            })
            .await
            .map_err(error)
    }
    async fn save(&self, scope: MemoryScope, project: Option<&str>, notes: &[MemoryNote]) -> Result<(), RuntimeError> {
        let scope = scope_of(scope, project)?;
        let mut kept = notes.to_vec();
        fit(&mut kept);
        let rows: Vec<notes::Note> = kept.iter().map(note_row).collect();
        self.client.write(move |c| notes::keep(c, &scope, &rows, now_ms())).await.map_err(error)
    }
    async fn forget(&self, scope: MemoryScope, project: Option<&str>, id: &str) -> Result<Option<MemoryNote>, RuntimeError> {
        let scope = scope_of(scope, project)?;
        let label = id.to_string();
        self.client
            .write(move |c| {
                let found = notes::in_use(c, &scope)?.into_iter().find(|n| n.label == label);
                if found.is_some() {
                    notes::forget(c, &scope, &label, now_ms())?;
                }
                Ok(found.map(note_of))
            })
            .await
            .map_err(error)
    }
}

pub struct SqliteTechniqueStore {
    client: StoreClient,
}
impl SqliteTechniqueStore {
    pub fn new(client: StoreClient) -> SqliteTechniqueStore {
        SqliteTechniqueStore { client }
    }
}
#[async_trait(?Send)]
impl TechniqueStore for SqliteTechniqueStore {
    async fn list(&self) -> Result<Vec<Technique>, RuntimeError> {
        let mut list: Vec<Technique> = self
            .client
            .read(techniques::in_use)
            .await
            .map_err(error)?
            .into_iter()
            .filter_map(|t| checked_technique(technique_of(t)))
            .collect();
        list.drain(..list.len().saturating_sub(MAX_TECHNIQUES));
        Ok(list)
    }
    async fn keep(&self, draft: TechniqueDraft, request: Option<String>, at: f64) -> Result<(Technique, bool), RuntimeError> {
        // Read, decide and write in one transaction: two Kumis keeping techniques at once can't take one
        // id or make room with the same technique.
        self.client
            .write(move |c| {
                let mut list: Vec<Technique> = techniques::in_use(c)?.into_iter().map(technique_of).collect();
                let (kept, refined, evicted) = keep_in(&mut list, draft, request, at);
                if let Some(evicted) = evicted {
                    techniques::archive(c, &evicted.id, at as i64)?;
                }
                techniques::put(c, &technique_row(&kept))?;
                Ok((kept, refined))
            })
            .await
            .map_err(error)
    }
    async fn record(&self, id: &str, undone: bool, at: f64) -> Result<(), RuntimeError> {
        let label = id.to_string();
        self.client.write(move |c| techniques::record(c, &label, undone, at as i64)).await.map(drop).map_err(error)
    }
    async fn save(&self, list: &[Technique]) -> Result<(), RuntimeError> {
        let rows: Vec<techniques::Technique> = list[list.len().saturating_sub(MAX_TECHNIQUES)..].iter().map(technique_row).collect();
        self.client.write(move |c| techniques::keep(c, &rows, now_ms())).await.map_err(error)
    }
    async fn forget(&self, id: &str) -> Result<Option<Technique>, RuntimeError> {
        let label = id.to_string();
        self.client
            .write(move |c| {
                let found = techniques::in_use(c)?.into_iter().find(|t| t.label == label);
                if found.is_some() {
                    techniques::forget(c, &label, now_ms())?;
                }
                Ok(found.map(technique_of))
            })
            .await
            .map_err(error)
    }
}

pub struct SqlitePlaybookStore {
    client: StoreClient,
}
impl SqlitePlaybookStore {
    pub fn new(client: StoreClient) -> SqlitePlaybookStore {
        SqlitePlaybookStore { client }
    }
}
#[async_trait(?Send)]
impl PlaybookStore for SqlitePlaybookStore {
    async fn list(&self) -> Result<Vec<Lesson>, RuntimeError> {
        let mut list: Vec<Lesson> =
            self.client.read(lessons::in_use).await.map_err(error)?.into_iter().filter_map(|l| checked_lesson(lesson_of(l))).collect();
        list.drain(..list.len().saturating_sub(MAX_LESSONS));
        Ok(list)
    }
    async fn put(&self, lesson: &Lesson) -> Result<bool, RuntimeError> {
        let row = lesson_row(lesson);
        self.client.write(move |c| lessons::put(c, &row, MAX_LESSONS, now_ms())).await.map_err(error)
    }
    async fn forget(&self, id: &str) -> Result<Option<Lesson>, RuntimeError> {
        let label = id.to_string();
        self.client
            .write(move |c| {
                let found = lessons::in_use(c)?.into_iter().find(|l| l.label == label);
                if found.is_some() {
                    lessons::forget(c, &label, now_ms())?;
                }
                Ok(found.map(lesson_of))
            })
            .await
            .map_err(error)
    }
    async fn react(&self, id: &str, reaction: Reaction) -> Result<(), RuntimeError> {
        let (label, reaction) = (id.to_string(), if reaction == Reaction::Liked { "liked" } else { "disliked" });
        self.client.write(move |c| lessons::react(c, &label, reaction)).await.map(drop).map_err(error)
    }
    async fn save(&self, list: &[Lesson]) -> Result<(), RuntimeError> {
        let rows: Vec<lessons::Lesson> = list[list.len().saturating_sub(MAX_LESSONS)..].iter().map(lesson_row).collect();
        self.client.write(move |c| lessons::keep(c, &rows, now_ms())).await.map_err(error)
    }
}
