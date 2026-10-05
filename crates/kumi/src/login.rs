use crate::{
    config::{read_settings, AppConfig, LoginMethod},
    input::TerminalInput,
    models::{create_model_control, ModelControlOptions, OFFER_ORDER},
    spinner::{spin, step, Spinning},
    tui::tty::TtyOutput,
};
use futures::FutureExt;
use kumi_common::{
    abort::{self, Signal},
    js::{
        number::to_string,
        string::{pad_end, trim},
    },
    time::now_ms,
};
use kumi_runtime::{
    ai::http::{default_fetch, Fetch},
    auth::{
        openai_codex::{login_codex_browser, login_codex_device, read_pi_codex_login, LoginOptions, OPENAI_CODEX},
        store::{open_credential_store, valid_api_key, Credential, CredentialStore},
    },
    core::errors::RuntimeError,
    providers::{
        api_key_for,
        local::{local_servers, probe_local},
        models::{ApiKeyCheck, Transport},
        provider_info, KeySource, SignIn,
    },
    system::{self, Env},
    KUMI, KUMI_START,
};
use std::{cell::RefCell, process::Stdio, rc::Rc};
#[derive(Clone)]
pub struct LoginIo {
    pub out: Rc<dyn TtyOutput>,
    pub env: Env,
    pub signal: Signal,
    pub open_browser: Option<Rc<dyn Fn(String)>>,
    pub input: Option<Rc<dyn TerminalInput>>,
    pub fetch: Option<Rc<dyn Fetch>>,
}
#[derive(Clone)]
pub struct AuthIo {
    pub out: Rc<dyn TtyOutput>,
    pub env: Env,
    pub fetch: Option<Rc<dyn Fetch>>,
}
pub async fn login(config: &AppConfig, io: LoginIo) -> Result<(), RuntimeError> {
    let AppConfig::Login { provider, method, auth_file, pi_auth_file, settings_file } = config else {
        return Err(RuntimeError::plain("Expected a login configuration."));
    };
    let store: Rc<dyn CredentialStore> = Rc::new(open_credential_store(auth_file));
    let info = provider_info(*provider);
    if *method == LoginMethod::Key {
        let input = io.input.clone().ok_or_else(|| RuntimeError::plain("Kumi needs a terminal to ask for the key."))?;
        let prompt =
            format!("Paste your {} API key (make one at {}). It won't show as you paste: ", info.name, info.key_page.unwrap_or(""));
        let key = trim(&read_hidden(input, io.out.clone(), &prompt, io.signal.clone()).await?).to_string();
        if !valid_api_key(&key) {
            return Err(RuntimeError::plain("That doesn't look like an API key (one word of 8 or more characters); nothing was saved."));
        }
        let models = create_model_control(ModelControlOptions {
            store: store.clone(),
            settings_file: settings_file.clone(),
            env: io.env.clone(),
            changed: Rc::new(|| async { Ok(()) }.boxed_local()),
            fetch: io.fetch.clone(),
            say: None,
            installed: None,
        });
        let verdict = step(
            io.out.clone(),
            &io.env,
            &format!("Checking it with {}…", info.name),
            models.save_key(*provider, &key, Some(abort::any([io.signal.clone(), abort::timeout(20000)]))),
            true,
        )
        .await?;
        if verdict == ApiKeyCheck::Refused {
            return Err(RuntimeError::plain(format!("{} didn't accept that key; nothing was saved.", info.name)));
        }
        io.out.write(&if verdict == ApiKeyCheck::Ok {
            format!("Signed in to {}. The key is in {} (owner-only).\n", info.name, store.path().display())
        } else {
            format!(
                "Saved the key in {} (owner-only). {} didn't answer just now, so it isn't checked yet.\n",
                store.path().display(),
                info.name
            )
        });
    } else {
        let credential = if *method == LoginMethod::ImportPi {
            read_pi_codex_login(pi_auth_file).await?
        } else {
            let waiting: Rc<RefCell<Option<Spinning>>> = Rc::new(RefCell::new(None));
            let options = LoginOptions { signal: io.signal.clone(), fetch: io.fetch.clone().unwrap_or_else(default_fetch) };
            let result = if *method == LoginMethod::Device {
                login_codex_device(
                    options,
                    Rc::new({
                        let io = io.clone();
                        let waiting = waiting.clone();
                        move |prompt| {
                            io.out.write(&format!("Open {} and enter the code {}\n", prompt.url, prompt.code));
                            *waiting.borrow_mut() = Some(spin(io.out.clone(), &io.env, "Waiting for approval...", true));
                        }
                    }),
                )
                .await
            } else {
                login_codex_browser(
                    options,
                    Rc::new({
                        let io = io.clone();
                        let waiting = waiting.clone();
                        move |url| {
                            io.out.write(&format!("Sign in to ChatGPT in your browser. If it did not open, visit:\n{url}\n"));
                            if let Some(open) = &io.open_browser {
                                open(url);
                            }
                            *waiting.borrow_mut() = Some(spin(
                                io.out.clone(),
                                &io.env,
                                "Waiting for the browser... (Ctrl-C cancels; use --device on a remote machine)",
                                true,
                            ));
                        }
                    }),
                    None,
                )
                .await
            };
            if let Some(waiting) = waiting.borrow_mut().take() {
                waiting.stop();
            }
            result?
        };
        store.update(OPENAI_CODEX, Box::new(move |_| async move { Ok(Some(credential.into())) }.boxed_local())).await?;
        io.out.write(&format!("Signed in to ChatGPT (openai-codex). Saved to {} (owner-only).\n", store.path().display()));
        if *method == LoginMethod::ImportPi {
            io.out.write("Imported from Pi: Kumi and Pi now share this session, so when either refreshes it the other may need to sign in again. Run login without --from-pi for a separate session.\n");
        }
    }
    let model = io.env.get("KUMI_MODEL").cloned().or_else(|| read_settings(settings_file).model);
    if model.as_ref().is_none_or(|m| m.is_empty()) {
        io.out.write(&format!("Next: {}. It starts with {}'s first model; /model changes it.\n", *KUMI_START, info.name));
    } else if !model.as_ref().unwrap().starts_with(&format!("{provider}/")) {
        io.out.write(&format!("Kumi still talks to {}; choose one of {}'s models with /model in Kumi.\n", model.unwrap(), info.name));
    }
    Ok(())
}
pub async fn logout(config: &AppConfig, io: AuthIo) -> Result<(), RuntimeError> {
    let AppConfig::Logout { provider, auth_file } = config else { return Err(RuntimeError::plain("Expected a logout configuration.")) };
    let store = open_credential_store(auth_file);
    let info = provider_info(*provider);
    let existed = store.get(info.credential).await?.is_some();
    if existed {
        store.update(info.credential, Box::new(|_| async { Ok(None) }.boxed_local())).await?;
    }
    let shared = if info.credential == "opencode" { " (OpenCode Zen and Go share it)" } else { "" };
    io.out.write(&if existed {
        format!("Removed Kumi's {} sign-in{shared} from {}.\n", info.name, store.path().display())
    } else {
        format!("Kumi has no {} sign-in to remove.\n", info.name)
    });
    if let Some(key_env) = info.key_env.filter(|key| io.env.get(*key).is_some_and(|v| !v.is_empty())) {
        io.out.write(&format!("{key_env} is still set in your environment, and Kumi uses it; unset it to sign out fully.\n"));
    }
    Ok(())
}
pub async fn auth_status(config: &AppConfig, io: AuthIo) -> Result<(), RuntimeError> {
    let AppConfig::Auth { auth_file, settings_file } = config else { return Err(RuntimeError::plain("Expected an auth configuration.")) };
    let store = open_credential_store(auth_file);
    let mut lines = Vec::new();
    for provider in OFFER_ORDER {
        let info = provider_info(provider);
        let status = if info.sign_in == SignIn::Chatgpt {
            match store.get(OPENAI_CODEX).await? {
                Some(Credential::Oauth(c)) => {
                    let hours = ((c.expires - now_ms() as f64) / 3600000.0).floor();
                    if hours >= 1.0 {
                        format!("signed in (token valid ~{} h; refreshes automatically)", to_string(hours))
                    } else {
                        "signed in (token refreshes on next use)".into()
                    }
                }
                _ => format!("not signed in ({} login openai-codex)", *KUMI),
            }
        } else {
            match api_key_for(provider, &store, Some(&io.env)).await? {
                Some(key) if key.source == KeySource::Env => format!("API key from {}", info.key_env.unwrap_or("")),
                Some(_) => "API key saved in Kumi".into(),
                None => format!("not signed in ({} login {provider})", *KUMI),
            }
        };
        lines.push(format!("{} {status}", pad_end(provider.as_str(), 13)));
    }
    let settings = read_settings(settings_file);
    for server in local_servers(&settings.model_servers, &io.env)? {
        if probe_local(&server, Transport { fetch: io.fetch.clone(), signal: None }).await {
            lines.push(format!("{} running {}; no sign-in needed", pad_end(&server.id, 13), server.r#where));
        }
    }
    let model = io
        .env
        .get("KUMI_MODEL")
        .filter(|s| !s.is_empty())
        .map(|model| format!("{model} (from KUMI_MODEL)"))
        .or(settings.model)
        .unwrap_or("not chosen yet (Kumi starts with a signed-in provider's first model)".into());
    io.out.write(&format!(
        "{}\nModel: {model}{}\nCredential file: {auth_file}\n",
        lines.join("\n"),
        settings.effort.map(|e| format!(", effort {}", e.as_str())).unwrap_or_default()
    ));
    Ok(())
}
pub fn open_browser(url: &str) {
    let command = match system::platform() {
        "darwin" => "open",
        "win32" => "explorer",
        _ => "xdg-open",
    };
    let mut command = std::process::Command::new(command);
    command.arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    if let Ok(mut child) = command.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}
struct HiddenState {
    value: Vec<u16>,
    escape: bool,
    done: bool,
    answer: Option<tokio::sync::oneshot::Sender<Result<String, RuntimeError>>>,
}
struct HiddenGuard {
    finish: Rc<dyn Fn(Option<RuntimeError>)>,
}
impl Drop for HiddenGuard {
    fn drop(&mut self) {
        (self.finish)(Some(RuntimeError::plain("Cancelled; nothing was saved.")));
    }
}
/// Read the first line silently, with source escape/backspace/Ctrl-C behavior, restoring raw mode on every exit.
pub async fn read_hidden(
    input: Rc<dyn TerminalInput>,
    out: Rc<dyn TtyOutput>,
    prompt: &str,
    signal: Signal,
) -> Result<String, RuntimeError> {
    out.write(prompt);
    let raw = input.is_tty();
    let (send, receive) = tokio::sync::oneshot::channel();
    let state = Rc::new(RefCell::new(HiddenState { value: Vec::new(), escape: false, done: false, answer: Some(send) }));
    let finish: Rc<dyn Fn(Option<RuntimeError>)> = Rc::new({
        let state = state.clone();
        let input = input.clone();
        move |error| {
            let (answer, value) = {
                let mut state = state.borrow_mut();
                if state.done {
                    return;
                }
                state.done = true;
                (state.answer.take(), String::from_utf16_lossy(&state.value))
            };
            if raw {
                let _ = input.set_raw_mode(false);
            }
            input.pause();
            if raw {
                out.write("\n");
            }
            if let Some(answer) = answer {
                let _ = answer.send(error.map_or(Ok(value), Err));
            }
        }
    });
    let _guard = HiddenGuard { finish: finish.clone() };
    if signal.is_cancelled() {
        finish(Some(RuntimeError::plain("Cancelled; nothing was saved.")));
    } else {
        if raw {
            input.set_raw_mode(true).map_err(|e| RuntimeError::plain(e.to_string()))?;
        }
        input.on_end(Rc::new({
            let finish = Rc::downgrade(&finish);
            move || {
                if let Some(finish) = finish.upgrade() {
                    finish(None);
                }
            }
        }));
        input.resume(Rc::new({
            let state = state.clone();
            let finish = finish.clone();
            move |chunk| {
                for c in String::from_utf8_lossy(chunk).chars() {
                    let mut state = state.borrow_mut();
                    if state.done {
                        return;
                    }
                    if state.escape {
                        if c.is_ascii_alphabetic() || c == '~' {
                            state.escape = false;
                        }
                        continue;
                    }
                    if c == '\x1b' {
                        state.escape = true;
                        continue;
                    }
                    if c == '\r' || c == '\n' {
                        drop(state);
                        finish(None);
                        return;
                    }
                    if c == '\x03' {
                        drop(state);
                        finish(Some(RuntimeError::plain("Cancelled; nothing was saved.")));
                        return;
                    }
                    if c == '\x7f' || c == '\x08' {
                        state.value.pop();
                        continue;
                    }
                    if c >= ' ' {
                        state.value.extend(c.encode_utf16(&mut [0; 2]).iter().copied());
                    }
                    if state.value.len() > 8192 {
                        drop(state);
                        finish(Some(RuntimeError::plain("That's too long to be an API key; nothing was saved.")));
                        return;
                    }
                }
            }
        }));
    }
    tokio::pin!(receive);
    tokio::select! {result=&mut receive=>result.unwrap_or_else(|_|Err(RuntimeError::plain("Cancelled; nothing was saved."))),_=signal.cancelled()=>{finish(Some(RuntimeError::plain("Cancelled; nothing was saved.")));receive.await.unwrap_or_else(|_|Err(RuntimeError::plain("Cancelled; nothing was saved.")))}}
}
