use kumi_common::abort;
use kumi_runtime::core::{
    contracts::{MemoryEvent, MemoryScope, MemoryStore},
    gaps::gap_tools,
    goal::Best,
    memory::{memory_tools, MemoryToolsOptions, MAX_NOTES},
    playbook::{Lesson, PlaybookStore, Reaction},
    store_backed::{SqliteMemoryStore, SqlitePlaybookStore, SqliteTechniqueStore},
    store_client::StoreClient,
    store_import::JsonFiles,
    techniques::{Technique, TechniqueBody, TechniqueSource, TechniqueStore},
};
use kumi_store::{gaps, Store};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn database() -> (tempfile::TempDir, StoreClient) {
    let dir = tempfile::tempdir().unwrap();
    let client = StoreClient::new(Store::open(dir.path().join("kumi.db")).unwrap());
    (dir, client)
}
fn count(client: &StoreClient, sql: &'static str) -> i64 {
    client.store().read(|c| Ok(c.query_row(sql, [], |row| row.get(0))?)).unwrap()
}

#[tokio::test]
async fn notes_kept_in_the_database_work_as_they_did_in_files_and_none_is_lost_to_a_full_list() {
    let (_dir, client) = database();
    let store = Rc::new(SqliteMemoryStore::new(client.clone()));
    let events = Rc::new(RefCell::new(vec![]));
    let notes = memory_tools(MemoryToolsOptions {
        store: store.clone(),
        project: Rc::new(|| Some(PROJECT.into())),
        set: None,
        on_event: {
            let events = events.clone();
            Rc::new(move |e| events.borrow_mut().push(e))
        },
    });
    let call = |index: usize, input: Value| {
        let tool = notes.tools[index].clone();
        async move { tool.execute(input.as_object().unwrap().clone(), abort::never()).await.unwrap() }
    };
    call(0, json!({"note":"The Reese is the main bass","about":"set"})).await;
    call(0, json!({"note":"Likes short, dark reverbs on drums","about":"producer"})).await;
    call(0, json!({"note":"Likes short, dark reverbs on drums and percussion","about":"producer","replaces":"p1"})).await;
    let memory = store.load(Some(PROJECT)).await.unwrap();
    assert_eq!(memory.set.iter().map(|n| (n.id.as_str(), n.text.as_str())).collect::<Vec<_>>(), [("s1", "The Reese is the main bass")]);
    assert_eq!(
        memory.producer.iter().map(|n| (n.id.as_str(), n.text.as_str())).collect::<Vec<_>>(),
        [("p1", "Likes short, dark reverbs on drums and percussion")]
    );

    // Forgetting deletes the note and remembers it as forgotten.
    call(1, json!({"id":"s1"})).await;
    assert!(store.load(Some(PROJECT)).await.unwrap().set.is_empty());
    assert!(matches!(events.borrow().last(), Some(MemoryEvent::Forgot { .. })));
    assert_eq!(count(&client, "SELECT count(*) FROM forgotten"), 1);
    assert_eq!(count(&client, "SELECT count(*) FROM notes WHERE scope_kind = 'project'"), 0);

    // A full list makes room by setting its oldest unpinned note aside, which stays in the database.
    for n in 0..MAX_NOTES {
        call(0, json!({"note":format!("A habit of the producer, number {n}"),"about":"producer"})).await;
    }
    let producer = store.load(None).await.unwrap().producer;
    assert_eq!(producer.len(), MAX_NOTES);
    assert!(!producer.iter().any(|n| n.text.starts_with("Likes short")), "the oldest is out of the prompt");
    assert_eq!(count(&client, "SELECT count(*) FROM notes WHERE archived_at IS NOT NULL"), 1, "and kept, archived");
    assert!(store.forget(MemoryScope::Producer, None, "p99").await.unwrap().is_none());
}

#[tokio::test]
async fn techniques_and_lessons_round_trip_through_the_database() {
    let (_dir, client) = database();
    let techniques = SqliteTechniqueStore::new(client.clone());
    let technique = |id: &str, name: &str| Technique {
        body: TechniqueBody {
            name: name.into(),
            fits: "dark basses".into(),
            idea: "Two detuned saws through a low-pass".into(),
            settings: Some("cutoff 400 Hz".into()),
            substitutes: None,
            recipe: None,
            source: Some(TechniqueSource { title: Some("A tutorial".into()), url: Some("https://example.com/t".into()) }),
        },
        id: id.into(),
        at: 1000.0,
        used: 2.0,
        updated: Some(2000.0),
        last_used: None,
        request: Some("a darker reese".into()),
        undone: 1.0,
    };
    techniques.save(&[technique("t1", "Reese"), technique("t2", "Air pad")]).await.unwrap();
    assert_eq!(techniques.list().await.unwrap(), [technique("t1", "Reese"), technique("t2", "Air pad")]);
    assert_eq!(techniques.forget("t1").await.unwrap(), Some(technique("t1", "Reese")));
    assert_eq!(techniques.list().await.unwrap(), [technique("t2", "Air pad")]);
    assert_eq!(count(&client, "SELECT count(*) FROM forgotten"), 1);

    let playbook = SqlitePlaybookStore::new(client.clone());
    let lesson = Lesson {
        id: "l0a1b2c3d".into(),
        at: 3000.0,
        matched: "the reference pad".into(),
        winner: "Wavetable".into(),
        from: 41.0,
        to: 77.0,
        moves: vec![Best { label: "brighter".into(), score: 63.0 }],
        reaction: Some(Reaction::Liked),
    };
    playbook.save(std::slice::from_ref(&lesson)).await.unwrap();
    assert_eq!(playbook.list().await.unwrap(), [lesson]);
}

#[tokio::test]
async fn gaps_go_to_the_database_when_there_is_one() {
    let (dir, client) = database();
    let file = dir.path().join("gaps.jsonl");
    let tools = gap_tools(&file, Some(client.clone()));
    let result = tools[0]
        .execute(
            json!({"missing":"setting Operator's voice count","asked":"make the Reese mono"}).as_object().unwrap().clone(),
            abort::never(),
        )
        .await
        .unwrap();
    assert_eq!(result.text, r#"{"noted":"setting Operator's voice count"}"#);
    let logged = client.store().read(gaps::all).unwrap();
    assert_eq!((logged[0].missing.as_str(), logged[0].asked.as_deref()), ("setting Operator's voice count", Some("make the Reese mono")));
    assert!(!file.exists(), "not the file");
}

#[tokio::test]
async fn the_database_opens_with_the_files_read_in_or_says_why_not() {
    let dir = tempfile::tempdir().unwrap();
    let files = JsonFiles {
        memory: dir.path().join("memory.json"),
        projects: dir.path().join("projects"),
        techniques: dir.path().join("techniques.json"),
        playbook: dir.path().join("playbook.json"),
        gaps: dir.path().join("gaps.jsonl"),
    };
    std::fs::write(&files.memory, r#"{"version":1,"notes":[{"id":"p1","text":"Mixes on headphones","at":1000}]}"#).unwrap();
    let client = StoreClient::open(dir.path().join("kumi.db"), files.clone(), 2000).await.unwrap();
    let producer = SqliteMemoryStore::new(client).load(None).await.unwrap().producer;
    assert_eq!(producer.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["Mixes on headphones"]);
    std::fs::write(dir.path().join("a file"), "not a folder").unwrap();
    assert!(StoreClient::open(dir.path().join("a file").join("kumi.db"), files, 2000).await.is_err(), "Kumi keeps the files instead");
}
