//! Notes, techniques and lessons kept in Kumi's database, behind the same stores the JSON files served.
//! A row a full list sets aside is archived, not dropped; a forgotten one is deleted, and its id kept so
//! no import brings it back.

use super::{
    contracts::{Memory, MemoryNote, MemoryScope, MemoryStore},
    errors::RuntimeError,
    memory::fit,
    playbook::{Lesson, PlaybookStore, MAX_LESSONS},
    store_client::StoreClient,
    store_rows::{lesson_of, lesson_row, note_of, note_row, technique_of, technique_row},
    techniques::{Technique, TechniqueStore, MAX_TECHNIQUES},
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
        let notes = |rows: Vec<notes::Note>| {
            let mut found: Vec<MemoryNote> = rows.into_iter().map(note_of).collect();
            fit(&mut found);
            found
        };
        Ok(Memory { producer: notes(producer), set: notes(set) })
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
        let mut list: Vec<Technique> = self.client.read(techniques::in_use).await.map_err(error)?.into_iter().map(technique_of).collect();
        list.drain(..list.len().saturating_sub(MAX_TECHNIQUES));
        Ok(list)
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
        let mut list: Vec<Lesson> = self.client.read(lessons::in_use).await.map_err(error)?.into_iter().map(lesson_of).collect();
        list.drain(..list.len().saturating_sub(MAX_LESSONS));
        Ok(list)
    }
    async fn save(&self, list: &[Lesson]) -> Result<(), RuntimeError> {
        let rows: Vec<lessons::Lesson> = list[list.len().saturating_sub(MAX_LESSONS)..].iter().map(lesson_row).collect();
        self.client.write(move |c| lessons::keep(c, &rows, now_ms())).await.map_err(error)
    }
}
