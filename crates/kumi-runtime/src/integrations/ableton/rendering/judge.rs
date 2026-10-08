//! The judge in Live: a goal as a checklist, heard quietly. The run starts with the whole stretch heard once (the
//! problems found there, placed in time), then judges each change on an excerpt (the bars around the target's worst
//! moment, else the loudest part), keeps it only if its target improved and nothing else got audibly worse, takes
//! it back with Kumi's undo otherwise, brings loudness back to target after every kept change, and logs every
//! round. The whole stretch is heard again only at the end.

use super::super::{connection::NO_CURRENT_LIVE, display::parse_display};
use super::rig::Window;
use super::*;
use crate::listening::{
    checklist::{note_stretch, worst_stretch, Checklist, Explicit, Goal, Profile, Quantity, Row, REGIONS},
    detect::{self, Problem, ProblemKind},
    embed,
    judging::{self, Listen, Placed, RoundHost, Unheard},
    listener::{compare, Listener, Opinion, Take},
    measure::{measure_file, percentile, Embedding, Heard, MeasureOptions},
    round::{Next, Round, RoundKind},
};
use crate::references::{store::KeptReference, tool::MEASURED};
use async_trait::async_trait;
use kumi_common::js::{number::to_string, string::head};
use std::path::Path;

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
    /// A single sound against a reference sound (its envelope and tone on the checklist too).
    pub sound: bool,
}

/// One part of the run's stretch heard as things stand: its checklist values and its sound.
#[derive(Debug, Clone)]
pub(super) struct Excerpt {
    pub window: Window,
    pub state: u32,
    pub values: Vec<Option<f64>>,
    pub file: PathBuf,
    pub start: f64,
    /// Its integrated loudness, to play it level with another take.
    pub loudness: Option<f64>,
}

/// Where rebalancing turns the level: a gain knob, and whether a limiter comes after it (then peaks stay put).
#[derive(Debug, Clone)]
struct GainStage {
    device: String,
    parameter: String,
    label: String,
    limited: bool,
}

/// The first round's word when no listening model is found (and none was turned off: that says why instead).
const NO_LISTENER: &str =
    "No listening model: judging by Kumi's meters alone (with a Gemini or OpenAI API key one listens too; /slots shows the choices)";

/// A judged run: its checklist, what the whole stretch measures as things stand, and the rounds so far.
pub struct JudgeRun {
    pub(super) checklist: Checklist,
    pub(super) track: Option<String>,
    pub(super) focus: Option<String>,
    pub(super) span: Window,
    /// The span's own capture (its file and where the part starts), to cut excerpts from before anything changes.
    pub(super) span_file: (PathBuf, f64),
    pub(super) span_focus: Option<(PathBuf, f64)>,
    pub(super) loudness: Vec<Option<f64>>,
    pub(super) problems: Vec<Problem>,
    pub(super) first: Vec<Option<f64>>,
    pub(super) whole: Vec<Option<f64>>,
    pub(super) excerpts: Vec<Excerpt>,
    pub(super) state: u32,
    pub(super) target: Option<usize>,
    pub(super) window: Window,
    pub(super) checkpoint: Vec<String>,
    pub(super) round: u32,
    pub(super) listens: u32,
    pub(super) started: i64,
    /// Why the run is over, once it is (it ended with done): no change is judged against it after.
    pub(super) ended: Option<String>,
    /// Live changed under it (other requests in between): its numbers are out of date until it hears its bars again.
    pub(super) stale: bool,
    /// Per checklist item, rounds in a row that went after it and were taken back.
    pub(super) misses: Vec<u32>,
    /// Per checklist item, where in the span (seconds from its start) a problem stands out most: its excerpt.
    pub(super) worst: Vec<Option<f64>>,
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
            self.judge_round(request.change.clone(), None, signal.clone()).await
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
        // The style model this run hears with, fixed now: the reference's style and every listen's are its.
        *self.style_model.borrow_mut() = None;
        if models_on() {
            let slot = crate::slots::kept().model_file(crate::slots::Job::Embeddings);
            let style = embed::style_id(slot.as_deref()).await;
            *self.style_model.borrow_mut() = Some((slot, style));
        }
        let (reference, unguarded) = match &goal.reference {
            Some(named) => match self.reference_profile(named, goal.sound, signal.clone()).await {
                Ok((profile, unguarded)) => (Some(profile), unguarded),
                Err(why) => return Ok(Err(why)),
            },
            None => (None, None),
        };
        // The learned models hear this run's listens when the reference was heard by them: its style always, a sound's
        // effects too.
        self.embedding_wanted.set(match &reference {
            Some(profile) if models_on() => (profile.vibe.is_some(), profile.effects.is_some() && goal.sound),
            _ => (false, false),
        });
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
        // Tracks by name, so the log says "Vocal Main", not a ref.
        let mut goal = goal.clone();
        if let Some(named) = goal.focus.clone() {
            goal.focus = Some(self.track_name(&named, signal.clone()).await.unwrap_or(named));
        }
        let track = match &request.track {
            Some(named) => Some(self.track_name(named, signal.clone()).await.unwrap_or_else(|| named.clone())),
            None => None,
        };
        let goal = &goal;
        let heard = match self.judge_hear(track.as_deref(), goal.focus.as_deref(), span, signal.clone()).await? {
            Ok(JudgeHeard { silent: Some(why), .. }) | Err(why) => return Ok(Err(why)),
            Ok(heard) => heard,
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
            sound: goal.sound,
        };
        let mut checklist = Checklist::new(&goal, &heard.main, &problems);
        let mut whole = checklist.read(&heard.main, heard.focus.as_ref());
        let unreadable = checklist.drop_unreadable(&mut whole);
        if !checklist.has_targets() {
            return Ok(Err(format!(
                "Nothing to work toward: {}. Give a target (a loudness, a true-peak ceiling, a reference or numbers), or tell the producer it already sounds right.",
                if unreadable.is_empty() {
                    "the goal names no target, and nothing stands out in what Kumi heard".to_string()
                } else {
                    format!("Kumi can't read {} in what it heard", unreadable.join(", ").to_lowercase())
                }
            )));
        }
        let mut run = JudgeRun {
            checklist,
            track,
            focus: goal.focus.clone(),
            span,
            span_file: (heard.file.clone(), heard.start),
            span_focus: heard.focus_file.clone(),
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
            ended: None,
            stale: false,
            misses: vec![],
            worst: vec![],
            rounds: vec![],
        };
        run.misses = vec![0; run.checklist.items.len()];
        // A problem's excerpt is where it stands out most (a steady one, too: the loudest bars may bury it).
        let length = self.excerpt_beats() * 60. / tempo;
        let bar = self.observer.beats_per_bar.get().max(1.) * 60. / tempo;
        run.worst = run
            .checklist
            .items
            .iter()
            .map(|item| match item.quantity {
                Quantity::Problem { problem: ProblemKind::Resonance | ProblemKind::Harshness, low, high, steady, .. } => {
                    worst_stretch(&heard.main, low, high, steady, length, bar)
                }
                Quantity::Problem { problem: ProblemKind::LoudNote, low, high, .. } => note_stretch(&heard.main, low, high, length, bar),
                _ => None,
            })
            .collect();
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
        // Said once a run: whether a listening model will hear the changes beside the meters, and which one, since what
        // it hears leaves the computer; or why none will.
        let alone = match self.listener(signal.clone()).await {
            (Some(listener), _) if listener.off() => {
                Some(format!("{}: judging by Kumi's meters alone, and nothing is sent", listener.off_why()))
            }
            (Some(listener), _) => Some(format!(
                "{} will hear 10 s of each change, before and after, beside the meters; /slots listening off stops it",
                listener.name()
            )),
            // A key is there but the model couldn't be reached: that, not the key, is what to say.
            (None, Some((why, _))) => {
                Some(format!("The listening model couldn't be reached ({why}): judging by Kumi's meters alone for now"))
            }
            (None, None) => Some(NO_LISTENER.to_string()),
        };
        let round = Round {
            round: 0,
            kind: RoundKind::Start,
            heard: self.describe(span, &run),
            target: None,
            change: None,
            changes: vec![],
            rows,
            kept: None,
            why: {
                let unread = (!unreadable.is_empty()).then(|| {
                    format!("Kumi can't read {} in what it heard, so it's left off the checklist", unreadable.join(", ").to_lowercase())
                });
                match (unread, unguarded) {
                    (Some(unread), Some(unguarded)) => Some(format!("{unread}. {unguarded}")),
                    (unread, unguarded) => unread.or(unguarded),
                }
            },
            rebalanced: None,
            listener: alone,
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

    /// A change judged on the target's excerpt: heard now, or already heard (`pre`, the last probe of a tune). Peaks and
    /// clipping are judged at the loudness the change ends at: shifted by the gain rebalancing will add, and once it
    /// has, the round is judged again on what's heard then.
    pub(super) async fn judge_round(
        self: &Rc<Self>,
        change: Option<String>,
        pre: Option<JudgeHeard>,
        signal: Signal,
    ) -> Result<Result<Round, String>, RuntimeError> {
        let (track, focus, window, target, state, checkpoint) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            if let Some(why) = &run.ended {
                return Ok(Err(format!("That run is over ({why}): start a new one with a goal.")));
            }
            (run.track.clone(), run.focus.clone(), run.window, run.target, run.state, run.checkpoint.clone())
        };
        if self.judge.borrow().as_ref().unwrap().stale {
            return Ok(self.rebaseline(signal).await?.map(|(round, _)| round));
        }
        let before = {
            let run = self.judge.borrow();
            run.as_ref().unwrap().excerpts.iter().find(|excerpt| excerpt.window == window && excerpt.state == state).cloned()
        };
        let Some(before) = before else {
            return Ok(Err("Kumi lost what this excerpt sounded like before the change; start the run again with a goal.".into()));
        };
        let changes = self.applied_since(&checkpoint);
        let heard = match pre {
            Some(heard) => heard,
            None => match self.judge_hear(track.as_deref(), focus.as_deref(), window, signal.clone()).await? {
                Ok(heard) => {
                    self.judge.borrow_mut().as_mut().unwrap().listens += 1;
                    heard
                }
                Err(why) => return Ok(Err(why)),
            },
        };
        let (after, target_label, aim) = {
            let mut guard = self.judge.borrow_mut();
            let run = guard.as_mut().unwrap();
            run.round += 1;
            // Silence after the change reads nothing: every reading lost, so it's taken back.
            let after = heard.read(&run.checklist);
            let aim = match target.map(|index| &run.checklist.items[index]) {
                Some(item) => format!("{} toward {}, with nothing else getting worse", item.label, item.wanted()),
                None => "a better-sounding mix with nothing getting worse".into(),
            };
            (after, target.map(|index| run.checklist.items[index].label.clone()), aim)
        };
        // The listening model hears the change too: advice, unless it hears an obvious artifact both ways. One that hears
        // a mono downmix at a low rate isn't asked about width or the top octave.
        let width_or_air = target.is_some_and(|index| {
            let run = self.judge.borrow();
            match run.as_ref().unwrap().checklist.items[index].quantity {
                Quantity::LowWidth => true,
                Quantity::Region { region } => crate::listening::measure::THIRDS[REGIONS[region].2] >= 8_000.,
                _ => false,
            }
        });
        // Silence is taken back anyway: no one is asked about it.
        let heard_by = match heard.silent {
            Some(_) => None,
            None => self.listen_to_change(&before, &heard, &aim, width_or_air, signal.clone()).await,
        };
        let listener = heard_by.as_ref().map(|(line, _)| line.clone());
        let excerpt = Listen { values: after, loudness: heard.main.measures.integrated, file: heard.file.clone(), start: heard.start };
        let host = InLive { rendering: self, track: track.clone(), focus: focus.clone(), window, signal: signal.clone() };
        let (checklist, whole) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            (run.checklist.clone(), run.whole.clone())
        };
        let made = self.created_by(&changes);
        let decided = judging::decide(
            &host,
            &checklist,
            target,
            &whole,
            &before.values,
            excerpt,
            heard_by.as_ref().and_then(|(_, opinion)| opinion.as_ref()),
            made,
            &checkpoint,
        )
        .await;
        self.judge.borrow_mut().as_mut().unwrap().listens += decided.listens;
        let judging::Decided { verdict, rebalanced, whole: whole_now, excerpt, stopped, stayed, .. } = decided;
        if verdict.kept {
            let file = self.keep_file(&excerpt.file).await;
            let mut guard = self.judge.borrow_mut();
            let run = guard.as_mut().unwrap();
            run.whole = whole_now;
            run.state += 1;
            let state = run.state;
            run.excerpts.retain(|excerpt| excerpt.state == state);
            run.excerpts.push(Excerpt { window, state, values: excerpt.values, file, start: excerpt.start, loudness: excerpt.loudness });
        }
        let ids = self.applied_ids();
        let next_window = {
            let mut guard = self.judge.borrow_mut();
            let run = guard.as_mut().unwrap();
            run.checkpoint = ids;
            // What Live wouldn't take back is still in the Set: the next judge hears the bars again first.
            if stayed {
                run.stale = true;
            }
            // A target two changes in a row failed on waits while another gap is open.
            if let Some(index) = target {
                run.misses[index] = if verdict.kept { 0 } else { run.misses[index] + 1 };
            }
            let skip: Vec<usize> = run.misses.iter().enumerate().filter(|(_, misses)| **misses >= 2).map(|(index, _)| index).collect();
            run.target = run.checklist.next_skipping(&run.whole, &skip);
            self.excerpt_for(run)
        };
        // The round is kept before the next excerpt is heard: a listen that fails then doesn't lose it.
        let mut round = {
            let mut guard = self.judge.borrow_mut();
            let run = guard.as_mut().unwrap();
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
            round
        };
        if stopped {
            self.tell_judged(&round);
            signal.check()?;
            return Err(RuntimeError::Aborted);
        }
        // The next change is judged on its own excerpt; its "before" is heard now if this state hasn't been. Without
        // it, the next change is judged on these bars again (their "before" is known).
        let missed = match self.ensure_before(next_window, signal.clone()).await {
            Ok(Ok(())) => None,
            Ok(Err(why)) => Some(why),
            Err(error) if signal.check().is_err() => {
                self.tell_judged(&round);
                return Err(error);
            }
            Err(error) => Some(head(&error.to_string(), 200)),
        };
        let mut guard = self.judge.borrow_mut();
        let run = guard.as_mut().unwrap();
        match missed {
            None => run.window = next_window,
            Some(why) if next_window != window => {
                let note = format!(
                    "; Kumi couldn't hear the next excerpt ({}), so the next change is judged on these bars again",
                    head(&why, 160)
                );
                round.why = round.why.map(|text| text + &note);
                if let Some(last) = run.rounds.last_mut() {
                    *last = round.clone();
                }
            }
            Some(_) => {}
        }
        Ok(Ok(round))
    }

    /// A new baseline when Live changed under the run (other requests in between): its bars heard as they are now, the
    /// whole stretch moved by what they moved, and HISTORY's changes so far taken as the run's starting point. This
    /// answer's own changes are what it means to judge, so they're taken back first (the bars are heard as Live was
    /// before them) and asked for again. Nothing is judged; the round says so, and what's next. With it, whether
    /// anything of this answer's was taken back.
    pub(super) async fn rebaseline(self: &Rc<Self>, signal: Signal) -> Result<Result<(Round, bool), String>, RuntimeError> {
        let (track, focus, window, checkpoint) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            (run.track.clone(), run.focus.clone(), run.window, run.checkpoint.clone())
        };
        let audible = |id: &String| self.history.entries.borrow().get(id).is_some_and(|entry| entry.borrow().audible());
        let taken = if self.applied_since(&checkpoint).iter().any(|(id, _)| audible(id)) {
            let host = InLive { rendering: self, track: track.clone(), focus: focus.clone(), window, signal: signal.clone() };
            Some(judging::take_back(&host, &checkpoint).await)
        } else {
            None
        };
        let heard = match self.judge_hear(track.as_deref(), focus.as_deref(), window, signal.clone()).await? {
            Ok(JudgeHeard { silent: Some(why), .. }) | Err(why) => return Ok(Err(why)),
            Ok(heard) => heard,
        };
        let file = self.keep_file(&heard.file).await;
        let ids = self.applied_ids();
        let round = {
            let mut guard = self.judge.borrow_mut();
            let run = guard.as_mut().unwrap();
            run.listens += 1;
            let values = heard.read(&run.checklist);
            let before = run
                .excerpts
                .iter()
                .find(|excerpt| excerpt.window == window && excerpt.state == run.state)
                .map(|excerpt| excerpt.values.clone());
            let was = run.whole.clone();
            run.whole = match before {
                Some(before) => judging::predict(&run.checklist, &run.whole, &before, &values),
                None if window == run.span => values.clone(),
                None => run.whole.clone(),
            };
            run.state += 1;
            let state = run.state;
            run.excerpts = vec![Excerpt { window, state, values, file, start: heard.start, loudness: heard.main.measures.integrated }];
            run.checkpoint = ids;
            run.stale = false;
            let skip: Vec<usize> = run.misses.iter().enumerate().filter(|(_, misses)| **misses >= 2).map(|(index, _)| index).collect();
            run.target = run.checklist.next_skipping(&run.whole, &skip);
            run.window = self.excerpt_for(run);
            let verdict = run.checklist.verdict(None, &was, &run.whole);
            let round = Round {
                round: run.round,
                kind: RoundKind::Start,
                heard: self.describe(window, run),
                target: None,
                change: None,
                changes: vec![],
                rows: verdict.rows,
                kept: None,
                why: Some(match &taken {
                    None => "Live changed since the run's last listen (other requests in between), so Kumi heard its bars again and starts from here: a change already made is in this baseline, not judged".into(),
                    Some(taken) => format!(
                        "Live changed since the run's last listen (other requests in between), so Kumi heard its bars again and starts from here. This answer's changes went first, to hear the bars without them ({}){}: make them again, then judge them",
                        taken.said,
                        if taken.stayed { "; what Live kept is in this baseline" } else { "" }
                    ),
                }),
                rebalanced: None,
                listener: None,
                problems: vec![],
                next: self.next_step(run),
                met: run.target.is_none(),
                listens: run.listens,
                elapsed_ms: now_ms() - run.started,
            };
            run.rounds.push(round.clone());
            round
        };
        // The next change is judged on its target's bars when their "before" can be heard now, else on these.
        let next = self.judge.borrow().as_ref().unwrap().window;
        if self.ensure_before(next, signal).await?.is_err() {
            self.judge.borrow_mut().as_mut().unwrap().window = window;
        }
        Ok(Ok((round, taken.is_some())))
    }

    /// The excerpt `window` as things stand, heard now unless it already has been in this state.
    pub(super) async fn ensure_before(self: &Rc<Self>, window: Window, signal: Signal) -> Result<Result<(), String>, RuntimeError> {
        let (known, track, focus) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            (
                run.excerpts.iter().any(|excerpt| excerpt.window == window && excerpt.state == run.state),
                run.track.clone(),
                run.focus.clone(),
            )
        };
        if known {
            return Ok(Ok(()));
        }
        let heard = match self.judge_hear(track.as_deref(), focus.as_deref(), window, signal).await? {
            Ok(JudgeHeard { silent: Some(why), .. }) | Err(why) => return Ok(Err(why)),
            Ok(heard) => heard,
        };
        let file = self.keep_file(&heard.file).await;
        let mut guard = self.judge.borrow_mut();
        let run = guard.as_mut().unwrap();
        run.listens += 1;
        let values = run.checklist.read(&heard.main, heard.focus.as_ref());
        let state = run.state;
        run.excerpts.push(Excerpt { window, state, values, file, start: heard.start, loudness: heard.main.measures.integrated });
        Ok(Ok(()))
    }

    /// Tells the app a round was judged.
    pub(super) fn tell_judged(&self, round: &Round) {
        if let Some(tell) = &self.on_judge {
            let _ = catch_unwind(AssertUnwindSafe(|| tell(round.clone())));
        }
    }

    /// The listening model: one found is kept; a definite none (no provider has one) is believed for ten minutes, to
    /// notice a key added since; a lookup that failed (the network, the service) for a minute, so a round doesn't wait
    /// on it each time. With why it failed, and whether that's news (to say it once a round).
    async fn listener(&self, signal: Signal) -> (Option<Rc<dyn Listener>>, Option<(String, bool)>) {
        if let Some((known, at)) = self.listener.borrow().clone() {
            match known {
                Ok(Some(found)) => return (Some(found), None),
                Ok(None) if now_ms() - at < 600_000 => return (None, None),
                Err(why) if now_ms() - at < 60_000 => return (None, Some((why, false))),
                _ => {}
            }
        }
        let found = match &self.listener_source {
            Some(source) => source(signal.clone()).await,
            None => Ok(None),
        };
        // Esc isn't the listener failing: nothing is kept.
        if signal.aborted() {
            return (None, None);
        }
        let was_failing = self.listener.borrow().as_ref().is_some_and(|(known, _)| known.is_err());
        *self.listener.borrow_mut() = Some((found.clone(), now_ms()));
        match found {
            Ok(found) => (found, None),
            Err(why) => (None, Some((why, !was_failing))),
        }
    }

    /// What the listening model makes of the change: the excerpt before and after, level-matched, both ways.
    async fn listen_to_change(
        &self,
        before: &Excerpt,
        after: &JudgeHeard,
        aim: &str,
        width_or_air: bool,
        signal: Signal,
    ) -> Option<(String, Option<Opinion>)> {
        let listener = match self.listener(signal.clone()).await {
            (Some(listener), _) => listener,
            (None, Some((why, true))) => {
                return Some((format!("the listening model couldn't be reached ({why}); judging by the meters alone for now"), None))
            }
            (None, _) => return None,
        };
        // Turned off since it was found (the listening slot): not asked, as with no listening model.
        if listener.off() {
            return None;
        }
        if !listener.hears_width() && width_or_air {
            return None;
        }
        let tempo = self.observer.tempo.get().unwrap_or(120.);
        let seconds = before.window.beats * 60. / tempo;
        // Ten seconds from the middle of the excerpt.
        let skip = ((seconds - 10.) / 2.).max(0.);
        // Level-matched by turning the louder take down: raising the quieter one could clip it, which the model would
        // hear as the change's fault.
        let (first_gain, second_gain) = judging::matched(before.loudness, after.main.measures.integrated);
        let first = Take { file: before.file.clone(), start: before.start + skip, seconds: seconds.min(10.), gain: first_gain };
        let second = Take { file: after.file.clone(), start: after.start + skip, seconds: seconds.min(10.), gain: second_gain };
        match compare(listener.as_ref(), &first, &second, aim, signal).await {
            Ok(opinion) => Some((opinion.line(&listener.name()), Some(opinion))),
            Err(why) => Some((format!("{} couldn't be asked ({why})", listener.name()), None)),
        }
    }

    async fn judge_done(self: &Rc<Self>, signal: Signal) -> Result<Result<Round, String>, RuntimeError> {
        let (track, focus, span) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            (run.track.clone(), run.focus.clone(), run.span)
        };
        let heard = match self.judge_hear(track.as_deref(), focus.as_deref(), span, signal).await? {
            Ok(JudgeHeard { silent: Some(why), .. }) | Err(why) => return Ok(Err(why)),
            Ok(heard) => heard,
        };
        let ids = self.applied_ids();
        let mut guard = self.judge.borrow_mut();
        let run = guard.as_mut().unwrap();
        run.listens += 1;
        // Over: what it kept stays, and nothing after is a round's to take back.
        run.ended = Some("it ended with done".into());
        run.checkpoint = ids;
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

    /// A reference's profile: one measured with the reference tool (by what was asked for or its name), else from a
    /// file or a clip in the Set. Its effects are heard by the effects model only for a sound's goal, the one run that
    /// guards them. Beside it, what the run won't guard because the models didn't hear the reference, and why.
    async fn reference_profile(&self, named: &str, sound: bool, signal: Signal) -> Result<(Profile, Option<String>), String> {
        if let Some(mut kept) = match &self.references {
            Some(store) => store.load(named).await,
            None => None,
        } {
            // Heard by another style model than this run's (or kept before models were recorded): its style is heard
            // again from its files when they're all still here, else this run doesn't guard it.
            let style = self.style_model.borrow().as_ref().map(|(_, style)| style.clone());
            if style.is_some() && kept.profile.vibe.is_some() && kept.profile.vibe_model != style {
                match self.restyled(&kept, &signal).await {
                    Some((vibe, spread, model)) => {
                        (kept.profile.vibe, kept.profile.vibe_spread, kept.profile.vibe_model) = (Some(vibe), spread, Some(model));
                        if let Some(store) = &self.references {
                            let _ = store.save(&kept).await;
                        }
                    }
                    None => {
                        signal.check().map_err(|error| error.to_string())?;
                        (kept.profile.vibe, kept.profile.vibe_spread, kept.profile.vibe_model) = (None, None, None);
                        let why = format!(
                            "{} was heard by another style model than this run's, so this run doesn't guard its style; the reference tool hears it again with this one when asked for it again",
                            kept.name
                        );
                        return Ok((kept.profile, Some(why)));
                    }
                }
            }
            let unguarded = (models_on() && kept.profile.vibe.is_none()).then(|| {
                format!(
                    "{} was kept without how it sounds to the style model, so this run doesn't guard its style; the reference tool adds it when asked for it again",
                    kept.name
                )
            });
            return Ok((kept.profile, unguarded));
        }
        let file = match self.reference_file(named, signal.clone()).await {
            Ok(Ok(file)) => file,
            Ok(Err(why)) => return Err(format!("The reference: {why}")),
            Err(error) => return Err(error.to_string()),
        };
        let heard = measure_file(&file, MeasureOptions { signal: Some(signal.clone()), ..Default::default() })
            .await
            .map_err(|error| format!("Kumi couldn't hear the reference: {}", head(&error.to_string(), 200)))?;
        let name = file.rsplit(['/', '\\']).next().unwrap_or(&file).to_string();
        let mut profile = Profile::of(&name, &heard);
        // How it sounds to the learned models, when they're on: what a run guards against drifting from.
        let mut unheard = vec![];
        if models_on() {
            let say = |said: &str| self.tell(said.to_string(), None);
            let seconds = heard.measures.seconds;
            let (slot, style) = self.style_model.borrow().clone().unzip();
            match embed::vibe(Path::new(&file), 0., seconds, heard.measures.integrated, slot.flatten(), &say, &signal).await {
                Ok((vibe, model)) if Some(&model) == style.as_ref() => (profile.vibe, profile.vibe_model) = (Some(vibe), Some(model)),
                Ok(_) => unheard.push("its style (the style model changed while it was heard)".into()),
                Err(why) => unheard.push(format!("its style ({why})")),
            }
            if sound {
                match embed::effects(Path::new(&file), 0., seconds, &say, &signal).await {
                    Ok(effects) => profile.effects = Some(effects),
                    Err(why) => unheard.push(format!("its effects ({why})")),
                }
            }
        }
        signal.check().map_err(|error| error.to_string())?;
        let unguarded = (!unheard.is_empty())
            .then(|| format!("The models couldn't hear the reference's {}, so this run doesn't guard them", unheard.join(" or ")));
        Ok((profile, unguarded))
    }

    /// A kept reference's style heard again with this run's style model, from its files when they're all still here (a
    /// file or a folder kept): each heard as the reference tool hears it, at a common loudness, the vibes averaged, with
    /// how far they lie from that. None when it wasn't kept from files, or one is gone or can't be heard.
    async fn restyled(&self, kept: &KeptReference, signal: &Signal) -> Option<(Vec<f32>, Option<f64>, String)> {
        let (slot, style) = self.style_model.borrow().clone()?;
        if kept.kind != "files" || kept.tracks.is_empty() || kept.tracks.iter().any(|track| !Path::new(&track.source).is_file()) {
            return None;
        }
        let say = |said: &str| self.tell(said.to_string(), None);
        let mut vibes = vec![];
        for track in &kept.tracks {
            let options = MeasureOptions { seconds: Some(MEASURED), signal: Some(signal.clone()), ..Default::default() };
            let heard = measure_file(&track.source, options).await.ok()?;
            let seconds = heard.measures.seconds.min(MEASURED);
            let file = Path::new(&track.source);
            let (vibe, model) = embed::vibe(file, 0., seconds, heard.measures.integrated, slot.clone(), &say, signal).await.ok()?;
            if model != style {
                return None;
            }
            vibes.push(vibe);
        }
        let centre = embed::averaged(&vibes)?;
        let mut far: Vec<f64> = vibes.iter().filter_map(|vibe| embed::distance(vibe, &centre)).collect();
        far.sort_by(f64::total_cmp);
        let spread = (vibes.len() > 1 && !far.is_empty()).then(|| (percentile(&far, 0.5) * 100.).round() / 100.);
        Some((centre, spread, style))
    }

    /// What the learned models make of a stretch of a capture (`loudness`, its integrated loudness, LUFS), when the run
    /// guards against drifting from its reference's style or effects; nothing when it doesn't. When a model can't be
    /// had, that's said once and the run goes on by its measures; Esc isn't a model failing, and leaves the guards on.
    async fn embedding(&self, file: &Path, start: f64, seconds: f64, loudness: Option<f64>, signal: &Signal) -> Option<Embedding> {
        let (vibe, effects) = self.embedding_wanted.get();
        if !vibe && !effects {
            return None;
        }
        let say = |said: &str| self.tell(said.to_string(), None);
        let mut embedding = Embedding::default();
        if vibe {
            let (slot, style) = self.style_model.borrow().clone().unzip();
            match embed::vibe(file, start, seconds, loudness, slot.flatten(), &say, signal).await {
                Ok((found, model)) if Some(&model) == style.as_ref() => (embedding.vibe, embedding.vibe_model) = (Some(found), Some(model)),
                // Its slot's file went partway: what heard this isn't the model the run started with.
                Ok(_) => {
                    self.tell(
                        "The style model changed during this run (the embeddings slot's file is gone), so the run stops guarding style and goes on by its measures.".to_string(),
                        None,
                    );
                    self.embedding_wanted.set((false, self.embedding_wanted.get().1));
                }
                Err(_) if signal.is_cancelled() => return None,
                Err(why) => {
                    self.tell(format!("Kumi can't hear style with its style model now ({why}); the run goes on by its measures."), None);
                    self.embedding_wanted.set((false, self.embedding_wanted.get().1));
                }
            }
        }
        if effects {
            match embed::effects(file, start, seconds, &say, signal).await {
                Ok(found) => embedding.effects = Some(found),
                Err(_) if signal.is_cancelled() => return None,
                Err(why) => {
                    self.tell(
                        format!("Kumi can't hear effects with its effects model now ({why}); the run goes on by its measures."),
                        None,
                    );
                    self.embedding_wanted.set((self.embedding_wanted.get().0, false));
                }
            }
        }
        (embedding.vibe.is_some() || embedding.effects.is_some()).then_some(embedding)
    }

    /// Hears `window` quietly: the mix (or the run's track) and the focus element in one pass, measured.
    pub(super) async fn judge_hear(
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
        let mut heard_main = measure(main.file.clone(), main.start).await?;
        // Silence isn't a mix within tolerance: it reads nothing, and what that means is the caller's to say.
        let silent = heard_main.measures.integrated.is_none_or(|loudness| loudness < -70.).then(|| {
            format!(
                "Kumi heard only silence from {}{}. Is Live's audio running, and is something playing there in the Arrangement?",
                track.unwrap_or("the mix"),
                if notes.is_empty() { String::new() } else { format!(" ({})", notes.join(" ")) }
            )
        });
        let (heard_focus, focus_file) = match focus_name.clone().and_then(|name| files.get(&name).cloned()) {
            Some(render) => {
                // Heard before its fader: as the mix hears it, at the fader's level (so turning it up reads as up).
                let fader = self.fader_of(focus_name.as_deref().unwrap_or(""), signal.clone()).await.unwrap_or(0.);
                let heard = measure(render.file.clone(), render.start).await?.gained(fader);
                (Some(heard), Some((self.keep_file(&PathBuf::from(&render.file)).await, render.start)))
            }
            None => (None, None),
        };
        if silent.is_none() {
            heard_main.embedding =
                self.embedding(Path::new(&main.file), main.start, seconds, heard_main.measures.integrated, &signal).await;
        }
        let file = self.keep_file(&PathBuf::from(&main.file)).await;
        Ok(Ok(JudgeHeard { main: heard_main, focus: heard_focus, file, start: main.start, focus_file, silent }))
    }

    /// A track's fader, dB as Live shows it (−inf reads as −120; None when Kumi can't read it).
    async fn fader_of(&self, track: &str, signal: Signal) -> Option<f64> {
        let tracks = self.rows("track", json!({"fields":["name","mixer"]}), signal).await.ok()?;
        let row = tracks.iter().find(|row| row.get("name").and_then(Value::as_str) == Some(track))?;
        let shown = row.get("mixer")?.get("volumeDisplay")?.as_str()?;
        if shown.trim_start().starts_with("-inf") {
            return Some(-120.);
        }
        parse_display(shown).filter(|reading| reading.unit == "db" && reading.value.is_finite()).map(|reading| reading.value)
    }

    /// A track's name, from its ref or its name.
    pub(super) async fn track_name(&self, named: &str, signal: Signal) -> Option<String> {
        let long = self.connection().references.borrow().lengthen(&json!({"trackRef":named}));
        let long = long["trackRef"].as_str().unwrap_or(named).to_owned();
        let tracks = self.rows("track", json!({"fields":["name"]}), signal).await.ok()?;
        tracks
            .iter()
            .find(|row| {
                let reference = row.get("ref").and_then(Value::as_str);
                reference == Some(named) || reference == Some(long.as_str()) || row.get("name").and_then(Value::as_str) == Some(named)
            })
            .and_then(|row| row.get("name").and_then(Value::as_str).map(str::to_owned))
    }

    /// Copies a capture into the judge's own folder, out of the listening folder's pruning.
    pub(super) async fn keep_file(&self, file: &std::path::Path) -> PathBuf {
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
    /// How long an excerpt is: eight bars, or twelve seconds' worth of whole bars at a fast tempo.
    fn excerpt_beats(&self) -> f64 {
        let tempo = self.observer.tempo.get().unwrap_or(120.);
        let meter = self.observer.beats_per_bar.get().max(1.);
        (8. * meter).max((12. * tempo / 60. / meter).ceil() * meter)
    }

    pub(super) fn excerpt_for(&self, run: &JudgeRun) -> Window {
        let tempo = self.observer.tempo.get().unwrap_or(120.);
        let meter = self.observer.beats_per_bar.get().max(1.);
        let length = self.excerpt_beats();
        if run.span.beats <= length * 1.5 {
            return run.span;
        }
        // A problem's own stretch (where it stands out most), as the span's beats.
        if let Some(start) = run.target.and_then(|index| run.worst.get(index).copied().flatten()) {
            let from = run.span.from + ((start * tempo / 60.) / meter).round() * meter;
            return Window { from: from.clamp(run.span.from, run.span.from + run.span.beats - length), beats: length };
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

    /// An excerpt of the span's own capture (and the focus element's), measured: what it sounded like before anything
    /// changed.
    pub(super) async fn cut_excerpt(&self, run: &JudgeRun, window: Window, signal: Signal) -> Result<Excerpt, RuntimeError> {
        let tempo = self.observer.tempo.get().unwrap_or(120.);
        let seconds = window.beats * 60. / tempo;
        let into = (window.from - run.span.from) * 60. / tempo;
        let cut = |file: PathBuf, start: f64| {
            let signal = signal.clone();
            async move {
                measure_file(
                    &file.to_string_lossy(),
                    MeasureOptions { start: Some(start + into), seconds: Some(seconds), signal: Some(signal) },
                )
                .await
                .map_err(|error| RuntimeError::plain(error.to_string()))
            }
        };
        let (file, start) = run.span_file.clone();
        let mut heard = cut(file.clone(), start).await?;
        heard.embedding = self.embedding(&file, start + into, seconds, heard.measures.integrated, &signal).await;
        let focus = match run.span_focus.clone() {
            Some((file, start)) => Some(cut(file, start).await?),
            None => None,
        };
        let values = run.checklist.read(&heard, focus.as_ref());
        Ok(Excerpt { window, state: run.state, values, file, start: start + into, loudness: heard.measures.integrated })
    }

    fn describe(&self, window: Window, run: &JudgeRun) -> String {
        let meter = self.observer.beats_per_bar.get().max(1.);
        let bar = |beat: f64| (beat / meter).floor() as i64 + 1;
        let what = run.track.clone().unwrap_or_else(|| "the mix".into());
        format!("{what}, bars {}–{}", bar(window.from), bar(window.from + window.beats - 1e-6))
    }

    pub(super) fn next_step(&self, run: &JudgeRun) -> Option<Next> {
        run.target.map(|index| next_of(run, index))
    }

    /// Kumi's changes in HISTORY now (applied ones), to tell a round's own from what came before.
    pub(super) fn applied_ids(&self) -> Vec<String> {
        self.history
            .entries
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.borrow().record.state == ChangeState::Applied)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The applied changes since a checkpoint, oldest first: their ids and titles.
    pub(super) fn applied_since(&self, checkpoint: &[String]) -> Vec<(String, String)> {
        self.history
            .entries
            .borrow()
            .iter()
            .filter(|(id, entry)| !checkpoint.contains(id) && entry.borrow().record.state == ChangeState::Applied)
            .map(|(id, entry)| (id.clone(), entry.borrow().record.title.clone()))
            .collect()
    }

    /// Turns the level by `gain` dB where rebalancing goes: the gain of the last Limiter on Main (or the run's track),
    /// else the last Utility's, else a Utility Kumi puts at the end. Found again each time (devices move between
    /// rounds). Says what it turned, and whether a limiter holds the peaks after it.
    async fn rebalance(self: &Rc<Self>, gain: f64, signal: Signal) -> Result<(String, bool), String> {
        let stage = self.gain_stage(signal.clone()).await.map_err(|error| head(&error.to_string(), 200))?;
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

    /// The run's track (Main without one), by ref.
    pub(super) async fn scope_ref(self: &Rc<Self>, track: Option<&str>, signal: Signal) -> Result<String, RuntimeError> {
        match track {
            Some(track) => {
                let tracks = self.rows("track", json!({"fields":["name"]}), signal).await?;
                tracks
                    .iter()
                    .find(|row| {
                        row.get("ref").and_then(Value::as_str) == Some(track) || row.get("name").and_then(Value::as_str) == Some(track)
                    })
                    .and_then(|row| row.get("ref").and_then(Value::as_str).map(str::to_owned))
                    .ok_or_else(|| observation(format!("{track} isn't a track in this Set now.")))
            }
            None => Ok(self.main_volume(signal).await?.0),
        }
    }

    /// The devices on the run's track (Main without one), in order.
    pub(super) async fn chain_now(self: &Rc<Self>, track: Option<&str>, signal: Signal) -> Result<Vec<Placed>, RuntimeError> {
        let parent = self.scope_ref(track, signal.clone()).await?;
        let rows = self.rows("device", json!({"parent":parent,"fields":["name","className","objectIdentity"]}), signal).await?;
        Ok(rows
            .iter()
            .zip(names(&rows))
            .map(|(row, name)| Placed {
                identity: identity(row),
                name,
                reference: row.get("ref").and_then(Value::as_str).unwrap_or("").to_string(),
            })
            .collect())
    }

    /// How many devices the changes made (loaded or duplicated), as HISTORY records them.
    fn created_by(&self, changes: &[(String, String)]) -> usize {
        let entries = self.history.entries.borrow();
        changes
            .iter()
            .filter(|(id, _)| {
                entries.get(id).is_some_and(|entry| {
                    let entry = entry.borrow();
                    entry.record.family == ChangeFamily::Device
                        && (entry.record.title.starts_with("Loaded ") || entry.record.title.starts_with("Duplicated "))
                })
            })
            .count()
    }

    /// Where rebalancing turns the level as the chain stands, without adding anything: the last Limiter, else a
    /// Utility last. None when there's neither (rebalancing then puts a Utility at the end).
    async fn find_gain_stage(self: &Rc<Self>, signal: Signal) -> Result<Option<GainStage>, RuntimeError> {
        let scope = self.judge.borrow().as_ref().and_then(|run| run.track.clone());
        let track = self.scope_ref(scope.as_deref(), signal.clone()).await?;
        let rows = self.rows("device", json!({"parent":track,"fields":["name","className"]}), signal.clone()).await?;
        let Some((device, limited)) = gain_device(&rows) else { return Ok(None) };
        self.gain_knob(&device, limited, signal).await.map(Some)
    }

    async fn gain_stage(self: &Rc<Self>, signal: Signal) -> Result<GainStage, RuntimeError> {
        if let Some(stage) = self.find_gain_stage(signal.clone()).await? {
            return Ok(stage);
        }
        let scope = self.judge.borrow().as_ref().and_then(|run| run.track.clone());
        let track = self.scope_ref(scope.as_deref(), signal.clone()).await?;
        self.step("load_device", json!({"itemId":"audio_effects/Utility","trackRef":track}), signal.clone()).await?;
        let rows = self.rows("device", json!({"parent":track,"fields":["name","className"]}), signal.clone()).await?;
        let device = rows.last().cloned().ok_or_else(|| observation("The Utility Kumi added didn't appear."))?;
        self.gain_knob(&device, false, signal).await
    }

    /// A gain stage's knob: its Gain, Input Gain or Output.
    async fn gain_knob(self: &Rc<Self>, device: &JsonObject, limited: bool, signal: Signal) -> Result<GainStage, RuntimeError> {
        let device_ref =
            device.get("ref").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| observation("A device without a ref."))?;
        let parameters = self.rows("parameter", json!({"parent":device_ref,"fields":["name"]}), signal).await?;
        let parameter = parameters
            .iter()
            .find(|row| matches!(row.get("name").and_then(Value::as_str), Some("Gain" | "Input Gain" | "Output")))
            .and_then(|row| row.get("ref").and_then(Value::as_str).map(str::to_owned))
            .ok_or_else(|| observation("Kumi couldn't find the gain knob."))?;
        let name = device.get("name").and_then(Value::as_str).unwrap_or("Utility").to_string();
        Ok(GainStage { device: device_ref, parameter, label: format!("{name} gain"), limited })
    }
}

/// The device rebalancing turns, from a chain's rows: the last Limiter (peaks then stay put), else a Utility last.
fn gain_device(rows: &[JsonObject]) -> Option<(JsonObject, bool)> {
    let named = |row: &JsonObject, name: &str| {
        row.get("className").and_then(Value::as_str) == Some(name) || row.get("name").and_then(Value::as_str) == Some(name)
    };
    if let Some(at) = rows.iter().rposition(|row| named(row, "Limiter")) {
        return Some((rows[at].clone(), true));
    }
    rows.last().filter(|row| named(row, "Utility") || named(row, "StereoGain")).map(|row| (row.clone(), false))
}

/// A device row's identity in Live (empty when the bridge doesn't say).
fn identity(row: &JsonObject) -> String {
    match row.get("objectIdentity") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// A round's moves in Live: its excerpt heard, its track's chain, Kumi's HISTORY. Taking back and removing go on after
/// Esc (what was changed is put back), with the cleanup's own time limit.
struct InLive<'a> {
    rendering: &'a Rc<Rendering>,
    track: Option<String>,
    focus: Option<String>,
    window: Window,
    signal: Signal,
}

#[async_trait(?Send)]
impl RoundHost for InLive<'_> {
    async fn turn(&self, gain: f64) -> Result<String, String> {
        self.rendering.rebalance(gain, self.signal.clone()).await.map(|(label, _)| label)
    }
    async fn hear(&self) -> Result<Listen, Unheard> {
        match self.rendering.judge_hear(self.track.as_deref(), self.focus.as_deref(), self.window, self.signal.clone()).await {
            Ok(Ok(heard)) => {
                let values = heard.read(&self.rendering.judge.borrow().as_ref().unwrap().checklist);
                Ok(Listen { values, loudness: heard.main.measures.integrated, file: heard.file, start: heard.start })
            }
            Ok(Err(why)) => Err(Unheard { why, stopped: false }),
            Err(error) => Err(Unheard { why: head(&error.to_string(), 200), stopped: self.signal.check().is_err() }),
        }
    }
    fn applied(&self) -> Vec<String> {
        self.rendering.applied_ids()
    }
    fn applied_since(&self, mark: &[String]) -> Vec<(String, String)> {
        self.rendering.applied_since(mark)
    }
    async fn undo(&self, id: &str) -> Result<(), String> {
        match self.rendering.history.undo(id, self.rendering.cleanup(), false).await {
            Ok(undone) if !undone.is_error => Ok(()),
            Ok(undone) => Err(head(&undone.text, 120)),
            Err(error) => Err(head(&error.to_string(), 120)),
        }
    }
    fn made(&self, changes: &[(String, String)]) -> Vec<String> {
        let entries = self.rendering.history.entries.borrow();
        changes.iter().filter_map(|(id, _)| entries.get(id).and_then(|entry| entry.borrow().created.clone())).collect()
    }
    fn stopped(&self) -> bool {
        self.signal.check().is_err()
    }
    async fn limited(&self) -> bool {
        matches!(self.rendering.find_gain_stage(self.signal.clone()).await, Ok(Some(stage)) if stage.limited)
    }
    async fn chain(&self) -> Result<Vec<Placed>, String> {
        self.rendering.chain_now(self.track.as_deref(), self.rendering.cleanup()).await.map_err(|error| head(&error.to_string(), 120))
    }
    async fn delete(&self, reference: &str) -> bool {
        self.rendering.step("delete_device", json!({"ref":reference}), self.rendering.cleanup()).await.is_ok()
    }
}

/// What the judge heard: the mix (or the run's track), the focus element, and the main file kept for later cuts.
pub(super) struct JudgeHeard {
    pub main: Heard,
    pub focus: Option<Heard>,
    /// The focus element's own capture and where its part starts, to cut excerpts from.
    pub focus_file: Option<(PathBuf, f64)>,
    pub file: PathBuf,
    pub start: f64,
    /// Silence came through (why that's a problem, said): nothing on the checklist can be read from it.
    pub silent: Option<String>,
}

impl JudgeHeard {
    /// What each checklist item reads: nothing at all from silence.
    pub fn read(&self, checklist: &Checklist) -> Vec<Option<f64>> {
        if self.silent.is_some() {
            return vec![None; checklist.items.len()];
        }
        checklist.read(&self.main, self.focus.as_ref())
    }
}

/// Devices by the name Live shows (its class when it has none).
fn names(rows: &[JsonObject]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .or_else(|| row.get("className").and_then(Value::as_str))
                .unwrap_or("Device")
                .to_owned()
        })
        .collect()
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

/// Whether the learned models may be used: the embeddings slot isn't off.
fn models_on() -> bool {
    crate::slots::kept().now(crate::slots::Job::Embeddings) != crate::slots::Choice::Off
}
