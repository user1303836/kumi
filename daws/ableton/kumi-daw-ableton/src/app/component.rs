//! Live's part as an agent host's setup puts it there (`kumi serve`'s `setup_daw`, C8): Kumi's Remote Script with the
//! bridge it starts, and Kumi Ears in the User Library, which follows the Remote Script's owner (one file in Live's one
//! User Library can't have another). The plan is read from files alone; carrying it out installs as `kumi bridge` does,
//! through the bridge's own lifecycle and from the runtime's own bundle, with Live closed, nothing asked, and Live
//! neither started nor quit. Kumi's extension for Live isn't part of it: the plugin's runtime doesn't use it.
use super::{
    answers::{LiveRunning, PreparedBridge},
    bridge_setup::{self, digest, lifecycle_answer, owner_paths, prepared_artifact, unpack_into, Unfinished},
    folders::{find_bridge_config, read_bridge_server, remote_scripts_dir},
    provider::Ableton,
};
use crate::ears::device::{ears_file, install_ears_with, EARS_NAME};
use kumi_common::time::now_ms;
use kumi_daw::{
    component::{folders_made, same_folder, After, ApplyError, ComponentPlan, Part, PartState},
    io::DawIo,
    process::{executable_dir, executable_name},
    provider::{newer, DawProvider},
};
use kumi_library::sources::{dirname, join};
use kumi_types::version::KUMI_VERSION;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const COMPONENT: &str = "Live Remote Script";
const SCRIPT: &str = "Remote Script";
const EARS: &str = "Kumi Ears";
const FOLDER: &str = "AbletonMcpBridge";
const CLOSED: &str = "Live must be closed while it's installed: Kumi neither quits nor starts it";
const IN_USE: &str =
    "Live is open, and its Remote Script can't change while it runs: save your work, quit Live, then set Kumi up for Live again.";

/// What's on disk for the Kumi home a plan is for, as the plan reads it and as carrying it out reads it again.
#[derive(Debug, Clone, PartialEq)]
struct Look {
    home: String,
    /// The Remote Scripts folder: the installed bridge's receipt's when it's this home's, else Live's (as the env says).
    scripts: String,
    /// The state folder, secret and Remote Scripts folder this home's installed bridge's receipt names (`owner_paths`):
    /// an update writes there, wherever that is (an earlier install's own folder, say).
    owned: Option<(String, String, String)>,
    /// The Remote Scripts folder Live uses now (its preferences', or the env's).
    live_scripts: String,
    /// The bridge's configuration, when Live has Kumi's Remote Script with a reference to one.
    config: Option<String>,
    /// The version of the bridge it starts, when it says one.
    installed: Option<String>,
    /// The Kumi home the configuration belongs to, as [`Ableton::bridge_owner`] tells it.
    owner: Option<String>,
    bundled: String,
    /// Where the runtime's bundle keeps the bridge's tarball (`prepared.json` and the file it names).
    prepared: String,
    artifact: Option<(String, String)>,
    script: PartState,
    /// The User Library the Remote Scripts folder is in, when it's one (a folder named Remote Scripts that's there).
    library: Option<String>,
    ears: PartState,
    /// Where this home records the Kumi Ears it wrote (`<home>/kumi-ears.json`: its path, SHA-256s and Kumi versions),
    /// so a device that differs is replaced only when this home wrote it with a Kumi no newer than this one.
    ears_record: String,
    /// Whether a Kumi Ears is there that can't be read: nothing says it's this home's, so it stays.
    ears_unreadable: bool,
}

impl Look {
    fn read(io: &DawIo, home: &Path) -> Result<Look, String> {
        let bundled = bridge_setup::bundled_bridge_version().ok_or("This Kumi brings no bridge for Live.")?;
        let scripts = remote_scripts_dir(&io.env);
        let config = find_bridge_config(&io.env);
        let server = config.as_deref().and_then(|config| read_bridge_server(config).ok());
        let owner = config.as_deref().and_then(|config| Ableton.bridge_owner(config, &io.env));
        let ours = owner.as_deref().is_some_and(|owner| same_folder(Path::new(owner), home));
        let installed = server.as_ref().and_then(|server| server.version.clone());
        let native = server.as_ref().is_some_and(|server| server.native());
        let package = server.as_ref().and_then(|server| server.package_root());
        let owned = (config.as_deref().zip(package.as_deref()))
            .filter(|_| ours)
            .and_then(|(config, package)| owner_paths(config, package, &home.display().to_string()));
        let live_scripts = scripts.clone();
        let scripts = owned.as_ref().map(|(_, _, scripts)| scripts.clone()).unwrap_or(scripts);
        let script = match (&config, &installed) {
            // A Remote Script with no reference to a bridge says nothing of whose it is: it stays.
            (None, _) if Path::new(&join(&scripts, FOLDER)).exists() => PartState::OtherOwner,
            (None, _) => PartState::Missing,
            (Some(_), _) if !ours => PartState::OtherOwner,
            (Some(_), Some(version)) if native && *version == bundled => PartState::Same,
            (Some(_), Some(version)) if native && newer(version, &bundled) => PartState::Newer,
            (Some(_), _) => PartState::Older,
        };
        let parent = dirname(&scripts);
        let library =
            (Path::new(&scripts).file_name().is_some_and(|name| name == "Remote Scripts") && Path::new(&parent).is_dir()).then_some(parent);
        // Kumi Ears follows the Remote Script's owner, and is this home's only when this home wrote it (its record): one
        // that differs from this Kumi's otherwise is another Kumi's device (perhaps a newer one), and stays, before and
        // after this home's Remote Script goes in.
        let ears_record = join(&home.display().to_string(), EARS_RECORD);
        let found = library.as_deref().map(|library| (ears_path(library), std::fs::read(ears_path(library))));
        let ears_unreadable = matches!(&found, Some((_, Err(error))) if error.kind() != std::io::ErrorKind::NotFound);
        let ears = match found {
            Some((_, Ok(bytes))) if bytes == ears_file() => PartState::Same,
            Some((path, Ok(bytes))) if matches!(script, PartState::Same | PartState::Older) => {
                recorded_state(&ears_record, &path, &bytes, KUMI_VERSION)
            }
            Some((_, Ok(_))) => PartState::OtherOwner,
            _ if ears_unreadable => PartState::OtherOwner,
            _ => PartState::Missing,
        };
        let prepared =
            io.answers.get::<PreparedBridge>().map(|prepared| prepared.0.clone()).unwrap_or_else(|| join(&executable_dir(), "bridge"));
        let artifact = prepared_artifact(&prepared);
        let look = Look {
            home: home.display().to_string(),
            scripts,
            owned,
            live_scripts,
            config,
            installed,
            owner,
            bundled,
            prepared,
            artifact,
            script,
            library,
            ears,
            ears_record,
            ears_unreadable,
        };
        if look.script_writes() {
            if look.artifact.is_none() {
                return Err(format!(
                    "Kumi's runtime has no copy of Live's bridge to install ({}): set up Kumi's runtime again.",
                    join(&look.prepared, "prepared.json")
                ));
            }
            if !Path::new(&look.scripts).exists() && look.library.is_none() {
                return Err(format!(
                    "Kumi couldn't find Live's User Library (it looked for {}). Open Live once so it makes one.",
                    dirname(&look.scripts)
                ));
            }
        }
        Ok(look)
    }

    /// Whether putting Live's part in writes the Remote Script (and the bridge it starts).
    fn script_writes(&self) -> bool {
        matches!(self.script, PartState::Missing | PartState::Older)
    }

    /// Whether it writes Kumi Ears: only where the Remote Script is this home's, or goes in for it.
    fn ears_writes(&self) -> bool {
        matches!(self.script, PartState::Missing | PartState::Older | PartState::Same)
            && self.library.is_some()
            && matches!(self.ears, PartState::Missing | PartState::Older)
    }

    fn script_path(&self) -> String {
        join(&self.scripts, FOLDER)
    }

    /// The state folder an install or update of the bridge writes (its configuration, secret, receipt and journal): the
    /// receipt's, else the configuration's own folder, else `<home>/bridge/state` for a new one.
    fn state_dir(&self) -> String {
        match (&self.owned, &self.config) {
            (Some((state, _, _)), _) => state.clone(),
            (None, Some(config)) => dirname(config),
            (None, None) => join(&self.home, "bridge/state"),
        }
    }

    fn plan(&self) -> ComponentPlan {
        let (script, bridge) = (self.script_path(), join(&self.home, "bridge"));
        let installed = self.installed.as_deref().unwrap_or("an earlier version");
        let (mut paths, mut effects) = (vec![], vec![]);
        match self.script {
            PartState::Missing => {
                if !Path::new(&self.scripts).exists() {
                    effects.push(format!("Make Live's Remote Scripts folder, {}", self.scripts));
                    paths.push(self.scripts.clone());
                } else {
                    paths.push(script.clone());
                }
                effects.push(format!("Install Kumi's Remote Script for Live, {FOLDER} {}, in {script}", self.bundled));
                effects.push(format!(
                    "Keep the bridge it starts ({}), with its settings and a secret only it and Kumi read, in {bridge}",
                    self.bundled
                ));
                effects.push(CLOSED.into());
                paths.push(bridge);
            }
            PartState::Older => {
                effects.push(format!(
                    "Update Kumi's Remote Script for Live in {script} from {installed} to {}, with the bridge it starts in {bridge}; the bridge keeps the one it replaces to go back to",
                    self.bundled
                ));
                effects.push(CLOSED.into());
                paths.extend([script.clone(), bridge.clone()]);
            }
            PartState::Newer => effects.push(format!(
                "Kumi's Remote Script in {script} is {installed}, newer than this Kumi's {}: it stays as it is. Update the Kumi plugin (hermes plugins update kumi) to match it, then set Kumi up again",
                self.bundled
            )),
            PartState::OtherOwner => effects.push(match &self.owner {
                Some(owner) => format!("Kumi's Remote Script in {script} belongs to the Kumi at {owner}: it stays as it is"),
                None => format!("The Remote Script in {script} doesn't say which Kumi it belongs to: it stays as it is"),
            }),
            PartState::Same => {}
        }
        if self.script_writes() {
            if self.scripts != self.live_scripts {
                effects.push(format!(
                    "An earlier install put Kumi's Remote Script in {}, not the folder Live uses now ({}): the update goes to {}",
                    self.scripts, self.live_scripts, self.scripts
                ));
            }
            // The bridge's lifecycle writes its configuration, secret, receipt and journal in the state folder, and the
            // configuration where it is: named too when that isn't in this home's bridge folder (an earlier install's).
            let mut others: Vec<String> = vec![self.state_dir()];
            others.extend(self.config.as_deref().map(dirname));
            for folder in others {
                if !paths.iter().any(|path| Path::new(&folder).starts_with(path)) {
                    effects.push(format!("Update the bridge's settings, secret and receipt where this installation keeps them: {folder}"));
                    paths.push(folder);
                }
            }
        }
        let ears = self.library.as_deref().map(ears_path);
        if self.ears_writes() {
            let ears = ears.clone().unwrap_or_default();
            effects.push(match self.ears {
                PartState::Older => format!("Update Kumi Ears, the Max for Live device Kumi listens to a track through, at {ears}"),
                _ => format!("Copy Kumi Ears, the Max for Live device Kumi listens to a track through, to {ears}"),
            });
            effects.push(format!("Note that this Kumi put it there, in {}", self.ears_record));
            paths.extend([ears, self.ears_record.clone()]);
        } else if self.ears == PartState::Newer {
            effects.push(format!(
                "The Kumi Ears at {} was written by a newer Kumi, or its version can't be compared with this Kumi's: it stays as it is. Update the Kumi plugin (hermes plugins update kumi), then set Kumi up again",
                ears.clone().unwrap_or_default()
            ));
        } else if self.ears == PartState::OtherOwner {
            let ears = ears.clone().unwrap_or_default();
            effects.push(match self.ears_unreadable {
                true => format!("Kumi can't read the Kumi Ears at {ears}, so nothing says it's this Kumi's: it stays as it is"),
                false => format!("The Kumi Ears at {ears} isn't one this Kumi put there: it stays as it is"),
            });
        }
        for folder in folders_made(&paths, Path::new(&self.home)) {
            effects.push(format!("Make the folder {folder}: it isn't there yet"));
            paths.push(folder);
        }
        if !paths.is_empty() {
            effects.push(format!(
                "While it works it makes temporary copies beside these (in {}, and in Kumi Ears' folder), each named for what it replaces, and removes them before it's done",
                self.scripts
            ));
        }
        let mut parts = vec![Part { name: SCRIPT, path: script, state: self.script }];
        parts.extend(ears.map(|path| Part { name: EARS, path, state: self.ears }));
        let state = match self.script {
            PartState::OtherOwner | PartState::Newer => self.script,
            _ if self.ears == PartState::Newer => PartState::Newer,
            _ if parts.iter().any(|part| part.state == PartState::Missing) => PartState::Missing,
            _ if parts.iter().any(|part| part.state == PartState::Older) => PartState::Older,
            _ => PartState::Same,
        };
        let version = match self.script {
            PartState::Newer | PartState::OtherOwner => self.installed.clone(),
            _ => Some(self.bundled.clone()),
        };
        let after = if self.script == PartState::Missing { After::ChooseControlSurface } else { After::None };
        ComponentPlan {
            component: COMPONENT,
            version,
            state,
            owner: self.owner.clone(),
            paths,
            effects,
            after,
            parts,
            keep_identity: false,
        }
    }
}

fn ears_path(library: &str) -> String {
    join(library, &format!("Kumi/Audio Effects/{EARS_NAME}.amxd"))
}

/// This home's record of the Kumi Ears it wrote, in the home.
const EARS_RECORD: &str = "kumi-ears.json";

/// Recent device versions this home wrote at one path. The entry for the device still on disk is retained even if
/// repeated interrupted updates have filled the history. An unversioned legacy entry keeps its ownership proof, but
/// cannot authorize replacing different bytes: its writer may be newer than this Kumi.
#[derive(Serialize)]
struct EarsRecord {
    path: String,
    versions: Vec<EarsVersion>,
}

#[derive(Serialize)]
struct EarsVersion {
    sha256: String,
    version: Option<String>,
}

fn read_ears_record(record: &str, path: &str) -> std::io::Result<Vec<EarsVersion>> {
    let text = match std::fs::read(record) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&text) else { return Ok(vec![]) };
    if value["path"].as_str() != Some(path) {
        return Ok(vec![]);
    }
    let mut versions: Vec<EarsVersion> = value["versions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(EarsVersion { sha256: entry["sha256"].as_str()?.into(), version: entry["version"].as_str().map(str::to_string) })
        })
        .collect();
    if let Some(sha256) = value["sha256"].as_str() {
        versions.push(EarsVersion { sha256: sha256.into(), version: value["version"].as_str().map(str::to_string) });
    }
    Ok(versions)
}

/// Release versions must be three whole numbers. A prerelease, an unknown writer or an invalid version never gives
/// permission to downgrade a device.
fn release_version(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.split('.');
    let mut numbers = [0; 3];
    for number in &mut numbers {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *number = part.parse().ok()?;
    }
    parts.next().is_none().then_some(numbers)
}

fn recorded_state(record: &str, path: &str, bytes: &[u8], version: &str) -> PartState {
    let sha256 = hex::encode(Sha256::digest(bytes));
    let entries = read_ears_record(record, path).unwrap_or_default();
    let mut ours = false;
    for entry in entries.iter().filter(|entry| entry.sha256 == sha256) {
        ours = true;
        match (entry.version.as_deref().and_then(release_version), release_version(version)) {
            (Some(written), Some(current)) if written <= current => {}
            _ => return PartState::Newer,
        }
    }
    if ours {
        PartState::Older
    } else {
        PartState::OtherOwner
    }
}

/// Live's part for the Kumi home `home`: see [`DawProvider::component_plan`].
pub fn plan(io: &DawIo, home: &Path) -> Result<ComponentPlan, String> {
    Look::read(io, home).map(|look| look.plan())
}

/// Carries out `plan`, Live's part for `home` as [`plan`] made it just now: see [`DawProvider::component_apply`].
pub async fn apply(io: &DawIo, home: &Path, plan: &ComponentPlan) -> Result<(), ApplyError> {
    let look = Look::read(io, home).map_err(ApplyError::Failed)?;
    if look.plan() != *plan {
        return Err(ApplyError::Changed("What's in Live changed since setup looked: plan the step again.".into()));
    }
    if look.script_writes() {
        if live_running(io).await {
            return Err(ApplyError::InUse(IN_USE.into()));
        }
        put_script(io, &look).await?;
    }
    if look.ears_writes() {
        let library = look.library.as_deref().unwrap_or_default();
        let written = async {
            // The legacy copy (`Kumi/Kumi Ears.amxd`) stays: nothing says it's this home's.
            // Write ahead: a failed record leaves the device untouched; a failed device leaves both its old entry and
            // the intended one, so either side of an interrupted rename remains owned on retry.
            record_ears(&look.ears_record, &ears_path(library), &ears_file(), KUMI_VERSION)?;
            install_ears_with(library, false).await.map(|_| ())
        };
        written.await.map_err(|error| {
            ApplyError::Failed(match look.script_writes() {
                // Each part goes in whole or not at all: the next setup puts in the one that's missing.
                true => {
                    format!("Kumi's Remote Script for Live is in, but Kumi Ears isn't ({error}): set Kumi up for Live again to add it.")
                }
                false => format!("Kumi couldn't put Kumi Ears in your User Library: {error}"),
            })
        })?;
    }
    Ok(())
}

/// Notes the device this home is about to write, before replacing it. The record is written beside itself and
/// renamed into place, or not at all; an earlier device's entry stays if the device write fails afterwards.
fn record_ears(record: &str, file: &str, bytes: &[u8], version: &str) -> std::io::Result<()> {
    let sha256 = hex::encode(Sha256::digest(bytes));
    let mut versions = read_ears_record(record, file)?;
    versions.retain(|entry| entry.sha256 != sha256);
    versions.push(EarsVersion { sha256, version: Some(version.into()) });
    let installed = std::fs::read(file).ok().map(|bytes| hex::encode(Sha256::digest(bytes)));
    while versions.len() > 8 {
        // The final entry is the write ahead; keep the current device too, even after many failed updates.
        let remove = versions[..versions.len() - 1].iter().position(|entry| installed.as_ref() != Some(&entry.sha256)).unwrap_or(0);
        versions.remove(remove);
    }
    let body = EarsRecord { path: file.into(), versions };
    if let Some(folder) = Path::new(record).parent() {
        std::fs::create_dir_all(folder)?;
    }
    let temporary = format!("{record}.{}.tmp", std::process::id());
    let written = std::fs::write(&temporary, serde_json::to_vec(&body)?).and_then(|()| std::fs::rename(&temporary, record));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

async fn live_running(io: &DawIo) -> bool {
    match io.answers.get::<LiveRunning>() {
        Some(running) => (running.0)().await,
        None => bridge_setup::is_live_running_on(io.run.clone(), kumi_types::system::platform(), &io.env).await,
    }
}

/// The Remote Script and the bridge it starts, put in as `kumi bridge` puts them (its lifecycle, planned, then applied
/// with Live closed), from the runtime's own bundle: its tarball copied into a new generation of the home's `bridge`
/// folder, checked against the SHA-256 the bundle lists, and unpacked there. A failure leaves nothing of it.
async fn put_script(io: &DawIo, look: &Look) -> Result<(), ApplyError> {
    let failed = |why: String| ApplyError::Failed(why);
    let Some((name, sha)) = look.artifact.clone() else {
        return Err(failed("Kumi's runtime has no copy of Live's bridge to install: set up Kumi's runtime again.".into()));
    };
    if !Path::new(&look.scripts).exists() {
        std::fs::create_dir(&look.scripts).map_err(|error| failed(format!("Kumi couldn't make {}: {error}", look.scripts)))?;
    }
    let folder = join(&look.home, &format!("bridge/{}-{}", look.bundled, now_ms()));
    let mut dirs = std::fs::DirBuilder::new();
    dirs.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        dirs.mode(0o700);
    }
    dirs.create(&folder).map_err(|error| failed(format!("Kumi couldn't make {folder}: {error}")))?;
    // Until the lifecycle has put this version in place, nothing refers to its folder.
    let unfinished = Unfinished(Some(folder.clone()));
    let artifact = join(&folder, &name);
    tokio::fs::copy(join(&look.prepared, &name), &artifact)
        .await
        .map_err(|error| failed(format!("Kumi couldn't copy the bridge: {error}")))?;
    if digest(&artifact).ok().as_deref() != Some(sha.as_str()) {
        return Err(failed("Kumi's runtime's copy of Live's bridge is damaged: set up Kumi's runtime again.".into()));
    }
    unpack_into(&folder, &name, ".").map_err(|why| failed(format!("Unpacking the bridge failed: {why}")))?;
    let root = join(&folder, "package");
    // The receipt's own folders when there's one, as the plan named them.
    let (state, secret, scripts) = (look.state_dir(), look.owned.as_ref().map(|(_, secret, _)| secret.clone()), look.scripts.clone());
    let action = if look.config.is_some() { "upgrade" } else { "install" };
    let lifecycle = |apply: bool| {
        let mut args: Vec<String> =
            ["lifecycle", action, "--remote-scripts-dir", scripts.as_str(), "--state-dir", state.as_str(), "--package-root", root.as_str()]
                .map(str::to_string)
                .into();
        if let Some(config) = &look.config {
            args.extend(["--config".into(), config.clone()]);
        }
        if let Some(secret) = &secret {
            args.extend(["--secret".into(), secret.clone()]);
        }
        args.extend(["--artifact".into(), artifact.clone(), "--artifact-sha256".into(), sha.clone()]);
        if apply {
            args.extend(["--apply".into(), "--confirm-live-stopped".into()]);
        }
        (io.run)(join(&root, &executable_name("ableton-mcp-server")), args, None)
    };
    lifecycle_answer(lifecycle(false).await).map_err(|why| failed(format!("The bridge's installer refused: {why}")))?;
    // Live may have opened since the first look: the switch runs with it closed.
    if live_running(io).await {
        return Err(ApplyError::InUse(IN_USE.into()));
    }
    lifecycle_answer(lifecycle(true).await)
        .map_err(|why| failed(format!("The bridge's installer stopped, and put back what was there: {why}")))?;
    unfinished.keep();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ears_versions_follow_the_device_through_an_interrupted_update_and_an_older_runtime() {
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join(EARS_RECORD).display().to_string();
        let file = root.path().join("Kumi Ears.amxd").display().to_string();
        record_ears(&record, &file, b"older", "1.0.0").unwrap();
        std::fs::write(&file, b"older").unwrap();
        record_ears(&record, &file, b"newer", "1.1.0").unwrap();
        assert_eq!(recorded_state(&record, &file, b"older", "1.1.0"), PartState::Older, "the device rename didn't happen");
        std::fs::write(&file, b"newer").unwrap();
        assert_eq!(recorded_state(&record, &file, b"newer", "1.0.0"), PartState::Newer, "the older runtime must leave it");
        assert_eq!(recorded_state(&record, &file, b"newer", "1.1.0"), PartState::Older, "the same writer can repair different bytes");
        assert_eq!(recorded_state(&record, &file, b"producer's edit", "1.1.0"), PartState::OtherOwner);
        // Many failed updates cannot evict the ownership proof for the device still on disk.
        for version in 2..12 {
            record_ears(&record, &file, &[version], &format!("1.{version}.0")).unwrap();
        }
        assert!(read_ears_record(&record, &file).unwrap().len() <= 8);
        assert_eq!(recorded_state(&record, &file, b"newer", "1.0.0"), PartState::Newer);
    }
}
