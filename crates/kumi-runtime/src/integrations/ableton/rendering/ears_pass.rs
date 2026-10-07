use super::super::{audition::render_span, bridge_version::ARRANGEMENT_BRIDGE, concurrent::eager_all};
use super::ears::RawFile;
use super::rig::Rig;
use super::*;
use crate::ears::capture::{frame_at, read_capture, runs, write_capture_wav, Anchors};
use kumi_common::js::number::{round, to_string};
impl Rendering {
    pub(super) async fn ears_pass(self: &Rc<Self>, rig: &mut Rig, signal: Signal) -> Result<IndexMap<String, Render>, RuntimeError> {
        let tempo = self.observer.tempo.get().unwrap_or(f64::NAN);
        let ears = rig.ears.as_ref().unwrap();
        let link = ears.link.clone();
        let taps: Vec<_> = ears.taps.iter().map(|(name, tap)| (name.clone(), tap.clone())).collect();
        let (main_ref, prior) = self.main_prior(rig, signal.clone()).await?;
        let held = rig.hold.is_some();
        let window = rig.window();
        let mut started = false;
        let mut files = IndexMap::new();
        let result: Result<(), RuntimeError> = self
            .history
            .quietly(None, async {
                if rig.hold.as_ref().and_then(|hold| hold.main.as_ref()).is_none() {
                    // Main goes quiet only once its level is noted for after a crash.
                    if !self.save_main(rig, prior, false) {
                        return Err(observation(MAIN_UNNOTED));
                    }
                    self.step("set_mixer", json!({"trackRef":main_ref,"volume":0}), signal.clone()).await?;
                    if let Some(hold) = &mut rig.hold {
                        hold.main = Some((main_ref, prior));
                    }
                }
                let beat_ms = 60. / tempo * 1000.;
                for longer in [false, true] {
                    let span = render_span(window.from, window.beats, self.observer.beats_per_bar.get(), tempo, longer, Some(0.));
                    let prime_key = to_string(span.position);
                    let priming = rig.hold.as_ref().is_none_or(|hold| hold.primed.as_ref() != Some(&prime_key));
                    if priming && self.supported(ARRANGEMENT_BRIDGE) {
                        self.step("play", json!({"action":"back-to-arrangement"}), signal.clone()).await?;
                    }
                    if priming && rig.transport.looped != Some(false) {
                        self.step("set_transport", json!({"loopEnabled":false}), signal.clone()).await?;
                    }
                    let seconds = (span.wait + 4. * self.observer.beats_per_bar.get()) * beat_ms / 1000. + 6.;
                    started = true;
                    eager_all(taps.iter().map(|(_, tap)| {
                        let link = link.clone();
                        let signal = signal.clone();
                        async move {
                            link.arm(tap, seconds, Some(signal)).await.map_err(plain)?;
                            Ok::<_, RuntimeError>(())
                        }
                    }))
                    .await?;
                    self.step("play", json!({"action":"continue"}), signal.clone()).await?;
                    let probe = &taps.first().ok_or_else(|| plain("Cannot read properties of undefined (reading '1')"))?.1;
                    let mut first = None;
                    for _ in 0..8 {
                        first = link.transport(probe, Some(signal.clone())).await.map_err(plain)?;
                        if first.as_ref().is_some_and(|first| first.running) {
                            break;
                        }
                        delay(15., signal.clone()).await?;
                    }
                    let there = first
                        .as_ref()
                        .is_some_and(|first| first.running && first.beats >= span.position - 0.01 && first.beats <= span.position + 0.25);
                    if !there {
                        self.step("set_transport", json!({"position":span.position}), signal.clone()).await?;
                    }
                    let end = window.from + window.beats + self.observer.beats_per_bar.get() / 2.;
                    let deadline = now_ms() as f64 + seconds * 1000.;
                    let mut jumped = there;
                    while (now_ms() as f64) < deadline {
                        let now = link.transport(probe, Some(signal.clone())).await.map_err(plain)?;
                        if let Some(now) = now.as_ref().filter(|now| now.running) {
                            if now.beats >= span.position - 0.01 && now.beats < window.from.max(span.position + 0.5) {
                                jumped = true;
                            }
                            if jumped && now.beats >= end {
                                break;
                            }
                        }
                        let ahead = if jumped { now.map(|now| (end - now.beats) * beat_ms).unwrap_or(0.) } else { 0. };
                        delay((ahead * 0.8).clamp(20., 500.), signal.clone()).await?;
                    }
                    if let Some(hold) = &mut rig.hold {
                        hold.primed = Some(prime_key);
                    }
                    self.step("play", json!({"action":"stop"}), signal.clone()).await?;
                    started = false;
                    let late = Cell::new(false);
                    let notes = RefCell::new(vec![]);
                    let collected = RefCell::new(IndexMap::new());
                    eager_all(taps.iter().map(|(name, tap)| {
                        let signal = signal.clone();
                        let link = link.clone();
                        let late = &late;
                        let notes = &notes;
                        let collected = &collected;
                        async move {
                            let raw = RawFile(self.ears_folder.join(format!("{}.raw", uuid::Uuid::new_v4())));
                            let written: Result<(), RuntimeError> = async {
                                let written =
                                    link.write(tap, &raw.0.to_string_lossy().replace('\\', "/"), Some(signal)).await.map_err(plain)?;
                                let capture = read_capture(&raw.0, written.channels, written.sample_rate).await.map_err(plain)?;
                                let stretches = runs(&capture, Anchors { first: Some(written.beats), after_jump: Some(span.position) });
                                let part = stretches.iter().rev().find(|run| {
                                    frame_at(run, window.from).is_some() && frame_at(run, window.from + window.beats - 1e-3).is_some()
                                });
                                let Some(part) = part else {
                                    if !stretches.is_empty() {
                                        late.set(true);
                                    }
                                    return Ok(());
                                };
                                let at = frame_at(part, window.from).unwrap() as f64;
                                let lead = round(0.1 * capture.sample_rate);
                                let end = (part.to as f64)
                                    .min(at + ((window.beats + self.observer.beats_per_bar.get() / 2.) * part.samples_per_beat).ceil());
                                let wav = self.ears_folder.join(format!("{}.wav", uuid::Uuid::new_v4()));
                                write_capture_wav(&wav, &capture, at - lead, end).await.map_err(plain)?;
                                collected.borrow_mut().insert(
                                    name.clone(),
                                    Render { file: wav.to_string_lossy().into_owned(), start: at.min(lead) / capture.sample_rate },
                                );
                                Ok(())
                            }
                            .await;
                            if let Err(error) = written {
                                notes.borrow_mut().push(kumi_common::js::string::head(&error.to_string(), 200));
                            }
                            drop(raw);
                            Ok::<_, RuntimeError>(())
                        }
                    }))
                    .await?;
                    files.extend(collected.into_inner());
                    rig.notes.extend(notes.into_inner());
                    if !late.get() {
                        break;
                    }
                    if longer {
                        rig.notes.push("Live jumped past the part's start before Kumi could hear it; listen again.".into());
                    }
                }
                Ok(())
            })
            .await;
        let cleanup = self.cleanup();
        if started {
            self.history.stop_everything(cleanup.clone()).await;
        }
        for (_, tap) in &taps {
            link.stop(tap);
        }
        if !held {
            if self.history.quietly(None, self.put_main_back(prior, cleanup)).await {
                self.clear_restore();
            } else {
                rig.notes.push(format!("Main may still be silent: set it back to {} in Live.", fader_db(prior)));
            }
        }
        let this = self.clone();
        tokio::task::spawn_local(async move {
            this.prune_ears().await;
        });
        result?;
        Ok(files)
    }
}
fn plain(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}
