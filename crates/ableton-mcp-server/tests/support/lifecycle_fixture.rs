#![allow(dead_code)]
use ableton_mcp_server::{delivery::*, lifecycle::*};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};
pub fn sha(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}
pub fn read(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
pub fn write(path: impl AsRef<Path>, value: &Value) {
    fs::write(path, kumi_common::js::json::file_text(value)).unwrap();
}
pub fn chmod(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}
pub fn tar(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, content) in files {
        let mut header = [0u8; 512];
        fn field(header: &mut [u8], offset: usize, size: usize, value: &[u8]) {
            header[offset..offset + value.len().min(size)].copy_from_slice(&value[..value.len().min(size)]);
        }
        field(&mut header, 0, 100, name.as_bytes());
        field(&mut header, 100, 8, b"0000600\0");
        field(&mut header, 108, 8, b"0000000\0");
        field(&mut header, 116, 8, b"0000000\0");
        field(&mut header, 124, 12, format!("{:011o}\0", content.len()).as_bytes());
        field(&mut header, 136, 12, b"00000000000\0");
        header[148..156].fill(b' ');
        header[156] = b'0';
        field(&mut header, 257, 6, b"ustar\0");
        field(&mut header, 263, 2, b"00");
        let sum = header.iter().map(|b| *b as u64).sum::<u64>();
        field(&mut header, 148, 8, format!("{sum:06o}\0 ").as_bytes());
        tar.extend_from_slice(&header);
        tar.extend_from_slice(content);
        tar.resize(tar.len() + content.len().div_ceil(512) * 512 - content.len(), 0);
    }
    tar.resize(tar.len() + 1024, 0);
    tar
}
pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}
pub fn bind(root: &Path) -> (PathBuf, String) {
    let manifest = read(root.join("release-manifest.json"));
    let mut files = BTreeMap::new();
    for name in manifest["files"].as_object().unwrap().keys() {
        files.insert(format!("package/{name}"), fs::read(root.join(name)).unwrap());
    }
    files.insert("package/release-manifest.json".into(), fs::read(root.join("release-manifest.json")).unwrap());
    let path = root.with_extension("tgz");
    fs::write(&path, gzip(&tar(&files))).unwrap();
    let hash = sha(fs::read(&path).unwrap());
    (path, hash)
}
pub fn package(parent: &Path, version: &str, policy: &str) -> (PathBuf, PathBuf, String) {
    package_with(parent, version, policy, &[])
}
/// A release package with more files in it, by their path in the package.
pub fn package_with(parent: &Path, version: &str, policy: &str, extra: &[(&str, &[u8])]) -> (PathBuf, PathBuf, String) {
    let root = parent.join(format!("candidate {version} ü {policy}"));
    let legacy = policy == "legacy";
    let native = policy == "native";
    let license = if legacy { "UNLICENSED" } else { "MIT" };
    let majors = if policy == "node25" { json!([22, 24, 25]) } else { json!([22, 24]) };
    let range = if policy == "node25" { ">=22 <23 || >=24 <25 || >=25 <26" } else { NODE_ENGINE_RANGE };
    let target = if cfg!(windows) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(target_os = "macos") {
        "aarch64-apple-darwin"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let binary = if cfg!(windows) { "ableton-mcp-server.exe" } else { "ableton-mcp-server" };
    let worker = if cfg!(windows) { "ableton-mcp-analysis-worker.exe" } else { "ableton-mcp-analysis-worker" };
    let mut metadata = json!({"name":"@ableton-mcp/mcp-server","version":version,"private":true,"license":license,"type":"module"});
    if native {
        metadata["runtime"] = "rust-native".into();
        metadata["target"] = target.into();
        metadata["bin"] = json!({"ableton-mcp-server":binary,"ableton-mcp-analysis-worker":worker});
    } else if !legacy {
        metadata["engines"] = json!({"node":range});
        metadata["abletonMcpSupport"] = json!({"nodeMajors":majors});
    }
    let mut files = BTreeMap::<String, Vec<u8>>::new();
    files.insert("package.json".into(), kumi_common::js::json::file_text(&metadata).into_bytes());
    files.insert(
        "LICENSE.md".into(),
        if legacy { b"# Legacy private license notice\n".to_vec() } else { include_bytes!("../../../../LICENSE.md").to_vec() },
    );
    files.insert(if native { binary } else { "dist/src/cli.js" }.into(), format!("fixture artifact payload {version}\n").into_bytes());
    if native {
        files.insert(worker.into(), format!("fixture worker payload {version}\n").into_bytes());
    }
    files.insert("remote-script/AbletonMcpBridge/__init__.py".into(), b"def create_instance(c_instance):\n    return None\n".to_vec());
    files.insert(
        "remote-script/AbletonMcpBridge/ableton_mcp_remote_script.py".into(),
        format!("class AbletonMcpBridge:\n    marker = '{version}'\n").into_bytes(),
    );
    for i in 0..9 {
        files.insert(format!("release-docs/doc-{i}.md"), format!("# {version} {i}\n").into_bytes());
    }
    for (name, bytes) in extra {
        files.insert(name.to_string(), bytes.to_vec());
    }
    for (name, bytes) in &files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let build = if native {
        json!({"runtime":"rust-native","target":target,"recipe":"test fixture","builder":{"rustc":"fixture","cargo":"fixture","platform":"fixture","architecture":"fixture","runnerImage":"fixture","runnerImageVersion":"fixture","cargoLockSha256":"a".repeat(64),"workflowSha256":"b".repeat(64)}})
    } else {
        json!({"runtime":"TypeScript compiled JavaScript","nodeRange":if legacy{">=22 <26"}else{range},"nodeMajors":majors,"recipe":"test fixture","builder":{"node":"fixture","npm":"fixture","typescript":"fixture","packageLockSha256":"a".repeat(64),"workflowSha256":"b".repeat(64)}})
    };
    let manifest = json!({"schema":if native{"ableton-mcp-native-release/v1"}else if legacy{"ableton-mcp-private-release/v1"}else{"ableton-mcp-release/v2"},"package":{"name":"@ableton-mcp/mcp-server","version":version,"license":license,"private":true},"source":{"commit":&sha(version)[..40],"dirty":true},"build":build,"protocol":{"registryHash":registry_digest()},"distribution":{"channel":if native{"local-native-tarball"}else if legacy{"private-local-npm-tarball"}else{"local-npm-tarball"},"published":false,"signed":false,"notarized":false,"integrityIsIdentityProof":false},"algorithm":"sha256","files":files.iter().map(|(name,bytes)|(name.clone(),json!(sha(bytes)))).collect::<serde_json::Map<_,_>>(),"roles":files.keys().map(|name|(name.clone(),json!(if name=="LICENSE.md"{if legacy{"private-license"}else{"license"}}else if name=="package.json"{"package-metadata"}else if name==binary||native&&name==worker{"native-runtime"}else if name.starts_with("dist/src/"){"compiled-runtime"}else if name.starts_with("release-docs/"){"documentation"}else{"ableton-remote-script"}))).collect::<serde_json::Map<_,_>>()});
    write(root.join("release-manifest.json"), &manifest);
    let (path, hash) = bind(&root);
    (root, path, hash)
}
pub struct Fixture {
    pub folder: tempfile::TempDir,
    pub options: LifecycleOptions,
    /// More files in every release package this fixture makes.
    pub extra: Vec<(&'static str, &'static [u8])>,
}
impl Fixture {
    pub fn new() -> Self {
        Self::with_files(&[])
    }
    pub fn with_files(extra: &[(&'static str, &'static [u8])]) -> Self {
        let folder = tempfile::tempdir().unwrap();
        let (root, artifact, hash) = package_with(folder.path(), "1.0.0", "native", extra);
        let remote = folder.path().join("Live Remote Scripts ü");
        fs::create_dir(&remote).unwrap();
        let port = free_port();
        let mut realtime = free_port();
        while realtime == port {
            realtime = free_port();
        }
        Self {
            options: LifecycleOptions {
                action: "install".into(),
                package_root: root,
                state_directory: folder.path().join("State ü space"),
                remote_scripts_directory: remote,
                artifact_path: Some(artifact),
                artifact_sha256: Some(hash),
                host: Some("127.0.0.1".into()),
                port: Some(port as f64),
                realtime_port: Some(realtime as f64),
                timeout_ms: Some(100.),
                apply: true,
                confirm_live_stopped: true,
                allow_dirty_private_build: true,
                ..Default::default()
            },
            folder,
            extra: extra.to_vec(),
        }
    }
    pub fn action(&self, action: &str) -> LifecycleOptions {
        LifecycleOptions { action: action.into(), ..self.options.clone() }
    }
    pub fn receipt_path(&self) -> PathBuf {
        self.options.state_directory.join("install-receipt.json")
    }
    pub fn receipt(&self) -> Value {
        read(self.receipt_path())
    }
    pub fn remote(&self) -> PathBuf {
        self.options.remote_scripts_directory.join(REMOTE_SCRIPT_PACKAGE)
    }
    pub fn upgrade(&self, version: &str) -> LifecycleOptions {
        let (root, artifact, hash) = package_with(self.folder.path(), version, "native", &self.extra);
        LifecycleOptions {
            action: "upgrade".into(),
            package_root: root,
            artifact_path: Some(artifact),
            artifact_sha256: Some(hash),
            ..self.options.clone()
        }
    }
    pub fn legacy_receipt(&self, policy: &str) {
        let (root, artifact, hash) = package(self.folder.path(), "0.9.0", policy);
        let mut r = self.receipt();
        let mut config = r["config"].clone();
        config["server"] = json!({"command":if cfg!(windows){"C:\\Program Files\\nodejs\\node.exe"}else{"/usr/bin/node"},"args":[root.join("dist/src/cli.js"),"--config",r["configPath"]]});
        write_config(Path::new(r["configPath"].as_str().unwrap()), &config, true).unwrap();
        let manifest = read(root.join("release-manifest.json"));
        r["packageRoot"] = json!(root);
        r["packageVersion"] = manifest["package"]["version"].clone();
        r["artifactSha256"] = json!(hash);
        r["releaseManifestSha256"] = json!(sha(fs::read(root.join("release-manifest.json")).unwrap()));
        r["config"] = config;
        r["configSha256"] = json!(sha(fs::read(r["configPath"].as_str().unwrap()).unwrap()));
        write(self.receipt_path(), &r);
        let _ = artifact;
    }
}
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}
