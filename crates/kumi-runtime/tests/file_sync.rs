//! Once a turn, Kumi looks at the files an older Kumi open beside it writes, and brings in what changed.
use kumi_runtime::core::{file_sync::FileSync, store_client::StoreClient, store_import::JsonFiles};
use kumi_store::{Connection, Kept};
use std::path::Path;

fn files(root: &Path) -> JsonFiles {
    JsonFiles {
        memory: root.join("memory.json"),
        projects: root.join("projects"),
        techniques: root.join("techniques.json"),
        playbook: root.join("playbook.json"),
        gaps: root.join("gaps.jsonl"),
    }
}
const ONE: &str = r#"{"version":1,"notes":[{"id":"p1","text":"Likes short reverbs","at":100}]}"#;
const TWO: &str = r#"{"version":1,"notes":[{"id":"p1","text":"Likes short reverbs","at":100},{"id":"p2","text":"Works at 140","at":200}]}"#;

#[tokio::test]
async fn a_look_brings_in_what_changed_once_and_passes_over_files_as_they_were() {
    let dir = tempfile::tempdir().unwrap();
    let files = files(dir.path());
    std::fs::write(&files.memory, ONE).unwrap();
    let (client, _) = StoreClient::open(dir.path().join("kumi.db"), files.clone(), 1).await.unwrap();
    let sync = FileSync::new(client, files.clone());
    assert_eq!(
        sync.look().await.unwrap().map(|looked| looked.brought_in),
        Some(Default::default()),
        "the first look reads them as they are"
    );
    assert_eq!(sync.look().await.unwrap(), None, "and the next sees nothing changed");
    std::fs::write(&files.memory, TWO).unwrap();
    let looked = sync.look().await.unwrap().unwrap();
    assert_eq!(looked.brought_in.notes, Kept { added: 1, changed: 0, archived: 0 });
    assert_eq!(looked.brought_in.sentence().as_deref(), Some("Brought in changes made with the older Kumi: 1 note added."));
    assert_eq!(sync.look().await.unwrap(), None);
}

#[tokio::test]
async fn a_look_under_way_isnt_started_again() {
    let dir = tempfile::tempdir().unwrap();
    let files = files(dir.path());
    std::fs::write(&files.memory, ONE).unwrap();
    let (client, _) = StoreClient::open(dir.path().join("kumi.db"), files.clone(), 1).await.unwrap();
    let sync = FileSync::new(client, files.clone());
    std::fs::write(&files.memory, TWO).unwrap();
    // Another Kumi holds the database's write lock, so the first look waits for it.
    let other = Connection::open(dir.path().join("kumi.db")).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let first = tokio::spawn({
        let sync = sync.clone();
        async move { sync.look().await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(sync.look().await.unwrap(), None, "a look is under way");
    other.execute_batch("ROLLBACK").unwrap();
    assert_eq!(first.await.unwrap().unwrap().unwrap().brought_in.notes, Kept { added: 1, changed: 0, archived: 0 });
}
