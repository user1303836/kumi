use kumi_store::{gaps, notes, params, read_only, Connection, Scope, Store, StoreError, SCHEMA_VERSION};
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
fn a_read_only_look_writes_nothing_and_leaves_a_newer_database_alone() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("kumi.db");
    let store = Store::open(&path).unwrap();
    store.write_wait(|c| gaps::add(c, &gap("freezing a track", 1))).unwrap();
    drop(store);
    assert_eq!(read_only(&path, gaps::all).unwrap().len(), 1);
    assert!(read_only(&path, |c| Ok(c.execute("DELETE FROM gaps", [])?)).is_err(), "it can't write");
    assert_eq!(read_only(&path, gaps::all).unwrap().len(), 1);
    Connection::open(&path).unwrap().pragma_update(None, "user_version", SCHEMA_VERSION as i64 + 1).unwrap();
    assert_eq!(read_only(&path, gaps::all).err(), Some(StoreError::Newer { found: SCHEMA_VERSION + 1, known: SCHEMA_VERSION }));
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

type Job = Box<dyn FnOnce(&Connection) -> Result<(), StoreError> + Send>;

/// Queue `jobs` so the writer takes them as one batch: they go in while the writer is busy with a write
/// that waits until they're all queued. Each job's answer, in order. The writer gathers a batch as it
/// takes its first write, so they're queued only once that write runs: queued while the writer was
/// still gathering, some would join its batch and the rest form another.
fn one_batch(store: &Store, jobs: Vec<Job>) -> Vec<Result<(), StoreError>> {
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let (running, started) = std::sync::mpsc::channel::<()>();
    store.write(
        move |_| {
            running.send(()).ok();
            wait.recv().ok();
            Ok(())
        },
        |_| {},
    );
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    let (sender, answers) = std::sync::mpsc::channel();
    let count = jobs.len();
    for (index, job) in jobs.into_iter().enumerate() {
        let sender = sender.clone();
        store.write(job, move |result| sender.send((index, result)).unwrap());
    }
    release.send(()).unwrap();
    let mut answered: Vec<_> = (0..count).map(|_| answers.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
    answered.sort_by_key(|(index, _)| *index);
    answered.into_iter().map(|(_, result)| result).collect()
}
fn insert(missing: &'static str) -> Job {
    Box::new(move |c| gaps::add(c, &gap(missing, 1)))
}

#[test]
fn when_sqlite_gives_up_a_batch_no_write_of_it_is_kept_and_every_caller_hears_so() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    // The transaction ends mid-batch, as SQLite ends it on a full disk or an I/O error.
    let answers = one_batch(
        &store,
        vec![
            insert("before"),
            Box::new(|c| {
                c.execute_batch("ROLLBACK")?;
                Ok(())
            }),
            insert("after"),
        ],
    );
    assert!(answers.iter().all(Result::is_err), "{answers:?}");
    // Each hears what happened, not that its savepoint went with the transaction.
    assert!(answers.iter().all(|answer| !format!("{answer:?}").contains("savepoint")), "{answers:?}");
    assert_eq!(count(&store, "SELECT count(*) FROM gaps"), 0, "nothing landed outside the transaction");
    store.write_wait(|c| gaps::add(c, &gap("next", 2))).unwrap();
    assert_eq!(count(&store, "SELECT count(*) FROM gaps"), 1, "the writer carries on");
}

#[test]
fn on_a_full_disk_a_caller_hears_ok_exactly_when_its_write_is_kept() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    store
        .write_wait(|c| {
            let pages: i64 = c.query_row("PRAGMA page_count", [], |row| row.get(0))?;
            c.query_row(&format!("PRAGMA max_page_count = {}", pages + 2), [], |_| Ok(()))?;
            Ok(())
        })
        .unwrap();
    let big = "x".repeat(64 * 1024);
    let answers = one_batch(&store, vec![insert("small before"), Box::new(move |c| gaps::add(c, &gap(&big, 1))), insert("small after")]);
    assert!(answers[1].is_err(), "the write that doesn't fit fails: {answers:?}");
    for (index, missing) in [(0, "small before"), (2, "small after")] {
        let kept = store
            .read(move |c| Ok(c.query_row("SELECT count(*) FROM gaps WHERE missing = ?1", [missing], |row| row.get::<_, i64>(0))?))
            .unwrap();
        assert_eq!(answers[index].is_ok(), kept == 1, "{missing}: told {:?}, kept {kept}", answers[index]);
    }
}

#[test]
fn a_store_dropped_by_its_own_last_write_ends_without_waiting_on_itself() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    let held = store.clone();
    let (sender, done) = std::sync::mpsc::channel();
    drop(store);
    held.clone().write(
        move |c| {
            drop(held);
            gaps::add(c, &gap("last", 1))
        },
        move |result| sender.send(result).unwrap(),
    );
    assert_eq!(done.recv_timeout(Duration::from_secs(10)).unwrap(), Ok(()));
}
