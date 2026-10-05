use super::store::{Credential, CredentialStore, OAuthCredential};
use crate::{
    ai::{
        error::LanguageModelError,
        http::{default_fetch, Fetch, FetchInit, Response},
    },
    core::errors::{FailureKind, KumiError, RuntimeError},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures::future::{FutureExt, LocalBoxFuture, Shared};
use kumi_common::{
    abort::{self, Signal},
    js::json::stringify,
    time::now_ms,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, path::Path, rc::Rc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

pub const OPENAI_CODEX: &str = "openai-codex";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTH_BASE: &str = "https://auth.openai.com";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
pub const DEVICE_VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
const SCOPE: &str = "openid profile email offline_access";
const REFRESH_MARGIN_MS: f64 = 5.0 * 60_000.0;
const DEVICE_TIMEOUT_MS: i64 = 15 * 60_000;
pub static LOGIN_HINT: std::sync::LazyLock<String> = std::sync::LazyLock::new(login_hint);
pub fn login_hint() -> String {
    format!("Sign in with /login in Kumi, or: {} login openai-codex", *crate::command::KUMI)
}
#[derive(Clone)]
pub struct LoginOptions {
    pub signal: Signal,
    pub fetch: Rc<dyn Fetch>,
}
impl Default for LoginOptions {
    fn default() -> Self {
        Self { signal: Signal::new(), fetch: default_fetch() }
    }
}
fn auth(message: impl Into<String>) -> RuntimeError {
    KumiError::new(FailureKind::Auth, message).into()
}
fn provider_auth(message: impl Into<String>) -> RuntimeError {
    KumiError::with_provider(FailureKind::Auth, message, OPENAI_CODEX).into()
}
fn fetch_error(error: LanguageModelError) -> RuntimeError {
    match error {
        LanguageModelError::Kumi(error) => error.into(),
        error => RuntimeError::plain(error.to_string()),
    }
}
fn claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    // Buffer.from(base64url) accepts padding and both alphabets.
    let clean = payload.trim_end_matches('=').replace('+', "-").replace('/', "_");
    let bytes = URL_SAFE_NO_PAD.decode(clean).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    (value.is_object() || value.is_array()).then_some(value)
}
pub fn account_id_from_token(token: &str) -> Option<String> {
    claims(token)?
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
fn form(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs.iter().copied()).finish()
}
async fn request_tokens(body: String, options: &LoginOptions, previous_refresh: Option<&str>) -> Result<OAuthCredential, RuntimeError> {
    let response = options
        .fetch
        .fetch(
            TOKEN_URL,
            FetchInit {
                method: "POST".into(),
                headers: [("content-type".into(), "application/x-www-form-urlencoded".into())].into(),
                body: Some(body),
                signal: Some(options.signal.clone()),
            },
        )
        .await
        .map_err(fetch_error)?;
    if !response.ok() {
        return Err(provider_auth(if previous_refresh.is_some() {
            format!("OpenAI sign-in could not be refreshed (HTTP {}). {}", response.status, login_hint())
        } else {
            format!("OpenAI sign-in failed (HTTP {}).", response.status)
        }));
    }
    let value = response.json().await.map_err(fetch_error)?;
    let refresh = value.get("refresh_token").and_then(Value::as_str).filter(|value| !value.is_empty()).or(previous_refresh);
    let access = value.get("access_token").and_then(Value::as_str).filter(|value| !value.is_empty());
    let (Some(access), Some(refresh)) = (access, refresh) else {
        return Err(provider_auth("OpenAI returned an incomplete token response."));
    };
    let account_id = account_id_from_token(access)
        .ok_or_else(|| provider_auth("The OpenAI token has no ChatGPT account; sign in with a ChatGPT plan that includes Codex."))?;
    let expires = value
        .get("expires_in")
        .and_then(Value::as_f64)
        .map(|seconds| now_ms() as f64 + seconds * 1000.0)
        .or_else(|| claims(access)?.get("exp")?.as_f64().map(|exp| exp * 1000.0))
        .unwrap_or_else(|| now_ms() as f64 + 60.0 * 60_000.0);
    Ok(OAuthCredential { access: access.into(), refresh: refresh.into(), expires, account_id })
}
pub async fn refresh_codex_credential(credential: &OAuthCredential, options: &LoginOptions) -> Result<OAuthCredential, RuntimeError> {
    request_tokens(
        form(&[("grant_type", "refresh_token"), ("refresh_token", &credential.refresh), ("client_id", CLIENT_ID)]),
        options,
        Some(&credential.refresh),
    )
    .await
}

/// PKCE authorization-code flow with a one-shot callback server on localhost:1455.
pub async fn login_codex_browser(
    options: LoginOptions,
    on_url: Rc<dyn Fn(String)>,
    port: Option<u16>,
) -> Result<OAuthCredential, RuntimeError> {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    let state = hex::encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(&format!("{AUTH_BASE}/oauth/authorize")).expect("authorization URL");
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", REDIRECT_URI),
        ("scope", SCOPE),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("state", &state),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", "kumi"),
    ]);
    if options.signal.is_cancelled() {
        return Err(auth("Sign-in cancelled."));
    }
    let listener = TcpListener::bind(("127.0.0.1", port.unwrap_or(1455))).await.map_err(|error| {
        auth(if error.kind() == std::io::ErrorKind::AddrInUse {
            "Port 1455 is busy (another sign-in may be open). Close it and retry, or use --device."
        } else {
            "Could not start the local sign-in callback; use --device."
        })
    })?;
    on_url(url.to_string());
    let mut connections = tokio::task::JoinSet::new();
    let code = loop {
        tokio::select! {
            biased;
            _ = options.signal.cancelled() => break Err(auth("Sign-in cancelled.")),
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Ok(Some(result))) = result { break result; }
            },
            accepted = listener.accept() => {
                let Ok((mut socket, _)) = accepted else { break Err(auth("Could not start the local sign-in callback; use --device.")); };
                let state = state.clone();
                connections.spawn_local(async move {
                    let mut request = Vec::new(); let mut bytes = [0u8; 1024];
                    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        let read = socket.read(&mut bytes).await.ok()?; if read == 0 { return None; } request.extend_from_slice(&bytes[..read]);
                        if request.len() > 16 * 1024 { return None; }
                    }
                    let text = String::from_utf8_lossy(&request);
                    let target = text.lines().next()?.split_whitespace().nth(1).unwrap_or("/");
                    let callback = Url::parse("http://localhost").ok()?.join(target).ok()?;
                    if callback.path() != "/auth/callback" { let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await; return None; }
                    let mut query = std::collections::HashMap::new();
                    for (key, value) in callback.query_pairs().into_owned() { query.entry(key).or_insert(value); }
                    let code = query.get("code").filter(|code| !code.is_empty()).cloned();
                    let valid = query.get("state") == Some(&state) && code.is_some() && query.get("error").is_none_or(|error| error.is_empty());
                    let message = if valid { "Kumi is signed in. You can close this tab." } else { "Sign-in failed. Return to the terminal." };
                    let page = format!("<!doctype html><meta charset=\"utf-8\"><title>Kumi</title><p style=\"font:16px system-ui;margin:3rem\">{message}</p>");
                    let status = if valid { "200 OK" } else { "400 Bad Request" };
                    let answer = format!("HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{page}", page.len());
                    let _ = socket.write_all(answer.as_bytes()).await; let _ = socket.shutdown().await;
                    Some(if valid { Ok(code.expect("valid code")) } else { Err(auth("OpenAI sign-in was denied or returned an invalid callback.")) })
                });
            }
        }
    };
    connections.abort_all();
    drop(connections);
    drop(listener);
    request_tokens(
        form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", &code?),
            ("code_verifier", &verifier),
            ("redirect_uri", REDIRECT_URI),
        ]),
        &options,
        None,
    )
    .await
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePrompt {
    pub url: String,
    pub code: String,
}
async fn post(path: &str, body: Value, options: &LoginOptions) -> Result<Response, RuntimeError> {
    options
        .fetch
        .fetch(
            &format!("{AUTH_BASE}{path}"),
            FetchInit {
                method: "POST".into(),
                headers: [("content-type".into(), "application/json".into())].into(),
                body: Some(stringify(&body)),
                signal: Some(options.signal.clone()),
            },
        )
        .await
        .map_err(fetch_error)
}
/// Device-code flow for machines without a local browser.
pub async fn login_codex_device(options: LoginOptions, on_code: Rc<dyn Fn(DevicePrompt)>) -> Result<OAuthCredential, RuntimeError> {
    let start = post("/api/accounts/deviceauth/usercode", json!({"client_id":CLIENT_ID}), &options).await?;
    if !start.ok() {
        return Err(auth(if start.status == 404 {
            "Device sign-in is not available; use browser sign-in.".into()
        } else {
            format!("Device sign-in could not start (HTTP {}).", start.status)
        }));
    }
    let device = start.json().await.map_err(fetch_error)?;
    let mut interval = match device.get("interval") {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => kumi_common::js::number::parse(text),
        Some(Value::Null) => Some(0.0),
        Some(Value::Bool(value)) => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    }
    .unwrap_or(f64::NAN);
    let (Some(device_id), Some(user_code)) =
        (device.get("device_auth_id").and_then(Value::as_str), device.get("user_code").and_then(Value::as_str))
    else {
        return Err(auth("OpenAI returned an invalid device sign-in response."));
    };
    if !interval.is_finite() {
        return Err(auth("OpenAI returned an invalid device sign-in response."));
    }
    interval = interval.max(1.0);
    on_code(DevicePrompt { url: DEVICE_VERIFICATION_URL.into(), code: user_code.into() });
    let deadline = now_ms() + DEVICE_TIMEOUT_MS;
    while now_ms() < deadline {
        tokio::select! { biased; _ = options.signal.cancelled() => return Err(RuntimeError::Aborted), _ = tokio::time::sleep(Duration::from_secs_f64(interval)) => {} }
        let poll = post("/api/accounts/deviceauth/token", json!({"device_auth_id":device_id,"user_code":user_code}), &options).await?;
        if poll.ok() {
            let grant = poll.json().await.map_err(fetch_error)?;
            let (Some(code), Some(verifier)) =
                (grant.get("authorization_code").and_then(Value::as_str), grant.get("code_verifier").and_then(Value::as_str))
            else {
                return Err(auth("OpenAI returned an invalid device grant."));
            };
            return request_tokens(
                form(&[
                    ("grant_type", "authorization_code"),
                    ("client_id", CLIENT_ID),
                    ("code", code),
                    ("code_verifier", verifier),
                    ("redirect_uri", DEVICE_REDIRECT_URI),
                ]),
                &options,
                None,
            )
            .await;
        }
        if matches!(poll.status, 403 | 404) {
            continue;
        }
        let status = poll.status;
        let body = poll.json().await.unwrap_or(json!({}));
        let error = body.get("error");
        let code = error.and_then(|error| if error.is_object() { error.get("code") } else { Some(error) }).and_then(Value::as_str);
        match code {
            Some("deviceauth_authorization_pending") => continue,
            Some("slow_down") => {
                interval += 5.0;
                continue;
            }
            _ => return Err(auth(format!("Device sign-in failed (HTTP {status})."))),
        }
    }
    Err(auth("Device sign-in expired; try again."))
}
/// One-time migration from the Pi-based POC's login. Only the openai-codex entry is read.
pub async fn read_pi_codex_login(path: impl AsRef<Path>) -> Result<OAuthCredential, RuntimeError> {
    let path = path.as_ref();
    let body = tokio::fs::read(path)
        .await
        .ok()
        .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
        .ok_or_else(|| auth(format!("No readable Pi login at {}.", path.display())))?;
    let entry = &body[OPENAI_CODEX];
    let access = entry.get("access").and_then(Value::as_str).filter(|s| !s.is_empty());
    let refresh = entry.get("refresh").and_then(Value::as_str).filter(|s| !s.is_empty());
    let expires = entry.get("expires").and_then(Value::as_f64);
    let (Some(access), Some(refresh), Some(expires)) = (access, refresh, expires) else {
        return Err(auth("The Pi login has no ChatGPT (openai-codex) session to import."));
    };
    if entry.get("type").and_then(Value::as_str) != Some("oauth") {
        return Err(auth("The Pi login has no ChatGPT (openai-codex) session to import."));
    }
    let account_id = entry
        .get("accountId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| account_id_from_token(access))
        .ok_or_else(|| auth("The Pi login has no ChatGPT account ID."))?;
    Ok(OAuthCredential { access: access.into(), refresh: refresh.into(), expires, account_id })
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexToken {
    pub access: String,
    pub account_id: String,
}
pub type TokenSource = Rc<dyn Fn() -> LocalBoxFuture<'static, Result<CodexToken, RuntimeError>>>;
#[derive(Default)]
pub struct TokenOptions {
    pub fetch: Option<Rc<dyn Fetch>>,
    pub now: Option<Rc<dyn Fn() -> f64>>,
}
/// Current token, refreshed shortly before expiry under the store's cross-process lock.
pub fn codex_token_source(store: Rc<dyn CredentialStore>, options: TokenOptions) -> TokenSource {
    type Refresh = Shared<LocalBoxFuture<'static, Result<OAuthCredential, RuntimeError>>>;
    let refreshing: Rc<RefCell<Option<Refresh>>> = Rc::new(RefCell::new(None));
    let now = options.now.unwrap_or_else(|| Rc::new(|| now_ms() as f64));
    let fetch = options.fetch.unwrap_or_else(default_fetch);
    Rc::new(move || {
        let store = store.clone();
        let refreshing = refreshing.clone();
        let now = now.clone();
        let fetch = fetch.clone();
        async move {
            let require = |current| match current {
                Some(Credential::Oauth(current)) => Ok(current),
                _ => Err(provider_auth(format!("Not signed in to ChatGPT (openai-codex). {}", login_hint()))),
            };
            let current = require(store.get(OPENAI_CODEX).await?)?;
            if current.expires - now() > REFRESH_MARGIN_MS {
                return Ok(CodexToken { access: current.access, account_id: current.account_id });
            }
            let pending = {
                let mut cell = refreshing.borrow_mut();
                cell.get_or_insert_with(|| {
                    let (send, receive) = tokio::sync::oneshot::channel();
                    let refreshing = refreshing.clone();
                    tokio::task::spawn_local(async move {
                        let result = store
                            .update(
                                OPENAI_CODEX,
                                Box::new(move |latest| {
                                    async move {
                                        let latest = require(latest)?;
                                        if latest.expires - now() > REFRESH_MARGIN_MS {
                                            return Ok(Some(latest.into()));
                                        }
                                        Ok(Some(
                                            refresh_codex_credential(&latest, &LoginOptions { signal: abort::timeout(30_000), fetch })
                                                .await?
                                                .into(),
                                        ))
                                    }
                                    .boxed_local()
                                }),
                            )
                            .await
                            .and_then(require);
                        *refreshing.borrow_mut() = None;
                        let _ = send.send(result);
                    });
                    async move { receive.await.unwrap_or_else(|_| Err(auth("OpenAI sign-in refresh stopped."))) }.boxed_local().shared()
                })
                .clone()
            };
            let credential = pending.await?;
            Ok(CodexToken { access: credential.access, account_id: credential.account_id })
        }
        .boxed_local()
    })
}
