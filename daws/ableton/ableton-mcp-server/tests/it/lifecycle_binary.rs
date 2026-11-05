//! The bridge's lifecycle through its own binary, as an agent host's setup runs it (`kumi serve`'s `setup_daw`): a plan,
//! then `--apply --confirm-live-stopped`, against a temporary home whose state folder is outside the package and, for
//! the upgrade, holds the receipt of the install before. The claim is "nothing outside": every file or folder the binary
//! creates, changes or removes is under the state folder, the configuration's folder or the Remote Scripts folder it's
//! given (the package root included among what it must leave alone), and a plan changes nothing at all.
#[path = "../support/lifecycle_fixture.rs"]
mod fixture;
use ableton_mcp_server::lifecycle::LifecycleOptions;
use fixture::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Every file, folder and link under `root`: a file by its bytes, a folder by a mark, a link by where it points.
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap().flatten() {
            let (path, kind) = (entry.path(), entry.file_type().unwrap());
            if kind.is_symlink() {
                found.insert(path.clone(), std::fs::read_link(&path).unwrap().display().to_string().into_bytes());
            } else if kind.is_dir() {
                found.insert(path.clone(), b"<folder>".to_vec());
                folders.push(path);
            } else {
                found.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    found
}

/// The binary's arguments for `options`, as setup gives them: the action, the folders, the artifact, the configuration
/// and secret when there are, loopback ports free now; with `apply`, `--apply --confirm-live-stopped`.
fn arguments(options: &LifecycleOptions, apply: bool) -> Vec<String> {
    let path = |path: &Path| path.display().to_string();
    let mut args = vec![
        "lifecycle".to_string(),
        options.action.clone(),
        "--remote-scripts-dir".into(),
        path(&options.remote_scripts_directory),
        "--state-dir".into(),
        path(&options.state_directory),
        "--package-root".into(),
        path(&options.package_root),
        "--artifact".into(),
        path(options.artifact_path.as_ref().unwrap()),
        "--artifact-sha256".into(),
        options.artifact_sha256.clone().unwrap(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        options.port.unwrap().to_string(),
        "--realtime-port".into(),
        options.realtime_port.unwrap().to_string(),
        // The fixture's packages are built from a working tree, as only local development's are.
        "--allow-dirty-private-build".into(),
    ];
    for (flag, value) in [("--config", &options.config_path), ("--secret", &options.secret_path)] {
        if let Some(value) = value {
            args.extend([flag.to_string(), path(value)]);
        }
    }
    if apply {
        args.extend(["--apply".into(), "--confirm-live-stopped".into()]);
    }
    args
}

/// Runs the binary for `options`, planned then applied, and checks what changed under `root` each time.
fn plan_then_apply(root: &Path, options: &LifecycleOptions) {
    let allowed: Vec<PathBuf> = [Some(options.state_directory.clone()), Some(options.remote_scripts_directory.clone())]
        .into_iter()
        .chain([options.config_path.as_ref().and_then(|config| config.parent().map(Path::to_path_buf))])
        .flatten()
        .collect();
    for apply in [false, true] {
        let before = tree(root);
        let ran = Command::new(env!("CARGO_BIN_EXE_ableton-mcp-server")).args(arguments(options, apply)).output().unwrap();
        let said = format!("{}{}", String::from_utf8_lossy(&ran.stdout), String::from_utf8_lossy(&ran.stderr));
        assert!(ran.status.success(), "{} (apply: {apply}): {said}", options.action);
        let after = tree(root);
        let changed: Vec<&PathBuf> = before.keys().chain(after.keys()).filter(|path| before.get(*path) != after.get(*path)).collect();
        if !apply {
            assert!(changed.is_empty(), "{}'s plan changed {changed:?}", options.action);
            continue;
        }
        assert!(!changed.is_empty(), "{} changed nothing", options.action);
        for path in changed {
            assert!(
                allowed.iter().any(|folder| path.starts_with(folder)),
                "{} changed {} outside {allowed:?}",
                options.action,
                path.display()
            );
        }
    }
}

#[test]
fn the_binarys_plan_changes_nothing_and_its_install_and_upgrade_change_only_the_folders_they_are_given() {
    let f = Fixture::new();
    let root = f.folder.path();
    plan_then_apply(root, &f.options);
    assert!(f.receipt_path().is_file(), "the install's receipt, in its state folder");
    // An upgrade of that install, as setup runs it with the receipt's own folders: its configuration and secret.
    let state = f.options.state_directory.clone();
    let mut upgrade = f.upgrade("1.0.1");
    upgrade.config_path = Some(state.join("bridge-config.json"));
    upgrade.secret_path = Some(state.join("bridge.secret"));
    plan_then_apply(root, &upgrade);
    assert_eq!(f.receipt()["packageVersion"], "1.0.1");
}
