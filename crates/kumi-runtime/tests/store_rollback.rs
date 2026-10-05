//! Kumi's database and the files an older Kumi uses, kept in step: what the database keeps is written
//! back to the files before a rollback, and what an older Kumi changes in them (after a rollback, or
//! running beside this one) comes back in, by a three-way merge with each file's base.
use kumi_runtime::core::{
    contracts::MemoryStore,
    memory::{create_memory_store, parse_notes, MemoryStoreOptions, MAX_NOTES},
    playbook::parse_lessons,
    store_backed::SqliteMemoryStore,
    store_client::StoreClient,
    store_import::{import_json, write_back, BroughtIn, Imported, JsonFiles},
    techniques::parse_techniques,
};
use kumi_store::{notes, techniques, Kept, Scope, Store};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Barrier},
};

const SET: &str = "0123456789abcdef0123456789abcdef";
const EMPTIED: &str = "fedcba9876543210fedcba9876543210";

fn files(root: &Path) -> JsonFiles {
    JsonFiles {
        memory: root.join("memory.json"),
        projects: root.join("projects"),
        techniques: root.join("techniques.json"),
        playbook: root.join("playbook.json"),
        gaps: root.join("gaps.jsonl"),
    }
}
fn note(label: &str, text: &str, at: i64) -> notes::Note {
    notes::Note { label: label.into(), text: text.into(), pinned: false, at }
}
fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
fn texts(store: &Store, scope: Scope) -> Vec<String> {
    store.read(move |c| notes::in_use(c, &scope)).unwrap().into_iter().map(|n| n.text).collect()
}
/// A database as a newer Kumi leaves it: notes about the producer and one Set, a Set whose notes were all
/// forgotten (its file still holds one), a technique and a lesson.
fn newer_kumi(root: &Path) -> Store {
    let files = files(root);
    std::fs::create_dir_all(files.projects.join(EMPTIED)).unwrap();
    std::fs::write(
        files.projects.join(EMPTIED).join("memory.json"),
        r#"{"version":1,"notes":[{"id":"s1","text":"Forgotten since","at":1}]}"#,
    )
    .unwrap();
    let store = Store::open(root.join("kumi.db")).unwrap();
    import_json(&store, &files, 10).unwrap();
    store
        .write_wait(|c| {
            notes::forget(c, &Scope::Project(EMPTIED.into()), "s1", 20)?;
            notes::keep(c, &Scope::Global, &[note("p1", "Likes short reverbs", 100), note("p2", "Names drums in capitals", 200)], 300)?;
            notes::keep(c, &Scope::Project(SET.into()), &[note("s1", "The Reese is the main bass", 150)], 300)?;
            techniques::keep(
                c,
                &[techniques::Technique {
                    label: "t1".into(),
                    name: "Reese".into(),
                    fits: "dark basses".into(),
                    idea: "Two detuned saws through a low-pass".into(),
                    settings: None,
                    substitutes: None,
                    recipe: None,
                    source_title: None,
                    source_url: None,
                    request: Some("a darker reese".into()),
                    used: 1.0,
                    undone: 0.0,
                    at: 400,
                    updated: None,
                    last_used: None,
                }],
                300,
            )?;
            Ok(())
        })
        .unwrap();
    store
}

#[tokio::test]
async fn a_rollback_writes_back_what_the_database_keeps_in_the_older_kumis_format() {
    let dir = tempfile::tempdir().unwrap();
    let files = files(dir.path());
    drop(newer_kumi(dir.path()));
    write_back(dir.path().join("kumi.db"), files.clone(), 500).await.unwrap();

    let producer: Vec<String> = parse_notes(&std::fs::read(&files.memory).unwrap(), 'p').into_iter().map(|n| n.text).collect();
    assert_eq!(producer, ["Likes short reverbs", "Names drums in capitals"]);
    assert_eq!(read(&files.memory)["version"], 1);
    let set = parse_notes(&std::fs::read(files.projects.join(SET).join("memory.json")).unwrap(), 's');
    assert_eq!(set.iter().map(|n| (n.id.as_str(), n.text.as_str())).collect::<Vec<_>>(), [("s1", "The Reese is the main bass")]);
    assert_eq!(
        read(&files.projects.join(EMPTIED).join("memory.json"))["notes"],
        json!([]),
        "a Set's forgotten note doesn't come back there"
    );
    let older = create_memory_store(MemoryStoreOptions { projects_dir: files.projects.clone(), producer_file: files.memory.clone() });
    assert_eq!(older.load(Some(SET)).await.unwrap().set.len(), 1, "the older Kumi's own store reads it");
    assert_eq!(parse_techniques(&std::fs::read(&files.techniques).unwrap()).iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t1"]);
    assert!(parse_lessons(&std::fs::read(&files.playbook).unwrap()).is_empty());

    // Upgrading again with the files as they were written back changes nothing.
    let store = Store::open(dir.path().join("kumi.db")).unwrap();
    assert_eq!(import_json(&store, &files, 600).unwrap(), Imported::default());
    assert_eq!(texts(&store, Scope::Global), ["Likes short reverbs", "Names drums in capitals"]);
}

#[tokio::test]
async fn what_the_older_kumi_changed_comes_back_in_and_nothing_is_lost() {
    let dir = tempfile::tempdir().unwrap();
    let files = files(dir.path());
    drop(newer_kumi(dir.path()));
    write_back(dir.path().join("kumi.db"), files.clone(), 500).await.unwrap();

    // The older Kumi: p1 reworded, p2 forgotten, then the list filled up, so its oldest note made room.
    let mut kept = vec![json!({"id":"p1","text":"Likes short reverbs on drums","at":700})];
    for n in 0..MAX_NOTES {
        kept.push(json!({"id":format!("p{}", n + 3),"text":format!("Habit {n}"),"at":800 + n as i64}));
    }
    kept.remove(0);
    std::fs::write(&files.memory, json!({"version":1,"notes":kept}).to_string()).unwrap();

    let store = Store::open(dir.path().join("kumi.db")).unwrap();
    let imported = import_json(&store, &files, 900).unwrap();
    assert_eq!(imported.brought_in, BroughtIn { notes: Kept { added: MAX_NOTES, changed: 0, archived: 2 }, ..Default::default() });
    assert_eq!(
        imported.brought_in.sentence().as_deref(),
        Some("Brought in changes made with the older Kumi: 24 notes added, 2 notes removed.")
    );
    let now = texts(&store, Scope::Global);
    assert_eq!(now.len(), MAX_NOTES);
    assert!(now.iter().all(|t| t.starts_with("Habit ")), "{now:?}");
    let archived: Vec<String> = store
        .read(|c| {
            Ok(c.prepare("SELECT text FROM notes WHERE archived_at IS NOT NULL ORDER BY text")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?)
        })
        .unwrap();
    assert_eq!(archived, ["Likes short reverbs", "Names drums in capitals"], "set aside, not deleted");
    assert_eq!(
        texts(&store, Scope::Project(SET.into())),
        ["The Reese is the main bass"],
        "a file the older Kumi left alone changes nothing"
    );
    assert_eq!(import_json(&store, &files, 950).unwrap(), Imported::default(), "and once is enough");
}

#[tokio::test]
async fn two_rollbacks_and_upgrades_in_a_row_keep_every_change_and_no_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let files = files(dir.path());
    drop(newer_kumi(dir.path()));
    for (round, edit) in ["Likes short reverbs (round 1)", "Likes short reverbs (round 2)"].iter().enumerate() {
        write_back(dir.path().join("kumi.db"), files.clone(), 1000 + round as i64).await.unwrap();
        let mut older = read(&files.memory);
        older["notes"][0]["text"] = json!(edit);
        older["notes"][0]["at"] = json!(2000 + round);
        std::fs::write(&files.memory, older.to_string()).unwrap();
        let (client, imported) = StoreClient::open(dir.path().join("kumi.db"), files.clone(), 3000 + round as i64).await.unwrap();
        assert_eq!(imported.unwrap().brought_in.notes, Kept { added: 0, changed: 1, archived: 0 }, "round {round}");
        let producer = SqliteMemoryStore::new(client).load(None).await.unwrap().producer;
        assert_eq!(
            producer.iter().map(|n| (n.id.as_str(), n.text.as_str())).collect::<Vec<_>>(),
            [("p1", *edit), ("p2", "Names drums in capitals")]
        );
    }
}

#[tokio::test]
async fn a_file_that_cant_come_in_comes_in_whole_next_start_and_the_ones_before_it_stay_in() {
    let dir = tempfile::tempdir().unwrap();
    let files = files(dir.path());
    drop(newer_kumi(dir.path()));
    write_back(dir.path().join("kumi.db"), files.clone(), 500).await.unwrap();
    edit(&files.memory, |file| file["notes"][0]["text"] = json!("Likes short reverbs on drums"));
    let idea = "Two detuned saws through a low-pass, opened slowly. ".repeat(23);
    let many: Vec<Value> = (0..40)
        .map(|n| json!({"id":format!("t{}", n + 2),"name":format!("Technique {n}"),"fits":"pads","idea":idea,"at":600 + n}))
        .collect();
    std::fs::write(&files.techniques, json!({"version":1,"techniques":many}).to_string()).unwrap();

    let store = Store::open(dir.path().join("kumi.db")).unwrap();
    // The disk is full: the database can't grow.
    store
        .write_wait(|c| {
            let pages: i64 = c.query_row("PRAGMA page_count", [], |row| row.get(0))?;
            c.query_row(&format!("PRAGMA max_page_count = {pages}"), [], |_| Ok(()))?;
            Ok(())
        })
        .unwrap();
    assert!(import_json(&store, &files, 700).is_err());
    assert_eq!(texts(&store, Scope::Global)[0], "Likes short reverbs on drums", "the notes file came in in a transaction of its own");
    assert_eq!(
        store.read(techniques::in_use).unwrap().iter().map(|t| t.label.as_str()).collect::<Vec<_>>(),
        ["t1"],
        "and nothing of the techniques file"
    );
    store.write_wait(|c| Ok(c.query_row("PRAGMA max_page_count = 1073741823", [], |_| Ok(()))?)).unwrap();
    let imported = import_json(&store, &files, 800).unwrap();
    assert_eq!(imported.brought_in, BroughtIn { techniques: Kept { added: 40, changed: 0, archived: 1 }, ..Default::default() });
}

fn edit(path: &Path, change: impl FnOnce(&mut Value)) {
    let mut file = read(path);
    change(&mut file);
    std::fs::write(path, file.to_string()).unwrap();
}
/// The database as a start left it, the files read in (each with its base): two notes about the
/// producer and a technique.
fn read_in_once(root: &Path) -> (Store, JsonFiles) {
    let files = files(root);
    std::fs::write(
        &files.memory,
        r#"{"version":1,"notes":[{"id":"p1","text":"Likes short reverbs","at":100},{"id":"p2","text":"Names drums in capitals","at":200}]}"#,
    )
    .unwrap();
    std::fs::write(
        &files.techniques,
        r#"{"version":1,"techniques":[{"id":"t1","name":"Reese","fits":"dark basses","idea":"Two detuned saws through a low-pass","at":300,"used":2}]}"#,
    )
    .unwrap();
    let store = Store::open(root.join("kumi.db")).unwrap();
    import_json(&store, &files, 400).unwrap();
    (store, files)
}

#[test]
fn the_same_note_edited_in_the_file_and_in_the_database_is_kept_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    let (store, files) = read_in_once(dir.path());
    store.write_wait(|c| notes::update(c, &Scope::Global, &note("p1", "Likes short, dark reverbs", 500))).unwrap();
    edit(&files.memory, |file| {
        file["notes"][0]["text"] = json!("Likes short reverbs on drums");
        file["notes"][0]["at"] = json!(600);
    });
    assert_eq!(import_json(&store, &files, 700).unwrap().brought_in.notes, Kept { added: 1, changed: 0, archived: 0 });
    let now: Vec<(String, String)> =
        store.read(|c| notes::in_use(c, &Scope::Global)).unwrap().into_iter().map(|n| (n.label, n.text)).collect();
    assert_eq!(
        now,
        [
            ("p1".into(), "Likes short, dark reverbs".into()),
            ("p2".into(), "Names drums in capitals".into()),
            ("p3".into(), "Likes short reverbs on drums".into())
        ]
    );
    // Edited again in the file, the file's copy takes it.
    edit(&files.memory, |file| file["notes"][0]["text"] = json!("Likes short reverbs on drums and percussion"));
    assert_eq!(import_json(&store, &files, 800).unwrap().brought_in.notes, Kept { added: 0, changed: 1, archived: 0 });
    assert_eq!(
        texts(&store, Scope::Global),
        ["Likes short, dark reverbs", "Names drums in capitals", "Likes short reverbs on drums and percussion"]
    );
}

#[test]
fn a_note_set_aside_here_and_edited_in_the_file_is_back_in_use_as_itself() {
    let dir = tempfile::tempdir().unwrap();
    let (store, files) = read_in_once(dir.path());
    let id = |store: &Store| {
        store
            .read(|c| Ok(c.query_row("SELECT id FROM notes WHERE text LIKE 'Likes short reverbs%'", [], |row| row.get::<_, String>(0))?))
            .unwrap()
    };
    let before = id(&store);
    assert!(store.write_wait(|c| notes::archive(c, &Scope::Global, "p1", 500)).unwrap());
    edit(&files.memory, |file| file["notes"][0]["text"] = json!("Likes short reverbs on drums"));
    assert_eq!(import_json(&store, &files, 600).unwrap().brought_in.notes, Kept { added: 0, changed: 1, archived: 0 });
    assert_eq!(texts(&store, Scope::Global), ["Likes short reverbs on drums", "Names drums in capitals"]);
    assert_eq!(id(&store), before, "the same row, so what refers to it still does");
}

#[test]
fn an_older_kumi_open_beside_this_one_has_its_changes_brought_in_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, files) = read_in_once(dir.path());
    // This Kumi rewords p2, keeps a note, and a build uses the Reese.
    store
        .write_wait(|c| {
            notes::update(c, &Scope::Global, &note("p2", "Names drums in CAPITALS", 500))?;
            notes::insert(c, &Scope::Global, &note("p3", "Mixes on headphones", 510))?;
            techniques::record(c, "t1", false, 520)?;
            Ok(())
        })
        .unwrap();
    // The older Kumi, still open, rewords p1, keeps its own p3, and uses the Reese twice.
    edit(&files.memory, |file| {
        file["notes"][0]["text"] = json!("Likes short reverbs on drums");
        file["notes"].as_array_mut().unwrap().push(json!({"id":"p3","text":"Works at 140","at":600}));
    });
    edit(&files.techniques, |file| {
        file["techniques"][0]["used"] = json!(4);
        file["techniques"][0]["lastUsed"] = json!(610);
    });
    let imported = import_json(&store, &files, 700).unwrap();
    assert_eq!(imported.brought_in, BroughtIn { notes: Kept { added: 1, changed: 1, archived: 0 }, ..Default::default() });
    assert_eq!(
        texts(&store, Scope::Global),
        ["Likes short reverbs on drums", "Names drums in CAPITALS", "Mixes on headphones", "Works at 140"]
    );
    let reese = store.read(techniques::in_use).unwrap().remove(0);
    assert_eq!((reese.used, reese.last_used), (4.0, Some(610)), "the larger count of uses, and the latest");
    // It rewords its p3 again: that note takes it, once, however often this Kumi looks.
    edit(&files.memory, |file| file["notes"][2]["text"] = json!("Works at 140 BPM"));
    assert_eq!(import_json(&store, &files, 800).unwrap().brought_in.notes, Kept { added: 0, changed: 1, archived: 0 });
    assert_eq!(import_json(&store, &files, 900).unwrap(), Imported::default());
    assert_eq!(
        texts(&store, Scope::Global),
        ["Likes short reverbs on drums", "Names drums in CAPITALS", "Mixes on headphones", "Works at 140 BPM"]
    );
}

#[test]
fn two_kumis_noticing_the_same_change_bring_it_in_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, files) = read_in_once(dir.path());
    edit(&files.memory, |file| {
        file["notes"][0]["text"] = json!("Likes short reverbs on drums");
        file["notes"].as_array_mut().unwrap().push(json!({"id":"p3","text":"Works at 140","at":600}));
    });
    let other = Store::open(dir.path().join("kumi.db")).unwrap();
    let start = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [store.clone(), other]
        .into_iter()
        .map(|store| {
            let (files, start) = (files.clone(), start.clone());
            std::thread::spawn(move || {
                start.wait();
                import_json(&store, &files, 700).unwrap()
            })
        })
        .collect();
    let imported: Vec<Imported> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
    let mut brought_in = Kept::default();
    for one in &imported {
        brought_in += one.brought_in.notes;
    }
    assert_eq!(brought_in, Kept { added: 1, changed: 1, archived: 0 }, "{imported:?}");
    assert_eq!(texts(&store, Scope::Global), ["Likes short reverbs on drums", "Names drums in capitals", "Works at 140"]);
}

#[test]
fn a_file_that_isnt_whole_sets_nothing_aside_and_waits_until_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let (store, files) = read_in_once(dir.path());
    // Cut short, it would read as no notes at all.
    std::fs::write(&files.memory, r#"{"version":1,"notes":[{"id":"p1","text":"Likes sh"#).unwrap();
    assert_eq!(import_json(&store, &files, 500).unwrap(), Imported::default());
    assert_eq!(texts(&store, Scope::Global), ["Likes short reverbs", "Names drums in capitals"]);
    std::fs::write(&files.memory, r#"{"version":1,"notes":[{"id":"p2","text":"Names drums in capitals","at":200}]}"#).unwrap();
    assert_eq!(import_json(&store, &files, 600).unwrap().brought_in.notes, Kept { added: 0, changed: 0, archived: 1 });
    assert_eq!(texts(&store, Scope::Global), ["Names drums in capitals"]);
}
