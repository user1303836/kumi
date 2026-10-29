//! Kept conversations found by id or name across every Set, with names, pins and copies beside them, an index made
//! again from the files, locks between Kumis, and keeping at least what Kumi 1.16 kept: hermetic, in temporary folders.
use kumi_agent::core::{
    contracts::{Claim, ConversationMatch, ConversationStore, ConversationSummary, SavedConversation},
    conversations::{create_conversation_store, create_conversation_store_keeping, fresh_id, title},
    session_log::log_name,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const A: &str = "0123456789abcdef0123456789abcdef";
const B: &str = "fedcba9876543210fedcba9876543210";
const DAY: Duration = Duration::from_secs(86_400);

fn said(text: &str, at: i64) -> SavedConversation {
    serde_json::from_value(json!({"savedAt":at,"checkpoint":{"version":1,"messages":[
        {"role":"user","content":text},{"role":"assistant","content":"done"}
    ]}}))
    .unwrap()
}

fn age(file: &Path, ago: Duration) {
    let file = std::fs::OpenOptions::new().write(true).open(file).unwrap();
    file.set_modified(SystemTime::now() - ago).unwrap();
}

/// Kumi 1.16.3's reading of a place's folder, as it was: what a rollback reads.
mod kumi_1_16_3 {
    use super::*;
    pub fn conversation(id: &str) -> bool {
        let bytes = id.as_bytes();
        (6..=24).contains(&bytes.len()) && bytes.iter().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
    }
    pub fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .map(|dir| dir.flatten().filter_map(|entry| entry.file_name().to_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        names.retain(|name| name.strip_suffix(".json").is_some_and(conversation));
        names.sort();
        names
    }
    /// Its reader: anything but these fields is dropped, and a file without them isn't a conversation.
    pub fn read(file: &Path) -> Option<Value> {
        let value: Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
        if !value["savedAt"].is_number()
            || value["checkpoint"]["version"].as_f64() != Some(1.0)
            || !value["checkpoint"]["messages"].is_array()
        {
            return None;
        }
        Some(value)
    }
    fn current(place: &Path) -> Option<String> {
        let text = std::fs::read_to_string(place.join("current")).ok()?;
        let id = text.trim().to_owned();
        conversation(&id).then_some(id)
    }
    fn modified(file: &Path) -> f64 {
        std::fs::metadata(file)
            .ok()
            .and_then(|s| s.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    }
    /// Its trim: the current one and the newest others, 20 in all.
    pub fn trim(place: &Path) {
        let kept = place.join("conversations");
        let current = format!("{}.json", current(place).as_deref().unwrap_or("undefined"));
        let names = names(&kept);
        let mut others: Vec<(&String, f64)> = names.iter().filter(|n| **n != current).map(|n| (n, modified(&kept.join(n)))).collect();
        others.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (name, _) in others.into_iter().skip(20 - usize::from(names.contains(&current))) {
            std::fs::remove_file(kept.join(name)).unwrap();
        }
    }
    /// The fields it knows of a conversation, the only ones it writes.
    fn known(value: &Value) -> Value {
        let mut known = json!({});
        for key in ["savedAt", "checkpoint", "changes", "first", "turns"] {
            if !value[key].is_null() {
                known[key] = value[key].clone();
            }
        }
        known
    }
    /// Its save: the conversation, as much as it knows of it, then the place's current one, then its trim.
    pub fn save(place: &Path, id: &str, conversation: &Value) {
        let kept = place.join("conversations");
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::write(kept.join(format!("{id}.json")), known(conversation).to_string()).unwrap();
        std::fs::write(place.join("current"), id).unwrap();
        trim(place);
    }
    /// Its move of an unsaved Set's conversation to the Set's own place, once the Set is saved.
    pub fn move_conversation(projects: &Path, id: &str, from: &str, to: &str) {
        let file = projects.join(from).join("conversations").join(format!("{id}.json"));
        let Some(value) = read(&file) else { return };
        let target = projects.join(to);
        std::fs::create_dir_all(target.join("conversations")).unwrap();
        std::fs::write(target.join("conversations").join(format!("{id}.json")), known(&value).to_string()).unwrap();
        std::fs::write(target.join("current"), id).unwrap();
        std::fs::remove_file(&file).unwrap();
        if current(&projects.join(from)).as_deref() == Some(id) {
            std::fs::write(projects.join(from).join("current"), "").unwrap();
        }
        trim(&target);
    }
    /// What its `kumi report` took for the last conversation: the newest .json in any place's folder.
    pub fn report_candidates(projects: &Path) -> Vec<PathBuf> {
        let mut found = vec![];
        for place in std::fs::read_dir(projects).unwrap().flatten() {
            for file in std::fs::read_dir(place.path().join("conversations")).into_iter().flatten().flatten() {
                if file.file_name().to_string_lossy().ends_with(".json") {
                    found.push(file.path());
                }
            }
        }
        found
    }
}

#[test]
fn a_new_id_is_made_again_when_its_taken() {
    let taken = ["mv66me0m3fa91c", "mv66me0m4bb000"];
    let mut made = taken.iter().map(|id| id.to_string()).chain(["mv66me0m5cc111".to_string()]);
    assert_eq!(fresh_id(|id| taken.contains(&id), || made.next().unwrap()), "mv66me0m5cc111");
}

/// A conversation's own fields of the wrong kind are read past, the rest of it kept, as Kumi 1.16.3 read it; a file
/// without a conversation's makings is none.
#[tokio::test(flavor = "current_thread")]
async fn a_conversations_fields_of_the_wrong_kind_are_read_past_and_the_rest_kept() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    let kept = dir.path().join(A).join("conversations");
    std::fs::create_dir_all(&kept).unwrap();
    let load = |file: Value| {
        std::fs::write(kept.join("aaaaaa000001.json"), file.to_string()).unwrap();
        async { store.load(A, "aaaaaa000001").await.unwrap() }
    };
    let mut file = serde_json::to_value(said("make the bass wider", 7)).unwrap();
    file["later"] = json!({"what": "a later Kumi's"});
    for (key, wrong) in [("changes", json!("not a list")), ("first", json!(7)), ("turns", json!("two"))] {
        file[key] = wrong;
    }
    let read = load(file.clone()).await.expect("a conversation");
    assert_eq!((read.saved_at, read.checkpoint.messages.len()), (7, 2));
    assert_eq!((read.changes, read.first, read.turns), (None, None, None));
    for (key, right) in [("changes", json!([])), ("first", json!("make the bass wider")), ("turns", json!(1))] {
        file[key] = right;
    }
    let read = load(file.clone()).await.expect("a conversation");
    assert_eq!((read.changes, read.first.as_deref(), read.turns), (Some(vec![]), Some("make the bass wider"), Some(1)));
    for broken in [
        json!({"checkpoint": file["checkpoint"]}),
        json!({"savedAt": 7, "checkpoint": {"version": 2, "messages": []}}),
        json!({"savedAt": 7, "checkpoint": {"version": 1, "messages": {}}}),
        json!([file]),
    ] {
        assert!(load(broken.clone()).await.is_none(), "{broken}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_id_is_none_kept_in_any_set() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    store.save(A, "aaaaaa000001", &said("one", 1)).await.unwrap();
    store.save(B, "bbbbbb000002", &said("two", 2)).await.unwrap();
    let id = store.new_id().await;
    assert!(kumi_1_16_3::conversation(&id) && id != "aaaaaa000001" && id != "bbbbbb000002", "{id}");
    // Two new ones in a row differ, as the clock and six random digits make them.
    assert_ne!(store.new_id().await, store.new_id().await);
}

#[tokio::test(flavor = "current_thread")]
async fn a_conversation_is_found_by_its_id_the_start_of_it_or_its_name_in_any_set() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    store.save(A, "mv66me0m3fa91c", &said("make the bass wider", 10)).await.unwrap();
    store.save(B, "mv66zz0m3fa91c", &said("tighten the drums", 20)).await.unwrap();
    store.save("unsaved", "kq01aa000000", &said("sketch some chords", 30)).await.unwrap();
    assert!(store.rename(B, "mv66zz0m3fa91c", Some("Drum Bus")).await.unwrap());
    let one = |found: ConversationMatch| match found {
        ConversationMatch::One(row) => (row.place, row.id),
        other => panic!("{other:?}"),
    };
    assert_eq!(one(store.find("mv66me0m3fa91c").await.unwrap()), (A.to_string(), "mv66me0m3fa91c".to_string()));
    assert_eq!(one(store.find("  MV66ZZ ").await.unwrap()), (B.to_string(), "mv66zz0m3fa91c".to_string()));
    assert_eq!(one(store.find("drum bus").await.unwrap()), (B.to_string(), "mv66zz0m3fa91c".to_string()));
    assert_eq!(one(store.find("kq01").await.unwrap()), ("unsaved".to_string(), "kq01aa000000".to_string()));
    // Several fit: the producer picks, newest first.
    let ConversationMatch::Several(rows) = store.find("mv66").await.unwrap() else { panic!() };
    assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["mv66zz0m3fa91c", "mv66me0m3fa91c"]);
    // Too short to be the start of an id, a first request isn't a name, and nothing at all.
    for word in ["mv6", "make the bass wider", "nothing", ""] {
        assert_eq!(store.find(word).await.unwrap(), ConversationMatch::None, "{word}");
    }
    // Every Set's, newest first, each with its place and current flag.
    let all = store.all().await.unwrap();
    assert_eq!(all.iter().map(|r| (r.place.as_str(), r.current)).collect::<Vec<_>>(), [("unsaved", true), (B, true), (A, true)]);
    assert_eq!(all[1].title(), "Drum Bus");
    assert_eq!(all[2].title(), "make the bass wider");
}

#[tokio::test(flavor = "current_thread")]
async fn one_let_go_says_so_and_one_deleted_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    for n in 0..22 {
        store.save(A, &format!("old{n:03}aaaaaa"), &said(&format!("request {n}"), n)).await.unwrap();
    }
    // The two oldest, unused for 100 days, beyond the 20 newest and not current.
    for n in 0..2 {
        age(&dir.path().join(A).join(format!("conversations/old{n:03}aaaaaa.json")), 100 * DAY);
    }
    store.tidy(A).await.unwrap();
    assert_eq!(store.list(A).await.unwrap().len(), 20);
    assert_eq!(store.find("old000aaaaaa").await.unwrap(), ConversationMatch::Gone);
    assert!(store.delete(A, "old005aaaaaa").await.unwrap());
    assert!(!store.delete(A, "old005aaaaaa").await.unwrap());
    assert_eq!(store.find("old005aaaaaa").await.unwrap(), ConversationMatch::None);
    assert!(store.load(A, "old005aaaaaa").await.unwrap().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn names_pins_and_copies_live_beside_the_conversation_where_kumi_1_16_reads_past_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    store.save(A, "mv66me0m3fa91c", &said("make the bass wider", 10)).await.unwrap();
    assert!(store.rename(A, "mv66me0m3fa91c", Some("  Bass\nideas  ")).await.unwrap());
    assert!(store.pin(A, "mv66me0m3fa91c", true).await.unwrap());
    store.create(A, "mv66me0m9fork1", &said("make the bass wider", 11), None, Some("mv66me0m3fa91c")).await.unwrap();
    store.create(B, "mv66me0m9copy1", &said("make the bass wider", 12), Some(" Copy\n"), Some("mv66me0m3fa91c")).await.unwrap();
    assert_eq!(store.claim(A, "mv66me0m9fork1"), Claim::Ours);
    assert!(!store.rename(A, "nothere00000", Some("x")).await.unwrap(), "only a kept one");
    let rows = store.all().await.unwrap();
    let row = |id: &str| rows.iter().find(|r| r.id == id).unwrap().clone();
    assert_eq!((row("mv66me0m3fa91c").name.as_deref(), row("mv66me0m3fa91c").pinned), (Some("Bass ideas"), true));
    assert_eq!(row("mv66me0m9fork1").forked_from.as_deref(), Some("mv66me0m3fa91c"));
    assert_eq!(
        (row("mv66me0m9copy1").forked_from.as_deref(), row("mv66me0m9copy1").name.as_deref()),
        (Some("mv66me0m3fa91c"), Some("Copy"))
    );
    // Kumi 1.16.3 reads every conversation file, lists only them, and its report never takes a name, pin or lock for one.
    for place in [A, B] {
        let kept = dir.path().join(place).join("conversations");
        let names = kumi_1_16_3::names(&kept);
        assert!(!names.is_empty());
        for name in &names {
            assert!(kumi_1_16_3::read(&kept.join(name)).is_some(), "{name}");
        }
        let all: BTreeSet<String> =
            std::fs::read_dir(&kept).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(all.iter().any(|name| name.ends_with(".meta")) && all.len() > names.len(), "{all:?}");
    }
    assert!(dir.path().join(A).join("conversations/mv66me0m9fork1.lock").exists());
    assert_eq!(create_conversation_store(dir.path()).claim(A, "mv66me0m9fork1"), Claim::Taken);
    for file in kumi_1_16_3::report_candidates(dir.path()) {
        assert!(kumi_1_16_3::read(&file).is_some(), "{}", file.display());
    }
    // Unpinned and unnamed, nothing is left beside it.
    assert!(store.rename(A, "mv66me0m3fa91c", None).await.unwrap());
    assert!(store.pin(A, "mv66me0m3fa91c", false).await.unwrap());
    assert!(!dir.path().join(A).join("conversations/mv66me0m3fa91c.meta").exists());
    store.release(A, "mv66me0m9fork1");
    assert_eq!(create_conversation_store(dir.path()).claim(A, "mv66me0m9fork1"), Claim::Ours);
}

#[tokio::test(flavor = "current_thread")]
async fn the_index_made_again_from_the_files_is_the_one_kept() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    std::fs::create_dir_all(dir.path().join(A)).unwrap();
    std::fs::write(
        dir.path().join(A).join("last-seen.json"),
        r#"{"version":1,"path":"/Music/Night Drive.als","name":"Night Drive","pages":[]}"#,
    )
    .unwrap();
    store.save(A, "mv66me0m3fa91c", &said("make the bass wider", 10)).await.unwrap();
    store.save(B, "mv66zz0m3fa91c", &said("tighten the drums", 20)).await.unwrap();
    store.save("unsaved", "kq01aa000000", &said("sketch some chords", 30)).await.unwrap();
    store.rename(B, "mv66zz0m3fa91c", Some("Drum Bus")).await.unwrap();
    store.pin(A, "mv66me0m3fa91c", true).await.unwrap();
    store.create(A, "mv66me0m9fork1", &said("make the bass wider", 40), None, Some("mv66me0m3fa91c")).await.unwrap();
    store.move_conversation("kq01aa000000", "unsaved", B).await.unwrap();
    store.delete(B, "mv66zz0m3fa91c").await.unwrap();
    let file = dir.path().join("conversation-index.json");
    let entries = |text: &str| {
        let mut value: Value = serde_json::from_str(text).unwrap();
        value.as_object_mut().unwrap().remove("gone");
        value
    };
    let kept = entries(&std::fs::read_to_string(&file).unwrap());
    assert_eq!(kept["places"][A]["name"], "Night Drive");
    assert_eq!(kept["conversations"].as_object().unwrap().len(), 3, "{kept}");
    let listed = store.all().await.unwrap();
    // Gone, or unreadable: made again by a look at the files.
    for broken in [None, Some("{broken")] {
        match broken {
            None => std::fs::remove_file(&file).unwrap(),
            Some(text) => std::fs::write(&file, text).unwrap(),
        }
        assert_eq!(store.all().await.unwrap(), listed);
        assert_eq!(entries(&std::fs::read_to_string(&file).unwrap()), kept);
    }
    // A file another Kumi wrote is read again.
    let other = create_conversation_store(dir.path());
    other.save(A, "mv66me0m3fa91c", &said("make the bass wider, and mono below 120 Hz", 50)).await.unwrap();
    let row = store.all().await.unwrap().into_iter().find(|r| r.id == "mv66me0m3fa91c").unwrap();
    assert_eq!((row.saved_at, row.pinned, row.set.as_deref()), (50, true, Some("Night Drive")));
}

#[tokio::test(flavor = "current_thread")]
async fn a_conversation_another_open_kumi_has_is_taken_until_that_kumi_lets_go() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    store.save(A, "mv66me0m3fa91c", &said("make the bass wider", 10)).await.unwrap();
    // Another Kumi (another store, its own lock handle) has it open.
    let other = create_conversation_store(dir.path());
    assert_eq!(other.claim(A, "mv66me0m3fa91c"), Claim::Ours);
    assert_eq!(store.claim(A, "mv66me0m3fa91c"), Claim::Taken);
    // It names the process for anyone looking (on Windows, nobody else can read a locked file).
    if cfg!(unix) {
        let lock = std::fs::read_to_string(dir.path().join(A).join("conversations/mv66me0m3fa91c.lock")).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&lock).unwrap()["pid"], std::process::id());
    }
    // Not let go while it's open there, however old; and not deleted from under it.
    for n in 0..21 {
        store.save(A, &format!("new{n:03}aaaaaa"), &said("newer", 20 + n)).await.unwrap();
    }
    age(&dir.path().join(A).join("conversations/mv66me0m3fa91c.json"), 200 * DAY);
    store.tidy(A).await.unwrap();
    assert!(store.load(A, "mv66me0m3fa91c").await.unwrap().is_some());
    assert!(store.delete(A, "mv66me0m3fa91c").await.unwrap_err().to_string().contains("open in another Kumi window"));
    // That Kumi closed, or crashed: the system let go, and this one takes it.
    drop(other);
    assert_eq!(store.claim(A, "mv66me0m3fa91c"), Claim::Ours);
    assert_eq!(store.claim(A, "mv66me0m3fa91c"), Claim::Ours, "its own again");
    // Let go, it's anyone's; a store dropped lets go of what it held.
    store.release(A, "mv66me0m3fa91c");
    let third = create_conversation_store(dir.path());
    assert_eq!(third.claim(A, "mv66me0m3fa91c"), Claim::Ours);
    assert_eq!(store.claim(A, "mv66me0m3fa91c"), Claim::Taken);
    drop(third);
    assert_eq!(store.claim(A, "mv66me0m3fa91c"), Claim::Ours);
}

/// Another Kumi, still running, as a lock file names it. On Unix, the process that started the tests: one started here
/// takes the tests' open files with it for a moment, and with them the locks other tests hold. On Windows, where a new
/// process takes none, one of the test's own.
fn another_kumi() -> (Option<std::process::Child>, Value) {
    #[cfg(unix)]
    let (child, pid) = (None, std::os::unix::process::parent_id());
    #[cfg(windows)]
    let (child, pid) = {
        let mut command = std::process::Command::new("ping");
        let child = command.args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        let pid = child.id();
        (Some(child), pid)
    };
    let started = kumi_common::process::started_at_ms(pid).unwrap();
    (child, json!({"pid":pid,"started":started}))
}

#[tokio::test(flavor = "current_thread")]
async fn where_the_file_system_cant_lock_the_lock_file_names_the_kumi_that_has_it() {
    let dir = tempfile::tempdir().unwrap();
    // As on a network drive without locks: taking the system's lock fails.
    let store = create_conversation_store(dir.path());
    store.without_file_locks();
    store.save(A, "mv66me0m3fa91c", &said("make the bass wider", 10)).await.unwrap();
    let lock = dir.path().join(A).join("conversations/mv66me0m3fa91c.lock");
    // Another Kumi that couldn't lock either named itself, and still runs: it has the conversation, for a Kumi that can
    // lock too.
    let (other, named) = another_kumi();
    std::fs::write(&lock, named.to_string()).unwrap();
    assert_eq!(store.claim(A, "mv66me0m3fa91c"), Claim::Taken);
    assert_eq!(create_conversation_store(dir.path()).claim(A, "mv66me0m3fa91c"), Claim::Taken);
    // Not let go while it's open there, however old; and not deleted from under it.
    for n in 0..21 {
        store.save(A, &format!("new{n:03}aaaaaa"), &said("newer", 20 + n)).await.unwrap();
    }
    age(&dir.path().join(A).join("conversations/mv66me0m3fa91c.json"), 200 * DAY);
    store.tidy(A).await.unwrap();
    assert!(store.load(A, "mv66me0m3fa91c").await.unwrap().is_some());
    assert!(store.delete(A, "mv66me0m3fa91c").await.unwrap_err().to_string().contains("open in another Kumi window"));
    // That Kumi is gone, and its pid went to a process started later: this one names itself.
    std::fs::write(&lock, json!({"pid":named["pid"],"started":1}).to_string()).unwrap();
    assert_eq!(store.claim(A, "mv66me0m3fa91c"), Claim::Ours);
    let says: Value = serde_json::from_str(&std::fs::read_to_string(&lock).unwrap()).unwrap();
    assert_eq!(says["pid"], std::process::id());
    // Let go, the file goes; a store dropped lets go of what it held.
    store.release(A, "mv66me0m3fa91c");
    assert!(!lock.exists());
    assert_eq!(store.claim(A, "mv66me0m3fa91c"), Claim::Ours);
    drop(store);
    assert!(!lock.exists());
    if let Some(mut other) = other {
        let _ = other.kill();
        let _ = other.wait();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_lock_kumi_cant_take_or_write_never_stops_a_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    store.save(A, "mv66me0m3fa91c", &said("make the bass wider", 10)).await.unwrap();
    // `<id>.lock` can be neither locked nor written (here, a folder): the conversation goes on, held by nobody.
    let lock = dir.path().join(A).join("conversations/mv66me0m3fa91c.lock");
    std::fs::create_dir(&lock).unwrap();
    assert_eq!(store.claim(A, "mv66me0m3fa91c"), Claim::Ours);
    assert_eq!(create_conversation_store(dir.path()).claim(A, "mv66me0m3fa91c"), Claim::Ours);
    let without = create_conversation_store(dir.path());
    without.without_file_locks();
    assert_eq!(without.claim(A, "mv66me0m3fa91c"), Claim::Ours);
    // Saved, found, let go of past its days, and deleted all the same.
    store.save(A, "mv66me0m3fa91c", &said("and brighter", 20)).await.unwrap();
    assert!(matches!(store.find("mv66me0m3fa91c").await.unwrap(), ConversationMatch::One(_)));
    store.release(A, "mv66me0m3fa91c");
    assert!(store.delete(A, "mv66me0m3fa91c").await.unwrap());
    assert!(store.load(A, "mv66me0m3fa91c").await.unwrap().is_none());
    store.save(A, "mv66me0m3fa91c", &said("again", 30)).await.unwrap();
    for n in 0..21 {
        store.save(A, &format!("new{n:03}aaaaaa"), &said("newer", 40 + n)).await.unwrap();
    }
    age(&dir.path().join(A).join("conversations/mv66me0m3fa91c.json"), 200 * DAY);
    store.tidy(A).await.unwrap();
    assert!(store.load(A, "mv66me0m3fa91c").await.unwrap().is_none());
}

#[test]
fn of_kumis_claiming_a_conversation_at_once_one_gets_it() {
    let dir = tempfile::tempdir().unwrap();
    for round in 0..20 {
        let id = format!("race{round:02}aaaaaa");
        let start = std::sync::Arc::new(std::sync::Barrier::new(4));
        let done = std::sync::Arc::new(std::sync::Barrier::new(4));
        let claims: Vec<_> = (0..4)
            .map(|_| {
                let (path, id, start, done) = (dir.path().to_path_buf(), id.clone(), start.clone(), done.clone());
                std::thread::spawn(move || {
                    let store = create_conversation_store(path);
                    start.wait();
                    let claim = store.claim(A, &id);
                    // Each keeps its hold until all have asked.
                    done.wait();
                    claim
                })
            })
            .collect();
        let claims: Vec<Claim> = claims.into_iter().map(|thread| thread.join().unwrap()).collect();
        assert_eq!(claims.iter().filter(|claim| **claim == Claim::Ours).count(), 1, "round {round}: {claims:?}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_days_settings_said_to_keep_outlast_an_earlier_kumi_dropping_them() {
    let dir = tempfile::tempdir().unwrap();
    let kept = dir.path().join(A).join("conversations");
    // The producer keeps everything; 25 conversations, 5 of them past the 20 newest and 200 days old.
    let store = create_conversation_store_keeping(dir.path(), Some(0));
    for n in 0..25 {
        store.save(A, &format!("old{n:03}aaaaaa"), &said("older", 10 + n)).await.unwrap();
    }
    for n in 0..5 {
        age(&kept.join(format!("old{n:03}aaaaaa.json")), 200 * DAY);
    }
    let index: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("conversation-index.json")).unwrap()).unwrap();
    assert_eq!(index["keepDays"], 0);
    // A rollback's Kumi rewrote settings.json without the key: the next Kumi keeps them all the same.
    let store = create_conversation_store(dir.path());
    assert_eq!(store.days_kept().await, 0);
    store.save(A, "new000aaaaaaaa", &said("newer", 100)).await.unwrap();
    assert_eq!(kumi_1_16_3::names(&kept).len(), 26);
    // Whichever keeps more: a shorter time said before gives way to 90 days, a longer one stays.
    for (said_last, now) in [(30, 90), (120, 120), (90, 90)] {
        create_conversation_store_keeping(dir.path(), Some(said_last)).tidy(A).await.unwrap();
        assert_eq!(create_conversation_store(dir.path()).days_kept().await, now, "{said_last}");
    }
    // What settings say now is what counts.
    assert_eq!(create_conversation_store_keeping(dir.path(), Some(7)).days_kept().await, 7);
}

#[tokio::test(flavor = "current_thread")]
async fn a_name_without_its_conversation_waits_a_while_then_goes_or_follows_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    store.save(A, "mv66me0m3fa91c", &said("make the bass wider", 10)).await.unwrap();
    let kept = dir.path().join(A).join("conversations");
    // A copy's name and source, written by another Kumi before the copy (5 minutes, for a slow one): they stay.
    std::fs::write(kept.join("mv66me0m9copy1.meta"), r#"{"version":1,"name":"Wide bass","forkedFrom":"mv66me0m3fa91c"}"#).unwrap();
    age(&kept.join("mv66me0m9copy1.meta"), Duration::from_secs(5 * 60));
    store.tidy(A).await.unwrap();
    assert!(kept.join("mv66me0m9copy1.meta").exists());
    // Left without it for a while (an earlier Kumi let the conversation go): they go.
    age(&kept.join("mv66me0m9copy1.meta"), Duration::from_secs(3600));
    store.tidy(A).await.unwrap();
    assert!(!kept.join("mv66me0m9copy1.meta").exists());
    // An earlier Kumi moved an unsaved Set's conversation to the Set's place once it was saved, and left its name
    // behind: the name follows it.
    let unsaved = dir.path().join("unsaved").join("conversations");
    store.save("unsaved", "mv66me0m0moved", &said("a new song", 20)).await.unwrap();
    assert!(store.rename("unsaved", "mv66me0m0moved", Some("Night drive")).await.unwrap());
    std::fs::rename(unsaved.join("mv66me0m0moved.json"), kept.join("mv66me0m0moved.json")).unwrap();
    age(&unsaved.join("mv66me0m0moved.meta"), Duration::from_secs(3600));
    store.tidy("unsaved").await.unwrap();
    assert!(!unsaved.join("mv66me0m0moved.meta").exists());
    let ConversationMatch::One(row) = store.find("night drive").await.unwrap() else { panic!("not found by its name") };
    assert_eq!((row.place.as_str(), row.id.as_str()), (A, "mv66me0m0moved"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_rollback_to_kumi_1_16_and_forward_again_keeps_what_each_kept() {
    const C: &str = "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a";
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path();
    let id = |n: usize| format!("old{n:03}aaaaaa");
    let kept_in = |place: &str| projects.join(place).join("conversations");
    // This Kumi keeps 26 conversations in A, named, pinned and forked ones among them, a copy in B, and a named, pinned
    // one for a Set not saved yet. Another window has one open; a Kumi that crashed had another.
    let store = create_conversation_store(projects);
    for n in 0..25 {
        store.save(A, &id(n), &said(&format!("request {n}"), 10 + n as i64)).await.unwrap();
    }
    store.create(A, "fork00aaaaaa", &said("forked", 40), Some("Alt take"), Some(&id(24))).await.unwrap();
    store.create(B, "copy00aaaaaa", &said("copied", 41), None, Some(&id(23))).await.unwrap();
    store.save("unsaved", "draft0aaaaaa", &said("a new song", 42)).await.unwrap();
    assert!(store.rename("unsaved", "draft0aaaaaa", Some("Night drive")).await.unwrap());
    assert!(store.pin("unsaved", "draft0aaaaaa", true).await.unwrap());
    assert!(store.rename(A, &id(24), Some("Newest named")).await.unwrap());
    assert!(store.rename(A, &id(0), Some("Oldest named")).await.unwrap());
    assert!(store.pin(A, &id(1), true).await.unwrap());
    // Each its own time, so both Kumis order them alike.
    for n in 0..25 {
        age(&kept_in(A).join(format!("{}.json", id(n))), Duration::from_secs(60 * (30 - n as u64)));
    }
    age(&kept_in(A).join("fork00aaaaaa.json"), Duration::from_secs(30));
    let window = create_conversation_store(projects);
    assert_eq!(window.claim(A, &id(20)), Claim::Ours);
    std::fs::write(kept_in(A).join(format!("{}.lock", id(21))), json!({"pid":u32::MAX - 1,"started":1}).to_string()).unwrap();
    assert_eq!(store.all().await.unwrap().len(), 28);
    drop(store);

    // Rolled back, Kumi 1.16 saves a new conversation in A (its trim keeps 20), saves the named one again with only the
    // fields it knows, and moves the unsaved Set's conversation to the Set's place once it's saved.
    let a = projects.join(A);
    kumi_1_16_3::save(&a, "back16aaaaaa", &serde_json::to_value(said("from 1.16", 50)).unwrap());
    let mut newest = kumi_1_16_3::read(&kept_in(A).join(format!("{}.json", id(24)))).unwrap();
    newest["checkpoint"]["messages"]
        .as_array_mut()
        .unwrap()
        .extend([json!({"role":"user","content":"and wider"}), json!({"role":"assistant","content":"done"})]);
    newest["turns"] = json!(2);
    kumi_1_16_3::save(&a, &id(24), &newest);
    kumi_1_16_3::move_conversation(projects, "draft0aaaaaa", "unsaved", C);
    let gone: Vec<String> = (0..7).map(id).collect();
    for id in &gone {
        assert!(!kept_in(A).join(format!("{id}.json")).exists(), "1.16 kept {id}");
    }
    // It reads every conversation file, and its report takes none of this Kumi's files.
    for place in [A, B, C, "unsaved"] {
        for name in kumi_1_16_3::names(&kept_in(place)) {
            assert!(kumi_1_16_3::read(&kept_in(place).join(&name)).is_some(), "{name}");
        }
    }
    for file in kumi_1_16_3::report_candidates(projects) {
        assert!(kumi_1_16_3::read(&file).is_some(), "{}", file.display());
    }

    // Forward again: every file reads, what 1.16 let go says so, and names, pins, sources and holds are as they were.
    let again = create_conversation_store(projects);
    for place in [A, B, C] {
        for name in kumi_1_16_3::names(&kept_in(place)) {
            assert!(again.load(place, &name[..name.len() - 5]).await.unwrap().is_some(), "{name}");
        }
    }
    let rows = again.all().await.unwrap();
    assert_eq!(rows.len(), 22, "{:?}", rows.iter().map(|r| &r.id).collect::<Vec<_>>());
    for id in &gone {
        assert!(matches!(again.find(id).await.unwrap(), ConversationMatch::Gone), "{id}");
    }
    let ConversationMatch::One(named) = again.find("newest named").await.unwrap() else { panic!("the name went") };
    assert_eq!((named.id.as_str(), named.turns), (id(24).as_str(), 2));
    let row = |id: &str| rows.iter().find(|r| r.id == id).unwrap().clone();
    assert_eq!((row("fork00aaaaaa").name.as_deref(), row("fork00aaaaaa").forked_from), (Some("Alt take"), Some(id(24))));
    assert_eq!(row("copy00aaaaaa").forked_from, Some(id(23)));
    assert_eq!(again.claim(A, &id(20)), Claim::Taken);
    assert_eq!(again.claim(A, &id(21)), Claim::Ours);
    // The moved one's name and pin follow it, and the names of those 1.16 let go go, once they've been left a while.
    for file in [
        kept_in("unsaved").join("draft0aaaaaa.meta"),
        kept_in(A).join(format!("{}.meta", id(0))),
        kept_in(A).join(format!("{}.meta", id(1))),
    ] {
        age(&file, Duration::from_secs(3600));
    }
    again.tidy("unsaved").await.unwrap();
    again.tidy(A).await.unwrap();
    let ConversationMatch::One(moved) = again.find("night drive").await.unwrap() else { panic!("the moved one's name went") };
    assert_eq!((moved.place.as_str(), moved.id.as_str(), moved.pinned), (C, "draft0aaaaaa", true));
    for gone in &gone[..2] {
        assert!(!kept_in(A).join(format!("{gone}.meta")).exists(), "{gone}");
    }
}

/// A tiny generator, so each run makes the same cases.
struct Random(u64);
impl Random {
    fn next(&mut self, below: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % below.max(1)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn every_conversation_kumi_1_16_3_kept_is_still_kept() {
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let mut let_go = (0, 0);
    for round in 0..120 {
        let dir = tempfile::tempdir().unwrap();
        let keep_days = [0, 1, 30, 90][random.next(4) as usize];
        let count = random.next(45) as usize;
        // A few shared times make ties, which both must order alike.
        let times: Vec<Duration> = (0..1 + random.next(count as u64 + 1))
            .map(|_| DAY * random.next(200) as u32 + Duration::from_secs(random.next(5000)))
            .collect();
        let ids: Vec<String> = (0..count).map(|n| format!("c{:03}{:08x}", n, random.next(u32::MAX as u64))).collect();
        let mut setup = vec![];
        for id in &ids {
            setup.push((id.clone(), times[random.next(times.len() as u64) as usize], random.next(5) == 0));
        }
        let current = match random.next(3) {
            0 if !ids.is_empty() => Some(ids[random.next(ids.len() as u64) as usize].clone()),
            1 => Some("gone00000000".to_string()),
            _ => None,
        };
        let make = |root: &Path| {
            let place = root.join(A);
            let kept = place.join("conversations");
            std::fs::create_dir_all(&kept).unwrap();
            for (id, ago, pinned) in &setup {
                let file = kept.join(format!("{id}.json"));
                std::fs::write(&file, serde_json::to_string(&said("words", 1)).unwrap()).unwrap();
                age(&file, *ago);
                if *pinned {
                    std::fs::write(kept.join(format!("{id}.meta")), r#"{"version":1,"pinned":true}"#).unwrap();
                }
            }
            if let Some(current) = &current {
                std::fs::write(place.join("current"), current).unwrap();
            }
            kept
        };
        let before = make(&dir.path().join("before"));
        let now = make(&dir.path().join("now"));
        kumi_1_16_3::trim(&before.parent().unwrap().to_path_buf());
        create_conversation_store_keeping(dir.path().join("now"), Some(keep_days)).tidy(A).await.unwrap();
        let old: BTreeSet<String> = kumi_1_16_3::names(&before).into_iter().collect();
        let new: BTreeSet<String> = kumi_1_16_3::names(&now).into_iter().collect();
        assert!(new.is_superset(&old), "round {round}, keeping {keep_days} days: lost {:?}", old.difference(&new).collect::<Vec<_>>());
        let_go.0 += setup.len() - old.len();
        let_go.1 += setup.len() - new.len();
        // And it lets go only what's old enough, never what's pinned.
        for (id, ago, pinned) in &setup {
            if !new.contains(&format!("{id}.json")) {
                assert!(!pinned && keep_days != 0 && *ago >= DAY * keep_days, "round {round}: {id} went");
            }
        }
    }
    // Both let some go: the cases reach past the 20 newest.
    assert!(let_go.0 > let_go.1 && let_go.1 > 0, "{let_go:?}");
}

#[test]
fn a_title_is_the_name_else_the_first_request_on_one_line_with_keys_hidden() {
    assert_eq!(title(Some("  Drum Bus "), "tighten the drums"), "Drum Bus");
    assert_eq!(title(None, "tighten\nthe   drums"), "tighten the drums");
    assert_eq!(title(Some(" "), "use key sk1234567890abcdefghij1234567890abcd now"), "use key [hidden] now");
    let long = title(None, &"word ".repeat(30));
    assert!(long.ends_with('…') && long.chars().count() <= 60, "{long}");
    assert_eq!(title(None, ""), "");
}

#[tokio::test(flavor = "current_thread")]
async fn what_a_later_kumi_adds_to_the_index_or_beside_a_conversation_is_read_past() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    store.save(A, "mv66me0m3fa91c", &said("make the bass wider", 10)).await.unwrap();
    store.all().await.unwrap();
    // The index is versioned, and a field more at any level (a DAW, say) is the same version.
    let file = dir.path().join("conversation-index.json");
    let mut index: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(index["version"], 1);
    let key = format!("{A}/mv66me0m3fa91c");
    index["daw"] = json!("Live");
    index["conversations"][&key]["daw"] = json!("Live");
    index["places"][A]["noun"] = json!("Set");
    // What the index says of a file that hasn't changed is taken as it is: here, a first request only it holds.
    index["conversations"][&key]["first"] = json!("as the index has it");
    std::fs::write(&file, index.to_string()).unwrap();
    let rows = store.all().await.unwrap();
    assert_eq!(rows[0].first, "as the index has it", "the index was read, not made again");
    // A sidecar with a field more keeps what's known in it, and what isn't when it's written again.
    let meta = dir.path().join(A).join("conversations/mv66me0m3fa91c.meta");
    std::fs::write(&meta, r#"{"version":1,"name":"Bass ideas","daw":"Live"}"#).unwrap();
    assert_eq!(store.all().await.unwrap()[0].name.as_deref(), Some("Bass ideas"));
    assert!(store.pin(A, "mv66me0m3fa91c", true).await.unwrap());
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&meta).unwrap()).unwrap();
    assert_eq!((&written["daw"], &written["pinned"], &written["name"]), (&json!("Live"), &json!(true), &json!("Bass ideas")));
    assert!(store.rename(A, "mv66me0m3fa91c", None).await.unwrap() && store.pin(A, "mv66me0m3fa91c", false).await.unwrap());
    assert!(meta.exists(), "what a later Kumi keeps here stays");
    // An index a later Kumi wrote in a version this one doesn't know is read past, and never written over.
    let later = json!({"version":2,"conversations":[],"keepDays":0}).to_string();
    std::fs::write(&file, &later).unwrap();
    store.save(A, "mv66me0m9newer", &said("and brighter", 20)).await.unwrap();
    assert_eq!(store.all().await.unwrap().len(), 2);
    assert!(matches!(store.find("mv66me0m9newer").await.unwrap(), ConversationMatch::One(_)));
    assert!(store.delete(A, "mv66me0m9newer").await.unwrap());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), later);
    assert_eq!(store.days_kept().await, 0, "what it says settings said is taken");
}

#[tokio::test(flavor = "current_thread")]
async fn a_conversation_from_a_file_and_every_copy_of_it_say_so() {
    use kumi_types::place::IMPORTED;
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    store.create(IMPORTED, "imported0001", &said("from a file", 10), Some("Theirs (imported)"), None).await.unwrap();
    store.create(A, "copied000001", &said("from a file", 11), None, Some("imported0001")).await.unwrap();
    store.create(B, "copied000002", &said("from a file", 12), None, Some("copied000001")).await.unwrap();
    store.create(A, "mine00000001", &said("my own", 13), None, None).await.unwrap();
    store.create(B, "mine00000002", &said("my own", 14), None, Some("mine00000001")).await.unwrap();
    let rows = store.all().await.unwrap();
    let imported = |id: &str| rows.iter().find(|row| row.id == id).unwrap().imported;
    assert!(imported("imported0001") && imported("copied000001") && imported("copied000002"));
    assert!(!imported("mine00000001") && !imported("mine00000002"));
    // A fresh look at the folders finds the same.
    std::fs::remove_file(dir.path().join("conversation-index.json")).ok();
    let again = create_conversation_store(dir.path()).all().await.unwrap();
    assert!(again.iter().find(|row| row.id == "copied000002").unwrap().imported);
}

#[tokio::test(flavor = "current_thread")]
async fn a_conversation_from_a_file_keeps_its_set_and_an_earlier_kumi_writing_the_index_back_loses_nothing() {
    use kumi_types::place::IMPORTED;
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    store.create(IMPORTED, "imported0001", &said("from a file", 10), None, None).await.unwrap();
    assert!(store.note_set(IMPORTED, "imported0001", "Their Song\nand more").await.unwrap());
    assert!(!store.note_set(IMPORTED, "nothere00001", "Their Song").await.unwrap());
    store.create(A, "copied000001", &said("from a file", 11), None, Some("imported0001")).await.unwrap();
    let row = |rows: &[ConversationSummary], id: &str| rows.iter().find(|row| row.id == id).unwrap().clone();
    let rows = store.all().await.unwrap();
    assert_eq!(row(&rows, "imported0001").set.as_deref(), Some("Their Song and more"));
    assert!(row(&rows, "copied000001").imported);
    // Written back by a Kumi that knew neither: the entries are read again.
    let file = dir.path().join("conversation-index.json");
    let mut index: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    for entry in index["conversations"].as_object_mut().unwrap().values_mut() {
        let entry = entry.as_object_mut().unwrap();
        entry.remove("imported");
        entry.remove("set");
        entry["stamp"].as_object_mut().unwrap().remove("reads");
    }
    std::fs::write(&file, index.to_string()).unwrap();
    let rows = create_conversation_store(dir.path()).all().await.unwrap();
    assert_eq!(row(&rows, "imported0001").set.as_deref(), Some("Their Song and more"));
    assert!(row(&rows, "copied000001").imported, "a copy of an import stays one");
}

#[test]
fn a_title_draws_what_it_says() {
    assert_eq!(title(None, "\u{202E}gnol \u{200B}kick\u{E0041}"), "gnol kick");
}

/// Kumi 1.24's reading of a place's folder, where it goes past 1.16.3's: what it takes for a conversation's file, and for
/// its name and pin beside it. Nothing else in the folder is anything to it.
mod kumi_1_24 {
    /// Its list, index, search, trim and report take `<id>.json` (`strip_suffix(".json")`), as 1.16.3's did.
    pub fn conversation_file(name: &str) -> Option<&str> {
        name.strip_suffix(".json").filter(|id| super::kumi_1_16_3::conversation(id))
    }
    pub fn meta_file(name: &str) -> Option<&str> {
        name.strip_suffix(".meta").filter(|id| super::kumi_1_16_3::conversation(id))
    }
}

/// A log beside a conversation, as this Kumi writes one: a header, then a request.
fn log_beside(kept: &Path, id: &str, words: &str) {
    let header = json!({"kind":"header","v":1,"id":id,"at":1,"kumi":"1.25.0","before":0});
    let request = json!({"kind":"request","at":2,"text":words});
    std::fs::create_dir_all(kept).unwrap();
    std::fs::write(kept.join(log_name(id)), format!("{header}\n{request}\n")).unwrap();
}

/// Kumi 1.24 lists conversations with `strip_suffix(".json")`: a conversation's log beside it is never one to it, so a
/// rollback lists, reads, keeps and reports what it did before there were logs, and forward again nothing's lost.
#[tokio::test(flavor = "current_thread")]
async fn a_conversations_log_is_never_a_conversation_to_kumi_1_24() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    let kept = dir.path().join(A).join("conversations");
    for n in 0..25 {
        let id = format!("old{n:03}aaaaaa");
        store.save(A, &id, &said(&format!("request {n}"), 10 + n)).await.unwrap();
        log_beside(&kept, &id, &format!("zebra {n}"));
        age(&kept.join(format!("{id}.json")), Duration::from_secs(60 * (30 - n as u64)));
    }
    for id in ["old000aaaaaa", "a", "mv66me0m3fa91c", "zzzzzzzzzzzzzzzzzzzzzzzz"] {
        assert_eq!((kumi_1_24::conversation_file(&log_name(id)), kumi_1_24::meta_file(&log_name(id))), (None, None), "{id}");
    }
    // Its listing, and what its report took: only the conversations themselves, each read as one.
    let listed = kumi_1_16_3::names(&kept);
    assert_eq!(listed.len(), 25);
    assert!(listed.iter().all(|name| name.ends_with(".json")), "{listed:?}");
    for file in kumi_1_16_3::report_candidates(dir.path()) {
        assert!(kumi_1_16_3::read(&file).is_some(), "{}", file.display());
    }
    // Its trim keeps the 20 newest by those names alone, and lets the others go; their logs it doesn't see.
    kumi_1_16_3::trim(&dir.path().join(A));
    assert_eq!(kumi_1_16_3::names(&kept).len(), 20);
    assert!(kept.join(log_name("old000aaaaaa")).exists());
    // Forward again, this Kumi lists, indexes and searches as before: a log's words aren't a conversation's.
    let again = create_conversation_store(dir.path());
    assert_eq!(again.list(A).await.unwrap().len(), 20);
    assert!(again.search(&["zebra".into()], 1, 10, None).await.unwrap().is_empty());
    // And the logs of those 1.24 let go go too, once they've been left a while.
    for n in 0..25 {
        age(&kept.join(log_name(&format!("old{n:03}aaaaaa"))), Duration::from_secs(3600));
    }
    again.tidy(A).await.unwrap();
    for n in 0..25 {
        let log = kept.join(log_name(&format!("old{n:03}aaaaaa")));
        assert_eq!(log.exists(), n >= 5, "old{n:03}aaaaaa");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_log_goes_with_its_conversation_and_one_left_behind_follows_it_or_goes() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    let kept = dir.path().join(A).join("conversations");
    // Deleted, a conversation's log goes with it.
    store.save(A, "mv66me0m0gone1", &said("one", 1)).await.unwrap();
    log_beside(&kept, "mv66me0m0gone1", "one");
    assert!(store.delete(A, "mv66me0m0gone1").await.unwrap());
    assert!(!kept.join(log_name("mv66me0m0gone1")).exists());
    // Let go past the newest and the days kept, its log goes with it.
    let keeping = create_conversation_store_keeping(dir.path(), Some(1));
    for n in 0..21 {
        let id = format!("kept{n:02}aaaaaa");
        keeping.save(A, &id, &said("words", 1)).await.unwrap();
        log_beside(&kept, &id, "words");
    }
    age(&kept.join("kept00aaaaaa.json"), DAY * 2);
    keeping.tidy(A).await.unwrap();
    assert!(!kept.join("kept00aaaaaa.json").exists() && !kept.join(log_name("kept00aaaaaa")).exists());
    assert!(kept.join(log_name("kept01aaaaaa")).exists());
    // Left without its conversation (an earlier Kumi let it go): it stays a while, then goes.
    log_beside(&kept, "mv66me0m0left1", "left");
    store.tidy(A).await.unwrap();
    assert!(kept.join(log_name("mv66me0m0left1")).exists());
    age(&kept.join(log_name("mv66me0m0left1")), Duration::from_secs(3600));
    store.tidy(A).await.unwrap();
    assert!(!kept.join(log_name("mv66me0m0left1")).exists());
    // One a Kumi has open stays, however long it's been: its conversation is on its way to it.
    log_beside(&kept, "mv66me0m0held1", "held");
    age(&kept.join(log_name("mv66me0m0held1")), Duration::from_secs(3600));
    let window = create_conversation_store(dir.path());
    assert_eq!(window.claim(A, "mv66me0m0held1"), Claim::Ours);
    store.tidy(A).await.unwrap();
    window.tidy(A).await.unwrap();
    assert!(kept.join(log_name("mv66me0m0held1")).exists());
    window.release(A, "mv66me0m0held1");
    store.tidy(A).await.unwrap();
    assert!(!kept.join(log_name("mv66me0m0held1")).exists());
    // An earlier Kumi moved an unsaved Set's conversation to the Set's place once it was saved, and left its log behind:
    // the log follows it.
    let unsaved = dir.path().join("unsaved").join("conversations");
    store.save("unsaved", "mv66me0m0moved", &said("a new song", 20)).await.unwrap();
    log_beside(&unsaved, "mv66me0m0moved", "a new song");
    std::fs::rename(unsaved.join("mv66me0m0moved.json"), kept.join("mv66me0m0moved.json")).unwrap();
    age(&unsaved.join(log_name("mv66me0m0moved")), Duration::from_secs(3600));
    store.tidy("unsaved").await.unwrap();
    assert!(!unsaved.join(log_name("mv66me0m0moved")).exists());
    assert!(store.log(A, "mv66me0m0moved").await.is_some());
}
