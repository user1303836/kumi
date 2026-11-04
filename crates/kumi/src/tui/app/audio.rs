//! Native audio settings. Keys use the existing key-box actions and never enter the composer/history.
use super::super::{render::Cursor, width::text_width};
use super::draw_panel::{say, Line};
use super::drawing::st;
use super::panels::{inert, item};
use super::*;
use kumi_agent::core::protocol::Secret;
use kumi_ai::audio::{Endpoint, LocalProtocol, Task};

impl TuiApp {
    pub(super) async fn open_audio(&self) -> Result<(), RuntimeError> {
        let status = self.session().op(op::AudioStatus {}).await?;
        let mut items = vec![
            inert("Powered by Mirelo · separate from conversation models"),
            inert("Each create spends credits and uploads supplied media."),
            inert("Off blocks Mirelo, even for explicit requests."),
            inert("Training: your Mirelo account settings (see the guide)."),
            item(
                if status.mirelo_configured { "Replace Mirelo API key" } else { "Connect an existing Mirelo account" },
                "key",
                format!(
                    "{} · submitting a key acknowledges paid use for On tasks",
                    status.key_source.as_deref().unwrap_or("Not connected")
                ),
            ),
            item("Remove saved Mirelo key", "remove", "Also revokes onboarding; an environment key alone does not enable Mirelo"),
        ];
        items.push(inert(if status.config.acknowledged {
            "Mirelo onboarding acknowledged. Only tasks switched On may use Mirelo."
        } else {
            "Mirelo is not enabled: a saved or environment key alone is not consent."
        }));
        if status.mirelo_configured && !status.config.acknowledged {
            items.push(item(
                "Enable existing Mirelo key…",
                "enable-existing",
                "Review the upload and paid-use notice, then explicitly confirm",
            ));
        }
        if let Err(error) = status.config.validate() {
            items.push(inert(format!("Configuration needs attention: {}", self.clean(&error.message(), 300))));
        }
        for task in Task::ALL {
            let config = status.config.task(task);
            items.push(item(
                task.label(),
                task.as_str(),
                format!(
                    "Allow Mirelo: {} · alternative: {} · fallback: {}",
                    if config.auto_mirelo { "On" } else { "Off" },
                    config.local.as_ref().map(|e| e.url.as_str()).unwrap_or("not configured"),
                    config.fallback,
                ),
            ));
        }
        self.pick(Picker::new("Audio models", items), |app, picked| async move {
            match picked.value.as_deref() {
                Some("key") => app.open_audio_field(AudioField::MireloKey, String::new()),
                Some("enable-existing") => app.confirm_mirelo_onboarding(),
                Some("remove") => {
                    app.session().op(op::RemoveMireloKey {}).await?;
                    app.notice("Saved key removed; Mirelo disabled until onboarding again. Task switches are unchanged.", NoticeTone::Info);
                    app.close_panel();
                    app.open_audio().await?;
                }
                Some(value) => {
                    if let Some(task) = Task::parse(value) {
                        app.open_audio_task(task).await?;
                    }
                }
                None => {}
            }
            Ok(())
        });
        Ok(())
    }

    fn confirm_mirelo_onboarding(&self) {
        let items = vec![
            inert("Each create spends Mirelo credits."),
            inert("Supplied media is uploaded to Mirelo."),
            inert("On tasks may use Mirelo automatically."),
            inert("Off blocks Mirelo, even for explicit requests."),
            inert("Free accounts cannot opt out of training."),
            inert("Paid accounts: opt out in Mirelo account settings."),
            item("Cancel", "cancel", "Leave Mirelo disabled"),
            item("Enable Mirelo for On tasks", "confirm", "Validate the existing key; environment keys are not copied into auth.json"),
        ];
        self.pick(Picker::new("Enable paid Mirelo use?", items), |app, picked| async move {
            if picked.value.as_deref() == Some("confirm") {
                app.session().op(op::AcknowledgeMirelo {}).await?;
                app.notice("Mirelo onboarding acknowledged. Off tasks remain off.", NoticeTone::Info);
                app.close_panel();
                app.close_panel();
                app.open_audio().await?;
            } else {
                app.close_panel();
            }
            Ok(())
        });
    }

    async fn open_audio_task(&self, task: Task) -> Result<(), RuntimeError> {
        let config = self.session().op(op::AudioStatus {}).await?.config.task(task);
        let endpoint = config.local.as_ref();
        let mut items = vec![
            item(
                format!("Allow Mirelo: {}", if config.auto_mirelo { "On" } else { "Off" }),
                "auto",
                "On allows Mirelo after onboarding; Off never uses Mirelo, even when explicitly requested",
            ),
            item(
                format!("Use alternative as fallback: {}", config.fallback),
                "fallback",
                "Only safe pre-submission failures can fall back; never repeats a paid job",
            ),
            item(
                "Alternative endpoint URL",
                "url",
                endpoint.map(|e| e.url.as_str()).unwrap_or("Not configured — enter an endpoint before enabling fallback"),
            ),
            item(
                "Alternative protocol",
                "protocol",
                endpoint.map(|e| protocol_name(e.protocol)).unwrap_or(if task == Task::AudioToMidi {
                    "MuScriptor (default once URL is entered)"
                } else {
                    "Mirelo-compatible (requires explicit model release)"
                }),
            ),
            item(
                "Alternative model release",
                "model",
                endpoint.and_then(|e| e.model.as_deref()).unwrap_or("Not set — required for Mirelo-compatible SFX"),
            ),
            item("Set alternative API key", "key", "Optional; hidden, owner-only, never written to settings.json"),
            item("Remove alternative API key", "remove-key", "Removes this task's saved authentication only"),
            item("Remove alternative endpoint", "remove-endpoint", "Also turns fallback off; does not change your Mirelo permission"),
        ];
        if task == Task::AudioToMidi {
            items.push(inert("MuScriptor: run uvx muscriptor serve; paste the server URL above."));
            items.push(inert("Accept its gated CC-BY-NC-4.0 model license yourself; Kumi does not download models."));
        }
        items.push(inert("Settings apply now. Endpoint availability is checked when used, not when opening this panel."));
        self.pick(Picker::new(format!("Audio · {}", task.label()), items), move |app, picked| async move {
            let mut config = app.session().op(op::AudioStatus {}).await?.config;
            let mut next = config.task(task);
            match picked.value.as_deref() {
                Some("auto") => next.auto_mirelo = !next.auto_mirelo,
                Some("fallback") => {
                    if next.local.is_none() {
                        return Err(RuntimeError::plain("Configure an alternative endpoint first; fallback remains off."));
                    }
                    next.fallback = !next.fallback;
                }
                Some("url") => {
                    app.open_audio_field(
                        AudioField::Url(task),
                        next.local.map(|e| e.url).unwrap_or_else(|| {
                            if task == Task::AudioToMidi {
                                "http://127.0.0.1:8222".into()
                            } else {
                                String::new()
                            }
                        }),
                    );
                    return Ok(());
                }
                Some("protocol") => {
                    if next.local.is_none() {
                        return Err(RuntimeError::plain("Enter the endpoint URL first. MIDI defaults to native MuScriptor."));
                    }
                    app.open_audio_protocol(task);
                    return Ok(());
                }
                Some("model") => {
                    if next.local.is_none() {
                        return Err(RuntimeError::plain("Enter the alternative endpoint URL first."));
                    }
                    app.open_audio_field(AudioField::Model(task), next.local.and_then(|e| e.model).unwrap_or_default());
                    return Ok(());
                }
                Some("key") => {
                    app.open_audio_field(AudioField::LocalKey(task), String::new());
                    return Ok(());
                }
                Some("remove-key") => {
                    app.session().op(op::SaveLocalAudioKey { task, key: Secret(String::new()) }).await?;
                    return Ok(());
                }
                Some("remove-endpoint") => {
                    next.local = None;
                    next.fallback = false;
                }
                _ => return Ok(()),
            }
            config.tasks.insert(task, next);
            app.session().op(op::ConfigureAudio { config }).await?;
            app.close_panel();
            app.open_audio_task(task).await
        });
        Ok(())
    }

    fn open_audio_protocol(&self, task: Task) {
        let mut items = vec![item(
            "Mirelo-compatible",
            "mirelo",
            "A server implementing the documented Mirelo API; select an explicit SFX model release",
        )];
        if task == Task::AudioToMidi {
            items.insert(0, item("MuScriptor", "muscriptor", "Native /transcribe endpoint; audio-to-MIDI only"));
        }
        self.pick(Picker::new("Alternative protocol", items), move |app, picked| async move {
            let mut config = app.session().op(op::AudioStatus {}).await?.config;
            let mut next = config.task(task);
            let endpoint = next.local.as_mut().ok_or_else(|| RuntimeError::plain("Enter an endpoint URL first."))?;
            endpoint.protocol =
                if picked.value.as_deref() == Some("muscriptor") { LocalProtocol::Muscriptor } else { LocalProtocol::Mirelo };
            config.tasks.insert(task, next);
            app.session().op(op::ConfigureAudio { config }).await?;
            app.close_panel();
            app.close_panel();
            app.open_audio_task(task).await
        });
    }

    fn open_audio_field(&self, field: AudioField, value: String) {
        self.open_overlay(AudioFieldPanel { field, value, checking: false, status: None });
    }

    async fn save_audio_field(&self, field: AudioField, value: &str) -> Result<(), RuntimeError> {
        match field {
            AudioField::MireloKey => {
                self.session().op(op::SaveMireloKey { key: Secret(value.into()) }).await?;
            }
            AudioField::LocalKey(task) => {
                self.session().op(op::SaveLocalAudioKey { task, key: Secret(value.into()) }).await?;
            }
            AudioField::Url(task) | AudioField::Model(task) => {
                let mut config = self.session().op(op::AudioStatus {}).await?.config;
                let mut next = config.task(task);
                match field {
                    AudioField::Url(_) => {
                        let endpoint = next.local.get_or_insert_with(|| Endpoint {
                            url: String::new(),
                            protocol: if task == Task::AudioToMidi { LocalProtocol::Muscriptor } else { LocalProtocol::Mirelo },
                            model: None,
                        });
                        endpoint.url = value.trim().into();
                    }
                    AudioField::Model(_) => {
                        next.local.as_mut().ok_or_else(|| RuntimeError::plain("Enter the endpoint URL first."))?.model =
                            (!value.trim().is_empty()).then(|| value.trim().into());
                    }
                    _ => unreachable!(),
                }
                config.tasks.insert(task, next);
                config.validate()?;
                self.session().op(op::ConfigureAudio { config }).await?;
            }
        }
        Ok(())
    }
}

fn protocol_name(protocol: LocalProtocol) -> &'static str {
    match protocol {
        LocalProtocol::Muscriptor => "MuScriptor",
        LocalProtocol::Mirelo => "Mirelo-compatible",
    }
}

#[derive(Clone, Copy)]
enum AudioField {
    MireloKey,
    LocalKey(Task),
    Url(Task),
    Model(Task),
}
impl AudioField {
    fn secret(self) -> bool {
        matches!(self, Self::MireloKey | Self::LocalKey(_))
    }
    fn title(self) -> &'static str {
        match self {
            Self::MireloKey => "Mirelo API key",
            Self::LocalKey(_) => "Alternative API key",
            Self::Url(_) => "Alternative endpoint URL",
            Self::Model(_) => "Alternative model release",
        }
    }
    fn task(self) -> Option<Task> {
        match self {
            Self::MireloKey => None,
            Self::LocalKey(task) | Self::Url(task) | Self::Model(task) => Some(task),
        }
    }
}

/// Uses the established key-box bindings for a single-line value, masked when it is a credential.
struct AudioFieldPanel {
    field: AudioField,
    value: String,
    checking: bool,
    status: Option<String>,
}
impl Overlay<TuiApp> for AudioFieldPanel {
    fn handle(&mut self, app: &TuiApp, event: InputEvent) -> Handled {
        match &event {
            InputEvent::Key { .. } => {
                for action in app.actions(Scope::KeyBox, &event) {
                    match action {
                        "key.cancel" => return Handled::Close,
                        "key.erase" if !self.checking => {
                            self.value.pop();
                        }
                        "key.clear" if !self.checking => self.value.clear(),
                        "key.check" if !self.checking => {
                            if self.field.secret() && self.value.is_empty() {
                                break;
                            }
                            self.checking = true;
                            self.status = Some("Checking and saving…".into());
                            let (field, value) = (self.field, self.value.clone());
                            if let Some(panel) = app.0.state.borrow().overlays.top() {
                                app.task(move |app| async move {
                                    let result = app.save_audio_field(field, &value).await;
                                    if let Err(error) = result {
                                        let mut secrets = app.0.state.borrow().secrets.clone();
                                        if field.secret() {
                                            secrets.push(value);
                                        }
                                        if let Some(editor) = panel.borrow_mut().downcast_mut::<AudioFieldPanel>() {
                                            editor.checking = false;
                                            editor.status = Some(safe_error_message(Some(&error.message()), &secrets));
                                        }
                                        app.0.scheduler.request();
                                        return Ok(());
                                    }
                                    if field.secret() {
                                        let mut state = app.0.state.borrow_mut();
                                        state.secrets.push(value.clone());
                                        state.stream.add_secret(value);
                                    }
                                    let was_open = app.0.state.borrow_mut().overlays.remove(&panel);
                                    if let Some(editor) = panel.borrow_mut().downcast_mut::<AudioFieldPanel>() {
                                        editor.value.clear();
                                    }
                                    if was_open {
                                        match field.task() {
                                            Some(task) => app.open_audio_task(task).await?,
                                            None => app.open_audio().await?,
                                        }
                                    }
                                    Ok(())
                                });
                            }
                        }
                        _ => continue,
                    }
                    break;
                }
            }
            InputEvent::Text { text } | InputEvent::Paste { text } if !self.checking => {
                let clean: String = text.chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect();
                self.value = head(&(self.value.clone() + &clean), 4096);
                self.status = None;
            }
            _ => {}
        }
        Handled::Kept
    }
    fn draw(&mut self, app: &TuiApp, screen: &mut Screen, area: Area) -> Option<Cursor> {
        let width = super::draw_panel::panel_width(76, area.boxed);
        let inner = width - 4;
        let mut lines = vec![Line::new(self.field.title(), st::TITLE)];
        let help = match self.field {
            AudioField::MireloKey => {
                "Powered by Mirelo. Submitting this key acknowledges paid use for On tasks: each create spends credits and uploads supplied media. Training follows https://mirelo.ai/studio/settings/profile. Off tasks remain off. Key checked with /v3/me, then saved owner-only. Get a key: https://mirelo.ai/studio/api-keys."
            }
            AudioField::LocalKey(_) => "Optional authentication, saved owner-only outside settings. The key stays hidden.",
            AudioField::Url(_) => {
                "Paste the server's base URL. Remote endpoints require HTTPS; loopback HTTP is allowed. Never embed credentials in a URL."
            }
            AudioField::Model(_) => {
                "Enter the explicit model release your Mirelo-compatible server supports. MuScriptor does not need a model name."
            }
        };
        say(&mut lines, help, st::DIM, inner);
        let shown =
            if self.field.secret() { "•".repeat(self.value.chars().count().min(inner.max(8) as usize)) } else { self.value.clone() };
        let mut line = Line::new(&shown, st::BRIGHT);
        line.band = true;
        lines.push(line);
        let cursor = (!self.checking).then_some((lines.len() - 1, text_width(&shown).min(inner - 1)));
        if let Some(status) = &self.status {
            say(&mut lines, status, st::WARN, inner);
        }
        app.draw_lines(screen, area.box_top, width, &lines, cursor)
    }
    fn hint(&self, app: &TuiApp) -> String {
        app.keys_hint(&[(&["key.check"], "to save"), (&["key.clear"], "to clear"), (&["key.cancel"], "to cancel")])
    }
    fn closed(&mut self, _: &TuiApp) {
        self.value.clear();
    }
    fn busy(&self, _: &TuiApp) -> bool {
        true
    }
}
