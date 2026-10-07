//! The round log: as much as possible, so the producer sees work happening and knows where to steer. Each round's
//! target, the change, the numbers before and after on every checklist item, keep or revert and why, what was
//! rebalanced, what the listening model heard, and what's next.

use super::{
    checklist::{number, Change, Row},
    detect::{clock, Problem},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoundKind {
    /// The first listen: the checklist and the problems found.
    Start,
    /// A change judged.
    Judged,
    /// The run's end: the whole thing heard again.
    Done,
}

/// What to work on next: the biggest gap, and what the problem calls for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Next {
    pub id: String,
    pub label: String,
    pub gap: f64,
    pub wanted: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// One round, as the log and the app show it (a `{ type: "judged" }` session event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Round {
    pub round: u32,
    pub kind: RoundKind,
    /// What was heard to judge it: "bars 49–56, the loudest part".
    pub heard: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// The change in the model's words, and its lines in HISTORY.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<String>,
    pub rows: Vec<Row>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebalanced: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<Problem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Next>,
    /// Every item within tolerance.
    pub met: bool,
    /// Listens the run has made so far (each one real time in Live).
    pub listens: u32,
    pub elapsed_ms: i64,
}

impl Round {
    /// The round in a few lines: what it was after, what changed, the numbers that moved, the verdict and what's next.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![];
        let head = match self.kind {
            RoundKind::Start => format!("Listened to {}", self.heard),
            RoundKind::Judged => format!(
                "Round {}{}",
                self.round,
                self.target.as_ref().map(|target| format!(" · {}", target.to_lowercase())).unwrap_or_default()
            ),
            RoundKind::Done => format!("Done after {} rounds · heard {}", self.round, self.heard),
        };
        lines.push(head);
        if let Some(change) = &self.change {
            lines.push(format!("  change: {change}"));
        }
        let moved: Vec<String> = self
            .rows
            .iter()
            .filter(|row| self.kind == RoundKind::Start || row.change != Change::Same || self.target.as_deref() == Some(row.label.as_str()))
            .map(|row| {
                let unit = if row.unit.is_empty() { String::new() } else { format!(" {}", row.unit) };
                let value = |value: Option<f64>| value.map(number).unwrap_or_else(|| "–".into());
                let mark = match row.change {
                    Change::Better => " ✓",
                    Change::Worse => " ✗",
                    Change::Same => "",
                };
                if self.kind == RoundKind::Start {
                    format!("{} {}{unit} (wants {}){}", row.label, value(row.after), row.wanted, if row.gap_after > 0. { "" } else { " ✓" })
                } else {
                    format!("{} {} → {}{unit}{mark}", row.label, value(row.before), value(row.after))
                }
            })
            .collect();
        if !moved.is_empty() {
            lines.push(format!("  {}", moved.join(" · ")));
        }
        for problem in self.problems.iter().take(6) {
            lines.push(format!("  found: {}", problem.what));
        }
        match self.kept {
            Some(true) => lines.push(format!("  kept: {}", self.why.clone().unwrap_or_default())),
            Some(false) => lines.push(format!("  reverted: {}", self.why.clone().unwrap_or_default())),
            None => {}
        }
        if let Some(rebalanced) = &self.rebalanced {
            lines.push(format!("  rebalanced: {rebalanced}"));
        }
        if let Some(listener) = &self.listener {
            lines.push(format!("  listener: {listener}"));
        }
        if self.met {
            lines.push("  every item is within tolerance".into());
        } else if let Some(next) = &self.next {
            lines.push(format!(
                "  next: {} ({}{}, wants {}){}",
                next.label.to_lowercase(),
                next.now.map(|now| format!("{} now, ", number(now))).unwrap_or_default(),
                format!("{} steps off", number(next.gap)),
                next.wanted,
                next.fix.as_ref().map(|fix| format!(": {fix}")).unwrap_or_default()
            ));
        }
        lines.push(format!("  {} listens · {}", self.listens, clock(self.elapsed_ms as f64 / 1000.)));
        lines
    }
}
