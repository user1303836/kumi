use super::super::{bridge_version::EARS_BRIDGE, samples::user_library};
use super::rig::{Rig, RigEars};
use super::*;
use crate::ears::{
    device::{install_ears, EARS_ITEM},
    link::{open_ears_link, EarsOptions, Tap},
};
use regex::Regex;
use std::path::Path;
/// How long a listening device that couldn't be set up is left alone: one wait (up to about 9 s) each
/// time at most, not one an audition (a match run auditions a dozen times).
pub(super) const EARS_RETRY_MS: i64 = 10 * 60_000;
/// A capture's raw file, removed however its read ends: a sibling tap failing, or a stop, drops the read partway.
pub(super) struct RawFile(pub(super) PathBuf);
impl Drop for RawFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
/// Who keeps an Ears folder: the next session to set Ears up sweeps a folder whose keeper is gone (a crash).
const OWNER: &str = "owner.json";
/// Marks `folder` as this process's, then sweeps its siblings (other sessions' folders) whose keeper isn't running:
/// one whose process is gone, or another process now has its pid (it started at another time). A folder from
/// before keepers were written goes once it's a day old.
pub(super) async fn sweep_ears(folder: &Path) {
    let pid = std::process::id();
    // A platform that keeps no process starts can't tell a gone keeper from a running one: nothing is swept.
    let Some(started) = kumi_common::process::started_at_ms(pid) else { return };
    let _ = tokio::fs::write(folder.join(OWNER), stringify(&json!({"pid":pid,"started":started}))).await;
    let Some(parent) = folder.parent() else { return };
    let Ok(mut entries) = tokio::fs::read_dir(parent).await else { return };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path == folder || !entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let gone = match tokio::fs::read(path.join(OWNER)).await {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes).ok().is_some_and(|owner| {
                let (Some(pid), Some(started)) = (owner["pid"].as_u64(), owner["started"].as_i64()) else { return false };
                !u32::try_from(pid).ok().and_then(kumi_common::process::started_at_ms).is_some_and(|now| (now - started).abs() <= 2_000)
            }),
            Err(_) => entry
                .metadata()
                .await
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|at| at.elapsed().ok())
                .is_some_and(|age| age > std::time::Duration::from_secs(86_400)),
        };
        if gone {
            let _ = tokio::fs::remove_dir_all(&path).await;
        }
    }
}
pub(super) struct TapError {
    pub error: RuntimeError,
    pub silent: bool,
}
impl From<RuntimeError> for TapError {
    fn from(error: RuntimeError) -> Self {
        Self { error, silent: false }
    }
}
impl Rendering {
    pub(super) async fn ears_ready(self: &Rc<Self>, signal: Signal) -> Result<Option<Rc<dyn EarsLink>>, RuntimeError> {
        if self.ears_disabled || std::env::var("KUMI_EARS").ok().as_deref() == Some("0") || self.ears_refused.get() {
            return Ok(None);
        }
        if self.ears_failed_at.get().is_some_and(|at| now_ms() - at < EARS_RETRY_MS) {
            return Ok(None);
        }
        if !self.connection().has("live_browser_load_preview")
            || !self.connection().has("live_browser_inspect")
            || !self.supported(EARS_BRIDGE)
        {
            return Ok(None);
        }
        // The connection the setup runs on: one that drops meanwhile isn't Ears failing.
        let lifetime = self.connection().lifetime.clone();
        let setup = self.ears_setup.borrow().clone();
        let setup = if let Some(setup) = setup {
            setup
        } else {
            let this = self.clone();
            let setup = async move {
                let ready: Result<Option<Rc<dyn EarsLink>>, RuntimeError> = async {
                    let link = if let Some(open) = &this.open_ears {
                        open().await?
                    } else {
                        open_ears_link(EarsOptions::default()).await.map_err(|e| RuntimeError::plain(e.to_string()))?
                    };
                    let mut directory = tokio::fs::DirBuilder::new();
                    directory.recursive(true);
                    #[cfg(unix)]
                    directory.mode(0o700);
                    directory.create(&this.ears_folder).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
                    sweep_ears(&this.ears_folder).await;
                    if this.open_ears.is_some() {
                        return Ok(Some(link));
                    }
                    install_ears(this.user_library.clone().unwrap_or_else(|| user_library(None, None)))
                        .await
                        .map_err(|e| RuntimeError::plain(e.to_string()))?;
                    // Live's browser shows a device it already had within moments, but one just written (or any,
                    // while Live is still indexing after it opened) can take a while.
                    let deadline = now_ms() + 20_000;
                    while now_ms() < deadline {
                        let seen = this
                            .connection()
                            .call(
                                "live_browser_inspect",
                                object(json!({"itemId":EARS_ITEM})),
                                abort::any([this.connection().lifetime.clone(), abort::timeout(5_000)]),
                            )
                            .await
                            .is_ok_and(|read| read.is_error != Some(true));
                        if seen {
                            return Ok(Some(link));
                        }
                        delay(400., this.connection().lifetime.clone()).await?;
                    }
                    link.close().await;
                    Ok(None)
                }
                .await;
                ready.ok().flatten()
            }
            .boxed_local()
            .shared();
            *self.ears_setup.borrow_mut() = Some(setup.clone());
            setup
        };
        // Shared with any other caller, the setup goes on without this one: Esc stops only this wait.
        let link = tokio::select! {
            link = setup => link,
            _ = signal.cancelled() => return Err(RuntimeError::Aborted),
        };
        if link.is_none() {
            *self.ears_setup.borrow_mut() = None;
            if !lifetime.is_cancelled() {
                self.ears_failed_at.set(Some(now_ms()));
            }
        }
        signal.check()?;
        Ok(link)
    }
    pub(super) async fn lom_track_path(&self, track: &str, signal: Signal) -> Result<String, RuntimeError> {
        let long = self.connection().references.borrow().lengthen(&json!({"trackRef":track}));
        let long = js_string(&long["trackRef"]);
        if long.contains(":main_track:") {
            return Ok("live_set master_track".into());
        }
        let returns = Regex::new(r":return_track:([0-9]+)$").unwrap();
        if let Some(found) = returns.captures(&long) {
            let index = found[1].parse::<f64>().unwrap_or(f64::INFINITY);
            return Ok(format!("live_set return_tracks {}", kumi_common::js::number::to_string(index)));
        }
        let tracks = Regex::new(r":track:([0-9]+)$").unwrap();
        let index = tracks
            .captures(&long)
            .and_then(|found| found[1].parse::<f64>().ok())
            .filter(|value| value.is_finite() && value.fract() == 0.)
            .ok_or_else(|| observation("Kumi couldn't tell where that track is; discover it again."))?;
        let regular = self.rows("track", json!({"fields":["name"]}), signal.clone()).await?.len() as f64;
        if index < regular {
            return Ok(format!("live_set tracks {}", kumi_common::js::number::to_string(index)));
        }
        let returns = self.rows("return-track", json!({"fields":["name"]}), signal).await?.len() as f64;
        Ok(if index < regular + returns {
            format!("live_set return_tracks {}", kumi_common::js::number::to_string(index - regular))
        } else {
            "live_set master_track".into()
        })
    }
    pub(super) async fn place_tap(&self, link: Rc<dyn EarsLink>, track: &str, signal: Signal) -> Result<Tap, TapError> {
        let location = self.lom_track_path(track, signal.clone()).await?;
        let before: Vec<_> = link.taps().iter().map(|tap| tap.id).collect();
        let loading = now_ms();
        self.step("load_device", json!({"itemId":EARS_ITEM,"trackRef":track}), signal.clone()).await?;
        let tap = link
            .wait_for(
                Rc::new(move |candidate| {
                    candidate.loaded_at.map_or_else(|| !before.contains(&candidate.id), |loaded| loaded >= loading as f64 - 1000.)
                        && candidate.path.starts_with(&format!("{location} devices "))
                }),
                6000,
                Some(signal),
            )
            .await;
        tap.ok_or_else(|| TapError {
            silent: true,
            error: observation(
                "Kumi's listening device didn't start on that track (Max for Live is needed: Live Suite, or Standard with Max for Live).",
            ),
        })
    }
    pub(super) async fn place_taps(&self, rig: &mut Rig, signal: Signal) -> Result<(), TapError> {
        for source in &rig.sources {
            let track = if source.mix { self.main_volume(signal.clone()).await?.0 } else { source.track.clone() };
            let RigEars { link, taps } = rig.ears.as_mut().unwrap();
            taps.insert(source.name.clone(), self.place_tap(link.clone(), &track, signal.clone()).await?);
        }
        Ok(())
    }
    pub(super) async fn remove_taps(&self, rig: &mut Rig) {
        let cleanup = self.cleanup();
        let loads: Vec<_> = rig
            .steps
            .iter()
            .filter(|id| self.history.entries.borrow().get(*id).is_some_and(|entry| entry.borrow().record.family == ChangeFamily::Device))
            .cloned()
            .collect();
        for id in loads.iter().rev() {
            // A device Live wouldn't take back stays the rig's: closing it tries again, and says so if it can't.
            if matches!(self.history.undo(id, cleanup.clone(), false).await, Ok(undone) if !undone.is_error) {
                self.history.entries.borrow_mut().shift_remove(id);
                if let Some(at) = rig.steps.iter().position(|step| step == id) {
                    rig.steps.remove(at);
                }
            }
        }
        if let Some(ears) = &mut rig.ears {
            ears.taps.clear();
        }
    }
    pub(super) async fn prune_ears(&self) {
        let result: Result<(), std::io::Error> = async {
            let mut directory = tokio::fs::read_dir(&self.ears_folder).await?;
            let mut names = vec![];
            while let Some(entry) = directory.next_entry().await? {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(".wav") {
                    names.push(entry.path());
                } else if name.ends_with(".raw")
                    && entry
                        .metadata()
                        .await
                        .ok()
                        .and_then(|meta| meta.modified().ok())
                        .and_then(|at| at.elapsed().ok())
                        .is_some_and(|age| age > std::time::Duration::from_secs(600))
                {
                    // A raw capture outlives its read only when Kumi stopped mid-read; none is still being written
                    // ten minutes on.
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
            if names.len() <= 96 {
                return Ok(());
            }
            let mut dated = vec![];
            for path in names {
                let modified = std::fs::metadata(&path)?.modified()?;
                dated.push((path, modified));
            }
            dated.sort_by_key(|(_, time)| *time);
            let remove = dated.len() - 64;
            for (path, _) in dated.into_iter().take(remove) {
                match tokio::fs::remove_file(path).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        }
        .await;
        let _ = result;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(flavor = "current_thread")]
    async fn a_crashed_sessions_ears_folder_is_swept_and_a_running_ones_kept() {
        let parent = tempfile::tempdir().unwrap();
        let [mine, running, crashed, older] = ["mine", "running", "crashed", "older"].map(|name| parent.path().join(name));
        for folder in [&mine, &running, &crashed, &older] {
            std::fs::create_dir_all(folder).unwrap();
            std::fs::write(folder.join("take.wav"), b"").unwrap();
        }
        let pid = std::process::id();
        let started = kumi_common::process::started_at_ms(pid).unwrap();
        // Another session in a running process; one whose process is gone (a child that exited, at a start that was
        // never its); and one from before keepers were written.
        std::fs::write(running.join(OWNER), stringify(&json!({"pid":pid,"started":started}))).unwrap();
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) { &["/C", "exit"][..] } else { &[][..] })
            .spawn()
            .unwrap();
        let gone = child.id();
        child.wait().unwrap();
        std::fs::write(crashed.join(OWNER), stringify(&json!({"pid":gone,"started":1}))).unwrap();
        sweep_ears(&mine).await;
        assert!(mine.join(OWNER).is_file(), "this session's folder says it's its");
        assert!(running.join("take.wav").is_file());
        assert!(!crashed.exists());
        assert!(older.join("take.wav").is_file(), "a keeperless folder goes only once it's a day old");
    }
    #[test]
    fn a_raw_capture_goes_when_its_read_ends() {
        let folder = tempfile::tempdir().unwrap();
        let raw = RawFile(folder.path().join("take.raw"));
        std::fs::write(&raw.0, b"captured").unwrap();
        let path = raw.0.clone();
        drop(raw);
        assert!(!path.exists());
    }
}
