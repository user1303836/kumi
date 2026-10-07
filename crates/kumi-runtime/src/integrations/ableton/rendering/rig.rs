use super::super::audition::MainRestore;
use super::*;
use crate::ears::link::Tap;

pub(super) struct Source {
    pub track: String,
    pub name: String,
    pub scratch: String,
    pub label: String,
    pub clip: Option<String>,
    pub scene: Option<usize>,
    pub mix: bool,
}
#[derive(Default)]
pub(super) struct Held {
    pub main: Option<(String, f64)>,
    pub primed: Option<String>,
    pub recording: bool,
    pub rearm: Vec<String>,
}
#[derive(Clone, Copy)]
pub(super) struct Window {
    pub from: f64,
    pub beats: f64,
}
#[derive(Default)]
pub(super) struct Transport {
    pub position: Option<f64>,
    pub looped: Option<bool>,
}
pub(super) struct RigEars {
    pub link: Rc<dyn EarsLink>,
    pub taps: IndexMap<String, Tap>,
}
pub(super) struct Rig {
    pub tag: String,
    pub sources: Vec<Source>,
    pub clips: bool,
    pub hold: Option<Held>,
    pub window: Option<Window>,
    pub from: f64,
    pub beats: f64,
    pub steps: Vec<String>,
    pub transport: Transport,
    pub notes: Vec<String>,
    pub ears: Option<RigEars>,
}
impl Rig {
    pub fn window(&self) -> Window {
        self.window.unwrap_or(Window { from: self.from, beats: self.beats })
    }
}
impl Rendering {
    pub(super) async fn transport_now(&self, signal: Signal) -> Transport {
        let rows = self.rows("set", json!({"fields":["position","loop"]}), signal).await.unwrap_or_default();
        let Some(set) = rows.first() else { return Transport::default() };
        Transport {
            position: set.get("position").and_then(Value::as_f64),
            looped: set.get("loop").and_then(|v| v.get("enabled")).and_then(Value::as_bool),
        }
    }
    pub(super) async fn open_rig(
        self: &Rc<Self>,
        candidates: &[AuditionCandidate],
        from: Option<f64>,
        beats: Option<f64>,
        signal: Signal,
    ) -> Result<Rig, RuntimeError> {
        let mut rig = Rig {
            tag: uuid::Uuid::new_v4().to_string()[..4].into(),
            sources: vec![],
            from: from.unwrap_or(0.),
            beats: beats.unwrap_or(8.),
            steps: vec![],
            notes: vec![],
            clips: candidates.iter().any(|c| c.clip.as_ref().is_some_and(|s| !s.is_empty())),
            transport: self.transport_now(signal.clone()).await,
            hold: None,
            window: None,
            ears: None,
        };
        let link = self.ears_ready(signal.clone()).await?;
        let tracks = self.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
        let mut shared = false;
        for (index, candidate) in candidates.iter().enumerate() {
            if candidate.mix == Some(true) {
                if !rig.sources.iter().any(|source| source.mix) {
                    rig.sources.push(Source {
                        track: MIX_CANDIDATE.into(),
                        name: MIX_CANDIDATE.into(),
                        scratch: format!("Kumi · render {} {}", rig.sources.len() + 1, rig.tag),
                        label: candidate.label.clone().unwrap_or_else(|| "The whole mix".into()),
                        mix: true,
                        clip: None,
                        scene: None,
                    });
                }
                continue;
            }
            let found = tracks
                .iter()
                .find(|track| track.get("ref").and_then(Value::as_str) == Some(&candidate.track))
                .or_else(|| tracks.iter().find(|track| track.get("name").and_then(Value::as_str) == Some(&candidate.track)));
            let name = found
                .and_then(|track| track.get("name"))
                .and_then(Value::as_str)
                .ok_or_else(|| observation(format!("{} isn't a track in this turn's discovery; discover again.", candidate.track)))?;
            if tracks.iter().filter(|track| track.get("name").and_then(Value::as_str) == Some(name)).count() > 1 {
                if link.is_none() {
                    return Err(observation(format!("Two tracks are named “{name}”; rename one so Kumi can render it.")));
                }
                shared = true;
            }
            if rig.sources.iter().any(|source| source.name == name) {
                continue;
            }
            rig.sources.push(Source {
                track: found.and_then(|track| track.get("ref")).map(js_string).unwrap_or_else(|| "undefined".into()),
                name: name.into(),
                scratch: format!("Kumi · render {} {}", rig.sources.len() + 1, rig.tag),
                label: candidate.label.clone().unwrap_or_else(|| format!("Candidate {}", index + 1)),
                clip: candidate.clip.clone().filter(|s| !s.is_empty()),
                scene: None,
                mix: false,
            });
        }
        let mut steps = vec![];
        let result: Result<(),RuntimeError> = self.history.quietly(Some(&mut steps),async {
            if rig.clips {
                let end = if self.connection().has("live_song_state") {
                    let song=self.connection().call("live_song_state",JsonObject::new(),signal.clone()).await?;
                    super::super::context::payload(&song)?.get("songLength").and_then(Value::as_f64).unwrap_or(0.)
                } else {0.};
                let meter=self.observer.beats_per_bar.get();rig.from=((end/meter).ceil()+2.)*meter;
                let mut longest: f64=0.;
                for index in 0..rig.sources.len() {
                    let track=rig.sources[index].track.clone();let clip=rig.sources[index].clip.clone();
                    let (length,scene)=self.copy_clip(&mut rig,&track,clip.as_deref(),signal.clone()).await?;
                    rig.sources[index].scene=scene;longest=if length.is_nan() || longest.is_nan() {f64::NAN} else {longest.max(length)};
                }
                rig.beats=beats.unwrap_or_else(||32_f64.min(if longest==0. || longest.is_nan() {8.} else {longest}));
            }
            if let Some(link)=link {
                rig.ears=Some(RigEars{link,taps:IndexMap::new()});
                if let Err(error)=self.place_taps(&mut rig,signal.clone()).await {
                    signal.check()?;
                    // Quiet steps have not been copied out until this scope ends; removeTaps needs them now.
                    // The history's current quiet list is retained as well, matching source nested undo bookkeeping.
                    self.remove_taps(&mut rig).await;
                    if error.silent { self.ears_refused.set(true);rig.notes.push("Kumi's listening device didn't start in this Live (it needs Max for Live), so Kumi records to listen instead.".into()); }
                    if shared {return Err(error.error);}
                    rig.ears=None;
                    self.add_scratch(&rig.sources,signal.clone()).await?;
                }
            } else { self.add_scratch(&rig.sources,signal.clone()).await?; }
            Ok(())
        }).await;
        rig.steps.extend(steps);
        if let Err(error) = result {
            self.close_rig(&mut rig).await;
            return Err(error);
        }
        Ok(rig)
    }
    pub(super) async fn copy_clip(
        &self,
        rig: &mut Rig,
        track: &str,
        clip_ref: Option<&str>,
        signal: Signal,
    ) -> Result<(f64, Option<usize>), RuntimeError> {
        let slots = self.rows("clip-slot", json!({"parent":track,"fields":["clipRef"]}), signal.clone()).await?;
        let scene = clip_ref
            .and_then(|s| s.strip_prefix("scene:"))
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<usize>().ok());
        let selected = if let Some(scene) = scene {
            slots.get(scene).map(|slot| (scene, slot))
        } else if let Some(clip) = clip_ref.filter(|s| !s.is_empty() && *s != "first") {
            slots.iter().enumerate().find(|(_, slot)| slot.get("clipRef").and_then(Value::as_str) == Some(clip))
        } else {
            slots.iter().enumerate().find(|(_, slot)| slot.get("clipRef").is_some_and(Value::is_string))
        };
        let Some((scene, slot)) = selected.filter(|(_, slot)| slot.get("clipRef").is_some_and(Value::is_string)) else {
            if let Some(reference) = clip_ref.filter(|s| !s.is_empty() && *s != "first") {
                return Err(observation(format!("{reference} isn't a Session clip on that track; discover its clip slots again.")));
            }
            rig.notes.push("A candidate has no Session clip to play, so it renders silent.".into());
            return Ok((0., None));
        };
        let clip = self.rows("session-clip", json!({"parent":slot.get("ref"),"fields":["length"]}), signal.clone()).await?;
        self.step("duplicate_clip", json!({"clipRef":slot["clipRef"],"arrangementPosition":rig.from}), signal).await?;
        Ok((clip.first().and_then(|v| v.get("length")).and_then(Value::as_f64).unwrap_or(0.), Some(scene)))
    }
    pub(super) async fn add_scratch(&self, sources: &[Source], signal: Signal) -> Result<(), RuntimeError> {
        self.step(
            "add_tracks_and_scenes",
            json!({"tracks":sources.iter().map(|source|json!({"name":source.scratch,"kind":"audio"})).collect::<Vec<_>>(),"scenes":[]}),
            signal.clone(),
        )
        .await?;
        let tracks = self.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
        for source in sources {
            let reference = tracks
                .iter()
                .find(|track| track.get("name").and_then(Value::as_str) == Some(&source.scratch))
                .and_then(|t| t.get("ref"))
                .and_then(Value::as_str)
                .ok_or_else(|| observation("Kumi's scratch track didn't appear."))?;
            let args = if source.mix {
                json!({"trackRef":reference,"inputType":"Resampling","arm":true,"monitoring":"off"})
            } else {
                json!({"trackRef":reference,"inputType":source.name,"inputSubRouting":"Post FX","arm":true,"monitoring":"off"})
            };
            self.step("set_routing", args, signal.clone()).await?;
        }
        Ok(())
    }
    pub(super) async fn add_to_rig(&self, rig: &mut Rig, mut source: Source, signal: Signal) -> Result<(), RuntimeError> {
        source.scratch = format!("Kumi · render {} {}", rig.sources.len() + 1, rig.tag);
        let mut steps = vec![];
        let result = self
            .history
            .quietly(Some(&mut steps), async {
                if rig.clips {
                    source.scene = self.copy_clip(rig, &source.track, source.clip.as_deref().or(Some("first")), signal.clone()).await?.1;
                }
                if let Some(ears) = &mut rig.ears {
                    let tap = self.place_tap(ears.link.clone(), &source.track, signal.clone()).await.map_err(|e| e.error)?;
                    ears.taps.insert(source.name.clone(), tap);
                } else {
                    self.add_scratch(std::slice::from_ref(&source), signal).await?;
                }
                Ok::<_, RuntimeError>(())
            })
            .await;
        rig.steps.extend(steps);
        result?;
        rig.sources.push(source);
        Ok(())
    }
    pub(super) async fn main_prior(&self, rig: &Rig, signal: Signal) -> Result<(String, f64), RuntimeError> {
        if let Some(main) = rig.hold.as_ref().and_then(|held| held.main.clone()) {
            return Ok(main);
        }
        let (reference, volume) = self.main_volume(signal).await?;
        let mut prior = volume.ok_or_else(|| observation("Live didn't say Main's level, so Kumi won't touch it."))?;
        if prior == 0. {
            if let Some(pending) = self.restore.as_ref().and_then(RestoreStore::load) {
                let path = pending.get("path").and_then(Value::as_str).filter(|s| !s.is_empty());
                let same = if let Some(path) = path {
                    self.history.remember.current().and_then(|p| p.path.clone()).as_deref() == Some(path)
                } else {
                    pending.get("set").and_then(Value::as_str) == self.connection().set.borrow().as_deref()
                };
                if same {
                    prior = pending["volume"].as_f64().unwrap();
                }
            }
        }
        Ok((reference, prior))
    }
    /// Notes Main's level to put back after a crash: whether it's noted (or there's no journal to keep).
    pub(super) fn save_main(&self, rig: &Rig, volume: f64, scratch: bool) -> bool {
        let Some(restore) = &self.restore else { return true };
        restore.save(&MainRestore {
            set: self.connection().set.borrow().clone().unwrap_or_default(),
            path: self.history.remember.current().and_then(|p| p.path.clone()),
            volume,
            at: self.connection().now().timestamp_millis() as f64,
            scratch: scratch.then(|| rig.sources.iter().map(|s| s.scratch.clone()).collect()),
        })
    }
    pub(super) fn clear_restore(&self) {
        if let Some(restore) = &self.restore {
            restore.clear();
        }
    }
    /// Takes back one of Kumi's own steps (a listening device, an earlier best take) and forgets it. One Live won't
    /// take back stays in the history, and the producer hears what's left in their Set.
    pub(super) async fn take_back(&self, id: &str, signal: Signal, discard: bool) {
        let title = self.history.entries.borrow().get(id).map(|entry| entry.borrow().record.title.clone());
        match self.history.undo(id, signal, discard).await {
            Ok(undone) if !undone.is_error => {
                self.history.entries.borrow_mut().shift_remove(id);
            }
            failed => {
                let why = failed.map(|undone| undone.text).unwrap_or_else(|error| error.to_string());
                self.tell(
                    &format!(
                        "Couldn't take back “{}” ({}); check it in Live.",
                        title.unwrap_or_else(|| "a step of Kumi's".into()),
                        kumi_common::js::string::head(&why, 200)
                    ),
                    None,
                );
            }
        }
    }
    pub(super) async fn close_rig(&self, rig: &mut Rig) {
        let cleanup = self.cleanup();
        let scratch: Vec<_> = rig.sources.iter().map(|s| s.scratch.clone()).collect();
        if let Some(held) = &mut rig.hold {
            if held.recording {
                let _ =
                    self.history.quietly(None, self.step("record", json!({"action":"stop","lane":"arrangement"}), cleanup.clone())).await;
                held.recording = false;
            }
            self.history.stop_everything(cleanup.clone()).await;
            for reference in held.rearm.drain(..) {
                if self
                    .history
                    .quietly(None, self.step("set_routing", json!({"trackRef":reference,"arm":true}), cleanup.clone()))
                    .await
                    .is_err()
                {
                    rig.notes.push("A track Kumi disarmed to render may still be disarmed; arm it again in Live.".into());
                }
            }
            if let Some((_, prior)) = held.main.clone() {
                if self.history.quietly(None, self.put_main_back(prior, cleanup.clone())).await {
                    self.clear_restore();
                } else {
                    rig.notes.push(format!("Main may still be silent: set it back to {} in Live.", fader_db(prior)));
                }
            }
        }
        self.history
            .quietly(None, async {
                for id in rig.steps.iter().rev() {
                    let entry = self.history.entries.borrow().get(id).map(|entry| entry.borrow().record.clone());
                    let Some(record) = entry.filter(|record| {
                        record.state == ChangeState::Applied
                            && !(record.family != ChangeFamily::Structure
                                && record.track.as_ref().is_some_and(|track| scratch.contains(&track.name)))
                    }) else {
                        continue;
                    };
                    let undone = self.history.undo(id, cleanup.clone(), record.family == ChangeFamily::Structure).await.ok();
                    if undone.as_ref().is_none_or(|undone| undone.is_error) {
                        if record.family == ChangeFamily::Structure {
                            let tracks = self.rows("track", json!({"fields":["name"]}), cleanup.clone()).await.unwrap_or_default();
                            for name in &scratch {
                                if let Some(reference) = tracks
                                    .iter()
                                    .find(|t| t.get("name").and_then(Value::as_str) == Some(name))
                                    .and_then(|t| t.get("ref"))
                                    .and_then(Value::as_str)
                                {
                                    let _ = self.step("set_routing", json!({"trackRef":reference,"arm":false}), cleanup.clone()).await;
                                }
                            }
                        }
                        rig.notes.push(format!(
                            "Couldn't take back “{}” ({}); {}.",
                            record.title,
                            kumi_common::js::string::head(undone.as_ref().map(|u| u.text.as_str()).unwrap_or("no answer"), 200),
                            if record.family == ChangeFamily::Structure {
                                "delete Kumi's render track by hand"
                            } else {
                                "check it in Live"
                            }
                        ));
                    }
                }
                for id in &rig.steps {
                    self.history.entries.borrow_mut().shift_remove(id);
                }
                if rig.transport.position.is_some() || rig.transport.looped.is_some() {
                    let mut args = JsonObject::new();
                    if let Some(position) = rig.transport.position {
                        args.insert("position".into(), json!(position));
                    }
                    if let Some(looped) = rig.transport.looped {
                        args.insert("loopEnabled".into(), json!(looped));
                    }
                    let _ = (self.step)("set_transport".into(), args, cleanup).await;
                }
            })
            .await;
    }
}
