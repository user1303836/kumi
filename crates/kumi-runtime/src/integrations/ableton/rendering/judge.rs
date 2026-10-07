//! The judge in Live: a goal as a checklist, heard quietly. The run starts with the whole stretch heard once (the
//! problems found there, placed in time), then judges each change on an excerpt (the bars around the target's worst
//! moment, else the loudest part), keeps it only if its target improved and nothing else got audibly worse, takes
//! it back with Kumi's undo otherwise, brings loudness back to target after every kept change, and logs every
//! round. The whole stretch is heard again only at the end.

use super::super::{connection::NO_CURRENT_LIVE, display::parse_display};
use super::rig::Window;
use super::*;
use crate::listening::{
    checklist::{Checklist, Explicit, Goal, Profile, Quantity, Row},
    detect::{self, Problem},
    listener::{compare, Choice, Listener, Opinion, Take},
    measure::{measure_file, Heard, MeasureOptions},
    round::{Next, Round, RoundKind},
};
use kumi_common::js::{number::to_string, string::head};

/// What the model asks of the judge: a goal (which starts a run), what to hear, the change since the last call, or
/// the end.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JudgeRequest {
    pub goal: Option<GoalRequest>,
    /// A track to judge instead of the mix.
    pub track: Option<String>,
    pub from_beat: Option<f64>,
    pub beats: Option<f64>,
    pub change: Option<String>,
    pub done: bool,
}

/// A goal as the model gives it: targets, a reference (a file or a clip), whether to clear what the detectors find,
/// and the element that must cut through.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GoalRequest {
    pub loudness: Option<f64>,
    pub true_peak: Option<f64>,
    pub reference: Option<String>,
    pub problems: bool,
    pub focus: Option<String>,
    pub targets: Vec<Explicit>,
}

/// One part of the run's stretch heard as things stand: its checklist values and its sound.
#[derive(Debug, Clone)]
struct Excerpt {
    window: Window,
    state: u32,
    values: Vec<Option<f64>>,
    file: PathBuf,
    start: f64,
    /// Its integrated loudness, to play it level with another take.
    loudness: Option<f64>,
}

/// Where rebalancing turns the level: a gain knob, and whether a limiter comes after it (then peaks stay put).
#[derive(Debug, Clone)]
struct GainStage {
    device: String,
    parameter: String,
    label: String,
    limited: bool,
}

/// A judged run: its checklist, what the whole stretch measures as things stand, and the rounds so far.
pub struct JudgeRun {
    checklist: Checklist,
    track: Option<String>,
    focus: Option<String>,
    span: Window,
    /// The span's own capture (its file and where the part starts), to cut excerpts from before anything changes.
    span_file: (PathBuf, f64),
    loudness: Vec<Option<f64>>,
    problems: Vec<Problem>,
    first: Vec<Option<f64>>,
    whole: Vec<Option<f64>>,
    excerpts: Vec<Excerpt>,
    state: u32,
    target: Option<usize>,
    window: Window,
    checkpoint: Vec<String>,
    round: u32,
    listens: u32,
    started: i64,
    gain: Option<GainStage>,
    pub rounds: Vec<Round>,
}

impl JudgeRun {
    /// The last round, as the app and /goal read it.
    pub fn last(&self) -> Option<&Round> {
        self.rounds.last()
    }
}

impl Rendering {
    pub async fn judge(self: &Rc<Self>, request: &JudgeRequest, original: Signal) -> Result<Result<Round, String>, RuntimeError> {
        if !self.available() {
            return Ok(Err(NO_CURRENT_LIVE.into()));
        }
        if self.observer.tempo.get().filter(|tempo| *tempo > 0.).is_none() {
            return Ok(Err("Kumi doesn't know the Set's tempo yet; try again.".into()));
        }
        if self.rendering.get() {
            return Ok(Err("Kumi is already listening to something; wait for it.".into()));
        }
        let signal = abort::any([original, self.connection().lifetime.clone()]);
        let outcome = if let Some(goal) = &request.goal {
            self.judge_start(goal, request, signal.clone()).await
        } else if self.judge.borrow().is_none() {
            return Ok(Err("Start with a goal: judge {goal: {...}} hears the mix (or a track) and makes the checklist.".into()));
        } else if request.done {
            self.judge_done(signal.clone()).await
        } else {
            self.judge_round(request.change.clone(), signal.clone()).await
        };
        match outcome {
            Ok(Ok(round)) => {
                if let Some(tell) = &self.on_judge {
                    let _ = catch_unwind(AssertUnwindSafe(|| tell(round.clone())));
                }
                Ok(Ok(round))
            }
            Ok(Err(why)) => Ok(Err(why)),
            Err(error) => {
                signal.check()?;
                Ok(Err(head(&error.to_string(), 400)))
            }
        }
    }

    /// The last round of the current (or last) run.
    pub fn judged(&self) -> Option<Round> {
        self.judge.borrow().as_ref().and_then(|run| run.last().cloned())
    }

    async fn judge_start(
        self: &Rc<Self>,
        goal: &GoalRequest,
        request: &JudgeRequest,
        signal: Signal,
    ) -> Result<Result<Round, String>, RuntimeError> {
        let tempo = self.observer.tempo.get().unwrap();
        let started = now_ms();
        *self.judge.borrow_mut() = None;
        let reference = match &goal.reference {
            Some(named) => match self.reference_profile(named, signal.clone()).await {
                Ok(profile) => Some(profile),
                Err(why) => return Ok(Err(why)),
            },
            None => None,
        };
        let span = match (request.from_beat, request.beats) {
            (Some(from), Some(beats)) if beats > 0. => Window { from, beats },
            (from, beats) => {
                let Some(end) = self.song_end(signal.clone()).await else {
                    return Ok(Err("Kumi couldn't tell where the song ends; give from_beat and beats.".into()));
                };
                let from = from.unwrap_or(0.).max(0.);
                Window { from, beats: beats.unwrap_or(end - from).max(self.observer.beats_per_bar.get()) }
            }
        };
        let heard = match self.judge_hear(request.track.as_deref(), goal.focus.as_deref(), span, signal.clone()).await? {
            Ok(heard) => heard,
            Err(why) => return Ok(Err(why)),
        };
        let offset = span.from * 60. / tempo;
        let mut problems = detect::harshness(&heard.main);
        problems.extend(detect::low_end(&heard.main));
        problems.extend(detect::peaks(&heard.main, goal.true_peak));
        if let (Some(focus), Some(name)) = (&heard.focus, &goal.focus) {
            problems.extend(detect::masking(focus, &heard.main, name));
        }
        // Times as the song's, not the capture's.
        for problem in &mut problems {
            for span in &mut problem.at {
                span[0] += offset;
                span[1] += offset;
            }
        }
        let goal = Goal {
            loudness: goal.loudness,
            true_peak: goal.true_peak,
            reference,
            problems: goal.problems,
            focus: goal.focus.clone(),
            targets: goal.targets.clone(),
        };
        let checklist = Checklist::new(&goal, &heard.main, &problems);
        let whole = checklist.read(&heard.main, heard.focus.as_ref());
        let mut run = JudgeRun {
            checklist,
            track: request.track.clone(),
            focus: goal.focus.clone(),
            span,
            span_file: (heard.file.clone(), heard.start),
            loudness: heard.main.measures.short_term.clone(),
            problems: problems.clone(),
            first: whole.clone(),
            whole: whole.clone(),
            excerpts: vec![],
            state: 0,
            target: None,
            window: span,
            checkpoint: self.applied_ids(),
            round: 0,
            listens: 1,
            started,
            gain: None,
            rounds: vec![],
        };
        run.target = run.checklist.next(&run.whole);
        run.window = self.excerpt_for(&run);
        // Before anything changes, the excerpt's "before" is cut from what was just heard.
        let cut = self.cut_excerpt(&run, run.window, signal.clone()).await?;
        run.excerpts.push(cut);
        let rows: Vec<Row> = run
            .checklist
            .items
            .iter()
            .zip(&run.whole)
            .map(|(item, value)| Row {
                id: item.id.clone(),
                label: item.label.clone(),
                unit: item.unit.clone(),
                wanted: item.wanted(),
                before: None,
                after: value.map(round1),
                gap_before: 0.,
                gap_after: round1(item.gap(*value)),
                change: crate::listening::checklist::Change::Same,
            })
            .collect();
        let round = Round {
            round: 0,
            kind: RoundKind::Start,
            heard: self.describe(span, &run),
            target: None,
            change: None,
            changes: vec![],
            rows,
            kept: None,
            why: None,
            rebalanced: None,
            listener: None,
            problems,
            next: self.next_step(&run),
            met: run.target.is_none(),
            listens: run.listens,
            elapsed_ms: now_ms() - run.started,
        };
        run.rounds.push(round.clone());
        *self.judge.borrow_mut() = Some(run);
        Ok(Ok(round))
    }

    async fn judge_round(self: &Rc<Self>, change: Option<String>, signal: Signal) -> Result<Result<Round, String>, RuntimeError> {
        let (track, focus, window, target, state) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            (run.track.clone(), run.focus.clone(), run.window, run.target, run.state)
        };
        let before = {
            let run = self.judge.borrow();
            run.as_ref().unwrap().excerpts.iter().find(|excerpt| excerpt.window == window && excerpt.state == state).cloned()
        };
        let Some(before) = before else {
            return Ok(Err("Kumi lost what this excerpt sounded like before the change; start the run again with a goal.".into()));
        };
        let changes = self.applied_since(&self.judge.borrow().as_ref().unwrap().checkpoint);
        let heard = match self.judge_hear(track.as_deref(), focus.as_deref(), window, signal.clone()).await? {
            Ok(heard) => heard,
            Err(why) => return Ok(Err(why)),
        };
        let (after, predicted, verdict, target_label) = {
            let mut guard = self.judge.borrow_mut();
            let run = guard.as_mut().unwrap();
            run.listens += 1;
            run.round += 1;
            let after = run.checklist.read(&heard.main, heard.focus.as_ref());
            // The whole stretch as it would be now: what the excerpt moved, moved there too.
            let predicted: Vec<Option<f64>> = run
                .whole
                .iter()
                .zip(&before.values)
                .zip(&after)
                .zip(&run.checklist.items)
                .map(|(((whole, before), after), item)| match (whole, before, after) {
                    (Some(whole), Some(before), Some(after)) => {
                        let moved = whole + (after - before);
                        Some(if item.quantity == Quantity::Clipped { moved.max(0.) } else { moved })
                    }
                    (_, _, after) => *after,
                })
                .collect();
            let verdict = run.checklist.verdict(target, &run.whole, &predicted);
            (after, predicted, verdict, target.map(|index| run.checklist.items[index].label.clone()))
        };
        let mut verdict = verdict;
        // The listening model hears the change too: advice, unless it hears an obvious artifact both ways.
        let aim = match (&target_label, target.map(|index| self.judge.borrow().as_ref().unwrap().checklist.items[index].wanted())) {
            (Some(label), Some(wanted)) => format!("{label} toward {wanted}, with nothing else getting worse"),
            _ => "a better-sounding mix with nothing getting worse".into(),
        };
        let heard_by = self.listen_to_change(&before, &heard, &aim, signal.clone()).await;
        let listener = heard_by.as_ref().map(|(name, opinion)| opinion.line(name));
        if let Some((_, opinion)) = &heard_by {
            let artifacts: Vec<&String> =
                opinion.new_problems.iter().filter(|problem| ["distorted", "pumping", "clipped"].contains(&problem.as_str())).collect();
            if verdict.kept && opinion.closer == Some(Choice::Before) && !artifacts.is_empty() {
                verdict.kept = false;
                verdict.why = format!(
                    "the listening model heard it {} both ways",
                    artifacts.iter().map(|problem| problem.as_str()).collect::<Vec<_>>().join(" and ")
                );
            }
        }
        let mut rebalanced = None;
        if verdict.kept {
            let gain = {
                let run = self.judge.borrow();
                let run = run.as_ref().unwrap();
                run.checklist.rebalance(&run.whole, &predicted)
            };
            let mut predicted = predicted;
            let mut excerpt_values = after.clone();
            if let Some(gain) = gain {
                match self.rebalance(gain, signal.clone()).await {
                    Ok((label, limited)) => {
                        let run = self.judge.borrow();
                        let items = &run.as_ref().unwrap().checklist.items;
                        for values in [&mut predicted, &mut excerpt_values] {
                            for (value, item) in values.iter_mut().zip(items) {
                                let shifts = item.quantity == Quantity::Integrated || (item.quantity == Quantity::TruePeak && !limited);
                                if let (Some(value), true) = (value.as_mut(), shifts) {
                                    *value += gain;
                                }
                            }
                        }
                        rebalanced = Some(format!("{label} {}{} dB", if gain > 0. { "+" } else { "" }, to_string(gain)));
                    }
                    Err(why) => rebalanced = Some(format!("loudness is {} dB off; {why}", to_string(-gain))),
                }
            }
            let file = self.keep_file(&heard.file).await;
            let mut guard = self.judge.borrow_mut();
            let run = guard.as_mut().unwrap();
            run.whole = predicted;
            run.state += 1;
            let state = run.state;
            run.excerpts.retain(|excerpt| excerpt.state == state);
            run.excerpts.push(Excerpt {
                window,
                state,
                values: excerpt_values,
                file,
                start: heard.start,
                loudness: heard.main.measures.integrated,
            });
        } else {
            // Kumi's undo takes the round's changes back, newest first.
            for id in changes.iter().rev().map(|(id, _)| id) {
                let _ = self.history.undo(id, signal.clone(), false).await;
            }
        }
        let ids = self.applied_ids();
        {
            let mut guard = self.judge.borrow_mut();
            let run = guard.as_mut().unwrap();
            run.checkpoint = ids;
            run.target = run.checklist.next(&run.whole);
        }
        // The next change is judged on its own excerpt; its "before" is heard now if this state hasn't been.
        let next_window = self.excerpt_for(self.judge.borrow().as_ref().unwrap());
        let known = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            run.excerpts.iter().any(|excerpt| excerpt.window == next_window && excerpt.state == run.state)
        };
        if !known {
            let (track, focus) = {
                let run = self.judge.borrow();
                let run = run.as_ref().unwrap();
                (run.track.clone(), run.focus.clone())
            };
            let heard = match self.judge_hear(track.as_deref(), focus.as_deref(), next_window, signal.clone()).await? {
                Ok(heard) => heard,
                Err(why) => return Ok(Err(why)),
            };
            let file = self.keep_file(&heard.file).await;
            let mut guard = self.judge.borrow_mut();
            let run = guard.as_mut().unwrap();
            run.listens += 1;
            let values = run.checklist.read(&heard.main, heard.focus.as_ref());
            let state = run.state;
            run.excerpts.push(Excerpt {
                window: next_window,
                state,
                values,
                file,
                start: heard.start,
                loudness: heard.main.measures.integrated,
            });
        }
        let mut guard = self.judge.borrow_mut();
        let run = guard.as_mut().unwrap();
        run.window = next_window;
        let round = Round {
            round: run.round,
            kind: RoundKind::Judged,
            heard: self.describe(window, run),
            target: target_label,
            change,
            changes: changes.into_iter().map(|(_, title)| title).collect(),
            rows: verdict.rows,
            kept: Some(verdict.kept),
            why: Some(verdict.why),
            rebalanced,
            listener,
            problems: vec![],
            next: self.next_step(run),
            met: run.target.is_none(),
            listens: run.listens,
            elapsed_ms: now_ms() - run.started,
        };
        run.rounds.push(round.clone());
        Ok(Ok(round))
    }

    /// The listening model, looked for once.
    async fn listener(&self, signal: Signal) -> Option<Rc<dyn Listener>> {
        if let Some(known) = self.listener.borrow().clone() {
            return known;
        }
        let found = match &self.listener_source {
            Some(source) => source(signal).await,
            None => None,
        };
        *self.listener.borrow_mut() = Some(found.clone());
        found
    }

    /// What the listening model makes of the change: the excerpt before and after, level-matched, both ways.
    async fn listen_to_change(&self, before: &Excerpt, after: &JudgeHeard, aim: &str, signal: Signal) -> Option<(String, Opinion)> {
        let listener = self.listener(signal.clone()).await?;
        let tempo = self.observer.tempo.get().unwrap_or(120.);
        let seconds = before.window.beats * 60. / tempo;
        // Ten seconds from the middle of the excerpt.
        let skip = ((seconds - 10.) / 2.).max(0.);
        let gain = match (before.loudness, after.main.measures.integrated) {
            (Some(before), Some(after)) => before - after,
            _ => 0.,
        };
        let first = Take { file: before.file.clone(), start: before.start + skip, seconds: seconds.min(10.), gain: 0. };
        let second = Take { file: after.file.clone(), start: after.start + skip, seconds: seconds.min(10.), gain };
        match compare(listener.as_ref(), &first, &second, aim, signal).await {
            Ok(opinion) => Some((listener.name(), opinion)),
            Err(why) => Some((listener.name(), Opinion { closer: None, new_problems: vec![], said: why })),
        }
    }

    async fn judge_done(self: &Rc<Self>, signal: Signal) -> Result<Result<Round, String>, RuntimeError> {
        let (track, focus, span) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            (run.track.clone(), run.focus.clone(), run.span)
        };
        let heard = match self.judge_hear(track.as_deref(), focus.as_deref(), span, signal).await? {
            Ok(heard) => heard,
            Err(why) => return Ok(Err(why)),
        };
        let mut guard = self.judge.borrow_mut();
        let run = guard.as_mut().unwrap();
        run.listens += 1;
        let now = run.checklist.read(&heard.main, heard.focus.as_ref());
        let verdict = run.checklist.verdict(None, &run.first, &now);
        run.whole = now;
        let target = run.checklist.next(&run.whole);
        let kept = run.rounds.iter().filter(|round| round.kept == Some(true)).count();
        let reverted = run.rounds.iter().filter(|round| round.kept == Some(false)).count();
        let round = Round {
            round: run.round,
            kind: RoundKind::Done,
            heard: self.describe(span, run),
            target: None,
            change: Some(format!("{kept} changes kept, {reverted} taken back")),
            changes: vec![],
            rows: verdict.rows,
            kept: None,
            why: None,
            rebalanced: None,
            listener: None,
            problems: vec![],
            next: target.map(|index| next_of(run, index)),
            met: target.is_none(),
            listens: run.listens,
            elapsed_ms: now_ms() - run.started,
        };
        run.rounds.push(round.clone());
        Ok(Ok(round))
    }

    /// A reference's profile, from a file or a clip in the Set.
    async fn reference_profile(&self, named: &str, signal: Signal) -> Result<Profile, String> {
        let file = (self.clip_file)(named.into(), signal.clone()).await.ok().flatten().unwrap_or_else(|| audio::audio_path(named));
        let heard = measure_file(&file, MeasureOptions { signal: Some(signal), ..Default::default() })
            .await
            .map_err(|error| format!("Kumi couldn't hear the reference: {}", head(&error.to_string(), 200)))?;
        let name = file.rsplit(['/', '\\']).next().unwrap_or(&file).to_string();
        Ok(Profile::of(&name, &heard))
    }

    /// Hears `window` quietly: the mix (or the run's track) and the focus element in one pass, measured.
    async fn judge_hear(
        self: &Rc<Self>,
        track: Option<&str>,
        focus: Option<&str>,
        window: Window,
        signal: Signal,
    ) -> Result<Result<JudgeHeard, String>, RuntimeError> {
        let tempo = self.observer.tempo.get().unwrap_or(120.);
        let main = match track {
            Some(track) => AuditionCandidate { track: track.into(), mix: None, label: None, clip: None },
            None => AuditionCandidate { track: MIX_CANDIDATE.into(), mix: Some(true), label: Some("The whole mix".into()), clip: None },
        };
        let mut candidates = vec![main];
        if let Some(focus) = focus.filter(|focus| Some(*focus) != track) {
            candidates.push(AuditionCandidate { track: focus.into(), mix: None, label: None, clip: None });
        }
        self.tell(format!("Listening quietly: {}", super::super::more_changes::bars(window.from)), Some(true));
        self.begin_rendering();
        let mut rig = None;
        let rendered: Result<(IndexMap<String, Render>, Vec<(String, bool)>), RuntimeError> = async {
            rig = Some(self.open_rig(&candidates, Some(window.from), Some(window.beats), signal.clone()).await?);
            let rig = rig.as_mut().unwrap();
            let files = self.render_pass(rig, signal.clone()).await?;
            Ok((files, rig.sources.iter().map(|source| (source.name.clone(), source.mix)).collect()))
        }
        .await;
        let mut notes = vec![];
        if let Some(rig) = rig.as_mut() {
            self.close_rig(rig).await;
            notes.extend(rig.notes.clone());
        }
        self.end_rendering();
        self.tell("Listened", Some(false));
        let (files, sources) = rendered?;
        let main_name = sources.iter().find(|(name, mix)| *mix || Some(name.as_str()) == track).map(|(name, _)| name.clone());
        let focus_name = sources.iter().find(|(name, mix)| !*mix && Some(name.as_str()) != track).map(|(name, _)| name.clone());
        let Some(main) = main_name.and_then(|name| files.get(&name).cloned()) else {
            return Ok(Err(format!(
                "Nothing came through{}",
                if notes.is_empty() { ". Is something playing there in the Arrangement?".into() } else { format!(": {}", notes.join(" ")) }
            )));
        };
        let seconds = main.seconds.unwrap_or(window.beats * 60. / tempo);
        let measure = |file: String, start: f64| {
            let signal = signal.clone();
            async move {
                measure_file(&file, MeasureOptions { start: Some(start), seconds: Some(seconds), signal: Some(signal) })
                    .await
                    .map_err(|error| RuntimeError::plain(error.to_string()))
            }
        };
        let heard_main = measure(main.file.clone(), main.start).await?;
        let heard_focus = match focus_name.and_then(|name| files.get(&name).cloned()) {
            Some(render) => Some(measure(render.file, render.start).await?),
            None => None,
        };
        let file = self.keep_file(&PathBuf::from(&main.file)).await;
        Ok(Ok(JudgeHeard { main: heard_main, focus: heard_focus, file, start: main.start }))
    }

    /// Copies a capture into the judge's own folder, out of the listening folder's pruning.
    async fn keep_file(&self, file: &std::path::Path) -> PathBuf {
        let folder = self.ears_folder.join("judge");
        let _ = tokio::fs::create_dir_all(&folder).await;
        let kept = folder.join(file.file_name().map(|name| name.to_os_string()).unwrap_or_else(|| "take.wav".into()));
        if file.parent() == Some(folder.as_path()) {
            return file.to_path_buf();
        }
        match tokio::fs::copy(file, &kept).await {
            Ok(_) => kept,
            Err(_) => file.to_path_buf(),
        }
    }

    /// The excerpt the next change is judged on: the bars around the target's worst moment when it has one, else the
    /// loudest part; the whole stretch when it's short.
    fn excerpt_for(&self, run: &JudgeRun) -> Window {
        let tempo = self.observer.tempo.get().unwrap_or(120.);
        let meter = self.observer.beats_per_bar.get().max(1.);
        let length = (8. * meter).max((12. * tempo / 60. / meter).ceil() * meter);
        if run.span.beats <= length * 1.5 {
            return run.span;
        }
        let worst = run.target.and_then(|index| {
            let id = &run.checklist.items[index].id;
            run.problems.iter().find(|problem| &problem.id == id).and_then(|problem| problem.at.first()).map(|span| span[0])
        });
        let center = match worst {
            Some(seconds) => seconds * tempo / 60. + meter,
            None => {
                // The loudest second of the short-term series (3 s windows, every second, from the span's start).
                let loudest = run
                    .loudness
                    .iter()
                    .enumerate()
                    .filter_map(|(at, value)| value.map(|value| (at, value)))
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                run.span.from + loudest.map_or(run.span.beats / 2., |(at, _)| (at as f64 + 1.5) * tempo / 60.)
            }
        };
        let from = ((center - length / 2.) / meter).floor() * meter;
        let from = from.clamp(run.span.from, run.span.from + run.span.beats - length);
        Window { from, beats: length }
    }

    /// An excerpt of the span's own capture, measured: what it sounded like before anything changed.
    async fn cut_excerpt(&self, run: &JudgeRun, window: Window, signal: Signal) -> Result<Excerpt, RuntimeError> {
        let tempo = self.observer.tempo.get().unwrap_or(120.);
        let (file, start) = &run.span_file;
        let offset = start + (window.from - run.span.from) * 60. / tempo;
        let heard = measure_file(
            &file.to_string_lossy(),
            MeasureOptions { start: Some(offset), seconds: Some(window.beats * 60. / tempo), signal: Some(signal) },
        )
        .await
        .map_err(|error| RuntimeError::plain(error.to_string()))?;
        // The focus element isn't kept from the span's listen: masking is judged on the whole, not excerpts.
        let values = run.checklist.read(&heard, None);
        let values = values
            .into_iter()
            .zip(&run.checklist.items)
            .zip(&run.whole)
            .map(|((value, item), whole)| if matches!(item.quantity, Quantity::Problem { focus: Some(_), .. }) { *whole } else { value })
            .collect();
        Ok(Excerpt { window, state: run.state, values, file: file.clone(), start: offset, loudness: heard.measures.integrated })
    }

    fn describe(&self, window: Window, run: &JudgeRun) -> String {
        let meter = self.observer.beats_per_bar.get().max(1.);
        let bar = |beat: f64| (beat / meter).floor() as i64 + 1;
        let what = run.track.clone().unwrap_or_else(|| "the mix".into());
        if window == run.span {
            format!("{what}, bars {}–{}", bar(window.from), bar(window.from + window.beats - 1e-6))
        } else {
            format!("{what}, bars {}–{}", bar(window.from), bar(window.from + window.beats - 1e-6))
        }
    }

    fn next_step(&self, run: &JudgeRun) -> Option<Next> {
        run.target.map(|index| next_of(run, index))
    }

    /// Kumi's changes in HISTORY now (applied ones), to tell a round's own from what came before.
    fn applied_ids(&self) -> Vec<String> {
        self.history
            .entries
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.borrow().record.state == ChangeState::Applied)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The applied changes since a checkpoint, oldest first: their ids and titles.
    fn applied_since(&self, checkpoint: &[String]) -> Vec<(String, String)> {
        self.history
            .entries
            .borrow()
            .iter()
            .filter(|(id, entry)| !checkpoint.contains(id) && entry.borrow().record.state == ChangeState::Applied)
            .map(|(id, entry)| (id.clone(), entry.borrow().record.title.clone()))
            .collect()
    }

    /// Turns the level by `gain` dB where rebalancing goes: the gain of the last Limiter on Main (or the run's track),
    /// else the last Utility's, else a Utility Kumi puts at the end. Says what it turned, and whether a limiter holds
    /// the peaks after it.
    async fn rebalance(self: &Rc<Self>, gain: f64, signal: Signal) -> Result<(String, bool), String> {
        let known = self.judge.borrow().as_ref().and_then(|run| run.gain.clone());
        let stage = match known {
            Some(stage) => stage,
            None => {
                let stage = self.gain_stage(signal.clone()).await.map_err(|error| head(&error.to_string(), 200))?;
                if let Some(run) = self.judge.borrow_mut().as_mut() {
                    run.gain = Some(stage.clone());
                }
                stage
            }
        };
        let read = self
            .rows("parameter", json!({"parent":stage.device,"fields":["name","displayValue"]}), signal.clone())
            .await
            .map_err(|error| head(&error.to_string(), 200))?;
        let shown = read
            .iter()
            .find(|row| row.get("ref").and_then(Value::as_str) == Some(stage.parameter.as_str()))
            .and_then(|row| row.get("displayValue").and_then(Value::as_str).map(str::to_owned))
            .ok_or("Kumi couldn't read the gain knob")?;
        let now = parse_display(&shown)
            .filter(|reading| reading.unit == "db")
            .map(|reading| reading.value)
            .ok_or("the gain knob doesn't show dB")?;
        let wanted = if now.is_finite() { now + gain } else { gain };
        self.step(
            "set_device_parameter",
            json!({"deviceRef":stage.device,"parameterRef":stage.parameter,"value":format!("{} dB", to_string((wanted * 10.).round() / 10.))}),
            signal,
        )
        .await
        .map_err(|error| head(&error.to_string(), 200))?;
        Ok((stage.label, stage.limited))
    }

    async fn gain_stage(self: &Rc<Self>, signal: Signal) -> Result<GainStage, RuntimeError> {
        let scope = self.judge.borrow().as_ref().and_then(|run| run.track.clone());
        let track = match scope {
            Some(track) => {
                let tracks = self.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
                tracks
                    .iter()
                    .find(|row| {
                        row.get("ref").and_then(Value::as_str) == Some(track.as_str())
                            || row.get("name").and_then(Value::as_str) == Some(track.as_str())
                    })
                    .and_then(|row| row.get("ref").and_then(Value::as_str).map(str::to_owned))
                    .ok_or_else(|| observation(format!("{track} isn't a track in this Set now.")))?
            }
            None => self.main_volume(signal.clone()).await?.0,
        };
        let devices = |signal: Signal| {
            let track = track.clone();
            async move { self.rows("device", json!({"parent":track,"fields":["name","className"]}), signal).await }
        };
        let named = |row: &JsonObject, name: &str| {
            row.get("className").and_then(Value::as_str) == Some(name) || row.get("name").and_then(Value::as_str) == Some(name)
        };
        let mut rows = devices(signal.clone()).await?;
        let limiter = rows.iter().rposition(|row| named(row, "Limiter"));
        let (device, limited) = match limiter {
            Some(at) => (rows[at].clone(), true),
            None => match rows.last().filter(|row| named(row, "Utility") || named(row, "StereoGain")) {
                Some(row) => (row.clone(), false),
                None => {
                    self.step("load_device", json!({"itemId":"audio_effects/Utility","trackRef":track}), signal.clone()).await?;
                    rows = devices(signal.clone()).await?;
                    (rows.last().cloned().ok_or_else(|| observation("The Utility Kumi added didn't appear."))?, false)
                }
            },
        };
        let device_ref =
            device.get("ref").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| observation("A device without a ref."))?;
        let parameters = self.rows("parameter", json!({"parent":device_ref,"fields":["name"]}), signal).await?;
        let parameter = parameters
            .iter()
            .find(|row| matches!(row.get("name").and_then(Value::as_str), Some("Gain" | "Input Gain")))
            .and_then(|row| row.get("ref").and_then(Value::as_str).map(str::to_owned))
            .ok_or_else(|| observation("Kumi couldn't find the gain knob."))?;
        let name = device.get("name").and_then(Value::as_str).unwrap_or("Utility").to_string();
        Ok(GainStage { device: device_ref, parameter, label: format!("{name} gain"), limited })
    }
}

/// What the judge heard: the mix (or the run's track), the focus element, and the main file kept for later cuts.
struct JudgeHeard {
    main: Heard,
    focus: Option<Heard>,
    file: PathBuf,
    start: f64,
}

fn next_of(run: &JudgeRun, index: usize) -> Next {
    let item = &run.checklist.items[index];
    Next {
        id: item.id.clone(),
        label: item.label.clone(),
        gap: round1(item.gap(run.whole[index])),
        wanted: item.wanted(),
        now: run.whole[index].map(round1),
        fix: item.fix.clone(),
    }
}

fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}
