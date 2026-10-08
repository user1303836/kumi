//! /goal: one standing objective Kumi keeps working toward until it's met, it's blocked, or its budget runs out. After
//! every turn it's checked against evidence: the judge's checklist when a judged run is part of it, else a forced
//! self-audit that ends in an explicit complete, blocked or continue. It shows its status as it goes, pauses on Esc
//! (never discarding what's done), and is kept on disk so it survives a restart. The listen, assess, adjust loop is one
//! of its tools: a turn that starts a judged run runs the loop inside it.

use super::errors::{FailureKind, RuntimeError};
use crate::listening::round::{Round, RoundKind};
use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, rc::Rc, sync::LazyLock};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectiveBudget {
    pub turns: u32,
    pub ms: i64,
    /// Turns in a row without progress before it counts as stuck: a judged turn progresses when it closes the gap by
    /// a step or more, an unjudged one when it changes the Set.
    pub idle: u32,
}
pub const OBJECTIVE_BUDGET: ObjectiveBudget = ObjectiveBudget { turns: 12, ms: 60 * 60_000, idle: 3 };

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectiveState {
    Running,
    Paused,
    Done,
}

/// What a check after a turn found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Complete,
    Continue,
    /// Something the producer must do first (sign in, open a Set, give a reference…).
    Blocked,
    /// Turns went by with nothing changing.
    Stuck,
    Budget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub verdict: Verdict,
    pub reason: String,
    /// What comes next (a continue's next step, or what's left when it stopped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// From the judge's measurements, not the model's own word.
    #[serde(default)]
    pub measured: bool,
}

/// A goal as kept on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Objective {
    pub version: u32,
    pub objective: String,
    pub state: ObjectiveState,
    pub turns: u32,
    pub budget: ObjectiveBudget,
    pub elapsed_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<Check>,
    /// Turns in a row without progress.
    #[serde(default)]
    pub idle: u32,
}

impl Objective {
    pub fn new(objective: impl Into<String>, budget: ObjectiveBudget) -> Self {
        Self {
            version: 2,
            objective: objective.into(),
            state: ObjectiveState::Running,
            turns: 0,
            budget,
            elapsed_ms: 0,
            last: None,
            idle: 0,
        }
    }
    /// What the app shows: the objective, where it is, the turns and time against the budget, and the last check.
    pub fn status(&self, elapsed_ms: i64) -> ObjectiveStatus {
        ObjectiveStatus {
            objective: self.objective.clone(),
            state: self.state,
            turns: self.turns,
            turn_budget: self.budget.turns,
            elapsed_ms,
            budget_ms: self.budget.ms,
            verdict: self.last.as_ref().map(|check| check.verdict),
            reason: self.last.as_ref().map(|check| check.reason.clone()),
            next: self.last.as_ref().and_then(|check| check.next.clone()),
            measured: self.last.as_ref().is_some_and(|check| check.measured),
        }
    }
}

/// A goal's live status (a `{ type: "objective" }` session event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectiveStatus {
    pub objective: String,
    pub state: ObjectiveState,
    pub turns: u32,
    pub turn_budget: u32,
    pub elapsed_ms: i64,
    pub budget_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    pub measured: bool,
}

/// The first turn's words: the objective, and how Kumi will hold the model to it.
pub fn objective_first(objective: &str) -> String {
    format!("[Kumi goal] {objective}\n\nThis is a goal: Kumi keeps you on it, turn after turn, until it's met. Work toward it now. Where it's about how something sounds, prove it with the judge (judge with a goal that measures it), so Kumi can check the numbers. End each answer by saying what you did and what's left.")
}

/// A later turn's words: the check's reason and the next step, and the budget left.
pub fn objective_next(objective: &Objective) -> String {
    let check = objective.last.as_ref();
    let left = objective.budget.turns.saturating_sub(objective.turns);
    format!(
        "[Kumi goal] {}\nNot there yet{}.{} Budget left: {left} turns. Carry on toward it.",
        objective.objective,
        check.map(|check| format!(": {}", check.reason)).unwrap_or_default(),
        check.and_then(|check| check.next.as_ref()).map(|next| format!(" Next: {next}.")).unwrap_or_default()
    )
}

/// The forced self-audit: no changes, one line that ends in complete, blocked or continue.
pub fn objective_audit(objective: &str) -> String {
    format!("[Kumi goal check] Don't change anything now. Is this goal met: {objective}? Judge it by what you measured or can see in the Set, not by what you meant to do. Answer with one line and nothing else: COMPLETE: <the evidence>, or BLOCKED: <what the producer must do first>, or CONTINUE: <the next step>.")
}

static AUDIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?im)^\W*(COMPLETE|BLOCKED|CONTINUE)\W*[:\-–—]?\s*(.*)$").unwrap());

/// The self-audit's line, read: an answer without one is a continue, its words the reason.
pub fn read_audit(answer: &str) -> Check {
    match AUDIT.captures(answer) {
        Some(found) => {
            let words = found[2].trim().to_string();
            match found[1].to_uppercase().as_str() {
                "COMPLETE" => Check { verdict: Verdict::Complete, reason: words, next: None, measured: false },
                "BLOCKED" => Check { verdict: Verdict::Blocked, reason: words.clone(), next: Some(words), measured: false },
                _ => Check {
                    verdict: Verdict::Continue,
                    reason: "not met yet, by the model's own check".into(),
                    next: Some(words),
                    measured: false,
                },
            }
        }
        None => Check {
            verdict: Verdict::Continue,
            reason: "the check got no clear answer".into(),
            next: Some(kumi_common::js::string::head(answer.trim(), 200)),
            measured: false,
        },
    }
}

/// The check by measurement: a judged run's last round, when the goal has one. Met is complete; otherwise it goes on
/// toward the next target. None when the judge hasn't measured anything for this goal.
pub fn measured_check(round: Option<&Round>) -> Option<Check> {
    let round = round?;
    if round.met {
        return Some(Check { verdict: Verdict::Complete, reason: "the judge's checklist is met".into(), next: None, measured: true });
    }
    let next = round.next.as_ref().map(|next| {
        format!(
            "{} ({}wants {})",
            next.label.to_lowercase(),
            next.now.map(|now| format!("{} now, ", kumi_common::js::number::to_string(now))).unwrap_or_default(),
            next.wanted
        )
    });
    let reason = match round.kind {
        RoundKind::Done => "the judged run ended short of its checklist".into(),
        _ => format!("the judge's checklist isn't met{}", next.as_ref().map(|next| format!(": {next}")).unwrap_or_default()),
    };
    Some(Check { verdict: Verdict::Continue, reason, next, measured: true })
}

#[async_trait(?Send)]
pub trait ObjectiveStore {
    async fn load(&self, place: &str) -> Result<Option<Objective>, RuntimeError>;
    async fn save(&self, place: &str, objective: &Objective) -> Result<(), RuntimeError>;
    async fn clear(&self, place: &str) -> Result<(), RuntimeError>;
}

/// Goals kept as files, one per Set (`<place>.goal.json`, beside the knob searches' own files).
pub struct FileObjectiveStore {
    folder: PathBuf,
}
pub fn create_objective_store(folder: impl Into<PathBuf>) -> Rc<FileObjectiveStore> {
    Rc::new(FileObjectiveStore { folder: folder.into() })
}
impl FileObjectiveStore {
    fn file(&self, place: &str) -> PathBuf {
        let safe: String = place.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).take(120).collect();
        self.folder.join(format!("{}.goal.json", if safe.is_empty() { "unsaved".into() } else { safe }))
    }
}
#[async_trait(?Send)]
impl ObjectiveStore for FileObjectiveStore {
    async fn load(&self, place: &str) -> Result<Option<Objective>, RuntimeError> {
        let Ok(bytes) = tokio::fs::read(self.file(place)).await else { return Ok(None) };
        Ok(serde_json::from_slice::<Objective>(&bytes).ok().filter(|objective| objective.version == 2))
    }
    async fn save(&self, place: &str, objective: &Objective) -> Result<(), RuntimeError> {
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(&self.folder).await.map_err(|error| RuntimeError::plain(error.to_string()))?;
        let temporary = self.folder.join(format!(".goal-{}", uuid::Uuid::new_v4()));
        let written = async {
            let mut options = tokio::fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&temporary).await?;
            file.write_all(&serde_json::to_vec(objective).expect("a goal serializes")).await?;
            file.flush().await?;
            drop(file);
            tokio::fs::rename(&temporary, self.file(place)).await
        }
        .await;
        if let Err(error) = written {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(RuntimeError::plain(error.to_string()));
        }
        Ok(())
    }
    async fn clear(&self, place: &str) -> Result<(), RuntimeError> {
        match tokio::fs::remove_file(self.file(place)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(RuntimeError::plain(error.to_string())),
        }
    }
}

/// After a turn: the check, with the budget and progress rules applied (code decides, not the model). `progress`: the
/// turn closed the judge's gap by a step or more, or (when nothing was judged) changed the Set.
pub fn after_turn(objective: &mut Objective, check: Check, progress: bool, elapsed_ms: i64) -> Check {
    objective.turns += 1;
    objective.elapsed_ms = elapsed_ms;
    objective.idle = if progress { 0 } else { objective.idle + 1 };
    let check = match check.verdict {
        Verdict::Complete | Verdict::Blocked => check,
        // Out of budget: what's still off stays the reason, so the status and a resume say where it got.
        _ if objective.turns >= objective.budget.turns || elapsed_ms >= objective.budget.ms => Check { verdict: Verdict::Budget, ..check },
        _ if objective.idle >= objective.budget.idle => Check {
            verdict: Verdict::Stuck,
            reason: format!("{} turns in a row got no closer", objective.idle),
            next: check.next.clone(),
            measured: check.measured,
        },
        _ => check,
    };
    objective.state = match check.verdict {
        Verdict::Continue => ObjectiveState::Running,
        Verdict::Complete => ObjectiveState::Done,
        // Stopped short: kept, so /goal resume carries on (with a fresh budget) once the cause is dealt with.
        Verdict::Blocked | Verdict::Stuck | Verdict::Budget => ObjectiveState::Paused,
    };
    objective.last = Some(check.clone());
    check
}

/// A turn's or a check's error as the goal's last check: one the producer must fix first (sign in, billing, the model,
/// Kumi's settings, Live) stops it as blocked, naming what to do; any other pauses it, for /goal resume.
pub fn error_check(error: &RuntimeError) -> Check {
    let reason = kumi_common::js::string::head(&error.message(), 200);
    let fix = match error.kumi().map(|kumi| kumi.kind) {
        Some(FailureKind::Auth) => Some("sign in again (/login)"),
        Some(FailureKind::Billing) => Some("sort out the provider account's billing or credits, or choose another model (/model)"),
        Some(FailureKind::Model) => Some("choose another model (/model)"),
        Some(FailureKind::Config) => Some("fix the setting the error names"),
        Some(FailureKind::Live) => Some("check that Live is open with Kumi's bridge"),
        _ => None,
    };
    match fix {
        Some(fix) => Check { verdict: Verdict::Blocked, reason, next: Some(format!("{fix}, then /goal resume")), measured: false },
        None => Check { verdict: Verdict::Continue, reason, next: None, measured: false },
    }
}

/// A command's words as a subcommand: any case, trailing punctuation and extra spaces aside (`Pause.` is `pause`).
pub fn command_word(words: &str) -> String {
    words.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches(|c: char| c.is_ascii_punctuation()).trim().to_lowercase()
}
