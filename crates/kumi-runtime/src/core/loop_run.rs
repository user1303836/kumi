//! The loop: listen, assess, adjust, repeat until a goal is met. Each time the model ends its answer, code looks at the
//! judge's rounds (listen and assess are the judge's; adjust is the model's one change) and decides, not the model:
//! the checklist met, changes no longer helping, or the budget spent ends it; otherwise the model goes back in with
//! the round's numbers and the next target. A run starts on an explicit /loop, or when the model starts a judged run
//! itself (Kumi judging that a request needs the loop).

use super::contracts::JsonObject;
use crate::listening::round::{Round, RoundKind};
use kumi_common::{js::number::to_string, time::now_ms};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{rc::Rc, sync::LazyLock};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopBudget {
    /// Judged rounds at most.
    pub rounds: u32,
    pub ms: i64,
    /// Rounds in a row that kept nothing (or closed less than a step between them) before it counts as stalled.
    pub stall: u32,
}
pub const LOOP_BUDGET: LoopBudget = LoopBudget { rounds: 16, ms: 45 * 60_000, stall: 4 };

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoopStop {
    /// Every checklist item within tolerance.
    Met,
    /// Changes stopped helping.
    Stalled,
    Budget,
    /// The model never judged anything.
    Unjudged,
    /// The run was ended (done) or the producer stopped it.
    Ended,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoopDecision {
    Next(String),
    Stop { stop: LoopStop, wrap_up: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoopState {
    Running,
    Done,
}

/// What the app shows while a loop runs (a `{ type: "loop" }` session event): rounds, kept and taken back, listens,
/// time, the next target and the last verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopStatus {
    pub state: LoopState,
    pub request: String,
    pub rounds: u32,
    pub kept: u32,
    pub reverted: u32,
    pub listens: u32,
    pub elapsed_ms: i64,
    pub rounds_left: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<LoopStop>,
}

/// Requests that call for the loop by themselves: making the mix, the master or a sound better against a measure,
/// a reference or a problem (the model's own judge call starts one too).
static WANTS_LOOP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(master(ing)?|mixdown|mix (it|this)|balance the mix|loud(er|ness)|lufs|true.?peak|harsh(ness)?|muddy|mud\b|boomy|resonan|sibilan|de-?ess|masking|buried|cut through|tonal balance)\b").unwrap()
});
static TO_FIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(make|get|fix|clean|tame|bring|master|mix|balance|reduce|remove|improve|polish|sort out)\b").unwrap()
});
/// Said with a request the judge can measure: work in judged rounds when it's about how the mix or a sound measures.
pub const LOOP_HINT: &str = "\n[Kumi] This request is about how the mix or a sound measures: when a clear target or problem is involved, work in judged rounds (judge with a goal, one change, judge it); Kumi then keeps the loop going until the goal is met.";
/// Whether a request calls for the loop by itself.
pub fn wants_loop(request: &str) -> bool {
    WANTS_LOOP.is_match(request) && TO_FIX.is_match(request)
}

pub struct LoopRun {
    pub request: String,
    started: i64,
    pub rounds: Vec<Round>,
    /// Times Kumi sent the model back, and nudged it to judge.
    asked: u32,
    nudged: u32,
    seen: usize,
    budget: LoopBudget,
    now: Rc<dyn Fn() -> i64>,
}

impl LoopRun {
    pub fn new(request: impl Into<String>, budget: LoopBudget) -> Self {
        Self::with_clock(request, budget, Rc::new(now_ms))
    }
    pub fn with_clock(request: impl Into<String>, budget: LoopBudget, now: Rc<dyn Fn() -> i64>) -> Self {
        Self { request: request.into(), started: now(), rounds: vec![], asked: 0, nudged: 0, seen: 0, budget, now }
    }
    /// A round the judge logged.
    pub fn judged(&mut self, round: Round) {
        if round.kind == RoundKind::Start {
            // A new run of the judge: what came before was another checklist.
            self.rounds.clear();
            self.seen = 0;
        }
        self.rounds.push(round);
    }
    fn judged_rounds(&self) -> impl Iterator<Item = &Round> {
        self.rounds.iter().filter(|round| round.kind == RoundKind::Judged)
    }
    pub fn status(&self, state: LoopState, stop: Option<LoopStop>) -> LoopStatus {
        let last = self.rounds.last();
        LoopStatus {
            state,
            request: self.request.clone(),
            rounds: self.judged_rounds().count() as u32,
            kept: self.judged_rounds().filter(|round| round.kept == Some(true)).count() as u32,
            reverted: self.judged_rounds().filter(|round| round.kept == Some(false)).count() as u32,
            listens: last.map_or(0, |round| round.listens),
            elapsed_ms: (self.now)() - self.started,
            rounds_left: self.budget.rounds.saturating_sub(self.judged_rounds().count() as u32),
            next: last.and_then(|round| round.next.as_ref()).map(|next| next.label.clone()),
            last: last.and_then(|round| round.why.clone()),
            stop,
        }
    }
    /// What happens after the model's answer.
    pub fn decide(&mut self) -> LoopDecision {
        let fresh = self.rounds.len() > self.seen;
        self.seen = self.rounds.len();
        let Some(last) = self.rounds.last().cloned() else {
            if self.nudged >= 1 {
                return LoopDecision::Stop { stop: LoopStop::Unjudged, wrap_up: None };
            }
            self.nudged += 1;
            return LoopDecision::Next(format!(
                "[Kumi loop] {} Start a judged run first: judge with the goal that fits the request (a loudness and a ceiling, a reference, the element that must cut through), hearing the mix or the track it's about. Then make one change toward the next target and judge it.",
                self.request
            ));
        };
        if last.kind == RoundKind::Done {
            return LoopDecision::Stop { stop: LoopStop::Ended, wrap_up: None };
        }
        let judged = self.judged_rounds().count() as u32;
        let elapsed = (self.now)() - self.started;
        if last.met {
            return LoopDecision::Stop { stop: LoopStop::Met, wrap_up: Some(wrap_up("Every item on the checklist is within tolerance.")) };
        }
        if judged >= self.budget.rounds || elapsed >= self.budget.ms {
            return LoopDecision::Stop { stop: LoopStop::Budget, wrap_up: Some(wrap_up("That's the loop's budget spent.")) };
        }
        if self.stalled() {
            return LoopDecision::Stop { stop: LoopStop::Stalled, wrap_up: Some(wrap_up("Changes stopped helping.")) };
        }
        if !fresh {
            // The model answered without judging anything new: once it's reminded; then the loop ends.
            if self.nudged >= 2 {
                return LoopDecision::Stop {
                    stop: LoopStop::Unjudged,
                    wrap_up: Some(wrap_up("The loop got no judged change from the last answers.")),
                };
            }
            self.nudged += 1;
            return LoopDecision::Next(format!(
                "[Kumi loop] Judge the change you made (judge with change saying what it was), or, if you made none, make one toward the next target{} and judge it.",
                last.next.as_ref().map(|next| format!(" ({})", next.label.to_lowercase())).unwrap_or_default()
            ));
        }
        self.asked += 1;
        let left = self.budget.rounds - judged;
        let minutes = ((self.budget.ms - elapsed) as f64 / 60_000.).round().max(0.);
        let verdict = match last.kept {
            Some(true) => format!("Kept: {}.", last.why.clone().unwrap_or_default()),
            Some(false) => format!("Taken back: {}. Try another way.", last.why.clone().unwrap_or_default()),
            None => String::new(),
        };
        let next = last
            .next
            .as_ref()
            .map(|next| {
                format!(
                    " Next: {} ({}wants {}){}.",
                    next.label.to_lowercase(),
                    next.now.map(|now| format!("{} now, ", to_string(now))).unwrap_or_default(),
                    next.wanted,
                    next.fix.as_ref().map(|fix| format!("; it calls for {fix}")).unwrap_or_default()
                )
            })
            .unwrap_or_default();
        LoopDecision::Next(format!(
            "[Kumi loop] Round {}. {verdict}{next} Budget left: {left} rounds, about {} minutes. Make one change toward it, then judge it (Kumi asks for done when it's time).",
            last.round,
            to_string(minutes)
        ))
    }
    /// Why the loop is over now (met, out of budget or stalled), as a judged round arrives: then only the run's end is
    /// judged. None while it goes on (or once the run has ended).
    pub fn over(&self) -> Option<&'static str> {
        let last = self.rounds.last()?;
        if last.kind == RoundKind::Done {
            return None;
        }
        let judged = self.judged_rounds().count() as u32;
        if last.met {
            Some("every item on the checklist is within tolerance")
        } else if judged >= self.budget.rounds || (self.now)() - self.started >= self.budget.ms {
            Some("its budget is spent")
        } else if self.stalled() {
            Some("changes stopped helping")
        } else {
            None
        }
    }
    /// The last rounds kept nothing, or the gaps they closed add up to less than a step.
    fn stalled(&self) -> bool {
        let rounds: Vec<&Round> = self.judged_rounds().collect();
        let n = self.budget.stall as usize;
        if rounds.len() < n {
            return false;
        }
        let recent = &rounds[rounds.len() - n..];
        if recent.iter().all(|round| round.kept != Some(true)) {
            return true;
        }
        let closed: f64 = recent
            .iter()
            .filter(|round| round.kept == Some(true))
            .map(|round| round.rows.iter().map(|row| row.gap_before - row.gap_after).sum::<f64>())
            .sum();
        closed < 1.
    }
}

fn wrap_up(why: &str) -> String {
    format!("[Kumi loop] {why} End the run with judge done: true (one last listen to the whole stretch), then tell the producer what changed, before → after, what's still off, and which devices hold the work.")
}

/// A loop's request as the first turn of an explicit /loop: what the producer asked, and how to work in rounds.
pub fn loop_setup(request: &str) -> String {
    format!("[Kumi loop] {request}\n\nWork in rounds. Start a judged run: judge with the goal that fits (loudness and a ceiling for a master, a reference when there's one, focus on the element that must cut through), hearing the mix or the track the request is about. Then make one change toward the next target with Live's devices and judge it with change. Kumi decides when the loop stops: don't end the run with done until it asks.")
}

/// A judged round as the session keeps it with a run's status (for tests and the app).
pub fn round_json(round: &Round) -> JsonObject {
    serde_json::to_value(round).ok().and_then(|value| value.as_object().cloned()).unwrap_or_default()
}
