#[path = "../support/tui_app.rs"]
mod support;
use kumi::tui::alerts::{NotifyChannel, TerminalProfile};
use serde_json::{json, Value};
use std::rc::Rc;
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

fn job(id: u32, state: &str, extra: Value) -> Value {
    let mut job = json!({"type":"job","id":id,"kind":"worker","title":"Burial vocal chops","state":state,"whenDone":"tell","startedAt":now_ms() - 65_000,"steps":0,"sources":0});
    for (key, value) in extra.as_object().unwrap() {
        job[key] = value.clone();
    }
    job
}

fn harness(width: i32) -> Harness {
    let control = Rc::new(Control::default());
    control.set("jobs", json!(true));
    Harness::with(width, 40, control, |_| {})
}

/// The strip's line: the row the tabs' names are on.
fn strip(lines: &[String]) -> usize {
    lines.iter().position(|s| s.contains("HISTORY") && s.contains("───")).unwrap()
}

case!(the_header_counts_running_jobs_and_jobs_joins_the_strip_with_the_first, async {
    let h = harness(140);
    h.start().await;
    h.connect();
    let lines = h.screen();
    assert!(!has(&lines, "JOBS"), "no JOBS before the first job");
    assert!(!lines[0].contains("job"));
    h.emit(job(1, "running", json!({"doing":"reading a page"})));
    let lines = h.screen();
    assert!(lines[0].contains("◐ 1 job"), "{}", lines[0]);
    assert!(lines[strip(&lines)].contains("JOBS 1"), "{}", lines[strip(&lines)]);
    h.emit(job(2, "running", json!({"title":"Tape loop tutorial","kind":"tool","tool":"watch_video"})));
    assert!(h.screen()[0].contains("◐ 2 jobs"));
    h.emit(job(2, "done", json!({"title":"Tape loop tutorial","line":"Loops tape at 7.5 ips","steps":1,"endedAt":now_ms()})));
    assert!(h.screen()[0].contains("◐ 1 job"));
    h.emit(job(1, "stopped", json!({"endedAt":now_ms()})));
    let lines = h.screen();
    assert!(!lines[0].contains("job"), "{}", lines[0]);
    // The tab stays with what ended, its count gone.
    assert!(lines[strip(&lines)].contains("JOBS") && !lines[strip(&lines)].contains("JOBS 1"));
    // Idle, NOW says nothing of jobs once none runs.
    h.has("Ready");
    h.close().await;
});

case!(jobs_rows_show_progress_x_or_a_click_stops_one_and_enter_opens_it, async {
    let h = harness(140);
    h.start().await;
    h.connect();
    h.control.set(
        "job-details",
        json!([{"status":job(1, "running", json!({"doing":"reading a page","steps":2,"tool":"read_web"})),"asked":"Find how Burial chops vocals","steps":[{"tool":"search_web","text":"searched “Burial vocal chops”","state":"done"},{"tool":"read_web","text":"","state":"running"}]}]),
    );
    h.emit(job(1, "running", json!({"doing":"reading a page","steps":2,"tool":"read_web"})));
    // Into the tabs, then on to JOBS.
    h.type_text("\x1b[Z").await;
    h.type_text("\x1b[Z").await;
    let lines = h.screen();
    assert!(has(&lines, "Burial vocal chops"), "{}", lines.join("\n"));
    assert!(has(&lines, "reading a page · 1:0"), "{}", lines.join("\n"));
    assert!(has(&lines, "stop"));
    // x stops the job the keyboard is on.
    h.type_text("x").await;
    h.wait_for_call("stop-job:1").await;
    // It went to the job, not the box.
    h.has("Ask Kumi about your Set");
    // A click on its stop does too.
    let lines = h.screen();
    let row = lines.iter().position(|l| l.contains("Burial vocal chops") && l.contains("stop")).unwrap();
    h.type_text(&click(&lines, row, "stop")).await;
    assert_eq!(h.calls().iter().filter(|c| *c == "stop-job:1").count(), 2);
    // Enter opens what it was asked, its steps and where it is.
    h.type_text("\x1b[Z").await;
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    for text in [
        "Job 1 · Burial vocal chops",
        "running · 1:0",
        "Asked: Find how Burial chops vocals",
        "searched “Burial vocal chops”",
        "reading a page",
    ] {
        h.has(text);
    }
    h.has("x stops it");
    h.type_text("\x1b").await;
    h.wait_until_hidden("Asked: Find how Burial chops vocals").await;
    h.close().await;
});

case!(a_card_arrives_while_the_producer_types_and_their_words_stay, async {
    let h = harness(140);
    h.start().await;
    h.connect();
    h.emit(job(1, "running", json!({})));
    h.type_text("make the bass").await;
    h.emit(job(1, "done", json!({"line":"Burial pitches vocal snippets up and drowns them in reverb.","steps":5,"sources":3,"usage":{"inputTokens":11000.0,"outputTokens":1400.0,"cacheReadTokens":0.0,"cacheWriteTokens":0.0},"endedAt":now_ms()})));
    h.has("✓ Job 1 · Burial vocal chops · 5 steps, 3 sources · 12.4k tokens");
    h.has("Burial pitches vocal snippets up and drowns them in reverb.");
    // The box still holds their words.
    h.has("make the bass");
    // One that failed says why.
    h.emit(job(2, "running", json!({"title":"Manual: Grain Delay"})));
    h.emit(job(2, "failed", json!({"title":"Manual: Grain Delay","line":"The manual couldn't be reached.","endedAt":now_ms()})));
    h.has("✗ Job 2 · Manual: Grain Delay · failed");
    h.has("The manual couldn't be reached.");
    h.close().await;
});

case!(a_job_that_carries_on_waits_until_the_producer_isnt_in_the_middle_of_something, async {
    let h = harness(140);
    h.start().await;
    h.connect();
    h.emit(job(1, "running", json!({"whenDone":"continue"})));
    h.type_text("half a thought").await;
    h.emit(job(1, "done", json!({"whenDone":"continue","line":"Done.","endedAt":now_ms()})));
    delay(50).await;
    assert!(!h.calls().iter().any(|c| c.starts_with("carry-on")), "not while they type");
    // The box emptied: Kumi carries on, as itself.
    h.type_text("\x15").await;
    h.wait_for_call("carry-on:1").await;
    h.emit(json!({"type":"carry-on","job":1,"title":"Burial vocal chops"}));
    h.emit(json!({"type":"state","state":"running"}));
    h.has("Carrying on with job 1's result: Burial vocal chops");
    h.close().await;
});

case!(ctrl_b_moves_the_step_and_slash_jobs_stops_one, async {
    let h = harness(140);
    h.start().await;
    h.connect();
    h.emit(json!({"type":"state","state":"running"}));
    h.type_text("\x02").await;
    h.wait_for_call("background").await;
    h.has("Moved to the background as job 7");
    h.control.set("background-refused", json!("Nothing to move: only a step away from the DAW can go to the background."));
    h.type_text("\x02").await;
    h.has("Nothing to move");
    h.emit(json!({"type":"state","state":"idle"}));
    // /jobs: a running job stops; it works while Kumi answers too.
    let running = job(3, "running", json!({"doing":"watching a video"}));
    let running_detail = running.clone();
    h.control.set("jobs", json!([running.clone()]));
    h.emit(running);
    h.emit(json!({"type":"state","state":"running"}));
    h.control.set("job-details", json!([{"status":running_detail,"asked":"Watch the tape loop tutorial","steps":[]}]));
    h.type_text("/jobs\r").await;
    h.has("Job 3 · Burial vocal chops");
    // Enter opens it; x stops it there.
    h.type_text("\r").await;
    h.wait_for("Asked: Watch the tape loop tutorial").await;
    assert!(!h.calls().iter().any(|c| c.starts_with("stop-job")));
    h.type_text("x").await;
    h.wait_for_call("stop-job:3").await;
    h.type_text("\x1b").await;
    // Esc stops the answer, not the job, and says so.
    h.type_text("\x1b").await;
    h.wait_for("A job is still running in the background: /jobs to see or stop it.").await;
    h.close().await;
});

case!(a_job_has_its_own_status_record_and_its_end_tells_a_producer_whos_away, async {
    let control = Rc::new(Control::default());
    control.set("jobs", json!(true));
    let h = Harness::with(140, 40, control, |o| {
        o.terminal = Some(TerminalProfile {
            status: true,
            progress: false,
            notify: NotifyChannel::Osc777,
            tmux: false,
            app_id: None,
            images: false,
            iterm: false,
        });
        o.alert_gap_ms = Some(60.);
    });
    h.start().await;
    h.connect();
    // The producer goes to Live.
    h.type_text("\x1b[O").await;
    h.emit(job(1, "running", json!({"doing":"reading a page"})));
    let written = || h.output.text.borrow().clone();
    let record = |state: &str| format!("\x1b]7501;state={state}:id=job-1:");
    assert!(written().contains(&record("working")), "the job's own record, beside Kumi's");
    let before = written().matches("id=job-1").count();
    h.emit(job(1, "running", json!({"doing":"reading a page"})));
    assert_eq!(written().matches("id=job-1").count(), before, "the same record isn't written twice");
    h.emit(job(1, "done", json!({"line":"Burial pitches vocal snippets up.","steps":2,"endedAt":now_ms()})));
    assert!(written().contains(&record("done")));
    // Away, its end is told: a notification and a line in Live.
    h.wait_for_call("daw-message:Kumi · job done · Burial vocal chops").await;
    assert!(written().contains("\x1b]777;notify;Kumi;Job 1 done: Burial vocal chops"), "a desktop notification");
    // One stopped leaves no record behind.
    h.emit(job(2, "running", json!({"title":"Tape loop tutorial"})));
    h.emit(job(2, "stopped", json!({"title":"Tape loop tutorial","endedAt":now_ms()})));
    assert!(written().contains("\x1b]7501;state=clear:id=job-2\x1b\\"));
    // Back at Kumi's window: the finished job's record goes too, so a long session doesn't pile them up.
    assert!(!written().contains("\x1b]7501;state=clear:id=job-1\x1b\\"));
    h.type_text("\x1b[I").await;
    assert!(written().contains("\x1b]7501;state=clear:id=job-1\x1b\\"), "cleared once the producer is back");
    // One that ends while they look on is seen in its card: no done record stays.
    h.emit(job(3, "running", json!({"title":"Manual: Grain Delay"})));
    assert!(written().contains("state=working:id=job-3:"));
    h.emit(job(3, "done", json!({"title":"Manual: Grain Delay","line":"Grain Delay pitches each grain.","endedAt":now_ms()})));
    assert!(!written().contains("state=done:id=job-3:"));
    assert!(written().contains("\x1b]7501;state=clear:id=job-3\x1b\\"));
    h.close().await;
});
