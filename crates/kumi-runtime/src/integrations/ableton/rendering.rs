//! Quiet render rigs, listening devices, auditions, and held goal passes.
mod ears;
mod ears_pass;
mod form;
mod goal;
mod groove;
mod judge;
mod listen;
mod pass;
mod probe;
mod rig;
mod roles;
mod sound;
mod tune;
use super::{
    audition::{restore_store, RestoreStore},
    bridge_version::at_least,
    connection::LiveConnection,
    history::History,
    observation::Observer,
    options::{AbletonOptions, EarsSetup},
    views::ViewHost,
};
use crate::{
    audio::{
        self,
        analyze::{Analysis, AnalyzeOptions},
        matching::Focus,
        tools::ResolveAudio,
    },
    core::{contracts::*, errors::RuntimeError},
    ears::link::EarsLink,
};
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use indexmap::IndexMap;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{json::stringify, number::to_fixed},
    time::now_ms,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

pub type RenderStep = Rc<dyn Fn(String, JsonObject, Signal) -> LocalBoxFuture<'static, Result<JsonObject, RuntimeError>>>;
type OpeningEars = Rc<dyn Fn() -> LocalBoxFuture<'static, Result<Rc<dyn EarsLink>, RuntimeError>>>;
type EarsFuture = Shared<LocalBoxFuture<'static, Option<Rc<dyn EarsLink>>>>;
pub struct Rendering {
    pub history: Rc<History>,
    pub observer: Rc<Observer>,
    step: RenderStep,
    clip_file: ResolveAudio,
    restore: Option<RestoreStore>,
    /// Scratch copies a search made and hasn't removed yet (beside the render journal): swept when Live is back.
    copies_journal: Option<PathBuf>,
    /// The prefixes of copies a search of this process is using right now: never swept.
    copies_live: RefCell<Vec<String>>,
    on_action: Option<Rc<dyn Fn(ActionEvent)>>,
    on_audition: Option<Rc<dyn Fn(AuditionEvent)>>,
    on_judge: Option<Rc<dyn Fn(crate::listening::round::Round)>>,
    change_timeout_ms: u64,
    user_library: Option<String>,
    ears_disabled: bool,
    open_ears: Option<OpeningEars>,
    ears_setup: RefCell<Option<EarsFuture>>,
    ears_refused: Cell<bool>,
    /// When Kumi last couldn't set its listening device up (Live's Browser never showed it, say): an
    /// audition in the next EARS_RETRY_MS records instead, rather than waiting for the same failure.
    ears_failed_at: Cell<Option<i64>>,
    ears_folder: PathBuf,
    rendering: Cell<bool>,
    rounds: Cell<usize>,
    best: Cell<Option<f64>>,
    told_quietly: Cell<bool>,
    reference_cache: RefCell<IndexMap<String, Analysis>>,
    best_steps: RefCell<Vec<String>>,
    /// The judged run under way (or the last one).
    judge: RefCell<Option<judge::JudgeRun>>,
    /// The groove run under way (or the last one).
    groove: RefCell<Option<groove::GrooveRun>>,
    listener_source: Option<super::options::ListenerSource>,
    /// Measured references, read by name for the judge.
    references: Option<Rc<crate::references::store::ReferenceStore>>,
    /// The listening model, once looked for (None inside: there's none).
    /// Whether the run's listens are heard by the learned models too: the style model, the effects model.
    embedding_wanted: Cell<(bool, bool)>,
    /// The listening model as last looked up (found, a definite none, or why the lookup failed), and when.
    listener: RefCell<Option<(Result<Option<Rc<dyn crate::listening::listener::Listener>>, String>, i64)>>,
}
pub use form::FormRequest;
pub use groove::GrooveRequest;
pub use judge::{GoalRequest, JudgeRequest};
pub use sound::SoundRequest;
pub use tune::{TuneHow, TuneRequest};
impl Rendering {
    /// Kumi starts playing the Set for itself, Main down: what Live plays meanwhile isn't heard by the producer. True
    /// when a render was already running, which nothing changes.
    pub fn begin_rendering(&self) -> bool {
        if self.rendering.replace(true) {
            return true;
        }
        self.history.connection.rendering.set(true);
        false
    }
    /// Kumi's render is over: what Live plays is the producer's again.
    pub fn end_rendering(&self) {
        self.rendering.set(false);
        self.history.connection.rendering.set(false);
    }
    pub fn new(
        history: Rc<History>,
        observer: Rc<Observer>,
        options: &AbletonOptions,
        step: RenderStep,
        clip_file: ResolveAudio,
    ) -> Rc<Self> {
        let ears_folder = std::env::temp_dir().join("kumi-ears").join(history.connection.generation.chars().take(8).collect::<String>());
        Rc::new(Self {
            history,
            observer,
            step,
            clip_file,
            restore: options.restore_file.as_ref().map(restore_store),
            copies_journal: options.restore_file.as_ref().map(|file| PathBuf::from(format!("{file}.copies"))),
            copies_live: RefCell::new(vec![]),
            on_action: options.on_action.clone(),
            on_audition: options.on_audition.clone(),
            on_judge: options.on_judge.clone(),
            change_timeout_ms: options.change_timeout_ms.unwrap_or(30_000),
            user_library: options.user_library.clone(),
            ears_disabled: matches!(options.ears, Some(EarsSetup::Disabled)),
            open_ears: match &options.ears {
                Some(EarsSetup::Open(open)) => Some(open.clone()),
                _ => None,
            },
            ears_setup: RefCell::new(None),
            ears_refused: Cell::new(false),
            ears_failed_at: Cell::new(None),
            ears_folder,
            rendering: Cell::new(false),
            rounds: Cell::new(0),
            best: Cell::new(None),
            told_quietly: Cell::new(false),
            reference_cache: RefCell::new(IndexMap::new()),
            best_steps: RefCell::new(vec![]),
            judge: RefCell::new(None),
            groove: RefCell::new(None),
            listener_source: options.listener.clone(),
            references: options.references.clone(),
            listener: RefCell::new(None),
            embedding_wanted: Cell::new((false, false)),
        })
    }
    pub fn round_count(&self) -> usize {
        self.rounds.get()
    }
    /// A match run continues the answer's rounds; a new answer starts a fresh score history.
    pub fn reset_turn(&self, continuing: bool) {
        if !continuing {
            self.rounds.set(0);
            self.best.set(None);
            // A judged run's changes are the ones since its last round: what the producer asked for in between isn't
            // a round's to take back. If that changed the sound, the run's numbers are out of date: its next judge hears
            // its bars again first.
            // An undo in between counts too: a change the run's numbers took in that isn't in the Set any more.
            let ids = self.applied_ids();
            // A groove run's the same: its rounds re-read the notes, so only its checkpoint moves.
            if let Some(run) = self.groove.borrow_mut().as_mut() {
                run.carry_on(ids.clone());
            }
            let mut guard = self.judge.borrow_mut();
            if let Some(run) = guard.as_mut() {
                let audible = |id: &String| self.history.entries.borrow().get(id).is_some_and(|entry| entry.borrow().audible());
                let made = self.applied_since(&run.checkpoint).iter().any(|(id, _)| audible(id));
                let undone = run.checkpoint.iter().any(|id| !ids.contains(id) && audible(id));
                if made || undone {
                    run.stale = true;
                }
                run.checkpoint = ids;
            }
        }
    }
    /// A restart of Live may make Max for Live available; bridge-only disconnects leave refusal intact.
    pub fn reset_live(&self) {
        self.ears_refused.set(false);
        self.ears_failed_at.set(None);
    }
    fn connection(&self) -> &LiveConnection {
        &self.history.connection
    }
    fn cleanup(&self) -> Signal {
        abort::any([self.connection().lifetime.clone(), abort::timeout(self.change_timeout_ms.saturating_mul(3))])
    }
    fn supported(&self, minimum: &str) -> bool {
        at_least(self.connection().version().as_deref(), minimum)
    }
    fn too_old(&self, minimum: &str) -> String {
        format!(
            "That needs the Ableton bridge {minimum} or later; this one is {}. Tell the producer to update it (kumi doctor says how).",
            self.connection().version().as_deref().unwrap_or("older")
        )
    }
    fn available(&self) -> bool {
        self.connection().available() && self.connection().epoch.get().is_some() && self.connection().tools().is_some()
    }
    async fn rows(&self, kind: &str, extra: Value, signal: Signal) -> Result<Vec<JsonObject>, RuntimeError> {
        self.connection().rows(kind, object(extra), signal).await.map_err(Into::into)
    }
    async fn step(&self, tool: &str, input: Value, signal: Signal) -> Result<JsonObject, RuntimeError> {
        (self.step)(tool.into(), object(input), signal).await
    }
    fn tell(&self, title: impl Into<String>, playing: Option<bool>) {
        if let Some(tell) = &self.on_action {
            let _ = catch_unwind(AssertUnwindSafe(|| tell(ActionEvent { title: title.into(), playing, recording: None })));
        }
    }
    async fn main_volume(&self, signal: Signal) -> Result<(String, Option<f64>), RuntimeError> {
        let main = self.rows("main-track", json!({"fields":["name","mixer"]}), signal).await?.into_iter().next();
        let main = main.ok_or_else(|| observation("Live didn't say where Main is."))?;
        let reference = main.get("ref").and_then(Value::as_str).ok_or_else(|| observation("Live didn't say where Main is."))?.to_owned();
        let mixer = main.get("mixer").filter(|value| !value.is_null()).cloned().unwrap_or_else(|| json!({}));
        let mixer = super::context::object(&mixer)?;
        Ok((reference, mixer.get("volume").and_then(Value::as_f64)))
    }
    async fn put_main_back(&self, volume: f64, signal: Signal) -> bool {
        for _ in 0..2 {
            let result = async {
                let (reference, current) = self.main_volume(signal.clone()).await?;
                if current.is_some_and(|current| (current - volume).abs() < 1e-4) {
                    return Ok::<_, RuntimeError>(true);
                }
                self.step("set_mixer", json!({"trackRef":reference,"volume":volume}), signal.clone()).await?;
                Ok(self.main_volume(signal.clone()).await?.1.is_some_and(|current| (current - volume).abs() < 1e-4))
            }
            .await;
            if result == Ok(true) {
                return true;
            }
        }
        false
    }
    pub async fn restore_after_crash(&self, identity: &str, path: Option<&str>, signal: Signal) -> Result<Option<String>, RuntimeError> {
        self.sweep_copies(identity, path, signal.clone()).await;
        let Some(pending) = self.restore.as_ref().and_then(RestoreStore::load) else { return Ok(None) };
        if self.rendering.get() {
            return Ok(None);
        }
        let saved_path = pending.get("path").and_then(Value::as_str).filter(|s| !s.is_empty());
        if saved_path.map_or_else(|| pending.get("set").and_then(Value::as_str) != Some(identity), |saved| Some(saved) != path) {
            return Ok(None);
        }
        let volume = pending["volume"].as_f64().unwrap();
        if !self.history.quietly(None, self.put_main_back(volume, signal)).await {
            return Ok(None);
        }
        self.restore.as_ref().unwrap().clear();
        let scratch = pending.get("scratch").and_then(Value::as_array).filter(|s| !s.is_empty());
        let text = format!(
            "Kumi's last render was cut off, so it put Main back to {}.{}",
            fader_db(volume),
            scratch
                .map(|items| format!(
                    " Delete its render tracks if they're still there: {}.",
                    items.iter().map(js_string).collect::<Vec<_>>().join(", ")
                ))
                .unwrap_or_default()
        );
        self.tell(&text, None);
        Ok(Some(text))
    }
    async fn heard_reference(&self, named: &str, request: &AuditionRequest, signal: Signal) -> Result<Analysis, RuntimeError> {
        let file = (self.clip_file)(named.into(), signal.clone()).await.ok().flatten().unwrap_or_else(|| audio::audio_path(named));
        let key = stringify(&json!([file, request.reference_from.unwrap_or(0.), request.reference_seconds, request.focus]));
        if let Some(known) = self.reference_cache.borrow().get(&key) {
            return Ok(known.clone());
        }
        let heard = audio::hear(
            &file,
            AnalyzeOptions {
                start: request.reference_from,
                seconds: request.reference_seconds,
                focus: analysis_focus(request.focus),
                signal: Some(signal),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| RuntimeError::plain(e.to_string()))?;
        let mut cache = self.reference_cache.borrow_mut();
        cache.insert(key, heard.clone());
        if cache.len() > 32 {
            cache.shift_remove_index(0);
        }
        Ok(heard)
    }
    pub async fn close(&self) {
        let setup = self.ears_setup.borrow().clone();
        if let Some(link) = match setup {
            Some(future) => future.await,
            None => None,
        } {
            link.close().await;
        }
        let _ = tokio::fs::remove_dir_all(&self.ears_folder).await;
    }
}
fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}
/// Why a render didn't start: Main's level couldn't be noted to put back after a crash.
const MAIN_UNNOTED: &str = "Kumi couldn't note Main's level to put back after a crash (is the disk full?), so it left Main as it is and didn't render. Free some space, then try again.";
fn observation(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Observation(message.into())
}
fn js_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Object(_) => "[object Object]".into(),
        Value::Array(a) => a.iter().map(|v| if v.is_null() { String::new() } else { js_string(v) }).collect::<Vec<_>>().join(","),
        _ => stringify(value),
    }
}
fn fader_db(volume: f64) -> String {
    if volume <= 0. {
        "-inf dB".into()
    } else {
        format!("{} dB", to_fixed(20. * (volume / 0.85).log10() * if volume > 0.85 { 0.3 } else { 1. }, 1))
    }
}
fn analysis_focus(focus: Option<Focus>) -> Option<String> {
    focus.map(|focus| if focus == Focus::Section { "mix" } else { "sound" }.into())
}
async fn delay(ms: f64, signal: Signal) -> Result<(), RuntimeError> {
    signal.check()?;
    let ms = if !ms.is_finite() || ms < 1. || ms > i32::MAX as f64 { 1 } else { ms.trunc() as u64 };
    tokio::select! { biased; _ = signal.cancelled() => Err(RuntimeError::Aborted), _ = tokio::time::sleep(Duration::from_millis(ms)) => Ok(()) }
}
