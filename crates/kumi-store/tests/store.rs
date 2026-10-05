use kumi_store::{gaps, notes, params, Connection, Scope, Store, StoreError, SCHEMA_VERSION};
use std::{
    path::Path,
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};

fn gap(missing: &str, at: i64) -> gaps::Gap {
    gaps::Gap { kumi_version: "test".into(), missing: missing.into(), asked: None, workaround: None, at }
}
fn count(store: &Store, sql: &'static str) -> i64 {
    store.read(|c| Ok(c.query_row(sql, [], |row| row.get(0))?)).unwrap()
}

#[test]
fn opening_makes_the_database_in_wal_and_reopening_finds_it_current() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("new folder").join("kumi.db");
    let store = Store::open(&path).unwrap();
    assert_eq!(count(&store, "PRAGMA user_version") as usize, SCHEMA_VERSION);
    assert_eq!(store.read(|c| Ok(c.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?)).unwrap(), "wal");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "only the producer reads it");
        assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
    store.write_wait(|c| gaps::add(c, &gap("a voice count", 1))).unwrap();
    drop(store);
    let again = Store::open(&path).unwrap();
    assert_eq!(gaps::all(&Connection::open(&path).unwrap()).unwrap().len(), 1);
    assert_eq!(count(&again, "SELECT count(*) FROM gaps"), 1);
}

#[test]
fn a_database_from_a_newer_kumi_is_left_alone() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("kumi.db");
    drop(Store::open(&path).unwrap());
    Connection::open(&path).unwrap().pragma_update(None, "user_version", SCHEMA_VERSION as i64 + 1).unwrap();
    assert_eq!(Store::open(&path).err(), Some(StoreError::Newer { found: SCHEMA_VERSION + 1, known: SCHEMA_VERSION }));
}

#[test]
fn an_unwritable_place_is_an_error_to_fall_back_from() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("a file"), "not a folder").unwrap();
    assert!(matches!(Store::open(folder.path().join("a file").join("kumi.db")), Err(StoreError::Io(_))));
}

#[test]
fn queued_writes_commit_together_and_a_failing_one_rolls_back_alone() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    let (sender, results) = std::sync::mpsc::channel();
    for (index, missing) in ["first", "fails", "third", "panics"].into_iter().enumerate() {
        let sender = sender.clone();
        store.write(
            move |c| {
                gaps::add(c, &gap(missing, index as i64))?;
                match missing {
                    "fails" => Err(StoreError::Sqlite("a write that fails after its first insert".into())),
                    "panics" => panic!("a write that panics after its first insert"),
                    _ => Ok(index),
                }
            },
            move |result| sender.send((index, result)).unwrap(),
        );
    }
    let mut answered: Vec<_> = (0..4).map(|_| results.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
    answered.sort_by_key(|(index, _)| *index);
    assert_eq!(answered[0].1, Ok(0));
    assert!(answered[1].1.is_err() && answered[3].1.is_err());
    assert_eq!(answered[2].1, Ok(2));
    let kept: Vec<String> = gaps::all(&Connection::open(store.path()).unwrap()).unwrap().into_iter().map(|g| g.missing).collect();
    assert_eq!(kept, ["first", "third"], "a failed or panicked write leaves nothing behind, and the others commit");
    store.write_wait(|c| gaps::add(c, &gap("after", 9))).unwrap();
    assert_eq!(count(&store, "SELECT count(*) FROM gaps"), 3, "the writer carries on");
}

#[test]
fn two_kumis_writing_to_one_database_at_once_lose_nothing() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("kumi.db");
    let stores = [Store::open(&path).unwrap(), Store::open(&path).unwrap()];
    let start = Arc::new(Barrier::new(2));
    let threads: Vec<_> = stores
        .iter()
        .enumerate()
        .map(|(which, store)| {
            let (store, start) = (store.clone(), start.clone());
            std::thread::spawn(move || {
                start.wait();
                for n in 0..300 {
                    store.write_wait(move |c| gaps::add(c, &gap(&format!("{which}-{n}"), n))).unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(count(&stores[0], "SELECT count(*) FROM gaps"), 600);
    assert_eq!(count(&stores[1], "SELECT count(DISTINCT missing) FROM gaps"), 600);
}

#[test]
fn reads_run_beside_writes_and_see_each_commit() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    let scope = Scope::Project("0123456789abcdef0123456789abcdef".into());
    for n in 0..24 {
        let (writing, at) = (scope.clone(), n);
        store
            .write_wait(move |c| {
                let mut kept = notes::in_use(c, &writing)?;
                kept.push(notes::Note { label: format!("s{}", at + 1), text: format!("note {at}"), pinned: false, at });
                notes::keep(c, &writing, &kept, at)
            })
            .unwrap();
        assert_eq!(store.read(|c| notes::in_use(c, &scope)).unwrap().len(), n as usize + 1);
    }
}

/// Run as a child by `a_crash_mid_write_keeps_whole_transactions_only`: write until killed.
#[test]
fn crash_child() {
    let Ok(path) = std::env::var("KUMI_STORE_CRASH_CHILD") else { return };
    let store = Store::open(&path).unwrap();
    for batch in 0.. {
        store.write(
            move |c| {
                for n in 0..10 {
                    c.execute(
                        "INSERT INTO gaps (id, kumi_version, missing, created_at) VALUES (?1, 'crash', ?2, ?3)",
                        params![format!("{batch}-{n}"), format!("batch {batch}"), n],
                    )?;
                }
                Ok(())
            },
            |_| {},
        );
        if batch % 50 == 0 {
            store.write_wait(|_| Ok(())).unwrap();
        }
    }
}

#[test]
fn a_crash_mid_write_keeps_whole_transactions_only() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("kumi.db");
    drop(Store::open(&path).unwrap());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_child", "--nocapture", "--test-threads=1"])
        .env("KUMI_STORE_CRASH_CHILD", &path)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while gaps_written(&path) < 2000 && started.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let written = gaps_written(&path);
    assert!(written >= 2000, "the child wrote {written} rows before it was killed");
    let store = Store::open(&path).unwrap();
    assert_eq!(store.read(|c| Ok(c.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?)).unwrap(), "ok");
    let partial = count(&store, "SELECT count(*) FROM (SELECT missing FROM gaps GROUP BY missing HAVING count(*) != 10)");
    assert_eq!(partial, 0, "every write is all there or not at all");
}

fn gaps_written(path: &Path) -> i64 {
    Connection::open(path).and_then(|c| c.query_row("SELECT count(*) FROM gaps", [], |row| row.get(0))).unwrap_or(0)
}
