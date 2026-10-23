//! Port of the CLI sign-in lifecycle test, with native hidden-input and provider-I/O coverage.
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures::FutureExt;
use kumi::{
    config::{load_config, write_settings},
    input::{ByteListener, TerminalInput},
    login::*,
    tui::tty::TtyOutput,
};
use kumi_common::{abort::Signal, time::now_ms};
use kumi_runtime::{
    ai::{
        error::LanguageModelError,
        http::{Fetch, FetchInit, Response},
    },
    auth::store::{open_credential_store, Credential, CredentialStore},
    system::Env,
    KUMI, KUMI_START,
};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
#[derive(Default)]
struct Out(RefCell<String>);
impl TtyOutput for Out {
    fn is_tty(&self) -> bool {
        false
    }
    fn columns(&self) -> Option<i32> {
        None
    }
    fn rows(&self) -> Option<i32> {
        None
    }
    fn write(&self, s: &str) {
        self.0.borrow_mut().push_str(s)
    }
}
struct Input {
    tty: bool,
    chunks: Vec<Vec<u8>>,
    eof: bool,
    listener: RefCell<Option<ByteListener>>,
    end: RefCell<Option<Rc<dyn Fn()>>>,
    raw: RefCell<Vec<bool>>,
    pauses: Cell<usize>,
}
impl Input {
    fn new(tty: bool, chunks: Vec<Vec<u8>>, eof: bool) -> Rc<Self> {
        Rc::new(Self {
            tty,
            chunks,
            eof,
            listener: RefCell::new(None),
            end: RefCell::new(None),
            raw: RefCell::new(vec![]),
            pauses: Cell::new(0),
        })
    }
}
impl TerminalInput for Input {
    fn is_tty(&self) -> bool {
        self.tty
    }
    fn set_raw_mode(&self, v: bool) -> std::io::Result<()> {
        self.raw.borrow_mut().push(v);
        Ok(())
    }
    fn resume(&self, listener: ByteListener) {
        *self.listener.borrow_mut() = Some(listener);
        for chunk in &self.chunks {
            let next = self.listener.borrow().clone();
            if let Some(next) = next {
                next(chunk);
            }
        }
        if self.eof && self.listener.borrow().is_some() {
            let end = self.end.borrow().clone();
            if let Some(end) = end {
                end();
            }
        }
    }
    fn pause(&self) {
        self.pauses.set(self.pauses.get() + 1);
        self.listener.borrow_mut().take();
    }
    fn on_end(&self, next: Rc<dyn Fn()>) {
        *self.end.borrow_mut() = Some(next)
    }
}
#[tokio::test(flavor = "current_thread")]
async fn hidden_input_first_line_escape_backspace_utf16_and_eof_without_echo() {
    for (tty, chunks, eof, want) in [
        (true, vec![b"sk-a\x1b[".to_vec(), b"Dz\x7fbc\x08d\rignored".to_vec()], false, "sk-abd"),
        (false, vec![b"piped-key\nsecond-line".to_vec()], false, "piped-key"),
        (false, vec![b"eof-key".to_vec()], true, "eof-key"),
        (true, vec!["a🎹\x7f\r".as_bytes().to_vec()], false, "a�"),
        (false, vec![vec![0xf0, 0x9f], vec![0x8e, 0xb9, b'\n']], false, "���"),
        (false, vec![b"\x01\t x\r".to_vec()], false, " x"),
    ] {
        let input = Input::new(tty, chunks, eof);
        let out = Rc::new(Out::default());
        assert_eq!(read_hidden(input.clone(), out.clone(), "Key: ", Signal::new()).await.unwrap(), want);
        assert_eq!(*out.0.borrow(), if tty { "Key: \n" } else { "Key: " });
        assert_eq!(*input.raw.borrow(), if tty { vec![true, false] } else { vec![] });
        assert_eq!(input.pauses.get(), 1);
    }
}
#[tokio::test(flavor = "current_thread")]
async fn hidden_input_cancels_and_limits_utf16_restoring_terminal() {
    for (text, want) in [
        ("secret\x03".to_string(), "Cancelled; nothing was saved."),
        ("x".repeat(8193), "That's too long to be an API key; nothing was saved."),
        ("🎹".repeat(4097), "That's too long to be an API key; nothing was saved."),
    ] {
        let input = Input::new(true, vec![text.into_bytes()], false);
        let out = Rc::new(Out::default());
        assert_eq!(read_hidden(input.clone(), out.clone(), "Key: ", Signal::new()).await.unwrap_err().message(), want);
        assert_eq!(*input.raw.borrow(), [true, false]);
        assert_eq!(*out.0.borrow(), "Key: \n");
    }
    let signal = Signal::new();
    signal.cancel();
    let input = Input::new(true, vec![], false);
    assert!(read_hidden(input.clone(), Rc::new(Out::default()), "", signal).await.is_err());
    assert_eq!(*input.raw.borrow(), [false]);
    let signal = Signal::new();
    let input = Input::new(true, vec![], false);
    let out = Rc::new(Out::default());
    let read = read_hidden(input.clone(), out.clone(), "", signal.clone());
    tokio::pin!(read);
    assert!(futures::poll!(&mut read).is_pending());
    signal.cancel();
    assert!(read.await.is_err());
    assert_eq!(*input.raw.borrow(), [true, false]);
    let input = Input::new(true, vec![], false);
    let mut pending = Box::pin(read_hidden(input.clone(), Rc::new(Out::default()), "", Signal::new()));
    assert!(futures::poll!(&mut pending).is_pending());
    drop(pending);
    assert_eq!(*input.raw.borrow(), [true, false]);
}
struct Provider {
    status: u16,
    calls: Cell<usize>,
}
#[async_trait(?Send)]
impl Fetch for Provider {
    async fn fetch(&self, url: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        self.calls.set(self.calls.get() + 1);
        if url.starts_with("https://api.anthropic.com/") {
            Ok(Response::json_response(self.status, json!({"data":[]})))
        } else {
            Ok(Response::text_response(404, ""))
        }
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    env: Env,
    out: Rc<Out>,
    fetch: Rc<Provider>,
}
impl Fixture {
    fn new(status: u16) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::from([
            ("HOME".into(), dir.path().display().to_string()),
            ("USERPROFILE".into(), dir.path().display().to_string()),
            ("KUMI_AUTH_FILE".into(), dir.path().join("auth.json").display().to_string()),
            ("KUMI_SETTINGS_FILE".into(), dir.path().join("settings.json").display().to_string()),
        ]);
        Self { _dir: dir, env, out: Rc::new(Out::default()), fetch: Rc::new(Provider { status, calls: Cell::new(0) }) }
    }
    fn config(&self, args: &[&str]) -> kumi::config::AppConfig {
        load_config(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &self.env).unwrap()
    }
    fn io(&self, input: Option<Rc<dyn TerminalInput>>) -> LoginIo {
        LoginIo {
            out: self.out.clone(),
            env: self.env.clone(),
            signal: Signal::new(),
            open_browser: None,
            input,
            fetch: Some(self.fetch.clone()),
        }
    }
    fn auth(&self) -> AuthIo {
        AuthIo { out: self.out.clone(), env: self.env.clone(), fetch: Some(self.fetch.clone()) }
    }
}
#[tokio::test(flavor = "current_thread")]
async fn api_key_login_checks_before_saving_and_reports_unreachable_or_refused() {
    for status in [200, 401, 500] {
        let f = Fixture::new(status);
        let key = "sk-ant-private-012345";
        let input = Input::new(true, vec![format!("  {key} \r").into_bytes()], false);
        let result = login(&f.config(&["login", "anthropic"]), f.io(Some(input))).await;
        let store = open_credential_store(&f.env["KUMI_AUTH_FILE"]);
        let saved = store.get("anthropic").await.unwrap();
        if status == 401 {
            assert_eq!(result.unwrap_err().message(), "Anthropic didn't accept that key; nothing was saved.");
            assert!(saved.is_none());
        } else {
            result.unwrap();
            assert_eq!(saved, Some(Credential::ApiKey { key: key.into() }));
            assert!(f.out.0.borrow().contains(if status == 200 {
                "Signed in to Anthropic."
            } else {
                "Anthropic didn't answer just now, so it isn't checked yet."
            }));
            assert!(f
                .out
                .0
                .borrow()
                .contains(&format!("Next: {}. It starts with Anthropic's first model; /model changes it.", *KUMI_START)));
        }
        assert!(!f.out.0.borrow().contains(key));
        assert_eq!(f.fetch.calls.get(), 1);
    }
    let f = Fixture::new(200);
    assert_eq!(
        login(&f.config(&["login", "anthropic"]), f.io(None)).await.unwrap_err().message(),
        "Kumi needs a terminal to ask for the key."
    );
    assert!(login(&f.config(&["login", "anthropic"]), f.io(Some(Input::new(false, vec![b"no\n".to_vec()], false))))
        .await
        .unwrap_err()
        .message()
        .starts_with("That doesn't look like an API key"));
    assert_eq!(f.fetch.calls.get(), 0);
    write_settings(&f.env["KUMI_SETTINGS_FILE"], &json!({"model":"openai/gpt-fixture"})).unwrap();
    login(&f.config(&["login", "anthropic"]), f.io(Some(Input::new(false, vec![b"sk-ant-private-012345\n".to_vec()], false))))
        .await
        .unwrap();
    assert!(f.out.0.borrow().contains("Kumi still talks to openai/gpt-fixture; choose one of Anthropic's models with /model in Kumi."));
}
#[tokio::test(flavor = "current_thread")]
async fn pi_login_imports_only_chatgpt_owner_only_then_auth_and_logout_never_print_secrets() {
    let mut f = Fixture::new(404);
    f.env.insert("OPENAI_API_KEY".into(), "sk-private-env".into());
    f.env.insert("ANTHROPIC_API_KEY".into(), "".into());
    let access = format!(
        "e30.{}.fixture",
        URL_SAFE_NO_PAD.encode(json!({"https://api.openai.com/auth":{"chatgpt_account_id":"acct-cli"}}).to_string())
    );
    let path = f._dir.path().join(".pi/agent/auth.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path,json!({"openai-codex":{"type":"oauth","access":access,"refresh":"refresh-private","expires":now_ms()+7_200_000},"anthropic":{"type":"api_key","key":"not-imported-private"}}).to_string()).unwrap();
    let mut config = f.config(&["login", "openai-codex", "--from-pi"]);
    if let kumi::config::AppConfig::Login { pi_auth_file, .. } = &mut config {
        *pi_auth_file = path.display().to_string();
    }
    login(&config, f.io(None)).await.unwrap();
    let imported = f.out.0.borrow().clone();
    assert!(imported.contains("Signed in to ChatGPT"));
    assert!(imported.contains("Imported from Pi:"));
    assert!(imported.contains(&format!("Next: {}. It starts with ChatGPT's first model; /model changes it.", *KUMI_START)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&f.env["KUMI_AUTH_FILE"]).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let store = open_credential_store(&f.env["KUMI_AUTH_FILE"]);
    assert!(matches!(store.get("openai-codex").await.unwrap(),Some(Credential::Oauth(c)) if c.account_id=="acct-cli"));
    assert!(store.get("anthropic").await.unwrap().is_none());
    f.out.0.borrow_mut().clear();
    auth_status(&f.config(&["auth"]), f.auth()).await.unwrap();
    let status = f.out.0.borrow().clone();
    assert!(status.contains("openai-codex  signed in"));
    assert!(status.contains("openai        API key from OPENAI_API_KEY"));
    assert!(status.contains(&format!("anthropic     not signed in ({} login anthropic)", *KUMI)));
    assert!(status.contains("Model: not chosen yet"));
    f.out.0.borrow_mut().clear();
    logout(&f.config(&["logout", "openai-codex"]), f.auth()).await.unwrap();
    let removed = f.out.0.borrow().clone();
    assert!(removed.starts_with("Removed Kumi's ChatGPT sign-in"));
    assert!(store.get("openai-codex").await.unwrap().is_none());
    for output in [imported, status, removed] {
        for secret in [&access, "refresh-private", "sk-private-env", "not-imported-private"] {
            assert!(!output.contains(secret));
        }
    }
    f.out.0.borrow_mut().clear();
    auth_status(&f.config(&["auth"]), f.auth()).await.unwrap();
    assert!(f.out.0.borrow().contains("openai-codex  not signed in"));
}
#[tokio::test(flavor = "current_thread")]
async fn logout_shared_opencode_preserves_environment_notice_and_status_model_effort() {
    let mut f = Fixture::new(404);
    f.env.insert("OPENCODE_API_KEY".into(), "private-env-fixture".into());
    f.env.insert("KUMI_MODEL".into(), "openai/gpt-fixture".into());
    let store = open_credential_store(&f.env["KUMI_AUTH_FILE"]);
    store
        .update("opencode", Box::new(|_| async { Ok(Some(Credential::ApiKey { key: "private-saved-fixture".into() })) }.boxed_local()))
        .await
        .unwrap();
    logout(&f.config(&["logout", "opencode-go"]), f.auth()).await.unwrap();
    assert!(f.out.0.borrow().contains("(OpenCode Zen and Go share it)"));
    assert!(f.out.0.borrow().contains("OPENCODE_API_KEY is still set in your environment, and Kumi uses it; unset it to sign out fully."));
    assert!(store.get("opencode").await.unwrap().is_none());
    f.out.0.borrow_mut().clear();
    logout(&f.config(&["logout", "opencode"]), f.auth()).await.unwrap();
    assert!(f.out.0.borrow().contains("Kumi has no OpenCode Zen sign-in to remove."));
    write_settings(&f.env["KUMI_SETTINGS_FILE"], &json!({"effort":"low"})).unwrap();
    f.out.0.borrow_mut().clear();
    auth_status(&f.config(&["auth"]), f.auth()).await.unwrap();
    let output = f.out.0.borrow();
    assert!(output.contains("Model: openai/gpt-fixture (from KUMI_MODEL), effort low\n"));
    assert!(!output.contains("private-env-fixture"));
}
/// The sign-in's host answering one way: an HTTP status, or a failure as Kumi's own HTTP reports it.
struct Reach(Result<u16, LanguageModelError>);
#[async_trait(?Send)]
impl Fetch for Reach {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        assert_eq!(url, "https://auth.openai.com/.well-known/openid-configuration");
        match &self.0 {
            Ok(0) => {
                // Nothing answers: Kumi's own wait runs out.
                init.signal.unwrap().cancelled().await;
                Err(LanguageModelError::other("This operation was aborted"))
            }
            Ok(status) => Ok(Response::text_response(*status, "")),
            Err(error) => Err(error.clone()),
        }
    }
}
#[tokio::test(flavor = "current_thread")]
async fn before_sign_in_kumi_says_why_it_cant_reach_it_with_the_proxy_settings() {
    use kumi_runtime::ai::error::ApiCallError;
    let reach = |answer| async move { reach_sign_in(&Env::new(), Some(Rc::new(Reach(answer)))).await };
    // Any answer from the host is a way there, even one that refuses this request.
    for status in [200, 403, 404, 500] {
        assert_eq!(reach(Ok(status)).await, None, "{status}");
    }
    let mut dns = ApiCallError::new("Cannot connect to API: error sending request", "u", None, None);
    dns.cause = Some("error sending request: client error (Connect): dns error: failed to lookup address information: nodename nor servname provided, or not known".into());
    assert_eq!(
        reach(Err(dns.into())).await.as_deref(),
        Some("Kumi couldn't reach ChatGPT's sign-in (auth.openai.com): its address couldn't be looked up (DNS). Check that this computer is online; if it is, its DNS isn't answering, so try another network. Behind a proxy? Set HTTPS_PROXY to it.")
    );
    let proxy = reach(Ok(407)).await.unwrap();
    assert!(proxy.starts_with("Kumi couldn't reach ChatGPT's sign-in (auth.openai.com): the proxy didn't let Kumi through."), "{proxy}");
    // A proxy in the environment is named, without its password.
    let env = Env::from([("HTTPS_PROXY".to_string(), "http://me:secret@proxy.example:3128".to_string())]);
    let mut tls = ApiCallError::new("Cannot connect to API", "u", None, None);
    tls.cause = Some("error sending request: client error (Connect): invalid peer certificate: UnknownIssuer".into());
    let said = reach_sign_in(&env, Some(Rc::new(Reach(Err(tls.into()))))).await.unwrap();
    assert!(said.contains("(TLS)") && said.ends_with("Kumi's proxy settings: HTTPS_PROXY=http://proxy.example:3128."), "{said}");
    assert!(!said.contains("secret"));
    // A proxy that isn't answering: start it, or go without it; a password wouldn't help.
    let mut down = ApiCallError::new("Cannot connect to API", "u", None, None);
    down.cause = Some("error sending request: client error (Connect): tunnel error: failed to create underlying connection: tcp connect error: Connection refused (os error 61)".into());
    let said = reach_sign_in(&env, Some(Rc::new(Reach(Err(down.into()))))).await.unwrap();
    assert!(
        said.starts_with("Kumi couldn't reach ChatGPT's sign-in (auth.openai.com): the proxy isn't answering. Start it again")
            && !said.contains("user:pass"),
        "{said}"
    );
}
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_sign_in_host_that_never_answers_is_a_timeout() {
    let said = reach_sign_in(&Env::new(), Some(Rc::new(Reach(Ok(0))))).await.unwrap();
    assert!(said.starts_with("Kumi couldn't reach ChatGPT's sign-in (auth.openai.com): nothing answered in time."), "{said}");
}
