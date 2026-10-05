//! Match runs: for "make it sound like this", the harness decides when to stop, not the model. Each
//! time the model ends its answer, the run looks at the auditions so far (auditioning the current
//! best itself when something changed since), and either ends it (the target reached, no gain after
//! trying something genuinely different, or the budget spent) or sends the model back in with the
//! score, what's left of the budget and the biggest gaps. A first draft can't end a run.

use serde::{Deserialize, Serialize};

use super::goal::Best;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MatchStop {
    Reached,
    Plateau,
    Budget,
    NoAudition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchState {
    Running,
    Done,
}

/// What the app shows while a run works: its check, the best so far and where it started, and how long it's been (a `{ type: "match" }` session event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchStatus {
    pub state: MatchState,
    pub check: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<Best>,
    pub elapsed_ms: i64,
    pub rounds_left: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<MatchStop>,
}

use super::{
    contracts::{AuditionCandidate, AuditionEvent, AuditionRequest, MIX_CANDIDATE},
    techniques::MATCHING,
};
use kumi_common::{
    js::number::{round, to_string},
    time::now_ms,
};
use regex::Regex;
use std::{rc::Rc, sync::LazyLock};
#[derive(Debug, Clone, Copy)]
pub struct MatchBudget {
    pub rounds: u32,
    pub ms: i64,
    pub target: f64,
    pub plateau_checks: usize,
    pub min_gain: f64,
    pub polish_ms: Option<u64>,
}
pub const MATCH_BUDGET: MatchBudget =
    MatchBudget { rounds: 12, ms: 45 * 60_000, target: 92.0, plateau_checks: 2, min_gain: 2.0, polish_ms: Some(8 * 60_000) };
static SOMETHING_TO_MATCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)https?://|\.(wav|wave|aiff?|flac|mp3|ogg|m4a|mp4|mov|webm|mkv)\b|\breferences?\b|\blike (this|that)\b|\b(this|that) (sound|track|clip|sample|recording|video|tutorial|song|tune|part|loop)\b|\bthe (sound|recording|video|tutorial|song) (from|in|of|at)\b").unwrap()
});
pub fn starts_match(request: &str) -> bool {
    MATCHING.is_match(request) && SOMETHING_TO_MATCH.is_match(request)
}
pub static KEEP_GOING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(keep going|carry on|continue|go on|keep trying|try more|more|again)\s*[.!]*\s*$|^\s*(keep going|carry on|keep trying|try more)\b").unwrap()
});
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MatchDecision {
    Stop {
        stop: MatchStop,
        #[serde(rename = "wrapUp", skip_serializing_if = "Option::is_none")]
        wrap_up: Option<String>,
    },
    Next {
        next: String,
    },
}
#[derive(Debug, Clone)]
pub struct LastAudition {
    pub request: Option<AuditionRequest>,
    pub event: AuditionEvent,
}
pub struct MatchRun {
    pub request: String,
    pub checks: Vec<f64>,
    started: i64,
    continuations: u32,
    explored: bool,
    nudged: u32,
    widened: bool,
    pub last: Option<LastAudition>,
    pub best: Option<Best>,
    pub best_candidate: Option<AuditionCandidate>,
    pub polished: bool,
    pub first: Option<f64>,
    pub changed_since: bool,
    pub reference: Option<String>,
    pub history: Vec<Best>,
    budget: MatchBudget,
    now: Rc<dyn Fn() -> i64>,
}
impl MatchRun {
    pub fn new(request: impl Into<String>, budget: MatchBudget) -> Self {
        Self::with_clock(request, budget, Rc::new(now_ms))
    }
    pub fn with_clock(request: impl Into<String>, budget: MatchBudget, now: Rc<dyn Fn() -> i64>) -> Self {
        Self {
            request: request.into(),
            started: now(),
            budget,
            now,
            checks: vec![],
            continuations: 0,
            explored: false,
            nudged: 0,
            widened: false,
            last: None,
            best: None,
            best_candidate: None,
            polished: false,
            first: None,
            changed_since: false,
            reference: None,
            history: vec![],
        }
    }
    pub fn carry_on(previous: &Self, budget: MatchBudget, now: Rc<dyn Fn() -> i64>) -> Self {
        let mut run = Self::with_clock(&previous.request, budget, now);
        run.history = previous.history.clone();
        run.first = previous.first;
        run.best = previous.best.clone();
        run.last = previous.last.clone();
        run.reference = previous.reference.clone();
        run.changed_since = previous.changed_since;
        run
    }
    pub fn auditioned(&mut self, event: AuditionEvent, request: Option<AuditionRequest>) {
        let asked = request.or_else(|| event.request.clone());
        if event.reference.as_ref().is_some_and(|s| !s.is_empty()) {
            self.reference = event.reference.clone();
        }
        self.changed_since = false;
        if let Some(best) = &event.best {
            let best = Best { label: best.label.clone(), score: best.score };
            self.history.push(best.clone());
            if self.first.is_none() {
                self.first = Some(best.score);
            }
            if self.best.as_ref().is_none_or(|old| best.score > old.score) {
                self.best_candidate =
                    event.takes.iter().find(|t| t.label == best.label).and_then(|t| t.r#where.as_ref()).map(|at| AuditionCandidate {
                        track: at.track.clone(),
                        label: Some(best.label.clone()),
                        clip: at.clip.clone().filter(|s| !s.is_empty()),
                        mix: (at.track == MIX_CANDIDATE).then_some(true),
                    });
                self.best = Some(best);
            }
        }
        self.last = Some(LastAudition { event, request: asked });
    }
    pub fn tuned(&mut self, label: impl Into<String>, score: f64) {
        self.polished = true;
        let best = Best { label: label.into(), score };
        self.history.push(best.clone());
        if self.best.as_ref().is_none_or(|b| score > b.score) {
            self.best = Some(best);
        }
    }
    pub fn polishes(&self) -> bool {
        !self.polished
            && self.budget.polish_ms.is_some_and(|ms| ms > 0)
            && self.best_candidate.as_ref().is_some_and(|c| c.mix != Some(true))
            && self.last.as_ref().and_then(|l| l.request.as_ref()).and_then(|r| r.reference.as_ref()).is_some_and(|s| !s.is_empty())
    }
    pub fn polish_ms(&self) -> u64 {
        self.budget.polish_ms.unwrap_or(0)
    }
    pub fn changed(&mut self) {
        self.changed_since = true;
    }
    pub fn needs_audition(&self) -> bool {
        self.changed_since && self.last.as_ref().is_some_and(|l| l.request.is_some())
    }
    pub fn status(&self, state: MatchState, stop: Option<MatchStop>) -> MatchStatus {
        MatchStatus {
            state,
            check: self.checks.len(),
            first: self.first,
            best: self.best.clone(),
            elapsed_ms: (self.now)() - self.started,
            rounds_left: self.budget.rounds.saturating_sub(self.continuations),
            stop,
        }
    }
    pub fn decide(&mut self) -> MatchDecision {
        let elapsed = (self.now)() - self.started;
        let rounds = self.budget.rounds as i64 - self.continuations as i64;
        let minutes = round((self.budget.ms - elapsed) as f64 / 60_000.0).max(0.0);
        let Some(best) = &self.best else {
            if self.nudged >= 2 || rounds <= 0 {
                return MatchDecision::Stop { stop: MatchStop::NoAudition, wrap_up: None };
            }
            self.nudged += 1;
            self.continuations += 1;
            return MatchDecision::Next {next:"[Kumi] Before finishing, audition what you built against the reference (the audition tool; several candidates on their own tracks render together). If the producer gave no reference, listen to what they pointed at, or ask them for one and stop.".into()};
        };
        self.checks.push(best.score);
        if best.score >= self.budget.target {
            return MatchDecision::Stop {
                stop: MatchStop::Reached,
                wrap_up: (self.continuations > 0)
                    .then(|| self.wrap_up(&format!("That reaches {}%, close enough to stop.", to_string(best.score)))),
            };
        }
        if rounds <= 0 || elapsed >= self.budget.ms {
            return MatchDecision::Stop { stop: MatchStop::Budget, wrap_up: Some(self.wrap_up("That's the run's budget spent.")) };
        }
        let n = self.budget.plateau_checks;
        let stalled =
            self.checks.len() > n && self.checks[self.checks.len() - 1] - self.checks[self.checks.len() - 1 - n] < self.budget.min_gain;
        if stalled && self.explored {
            return MatchDecision::Stop {
                stop: MatchStop::Plateau,
                wrap_up: Some(self.wrap_up("Refining and new ideas both stopped gaining.")),
            };
        }
        self.next(rounds, minutes, stalled)
    }
    pub fn wrap_up(&self, why: &str) -> String {
        format!("[Kumi] {why} Tidy up, then give your final answer. Put the winner where the producer asked for it: when they asked for it on a track (\"this track\", one they named or had selected) and it won elsewhere, rebuild it there (the same devices, the settings you gave them, its clip) and audition it once to check it scores the same; otherwise keep it on its own. Mute every other candidate track you made (don't undo them: Live removes a track only from the last one back, and not one changed since). Then say the score before and after ({}% → {}%), which candidate won and why, what still differs, and which muted tracks hold the others for the producer to A/B or delete. Call it the closest you got.",self.first.map(to_string).unwrap_or("undefined".into()),self.best.as_ref().map(|b|to_string(b.score)).unwrap_or("undefined".into()))
    }
    fn next(&mut self, rounds: i64, minutes: f64, stalled: bool) -> MatchDecision {
        let Some(best) = self.best.as_ref() else {
            return MatchDecision::Stop { stop: MatchStop::NoAudition, wrap_up: None };
        };
        let gaps = self
            .last
            .as_ref()
            .filter(|l| !l.event.gaps.is_empty())
            .map(|l| format!(" Biggest gaps: {}.", l.event.gaps.join("; ")))
            .unwrap_or_default();
        let scores = if self.checks.len() > 1 {
            format!("{}% → {}%", to_string(self.checks[self.checks.len() - 2]), to_string(best.score))
        } else {
            format!("{}%", to_string(best.score))
        };
        let minutes = to_string(minutes);
        if self.checks.len() == 1 && !self.widened && self.last.as_ref().map(|l| l.event.takes.len()).unwrap_or(0) < 2 {
            self.widened = true;
            self.continuations += 1;
            return MatchDecision::Next {next:format!("[Kumi] Score {scores} with a single candidate. Budget left: {} rounds, about {minutes} minutes. Start wide: build 2–3 more genuinely different candidates on new tracks (other base instruments, serial against parallel), audition them all together with this one, then refine the best.",rounds-1)};
        }
        let budget = format!(" Budget left: {rounds} rounds, about {minutes} minutes.");
        self.continuations += 1;
        if stalled {
            self.explored = true;
            self.checks.clear();
            self.checks.push(best.score);
            return MatchDecision::Next {next:format!("[Kumi] Score {scores}, and refining has stalled.{budget}{gaps} Try something genuinely different now: 2–4 new candidates on new tracks with other base instruments or another topology (parallel against serial, a rack of layers, resampling), audition them with the best so far, then refine the winner.")};
        }
        if let Some(structural) = self.last.as_ref().and_then(|l| l.event.structural.as_ref()) {
            return MatchDecision::Next {next:format!("[Kumi] Score {scores} (best: {}).{budget} Knobs can't close this: {}. Change the structure: {}. Build it on a copy or a new track, audition it with the best, then refine.",best.label,structural.gap,structural.r#move)};
        }
        MatchDecision::Next {next:format!("[Kumi] Score {scores} (best: {}).{budget}{gaps} Keep going: fix the biggest gaps on the best candidate, trying several values side by side on copies, and audition again. Try something different if refinement has stalled.",best.label)}
    }
}
