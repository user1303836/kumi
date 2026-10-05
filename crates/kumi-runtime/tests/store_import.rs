use kumi_runtime::core::store_import::{import_json, Imported, JsonFiles};
use kumi_store::{gaps, lessons, notes, techniques, Scope, Store};
use serde_json::json;
use std::{path::Path, sync::Arc, time::SystemTime};

const SET: &str = "0123456789abcdef0123456789abcdef";

fn write(path: &Path, value: &serde_json::Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}
fn home() -> (tempfile::TempDir, JsonFiles) {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    let files = JsonFiles {
        memory: root.join("memory.json"),
        projects: root.join("projects"),
        techniques: root.join("techniques.json"),
        playbook: root.join("playbook.json"),
        gaps: root.join("gaps.jsonl"),
    };
    write(
        &files.memory,
        &json!({"version":1,"notes":[
        {"id":"p1","text":"Likes short reverbs on drums","at":1000,"pinned":false},
        {"id":"p2","text":"Names drum tracks in capitals","at":2000,"pinned":true}]}),
    );
    write(
        &files.projects.join(SET).join("memory.json"),
        &json!({"version":1,"notes":[{"id":"s1","text":"The Reese is the main bass","at":3000}]}),
    );
    write(&files.projects.join("not-a-set").join("memory.json"), &json!({"version":1,"notes":[{"id":"s1","text":"Not read","at":1}]}));
    write(
        &files.techniques,
        &json!({"version":1,"techniques":[
        {"id":"t1","name":"Reese","fits":"dark basses","idea":"Two detuned saws through a low-pass","at":4000,"used":2,"source":{"title":"A tutorial","url":"https://example.com/t"}},
        {"id":"t2","name":"Air pad","fits":"wide pads","idea":"Noise through a resonant band-pass","at":4500,"used":0,"request":"an airy pad"}]}),
    );
    write(
        &files.playbook,
        &json!({"version":1,"lessons":[
        {"id":"l0a1b2c3d","at":5000,"matched":"the reference pad","winner":"Wavetable","from":41,"to":77,"moves":[{"label":"brighter","score":63}],"reaction":"liked"}]}),
    );
    std::fs::write(
        &files.gaps,
        "{\"at\":\"2026-10-04T12:00:00.000Z\",\"kumi\":\"1.8.0\",\"missing\":\"setting Operator's voice count\"}\nnot json\n{\"at\":\"2026-10-04T13:00:00.000Z\",\"kumi\":\"1.8.1\",\"missing\":\"a time selection\",\"asked\":\"delete bars 9-12\"}\n",
    )
    .unwrap();
    (folder, files)
}
fn labels_and_texts(store: &Store, scope: Scope) -> Vec<(String, String)> {
    let mut rows: Vec<_> = store.read(move |c| notes::in_use(c, &scope)).unwrap().into_iter().map(|n| (n.label, n.text)).collect();
    rows.sort();
    rows
}
fn fingerprint(files: &JsonFiles) -> Vec<(Vec<u8>, SystemTime)> {
    [&files.memory, &files.projects.join(SET).join("memory.json"), &files.techniques, &files.playbook, &files.gaps]
        .iter()
        .map(|path| (std::fs::read(path).unwrap(), std::fs::metadata(path).unwrap().modified().unwrap()))
        .collect()
}

#[test]
fn what_earlier_kumis_kept_in_files_is_read_in_once_and_the_files_are_left_as_they_were() {
    let (folder, files) = home();
    let before = fingerprint(&files);
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    assert_eq!(import_json(&store, &files, 9000).unwrap(), Imported { files: 5, rows: 8 });
    assert_eq!(fingerprint(&files), before, "the files are never written");

    assert_eq!(
        labels_and_texts(&store, Scope::Global),
        [("p1".into(), "Likes short reverbs on drums".into()), ("p2".into(), "Names drum tracks in capitals".into())]
    );
    assert!(store.read(|c| notes::in_use(c, &Scope::Global)).unwrap().iter().any(|n| n.label == "p2" && n.pinned));
    assert_eq!(labels_and_texts(&store, Scope::Project(SET.into())), [("s1".into(), "The Reese is the main bass".into())]);
    let kept = store.read(techniques::in_use).unwrap();
    assert_eq!(kept.iter().map(|t| (t.label.as_str(), t.used)).collect::<Vec<_>>(), [("t1", 2.0), ("t2", 0.0)]);
    assert_eq!((kept[0].source_url.as_deref(), kept[1].request.as_deref()), (Some("https://example.com/t"), Some("an airy pad")));
    let lesson = store.read(lessons::in_use).unwrap().remove(0);
    assert_eq!(
        (lesson.label.as_str(), lesson.moves.clone(), lesson.reaction.as_deref()),
        ("l0a1b2c3d", json!([{"label":"brighter","score":63.0}]), Some("liked"))
    );
    let logged = store.read(gaps::all).unwrap();
    assert_eq!(logged.iter().map(|g| g.missing.as_str()).collect::<Vec<_>>(), ["setting Operator's voice count", "a time selection"]);
    assert_eq!(logged[1].asked.as_deref(), Some("delete bars 9-12"));

    assert_eq!(import_json(&store, &files, 9001).unwrap(), Imported::default(), "files read in as they are now aren't read again");
}

#[test]
fn a_file_an_older_kumi_changed_adds_what_it_added_and_never_brings_back_a_forgotten_note() {
    let (folder, files) = home();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    import_json(&store, &files, 9000).unwrap();
    assert!(store.write_wait(|c| notes::forget(c, &Scope::Global, "p1", 9100)).unwrap());
    // An older Kumi (after a rollback) kept a new note and reworded p2 in the file.
    write(
        &files.memory,
        &json!({"version":1,"notes":[
        {"id":"p1","text":"Likes short reverbs on drums","at":1000},
        {"id":"p2","text":"Names drum tracks in capitals, always","at":9200,"pinned":true},
        {"id":"p3","text":"Works at 140","at":9300}]}),
    );
    assert_eq!(import_json(&store, &files, 9400).unwrap(), Imported { files: 1, rows: 2 });
    let now = labels_and_texts(&store, Scope::Global);
    let texts: Vec<&str> = now.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(texts.len(), 3, "{now:?}");
    for text in ["Names drum tracks in capitals", "Names drum tracks in capitals, always", "Works at 140"] {
        assert!(texts.contains(&text), "{text} is kept: {now:?}");
    }
    assert!(!texts.contains(&"Likes short reverbs on drums"), "a forgotten note stays forgotten");
    let mut labels: Vec<&str> = now.iter().map(|(label, _)| label.as_str()).collect();
    labels.dedup();
    assert_eq!(labels.len(), 3, "no two notes share a label: {now:?}");
}

#[test]
fn a_note_written_back_for_an_older_kumi_is_the_same_note_when_read_in_again() {
    let (folder, files) = home();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    import_json(&store, &files, 9000).unwrap();
    // Kept in the database, then written back to the file for an older Kumi (a rollback), then read in.
    store
        .write_wait(|c| {
            let mut kept = notes::in_use(c, &Scope::Global)?;
            kept.push(notes::Note { label: "p5".into(), text: "Sidechains the pad to the kick".into(), pinned: false, at: 9500 });
            notes::keep(c, &Scope::Global, &kept, 9500)
        })
        .unwrap();
    let written: Vec<_> = store
        .read(|c| notes::in_use(c, &Scope::Global))
        .unwrap()
        .into_iter()
        .map(|n| json!({"id":n.label,"text":n.text,"at":n.at,"pinned":n.pinned}))
        .collect();
    write(&files.memory, &json!({"version":1,"notes":written}));
    assert_eq!(import_json(&store, &files, 9600).unwrap(), Imported { files: 1, rows: 0 });
    assert_eq!(labels_and_texts(&store, Scope::Global).len(), 3);
}

#[test]
fn two_kumis_starting_at_once_read_the_files_in_once() {
    let (folder, files) = home();
    let path = folder.path().join("kumi.db");
    let stores = [Store::open(&path).unwrap(), Store::open(&path).unwrap()];
    let start = Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = stores
        .iter()
        .map(|store| {
            let (store, files, start) = (store.clone(), files.clone(), start.clone());
            std::thread::spawn(move || {
                start.wait();
                import_json(&store, &files, 9000).unwrap()
            })
        })
        .collect();
    let imported: Vec<Imported> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
    assert_eq!(imported.iter().map(|i| i.rows).sum::<usize>(), 8, "{imported:?}");
    assert_eq!(stores[0].read(|c| Ok(c.query_row("SELECT count(*) FROM notes", [], |row| row.get::<_, i64>(0))?)).unwrap(), 3);
}
