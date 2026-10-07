//! /goal as a goal mode: one standing objective Kumi keeps working toward, checked after every turn against
//! evidence (the judge's checklist first, else a forced self-audit), bounded by turns, time and no-progress, shown as
//! it goes, paused on Esc without losing anything, and kept on disk for after a restart.
use super::matching::add_usage;
use super::*;
use crate::core::goal_mode::{
    after_turn, measured_check, objective_audit, objective_first, objective_next, read_audit, Check, Objective, ObjectiveState, Verdict,
    OBJECTIVE_BUDGET,
};

/// A goal run's hold on the session's objective flags, released however the run ends.
struct ObjectiveHold {
    session: Session,
    op: u64,
}
impl ObjectiveHold {
    fn begin(session: &Session, op: &Rc<Operation>) -> Self {
        let mut s = session.0.state.borrow_mut();
        s.objective_op = Some(op.clone());
        s.objective_stopped = false;
        Self { session: session.clone(), op: op.id }
    }
}
impl Drop for ObjectiveHold {
    fn drop(&mut self) {
        if let Ok(mut s) = self.session.0.state.try_borrow_mut() {
            if s.objective_op.as_ref().is_some_and(|op| op.id == self.op) {
                s.objective_op = None;
            }
        }
    }
}

impl Session {
    fn objective_place(&self) -> String {
        self.0.state.borrow().project.clone().unwrap_or(UNSAVED.into())
    }
    /// The objective for this Set: in memory, else from disk.
    pub(super) async fn objective_kept(&self) -> Option<Objective> {
        let place = self.objective_place();
        let held = self.0.state.borrow().objective.as_ref().filter(|(at, _)| *at == place).map(|(_, objective)| objective.clone());
        if held.is_some() {
            return held;
        }
        let store = self.0.options.objectives.clone()?;
        store.load(&place).await.ok().flatten()
    }
    fn persist_objective(&self, objective: &Objective) {
        let place = self.objective_place();
        self.0.state.borrow_mut().objective = Some((place.clone(), objective.clone()));
        if let Some(store) = self.0.options.objectives.clone() {
            let kept = objective.clone();
            self.enqueue(async move { store.save(&place, &kept).await });
        }
    }
    pub(super) fn emit_objective(&self, objective: &Objective, elapsed_ms: i64) {
        self.emit(SessionEvent::Objective(objective.status(elapsed_ms)));
    }
    /// One turn toward the objective: the model works, and the loop runs inside when it starts a judged run.
    async fn objective_turn(
        &self,
        op: &Rc<Operation>,
        prompt: &str,
        objective: &str,
        usage: &mut Usage,
    ) -> Result<TurnResult, RuntimeError> {
        {
            let mut s = self.0.state.borrow_mut();
            s.looping = None;
            s.turn_request = Some(objective.to_owned());
        }
        let snapshot = self.observe(op, None, true).await?;
        self.assert_current(op)?;
        op.phase.set(Phase::Inference);
        let result = self.ask(op, prompt, &snapshot.context, None, vec![], "").await?;
        add_usage(usage, result.usage.as_ref());
        let looping = self.0.state.borrow().looping.clone();
        let result = match looping {
            Some(looping) if result.stop_reason == StopReason::Completed => {
                let looped = self.run_loop(op, &looping, result).await;
                self.0.state.borrow_mut().looping = None;
                looped?
            }
            _ => result,
        };
        Ok(result)
    }
    /// The check after a turn: the judge's numbers when this turn measured something, else the model's own audit, which
    /// must end in complete, blocked or continue.
    async fn objective_check(
        &self,
        op: &Rc<Operation>,
        objective: &str,
        judged_before: &Option<crate::listening::round::Round>,
        usage: &mut Usage,
    ) -> Result<Check, RuntimeError> {
        let judged = self.0.state.borrow().judged_last.clone();
        if judged != *judged_before {
            if let Some(check) = measured_check(judged.as_ref()) {
                return Ok(check);
            }
        }
        let said = Rc::new(RefCell::new(String::new()));
        let snapshot = self.observe(op, None, true).await?;
        self.assert_current(op)?;
        op.phase.set(Phase::Inference);
        let result = self.ask(op, &objective_audit(objective), &snapshot.context, Some(said.clone()), vec![], "").await?;
        add_usage(usage, result.usage.as_ref());
        let answer = said.borrow().clone();
        Ok(read_audit(&answer))
    }
    pub(super) async fn run_objective(
        &self,
        op: Rc<Operation>,
        mut objective: Objective,
        fresh: bool,
    ) -> Result<Option<TurnResult>, RuntimeError> {
        let _hold = ObjectiveHold::begin(self, &op);
        let started = now() - objective.elapsed_ms;
        op.extend(objective.budget.ms.max(0) as u64 + 15 * 60_000);
        op.steady.set(true);
        op.progress(None);
        let mut usage = Usage::default();
        objective.state = ObjectiveState::Running;
        self.persist_objective(&objective);
        self.emit_objective(&objective, now() - started);
        let mut prompt = if fresh { objective_first(&objective.objective) } else { objective_next(&objective) };
        let mut stop_reason = StopReason::Completed;
        loop {
            let applied = self.0.state.borrow().applied;
            let judged_before = self.0.state.borrow().judged_last.clone();
            let turn = self.objective_turn(&op, &prompt, &objective.objective, &mut usage).await;
            let turn = match turn {
                // A turn that ran out of steps did work too: the check says where it got.
                Ok(result) if result.stop_reason != StopReason::Cancelled && !op.signal.is_cancelled() => Ok(result),
                Ok(result) => Err(result.stop_reason),
                Err(error) if op.signal.is_cancelled() => {
                    let _ = error;
                    Err(StopReason::Cancelled)
                }
                Err(error) => {
                    // An error the producer must fix stops it, named; others pause it.
                    let auth = error.kumi().is_some_and(|kumi| kumi.kind == FailureKind::Auth);
                    let check = Check {
                        verdict: if auth { Verdict::Blocked } else { Verdict::Continue },
                        reason: head(&error.message(), 200),
                        next: auth.then(|| "sign in again (/login), then /goal resume".into()),
                        measured: false,
                    };
                    objective.state = ObjectiveState::Paused;
                    objective.elapsed_ms = now() - started;
                    objective.last = Some(check);
                    self.persist_objective(&objective);
                    self.emit_objective(&objective, now() - started);
                    return Err(error);
                }
            };
            if let Err(reason) = turn {
                // Esc, or the producer's own message taking over: paused, nothing lost.
                stop_reason = reason;
                break;
            }
            let check = match self.objective_check(&op, &objective.objective, &judged_before, &mut usage).await {
                Ok(check) => check,
                Err(_) if op.signal.is_cancelled() => {
                    stop_reason = StopReason::Cancelled;
                    break;
                }
                Err(error) => return Err(error),
            };
            let changed = self.0.state.borrow().applied > applied;
            let check = after_turn(&mut objective, check, changed, now() - started);
            self.persist_objective(&objective);
            self.emit_objective(&objective, now() - started);
            if check.verdict != Verdict::Continue {
                break;
            }
            prompt = objective_next(&objective);
        }
        if stop_reason != StopReason::Completed {
            // Esc pauses it, /goal stop ends it; the last check stays as it was.
            let stopped = self.0.state.borrow().objective_stopped;
            objective.state = if stopped { ObjectiveState::Done } else { ObjectiveState::Paused };
            objective.elapsed_ms = now() - started;
            self.persist_objective(&objective);
            self.emit_objective(&objective, now() - started);
        }
        let check = objective.last.clone();
        let word = match (objective.state, check.as_ref().map(|check| check.verdict)) {
            (ObjectiveState::Done, Some(Verdict::Complete)) => "met",
            (ObjectiveState::Done, _) => "stopped",
            _ if stop_reason != StopReason::Completed => "paused",
            (_, Some(Verdict::Blocked)) => "blocked",
            (_, Some(Verdict::Budget)) => "out of budget",
            (_, Some(Verdict::Stuck)) => "stuck",
            _ => "paused",
        };
        self.notice(format!(
            "Goal {word} after {} turn{}{}{}{}",
            objective.turns,
            if objective.turns == 1 { "" } else { "s" },
            check.as_ref().map(|check| format!(": {}", check.reason)).unwrap_or_default(),
            check.as_ref().and_then(|check| check.next.as_ref()).map(|next| format!(" · next: {next}")).unwrap_or_default(),
            if objective.state == ObjectiveState::Paused { " · /goal resume carries on" } else { "" }
        ));
        Ok(Some(TurnResult { stop_reason, usage: Some(usage) }))
    }
    /// /goal's words: a new objective, or resume, edit <words>, pause, clear.
    pub(super) async fn objective_command(&self, text: Option<String>) -> Result<GoalCommand, RuntimeError> {
        let budget = self.0.options.objective_budget.unwrap_or(OBJECTIVE_BUDGET);
        let words = text.as_deref().map(trim).unwrap_or("");
        let lower = words.to_lowercase();
        match lower.as_str() {
            "" | "status" => Ok(GoalCommand::Show(self.objective_kept().await)),
            "resume" | "carry on" | "continue" => {
                match self.objective_kept().await.filter(|objective| objective.state != ObjectiveState::Done) {
                    Some(mut objective) => {
                        // A goal stopped at its budget gets a fresh one.
                        if objective.turns >= objective.budget.turns {
                            objective.budget.turns = objective.turns + budget.turns;
                        }
                        if objective.elapsed_ms >= objective.budget.ms {
                            objective.budget.ms = objective.elapsed_ms + budget.ms;
                        }
                        objective.idle = 0;
                        Ok(GoalCommand::Run(objective, false))
                    }
                    None => {
                        Err(KumiError::new(FailureKind::Request, "There's no goal to resume. Start one: /goal and what to reach.").into())
                    }
                }
            }
            "clear" | "stop" | "end" => {
                self.stop_objective().await?;
                Ok(GoalCommand::Show(None))
            }
            _ if lower.starts_with("edit ") => match self.objective_kept().await {
                Some(mut objective) => {
                    objective.objective = trim(&words[5..]).to_owned();
                    objective.last = None;
                    objective.idle = 0;
                    if objective.state == ObjectiveState::Done {
                        objective.state = ObjectiveState::Paused;
                    }
                    self.persist_objective(&objective);
                    self.emit_objective(&objective, objective.elapsed_ms);
                    Ok(GoalCommand::Show(Some(objective)))
                }
                None => Err(KumiError::new(FailureKind::Request, "There's no goal to edit. Start one: /goal and what to reach.").into()),
            },
            _ => Ok(GoalCommand::Run(Objective::new(words, budget), true)),
        }
    }
    /// Ends the objective: the running one stops (done), a paused one is cleared.
    pub(super) async fn stop_objective(&self) -> Result<bool, RuntimeError> {
        let active = {
            let s = self.0.state.borrow();
            s.objective_op.clone().filter(|op| s.active.as_ref().is_some_and(|a| a.id == op.id))
        };
        if let Some(op) = active {
            self.0.state.borrow_mut().objective_stopped = true;
            op.signal.cancel();
            op.done.clone().await?;
            return Ok(true);
        }
        let Some(mut objective) = self.objective_kept().await.filter(|objective| objective.state != ObjectiveState::Done) else {
            return Ok(false);
        };
        objective.state = ObjectiveState::Done;
        self.persist_objective(&objective);
        self.emit_objective(&objective, objective.elapsed_ms);
        Ok(true)
    }
}

/// What /goal's words came to.
pub(super) enum GoalCommand {
    /// Just its status (none: no goal here).
    Show(Option<Objective>),
    /// Work on it: a new one (true) or one picked up (false).
    Run(Objective, bool),
}
