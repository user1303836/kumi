//! A conversation's log beside its checkpoint: a line cut short by a crash is skipped and the next starts on its own,
//! the log stops at its cap until a request begins it again, it's the producer's alone, its words pass the redactor while
//! its records stay as they were, it waits for the conversation's first save, it begins with what a conversation had and
//! a copy's with the original's, it moves with its conversation, and a later Kumi's is left alone: hermetic, in temporary
//! folders.
use kumi_agent::{
    core::{
        contracts::{
            ConnectionState, ConversationStore, KernelCheckpoint, KernelEvent, SavedConversation, SessionEvent, StopReason, TurnResult,
            TurnState,
        },
        conversations::create_conversation_store,
        session_log::{log_name, read_log, LogLine, ReadLog, SessionLog, MAX_LOG_BYTES},
    },
    kernel::budget::OBSERVATION_MARKER,
};
use kumi_types::place::Places;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
};

const A: &str = "0123456789abcdef0123456789abcdef";
const B: &str = "fedcba9876543210fedcba9876543210";
const ID: &str = "mv66me0m3fa91c";

async fn local<T>(work: impl Future<Output = T>) -> T {
    tokio::task::LocalSet::new().run_until(work).await
}

fn same() -> Rc<dyn Fn(&str) -> String> {
    Rc::new(|text: &str| text.to_owned())
}

fn said(text: &str, at: i64) -> SavedConversation {
    serde_json::from_value(json!({"savedAt":at,"checkpoint":{"version":1,"messages":[
        {"role":"user","content":text},{"role":"assistant","content":"done"}
    ]}}))
    .unwrap()
}

/// A conversation as it was kept: these messages, `turns` requests, kept at `at`.
fn kept(messages: &[Value], turns: u32, at: i64) -> SavedConversation {
    SavedConversation {
        saved_at: at,
        checkpoint: KernelCheckpoint { version: 1, messages: messages.to_vec(), origin: None, tools: None },
        changes: None,
        first: None,
        turns: Some(turns),
    }
}

/// What a conversation had: these messages.
fn had(messages: &[Value]) -> Option<Rc<KernelCheckpoint>> {
    Some(Rc::new(KernelCheckpoint { version: 1, messages: messages.to_vec(), origin: None, tools: None }))
}

fn notice(text: &str) -> SessionEvent {
    SessionEvent::Notice { message: text.into() }
}

fn log_file(root: &Path, place: &str, id: &str) -> PathBuf {
    root.join(place).join("conversations").join(log_name(id))
}

fn read(file: &Path) -> ReadLog {
    read_log(&std::fs::read(file).unwrap()).expect("a log this Kumi reads")
}

fn notices(log: &ReadLog) -> Vec<String> {
    log.lines
        .iter()
        .filter_map(|line| match line {
            LogLine::Event { event: SessionEvent::Notice { message }, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn kind(line: &LogLine) -> &'static str {
    match line {
        LogLine::Header(_) => "header",
        LogLine::Request(_) => "request",
        LogLine::Event { .. } => "event",
        LogLine::Message { .. } => "message",
        LogLine::Saved { .. } => "saved",
    }
}

fn kinds(log: &ReadLog) -> Vec<&'static str> {
    log.lines.iter().map(kind).collect()
}

fn completed() -> SessionEvent {
    SessionEvent::TurnComplete { result: TurnResult { stop_reason: StopReason::Completed, usage: None }, elapsed_ms: 5 }
}

#[test]
fn a_log_reads_past_what_it_cant_and_isnt_one_without_its_header() {
    let header = r#"{"kind":"header","v":1,"id":"mv66me0m3fa91c","at":1,"kumi":"1.25.0","before":2}"#;
    let event = |text: &str| json!({"kind":"event","at":2,"event":{"type":"notice","message":text}}).to_string();
    let lines = [
        header.to_string(),
        event("kept"),
        "not json".into(),
        // A later Kumi's kind of line, and its kind of event: skipped, as the shape says.
        json!({"kind":"later-kind","at":3}).to_string(),
        json!({"kind":"event","at":4,"event":{"type":"a-later-event"}}).to_string(),
        // A line cut short by a crash, that another Kumi's line followed.
        r#"{"kind":"event","at":5,"event":{"ty"#.into(),
        header.to_string(),
        event("also kept"),
    ];
    let read = read_log((lines.join("\n") + "\n").as_bytes()).expect("a log");
    assert_eq!((read.header.id.as_str(), read.header.before, read.header.from.as_deref()), (ID, 2, None));
    assert_eq!(notices(&read), ["kept", "also kept"]);
    assert_eq!(read.lines.len(), 2);
    // The last line cut short is left out, never an error.
    let cut = format!("{header}\n{}\n{}", event("whole"), &event("cut")[..20]);
    assert_eq!(notices(&read_log(cut.as_bytes()).unwrap()), ["whole"]);
    // Without a header this Kumi reads first, it isn't a log it reads: nothing, a header cut short, an event first, a
    // later Kumi's form.
    for bytes in [String::new(), "\n".into(), header.into(), event("first") + "\n", header.replace("\"v\":1", "\"v\":2") + "\n"] {
        assert!(read_log(bytes.as_bytes()).is_none(), "{bytes}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_line_cut_short_is_skipped_and_the_next_one_starts_on_its_own() {
    local(async {
        let dir = tempfile::tempdir().unwrap();
        create_conversation_store(dir.path()).save(A, ID, &said("make the bass wider", 7)).await.unwrap();
        let file = log_file(dir.path(), A, ID);
        let log = SessionLog::new(dir.path(), Places::LIVE, same());
        log.open(A, ID, None, None);
        log.event(&notice("first"));
        log.close().await;
        // Kumi stopped partway through a line.
        let mut cut = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        cut.write_all(br#"{"kind":"event","at":9,"event":{"type":"notice","mess"#).unwrap();
        drop(cut);
        assert_eq!(notices(&read(&file)), ["first"]);
        // The next Kumi's first line starts on a line of its own: both read, and the cut one is skipped.
        let next = SessionLog::new(dir.path(), Places::LIVE, same());
        next.open(A, ID, None, None);
        next.event(&notice("second"));
        next.close().await;
        let bytes = String::from_utf8(std::fs::read(&file).unwrap()).unwrap();
        assert!(bytes.contains("\"mess\n{\"kind\":\"event\""), "{bytes}");
        assert_eq!(notices(&read(&file)), ["first", "second"]);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_log_stops_at_its_cap_until_a_request_begins_it_again() {
    local(async {
        assert_eq!(MAX_LOG_BYTES, 32 * 1024 * 1024);
        let dir = tempfile::tempdir().unwrap();
        create_conversation_store(dir.path()).save(A, ID, &said("make the bass wider", 7)).await.unwrap();
        let file = log_file(dir.path(), A, ID);
        let size = || std::fs::metadata(&file).unwrap().len();
        let log = SessionLog::new(dir.path(), Places::LIVE, same());
        log.set_cap(4096);
        let first = [json!({"role":"user","content":"make the bass wider"}), json!({"role":"assistant","content":"done"})];
        log.open(A, ID, Some(&kept(&first, 1, 7)), None);
        // Notices of 300 bytes until it's full: what would go past 4 KB isn't written, nor anything after it.
        for n in 0..20 {
            log.event(&notice(&format!("{n:02} {}", "x".repeat(300))));
        }
        log.event(&notice("short"));
        log.written().await;
        assert!(size() <= 4096, "{}", size());
        let kept = notices(&read(&file));
        assert!((5..20).contains(&kept.len()), "{}", kept.len());
        assert!(kept.iter().enumerate().all(|(n, text)| text.starts_with(&format!("{n:02} "))), "in order, none left out between");
        // The next request begins it again, from what the conversation had by then.
        let then =
            [first[0].clone(), first[1].clone(), json!({"role":"user","content":"more"}), json!({"role":"assistant","content":"there"})];
        log.request("and wider", vec![], None, 2, had(&then));
        log.event(&notice("after"));
        log.written().await;
        let again = read(&file);
        assert_eq!(again.header.before, 2);
        assert_eq!(kinds(&again), ["message", "message", "message", "message", "request", "event"]);
        assert_eq!(notices(&again), ["after"]);
        assert!(size() <= 4096, "{}", size());
        // A line longer than the whole log is left out, and the log is full again until the next request.
        log.event(&notice(&"y".repeat(5000)));
        log.event(&notice("then"));
        log.close().await;
        assert_eq!(notices(&read(&file)), ["after"]);
    })
    .await;
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_log_is_the_producers_alone() {
    use std::os::unix::fs::PermissionsExt;
    local(async {
        let dir = tempfile::tempdir().unwrap();
        create_conversation_store(dir.path()).save("unsaved", ID, &said("a sketch", 7)).await.unwrap();
        let log = SessionLog::new(dir.path(), Places::LIVE, same());
        log.open("unsaved", ID, None, None);
        log.event(&notice("first"));
        // Once the Set is saved, the log goes to a place of its own, in a folder made for it.
        log.moved(B);
        log.event(&notice("second"));
        log.close().await;
        let file = log_file(dir.path(), B, ID);
        assert_eq!(notices(&read(&file)), ["first", "second"]);
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(file.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(dir.path().join(B)).unwrap().permissions().mode() & 0o777, 0o700);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn words_pass_the_redactor_and_records_stay_as_they_were() {
    local(async {
        let dir = tempfile::tempdir().unwrap();
        create_conversation_store(dir.path()).save(A, ID, &said("make the bass wider", 7)).await.unwrap();
        const KEY: &str = "sk-live-0123456789abcdef";
        let redact: Rc<dyn Fn(&str) -> String> = Rc::new(|text: &str| text.replace(KEY, "[redacted]"));
        let log = SessionLog::new(dir.path(), Places::LIVE, redact);
        log.open(A, ID, None, None);
        log.request(&format!("use {KEY} for the sampler"), vec!["drums.wav".into()], None, 1, None);
        // An answer's words, the key split between two of them: one event, the key taken out whole.
        log.event(&SessionEvent::Kernel(KernelEvent::Text { text: format!("I'll keep {}", &KEY[..10]) }));
        log.event(&SessionEvent::Kernel(KernelEvent::Text { text: format!("{} safe.", &KEY[10..]) }));
        log.event(&SessionEvent::Kernel(KernelEvent::ToolStart { id: format!("call-{KEY}"), name: "set_key".into() }));
        log.event(&SessionEvent::Kernel(KernelEvent::Steer { text: format!("no, {KEY} is old") }));
        log.event(&notice(&format!("Saved {KEY}.")));
        log.event(&SessionEvent::Error { message: format!("{KEY} was refused"), kind: None, provider: Some("openai".into()) });
        let observation = format!("{OBSERVATION_MARKER}\ntrack {KEY}\n</current_observation_untrusted>");
        log.messages(&[
            json!({"role":"user","content":[{"type":"text","text":format!("use {KEY}{observation}")}]}),
            json!({"role":"assistant","content":[
                {"type":"reasoning","text":format!("the key {KEY}"),"providerOptions":{"anthropic":{"signature":"c2ln"}}},
                {"type":"text","text":format!("set {KEY}")},
                {"type":"tool-call","toolCallId":format!("call-{KEY}"),"toolName":"set_key","input":{"key":KEY}}
            ]}),
            json!({"role":"tool","content":[
                {"type":"tool-result","toolCallId":format!("call-{KEY}"),"toolName":"set_key","output":{"type":"text","value":format!("kept {KEY}")}}
            ]}),
            json!({"role":"user","content":format!("an older form {KEY}")}),
        ]);
        log.close().await;
        let read = read(&log_file(dir.path(), A, ID));
        let LogLine::Request(request) = &read.lines[0] else { panic!("{:?}", read.lines[0]) };
        assert_eq!((request.text.as_str(), request.files.as_slice()), ("use [redacted] for the sampler", &["drums.wav".to_string()][..]));
        let events: Vec<Value> = read
            .lines
            .iter()
            .filter_map(|line| match line {
                LogLine::Event { event, .. } => Some(serde_json::to_value(event).unwrap()),
                _ => None,
            })
            .collect();
        assert_eq!(
            events,
            [
                json!({"type":"text","text":"I'll keep [redacted] safe."}),
                // A record names what it is by as it was.
                json!({"type":"tool-start","id":format!("call-{KEY}"),"name":"set_key"}),
                json!({"type":"steer","text":"no, [redacted] is old"}),
                json!({"type":"notice","message":"Saved [redacted]."}),
                json!({"type":"error","message":"[redacted] was refused","provider":"openai"}),
            ]
        );
        let messages: Vec<&Value> = read
            .lines
            .iter()
            .filter_map(|line| match line {
                LogLine::Message { message, .. } => Some(message),
                _ => None,
            })
            .collect();
        // The producer's words, not the DAW's observation Kumi adds after them.
        assert_eq!(messages[0]["content"][0]["text"], format!("use [redacted]{observation}"));
        // The model's words; its reasoning (bound to its signature), its calls and their results as they were.
        assert_eq!(messages[1]["content"][0]["text"], format!("the key {KEY}"));
        assert_eq!(messages[1]["content"][1]["text"], "set [redacted]");
        assert_eq!(messages[1]["content"][2]["input"]["key"], KEY);
        assert_eq!(messages[2]["content"][0]["output"]["value"], format!("kept {KEY}"));
        assert_eq!(messages[3]["content"], "an older form [redacted]");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_conversations_log_waits_for_its_first_save_and_one_never_kept_leaves_none() {
    local(async {
        let dir = tempfile::tempdir().unwrap();
        let store = create_conversation_store(dir.path());
        let file = log_file(dir.path(), A, ID);
        let log = SessionLog::new(dir.path(), Places::LIVE, same());
        log.open(A, ID, None, None);
        log.request("a new song", vec![], None, 0, None);
        log.event(&SessionEvent::State { state: TurnState::Running });
        log.event(&SessionEvent::Kernel(KernelEvent::Text { text: "Here's".into() }));
        log.event(&SessionEvent::Kernel(KernelEvent::Text { text: " a start.".into() }));
        log.event(&completed());
        log.messages(&[json!({"role":"user","content":"a new song"}), json!({"role":"assistant","content":"Here's a start."})]);
        log.written().await;
        // Not kept yet: nothing beside it.
        assert!(!file.exists());
        store.save(A, ID, &said("a new song", 9)).await.unwrap();
        log.saved(ID, 9, 1, None);
        // A save of a conversation this run never wrote is nothing to it.
        log.saved("mv66me0m0other", 10, 4, None);
        log.written().await;
        let read = read(&file);
        assert_eq!((read.header.id.as_str(), read.header.before, read.header.from.as_deref()), (ID, 0, None));
        assert_eq!(kinds(&read), ["request", "event", "event", "event", "message", "message", "saved"]);
        assert!(matches!(&read.lines[2], LogLine::Event { event: SessionEvent::Kernel(KernelEvent::Text { text }), .. } if text == "Here's a start."));
        assert!(matches!(read.lines[6], LogLine::Saved { saved_at: 9, turns: 1, .. }));
        assert!(!log_file(dir.path(), A, "mv66me0m0other").exists());
        // Kept, it's written as it goes: each line is in the file before the session closes.
        log.event(&notice("then"));
        log.written().await;
        assert_eq!(notices(&self::read(&file)), ["then"]);
        // A conversation left before its first save landed (a /new straight after an answer) keeps its lines for it.
        const LEFT: &str = "mv66me0m0left1";
        log.open(A, LEFT, None, None);
        log.request("sketch some chords", vec![], None, 0, None);
        log.event(&completed());
        log.open(A, "mv66me0m0next1", None, None);
        log.event(&notice("New conversation."));
        store.save(A, LEFT, &said("sketch some chords", 11)).await.unwrap();
        log.saved(LEFT, 11, 1, None);
        log.written().await;
        let left = self::read(&log_file(dir.path(), A, LEFT));
        assert_eq!(kinds(&left), ["request", "event", "saved"]);
        assert!(notices(&left).is_empty());
        // A conversation never kept leaves no log.
        log.open(A, "mv66me0m0never", None, None);
        log.request("just looking", vec![], None, 0, None);
        log.close().await;
        assert!(!log_file(dir.path(), A, "mv66me0m0never").exists());
        assert!(!log_file(dir.path(), A, "mv66me0m0next1").exists());
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_log_begins_with_what_its_conversation_had_and_a_copys_with_the_originals_as_the_copy_began() {
    local(async {
        const FORK: &str = "mv66me0m9fork1";
        const WINDOW: &str = "mv66me0m9copy1";
        let dir = tempfile::tempdir().unwrap();
        let store = create_conversation_store(dir.path());
        store.save(A, ID, &said("make the bass wider", 7)).await.unwrap();
        let first = [json!({"role":"user","content":"make the bass wider"}), json!({"role":"assistant","content":"done"})];
        let log = SessionLog::new(dir.path(), Places::LIVE, same());
        // Kept before there were logs: its log begins with its requests and messages.
        log.open(A, ID, Some(&kept(&first, 1, 7)), None);
        log.request("and wider", vec![], None, 1, None);
        log.event(&completed());
        log.saved(ID, 8, 2, None);
        log.written().await;
        let original = read(&log_file(dir.path(), A, ID));
        assert_eq!(original.header.before, 1);
        assert_eq!(kinds(&original), ["message", "message", "request", "event", "saved"]);
        assert!(matches!(&original.lines[0], LogLine::Message { message, .. } if *message == first[0]));
        // Forked: the copy's log begins as the original's is, to its last save.
        store.create(A, FORK, &said("make the bass wider", 8), None, Some(ID)).await.unwrap();
        log.open(A, FORK, Some(&kept(&first, 2, 8)), Some(ID.into()));
        log.event(&notice("Forked"));
        log.written().await;
        let fork = read(&log_file(dir.path(), A, FORK));
        assert_eq!((fork.header.id.as_str(), fork.header.before, fork.header.from.as_deref()), (FORK, 1, Some(ID)));
        assert_eq!(fork.lines[..5], original.lines[..]);
        assert_eq!(notices(&fork), ["Forked"]);
        // Carried on in a copy because another window has the original: the copy's log begins with the original's as
        // it was when the copy began (what its checkpoint holds), not with what that window does after.
        log.open(A, WINDOW, Some(&kept(&first, 2, 8)), Some(ID.into()));
        log.written().await;
        let other = SessionLog::new(dir.path(), Places::LIVE, same());
        other.open(A, ID, None, None);
        other.request("in the other window", vec![], None, 2, None);
        other.written().await;
        log.request("from here", vec![], None, 2, None);
        store.create(A, WINDOW, &said("make the bass wider", 9), None, Some(ID)).await.unwrap();
        log.saved(WINDOW, 9, 3, Some(ID.into()));
        log.written().await;
        let window = read(&log_file(dir.path(), A, WINDOW));
        assert_eq!((window.header.before, window.header.from.as_deref()), (1, Some(ID)));
        assert_eq!(kinds(&window), ["message", "message", "request", "event", "saved", "request", "saved"]);
        let asked: Vec<&str> = window
            .lines
            .iter()
            .filter_map(|line| match line {
                LogLine::Request(request) => Some(request.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(asked, ["and wider", "from here"]);
        // A copy whose original only its first save names (one from a file's, carried on here) begins from that one's.
        const LATER: &str = "mv66me0m9copy2";
        log.open(A, LATER, Some(&kept(&first, 2, 8)), None);
        log.request("and here", vec![], None, 2, None);
        log.written().await;
        store.create(A, LATER, &said("make the bass wider", 10), None, Some(ID)).await.unwrap();
        log.saved(LATER, 10, 3, Some(ID.into()));
        log.close().await;
        other.close().await;
        let later = read(&log_file(dir.path(), A, LATER));
        assert_eq!((later.header.before, later.header.from.as_deref()), (1, Some(ID)));
        assert_eq!(kinds(&later), ["message", "message", "request", "event", "saved", "request", "saved"]);
        // The original's is as it was, and the other window's request after it.
        let now = read(&log_file(dir.path(), A, ID));
        assert_eq!(now.lines[..5], original.lines[..]);
        assert_eq!(kinds(&now), ["message", "message", "request", "event", "saved", "request"]);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_log_behind_what_was_kept_begins_again_from_it_and_one_that_cant_be_read_now_is_left_as_it_is() {
    local(async {
        let dir = tempfile::tempdir().unwrap();
        let store = create_conversation_store(dir.path());
        store.save(A, ID, &said("make the bass wider", 7)).await.unwrap();
        let file = log_file(dir.path(), A, ID);
        let first = [json!({"role":"user","content":"make the bass wider"}), json!({"role":"assistant","content":"done"})];
        let log = SessionLog::new(dir.path(), Places::LIVE, same());
        log.open(A, ID, Some(&kept(&first, 1, 7)), None);
        log.request("and wider", vec![], None, 1, None);
        log.event(&completed());
        log.saved(ID, 8, 2, None);
        // A request whose save never made it into the log (Kumi stopped right after): its end is there.
        log.request("and darker", vec![], None, 2, None);
        log.event(&completed());
        log.close().await;
        // Taken up as kept by that save: it goes on.
        let next = SessionLog::new(dir.path(), Places::LIVE, same());
        next.open(A, ID, Some(&kept(&first, 3, 9)), None);
        next.event(&notice("Continuing."));
        next.saved(ID, 10, 3, None);
        next.close().await;
        assert_eq!(notices(&read(&file)), ["Continuing."]);
        // Kept with more requests counted than the log's (a checkpoint counts each of the producer's lines, a steer's
        // too) but by the save the log has: it goes on.
        let steered = SessionLog::new(dir.path(), Places::LIVE, same());
        steered.open(A, ID, Some(&kept(&first, 7, 10)), None);
        steered.event(&notice("Still going."));
        steered.close().await;
        assert_eq!(notices(&read(&file)), ["Continuing.", "Still going."]);
        assert_eq!(read(&file).header.before, 1);
        // An earlier Kumi kept it since without the log: it begins again from what's kept, with no gap.
        let carried = [
            first[0].clone(),
            first[1].clone(),
            json!({"role":"user","content":"from 1.24"}),
            json!({"role":"assistant","content":"done"}),
        ];
        let after = SessionLog::new(dir.path(), Places::LIVE, same());
        after.open(A, ID, Some(&kept(&carried, 6, 20)), None);
        after.event(&notice("Back again."));
        after.close().await;
        let again = read(&file);
        assert_eq!(again.header.before, 6);
        assert_eq!(kinds(&again), ["message", "message", "message", "message", "event"]);
        assert_eq!(notices(&again), ["Back again."]);
        // Not to be read now (its permissions changed): left as it is, and its lines wait for the next save.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let before = std::fs::read(&file).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
            // (As root, nothing keeps a file from being read: nothing to see.)
            if std::fs::read(&file).is_ok() {
                return;
            }
            let locked = SessionLog::new(dir.path(), Places::LIVE, same());
            locked.open(A, ID, Some(&kept(&carried, 6, 20)), None);
            locked.request("louder", vec![], None, 6, None);
            locked.written().await;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(std::fs::read(&file).unwrap(), before);
            locked.saved(ID, 21, 7, None);
            locked.close().await;
            let read = read(&file);
            assert_eq!(read.lines[..again.lines.len()], again.lines[..]);
            assert_eq!(kinds(&read)[again.lines.len()..], ["request", "saved"]);
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_later_kumis_log_is_left_as_it_is() {
    local(async {
        let dir = tempfile::tempdir().unwrap();
        create_conversation_store(dir.path()).save(A, ID, &said("make the bass wider", 7)).await.unwrap();
        let file = log_file(dir.path(), A, ID);
        // Its header says other things than this Kumi's does.
        let theirs = "{\"kind\":\"header\",\"v\":2,\"made\":\"later\"}\n{\"kind\":\"later\"}\n";
        std::fs::write(&file, theirs).unwrap();
        let log = SessionLog::new(dir.path(), Places::LIVE, same());
        log.open(A, ID, None, None);
        log.request("make it wider", vec![], None, 1, None);
        log.event(&notice("mine"));
        log.close().await;
        assert_eq!(std::fs::read_to_string(&file).unwrap(), theirs);
        assert!(read_log(theirs.as_bytes()).is_none());
        // A first line that never ends, as a later Kumi's header would begin it: left as it is too.
        let longer = format!("{{\"kind\":\"header\",\"v\":3,\"notes\":\"{}", "x".repeat(70_000));
        std::fs::write(&file, &longer).unwrap();
        let log = SessionLog::new(dir.path(), Places::LIVE, same());
        log.open(A, ID, None, None);
        log.event(&notice("mine"));
        log.close().await;
        assert_eq!(std::fs::read_to_string(&file).unwrap(), longer);
        // One that's only damage (zeros, after a power cut) is made again.
        std::fs::write(&file, [0u8; 300]).unwrap();
        let log = SessionLog::new(dir.path(), Places::LIVE, same());
        log.open(A, ID, None, None);
        log.event(&notice("mine"));
        log.close().await;
        assert_eq!(notices(&read(&file)), ["mine"]);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_log_tells_what_happened_not_how_things_are_now() {
    local(async {
        let dir = tempfile::tempdir().unwrap();
        create_conversation_store(dir.path()).save(A, ID, &said("make the bass wider", 7)).await.unwrap();
        let log = SessionLog::new(dir.path(), Places::LIVE, same());
        let shown = Rc::new(RefCell::new(vec![]));
        let screen = {
            let shown = shown.clone();
            SessionLog::tee(&log, Rc::new(move |event| shown.borrow_mut().push(event)))
        };
        // Before a conversation goes on, nothing is logged.
        screen(notice("Starting"));
        log.open(A, ID, None, None);
        let now = [
            SessionEvent::Focus { focus: None },
            SessionEvent::Transport { transport: None },
            SessionEvent::Connection { state: ConnectionState::Connected },
            SessionEvent::Observation { label: "Fixture Set".into(), name: None },
            SessionEvent::Doing { text: "looking at 2:05".into() },
            SessionEvent::Watching { on: true },
            SessionEvent::Palette { palette: None },
            SessionEvent::SetAsked { ask: None },
            SessionEvent::Resend { text: "make it wider".into() },
            SessionEvent::Resumed { saved_at: 7, lines: vec![], changes: None, chosen: None, unreadable: None },
        ];
        let story = [
            SessionEvent::State { state: TurnState::Running },
            notice("The open Set changed."),
            SessionEvent::Error { message: "Kumi stopped.".into(), kind: None, provider: None },
            completed(),
            SessionEvent::ObjectiveCleared,
            SessionEvent::State { state: TurnState::Idle },
        ];
        for (n, event) in story.iter().enumerate() {
            screen(now[n].clone());
            screen(event.clone());
        }
        for event in &now[story.len()..] {
            screen(event.clone());
        }
        log.close().await;
        // The screen got everything, in order; the log, the story.
        assert_eq!(shown.borrow().len(), 1 + now.len() + story.len());
        let read = read(&log_file(dir.path(), A, ID));
        let logged: Vec<SessionEvent> = read
            .lines
            .into_iter()
            .map(|line| match line {
                LogLine::Event { event, .. } => event,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(logged, story);
    })
    .await;
}
