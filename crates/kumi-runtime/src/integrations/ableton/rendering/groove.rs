//! The groove judge: a part's feel shaped toward a reference's, judged on the notes, without listening. Each round
//! re-reads the part, keeps a change only if its target got closer and nothing else moved away, and takes it back
//! otherwise. Code can move the notes itself (the reference's timing and accents, step by step); a reference's drums,
//! from Live's Drums to MIDI, have their hits moved onto the drum stem's own onsets first. A pitched part that isn't in
//! Live (an audio file) is turned into notes with Basic Pitch, on a grid fitted to them. A part on a Drum Rack is read
//! in lanes by its pads; any other part, and a transcribed one, is one lane.

use super::super::{
    connection::NO_CURRENT_LIVE,
    notes::{clip_and_track, drum_rack, read_notes, track_name},
};
use super::rig::Window;
use super::*;
use crate::listening::{
    checklist::{Change, Row},
    notes::{feel, fit_grid, gaps, lines, onsets, refine, toward, Feel, Kit, Line, Note},
    round::{Next, Round, RoundKind},
};
use kumi_common::js::{number::to_string, string::head};
use std::collections::HashMap;

/// The tools that change a clip's notes: what a groove round takes back, on its clip, when it isn't kept.
const NOTE_TOOLS: [&str; 4] = ["change_notes", "delete_notes", "edit_notes", "transform_midi"];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GrooveRequest {
    /// The part: a MIDI clip.
    pub clip: Option<String>,
    /// The reference: a MIDI clip (a reference's drums through Drums to MIDI, say), or an audio file of a pitched part
    /// that isn't in Live.
    pub reference: Option<String>,
    /// An audio file reference's tempo (BPM); estimated when left out.
    pub reference_tempo: Option<f64>,
    /// The reference's drum stem (an audio clip), to move its hits onto their onsets.
    pub audio: Option<String>,
    pub change: Option<String>,
    /// Code moves the notes toward the reference, by `amount`.
    pub apply: bool,
    pub amount: Option<f64>,
    pub done: bool,
}

pub struct GrooveRun {
    clip: String,
    /// How the part's notes fall into lanes.
    kit: Kit,
    reference: Feel,
    reference_name: String,
    lines: Vec<Line>,
    first: Vec<Line>,
    target: Option<String>,
    checkpoint: Vec<String>,
    round: u32,
    started: i64,
    misses: HashMap<String, u32>,
    rounds: Vec<Round>,
}

impl GrooveRun {
    /// A new answer: what the producer asked for since isn't a round's to take back, so the run's changes count from
    /// these (the changes applied now).
    pub(super) fn carry_on(&mut self, applied: Vec<String>) {
        self.checkpoint = applied;
    }
}

/// Notes to measure: a clip's (with their ids) in its own time, or a transcription's in beats of its fitted grid.
struct Read {
    name: String,
    /// The notes Live plays, each with its id (-1 when Live didn't give one).
    notes: Vec<(i64, Note)>,
    /// Song beat of the clip's own time zero.
    song_zero: Option<f64>,
    /// Where what Live plays of the clip ends, in its own time (its loop's end, say).
    to: f64,
    /// Where the clip's notes have to end, in its own time: Live's bridge refuses a note past it.
    end: f64,
    /// How its notes fall into lanes.
    kit: Kit,
    /// Its tempo, when it isn't the Set's (a recording's).
    tempo: Option<f64>,
    /// Whether its velocities say how hard each note was played (a transcription's say how sure it was).
    velocities: bool,
}

impl Rendering {
    pub async fn groove(self: &Rc<Self>, request: &GrooveRequest, original: Signal) -> Result<Result<Round, String>, RuntimeError> {
        if !self.available() {
            return Ok(Err(NO_CURRENT_LIVE.into()));
        }
        let signal = abort::any([original, self.connection().lifetime.clone()]);
        let outcome = if let (Some(clip), Some(reference)) = (&request.clip, &request.reference) {
            self.groove_start(clip, (reference, request.reference_tempo), request.audio.as_deref(), signal.clone()).await
        } else if self.groove.borrow().is_none() {
            return Ok(Err("Start with clip (the part) and reference (a MIDI clip): groove compares their feel.".into()));
        } else if request.done {
            Ok(Ok(self.groove_done()))
        } else {
            self.groove_round(request, signal.clone()).await
        };
        match outcome {
            Ok(Ok(round)) => {
                self.tell_judged(&round);
                Ok(Ok(round))
            }
            Ok(Err(why)) => Ok(Err(why)),
            Err(error) => {
                signal.check()?;
                Ok(Err(head(&error.to_string(), 400)))
            }
        }
    }

    /// A pitched part in an audio file (one that isn't in Live) as notes, with Basic Pitch: in beats of a grid fitted
    /// to them (its tempo within 2 % of the given or estimated one, half or double an estimate when that's nearer the
    /// Set's, and where its first downbeat falls), so a lead-in or a tempo off by a hair doesn't read as its feel. Its
    /// velocities are Basic Pitch's confidence, not how hard it was played: they aren't compared.
    async fn file_notes(&self, file: &str, tempo: Option<f64>, signal: Signal) -> Result<Read, String> {
        let base = match tempo {
            Some(tempo) => tempo,
            None => crate::audio::analyze::analyze_file(
                file,
                crate::audio::analyze::AnalyzeOptions { seconds: Some(120.), signal: Some(signal.clone()), ..Default::default() },
            )
            .await
            .ok()
            .and_then(|analysis| analysis.tempo)
            .map(|tempo| tempo.bpm)
            .ok_or("Kumi couldn't tell its tempo: give reference_tempo (its BPM).")?,
        };
        let say = |said: &str| self.tell(said.to_string(), None);
        let heard = crate::listening::transcribe::pitched_notes(std::path::Path::new(file), 0., 120., &say, &signal).await?;
        if heard.is_empty() {
            return Err("Basic Pitch heard no notes in it.".into());
        }
        let onsets: Vec<(f64, f64)> = heard.iter().map(|note| (note.start, note.strength as f64)).collect();
        let meter = self.observer.beats_per_bar.get().max(1.);
        let grid = fit_grid(&onsets, base, meter, tempo.is_none(), self.observer.tempo.get());
        let beats = grid.tempo / 60.;
        let notes: Vec<(i64, Note)> = heard
            .iter()
            .enumerate()
            .map(|(index, note)| {
                (
                    index as i64,
                    Note {
                        start: (note.start - grid.downbeat) * beats,
                        length: ((note.end - note.start) * beats).max(1. / 64.),
                        pitch: note.pitch as i32,
                        velocity: 100.,
                    },
                )
            })
            .collect();
        let name = file.rsplit(['/', '\\']).next().unwrap_or(file).to_string();
        Ok(Read {
            name: format!(
                "{name} ({} notes by Basic Pitch, on its grid: {} BPM, beat one {} s in)",
                notes.len(),
                to_string((grid.tempo * 100.).round() / 100.),
                to_string((grid.downbeat * 1000.).round() / 1000.)
            ),
            notes,
            song_zero: None,
            to: f64::INFINITY,
            end: f64::INFINITY,
            kit: Kit::pitched(),
            tempo: Some(grid.tempo),
            velocities: false,
        })
    }

    /// A clip's notes as Live has them: the ones it plays (not those before its start or past its end, which a split or
    /// shortened clip keeps), each with its id.
    async fn clip_notes(&self, clip: &str, signal: Signal) -> Result<Read, String> {
        let input = object(json!({"clipRef": clip, "format": "json"}));
        let read = read_notes(&input, self.connection(), self.observer.tempo.get(), signal).await;
        let done: Value = serde_json::from_str(&read.text).map_err(|_| "Kumi couldn't read the clip's notes.".to_string())?;
        let clip = &done["clips"][0];
        if let Some(error) = clip["error"].as_str() {
            return Err(format!("{}: {error}", clip["clip"].as_str().unwrap_or("the clip")));
        }
        let (from, to, end) = window(clip);
        let notes: Vec<(i64, Note)> = clip["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row["mute"].as_bool() != Some(true))
            .filter_map(|row| {
                Some((
                    row["id"].as_i64().unwrap_or(-1),
                    Note {
                        start: row["start"].as_f64()?,
                        length: row["duration"].as_f64().unwrap_or(0.25),
                        pitch: row["pitch"].as_i64()? as i32,
                        velocity: row["velocity"].as_f64().unwrap_or(100.),
                    },
                ))
            })
            .filter(|(_, note)| note.start >= from - 1e-9 && note.start < to - 1e-9)
            .collect();
        if notes.is_empty() {
            return Err(format!("{} has no notes to measure.", clip["name"].as_str().unwrap_or("The clip")));
        }
        let placement = &clip["placement"];
        let song_zero = placement["start"].as_f64().map(|start| start - placement["startMarker"].as_f64().unwrap_or(0.));
        Ok(Read {
            name: clip["name"].as_str().unwrap_or("the clip").to_string(),
            notes,
            song_zero,
            to,
            end,
            kit: Kit::pitched(),
            tempo: None,
            velocities: true,
        })
    }

    /// How a clip's notes fall into lanes: by its track's Drum Rack, when it has one.
    async fn lanes_of(&self, clip: &str, signal: Signal) -> Kit {
        let Some((_, track)) = clip_and_track(self.connection(), clip) else { return Kit::pitched() };
        drum_rack(self.connection(), &track, &signal).await.map_or_else(Kit::pitched, |pads| Kit::rack(&pads))
    }

    fn feel_of(&self, notes: &[(i64, Note)], kit: &Kit, tempo: Option<f64>) -> Feel {
        let notes: Vec<Note> = notes.iter().map(|(_, note)| *note).collect();
        let tempo = tempo.or(self.observer.tempo.get()).unwrap_or(120.);
        feel(&notes, tempo, self.observer.beats_per_bar.get().max(1.), kit)
    }

    /// The changes applied since a checkpoint, split into the note edits on the run's clip (by its long ref) and the
    /// rest: what the producer changed in between, or a judge round kept, isn't a groove round's to take back.
    fn note_changes_since(&self, checkpoint: &[String], clip: Option<&str>) -> (Vec<(String, String)>, Vec<(String, String)>) {
        let entries = self.history.entries.borrow();
        self.applied_since(checkpoint).into_iter().partition(|(id, _)| {
            entries.get(id).is_some_and(|entry| {
                let entry = entry.borrow();
                entry.tool.as_deref().is_some_and(|tool| NOTE_TOOLS.contains(&tool)) && clip.is_some() && entry.clip.as_deref() == clip
            })
        })
    }

    /// The reference's hits moved onto its drum stem's onsets: the stem heard quietly over the reference's span.
    async fn refined(self: &Rc<Self>, reference: &mut Read, audio: &str, signal: Signal) -> Result<usize, String> {
        let Some(zero) = reference.song_zero else {
            return Err("Onsets line up only with an Arrangement clip: put the reference's MIDI in the Arrangement.".into());
        };
        // The model has the stem's short ref: its long one says its track.
        let Some((_, track)) = clip_and_track(self.connection(), audio).filter(|(long, _)| long.contains(":arrangement_clip:")) else {
            return Err("audio is the drum stem's Arrangement clip (its clipRef).".into());
        };
        let name = track_name(self.connection(), &track, signal.clone()).await.ok_or("Kumi couldn't find the drum stem's track.")?;
        let first = reference.notes.iter().map(|(_, note)| note.start).fold(f64::MAX, f64::min);
        let last = reference.notes.iter().map(|(_, note)| note.start + note.length).fold(0., f64::max);
        let meter = self.observer.beats_per_bar.get().max(1.);
        let from = ((zero + first) / meter).floor() * meter;
        let window = Window { from, beats: (((zero + last) - from) / meter).ceil().max(1.) * meter };
        let candidates = vec![AuditionCandidate { track: name, mix: None, label: None, clip: None }];
        self.begin_rendering();
        let mut rig = None;
        let rendered: Result<IndexMap<String, Render>, RuntimeError> = async {
            rig = Some(self.open_rig(&candidates, Some(window.from), Some(window.beats), signal.clone()).await?);
            self.render_pass(rig.as_mut().unwrap(), signal.clone()).await
        }
        .await;
        if let Some(rig) = rig.as_mut() {
            self.close_rig(rig).await;
        }
        self.end_rendering();
        let files = rendered.map_err(|error| error.to_string())?;
        let render = files.values().next().cloned().ok_or("Nothing came through from the drum stem.")?;
        let mut source = crate::audio::decode::open_audio(&render.file, Some(signal)).await.map_err(|error| error.0)?;
        let rate = source.sample_rate;
        source.seek(render.start * rate);
        let mut samples: Vec<f32> = vec![];
        let frames = (window.beats * 60. / self.observer.tempo.get().unwrap_or(120.) * rate) as usize;
        while samples.len() < frames {
            let Some(block) = source.read(65536.min(frames - samples.len())).await.map_err(|error| error.0)? else { break };
            let right = block.get(1).unwrap_or(&block[0]);
            samples.extend(block[0].iter().zip(right).map(|(l, r)| (l + r) / 2.));
        }
        let _ = source.close().await;
        let tempo = self.observer.tempo.get().unwrap_or(120.);
        // Onsets in the clip's own beats.
        let found: Vec<f64> = onsets(&samples, rate).iter().map(|seconds| window.from + seconds * tempo / 60. - zero).collect();
        let mut notes: Vec<Note> = reference.notes.iter().map(|(_, note)| *note).collect();
        let moved = refine(&mut notes, &found, 0.04 * tempo / 60.);
        for ((_, note), refined) in reference.notes.iter_mut().zip(notes) {
            *note = refined;
        }
        Ok(moved)
    }

    async fn groove_start(
        self: &Rc<Self>,
        clip: &str,
        (reference, reference_tempo): (&str, Option<f64>),
        audio: Option<&str>,
        signal: Signal,
    ) -> Result<Result<Round, String>, RuntimeError> {
        let file = audio::audio_path(reference);
        let path = std::path::Path::new(&file);
        if path.is_file()
            && path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ["mid", "midi"].contains(&ext.to_lowercase().as_str()))
        {
            return Ok(Err(
                "The reference is a MIDI file: put it on a MIDI track in Live (drag it in) and give that clip as reference.".into()
            ));
        }
        let mut wanted = if path.is_file() {
            match self.file_notes(&file, reference_tempo, signal.clone()).await {
                Ok(read) => read,
                Err(why) => return Ok(Err(format!("The reference: {why}"))),
            }
        } else {
            match self.clip_notes(reference, signal.clone()).await {
                Ok(mut read) => {
                    read.kit = self.lanes_of(reference, signal.clone()).await;
                    read
                }
                Err(why) => return Ok(Err(format!("The reference: {why}"))),
            }
        };
        let mut said = String::new();
        if let Some(audio) = audio {
            match self.refined(&mut wanted, audio, signal.clone()).await {
                Ok(moved) => said = format!("; {moved} of its hits moved onto the drum stem's onsets"),
                Err(why) => return Ok(Err(why)),
            }
        }
        let part = match self.clip_notes(clip, signal.clone()).await {
            Ok(read) => read,
            Err(why) => return Ok(Err(why)),
        };
        let mut kit = self.lanes_of(clip, signal).await;
        let mut reference_feel = self.feel_of(&wanted.notes, &wanted.kit, wanted.tempo);
        let mut part_feel = self.feel_of(&part.notes, &kit, None);
        // Lanes in common or none: a drum part against a pitched reference (or the other way round) is read as one lane
        // each, so their timing still compares.
        if !part_feel.lanes.iter().any(|lane| reference_feel.lane(&lane.name).is_some()) {
            kit = Kit::pitched();
            reference_feel = self.feel_of(&wanted.notes, &kit, wanted.tempo);
            part_feel = self.feel_of(&part.notes, &kit, None);
        }
        if !wanted.velocities {
            reference_feel = reference_feel.without_velocities();
        }
        let now = lines(&gaps(&part_feel, &reference_feel));
        let mut run = GrooveRun {
            clip: clip.into(),
            kit,
            reference: reference_feel,
            reference_name: wanted.name.clone(),
            lines: now.clone(),
            first: now,
            target: None,
            checkpoint: self.applied_ids(),
            round: 0,
            started: now_ms(),
            misses: HashMap::new(),
            rounds: vec![],
        };
        run.target = next_line(&run);
        let rows: Vec<Row> = run
            .lines
            .iter()
            .map(|line| Row {
                id: line.id.clone(),
                label: line.label.clone(),
                unit: line.unit.clone(),
                wanted: wanted_of(line),
                before: None,
                after: Some(line.value),
                gap_before: 0.,
                gap_after: round1(line.off()),
                change: Change::Same,
            })
            .collect();
        let round = Round {
            round: 0,
            kind: RoundKind::Start,
            heard: format!("the notes of {} against {}{said}", part.name, wanted.name),
            target: None,
            change: None,
            changes: vec![],
            rows,
            kept: None,
            why: None,
            rebalanced: None,
            listener: None,
            problems: vec![],
            next: next_of(&run),
            met: run.target.is_none(),
            listens: 0,
            elapsed_ms: 0,
        };
        run.rounds.push(round.clone());
        *self.groove.borrow_mut() = Some(run);
        Ok(Ok(round))
    }

    async fn groove_round(self: &Rc<Self>, request: &GrooveRequest, signal: Signal) -> Result<Result<Round, String>, RuntimeError> {
        let (clip, checkpoint, kit) = {
            let run = self.groove.borrow();
            let run = run.as_ref().unwrap();
            (run.clip.clone(), run.checkpoint.clone(), run.kit.clone())
        };
        let mut change = request.change.clone();
        if request.apply {
            // Code moves the notes: each step's by how far the reference's timing and accent there are from the part's.
            let read = match self.clip_notes(&clip, signal.clone()).await {
                Ok(read) => read,
                Err(why) => return Ok(Err(why)),
            };
            if read.notes.iter().any(|(id, _)| *id < 0) {
                return Ok(Err(
                    "Live gave the clip's notes without their ids, so Kumi can't move them: update Kumi's bridge (kumi doctor says how)."
                        .into(),
                ));
            }
            let amount = request.amount.unwrap_or(1.).clamp(0., 1.);
            let notes: Vec<Note> = read.notes.iter().map(|(_, note)| *note).collect();
            let part = self.feel_of(&read.notes, &kit, None);
            let moved = {
                let run = self.groove.borrow();
                toward(&notes, &part, &run.as_ref().unwrap().reference, self.observer.beats_per_bar.get().max(1.), amount)
            };
            let patches: Vec<Value> =
                read.notes.iter().zip(&moved).filter_map(|((id, before), after)| inside(*id, before, after, (read.to, read.end))).collect();
            if patches.is_empty() {
                return Ok(Err("The notes already sit where the reference's do; nothing to move.".into()));
            }
            let count = patches.len();
            // One change, so Live takes all of it or none, and HISTORY takes it back as one.
            self.step("change_notes", json!({"clipRef": clip, "notes": patches}), signal.clone()).await?;
            change = Some(format!(
                "{}moved {count} notes {}toward the reference's timing and accents",
                change.map(|said| format!("{}: ", said.trim_end_matches('.'))).unwrap_or_default(),
                if amount < 1. { format!("{}% of the way ", (amount * 100.).round()) } else { String::new() }
            ));
        }
        let read = match self.clip_notes(&clip, signal.clone()).await {
            Ok(read) => read,
            Err(why) => return Ok(Err(why)),
        };
        let part = self.feel_of(&read.notes, &kit, None);
        let mine = clip_and_track(self.connection(), &clip).map(|(long, _)| long);
        let (changes, others) = self.note_changes_since(&checkpoint, mine.as_deref());
        let (rows, kept, why, target_label) = {
            let run = self.groove.borrow();
            let run = run.as_ref().unwrap();
            let now = lines(&gaps(&part, &run.reference));
            verdict(run, &now)
        };
        let mut why = why;
        if !kept {
            let mut refused = vec![];
            for (id, title) in changes.iter().rev() {
                match self.history.undo(id, signal.clone(), false).await {
                    Ok(undone) if !undone.is_error => {}
                    _ => refused.push(title.clone()),
                }
            }
            let titles = |changes: &[(String, String)]| changes.iter().map(|(_, title)| title.as_str()).collect::<Vec<_>>().join(", ");
            if changes.is_empty() {
                why.push_str("; no note changes on the part to take back");
            } else if refused.is_empty() {
                why.push_str(&format!("; taken back: {}", titles(&changes)));
            } else {
                why.push_str(&format!("; Live wouldn't take back {}: undo it yourself", refused.join(", ")));
            }
            // A groove round takes back only the part's note edits: say what else stays.
            if !others.is_empty() {
                why.push_str(&format!("; left as they are (not note changes on the part): {}", titles(&others)));
            }
        }
        let ids = self.applied_ids();
        let mut guard = self.groove.borrow_mut();
        let run = guard.as_mut().unwrap();
        run.round += 1;
        run.checkpoint = ids;
        if let Some(target) = run.target.clone() {
            let misses = run.misses.entry(target).or_insert(0);
            *misses = if kept { 0 } else { *misses + 1 };
        }
        if kept {
            run.lines =
                rows.iter().zip(run.lines.clone()).map(|(row, line)| Line { value: row.after.unwrap_or(line.value), ..line }).collect();
        }
        run.target = next_line(run);
        let round = Round {
            round: run.round,
            kind: RoundKind::Judged,
            heard: format!("the notes of {} against {}", read.name, run.reference_name),
            target: target_label,
            change,
            changes: changes.into_iter().map(|(_, title)| title).collect(),
            rows,
            kept: Some(kept),
            why: Some(why),
            rebalanced: None,
            listener: None,
            problems: vec![],
            next: next_of(run),
            met: run.target.is_none(),
            listens: 0,
            elapsed_ms: now_ms() - run.started,
        };
        run.rounds.push(round.clone());
        Ok(Ok(round))
    }

    fn groove_done(&self) -> Round {
        let mut guard = self.groove.borrow_mut();
        let run = guard.as_mut().unwrap();
        let kept = run.rounds.iter().filter(|round| round.kept == Some(true)).count();
        let reverted = run.rounds.iter().filter(|round| round.kept == Some(false)).count();
        let rows: Vec<Row> = run
            .first
            .iter()
            .zip(&run.lines)
            .map(|(first, now)| Row {
                id: now.id.clone(),
                label: now.label.clone(),
                unit: now.unit.clone(),
                wanted: wanted_of(now),
                before: Some(first.value),
                after: Some(now.value),
                gap_before: round1(first.off()),
                gap_after: round1(now.off()),
                change: change_of(first, now),
            })
            .collect();
        let round = Round {
            round: run.round,
            kind: RoundKind::Done,
            heard: format!("the notes against {}", run.reference_name),
            target: None,
            change: Some(format!("{kept} changes kept, {reverted} taken back")),
            changes: vec![],
            rows,
            kept: None,
            why: None,
            rebalanced: None,
            listener: None,
            problems: vec![],
            next: next_of(run),
            met: run.target.is_none(),
            listens: 0,
            elapsed_ms: now_ms() - run.started,
        };
        run.rounds.push(round.clone());
        round
    }
}

/// The part of a clip's own time Live plays (its notes there are heard), and where its notes have to end: the latest
/// of its length, loop end and end marker, as Live's bridge holds them. Unbounded where Live didn't say.
fn window(clip: &Value) -> (f64, f64, f64) {
    let (info, session) = match clip.get("loop") {
        Some(info) => (info, true),
        None => (&clip["placement"], false),
    };
    let number = |key: &str| info[key].as_f64().filter(|value| value.is_finite());
    let end = ["length", "loopEnd", "endMarker"].iter().filter_map(|key| number(key)).fold(f64::NEG_INFINITY, f64::max);
    let end = if end.is_finite() { end } else { f64::INFINITY };
    let looped = match (number("loopStart"), number("loopEnd")) {
        (Some(from), Some(to)) if info["looping"].as_bool() == Some(true) && to > from => Some((from, to)),
        _ => None,
    };
    let marker = number("startMarker").or(number("loopStart"));
    let unbounded = (f64::NEG_INFINITY, f64::INFINITY, end);
    if session {
        // A Session clip plays from its start marker: its loop over and over, or on to its end marker.
        return match (looped, marker) {
            (Some((from, to)), marker) => (marker.unwrap_or(from).min(from), to, end),
            (None, marker) => (marker.unwrap_or(f64::NEG_INFINITY), number("endMarker").unwrap_or(f64::INFINITY), end),
        };
    }
    // An Arrangement clip plays for its span from its start marker, its loop repeating once reached.
    let span = number("endTime").zip(number("start")).map(|(to, from)| to - from).or(number("length"));
    match (marker, span, looped) {
        (Some(marker), Some(span), Some((from, to))) if span > to - marker => (marker.min(from), to, end),
        (Some(marker), Some(span), _) => (marker, marker + span, end),
        _ => unbounded,
    }
}

/// A note's patch, when it moved: kept inside what the clip plays (`to`, where its loop ends, say) and inside the clip
/// (`end`), since Live's bridge refuses a note that ends past it. A note moved later starts no later than just short
/// of what's played, and ends by the clip's end.
fn inside(id: i64, before: &Note, after: &Note, (to, end): (f64, f64)) -> Option<Value> {
    let start = after.start.min((to.min(end) - 1. / 64.).max(before.start));
    if (start - before.start).abs() <= 1e-4 && before.velocity == after.velocity {
        return None;
    }
    let mut patch = json!({"id": id, "start": start, "velocity": after.velocity});
    let length = before.length.min(end - start);
    if length < before.length - 1e-9 {
        patch["duration"] = json!(length);
    }
    Some(patch)
}

/// The biggest gap, passing over a target two changes in a row failed on while another is open.
fn next_line(run: &GrooveRun) -> Option<String> {
    let open: Vec<&Line> = run.lines.iter().filter(|line| line.off() > 0.).collect();
    let fresh = open.iter().filter(|line| run.misses.get(&line.id).copied().unwrap_or(0) < 2).max_by(|a, b| a.off().total_cmp(&b.off()));
    fresh.or_else(|| open.iter().max_by(|a, b| a.off().total_cmp(&b.off()))).map(|line| line.id.clone())
}

fn next_of(run: &GrooveRun) -> Option<Next> {
    let line = run.lines.iter().find(|line| Some(&line.id) == run.target.as_ref())?;
    let fix = if line.id == "swing" {
        "transform_midi swing, or groove with apply: true"
    } else if line.id.starts_with("timing") {
        "move those notes (change_notes), or groove with apply: true"
    } else if line.id.starts_with("velocity") {
        "change those notes' velocities, or groove with apply: true"
    } else if line.id.starts_with("density") {
        "add or remove notes where the reference plays and doesn't"
    } else {
        "change the notes toward the reference's"
    };
    Some(Next {
        id: line.id.clone(),
        label: line.label.clone(),
        gap: round1(line.off()),
        wanted: wanted_of(line),
        now: Some(line.value),
        fix: Some(fix.into()),
    })
}

fn wanted_of(line: &Line) -> String {
    format!("≤ {}{}", to_string(line.within), if line.unit.is_empty() { String::new() } else { format!(" {}", line.unit) })
}

fn change_of(before: &Line, after: &Line) -> Change {
    let (was, now) = (before.off(), after.off());
    if now < was - 0.5 || (was > 0. && now == 0.) {
        Change::Better
    } else if now > was + 1. || (was == 0. && now > 0.5) {
        Change::Worse
    } else {
        Change::Same
    }
}

/// Before against after: kept when the target got closer (or met) and nothing else moved away by a step.
fn verdict(run: &GrooveRun, now: &[Line]) -> (Vec<Row>, bool, String, Option<String>) {
    let rows: Vec<Row> = run
        .lines
        .iter()
        .map(|before| {
            let after = now.iter().find(|line| line.id == before.id).unwrap_or(before);
            Row {
                id: before.id.clone(),
                label: before.label.clone(),
                unit: before.unit.clone(),
                wanted: wanted_of(before),
                before: Some(before.value),
                after: Some(after.value),
                gap_before: round1(before.off()),
                gap_after: round1(after.off()),
                change: change_of(before, after),
            }
        })
        .collect();
    let target = run.target.as_ref().and_then(|id| rows.iter().find(|row| &row.id == id));
    let label = target.map(|row| row.label.clone());
    let worse: Vec<String> = rows
        .iter()
        .filter(|row| row.change == Change::Worse && Some(&row.id) != run.target.as_ref())
        .map(|row| row.label.to_lowercase())
        .collect();
    let improved = target.is_none_or(|row| row.change == Change::Better);
    let (kept, why) = match (improved, worse.is_empty()) {
        (true, true) => {
            (true, format!("{} closer to the reference and nothing else moved away", label.clone().unwrap_or_else(|| "The feel".into())))
        }
        (true, false) => (false, format!("it moved {} away from the reference", worse.join(", "))),
        (false, _) => (false, format!("{} didn't get closer", label.clone().unwrap_or_else(|| "The target".into()))),
    };
    (rows, kept, why, label)
}

fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}
