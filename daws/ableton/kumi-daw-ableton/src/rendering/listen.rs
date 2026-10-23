use super::super::{
    bridge_version::RENDER_BRIDGE,
    concurrent::eager_all,
    connection::NO_CURRENT_LIVE,
    context::{object as strict_object, payload},
    history::result_text,
    more_changes::bars,
};
use super::ears::RawFile;
use super::ears_pass::PASS_SECONDS;
use super::rig::{Rig, Source, Window};
use super::*;
use crate::ears::capture::{read_capture, write_capture_wav};
use kumi_common::js::{
    number::{round, to_string},
    string::head,
};
use kumi_listening::renderer::{Audible, AuditionOptions, AuditionPass, AuditionRender, AuditionSource};
impl LiveRenderer {
    /// Plays the candidates over the part and captures each (see `Renderer::audition_pass`).
    pub(super) async fn audition_pass_of(
        self: &Rc<Self>,
        candidates: &[AuditionCandidate],
        from_beat: Option<f64>,
        beats: Option<f64>,
        options: AuditionOptions,
        signal: Signal,
    ) -> AuditionPass {
        let mut rig = None;
        let mut opened = false;
        let rendered: Result<AuditionRender, RuntimeError> = async {
            rig = Some(self.open_rig(candidates, from_beat, beats, signal.clone()).await?);
            opened = true;
            let rig = rig.as_mut().unwrap();
            // The producer hears the first candidate where it plays; Kumi hears the others on their own.
            if options.audible == Audible::First {
                for source in rig.sources.iter_mut().skip(1) {
                    source.quiet = !source.mix;
                }
            }
            let beats = rig.beats;
            let length = options.first.then(|| round_number(beats * 60. / options.tempo + 6.));
            self.tell(auditioning(rig, options.round, length), Some(true));
            let files = self.render_pass(rig, signal.clone()).await?;
            let sources = rig
                .sources
                .iter()
                .map(|source| AuditionSource {
                    track: source.track.clone(),
                    name: source.name.clone(),
                    scene: source.scene,
                    audible: !source.quiet,
                })
                .collect();
            Ok(AuditionRender { sources, files, beats })
        }
        .await;
        let mut notes = vec![];
        if let Some(rig) = rig.as_mut() {
            self.close_rig(rig).await;
            notes.extend(rig.notes.clone());
        }
        AuditionPass { rendered, notes, opened }
    }
    /// The track's own audio between two points, rendered to a file at once (see `Renderer::render`).
    pub(super) async fn render_offline(
        &self,
        track: &str,
        from: f64,
        beats: f64,
        signal: Signal,
    ) -> Result<Result<JsonObject, String>, RuntimeError> {
        let mut args = strict_object(&self.connection().references.borrow().lengthen(&json!({"trackRef":track})))?;
        if let Err(error) = self.connection().references.borrow().require_fresh_references(&args) {
            return Ok(Err(error.to_string()));
        }
        let name = args["trackRef"]
            .as_str()
            .and_then(|r| self.connection().references.borrow().known.get(r).map(|t| t.name.clone()))
            .filter(|s| !s.is_empty());
        args.insert("fromBeat".into(), json!(from));
        args.insert("toBeat".into(), json!(from + beats));
        if let Some(name) = name {
            args.insert("expectedName".into(), json!(name));
        }
        let result = self.connection().call("live_render_offline", args, abort::any([signal, self.connection().lifetime.clone()])).await?;
        if result.is_error == Some(true) {
            return Ok(Err(result_text(&result)));
        }
        let rendered = payload(&result)?;
        let mut reply = JsonObject::new();
        for (key, source) in [("file", "path"), ("seconds", "seconds"), ("channels", "channels"), ("sampleRate", "sampleRate")] {
            if let Some(value) = rendered.get(source) {
                reply.insert(key.into(), value.clone());
            }
        }
        Ok(Ok(reply))
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
        // While Live plays, a listen with no place hears it as it plays; the whole song is played from its start.
        if set.and_then(|set| set.get("playing")) == Some(&json!(true)) && request.from_beat.is_none() && request.whole != Some(true) {
            if let Some(link) = self.ears_ready(signal.clone()).await? {
                return self.hear_as_it_plays(link, request, signal).await;
            }
        }
        if !self.supported(RENDER_BRIDGE) {
            return Ok(Err(self.too_old(RENDER_BRIDGE)));
        }
        if self.rendering.is_on() {
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
        let rendering = self.rendering_now();
        let mut rig = None;
        let result: Result<Vec<HeardTake>, RuntimeError> = async {
            rig = Some(self.open_rig(&candidates, Some(from), Some(beats), signal.clone()).await?);
            let rig = rig.as_mut().unwrap();
            let heard: Vec<String> =
                rig.sources.iter().map(|source| if source.mix { "the mix".into() } else { source.name.clone() }).collect();
            self.tell(format!("Playing {} · listening to {}", self.bars_of(Window { from, beats }), heard.join(", ")), Some(true));
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
                    link.arm(tap, seconds + 2., false, Some(signal)).await.map_err(plain)?;
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
/// What an audition plays, as it starts: the first candidate, which the producer hears in the Set, and how many more
/// Kumi hears on its own (with Kumi Ears; the record pass plays them all), with how long a round takes the first time.
fn auditioning(rig: &Rig, round: usize, length: Option<f64>) -> String {
    let label = |source: &Source| if source.mix { "the whole mix".to_string() } else { source.label.clone() };
    let first = rig.sources.first().map(label).unwrap_or_else(|| "the candidates".into());
    let others = rig.sources.len().saturating_sub(1);
    let more = match (others, rig.ears.is_some()) {
        (0, _) => String::new(),
        (1, true) => "; Kumi hears the other one on its own".into(),
        (others, true) => format!("; Kumi hears the other {others} on their own"),
        (others, false) => format!(" and {others} more beside it"),
    };
    let length = length.map(|seconds| format!(" (about {} s a round)", to_string(seconds))).unwrap_or_default();
    format!("Round {round}: playing {first}{more}{length}")
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
