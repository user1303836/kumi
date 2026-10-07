use super::super::{audition::render_span, bridge_version::ARRANGEMENT_BRIDGE, concurrent::eager_all};
use super::ears::RawFile;
use super::rig::{Rig, Window};
use super::*;
use crate::ears::{
    capture::{cover, frame_at, join_wavs, read_capture, runs, write_capture_wav, Anchors, Missed},
    link::Tap,
};
use kumi_common::js::number::{round, to_string};
use std::path::Path;

/// The longest part one pass records, in seconds: Kumi keeps a capture's beat and Live's position in memory (about 4
/// bytes a frame) and copies the sound straight from the device's file.
pub(super) const PASS_SECONDS: f64 = 600.;
/// Kumi Ears arms for at most 900 s; a pass arms for its lead-in, the part, half a bar, four bars more and 6 s.
const ARMED_MOST: f64 = 880.;
/// What the devices hold at once, in seconds summed over them: Live keeps each recording in memory, 16 bytes a frame.
const PASS_BUDGET: f64 = 1200.;
/// Captures longer than this are read one after another rather than side by side.
const READ_ALONE: f64 = 60.;
/// How far a part runs on into the next one's start, in seconds: two playbacks (an LFO or a delay elsewhere in its
/// cycle) are crossfaded there rather than butted together.
const SEAM: f64 = 0.01;

/// What one device heard of a part: the part as a file, the lead before it (seconds), the frames it runs on past its
/// end for the seam, and where Live stopped short of the part's end when it did.
struct Part {
    file: PathBuf,
    lead: f64,
    tail: usize,
    short: Option<Missed>,
}

/// The parts one listen plays, each short enough for Kumi Ears to hold: the whole stretch in one pass when it
/// fits, else whole bars back to back.
fn pieces(window: Window, tempo: f64, meter: f64, taps: usize) -> Vec<Window> {
    let by_memory = PASS_SECONDS.min(PASS_BUDGET / taps.max(1) as f64) * tempo / 60.;
    let by_device = (ARMED_MOST - 6.) * tempo / 60. - (2. + 0.5 + 4.) * meter;
    let longest = by_memory.min(by_device);
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
        // A listen in parts keeps them in a folder of its own until they're joined, out of the folder's pruning.
        let folder =
            if pieces.len() > 1 { self.ears_folder.join(format!("parts-{}", uuid::Uuid::new_v4())) } else { self.ears_folder.clone() };
        let mut started = false;
        // Each tap's parts so far, in order; a tap that missed a part keeps what came before, and says so.
        let mut parts: IndexMap<String, (Vec<Part>, bool)> = IndexMap::new();
        let mut ended: Vec<String> = vec![];
        let result: Result<(), RuntimeError> = self
            .history
            .quietly(None, async {
                if pieces.len() > 1 {
                    tokio::fs::create_dir_all(&folder).await.map_err(plain)?;
                }
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
                    let heard =
                        self.ears_piece(rig, &link, &listening, &folder, *piece, index == 0, last, &mut started, signal.clone()).await?;
                    for (name, outcome) in heard {
                        let named = if taps.len() > 1 { format!("{name}: ") } else { String::new() };
                        match outcome {
                            Ok(part) => {
                                if let Some(short) = &part.short {
                                    rig.notes.push(format!("{named}{}", short.describe(piece.from, piece.beats)));
                                    ended.push(name.clone());
                                }
                                let entry = parts.entry(name).or_insert_with(|| (vec![], false));
                                entry.1 |= part.short.is_some();
                                entry.0.push(part);
                            }
                            Err(missed) => {
                                rig.notes.push(format!("{named}{}", missed.describe(piece.from, piece.beats)));
                                if let Some(entry) = parts.get_mut(&name) {
                                    entry.1 = true;
                                }
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
        let mut files = IndexMap::new();
        if result.is_ok() {
            for (name, (heard, short)) in parts {
                let lead = heard.first().map_or(0., |part| part.lead);
                let file = self.ears_folder.join(format!("{}.wav", uuid::Uuid::new_v4()));
                let written = match heard.as_slice() {
                    [only] if only.file.parent() == Some(self.ears_folder.as_path()) => Ok(only.file.clone()),
                    [only] => tokio::fs::rename(&only.file, &file).await.map(|_| file),
                    _ => {
                        let seams: Vec<_> = heard.iter().map(|part| (part.file.clone(), part.tail)).collect();
                        join_wavs(&file, &seams).await.map(|_| file)
                    }
                };
                match written {
                    Ok(file) => {
                        // What a short take holds after its lead, from the file's own length.
                        let seconds = if short { wav_seconds(&file).await.map(|seconds| (seconds - lead).max(0.)) } else { None };
                        files.insert(name, Render { file: file.to_string_lossy().into_owned(), start: lead, seconds });
                    }
                    Err(error) => rig.notes.push(format!("Kumi couldn't join what it heard of {name}: {error}")),
                }
            }
        }
        if pieces.len() > 1 {
            let _ = tokio::fs::remove_dir_all(&folder).await;
        }
        let this = self.clone();
        tokio::task::spawn_local(async move {
            this.prune_ears().await;
        });
        result?;
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
        folder: &Path,
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
                    let outcome = self.ears_part(&link, &tap, &raw, folder, span.position, piece, first, last, signal).await;
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
            if std::env::var("KUMI_TIMING").is_ok_and(|s| !s.is_empty()) {
                let missed: Vec<_> = heard
                    .values()
                    .filter_map(|outcome| outcome.as_ref().err())
                    .map(|missed| missed.describe(piece.from, piece.beats))
                    .collect();
                eprintln!(
                    "[ears pass · {} taps · {} beats from {}{}] {}",
                    taps.len(),
                    to_string(piece.beats),
                    to_string(piece.from),
                    if longer { " · again" } else { "" },
                    if missed.is_empty() { "heard".into() } else { missed.join(" ") }
                );
            }
            if !heard.values().any(|outcome| matches!(outcome, Err(missed) if missed.retry())) {
                break;
            }
        }
        Ok(heard)
    }

    /// One device's capture of a part, cut to it and written into `folder`, or why Live's playing missed it.
    #[allow(clippy::too_many_arguments)]
    async fn ears_part(
        &self,
        link: &Rc<dyn EarsLink>,
        tap: &Tap,
        raw: &RawFile,
        folder: &Path,
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
        // The last part keeps half a bar after it; the others run on a little into the next part, for the seam.
        let (after, seam) = if last { (meter / 2., 0.) } else { (0., round(SEAM * capture.sample_rate)) };
        let end = at + ((piece.beats + after) * part.samples_per_beat).round();
        let reached = (part.to as f64).min(end + seam);
        let wav = folder.join(format!("{}.wav", uuid::Uuid::new_v4()));
        write_capture_wav(&wav, &capture, at - lead, reached).await.map_err(unread)?;
        Ok(Part { file: wav, lead: lead / capture.sample_rate, tail: (reached - end).max(0.) as usize, short })
    }
}
fn plain(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}
/// A WAV's length in seconds, from its header (Kumi's own 32-bit float stereo files).
async fn wav_seconds(file: &Path) -> Option<f64> {
    use tokio::io::AsyncReadExt;
    let mut header = [0u8; 44];
    tokio::fs::File::open(file).await.ok()?.read_exact(&mut header).await.ok()?;
    let rate = u32::from_le_bytes(header[24..28].try_into().ok()?) as f64;
    let data = u32::from_le_bytes(header[40..44].try_into().ok()?) as f64;
    (rate > 0.).then(|| data / 8. / rate)
}
fn unread(error: impl std::fmt::Display) -> Missed {
    Missed::Unread(kumi_common::js::string::head(&error.to_string(), 200))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_long_listen_is_heard_in_whole_bars_each_kumi_ears_can_hold() {
        let window = Window { from: 8., beats: 2000. };
        // One tap at 120 BPM: ten minutes a part (1200 beats), then the rest.
        let one = pieces(window, 120., 4., 1);
        assert_eq!(one.iter().map(|piece| (piece.from, piece.beats)).collect::<Vec<_>>(), [(8., 1200.), (1208., 800.)]);
        // Eight taps share what Live holds at once: 150 s each, 300 beats, every part but the last on a bar.
        let eight = pieces(window, 120., 4., 8);
        assert_eq!(eight.len(), 7);
        assert!(eight[..6].iter().all(|piece| piece.beats == 300.));
        assert_eq!(eight.iter().map(|piece| piece.beats).sum::<f64>(), 2000.);
        assert!(eight.windows(2).all(|pair| pair[0].from + pair[0].beats == pair[1].from));
        // At 20 BPM the device's 900 s, not memory, bounds a part: the lead-in, margins and 6 s fit beside it.
        let slow = pieces(Window { from: 0., beats: 400. }, 20., 4., 1);
        let armed = (2. * 4. + slow[0].beats + 2. + 16.) * 60. / 20. + 6.;
        assert!(armed <= 900., "armed for {armed} s");
        // What fits is one part, whatever the tempo is unknown to be.
        assert_eq!(pieces(Window { from: 4., beats: 32. }, 128., 4., 2).len(), 1);
        assert_eq!(pieces(Window { from: 4., beats: 32. }, f64::NAN, 4., 2).len(), 1);
    }
}
