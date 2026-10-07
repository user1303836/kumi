use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use kumi_common::{
    abort::{Controller, Signal},
    time::now_ms,
};
use kumi_runtime::{
    ai::{
        error::LanguageModelError,
        http::{Fetch, FetchInit, Response},
    },
    auth::{
        openai_codex::{
            account_id_from_token, codex_token_source, login_codex_browser, login_codex_device, read_pi_codex_login, LoginOptions,
            TokenOptions,
        },
        store::{open_credential_store, Credential, CredentialStore, OAuthCredential},
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::{Duration, SystemTime},
};
use tokio::{
    net::TcpListener,
    task::{spawn_local, LocalSet},
    time::sleep,
};
use url::Url;

fn jwt(value: Value) -> String {
    format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap()))
}
fn token(account: &str) -> String {
    jwt(json!({"https://api.openai.com/auth":{"chatgpt_account_id":account}}))
}
fn credential(refresh: &str) -> Credential {
    OAuthCredential {
        access: token("acct-1"),
        refresh: refresh.into(),
        expires: now_ms() as f64 + 3_600_000.0,
        account_id: "acct-1".into(),
    }
    .into()
}
struct FakeFetch(Rc<dyn Fn(&str, FetchInit) -> Response>);
#[async_trait(?Send)]
impl Fetch for FakeFetch {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        Ok((self.0)(url, init))
    }
}
fn form(body: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(body.as_bytes()).into_owned().collect()
}
macro_rules! local_test { ($name:ident, $body:block) => { #[tokio::test] async fn $name() { LocalSet::new().run_until(async $body).await } }; }
#[tokio::test]
async fn credential_store_is_owner_only_atomic_and_removes_entries_on_request() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested/auth.json");
    let store = open_credential_store(&path);
    assert_eq!(store.get("openai-codex").await.unwrap(), None);
    store.update_with("openai-codex", |_| async { Ok(Some(credential("r"))) }).await.unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap()["version"], 1);
    store.update_with("openai-codex", |_| async { Ok(None) }).await.unwrap();
    assert!(store.list().await.unwrap().is_empty());
    let malformed =
        Credential::Oauth(OAuthCredential { access: String::new(), refresh: String::new(), expires: 0.0, account_id: String::new() });
    assert!(store.update_with("x", |_| async { Ok(Some(malformed)) }).await.unwrap_err().to_string().contains("malformed credential"));
}
#[tokio::test]
async fn credential_store_keeps_api_keys_beside_sign_ins_and_refuses_anything_that_isnt_one_word() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let store = open_credential_store(&path);
    store.update_with("anthropic", |_| async { Ok(Some(Credential::ApiKey { key: "sk-ant-fixture-0001".into() })) }).await.unwrap();
    assert_eq!(
        open_credential_store(&path).get("anthropic").await.unwrap(),
        Some(Credential::ApiKey { key: "sk-ant-fixture-0001".into() })
    );
    for key in ["", "short", "two words-here", "line\nbreak-0000", &"x".repeat(4097)] {
        assert!(store
            .update_with("openai", |_| async { Ok(Some(Credential::ApiKey { key: key.into() })) })
            .await
            .unwrap_err()
            .to_string()
            .contains("malformed"));
    }
    assert_eq!(store.list().await.unwrap().keys().map(String::as_str).collect::<Vec<_>>(), ["anthropic"]);
}
#[tokio::test]
async fn credential_store_refuses_readable_by_others_and_malformed_files_without_printing_their_contents() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let store = open_credential_store(&path);
    store.update_with("openai-codex", |_| async { Ok(Some(credential("secret-refresh"))) }).await.unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.get("openai-codex").await.unwrap_err().to_string().contains("chmod 600"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(&path, "{\"token\": \"secret-refresh\"").unwrap();
    let error = store.get("openai-codex").await.unwrap_err().to_string();
    assert!(error.contains("malformed"));
    assert!(!error.contains("secret-refresh"));
}
#[tokio::test]
async fn concurrent_updates_from_separate_processes_stores_serialize_under_the_lock_stale_locks_are_broken() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let a = open_credential_store(&path);
    let b = open_credential_store(&path);
    let (first, second) = tokio::join!(
        a.update_with("a", |_| async {
            sleep(Duration::from_millis(20)).await;
            Ok(Some(credential("a")))
        }),
        b.update_with("b", |_| async {
            sleep(Duration::from_millis(20)).await;
            Ok(Some(credential("b")))
        })
    );
    first.unwrap();
    second.unwrap();
    let mut keys: Vec<_> = a.list().await.unwrap().into_keys().collect();
    keys.sort();
    assert_eq!(keys, ["a", "b"]);
    let lock = path.with_extension("json.lock");
    std::fs::write(&lock, "999999").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(120)))
        .unwrap();
    a.update_with("c", |_| async { Ok(Some(credential("c"))) }).await.unwrap();
    assert!(matches!(a.get("c").await.unwrap(), Some(Credential::Oauth(OAuthCredential { refresh, .. })) if refresh == "c"));
    // A lock 40 s old may be a refresh still under way (it gives up at 30 s): waited for, not broken.
    std::fs::write(&lock, "another kumi").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(40)))
        .unwrap();
    let released = Rc::new(Cell::new(false));
    let (waited, ()) = tokio::join!(
        a.update_with("d", |_| {
            let released = released.get();
            async move {
                assert!(released, "the other Kumi's lock was broken while it held it");
                Ok(Some(credential("d")))
            }
        }),
        async {
            sleep(Duration::from_millis(150)).await;
            released.set(true);
            std::fs::remove_file(&lock).unwrap();
        }
    );
    waited.unwrap();
    // A lock broken as stale while its holder ran is the next Kumi's: the first's release leaves it.
    a.update_with("e", |_| async {
        std::fs::write(&lock, "the next kumi").unwrap();
        Ok(Some(credential("e")))
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&lock).unwrap(), "the next kumi");
}
#[test]
fn account_ids_come_only_from_the_chatgpt_auth_claim() {
    assert_eq!(account_id_from_token(&token("acct-9")), Some("acct-9".into()));
    for value in ["".into(), "not-a-jwt".into(), jwt(json!({})), jwt(json!({"https://api.openai.com/auth":{"chatgpt_account_id":7}}))] {
        assert_eq!(account_id_from_token(&value), None);
    }
}
local_test!(browser_sign_in_pkce_state_check_one_shot_localhost_callback_token_exchange, {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let exchanges = Rc::new(RefCell::new(Vec::new()));
    let captured = exchanges.clone();
    let fetch: Rc<dyn Fetch> = Rc::new(FakeFetch(Rc::new(move |_, init| {
        captured.borrow_mut().push(form(&init.body.unwrap()));
        Response::json_response(200, json!({"access_token":token("acct-7"),"refresh_token":"refresh-7","expires_in":600}))
    })));
    let authorize = Rc::new(RefCell::new(None));
    let captured = authorize.clone();
    let options = LoginOptions { signal: Signal::new(), fetch: fetch.clone() };
    let login =
        spawn_local(login_codex_browser(options, Rc::new(move |url| *captured.borrow_mut() = Some(Url::parse(&url).unwrap())), Some(port)));
    while authorize.borrow().is_none() {
        sleep(Duration::from_millis(5)).await;
    }
    let url = authorize.borrow().clone().unwrap();
    let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(format!("{}{}", url.origin().ascii_serialization(), url.path()), "https://auth.openai.com/oauth/authorize");
    assert_eq!(query["originator"], "kumi");
    assert_eq!(query["code_challenge_method"], "S256");
    let denied = reqwest::get(format!("http://127.0.0.1:{port}/auth/callback?code=x&state=wrong")).await.unwrap();
    assert_eq!(denied.status(), 400);
    assert!(login.await.unwrap().unwrap_err().to_string().contains("denied or returned an invalid callback"));
    assert!(exchanges.borrow().is_empty());
    let second = Rc::new(RefCell::new(None));
    let captured = second.clone();
    let retry = spawn_local(login_codex_browser(
        LoginOptions { signal: Signal::new(), fetch },
        Rc::new(move |url| *captured.borrow_mut() = Some(Url::parse(&url).unwrap())),
        Some(port),
    ));
    while second.borrow().is_none() {
        sleep(Duration::from_millis(5)).await;
    }
    let query: HashMap<_, _> = second.borrow().as_ref().unwrap().query_pairs().into_owned().collect();
    let callback = reqwest::get(format!("http://127.0.0.1:{port}/auth/callback?code=the-code&state={}", query["state"])).await.unwrap();
    assert_eq!(callback.status(), 200);
    let mut result = retry.await.unwrap().unwrap();
    result.expires = 0.0;
    assert_eq!(result, OAuthCredential { access: token("acct-7"), refresh: "refresh-7".into(), expires: 0.0, account_id: "acct-7".into() });
    let exchanges = exchanges.borrow();
    let values = &exchanges[0];
    assert_eq!(values["code"], "the-code");
    assert_eq!(values["redirect_uri"], "http://localhost:1455/auth/callback");
    assert_eq!(URL_SAFE_NO_PAD.encode(Sha256::digest(values["code_verifier"].as_bytes())), query["code_challenge"]);
});
local_test!(browser_sign_in_can_be_cancelled_and_reports_a_busy_port, {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let controller = Controller::new();
    let signal = controller.signal.clone();
    assert!(login_codex_browser(LoginOptions { signal, ..Default::default() }, Rc::new(move |_| controller.abort()), Some(port))
        .await
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    let _blocker = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    assert!(login_codex_browser(LoginOptions::default(), Rc::new(|_| {}), Some(port)).await.unwrap_err().to_string().contains("busy"));
});
local_test!(device_sign_in_polls_until_approval_then_exchanges_the_device_grant, {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let captured = calls.clone();
    let polls = Rc::new(Cell::new(0));
    let fetch = Rc::new(FakeFetch(Rc::new(move |url, init| {
        let path = Url::parse(url).unwrap().path().to_string();
        captured.borrow_mut().push(path.clone());
        if path.ends_with("/usercode") {
            return Response::json_response(200, json!({"device_auth_id":"dev-1","user_code":"ABCD-1234","interval":"1"}));
        }
        if path.ends_with("/deviceauth/token") {
            polls.set(polls.get() + 1);
            return if polls.get() == 1 {
                Response::json_response(403, json!({}))
            } else {
                Response::json_response(200, json!({"authorization_code":"auth-code","code_verifier":"verifier"}))
            };
        }
        let values = form(&init.body.unwrap());
        assert_eq!(values["code_verifier"], "verifier");
        assert_eq!(values["redirect_uri"], "https://auth.openai.com/deviceauth/callback");
        Response::json_response(200, json!({"access_token":token("acct-1"),"refresh_token":"refresh","expires_in":600}))
    })));
    let prompt = Rc::new(RefCell::new(None));
    let captured = prompt.clone();
    let result =
        login_codex_device(LoginOptions { signal: Signal::new(), fetch }, Rc::new(move |value| *captured.borrow_mut() = Some(value)))
            .await
            .unwrap();
    assert_eq!(
        serde_json::to_value(prompt.borrow().as_ref().unwrap()).unwrap(),
        json!({"url":"https://auth.openai.com/codex/device","code":"ABCD-1234"})
    );
    assert_eq!(result.account_id, "acct-1");
    assert_eq!(
        *calls.borrow(),
        ["/api/accounts/deviceauth/usercode", "/api/accounts/deviceauth/token", "/api/accounts/deviceauth/token", "/oauth/token"]
    );
});
#[tokio::test]
async fn pi_import_reads_only_the_openai_codex_entry_and_derives_a_missing_account_id() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pi-auth.json");
    std::fs::write(&path, serde_json::to_vec(&json!({"other":{"type":"oauth","access":"x"},"openai-codex":{"type":"oauth","access":token("acct-3"),"refresh":"r","expires":5}})).unwrap()).unwrap();
    assert_eq!(
        read_pi_codex_login(&path).await.unwrap(),
        OAuthCredential { access: token("acct-3"), refresh: "r".into(), expires: 5.0, account_id: "acct-3".into() }
    );
    std::fs::write(&path, "{\"other\":{}}").unwrap();
    assert!(read_pi_codex_login(&path).await.unwrap_err().to_string().contains("no ChatGPT (openai-codex) session"));
    assert!(read_pi_codex_login(temp.path().join("missing.json")).await.unwrap_err().to_string().contains("No readable Pi login"));
}
local_test!(concurrent_token_requests_share_one_refresh_and_keep_the_rotated_refresh_token, {
    let temp = tempfile::tempdir().unwrap();
    let store = Rc::new(open_credential_store(temp.path().join("auth.json")));
    store
        .update_with("openai-codex", |_| async {
            Ok(Some(
                OAuthCredential { access: token("acct-1"), refresh: "old-refresh".into(), expires: 0.0, account_id: "acct-1".into() }
                    .into(),
            ))
        })
        .await
        .unwrap();
    let calls = Rc::new(Cell::new(0));
    let captured = calls.clone();
    let fetch = Rc::new(FakeFetch(Rc::new(move |url, init| {
        assert_eq!(url, "https://auth.openai.com/oauth/token");
        assert_eq!(form(&init.body.unwrap())["refresh_token"], "old-refresh");
        captured.set(captured.get() + 1);
        Response::json_response(200, json!({"access_token":token("acct-2"),"refresh_token":"rotated-refresh","expires_in":3600}))
    })));
    let source = codex_token_source(store.clone(), TokenOptions { fetch: Some(fetch), ..Default::default() });
    let results = futures::future::join_all((0..5).map(|_| source())).await;
    for result in results {
        assert_eq!(result.unwrap().account_id, "acct-2");
    }
    assert_eq!(calls.get(), 1);
    assert_eq!(source().await.unwrap().account_id, "acct-2");
    assert_eq!(calls.get(), 1);
    assert!(
        matches!(store.get("openai-codex").await.unwrap(), Some(Credential::Oauth(OAuthCredential { refresh, .. })) if refresh == "rotated-refresh")
    );
});
