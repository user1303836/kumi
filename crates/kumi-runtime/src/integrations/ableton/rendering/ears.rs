use super::super::{bridge_version::EARS_BRIDGE, samples::user_library};
use super::rig::{Rig, RigEars};
use super::*;
use crate::ears::{
    device::{install_ears, EARS_ITEM},
    link::{open_ears_link, EarsOptions, Tap},
};
use regex::Regex;
/// How long a listening device that couldn't be set up is left alone: one wait (up to about 9 s) each
/// time at most, not one an audition (a match run auditions a dozen times).
pub(super) const EARS_RETRY_MS: i64 = 10 * 60_000;
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
                    if this.open_ears.is_some() {
                        return Ok(Some(link));
                    }
                    let installed = install_ears(this.user_library.clone().unwrap_or_else(|| user_library(None, None)))
                        .await
                        .map_err(|e| RuntimeError::plain(e.to_string()))?;
                    let deadline = now_ms() + if installed.written { 20_000 } else { 4_000 };
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
        let link = setup.await;
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
            let _ = self.history.undo(id, cleanup.clone(), false).await;
            self.history.entries.borrow_mut().shift_remove(id);
            if let Some(at) = rig.steps.iter().position(|step| step == id) {
                rig.steps.remove(at);
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
                if entry.file_name().to_string_lossy().ends_with(".wav") {
                    names.push(entry.path());
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
