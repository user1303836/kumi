use super::super::{audition::render_span, bridge_version::ARRANGEMENT_BRIDGE, concurrent::eager_all};
use super::rig::Rig;
use super::*;
use kumi_common::js::number::to_string;

impl Rendering {
    pub(super) async fn render_pass(self: &Rc<Self>, rig: &mut Rig, signal: Signal) -> Result<IndexMap<String, Render>, RuntimeError> {
        if rig.ears.is_some() {
            return self.ears_pass(rig, signal).await;
        }
        let tempo = self.observer.tempo.get().unwrap_or(f64::NAN);
        let mut lap_at = now_ms();
        let mut laps = vec![];
        let mut lap = |what: &str| {
            let now = now_ms();
            laps.push(format!("{what} {}", now - lap_at));
            lap_at = now;
        };
        let (main_ref, prior) = self.main_prior(rig, signal.clone()).await?;
        lap("main read");
        let held = rig.hold.is_some();
        let window = rig.window();
        let mut started = false;
        let mut rearm = rig.hold.as_mut().map(|hold| std::mem::take(&mut hold.rearm)).unwrap_or_default();
        let mut files = IndexMap::new();
        let result: Result<(), RuntimeError> = self
            .history
            .quietly(None, async {
                if rig.hold.as_ref().and_then(|held| held.main.as_ref()).is_none() {
                    // Main goes quiet only once its level is noted for after a crash.
                    if !self.save_main(rig, prior, true) {
                        return Err(observation(MAIN_UNNOTED));
                    }
                    self.step("set_mixer", json!({"trackRef":main_ref,"volume":0}), signal.clone()).await?;
                    if let Some(held) = &mut rig.hold {
                        held.main = Some((main_ref, prior));
                    }
                }
                lap("main down");
                let tracks = self.rows("track", json!({"fields":["name","armed"]}), signal.clone()).await?;
                lap("tracks read");
                let mut indices = vec![];
                for source in &rig.sources {
                    let index = tracks
                        .iter()
                        .position(|track| track.get("name").and_then(Value::as_str) == Some(&source.scratch))
                        .filter(|index| tracks[*index].get("ref").is_some_and(Value::is_string))
                        .ok_or_else(|| observation(format!("Kumi's render track “{}” is gone.", source.scratch)))?;
                    indices.push(index);
                }
                let refs: Vec<_> = indices.iter().map(|index| &tracks[*index]).collect();
                for track in &refs {
                    if track.get("armed") != Some(&Value::Bool(true)) {
                        self.step("set_routing", json!({"trackRef":track["ref"],"arm":true}), signal.clone()).await?;
                    }
                }
                for (index, track) in tracks.iter().enumerate() {
                    if track.get("armed") != Some(&Value::Bool(true))
                        || !track.get("ref").is_some_and(Value::is_string)
                        || indices.contains(&index)
                    {
                        continue;
                    }
                    self.step("set_routing", json!({"trackRef":track["ref"],"arm":false}), signal.clone()).await?;
                    rearm.push(track["ref"].as_str().unwrap().to_owned());
                }
                let destination = refs.first().ok_or_else(|| RuntimeError::plain("Cannot read properties of undefined (reading 'ref')"))?;
                let mut record = object(json!({"action":"start","lane":"arrangement","destinationTrackRef":destination.get("ref")}));
                if refs.len() > 1 {
                    record.insert("alsoTrackRefs".into(), json!(refs.iter().skip(1).map(|track| track["ref"].clone()).collect::<Vec<_>>()));
                }
                let beat_ms = 60. / tempo * 1000.;
                for longer in [false, true] {
                    let span = render_span(window.from, window.beats, self.observer.beats_per_bar.get(), tempo, longer, None);
                    let prime_key = to_string(span.position);
                    let priming = rig.hold.as_ref().is_none_or(|held| held.primed.as_deref() != Some(&prime_key));
                    if priming && self.supported(ARRANGEMENT_BRIDGE) {
                        self.step("play", json!({"action":"back-to-arrangement"}), signal.clone()).await?;
                        lap("back to arrangement");
                    }
                    if rig.hold.as_ref().is_some_and(|held| held.recording) {
                        self.step("record", json!({"action":"stop","lane":"arrangement"}), signal.clone()).await?;
                        rig.hold.as_mut().unwrap().recording = false;
                    }
                    started = true;
                    if span.position > 0. {
                        self.step("play", json!({"action":"continue"}), signal.clone()).await?;
                        lap("play");
                        let mut transport = object(json!({"position":span.position}));
                        if priming {
                            transport.insert("loopEnabled".into(), json!(false));
                        }
                        (self.step)("set_transport".into(), transport, signal.clone()).await?;
                        let jumped = now_ms();
                        lap("jump");
                        (self.step)("record".into(), record.clone(), signal.clone()).await?;
                        if let Some(held) = &mut rig.hold {
                            held.recording = true;
                        }
                        lap("record start");
                        delay((span.wait * beat_ms - (now_ms() - jumped) as f64).max(0.), signal.clone()).await?;
                    } else {
                        self.step("play", json!({"action":"stop"}), signal.clone()).await?;
                        self.step("play", json!({"action":"stop"}), signal.clone()).await?;
                        if priming && rig.transport.looped != Some(false) {
                            self.step("set_transport", json!({"loopEnabled":false}), signal.clone()).await?;
                        }
                        lap("to the start");
                        (self.step)("record".into(), record.clone(), signal.clone()).await?;
                        if let Some(held) = &mut rig.hold {
                            held.recording = true;
                        }
                        lap("record start");
                        let now = self.rows("set", json!({"fields":["position","playing"]}), signal.clone()).await?;
                        let playing = now.first().and_then(|set| set.get("playing")) == Some(&Value::Bool(true));
                        if !playing {
                            self.step("play", json!({"action":"start"}), signal.clone()).await?;
                            lap("play");
                        }
                        let mut at = if playing {
                            now.first().and_then(|set| set.get("position")).and_then(Value::as_f64).unwrap_or(0.)
                        } else {
                            0.
                        };
                        for _ in 0..4 {
                            if !(at < span.wait - 0.25) {
                                break;
                            }
                            delay((span.wait - at) * beat_ms, signal.clone()).await?;
                            let read = self.rows("set", json!({"fields":["position"]}), signal.clone()).await?;
                            let Some(read) = read.first().and_then(|set| set.get("position")).and_then(Value::as_f64) else { break };
                            at = read;
                        }
                    }
                    if let Some(held) = &mut rig.hold {
                        held.primed = Some(prime_key);
                    }
                    lap("wait");
                    self.step("play", json!({"action":"stop"}), signal.clone()).await?;
                    lap("stop");
                    if !held {
                        self.step("record", json!({"action":"stop","lane":"arrangement"}), signal.clone()).await?;
                        lap("record stop");
                    } else {
                        rig.hold.as_mut().unwrap().recording = false;
                    }
                    started = false;
                    let late = Cell::new(false);
                    let collected = RefCell::new(IndexMap::new());
                    eager_all(rig.sources.iter().enumerate().map(|(index, source)| {
                        let reference = refs[index]["ref"].clone();
                        let signal = signal.clone();
                        let late = &late;
                        let collected = &collected;
                        async move {
                            let clips = self
                                .rows("arrangement-clip", json!({"parent":reference,"fields":["start","length","isAudio"]}), signal.clone())
                                .await?
                                .into_iter()
                                .filter(|row| {
                                    row.get("isAudio") == Some(&Value::Bool(true)) && row.get("ref").is_some_and(Value::is_string)
                                })
                                .collect::<Vec<_>>();
                            let clip = clips
                                .iter()
                                .enumerate()
                                .filter(|(_, clip)| clip.get("start").and_then(Value::as_f64).is_some_and(|start| start <= window.from))
                                .max_by(|(a, x), (b, y)| {
                                    x["start"]
                                        .as_f64()
                                        .unwrap()
                                        .partial_cmp(&y["start"].as_f64().unwrap())
                                        .unwrap_or(std::cmp::Ordering::Equal)
                                        .then(a.cmp(b))
                                })
                                .map(|(_, clip)| clip)
                                .or_else(|| clips.last());
                            let Some(clip) = clip else { return Ok::<_, RuntimeError>(()) };
                            let start = clip.get("start").and_then(Value::as_f64);
                            if start.is_some_and(|start| start > window.from + 1e-3) {
                                late.set(true);
                            }
                            if let Some(file) = (self.clip_file)(clip["ref"].as_str().unwrap().into(), signal).await.ok().flatten() {
                                collected.borrow_mut().insert(
                                    source.name.clone(),
                                    Render { file, start: ((window.from - start.unwrap_or(span.position)) * 60. / tempo - 0.1).max(0.) },
                                );
                            }
                            Ok(())
                        }
                    }))
                    .await?;
                    files.extend(collected.into_inner());
                    lap("clips and files");
                    if !late.get() {
                        break;
                    }
                    if longer {
                        rig.notes
                            .push("Live started recording after the part had begun, so a take may miss its start; render it again.".into());
                    } else {
                        lap("late: again with a longer lead-in");
                    }
                }
                Ok(())
            })
            .await;
        let cleanup = self.cleanup();
        if started {
            self.history.stop_everything(cleanup.clone()).await;
            if let Some(held) = &mut rig.hold {
                held.recording = false;
            }
        }
        if !held || result.is_err() {
            for reference in rearm.drain(..) {
                if self
                    .history
                    .quietly(None, self.step("set_routing", json!({"trackRef":reference,"arm":true}), cleanup.clone()))
                    .await
                    .is_err()
                {
                    rig.notes.push("A track Kumi disarmed to render may still be disarmed; arm it again in Live.".into());
                }
            }
            if !held {
                let back = self.history.quietly(None, self.put_main_back(prior, cleanup)).await;
                lap("main back");
                if back {
                    self.clear_restore();
                } else {
                    rig.notes.push(format!("Main may still be silent: set it back to {} in Live.", fader_db(prior)));
                }
            }
        } else {
            rig.hold.as_mut().unwrap().rearm = rearm;
        }
        if std::env::var("KUMI_TIMING").is_ok_and(|s| !s.is_empty()) {
            eprintln!("[render pass · {} sources{}] {} ms", rig.sources.len(), if held { " · held" } else { "" }, laps.join(" · "));
        }
        result?;
        Ok(files)
    }
}
