//! The files the lifecycle writes for a bridge (the secret, the reference) and the discovery files bridges write back.
use crate::auth::{sha256_hex, Secret};
use crate::daw::Daw;
use crate::owner;
use crate::wire::{Project, PROTOCOL};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const REFERENCE_SCHEMA: &str = "kumi-daw-reference/1";
pub const DISCOVERY_SCHEMA: &str = "kumi-daw-discovery/1";
/// Larger reference or discovery files aren't read.
pub const MAX_SMALL_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub enum FileError {
    Io(std::io::Error),
    Invalid(String),
    /// Unix: group or others can read it, or it isn't the current user's.
    NotOwnerOnly,
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::Io(error) => error.fmt(f),
            FileError::Invalid(why) => f.write_str(why),
            FileError::NotOwnerOnly => f.write_str("other accounts can read it"),
        }
    }
}

impl std::error::Error for FileError {}

impl From<std::io::Error> for FileError {
    fn from(error: std::io::Error) -> Self {
        FileError::Io(error)
    }
}

fn read_small(path: &Path) -> Result<String, FileError> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return Err(FileError::Invalid("not a regular file".into()));
    }
    if meta.len() > MAX_SMALL_FILE_BYTES {
        return Err(FileError::Invalid("too large".into()));
    }
    String::from_utf8(std::fs::read(path)?).map_err(|_| FileError::Invalid("not UTF-8".into()))
}

/// Reads the secret. On unix it must be owner-only; on Windows that takes PowerShell, so the lifecycle and doctor
/// check it ([`owner::is_owner_only`]) rather than every read.
pub fn read_secret(path: &Path) -> Result<Secret, FileError> {
    let text = read_small(path)?;
    if cfg!(unix) && !owner::is_owner_only(path) {
        return Err(FileError::NotOwnerOnly);
    }
    Secret::from_file_text(&text).map_err(|error| FileError::Invalid(error.to_string()))
}

/// The secret at `path`, made (owner-only, in an owner-only folder) when there's none. One that's there but unreadable
/// or invalid is an error, never replaced: a running bridge may hold it.
pub fn ensure_secret(path: &Path) -> Result<Secret, FileError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => read_secret(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                owner::create_owner_only_dir(parent)?;
            }
            let secret = Secret::generate();
            owner::write_owner_only(path, format!("{}\n", secret.expose()).as_bytes())?;
            Ok(secret)
        }
        Err(error) => Err(error.into()),
    }
}

/// Where the bridge finds its secret and where to say it's listening; written beside the bridge's files by the
/// lifecycle. It never holds the secret itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub schema: String,
    pub daw: String,
    pub secret_file: String,
    pub discovery_dir: String,
    /// The first port to try; the DAW's range start when missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Where the bridge writes its own log, if it keeps one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<String>,
}

impl Reference {
    pub fn new(daw: Daw, paths: &crate::paths::BridgePaths) -> Self {
        Self {
            schema: REFERENCE_SCHEMA.into(),
            daw: daw.id().into(),
            secret_file: paths.secret().to_string_lossy().into_owned(),
            discovery_dir: paths.discovery().to_string_lossy().into_owned(),
            port: Some(daw.default_port()),
            log_dir: Some(paths.logs().to_string_lossy().into_owned()),
        }
    }

    pub fn read(path: &Path) -> Result<Self, FileError> {
        let reference: Reference =
            serde_json::from_str(&read_small(path)?).map_err(|error| FileError::Invalid(format!("not a reference: {error}")))?;
        if reference.schema != REFERENCE_SCHEMA {
            return Err(FileError::Invalid(format!("unknown reference schema {}", reference.schema)));
        }
        if !Path::new(&reference.secret_file).is_absolute() || !Path::new(&reference.discovery_dir).is_absolute() {
            return Err(FileError::Invalid("reference paths must be absolute".into()));
        }
        Ok(reference)
    }

    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("references serialize");
        text.push('\n');
        text
    }
}

/// A running bridge's notice of where it listens, in `<discovery folder>/<daw>-<pid>-<key>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub schema: String,
    pub daw: String,
    /// 0 when the bridge can't learn it (see [`crate::wire::Identity::pid`]).
    pub pid: u32,
    pub host: String,
    pub port: u16,
    pub protocol: String,
    /// The bridge's version.
    pub version: String,
    pub daw_version: String,
    pub resource: String,
    pub project: Option<Project>,
    pub started_ms: u64,
}

impl Discovery {
    pub fn file_name(&self) -> String {
        discovery_file_name(&self.daw, self.pid, &self.resource)
    }
}

/// The first 16 hex characters of the SHA-256 of the resource folder's path as given (UTF-8): with the pid, what
/// keeps several instances' files apart.
pub fn discovery_key(resource: &str) -> String {
    sha256_hex(resource.as_bytes())[..16].to_string()
}

pub fn discovery_file_name(daw: &str, pid: u32, resource: &str) -> String {
    format!("{daw}-{pid}-{}.json", discovery_key(resource))
}

/// Writes a bridge's discovery file whole (what the REAPER extension calls; the Renoise tool does the same in Lua).
/// The lifecycle made the folder owner-only; the file holds no secret, so it's written without Windows' ACL step,
/// which would start PowerShell from inside the DAW.
pub fn write_discovery(dir: &Path, discovery: &Discovery) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(discovery.file_name());
    let mut text = serde_json::to_string(discovery).expect("discovery serializes");
    text.push('\n');
    owner::write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

/// A discovery file found by [`scan_discovery`].
#[derive(Debug)]
pub struct Found {
    pub path: PathBuf,
    pub discovery: Result<Discovery, String>,
    /// Whether its pid still runs (as this user). A file with pid 0 (unknown) counts as alive: connect to know.
    pub alive: bool,
}

/// The discovery files of one DAW's bridges, newest first. A file whose name doesn't match its contents, or that
/// another protocol wrote, is reported as invalid, not trusted.
pub fn scan_discovery(dir: &Path, daw: Daw) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let prefix = format!("{}-", daw.id());
    let mut found: Vec<Found> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rest = name.strip_prefix(&prefix)?.strip_suffix(".json")?;
            let (pid, key) = rest.split_once('-')?;
            let pid: u32 = pid.parse().ok()?;
            if key.len() != 16 || !key.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
                return None;
            }
            let path = entry.path();
            let discovery = read_small(&path)
                .map_err(|error| error.to_string())
                .and_then(|text| serde_json::from_str::<Discovery>(&text).map_err(|error| error.to_string()))
                .and_then(|discovery| {
                    if discovery.schema != DISCOVERY_SCHEMA {
                        Err(format!("unknown schema {}", discovery.schema))
                    } else if discovery.protocol != PROTOCOL {
                        Err(format!("another protocol: {}", discovery.protocol))
                    } else if discovery.file_name() != name || discovery.daw != daw.id() {
                        Err("its name doesn't match what it says".into())
                    } else if discovery.host != "127.0.0.1" {
                        Err("not a loopback address".into())
                    } else {
                        Ok(discovery)
                    }
                });
            Some(Found { path, discovery, alive: pid == 0 || process_alive(pid) })
        })
        .collect();
    found.sort_by(|a, b| {
        let started = |found: &Found| found.discovery.as_ref().map(|d| d.started_ms).unwrap_or(0);
        started(b).cmp(&started(a)).then_with(|| a.path.cmp(&b.path))
    });
    found
}

/// Removes discovery files whose process has ended; returns how many.
pub fn prune_discovery(dir: &Path, daw: Daw) -> usize {
    scan_discovery(dir, daw).into_iter().filter(|found| !found.alive && std::fs::remove_file(&found.path).is_ok()).count()
}

/// Whether `pid` runs as this user (another user's process can't be the bridge Kumi set up), the same on every system.
pub fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else { return false };
    if pid <= 0 {
        return false;
    }
    #[cfg(unix)]
    {
        let answer = match unsafe { libc::kill(pid, 0) } {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)),
        };
        alive(Answer::Unix(answer))
    }
    #[cfg(windows)]
    {
        unsafe {
            #[link(name = "kernel32")]
            extern "system" {
                fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
                fn GetExitCodeProcess(handle: *mut std::ffi::c_void, code: *mut u32) -> i32;
                fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
                fn GetLastError() -> u32;
            }
            // PROCESS_QUERY_LIMITED_INFORMATION, which this user gets for its own processes, elevated ones too.
            let handle = OpenProcess(0x1000, 0, pid as u32);
            if handle.is_null() {
                return alive(Answer::Windows(Err(GetLastError())));
            }
            let mut code = 0;
            let answered = GetExitCodeProcess(handle, &mut code) != 0;
            let error = GetLastError();
            CloseHandle(handle);
            alive(Answer::Windows(if answered { Ok(code) } else { Err(error) }))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// What a system said about a pid: Unix's kill(pid, 0), with errno when it failed; Windows's OpenProcess, with its error
/// when it failed, else the process's exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Unix(Result<(), i32>),
    Windows(Result<u32, u32>),
}

/// Running, and this user's: kill succeeds (EPERM is another user's process, ESRCH none), or the process opens and
/// still runs (STILL_ACTIVE). ERROR_ACCESS_DENIED is another user's process on Windows, as EPERM is on Unix: not alive.
fn alive(answer: Answer) -> bool {
    match answer {
        Answer::Unix(said) => said.is_ok(),
        Answer::Windows(said) => said == Ok(259),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::BridgePaths;

    fn discovery(pid: u32, resource: &str, started_ms: u64) -> Discovery {
        Discovery {
            schema: DISCOVERY_SCHEMA.into(),
            daw: "reaper".into(),
            pid,
            host: "127.0.0.1".into(),
            port: 9770,
            protocol: PROTOCOL.into(),
            version: "0.1.0".into(),
            daw_version: "7.82/macOS-arm64".into(),
            resource: resource.into(),
            project: None,
            started_ms,
        }
    }

    #[test]
    fn secrets_are_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(dir.path(), Daw::Reaper);
        let made = ensure_secret(&paths.secret()).unwrap();
        assert_eq!(ensure_secret(&paths.secret()).unwrap(), made);
        assert_eq!(read_secret(&paths.secret()).unwrap(), made);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(owner::is_owner_only(&paths.root));
            std::fs::set_permissions(paths.secret(), std::fs::Permissions::from_mode(0o640)).unwrap();
            assert!(matches!(read_secret(&paths.secret()), Err(FileError::NotOwnerOnly)));
        }
    }

    #[test]
    fn references_round_trip_and_need_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(dir.path(), Daw::Renoise);
        let reference = Reference::new(Daw::Renoise, &paths);
        let file = dir.path().join("bridge-reference.json");
        std::fs::write(&file, reference.to_json()).unwrap();
        assert_eq!(Reference::read(&file).unwrap(), reference);
        // (The path as the JSON spells it: a Windows path's backslashes are escaped there.)
        let secret = serde_json::to_string(&paths.secret().to_string_lossy()).unwrap();
        let secret = &secret[1..secret.len() - 1];
        std::fs::write(&file, reference.to_json().replace(secret, "relative")).unwrap();
        assert!(Reference::read(&file).is_err());
    }

    /// The Renoise tool cuts a long resource folder and names the file by the text it writes (the same key in its Lua tests):
    /// a host finds the file by hashing what it reads.
    #[test]
    fn a_resource_cut_by_the_bridge_is_keyed_by_the_cut_text() {
        let cut = "/Users/K\u{fc}mi/Library/Preferences/Renoise/V3.5.4/\u{65e5}\u{65e5}\u{65e5}\u{2026}";
        assert_eq!(discovery_key(cut), "7596818ce6a714b1");
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let mut long = discovery(me, cut, 1);
        long.daw = "renoise".into();
        write_discovery(dir.path(), &long).unwrap();
        assert!(long.file_name().ends_with("-7596818ce6a714b1.json"));
        let found = scan_discovery(dir.path(), Daw::Renoise);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].discovery.as_ref().unwrap(), &long, "a file named by the cut text is trusted");
    }

    #[test]
    fn another_users_process_counts_as_not_running_on_every_system() {
        // What each system answers, read the same way: before, Windows took ERROR_ACCESS_DENIED for running.
        const EPERM: i32 = 1;
        const ESRCH: i32 = 3;
        for (answer, running) in [
            (Answer::Unix(Ok(())), true),
            (Answer::Unix(Err(EPERM)), false),
            (Answer::Unix(Err(ESRCH)), false),
            (Answer::Windows(Ok(259)), true),
            (Answer::Windows(Ok(0)), false),
            (Answer::Windows(Err(5)), false),
            (Answer::Windows(Err(87)), false),
        ] {
            assert_eq!(alive(answer), running, "{answer:?}");
        }
        // Each system's own answers: tests/system.rs, which macOS and Windows run too.
    }

    #[test]
    fn discovery_files_are_keyed_by_pid_and_resource_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let a = discovery(me, "/a/REAPER", 1);
        let b = discovery(me, "/b/REAPER", 2);
        write_discovery(dir.path(), &a).unwrap();
        write_discovery(dir.path(), &b).unwrap();
        assert_ne!(a.file_name(), b.file_name());
        let found = scan_discovery(dir.path(), Daw::Reaper);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].discovery.as_ref().unwrap(), &b);
        assert!(found.iter().all(|found| found.alive));
        assert!(scan_discovery(dir.path(), Daw::Renoise).is_empty());

        // A file renamed to claim another instance is not trusted.
        std::fs::rename(dir.path().join(a.file_name()), dir.path().join(discovery(me, "/c", 0).file_name())).unwrap();
        let found = scan_discovery(dir.path(), Daw::Reaper);
        assert!(found.iter().any(|found| found.discovery.as_ref().is_err_and(|why| why.contains("name"))));

        // A file without a pid (Renoise on Windows) is kept for the host to check by connecting.
        let unknown = discovery(0, "/u", 0);
        write_discovery(dir.path(), &unknown).unwrap();
        assert!(scan_discovery(dir.path(), Daw::Reaper).iter().any(|found| found.alive && found.path.ends_with(unknown.file_name())));
        std::fs::remove_file(dir.path().join(unknown.file_name())).unwrap();

        // A dead pid's file is pruned.
        let dead = discovery(999_999_999, "/d", 0);
        std::fs::write(dir.path().join(dead.file_name()), serde_json::to_string(&dead).unwrap()).unwrap();
        assert_eq!(prune_discovery(dir.path(), Daw::Reaper), 1);
        assert_eq!(scan_discovery(dir.path(), Daw::Reaper).len(), 2);
    }
}
