//! The TypeScript test drives `live_library_search` through `McpHost`; the host's own glue
//! (allowlist containment, the `-wal` file check, the response envelope with `schema`,
//! `unavailable`, `privacy` and `provenance`) belongs to the host's tests. Everything the
//! library-search module itself decides is asserted here against the same fixtures.

#[path = "support/library_fixtures.rs"]
mod library_fixtures;

use std::collections::HashMap;
use std::fs;

use ableton_mcp_server::library_search::{
    assert_supported_files_schema, assert_supported_plugins_schema, query_library_files, query_library_plugins,
    query_library_tag_vocabulary, LibraryItem, LibraryKind, LibraryMode, LibraryPluginItem, LibraryQuery, LibrarySearchError, LibrarySort,
    LibraryTagEntry, PluginFormat, SUPPORTED_FILES_SCHEMA_VERSIONS,
};
use ableton_mcp_server::sqlite_reader::SqliteReader;
use serde::Serialize;
use sha2::{Digest, Sha256};

fn files_reader() -> SqliteReader {
    SqliteReader::new(library_fixtures::files_db()).unwrap()
}

fn plugins_reader() -> SqliteReader {
    SqliteReader::new(library_fixtures::plugins_db()).unwrap()
}

/// The query the host builds from `{ database, allowlistRoot }` alone.
fn files_args() -> LibraryQuery {
    LibraryQuery::new(LibraryMode::Files, 50)
}

fn files(reader: &SqliteReader, query: &LibraryQuery) -> Vec<LibraryItem> {
    query_library_files(reader, query).unwrap().items
}

fn names(items: &[LibraryItem]) -> Vec<&str> {
    items.iter().map(|item| item.name.as_str()).collect()
}

fn json<T: Serialize>(value: &T) -> String {
    kumi_common::js::json::stringify(&serde_json::to_value(value).unwrap())
}

fn is_unavailable(error: &LibrarySearchError) -> bool {
    matches!(error, LibrarySearchError::Unavailable(_))
}

#[test]
fn fixtures_decode_to_the_same_bytes_as_the_typescript_suite() {
    let digest = |bytes: &[u8]| hex::encode(Sha256::digest(bytes));
    assert_eq!(digest(&library_fixtures::files_db()), "dac33eb11965896c03a514afaaa88f70ad96d299d17241da2677d556184e33ec");
    assert_eq!(digest(&library_fixtures::plugins_db()), "abf6f677f5f63bcda61bc00b55ab13528d73e90aa264dc6388ab1c9b23b1e26a");
    assert_eq!(digest(&library_fixtures::unsupported_db()), "48d9226267001af3206708bb3df3ddccecb4314617a09b00334841be86d467e7");
}

#[test]
fn files_mode_classifies_kinds_with_evidence_and_sorts_by_usage_count() {
    let reader = files_reader();
    assert_eq!(assert_supported_files_schema(&reader).unwrap(), 12300);
    assert_eq!(SUPPORTED_FILES_SCHEMA_VERSIONS, &[12300]);
    let items = files(&reader, &files_args());
    assert_eq!(
        names(&items),
        ["Kick 909 Core.wav", "Wavetable", "My Set.als", "FX Rack.adg", "Envelope MIDI.amxd", "Operator", "Dusty Rhodes.aif"]
    );
    let kinds: HashMap<&str, LibraryKind> = items.iter().map(|item| (item.name.as_str(), item.kind)).collect();
    assert_eq!(kinds["Kick 909 Core.wav"], LibraryKind::Audio);
    assert_eq!(kinds["Wavetable"], LibraryKind::Device);
    assert_eq!(kinds["My Set.als"], LibraryKind::Set);
    assert_eq!(kinds["FX Rack.adg"], LibraryKind::DeviceGroup);
    assert_eq!(kinds["Envelope MIDI.amxd"], LibraryKind::MaxDevice);
    assert!(items.iter().find(|item| item.name == "Kick 909 Core.wav").unwrap().kind_evidence.contains("file_type 'wav'"));
    assert!(items.iter().all(|item| item.kind != LibraryKind::Other || item.kind_evidence.contains("not classified")));
    // folder and tag-vocabulary rows never appear as content
    assert!(!names(&items).contains(&"Samples"));
    assert!(!names(&items).contains(&"Delay"));
}

#[test]
fn name_wildcard_kind_source_and_sort_filters_compose() {
    let reader = files_reader();
    let substring = files(&reader, &LibraryQuery { query: Some("kick".to_string()), ..files_args() });
    assert_eq!(names(&substring), ["Kick 909 Core.wav"]);
    let wildcard = files(&reader, &LibraryQuery { query: Some("*.a*".to_string()), ..files_args() });
    let mut wildcard_names = names(&wildcard);
    wildcard_names.sort();
    let mut expected = ["Dusty Rhodes.aif", "FX Rack.adg", "Envelope MIDI.amxd", "My Set.als"];
    expected.sort();
    assert_eq!(wildcard_names, expected);
    let audio = files(&reader, &LibraryQuery { kinds: Some(vec![LibraryKind::Audio]), ..files_args() });
    assert_eq!(names(&audio), ["Kick 909 Core.wav", "Dusty Rhodes.aif"]);
    let user_library = files(&reader, &LibraryQuery { sources: Some(vec!["User Library".to_string()]), ..files_args() });
    assert_eq!(names(&user_library), ["Kick 909 Core.wav", "Dusty Rhodes.aif"]);
    let by_modified = files(&reader, &LibraryQuery { sort: Some(LibrarySort::Modified), ..files_args() });
    assert_eq!(by_modified[0].name, "My Set.als");
    let by_name = files(&reader, &LibraryQuery { sort: Some(LibrarySort::Name), ..files_args() });
    let mut sorted = names(&by_name);
    sorted.sort();
    assert_eq!(names(&by_name), sorted);
}

#[test]
fn tag_conjunction_filters_accept_leaf_names_and_full_paths() {
    let reader = files_reader();
    let with_tags =
        |tags: &[&str]| files(&reader, &LibraryQuery { tags: Some(tags.iter().map(|tag| tag.to_string()).collect()), ..files_args() });
    let kick = with_tags(&["Kick"]);
    assert_eq!(names(&kick), ["Kick 909 Core.wav"]);
    let conjunction = with_tags(&["Drums", "Kick"]);
    assert_eq!(names(&conjunction), ["Kick 909 Core.wav"]);
    let path = with_tags(&["Devices|Synthesizer|FM"]);
    assert_eq!(names(&path), ["Wavetable"]);
    let none = with_tags(&["Synthesizer", "Kick"]);
    assert_eq!(none.len(), 0);
    let item = &kick[0];
    let mut paths: Vec<&str> = item.tags.iter().map(|tag| tag.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, ["Drums", "Drums|Kick"]);
    assert!(item.tags.iter().all(|tag| !tag.is_auto));
    let auto_tagged = &with_tags(&["Keys"])[0];
    assert!(auto_tagged.tags[0].is_auto, "auto-assigned tags are labeled");
}

#[test]
fn paging_is_revision_bound_and_honest_about_truncation() {
    let reader = files_reader();
    let first = query_library_files(&reader, &LibraryQuery { limit: 3, ..files_args() }).unwrap();
    assert_eq!(first.paging.returned, 3);
    assert_eq!(first.paging.total, 7);
    assert!(!first.paging.complete);
    assert!(!first.paging.truncated);
    let second =
        query_library_files(&reader, &LibraryQuery { limit: 3, cursor: first.paging.next_cursor.clone(), ..files_args() }).unwrap();
    assert_eq!(second.paging.returned, 3);
    let third =
        query_library_files(&reader, &LibraryQuery { limit: 3, cursor: second.paging.next_cursor.clone(), ..files_args() }).unwrap();
    assert_eq!(third.paging.returned, 1);
    assert!(third.paging.complete);
    let all: std::collections::HashSet<&str> =
        first.items.iter().chain(&second.items).chain(&third.items).map(|item| item.name.as_str()).collect();
    assert_eq!(all.len(), 7, "pages are disjoint and cover the match set");
    let stale = query_library_files(
        &reader,
        &LibraryQuery { query: Some("kick".to_string()), limit: 3, cursor: first.paging.next_cursor.clone(), ..files_args() },
    );
    assert!(is_unavailable(&stale.unwrap_err()), "a cursor from a different query is stale");
    let garbage = query_library_files(&reader, &LibraryQuery { cursor: Some("!!not-a-cursor!!".to_string()), ..files_args() });
    assert!(is_unavailable(&garbage.unwrap_err()));
}

#[test]
fn browser_candidates_resolve_only_with_device_class_evidence_everything_else_is_discovery_only() {
    let reader = files_reader();
    let items = files(&reader, &files_args());
    let wavetable = items.iter().find(|item| item.name == "Wavetable").unwrap();
    assert_eq!(wavetable.browser_candidate.as_ref().unwrap().item_id, "instruments/Wavetable");
    assert!(!wavetable.discovery_only);
    assert!(wavetable.browser_candidate.as_ref().unwrap().resolution.contains("live_browser_inspect"));
    let sample = items.iter().find(|item| item.name == "Kick 909 Core.wav").unwrap();
    assert_eq!(sample.browser_candidate, None);
    assert!(sample.discovery_only);
}

#[test]
fn plug_in_inventory_covers_vendor_and_format_filters_with_redacted_paths() {
    let reader = plugins_reader();
    assert_eq!(assert_supported_plugins_schema(&reader).unwrap(), 1);
    let plugins_args = || LibraryQuery::new(LibraryMode::Plugins, 50);
    let inventory = query_library_plugins(&reader, &plugins_args()).unwrap();
    let by_name: HashMap<&str, &LibraryPluginItem> = inventory.items.iter().map(|item| (item.name.as_str(), item)).collect();
    assert_eq!(by_name["Serum"].format, PluginFormat::Vst3);
    assert_eq!(by_name["Serum"].vendor.as_deref(), Some("Xfer Records"));
    assert_eq!(by_name["Diva FX"].format, PluginFormat::Au);
    assert_eq!(by_name["OldSynth"].format, PluginFormat::Vst2);
    assert!(!by_name["OldSynth"].enabled);
    assert!(!by_name["OldSynth"].scanned);
    assert_eq!(by_name["Serum"].module_basename.as_deref(), Some("Serum.vst3"));
    assert!(!json(&inventory).contains("/Library/Audio"), "raw filesystem paths are redacted");
    let vendor = query_library_plugins(&reader, &LibraryQuery { vendors: Some(vec!["u-he".to_string()]), ..plugins_args() }).unwrap();
    assert_eq!(vendor.items.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(), ["Diva FX"]);
    let format = query_library_plugins(&reader, &LibraryQuery { formats: Some(vec![PluginFormat::Vst3]), ..plugins_args() }).unwrap();
    assert_eq!(format.items.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(), ["Serum"]);
}

#[test]
fn tag_vocabulary_mode_lists_the_tag_tree_with_usage_counts() {
    let reader = files_reader();
    let tags_args = || LibraryQuery::new(LibraryMode::Tags, 50);
    let response = query_library_tag_vocabulary(&reader, &tags_args()).unwrap();
    let by_path: HashMap<&str, &LibraryTagEntry> = response.items.iter().map(|item| (item.path.as_str(), item)).collect();
    assert_eq!(by_path["Devices|Synthesizer"].usage_count, 2);
    assert_eq!(by_path["Devices|Synthesizer|FM"].usage_count, 1);
    assert_eq!(by_path["Drums|Kick"].usage_count, 1);
    assert_eq!(by_path["Devices|Delay"].usage_count, 1);
    assert_eq!(by_path["Keys"].usage_count, 1);
    let filtered = query_library_tag_vocabulary(&reader, &LibraryQuery { query: Some("synth".to_string()), ..tags_args() }).unwrap();
    assert_eq!(filtered.items.iter().map(|item| item.path.as_str()).collect::<Vec<_>>(), ["Devices|Synthesizer", "Devices|Synthesizer|FM"]);
}

#[test]
fn schema_gating_is_fail_closed_unsupported_versions_missing_databases_and_non_databases() {
    let unsupported = SqliteReader::new(library_fixtures::unsupported_db()).unwrap();
    let error = assert_supported_files_schema(&unsupported).unwrap_err();
    let LibrarySearchError::Unavailable(unavailable) = &error else { panic!("expected LibraryUnavailable, got {error:?}") };
    assert_eq!(unavailable.details["observedVersion"], 99999);
    assert_eq!(unavailable.details["supportedVersions"], serde_json::json!([12300]));
    // A missing database never reaches the reader (the host reports it unavailable); a non-database fails in it.
    assert!(SqliteReader::new(b"this is not a sqlite database at all".to_vec()).is_err());
}

#[test]
fn a_database_with_uncheckpointed_wal_frames_is_refused_an_empty_wal_is_checkpoint_equivalent() {
    // byte 19 is the file-format read version: 2 means WAL; the host refuses a non-empty -wal file
    // beside it and otherwise reads the main file, which carries the same content.
    let mut wal_bytes = library_fixtures::files_db();
    wal_bytes[19] = 2;
    let reader = SqliteReader::new(wal_bytes).unwrap();
    assert!(reader.wal_mode);
    assert!(!files_reader().wal_mode);
    assert_eq!(assert_supported_files_schema(&reader).unwrap(), 12300, "a fully checkpointed WAL-mode database reads from the main file");
}

#[test]
fn library_search_never_writes_to_any_live_file() {
    let root = tempfile::Builder::new().prefix("library-search-").tempdir().unwrap();
    let files_db = root.path().join("Live-files-12300.db");
    let plugins_db = root.path().join("Live-plugins-1.db");
    fs::write(&files_db, library_fixtures::files_db()).unwrap();
    fs::write(&plugins_db, library_fixtures::plugins_db()).unwrap();
    let listing = || {
        let mut names: Vec<String> =
            fs::read_dir(root.path()).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    };
    let before_files = fs::read(&files_db).unwrap();
    let before_plugins = fs::read(&plugins_db).unwrap();
    let before_listing = listing();
    let files_reader = SqliteReader::new(fs::read(&files_db).unwrap()).unwrap();
    let plugins_reader = SqliteReader::new(fs::read(&plugins_db).unwrap()).unwrap();
    query_library_files(&files_reader, &files_args()).unwrap();
    query_library_tag_vocabulary(&files_reader, &LibraryQuery::new(LibraryMode::Tags, 50)).unwrap();
    query_library_plugins(&plugins_reader, &LibraryQuery::new(LibraryMode::Plugins, 50)).unwrap();
    assert_eq!(fs::read(&files_db).unwrap(), before_files, "the files database is byte-identical after queries");
    assert_eq!(fs::read(&plugins_db).unwrap(), before_plugins, "the plug-ins database is byte-identical after queries");
    assert_eq!(listing(), before_listing, "no journals, WAL files, or other artifacts are created");
}

/// Pins taken from the TypeScript module over the same fixtures: cursors, page JSON and the
/// parsed schema must come out byte for byte the same.
#[test]
fn serialized_pages_and_cursors_match_the_typescript_byte_for_byte() {
    let reader = files_reader();
    let first = query_library_files(&reader, &LibraryQuery { limit: 3, ..files_args() }).unwrap();
    assert_eq!(
        first.paging.next_cursor.as_deref(),
        Some("eyJyZXZpc2lvbiI6ImUyNmU1ZThmMjZmZWJjMTM3OWEzNGJiMWZjMDk3YmIzMGMyOGIwNDE5ZDg0Mjk0MTIyYjVlNGY2YzI4ZGE0NDciLCJvZmZzZXQiOjN9")
    );
    let second =
        query_library_files(&reader, &LibraryQuery { limit: 3, cursor: first.paging.next_cursor.clone(), ..files_args() }).unwrap();
    assert_eq!(
        second.paging.next_cursor.as_deref(),
        Some("eyJyZXZpc2lvbiI6ImUyNmU1ZThmMjZmZWJjMTM3OWEzNGJiMWZjMDk3YmIzMGMyOGIwNDE5ZDg0Mjk0MTIyYjVlNGY2YzI4ZGE0NDciLCJvZmZzZXQiOjZ9")
    );
    let digest = |text: String| hex::encode(Sha256::digest(text.as_bytes()));
    assert_eq!(
        digest(json(&query_library_files(&reader, &files_args()).unwrap())),
        "e90854a51089db5064c04a3355d4b86277ff7bef6966453d0ef80cf2d162adb1"
    );
    assert_eq!(
        digest(json(&query_library_plugins(&plugins_reader(), &LibraryQuery::new(LibraryMode::Plugins, 50)).unwrap())),
        "ca809df00ce87ff383c7042308c2ba85eac693b35045ec870631fb99d575425d"
    );
    assert_eq!(
        digest(json(&query_library_tag_vocabulary(&reader, &LibraryQuery::new(LibraryMode::Tags, 50)).unwrap())),
        "a0fbc3668f1b19089375776bd7e5510df56fe189e51d62faef17973d5ba21ff1"
    );
    assert_eq!(reader.table_names().join(","), "ancestors,devices,fe_values,fe_values_record,file_devices,files,keywords,metadata,metadata_values,places,sqlite_sequence,version,vfolder_patterns,vfolders");
    assert_eq!(reader.table_columns("files").unwrap().join(","), "file_id,parent_id,file_type,subtype,file_kind,mod_date,file_size,aggr_id,name,colors,md_version,scanner_version,use_count,place_id,flags,device_type,device_arch,device_id,edit_source,edit_date,fe_version");
}

#[test]
fn results_redact_the_database_path_and_owner_allowlist_root() {
    let reader = files_reader();
    let serialized = json(&query_library_files(&reader, &files_args()).unwrap());
    assert!(!serialized.contains("Live-files-12300.db"), "the database path never appears in results");
    assert!(!serialized.contains("/Users/"), "no filesystem path appears in results");
}
