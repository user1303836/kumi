//! Render rigs, listening devices, auditions, and held goal passes: Live's side of the listening tools.
mod ears;
mod ears_pass;
mod goal;
mod groove;
mod host;
mod judge;
mod listen;
pub mod main_chain;
mod pass;
mod probe;
mod rig;
mod roles;
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
use crate::ears::link::EarsLink;
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use indexmap::IndexMap;
use kumi_agent::core::contracts::*;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{json::stringify, number::to_fixed},
    time::now_ms,
};
use kumi_listening::{
    engine::{Listening, Named},
    renderer::{ListenFlag, ListenNow},
    round::Round,
};
use kumi_media::audio::{matching::Focus, tools::ResolveAudio};
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;
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
/// Live as the listening tools meet it: its bridge, its render rigs and Kumi Ears. The tools themselves are
/// `Listening`'s; this answers them (see `host.rs`).
pub struct LiveRenderer {
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
    rendering: ListenFlag,
    /// Held for the whole of each Live tool call. A stopped answer's call goes on putting Live back (taking a round
    /// back, closing a render, removing copies), and the next call, or the next answer, waits for it.
    busy: tokio::sync::Mutex<()>,
    best_steps: RefCell<Vec<String>>,
}
/// The listening tools over Live: the tools and their state are `Listening`'s (a judged run, a tune), Live is what they
/// listen through. A tool call comes in here and goes to `Listening`; what only Live has is `LiveRenderer`'s.
pub struct Rendering {
    live: Rc<LiveRenderer>,
    listening: Rc<Listening>,
}
impl std::ops::Deref for Rendering {
    type Target = LiveRenderer;
    fn deref(&self) -> &LiveRenderer {
        &self.live
    }
}
pub use judge::{gain_device, gain_parameter};
pub use kumi_listening::tools::{FormRequest, GoalRequest, GrooveRequest, JudgeRequest, SoundRequest, TuneHow, TuneRequest};
/// Kumi playing the Set to listen (see `ListenNow`).
pub type RenderingNow<'a> = ListenNow<'a>;

impl LiveRenderer {
    /// Kumi starts playing the Set to listen (see `RenderingNow`).
    pub fn rendering_now(&self) -> RenderingNow<'_> {
        self.rendering.begin()
    }
    /// Whether Kumi is playing the Set to listen now.
    pub fn is_rendering(&self) -> bool {
        self.rendering.is_on()
    }
    /// Live, for one tool call: once a stopped answer's call has finished putting Live back, and Esc while waiting
    /// stops this one before it starts.
    pub async fn hold(&self, signal: &Signal) -> Result<tokio::sync::MutexGuard<'_, ()>, RuntimeError> {
        if let Ok(held) = self.busy.try_lock() {
            return Ok(held);
        }
        // Said, so a call that waits doesn't look stuck.
        self.tell("Finishing the stopped answer's clean-up in Live first", Some(true));
        let held = tokio::select! {
            biased;
            _ = signal.cancelled() => Err(RuntimeError::Aborted),
            held = self.busy.lock() => Ok(held),
        };
        self.tell(if held.is_ok() { "Live is free again" } else { "Stopped waiting for Live" }, Some(false));
        held
    }
    /// Resolves once no Live tool call is running: a stopped answer's has finished putting Live back.
    pub async fn settled(&self) {
        drop(self.busy.lock().await);
    }
    /// Whether a Live tool call is running now.
    pub fn is_busy(&self) -> bool {
        self.busy.try_lock().is_err()
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
            rendering: ListenFlag::default(),
            busy: tokio::sync::Mutex::new(()),
            best_steps: RefCell::new(vec![]),
        })
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
        if self.rendering.is_on() {
            return Ok(None);
        }
        let saved_path = pending.get("path").and_then(Value::as_str).filter(|s| !s.is_empty());
        if saved_path.map_or_else(|| pending.get("set").and_then(Value::as_str) != Some(identity), |saved| Some(saved) != path) {
            return Ok(None);
        }
        // Main's level is only in a journal an older Kumi left: it turned Main down to listen.
        let volume = pending.get("volume").and_then(Value::as_f64);
        if let Some(volume) = volume {
            if !self.history.quietly(None, self.put_main_back(volume, signal)).await {
                return Ok(None);
            }
        }
        self.restore.as_ref().unwrap().clear();
        let scratch = pending.get("scratch").and_then(Value::as_array).filter(|s| !s.is_empty());
        let delete = scratch.map(|items| {
            format!(" Delete its render tracks if they're still there: {}.", items.iter().map(js_string).collect::<Vec<_>>().join(", "))
        });
        let text = match (volume, delete) {
            (Some(volume), delete) => {
                format!("Kumi's last render was cut off, so it put Main back to {}.{}", fader_db(volume), delete.unwrap_or_default())
            }
            (None, Some(delete)) => format!("Kumi's last listen was cut off.{delete}"),
            (None, None) => return Ok(None),
        };
        self.tell(&text, None);
        Ok(Some(text))
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
impl Rendering {
    pub fn new(
        history: Rc<History>,
        observer: Rc<Observer>,
        options: &AbletonOptions,
        step: RenderStep,
        clip_file: ResolveAudio,
    ) -> Rc<Self> {
        let live = LiveRenderer::new(history, observer, options, step, clip_file);
        // The tools reach Live through its renderer; they hold no more of Rendering than that.
        let host: Rc<dyn kumi_listening::renderer::Renderer> = Rc::new(host::LiveHost(live.clone()));
        let listening = Listening::new(
            host,
            options.on_judge.clone(),
            options.on_audition.clone(),
            options.listener.clone(),
            options.references.clone(),
            options.search_memory.clone(),
        );
        Rc::new(Self { live, listening })
    }
    /// A match run continues the answer's rounds; a new answer starts a fresh score history.
    pub fn reset_turn(&self, continuing: bool) {
        if !continuing {
            self.listening.new_answer(self.applied_ids());
        }
    }
    /// The producer's words came into the answer under way: a judged or groove run's changes from here may be theirs,
    /// so they aren't a round's to take back.
    pub fn steered(&self) {
        self.listening.steered(self.applied_ids());
    }
    /// One of the listening tools (judge, tune, sound, form, groove, audition, render) as the model calls it; None for
    /// any other tool.
    pub async fn call(&self, name: &str, input: JsonObject, signal: Signal) -> Option<Result<ToolResult, RuntimeError>> {
        self.listening.call(name, input, signal).await
    }
    pub async fn judge(&self, request: &JudgeRequest, original: Signal) -> Result<Result<Round, String>, RuntimeError> {
        self.listening.judge(request, original).await
    }
    /// The last round of the current (or last) judged run.
    pub fn judged(&self) -> Option<Round> {
        self.listening.judged()
    }
    /// A run's track and focus as it starts (see `Named`).
    pub async fn named(&self, track: Option<&str>, focus: Option<&str>, signal: Signal) -> Named {
        self.listening.named(track, focus, signal).await
    }
    /// The names a run's track and focus go by now.
    pub fn names_now(&self, named: &Named) -> (Option<String>, Option<String>) {
        self.listening.names_now(named)
    }
    pub async fn groove(&self, request: &GrooveRequest, original: Signal) -> Result<Result<Round, String>, RuntimeError> {
        self.listening.groove(request, original).await
    }
    pub async fn form(&self, request: &FormRequest, original: Signal) -> Result<Result<Value, String>, RuntimeError> {
        self.listening.form(request, original).await
    }
    pub async fn sound(&self, request: &SoundRequest, original: Signal) -> Result<Result<Value, String>, RuntimeError> {
        self.listening.sound(request, original).await
    }
    pub async fn tune(&self, request: &TuneRequest, original: Signal) -> Result<Result<Round, String>, RuntimeError> {
        self.listening.tune(request, original).await
    }
    /// A round's next fix names a mapped plug-in's role when one on the run's track does the job.
    pub async fn name_plugin_roles(&self, round: &mut Round, signal: Signal) {
        self.listening.name_plugin_roles(round, signal).await
    }
    pub async fn audition(&self, request: &AuditionRequest, original: Signal) -> Result<Result<AuditionResult, String>, RuntimeError> {
        self.listening.audition(request, original).await
    }
    /// How many auditions the answer has made.
    pub fn round_count(&self) -> usize {
        self.listening.round_count()
    }
    pub async fn hear_in_set(&self, request: &HearRequest, original: Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        self.live.hear_in_set(request, original).await
    }
    pub async fn open_goal(&self, request: &AuditionRequest, original: Signal) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        self.live.open_goal(&self.listening, request, original).await
    }
}
fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}
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
async fn delay(ms: f64, signal: Signal) -> Result<(), RuntimeError> {
    signal.check()?;
    let ms = if !ms.is_finite() || ms < 1. || ms > i32::MAX as f64 { 1 } else { ms.trunc() as u64 };
    tokio::select! { biased; _ = signal.cancelled() => Err(RuntimeError::Aborted), _ = tokio::time::sleep(Duration::from_millis(ms)) => Ok(()) }
}
