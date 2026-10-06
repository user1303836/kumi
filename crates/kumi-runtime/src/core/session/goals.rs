//! Persistent goal search, model leaps, cancellation cleanup and final result retention.
use super::matching::{add_usage, generation_trials, knob_key};
use super::*;
use crate::core::{
    evolve::{Evolution, NewSlot, Slot},
    goal::{goal_leap, goal_setup, Best, GoalPhase, GoalRun, GoalState, GoalStatus, GOAL_BUDGET},
    playbook::{lesson_from_goal, lesson_line, playbook_brief, GoalLeader},
};
use indexmap::IndexMap;
use kumi_common::js::number::{round, to_string};

fn reported(e: &Evolution, full: &IndexMap<String, f64>, screening: bool) -> Option<Slot> {
    if !screening || full.is_empty() {
        return e.leader().cloned();
    }
    let (name, score) = full.iter().reduce(|best, item| if item.1 > best.1 { item } else { best }).unwrap();
    e.slots.iter().find(|s| &s.name == name).map(|s| Slot { score: Some(*score), ..s.clone() }).or_else(|| e.leader().cloned())
}
fn sync(state: &mut GoalState, evolution: &Evolution, started: i64) {
    state.slots = evolution.slots.clone();
    state.generation = evolution.generation;
    state.rendered = evolution.rendered;
    state.trend = evolution.trend.iter().skip(evolution.trend.len().saturating_sub(500)).copied().collect();
    state.elapsed_ms = now() - started;
}
fn join(e: &mut Evolution, slot: GoalSlotInfo, state: &GoalState) {
    let name = slot.name.clone();
    e.add(NewSlot { name: slot.name, label: slot.label, chain: slot.chain, knobs: slot.knobs, score: None, heard: None });
    if let Some(kept) = state.slots.iter().find(|s| s.name == name) {
        let added = e.slots.iter_mut().find(|s| s.name == name).unwrap();
        for (i, knob) in added.knobs.iter().enumerate() {
            if let Some(j) = kept.knobs.iter().position(|old| knob_key(old) == knob_key(knob)) {
                if let Some(v) = kept.elite.get(j) {
                    added.elite[i] = *v;
                }
            }
        }
        if kept.score.is_some() {
            added.score = kept.score;
        }
        added.sigma = kept.sigma;
        added.stale = kept.stale;
    }
}
fn candidates(e: &Evolution, clips: bool) -> Vec<AuditionCandidate> {
    e.slots
        .iter()
        .map(|s| AuditionCandidate { track: s.name.clone(), label: Some(s.label.clone()), clip: clips.then(|| "first".into()), mix: None })
        .collect()
}
impl Session {
    fn goal_where(&self) -> String {
        self.0.state.borrow().project.clone().unwrap_or(UNSAVED.into())
    }
    fn persist_goal(&self, state: &GoalState) {
        if let Some(store) = self.0.options.goals.clone() {
            let kept = state.clone();
            let this = self.clone();
            self.enqueue(async move { store.save(&this.goal_where(), &kept).await });
        }
    }
    fn goal_event(
        &self,
        next: GoalPhase,
        state: Option<&GoalState>,
        text: Option<&str>,
        started: i64,
        e: Option<&Evolution>,
        full: &IndexMap<String, f64>,
        screening: bool,
    ) {
        let leader = e.and_then(|e| reported(e, full, screening));
        let status = GoalStatus {
            state: next,
            goal: state.map(|s| s.goal.clone()).unwrap_or_else(|| text.unwrap_or("").into()),
            generation: state.map(|s| s.generation).unwrap_or(0),
            rendered: state.map(|s| s.rendered).unwrap_or(0),
            trend: state.map(|s| s.trend.iter().skip(s.trend.len().saturating_sub(60)).copied().map(round).collect()).unwrap_or_default(),
            best: leader.as_ref().and_then(|l| l.score.map(|score| Best { label: l.label.clone(), score: round(score) })),
            leader: leader.as_ref().filter(|l| l.score.is_some()).map(|l| format!("{} · {}", l.label, l.chain)),
            first: state.and_then(|s| s.first).map(round),
            idea: state.and_then(|s| s.idea.clone()).filter(|s| !s.is_empty()),
            elapsed_ms: now() - started,
            candidates: e.map(|e| e.slots.len()).or_else(|| state.map(|s| s.slots.len())).unwrap_or(0) as u32,
            best_track: state.and_then(|s| s.best_track.clone()).filter(|s| !s.is_empty()),
            why: state.and_then(|s| s.why.clone()).filter(|s| !s.is_empty()),
        };
        self.0.state.borrow_mut().goal_status = Some(status.clone());
        self.emit(status.into());
    }
    async fn goal_ask(
        &self,
        op: &Rc<Operation>,
        prompt: &str,
        usage: &mut Usage,
        said: &Rc<RefCell<String>>,
    ) -> Result<TurnResult, RuntimeError> {
        let snapshot = self.observe(op, None, true).await?;
        self.assert_current(op)?;
        op.phase.set(Phase::Inference);
        said.borrow_mut().clear();
        let result = self.ask(op, prompt, &snapshot.context, Some(said.clone()), vec![], "").await?;
        add_usage(usage, result.usage.as_ref());
        Ok(result)
    }
    pub(super) async fn run_goal(&self, op: Rc<Operation>, text: Option<String>) -> Result<Option<TurnResult>, RuntimeError> {
        let integration =
            self.0.state.borrow().integration.clone().filter(|i| i.has_goal()).ok_or_else(|| {
                KumiError::new(FailureKind::Request, "Goals need Live connected, with the Ableton bridge 1.0.49 or later.")
            })?;
        let mut state =
            if text.is_some() { None } else { self.0.state.borrow().goal_state.clone().filter(|s| s.status == GoalRun::Paused) };
        if text.is_none() && state.is_none() {
            if let Some(store) = &self.0.options.goals {
                state = store.load(&self.goal_where()).await?;
            }
        }
        if text.is_none() && state.is_none() {
            return Err(KumiError::new(
                FailureKind::Request,
                "There's no goal to pick up. Say what to reach, such as: /goal make my pad sound like ~/ref.wav",
            )
            .into());
        }
        if text.is_none() && state.as_ref().is_some_and(|s| s.status == GoalRun::Done) {
            return Err(KumiError::new(
                FailureKind::Request,
                format!(
                    "That goal is done ({}). Start another with /goal and what to reach.",
                    state.as_ref().unwrap().why.as_deref().unwrap_or("finished")
                ),
            )
            .into());
        }
        // Work toward a goal goes on with nobody to ask, so it makes no techniques.
        if let Some(l) = &self.0.learned {
            l.drafts.turn_started(text.as_deref().unwrap_or_default(), false);
        }
        if let Some(taste) = &self.0.taste {
            taste.turn_started("", false);
        }
        let budget = self.0.options.goal_budget.unwrap_or(GOAL_BUDGET);
        op.extend(budget.ms + 15 * 60_000);
        op.steady.set(true);
        op.progress(None);
        let mut usage = Usage::default();
        let said = Rc::new(RefCell::new(String::new()));
        let started = now() - state.as_ref().map(|s| s.elapsed_ms).unwrap_or(0);
        let mut full = IndexMap::new();
        let mut screening = false;
        let mut last_confirm = None;
        self.goal_event(GoalPhase::Starting, state.as_ref(), text.as_deref(), started, None, &full, screening);
        let request;
        if state.is_none() {
            self.0.state.borrow_mut().heard_last = None;
            let brief = playbook_brief(
                &self.playbook_serial(|s| async move { s.list().await }.boxed_local()).await.unwrap_or_default(),
                text.as_ref().unwrap(),
                5,
            );
            let mut prompt = goal_setup(text.as_ref().unwrap());
            if !brief.is_empty() {
                prompt.push_str("\n\n");
                prompt.push_str(&brief);
            }
            let mut setup = self.goal_ask(&op, &prompt, &mut usage, &said).await?;
            if setup.stop_reason != StopReason::Completed || op.signal.is_cancelled() {
                setup.usage = Some(usage);
                return Ok(Some(setup));
            }
            let heard = self.0.state.borrow().heard_last.clone();
            if !heard.as_ref().and_then(|e| e.request.as_ref()).is_some_and(|r| r.reference.as_ref().is_some_and(|s| !s.is_empty())) {
                let status = {
                    let mut s = self.0.state.borrow_mut();
                    let status = s.goal_status.as_mut().unwrap();
                    status.state = GoalPhase::Done;
                    status.why = Some("no reference to reach, so it was done as a regular request".into());
                    status.clone()
                };
                self.emit(status.into());
                self.notice("A goal searches toward something: with no reference, Kumi did this as a regular request. To search, give /goal a reference too (an audio file, a clip in the Set, or a video).");
                return Ok(Some(TurnResult { stop_reason: StopReason::Completed, usage: Some(usage) }));
            }
            let heard = heard.unwrap();
            request = heard.request.unwrap();
            self.0.state.borrow_mut().goal_reference = heard.reference;
            let mut kept_request = request.clone();
            kept_request.candidates.clear();
            state = Some(GoalState {
                version: 1,
                goal: text.clone().unwrap(),
                request: kept_request,
                clips: request.candidates.iter().any(|c| c.clip.as_ref().is_some_and(|s| !s.is_empty())).then_some(true),
                slots: vec![],
                generation: 0,
                rendered: 0,
                trend: vec![],
                elapsed_ms: 0,
                status: GoalRun::Running,
                first: heard.best.map(|b| b.score),
                idea: None,
                best_track: None,
                why: None,
                lesson: None,
            });
        } else {
            let kept = state.as_mut().unwrap();
            let mut given = kept.request.clone();
            given.candidates = kept
                .slots
                .iter()
                .map(|s| AuditionCandidate {
                    track: s.name.clone(),
                    label: Some(s.label.clone()),
                    clip: (kept.clips == Some(true)).then(|| "first".into()),
                    mix: None,
                })
                .collect();
            request = given;
            kept.status = GoalRun::Running;
        }
        let mut state = state.unwrap();
        self.0.state.borrow_mut().goal_op = Some(op.clone());
        let mut rig = match integration.goal(&request, op.signal.clone()).await? {
            Ok(rig) => rig,
            Err(why) => {
                self.notice(format!("The goal couldn't start its search: {why}"));
                state.status = GoalRun::Paused;
                self.persist_goal(&state);
                return Ok(Some(TurnResult { stop_reason: StopReason::Completed, usage: Some(usage) }));
            }
        };
        let mut evolution = self.evolution();
        for slot in rig.slots() {
            join(&mut evolution, slot, &state);
        }
        evolution.generation = state.generation;
        evolution.rendered = state.rendered;
        evolution.trend = state.trend.clone();
        let mut gaps = vec![];
        let mut last_leap = evolution.generation;
        let mut silent_runs = 0;
        let mut open = true;
        let mut closed_notes = vec![];
        op.linger.set(180_000);
        let searching = async {
            sync(&mut state, &evolution, started);
            self.persist_goal(&state);
            self.goal_event(GoalPhase::Running, Some(&state), None, started, Some(&evolution), &full, screening);

            while !op.signal.is_cancelled() {
                let reached = if screening {
                    reported(&evolution, &full, screening).and_then(|s| s.score).unwrap_or(0.)
                } else {
                    evolution.best().unwrap_or(0.)
                };

                if reached >= budget.target {
                    state.status = GoalRun::Done;
                    state.why = Some(format!("reached {}%", to_string(round(reached))));
                    break;
                }
                screening = rig.screens();

                if screening && evolution.generation > 0 && evolution.generation % 4 == 0 && last_confirm != Some(evolution.generation) {
                    last_confirm = Some(evolution.generation);
                    let elites = evolution
                        .slots
                        .iter()
                        .filter(|s| s.score.is_some())
                        .map(|s| GenerationTrial { slot: s.name.clone(), knobs: s.knobs.clone(), values: s.elite.clone(), fresh: None })
                        .collect::<Vec<_>>();

                    let checked = rig.generation(&elites, op.signal.clone(), Some(GenerationOptions { screen: Some(false) })).await?;

                    // Stable slot order preserves source Map ties in the displayed leader.
                    for slot in &evolution.slots {
                        if let Some(score) = checked.scores.get(&slot.name) {
                            full.insert(slot.name.clone(), *score);
                        }
                    }
                    evolution.rendered += checked.scores.len() as u32;
                    sync(&mut state, &evolution, started);
                    self.goal_event(GoalPhase::Running, Some(&state), None, started, Some(&evolution), &full, screening);
                    continue;
                }
                if now() - started >= budget.ms as i64 {
                    state.status = GoalRun::Done;
                    state.why = Some("the safety cap on its time".into());
                    break;
                }
                let trials = evolution.propose();
                let result = rig
                    .generation(&generation_trials(&evolution, &trials), op.signal.clone(), Some(GenerationOptions { screen: Some(screening) }))
                    .await?;

                evolution.scored(&trials, &result.scores);
                silent_runs = if result.scores.is_empty() { silent_runs + 1 } else { 0 };

                if silent_runs >= 2 {
                    state.status = GoalRun::Paused;
                    state.why = Some("nothing came through the last renders; check the candidates play at that spot".into());
                    break;
                }
                for (slot, keys) in &result.frozen {
                    evolution.freeze(slot, keys);
                }
                if state.first.is_none() {
                    state.first = evolution.best();
                }
                let leader = evolution.leader().cloned();
                if let Some(found) = leader.as_ref().and_then(|s| result.gaps.get(&s.name)) {
                    gaps = found.clone();
                }
                sync(&mut state, &evolution, started);
                self.persist_goal(&state);
                self.goal_event(GoalPhase::Running, Some(&state), None, started, Some(&evolution), &full, screening);

                op.progress(Some(&KernelEvent::ToolEnd { id: String::new(), name: String::new(), is_error: false, elapsed_ms: 0 }));

                let stalled = evolution.stalled_for() >= budget.stall_generations;
                let structural = leader.as_ref().and_then(|s| result.structural.get(&s.name));
                let since = evolution.generation - last_leap;

                if since >= budget.leap_every || (stalled && since >= budget.stall_generations) || (structural.is_some() && since >= 2) {
                    last_leap = evolution.generation;
                    self.0.state.borrow_mut().heard_last = None;
                    let closing = rig.close().await.unwrap_or_default();
                    open = false;
                    closed_notes.extend(closing.clone());

                    if closing.iter().any(|s| s.contains("Main may still be silent")) {
                        state.status = GoalRun::Paused;
                        state.why = Some("Main couldn't be put back; set it in Live, then /goal carries on".into());
                        break;
                    }
                    let best = leader.as_ref().and_then(|l| l.score.map(|score| Best { label: l.label.clone(), score: round(score) }));
                    let structural = structural.cloned();

                    let leap = self
                        .goal_ask(
                            &op,
                            &goal_leap(&state, best.as_ref(), &gaps.iter().take(3).cloned().collect::<Vec<_>>(), stalled, structural.as_ref()),
                            &mut usage,
                            &said,
                        )
                        .await?;

                    if leap.stop_reason == StopReason::Cancelled || op.signal.is_cancelled() {
                        break;
                    }
                    let words = said.borrow().clone();
                    let idea=regex::Regex::new(r"(?i-u:Tried:)[\t\n\x0b\x0c\r \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]*([^\r\n\u{2028}\u{2029}]+)").unwrap().captures(&words).map(|m|m[1].to_owned()).or_else(||trim(&words).split('\n').find(|s|!s.is_empty()).map(str::to_owned));

                    if let Some(idea) = idea {
                        let clean = idea.replace(['*', '_', '`'], "");
                        let clean = head(trim(&clean), 200);
                        if !clean.is_empty() {
                            state.idea = Some(clean);
                        }
                    }
                    let offered =
                        self.0.state.borrow().heard_last.as_ref().and_then(|e| e.request.clone()).map(|r| r.candidates).unwrap_or_default();
                    let mut again = request.clone();
                    again.candidates = candidates(&evolution, state.clips == Some(true));

                    again.candidates.extend(offered.into_iter().map(|mut c| {
                        if state.clips == Some(true) && c.clip.as_ref().is_none_or(|s| s.is_empty()) {
                            c.clip = Some("first".into());
                        }
                        c
                    }));

                    rig = match integration.goal(&again, op.signal.clone()).await? {
                        Ok(rig) => rig,
                        Err(why) => {
                            state.status = GoalRun::Paused;
                            state.why = Some(head(&format!("the search couldn't start again after the model's idea: {why}"), 200));
                            break;
                        }
                    };
                    open = true;

                    for slot in rig.slots() {
                        if !evolution.slots.iter().any(|s| s.name == slot.name) {
                            join(&mut evolution, slot, &state);
                        }
                    }
                    sync(&mut state, &evolution, started);
                    self.persist_goal(&state);
                    self.goal_event(GoalPhase::Running, Some(&state), None, started, Some(&evolution), &full, screening);
                }
            }
            Ok::<(), RuntimeError>(())
        }.await;

        if let Err(error) = searching {
            if !op.signal.is_cancelled() {
                state.status = GoalRun::Paused;
                state.why = Some(head(&error.message(), 200));
            }
        }
        let cleanup = abort::timeout(150_000);
        if op.signal.is_cancelled() {
            let stopped = self.0.state.borrow().goal_stopped;
            state.status = if stopped { GoalRun::Done } else { GoalRun::Paused };
            state.why = Some(if stopped { "stopped" } else { "paused" }.into());
        }
        let leader = reported(&evolution, &full, screening);
        let mut notes = closed_notes;
        if open {
            notes.extend(
                rig.close()
                    .await
                    .unwrap_or_else(|_| vec!["Kumi couldn't remove its render tracks; delete the “Kumi · render” tracks by hand.".into()]),
            );
        }
        if state.status == GoalRun::Done {
            let mut ranked = evolution.slots.iter().filter(|s| s.score.is_some()).collect::<Vec<_>>();
            ranked.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
            let top = ranked.iter().take(2).map(|s| s.name.clone()).collect::<Vec<_>>();
            notes.extend(rig.tidy(&top, cleanup.clone()).await.unwrap_or_default());
        }
        if let Some(leader) = leader.as_ref().filter(|l| l.score.is_some()) {
            let kept = rig.keep_best(&leader.name, &leader.knobs, &leader.elite, cleanup).await.unwrap_or_default();
            if kept.starts_with("Kumi · Goal best") {
                state.best_track = Some(kept);
            }
        }
        sync(&mut state, &evolution, started);
        self.persist_goal(&state);
        self.goal_event(state.status.into(), Some(&state), None, started, Some(&evolution), &full, screening);
        {
            let mut s = self.0.state.borrow_mut();
            s.goal_op = None;
            s.goal_stopped = false;
        }
        let score = leader
            .as_ref()
            .and_then(|l| {
                l.score.map(|score| {
                    format!(
                        "{}{}% ({})",
                        state
                            .first
                            .filter(|f| round(*f) != round(score))
                            .map(|f| format!("{}% → ", to_string(round(f))))
                            .unwrap_or_default(),
                        to_string(round(score)),
                        l.label
                    )
                })
            })
            .unwrap_or("no score yet".into());
        self.notice(format!(
            "Goal {}: {score} · {} generations · {} candidates{}{}.{}",
            if state.status == GoalRun::Paused { "paused" } else { "done" },
            state.generation,
            state.rendered,
            state
                .best_track
                .as_ref()
                .map(|track| format!(
                    " · the best is on “{track}”{}",
                    if state.status == GoalRun::Done { " (say if you want it on one of your tracks)" } else { "" }
                ))
                .unwrap_or_default(),
            if state.status == GoalRun::Paused { " · /goal carries on" } else { "" },
            if notes.is_empty() { String::new() } else { format!(" {}", notes.join(" ")) }
        ));
        let reference = self.0.state.borrow().goal_reference.clone();
        let best = leader.as_ref().and_then(|l| l.score.map(|score| GoalLeader { label: &l.label, chain: &l.chain, score }));
        if let Some(mut lesson) = lesson_from_goal(&state.goal, reference.as_deref(), best, state.first, &state.trend, now() as f64) {
            if let Some(id) = &state.lesson {
                lesson.id = id.clone();
            }
            state.lesson = Some(lesson.id.clone());
            self.persist_goal(&state);
            let this = self.clone();
            let work = self.playbook_serial(move |store| {
                async move {
                    let existed = store.put(&lesson).await?;
                    this.emit(SessionEvent::Lesson {
                        action: if existed { LessonAction::Updated } else { LessonAction::Learned },
                        id: lesson.id.clone(),
                        line: lesson_line(&lesson),
                    });
                    Ok(())
                }
                .boxed_local()
            });
            tokio::task::spawn_local(work);
        }
        self.0.state.borrow_mut().goal_state = Some(state);
        Ok(Some(TurnResult {
            stop_reason: if op.signal.is_cancelled() { StopReason::Cancelled } else { StopReason::Completed },
            usage: Some(usage),
        }))
    }
    pub(super) async fn stop_goal_inner(&self) -> Result<bool, RuntimeError> {
        let active = {
            let s = self.0.state.borrow();
            s.goal_op.clone().filter(|op| s.active.as_ref().is_some_and(|a| a.id == op.id))
        };
        if let Some(op) = active {
            self.0.state.borrow_mut().goal_stopped = true;
            op.signal.cancel();
            op.done.clone().await?;
            return Ok(true);
        }
        let mut kept = self.0.state.borrow().goal_state.clone().filter(|s| s.status == GoalRun::Paused);
        let place = self.goal_where();
        if kept.is_none() {
            if let Some(store) = &self.0.options.goals {
                kept = store.load(&place).await?;
            }
        }
        let Some(mut kept) = kept.filter(|s| s.status != GoalRun::Done) else {
            return Ok(false);
        };
        kept.status = GoalRun::Done;
        kept.why = Some("stopped".into());
        self.0.state.borrow_mut().goal_state = Some(kept.clone());
        if let Some(store) = &self.0.options.goals {
            let _ = store.save(&place, &kept).await;
        }
        let status = {
            let mut s = self.0.state.borrow_mut();
            s.goal_status.as_mut().map(|status| {
                status.state = GoalPhase::Done;
                status.why = Some("stopped".into());
                status.clone()
            })
        };
        if let Some(status) = status {
            self.emit(status.into());
        }
        Ok(true)
    }
}
