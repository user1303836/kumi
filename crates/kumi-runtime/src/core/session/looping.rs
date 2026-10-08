//! The loop's turns: after each answer, code decides whether the model goes on, with the judge's numbers.
use super::matching::add_usage;
use super::*;
use crate::core::loop_run::{LoopDecision, LoopRun, LoopState};

impl Session {
    pub(super) async fn run_loop(
        &self,
        op: &Rc<Operation>,
        run: &Rc<RefCell<LoopRun>>,
        first: TurnResult,
    ) -> Result<TurnResult, RuntimeError> {
        op.extend((run.borrow().budget().ms + 10 * 60_000).max(0) as u64);
        let mut ended = false;
        let looped = self.loop_rounds(op, run, first, &mut ended).await;
        // However it ends (Esc, the producer stepping in, an error), the app hears the loop isn't running anymore.
        if !ended {
            self.emit(SessionEvent::Loop(run.borrow().status(LoopState::Paused, None)));
        }
        looped
    }
    async fn loop_rounds(
        &self,
        op: &Rc<Operation>,
        run: &Rc<RefCell<LoopRun>>,
        first: TurnResult,
        ended: &mut bool,
    ) -> Result<TurnResult, RuntimeError> {
        let mut result = first;
        let mut usage = result.usage.clone().unwrap_or_default();
        let mut steers = self.0.state.borrow().steers;
        while result.stop_reason == StopReason::Completed && !op.signal.is_cancelled() {
            // Inside a goal, the producer's message comes first: the answer that took it ends the loop, and the goal
            // waits for them.
            {
                let s = self.0.state.borrow();
                if s.objective_op.is_some() && s.steers > steers {
                    break;
                }
                steers = s.steers;
            }
            let decision = run.borrow_mut().decide();
            let (text, stop) = match decision {
                LoopDecision::Next(next) => (Some(next), None),
                LoopDecision::Stop { stop, wrap_up } => (wrap_up, Some(stop)),
            };
            let state = if stop.is_some() { LoopState::Done } else { LoopState::Running };
            self.emit(SessionEvent::Loop(run.borrow().status(state, stop)));
            *ended = stop.is_some();
            let Some(text) = text else { break };
            let snapshot = self.observe(op, None, true).await?;
            self.assert_current(op)?;
            op.phase.set(Phase::Inference);
            result = self.ask(op, &text, &snapshot.context, None, vec![], "").await?;
            add_usage(&mut usage, result.usage.as_ref());
            if stop.is_some() {
                break;
            }
        }
        result.usage = Some(usage);
        Ok(result)
    }
}
