use super::super::{audition::render_span, bridge_version::ARRANGEMENT_BRIDGE, concurrent::eager_all};
use super::ears::RawFile;
use super::rig::{Rig, Window};
use super::*;
use crate::ears::{
    capture::{cover, frame_at, join_wavs, read_capture, runs, write_capture_wav, Anchors, Missed},
    link::Tap,
};
use kumi_common::js::number::{round, to_string};

/// The longest part one pass records, in seconds: Kumi Ears holds 900 s, less the lead-in and a margin.
pub(super) const PASS_SECONDS: f64 = 840.;
/// What the devices hold at once, in seconds summed over them: Live keeps each recording in memory, and Kumi
/// reads each one whole.
const PASS_BUDGET: f64 = 1800.;
/// Captures longer than this are read one after another rather than side by side.
const READ_ALONE: f64 = 60.;

/// What one device heard of a part: the part as a file, the lead before it (seconds), and where Live stopped
/// short of the part's end when it did.
struct Part {
    file: PathBuf,
    lead: f64,
    short: Option<Missed>,
}

/// The parts one listen plays, each short enough for Kumi Ears to hold: the whole stretch in one pass when it
/// fits, else whole bars back to back.
fn pieces(window: Window, tempo: f64, meter: f64, taps: usize) -> Vec<Window> {
    let longest = PASS_SECONDS.min(PASS_BUDGET / taps.max(1) as f64) * tempo / 60.;
    if !(window.beats > longest) {
        return vec![window];
    }
    let step = ((longest / meter).floor() * meter).max(meter);
    let end = window.from + window.beats;
    let mut found = vec![];
    let mut from = window.from;
    while from < end - 1e-9 {
        let beats = step.min(end - from);
        found.push(Window { from, beats });
        from += beats;
    }
    found
}

impl Rendering {
    pub(super) async fn ears_pass(self: &Rc<Self>, rig: &mut Rig, signal: Signal) -> Result<IndexMap<String, Render>, RuntimeError> {
        let tempo = self.observer.tempo.get().unwrap_or(f64::NAN);
        let ears = rig.ears.as_ref().unwrap();
        let link = ears.link.clone();
        let taps: Vec<_> = ears.taps.iter().map(|(name, tap)| (name.clone(), tap.clone())).collect();
        let (main_ref, prior) = self.main_prior(rig, signal.clone()).await?;
        let held = rig.hold.is_some();
        let pieces = pieces(rig.window(), tempo, self.observer.beats_per_bar.get(), taps.len());
        let mut started = false;
        // Each tap's parts so far, in order, with the first one's lead; a tap that missed a part keeps what came before.
        let mut parts: IndexMap<String, (Vec<PathBuf>, f64)> = IndexMap::new();
        let mut ended: Vec<String> = vec![];
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
                for (index, piece) in pieces.iter().enumerate() {
                    let last = index + 1 == pieces.len();
                    let listening: Vec<_> = taps.iter().filter(|(name, _)| !ended.contains(name)).cloned().collect();
                    if listening.is_empty() && index > 0 {
                        break;
                    }
                    let heard = self.ears_piece(rig, &link, &listening, *piece, index == 0, last, &mut started, signal.clone()).await?;
                    for (name, outcome) in heard {
                        let named = if taps.len() > 1 { format!("{name}: ") } else { String::new() };
                        match outcome {
                            Ok(part) => {
                                let entry = parts.entry(name.clone()).or_insert_with(|| (vec![], part.lead));
                                entry.0.push(part.file);
                                if let Some(short) = part.short {
                                    rig.notes.push(format!("{named}{}", short.describe(piece.from, piece.beats)));
                                    ended.push(name);
                                }
                            }
                            Err(missed) => {
                                rig.notes.push(format!("{named}{}", missed.describe(piece.from, piece.beats)));
                                ended.push(name);
                            }
                        }
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
        let mut files = IndexMap::new();
        for (name, (mut heard, lead)) in parts {
            let file = if heard.len() == 1 {
                heard.pop().unwrap()
            } else {
                let joined = self.ears_folder.join(format!("{}.wav", uuid::Uuid::new_v4()));
                let written = join_wavs(&joined, &heard).await;
                for part in &heard {
                    let _ = tokio::fs::remove_file(part).await;
                }
                if let Err(error) = written {
                    rig.notes.push(format!("Kumi couldn't join what it heard of {name}: {error}"));
                    continue;
                }
                joined
            };
            files.insert(name, Render { file: file.to_string_lossy().into_owned(), start: lead });
        }
        Ok(files)
    }

    /// One part of a listen: Live plays it once (again from further ahead when it reached the part late), and each
    /// device's capture is cut to the part. The first part keeps a short lead before it; the last, half a bar after.
    #[allow(clippy::too_many_arguments)]
    async fn ears_piece(
        self: &Rc<Self>,
        rig: &mut Rig,
        link: &Rc<dyn EarsLink>,
        taps: &[(String, Tap)],
        piece: Window,
        first: bool,
        last: bool,
        started: &mut bool,
        signal: Signal,
    ) -> Result<IndexMap<String, Result<Part, Missed>>, RuntimeError> {
        let tempo = self.observer.tempo.get().unwrap_or(f64::NAN);
        let meter = self.observer.beats_per_bar.get();
        let beat_ms = 60. / tempo * 1000.;
        let mut heard: IndexMap<String, Result<Part, Missed>> = IndexMap::new();
        for longer in [false, true] {
            let span = render_span(piece.from, piece.beats, meter, tempo, longer, Some(0.));
            let prime_key = to_string(span.position);
            let priming = rig.hold.as_ref().is_none_or(|hold| hold.primed.as_ref() != Some(&prime_key));
            if priming && self.supported(ARRANGEMENT_BRIDGE) {
                self.step("play", json!({"action":"back-to-arrangement"}), signal.clone()).await?;
            }
            if priming && rig.transport.looped != Some(false) {
                self.step("set_transport", json!({"loopEnabled":false}), signal.clone()).await?;
            }
            let seconds = (span.wait + 4. * meter) * beat_ms / 1000. + 6.;
            *started = true;
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
            let mut first_seen = None;
            for _ in 0..8 {
                first_seen = link.transport(probe, Some(signal.clone())).await.map_err(plain)?;
                if first_seen.as_ref().is_some_and(|seen| seen.running) {
                    break;
                }
                delay(15., signal.clone()).await?;
            }
            let there = first_seen
                .as_ref()
                .is_some_and(|seen| seen.running && seen.beats >= span.position - 0.01 && seen.beats <= span.position + 0.25);
            if !there {
                self.step("set_transport", json!({"position":span.position}), signal.clone()).await?;
            }
            let end = piece.from + piece.beats + meter / 2.;
            let deadline = now_ms() as f64 + seconds * 1000.;
            let mut jumped = there;
            while (now_ms() as f64) < deadline {
                let now = link.transport(probe, Some(signal.clone())).await.map_err(plain)?;
                if let Some(now) = now.as_ref().filter(|now| now.running) {
                    if now.beats >= span.position - 0.01 && now.beats < piece.from.max(span.position + 0.5) {
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
            *started = false;
            let read = |name: String, tap: Tap| {
                let link = link.clone();
                let signal = signal.clone();
                async move {
                    let raw = RawFile(self.ears_folder.join(format!("{}.raw", uuid::Uuid::new_v4())));
                    let outcome = self.ears_part(&link, &tap, &raw, span.position, piece, first, last, signal).await;
                    drop(raw);
                    Ok::<_, RuntimeError>((name, outcome))
                }
            };
            // Long captures are big (Kumi reads each whole), so they're read one at a time.
            let outcomes = if piece.beats * 60. / tempo > READ_ALONE {
                let mut outcomes = vec![];
                for (name, tap) in taps {
                    outcomes.push(read(name.clone(), tap.clone()).await?);
                }
                outcomes
            } else {
                eager_all(taps.iter().map(|(name, tap)| read(name.clone(), tap.clone()))).await?
            };
            for (name, outcome) in outcomes {
                // What a device heard on the first pass stays unless the second heard the part whole.
                let replace = match (&outcome, heard.get(&name)) {
                    (Ok(part), _) if part.short.is_none() => true,
                    (_, Some(Ok(_))) => false,
                    _ => true,
                };
                if replace {
                    if let Some(Ok(earlier)) = heard.insert(name, outcome) {
                        let _ = tokio::fs::remove_file(&earlier.file).await;
                    }
                } else if let Ok(part) = outcome {
                    let _ = tokio::fs::remove_file(&part.file).await;
                }
            }
            if !heard.values().any(|outcome| matches!(outcome, Err(missed) if missed.retry())) {
                break;
            }
        }
        Ok(heard)
    }

    /// One device's capture of a part, cut to it: written next to the raw file, or why Live's playing missed it.
    #[allow(clippy::too_many_arguments)]
    async fn ears_part(
        &self,
        link: &Rc<dyn EarsLink>,
        tap: &Tap,
        raw: &RawFile,
        position: f64,
        piece: Window,
        first: bool,
        last: bool,
        signal: Signal,
    ) -> Result<Part, Missed> {
        let meter = self.observer.beats_per_bar.get();
        let written = link.write(tap, &raw.0.to_string_lossy().replace('\\', "/"), Some(signal)).await.map_err(unread)?;
        let capture = read_capture(&raw.0, written.channels, written.sample_rate).await.map_err(unread)?;
        let stretches = runs(&capture, Anchors { first: Some(written.beats), after_jump: Some(position) });
        let (part, short) = match cover(&stretches, piece.from, piece.beats, capture.sample_rate) {
            Ok(part) => (part, None),
            // Heard from the part's start but not to its end: what was heard is kept, and said.
            Err(Missed::Cut { run, to_beat, pieces, seconds }) => {
                let short = Missed::Cut { run: run.clone(), to_beat, pieces, seconds };
                (run, Some(short))
            }
            Err(missed) => return Err(missed),
        };
        let at = frame_at(&part, piece.from).unwrap() as f64;
        let lead = if first { round(0.1 * capture.sample_rate).min(at) } else { 0. };
        let after = if last { meter / 2. } else { 0. };
        let end = (part.to as f64).min(at + ((piece.beats + after) * part.samples_per_beat).round());
        let wav = self.ears_folder.join(format!("{}.wav", uuid::Uuid::new_v4()));
        write_capture_wav(&wav, &capture, at - lead, end).await.map_err(unread)?;
        Ok(Part { file: wav, lead: lead / capture.sample_rate, short })
    }
}
fn plain(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}
fn unread(error: impl std::fmt::Display) -> Missed {
    Missed::Unread(kumi_common::js::string::head(&error.to_string(), 200))
}
