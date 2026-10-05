//! Port of `apps/kumi/test/report.test.ts` with full TypeScript redaction oracle results.
use futures::FutureExt;
use kumi::{
    doctor::{DoctorIo, TerminalInfo, VideoPrograms},
    report::*,
    tui::tty::TtyOutput,
};
use kumi_runtime::system::Env;
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
#[test]
fn redaction_removes_keys_tokens_long_values_paths_and_account_names() {
    let cases: Value = serde_json::from_str(include_str!("support/report-reference.json")).unwrap();
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
        [1800, 4200, 9000]
            .map(|ms| json!({"ms":ms,"stop":"completed","modelCalls":2,"modelMs":ms-600,"firstPartMs":[700,500],"tools":1,"toolMs":400,"liveRequests":3,"sentBytes":204800}).to_string() + "\n")
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
    let mut doctor = DoctorIo::new(out.clone(), env);
    doctor.terminal = Some(TerminalInfo::default());
    doctor.video_programs = Some(Rc::new(|| async { Ok(VideoPrograms::default()) }.boxed_local()));
    doctor.hands = Some(Rc::new(|| async { Ok(None) }.boxed_local()));
    doctor.model_servers = Some(Rc::new(|| async { Ok(vec![]) }.boxed_local()));
    doctor.voice = Some(Rc::new(|| {
        async { Ok(serde_json::from_value(json!({"model":{"name":"ggml-small.en-q5_1.bin"},"fetches":true})).unwrap()) }.boxed_local()
    }));
    let mut io = ReportIo::new(doctor);
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
        "producer: Load my sample from ~/Music/kick.wav",
        "→ make_changes {\"steps\":[{\"tool\":\"set_tempo\"",
        "← {\"type\":\"text\",\"value\":",
        "applied · Tempo 120 → 124 BPM",
        "Freezing a track",
        "3 turns. Medians: 4.2 s an answer · model 3.6 s · tools 0.4 s · 2 model calls · first part 0.7 s · 3 Live requests · 200 KB sent",
        "AbletonMcpBridge) Initializing",
        "RuntimeError: boom",
        "File \"bridge.py\"",
        "Python: ValueError: bad value",
    ] {
        assert!(text.contains(expected), "missing{expected}: {text}")
    }
    for absent in [
        "an older one",
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
