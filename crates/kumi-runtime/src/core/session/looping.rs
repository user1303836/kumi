//! The loop's turns: after each answer, code decides whether the model goes on, with the judge's numbers.
use super::matching::add_usage;
use super::*;
use crate::core::loop_run::{LoopDecision, LoopRun, LoopState, LOOP_BUDGET};

impl Session {
    pub(super) async fn run_loop(
        &self,
        op: &Rc<Operation>,
        run: &Rc<RefCell<LoopRun>>,
        first: TurnResult,
    ) -> Result<TurnResult, RuntimeError> {
        op.extend((LOOP_BUDGET.ms + 10 * 60_000) as u64);
        let mut result = first;
        let mut usage = result.usage.clone().unwrap_or_default();
        while result.stop_reason == StopReason::Completed && !op.signal.is_cancelled() {
            let decision = run.borrow_mut().decide();
            let (text, stop) = match decision {
                LoopDecision::Next(next) => (Some(next), None),
                LoopDecision::Stop { stop, wrap_up } => (wrap_up, Some(stop)),
            };
            let state = if stop.is_some() { LoopState::Done } else { LoopState::Running };
            self.emit(SessionEvent::Loop(run.borrow().status(state, stop)));
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
