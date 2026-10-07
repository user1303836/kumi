//! Web reading, with complete-result oracles.
use kumi_runtime::web::html::{html_to_text, read_html};
use serde_json::Value;
#[test]
fn a_page_becomes_text_with_structure_code_formulas_links_and_no_furniture() {
    for source in [include_str!("support/web/html-reference.json"), include_str!("support/web/html-fastpaths.json")] {
        let fixtures: Vec<Value> = serde_json::from_str(source).unwrap();
        for f in fixtures {
            let html = f["html"].as_str().unwrap();
            let base = f["base"].as_str().unwrap();
            assert_eq!(html_to_text(html, base), f["text"].as_str().unwrap(), "{html}");
            assert_eq!(serde_json::to_value(read_html(html, base)).unwrap(), f["read"], "{html}");
        }
    }
}
#[test]
fn a_big_page_reads_in_a_moment() {
    let block="<div><p>Words with <a href=\"/x\">a link</a>, <code>code</code> and x<sup>2</sup>.</p><script>var a = \"<p>\";</script><ul><li>one</li><li>two</li></ul></div>\n";
    let start = std::time::Instant::now();
    let page = read_html(&format!("<html><body>{}</body></html>", block.repeat(25_000)), "https://example.com/");
    assert!(page.text.len() > 1_000_000);
    // A debug build on a busy runner is slower; its budget still catches quadratic work.
    let budget = if cfg!(debug_assertions) { 10_000 } else { 4_000 };
    assert!(start.elapsed().as_millis() < budget, "4 MB of HTML took {} ms", start.elapsed().as_millis());
}
#[test]
fn a_page_built_to_blow_up_the_reader_reads_small_and_quick() {
    let base = "https://example.com/";
    let start = std::time::Instant::now();
    // Lists, quotes, links and superscripts 20,000 deep: 32 levels count, and deeper ones read as plain text.
    let lists = html_to_text(&"<ul><li>x".repeat(20_000), base);
    assert!(lists.len() < 2_000_000, "{} bytes", lists.len());
    assert!(lists.ends_with(&format!("\n{}- x", "  ".repeat(31))), "{}", &lists[lists.len() - 80..]);
    let quotes = html_to_text(&format!("{}x{}", "<blockquote>".repeat(20_000), "</blockquote>".repeat(20_000)), base);
    assert_eq!(quotes, format!("{}x", "> ".repeat(32)));
    let links = html_to_text(&"<a href=\"https://x.y/\">x".repeat(20_000), base);
    assert_eq!(links, "x".repeat(20_000));
    let scripts = html_to_text(&format!("{}x{}", "<sup>".repeat(20_000), "</sup>".repeat(20_000)), base);
    assert_eq!(scripts, format!("{}^x{}", "^(".repeat(31), ")".repeat(31)));
    // A "<" that opens no tag is text: matching it stops at the next "<", not at the end of the page.
    let unopened = "<a b<a b".repeat(25_000);
    assert_eq!(html_to_text(&unopened, base), unopened);
    let budget = if cfg!(debug_assertions) { 10_000 } else { 4_000 };
    assert!(start.elapsed().as_millis() < budget, "took {} ms", start.elapsed().as_millis());
}
#[test]
fn tags_closed_out_of_order_never_cut_the_text_inside_a_character() {
    // A tag closed out of order rewrites text that a tag still open kept an offset into.
    let base = "https://example.com/";
    assert_eq!(html_to_text("<blockquote>\u{20ac}<a href=\"https://x.y/\"></blockquote>", base), "> [\u{20ac}](https://x.y/)");
    assert_eq!(html_to_text("<sup>\u{65e5}<a href=\"https://x.y/\">x</sup></a>", base), "^([\u{65e5}x)](https://x.y/)");
    // Every mix of up to four of the tags whose ends rewrite the text, around characters of two to three bytes.
    let pieces = [
        "<a href=\"https://x.y/\">",
        "</a>",
        "<sup>",
        "</sup>",
        "<code>",
        "</code>",
        "<blockquote>",
        "</blockquote>",
        "\u{20ac}",
        "\u{65e5}\u{672c}",
        " \u{e9} ",
    ];
    let mut mixes: Vec<String> = vec![String::new()];
    for _ in 0..4 {
        mixes = mixes.iter().flat_map(|mix| pieces.iter().map(move |piece| format!("{mix}{piece}"))).collect();
        for mix in &mixes {
            html_to_text(mix, base);
        }
    }
}

use async_trait::async_trait;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::core::contracts::{KernelTool, SessionEvent};
use kumi_runtime::web::{
    free::{Builtin, FreeServices},
    net::{WebClient, WebFailure, WebRequest, WebResponse},
    read::read_page,
    search::{search_web, SearchWebOptions, SearchWhere},
    tool::{web_tools, WebToolOptions},
};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};

struct RecordedWeb {
    requests: RefCell<VecDeque<Value>>,
}
#[async_trait(?Send)]
impl WebClient for RecordedWeb {
    async fn fetch(&self, url: &str, request: WebRequest) -> Result<WebResponse, WebFailure> {
        // A network request always yields; this exercises sharing searches made concurrently.
        tokio::task::yield_now().await;
        let expected = self.requests.borrow_mut().pop_front().expect("unexpected request");
        assert_eq!(url, expected["url"].as_str().unwrap());
        let mut actual = json!({"method":request.method.as_str(),"headers":request.headers,"body":request.body,"maxBytes":request.max_bytes,"timeoutMs":request.timeout_ms});
        let mut wanted = expected["request"].clone();
        for value in [&mut actual, &mut wanted] {
            if let Some(body) = value["body"].as_str() {
                if let Ok(mut parsed) = serde_json::from_str::<Value>(body) {
                    if let Some(id) = parsed["params"]["arguments"]["session_id"].as_str() {
                        assert_eq!(id.len(), 32);
                        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
                        parsed["params"]["arguments"]["session_id"] = json!("SESSION");
                        value["body"] = json!(stringify(&parsed));
                    }
                }
            }
        }
        assert_eq!(actual, wanted, "request for {url}");
        let e = &expected["response"];
        let kind = e["contentType"].as_str().unwrap();
        let skipped = request.wants.as_ref().is_some_and(|wants| !wants(kind));
        Ok(WebResponse {
            url: e["url"].as_str().unwrap().into(),
            status: e["status"].as_u64().unwrap() as u16,
            headers: serde_json::from_value(e["headers"].clone()).unwrap(),
            content_type: kind.into(),
            charset: None,
            body: if skipped {
                Vec::new()
            } else {
                {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.decode(e["bodyBase64"].as_str().unwrap()).unwrap()
                }
            },
            truncated: e["truncated"].as_bool().unwrap(),
            skipped,
        })
    }
}
async fn perform(action: &Value, web: Rc<RecordedWeb>, services: Rc<FreeServices>, tools: &[Rc<dyn KernelTool>]) -> Value {
    let result: Result<Value, String> = match action["kind"].as_str().unwrap() {
        "page" => read_page(web.as_ref(), action["url"].as_str().unwrap(), None, Some(services))
            .await
            .map(|v| serde_json::to_value(v).unwrap())
            .map_err(|e| e.to_string()),
        "search" => search_web(
            web.as_ref(),
            action["query"].as_str().unwrap(),
            SearchWebOptions {
                about: action["about"].as_str().map(str::to_string),
                count: action["count"].as_u64().unwrap_or(8) as usize,
                scope: if action["where"] == "github" { SearchWhere::Github } else { SearchWhere::Web },
                signal: None,
                services: Some(services),
            },
        )
        .await
        .map(|v| serde_json::to_value(v).unwrap())
        .map_err(|e| e.to_string()),
        tool => tools[usize::from(tool == "tool_read")]
            .execute(action["input"].as_object().unwrap().clone(), Signal::new())
            .await
            .map(|v| serde_json::to_value(v).unwrap())
            .map_err(|e| e.to_string()),
    };
    match result {
        Ok(value) => json!({"ok":value}),
        Err(error) => json!({"error":error}),
    }
}
async fn run_scenario(index: usize) {
    let scenarios: Vec<Value> = serde_json::from_str(include_str!("support/web/reference.json")).unwrap();
    let scenario = &scenarios[index];
    let web = Rc::new(RecordedWeb { requests: RefCell::new(serde_json::from_value(scenario["requests"].clone()).unwrap()) });
    let clock = Rc::new(Cell::new(0.0));
    let now: Rc<dyn Fn() -> f64> = Rc::new({
        let clock = clock.clone();
        move || clock.get()
    });
    let services = Rc::new(FreeServices::new(
        scenario["services"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                Rc::new(match s.as_str().unwrap() {
                    "Exa" => Builtin::Exa,
                    "Parallel" => Builtin::Parallel,
                    "Keenable" => Builtin::Keenable,
                    "Firecrawl" => Builtin::Firecrawl,
                    _ => unreachable!(),
                }) as Rc<dyn kumi_runtime::web::free::FreeService>
            })
            .collect(),
        now.clone(),
        0,
    ));
    let events = Rc::new(RefCell::new(Vec::<SessionEvent>::new()));
    let tools = web_tools(WebToolOptions {
        on_event: Rc::new({
            let events = events.clone();
            move |event| events.borrow_mut().push(event)
        }),
        client: Some(web.clone()),
        services: Some(services.clone()),
        now: Some(now),
    });
    for turn in scenario["turns"].as_array().unwrap() {
        clock.set(turn["clock"].as_f64().unwrap());
        let action = &turn["action"];
        let actual = if action["kind"] == "batch" {
            json!(
                futures::future::join_all(action["actions"].as_array().unwrap().iter().map(|a| perform(
                    a,
                    web.clone(),
                    services.clone(),
                    &tools
                )))
                .await
            )
        } else {
            perform(action, web.clone(), services.clone(), &tools).await
        };
        assert_eq!(stringify(&actual), stringify(&turn["expected"]), "scenario {} action {action}", scenario["name"]);
    }
    assert!(web.requests.borrow().is_empty(), "unmade requests");
    assert_eq!(serde_json::to_value(&*events.borrow()).unwrap(), scenario["events"]);
}
#[tokio::test(flavor = "current_thread")]
async fn github_repositories_folders_and_raw_files() {
    run_scenario(0).await;
}
#[tokio::test(flavor = "current_thread")]
async fn max_patches_devices_and_pictures() {
    run_scenario(1).await;
}
#[tokio::test(flavor = "current_thread")]
async fn pdf_script_and_wall_reader_fallback() {
    run_scenario(2).await;
}
#[tokio::test(flavor = "current_thread")]
async fn search_rotation_resting_fallback_and_github() {
    run_scenario(3).await;
}
#[tokio::test(flavor = "current_thread")]
async fn paginated_tools_cached_reads_and_source_events() {
    run_scenario(4).await;
}
#[tokio::test(flavor = "current_thread")]
async fn search_caching_concurrency_expiry_and_recovery() {
    run_scenario(5).await;
}
#[tokio::test(flavor = "current_thread")]
async fn reader_adapters_and_long_complete_pdf() {
    run_scenario(6).await;
}

async fn local_site() -> (u16, Rc<RefCell<Vec<String>>>, tokio::task::JoinHandle<()>) {
    use std::io::Write;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Rc::new(RefCell::new(Vec::new()));
    let noted = requests.clone();
    let handle = tokio::task::spawn_local(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let noted = noted.clone();
            tokio::task::spawn_local(async move {
                let mut raw = vec![0; 8192];
                let mut filled = 0;
                loop {
                    let n = socket.read(&mut raw[filled..]).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    filled += n;
                    if raw[..filled].windows(4).any(|s| s == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&raw[..filled]);
                let first = request.lines().next().unwrap();
                noted.borrow_mut().push(first.split_whitespace().take(2).collect::<Vec<_>>().join(" "));
                let path = first.split_whitespace().nth(1).unwrap();
                if path == "/slow" {
                    let _ = socket.read(&mut raw).await;
                    return;
                }
                let (status, headers, body) = match path {
                    "/away" => ("302 Found", format!("Location: http://localhost:{port}/secret\r\n"), Vec::new()),
                    "/old" => ("301 Moved Permanently", "Location: /page\r\n".into(), Vec::new()),
                    "/form" => ("303 See Other", "Location: /page\r\n".into(), Vec::new()),
                    "/page" => {
                        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                        gzip.write_all(b"<h1>Hello</h1>").unwrap();
                        ("200 OK", "Content-Type: text/html; charset=utf-8\r\nContent-Encoding: gzip\r\n".into(), gzip.finish().unwrap())
                    }
                    "/big" => ("200 OK", "Content-Type: text/plain\r\n".into(), vec![b'x'; 100_000]),
                    "/paper.pdf" => ("200 OK", "Content-Type: application/pdf\r\n".into(), b"%PDF-1.7 ...".to_vec()),
                    _ => ("200 OK", String::new(), b"secret".to_vec()),
                };
                let header = format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    (port, requests, handle)
}
#[tokio::test(flavor = "current_thread")]
async fn a_private_resolution_is_refused_before_connecting_and_redirects_are_checked() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use futures::FutureExt;
            use kumi_runtime::web::net::{create_web_client, WebClientOptions};
            use std::sync::Arc;
            let (port, requests, server) = local_site().await;
            let tricked = create_web_client(WebClientOptions {
                lookup: Some(Arc::new(|_| async { Ok(vec!["127.0.0.1".parse().unwrap()]) }.boxed())),
                ..Default::default()
            });
            let error = tricked.fetch(&format!("http://kumi-test.example:{port}/secret"), Default::default()).await.unwrap_err();
            assert!(error.to_string().contains("on this computer or a private network"), "{error}");
            assert!(requests.borrow().is_empty());
            let local = create_web_client(WebClientOptions { allow: Some(Arc::new(|_| true)), ..Default::default() });
            let error = local.fetch(&format!("http://127.0.0.1:{port}/away"), Default::default()).await.unwrap_err();
            assert!(error.to_string().contains("not this computer"));
            assert_eq!(*requests.borrow(), ["GET /away"]);
            server.abort();
        })
        .await;
}
#[tokio::test(flavor = "current_thread")]
async fn native_client_redirects_decompresses_caps_skips_times_out_and_cancels() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use kumi_runtime::web::net::{create_web_client, Method, WebClientOptions};
            use std::sync::Arc;
            let (port, requests, server) = local_site().await;
            let client = create_web_client(WebClientOptions { allow: Some(Arc::new(|_| true)), ..Default::default() });
            let base = format!("http://127.0.0.1:{port}");
            let page = client.fetch(&format!("{base}/old"), Default::default()).await.unwrap();
            assert_eq!(page.url, format!("{base}/page"));
            assert_eq!(page.status, 200);
            assert_eq!(page.content_type, "text/html");
            assert_eq!(page.charset.as_deref(), Some("utf-8"));
            assert_eq!(page.body, b"<h1>Hello</h1>");
            let posted = client
                .fetch(&format!("{base}/form"), WebRequest { method: Method::Post, body: Some("q=1".into()), ..Default::default() })
                .await
                .unwrap();
            assert_eq!(posted.body, b"<h1>Hello</h1>");
            assert_eq!(&requests.borrow()[requests.borrow().len() - 2..], ["POST /form", "GET /page"]);
            let big = client.fetch(&format!("{base}/big"), WebRequest { max_bytes: Some(1000), ..Default::default() }).await.unwrap();
            assert_eq!(big.body.len(), 1000);
            assert!(big.truncated);
            let pdf = client
                .fetch(
                    &format!("{base}/paper.pdf"),
                    WebRequest { wants: Some(Rc::new(|kind| kind != "application/pdf")), ..Default::default() },
                )
                .await
                .unwrap();
            assert!(pdf.skipped);
            assert!(pdf.body.is_empty());
            let error =
                client.fetch(&format!("{base}/slow"), WebRequest { timeout_ms: Some(150), ..Default::default() }).await.unwrap_err();
            assert!(error.to_string().contains("didn't answer within"));
            let signal = Signal::new();
            let address = format!("{base}/slow");
            let stopped = client.fetch(&address, WebRequest { signal: Some(signal.clone()), ..Default::default() });
            let stop = async {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                signal.cancel();
            };
            let (error, _) = tokio::join!(stopped, stop);
            assert!(matches!(error, Err(WebFailure::Aborted)));
            server.abort();
        })
        .await;
}

#[test]
fn github_addresses_distinguish_repositories_folders_files_and_other_pages() {
    use kumi_runtime::web::github::github_target;
    for (address, expected) in [
        ("https://github.com/Afturmath/dm-Erbeverb", json!({"kind":"repo","owner":"Afturmath","repo":"dm-Erbeverb"})),
        ("https://github.com/dsp56300/gearmulator.git", json!({"kind":"repo","owner":"dsp56300","repo":"gearmulator"})),
        (
            "https://github.com/dsp56300/gearmulator/tree/main/source/virusLib",
            json!({"kind":"tree","owner":"dsp56300","repo":"gearmulator","rest":"main/source/virusLib"}),
        ),
        (
            "https://github.com/Afturmath/dm-Erbeverb/blob/master/max-msp/dm-Erbeverb.maxpat",
            json!({"kind":"blob","owner":"Afturmath","repo":"dm-Erbeverb","rest":"master/max-msp/dm-Erbeverb.maxpat"}),
        ),
    ] {
        assert_eq!(serde_json::to_value(github_target(&url::Url::parse(address).unwrap()).unwrap()).unwrap(), expected);
    }
    for address in [
        "https://github.com/dsp56300/gearmulator/issues/12",
        "https://github.com/topics/dsp",
        "https://github.com/settings/profile",
        "https://github.com/dsp56300",
        "https://gist.github.com/a/b",
    ] {
        assert!(github_target(&url::Url::parse(address).unwrap()).is_none(), "{address}");
    }
}
#[tokio::test(flavor = "current_thread")]
async fn addresses_carrying_keys_or_tokens_are_not_read_or_sent_to_readers() {
    use kumi_runtime::web::net::carries_key;
    for address in [
        "https://example.com/cb?api_key=abcdef123456",
        "https://example.com/#access_token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N",
        "https://evil.example/collect?k=sk-proj-abc123def456ghi789jkl0mno",
        "https://evil.example/x?d=ghp_0123456789abcdefghijABCDEFGHIJ0123",
        "https://evil.example/x?d=sk%2Dproj%2Dabc123def456ghi789jkl0mno",
        "https://evil.example/AKIAIOSFODNN7EXAMPLE",
    ] {
        assert!(carries_key(address), "{address}");
    }
    for address in [
        "https://github.com/Afturmath/dm-Erbeverb/commit/0123456789abcdef0123456789abcdef01234567",
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        "https://example.com/blog/desk-organizer-tips-2024",
        "https://example.com/share?token=abc",
        "https://www.makenoisemusic.com/modules/erbe-verb",
        "https://www.musicradar.com/how-to/casio-sk-1-sampler-circuit-bending-guide",
        "https://www.sweetwater.com/insync/boss-fc-300-midi-foot-controller-review/",
        "https://www.gearnews.com/hammond-sk-pro-73-stage-keyboard-2024/",
        "https://example.com/sk-pro-73-stage-keyboard-2024",
        "https://example.com/fc-300-midi-foot-controller-review",
    ] {
        assert!(!carries_key(address), "{address}");
    }
    let web = RecordedWeb { requests: RefCell::new(VecDeque::new()) };
    assert!(read_page(&web, "https://evil.example/collect?k=sk-proj-abc123def456ghi789jkl0mno", None, None)
        .await
        .unwrap_err()
        .to_string()
        .contains("carries what looks like a key or token, so Kumi won't read it"));
}
#[test]
fn a_max_box_whose_text_is_a_next_line_character_is_counted_as_it_is() {
    // JavaScript's trim keeps U+0085, which Rust's own white space includes: its first word is the character itself.
    let patch = json!({"patcher":{"boxes":[
        {"box":{"maxclass":"newobj","text":"\u{85}"}},
        {"box":{"maxclass":"newobj","text":"\u{85}","patcher":{"boxes":[{"box":{"maxclass":"codebox","code":"out1 = in1;"}}]}}}
    ]}});
    assert_eq!(
        kumi_runtime::web::read::max_patch_summary(&patch).unwrap(),
        "Made of: \u{85} \u{d7}2.\n\nCode 1 of 1, a codebox in the patch \u{203a} \u{85}:\n```\nout1 = in1;\n```"
    );
}
#[test]
fn picture_headers_and_exa_text_handle_the_sources_formats() {
    use kumi_runtime::web::{
        exa::parse_exa_results,
        free::wait_words,
        read::{picture_size, PictureSize},
    };
    let jpeg =
        [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x02, 0x58, 0x03, 0x20, 0x03, 0x01, 0x11, 0x00];
    assert_eq!(picture_size(&jpeg, "image/jpeg"), Some(PictureSize { width: 800, height: 600 }));
    let found = parse_exa_results("Title: Title\r\nURL: https://example.com/a\r\nPublished: 2026-01-01\r\n");
    assert_eq!(found[0].title, "Title");
    assert_eq!(found[0].published.as_deref(), Some("2026-01-01"));
    assert_eq!([wait_words(60_000.0), wait_words(300_000.0), wait_words(40_124_000.0)], ["a minute", "5 minutes", "11 hours"]);
}
#[tokio::test(flavor = "current_thread")]
async fn no_service_connection_is_said_as_offline() {
    use kumi_runtime::web::net::{WebError, WebTrouble};
    struct Offline;
    #[async_trait(?Send)]
    impl WebClient for Offline {
        async fn fetch(&self, url: &str, _: WebRequest) -> Result<WebResponse, WebFailure> {
            Err(WebError::with_trouble(
                format!(
                    "Kumi couldn't find {}: check the address, or the internet connection.",
                    url::Url::parse(url).unwrap().host_str().unwrap()
                ),
                None,
                WebTrouble::UNREACHABLE,
            )
            .into())
        }
    }
    let services = Rc::new(FreeServices::new(
        vec![Rc::new(Builtin::Exa), Rc::new(Builtin::Parallel), Rc::new(Builtin::Keenable), Rc::new(Builtin::Firecrawl)],
        Rc::new(|| 0.0),
        0,
    ));
    let error =
        search_web(&Offline, "erbe verb", SearchWebOptions { count: 8, services: Some(services), ..Default::default() }).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Kumi couldn't reach any search service (Exa, Parallel, Keenable, Firecrawl or DuckDuckGo): is this computer online?"
    );
}
#[tokio::test(flavor = "current_thread")]
async fn max_patch_json_keeps_source_number_spelling_and_numeric_key_order() {
    struct Patch;
    #[async_trait(?Send)]
    impl WebClient for Patch {
        async fn fetch(&self, url: &str, _request: WebRequest) -> Result<WebResponse, WebFailure> {
            Ok(WebResponse {
                url: url.into(),
                status: 200,
                headers: Default::default(),
                content_type: "application/json".into(),
                charset: None,
                body: br#"{"patcher":{"boxes":[{"box":{"maxclass":"newobj","text":"cycle~"}}],"lines":[],"rect":[0.0,1.0,1e-7,1e20],"10":"ten","2":"two"}}"#.to_vec(),
                truncated: false,
                skipped: false,
            })
        }
    }
    let page = read_page(&Patch, "https://example.com/device.maxpat", None, None).await.unwrap();
    let text = page.text.split_once("The whole patch, as Max saves it:\n").unwrap().1;
    assert_eq!(text,"{\n \"patcher\": {\n  \"2\": \"two\",\n  \"10\": \"ten\",\n  \"boxes\": [\n   {\n    \"box\": {\n     \"maxclass\": \"newobj\",\n     \"text\": \"cycle~\"\n    }\n   }\n  ],\n  \"lines\": [],\n  \"rect\": [\n   0,\n   1,\n   1e-7,\n   100000000000000000000\n  ]\n }\n}");
}
