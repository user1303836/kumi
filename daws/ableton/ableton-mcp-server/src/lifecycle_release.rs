//! Strict extracted package and exact bounded tarball verification.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
};
const MIT_LICENSE_SHA256: &str = "f6a4bf820a492313c9d4e100e16bd474cd5cf06c0fba27c1035238acb4af75cb";
const MAX_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ARTIFACT_TAR_BYTES: u64 = 64 * 1024 * 1024;
#[derive(Debug, Clone)]
pub struct ReleaseEvidence {
    pub manifest: Value,
    pub manifest_sha256: String,
}
fn release_role(name: &str, legacy: bool, native: bool) -> Option<&'static str> {
    if name == "LICENSE.md" {
        Some(if legacy { "private-license" } else { "license" })
    } else if name == "package.json" {
        Some("package-metadata")
    } else if native
        && ["ableton-mcp-server", "ableton-mcp-server.exe", "ableton-mcp-analysis-worker", "ableton-mcp-analysis-worker.exe"]
            .contains(&name)
    {
        Some("native-runtime")
    } else if !native && name.starts_with("dist/src/") && (name.ends_with(".js") || name.ends_with(".d.ts")) {
        Some("compiled-runtime")
    } else if name == "README.md" || name.starts_with("release-docs/") && name.ends_with(".md") {
        Some("documentation")
    } else if name.starts_with("remote-script/") {
        Some("ableton-remote-script")
    } else if name.starts_with("live-extension/") {
        Some("ableton-live-extension")
    } else {
        None
    }
}
fn roles_valid(manifest: &Value) -> bool {
    let (Some(files), Some(roles)) = (manifest["files"].as_object(), manifest["roles"].as_object()) else {
        return false;
    };
    let legacy = manifest["schema"] == "ableton-mcp-private-release/v1";
    let native = manifest["schema"] == "ableton-mcp-native-release/v1";
    files.len() == roles.len()
        && files.keys().all(|name| {
            roles.get(name).and_then(Value::as_str) == release_role(name, legacy, native) && release_role(name, legacy, native).is_some()
        })
}
/// A release this bridge installs, upgrades to or repairs: one built with this bridge's own registry.
pub fn verify_release_package(package_root: &Path, allow_dirty: bool) -> Result<ReleaseEvidence, LiveError> {
    let evidence = verify_package(package_root, allow_dirty)?;
    if evidence.manifest["protocol"]["registryHash"] != registry_digest() {
        return Err(fail("release and runtime registry hashes disagree"));
    }
    Ok(evidence)
}
/// A generation kept to go back to, or the one installed: it runs its own bridge, so it carries the registry its
/// manifest names, which an older release's differs from this bridge's.
pub fn verify_retained_package(package_root: &Path, allow_dirty: bool) -> Result<ReleaseEvidence, LiveError> {
    let evidence = verify_package(package_root, allow_dirty)?;
    let names: Vec<&String> = evidence.manifest["files"]
        .as_object()
        .map(|files| files.keys().filter(|name| name.rsplit('/').next() == Some("ableton-live-v1.operations.json")).collect())
        .unwrap_or_default();
    // A release without its registry file can't show its own: it's held to this bridge's, as any release was before.
    if names.is_empty() {
        if evidence.manifest["protocol"]["registryHash"] != registry_digest() {
            return Err(fail("release and runtime registry hashes disagree"));
        }
        return Ok(evidence);
    }
    for name in names {
        let text = String::from_utf8(read(&package_root.join(name))?).map_err(|_| fail("the retained release's registry isn't text"))?;
        let hash = crate::registry::registry_text_hash(&text).map_err(|error| fail(error.0))?;
        if json!(hash) != evidence.manifest["protocol"]["registryHash"] {
            return Err(fail("the retained release's registry and its manifest disagree"));
        }
    }
    Ok(evidence)
}
fn verify_package(package_root: &Path, allow_dirty: bool) -> Result<ReleaseEvidence, LiveError> {
    validate_absolute_path(package_root, "package root")?;
    assert_no_linked_ancestors(package_root)?;
    let manifest_path = package_root.join("release-manifest.json");
    let entry = lstat(&manifest_path)?;
    if !entry.is_file() || entry.file_type().is_symlink() {
        return Err(fail("release manifest must be a regular file"));
    }
    let manifest: Value = serde_json::from_slice(&read(&manifest_path)?)?;
    let package = &manifest["package"];
    let distribution = &manifest["distribution"];
    let build = &manifest["build"];
    let builder = &build["builder"];
    let files = &manifest["files"];
    let legacy = manifest["schema"] == "ableton-mcp-private-release/v1"
        && package["license"] == "UNLICENSED"
        && distribution["channel"] == "private-local-npm-tarball"
        && distribution["integrityIsIdentityProof"] == false
        && manifest["roles"]["LICENSE.md"] == "private-license";
    let current_runtime = build["nodeRange"] == NODE_ENGINE_RANGE && build["nodeMajors"] == json!(SUPPORTED_NODE_MAJORS);
    let prior_runtime = build["nodeRange"] == ">=22 <23 || >=24 <25 || >=25 <26" && build["nodeMajors"] == json!([22, 24, 25]);
    let mit_policy = package["license"] == "MIT"
        && distribution["integrityIsIdentityProof"] == false
        && manifest["roles"]["LICENSE.md"] == "license"
        && files["LICENSE.md"] == MIT_LICENSE_SHA256;
    let current = manifest["schema"] == "ableton-mcp-release/v2"
        && distribution["channel"] == "local-npm-tarball"
        && mit_policy
        && (current_runtime || prior_runtime);
    let native = manifest["schema"] == "ableton-mcp-native-release/v1"
        && distribution["channel"] == "local-native-tarball"
        && mit_policy
        && build["runtime"] == "rust-native"
        && build["target"].as_str().is_some_and(|s| !s.is_empty());
    let nonempty = |v: &Value| v.as_str().is_some_and(|s| !s.is_empty());
    let native_builder =
        ["rustc", "cargo", "platform", "architecture", "runnerImage", "runnerImageVersion"].iter().all(|key| nonempty(&builder[*key]))
            && valid_hash(&builder["cargoLockSha256"]);
    let legacy_builder =
        ["node", "npm", "typescript"].iter().all(|key| truthy(&builder[*key])) && valid_hash(&builder["packageLockSha256"]);
    let source_valid = manifest["source"]["commit"]
        .as_str()
        .is_some_and(|v| v.len() == 40 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        && manifest["source"]["dirty"].is_boolean();
    if !(legacy || current || native)
        || !roles_valid(&manifest)
        || !source_valid
        || package["name"] != "@ableton-mcp/mcp-server"
        || package["private"] != true
        || manifest["algorithm"] != "sha256"
        || ["published", "signed", "notarized"].iter().any(|key| distribution[*key] != false)
        || !valid_hash(&manifest["protocol"]["registryHash"])
        || !truthy(&build["recipe"])
        || !(if native { native_builder } else { legacy_builder })
        || !valid_hash(&builder["workflowSha256"])
        || files.get("release-manifest.json").is_some()
        || !valid_hash(&files["package.json"])
        || !valid_hash(&files["LICENSE.md"])
        || files.as_object().is_none_or(|f| f.len() < 10)
    {
        return Err(fail("release manifest policy is invalid"));
    }
    let metadata: Value = serde_json::from_slice(&read(&package_root.join("package.json"))?)?;
    let native_binary =
        if build["target"].as_str().unwrap_or("").contains("windows") { "ableton-mcp-server.exe" } else { "ableton-mcp-server" };
    let native_worker = if build["target"].as_str().unwrap_or("").contains("windows") {
        "ableton-mcp-analysis-worker.exe"
    } else {
        "ableton-mcp-analysis-worker"
    };
    let native_metadata_valid = metadata["runtime"] == "rust-native"
        && metadata["target"] == build["target"]
        && metadata["bin"]["ableton-mcp-server"] == native_binary
        && metadata["bin"]["ableton-mcp-analysis-worker"] == native_worker
        && valid_hash(&files[native_binary])
        && manifest["roles"][native_binary] == "native-runtime"
        && valid_hash(&files[native_worker])
        && manifest["roles"][native_worker] == "native-runtime"
        && manifest["roles"].as_object().unwrap().values().filter(|role| *role == "native-runtime").count() == 2;
    if ["name", "version", "license", "private"].iter().any(|key| metadata[*key] != package[*key])
        || (current
            && (metadata["engines"]["node"] != build["nodeRange"] || metadata["abletonMcpSupport"]["nodeMajors"] != build["nodeMajors"]))
        || (native && !native_metadata_valid)
    {
        return Err(fail("package metadata and release manifest policy disagree"));
    }
    if manifest["source"]["dirty"] == true && !allow_dirty {
        return Err(fail("release manifest identifies a dirty source tree; only explicit local development testing may override this"));
    }
    let manifest_sha256 = file_digest(&manifest_path)?;
    let files = files.as_object().unwrap();
    let mut expected_names = files.keys().cloned().collect::<BTreeSet<_>>();
    expected_names.insert("release-manifest.json".into());
    let current_files = hash_regular_tree(package_root)?;
    if current_files.keys().cloned().collect::<BTreeSet<_>>() != expected_names {
        return Err(fail("extracted package root inventory differs from the strict release manifest"));
    }
    let mut expected_directories = BTreeSet::new();
    for name in &expected_names {
        let parts = name.split('/').collect::<Vec<_>>();
        for i in 1..parts.len() {
            expected_directories.insert(parts[..i].join("/"));
        }
    }
    fn inspect(root: &Path, directory: &Path, expected: &BTreeSet<String>) -> Result<(), LiveError> {
        let mut paths = std::fs::read_dir(directory)
            .map_err(|e| io_error(&e, "scandir", &[directory]))?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| fail(e.to_string()))?;
        paths.sort();
        for path in paths {
            if !lstat(&path)?.is_dir() {
                continue;
            }
            let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            if !expected.contains(&relative) {
                return Err(fail(format!("extracted package root contains an unknown directory: {relative}")));
            }
            inspect(root, &path, expected)?;
        }
        Ok(())
    }
    inspect(package_root, package_root, &expected_directories)?;
    for (name, digest) in files {
        if name.contains("..") || name.starts_with('/') || !valid_hash(digest) {
            return Err(fail("release manifest contains an unsafe file entry"));
        }
        // The inventory above read and hashed every file already.
        if current_files.get(name) != Some(digest) {
            return Err(fail(format!("release payload hash mismatch: {name}")));
        }
    }
    Ok(ReleaseEvidence { manifest, manifest_sha256 })
}
// Node parseInt(text, 8) permits a signed numeric prefix followed by non-octal text.
fn octal(text: &str) -> Option<u64> {
    let text = kumi_common::js::string::trim(text);
    let (negative, text) = if let Some(s) = text.strip_prefix('-') { (true, s) } else { (false, text.strip_prefix('+').unwrap_or(text)) };
    let digits = text.bytes().take_while(|b| (b'0'..=b'7').contains(b)).collect::<Vec<_>>();
    if digits.is_empty() {
        return None;
    }
    let mut value = 0u64;
    for b in digits {
        value = value.checked_mul(8)?.checked_add((b - b'0') as u64)?;
    }
    if value > 9_007_199_254_740_991 || negative && value != 0 {
        None
    } else {
        Some(value)
    }
}
pub fn verify_artifact_binding(
    artifact_path: Option<&Path>,
    expected_sha256: Option<&str>,
    package_root: &Path,
    release_manifest_sha256: &str,
) -> Result<String, LiveError> {
    let Some(path) = artifact_path.filter(|p| !p.as_os_str().is_empty()) else {
        return Err(fail("the exact local npm tarball path is required"));
    };
    validate_absolute_path(path, "artifact path")?;
    assert_no_linked_ancestors(path)?;
    let entry = lstat(path)?;
    if !entry.is_file() || entry.file_type().is_symlink() {
        return Err(fail("artifact must be a regular local file"));
    }
    if entry.len() < 1 || entry.len() > MAX_ARTIFACT_BYTES {
        return Err(fail("artifact exceeds the bounded compressed size"));
    }
    let compressed = read(path)?;
    let artifact_sha256 = sha256(&compressed);
    if expected_sha256.is_none_or(|expected| !valid_hash(&json!(expected)) || expected != artifact_sha256) {
        return Err(fail("artifact SHA-256 does not match the exact tarball bytes"));
    }
    let mut tar = Vec::new();
    let mut remaining = compressed.as_slice();
    loop {
        // gunzipSync accepts concatenated gzip members and ignores padding after
        // a zero byte following a complete member. Each member shares one limit.
        let mut decoder = flate2::bufread::GzDecoder::new(remaining);
        let limit = MAX_ARTIFACT_TAR_BYTES + 1 - tar.len() as u64;
        let decoded = decoder.by_ref().take(limit).read_to_end(&mut tar);
        if tar.len() as u64 > MAX_ARTIFACT_TAR_BYTES {
            return Err(fail("artifact exceeds the bounded decompressed size"));
        }
        if decoded.is_err() {
            return Err(fail("artifact is not a valid gzip-compressed npm tarball"));
        }
        remaining = decoder.into_inner();
        if remaining.is_empty() || remaining[0] == 0 {
            break;
        }
    }
    let mut tar_files = BTreeMap::<String, &[u8]>::new();
    let mut terminated = false;
    let mut offset = 0usize;
    while offset + 512 <= tar.len() {
        let header = &tar[offset..offset + 512];
        if header.iter().all(|b| *b == 0) {
            if tar[offset..].iter().any(|b| *b != 0) {
                return Err(fail("artifact tarball has non-zero trailing content"));
            }
            terminated = true;
            break;
        }
        let text = |start: usize, length: usize| {
            let bytes = &header[start..start + length];
            String::from_utf8_lossy(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())]).into_owned()
        };
        let stored = octal(&text(148, 8));
        let computed = header.iter().enumerate().map(|(i, b)| if (148..156).contains(&i) { 32 } else { *b as u64 }).sum::<u64>();
        if stored != Some(computed) {
            return Err(fail("artifact tar header checksum is invalid"));
        }
        let name = text(0, 100);
        let prefix = text(345, 155);
        let name = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let size_text = text(124, 12);
        let size = if kumi_common::js::string::trim(&size_text).is_empty() { Some(0) } else { octal(&size_text) };
        let Some(size) = size.filter(|size| {
            *size <= usize::MAX as u64
                && *size <= tar.len().saturating_sub(offset + 512) as u64
                && name.starts_with("package/")
                && !name.contains("..")
                && !name.contains('\\')
        }) else {
            return Err(fail("artifact tar header or path is malformed"));
        };
        let size = size as usize;
        if header[156] != 0 && header[156] != b'0' {
            return Err(fail(format!("artifact contains a non-regular entry: {name}")));
        }
        if tar_files.insert(name.clone(), &tar[offset + 512..offset + 512 + size]).is_some() {
            return Err(fail(format!("artifact contains a duplicate entry: {name}")));
        }
        offset += 512 + size.div_ceil(512) * 512;
    }
    if !terminated {
        return Err(fail("artifact tarball has no valid end marker"));
    }
    let manifest_bytes = tar_files.get("package/release-manifest.json");
    if manifest_bytes.is_none_or(|b| sha256(b) != release_manifest_sha256) {
        return Err(fail("artifact tarball and extracted package root have different release manifests"));
    }
    let embedded: Value = serde_json::from_slice(manifest_bytes.unwrap())?;
    if !valid_hash(&embedded["files"]["package.json"]) {
        return Err(fail("artifact release manifest does not bind package metadata"));
    }
    let files = embedded["files"].as_object().ok_or_else(|| fail("artifact release manifest does not bind package metadata"))?;
    let expected = std::iter::once("package/release-manifest.json".to_owned())
        .chain(files.keys().map(|name| format!("package/{name}")))
        .collect::<BTreeSet<_>>();
    if tar_files.keys().cloned().collect::<BTreeSet<_>>() != expected {
        return Err(fail("artifact inventory differs from the strict embedded release manifest"));
    }
    for (name, digest) in files {
        if json!(sha256(tar_files.get(&format!("package/{name}")).copied().unwrap_or(b""))) != *digest {
            return Err(fail(format!("artifact payload hash mismatch: {name}")));
        }
    }
    let Some(embedded_package) = tar_files.get("package/package.json") else {
        return Err(fail("artifact package metadata differs from the extracted package root"));
    };
    if sha256(embedded_package) != file_digest(&package_root.join("package.json"))? {
        return Err(fail("artifact package metadata differs from the extracted package root"));
    }
    Ok(artifact_sha256)
}
pub(super) fn assert_package_still_bound(root: &Path, expected: &ReleaseEvidence, allow_dirty: bool) -> Result<(), LiveError> {
    let current = verify_release_package(root, allow_dirty)?;
    if current.manifest_sha256 != expected.manifest_sha256 {
        return Err(fail("package root changed during lifecycle operation"));
    }
    Ok(())
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(n) => n.as_f64().is_some_and(|v| v != 0. && !v.is_nan()),
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}
