//! The judge: a goal as a checklist, heard as the DAW plays. The run starts with the whole stretch heard once (the
//! problems found there, placed in time), then judges each change on an excerpt (the bars around the target's worst
//! moment, else the loudest part), keeps it only if its target improved and nothing else got audibly worse, takes
//! it back with Kumi's undo otherwise, brings loudness back to target after every kept change, and logs every
//! round. The whole stretch is heard again only at the end.

use super::Listening;
use crate::{
    checklist::{masking_unfair_in, note_stretch, worst_stretch, Checklist, Goal, Profile, Quantity, Row, REGIONS},
    detect::{self, Problem, ProblemKind},
    embed,
    judging::{self, Listen, Placed, RoundHost, Unheard},
    listener::{compare, Listener, Opinion, Take},
    measure::{measure_file, percentile, Embedding, Heard, MeasureOptions},
    references::{store::KeptReference, tool::MEASURED},
    renderer::{Captured, Window},
    round::{Next, Round, RoundKind},
    tools::{GoalRequest, JudgeRequest},
};
use async_trait::async_trait;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::string::head,
    time::now_ms,
};
use kumi_types::{contracts::*, daw::DawProfile, errors::RuntimeError};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    rc::Rc,
};

/// One part of the run's stretch heard as things stand: its checklist values and its sound.
#[derive(Debug, Clone)]
pub(crate) struct Excerpt {
    pub window: Window,
    pub state: u32,
    pub values: Vec<Option<f64>>,
    pub file: PathBuf,
    pub start: f64,
    /// Its integrated loudness, to play it level with another take.
    pub loudness: Option<f64>,
}

/// The first round's word when no listening model is found (and none was turned off: that says why instead).
const NO_LISTENER: &str =
    "No listening model: judging by Kumi's meters alone (with a Gemini or OpenAI API key one listens too; /slots shows the choices)";

/// A judged run: its checklist, what the whole stretch measures as things stand, and the rounds so far.
pub struct JudgeRun {
    pub(crate) checklist: Checklist,
    /// The track and focus it hears, by name: after a rename Kumi made, the new one.
    pub(crate) track: Option<String>,
    pub(crate) focus: Option<String>,
    /// Their names and refs as the run started, and Kumi's changes then: the renames since are followed from these.
    pub(crate) named: Named,
    pub(crate) span: Window,
    /// The span's own capture (its file and where the part starts), to cut excerpts from before anything changes.
    pub(crate) span_file: (PathBuf, f64),
    pub(crate) span_focus: Option<(PathBuf, f64)>,
    /// The span's mix as it came into Main's chain, when Kumi Ears heard it there (masking reads against it).
    pub(crate) span_premix: Option<(PathBuf, f64)>,
    pub(crate) loudness: Vec<Option<f64>>,
    pub(crate) problems: Vec<Problem>,
    pub(crate) first: Vec<Option<f64>>,
    pub(crate) whole: Vec<Option<f64>>,
    pub(crate) excerpts: Vec<Excerpt>,
    pub(crate) state: u32,
    pub(crate) target: Option<usize>,
    pub(crate) window: Window,
    pub(crate) checkpoint: Vec<String>,
    /// How much had changed Live outside HISTORY at the checkpoint (a count of HISTORY's `outside`).
    pub(crate) outside: usize,
    pub(crate) round: u32,
    pub(crate) listens: u32,
    pub(crate) started: i64,
    /// Why the run is over, once it is (it ended with done): no change is judged against it after.
    pub(crate) ended: Option<String>,
    /// Live changed under it (other requests in between): its numbers are out of date until it hears its bars again.
    pub(crate) stale: bool,
    /// The producer's words came in while it ran (a steer): HISTORY's changes then. What this answer made before them
    /// is the round's own; what came after may be what the producer asked for, so it isn't a round's to take back.
    pub(crate) steered: Option<Vec<String>>,
    /// The gain (dB) the span's focus element was heard with (its fader, and Main's level-only Utilities): its excerpts
    /// are cut from that capture at the same gain.
    pub(crate) span_focus_gain: f64,
    /// Per checklist item, rounds in a row that went after it and were taken back.
    pub(crate) misses: Vec<u32>,
    /// Per checklist item, where in the span (seconds from its start) a problem stands out most: its excerpt.
    pub(crate) worst: Vec<Option<f64>>,
    pub rounds: Vec<Round>,
}

/// A judged run's track and focus as it started: each one's name and long ref then (as HISTORY keeps refs), and
/// Kumi's changes then. A rename since is followed only when it's of that ref and from the name the track had: a
/// namesake's isn't, and nor is that of a track that came to that place (refs are positions) when tracks moved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Named {
    pub track: Option<(String, Option<String>)>,
    pub focus: Option<(String, Option<String>)>,
    pub from: Vec<String>,
}

impl JudgeRun {
    /// The last round, as the app and /goal read it.
    pub fn last(&self) -> Option<&Round> {
        self.rounds.last()
    }

    /// Whether its bars are heard again before the next change is judged or tuned.
    pub(crate) fn out_of_date(&self) -> bool {
        self.stale || self.steered.is_some()
    }
}

/// Every file in `folder` but those `stored`.
pub(crate) async fn prune_unstored(folder: &std::path::Path, stored: &[PathBuf]) {
    let Ok(mut listed) = tokio::fs::read_dir(folder).await else { return };
    while let Ok(Some(entry)) = listed.next_entry().await {
        if !stored.contains(&entry.path()) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

impl Listening {
    pub async fn judge(self: &Rc<Self>, request: &JudgeRequest, original: Signal) -> Result<Result<Round, String>, RuntimeError> {
        let judged = self.judge_now(request, original).await;
        // The takes the run no longer stores go, however the call ended.
        self.prune_kept().await;
        judged
    }

    async fn judge_now(self: &Rc<Self>, request: &JudgeRequest, original: Signal) -> Result<Result<Round, String>, RuntimeError> {
        if let Some(why) = self.host.refusal() {
            return Ok(Err(why));
        }
        if self.host.tempo().filter(|tempo| *tempo > 0.).is_none() {
            return Ok(Err(self.no_tempo()));
        }
        if self.host.flag().is_on() {
            return Ok(Err("Kumi is already listening to something; wait for it.".into()));
        }
        let signal = abort::any([original, self.host.lifetime()]);
        let outcome = if let Some(goal) = &request.goal {
            self.judge_start(goal, request, signal.clone()).await
        } else if self.judge.borrow().is_none() {
            return Ok(Err("Start with a goal: judge {goal: {...}} hears the mix (or a track) and makes the checklist.".into()));
        } else if request.done {
            self.follow_renames();
            self.judge_done(signal.clone()).await
        } else {
            self.follow_renames();
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
        let tempo = self.host.tempo().unwrap();
        let started = now_ms();
        *self.judge.borrow_mut() = None;
        // The style model this run hears with, fixed now: the reference's style and every listen's are its.
        *self.style_model.borrow_mut() = None;
        if models_on() {
            let slot = crate::slots::kept().model_file(crate::slots::Job::Embeddings);
            let style = embed::style_id(slot.as_deref(), &signal).await;
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
                let Some(end) = self.host.song_end(signal.clone()).await else {
                    return Ok(Err("Kumi couldn't tell where the song ends; give from_beat and beats.".into()));
                };
                let from = from.unwrap_or(0.).max(0.);
                Window { from, beats: beats.unwrap_or(end - from).max(self.host.beats_per_bar()) }
            }
        };
        if let Some(why) = super::too_long(span.beats, tempo, "judge part of it: give from_beat and beats") {
            return Ok(Err(why));
        }
        // Tracks by name, so the log says "Vocal Main", not a ref.
        let mut goal = goal.clone();
        if let Some(named) = goal.focus.clone() {
            goal.focus = Some(self.host.track_name(&named, signal.clone()).await.unwrap_or(named));
        }
        let track = match &request.track {
            Some(named) => Some(self.host.track_name(named, signal.clone()).await.unwrap_or_else(|| named.clone())),
            None => None,
        };
        let goal = &goal;
        let heard = match self.judge_hear(track.as_deref(), goal.focus.as_deref(), span, "where the run starts", signal.clone()).await? {
            Ok(JudgeHeard { silent: Some(why), .. }) | Err(why) => return Ok(Err(why)),
            Ok(heard) => heard,
        };
        let offset = span.from * 60. / tempo;
        // Masking only where it can be read fairly: not through a Main chain that moves the mix but not the focus.
        let masking_off = goal
            .focus
            .as_deref()
            .and_then(|name| masking_unfair_in(self.host.profile(), name, &heard.main_unfair, heard.premix_missed.as_deref()));
        let read_masking = masking_off.is_none();
        let goal = Goal {
            loudness: goal.loudness,
            true_peak: goal.true_peak,
            reference,
            problems: goal.problems,
            focus: goal.focus.clone(),
            targets: goal.targets.clone(),
            sound: goal.sound,
        };
        // A problem's excerpt is where it stands out most (a steady one, too: the loudest bars may bury it).
        let length = self.excerpt_beats() * 60. / tempo;
        let bar = self.host.beats_per_bar().max(1.) * 60. / tempo;
        // What was heard is worked out off the app's thread: detectors, the checklist and where each problem stands
        // out take a while over a whole song.
        let (main, focus, premix) = (heard.main, heard.focus, heard.premix);
        let analysed = tokio::task::spawn_blocking(move || {
            let mut problems = detect::harshness(&main);
            problems.extend(detect::low_end(&main));
            problems.extend(detect::peaks(&main, goal.true_peak));
            if let (Some(focus), Some(name), true) = (&focus, &goal.focus, read_masking) {
                problems.extend(detect::masking(focus, premix.as_ref().unwrap_or(&main), name));
            }
            // Times as the song's, not the capture's.
            for problem in &mut problems {
                for span in &mut problem.at {
                    span[0] += offset;
                    span[1] += offset;
                }
            }
            let mut checklist = Checklist::new(&goal, &main, &problems);
            let mut whole = checklist.read_with(&main, focus.as_ref(), premix.as_ref());
            let unreadable = checklist.drop_unreadable(&mut whole);
            let worst: Vec<Option<f64>> = checklist
                .items
                .iter()
                .map(|item| match item.quantity {
                    Quantity::Problem { problem: ProblemKind::Resonance | ProblemKind::Harshness, low, high, steady, .. } => {
                        worst_stretch(&main, low, high, steady, length, bar)
                    }
                    Quantity::Problem { problem: ProblemKind::LoudNote, low, high, .. } => note_stretch(&main, low, high, length, bar),
                    _ => None,
                })
                .collect();
            (main, goal, problems, checklist, whole, unreadable, worst)
        })
        .await
        .map_err(|error| RuntimeError::plain(error.to_string()))?;
        let (main, goal, mut problems, mut checklist, whole, unreadable, worst) = analysed;
        // The fixes name the DAW's own devices.
        for problem in &mut problems {
            problem.fix = detect::fix_in_words(&problem.fix, self.host.profile());
        }
        checklist.in_words(self.host.profile());
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
        // Where its track and focus are, so Kumi's renames of them (not of a namesake) are followed.
        let named = self.named(track.as_deref(), goal.focus.as_deref(), signal.clone()).await;
        let mut run = JudgeRun {
            checklist,
            named,
            track,
            focus: goal.focus.clone(),
            span,
            span_file: (heard.file.clone(), heard.start),
            span_focus: heard.focus_file.clone(),
            span_premix: heard.premix_file.clone(),
            loudness: main.measures.short_term.clone(),
            problems: problems.clone(),
            first: whole.clone(),
            whole: whole.clone(),
            excerpts: vec![],
            state: 0,
            target: None,
            window: span,
            checkpoint: self.applied_ids(),
            outside: self.host.outside(),
            round: 0,
            listens: 1,
            started,
            ended: None,
            stale: false,
            steered: None,
            span_focus_gain: heard.focus_gain,
            misses: vec![],
            worst: vec![],
            rounds: vec![],
        };
        run.misses = vec![0; run.checklist.items.len()];
        run.worst = worst;
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
                change: crate::checklist::Change::Same,
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
                let notes: Vec<String> = [unread, unguarded, masking_off].into_iter().flatten().collect();
                (!notes.is_empty()).then(|| notes.join(". "))
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
    pub(crate) async fn judge_round(
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
        if self.judge.borrow().as_ref().unwrap().out_of_date() {
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
            None => match self.judge_hear(track.as_deref(), focus.as_deref(), window, &after(change.as_deref()), signal.clone()).await? {
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
                Quantity::Region { region } => crate::measure::THIRDS[REGIONS[region].2] >= 8_000.,
                _ => false,
            }
        });
        // Silence is taken back anyway: no one is asked about it.
        let heard_by = match heard.silent {
            Some(_) => None,
            None => self.listen_to_change(&before, &heard, &aim, width_or_air, signal.clone()).await,
        };
        let listener = heard_by.as_ref().map(|(line, _)| line.clone());
        // Main's chain now holds something that moves the mix but not the focus, which is heard before it: masking can't
        // be read across it, so it reads as it was this round (as clipping does across a rebalance), and goes off the
        // checklist from here if the round is kept.
        let masking = masking_items(&self.judge.borrow().as_ref().unwrap().checklist);
        let held: Vec<(usize, Option<f64>)> =
            if heard.main_unfair.is_empty() { vec![] } else { masking.iter().map(|index| (*index, before.values[*index])).collect() };
        let (unfair, missed) = (heard.main_unfair.clone(), heard.premix_missed.clone());
        let mut after = after;
        for (index, value) in &held {
            after[*index] = *value;
        }
        let excerpt = Listen { values: after, loudness: heard.main.measures.integrated, file: heard.file.clone(), start: heard.start };
        let host = InHost { listening: self, track: track.clone(), focus: focus.clone(), window, signal: signal.clone(), held };
        let (checklist, whole) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            (run.checklist.clone(), run.whole.clone())
        };
        let made = self.host.devices_made(&changes);
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
        let judging::Decided { mut verdict, rebalanced, whole: whole_now, excerpt, stopped, stayed, .. } = decided;
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
            run.outside = self.host.outside();
            // What Live wouldn't take back is still in the Set: the next judge hears the bars again first.
            if stayed {
                run.stale = true;
            }
            // A target two changes in a row failed on waits while another gap is open.
            if let Some(index) = target {
                run.misses[index] = if verdict.kept { 0 } else { run.misses[index] + 1 };
            }
            // A kept change left Main's chain moving the mix but not the focus: masking can't be read from here.
            if verdict.kept && !masking.is_empty() {
                if let Some(note) =
                    masking_unfair_in(self.host.profile(), run.focus.as_deref().unwrap_or("the focus"), &unfair, missed.as_deref())
                {
                    drop_items(run, &masking);
                    verdict.why = format!("{}; {}", verdict.why, note);
                }
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
    pub(crate) async fn rebaseline(self: &Rc<Self>, signal: Signal) -> Result<Result<(Round, bool), String>, RuntimeError> {
        let (track, focus, window, checkpoint, steered) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            (run.track.clone(), run.focus.clone(), run.window, run.checkpoint.clone(), run.steered.clone())
        };
        let audible = |id: &String| self.host.audible(id) == Some(true);
        // This answer's changes go first, to hear the bars without them, and are asked for again. After the producer's
        // words came in, only those made before them: what came after may be what they asked for, and stays.
        let before_words = |id: &String| steered.as_ref().is_none_or(|mark| mark.contains(id));
        let these: Vec<(String, String)> =
            self.applied_since(&checkpoint).into_iter().filter(|(id, _)| audible(id) && before_words(id)).collect();
        let mut taken = if these.is_empty() {
            None
        } else {
            let host = InHost { listening: self, track: track.clone(), focus: focus.clone(), window, signal: signal.clone(), held: vec![] };
            Some(judging::take_back_these(&host, these).await)
        };
        // What changed outside HISTORY can't go: it's in the new baseline, and isn't to be made again.
        if let (Some(taken), Some(what)) = (taken.as_mut(), self.outside_since_round()) {
            taken.said.push_str(&format!("; whatever {what} changed isn't in HISTORY, so it's in this baseline: don't run it again"));
            taken.stayed = true;
        }
        // What went is said however the listen ends: the model mustn't report it as made.
        let went = taken
            .as_ref()
            .filter(|taken| !taken.said.starts_with("nothing"))
            .map(|taken| format!("This answer's changes went first, to hear the bars without them ({}): make them again", taken.said));
        let what = if went.is_some() { "without this answer's changes" } else { "as it is now" };
        let heard = match self.judge_hear(track.as_deref(), focus.as_deref(), window, what, signal.clone()).await {
            Ok(Ok(JudgeHeard { silent: Some(why), .. })) | Ok(Err(why)) => {
                return Ok(Err(match went {
                    Some(went) => format!("{why}. {went} once the bars can be heard"),
                    None => why,
                }))
            }
            Ok(Ok(heard)) => heard,
            Err(error) => {
                if let Some(went) = went {
                    self.tell(went, None);
                }
                return Err(error);
            }
        };
        let file = self.keep_file(&heard.file).await;
        let ids = self.applied_ids();
        let round = {
            let mut guard = self.judge.borrow_mut();
            let run = guard.as_mut().unwrap();
            run.listens += 1;
            // Main's chain now moves the mix but not the focus: the new baseline holds no masking.
            let masking_off = drop_masking_if_unfair(self.host.profile(), run, &heard.main_unfair, heard.premix_missed.as_deref());
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
            run.outside = self.host.outside();
            run.stale = false;
            run.steered = None;
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
                why: Some({
                    let app = self.host.profile().words.app;
                    let said = match (&taken, steered.is_some()) {
                        (None, true) => "Your words came in during the run, and nothing of this answer's came before them, so Kumi heard its bars again with what you asked for in them, and starts from here".into(),
                        (None, false) => format!("{app} changed since the run's last listen (other requests in between), so Kumi heard its bars again and starts from here: a change already made is in this baseline, not judged"),
                        (Some(taken), true) => format!(
                            "Your words came in during the run: this answer's change made before them went first, to hear the bars without it ({}){}, and what came after them stays in this baseline. Make the change again, then judge it",
                            taken.said,
                            if taken.stayed { format!("; what {app} kept is in this baseline") } else { String::new() }
                        ),
                        (Some(taken), false) => format!(
                            "{app} changed since the run's last listen (other requests in between), so Kumi heard its bars again and starts from here. This answer's changes went first, to hear the bars without them ({}){}: make them again, then judge them",
                            taken.said,
                            if taken.stayed { format!("; what {app} kept is in this baseline") } else { String::new() }
                        ),
                    };
                    match masking_off {
                        Some(note) => format!("{said}. {note}"),
                        None => said,
                    }
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
        let heard_before = self.ensure_before(next, signal.clone()).await;
        if !matches!(heard_before, Ok(Ok(()))) {
            self.judge.borrow_mut().as_mut().unwrap().window = window;
        }
        // Esc stops here, once the app has the round (what it took back is said); a "before" that couldn't be heard
        // only leaves these bars.
        if let Err(error) = heard_before {
            if signal.check().is_err() {
                self.tell_judged(&round);
                return Err(error);
            }
        }
        Ok(Ok((round, taken.is_some())))
    }

    /// The excerpt `window` as things stand, heard now unless it already has been in this state.
    pub(crate) async fn ensure_before(self: &Rc<Self>, window: Window, signal: Signal) -> Result<Result<(), String>, RuntimeError> {
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
        let heard = match self.judge_hear(track.as_deref(), focus.as_deref(), window, "before the next change", signal).await? {
            Ok(JudgeHeard { silent: Some(why), .. }) | Err(why) => return Ok(Err(why)),
            Ok(heard) => heard,
        };
        let file = self.keep_file(&heard.file).await;
        let mut guard = self.judge.borrow_mut();
        let run = guard.as_mut().unwrap();
        run.listens += 1;
        if let Some(note) = drop_masking_if_unfair(self.host.profile(), run, &heard.main_unfair, heard.premix_missed.as_deref()) {
            self.tell(note, None);
        }
        let values = heard.read(&run.checklist);
        let state = run.state;
        run.excerpts.push(Excerpt { window, state, values, file, start: heard.start, loudness: heard.main.measures.integrated });
        Ok(Ok(()))
    }

    /// Tells the app a round was judged.
    pub(crate) fn tell_judged(&self, round: &Round) {
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
        let tempo = self.host.tempo().unwrap_or(120.);
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
        let heard = match self.judge_hear(track.as_deref(), focus.as_deref(), span, "where the run ends", signal).await? {
            Ok(JudgeHeard { silent: Some(why), .. }) | Err(why) => return Ok(Err(why)),
            Ok(heard) => heard,
        };
        let ids = self.applied_ids();
        // Main's chain moves the mix but not the focus: the last listen can't read masking, so it comes off.
        let masking_off = drop_masking_if_unfair(
            self.host.profile(),
            self.judge.borrow_mut().as_mut().unwrap(),
            &heard.main_unfair,
            heard.premix_missed.as_deref(),
        );
        // The whole span's readings are worked out off the app's thread.
        let checklist = self.judge.borrow().as_ref().unwrap().checklist.clone();
        let (main, focus, premix) = (heard.main, heard.focus, heard.premix);
        let now = tokio::task::spawn_blocking(move || checklist.read_with(&main, focus.as_ref(), premix.as_ref()))
            .await
            .map_err(|error| RuntimeError::plain(error.to_string()))?;
        let mut guard = self.judge.borrow_mut();
        let run = guard.as_mut().unwrap();
        run.listens += 1;
        // Over: what it kept stays, and nothing after is a round's to take back.
        run.ended = Some("it ended with done".into());
        run.checkpoint = ids;
        run.outside = self.host.outside();
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
            why: masking_off,
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
        // Its first six minutes, as the reference tool measures a track.
        let options = MeasureOptions { seconds: Some(MEASURED), signal: Some(signal.clone()), ..Default::default() };
        let heard = measure_file(&file, options)
            .await
            .map_err(|error| format!("Kumi couldn't hear the reference: {}", head(&error.to_string(), 200)))?;
        let name = file.rsplit(['/', '\\']).next().unwrap_or(&file).to_string();
        let (mut profile, heard) =
            tokio::task::spawn_blocking(move || (Profile::of(&name, &heard), heard)).await.map_err(|error| error.to_string())?;
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
    /// file or a folder kept, or the producer's own songs): each heard as the reference tool hears it, at a common
    /// loudness, the vibes averaged, with how far they lie from that. None when it wasn't kept from files, or one is gone
    /// or can't be heard.
    async fn restyled(&self, kept: &KeptReference, signal: &Signal) -> Option<(Vec<f32>, Option<f64>, String)> {
        let (slot, style) = self.style_model.borrow().clone()?;
        let from_files = matches!(kept.kind.as_str(), "files" | "music");
        if !from_files || kept.tracks.is_empty() || kept.tracks.iter().any(|track| !Path::new(&track.source).is_file()) {
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

    /// Hears `window`: the mix (or the run's track) and the focus element in one pass, measured. The producer hears it
    /// too, told the bars and `what` they hear (the state it's in: "after …", "where the run starts").
    pub(crate) async fn judge_hear(
        self: &Rc<Self>,
        track: Option<&str>,
        focus: Option<&str>,
        window: Window,
        what: &str,
        signal: Signal,
    ) -> Result<Result<JudgeHeard, String>, RuntimeError> {
        let tempo = self.host.tempo().unwrap_or(120.);
        let main = match track {
            Some(track) => AuditionCandidate { track: track.into(), mix: None, label: None, clip: None },
            None => AuditionCandidate { track: MIX_CANDIDATE.into(), mix: Some(true), label: Some("The whole mix".into()), clip: None },
        };
        let mut candidates = vec![main];
        if let Some(focus) = focus.filter(|focus| Some(*focus) != track) {
            candidates.push(AuditionCandidate { track: focus.into(), mix: None, label: None, clip: None });
        }
        self.tell(format!("{} {} · {what}", self.hearing(), self.bars_of(window)), Some(true));
        let rendering = self.host.flag().begin();
        // Over the mix with a focus, the mix is heard as it comes into the master chain too: masking reads there.
        let before_main = track.is_none() && candidates.len() > 1;
        let pass = self.host.hear_pass(&candidates, window, before_main, signal.clone()).await;
        drop(rendering);
        self.tell("Listened", Some(false));
        let (notes, tried) = (pass.notes, pass.tried);
        let Captured { files, sources, before_master } = pass.rendered?;
        let main_name = sources.iter().find(|(name, mix)| *mix || Some(name.as_str()) == track).map(|(name, _)| name.clone());
        let focus_name = sources.iter().find(|(name, mix)| !*mix && Some(name.as_str()) != track).map(|(name, _)| name.clone());
        let Some(main) = main_name.and_then(|name| files.get(&name).cloned()) else {
            return Ok(Err(format!(
                "Nothing came through{}",
                if notes.is_empty() {
                    format!(". Is something playing there in the {}?", self.host.profile().words.timeline)
                } else {
                    format!(": {}", notes.join(" "))
                }
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
            let profile = self.host.profile();
            // (a render offline needs no audio device: only what's in the part)
            let running =
                if profile.caps.renders_to_listen { String::new() } else { format!("{}'s audio running, and is ", profile.words.app) };
            format!(
                "Kumi heard only silence from {}{}. Is {running}something playing there in the {}?",
                track.unwrap_or("the mix"),
                if notes.is_empty() { String::new() } else { format!(" ({})", notes.join(" ")) },
                profile.words.timeline
            )
        });
        // On a run over the mix with a focus, the mix as it comes into Main's chain, when Kumi Ears heard it there: the
        // focus is heard before Main's chain too, so masking reads fairly whatever Main's chain holds.
        let premix = match (track, &focus_name, before_master.as_ref()) {
            (None, Some(_), Some(render)) => {
                let heard = measure(render.file.clone(), render.start).await?;
                Some((heard, (self.keep_file(&PathBuf::from(&render.file)).await, render.start)))
            }
            _ => None,
        };
        // Without it, Main's chain as it is now (read from Live, so an undo or the producer's own move is in it): the
        // mix is heard after it, the focus before it.
        let main_chain = match (track, &focus_name, &premix) {
            (None, Some(_), None) => Some(self.host.master_chain(signal.clone()).await),
            _ => None,
        };
        let (heard_focus, focus_file, focus_gain) = match focus_name.clone().and_then(|name| files.get(&name).cloned()) {
            Some(render) => {
                // Heard before its fader: as the mix hears it, at the fader's level (so turning it up reads as up), and
                // with the level Main's Utilities add.
                let fader = self.host.fader_of(focus_name.as_deref().unwrap_or(""), signal.clone()).await.unwrap_or(0.);
                let gain = fader + main_chain.as_ref().map_or(0., |state| state.gain);
                let heard = measure(render.file.clone(), render.start).await?.gained(gain);
                (Some(heard), Some((self.keep_file(&PathBuf::from(&render.file)).await, render.start)), gain)
            }
            None => (None, None, 0.),
        };
        if silent.is_none() {
            heard_main.embedding =
                self.embedding(Path::new(&main.file), main.start, seconds, heard_main.measures.integrated, &signal).await;
        }
        let file = self.keep_file(&PathBuf::from(&main.file)).await;
        let main_unfair = main_chain.map(|state| state.unfair).unwrap_or_default();
        let premix_missed = if premix.is_none() { tried } else { None };
        let (premix, premix_file) = premix.unzip();
        // What went wrong around a listen that worked is said too (with silence, it's in what's said about that).
        if silent.is_none() {
            for note in &notes {
                self.tell(note.clone(), None);
            }
        }
        Ok(Ok(JudgeHeard {
            main: heard_main,
            focus: heard_focus,
            file,
            start: main.start,
            focus_file,
            premix,
            premix_file,
            silent,
            focus_gain,
            main_unfair,
            premix_missed,
        }))
    }

    /// Copies a capture into the judge's own folder, out of the listening folder's pruning.
    /// What judge/ holds that the run doesn't store goes: its span's takes and its excerpts stay, and a new run's
    /// takes replace an old run's.
    pub(crate) async fn prune_kept(&self) {
        let stored: Vec<PathBuf> = match self.judge.borrow().as_ref() {
            Some(run) => std::iter::once(run.span_file.0.clone())
                .chain(run.span_focus.iter().map(|(file, _)| file.clone()))
                .chain(run.excerpts.iter().map(|excerpt| excerpt.file.clone()))
                .collect(),
            None => vec![],
        };
        prune_unstored(&self.host.kept_folder(), &stored).await;
    }

    pub(crate) async fn keep_file(&self, file: &std::path::Path) -> PathBuf {
        let folder = self.host.kept_folder();
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
        let tempo = self.host.tempo().unwrap_or(120.);
        let meter = self.host.beats_per_bar().max(1.);
        (8. * meter).max((12. * tempo / 60. / meter).ceil() * meter)
    }

    pub(crate) fn excerpt_for(&self, run: &JudgeRun) -> Window {
        let tempo = self.host.tempo().unwrap_or(120.);
        let meter = self.host.beats_per_bar().max(1.);
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
    pub(crate) async fn cut_excerpt(&self, run: &JudgeRun, window: Window, signal: Signal) -> Result<Excerpt, RuntimeError> {
        let tempo = self.host.tempo().unwrap_or(120.);
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
            Some((file, start)) => Some(cut(file, start).await?.gained(run.span_focus_gain)),
            None => None,
        };
        let premix = match run.span_premix.clone() {
            Some((file, start)) => Some(cut(file, start).await?),
            None => None,
        };
        let values = run.checklist.read_with(&heard, focus.as_ref(), premix.as_ref());
        Ok(Excerpt { window, state: run.state, values, file, start: start + into, loudness: heard.measures.integrated })
    }

    fn describe(&self, window: Window, run: &JudgeRun) -> String {
        let meter = self.host.beats_per_bar().max(1.);
        let bar = |beat: f64| (beat / meter).floor() as i64 + 1;
        let what = run.track.clone().unwrap_or_else(|| "the mix".into());
        format!("{what}, bars {}–{}", bar(window.from), bar(window.from + window.beats - 1e-6))
    }

    pub(crate) fn next_step(&self, run: &JudgeRun) -> Option<Next> {
        run.target.map(|index| next_of(run, index))
    }

    /// What changed Live outside HISTORY since the judged run's last round, in words (each said once).
    pub(crate) fn outside_since_round(&self) -> Option<String> {
        let mark = self.judge.borrow().as_ref()?.outside;
        let what = self.host.outside_since(mark);
        (!what.is_empty()).then(|| what.join(" and "))
    }

    /// The judged run hears its track and focus by name: after Kumi renamed one of them since it started, by its new
    /// name (a rename taken back since drops out, and the name goes back with it).
    pub(crate) fn follow_renames(&self) {
        let named = match self.judge.borrow().as_ref() {
            Some(run) => run.named.clone(),
            None => return,
        };
        let (track, focus) = self.names_now(&named);
        if let Some(run) = self.judge.borrow_mut().as_mut() {
            (run.track, run.focus) = (track, focus);
        }
    }

    /// A run's track and focus as it starts (see `Named`): their names, and their refs read from the DAW, made long.
    pub async fn named(self: &Rc<Self>, track: Option<&str>, focus: Option<&str>, signal: Signal) -> Named {
        let mut named = Named { from: self.applied_ids(), ..Named::default() };
        for (name, slot) in [(track, &mut named.track), (focus, &mut named.focus)] {
            let Some(name) = name else { continue };
            let long = self.host.track_ref(name, signal.clone()).await;
            *slot = Some((name.to_owned(), long));
        }
        named
    }

    /// The names a run's track and focus go by now: Kumi's track renames since it started (applied ones), followed by
    /// the ref and the name each had.
    pub fn names_now(&self, named: &Named) -> (Option<String>, Option<String>) {
        // Oldest first: the renamed track's ref, the name it had, and its new one.
        let renames = self.host.renames(&named.from);
        let follow = |slot: &Option<(String, Option<String>)>| {
            slot.as_ref().map(|(name, reference)| judging::renamed(name, reference.as_deref(), &renames))
        };
        (follow(&named.track), follow(&named.focus))
    }
}

/// Takes masking off a run's checklist when Main's chain (`unfair`, as last heard) moves the mix but not the focus;
/// what to tell, when it did (and why the mix wasn't heard as it comes into Main's chain, when Kumi tried: `missed`).
fn drop_masking_if_unfair(profile: &DawProfile, run: &mut JudgeRun, unfair: &[String], missed: Option<&str>) -> Option<String> {
    let masking = masking_items(&run.checklist);
    if masking.is_empty() {
        return None;
    }
    let note = masking_unfair_in(profile, run.focus.as_deref().unwrap_or("the focus"), unfair, missed)?;
    drop_items(run, &masking);
    Some(note)
}

/// The checklist's masking items.
pub(crate) fn masking_items(checklist: &Checklist) -> Vec<usize> {
    (0..checklist.items.len())
        .filter(|index| matches!(checklist.items[*index].quantity, Quantity::Problem { problem: ProblemKind::Masking, .. }))
        .collect()
}

/// Takes items off a run's checklist from here, with their readings, misses and excerpts' values.
fn drop_items(run: &mut JudgeRun, indices: &[usize]) {
    let keep = |values: &[Option<f64>]| -> Vec<Option<f64>> {
        values.iter().enumerate().filter(|(index, _)| !indices.contains(index)).map(|(_, value)| *value).collect()
    };
    run.checklist = run.checklist.without(indices);
    run.whole = keep(&run.whole);
    run.first = keep(&run.first);
    run.worst = keep(&run.worst);
    run.misses = run.misses.iter().enumerate().filter(|(index, _)| !indices.contains(index)).map(|(_, misses)| *misses).collect();
    for excerpt in &mut run.excerpts {
        excerpt.values = keep(&excerpt.values);
    }
    run.target = None;
}

/// A round's moves in Live: its excerpt heard, its track's chain, Kumi's HISTORY. Taking back and removing go on after
/// Esc (what was changed is put back), with the cleanup's own time limit.
/// What a round's listen plays, said to the producer: after the change it judges, in the model's (or tune's) words.
fn after(change: Option<&str>) -> String {
    match change.map(str::trim).filter(|change| !change.is_empty()) {
        Some(change) => format!("after {}", kumi_common::js::string::head(change.trim_end_matches('.'), 120)),
        None => "after this answer's changes".into(),
    }
}

struct InHost<'a> {
    listening: &'a Rc<Listening>,
    track: Option<String>,
    focus: Option<String>,
    window: Window,
    signal: Signal,
    /// Checklist items read as they were before the change, whatever a listen says (masking, after a change on Main's
    /// chain: the focus is heard before it), with that reading.
    held: Vec<(usize, Option<f64>)>,
}

impl InHost<'_> {
    /// The run's track now (the master without one).
    fn scope(&self) -> Option<String> {
        self.listening.judge.borrow().as_ref().and_then(|run| run.track.clone())
    }
}

#[async_trait(?Send)]
impl RoundHost for InHost<'_> {
    fn app(&self) -> &str {
        self.listening.host.profile().words.app
    }
    async fn turn(&self, gain: f64) -> Result<String, String> {
        let scope = self.scope();
        self.listening.host.turn_gain(gain, scope.as_deref(), self.signal.clone()).await.map(|(label, _)| label)
    }
    async fn hear(&self) -> Result<Listen, Unheard> {
        match self
            .listening
            .judge_hear(self.track.as_deref(), self.focus.as_deref(), self.window, "as it is now", self.signal.clone())
            .await
        {
            Ok(Ok(heard)) => {
                let mut values = heard.read(&self.listening.judge.borrow().as_ref().unwrap().checklist);
                for (index, value) in &self.held {
                    values[*index] = *value;
                }
                Ok(Listen { values, loudness: heard.main.measures.integrated, file: heard.file, start: heard.start })
            }
            Ok(Err(why)) => Err(Unheard { why, stopped: false }),
            Err(error) => Err(Unheard { why: head(&error.to_string(), 200), stopped: self.signal.check().is_err() }),
        }
    }
    fn applied(&self) -> Vec<String> {
        self.listening.applied_ids()
    }
    fn applied_since(&self, mark: &[String]) -> Vec<(String, String)> {
        self.listening.applied_since(mark)
    }
    fn audible(&self, id: &str) -> bool {
        self.listening.host.audible(id).unwrap_or(true)
    }
    fn outside(&self) -> Option<String> {
        self.listening.outside_since_round()
    }
    async fn undo(&self, id: &str) -> Result<(), String> {
        self.listening.host.undo(id).await
    }
    fn made(&self, changes: &[(String, String)]) -> Vec<String> {
        self.listening.host.made(changes)
    }
    fn stopped(&self) -> bool {
        self.signal.check().is_err()
    }
    async fn limited(&self) -> bool {
        let scope = self.scope();
        self.listening.host.limited(scope.as_deref(), self.signal.clone()).await
    }
    async fn chain(&self) -> Result<Vec<Placed>, String> {
        self.listening.host.chain(self.track.as_deref()).await
    }
    async fn delete(&self, reference: &str) -> bool {
        self.listening.host.delete_device(reference).await
    }
}

/// What the judge heard: the mix (or the run's track), the focus element, and the main file kept for later cuts.
pub(crate) struct JudgeHeard {
    pub main: Heard,
    pub focus: Option<Heard>,
    /// The focus element's own capture and where its part starts, to cut excerpts from.
    pub focus_file: Option<(PathBuf, f64)>,
    /// On a run over the mix with a focus, the mix as it comes into Main's chain, when Kumi Ears heard it there too
    /// (a second device, first on Main's chain): masking reads the focus against it, both before Main's chain. And its
    /// capture, to cut excerpts from.
    pub premix: Option<Heard>,
    pub premix_file: Option<(PathBuf, f64)>,
    pub file: PathBuf,
    pub start: f64,
    /// Silence came through (why that's a problem, said): nothing on the checklist can be read from it.
    pub silent: Option<String>,
    /// The gain (dB) the focus element was heard with.
    pub focus_gain: f64,
    /// On a run over the mix with a focus, Main's devices that change the mix in a way the focus (heard before Main)
    /// doesn't share: masking can't be read fairly while there are any.
    pub main_unfair: Vec<String>,
    /// Why the mix wasn't heard as it comes into Main's chain, when Kumi Ears was there and tried to.
    pub premix_missed: Option<String>,
}

impl JudgeHeard {
    /// What each checklist item reads: nothing at all from silence.
    pub fn read(&self, checklist: &Checklist) -> Vec<Option<f64>> {
        if self.silent.is_some() {
            return vec![None; checklist.items.len()];
        }
        checklist.read_with(&self.main, self.focus.as_ref(), self.premix.as_ref())
    }
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

#[cfg(test)]
mod tests {
    use super::prune_unstored;

    #[tokio::test(flavor = "current_thread")]
    async fn a_judged_runs_folder_keeps_only_what_the_run_stores() {
        let folder = tempfile::tempdir().unwrap();
        let [span, excerpt, dropped] = ["span.wav", "excerpt.wav", "dropped.wav"].map(|name| {
            let path = folder.path().join(name);
            std::fs::write(&path, b"take").unwrap();
            path
        });
        prune_unstored(folder.path(), &[span.clone(), excerpt.clone()]).await;
        assert!(span.exists() && excerpt.exists() && !dropped.exists());
        // A new run, or none, stores none of the old run's takes.
        prune_unstored(folder.path(), &[]).await;
        assert!(!span.exists() && !excerpt.exists());
    }
}
