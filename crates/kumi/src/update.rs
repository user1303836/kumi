//! `kumi update` for a checkout: pulls, rebuilds and launches native binaries.
use crate::{
    bridge_setup::{default_run, executable_name, is_live_running, repository_dir, AsyncBool, Ran, Run},
    config::find_bridge_config,
    doctor::read_bridge_server,
    spinner::step,
    tui::tty::TtyOutput,
};
use futures::future::LocalBoxFuture;
use kumi_common::{
    js::{
        json::stringify,
        number,
        string::{head, trim},
    },
    time::now_ms,
};
use kumi_runtime::{
    core::errors::RuntimeError,
    library::sources::{dirname, join},
    system::Env,
    KUMI, KUMI_REPAIR, KUMI_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Stdio, rc::Rc};
const DAY: f64 = 86400000.;
pub fn newer(left: &str, right: &str) -> bool {
    fn parts(text: &str) -> Vec<f64> {
        text.split(['.', '-'])
            .map(|s| {
                let s = trim(s);
                let sign = if s.starts_with('-') { -1. } else { 1. };
                let s = s.strip_prefix(['+', '-']).unwrap_or(s);
                let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
                sign * digits.parse::<f64>().unwrap_or(0.)
            })
            .collect()
    }
    let a = parts(left);
    let b = parts(right);
    for i in 0..3 {
        let a = a.get(i).copied().unwrap_or(0.);
        let b = b.get(i).copied().unwrap_or(0.);
        if a != b {
            return a > b;
        }
    }
    false
}
#[derive(Clone)]
pub struct UpdateControl {
    pub current: String,
    pub check: Rc<dyn Fn() -> LocalBoxFuture<'static, Result<Option<String>, RuntimeError>>>,
    pub request: Rc<dyn Fn()>,
}
#[derive(Clone, Default)]
pub struct CheckIo {
    pub cache_file: String,
    pub run: Option<Run>,
    pub repo_dir: Option<String>,
    pub now: Option<Rc<dyn Fn() -> f64>>,
    pub version: Option<String>,
}
async fn upstream(run: &Run, repo: &str) -> Option<String> {
    if !Path::new(&join(repo, ".git")).exists() {
        return None;
    }
    let ran =
        run("git".into(), ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"].map(str::to_string).into(), Some(repo.into()))
            .await;
    let name = trim(&ran.stdout);
    (ran.code == 0 && regex::Regex::new(r"^[A-Za-z0-9_./-]+/[A-Za-z0-9_./-]+$").unwrap().is_match(name)).then(|| name.into())
}
async fn upstream_version(run: &Run, repo: &str, branch: &str) -> Option<String> {
    let (remote, name) = branch.split_once('/')?;
    if run("git".into(), vec!["fetch".into(), "--quiet".into(), remote.into(), name.into()], Some(repo.into())).await.code != 0 {
        return None;
    }
    let shown = run("git".into(), vec!["show".into(), format!("{branch}:package.json")], Some(repo.into())).await;
    if shown.code != 0 {
        return None;
    }
    serde_json::from_str::<Value>(&shown.stdout).ok()?.get("version")?.as_str().map(str::to_string)
}
pub async fn check_checkout(io: CheckIo) -> Result<Option<String>, RuntimeError> {
    let run = io.run.unwrap_or_else(default_run);
    let repo = io.repo_dir.unwrap_or_else(repository_dir);
    let branch = upstream(&run, &repo).await.ok_or_else(|| RuntimeError::plain("this Kumi isn't a git checkout that follows a branch"))?;
    let latest = upstream_version(&run, &repo, &branch)
        .await
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RuntimeError::plain("Kumi couldn't reach the repository to ask; check your internet connection"))?;
    Ok(newer(&latest, io.version.as_deref().unwrap_or(KUMI_VERSION)).then_some(latest))
}
pub async fn newer_kumi(io: CheckIo) -> Option<String> {
    let now = io.now.as_ref().map(|now| now()).unwrap_or_else(|| now_ms() as f64);
    let current = io.version.as_deref().unwrap_or(KUMI_VERSION);
    if let Some(cached) = tokio::fs::read(&io.cache_file).await.ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
        if cached.get("checkedAt").and_then(Value::as_f64).is_some_and(|at| now - at < DAY && now >= at) {
            return cached.get("latest").and_then(Value::as_str).filter(|latest| newer(latest, current)).map(str::to_string);
        }
    }
    let run = io.run.unwrap_or_else(default_run);
    let repo = io.repo_dir.unwrap_or_else(repository_dir);
    let branch = upstream(&run, &repo).await?;
    let latest = upstream_version(&run, &repo, &branch).await.filter(|s| !s.is_empty())?;
    let mut dir = tokio::fs::DirBuilder::new();
    dir.recursive(true);
    #[cfg(unix)]
    dir.mode(0o700);
    let _ = dir.create(dirname(&io.cache_file)).await;
    let mut file = tokio::fs::OpenOptions::new();
    file.write(true).create(true).truncate(true);
    #[cfg(unix)]
    file.mode(0o600);
    if let Ok(mut file) = file.open(&io.cache_file).await {
        use tokio::io::AsyncWriteExt;
        let _ = file.write_all(stringify(&json!({"checkedAt":now,"latest":latest})).as_bytes()).await;
        let _ = file.flush().await;
    }
    newer(&latest, current).then_some(latest)
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OlderBridge {
    pub installed: String,
    pub bundled: String,
    #[serde(default)]
    pub runtime_migration: bool,
}
pub fn older_bridge(env: &Env, bundled: Option<&str>) -> Option<OlderBridge> {
    let config = find_bridge_config(env)?;
    let bundled = bundled.filter(|s| !s.is_empty())?;
    let server = read_bridge_server(&config).ok()?;
    let runtime_migration = !server.native();
    let installed = server.version?;
    (newer(bundled, &installed) || (runtime_migration && bundled == installed)).then(|| OlderBridge {
        installed,
        bundled: bundled.into(),
        runtime_migration,
    })
}
#[derive(Clone)]
pub struct UpdateIo {
    pub out: Rc<dyn TtyOutput>,
    pub env: Env,
    pub run: Option<Run>,
    pub repo_dir: Option<String>,
    pub live_running: Option<AsyncBool>,
    pub update_bridge: Option<Rc<dyn Fn(String) -> LocalBoxFuture<'static, i32>>>,
}
impl UpdateIo {
    pub fn new(out: Rc<dyn TtyOutput>, env: Env) -> Self {
        Self { out, env, run: None, repo_dir: None, live_running: None, update_bridge: None }
    }
}
fn last(ran: &Ran) -> String {
    head(trim(if ran.stderr.is_empty() { &ran.stdout } else { &ran.stderr }).split('\n').next_back().unwrap_or(""), 300)
}
pub async fn checkout_bridge(repo: &str) -> i32 {
    let target = std::env::var("CARGO_TARGET_DIR")
        .ok()
        .map(|dir| if Path::new(&dir).is_absolute() { dir } else { join(repo, &dir) })
        .unwrap_or_else(|| join(repo, "target"));
    let mut command = tokio::process::Command::new(join(&target, &format!("release/{}", executable_name("kumi"))));
    command.arg("bridge").current_dir(repo).stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    command.status().await.ok().and_then(|s| s.code()).unwrap_or(1)
}
pub async fn run_update(io: UpdateIo) -> i32 {
    let say = |s: String| io.out.write(&format!("{s}\n"));
    let run = io.run.clone().unwrap_or_else(default_run);
    let repo = io.repo_dir.clone().unwrap_or_else(repository_dir);
    let Some(branch) = upstream(&run, &repo).await else {
        say("This Kumi isn't a git checkout that follows a branch, so update it the way you installed it.".into());
        return 1;
    };
    let status = run("git".into(), ["status", "--porcelain", "--untracked-files=no"].map(str::to_string).into(), Some(repo.clone())).await;
    if status.code != 0 {
        say(format!("git couldn't read this checkout: {}", last(&status)));
        return 1;
    }
    if !trim(&status.stdout).is_empty() {
        say(format!("This checkout has changes of its own, so Kumi leaves it as it is. Commit or stash them, then run: {} update", *KUMI));
        return 1;
    }
    let (remote, name) = branch.split_once('/').unwrap();
    let fetched = step(
        io.out.clone(),
        &io.env,
        &format!("Looking for a newer Kumi on {branch}…"),
        run("git".into(), vec!["fetch".into(), "--quiet".into(), remote.into(), name.into()], Some(repo.clone())),
        true,
    )
    .await;
    if fetched.code != 0 {
        say("Couldn't reach the repository; check the network, then run update again.".into());
        return 1;
    }
    let behind = number::parse(trim(
        &run("git".into(), vec!["rev-list".into(), "--count".into(), format!("HEAD..{branch}")], Some(repo.clone())).await.stdout,
    ))
    .unwrap_or(0.);
    if behind > 0. {
        let pulled = run("git".into(), vec!["merge".into(), "--ff-only".into(), branch.clone()], Some(repo.clone())).await;
        if pulled.code != 0 {
            say(format!("This checkout has moved away from {branch}, so it can't simply move forward: {}", last(&pulled)));
            return 1;
        }
        let built = step(
            io.out.clone(),
            &io.env,
            "Installing and building (a few minutes)…",
            run("cargo".into(), ["build", "--release", "--locked", "--workspace", "--bins"].map(str::to_string).into(), Some(repo.clone())),
            true,
        )
        .await;
        if built.code != 0 {
            say(format!("Building failed: {}. Run {} to see why.", last(&built), *KUMI_REPAIR));
            return 1;
        }
    }
    let version = read_version(&join(&repo, "package.json")).unwrap_or(KUMI_VERSION.into());
    let bundled = read_cargo_version(&join(&repo, "crates/ableton-mcp-server/Cargo.toml"));
    say(if behind > 0. { format!("Kumi is now {version}.") } else { format!("Kumi is up to date ({version}).") });
    let bridge = older_bridge(&io.env, bundled.as_deref());
    if find_bridge_config(&io.env).is_none() {
        say(format!("To connect Live, quit Live, then run: {} bridge", *KUMI));
        return 0;
    }
    let Some(bridge) = bridge else {
        say("The bridge in Live is up to date.".into());
        return 0;
    };
    say(if bridge.runtime_migration {
        format!("The bridge in Live uses JavaScript ({}); this Kumi includes the native bridge ({}).", bridge.installed, bridge.bundled)
    } else {
        format!("The bridge in Live is {}; this Kumi's is {}.", bridge.installed, bridge.bundled)
    });
    if step(
        io.out.clone(),
        &io.env,
        "Checking whether Live is open…",
        async {
            if let Some(live) = &io.live_running {
                live().await
            } else {
                is_live_running(run).await
            }
        },
        false,
    )
    .await
    {
        say(format!("Quit Live (save your work first), then run: {} bridge", *KUMI));
        return 0;
    }
    if let Some(bridge) = io.update_bridge {
        bridge(repo).await
    } else {
        checkout_bridge(&repo).await
    }
}
fn read_version(file: &str) -> Option<String> {
    serde_json::from_slice::<Value>(&fs::read(file).ok()?).ok()?.get("version")?.as_str().map(str::to_string)
}
/// A Cargo.toml's `[package]` version, as written there (`version = "1.0.74"`).
fn read_cargo_version(file: &str) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    let package = text.split("[package]").nth(1)?.split("\n[").next()?;
    package.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "version").then(|| value.trim().trim_matches('"').to_string())
    })
}
