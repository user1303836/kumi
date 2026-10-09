use super::super::{audition::render_span, bridge_version::ARRANGEMENT_BRIDGE, concurrent::eager_all};
use super::rig::Rig;
use super::*;
use kumi_common::js::number::to_string;

/// How long the record pass's takes run before the part, seconds: a lead for its first hit to rise out of.
pub(super) const RECORD_LEAD: f64 = 0.1;
/// A second of a take as a pass may hold it: stereo floats at 96 kHz.
const TAKE_BYTES_PER_SECOND: f64 = 96_000. * 2. * 4.;

/// Whether a clip's file is Live's own recording of one of Kumi's render tracks: in a Recorded folder, named after
/// the track. Anything else a clip holds may be the producer's.
pub(super) fn kumi_recording(file: &std::path::Path, tracks: &[String]) -> bool {
    file.parent().and_then(|folder| folder.file_name()).is_some_and(|folder| folder == "Recorded")
        && file
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| tracks.iter().any(|track| name.starts_with(&format!("{track} "))))
}

impl Rendering {
    pub(super) async fn render_pass(self: &Rc<Self>, rig: &mut Rig, signal: Signal) -> Result<IndexMap<String, Render>, RuntimeError> {
        // A pass writes each take before it's read: with too little room for them, it's refused before Live plays.
        if let Some(why) = self.no_room_for(rig).await {
            return Err(observation(why));
        }
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
        let held = rig.hold.is_some();
        let window = rig.window();
        let mut started = false;
        let mut rearm = rig.hold.as_mut().map(|hold| std::mem::take(&mut hold.rearm)).unwrap_or_default();
        let mut files = IndexMap::new();
        // Every recording the pass makes, in the project: a late take's first try too.
        let recorded = RefCell::new(Vec::<String>::new());
        let result: Result<(), RuntimeError> = self
            .history
            .quietly(None, async {
                // Main stays as the producer has it: they hear the pass as Kumi does.
                if !rig.noted {
                    self.note_render(rig);
                }
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
                        let recorded = &recorded;
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
                                recorded.borrow_mut().push(file.clone());
                                collected.borrow_mut().insert(
                                    source.name.clone(),
                                    Render {
                                        file,
                                        start: ((window.from - start.unwrap_or(span.position)) * 60. / tempo - RECORD_LEAD).max(0.),
                                        seconds: None,
                                    },
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
        } else {
            rig.hold.as_mut().unwrap().rearm = rearm;
        }
        if std::env::var("KUMI_TIMING").is_ok_and(|s| !s.is_empty()) {
            eprintln!("[render pass · {} sources{}] {} ms", rig.sources.len(), if held { " · held" } else { "" }, laps.join(" · "));
        }
        // Live's recordings of the render tracks land in the project (Samples/Recorded, each named after its track):
        // Kumi reads copies in its own folder, and the recordings go with the render tracks (close_rig). Any other file
        // a clip holds, or one that can't be copied, is read where it is and stays.
        let tracks: Vec<String> = rig.sources.iter().map(|source| source.scratch.clone()).collect();
        let mut recorded: Vec<PathBuf> =
            recorded.into_inner().into_iter().map(PathBuf::from).filter(|file| kumi_recording(file, &tracks)).collect();
        if result.is_ok() {
            let _ = tokio::fs::create_dir_all(&self.ears_folder).await;
            for render in files.values_mut() {
                let original = PathBuf::from(&render.file);
                if !recorded.contains(&original) {
                    continue;
                }
                let extension = original.extension().and_then(|extension| extension.to_str()).unwrap_or("wav");
                let copy = self.ears_folder.join(format!("{}.{extension}", uuid::Uuid::new_v4()));
                match tokio::fs::copy(&original, &copy).await {
                    Ok(_) => render.file = copy.to_string_lossy().into_owned(),
                    Err(_) => recorded.retain(|file| *file != original),
                }
            }
        }
        rig.recorded.extend(recorded);
        // Its copies are Kumi's takes too: pruned as an Ears pass's are.
        let this = self.clone();
        tokio::task::spawn_local(async move {
            this.prune_ears().await;
        });
        result?;
        Ok(files)
    }

    /// Why a pass can't go ahead for lack of disk, when it can't. Kumi's own folder holds every take; a record pass's
    /// are Live's recordings first, in the project.
    async fn no_room_for(&self, rig: &Rig) -> Option<String> {
        let tempo = self.observer.tempo.get().filter(|tempo| *tempo > 0.)?;
        let seconds = rig.window().beats * 60. / tempo;
        let takes = rig.sources.len();
        if let Some(why) = self.no_room(seconds, takes, &std::env::temp_dir()).await {
            return Some(why);
        }
        match rig.ears {
            Some(_) => None,
            None => self.no_room(seconds, takes, &self.recordings_folder()).await,
        }
    }

    /// Why there's no room on the disk holding `folder` for `takes` takes of `seconds` each, as captured and as written
    /// out with some to spare, when there isn't.
    pub(super) async fn no_room(&self, seconds: f64, takes: usize, folder: &std::path::Path) -> Option<String> {
        let needed = (seconds + 10.) * TAKE_BYTES_PER_SECOND * 2. * takes.max(1) as f64 + 200e6;
        crate::core::disk::low_disk(&folder.to_string_lossy(), needed, "for Kumi to hear this").await
    }

    /// Where Live records a record pass's takes: in the Set's project, else (an unsaved Set) in a project of its own
    /// under the home folder.
    fn recordings_folder(&self) -> PathBuf {
        let current = self.history.remember.current.borrow().clone();
        current
            .and_then(|set| set.path.as_ref().and_then(|path| std::path::Path::new(path).parent().map(std::path::Path::to_path_buf)))
            .unwrap_or_else(|| home::home_dir().unwrap_or_else(std::env::temp_dir))
    }
}
