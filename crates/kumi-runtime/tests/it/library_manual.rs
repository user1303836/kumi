use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_runtime::{
    integrations::live_manual::{
        chapter_addresses, chapter_sections, fetch_manual, manual_tool, search_manual, stems, ManualOptions, ManualSection,
    },
    web::net::{WebClient, WebError, WebFailure, WebRequest, WebResponse},
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
struct Client {
    pages: BTreeMap<String, String>,
    reads: RefCell<Vec<String>>,
    failed: Cell<bool>,
    delay: bool,
}
#[async_trait(?Send)]
impl WebClient for Client {
    async fn fetch(&self, url: &str, request: WebRequest) -> Result<WebResponse, WebFailure> {
        self.reads.borrow_mut().push(url.into());
        if self.delay {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        if request.signal.is_some_and(|s| s.is_cancelled()) {
            return Err(WebFailure::Aborted);
        }
        if self.failed.get() {
            return Err(WebError::new("offline").into());
        }
        let body = self.pages.get(url);
        Ok(WebResponse {
            url: url.into(),
            status: if body.is_some() { 200 } else { 404 },
            headers: BTreeMap::new(),
            content_type: "text/html".into(),
            charset: None,
            body: body.cloned().unwrap_or_default().into_bytes(),
            truncated: false,
            skipped: false,
        })
    }
}
fn fixture() -> Value {
    serde_json::from_str(&kumi_testkit::oracle("kumi-runtime/library-manual-oracle.json")).unwrap()
}
fn client(fixture: &Value) -> Rc<Client> {
    Rc::new(Client {
        pages: serde_json::from_value(fixture["pages"].clone()).unwrap(),
        reads: RefCell::new(vec![]),
        failed: Cell::new(false),
        delay: false,
    })
}
fn normalized(value: impl serde::Serialize) -> Value {
    serde_json::from_str(&kumi_common::js::json::stringify(&serde_json::to_value(value).unwrap())).unwrap()
}
#[test]
fn parsing_stemming_bm25_and_passages_match_typescript() {
    let f = fixture();
    let base = f["base"].as_str().unwrap();
    assert_eq!(normalized(chapter_addresses(f["pages"][base].as_str().unwrap(), Some(base))), f["addresses"]);
    let sections: Vec<ManualSection> = serde_json::from_value(f["sections"].clone()).unwrap();
    let mut actual = vec![];
    for url in f["addresses"].as_array().unwrap() {
        actual.extend(chapter_sections(f["pages"][url.as_str().unwrap()].as_str().unwrap(), url.as_str().unwrap()));
    }
    assert_eq!(actual, sections);
    for case in f["extras"].as_array().unwrap() {
        assert_eq!(normalized(chapter_sections(case["html"].as_str().unwrap(), case["url"].as_str().unwrap())), case["expected"]);
    }
    for case in f["stems"].as_array().unwrap() {
        assert_eq!(normalized(stems(case["word"].as_str().unwrap())), case["expected"]);
    }
    for case in f["queries"].as_array().unwrap() {
        assert_eq!(normalized(search_manual(&sections, case["question"].as_str().unwrap(), 4)), case["expected"]);
    }
}
#[tokio::test]
async fn cached_manual_tool_results_events_offline_errors_and_abort_match_source() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let client = client(&f);
    let events = Rc::new(RefCell::new(vec![]));
    let seen = events.clone();
    let options = ManualOptions {
        dir: dir.path().to_string_lossy().into(),
        base: Some(f["base"].as_str().unwrap().into()),
        client: Some(client.clone()),
        now: Some(Rc::new(|| 1234)),
        on_event: Some(Rc::new(move |e| seen.borrow_mut().push(serde_json::to_value(e).unwrap()))),
    };
    let tool = manual_tool(options.clone());
    for case in f["cases"].as_array().unwrap() {
        events.borrow_mut().clear();
        let result = tool.execute(case["input"].as_object().unwrap().clone(), Signal::new()).await.unwrap();
        assert_eq!(normalized(result), case["expected"]);
        assert_eq!(*events.borrow(), *case["events"].as_array().unwrap());
    }
    assert_eq!(*client.reads.borrow(), serde_json::from_value::<Vec<String>>(f["reads"].clone()).unwrap());
    client.failed.set(true);
    let offline = manual_tool(options.clone());
    let input = json!({"question":"launch a clip"}).as_object().unwrap().clone();
    assert!(offline.execute(input.clone(), Signal::new()).await.unwrap().text.contains("8.1 Launching Clips"));
    assert_eq!(client.reads.borrow().len(), 3);
    std::fs::remove_file(dir.path().join("manual-12.json")).unwrap();
    let failed = manual_tool(options.clone()).execute(input.clone(), Signal::new()).await.unwrap();
    assert!(failed.is_error && failed.text.contains("just now (offline)"));
    let signal = Signal::new();
    signal.cancel();
    assert!(manual_tool(options).execute(input, signal).await.unwrap_err().is_aborted());
}
#[tokio::test]
async fn concurrent_reads_share_fetch_and_stale_copy_answers_during_background_refresh() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = fixture();
            let dir = tempfile::tempdir().unwrap();
            let mut c = client(&f);
            Rc::get_mut(&mut c).unwrap().delay = true;
            let clock = Rc::new(Cell::new(1234));
            let now = clock.clone();
            let options = ManualOptions {
                dir: dir.path().to_string_lossy().into(),
                base: Some(f["base"].as_str().unwrap().into()),
                client: Some(c.clone()),
                now: Some(Rc::new(move || now.get())),
                ..Default::default()
            };
            let tool = manual_tool(options);
            let input = json!({"question":"warp"}).as_object().unwrap().clone();
            let (a, b) = tokio::join!(tool.execute(input.clone(), Signal::new()), tool.execute(input.clone(), Signal::new()));
            assert_eq!(a.unwrap().text, b.unwrap().text);
            assert_eq!(c.reads.borrow().len(), 3);
            clock.set(1234 + 46 * 24 * 60 * 60000);
            let answer = tool.execute(input, Signal::new()).await.unwrap();
            assert!(!answer.is_error);
            assert_eq!(c.reads.borrow().len(), 3);
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert_eq!(c.reads.borrow().len(), 6);
        })
        .await;
}
#[tokio::test]
async fn changed_contents_page_and_http_errors_are_reported() {
    let c = Rc::new(Client {
        pages: BTreeMap::from([("https://manual.test/".into(), "no chapters".into())]),
        reads: RefCell::new(vec![]),
        failed: Cell::new(false),
        delay: false,
    });
    assert!(fetch_manual(c.clone(), Signal::new(), Some("https://manual.test/"))
        .await
        .unwrap_err()
        .to_string()
        .contains("couldn't find its chapters"));
    assert_eq!(fetch_manual(c, Signal::new(), Some("https://manual.test/missing/")).await.unwrap_err().web().unwrap().status, Some(404));
}
