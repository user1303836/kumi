//! Match rounds, accumulated usage, learned lessons and final knob search.
use super::*;
use crate::core::{
    evolve::{Evolution, NewSlot, TrialHow, EVOLVE},
    match_run::{starts_match, MatchBudget, MatchDecision, MatchRun, MatchState, MatchStop, KEEP_GOING, MATCH_BUDGET},
    playbook::{lesson_from, lesson_line, playbook_brief, PlaybookStore, Reaction},
    techniques::{NEGATIVE, POSITIVE},
};
use kumi_common::js::number::{round, to_string};

pub(super) struct LastLesson {
    pub id: String,
    pub judged: bool,
}
pub(super) fn add_usage(to: &mut Usage, from: Option<&Usage>) {
    if let Some(from) = from {
        to.input_tokens += from.input_tokens;
        to.output_tokens += from.output_tokens;
        to.cache_read_tokens += from.cache_read_tokens;
        to.cache_write_tokens += from.cache_write_tokens;
    }
}
impl Operation {
    pub(super) fn extend(&self, ms: u64) {
        self.limit.set(Some(Instant::now() + Duration::from_millis(ms)));
        self.changed.notify_one();
    }
}
impl Session {
    pub(super) fn playbook_serial<T: 'static>(
        &self,
        work: impl FnOnce(Rc<dyn PlaybookStore>) -> LocalBoxFuture<'static, Result<T, RuntimeError>> + 'static,
    ) -> LocalBoxFuture<'static, Option<T>> {
        let Some(store) = self.0.options.playbook.clone() else {
            return async { None }.boxed_local();
        };
        let previous = self.0.playbook_queue.borrow().clone();
        let (tx, rx) = oneshot::channel();
        *self.0.playbook_queue.borrow_mut() = async { rx.await.unwrap_or(Ok(())) }.boxed_local().shared();
        async move {
            let _ = previous.await;
            let result = work(store).await.ok();
            let _ = tx.send(Ok(()));
            result
        }
        .boxed_local()
    }
    fn learn_from(&self, run: &MatchRun, carried: bool) {
        let Some(mut lesson) = lesson_from(run, now() as f64) else {
            return;
        };
        let replacing = if carried { self.0.state.borrow().last_lesson.as_ref().map(|l| l.id.clone()) } else { None };
        if let Some(id) = &replacing {
            lesson.id = id.clone();
        }
        self.0.state.borrow_mut().last_lesson = Some(LastLesson { id: lesson.id.clone(), judged: false });
        let this = self.clone();
        let work = self.playbook_serial(move |store| {
            async move {
                let mut lessons = store.list().await?;
                lessons.retain(|l| l.id != lesson.id);
                lessons.push(lesson.clone());
                store.save(&lessons).await?;
                this.emit(SessionEvent::Lesson {
                    action: if replacing.is_some() { LessonAction::Updated } else { LessonAction::Learned },
                    id: lesson.id.clone(),
                    line: lesson_line(&lesson),
                });
                Ok(())
            }
            .boxed_local()
        });
        tokio::task::spawn_local(work);
    }
    fn judge_lesson(&self, input: &str) {
        let id = {
            let mut s = self.0.state.borrow_mut();
            let Some(l) = s.last_lesson.as_mut().filter(|l| !l.judged) else {
                return;
            };
            l.judged = true;
            l.id.clone()
        };
        let reaction = if NEGATIVE.is_match(input) {
            Reaction::Disliked
        } else if POSITIVE.is_match(input) {
            Reaction::Liked
        } else {
            return;
        };
        let work = self.playbook_serial(move |store| {
            async move {
                let mut lessons = store.list().await?;
                if let Some(lesson) = lessons.iter_mut().find(|l| l.id == id) {
                    lesson.reaction = Some(reaction);
                    store.save(&lessons).await?;
                }
                Ok(())
            }
            .boxed_local()
        });
        tokio::task::spawn_local(work);
    }
    pub(super) async fn ask(
        &self,
        op: &Rc<Operation>,
        text: &str,
        observation: &str,
        said: Option<Rc<RefCell<String>>>,
        pictures: Vec<Picture>,
    ) -> Result<TurnResult, RuntimeError> {
        let held = self.0.state.borrow().kernel.clone().ok_or_else(|| RuntimeError::plain("Operation cancelled"))?;
        let current = self.clone();
        let progress = op.clone();
        let emit = Rc::new(move |event: KernelEvent| {
            if current.current(&progress) {
                if let (Some(said), KernelEvent::Text { text }) = (&said, &event) {
                    said.borrow_mut().push_str(text);
                }
                progress.progress(Some(&event));
                current.emit(event.into());
            }
            Ok(())
        });
        let input = format!("{text}{OBSERVATION_MARKER}\n{observation}\n</current_observation_untrusted>");
        if pictures.is_empty() {
            held.value.run(&input, op.signal.clone(), emit).await
        } else {
            held.value.run_with(&input, pictures, op.signal.clone(), emit).await
        }
    }
    pub(super) async fn submit_turn(
        &self,
        op: Rc<Operation>,
        text: String,
        pinned: Option<PinnedNode>,
        pictures: Vec<Picture>,
    ) -> Result<Option<TurnResult>, RuntimeError> {
        let snapshot = self.observe(&op, pinned, false).await?;
        self.assert_current(&op)?;
        if let Some(l) = &self.0.learned {
            l.drafts.said(&text);
            l.drafts.turn_started(&text);
        }
        let budget = self.0.options.match_budget.unwrap_or(MATCH_BUDGET);
        let carried =
            self.0.options.matching && !starts_match(&text) && KEEP_GOING.is_match(&text) && self.0.state.borrow().last_run.is_some();
        if !carried {
            self.judge_lesson(&text);
        }
        let run = if !self.0.options.matching {
            None
        } else if starts_match(&text) {
            Some(Rc::new(RefCell::new(MatchRun::new(&text, budget))))
        } else if carried {
            let previous = self.0.state.borrow().last_run.clone().unwrap();
            {
                let run = MatchRun::carry_on(&previous.borrow(), budget, Rc::new(now));
                Some(Rc::new(RefCell::new(run)))
            }
        } else {
            None
        };
        {
            let mut s = self.0.state.borrow_mut();
            if run.is_none() {
                s.last_run = None;
            }
            s.matching = run.clone();
        }
        let brief = if run.is_some() && !carried {
            playbook_brief(&self.playbook_serial(|s| async move { s.list().await }.boxed_local()).await.unwrap_or_default(), &text, 5)
        } else {
            String::new()
        };
        self.assert_current(&op)?;
        op.phase.set(Phase::Inference);
        let prompt = if brief.is_empty() { text } else { format!("{text}\n\n{brief}") };
        let showing = !pictures.is_empty();
        let result = match self.ask(&op, &prompt, &snapshot.context, None, pictures).await {
            // A model that can't see pictures refuses the request: say so, and what to do.
            Err(RuntimeError::Kumi(error)) if showing && error.kind == FailureKind::Request => {
                return Err(KumiError {
                    message: format!(
                        "{} The model may not take pictures: choose another with /model, or send this without the picture.",
                        error.message
                    ),
                    ..error
                }
                .into())
            }
            result => result?,
        };
        let Some(run) = run else {
            return Ok(Some(result));
        };
        let result = self.run_match(&op, &run, result, budget).await;
        {
            let mut s = self.0.state.borrow_mut();
            s.matching = None;
            s.last_run = Some(run.clone());
        }
        self.learn_from(&run.borrow(), carried);
        result.map(Some)
    }
    async fn run_match(
        &self,
        op: &Rc<Operation>,
        run: &Rc<RefCell<MatchRun>>,
        first: TurnResult,
        budget: MatchBudget,
    ) -> Result<TurnResult, RuntimeError> {
        op.extend((budget.ms + 5 * 60_000).max(0) as u64);
        let mut result = first;
        let mut usage = result.usage.clone().unwrap_or_default();
        self.emit(run.borrow().status(MatchState::Running, None).into());
        while result.stop_reason == StopReason::Completed && !op.signal.is_cancelled() {
            let integration = self.0.state.borrow().integration.clone();
            if run.borrow().needs_audition() {
                if let Some(i) = integration.filter(|i| i.has_audition()) {
                    self.emit(SessionEvent::Doing { text: "Listening to where it's got to".into() });
                    let request = run.borrow().last.as_ref().and_then(|l| l.request.clone()).unwrap();
                    let _ = i.audition(&request, op.signal.clone()).await;
                    self.assert_current(op)?;
                }
            }
            let mut decision = run.borrow_mut().decide();
            if matches!(decision, MatchDecision::Stop { stop: MatchStop::Plateau | MatchStop::Budget, .. }) && run.borrow().polishes() {
                let tuned = self.polish(op, run).await?;
                self.assert_current(op)?;
                if let (Some(tuned), MatchDecision::Stop { stop, wrap_up }) = (tuned, &mut decision) {
                    *wrap_up = Some(format!(
                        "{tuned} {}",
                        run.borrow().wrap_up(if *stop == MatchStop::Plateau {
                            "Refining and new ideas both stopped gaining."
                        } else {
                            "That's the run's budget spent."
                        })
                    ));
                }
            }
            self.emit(run.borrow().status(MatchState::Running, None).into());
            let (text, stop) = match decision {
                MatchDecision::Next { next } => (Some(next), None),
                MatchDecision::Stop { stop, wrap_up } => (wrap_up, Some(stop)),
            };
            let Some(text) = text.filter(|s| !s.is_empty()) else {
                self.emit(run.borrow().status(MatchState::Done, stop).into());
                break;
            };
            let snapshot = self.observe(op, None, true).await?;
            self.assert_current(op)?;
            op.phase.set(Phase::Inference);
            result = self.ask(op, &text, &snapshot.context, None, vec![]).await?;
            add_usage(&mut usage, result.usage.as_ref());
            if stop.is_some() {
                self.emit(run.borrow().status(MatchState::Done, stop).into());
                break;
            }
        }
        result.usage = Some(usage);
        Ok(result)
    }
    async fn polish(&self, op: &Rc<Operation>, run: &Rc<RefCell<MatchRun>>) -> Result<Option<String>, RuntimeError> {
        let (request, candidate, label, ms) = {
            let mut run = run.borrow_mut();
            run.polished = true;
            (
                run.last.as_ref().and_then(|l| l.request.clone()),
                run.best_candidate.clone(),
                run.best.as_ref().map(|b| b.label.clone()),
                run.polish_ms(),
            )
        };
        let integration = self.0.state.borrow().integration.clone().filter(|i| i.has_goal());
        let (Some(i), Some(mut request), Some(mut candidate), Some(label)) =
            (integration, request.filter(|r| r.reference.is_some()), candidate, label)
        else {
            return Ok(None);
        };
        op.extend(ms + 10 * 60_000);
        let start_event = KernelEvent::ToolStart { id: String::new(), name: String::new() };
        let end_event = KernelEvent::ToolEnd { id: String::new(), name: String::new(), is_error: false, elapsed_ms: 0 };
        op.progress(Some(&start_event));
        self.emit(SessionEvent::Doing { text: format!("Tuning {label}'s knobs") });
        candidate.label = Some(label.clone());
        request.candidates = vec![candidate];
        let result = async {
            let rig = match i.goal(&request, op.signal.clone()).await {
                Ok(Ok(rig)) => rig,
                other => {
                    let why = match other {
                        Ok(Err(s)) => s,
                        Err(e) => e.message(),
                        _ => unreachable!(),
                    };
                    self.notice(format!("Kumi's knob search couldn't tune {label}: {why}"));
                    return Ok(None);
                }
            };

            let mut evolution = self.evolution();
            for slot in rig.slots() {
                evolution.add(NewSlot { name: slot.name, label: slot.label, chain: slot.chain, knobs: slot.knobs, score: None, heard: None });
            }
            let winner = evolution.slots.first().cloned().ok_or_else(|| RuntimeError::plain("Empty goal rig"))?;

            let knobs = winner.knobs;
            let start = winner.elite;
            let mut from = None;
            let mut to = None;
            let mut kept = None;

            let trial = |knobs, values, fresh| GenerationTrial { slot: winner.name.clone(), knobs, values, fresh };

            let search: Result<(), RuntimeError> = async {
                from = rig
                    .generation(&[trial(knobs.clone(), start.clone(), None)], op.signal.clone(), Some(GenerationOptions { screen: Some(false) }))
                    .await?
                    .scores
                    .get(&winner.name)
                    .copied();

                let Some(score) = from else {
                    return Ok(());
                };
                let began = Instant::now();
                let mut silent = 0;

                while began.elapsed().as_millis() < ms as u128 && !op.signal.is_cancelled() {
                    let trials = evolution.propose();
                    let requests = generation_trials(&evolution, &trials);

                    let result = rig.generation(&requests, op.signal.clone(), Some(GenerationOptions { screen: Some(rig.screens()) })).await?;

                    evolution.scored(&trials, &result.scores);
                    for (name, keys) in &result.frozen {
                        evolution.freeze(name, keys);
                    }
                    op.progress(Some(&start_event));
                    op.progress(Some(&end_event));
                    self.emit(SessionEvent::Doing { text: format!("Tuning {label}'s knobs · {} settings heard", evolution.rendered) });

                    silent = if result.scores.is_empty() { silent + 1 } else { 0 };
                    if silent >= 3 || evolution.stalled_for() >= 12 {
                        break;
                    }
                }
                if let Some(best) = evolution.leader() {
                    let moved = best
                        .knobs
                        .iter()
                        .enumerate()
                        .any(|(i, k)| knobs.iter().position(|old| knob_key(old) == knob_key(k)).is_none_or(|at| best.elite[i] != start[at]));

                    if moved {
                        to = rig
                            .generation(
                                &[trial(best.knobs.clone(), best.elite.clone(), Some(true))],
                                op.signal.clone(),
                                Some(GenerationOptions { screen: Some(false) }),
                            )
                            .await?
                            .scores
                            .get(&winner.name)
                            .copied();
                    }
                    if to.is_some_and(|to| to >= score + 1.) {
                        kept = Some((best.knobs.clone(), best.elite.clone()));
                    }
                }
                Ok(())
            }
            .await;

            let mut notes = rig
                .close()
                .await
                .unwrap_or_else(|_| vec!["Kumi couldn't remove its render tracks; delete the “Kumi · render” tracks by hand.".into()]);

            let (final_knobs, final_values) = kept.as_ref().map(|(k, v)| (k.as_slice(), v.as_slice())).unwrap_or((&knobs, &start));

            if let Some(reason) = rig.settle(&winner.name, final_knobs, final_values, abort::timeout(120_000)).await?.filter(|s| !s.is_empty()) {
                notes.push(format!("Its settings couldn't all be put back ({reason}); check {}.", winner.name));
            }
            search?;
            let Some(from) = from else {
                return Ok(None);
            };

            let heard = format!("Kumi's knob search then tried {} settings of {label} (on “{}”)", evolution.rendered, winner.name);

            let notes = if notes.is_empty() { String::new() } else { format!(" {}", notes.join(" ")) };

            let Some((kept_knobs, kept_values)) = kept else {
                return Ok(Some(format!(
                    "[Kumi] {heard}: none beat it at full length ({}%), so its settings are as you left them.{notes}",
                    to_string(round(from))
                )));
            };

            run.borrow_mut().tuned(format!("{label}, tuned"), round(to.unwrap()));

            let changed: Vec<_> = kept_knobs
                .iter()
                .zip(&kept_values)
                .filter_map(|(knob, value)| {
                    let was = *start.get(knobs.iter().position(|k| knob_key(k) == knob_key(knob))?)?;

                    if (value - was).abs() < 1e-6 * (knob.max - knob.min).max(1.) {
                        return None;
                    }
                    let device = knob
                        .device
                        .split_once(':')
                        .filter(|(prefix, _)| !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_digit()))
                        .map(|(_, rest)| rest)
                        .unwrap_or(&knob.device);

                    Some(format!("{device} {} {} → {}", knob.name, round3(was), round3(*value)))
                })
                .collect();

            Ok(Some(format!("[Kumi] {heard}: {}% → {}% at full length, kept on the track (and a Limiter ends its chain for safety). The knobs it moved: {}{}. Use these values if you rebuild it elsewhere.{notes}", to_string(round(from)), to_string(round(to.unwrap())), changed.iter().take(30).cloned().collect::<Vec<_>>().join("; "), if changed.len()>30 {
         format!("; and {} more",changed.len()-30)
        } else {
         String::new()
        })))
        }.await;

        op.progress(Some(&end_event));
        result
    }
    pub(super) fn evolution(&self) -> Evolution {
        let random = self.0.options.goal_random.clone();
        Evolution::new(move || random.as_ref().map(|r| r()).unwrap_or_else(rand::random::<f64>), EVOLVE)
    }
}
pub(super) fn knob_key(knob: &crate::core::evolve::Knob) -> String {
    format!("{}|{}", knob.device, knob.name)
}
pub(super) fn generation_trials(e: &Evolution, trials: &[crate::core::evolve::Trial]) -> Vec<GenerationTrial> {
    trials
        .iter()
        .map(|t| GenerationTrial {
            slot: t.slot.clone(),
            knobs: e.slots.iter().find(|s| s.name == t.slot).unwrap().knobs.clone(),
            values: t.values.clone(),
            fresh: (t.how == TrialHow::Recheck).then_some(true),
        })
        .collect()
}
fn round3(n: f64) -> String {
    to_string(format!("{n:.2e}").parse().unwrap_or(n))
}
