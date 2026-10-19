#[path = "support/lifecycle_fixture.rs"]
mod fixture;
use ableton_mcp_server::lifecycle::*;
use fixture::*;
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, io::Read};
#[test]
fn strict_native_and_legacy_manifests_bind_all_payloads_and_reject_mixed_policy() {
    let folder = tempfile::tempdir().unwrap();
    for policy in ["native", "legacy", "current", "node25"] {
        let (root, artifact, hash) = package(folder.path(), "1.0.0", policy);
        let evidence = verify_release_package(&root, true).unwrap();
        assert_eq!(verify_artifact_binding(Some(&artifact), Some(&hash), &root, &evidence.manifest_sha256).unwrap(), hash);
        assert!(verify_release_package(&root, false).unwrap_err().message().contains("dirty source tree"));
        let manifest = root.join("release-manifest.json");
        let original = read(&manifest);
        for (pointer, value) in [
            ("/distribution/signed", json!(true)),
            ("/distribution/integrityIsIdentityProof", json!(true)),
            ("/distribution/channel", json!("public")),
            ("/package/private", json!(false)),
            ("/source/commit", json!("not-hex")),
            ("/roles/LICENSE.md", json!("documentation")),
            ("/roles/package.json", Value::Null),
            ("/build/builder/workflowSha256", json!("x".repeat(64))),
        ] {
            let mut invalid = original.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            write(&manifest, &invalid);
            let error = verify_release_package(&root, true).unwrap_err();
            assert!(error.message().contains("policy is invalid"), "{policy} {pointer}: {}", error.message());
        }
        write(&manifest, &original);
        fs::write(root.join("unknown.txt"), "unknown").unwrap();
        assert!(verify_release_package(&root, true).unwrap_err().message().contains("root inventory differs"));
        fs::remove_file(root.join("unknown.txt")).unwrap();
        fs::create_dir(root.join("unknown-empty")).unwrap();
        assert!(verify_release_package(&root, true).unwrap_err().message().contains("unknown directory"));
        fs::remove_dir(root.join("unknown-empty")).unwrap();
        fs::write(root.join("release-docs/doc-0.md"), "modified").unwrap();
        assert!(verify_release_package(&root, true).unwrap_err().message().contains("payload hash mismatch"));
    }
}
#[test]
fn native_metadata_roles_and_license_cannot_be_substituted() {
    let folder = tempfile::tempdir().unwrap();
    let (root, _, _) = package(folder.path(), "1.0.0", "native");
    let original = read(root.join("release-manifest.json"));
    let metadata = read(root.join("package.json"));
    for (key, value) in [
        ("runtime", json!("node")),
        ("target", json!("different-target")),
        ("bin", json!({"ableton-mcp-server":"dist/src/cli.js"})),
        ("version", json!("2.0.0")),
    ] {
        let mut changed = metadata.clone();
        changed[key] = value;
        write(root.join("package.json"), &changed);
        assert!(verify_release_package(&root, true).unwrap_err().message().contains("metadata and release manifest policy disagree"));
    }
    write(root.join("package.json"), &metadata);
    for (name, role, accepted) in [
        ("README.md", "documentation", true),
        ("NOTES.md", "documentation", false),
        ("README.md", "compiled-runtime", false),
        ("dist/src/cli.js", "compiled-runtime", false),
    ] {
        let file = root.join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "documentation").unwrap();
        let mut manifest = original.clone();
        manifest["files"][name] = json!(sha("documentation"));
        manifest["roles"][name] = json!(role);
        write(root.join("release-manifest.json"), &manifest);
        assert_eq!(verify_release_package(&root, true).is_ok(), accepted, "{name} {role}");
        fs::remove_file(file).unwrap();
        if name.starts_with("dist/") {
            fs::remove_dir_all(root.join("dist")).unwrap();
        }
    }
}
#[test]
fn strict_tar_parser_rejects_checksum_links_duplicates_paths_and_termination() {
    let folder = tempfile::tempdir().unwrap();
    let (root, artifact, hash) = package(folder.path(), "1.0.0", "native");
    let evidence = verify_release_package(&root, true).unwrap();
    let mut original = Vec::new();
    flate2::read::GzDecoder::new(fs::read(&artifact).unwrap().as_slice()).read_to_end(&mut original).unwrap();
    let check = |bytes: &[u8], expected: &str| {
        let compressed = gzip(bytes);
        fs::write(&artifact, &compressed).unwrap();
        let error = verify_artifact_binding(Some(&artifact), Some(&sha(&compressed)), &root, &evidence.manifest_sha256).unwrap_err();
        assert!(error.message().contains(expected), "expected {expected}: {}", error.message());
    };
    let checksum = |header: &mut [u8]| {
        header[148..156].fill(b' ');
        let sum = header.iter().map(|b| *b as u64).sum::<u64>();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    };
    let mut bytes = original.clone();
    bytes[100] ^= 1;
    check(&bytes, "checksum is invalid");
    for kind in [b'1', b'2', b'5', b'x', b'g'] {
        let mut bytes = original.clone();
        bytes[156] = kind;
        checksum(&mut bytes[..512]);
        check(&bytes, "non-regular entry");
    }
    for name in ["../escape", "package/../escape", "other/path", "package/back\\slash"] {
        let mut bytes = original.clone();
        bytes[..100].fill(0);
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        checksum(&mut bytes[..512]);
        check(&bytes, "path is malformed");
    }
    let size = usize::from_str_radix(std::str::from_utf8(&original[124..135]).unwrap(), 8).unwrap();
    let first = 512 + size.div_ceil(512) * 512;
    let mut bytes = original[..first].to_vec();
    bytes.extend_from_slice(&original);
    check(&bytes, "duplicate entry");
    check(&original[..original.len() - 1024], "no valid end marker");
    let mut bytes = original.clone();
    *bytes.last_mut().unwrap() = 1;
    check(&bytes, "non-zero trailing content");
    let mut bytes = original.clone();
    bytes[124..136].copy_from_slice(b"77777777777\0");
    checksum(&mut bytes[..512]);
    check(&bytes, "path is malformed");
    let mut files = BTreeMap::new();
    let manifest = read(root.join("release-manifest.json"));
    for name in manifest["files"].as_object().unwrap().keys() {
        files.insert(format!("package/{name}"), fs::read(root.join(name)).unwrap());
    }
    files.insert("package/release-manifest.json".into(), fs::read(root.join("release-manifest.json")).unwrap());
    files.insert("package/extra.js".into(), b"extra".to_vec());
    check(&tar(&files), "inventory differs");
    files.remove("package/extra.js");
    files.insert("package/release-docs/doc-0.md".into(), b"drift".to_vec());
    check(&tar(&files), "payload hash mismatch");
    fs::write(&artifact, b"not gzip").unwrap();
    assert!(verify_artifact_binding(Some(&artifact), Some(&sha("not gzip")), &root, &evidence.manifest_sha256)
        .unwrap_err()
        .message()
        .contains("not a valid gzip"));
    assert!(verify_artifact_binding(Some(&artifact), Some(&hash), &root, &evidence.manifest_sha256)
        .unwrap_err()
        .message()
        .contains("exact tarball bytes"));
}
#[test]
fn archive_decompression_is_bounded_and_manifest_binding_is_exact() {
    let folder = tempfile::tempdir().unwrap();
    let (root, artifact, _) = package(folder.path(), "1.0.0", "native");
    let evidence = verify_release_package(&root, true).unwrap();
    let compressed = gzip(&vec![0; 64 * 1024 * 1024 + 1]);
    fs::write(&artifact, &compressed).unwrap();
    assert!(verify_artifact_binding(Some(&artifact), Some(&sha(&compressed)), &root, &evidence.manifest_sha256)
        .unwrap_err()
        .message()
        .contains("bounded decompressed size"));
    let (artifact, hash) = bind(&root);
    assert!(verify_artifact_binding(Some(&artifact), Some(&hash), &root, &"a".repeat(64))
        .unwrap_err()
        .message()
        .contains("different release manifests"));
    let file = fs::File::create(&artifact).unwrap();
    file.set_len(32 * 1024 * 1024 + 1).unwrap();
    assert!(verify_artifact_binding(Some(&artifact), Some(&hash), &root, &evidence.manifest_sha256)
        .unwrap_err()
        .message()
        .contains("bounded compressed size"));
}

#[test]
fn gzip_members_and_zero_padding_match_node_gunzip() {
    let folder = tempfile::tempdir().unwrap();
    let (root, artifact, _) = package(folder.path(), "1.0.0", "native");
    let evidence = verify_release_package(&root, true).unwrap();
    let original = fs::read(&artifact).unwrap();
    for suffix in [vec![0], vec![0, 7], gzip(b"")] {
        let mut bytes = original.clone();
        bytes.extend_from_slice(&suffix);
        fs::write(&artifact, &bytes).unwrap();
        assert!(verify_artifact_binding(Some(&artifact), Some(&sha(&bytes)), &root, &evidence.manifest_sha256).is_ok());
    }
    let mut bytes = original;
    bytes.push(7);
    fs::write(&artifact, &bytes).unwrap();
    assert!(verify_artifact_binding(Some(&artifact), Some(&sha(&bytes)), &root, &evidence.manifest_sha256)
        .unwrap_err()
        .message()
        .contains("not a valid gzip"));
}
#[test]
fn native_release_requires_one_matching_analysis_worker_and_binds_its_bytes() {
    let folder = tempfile::tempdir().unwrap();
    let (root, artifact, hash) = package(folder.path(), "1.0.0", "native");
    let worker = if cfg!(windows) { "ableton-mcp-analysis-worker.exe" } else { "ableton-mcp-analysis-worker" };
    let alternate = if cfg!(windows) { "ableton-mcp-analysis-worker" } else { "ableton-mcp-analysis-worker.exe" };
    let manifest_path = root.join("release-manifest.json");
    let manifest = read(&manifest_path);
    let payload = fs::read(root.join(worker)).unwrap();
    let evidence = verify_release_package(&root, true).unwrap();
    assert_eq!(verify_artifact_binding(Some(&artifact), Some(&hash), &root, &evidence.manifest_sha256).unwrap(), hash);
    let mut missing = manifest.clone();
    missing["files"].as_object_mut().unwrap().remove(worker);
    missing["roles"].as_object_mut().unwrap().remove(worker);
    write(&manifest_path, &missing);
    fs::remove_file(root.join(worker)).unwrap();
    assert!(verify_release_package(&root, true).unwrap_err().message().contains("metadata and release manifest policy disagree"));
    let mut mismatched = missing.clone();
    mismatched["files"][alternate] = json!(sha(&payload));
    mismatched["roles"][alternate] = json!("native-runtime");
    fs::write(root.join(alternate), &payload).unwrap();
    write(&manifest_path, &mismatched);
    assert!(verify_release_package(&root, true).unwrap_err().message().contains("metadata and release manifest policy disagree"));
    fs::remove_file(root.join(alternate)).unwrap();
    fs::write(root.join(worker), &payload).unwrap();
    write(&manifest_path, &manifest);
    fs::write(root.join(worker), "tampered worker").unwrap();
    assert!(verify_release_package(&root, true).unwrap_err().message().contains("release payload hash mismatch"));
}
