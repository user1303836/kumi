use super::super::{bridge_version::EARS_BRIDGE, samples::user_library};
use super::rig::{BeforeMainTap, Rig, RigEars, BEFORE_MAIN};
use super::*;
use crate::ears::{
    device::{install_ears, EARS_ITEM},
    link::{open_ears_link, EarsOptions, Tap},
};
use crate::references::LiveBook;
use regex::Regex;
use std::path::Path;
/// How long a listening device that couldn't be set up is left alone: one wait (up to about 9 s) each
/// time at most, not one an audition (a match run auditions a dozen times).
pub(super) const EARS_RETRY_MS: i64 = 10 * 60_000;
/// How many bytes of takes Kumi's folder keeps (the newest).
const TAKES_KEPT: u64 = 1_000_000_000;
/// A take this young may still be being read: pruning leaves it.
const FRESH: std::time::Duration = std::time::Duration::from_secs(120);
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
impl LiveRenderer {
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
            // Driven on its own, so a caller that stops waiting doesn't pause it for the next.
            tokio::task::spawn_local(setup.clone());
            setup
        };
        // The setup goes on without this caller: Esc stops only this wait.
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
        self.place_tap_besides(link, track, &[], signal).await
    }
    /// `place_tap`, never taking one of `placed` (the rig's own taps on the same track, loaded moments before) for it.
    async fn place_tap_besides(&self, link: Rc<dyn EarsLink>, track: &str, placed: &[f64], signal: Signal) -> Result<Tap, TapError> {
        let location = self.lom_track_path(track, signal.clone()).await?;
        let before: Vec<_> = link.taps().iter().map(|tap| tap.id).collect();
        let placed = placed.to_vec();
        let loading = now_ms();
        self.step("load_device", json!({"itemId":EARS_ITEM,"trackRef":track}), signal.clone()).await?;
        let tap = link
            .wait_for(
                Rc::new(move |candidate| {
                    candidate.loaded_at.map_or_else(|| !before.contains(&candidate.id), |loaded| loaded >= loading as f64 - 1000.)
                        && !placed.contains(&candidate.id)
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
            let RigEars { link, taps, .. } = rig.ears.as_mut().unwrap();
            taps.insert(source.name.clone(), self.place_tap(link.clone(), &track, signal.clone()).await?);
        }
        if rig.before_main {
            self.place_before_main(rig, signal).await;
        }
        Ok(())
    }
    /// A second tap on Main, first on its chain: the mix as it comes into Main's chain, heard in the same pass. Live
    /// loads it last; it's moved first. One that can't be placed is deleted again, the listen goes on without it, and
    /// why is kept for the masking note (masking then reads only through a Main chain that leaves it be).
    async fn place_before_main(&self, rig: &mut Rig, signal: Signal) {
        let began = now_ms();
        let marked: std::collections::HashSet<String> = self.history.entries.borrow().keys().cloned().collect();
        // Main's refs before anything of Kumi's is on it: the move and the delete retire them, and once Main's chain is
        // as it was again, they name what they did.
        let mut kept = None;
        let mut identity = None;
        let placed: Result<Tap, RuntimeError> = async {
            let (main, _) = self.main_volume(signal.clone()).await?;
            let read = |signal: Signal| self.rows("device", json!({"parent":main,"fields":["objectIdentity"]}), signal);
            let had: Vec<String> = read(signal.clone()).await?.iter().map(object_identity).collect();
            kept = Some((had, self.history.connection.references.borrow().keep(&main)));
            let ears = rig.ears.as_ref().unwrap();
            let placed: Vec<f64> = ears.taps.values().map(|tap| tap.id).collect();
            // Live's browser now and then loads nothing, and the bridge says so with nothing left behind: once more.
            let mut again = true;
            let tap = loop {
                match self.place_tap_besides(ears.link.clone(), &main, &placed, signal.clone()).await {
                    Ok(tap) => break tap,
                    Err(error) if error.silent => return Err(observation("its device there didn't start in time")),
                    Err(error) if again && error.error.to_string().contains("without a residual device") => {
                        if std::env::var("KUMI_TIMING").is_ok_and(|s| !s.is_empty()) {
                            eprintln!("[ears] Live loaded nothing before Main: once more");
                        }
                        again = false;
                        delay(250., signal.clone()).await?;
                    }
                    Err(error) => return Err(error.error),
                }
            };
            // Its device: the one this load made, by the identity Live gave for it, wherever it landed on Main.
            let made = self.history.entries.borrow().iter().filter(|(id, _)| !marked.contains(*id)).find_map(|(_, entry)| {
                let entry = entry.borrow();
                (entry.tool.as_deref() == Some("load_device")).then(|| entry.created.clone()).flatten()
            });
            let made = made.ok_or_else(|| observation("Live didn't say which device it loaded"))?;
            let rows = read(signal.clone()).await?;
            let device = device_with(&rows, &made).ok_or_else(|| observation("Kumi couldn't find its device on Main"))?;
            identity = Some(made);
            let reference = device.get("ref").and_then(Value::as_str).ok_or_else(|| observation("Live gave its device no ref"))?;
            self.step("move_device", json!({"deviceRef":reference,"index":0}), signal.clone()).await?;
            Ok(tap)
        }
        .await;
        let steps: Vec<String> = self.history.entries.borrow().keys().filter(|id| !marked.contains(*id)).cloned().collect();
        let ears = rig.ears.as_mut().unwrap();
        let (had, refs) = kept.unwrap_or_default();
        match (placed, identity) {
            (Ok(tap), Some(identity)) => {
                ears.taps.insert(BEFORE_MAIN.into(), tap);
                ears.before_main = Some(BeforeMainTap { identity, steps, had, refs });
                if std::env::var("KUMI_TIMING").is_ok_and(|s| !s.is_empty()) {
                    eprintln!("[ears] placed before Main: {} ms", now_ms() - began);
                }
            }
            (failed, identity) => {
                // Taken off at once: half placed, it would sit on Main through the listen.
                if let Some(identity) = identity {
                    ears.before_main = Some(BeforeMainTap { identity, steps, had, refs });
                    self.remove_before_main(rig, self.cleanup()).await;
                }
                if let Err(error) = failed {
                    let error = error.to_string();
                    if std::env::var("KUMI_TIMING").is_ok_and(|s| !s.is_empty()) {
                        eprintln!("[ears] not placed before Main: {error}");
                    }
                    rig.before_main_missed = Some(kumi_common::js::string::head(&error, 160).to_owned());
                }
            }
        }
    }
    /// Takes the tap first on Main off again by Live's identity for its device (undoing its move and its load is less
    /// sure), and forgets the steps that placed it. Main's refs come back when its chain is as it was. Said when the
    /// device can't be taken off.
    pub(super) async fn remove_before_main(&self, rig: &mut Rig, signal: Signal) {
        let Some(ears) = rig.ears.as_mut() else { return };
        let Some(placed) = ears.before_main.take() else { return };
        ears.taps.shift_remove(BEFORE_MAIN);
        let began = now_ms();
        let read = |signal: Signal| async move {
            let (main, _) = self.main_volume(signal.clone()).await.ok()?;
            self.rows("device", json!({"parent":main,"fields":["objectIdentity"]}), signal).await.ok()
        };
        let found = read(signal.clone())
            .await
            .map(|rows| device_with(&rows, &placed.identity).and_then(|row| row.get("ref").and_then(Value::as_str).map(str::to_owned)));
        let gone = match found {
            Some(None) => true,
            Some(Some(reference)) => {
                self.history.quietly(None, self.step("delete_device", json!({"ref":reference}), signal.clone())).await.is_ok()
            }
            None => false,
        };
        if gone {
            self.history.forget(&placed.steps);
            rig.steps.retain(|id| !placed.steps.contains(id));
            let now: Option<Vec<String>> = read(signal).await.map(|rows| rows.iter().map(object_identity).collect());
            let back = now.as_ref() == Some(&placed.had);
            if back {
                self.history.connection.references.borrow_mut().put_back(placed.refs);
            }
            if std::env::var("KUMI_TIMING").is_ok_and(|s| !s.is_empty()) {
                eprintln!(
                    "[ears] taken off Main: {} ms{}",
                    now_ms() - began,
                    if back { "" } else { " (its chain changed: refs stay retired)" }
                );
            }
        } else {
            rig.notes.push("Kumi's listening device may still be first on Main's chain: delete it in Live.".into());
        }
    }
    pub(super) async fn remove_taps(&self, rig: &mut Rig) {
        self.remove_before_main(rig, self.cleanup()).await;
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
    /// Kumi's own takes in its folder (see `prune_takes`).
    pub(super) async fn prune_ears(&self) {
        let _ = prune_takes(&self.ears_folder, TAKES_KEPT, FRESH).await;
    }
}

/// Kumi's own takes in `folder`: the newest stay, up to `budget` bytes of them (and 64 once there are more than 96).
/// A take younger than `fresh` may still be being read, so it always stays. Raw captures a stopped read left go
/// after ten minutes.
pub(super) async fn prune_takes(folder: &Path, budget: u64, fresh: std::time::Duration) -> std::io::Result<()> {
    let mut directory = tokio::fs::read_dir(folder).await?;
    let mut takes = vec![];
    while let Some(entry) = directory.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let Ok(meta) = entry.metadata().await else { continue };
        let age = meta.modified().ok().and_then(|at| at.elapsed().ok()).unwrap_or_default();
        if [".wav", ".aif", ".aiff"].iter().any(|ending| name.ends_with(ending)) {
            takes.push((entry.path(), meta.len(), age));
        } else if name.ends_with(".raw") && age > std::time::Duration::from_secs(600) {
            // A raw capture outlives its read only when Kumi stopped mid-read; none is still being written ten
            // minutes on.
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
    takes.sort_by_key(|(_, _, age)| *age);
    let crowded = takes.len() > 96;
    let mut kept = 0u64;
    for (index, (path, bytes, age)) in takes.into_iter().enumerate() {
        kept += bytes;
        if age < fresh || !(kept > budget || (crowded && index >= 64)) {
            continue;
        }
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_device_moved_first_on_main_is_only_ever_the_one_its_load_made() {
        let row = |reference: &str, identity: &str| json!({"ref": reference, "objectIdentity": identity}).as_object().unwrap().clone();
        // Main as Live's walk lists it: a rack's devices right after the rack, so a row's place isn't its place on
        // Main's chain. The Ears device the load made (Live's identity "ears") is found wherever it is.
        let rows = [
            row("7:device:6:0", "rack"),
            row("7:device:6:0:0:0", "eq"),
            row("7:device:6:1", "limiter"),
            row("7:device:6:2", "main tap"),
            row("7:device:6:3", "ears"),
        ];
        assert_eq!(device_with(&rows, "ears").and_then(|row| row["ref"].as_str()), Some("7:device:6:3"));
        // The producer's devices are never it: not the last one, nor one inside the rack, when the load's isn't there.
        assert!(device_with(&rows[..3], "ears").is_none());
        // Nor a row Live gave no identity for, when the load had none either.
        assert!(device_with(&[json!({"ref": "7:device:6:0"}).as_object().unwrap().clone()], "").is_none());
    }
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
    #[tokio::test(flavor = "current_thread")]
    async fn takes_are_kept_to_a_budget_newest_first_and_a_fresh_one_always() {
        let folder = tempfile::tempdir().unwrap();
        let take = |name: &str, bytes: usize, minutes: u64| {
            let path = folder.path().join(name);
            std::fs::write(&path, vec![0u8; bytes]).unwrap();
            let at = std::time::SystemTime::now() - std::time::Duration::from_secs(minutes * 60);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
            path
        };
        // Newest first: a fresh 300-byte take still being read, then 50, 50 and 80 bytes of older ones.
        let reading = take("reading.wav", 300, 0);
        let newer = take("newer.wav", 50, 5);
        let older = take("older.aif", 50, 10);
        let oldest = take("oldest.wav", 80, 20);
        let other = take("notes.txt", 500, 30);
        prune_takes(folder.path(), 420, std::time::Duration::from_secs(60)).await.unwrap();
        // 300 + 50 + 50 fit in 420; the oldest would pass it. A fresh take stays even past the budget.
        assert!(reading.exists() && newer.exists() && older.exists() && !oldest.exists() && other.exists());
        prune_takes(folder.path(), 100, std::time::Duration::from_secs(60)).await.unwrap();
        assert!(reading.exists() && !newer.exists() && !older.exists(), "past the budget, only what's fresh stays");
    }
    #[test]
    fn only_kumis_own_recordings_go_from_the_project() {
        let tracks = vec!["Kumi · render 1 ab12".to_string()];
        let recorded = |path: &str| super::super::pass::kumi_recording(std::path::Path::new(path), &tracks);
        assert!(recorded("/Set Project/Samples/Recorded/Kumi · render 1 ab12 0001 [2026-10-08 101500].wav"));
        // Another render's, the producer's own recordings, or what a clip holds from elsewhere: kept.
        assert!(!recorded("/Set Project/Samples/Recorded/Kumi · render 10 ab12 0001.wav"));
        assert!(!recorded("/Set Project/Samples/Recorded/Vocal 0001 [2026-10-08 101500].wav"));
        assert!(!recorded("/Set Project/Samples/Imported/Kumi · render 1 ab12 0001.wav"));
        assert!(!recorded("/Users/me/Music/square.wav"));
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

/// The device among a chain's rows that Live knows by `identity` (a rack's devices are among them too).
fn device_with<'a>(rows: &'a [JsonObject], identity: &str) -> Option<&'a JsonObject> {
    rows.iter().find(|row| !identity.is_empty() && object_identity(row) == identity)
}

/// A device row's identity in Live (empty when the bridge doesn't say).
fn object_identity(row: &JsonObject) -> String {
    match row.get("objectIdentity") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}
