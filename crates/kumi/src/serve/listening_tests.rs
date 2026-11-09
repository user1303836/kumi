use super::*;
use crate::serve::ctx::{Host, Progress};
use crate::serve::Args;
use async_trait::async_trait;
use kumi_agent::core::contracts::{Integration, Observation};
use kumi_agent::core::view::Transport;
use kumi_types::contracts::{ChangeFamily, ChangeRecord, ChangeState, KernelTool};
use kumi_types::daw::DawId;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};

struct Daw;

thread_local! {
    /// Each budget the runtime gave this test's DAW for a call, and each None it took it back with.
    static BUDGETS: RefCell<Vec<Option<f64>>> = const { RefCell::new(vec![]) };
}

#[async_trait(?Send)]
impl Integration for Daw {
    fn listen_budget(&self, seconds: Option<f64>) {
        BUDGETS.with(|budgets| budgets.borrow_mut().push(seconds));
    }
    async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn observe(&self, _: Signal, _: Option<kumi_agent::core::contracts::ObserveHints>) -> Result<Observation, RuntimeError> {
        unreachable!("the link is given its look")
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

/// One of Live's listening tools as the runtime reaches it, with its own schema: each call's input kept; it records
/// `records` as Kumi's integration does, then answers `answer`.
struct Tool {
    name: &'static str,
    ctx: RefCell<Weak<Ctx>>,
    inputs: RefCell<Vec<JsonObject>>,
    records: RefCell<Vec<ChangeRecord>>,
    answer: RefCell<Result<Value, String>>,
}

#[async_trait(?Send)]
impl KernelTool for Tool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Kumi's listening"
    }
    fn input_schema(&self) -> JsonObject {
        let profile = &DawProfile::ABLETON;
        match self.name {
            "sound" => sound_tool(profile).schema,
            "audition" => audition_tool(profile).schema,
            "groove" => groove_tool(profile).schema,
            "form" => form_tool(profile).schema,
            _ => render_tool(profile).schema,
        }
    }
    async fn execute(&self, input: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
        if let Some(file) = input.get("reference").and_then(Value::as_str).filter(|_| self.name == "audition") {
            // The staged copy is there while the tool reads it.
            assert!(Path::new(file).is_file() || file.contains(":clip:"), "{file}");
        }
        self.inputs.borrow_mut().push(input);
        let ctx = self.ctx.borrow().upgrade().unwrap();
        let key = ctx.link().project().unwrap().key;
        for record in self.records.borrow().iter() {
            ctx.history().record(record, &key, ctx.snapshots());
        }
        Ok(match self.answer.borrow().clone() {
            Ok(answer) => ToolResult::text(answer.to_string()),
            Err(why) => ToolResult::error(why),
        })
    }
}

fn record(id: &str, family: ChangeFamily, title: &str, state: ChangeState) -> ChangeRecord {
    ChangeRecord {
        id: id.into(),
        family,
        title: title.into(),
        track: None,
        from: None,
        to: None,
        range: None,
        colors: None,
        clip: None,
        devices: None,
        state,
        score: None,
        note: None,
        at: 1_700_000_000_000,
        picture: None,
        batch: None,
    }
}

struct Lab {
    dir: tempfile::TempDir,
    ctx: Rc<Ctx>,
    tools: Vec<Rc<Tool>>,
    key: String,
}

impl Lab {
    fn new() -> Lab {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("media")).unwrap();
        let args = Args {
            state_dir: root.join("state"),
            kumi_home: root.join("home"),
            hermes_home: None,
            hermes_root: None,
            daw: DawId::Ableton,
            library_roots: vec![],
            media_roots: vec![root.join("media")],
            tools: BTreeMap::new(),
        };
        let ctx = Ctx::new(args, kumi_types::system::Env::new(), kumi_daw::io::Answers::default()).unwrap();
        let tools: Vec<Rc<Tool>> = ["sound", "audition", "groove", "render", "form"]
            .into_iter()
            .map(|name| {
                Rc::new(Tool {
                    name,
                    ctx: RefCell::new(Rc::downgrade(&ctx)),
                    inputs: RefCell::default(),
                    records: RefCell::default(),
                    answer: RefCell::new(Ok(json!({"heard": "ok"}))),
                })
            })
            .collect();
        ctx.link().connect_for_test(Rc::new(Daw));
        ctx.link().connection().unwrap().transport_for_test(Some(Transport {
            playing: false,
            tempo: Some(120.),
            beat: None,
            at: 0.,
            beats_per_bar: Some(4.),
            meter: None,
            line: None,
        }));
        let project = ctx.link().saw(&Observation {
            key: "live:0123".into(),
            revision: None,
            label: "Night Drive".into(),
            context: String::new(),
            instructions: String::new(),
            tools: tools.iter().map(|tool| tool.clone() as Rc<dyn KernelTool>).collect(),
            project: None,
            set: Some("1".into()),
            tracks: None,
            saved_at: None,
            identity: None,
            native_epoch: None,
        });
        Lab { dir, ctx, tools, key: project.key }
    }

    fn tool(&self, name: &str) -> &Tool {
        self.tools.iter().find(|tool| tool.name == name).unwrap()
    }

    async fn call(&self, operation: &Operation, mut args: Value) -> Value {
        // Each call goes on the project as it is now: one that heard or changed it moves its epoch on.
        let project = self.ctx.link().project().unwrap();
        assert_eq!(project.key, self.key);
        args["expected_project_key"] = json!(project.key);
        args["expected_epoch"] = json!(project.epoch);
        let Value::Object(args) = args else { panic!("an object") };
        let host = Host { session_id: Some("chat-1".into()), tool_call_id: Some("call-1".into()), ..Host::default() };
        (operation.run)(&self.ctx, Call { args, host, signal: Signal::new(), progress: Progress::none() }).await.unwrap()
    }

    fn media(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let file = self.dir.path().canonicalize().unwrap().join("media").join(name);
        std::fs::write(&file, bytes).unwrap();
        file
    }
}

macro_rules! case {
    ($name:ident, $body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap()
}

fn a_wav() -> Vec<u8> {
    kumi_media::audio::decode::float_wav(&[(0..48_000).map(|n| (n as f32 * 0.05).sin() * 0.3).collect()], 48_000)
}

#[test]
fn the_schemas_are_lives_own_bounded_with_no_path_and_no_audio_stem() {
    for operation in [&SOUND, &AUDITION, &GROOVE, &RENDER] {
        let schema = (operation.schema)();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("expected_project_key")) && required.contains(&json!("expected_epoch")), "{}", operation.name);
    }
    assert!((SOUND.schema)()["required"].as_array().unwrap().contains(&json!("from_beat")));
    let audition = (AUDITION.schema)();
    assert_eq!(audition["properties"]["reference_seconds"]["maximum"], json!(60.));
    let reference = audition["properties"]["reference"].to_string();
    assert!(reference.contains("clip_ref") && reference.contains("artifact_id") && !reference.contains("path"), "{reference}");
    let groove = (GROOVE.schema)();
    assert!(groove["properties"].get("audio").is_none() && groove["properties"].get("reference_tempo").is_none());
    assert_eq!((GROOVE.effect)(&object(json!({"clip": "a", "reference": "b"}))), Effect::Read);
    assert_eq!((GROOVE.effect)(&object(json!({"change": "nudged the hats"}))), Effect::Write);
    assert_eq!((GROOVE.effect)(&object(json!({"apply": true}))), Effect::Write);
    assert_eq!((GROOVE.effect)(&object(json!({"done": true}))), Effect::Read);
    for operation in [&SOUND, &AUDITION, &RENDER] {
        assert_eq!((operation.effect)(&object(json!({}))), Effect::Playback, "{}", operation.name);
    }
}

case!(sound_reaches_lives_own_tool_within_a_minute_lead_in_included, async {
    let lab = Lab::new();
    let answer = lab.call(&SOUND, json!({"tracks": ["1:track:0", "1:track:1"], "from_beat": 16})).await;
    assert_eq!(answer["state"], "done", "{answer}");
    assert_eq!(answer["round"]["heard"], "ok");
    assert!(answer["in_hermes"].as_str().unwrap().contains("sound is kumi_sound"));
    let input = lab.tool("sound").inputs.borrow()[0].clone();
    assert_eq!(input["tracks"], json!(["1:track:0", "1:track:1"]));
    assert!(!input.contains_key("expected_project_key"));
    // 116 beats at 120 BPM is 58 s alone, 61 s with the bar before it and the half bar after: refused, Live never asked.
    let long = lab.call(&SOUND, json!({"tracks": ["1:track:0"], "from_beat": 16, "beats": 116})).await;
    assert_eq!((long["state"].as_str(), long["code"].as_str()), (Some("refused"), Some("window_too_long")), "{long}");
    assert_eq!(lab.tool("sound").inputs.borrow().len(), 1);
});

case!(an_audition_hears_a_staged_reference_for_a_minute_at_most_and_is_a_heard_row, async {
    let lab = Lab::new();
    let reference = lab.media("reference.wav", &a_wav());
    lab.tool("audition").records.borrow_mut().push(record("a1", ChangeFamily::Clip, "Auditioned 2 takes", ChangeState::Heard));
    let answer = lab
        .call(
            &AUDITION,
            json!({"candidates": [{"track": "1:track:0"}, {"track": "1:track:1"}], "from_beat": 0, "beats": 16,
                "reference": {"file": reference}}),
        )
        .await;
    assert_eq!(answer["state"], "done", "{answer}");
    let input = lab.tool("audition").inputs.borrow()[0].clone();
    let staged = input["reference"].as_str().unwrap();
    assert!(staged != reference.to_string_lossy() && staged.contains("state"), "a staged copy: {staged}");
    assert!(!Path::new(staged).exists(), "the copy goes with the call");
    assert_eq!(input["reference_seconds"], json!(60.));
    let row = lab.ctx.history().rows(crate::serve::history::KEPT).into_iter().find(|row| row.id == answer["history_ids"][0]).unwrap();
    assert_eq!(row.state, "heard");
    // A clip's ref goes as it is (this Live says no file behind it); over a minute of reference is refused.
    let clip =
        lab.call(&AUDITION, json!({"candidates": [{"track": "1:track:0"}], "from_beat": 0, "reference": {"clip_ref": "1:clip:3"}})).await;
    assert_eq!(clip["state"], "done", "{clip}");
    assert_eq!(lab.tool("audition").inputs.borrow()[1]["reference"], "1:clip:3");
    let long = lab
        .call(
            &AUDITION,
            json!({"candidates": [{"track": "1:track:0"}], "from_beat": 0, "reference": {"clip_ref": "1:clip:3"}, "reference_seconds": 90}),
        )
        .await;
    assert_eq!(long["state"], "refused", "{long}");
    // A raw path as the reference is never taken.
    let raw = lab.call(&AUDITION, json!({"candidates": [{"track": "1:track:0"}], "from_beat": 0, "reference": reference})).await;
    assert_eq!(raw["state"], "refused", "{raw}");
    assert_eq!(lab.tool("audition").inputs.borrow().len(), 2);
});

case!(a_groove_takes_midi_clips_only_and_its_apply_is_one_undoable_change, async {
    let lab = Lab::new();
    let start = lab.call(&GROOVE, json!({"clip": "1:clip:0", "reference": "1:clip:4"})).await;
    assert_eq!(start["state"], "done", "{start}");
    let file = lab.media("drums.wav", &a_wav());
    for args in [
        json!({"clip": "1:clip:0", "reference": file}),
        json!({"clip": "1:clip:0", "reference": "beat.mid"}),
        json!({"clip": "1:clip:0", "reference": "~/drums.wav"}),
        json!({"clip": "1:clip:0", "reference": "1:clip:4", "audio": "1:clip:5"}),
        json!({"clip": "1:clip:0", "reference": "1:clip:4", "reference_tempo": 120}),
    ] {
        let refused = lab.call(&GROOVE, args.clone()).await;
        assert_eq!(refused["state"], "refused", "{args}: {refused}");
    }
    assert_eq!(lab.tool("groove").inputs.borrow().len(), 1, "nothing else reached Live");
    lab.tool("groove").records.borrow_mut().push(record("g1", ChangeFamily::Clip, "Groove applied", ChangeState::Applied));
    let applied = lab.call(&GROOVE, json!({"apply": true, "amount": 0.5})).await;
    assert_eq!(applied["state"], "done", "{applied}");
    assert_eq!(applied["history_ids"].as_array().unwrap().len(), 1);
});

case!(a_render_is_kept_as_an_artifact_lives_copy_deleted_and_no_path_said, async {
    let lab = Lab::new();
    let renders = lab.dir.path().canonicalize().unwrap().join("extension").join("renders");
    std::fs::create_dir_all(&renders).unwrap();
    let written = renders.join("1700000000000-abc123.wav");
    std::fs::write(&written, a_wav()).unwrap();
    *lab.tool("render").answer.borrow_mut() =
        Ok(json!({"file": written, "seconds": 1, "channels": 1, "sampleRate": 48000, "note": "Hear it with listen (file)."}));
    let answer = lab.call(&RENDER, json!({"track": "1:track:2", "from_beat": 0, "beats": 8})).await;
    assert_eq!(answer["state"], "done", "{answer}");
    assert!(answer["round"]["artifact_id"].is_string(), "{answer}");
    assert!(answer["round"].get("file").is_none() && !answer.to_string().contains(&*renders.to_string_lossy()), "{answer}");
    assert!(!written.exists(), "Live's copy goes");
    // Over a minute is refused before Live renders; a file anywhere else is neither kept nor deleted.
    let long = lab.call(&RENDER, json!({"track": "1:track:2", "from_beat": 0, "beats": 121})).await;
    assert_eq!((long["state"].as_str(), long["code"].as_str()), (Some("refused"), Some("window_too_long")), "{long}");
    let elsewhere = lab.media("mine.wav", &a_wav());
    *lab.tool("render").answer.borrow_mut() = Ok(json!({"file": elsewhere}));
    let refused = lab.call(&RENDER, json!({"track": "1:track:2", "from_beat": 0, "beats": 8})).await;
    assert_eq!(refused["state"], "refused", "{refused}");
    assert!(elsewhere.exists());
});

case!(form_hears_the_whole_arrangement_up_to_15_minutes_while_other_listens_keep_a_minute, async {
    let lab = Lab::new();
    assert_eq!((FORM.effect)(&object(json!({}))), Effect::Playback);
    // The whole Arrangement by default: no stretch asked, none added.
    let whole = lab.call(&FORM, json!({})).await;
    assert_eq!(whole["state"], "done", "{whole}");
    assert!(whole["in_hermes"].as_str().unwrap().contains("form is kumi_form"));
    let input = lab.tool("form").inputs.borrow()[0].clone();
    assert!(!input.contains_key("from_beat") && !input.contains_key("beats"), "{input:?}");
    // At 120 BPM, 14 minutes of song plays; 15 whole minutes with the half bar after doesn't.
    let long = lab.call(&FORM, json!({"from_beat": 0, "beats": 1680})).await;
    assert_eq!(long["state"], "done", "{long}");
    let over = lab.call(&FORM, json!({"from_beat": 0, "beats": 1800})).await;
    assert_eq!((over["state"].as_str(), over["code"].as_str()), (Some("refused"), Some("window_too_long")), "{over}");
    assert!(over["reason"].as_str().unwrap().contains("900 seconds in one call here"), "{over}");
    assert_eq!(lab.tool("form").inputs.borrow().len(), 2, "Live never asked for over 15 minutes");
    // Every other listen is still held to a minute: 2 minutes of sound is refused.
    let sound = lab.call(&SOUND, json!({"tracks": ["1:track:0"], "from_beat": 0, "beats": 240})).await;
    assert_eq!(sound["code"], "window_too_long", "{sound}");
    // A reference is staged as an audition's, never a path.
    let reference = lab.media("song.wav", &a_wav());
    let compared = lab.call(&FORM, json!({"reference": {"file": reference}})).await;
    assert_eq!(compared["state"], "done", "{compared}");
    let staged = lab.tool("form").inputs.borrow()[2]["reference"].as_str().unwrap().to_string();
    assert!(staged != reference.to_string_lossy() && !Path::new(&staged).exists(), "{staged}");
    let raw = lab.call(&FORM, json!({"reference": reference})).await;
    assert_eq!(raw["state"], "refused", "{raw}");
});

case!(form_plays_within_the_hosts_budget_and_refuses_a_song_that_doesnt_fit, async {
    let lab = Lab::new();
    BUDGETS.with(|budgets| budgets.borrow_mut().clear());
    // The host's deadline, 390 s: 6 minutes of song plays, the budget goes to Live for the call and is taken back after.
    let fits = lab.call(&FORM, json!({"from_beat": 0, "beats": 720, "max_seconds": 390})).await;
    assert_eq!(fits["state"], "done", "{fits}");
    assert!(!lab.tool("form").inputs.borrow()[0].contains_key("max_seconds"), "the runtime's own field, not Live's");
    assert_eq!(BUDGETS.with(|budgets| budgets.borrow().clone()), [Some(390.), None]);
    // 7 minutes doesn't fit: refused before Live is asked, saying to read it in parts.
    let over = lab.call(&FORM, json!({"from_beat": 0, "beats": 840, "max_seconds": 390})).await;
    assert_eq!((over["state"].as_str(), over["code"].as_str()), (Some("refused"), Some("window_too_long")), "{over}");
    assert!(over["reason"].as_str().unwrap().contains("read the form in parts"), "{over}");
    assert_eq!(lab.tool("form").inputs.borrow().len(), 1);
    // The whole song: the budget goes to Live, which refuses what can't fit; more than 15 minutes is taken as 15.
    BUDGETS.with(|budgets| budgets.borrow_mut().clear());
    let whole = lab.call(&FORM, json!({"max_seconds": 5000})).await;
    assert_eq!(whole["state"], "done", "{whole}");
    assert_eq!(BUDGETS.with(|budgets| budgets.borrow().clone()), [Some(900.), None]);
    for bad in [json!(0), json!(-5), json!("390")] {
        let refused = lab.call(&FORM, json!({"max_seconds": bad})).await;
        assert_eq!(refused["state"], "refused", "{bad}: {refused}");
    }
});

#[test]
fn only_the_extensions_own_render_files_are_taken() {
    assert!(an_extension_render(Path::new("/tmp/kumi-live-extension/tmp/renders/1700000000000-Ab12z9.wav")));
    for file in ["/tmp/renders/../x.wav", "/tmp/other/1-abc.wav", "renders/1-abc.wav", "/tmp/renders/1-abc.mp3", "/tmp/renders/x-abc.wav"] {
        assert!(!an_extension_render(Path::new(file)), "{file}");
    }
}
