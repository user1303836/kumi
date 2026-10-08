use super::super::{
    audition::silent_render, bridge_version::RENDER_BRIDGE, concurrent::eager_all, connection::NO_CURRENT_LIVE, more_changes::bars,
};
use super::ears::RawFile;
use super::ears_pass::PASS_SECONDS;
use super::*;
use crate::{
    audio::{matching::closeness, tools::summary},
    ears::capture::{read_capture, write_capture_wav},
};
use kumi_common::js::{
    number::{round, to_string},
    string::head,
};
impl Rendering {
    pub async fn audition(
        self: &Rc<Self>,
        request: &AuditionRequest,
        original: Signal,
    ) -> Result<Result<AuditionResult, String>, RuntimeError> {
        let began = now_ms();
        if !self.available() {
            return Ok(Err(NO_CURRENT_LIVE.into()));
        }
        if !self.supported(RENDER_BRIDGE) {
            return Ok(Err(self.too_old(RENDER_BRIDGE)));
        }
        if self.rendering.get() {
            return Ok(Err("An audition is already running; wait for it.".into()));
        }
        let Some(tempo) = self.observer.tempo.get().filter(|value| *value != 0. && !value.is_nan()) else {
            return Ok(Err("Kumi doesn't know the Set's tempo yet; try again.".into()));
        };
        let signal = abort::any([original.clone(), self.connection().lifetime.clone()]);
        let mut notes = vec![];
        self.rounds.set(self.rounds.get() + 1);
        let round = self.rounds.get();
        let mut files = vec![];
        let mut takes: Vec<_> = request
            .candidates
            .iter()
            .enumerate()
            .map(|(i, candidate)| AuditionTake {
                label: candidate.label.clone().unwrap_or_else(|| format!("Candidate {}", i + 1)),
                track: candidate.track.clone(),
                r#where: None,
                silent: None,
                closeness: None,
                render: None,
                heard: None,
            })
            .collect();
        let mut beats = request.beats.unwrap_or(8.);
        if let Some(why) = too_long(beats, tempo, "audition a part") {
            return Ok(Err(why));
        }
        let rendering = self.rendering_now();
        let mut rig = None;
        let rendered: Result<(), RuntimeError> = async {
            self.tell(
                if !self.told_quietly.get() {
                    format!("Listening to my version quietly (about {} s a round)", to_string(round_number(beats * 60. / tempo + 6.)))
                } else {
                    format!("Round {round}: listening quietly")
                },
                Some(true),
            );
            self.told_quietly.set(true);
            rig = Some(self.open_rig(&request.candidates, request.from_beat, request.beats, signal.clone()).await?);
            let rig = rig.as_mut().unwrap();
            beats = rig.beats;
            let rendered = self.render_pass(rig, signal.clone()).await?;
            for (index, source) in rig.sources.iter().enumerate() {
                let at = takes.iter().position(|take| take.track == source.track || take.track == source.name).unwrap_or(index);
                let take = &mut takes[at];
                take.r#where = Some(Where { track: source.name.clone(), clip: source.scene.map(|scene| format!("scene:{scene}")) });
                if let Some(found) = rendered.get(&source.name) {
                    files.push((at, found.clone()));
                } else {
                    take.silent = Some(true);
                }
            }
            Ok(())
        }
        .await;
        if let Err(error) = rendered {
            notes.push(if original.is_cancelled() { "Stopped before it finished.".into() } else { head(&error.to_string(), 400) });
        }
        if let Some(rig) = rig.as_mut() {
            self.close_rig(rig).await;
            notes.extend(rig.notes.clone());
        }
        drop(rendering);
        let listen: Result<AuditionResult, RuntimeError> = async {
            let reference = match request.reference.as_deref().filter(|s| !s.is_empty()) {
                Some(named) => Some(self.heard_reference(named, request, signal.clone()).await?),
                None => None,
            };
            for (index, render) in &files {
                let seconds = render.seconds.unwrap_or(beats * 60. / tempo);
                let heard =
                    audio::hear(&render.file, heard_options(render.start, seconds, request.focus, signal.clone())).await.map_err(plain)?;
                let take = &mut takes[*index];
                take.heard = Some(Heard { lufs: heard.loudness.integrated_lufs, summary: summary(&heard) });
                take.render = Some(render.clone());
                if silent_render(&heard) {
                    take.silent = Some(true);
                    continue;
                }
                if let Some(reference) = &reference {
                    take.closeness = Some(closeness(&heard, reference, request.focus));
                }
            }
            let mut scored: Vec<_> = takes.iter().filter(|take| take.closeness.is_some()).collect();
            scored.sort_by(|a, b| b.closeness.as_ref().unwrap().score.total_cmp(&a.closeness.as_ref().unwrap().score));
            let best = scored.first().copied();
            let silent = takes.iter().all(|take| take.silent == Some(true)) && !files.is_empty();
            if silent {
                notes.push(
                    "The render was silent: is the source playing in the Arrangement at that spot (its clips there, the track not muted)?"
                        .into(),
                );
            }
            let previous = self.best.get();
            if let Some(best) = best {
                let score = best.closeness.as_ref().unwrap().score;
                if previous.is_none_or(|previous| score > previous) {
                    self.best.set(Some(score));
                }
            }
            let title = if let Some(best) = best {
                format!(
                    "Auditioned {}",
                    if takes.len() == 1 { best.label.clone() } else { format!("{} candidates · best {}", takes.len(), best.label) }
                )
            } else if silent {
                "Auditioned: the render was silent".into()
            } else {
                format!("Auditioned {}", if takes.len() == 1 { takes[0].label.clone() } else { format!("{} candidates", takes.len()) })
            };
            self.history.emit(&ChangeRecord {
                id: format!("a{}", &uuid::Uuid::new_v4().to_string()[..8]),
                family: ChangeFamily::Clip,
                title,
                state: ChangeState::Heard,
                score: best.and_then(|take| take.closeness.as_ref()).map(|close| close.score),
                at: self.connection().now().timestamp_millis(),
                track: None,
                from: None,
                to: None,
                range: None,
                colors: None,
                clip: None,
                devices: None,
                note: None,
            });
            if let Some(tell) = &self.on_audition {
                let event = AuditionEvent {
                    round: round as u32,
                    best: best.map(|take| BestTake { label: take.label.clone(), score: take.closeness.as_ref().unwrap().score }),
                    previous,
                    takes: scored
                        .iter()
                        .map(|take| TakeScore {
                            label: take.label.clone(),
                            score: take.closeness.as_ref().map(|close| close.score),
                            silent: None,
                            r#where: take.r#where.clone(),
                        })
                        .chain(takes.iter().filter(|take| take.closeness.is_none()).map(|take| TakeScore {
                            label: take.label.clone(),
                            score: None,
                            silent: take.silent.filter(|s| *s),
                            r#where: take.r#where.clone(),
                        }))
                        .collect(),
                    gaps: best
                        .and_then(|take| take.closeness.as_ref())
                        .map(|close| close.gaps.iter().take(3).cloned().collect())
                        .unwrap_or_default(),
                    request: Some(request.clone()),
                    reference: reference.as_ref().map(summary),
                    structural: best
                        .and_then(|take| take.closeness.as_ref())
                        .and_then(|close| close.structural.as_ref())
                        .map(|s| StructuralMove { gap: s.gap.clone(), r#move: s.r#move.clone() }),
                };
                let _ = catch_unwind(AssertUnwindSafe(|| tell(event)));
            }
            self.tell(
                best.map(|take| format!("Auditioned · {}%", to_string(take.closeness.as_ref().unwrap().score)))
                    .unwrap_or_else(|| "Auditioned".into()),
                Some(false),
            );
            Ok(AuditionResult {
                best: best.map(|take| take.label.clone()),
                reference: reference.map(|heard| ReferenceHeard { summary: summary(&heard), file: heard.file }),
                seconds: round_number((now_ms() - began) as f64 / 100.) / 10.,
                takes,
                notes: notes.clone(),
            })
        }
        .await;
        match listen {
            Ok(result) => Ok(Ok(result)),
            Err(error) => {
                self.tell("Auditioned", Some(false));
                signal.check()?;
                Ok(Err(format!(
                    "{}Kumi couldn't listen to the render: {}",
                    if notes.is_empty() { String::new() } else { format!("{} ", notes.join(" ")) },
                    head(&error.to_string(), 300)
                )))
            }
        }
    }
    pub async fn hear_in_set(
        self: &Rc<Self>,
        request: &HearRequest,
        original: Signal,
    ) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        if !self.available() {
            return Ok(Err(NO_CURRENT_LIVE.into()));
        }
        let Some(tempo) = self.observer.tempo.get().filter(|value| *value != 0. && !value.is_nan()) else {
            return Ok(Err("Kumi doesn't know the Set's tempo yet; try again.".into()));
        };
        let signal = abort::any([original, self.connection().lifetime.clone()]);
        let rows = self.rows("set", json!({"fields":["playing","position","loop"]}), signal.clone()).await.unwrap_or_default();
        let set = rows.first();
        // While Live plays, a listen with no place hears it as it plays; the whole song is heard quietly from its start.
        if set.and_then(|set| set.get("playing")) == Some(&json!(true)) && request.from_beat.is_none() && request.whole != Some(true) {
            if let Some(link) = self.ears_ready(signal.clone()).await? {
                return self.hear_as_it_plays(link, request, signal).await;
            }
        }
        if !self.supported(RENDER_BRIDGE) {
            return Ok(Err(self.too_old(RENDER_BRIDGE)));
        }
        if self.rendering.get() {
            return Ok(Err("Kumi is already listening to something; wait for it.".into()));
        }
        let looped = set.and_then(|set| set.get("loop")).filter(|v| v["enabled"] == true && v["length"].as_f64().is_some_and(|v| v > 0.));
        let whole = if request.whole == Some(true) {
            let Some(end) = self.song_end(signal.clone()).await else {
                return Ok(Err("Kumi couldn't tell where the song ends; give from_beat and beats.".into()));
            };
            Some(end)
        } else {
            None
        };
        let from = if whole.is_some() {
            0.
        } else {
            request.from_beat.unwrap_or_else(|| {
                looped
                    .map(|v| v["start"].as_f64().unwrap_or(0.))
                    .unwrap_or_else(|| set.and_then(|set| set.get("position")).and_then(Value::as_f64).unwrap_or(0.))
            })
        };
        let beats = whole.unwrap_or_else(|| {
            request.beats.unwrap_or_else(|| looped.and_then(|v| v["length"].as_f64()).unwrap_or(4. * self.observer.beats_per_bar.get()))
        });
        if let Some(why) = too_long(beats, tempo, "listen in parts") {
            return Ok(Err(why));
        }
        let candidates = if request.mix == Some(true) {
            vec![AuditionCandidate { track: MIX_CANDIDATE.into(), mix: Some(true), label: Some("The whole mix".into()), clip: None }]
        } else {
            request.tracks.iter().map(|track| AuditionCandidate { track: track.clone(), mix: None, label: None, clip: None }).collect()
        };
        self.tell(format!("Listening quietly from {}", bars(from)), Some(true));
        let rendering = self.rendering_now();
        let mut rig = None;
        let result: Result<Vec<HeardTake>, RuntimeError> = async {
            rig = Some(self.open_rig(&candidates, Some(from), Some(beats), signal.clone()).await?);
            let rig = rig.as_mut().unwrap();
            let rendered = self.render_pass(rig, signal.clone()).await?;
            Ok(rig
                .sources
                .iter()
                .filter_map(|source| {
                    rendered.get(&source.name).map(|found| HeardTake {
                        label: if source.mix { "The whole mix".into() } else { source.name.clone() },
                        file: found.file.clone(),
                        start: found.start,
                        // A take Live cut short says how much it holds; the note says why.
                        seconds: Some(found.seconds.unwrap_or(beats * 60. / tempo)),
                        live: false,
                        note: None,
                    })
                })
                .collect())
        }
        .await;
        if let Some(rig) = rig.as_mut() {
            self.close_rig(rig).await;
        }
        drop(rendering);
        self.tell("Listened", Some(false));
        let notes = rig.map(|rig| rig.notes).unwrap_or_default();
        match result {
            Err(error) => {
                signal.check()?;
                Ok(Err(head(&error.to_string(), 400)))
            }
            // What the capture held says what went wrong; with nothing to say, the Set may have been silent there.
            Ok(takes) if takes.is_empty() && !notes.is_empty() => Ok(Err(format!("Nothing came through: {}", notes.join(" ")))),
            Ok(takes) if takes.is_empty() => Ok(Err(format!(
                "Nothing came through. Is something playing there in the Arrangement (its clips at {}, the track not muted)?",
                bars(from)
            ))),
            Ok(mut takes) => {
                if !notes.is_empty() {
                    for take in &mut takes {
                        take.note = Some(notes.join(" "));
                    }
                }
                Ok(Ok(takes))
            }
        }
    }
    /// Where the song ends, in beats: its last Arrangement clip's end (Live's own song end often runs on, 58 bars in a
    /// new Set), else Live's song end.
    pub(super) async fn song_end(&self, signal: Signal) -> Option<f64> {
        if self.connection().has("live_run_python") {
            let read = self
                .connection()
                .call("live_run_python", object(json!({"code":SONG_END_SCRIPT,"mode":"exec","timeoutMs":5000})), signal.clone())
                .await;
            let end = read.ok().filter(|read| read.is_error != Some(true)).and_then(|read| {
                let done = super::super::context::payload(&read).ok()?;
                (done.get("ok") == Some(&Value::Bool(true))).then(|| done.get("result")?.get("end")?.as_f64()).flatten()
            });
            if let Some(end) = end.filter(|end| *end > 0.) {
                return Some(end);
            }
        }
        let song = self.connection().call("live_song_state", JsonObject::new(), signal).await.ok()?;
        super::super::context::payload(&song).ok()?.get("songLength")?.as_f64().filter(|end| *end > 0.)
    }
    async fn hear_as_it_plays(
        self: &Rc<Self>,
        link: Rc<dyn EarsLink>,
        request: &HearRequest,
        signal: Signal,
    ) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        let seconds = request.seconds.unwrap_or(8.).clamp(2., PASS_SECONDS);
        // Each tap's capture lands in Kumi's folder: with too little room for them, it's refused before Live plays.
        let taps = if request.mix == Some(true) { 1 } else { request.tracks.len() };
        if let Some(why) = self.no_room(seconds, taps, &std::env::temp_dir()).await {
            return Ok(Err(why));
        }
        let mut steps = vec![];
        let mut placed = vec![];
        let result: Result<Vec<HeardTake>, RuntimeError> = async {
            self.history
                .quietly(Some(&mut steps), async {
                    if request.mix == Some(true) {
                        let main = self.main_volume(signal.clone()).await?.0;
                        placed.push((
                            "The whole mix".to_owned(),
                            self.place_tap(link.clone(), &main, signal.clone()).await.map_err(|e| e.error)?,
                        ));
                        return Ok::<_, RuntimeError>(());
                    }
                    let mut tracks = self.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
                    tracks.extend(self.rows("return-track", json!({"fields":["name"]}), signal.clone()).await?);
                    for named in &request.tracks {
                        let found = tracks
                            .iter()
                            .find(|track| track.get("ref").and_then(Value::as_str) == Some(named))
                            .or_else(|| tracks.iter().find(|track| track.get("name").and_then(Value::as_str) == Some(named)))
                            .filter(|track| track.get("ref").is_some_and(Value::is_string))
                            .ok_or_else(|| observation(format!("{named} isn't a track in this turn's discovery; discover it again.")))?;
                        placed.push((
                            found.get("name").and_then(Value::as_str).unwrap_or(named).to_owned(),
                            self.place_tap(link.clone(), found["ref"].as_str().unwrap(), signal.clone()).await.map_err(|e| e.error)?,
                        ));
                    }
                    Ok(())
                })
                .await?;
            self.tell(
                format!(
                    "Listening to {} as it plays ({} s)",
                    if placed.len() == 1 { placed[0].0.clone() } else { format!("{} tracks", placed.len()) },
                    to_string(round(seconds))
                ),
                None,
            );
            eager_all(placed.iter().map(|(_, tap)| {
                let signal = signal.clone();
                let link = link.clone();
                async move {
                    link.arm(tap, seconds + 2., Some(signal)).await.map_err(plain)?;
                    Ok::<_, RuntimeError>(())
                }
            }))
            .await?;
            delay(seconds * 1000., signal.clone()).await?;
            let takes = RefCell::new(vec![]);
            eager_all(placed.iter().map(|(label, tap)| {
                let signal = signal.clone();
                let link = link.clone();
                let takes = &takes;
                async move {
                    let raw = RawFile(self.ears_folder.join(format!("{}.raw", uuid::Uuid::new_v4())));
                    let write: Result<(), RuntimeError> = async {
                        let written = link.write(tap, &raw.0.to_string_lossy().replace('\\', "/"), Some(signal)).await.map_err(plain)?;
                        let capture = read_capture(&raw.0, written.channels, written.sample_rate).await.map_err(plain)?;
                        let wav = self.ears_folder.join(format!("{}.wav", uuid::Uuid::new_v4()));
                        write_capture_wav(&wav, &capture, 0., capture.frames() as f64).await.map_err(plain)?;
                        takes.borrow_mut().push(HeardTake {
                            label: label.clone(),
                            file: wav.to_string_lossy().into_owned(),
                            start: 0.,
                            seconds: Some(capture.frames() as f64 / capture.sample_rate),
                            live: true,
                            note: None,
                        });
                        Ok(())
                    }
                    .await;
                    drop(raw);
                    write
                }
            }))
            .await?;
            let mut takes = takes.into_inner();
            takes.sort_by_key(|take| placed.iter().position(|(label, _)| *label == take.label));
            Ok(takes)
        }
        .await;
        for (_, tap) in &placed {
            link.stop(tap);
        }
        let cleanup = self.cleanup();
        self.history
            .quietly(None, async {
                for id in steps.iter().rev() {
                    self.take_back(id, cleanup.clone(), false).await;
                }
            })
            .await;
        self.tell("Listened", None);
        let this = self.clone();
        tokio::task::spawn_local(async move {
            this.prune_ears().await;
        });
        match result {
            Ok(takes) => Ok(Ok(takes)),
            Err(error) => {
                signal.check()?;
                Ok(Err(head(&error.to_string(), 400)))
            }
        }
    }
}
fn round_number(value: f64) -> f64 {
    round(value)
}
/// The end of the last clip in the Arrangement, in beats.
const SONG_END_SCRIPT: &str = "end = 0.0\nfor track in list(song.tracks):\n    for clip in list(getattr(track, 'arrangement_clips', None) or []):\n        end = max(end, float(clip.end_time))\nresult = {'end': end}\n";
/// The longest stretch one listen hears, in seconds (a judged run's span, a form's, a sound's and an audition's too).
pub(super) const LONGEST_LISTEN: f64 = 3600.;
/// Why a stretch of `beats` is too long to hear at once, when it is, and what to do instead.
pub(super) fn too_long(beats: f64, tempo: f64, instead: &str) -> Option<String> {
    let seconds = beats * 60. / tempo;
    (seconds > LONGEST_LISTEN || !seconds.is_finite())
        .then(|| format!("That's {} of music; Kumi listens to at most an hour at once, so {instead}.", clock(seconds)))
}
fn clock(seconds: f64) -> String {
    let whole = round(seconds) as i64;
    format!("{}:{:02}", whole / 60, whole % 60)
}
fn plain(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}
pub(super) fn heard_options(start: f64, seconds: f64, focus: Option<Focus>, signal: Signal) -> AnalyzeOptions {
    AnalyzeOptions {
        start: Some(start + if focus == Some(Focus::Section) { 0.1 } else { 0. }),
        seconds: Some(seconds + if focus == Some(Focus::Section) { 0. } else { 0.1 }),
        focus: analysis_focus(focus),
        signal: Some(signal),
        ..Default::default()
    }
}
