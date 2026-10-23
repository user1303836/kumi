use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use ableton_mcp_server::project::{
    project_backup, project_info, project_limitation, project_source_evidence, ObservedKind, ProjectBackupOptions, ProjectLimitation,
    ProjectReference, ReferenceBounds, ReferenceResolution,
};
use flate2::write::GzEncoder;
use flate2::Compression;

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn live_set(path: &Path, media: &[String]) {
    let escaped = |value: &str| value.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
    let refs: String = media.iter().map(|value| format!("<FileRef><Path Value=\"{}\" /></FileRef>", escaped(value))).collect();
    fs::write(
        path,
        gzip(
            format!("<Ableton><AudioTrack Id=\"1\"></AudioTrack><MidiTrack Id=\"2\"/><Scene /><Scene></Scene>{refs}</Ableton>").as_bytes(),
        ),
    )
    .unwrap();
}

fn temp_dir(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(prefix).tempdir().unwrap()
}

fn error_text<T: std::fmt::Debug>(result: Result<T, impl std::fmt::Display>) -> String {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn reads_bounded_live_set_metadata_and_writes_a_verified_colocated_backup_without_reading_media() {
    let root = temp_dir("ableton-project-");
    let existing = root.path().join("audio & source.wav");
    fs::write(&existing, "not read by project metadata").unwrap();
    let missing = path_string(&root.path().join("missing & source.wav"));
    let set = root.path().join("Song ü.als");
    live_set(&set, &[path_string(&existing), missing.clone(), missing.clone(), "relative.wav".to_string()]);
    let set = path_string(&set);
    let info = project_info(&set).unwrap();
    assert!(info.exists);
    assert_eq!(info.tracks, 2);
    assert_eq!(info.scenes, 2);
    assert_eq!(info.media_refs, 3);
    assert_eq!(info.missing_media, [missing]);
    assert!(info.sha256.len() == 64 && info.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    let options = ProjectBackupOptions {
        allowed_root: Some(path_string(root.path())),
        expected_sha256: Some(info.sha256.clone()),
        expected_size: Some(info.size),
        expected_mtime_ms: Some(info.mtime_ms),
    };
    let backup = project_backup(&set, &options).unwrap();
    assert!(backup.verified);
    assert!(Path::new(&backup.backup).exists());
    assert_eq!(fs::read(&backup.backup).unwrap(), fs::read(&set).unwrap());
    assert_eq!(backup.manifest.sha256, info.sha256);
    live_set(Path::new(&set), &[]);
    assert!(error_text(project_backup(&set, &options)).contains("changed since backup preview"));
    let other_root = temp_dir("ableton-project-other-");
    assert!(error_text(project_backup(
        &set,
        &ProjectBackupOptions { allowed_root: Some(path_string(other_root.path())), ..Default::default() }
    ))
    .contains("outside the explicit backup allowlist"));
    assert_eq!(
        project_limitation("save-as"),
        ProjectLimitation {
            available: false,
            operation: "save-as".to_string(),
            reason: "save-as is not exposed by the Live Remote Script API in this Live version and is not fabricated".to_string(),
            extension_point: "canonical project.new/open/save/save-as/collect/export/bounce operations are reserved for a future adapter and remain unadvertised until executable; project.info and project.backup are available now".to_string(),
        }
    );
}

#[test]
fn rejects_unsafe_linked_wrong_extension_and_malformed_live_set_paths() {
    let root = temp_dir("ableton-project-invalid-");
    let text = root.path().join("set.txt");
    fs::write(&text, "plain").unwrap();
    assert!(error_text(project_info("relative.als")).contains("absolute and safe"));
    assert!(error_text(project_info(&format!("{}\0unsafe.als", path_string(root.path())))).contains("absolute and safe"));
    assert!(error_text(project_info(&path_string(&text))).contains(".als file"));
    let directory_set = root.path().join("directory.als");
    fs::create_dir(&directory_set).unwrap();
    assert!(error_text(project_info(&path_string(&directory_set))).contains("not a regular file"));
    let oversized = root.path().join("oversized.als");
    fs::write(&oversized, "").unwrap();
    fs::OpenOptions::new().write(true).open(&oversized).unwrap().set_len(64 * 1024 * 1024 + 1).unwrap();
    assert!(error_text(project_info(&path_string(&oversized))).contains("bounded size"));
    let malformed = root.path().join("bad.als");
    fs::write(&malformed, "not gzip").unwrap();
    assert!(error_text(project_info(&path_string(&malformed))).contains("valid gzip-compressed Live set"));
    let bomb = root.path().join("bounded.als");
    fs::write(&bomb, gzip(&vec![0u8; 64 * 1024 * 1024 + 1])).unwrap();
    assert!(error_text(project_info(&path_string(&bomb))).contains("decompressed set exceeds the bounded size"));
    #[cfg(unix)]
    {
        let linked = root.path().join("linked.als");
        std::os::unix::fs::symlink(&malformed, &linked).unwrap();
        assert!(error_text(project_info(&path_string(&linked))).contains("symbolic link"));
    }
    #[cfg(not(unix))]
    eprintln!("symbolic-link fixture requires optional Windows privilege");
}

#[test]
fn reads_bounded_semantic_source_evidence_without_guessing_relative_file_refs() {
    let root = temp_dir("ableton-project-evidence-");
    let set = root.path().join("evidence.als");
    let absolute = path_string(&root.path().join("missing.wav"));
    fs::write(&set, gzip(format!("<Ableton Creator=\"Ableton Live 12\" MajorVersion=\"5\" MinorVersion=\"12.1\" SchemaChangeCount=\"3\"><FileRef><Path Value=\"relative.wav\"/></FileRef><FileRef><Path Value=\"{absolute}\"/></FileRef></Ableton>").as_bytes())).unwrap();
    let before = fs::read(&set).unwrap();
    let evidence = project_source_evidence(&path_string(&set)).unwrap();
    assert_eq!(evidence.ableton.creator.as_deref(), Some("Ableton Live 12"));
    assert_eq!(evidence.ableton.major_version.as_deref(), Some("5"));
    assert_eq!(evidence.ableton.minor_version.as_deref(), Some("12.1"));
    assert_eq!(evidence.ableton.schema_change_count.as_deref(), Some("3"));
    assert_eq!(
        evidence.references.iter().find(|row| row.value == "relative.wav"),
        Some(&ProjectReference {
            value: "relative.wav".to_string(),
            resolved_path: None,
            exists: None,
            project_local: None,
            resolution: ReferenceResolution::Unresolved
        })
    );
    assert_eq!(evidence.references.iter().find(|row| row.value == absolute).and_then(|row| row.exists), Some(false));
    assert_eq!(fs::read(&set).unwrap(), before);
}

#[test]
fn bounds_file_ref_probes_blocks_network_device_references_and_identifies_project_symlinks_conservatively() {
    let root = temp_dir("ableton-project-ref-bounds-");
    let project = root.path().join("Project");
    fs::create_dir(&project).unwrap();
    let outside = root.path().join("outside.wav");
    fs::write(&outside, "not read").unwrap();
    let linked = path_string(&project.join("linked.wav"));
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &linked).unwrap();
    #[cfg(not(unix))]
    eprintln!("symlink classification fixture skipped on Windows");
    let mut refs: Vec<String> = (0..4_100).map(|index| format!("/definitely-missing/ref-{index}.wav")).collect();
    let mut front: Vec<String> = [
        "\\\\attacker\\share\\sample.wav",
        "\\\\?\\C:\\device\\sample.wav",
        "https://private.example/sample.wav",
        "smb://attacker/share/sample.wav",
        "nfs://studio/export/sample.wav",
    ]
    .iter()
    .map(|value| value.to_string())
    .collect();
    if cfg!(unix) {
        front.insert(0, linked.clone());
    }
    front.append(&mut refs);
    let refs = front;
    let set = project.join("bounded.als");
    live_set(&set, &refs);
    let evidence = project_source_evidence(&path_string(&set)).unwrap();
    assert_eq!(evidence.reference_bounds.observed, 4097);
    assert_eq!(evidence.reference_bounds.observed_kind, ObservedKind::LowerBound);
    assert_eq!(evidence.reference_bounds.included, 4096);
    assert!(!evidence.reference_bounds.complete);
    assert_eq!(evidence.reference_bounds.omitted, 1);
    let network: Vec<&ProjectReference> =
        evidence.references.iter().filter(|reference| reference.resolution == ReferenceResolution::Network).collect();
    assert!(network.len() >= 5);
    assert!(network.iter().all(|reference| reference.value.starts_with("network-") && !reference.value.contains("://")));
    if cfg!(unix) {
        assert_eq!(
            evidence
                .references
                .iter()
                .find(|reference| reference.resolved_path.as_deref() == Some(linked.as_str()))
                .and_then(|reference| reference.project_local),
            Some(false)
        );
    }
}

#[test]
fn stops_file_ref_observation_after_the_first_overflow_without_retaining_the_remaining_unique_paths() {
    let root = temp_dir("ableton-project-many-refs-");
    let set = root.path().join("many.als");
    let refs: String = (0..20_000).map(|index| format!("<FileRef><Path Value=\"/missing/{index}.wav\" /></FileRef>")).collect();
    fs::write(&set, gzip(format!("<Ableton>{refs}</Ableton>").as_bytes())).unwrap();
    let started = Instant::now();
    let evidence = project_source_evidence(&path_string(&set)).unwrap();
    assert_eq!(evidence.references.len(), 4096);
    assert_eq!(
        evidence.reference_bounds,
        ReferenceBounds { observed: 4097, observed_kind: ObservedKind::LowerBound, included: 4096, omitted: 1, complete: false }
    );
    assert_eq!(evidence.manifest.media_refs, 4097);
    assert!(started.elapsed().as_millis() < 5_000);
}

#[test]
fn decodes_numeric_xml_path_entities_and_validates_backup_allowlist_roots() {
    let root = temp_dir("ableton-project-entities-");
    let set = root.path().join("entities.als");
    let missing = path_string(&root.path().join("missing'A.wav"));
    let encoded = missing.replacen('/', "&#47;", 1).replace('/', "&#x2f;").replacen('\'', "&apos;", 1).replacen('A', "&#65;", 1);
    fs::write(&set, gzip(format!("<Ableton><FileRef><Path Value=\"{encoded}\" /></FileRef></Ableton>").as_bytes())).unwrap();
    let set = path_string(&set);
    assert_eq!(project_info(&set).unwrap().missing_media, [missing]);

    assert!(error_text(project_backup(&set, &ProjectBackupOptions { allowed_root: Some("relative".to_string()), ..Default::default() }))
        .contains("absolute safe directory"));
    let root_file = root.path().join("not-directory");
    fs::write(&root_file, "x").unwrap();
    assert!(error_text(project_backup(&set, &ProjectBackupOptions { allowed_root: Some(path_string(&root_file)), ..Default::default() }))
        .contains("real directory"));

    let invalid_entity = root.path().join("invalid-entity.als");
    fs::write(&invalid_entity, gzip(b"<Ableton><FileRef><Path Value=\"/tmp/&#xD800;.wav\" /></FileRef></Ableton>")).unwrap();
    assert!(error_text(project_info(&path_string(&invalid_entity))).contains("invalid XML path entity"));
}
