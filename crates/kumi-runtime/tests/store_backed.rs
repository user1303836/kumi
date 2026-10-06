use kumi_common::abort;
use kumi_runtime::core::{
    contracts::{MemoryEvent, MemoryScope, MemoryStore, NoteChange},
    gaps::gap_tools,
    goal::Best,
    memory::{create_memory_store, memory_instructions, memory_tools, MemoryStoreOptions, MemoryToolsOptions, MAX_NOTES},
    playbook::{create_playbook_store, Lesson, PlaybookStore, Reaction},
    store_backed::{SqliteMemoryStore, SqlitePlaybookStore, SqliteTechniqueStore},
    store_client::StoreClient,
    store_import::JsonFiles,
    techniques::{
        create_technique_store, technique_instructions, Technique, TechniqueBody, TechniqueDraft, TechniqueSource, TechniqueStore,
    },
};
use kumi_store::{gaps, Store};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashSet, path::Path, rc::Rc};

const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn database() -> (tempfile::TempDir, StoreClient) {
    let dir = tempfile::tempdir().unwrap();
    let client = StoreClient::new(Store::open(dir.path().join("kumi.db")).unwrap());
    (dir, client)
}
fn count(client: &StoreClient, sql: &'static str) -> i64 {
    client.store().read(|c| Ok(c.query_row(sql, [], |row| row.get(0))?)).unwrap()
}
fn json_files(root: &Path) -> JsonFiles {
    JsonFiles {
        memory: root.join("memory.json"),
        projects: root.join("projects"),
        techniques: root.join("techniques.json"),
        playbook: root.join("playbook.json"),
        gaps: root.join("gaps.jsonl"),
    }
}
fn draft(name: &str, idea: &str) -> TechniqueDraft {
    TechniqueDraft {
        body: TechniqueBody {
            name: name.into(),
            fits: "dark basses".into(),
            idea: idea.into(),
            settings: None,
            substitutes: None,
            recipe: None,
            source: None,
        },
        replaces: None,
    }
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
    let files = json_files(dir.path());
    std::fs::write(&files.memory, r#"{"version":1,"notes":[{"id":"p1","text":"Mixes on headphones","at":1000}]}"#).unwrap();
    let (client, imported) = StoreClient::open(dir.path().join("kumi.db"), files.clone(), 2000).await.unwrap();
    assert!(imported.is_ok());
    let producer = SqliteMemoryStore::new(client).load(None).await.unwrap().producer;
    assert_eq!(producer.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["Mixes on headphones"]);
    std::fs::write(dir.path().join("a file"), "not a folder").unwrap();
    assert!(StoreClient::open(dir.path().join("a file").join("kumi.db"), files, 2000).await.is_err(), "Kumi keeps the files instead");
}

#[tokio::test]
async fn a_file_that_cant_be_read_in_leaves_the_database_in_use_and_comes_in_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let files = json_files(dir.path());
    // A folder where the producer's notes should be: that file can't be read.
    std::fs::create_dir(&files.memory).unwrap();
    std::fs::write(
        &files.techniques,
        r#"{"version":1,"techniques":[{"id":"t1","name":"Reese","fits":"dark basses","idea":"Two detuned saws through a low-pass","at":1000}]}"#,
    )
    .unwrap();
    let (client, imported) = StoreClient::open(dir.path().join("kumi.db"), files.clone(), 2000).await.unwrap();
    assert!(imported.is_err(), "{imported:?}");
    SqliteMemoryStore::new(client.clone()).remember(MemoryScope::Producer, None, "Mixes on headphones", None, 3000).await.unwrap();
    assert!(SqliteTechniqueStore::new(client.clone()).list().await.unwrap().is_empty(), "nothing read in by halves");
    drop(client);

    std::fs::remove_dir(&files.memory).unwrap();
    std::fs::write(&files.memory, r#"{"version":1,"notes":[{"id":"p1","text":"Likes short reverbs","at":1000}]}"#).unwrap();
    let (client, imported) = StoreClient::open(dir.path().join("kumi.db"), files, 4000).await.unwrap();
    assert_eq!(imported.unwrap().files, 2);
    let producer = SqliteMemoryStore::new(client.clone()).load(None).await.unwrap().producer;
    assert_eq!(producer.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["Likes short reverbs", "Mixes on headphones"]);
    assert_eq!(SqliteTechniqueStore::new(client).list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn two_kumis_at_once_keep_every_note_and_every_use_and_share_no_id() {
    let dir = tempfile::tempdir().unwrap();
    // Two Kumis, each with its own connections and writer, on one database.
    let open = || StoreClient::new(Store::open(dir.path().join("kumi.db")).unwrap());
    let (one, two) = (open(), open());
    let notes = |client: StoreClient, who: &'static str| async move {
        let store = SqliteMemoryStore::new(client);
        for n in 0..10 {
            store.remember(MemoryScope::Producer, None, &format!("{who} habit {n}"), None, 1000 + n).await.unwrap();
        }
    };
    tokio::join!(notes(one.clone(), "First"), notes(two.clone(), "Second"));
    let producer = SqliteMemoryStore::new(one.clone()).load(None).await.unwrap().producer;
    assert_eq!(producer.len(), 20, "none lost");
    assert_eq!(producer.iter().map(|n| n.id.as_str()).collect::<HashSet<_>>().len(), 20, "no id kept twice");

    let techniques = |client: StoreClient, who: &'static str| async move {
        let store = SqliteTechniqueStore::new(client);
        for n in 0..5 {
            store.keep(draft(&format!("{who} technique {n}"), "Two detuned saws through a low-pass"), None, 2000.0).await.unwrap();
        }
    };
    tokio::join!(techniques(one.clone(), "First"), techniques(two.clone(), "Second"));
    let uses = |client: StoreClient| async move {
        let store = SqliteTechniqueStore::new(client);
        for n in 0..10 {
            store.record("t1", false, 3000.0 + n as f64).await.unwrap();
        }
    };
    tokio::join!(uses(one.clone()), uses(two));
    let list = SqliteTechniqueStore::new(one).list().await.unwrap();
    assert_eq!(list.iter().map(|t| t.id.as_str()).collect::<HashSet<_>>().len(), 10, "every technique, each its own id");
    assert_eq!(list.iter().find(|t| t.id == "t1").unwrap().used, 20.0, "every use counted");
}

async fn remember(memory: &dyn MemoryStore, text: &str, replaces: Option<&str>, at: i64) {
    memory.remember(MemoryScope::Producer, None, text, replaces, at).await.unwrap();
}
/// The same notes, techniques and lessons kept, changed and forgotten through these stores: what the
/// prompt then says of the notes and techniques, and the lessons.
async fn kept_through(
    memory: &dyn MemoryStore,
    techniques: &dyn TechniqueStore,
    playbook: &dyn PlaybookStore,
) -> (String, String, Vec<Lesson>) {
    // Kept in one millisecond.
    for n in 0..22 {
        remember(memory, &format!("Habit {n}"), None, 100).await;
    }
    remember(memory, "Likes short reverbs", None, 110).await;
    remember(memory, "Names drums in capitals", None, 110).await;
    memory.remember(MemoryScope::Set, Some(PROJECT), "The Reese is the main bass", None, 110).await.unwrap();
    // Kept in place of itself, p23 goes last; reworded, p24 keeps its place.
    remember(memory, "Likes short, dark reverbs", Some("p23"), 120).await;
    memory.change(MemoryScope::Producer, None, "p24", NoteChange::Text("Names drums in CAPITALS".into()), 130).await.unwrap();
    memory.change(MemoryScope::Producer, None, "p24", NoteChange::Pinned(true), 140).await.unwrap();
    memory.forget(MemoryScope::Producer, None, "p5").await.unwrap();
    // There's room for one of these.
    memory.add(MemoryScope::Producer, None, &["Mixes on headphones".into(), "Works at 140 BPM".into()], 150).await.unwrap();
    // The list is full: the first of the oldest makes room.
    remember(memory, "Habit 22", None, 200).await;
    let notes = memory_instructions(&memory.load(Some(PROJECT)).await.unwrap(), Some("Night Drive"));

    techniques.keep(draft("Reese", "Two detuned saws through a low-pass"), Some("a darker reese".into()), 1000.0).await.unwrap();
    techniques.keep(draft("Air pad", "Noise through a resonant band-pass"), None, 1000.0).await.unwrap();
    techniques.keep(draft("Parallel filter", "Two filters side by side, blended"), None, 1000.0).await.unwrap();
    // Refined by its name, in its place.
    techniques.keep(draft("reese", "Three detuned saws through a low-pass"), None, 1100.0).await.unwrap();
    techniques.record("t2", false, 1200.0).await.unwrap();
    techniques.record("t3", false, 1250.0).await.unwrap();
    techniques.record("t3", true, 1300.0).await.unwrap();
    techniques.forget("t2").await.unwrap();
    for n in 0..9 {
        techniques.keep(draft(&format!("Technique {n}"), "One saw through a comb filter"), None, 1400.0).await.unwrap();
    }
    let list = techniques.list().await.unwrap();

    let lesson = |id: &str, matched: &str| Lesson {
        id: id.into(),
        at: 3000.0,
        matched: matched.into(),
        winner: "Wavetable".into(),
        from: 41.0,
        to: 77.0,
        moves: vec![Best { label: "brighter".into(), score: 63.0 }],
        reaction: None,
    };
    playbook.put(&lesson("la0000001", "the reference pad")).await.unwrap();
    playbook.put(&lesson("la0000002", "the reference bass")).await.unwrap();
    playbook.put(&lesson("la0000003", "the reference lead")).await.unwrap();
    playbook.react("la0000002", Reaction::Liked).await.unwrap();
    // Kept again, in the same millisecond, a lesson goes last.
    playbook.put(&lesson("la0000001", "the reference pad, closer")).await.unwrap();
    playbook.forget("la0000003").await.unwrap();
    (notes, technique_instructions(&list), playbook.list().await.unwrap())
}

#[tokio::test]
async fn the_prompt_reads_the_same_from_the_database_as_from_the_files() {
    let dir = tempfile::tempdir().unwrap();
    let files = json_files(dir.path());
    let from_files = kept_through(
        &*create_memory_store(MemoryStoreOptions { projects_dir: files.projects.clone(), producer_file: files.memory.clone() }),
        &*create_technique_store(&files.techniques),
        &*create_playbook_store(&files.playbook),
    )
    .await;
    let (_dir, client) = database();
    let from_database = kept_through(
        &SqliteMemoryStore::new(client.clone()),
        &SqliteTechniqueStore::new(client.clone()),
        &SqlitePlaybookStore::new(client),
    )
    .await;
    assert_eq!(from_database, from_files);

    let (notes, techniques, lessons) = from_files;
    for kept in
        ["Habit 1", "Habit 21", "Names drums in CAPITALS", "Likes short, dark reverbs", "Mixes on headphones", "Habit 22", "The Reese"]
    {
        assert!(notes.contains(kept), "{kept}: {notes}");
    }
    for gone in ["Habit 0", "Habit 4", "Likes short reverbs", "capitals", "Works at 140 BPM"] {
        assert!(!notes.contains(gone), "{gone}: {notes}");
    }
    assert!(
        techniques.contains("[t1] reese") && techniques.contains("[t12] Technique 8") && !techniques.contains("Air pad"),
        "{techniques}"
    );
    assert_eq!(lessons.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["la0000002", "la0000001"]);
}
