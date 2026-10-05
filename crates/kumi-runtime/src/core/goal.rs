//! Goal mode (/goal): Kumi goes after a sound or part until it gets there, the producer stops it, or
//! a safety cap of hours. Code does most of the searching (evolve.rs: knobs nudged, crossed and
//! redrawn around the best, a generation of candidates rendered in one silent pass); the model makes
//! the structural leaps, every few generations or when the search stalls. A goal is kept on disk as it
//! goes, so it survives a restart and /goal picks it up again.

use serde::{Deserialize, Serialize};

use super::contracts::AuditionRequest;
use super::errors::RuntimeError;
use super::evolve::Slot;
use async_trait::async_trait;
use kumi_common::js::{json::stringify, number::to_string};
use serde_json::Value;
use std::{path::PathBuf, rc::Rc};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalBudget {
    pub target: f64,
    pub ms: u64,
    pub leap_every: u32,
    pub stall_generations: u32,
}
pub const GOAL_BUDGET: GoalBudget = GoalBudget { target: 95.0, ms: 4 * 60 * 60_000, leap_every: 8, stall_generations: 5 };

/// Where a goal is: searching, left for later, or over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalRun {
    Running,
    Paused,
    Done,
}

/// A goal as kept on disk: what it's after, where the part is, and the search so far.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalState {
    pub version: u32,
    pub goal: String,
    /// The reference, the span and the focus, as the setup audition gave them (candidates by track name).
    pub request: AuditionRequest,
    /// The candidates play their Session clips (copied into the Arrangement for each render).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<bool>,
    pub slots: Vec<Slot>,
    pub generation: u32,
    pub rendered: u32,
    pub trend: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<f64>,
    pub elapsed_ms: i64,
    pub status: GoalRun,
    /// What the model tried last (a leap), and where the best was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idea: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_track: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Its lesson in the playbook, updated as the goal goes on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lesson: Option<String>,
}

/// A goal's state as the app shows it: the search's, or "starting" before it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalPhase {
    Running,
    Paused,
    Done,
    Starting,
}

impl From<GoalRun> for GoalPhase {
    fn from(run: GoalRun) -> Self {
        match run {
            GoalRun::Running => Self::Running,
            GoalRun::Paused => Self::Paused,
            GoalRun::Done => Self::Done,
        }
    }
}

/// The best so far: its label and score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Best {
    pub label: String,
    pub score: f64,
}

/// What the app shows of a goal: the dashboard's numbers (a `{ type: "goal" }` session event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalStatus {
    pub state: GoalPhase,
    pub goal: String,
    pub generation: u32,
    pub rendered: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<Best>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<f64>,
    pub trend: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idea: Option<String>,
    pub elapsed_ms: i64,
    pub candidates: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_track: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

#[async_trait(?Send)]
pub trait GoalStore {
    async fn load(&self, place: &str) -> Result<Option<GoalState>, RuntimeError>;
    async fn save(&self, place: &str, state: &GoalState) -> Result<(), RuntimeError>;
    async fn clear(&self, place: &str) -> Result<(), RuntimeError>;
}
pub struct FileGoalStore {
    folder: PathBuf,
}
pub fn create_goal_store(folder: impl Into<PathBuf>) -> Rc<FileGoalStore> {
    Rc::new(FileGoalStore { folder: folder.into() })
}
fn safe_place(place: &str) -> String {
    let safe: String = place
        .encode_utf16()
        .map(|c| if c <= 127 && ((c as u8).is_ascii_alphanumeric() || b"_.-".contains(&(c as u8))) { char::from(c as u8) } else { '_' })
        .take(120)
        .collect();
    if safe.is_empty() {
        "unsaved".into()
    } else {
        safe
    }
}
impl FileGoalStore {
    fn file(&self, place: &str) -> PathBuf {
        self.folder.join(format!("{}.json", safe_place(place)))
    }
    /// The file format's deliberately shallow source validation, before decoding a typed state.
    pub async fn load_value(&self, place: &str) -> Option<Value> {
        let bytes = tokio::fs::read(self.file(place)).await.ok()?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        if value["version"].as_f64() != Some(1.0)
            || !value["goal"].is_string()
            || !match &value["request"] {
                Value::Null => false,
                Value::Bool(v) => *v,
                Value::Number(v) => v.as_f64().is_some_and(|v| v != 0.0),
                Value::String(v) => !v.is_empty(),
                _ => true,
            }
            || !value["slots"].is_array()
        {
            return None;
        }
        Some(value)
    }
}
#[async_trait(?Send)]
impl GoalStore for FileGoalStore {
    async fn load(&self, place: &str) -> Result<Option<GoalState>, RuntimeError> {
        Ok(self.load_value(place).await.and_then(|v| serde_json::from_value(v).ok()))
    }
    async fn save(&self, place: &str, state: &GoalState) -> Result<(), RuntimeError> {
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(&self.folder).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
        let temporary = self.folder.join(format!(".goal-{}", uuid::Uuid::new_v4()));
        let result = async {
            let mut options = tokio::fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&temporary).await?;
            let mut data = stringify(&serde_json::to_value(state).expect("goal serialization"));
            data.push('\n');
            file.write_all(data.as_bytes()).await?;
            file.flush().await?;
            drop(file);
            tokio::fs::rename(&temporary, self.file(place)).await
        }
        .await;
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(RuntimeError::plain(e.to_string()));
        }
        Ok(())
    }
    async fn clear(&self, place: &str) -> Result<(), RuntimeError> {
        match tokio::fs::remove_file(self.file(place)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(RuntimeError::plain(e.to_string())),
        }
    }
}
pub fn goal_setup(goal: &str) -> String {
    format!("[Kumi goal] {goal}\n\nThis is a goal: Kumi keeps searching until it gets there. Set the search up: listen to the reference, then build 3–4 genuinely different candidates, each on a new track named \"Kumi · Goal · <idea>\" (different base instruments such as Operator, Wavetable, Drift, Collision or a Simpler sample; serial chains against parallel racks), with the part on each, and audition them together against the reference once. Then stop: Kumi's own search takes over from there, trying knob settings on your candidates by the hundred, and comes back to you for bigger ideas.\n\nWhen the producer gave nothing to compare with (no audio file, clip in the Set or video), there's nothing to search toward: do what they asked as an ordinary request instead, completely (every tool is yours: set the knobs, add rack chains, load devices into them, rename), and don't audition.")
}
pub fn goal_leap(
    state: &GoalState,
    best: Option<&Best>,
    gaps: &[String],
    stalled: bool,
    structural: Option<&super::contracts::StructuralMove>,
) -> String {
    let trend = state.trend[state.trend.len().saturating_sub(8)..].iter().map(|v| to_string(*v)).collect::<Vec<_>>().join("% → ");
    let best = best.map(|b| format!("{}% ({})", to_string(b.score), b.label)).unwrap_or_else(|| "none yet".into());
    let lately = if trend.is_empty() { "no scores".into() } else { format!("{trend}%") };
    let gaps = if gaps.is_empty() { String::new() } else { format!(" Its biggest gaps: {}.", gaps.join("; ")) };
    let reason = if let Some(s) = structural {
        format!("Knobs can't close this: {}. Change the structure: {}.", s.gap, s.r#move)
    } else if stalled {
        "Tweaking knobs has stalled.".into()
    } else {
        "Time for a bigger idea.".into()
    };
    let labels = state.slots.iter().map(|s| s.label.as_str()).collect::<Vec<_>>().join(", ");
    format!("[Kumi goal] {}\nGeneration {}, {} candidates rendered. Best {best}; lately {lately}.{gaps}\n{reason} Make a structural leap: 1–2 new candidates on new tracks named \"Kumi · Goal · <idea>\" that differ from the ones in the search ({labels}): another base instrument or topology, parallel against serial, resampling, or a Max for Live device via make_device when nothing native gets there. Audition them with the leader, then stop; Kumi's search picks them up. End with one line starting \"Tried:\" that says what you tried.",state.goal,state.generation,state.rendered)
}
