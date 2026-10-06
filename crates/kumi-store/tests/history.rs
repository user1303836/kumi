use kumi_store::{
    history::{self, Object, Op, HISTORY_SCHEMA_VERSION},
    Store, StoreError,
};
use serde_json::json;

fn count(store: &Store, sql: &'static str) -> i64 {
    store.read(|c| Ok(c.query_row(sql, [], |row| row.get(0))?)).unwrap()
}

#[test]
fn a_sets_history_opens_in_its_own_file_with_its_own_schema() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("projects").join("0123456789abcdef0123456789abcdef").join("history.db");
    let store = Store::open_history(&path).unwrap();
    assert_eq!(count(&store, "PRAGMA user_version") as usize, HISTORY_SCHEMA_VERSION);
    assert_eq!(store.read(|c| Ok(c.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?)).unwrap(), "wal");
    for table in ["objects", "ops", "snapshots", "refs"] {
        let found: i64 = store
            .read(move |c| {
                Ok(c.query_row("SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1", [table], |row| row.get(0))?)
            })
            .unwrap();
        assert_eq!(found, 1, "{table}");
    }
    // kumi.db's tables aren't in it.
    assert_eq!(count(&store, "SELECT count(*) FROM sqlite_schema WHERE name = 'notes'"), 0);
    drop(store);
    // A history from a newer Kumi is left as it is.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    assert_eq!(Store::open_history(&path).err(), Some(StoreError::Newer { found: 99, known: HISTORY_SCHEMA_VERSION }));
}

#[test]
fn objects_are_kept_once_compressed_and_read_back_exactly() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open_history(folder.path().join("history.db")).unwrap();
    // A clip of 2,000 notes, with every field a note has.
    let notes: Vec<_> = (0..2000).map(|i| json!([36 + i % 48, i as f64 * 0.25, 0.25, 100.0, false, 1.0, 0.0, 64.0])).collect();
    let clip = json!({"kind":"midi","name":"Bass","looping":true,"loop":[0.0,4.0],"notes":notes});
    let one = Object::new("clip", 1, &clip);
    let other = Object::new("clip", 1, &json!({"kind":"midi","name":"Lead","notes":[]}));
    let objects = vec![one.clone(), other.clone(), one.clone()];
    assert_eq!(store.write_wait(move |c| history::put_objects(c, &objects)).unwrap(), 2);
    let again = vec![one.clone()];
    assert_eq!(store.write_wait(move |c| history::put_objects(c, &again)).unwrap(), 0, "the same content is kept once");
    let hash = one.hash.clone();
    let (kind, value) = store.read(move |c| history::object(c, &hash)).unwrap().unwrap();
    assert_eq!((kind.as_str(), &value), ("clip", &clip));
    let stored: i64 = store.read(|c| Ok(c.query_row("SELECT sum(length(z)) FROM objects", [], |row| row.get(0))?)).unwrap();
    assert!((stored as usize) * 4 < one.raw.len(), "{stored} bytes for {} of JSON", one.raw.len());
    assert_eq!(store.read(|c| history::object(c, "0".repeat(64).as_str())).unwrap(), None);
}

#[test]
fn ops_are_kept_with_their_view_and_listed_newest_first() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open_history(folder.path().join("history.db")).unwrap();
    let first = Op::new(None, 1_000, "cut", "Deleted Bass", json!({"leaves":["a"]}));
    let second = Op::new(Some(first.id.clone()), 2_000, "cut", "Cleared beats 4 to 8 on Drums", json!({"leaves":["b","c"]}));
    let ops = vec![first.clone(), second.clone()];
    store.write_wait(move |c| ops.iter().map(|op| history::put_op(c, op)).collect::<Result<Vec<_>, _>>()).unwrap();
    assert_eq!(store.read(|c| history::recent_ops(c, 10)).unwrap(), vec![second.clone(), first.clone()]);
    assert_eq!(store.read(|c| history::recent_ops(c, 1)).unwrap(), vec![second]);
}
