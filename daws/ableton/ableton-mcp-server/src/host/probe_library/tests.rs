//! The library probe's cache of read databases and their decoded indexes.
use super::*;
#[path = "../../../tests/support/library_fixtures.rs"]
mod library_fixtures;

/// A files database on disk, read through the host's cache.
async fn read(reads: &LibraryReads, path: &Path) -> (Arc<SqliteReader>, Arc<LibraryIndex>) {
    let reader = read_database(reads, path).await.unwrap();
    let index = library_index(reads, path, reader.clone()).await.unwrap();
    (reader, index)
}

#[tokio::test(flavor = "current_thread")]
async fn the_index_is_kept_with_its_database_read_and_made_again_when_the_file_changes() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("files.db");
    fs::write(&path, library_fixtures::files_db()).unwrap();
    let reads: LibraryReads = Default::default();
    let (reader, first) = read(&reads, &path).await;
    let (same_reader, again) = read(&reads, &path).await;
    assert!(Arc::ptr_eq(&reader, &same_reader), "the database is read once");
    assert!(Arc::ptr_eq(&first, &again), "the index is decoded once");
    // Another file in its place (the stamp moves): both are made again.
    let mut changed = library_fixtures::files_db();
    changed.extend_from_slice(&[0u8; 4096]);
    fs::write(&path, changed).unwrap();
    let (other_reader, other) = read(&reads, &path).await;
    assert!(!Arc::ptr_eq(&reader, &other_reader));
    assert!(!Arc::ptr_eq(&first, &other));
}

#[tokio::test(flavor = "current_thread")]
async fn an_unsupported_schema_is_refused_by_its_version_before_a_row_is_decoded() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("files.db");
    fs::write(&path, library_fixtures::unsupported_db()).unwrap();
    let reads: LibraryReads = Default::default();
    let reader = read_database(&reads, &path).await.unwrap();
    let Err(error) = library_index(&reads, &path, reader).await else { panic!("an unsupported schema was indexed") };
    let LibrarySearchError::Unavailable(unavailable) = error else { panic!("expected an unavailable error, got {error:?}") };
    assert_eq!(unavailable.details["observedVersion"], 99999);
}
