//! Exports as Kumi writes them: redacted by `kumi report`'s redactor, pictures drawn for the page, files kept private and
//! never written over another.
use std::cell::RefCell;
use std::fs;

use chrono::FixedOffset;
use kumi::export::{export_command, import_command, picture_of, redactor_maker, render, svg_of, write_new, ExportRequest};
use kumi::report::redactor;
use kumi::tui::tty::TtyOutput;
use kumi_runtime::core::contracts::{ChangePicture, ChangeRecord, ConversationStore, KernelCheckpoint, SavedConversation, TranscriptRole};
use kumi_runtime::core::export::{Format, Source};
use kumi_runtime::kernel::budget::transcript_of;
use kumi_runtime::system::Env;
use kumi_runtime::{create_conversation_store, IMPORTED};
use serde_json::{json, Value};

const HOME: &str = "/Users/janedoe";
const USER: &str = "janedoe";
const KEY: &str = "sk-proj-Q2xheXRvbkVxdWFsaXplcjEyMzQ1Njc4OTA";
const BEARER: &str = "abc123def456ghi789";
const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
/// A sign-in Kumi keeps: known to be secret, whatever it looks like.
const SIGNED_IN: &str = "refresh-token-value-0123";
const NOW: i64 = 1_760_400_000_000;

fn zone() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).unwrap()
}

/// Secrets by their shapes, each with what of it must not survive.
const SHAPES: [(&str, &str); 14] = [
    // An example, not a key, and in pieces as the Stripe one below: whole, secret scanning takes it for a real key.
    (concat!("AIza", "SyEXAMPLE-EXAMPLE-EXAMPLE-EXAMPLE-0"), "EXAMPLE-EXAMPLE-EXAMPLE"),
    ("ghp_16C7e42F292c6912E7710c838347Ae178B4a", "16C7e42F292c6912E77"),
    ("AKIAIOSFODNN7EXAMPLE", "IOSFODNN7EXAMPLE"),
    ("aws_secret_access_key=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", "bPxRfiCYEXAMPLEKEY"),
    ("xoxp-1234567890-1234567890-abcdef1234567890", "abcdef1234567890"),
    // In pieces: whole, a push's secret scanning takes it for a real key.
    (concat!("sk_", "live_4eC39HqLyjWDarjtT1zdp7dc"), "4eC39HqLyjWDarjtT1zdp7dc"),
    ("password=hunter2hunter2", "hunter2hunter2"),
    ("password: 'S1ngl3Quot3dPw'", "S1ngl3Quot3dPw"),
    ("https://api.example.com/v1?token=t0k3nVALUE123&key=k3yVALUE4567", "VALUE"),
    ("https://admin:UsrP4ssW0rdX@db.example.com/main", "UsrP4ssW0rdX"),
    ("%2FUsers%2Fjanedoe%2FMusic", "%2Fjanedoe"),
    ("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b", "9f86d081884c7d659a2f"),
    (r#"{\"password\": \"jsonEsc4pedPw\"}"#, "jsonEsc4pedPw"),
    ("db_password=Pa55w0rdInEnv", "Pa55w0rdInEnv"),
];

/// Every secret planted in `text`'s kind of place.
fn planted(text: &str) -> String {
    let shapes: Vec<&str> = SHAPES.iter().map(|(planted, _)| *planted).collect();
    format!("{text} {KEY} Bearer {BEARER} {JWT} {HOME}/Music/Song.als {USER}'s mix {SIGNED_IN} {}", shapes.join(" "))
}

fn conversation() -> SavedConversation {
    let messages = vec![
        json!({"role": "user", "content": [{"type": "text", "text": planted("Louder kick, please.")}]}),
        json!({"role": "assistant", "content": [
            {"type": "text", "text": planted("Looking.")},
            {"type": "tool-call", "toolCallId": "toolu_1", "toolName": "set_mixer", "input": {"track": planted("Kick")}}
        ]}),
        json!({"role": "tool", "content": [
            {"type": "tool-result", "toolCallId": "toolu_1", "toolName": "set_mixer", "output": {"type": "text", "value": planted("Done")}}
        ]}),
        json!({"role": "assistant", "content": [{"type": "text", "text": planted("Raised it.")}]}),
    ];
    let changes: Value = json!([{
        "id": "c1",
        "family": "mixer",
        "title": planted("Volume -6 → -3 dB"),
        "track": {"name": planted("Kick")},
        "state": "applied",
        "at": NOW - 1000,
        "picture": {"kind": "values", "values": [{"name": planted("Volume"), "fromText": "-6 dB", "toText": "-3 dB"}]}
    }]);
    SavedConversation {
        saved_at: NOW - 100,
        checkpoint: KernelCheckpoint { version: 1, messages, origin: Some("openai/gpt-test".into()), tools: None },
        changes: Some(serde_json::from_value(changes).unwrap()),
        first: Some("Louder kick, please.".into()),
        turns: Some(1),
    }
}

#[test]
fn no_planted_secret_survives_any_export() {
    let conversation = conversation();
    let title = planted("Louder kick");
    let place = planted("Song");
    let source = Source {
        id: "k3x9a1b2",
        conversation: &conversation,
        title: &title,
        place: Some(&place),
        words: ("Live", "Set"),
        created: NOW - 5000,
    };
    let secrets = vec![SIGNED_IN.to_string(), String::new()];
    let redact: &dyn Fn(&str) -> String = &redactor(&secrets, HOME, USER);
    for format in [Format::Markdown, Format::Html, Format::Json] {
        for full in [false, true] {
            let out = render(format, &source, full, Some(redact), zone(), NOW);
            for secret in [KEY, BEARER, JWT, HOME, USER, SIGNED_IN].into_iter().chain(SHAPES.iter().map(|(_, kept)| *kept)) {
                assert!(!out.contains(secret), "{format:?} full={full}: {secret} survived:\n{out}");
            }
            assert!(out.contains("~/Music/Song.als"), "{format:?} full={full}: the home folder wasn't shortened:\n{out}");
            assert!(out.contains("Bearer [secret]"), "{format:?} full={full}:\n{out}");
            assert!(out.contains("Louder kick, please."), "{format:?} full={full}: the producer's words are missing");
        }
    }
    let raw = render(Format::Markdown, &source, false, None, zone(), NOW);
    assert!(raw.contains(KEY), "--raw leaves the file as it is");
    assert!(raw.contains("Nothing was taken out: this file may hold keys, tokens and paths."));
}

#[test]
fn a_full_export_takes_out_what_a_field_named_for_a_secret_holds_whatever_it_looks_like() {
    // Values no shape gives away: only the field's name says they're secret.
    let hidden = [
        "DblxMLBwPWoG8rQN",
        "dXNlcjpwYXNz",
        "k-short",
        "AKIDshort12",
        "tok3nV4lue",
        "sid=abc123",
        "pr0ducerName",
        "PinCode4821",
        "r3sultS3cret",
        "pwTwo99",
        "t3xtPwdV4l",
        "c00kieOne",
        "Errpw7788",
        "awsS3cr3tK",
    ];
    let input = json!({
        "track": "Kick",
        "nested": {"password": "DblxMLBwPWoG8rQN"},
        "auth": "Basic dXNlcjpwYXNz",
        "apiKey": "k-short",
        "access_key": "AKIDshort12",
        "X-Auth-Token": "tok3nV4lue",
        "Cookie": "sid=abc123",
        "AWSSecretKey": "awsS3cr3tK",
        "credential": {"user": "pr0ducerName", "pin": "PinCode4821"},
        "author": "Aphex Twin",
        "max_tokens": 512
    });
    let messages = vec![
        json!({"role": "user", "content": [{"type": "text", "text": "Sign in to the sample store."}]}),
        json!({"role": "assistant", "content": [
            {"type": "tool-call", "toolCallId": "toolu_1", "toolName": "store_login", "input": input},
            {"type": "tool-call", "toolCallId": "toolu_2", "toolName": "store_session", "input": {}},
            {"type": "tool-call", "toolCallId": "toolu_3", "toolName": "store_check", "input": {}}
        ]}),
        json!({"role": "tool", "content": [
            {"type": "tool-result", "toolCallId": "toolu_1", "toolName": "store_login",
                "output": {"type": "json", "value": {"device": "EQ Eight", "session": {"secret": "r3sultS3cret", "Passwd": "pwTwo99"}}}},
            {"type": "tool-result", "toolCallId": "toolu_2", "toolName": "store_session",
                "output": {"type": "text", "value": r#"{"ok":true,"login":{"pwd":"t3xtPwdV4l","cookies":["c00kieOne"]}}"#}},
            {"type": "tool-result", "toolCallId": "toolu_3", "toolName": "store_check",
                "output": {"type": "error-json", "value": {"reason": "expired", "client_secret": "Errpw7788"}}}
        ]}),
        json!({"role": "assistant", "content": [{"type": "text", "text": "Signed in."}]}),
    ];
    let conversation = SavedConversation {
        saved_at: NOW - 100,
        checkpoint: KernelCheckpoint { version: 1, messages, origin: Some("openai/gpt-test".into()), tools: None },
        changes: None,
        first: Some("Sign in to the sample store.".into()),
        turns: Some(1),
    };
    let source = Source {
        id: "k3x9a1b2",
        conversation: &conversation,
        title: "Store",
        place: Some("Song"),
        words: ("Live", "Set"),
        created: NOW - 5000,
    };
    let redact: &dyn Fn(&str) -> String = &redactor(&[], HOME, USER);
    for format in [Format::Markdown, Format::Html, Format::Json] {
        let out = render(format, &source, true, Some(redact), zone(), NOW);
        for secret in hidden {
            assert!(!out.contains(secret), "{format:?}: {secret} survived:\n{out}");
        }
        // What isn't named for a secret stays: the track, an author, a count of tokens, a device, why it failed.
        for kept in ["Kick", "Aphex Twin", "512", "EQ Eight", "expired"] {
            assert!(out.contains(kept), "{format:?}: {kept} was taken out:\n{out}");
        }
    }
    let raw = render(Format::Json, &source, true, None, zone(), NOW);
    assert!(hidden.iter().all(|secret| raw.contains(secret)), "--raw leaves every field as it is:\n{raw}");
}

#[test]
fn files_go_beside_others_never_over_them_and_only_the_producer_reads_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kumi-louder-kick-k3x9a1b2.md");
    let written: Vec<_> = ["one", "two", "three"].iter().map(|text| write_new(&path, text).unwrap()).collect();
    assert_eq!(
        written,
        [path.clone(), dir.path().join("kumi-louder-kick-k3x9a1b2-2.md"), dir.path().join("kumi-louder-kick-k3x9a1b2-3.md")]
    );
    assert_eq!(fs::read_to_string(&written[0]).unwrap(), "one", "a file was written over");
    assert_eq!(fs::read_to_string(&written[2]).unwrap(), "three");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in &written {
            assert_eq!(fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600, "{}", file.display());
        }
    }
    let missing = dir.path().join("no such folder").join("kumi.md");
    assert!(write_new(&missing, "x").is_err(), "a folder that isn't there is an error, not a guess");
}

#[test]
fn pictures_draw_in_the_pages_own_colours_and_load_nothing() {
    let eq: ChangePicture = serde_json::from_value(json!({
        "kind": "eq",
        "before": [{"shape": "bell", "freq": 1000, "gainDb": 0}],
        "after": [{"shape": "bell", "freq": 1000, "gainDb": 4}]
    }))
    .unwrap();
    let svg = svg_of(&eq).unwrap();
    assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 400 160\""), "{svg}");
    for role in ["class=\"now\"", "class=\"ghost\"", "class=\"guide\""] {
        assert!(svg.contains(role), "{role} is missing: {svg}");
    }
    for outside in ["<script", "href", "<image", "url(", "style=", "<foreignObject"] {
        assert!(!svg.contains(outside), "{outside}: {svg}");
    }
    let notes: ChangePicture = serde_json::from_value(json!({
        "kind": "notes",
        "length": 4,
        "before": [[0, 60, 1, 100], [1, 62, 1, 100]],
        "after": [[0, 60, 1, 100], [2, 64, 1, 100]]
    }))
    .unwrap();
    let svg = svg_of(&notes).unwrap();
    assert!(svg.contains("<rect class=\"dim\"") && svg.contains("<rect class=\"now\"") && svg.contains("<rect class=\"ghost\""), "{svg}");
    let values: ChangePicture =
        serde_json::from_value(json!({"kind": "values", "values": [{"name": "Drive", "from": 1, "to": 2}]})).unwrap();
    assert_eq!(svg_of(&values), None, "values read as words on the page");
}

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

#[tokio::test(flavor = "current_thread")]
async fn kumi_export_saves_a_kept_conversation_and_kumi_import_keeps_it_to_carry_on() {
    let home = tempfile::tempdir().unwrap();
    let kumi = |name: &str| Env::from([("KUMI_HOME".to_string(), home.path().join(name).display().to_string())]);
    let here = kumi("here");
    let store = create_conversation_store(home.path().join("here/projects"));
    store.save("0123456789abcdef0123456789abcdef", "k3x9a1b2c4", &conversation()).await.unwrap();
    let folder = home.path().join("files");
    fs::create_dir(&folder).unwrap();
    let (out, err) = (Out::default(), Out::default());
    let ask = |conversation: Option<&str>| ExportRequest {
        conversation: conversation.map(str::to_string),
        format: Format::Json,
        out: Some(folder.display().to_string()),
        full: false,
        raw: false,
    };
    assert_eq!(export_command(ask(Some("zzzz9")), &here, &out, &err, "kumi").await.unwrap(), 1);
    assert!(err.0.borrow().contains("No conversation zzzz9. kumi -r lists them."), "{}", err.0.borrow());
    assert_eq!(export_command(ask(Some("k3x9")), &here, &out, &err, "kumi").await.unwrap(), 0);
    assert_eq!(export_command(ask(None), &here, &out, &err, "kumi").await.unwrap(), 0);
    assert!(out.0.borrow().contains("Your latest conversation: Louder kick, please. · k3x9a1b2c4"), "{}", out.0.borrow());
    let mut files: Vec<_> = fs::read_dir(&folder).unwrap().map(|entry| entry.unwrap().path()).collect();
    files.sort();
    assert_eq!(
        files,
        [folder.join("kumi-louder-kick-please-k3x9a1b2c4-2.json"), folder.join("kumi-louder-kick-please-k3x9a1b2c4.json")],
        "the second export goes beside the first"
    );
    for file in &files {
        let text = fs::read_to_string(file).unwrap();
        for secret in [KEY, BEARER, JWT] {
            assert!(!text.contains(secret), "{secret} survived in {}", file.display());
        }
        assert!(out.0.borrow().contains(&format!("Saved for Kumi: {}", file.display())));
    }
    // Another Kumi keeps it, with its id while that's free, to carry on with kumi -r.
    let there = kumi("there");
    assert_eq!(import_command(&files[1].display().to_string(), &there, &out, &err, "kumi").await.unwrap(), 0);
    assert!(out.0.borrow().contains("as k3x9a1b2c4. Carry it on: kumi -r k3x9a1b2c4"), "{}", out.0.borrow());
    let kept = create_conversation_store(home.path().join("there/projects"));
    let rows = kept.all().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].place.as_str(), rows[0].id.as_str()), (IMPORTED, "k3x9a1b2c4"));
    assert_eq!(rows[0].title(), "Louder kick, please. (imported)");
    let saved = kept.load(IMPORTED, "k3x9a1b2c4").await.unwrap().unwrap();
    let said: Vec<String> = transcript_of(&saved.checkpoint.messages)
        .into_iter()
        .filter(|line| line.role == TranscriptRole::User)
        .map(|line| line.text)
        .collect();
    assert_eq!(said.len(), 1);
    assert!(said[0].starts_with("Louder kick, please. [secret] Bearer [secret] [secret]"), "{}", said[0]);
    assert_eq!(import_command(&files[0].display().to_string(), &there, &out, &err, "kumi").await.unwrap(), 0);
    let ids: Vec<String> = kept.all().await.unwrap().into_iter().map(|row| row.id).collect();
    assert_eq!(ids.len(), 2, "an id that's taken gets another: {ids:?}");
    let again = ids.iter().find(|id| *id != "k3x9a1b2c4").unwrap();
    assert!(
        out.0.borrow().contains(&format!("as {again} (k3x9a1b2c4 is here already, imported before). Carry it on: kumi -r {again}")),
        "{}",
        out.0.borrow()
    );
    // Taken by a conversation of the producer's own.
    assert_eq!(import_command(&files[0].display().to_string(), &here, &out, &err, "kumi").await.unwrap(), 0);
    assert!(out.0.borrow().contains("(its own id, k3x9a1b2c4, is taken here)."), "{}", out.0.borrow());
    let not_kumis = home.path().join("other.json");
    fs::write(&not_kumis, "{}").unwrap();
    assert_eq!(import_command(&not_kumis.display().to_string(), &there, &out, &err, "kumi").await.unwrap(), 1);
    assert!(err.0.borrow().contains("That file isn't a Kumi conversation."));
}

#[test]
fn file_names_and_ordinary_words_come_out_right() {
    let redact = redactor(&[], HOME, USER);
    let title = redact("user hidden pw=hunter2hunter2");
    assert_eq!(kumi_runtime::core::export::file_name(&title, "k3x9a1b2", Format::Markdown), "kumi-user-hidden-pw-secret-k3x9a1b2.md");
    // Words that name a secret without holding one stay as they are.
    for words in [
        "token: abc",
        "refresh: reload the Set",
        "~/.config/ableton-mcp-release-a8a291e/bridge-config.json",
        "key saved-key-1234567890",
        "https://www.ableton.com/en/manual/",
        "the password field is empty",
    ] {
        assert_eq!(redact(words), words);
    }
}

#[test]
fn the_page_draws_what_history_draws_from_a_record() {
    let record = |extra: Value| -> ChangeRecord {
        let mut change = json!({"id": "c1", "family": "clip", "title": "t", "state": "applied", "at": 1});
        change.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(change).unwrap()
    };
    let loads = |page: &str| ["<script", "href", "src=", "<image", "url(", "<foreignObject"].iter().any(|outside| page.contains(outside));
    // A new clip's notes.
    let clip =
        picture_of(&record(json!({"clip": {"length": 4, "notes": [{"pitch": 60, "start": 0, "duration": 1, "velocity": 100}]}}))).unwrap();
    assert!(clip.starts_with("<svg") && clip.contains("<rect class=\"now\""), "{clip}");
    // Where a device went, its name written as words.
    let device =
        picture_of(&record(json!({"devices": {"devices": ["EQ Eight", "<script>alert(1)</script>", "Limiter"], "index": 1}}))).unwrap();
    assert!(device.contains("EQ Eight <span>→</span> <strong>&lt;script&gt;alert(1)&lt;/script&gt;</strong>"), "{device}");
    let rack = picture_of(&record(json!({"devices": {"rack": "Drum Rack", "chains": [{"name": "Kick", "devices": ["Saturator"]}, {"name": "Snare", "devices": []}], "chain": 1}}))).unwrap();
    assert!(rack.contains("<p class=\"rack\">Drum Rack</p>") && rack.contains("<strong>Snare</strong>: empty"), "{rack}");
    // Colours as swatches, and only colours.
    let colors = picture_of(&record(json!({"colors": {"from": "#FF0000", "to": "#00aa55"}}))).unwrap();
    assert!(colors.contains("fill=\"#ff0000\"") && colors.contains("fill=\"#00aa55\""), "{colors}");
    assert_eq!(picture_of(&record(json!({"colors": {"to": "red\" onload=\"x"}}))), None);
    // A value's move as bars.
    let bars = picture_of(&record(json!({"from": 0.25, "to": 0.75, "range": [0, 1]}))).unwrap();
    assert!(
        bars.contains("<rect class=\"ghost\" x=\"0\" y=\"10\" width=\"100.0\"")
            && bars.contains("<rect class=\"now\" x=\"0\" y=\"40\" width=\"300.0\""),
        "{bars}"
    );
    for page in [&clip, &device, &rack, &colors, &bars] {
        assert!(!loads(page), "{page}");
    }
    assert_eq!(picture_of(&record(json!({}))), None, "nothing to draw");
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_import_leaves_nothing_behind() {
    let home = tempfile::tempdir().unwrap();
    let env = Env::from([("KUMI_HOME".to_string(), home.path().join("kumi").display().to_string())]);
    // One exchange past what Kumi keeps of one (about 1 MB): twenty results of 60 KB.
    let calls: Vec<Value> =
        (0..20).map(|n| json!({"type": "tool-call", "toolCallId": format!("x{n}"), "toolName": "read", "input": {}})).collect();
    let results: Vec<Value> = (0..20)
        .map(|n| json!({"type": "tool-result", "toolCallId": format!("x{n}"), "toolName": "read", "output": {"type": "text", "value": "c".repeat(60 * 1024)}}))
        .collect();
    let file = home.path().join("long.json");
    let messages = json!([
        {"role": "user", "content": [{"type": "text", "text": "Read everything"}]},
        {"role": "assistant", "content": calls},
        {"role": "tool", "content": results}
    ]);
    fs::write(&file, json!({"format": "kumi-conversation", "version": 1, "title": "Too long", "messages": messages}).to_string()).unwrap();
    let (out, err) = (Out::default(), Out::default());
    assert_eq!(import_command(&file.display().to_string(), &env, &out, &err, "kumi").await.unwrap(), 1);
    assert!(err.0.borrow().contains("its last exchange runs longer than Kumi keeps of one (about 1 MB)"), "{}", err.0.borrow());
    let kept = home.path().join("kumi/projects").join(IMPORTED).join("conversations");
    let left: Vec<_> = fs::read_dir(&kept).map(|files| files.map(|file| file.unwrap().file_name()).collect()).unwrap_or_default();
    assert!(left.is_empty(), "a refused import left {left:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn an_import_keeps_the_set_its_file_names_for_resume_and_another_export() {
    let home = tempfile::tempdir().unwrap();
    let env = Env::from([("KUMI_HOME".to_string(), home.path().join("kumi").display().to_string())]);
    let file = home.path().join("theirs.json");
    let messages = json!([
        {"role": "user", "content": [{"type": "text", "text": "Warmer pads"}]},
        {"role": "assistant", "content": [{"type": "text", "text": "Warmed them."}]}
    ]);
    fs::write(&file, json!({"format": "kumi-conversation", "version": 1, "id": "t4e1r5s6", "title": "Warmer pads", "set": "Their Song", "messages": messages}).to_string())
        .unwrap();
    let (out, err) = (Out::default(), Out::default());
    assert_eq!(import_command(&file.display().to_string(), &env, &out, &err, "kumi").await.unwrap(), 0);
    let kept = create_conversation_store(home.path().join("kumi/projects"));
    let rows = kept.all().await.unwrap();
    assert_eq!((rows[0].id.as_str(), rows[0].set.as_deref()), ("t4e1r5s6", Some("Their Song")));
    let folder = home.path().join("files");
    fs::create_dir(&folder).unwrap();
    let ask = ExportRequest {
        conversation: Some("t4e1r5s6".into()),
        format: Format::Markdown,
        out: Some(folder.display().to_string()),
        full: false,
        raw: false,
    };
    assert_eq!(export_command(ask, &env, &out, &err, "kumi").await.unwrap(), 0, "{}", err.0.borrow());
    let files: Vec<_> = fs::read_dir(&folder).unwrap().map(|entry| entry.unwrap().path()).collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let page = fs::read_to_string(&files[0]).unwrap();
    assert!(page.contains("- **Set:** Their Song\n"), "{page}");
}

/// `kumi export` speaks the DAW of the conversation's place: a Logic Pro project's says Logic Pro's words, as the session's
/// own export does; Live's and an imported one say Live's, as before.
#[tokio::test(flavor = "current_thread")]
async fn kumi_export_says_the_daw_of_the_conversations_place() {
    use kumi::export::words_of;
    let id = "0123456789abcdef0123456789abcdef";
    assert_eq!(words_of(id).0, ("Live", "Set"));
    assert_eq!(words_of("imported").0, ("Live", "Set"));
    assert_eq!(words_of(&format!("logic-{id}")).0, ("Logic Pro", "project"));
    assert_eq!(words_of("logic-unsaved").0, ("Logic Pro", "project"));
    assert_eq!(words_of(&format!("reaper-{id}")).0, ("REAPER", "project"));
    assert_eq!(words_of(&format!("renoise-{id}")).0, ("Renoise", "song"));
    assert_eq!(words_of("somewhere").0, ("Live", "Set"));
    let home = tempfile::tempdir().unwrap();
    let env = Env::from([("KUMI_HOME".to_string(), home.path().join("kumi").display().to_string())]);
    let store = kumi_runtime::core::conversations::create_conversation_store_in(
        home.path().join("kumi/projects"),
        None,
        kumi_runtime::integrations::PLACES,
    );
    store.save(&format!("logic-{id}"), "l0g1c2b3a4", &conversation()).await.unwrap();
    let folder = home.path().join("files");
    fs::create_dir(&folder).unwrap();
    let (out, err) = (Out::default(), Out::default());
    let ask = ExportRequest {
        conversation: Some("l0g1c2b3a4".into()),
        format: Format::Markdown,
        out: Some(folder.display().to_string()),
        full: false,
        raw: false,
    };
    assert_eq!(export_command(ask, &env, &out, &err, "kumi").await.unwrap(), 0, "{}", err.0.borrow());
    let files: Vec<_> = fs::read_dir(&folder).unwrap().map(|entry| entry.unwrap().path()).collect();
    let page = fs::read_to_string(&files[0]).unwrap();
    assert!(page.contains("What Kumi changed in Logic Pro"), "{page}");
    assert!(!page.contains("What Kumi changed in Live"), "{page}");
}

#[tokio::test(flavor = "current_thread")]
async fn exports_take_out_every_key_kumi_knows_as_it_knows_them_then() {
    let home = tempfile::tempdir().unwrap();
    let kumi = home.path().join("kumi");
    fs::create_dir_all(&kumi).unwrap();
    let servers = |keys: &[&str]| {
        let servers: Vec<Value> = keys
            .iter()
            .enumerate()
            .map(|(n, key)| json!({"name": format!("studio{n}"), "baseURL": "http://studio.lan:1234/v1", "apiKey": key}))
            .collect();
        fs::write(kumi.join("settings.json"), json!({"modelServers": servers}).to_string()).unwrap();
    };
    servers(&["serverkey-first"]);
    let env = Env::from([("KUMI_HOME".to_string(), kumi.display().to_string()), ("LM_API_TOKEN".to_string(), "lmtoken-words".to_string())]);
    let secrets = kumi::report::known_secrets(&env).await;
    assert!(secrets.iter().any(|s| s == "lmtoken-words") && secrets.iter().any(|s| s == "serverkey-first"), "{secrets:?}");
    // Made as Kumi starts; a server's key added later is known at the next export.
    let make = redactor_maker(&env);
    servers(&["serverkey-first", "serverkey-added"]);
    let redact = make().await;
    let out = redact("keys: lmtoken-words serverkey-first serverkey-added");
    for key in ["lmtoken-words", "serverkey-first", "serverkey-added"] {
        assert!(!out.contains(key), "{key} survived: {out}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn words_about_importing_name_the_command_kumi_runs_with() {
    let home = tempfile::tempdir().unwrap();
    let env = Env::from([("KUMI_HOME".to_string(), home.path().join("kumi").display().to_string())]);
    let (out, err) = (Out::default(), Out::default());
    let binary = home.path().join("not-text.json");
    fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
    let other = home.path().join("other.json");
    fs::write(&other, "{}").unwrap();
    for file in [&binary, &other] {
        assert_eq!(import_command(&file.display().to_string(), &env, &out, &err, "cargo run -p kumi --").await.unwrap(), 1);
    }
    let how = "cargo run -p kumi -- import takes a conversation Kumi saved with: cargo run -p kumi -- export --json";
    assert_eq!(err.0.borrow().matches(how).count(), 2, "{}", err.0.borrow());
    let saved = kumi::export::saved_words(Format::Json, std::path::Path::new("k.json"), "cargo run -p kumi --");
    assert!(saved.contains("Another Kumi carries it on after: cargo run -p kumi -- import <this file>."), "{saved}");
}

/// `kumi export` of a REAPER or Renoise conversation speaks that DAW's words, whichever DAW Kumi was started for; a Live
/// conversation's (and an imported one's) stay Live's.
#[test]
fn an_exported_conversation_speaks_the_words_of_the_daw_its_place_is_in() {
    use kumi::export::{words_of, WORDS};
    let id = "0123456789abcdef0123456789abcdef";
    let (words, label) = words_of(&format!("reaper-{id}"));
    assert_eq!(words, ("REAPER", "project"));
    assert_eq!(label("reaper_read"), "looked at your project");
    let (words, label) = words_of("renoise-unsaved");
    assert_eq!(words, ("Renoise", "song"));
    assert_eq!(label("renoise_read"), "looked at your song");
    for place in [id, "unsaved", IMPORTED] {
        assert_eq!(words_of(place).0, WORDS, "{place}");
    }
}
