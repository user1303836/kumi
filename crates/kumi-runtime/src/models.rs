//! Kumi's own model runtime: ONNX Runtime and the models it runs, fetched the first time they're needed into
//! `models` in Kumi's folder (KUMI_HOME, or ~/.kumi), each file checked against the SHA-256 pinned here before it's
//! kept. Nothing native is built into Kumi: ONNX Runtime is loaded from that copy when a model first runs.

use crate::core::disk::free_bytes;
use kumi_common::abort::{Signal, SignalExt};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

/// A file Kumi fetches: where from, its SHA-256 (hex) and its size in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pinned {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// The ONNX Runtime Kumi runs (the last release built for Intel Macs too).
pub const RUNTIME_VERSION: &str = "1.23.2";

/// ONNX Runtime's own build for a computer (`std::env::consts` names): its archive, and the library inside it.
pub fn runtime_for(os: &str, arch: &str) -> Option<(Pinned, &'static str)> {
    let pinned = |name, url, sha256, size| Pinned { name, url, sha256, size };
    Some(match (os, arch) {
        ("macos", "aarch64") => (
            pinned(
                "onnxruntime-osx-arm64-1.23.2.tgz",
                "https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-osx-arm64-1.23.2.tgz",
                "b4d513ab2b26f088c66891dbbc1408166708773d7cc4163de7bdca0e9bbb7856",
                9_999_931,
            ),
            "libonnxruntime.1.23.2.dylib",
        ),
        ("macos", "x86_64") => (
            pinned(
                "onnxruntime-osx-x86_64-1.23.2.tgz",
                "https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-osx-x86_64-1.23.2.tgz",
                "d10359e16347b57d9959f7e80a225a5b4a66ed7d7e007274a15cae86836485a6",
                11_676_322,
            ),
            "libonnxruntime.1.23.2.dylib",
        ),
        ("linux", "x86_64") => (
            pinned(
                "onnxruntime-linux-x64-1.23.2.tgz",
                "https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-linux-x64-1.23.2.tgz",
                "1fa4dcaef22f6f7d5cd81b28c2800414350c10116f5fdd46a2160082551c5f9b",
                8_309_231,
            ),
            "libonnxruntime.so.1.23.2",
        ),
        ("linux", "aarch64") => (
            pinned(
                "onnxruntime-linux-aarch64-1.23.2.tgz",
                "https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-linux-aarch64-1.23.2.tgz",
                "7c63c73560ed76b1fac6cff8204ffe34fe180e70d6582b5332ec094810241e5c",
                7_254_068,
            ),
            "libonnxruntime.so.1.23.2",
        ),
        ("windows", "x86_64") => (
            pinned(
                "onnxruntime-win-x64-1.23.2.zip",
                "https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-win-x64-1.23.2.zip",
                "0b38df9af21834e41e73d602d90db5cb06dbd1ca618948b8f1d66d607ac9f3cd",
                78_127_794,
            ),
            "onnxruntime.dll",
        ),
        ("windows", "aarch64") => (
            pinned(
                "onnxruntime-win-arm64-1.23.2.zip",
                "https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-win-arm64-1.23.2.zip",
                "1cfe88b6435df3b5fb0e9f6bd7d6f5df1e887b6174de7f6e2a47bab956f3f168",
                78_932_411,
            ),
            "onnxruntime.dll",
        ),
        _ => return None,
    })
}

/// The models Kumi ships, from its own models release (each Apache-2.0, converted to ONNX; the release carries their
/// licenses and how they were made).
pub mod pinned {
    use super::Pinned;
    /// LAION-CLAP's music model (laion/larger_clap_music), its audio side (weights in 8 bits): a 10 s window's log-mel
    /// in, a 512-number embedding out. What sounds alike, by style and vibe.
    pub const CLAP: Pinned = Pinned {
        name: "clap-music-audio.onnx",
        url: "https://github.com/user1303836/kumi/releases/download/models-1/clap-music-audio.onnx",
        sha256: "fd5c88d90b1f3aff88561a0352b4a91d85174a11bed3e4e19dc2571cbd9c1489",
        size: 71_315_939,
    };
    /// AFx-Rep (csteinmetz1/afx-rep): stereo audio at 48 kHz in, its mid's and side's 512-number embeddings out. What
    /// effects a sound went through.
    pub const AFX_REP: Pinned = Pinned {
        name: "afx-rep.onnx",
        url: "https://github.com/user1303836/kumi/releases/download/models-1/afx-rep.onnx",
        sha256: "3f4933453cab9682a9bcb0612fca7eb7ee208b6a6701338cb3bd869338daf4da",
        size: 172_478_808,
    };
    /// Basic Pitch (spotify/basic-pitch): audio at 22 050 Hz in, notes, onsets and pitch contour out. Pitched parts as
    /// notes, from a reference that isn't in Live.
    pub const BASIC_PITCH: Pinned = Pinned {
        name: "basic-pitch.onnx",
        url: "https://github.com/user1303836/kumi/releases/download/models-1/basic-pitch.onnx",
        sha256: "2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec",
        size: 230_444,
    };
}

/// Where Kumi keeps them: `models` in Kumi's folder.
pub fn dir() -> PathBuf {
    std::env::var("KUMI_HOME")
        .ok()
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home::home_dir().unwrap_or_default().join(".kumi"))
        .join("models")
}

/// How a fetch tells the producer what it's doing ("Kumi is fetching…").
pub type Say<'a> = &'a dyn Fn(&str);

/// A pinned file, fetched into `to` unless it's there already: downloaded beside it, kept only when its SHA-256 is the
/// pinned one, and refused (nothing kept) when it isn't.
pub async fn fetch(pinned: &Pinned, to: &Path, what: &str, say: Say<'_>, signal: &Signal) -> Result<(), String> {
    if to.exists() {
        return Ok(());
    }
    // One fetch at a time in this Kumi: two that want the same file wait for the first, then find it there.
    let _turn = FETCHING.lock().await;
    if to.exists() {
        return Ok(());
    }
    if pinned.sha256.is_empty() {
        return Err(format!("{what} isn't published yet, so Kumi can't fetch it."));
    }
    let folder = to.parent().ok_or("Kumi has no folder to keep models in.")?;
    std::fs::create_dir_all(folder).map_err(|error| format!("Kumi couldn't make {}: {error}", folder.display()))?;
    // The download and what's unpacked from it, side by side for a moment, and room to spare.
    if free_bytes(folder).await.is_some_and(|free| free < 2. * pinned.size as f64 + 200e6) {
        return Err(format!(
            "Kumi needs about {} MB free on the disk with {} to fetch {what}; free some space and ask again.",
            (2 * pinned.size + 200_000_000) / 1_000_000,
            folder.display()
        ));
    }
    sweep_partials(folder);
    say(&format!("Kumi is fetching {what} (once, about {} MB).", (pinned.size / 1_000_000).max(1)));
    let partial = partial_beside(to);
    // A body longer than the pinned file isn't it: stopped as soon as it's longer.
    let fetched = download_at_most(pinned.url, &partial, Some(pinned.size), signal).await;
    let digest = match fetched {
        Ok(digest) => digest,
        Err(why) => {
            let _ = std::fs::remove_file(&partial);
            return Err(format!("Kumi couldn't fetch {what}: {why}"));
        }
    };
    if !digest.eq_ignore_ascii_case(pinned.sha256) {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("The {what} Kumi downloaded didn't match its checksum, so it wasn't kept."));
    }
    keep(&partial, to).map_err(|error| format!("Kumi couldn't keep {what}: {error}"))
}

/// One fetch at a time in this Kumi.
static FETCHING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A temporary name beside `to`, this Kumi's and this fetch's own (`<name>.partial-<process>-<random>`).
fn partial_beside(to: &Path) -> PathBuf {
    let tag = uuid::Uuid::new_v4().simple().to_string();
    to.with_extension(format!("partial-{}-{}", std::process::id(), &tag[..8]))
}

/// A finished download moved into place; when another Kumi got there first (the move fails, the file is there), the
/// download goes and theirs stays.
fn keep(partial: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(partial, to) {
        Ok(()) => Ok(()),
        Err(_) if to.exists() => {
            let _ = std::fs::remove_file(partial);
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::remove_file(partial);
            Err(error)
        }
    }
}

/// What a Kumi that's gone left behind in `folder`: its half-fetched files and half-unpacked folders.
fn sweep_partials(folder: &Path) {
    let Ok(listed) = std::fs::read_dir(folder) else { return };
    for entry in listed.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        let Some(rest) = name.split_once(".partial-").or_else(|| name.split_once(".unpacking-")).map(|(_, rest)| rest) else {
            continue;
        };
        let pid: Option<u32> = rest.split('-').next().and_then(|pid| pid.parse().ok());
        if pid.is_some_and(|pid| pid != std::process::id() && !crate::library::state::alive(pid as f64)) {
            let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        }
    }
}

/// The largest model Kumi fetches for a slot.
const UNPINNED_MOST: u64 = 4_000_000_000;

/// A file the producer named (a model for a slot), fetched into `to`: not pinned, as it's their own choice (it's
/// tried before it's used). Over https only, redirects included; at most 4 GB, and only with room for it on the disk.
/// Downloaded beside it and moved into place.
pub async fn fetch_unpinned(url: &str, to: &Path, signal: &Signal) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err(format!("Kumi fetches models over https only, and {url} isn't."));
    }
    if to.exists() {
        return Ok(());
    }
    let folder = to.parent().ok_or("Kumi has no folder to keep models in.")?;
    std::fs::create_dir_all(folder).map_err(|error| format!("Kumi couldn't make {}: {error}", folder.display()))?;
    let partial = to.with_extension(format!("partial-{}", std::process::id()));
    let fetched: Result<(), String> = async {
        let client = reqwest::Client::builder()
            .user_agent(format!("kumi/{}", crate::version::KUMI_VERSION))
            .https_only(true)
            .connect_timeout(std::time::Duration::from_secs(20))
            .read_timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|error| error.to_string())?;
        let mut response = tokio::select! {
            sent = client.get(url).send() => sent.map_err(|error| error.to_string())?,
            _ = signal.cancelled() => return Err("stopped".into()),
        };
        if !response.status().is_success() {
            return Err(format!("it answered {}", response.status()));
        }
        // Its size, said or counted so far, against the cap and the disk (200 MB kept spare).
        let room = free_bytes(folder).await.map_or(u64::MAX, |free| (free - 200e6).max(0.) as u64);
        let fits = |size: u64| {
            if size > UNPINNED_MOST {
                Err(format!("it's larger than the {} GB Kumi takes for a model", UNPINNED_MOST / 1_000_000_000))
            } else if size > room {
                Err(format!(
                    "it needs {} MB or more, and the disk with {} hasn't room for it; free some space and ask again",
                    size / 1_000_000,
                    folder.display()
                ))
            } else {
                Ok(())
            }
        };
        if let Some(size) = response.content_length() {
            fits(size)?;
        }
        let mut file = std::fs::File::create(&partial).map_err(|error| error.to_string())?;
        let mut written: u64 = 0;
        loop {
            let chunk = tokio::select! {
                chunk = response.chunk() => chunk.map_err(|error| error.to_string())?,
                _ = signal.cancelled() => return Err("stopped".into()),
            };
            let Some(chunk) = chunk else { break };
            written += chunk.len() as u64;
            fits(written)?;
            std::io::Write::write_all(&mut file, &chunk).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
    .await;
    if let Err(why) = fetched {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("Kumi couldn't fetch {url}: {why}"));
    }
    std::fs::rename(&partial, to).map_err(|error| format!("Kumi couldn't keep {url}: {error}"))
}

/// A download streamed to `path`, stopped past `most` bytes: its SHA-256 (hex). A stalled connection gives up rather
/// than wait for Esc.
async fn download_at_most(url: &str, path: &Path, most: Option<u64>, signal: &Signal) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("kumi/{}", crate::version::KUMI_VERSION))
        .connect_timeout(std::time::Duration::from_secs(20))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|error| error.to_string())?;
    let mut response = client.get(url).send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("{} answered {}", url, response.status()));
    }
    let mut file = std::fs::File::create(path).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut written: u64 = 0;
    loop {
        signal.check().map_err(|_| "stopped".to_string())?;
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(|error| error.to_string())?,
            _ = signal.cancelled() => return Err("stopped".into()),
        };
        let Some(chunk) = chunk else { break };
        written += chunk.len() as u64;
        if most.is_some_and(|most| written > most) {
            return Err("it's larger than the file it should be".into());
        }
        hash.update(&chunk);
        std::io::Write::write_all(&mut file, &chunk).map_err(|error| error.to_string())?;
    }
    Ok(hex::encode(hash.finalize()))
}

static LOADED: OnceLock<Result<(), String>> = OnceLock::new();

/// ONNX Runtime ready to run models: fetched and unpacked the first time, then loaded once for this Kumi.
pub async fn runtime(say: Say<'_>, signal: &Signal) -> Result<(), String> {
    if let Some(loaded) = LOADED.get() {
        return loaded.clone();
    }
    let Some((archive, library)) = runtime_for(std::env::consts::OS, std::env::consts::ARCH) else {
        return Err("Kumi has no model runtime for this computer.".into());
    };
    let folder = dir().join(format!("onnxruntime-{RUNTIME_VERSION}"));
    let path = folder.join(library);
    if !path.exists() {
        // One setup at a time in this Kumi: a second waits, then finds the library there.
        let _turn = PREPARING.lock().await;
        if !path.exists() {
            let download = dir().join(archive.name);
            fetch(&archive, &download, "its model runtime, ONNX Runtime", say, signal).await?;
            let unpacked = unpack(&download, &folder, library).await;
            let _ = std::fs::remove_file(&download);
            unpacked?;
        }
    }
    LOADED
        .get_or_init(|| match ort::init_from(&path) {
            Ok(environment) => {
                // The runtime's own log stays out of the terminal (and the app on it), unless KUMI_TIMING asks for it.
                let log: ort::logging::LoggerFunction = std::sync::Arc::new(|level, category, _, _, message| {
                    if std::env::var("KUMI_TIMING").is_ok_and(|set| !set.is_empty()) {
                        eprintln!("[onnxruntime {level:?}] {category}: {message}");
                    }
                });
                environment.with_logger(log).commit();
                if let Ok(environment) = ort::environment::Environment::current() {
                    environment.set_log_level(ort::logging::LogLevel::Warning);
                }
                Ok(())
            }
            Err(error) => Err(format!("Kumi couldn't load its model runtime: {error}")),
        })
        .clone()
}

static PREPARING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The library out of the runtime's archive, into `folder` (tar reads the zips too; Windows has had it since 2018).
/// Only the library is unpacked: Windows' archive also holds a 380 MB debug file.
async fn unpack(archive: &Path, folder: &Path, library: &str) -> Result<(), String> {
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let scratch = folder.with_extension(format!("unpacking-{}-{}", std::process::id(), &tag[..8]));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|error| error.to_string())?;
    let result = async {
        let listed = tokio::process::Command::new("tar")
            .arg("-tf")
            .arg(archive)
            .output()
            .await
            .map_err(|error| format!("Kumi couldn't read its model runtime's archive: {error}"))?;
        let member = String::from_utf8_lossy(&listed.stdout)
            .lines()
            .map(str::trim)
            .find(|member| member.rsplit(['/', '\\']).next() == Some(library))
            .map(str::to_string)
            .ok_or("Kumi's model runtime wasn't where its archive should have it.")?;
        let output = tokio::process::Command::new("tar")
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(&scratch)
            .arg(&member)
            .output()
            .await
            .map_err(|error| format!("Kumi couldn't unpack its model runtime: {error}"))?;
        if !output.status.success() {
            return Err(format!("Kumi couldn't unpack its model runtime: {}", String::from_utf8_lossy(&output.stderr).trim()));
        }
        let found = find(&scratch, library).ok_or("Kumi's model runtime wasn't where its archive should have it.")?;
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        keep(&found, &folder.join(library)).map_err(|error| error.to_string())
    }
    .await;
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn find(folder: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(folder).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find(&path, name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|file| file.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}

/// A tensor: its shape and its numbers, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

/// The models loaded, each with when it last ran: one that hasn't run for `IDLE` is let go (AFx-Rep alone holds
/// about 0.4 GB once loaded).
static SESSIONS: Mutex<Vec<(PathBuf, ort::session::Session, std::time::Instant)>> = Mutex::new(vec![]);
const IDLE: std::time::Duration = std::time::Duration::from_secs(180);
/// Whether a thread is letting idle models go.
static REAPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Lets go of models that haven't run for a while, every half minute, until none are loaded.
fn reap() {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(30));
        let Ok(mut sessions) = SESSIONS.lock() else { return };
        sessions.retain(|(_, _, ran)| ran.elapsed() < IDLE);
        if sessions.is_empty() {
            REAPING.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
    }
}

/// Runs a model on named inputs (once the runtime is ready): its outputs by name. On a thread of its own, as a model
/// can take a while; each model is loaded once.
pub async fn run(model: &Path, inputs: Vec<(String, Tensor)>, outputs: Vec<String>) -> Result<Vec<Tensor>, String> {
    if LOADED.get().is_none_or(|loaded| loaded.is_err()) {
        return Err("Kumi's model runtime isn't ready.".into());
    }
    let model = model.to_path_buf();
    tokio::task::spawn_blocking(move || run_now(&model, inputs, &outputs)).await.map_err(|error| format!("The model stopped: {error}"))?
}

fn run_now(model: &Path, inputs: Vec<(String, Tensor)>, outputs: &[String]) -> Result<Vec<Tensor>, String> {
    let mut sessions = SESSIONS.lock().map_err(|_| "Kumi's models were left in a bad state.".to_string())?;
    let index = match sessions.iter().position(|(path, _, _)| path == model) {
        Some(index) => index,
        None => {
            let session = ort::session::Session::builder()
                .and_then(|mut builder| builder.commit_from_file(model))
                .map_err(|error| format!("Kumi couldn't load {}: {error}", model.display()))?;
            sessions.push((model.to_path_buf(), session, std::time::Instant::now()));
            if !REAPING.swap(true, std::sync::atomic::Ordering::SeqCst) {
                std::thread::spawn(reap);
            }
            sessions.len() - 1
        }
    };
    sessions[index].2 = std::time::Instant::now();
    let session = &mut sessions[index].1;
    let mut values: Vec<(String, ort::session::SessionInputValue<'static>)> = vec![];
    for (name, tensor) in inputs {
        let value = ort::value::Tensor::from_array((tensor.shape, tensor.data.into_boxed_slice())).map_err(|error| error.to_string())?;
        values.push((name, value.into()));
    }
    let results = session.run(values).map_err(|error| format!("The model couldn't run: {error}"))?;
    outputs
        .iter()
        .map(|name| {
            let value = results.get(name.as_str()).ok_or_else(|| format!("The model has no output called {name}."))?;
            let (shape, data) = value.try_extract_tensor::<f32>().map_err(|error| error.to_string())?;
            Ok(Tensor { shape: shape.iter().map(|size| *size as usize).collect(), data: data.to_vec() })
        })
        .collect()
}
