//! `kumi report`, with full redaction oracle results.
#[path = "../support/live_words.rs"]
mod live_words;
use futures::FutureExt;
use kumi::{
    doctor::{DoctorIo, InstallationIo, TerminalInfo, VideoPrograms},
    report::*,
    tui::tty::TtyOutput,
};
use kumi_agent::core::{gaps::gap_tools, store_client::StoreClient};
use kumi_common::abort;
use kumi_types::system::Env;
use serde_json::{json, Value};
use std::{cell::RefCell, fs, rc::Rc};
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
/// The doctor, with what it would look for on this computer answered.
fn doctor(out: Rc<Out>, env: Env) -> DoctorIo {
    let mut doctor = DoctorIo::new(out, env);
    doctor.terminal = Some(TerminalInfo::default());
    doctor.video_programs = Some(Rc::new(|| async { Ok(VideoPrograms::default()) }.boxed_local()));
    doctor.hands = Some(Rc::new(|| async { Ok(None) }.boxed_local()));
    doctor.model_servers = Some(Rc::new(|| async { Ok(vec![]) }.boxed_local()));
    doctor.voice = Some(Rc::new(|| {
        async { Ok(serde_json::from_value(json!({"model":{"name":"ggml-small.en-q5_1.bin"},"fetches":true})).unwrap()) }.boxed_local()
    }));
    // The Installation section's look at this computer, answered: no ports tried, no programs run, no Live looked for.
    doctor.installation = InstallationIo {
        platform: Some("darwin".into()),
        lives: Some(Rc::new(Vec::new)),
        free: Some(Rc::new(|_| async { Some(50e9) }.boxed_local())),
        port: Some(Rc::new(|_| async { None }.boxed_local())),
        live_running: Some(Rc::new(|| async { false }.boxed_local())),
        live_extension_host: Some(Rc::new(|| async { false }.boxed_local())),
        now: None,
    };
    doctor
}
#[test]
fn redaction_removes_keys_tokens_long_values_paths_and_account_names() {
    let cases: Value = serde_json::from_str(include_str!("../support/report-reference.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let secrets: Vec<String> = serde_json::from_value(case["secrets"].clone()).unwrap();
        assert_eq!(
            redactor(&secrets, case["home"].as_str().unwrap(), case["user"].as_str().unwrap())(case["input"].as_str().unwrap()),
            case["output"],
            "{}",
            case["input"]
        );
    }
}
#[tokio::test(flavor = "current_thread")]
async fn report_contains_versions_doctor_latest_conversation_gaps_live_log_without_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    let kumi = dir.path().join(".kumi");
    let projects = kumi.join("projects");
    let conversations = projects.join("a".repeat(32)).join("conversations");
    fs::create_dir_all(&conversations).unwrap();
    let token = "oauth-access-token-0123456789abcdef";
    fs::write(kumi.join("auth.json"),json!({"version":1,"credentials":{"openai-codex":{"type":"oauth","access":token,"refresh":"refresh-token-abcdefghijkl","expires":1,"accountId":"fixture-report-account"}}}).to_string()).unwrap();
    fs::write(kumi.join("settings.json"), json!({"model":"openai-codex/gpt-6-astra"}).to_string()).unwrap();
    fs::write(kumi.join("gaps.jsonl"), format!("{}\n", json!({"missing":"Freezing a track","asked":"Freeze the Bass"}))).unwrap();
    fs::write(
        kumi.join("timings.jsonl"),
        [(1800, "high"), (4200, "high"), (9000, "default")]
            .map(|(ms, effort)| {
                let mut turn = json!({"ms":ms,"stop":"completed","modelCalls":2,"modelMs":ms-600,"firstPartMs":[700,500],"tools":1,"toolMs":400,"liveRequests":3,"sentBytes":204800,"lookMs":ms/30,"lookRequests":3,"lookBytes":58368,"effort":effort,"slowTools":[{"tool":"watch_video","calls":1,"ms":ms-1000},{"tool":"make_changes","calls":2,"ms":300}]});
                if ms == 9000 {
                    turn["tier"] = json!("priority");
                    turn["slowTools"].as_array_mut().unwrap().push(json!({"tool":"search_web","calls":1,"ms":40}));
                }
                turn.to_string() + "\n"
            })
            .concat(),
    )
    .unwrap();
    fs::write(
        conversations.join("older1.json"),
        json!({"savedAt":1,"checkpoint":{"version":1,"messages":[{"role":"user","content":"an older one"}]}}).to_string(),
    )
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    fs::write(conversations.join("latest1.json"),json!({"savedAt":2,"checkpoint":{"version":1,"messages":[{"role":"user","content":[{"type":"text","text":format!("Load my sample from {home}/Music/kick.wav")}]},{"role":"assistant","content":[{"type":"tool-call","toolName":"make_changes","input":{"steps":[{"tool":"set_tempo","input":{"tempo":124}}]}}]},{"role":"tool","content":[{"type":"tool-result","output":{"type":"text","value":"{\"done\":[{\"changed\":\"Tempo 120 → 124 BPM\"}]}"}}]}]},"changes":[{"title":"Tempo 120 → 124 BPM","state":"applied"}]}).to_string()).unwrap();
    // A name beside it, written after it, isn't a conversation.
    fs::write(conversations.join("latest1.meta"), json!({"version":1,"name":"Kick"}).to_string()).unwrap();
    // Newer, but someone else's words: an import, and a copy of one taken up in a Set.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let imports = conversations.parent().unwrap().parent().unwrap().join("imported/conversations");
    fs::create_dir_all(&imports).unwrap();
    let words = |text: &str| json!({"savedAt":3,"checkpoint":{"version":1,"messages":[{"role":"user","content":text}]}}).to_string();
    fs::write(imports.join("import01.json"), words("SOMEONE ELSES WORDS")).unwrap();
    fs::write(conversations.join("copy0001.json"), words("A COPY OF AN IMPORT")).unwrap();
    fs::write(conversations.join("copy0001.meta"), json!({"version":1,"imported":true}).to_string()).unwrap();
    let log = dir.path().join("Log.txt");
    fs::write(
        &log,
        [
            "info: MemoryUsage: fine",
            " Exception: 0x0000000103218c50:0x0000000000000000",
            "error: Python: ValueError: bad value",
            "  MidiRemoteScript 5 [Control Surface=\"None\"]",
            "info: RemoteScriptMessage: (AbletonMcpBridge) Initializing...",
            "error: Python: Traceback (most recent call last):",
            "  File \"bridge.py\", line 1",
            "RuntimeError: boom",
            "info: unrelated",
        ]
        .join("\n"),
    )
    .unwrap();
    let env = Env::from([
        ("KUMI_HOME".into(), kumi.display().to_string()),
        ("KUMI_AUTH_FILE".into(), kumi.join("auth.json").display().to_string()),
        ("KUMI_SETTINGS_FILE".into(), kumi.join("settings.json").display().to_string()),
        ("KUMI_PROJECTS_DIR".into(), projects.display().to_string()),
        ("KUMI_GAPS_FILE".into(), kumi.join("gaps.jsonl").display().to_string()),
        ("KUMI_TIMINGS_FILE".into(), kumi.join("timings.jsonl").display().to_string()),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), dir.path().join("none").display().to_string()),
        ("OPENAI_API_KEY".into(), "sk-live-abcdefghijklmnop".into()),
        ("TERM_PROGRAM".into(), "ghostty".into()),
    ]);
    let out = Rc::new(Out::default());
    let mut io = ReportIo::new(doctor(out.clone(), env));
    io.home = Some(home.clone());
    io.folder = Some(home.clone());
    io.user = Some("fixtureuser".into());
    io.now = Some(Rc::new(|| "2026-09-29T20:00:00Z".parse().unwrap()));
    io.live_logs = Some(Rc::new(move || {
        let log = log.display().to_string();
        async move { Ok(vec![log]) }.boxed_local()
    }));
    assert_eq!(write_report(io).await.unwrap(), 0);
    let file = dir.path().join("kumi-report-2026-09-29T20-00-00.txt");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o077, 0)
    }
    assert!(out.0.borrow().contains(&format!("Kumi's report is in ~{}kumi-report-2026-09-29T20-00-00.txt", std::path::MAIN_SEPARATOR)));
    let text = fs::read_to_string(file).unwrap();
    // A Live producer's report has no sections for DAWs they never set up.
    assert!(!text.contains("REAPER's bridge log") && !text.contains("Renoise's bridge log"), "{text}");
    for heading in [
        "## Versions",
        "## Doctor",
        "## Settings",
        "## Last conversation",
        "## What Kumi couldn't do (gap log)",
        "## Turn timing (last 50 turns)",
        "## Live's log",
    ] {
        assert!(text.contains(heading), "{heading}; report had {} bytes", text.len())
    }
    for expected in [
        "Runtime: native Rust",
        "Terminal: ghostty",
        "model: openai-codex/gpt-6-astra",
        "Conversation latest1, saved",
        "producer: Load my sample from ~/Music/kick.wav",
        "→ make_changes {\"steps\":[{\"tool\":\"set_tempo\"",
        "← {\"type\":\"text\",\"value\":",
        "applied · Tempo 120 → 124 BPM",
        "Freezing a track",
        "3 turns. Medians: 4.2 s an answer · model 3.6 s · tools 0.4 s · 2 model calls · first part 0.7 s · 3 Live requests · 200 KB sent\nLooking at the Set before each answer (medians): 0.1 s · 3 Live requests · 57 KB\nSlowest tools (each turn's three, summed): watch_video 12.0 s (3 calls) · make_changes 0.9 s (6 calls) · search_web 0.0 s (1 call)\nEffort: high (2 turns) · default (1 turn)\nService tier: priority (1 turn)",
        "AbletonMcpBridge) Initializing",
        "RuntimeError: boom",
        "File \"bridge.py\"",
        "Python: ValueError: bad value",
    ] {
        assert!(text.contains(expected), "missing{expected}: {text}")
    }
    for absent in [
        "an older one",
        "SOMEONE ELSES WORDS",
        "A COPY OF AN IMPORT",
        "MemoryUsage",
        "unrelated",
        "MidiRemoteScript",
        "0x0000000103218c50",
        token,
        "refresh-token-abcdefghijkl",
        "sk-live-abcdefghijklmnop",
        &home,
    ] {
        assert!(!text.contains(absent), "{absent}")
    }
}
#[tokio::test(flavor = "current_thread")]
async fn the_gap_log_comes_from_kumis_database_with_what_an_older_kumi_logged_since() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    let kumi = dir.path().join(".kumi");
    fs::create_dir_all(&kumi).unwrap();
    let env = Env::from([
        ("KUMI_HOME".into(), kumi.display().to_string()),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), dir.path().join("none").display().to_string()),
    ]);
    // Kumi logged a gap in its database...
    let (database, _) = StoreClient::open(kumi.join("kumi.db"), kumi::config::json_files(&env).unwrap(), 1).await.unwrap();
    gap_tools(kumi.join("gaps.jsonl"), Some(database))[0]
        .execute(json!({"missing":"Freezing a track","asked":"Freeze the Bass"}).as_object().unwrap().clone(), abort::never())
        .await
        .unwrap();
    // ...and an older Kumi, rolled back to, one in its file.
    fs::write(
        kumi.join("gaps.jsonl"),
        format!("{}\n", json!({"at":"2026-10-01T09:30:00.000Z","kumi":"1.8.1","missing":"Grouping tracks"})),
    )
    .unwrap();
    let mut io = ReportIo::new(doctor(Rc::new(Out::default()), env));
    io.home = Some(home.clone());
    io.folder = Some(home);
    io.user = Some("fixtureuser".into());
    io.now = Some(Rc::new(|| "2026-10-05T20:00:00Z".parse().unwrap()));
    io.live_logs = Some(Rc::new(|| async { Ok(vec![]) }.boxed_local()));
    assert_eq!(write_report(io).await.unwrap(), 0);
    let text = fs::read_to_string(dir.path().join("kumi-report-2026-10-05T20-00-00.txt")).unwrap();
    let gaps = text.split("## What Kumi couldn't do (gap log)").nth(1).unwrap().split("\n## ").next().unwrap();
    let older = gaps.find("Grouping tracks").expect(gaps);
    let newer = gaps.find("Freezing a track").expect(gaps);
    assert!(older < newer, "oldest first: {gaps}");
    assert_eq!(
        kumi_agent::core::gaps::logged_gaps(&kumi.join("kumi.db"), &dir.path().join("none")).unwrap().len(),
        1,
        "and the report read the file's gap without writing it to the database"
    );
}

/// A producer with REAPER's bridge alone: the report (and what Kumi says of it) speaks of REAPER, never of Live.
#[tokio::test(flavor = "current_thread")]
async fn a_reaper_only_report_never_speaks_lives_words() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    let kumi = dir.path().join(".kumi");
    let resource = dir.path().join("REAPER");
    fs::create_dir_all(resource.join("UserPlugins")).unwrap();
    let env = Env::from([
        ("KUMI_HOME".into(), kumi.display().to_string()),
        ("HOME".into(), home.clone()),
        ("KUMI_REAPER_RESOURCE_DIR".into(), resource.display().to_string()),
        ("KUMI_RENOISE_PREFS_DIR".into(), dir.path().join("Renoise").display().to_string()),
    ]);
    let paths = daw_bridge::paths::BridgePaths::new(&kumi, daw_bridge::daw::Daw::Reaper);
    fs::create_dir_all(&paths.root).unwrap();
    let receipt = daw_bridge::lifecycle::Receipt {
        schema: daw_bridge::lifecycle::RECEIPT_SCHEMA.into(),
        daw: "reaper".into(),
        status: "installed".into(),
        target: resource.join("UserPlugins/reaper_kumi.dylib").display().to_string(),
        reference: resource.join("UserPlugins/kumi-bridge-reference.json").display().to_string(),
        generation: Some("g1".into()),
        version: Some("0.1.0".into()),
        sha256: None,
        previous: vec![],
        copies: vec![],
        updated_ms: 0,
        history: vec![],
    };
    fs::write(paths.receipt(), serde_json::to_string(&receipt).unwrap()).unwrap();
    kumi::daw::work_with(kumi::daw::chosen(None, &env).0.id());
    let out = Rc::new(Out::default());
    let mut io = ReportIo::new(doctor(out.clone(), env));
    io.home = Some(home.clone());
    io.folder = Some(home);
    io.user = Some("fixtureuser".into());
    io.now = Some(Rc::new(|| "2026-10-05T20:00:00Z".parse().unwrap()));
    io.live_logs = Some(Rc::new(|| async { Ok(vec![]) }.boxed_local()));
    assert_eq!(write_report(io).await.unwrap(), 0);
    let text = fs::read_to_string(dir.path().join("kumi-report-2026-10-05T20-00-00.txt")).unwrap();
    let said = format!("{}\n{text}", out.0.borrow());
    let leaks = live_words::leaks(&said);
    assert!(leaks.is_empty(), "the report says Live's words:\n{}\n\nin:\n{said}", leaks.join("\n"));
    assert!(said.contains("REAPER"), "{said}");
    kumi::daw::work_with(kumi_types::daw::DawId::Ableton);
}
