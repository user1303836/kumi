//! A release-like folder of the Logic bridge, with its already built helper. Provider setup never compiles Swift
//! or Rust: a checkout builds those explicitly, then this stages their existing files for the shared lifecycle.
use daw_bridge::{
    detect::Version,
    install::{LOGIC_BINARY, LOGIC_BRIDGE_FOLDER, LOGIC_HELPER_PREFIX, LOGIC_VERSION_FILE},
};
use kumi_daw_own::provider::Own;
use kumi_types::system::Env;
use std::path::{Path, PathBuf};

pub const LOGIC_BRIDGE_SOURCE_VAR: &str = "KUMI_LOGIC_BRIDGE_SOURCE";

fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}
fn staging() -> PathBuf {
    checkout().join("target/logic-bridge").join(LOGIC_BRIDGE_FOLDER)
}

fn helper_in(directory: &Path) -> Option<PathBuf> {
    let mut helpers: Vec<PathBuf> = std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file()) && entry.file_name().to_string_lossy().starts_with(LOGIC_HELPER_PREFIX)
        })
        .map(|entry| entry.path())
        .collect();
    helpers.sort_by_key(|helper| std::fs::metadata(helper).and_then(|meta| meta.modified()).ok());
    helpers.pop()
}

fn valid(directory: &Path) -> bool {
    let regular = |path: PathBuf| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file());
    regular(directory.join(LOGIC_BINARY)) && regular(directory.join(LOGIC_VERSION_FILE)) && helper_in(directory).is_some()
}

pub(super) fn built(_: &Path) -> Option<PathBuf> {
    let staged = staging();
    valid(&staged).then_some(staged)
}

pub(super) fn source(own: &Own, env: &Env) -> Option<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    if let Some(carried) = beside.as_deref().map(|beside| beside.join("daws/logic").join((own.carried)())).filter(|dir| valid(dir)) {
        return Some(carried);
    }
    if let Some(named) = env.get(LOGIC_BRIDGE_SOURCE_VAR).filter(|named| !named.is_empty()) {
        let named = PathBuf::from(named);
        return valid(&named).then_some(named);
    }
    stage_existing(beside.as_deref(), env).or_else(|| built(Path::new("")))
}

fn bridge_version(binary: &Path) -> Option<String> {
    use std::{
        io::Read,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let mut child = Command::new(binary).arg("--version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let began = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if began.elapsed() < Duration::from_secs(2) => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut bytes = Vec::new();
    child.stdout.take()?.take(4096).read_to_end(&mut bytes).ok()?;
    String::from_utf8_lossy(&bytes).split_whitespace().rev().find_map(|word| Version::parse(word).map(|_| word.to_owned()))
}

fn stage_existing(beside: Option<&Path>, env: &Env) -> Option<PathBuf> {
    let target =
        env.get("CARGO_TARGET_DIR").filter(|target| !target.is_empty()).map(PathBuf::from).unwrap_or_else(|| checkout().join("target"));
    let binary = beside
        .into_iter()
        .flat_map(|beside| [beside.join(LOGIC_BINARY), beside.parent().unwrap_or(beside).join(LOGIC_BINARY)])
        .chain([target.join("release").join(LOGIC_BINARY), target.join("debug").join(LOGIC_BINARY)])
        .find(|path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()))?;
    let helper = helper_in(&target.join("logic-hands"))?;
    let version = bridge_version(&binary)?;
    let staged = staging();
    if staged.exists() {
        std::fs::remove_dir_all(&staged).ok()?;
    }
    std::fs::create_dir_all(&staged).ok()?;
    std::fs::copy(binary, staged.join(LOGIC_BINARY)).ok()?;
    std::fs::copy(&helper, staged.join(helper.file_name()?)).ok()?;
    std::fs::write(staged.join(LOGIC_VERSION_FILE), format!("{version}\n")).ok()?;
    Some(staged)
}
