use super::super::{audition::RenderJournal, references::KeptRefs};
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
    /// A candidate Kumi compares beside the one the producer hears: Kumi Ears holds its sound back while it records.
    /// Never the mix.
    pub quiet: bool,
}
#[derive(Default)]
pub(super) struct Held {
    pub primed: Option<String>,
    pub recording: bool,
    pub rearm: Vec<String>,
}
pub(super) use kumi_listening::renderer::Window;
#[derive(Default)]
pub(super) struct Transport {
    pub position: Option<f64>,
    pub looped: Option<bool>,
}
pub(super) struct RigEars {
    pub link: Rc<dyn EarsLink>,
    pub taps: IndexMap<String, Tap>,
    /// The tap first on Main's chain (the mix as it comes into it), when there's one.
    pub before_main: Option<BeforeMainTap>,
}
pub(super) struct BeforeMainTap {
    /// Live's identity for its device, to delete it by.
    pub identity: String,
    /// The steps that loaded and moved it.
    pub steps: Vec<String>,
    /// Main's devices before it was loaded (their identities, in order), and the refs to them, which its move and
    /// delete retire: they're put back when Main's chain is as it was again.
    pub had: Vec<String>,
    pub refs: KeptRefs,
}
/// Its capture's name among a pass's files: no track's (Kumi's own render tracks are named "Kumi · render …").
pub(super) const BEFORE_MAIN: &str = "Kumi · before Main";
/// Why the plugin host's listening can't hear through Kumi Ears (it never records to listen, or installs the device).
pub(super) const EARS_MISSING: &str =
    "The preinstalled Kumi Ears device is missing or didn't start. It requires Max for Live; listening never installs it: run setup again.";
/// The longest pass the plugin host's listening plays, seconds: from Live starting to play to stopping, its lead-in and
/// what follows the stretch included.
pub(super) const RESTRICTED_SECONDS: f64 = 60.;
/// The most a form call plays in all under the plugin host's listening, seconds (its passes, lead-ins and what follows
/// included): the whole Arrangement once, for a song up to about a quarter of an hour, and nothing runs away.
pub(super) const WHOLE_SONG_SECONDS: f64 = 900.;
/// How long the plugin host's listening lets a call play: a minute a pass, or, for form alone, the whole Arrangement up
/// to [`WHOLE_SONG_SECONDS`] in all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PlayLimit {
    Minute,
    WholeSong,
}
impl PlayLimit {
    pub(super) fn seconds(self) -> f64 {
        match self {
            Self::Minute => RESTRICTED_SECONDS,
            Self::WholeSong => WHOLE_SONG_SECONDS,
        }
    }
    fn too_long(self) -> &'static str {
        match self {
            Self::Minute => "Kumi plays at most 60 seconds at a time here, a bar before the stretch and half a bar after it included: hear a shorter stretch (from_beat and beats).",
            Self::WholeSong => "Kumi plays at most 15 minutes of the Arrangement here, a bar before and half a bar after included: read the form in parts (from_beat and beats).",
        }
    }
}
/// Why the plugin host's listening captures nowhere: it was given no private per-call scratch under its root.
pub(super) const SCRATCH_MISSING: &str = "Restricted listening requires a private per-call scratch directory inside its configured root.";
/// How long Live plays a Kumi Ears pass over `window`, seconds: its lead-in (a bar, or two when it plays again because
/// Live reached the stretch late; less when the stretch starts sooner), the stretch, and half a bar after it.
pub(super) fn played_seconds(window: Window, beats_per_bar: f64, tempo: f64, again: bool) -> f64 {
    super::super::audition::render_span(window.from, window.beats, beats_per_bar, tempo, again, Some(0.)).wait * 60. / tempo
}
/// Why the plugin host's listening won't play passes of `seconds` in all (as [`played_seconds`] counts each) under
/// `limit`, with Live's transport and song as read (Live playing or recording, not following the Arrangement, or more
/// than the limit); None when it may.
pub(super) fn restricted_refusal(seconds: f64, limit: PlayLimit, transport: &Value, song: &Value) -> Option<&'static str> {
    if !seconds.is_finite() || seconds <= 0. || seconds > limit.seconds() {
        return Some(limit.too_long());
    }
    if ["playing", "arrangementRecord", "sessionRecord"].iter().any(|key| transport.get(*key) != Some(&Value::Bool(false))) {
        return Some("Stop playback and recording before listening to an Arrangement window.");
    }
    if song.get("backToArranger") != Some(&Value::Bool(false)) {
        return Some("Live is not following the Arrangement. Resolve Back to Arrangement yourself, then observe again.");
    }
    None
}
/// The folder a pass of the plugin host's listening captures into: the per-call scratch its host gave (`call`), checked
/// to be a private folder directly under the policy's root; refused without one. None standalone (Kumi's own folder).
pub(super) async fn capture_folder(policy: &ListeningPolicy, call: Option<PathBuf>) -> Result<Option<PathBuf>, &'static str> {
    if matches!(policy, ListeningPolicy::Standalone) {
        return Ok(None);
    }
    let request = kumi_types::contracts::HearRequest { scratch_dir: call, ..Default::default() };
    policy.scratch_dir(&request).await.map(Some).map_err(|_| SCRATCH_MISSING)
}
pub(super) struct Rig {
    pub tag: String,
    /// The Set's tempo as the rig opened: what a pass records at when a look since can't say what it is now.
    pub tempo: Option<f64>,
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
    /// Live's own recordings of the record pass, in the project: Kumi reads copies, and these go with the render tracks.
    pub recorded: Vec<PathBuf>,
    /// Whether the mix is heard as it comes into Main's chain too (Kumi Ears only): a second tap, first on Main.
    pub before_main: bool,
    /// Why that tap wasn't placed, when Kumi Ears was there but it couldn't be.
    pub before_main_missed: Option<String>,
    /// Whether the record pass noted its render tracks in the crash journal: closing the rig clears it.
    pub noted: bool,
}
impl Rig {
    pub fn window(&self) -> Window {
        self.window.unwrap_or(Window { from: self.from, beats: self.beats })
    }
}
impl LiveRenderer {
    /// Whether this is the plugin host's listening: Kumi Ears alone, Live stopped and not recording as a pass starts,
    /// at most a minute a pass.
    pub(super) fn restricted(&self) -> bool {
        matches!(self.listening_policy, ListeningPolicy::Restricted { .. })
    }
    /// How long the plugin host's listening lets the call under way play: the whole Arrangement for form, else a minute
    /// a pass.
    pub(super) fn play_limit(&self) -> PlayLimit {
        if self.whole_song.get() {
            PlayLimit::WholeSong
        } else {
            PlayLimit::Minute
        }
    }
    /// The plugin host's limits on a pass about to play, read from Live now: Kumi Ears there, a per-call scratch to
    /// capture into, and what it plays (each part's lead-in, the part, half a bar after) within the call's limit.
    pub(super) async fn restricted_pass(&self, rig: &Rig, signal: Signal) -> Result<(), RuntimeError> {
        if rig.ears.is_none() {
            return Err(observation(EARS_MISSING));
        }
        let call = self.call_scratch.borrow().clone();
        capture_folder(&self.listening_policy, call).await.map_err(observation)?;
        let (meter, tempo) = (self.observer.beats_per_bar.get(), self.pass_tempo(rig)?);
        let taps = rig.ears.as_ref().map_or(1, |ears| ears.taps.len());
        let seconds: f64 = super::ears_pass::pieces(rig.window(), tempo, meter, taps)
            .into_iter()
            .map(|piece| played_seconds(piece, meter, tempo, false))
            .sum();
        let unreadable =
            || observation("This Live bridge can't say whether Live is playing or recording; update its bridge and restart Live.");
        let playback = self.rows("session-playback", json!({}), signal.clone()).await?;
        let transport = playback.first().and_then(|row| row.get("transport")).cloned().ok_or_else(unreadable)?;
        if !self.connection().has("live_song_state") {
            return Err(unreadable());
        }
        let song = super::super::context::payload(&self.connection().call("live_song_state", JsonObject::new(), signal).await?)?;
        match restricted_refusal(seconds, self.play_limit(), &transport, &Value::Object(song)) {
            Some(why) => Err(observation(why)),
            None => Ok(()),
        }
    }
}
impl LiveRenderer {
    /// The tempo a pass records at: the Set's as it is now, else as the rig opened; refused when neither can say.
    pub(super) fn pass_tempo(&self, rig: &Rig) -> Result<f64, RuntimeError> {
        self.observer
            .tempo
            .get()
            .filter(|tempo| *tempo > 0.)
            .or(rig.tempo)
            .ok_or_else(|| observation("Kumi doesn't know the Set's tempo yet; try again."))
    }
    /// The bars `window` covers, as said while Live plays them: "bars 17–24", or "bar 17".
    pub(super) fn bars_of(&self, window: Window) -> String {
        kumi_listening::renderer::bars_of(self.observer.beats_per_bar.get(), window)
    }
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
        self.open_rig_with(candidates, from, beats, false, signal).await
    }
    /// A rig that, with `before_main` and Kumi Ears, also hears the mix as it comes into Main's chain.
    pub(super) async fn open_rig_with(
        self: &Rc<Self>,
        candidates: &[AuditionCandidate],
        from: Option<f64>,
        beats: Option<f64>,
        before_main: bool,
        signal: Signal,
    ) -> Result<Rig, RuntimeError> {
        let mut rig = Rig {
            tag: uuid::Uuid::new_v4().to_string()[..4].into(),
            tempo: self.observer.tempo.get().filter(|tempo| *tempo > 0.),
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
            recorded: vec![],
            before_main,
            before_main_missed: None,
            noted: false,
        };
        let link = self.ears_ready(signal.clone()).await?;
        // The plugin host's listening hears through Kumi Ears alone, never a record pass.
        if link.is_none() && self.restricted() {
            return Err(observation(EARS_MISSING));
        }
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
                        quiet: false,
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
                quiet: false,
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
                rig.ears=Some(RigEars{link,taps:IndexMap::new(),before_main:None});
                if let Err(error)=self.place_taps(&mut rig,signal.clone()).await {
                    signal.check()?;
                    if std::env::var("KUMI_TIMING").is_ok_and(|s| !s.is_empty()) {
                        eprintln!("[ears] not placed{}: {}", if error.silent { " (the device didn't start)" } else { "" }, error.error);
                    }
                    // Quiet steps have not been copied out until this scope ends; removeTaps needs them now.
                    // The history's current quiet list is retained as well, matching source nested undo bookkeeping.
                    self.remove_taps(&mut rig).await;
                    if error.silent { self.ears_refused.set(true);rig.notes.push("Kumi's listening device didn't start in this Live (it needs Max for Live), so Kumi records to listen instead.".into()); }
                    if shared {return Err(error.error);}
                    if self.restricted() {return Err(observation(EARS_MISSING));}
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
    /// Notes the record pass's render tracks, so a listen cut off by a crash can say to delete them. A journal that
    /// can't be written (a full disk) doesn't stop the listen: Live is left as it is, apart from those tracks.
    pub(super) fn note_render(&self, rig: &mut Rig) {
        let Some(restore) = &self.restore else { return };
        rig.noted = true;
        restore.save(&RenderJournal {
            set: self.connection().set.borrow().clone().unwrap_or_default(),
            path: self.history.remember.current().and_then(|p| p.path.clone()),
            volume: None,
            at: self.connection().now().timestamp_millis() as f64,
            scratch: Some(rig.sources.iter().map(|s| s.scratch.clone()).collect()),
        });
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
        }
        let mut scratch_left = false;
        // The tap first on Main goes by its identity, not by undoing its move and its load.
        self.remove_before_main(rig, cleanup.clone()).await;
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
                            scratch_left = true;
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
        if std::mem::take(&mut rig.noted) {
            self.clear_restore();
        }
        // The record pass's recordings stay in the project after its render tracks go: Kumi read copies, so they go
        // too. A render track Live kept still plays its clips, so then they stay.
        if !scratch_left {
            for file in rig.recorded.drain(..) {
                let _ = tokio::fs::remove_file(file).await;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// The minute covers all Live plays: a bar before the stretch and half a bar after, and the replay's two bars.
    #[test]
    fn the_plugin_hosts_minute_counts_the_lead_in_and_what_follows_the_stretch() {
        let stopped = json!({"playing": false, "arrangementRecord": false, "sessionRecord": false});
        let following = json!({"backToArranger": false});
        let fits = |window: Window, again: bool| {
            restricted_refusal(played_seconds(window, 4., 120., again), PlayLimit::Minute, &stopped, &following).is_none()
        };
        // At 120 BPM in 4/4 a beat is half a second: 4 beats before, 2 after.
        assert_eq!(played_seconds(Window { from: 16., beats: 114. }, 4., 120., false), 60.);
        assert!(fits(Window { from: 16., beats: 114. }, false));
        // 116 beats is 58 s of stretch, under a minute alone, but 61 s with its lead-in and what follows.
        assert!(!fits(Window { from: 16., beats: 116. }, false));
        // Played again from two bars ahead, the same stretch takes 62 s: it isn't.
        assert_eq!(played_seconds(Window { from: 16., beats: 114. }, 4., 120., true), 62.);
        assert!(!fits(Window { from: 16., beats: 114. }, true));
        assert!(fits(Window { from: 16., beats: 110. }, true));
        // A stretch from the Arrangement's start has no lead-in to play.
        assert_eq!(played_seconds(Window { from: 0., beats: 118. }, 4., 120., true), 60.);
    }

    /// form alone hears the whole Arrangement: up to 15 minutes in all, its parts' lead-ins and what follows included;
    /// every other tool stays at a minute a pass.
    #[test]
    fn only_form_plays_the_whole_song_and_for_15_minutes_at_most() {
        let stopped = json!({"playing": false, "arrangementRecord": false, "sessionRecord": false});
        let following = json!({"backToArranger": false});
        // A song at 120 BPM in 4/4 (half a second a beat), heard whole from its start, with one tap: in parts each
        // Kumi Ears can hold, each with its own lead-in and half a bar after.
        let song = |beats: f64| -> f64 {
            super::super::ears_pass::pieces(Window { from: 0., beats }, 120., 4., 1)
                .into_iter()
                .map(|piece| played_seconds(piece, 4., 120., false))
                .sum()
        };
        // Four minutes of song (480 beats): form plays it; anything else is held to a minute.
        assert_eq!(song(480.), 241.);
        assert_eq!(restricted_refusal(song(480.), PlayLimit::WholeSong, &stopped, &following), None);
        assert!(restricted_refusal(song(480.), PlayLimit::Minute, &stopped, &following).unwrap().contains("at most 60 seconds"));
        // Fourteen minutes and a half fits; fifteen whole minutes doesn't, once the parts' lead-in and tail are counted.
        assert!(song(1740.) <= WHOLE_SONG_SECONDS, "{}", song(1740.));
        assert_eq!(restricted_refusal(song(1740.), PlayLimit::WholeSong, &stopped, &following), None);
        let refused = restricted_refusal(song(1800.), PlayLimit::WholeSong, &stopped, &following).unwrap();
        assert!(refused.contains("at most 15 minutes"), "{refused}");
        // The whole song keeps every other condition: Live stopped, not recording, following the Arrangement.
        let playing = json!({"playing": true, "arrangementRecord": false, "sessionRecord": false});
        assert!(restricted_refusal(song(480.), PlayLimit::WholeSong, &playing, &following).is_some());
        assert!(restricted_refusal(song(480.), PlayLimit::WholeSong, &stopped, &json!({"backToArranger": true})).is_some());
    }

    /// The plugin host's captures go only into the private per-call folder its host gave, directly under the policy's
    /// root (never Kumi's shared folder); none given, or one elsewhere, and the pass is refused.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_plugin_hosts_captures_go_only_into_its_per_call_scratch() {
        use std::os::unix::fs::DirBuilderExt;
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("scratch").join("listen");
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&root).unwrap();
        let call = root.join("1-1");
        std::fs::DirBuilder::new().mode(0o700).create(&call).unwrap();
        let policy = ListeningPolicy::Restricted { scratch_root: root.clone(), quarantine_root: home.path().join("quarantine") };
        let folder = capture_folder(&policy, Some(call.clone())).await.unwrap().unwrap();
        assert_eq!(folder, std::fs::canonicalize(&call).unwrap());
        assert_eq!(capture_folder(&policy, None).await, Err(SCRATCH_MISSING));
        let shared = std::env::temp_dir().join("kumi-ears");
        std::fs::create_dir_all(&shared).unwrap();
        assert_eq!(capture_folder(&policy, Some(shared)).await, Err(SCRATCH_MISSING));
        let deeper = call.join("deeper");
        std::fs::DirBuilder::new().mode(0o700).create(&deeper).unwrap();
        assert_eq!(capture_folder(&policy, Some(deeper)).await, Err(SCRATCH_MISSING));
        assert_eq!(capture_folder(&ListeningPolicy::Standalone, Some(call)).await, Ok(None));
    }

    #[test]
    fn the_plugin_hosts_pass_plays_only_a_stopped_unrecording_arrangement_for_a_minute_at_most() {
        let stopped = json!({"playing": false, "arrangementRecord": false, "sessionRecord": false});
        let following = json!({"backToArranger": false});
        assert_eq!(restricted_refusal(60., PlayLimit::Minute, &stopped, &following), None);
        assert!(restricted_refusal(60.5, PlayLimit::Minute, &stopped, &following).unwrap().contains("at most 60 seconds"));
        assert!(restricted_refusal(f64::NAN, PlayLimit::Minute, &stopped, &following).is_some());
        for key in ["playing", "arrangementRecord", "sessionRecord"] {
            let mut moving = stopped.clone();
            moving[key] = json!(true);
            assert!(
                restricted_refusal(8., PlayLimit::Minute, &moving, &following).unwrap().starts_with("Stop playback and recording"),
                "{key}"
            );
            // A transport Live doesn't say is stopped isn't taken as stopped.
            moving.as_object_mut().unwrap().remove(key);
            assert!(restricted_refusal(8., PlayLimit::Minute, &moving, &following).is_some(), "{key} unread");
        }
        assert!(restricted_refusal(8., PlayLimit::Minute, &stopped, &json!({"backToArranger": true}))
            .unwrap()
            .contains("Back to Arrangement"));
        assert!(restricted_refusal(8., PlayLimit::Minute, &stopped, &json!({})).is_some());
    }
}
