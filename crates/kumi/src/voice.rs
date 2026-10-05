use crate::config::{read_settings, write_settings};
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use kumi_common::abort::{Aborted, Signal};
use kumi_runtime::{
    core::errors::RuntimeError,
    system::{self, Env},
    video::programs::{self, FfmpegOptions, OnFetch},
    voice::{self, Heard, ListenOptions, Listening, PrepareVoiceOptions, VoiceFailure, VoiceOptions, WriteDownOptions},
};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, Mutex},
};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceChoices {
    pub send: bool,
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microphone: Option<String>,
}
#[derive(Debug, Clone, Default)]
pub struct VoiceChange {
    pub send: Option<bool>,
    pub language: Option<String>,
    pub microphone: Option<Option<String>>,
}
#[derive(Clone)]
pub struct VoiceIo {
    pub signal: Signal,
    pub on_fetch: OnFetch,
    pub on_progress: OnFetch,
}
#[derive(Default)]
pub struct VoiceControlOptions {
    pub env: Option<Env>,
    pub tools_dir: String,
    pub settings_file: String,
    pub open: Option<Rc<dyn Fn(&str)>>,
    pub platform: Option<String>,
}
type Prepared = Shared<LocalBoxFuture<'static, ()>>;
struct State {
    env: Env,
    platform: String,
    tools_dir: String,
    settings_file: String,
    language: String,
    open: Option<Rc<dyn Fn(&str)>>,
    ready: RefCell<Option<(String, Prepared)>>,
    generation: Cell<u64>,
    progress: Arc<Mutex<Option<OnFetch>>>,
}
#[derive(Clone)]
pub struct VoiceControl(Rc<State>);
pub fn system_language(env: &Env) -> String {
    let tag = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| env.get(key).filter(|v| !v.is_empty()).cloned())
        .unwrap_or_else(kumi_common::js::string::default_locale);
    let mut end = 0;
    for c in tag.chars() {
        if !c.is_ascii_alphabetic() {
            break;
        }
        end += 1;
    }
    if (2..=3).contains(&end) && (tag.len() == end || tag.as_bytes().get(end).is_some_and(|c| b"-_.@".contains(c))) {
        tag[..end].to_ascii_lowercase()
    } else {
        "en".into()
    }
}
pub fn create_voice_control(options: VoiceControlOptions) -> VoiceControl {
    let env = options.env.unwrap_or_else(system::process_env);
    let language = system_language(&env);
    VoiceControl(Rc::new(State {
        env,
        language,
        platform: options.platform.unwrap_or_else(|| system::platform().into()),
        tools_dir: options.tools_dir,
        settings_file: options.settings_file,
        open: options.open,
        ready: RefCell::new(None),
        generation: Cell::new(0),
        progress: Arc::new(Mutex::new(None)),
    }))
}
impl VoiceControl {
    pub fn system_language(&self) -> &str {
        &self.0.language
    }
    pub fn choices(&self) -> VoiceChoices {
        let voice = read_settings(&self.0.settings_file).voice.unwrap_or_default();
        VoiceChoices {
            send: voice.send == Some(true),
            language: voice.language.unwrap_or_else(|| self.0.language.clone()),
            microphone: voice.microphone.filter(|m| !m.is_empty()),
        }
    }
    pub fn choose(&self, change: VoiceChange) -> Result<(), RuntimeError> {
        let mut settings = read_settings(&self.0.settings_file);
        let mut voice = settings.voice.unwrap_or_default();
        if let Some(send) = change.send {
            voice.send = send.then_some(true)
        }
        if let Some(language) = change.language {
            voice.language = Some(language)
        }
        if let Some(mic) = change.microphone {
            voice.microphone = mic.filter(|m| !m.is_empty())
        }
        settings.voice = Some(voice);
        write_settings(&self.0.settings_file, &serde_json::to_value(settings).expect("settings"))
    }
    fn options(&self, io: &VoiceIo) -> VoiceOptions {
        VoiceOptions {
            env: Some(self.0.env.clone()),
            tools_dir: self.0.tools_dir.clone(),
            platform: Some(self.0.platform.clone()),
            signal: Some(io.signal.clone()),
            on_fetch: Some(io.on_fetch.clone()),
            on_progress: Some(io.on_progress.clone()),
        }
    }
    fn prepare(&self, io: &VoiceIo, language: String) {
        *self.0.progress.lock().unwrap() = Some(io.on_progress.clone());
        if self.0.ready.borrow().as_ref().is_some_and(|r| r.0 == language) {
            return;
        }
        let generation = self.0.generation.get() + 1;
        self.0.generation.set(generation);
        let progress = self.0.progress.clone();
        let mut options = self.options(io);
        options.signal = None;
        options.on_progress = Some(Arc::new(move |text| {
            let callback = progress.lock().unwrap().clone();
            if let Some(callback) = callback {
                callback(text)
            }
        }));
        let weak = Rc::downgrade(&self.0);
        let chosen = language.clone();
        let done = async move {
            if voice::prepare_voice(PrepareVoiceOptions { voice: options, language: Some(chosen) }).await.is_err() {
                if let Some(state) = weak.upgrade() {
                    if state.generation.get() == generation {
                        state.ready.borrow_mut().take();
                    }
                }
            }
        }
        .boxed_local()
        .shared();
        *self.0.ready.borrow_mut() = Some((language, done.clone()));
        tokio::task::spawn_local(done);
    }
    pub async fn listen(&self, io: VoiceIo) -> Result<Listening, VoiceFailure> {
        let choices = self.choices();
        let listening = voice::listen(ListenOptions { voice: self.options(&io), microphone: choices.microphone, ffmpeg: None }).await?;
        self.prepare(&io, choices.language);
        Ok(listening)
    }
    pub async fn write_down(&self, heard: Heard, io: VoiceIo, names: Vec<String>) -> Result<String, VoiceFailure> {
        let language = self.choices().language;
        *self.0.progress.lock().unwrap() = Some(io.on_progress.clone());
        let ready = self.0.ready.borrow().as_ref().filter(|r| r.0 == language).map(|r| r.1.clone());
        if let Some(ready) = ready {
            tokio::select! {_=ready=>{},_=io.signal.cancelled()=>return Err(Aborted.into())}
        }
        voice::write_down(heard, WriteDownOptions { voice: self.options(&io), language: Some(language), names }).await
    }
    pub async fn microphones(&self) -> Result<Vec<String>, VoiceFailure> {
        let ffmpeg = programs::find_ffmpeg(FfmpegOptions {
            env: Some(self.0.env.clone()),
            tools_dir: Some(self.0.tools_dir.clone()),
            installed_only: true,
            ..Default::default()
        })
        .await?;
        Ok(match ffmpeg {
            Some(ffmpeg) => voice::microphone::list_microphones(&ffmpeg, Some(&self.0.platform)).await,
            None => vec![],
        })
    }
    pub fn has_privacy(&self) -> bool {
        self.0.open.is_some() && matches!(self.0.platform.as_str(), "darwin" | "win32")
    }
    pub fn open_privacy(&self) {
        if let Some(open) = &self.0.open {
            match self.0.platform.as_str() {
                "darwin" => open("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"),
                "win32" => open("ms-settings:privacy-microphone"),
                _ => {}
            }
        }
    }
}
