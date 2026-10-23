//! Running programs for a DAW's steps and the app's: a command's output (this computer's, or a test's), where Kumi's
//! executables are, the checkout a build was made from, and copying a folder.
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::abort::Signal;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::Path,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncReadExt;
/// Asks something that answers yes or no later (whether an app is running, say).
pub type AsyncBool = Rc<dyn Fn() -> LocalBoxFuture<'static, bool>>;
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ran {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}
pub type Run = Rc<dyn Fn(String, Vec<String>, Option<String>) -> LocalBoxFuture<'static, Ran>>;
pub fn default_run() -> Run {
    Rc::new(|command, args, cwd| async move { run_program(&command, &args, cwd.as_deref()).await }.boxed_local())
}
/// Run one command, preserving partial output on timeout and limiting each output stream to 16 MiB.
pub async fn run_program(command: &str, args: &[String], cwd: Option<&str>) -> Ran {
    let mut process = tokio::process::Command::new(command);
    process.args(args);
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }
    process.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    process.creation_flags(0x0800_0000);
    let Ok(mut child) = process.spawn() else { return Ran { code: 1, ..Default::default() } };
    let stdout = Arc::new(Mutex::new(vec![]));
    let stderr = Arc::new(Mutex::new(vec![]));
    let full = Signal::new();
    async fn read(mut source: impl tokio::io::AsyncRead + Unpin, data: Arc<Mutex<Vec<u8>>>, full: Signal) {
        let mut buffer = [0; 65536];
        loop {
            let Ok(n) = source.read(&mut buffer).await else { return };
            if n == 0 {
                return;
            }
            let mut data = data.lock().unwrap();
            let left = (16 * 1024 * 1024usize).saturating_sub(data.len());
            data.extend_from_slice(&buffer[..n.min(left)]);
            if n > left {
                full.cancel();
                return;
            }
        }
    }
    let out = tokio::spawn(read(child.stdout.take().unwrap(), stdout.clone(), full.clone()));
    let err = tokio::spawn(read(child.stderr.take().unwrap(), stderr.clone(), full.clone()));
    let status = tokio::select! {status=child.wait()=>status.ok(),_=full.cancelled()=>{let _=child.kill().await;None},_=tokio::time::sleep(Duration::from_secs(600))=>{let _=child.kill().await;None}};
    let _ = tokio::join!(out, err);
    let result = Ran {
        code: status.and_then(|s| s.code()).unwrap_or(1),
        stdout: String::from_utf8_lossy(&stdout.lock().unwrap()).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.lock().unwrap()).into_owned(),
    };
    result
}
pub fn executable_name(name: &str) -> String {
    format!("{name}{}", if cfg!(windows) { ".exe" } else { "" })
}
pub fn executable_dir() -> String {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.display().to_string())).unwrap_or(".".into())
}
/// The checkout this build was made from; for a release's, a folder that isn't there (this crate's own, where it was
/// built), so that what's looked for in it isn't found.
pub fn repository_dir() -> String {
    kumi_common::dev::checkout()
        .and_then(|checkout| checkout.canonicalize().ok())
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf())
        .display()
        .to_string()
}
pub async fn copy_tree(from: &str, to: &str) -> std::io::Result<()> {
    let from = from.to_string();
    let to = to.to_string();
    tokio::task::spawn_blocking(move || {
        let mut stack = vec![(std::path::PathBuf::from(from), std::path::PathBuf::from(to))];
        while let Some((from, to)) = stack.pop() {
            let metadata = fs::symlink_metadata(&from)?;
            if metadata.is_symlink() {
                return Err(std::io::Error::other("native bridge packages cannot contain symbolic links"));
            }
            if metadata.is_dir() {
                fs::create_dir_all(&to)?;
                for entry in fs::read_dir(&from)? {
                    let entry = entry?;
                    stack.push((entry.path(), to.join(entry.file_name())));
                }
            } else if metadata.is_file() {
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&from, &to)?;
            } else {
                return Err(std::io::Error::other("native bridge packages cannot contain special files"));
            }
        }
        Ok(())
    })
    .await
    .map_err(std::io::Error::other)?
}

/// The version a crate's `Cargo.toml` gives its package (a checkout's build of the bridge says its version so).
pub fn read_cargo_version(file: &str) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    let package = text.split("[package]").nth(1)?.split("\n[").next()?;
    package.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "version").then(|| value.trim().trim_matches('"').to_string())
    })
}
