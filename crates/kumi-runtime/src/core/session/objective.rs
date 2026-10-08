//! /goal as a goal mode: one standing objective Kumi keeps working toward, checked after every turn against
//! evidence (the judge's checklist first, else a forced self-audit), bounded by turns, time and no-progress, shown as
//! it goes, paused on Esc without losing anything, and kept on disk for after a restart.
use super::matching::add_usage;
use super::*;
use crate::core::goal_mode::{
    after_turn, audit_against, command_word, error_check, gap_closed, measured_check, objective_audit, objective_first, objective_next,
    read_audit, Check, Objective, ObjectiveState, Verdict, OBJECTIVE_BUDGET,
};

/// A goal run's hold on the session's objective flags, released however the run ends.
struct ObjectiveHold {
    session: Session,
    op: u64,
}
impl ObjectiveHold {
    fn begin(session: &Session, op: &Rc<Operation>, until: i64) -> Self {
        let mut s = session.0.state.borrow_mut();
        s.objective_op = Some(op.clone());
        s.objective_stopped = false;
        s.objective_until = Some(until);
        Self { session: session.clone(), op: op.id }
    }
}
impl Drop for ObjectiveHold {
    fn drop(&mut self) {
        if let Ok(mut s) = self.session.0.state.try_borrow_mut() {
            if s.objective_op.as_ref().is_some_and(|op| op.id == self.op) {
                s.objective_op = None;
                s.objective_until = None;
            }
            s.checking = false;
        }
    }
}

/// Where a running goal is kept: the Set it began in, pinned for the run. An unsaved Set's goal moves to the Set's own
/// place when it's first saved (the same Set, by Live's identity, or the same conversation without one), never to
/// another Set opened meanwhile.
struct Place {
    at: String,
    conversation: String,
}

/// An unsaved Set's own place: Live's identity for it.
fn unsaved_place(identity: &str) -> String {
    format!("{UNSAVED}:{identity}")
}

impl Session {
    /// Where this Set's goal is kept: its project; for an unsaved Set, Live's identity for it (which holds while the Set
    /// stays open, through a Kumi restart and a new conversation); without one, the place every unsaved Set shares,
    /// where each goal is its own conversation's.
    fn objective_place(&self) -> String {
        let s = self.0.state.borrow();
        match (&s.project, &s.set_identity) {
            (Some(project), _) => project.clone(),
            (None, Some(identity)) => unsaved_place(identity),
            (None, None) => UNSAVED.into(),
        }
    }
    fn objective_running(&self) -> bool {
        let s = self.0.state.borrow();
        s.objective_op.as_ref().is_some_and(|op| s.active.as_ref().is_some_and(|active| active.id == op.id))
    }
    /// The objective for this Set: in memory, else from disk. In the place unsaved Sets share (Live's identity for the
    /// Set unknown), a goal is only the one set in its own conversation. One kept as running with nothing running it
    /// (Kumi closed, or crashed, mid-turn) is paused.
    pub(super) async fn objective_kept(&self) -> Option<Objective> {
        let (place, conversation) = (self.objective_place(), self.0.state.borrow().conversation_id.clone());
        let held = self.0.state.borrow().objective.as_ref().filter(|(at, _)| *at == place).map(|(_, objective)| objective.clone());
        let mut kept = match held {
            Some(held) => Some(held),
            None => self.0.options.objectives.clone()?.load(&place).await.ok().flatten(),
        }
        .filter(|objective| place != UNSAVED || objective.conversation.as_deref() == Some(conversation.as_str()));
        if let Some(objective) = kept.as_mut().filter(|objective| objective.state == ObjectiveState::Running) {
            if !self.objective_running() {
                objective.state = ObjectiveState::Paused;
            }
        }
        kept
    }
    fn persist_objective(&self, place: &str, objective: &Objective) {
        self.0.state.borrow_mut().objective = Some((place.to_owned(), objective.clone()));
        if let Some(store) = self.0.options.objectives.clone() {
            let (place, kept) = (place.to_owned(), objective.clone());
            self.enqueue(async move { store.save(&place, &kept).await });
        }
    }
    /// Saves the running goal where it belongs, following its Set's first save.
    fn persist_running(&self, place: &mut Place, objective: &Objective) {
        let saved = {
            let s = self.0.state.borrow();
            // Still the Set it began in: the same Live identity, or (without one) the same conversation.
            let same = match &s.set_identity {
                _ if place.at == UNSAVED => s.conversation_id == place.conversation,
                Some(identity) => place.at == unsaved_place(identity),
                None => false,
            };
            s.project.clone().filter(|_| same)
        };
        if let Some(to) = saved {
            if let Some(store) = self.0.options.objectives.clone() {
                let from = place.at.clone();
                self.enqueue(async move { store.clear(&from).await });
            }
            place.at = to;
        }
        self.persist_objective(&place.at, objective);
    }
    /// The unsaved Set's goal, moved to the Set's own place when it's first saved. Only its own: a goal another unsaved
    /// Set left stays where it is.
    pub(super) fn move_unsaved_objective(&self, to: &str) {
        let (from, conversation) = {
            let mut s = self.0.state.borrow_mut();
            let from = s.set_identity.as_deref().map(unsaved_place).unwrap_or(UNSAVED.into());
            let conversation = s.conversation_id.clone();
            let shared = from == UNSAVED;
            if let Some((at, _)) = s
                .objective
                .as_mut()
                .filter(|(at, objective)| *at == from && (!shared || objective.conversation.as_ref() == Some(&conversation)))
            {
                *at = to.to_owned();
            }
            (from, conversation)
        };
        if let Some(store) = self.0.options.objectives.clone() {
            let to = to.to_owned();
            self.enqueue(async move {
                let shared = from == UNSAVED;
                if let Some(goal) = store.load(&from).await?.filter(|goal| !shared || goal.conversation.as_ref() == Some(&conversation)) {
                    store.save(&to, &goal).await?;
                    store.clear(&from).await?;
                }
                Ok(())
            });
        }
    }
    pub(super) fn emit_objective(&self, objective: &Objective, elapsed_ms: i64) {
        self.emit(SessionEvent::Objective(objective.status(elapsed_ms)));
    }
    /// One turn toward the objective: the model works, and the loop runs inside when it judges a change. The run's
    /// first turn is a new request (the judge starts afresh); later ones carry on.
    async fn objective_turn(
        &self,
        op: &Rc<Operation>,
        prompt: &str,
        objective: &str,
        first: bool,
        usage: &mut Usage,
    ) -> Result<TurnResult, RuntimeError> {
        {
            let mut s = self.0.state.borrow_mut();
            s.looping = None;
            s.judge_start = None;
            s.turn_request = Some(objective.to_owned());
            s.turn_steers = s.steers;
        }
        let snapshot = self.observe(op, None, !first).await?;
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
    /// must end in complete, blocked or continue. The producer's messages wait meanwhile: the check changes nothing.
    /// None when it was stopped (Esc, /goal stop): what it said by then isn't an answer.
    async fn objective_check(
        &self,
        op: &Rc<Operation>,
        objective: &str,
        (goal_rounds, rounds_before): (u64, u64),
        usage: &mut Usage,
    ) -> Result<Option<Check>, RuntimeError> {
        let (judged, rounds) = {
            let s = self.0.state.borrow();
            (s.judged_last.clone(), s.judged_rounds)
        };
        if rounds > rounds_before {
            if let Some(check) = measured_check(judged.as_ref()) {
                return Ok(Some(check));
            }
        }
        let said = Rc::new(RefCell::new(String::new()));
        let snapshot = self.observe(op, None, true).await?;
        self.assert_current(op)?;
        op.phase.set(Phase::Inference);
        self.0.state.borrow_mut().checking = true;
        let result = self.ask(op, &objective_audit(objective), &snapshot.context, Some(said.clone()), vec![], "").await;
        self.0.state.borrow_mut().checking = false;
        let result = result?;
        add_usage(usage, result.usage.as_ref());
        if result.stop_reason == StopReason::Cancelled || op.signal.is_cancelled() {
            return Ok(None);
        }
        let answer = said.borrow().clone();
        // The model's word doesn't outrank the judge's measure in this goal's run.
        Ok(Some(audit_against(read_audit(&answer), judged.as_ref().filter(|_| rounds > goal_rounds))))
    }
    /// A turn's or a check's error: the goal is kept paused (blocked when the producer must fix something first,
    /// named), never left running.
    fn objective_failed(&self, place: &mut Place, objective: &mut Objective, started: i64, error: &RuntimeError) {
        objective.state = ObjectiveState::Paused;
        objective.elapsed_ms = now() - started;
        objective.last = Some(error_check(error));
        self.persist_running(place, objective);
        self.emit_objective(objective, now() - started);
    }
    pub(super) async fn run_objective(
        &self,
        op: Rc<Operation>,
        mut objective: Objective,
        fresh: bool,
    ) -> Result<Option<TurnResult>, RuntimeError> {
        let started = now() - objective.elapsed_ms;
        let _hold = ObjectiveHold::begin(self, &op, started + objective.budget.ms);
        let mut place = Place { at: self.objective_place(), conversation: self.0.state.borrow().conversation_id.clone() };
        op.extend(objective.budget.ms.max(0) as u64 + 15 * 60_000);
        op.steady.set(true);
        op.progress(None);
        // Work toward a goal goes on with nobody to ask: it makes no techniques, and its turns aren't the producer's.
        if let Some(l) = &self.0.learned {
            l.drafts.turn_started(&objective.objective, false);
        }
        if let Some(taste) = &self.0.taste {
            taste.turn_started("", false);
        }
        let mut usage = Usage::default();
        objective.state = ObjectiveState::Running;
        objective.conversation = Some(place.conversation.clone());
        self.persist_running(&mut place, &objective);
        self.emit_objective(&objective, now() - started);
        let mut prompt = if fresh { objective_first(&objective.objective) } else { objective_next(&objective) };
        let mut stop_reason = StopReason::Completed;
        let mut stepped_in = false;
        let mut first = true;
        // The judge's rounds before this goal ran: one logged since is its own measure.
        let goal_rounds = self.0.state.borrow().judged_rounds;
        loop {
            let (applied, judged_before, rounds, closed, judged_changes, steers) = {
                let s = self.0.state.borrow();
                (s.applied, s.judged_last.clone(), s.judged_rounds, s.judged_closed, s.judged_changes, s.steers)
            };
            let turn = self.objective_turn(&op, &prompt, &objective.objective, first, &mut usage).await;
            first = false;
            let turn = match turn {
                // A turn that ran out of steps did work too: the check says where it got.
                Ok(result) if result.stop_reason != StopReason::Cancelled && !op.signal.is_cancelled() => Ok(result),
                Ok(result) => Err(result.stop_reason),
                Err(_) if op.signal.is_cancelled() => Err(StopReason::Cancelled),
                Err(error) => {
                    self.objective_failed(&mut place, &mut objective, started, &error);
                    return Err(error);
                }
            };
            if let Err(reason) = turn {
                // Esc, or the producer's own message taking over: paused, nothing lost.
                stop_reason = reason;
                break;
            }
            // The producer stepped in during the turn: their message was answered, and the goal waits for them.
            if self.0.state.borrow().steers > steers {
                objective.turns += 1;
                stepped_in = true;
                break;
            }
            let check = match self.objective_check(&op, &objective.objective, (goal_rounds, rounds), &mut usage).await {
                Ok(Some(check)) => check,
                // Stopped mid-check: no answer, no turn counted.
                Ok(None) => {
                    stop_reason = StopReason::Cancelled;
                    break;
                }
                Err(_) if op.signal.is_cancelled() => {
                    stop_reason = StopReason::Cancelled;
                    break;
                }
                Err(error) => {
                    self.objective_failed(&mut place, &mut objective, started, &error);
                    return Err(error);
                }
            };
            // Progress: the judge's gap closed by a step or more, by kept changes or between its readings before and
            // after the turn (measure, change, measure again); a change taken back closes nothing. Readings are
            // compared only when the turn logged a round (else the last one is the turn's start, compared with
            // itself). With nothing to compare and no change judged, a change in the Set.
            let progress = {
                let s = self.0.state.borrow();
                let standing = (s.judged_rounds > rounds).then(|| gap_closed(judged_before.as_ref(), s.judged_last.as_ref())).flatten();
                s.judged_closed - closed >= 1.
                    || standing.is_some_and(|closed| closed >= 1.)
                    || (standing.is_none() && s.judged_changes == judged_changes && s.applied > applied)
            };
            let check = after_turn(&mut objective, check, progress, now() - started);
            self.persist_running(&mut place, &objective);
            self.emit_objective(&objective, now() - started);
            if check.verdict != Verdict::Continue {
                break;
            }
            prompt = objective_next(&objective);
        }
        if stop_reason != StopReason::Completed || stepped_in {
            // Esc or the producer's message pauses it, /goal stop ends it; the last check stays as it was.
            let stopped = self.0.state.borrow().objective_stopped;
            objective.state = if stopped { ObjectiveState::Done } else { ObjectiveState::Paused };
            objective.elapsed_ms = now() - started;
            self.persist_running(&mut place, &objective);
            self.emit_objective(&objective, now() - started);
        }
        let check = objective.last.clone();
        let paused = stop_reason != StopReason::Completed || stepped_in;
        let word = match (objective.state, check.as_ref().map(|check| check.verdict)) {
            (ObjectiveState::Done, Some(Verdict::Complete)) => "met",
            (ObjectiveState::Done, _) => "stopped",
            _ if paused => "paused",
            (_, Some(Verdict::Blocked)) => "blocked",
            (_, Some(Verdict::Budget)) => "out of budget",
            (_, Some(Verdict::Stuck)) => "stuck",
            _ => "paused",
        };
        let said = if stepped_in && objective.state != ObjectiveState::Done {
            ": you stepped in".to_string()
        } else {
            format!(
                "{}{}",
                check.as_ref().map(|check| format!(": {}", check.reason)).unwrap_or_default(),
                check.as_ref().and_then(|check| check.next.as_ref()).map(|next| format!(" · next: {next}")).unwrap_or_default()
            )
        };
        self.notice(format!(
            "Goal {word} after {} turn{}{said}{}",
            objective.turns,
            if objective.turns == 1 { "" } else { "s" },
            if objective.state == ObjectiveState::Paused { " · /goal resume carries on" } else { "" }
        ));
        Ok(Some(TurnResult { stop_reason, usage: Some(usage) }))
    }
    /// /goal's words: a new objective, or status, resume, pause, edit <words>, new: <words>, stop. The subcommands are
    /// read in any case and with trailing punctuation; they never start a goal of their own.
    pub(super) async fn objective_command(&self, text: Option<String>) -> Result<GoalCommand, RuntimeError> {
        let budget = self.0.options.objective_budget.unwrap_or(OBJECTIVE_BUDGET);
        let words = text.as_deref().map(trim).unwrap_or("");
        let word = command_word(words);
        let (first, rest) = match words.split_once(char::is_whitespace) {
            Some((first, rest)) => (command_word(first), trim(rest)),
            None => (word.clone(), ""),
        };
        let pending = self.0.state.borrow_mut().pending_goal.take();
        match word.as_str() {
            "" | "status" | "show" => return Ok(GoalCommand::Show(self.objective_kept().await)),
            "resume" | "carry on" | "continue" | "go on" => return self.objective_resume(budget).await,
            // A running goal is paused with Esc (the app's /goal pause): here nothing runs.
            "pause" | "hold" => return Ok(GoalCommand::Say("There's no goal running to pause.".into())),
            "clear" | "stop" | "end" | "cancel" | "done" => {
                self.stop_objective().await?;
                return Ok(GoalCommand::Show(None));
            }
            "edit" | "new" => {
                let how = if word == "new" { "new:" } else { "edit" };
                return Err(KumiError::new(
                    FailureKind::Request,
                    format!("Say the goal's words after /goal {how}, such as /goal {how} master this to -9 LUFS."),
                )
                .into());
            }
            _ => {}
        }
        if first == "edit" {
            return match self.objective_kept().await {
                Some(mut objective) => {
                    objective.objective = rest.to_owned();
                    objective.last = None;
                    objective.idle = 0;
                    if objective.state == ObjectiveState::Done {
                        objective.state = ObjectiveState::Paused;
                    }
                    self.persist_objective(&self.objective_place(), &objective);
                    self.emit_objective(&objective, objective.elapsed_ms);
                    Ok(GoalCommand::Show(Some(objective)))
                }
                None => Err(KumiError::new(FailureKind::Request, "There's no goal to edit. Start one: /goal and what to reach.").into()),
            };
        }
        // `/goal new: <words>` replaces an unfinished goal at once ("new chords for the bridge" is a goal's words).
        if let Some(rest) = words.get(..4).filter(|start| start.eq_ignore_ascii_case("new:")).map(|_| trim(&words[4..])) {
            return Ok(GoalCommand::Run(Objective::new(rest, budget), true));
        }
        // Otherwise an unfinished goal is replaced only when the same words are sent again; the goal's own words carry
        // it on.
        if let Some(kept) = self.objective_kept().await.filter(|kept| kept.state != ObjectiveState::Done) {
            if command_word(&kept.objective) == command_word(words) {
                return self.objective_resume(budget).await;
            }
            if pending.as_deref() != Some(words) {
                self.0.state.borrow_mut().pending_goal = Some(words.to_owned());
                return Ok(GoalCommand::Say(format!(
                    "This Set has an unfinished goal: “{}” ({} turn{} so far). Send the same /goal again, or /goal new: and the words, to replace it; /goal resume carries it on.",
                    kept.objective,
                    kept.turns,
                    if kept.turns == 1 { "" } else { "s" }
                )));
            }
        }
        Ok(GoalCommand::Run(Objective::new(words, budget), true))
    }
    /// The kept goal picked up; one stopped at its budget gets a fresh one.
    async fn objective_resume(&self, budget: crate::core::goal_mode::ObjectiveBudget) -> Result<GoalCommand, RuntimeError> {
        match self.objective_kept().await.filter(|objective| objective.state != ObjectiveState::Done) {
            Some(mut objective) => {
                if objective.turns >= objective.budget.turns {
                    objective.budget.turns = objective.turns + budget.turns;
                }
                if objective.elapsed_ms >= objective.budget.ms {
                    objective.budget.ms = objective.elapsed_ms + budget.ms;
                }
                objective.idle = 0;
                Ok(GoalCommand::Run(objective, false))
            }
            None => Err(KumiError::new(FailureKind::Request, "There's no goal to resume. Start one: /goal and what to reach.").into()),
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
        self.persist_objective(&self.objective_place(), &objective);
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
    /// Nothing to run: what to tell the producer.
    Say(String),
}
