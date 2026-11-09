use super::*;
use crate::serve::ctx::{Host, Progress};
use crate::serve::history::Row;
use crate::serve::Args;
use async_trait::async_trait;
use kumi_agent::core::contracts::{Integration, Observation};
use kumi_common::abort::Signal;
use kumi_types::contracts::{ChangeFamily, ChangeRecord, ChangeState, KernelTool};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};

struct Daw;

#[async_trait(?Send)]
impl Integration for Daw {
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

/// Live's judge or tune tool as the runtime reaches it, with its own schema: each call's input kept; a call records
/// `writes` changes as Kumi's integration does, takes back change `take_back` (recorded undone), then answers `answer`.
struct Listening {
    name: &'static str,
    ctx: RefCell<Weak<Ctx>>,
    inputs: RefCell<Vec<JsonObject>>,
    writes: Cell<usize>,
    take_back: RefCell<Option<ChangeRecord>>,
    answer: RefCell<Result<Value, String>>,
}

fn record(id: &str, state: ChangeState) -> ChangeRecord {
    ChangeRecord {
        id: id.into(),
        family: ChangeFamily::Device,
        title: format!("Change {id}"),
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

#[async_trait(?Send)]
impl KernelTool for Listening {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Kumi's listening"
    }
    fn input_schema(&self) -> JsonObject {
        if self.name == "judge" {
            judge_tool(&DawProfile::ABLETON).schema
        } else {
            tune_tool(&DawProfile::ABLETON).schema
        }
    }
    async fn execute(&self, input: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
        if let Some(file) = input.get("goal").and_then(|goal| goal.get("reference")).and_then(Value::as_str) {
            // The staged copy is there while the tool reads it.
            assert!(std::path::Path::new(file).is_file() || file.contains(":clip:"), "{file}");
        }
        self.inputs.borrow_mut().push(input);
        let call = self.inputs.borrow().len();
        let ctx = self.ctx.borrow().upgrade().unwrap();
        let key = ctx.link().project().unwrap().key;
        for n in 0..self.writes.get() {
            ctx.history().record(&record(&format!("{}{call}-{n}", self.name), ChangeState::Applied), &key, ctx.snapshots());
        }
        if let Some(mut back) = self.take_back.borrow_mut().take() {
            back.state = ChangeState::Undone;
            ctx.history().record(&back, &key, ctx.snapshots());
        }
        Ok(match self.answer.borrow().clone() {
            Ok(answer) => ToolResult::text(answer.to_string()),
            Err(why) => ToolResult::error(why),
        })
    }
}

struct Lab {
    dir: tempfile::TempDir,
    ctx: Rc<Ctx>,
    judge: Rc<Listening>,
    tune: Rc<Listening>,
    key: String,
    epoch: u64,
}

fn listening(name: &'static str, ctx: &Rc<Ctx>) -> Rc<Listening> {
    Rc::new(Listening {
        name,
        ctx: RefCell::new(Rc::downgrade(ctx)),
        inputs: RefCell::default(),
        writes: Cell::new(0),
        take_back: RefCell::new(None),
        answer: RefCell::new(Ok(
            json!({"kind": "start", "next": "loudness", "note": "Make one change toward next, then call judge with change."}),
        )),
    })
}

impl Lab {
    fn new(daw: DawId) -> Lab {
        let dir = tempfile::tempdir().unwrap();
        // Approved roots are whole paths without links: the temporary folder's own (macOS links /var to /private/var).
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("media")).unwrap();
        let args = Args {
            state_dir: root.join("state"),
            kumi_home: root.join("home"),
            hermes_home: None,
            hermes_root: None,
            daw,
            library_roots: vec![],
            media_roots: vec![root.join("media")],
            tools: BTreeMap::new(),
        };
        let ctx = Ctx::new(args, kumi_types::system::Env::new(), kumi_daw::io::Answers::default()).unwrap();
        let (judge, tune) = (listening("judge", &ctx), listening("tune", &ctx));
        ctx.link().connect_for_test(Rc::new(Daw));
        let project = ctx.link().saw(&Observation {
            key: "live:0123".into(),
            revision: None,
            label: "Night Drive".into(),
            context: String::new(),
            instructions: String::new(),
            tools: vec![judge.clone() as Rc<dyn KernelTool>, tune.clone() as Rc<dyn KernelTool>],
            project: None,
            set: Some("1".into()),
            tracks: None,
            saved_at: None,
            identity: None,
            native_epoch: None,
        });
        Lab { dir, ctx, judge, tune, key: project.key, epoch: project.epoch }
    }

    fn on_project(&self, mut args: Value) -> Value {
        args["expected_project_key"] = json!(self.key);
        args["expected_epoch"] = json!(self.epoch);
        args
    }

    async fn call(&self, operation: &Operation, args: Value) -> Value {
        let Value::Object(args) = args else { panic!("an object") };
        let host = Host { session_id: Some("chat-1".into()), tool_call_id: Some("call-1".into()), ..Host::default() };
        (operation.run)(&self.ctx, Call { args, host, signal: Signal::new(), progress: Progress::none() }).await.unwrap()
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

case!(a_goal_hears_a_bounded_stretch_with_its_reference_resolved_never_a_path_and_on_the_looked_project, async {
    let lab = Lab::new(DawId::Ableton);
    let note = lab.dir.path().canonicalize().unwrap().join("media").join("reference.wav");
    std::fs::write(&note, b"RIFF a reference").unwrap();
    let goal = json!({"goal": {"loudness": -14, "reference": {"file": note}}, "track": "1:track:0", "from_beat": 16, "beats": 32});
    assert_eq!((JUDGE.effect)(&object(goal.clone())), Effect::Playback);
    assert_eq!((JUDGE.effect)(&object(json!({"change": "cut 300 Hz"}))), Effect::Write);
    assert_eq!((TUNE.effect)(&object(json!({}))), Effect::Write);
    let answer = lab.call(&JUDGE, lab.on_project(goal.clone())).await;
    assert_eq!(answer["state"], "done", "{answer}");
    assert_eq!(answer["round"]["next"], "loudness");
    assert!(answer["in_hermes"].as_str().unwrap().contains("make_changes is kumi_change"));
    assert_eq!((answer["project_key"].clone(), answer["epoch"].clone()), (json!(lab.key), json!(lab.epoch + 1)));
    let input = lab.judge.inputs.borrow()[0].clone();
    let staged = input["goal"]["reference"].as_str().unwrap();
    assert!(
        !staged.ends_with("reference.wav") && staged.starts_with(&*lab.dir.path().canonicalize().unwrap().join("state").to_string_lossy()),
        "a copy: {staged}"
    );
    assert!(!std::path::Path::new(staged).exists(), "the copy goes with the call");
    assert_eq!((input["from_beat"].clone(), input["beats"].clone(), input["track"].clone()), (json!(16), json!(32), json!("1:track:0")));
    assert!(!input.contains_key("expected_project_key") && !input.contains_key("expected_epoch"));
    // A clip in the Set is named by its ref.
    let clip = json!({"goal": {"reference": {"clip_ref": "1:clip:0:2"}}, "from_beat": 0, "beats": 16});
    lab.call(
        &JUDGE,
        json!({"expected_project_key": lab.key, "expected_epoch": lab.epoch + 1, "goal": clip["goal"], "from_beat": 0, "beats": 16}),
    )
    .await;
    assert_eq!(lab.judge.inputs.borrow()[1]["goal"]["reference"], "1:clip:0:2");

    // Refused before Live hears of it: a path for a reference, a file outside the approved folders, no stretch, two
    // kinds of call at once, an old epoch.
    let lab = Lab::new(DawId::Ableton);
    let outside = lab.dir.path().canonicalize().unwrap().join("outside.wav");
    std::fs::write(&outside, b"RIFF").unwrap();
    for (args, code) in [
        (json!({"goal": {"reference": "/Users/me/ref.wav"}, "from_beat": 0, "beats": 8}), "invalid"),
        (json!({"goal": {"reference": {"file": outside}}, "from_beat": 0, "beats": 8}), "outside_root"),
        (json!({"goal": {"loudness": -9}}), "invalid"),
        (json!({"goal": {"loudness": -9}, "change": "x", "from_beat": 0, "beats": 8}), "invalid"),
        (json!({"done": false}), "invalid"),
        (json!({"change": "x", "track": "1:track:0"}), "invalid"),
    ] {
        let answer = lab.call(&JUDGE, lab.on_project(args.clone())).await;
        assert_eq!((answer["state"].clone(), answer["code"].clone()), (json!("refused"), json!(code)), "{args}: {answer}");
    }
    let mut late = lab.on_project(json!({"done": true}));
    late["expected_epoch"] = json!(lab.epoch - 1);
    assert_eq!(lab.call(&JUDGE, late).await["code"], "stale");
    assert!(lab.judge.inputs.borrow().is_empty(), "nothing reached Live");
    assert!(lab.ctx.history().rows(10).is_empty());
});

case!(a_rounds_changes_are_one_change_in_history_and_a_change_it_took_back_is_named, async {
    let lab = Lab::new(DawId::Ableton);
    // The change being judged, made by an earlier kumi_change call.
    let made = record("c7", ChangeState::Applied);
    lab.ctx.history().record(&made, &lab.key, lab.ctx.snapshots());
    let judged_id = lab.ctx.history().rows(1)[0].id.clone();
    lab.judge.writes.set(1);
    *lab.judge.take_back.borrow_mut() = Some(made);
    *lab.judge.answer.borrow_mut() = Ok(json!({"kind": "judged", "kept": false, "next": "harshness"}));
    let answer = lab.call(&JUDGE, lab.on_project(json!({"change": "cut 3 kHz on the hats"}))).await;
    assert_eq!((answer["state"].clone(), answer["round"]["kept"].clone()), (json!("done"), json!(false)), "{answer}");
    assert_eq!(answer["taken_back"], json!([judged_id]));
    let ids: Vec<String> = serde_json::from_value(answer["history_ids"].clone()).unwrap();
    assert_eq!(ids.len(), 1, "the level match the round added: {answer}");
    let row: Row = lab.ctx.history().row(&ids[0]).unwrap();
    assert!(row.change_id.is_some() && row.tool_call_id.as_deref() == Some("call-1") && row.call_outcome.as_deref() == Some("done"));
    assert_eq!(answer["undo_available"], true);

    // tune: its settings are rows of one change; the tool's own schema checks what it's given.
    lab.tune.writes.set(3);
    *lab.tune.answer.borrow_mut() = Ok(json!({"kind": "judged", "kept": true}));
    let epoch = answer["epoch"].clone();
    let tuned = lab
        .call(&TUNE, json!({"device": "1:device:0:1", "how": "search", "knobs": ["Attack", "Release"], "expected_project_key": lab.key, "expected_epoch": epoch}))
        .await;
    assert_eq!(tuned["state"], "done", "{tuned}");
    let ids: Vec<String> = serde_json::from_value(tuned["history_ids"].clone()).unwrap();
    let rows: Vec<Row> = ids.iter().map(|id| lab.ctx.history().row(id).unwrap()).collect();
    assert!(rows.len() == 3 && rows.iter().all(|row| row.change_id == rows[0].change_id), "{rows:?}");
    let input = lab.tune.inputs.borrow()[0].clone();
    assert_eq!(input, object(json!({"device": "1:device:0:1", "how": "search", "knobs": ["Attack", "Release"]})));
    let odd =
        lab.call(&TUNE, json!({"device": "d", "how": "guess", "expected_project_key": lab.key, "expected_epoch": tuned["epoch"]})).await;
    assert_eq!(odd["code"], "invalid", "{odd}");
});

case!(the_tools_refusal_changes_nothing_a_stop_after_a_change_is_unsure_and_other_daws_are_refused, async {
    let lab = Lab::new(DawId::Ableton);
    *lab.tune.answer.borrow_mut() = Err("Start a judged run first (judge with a goal): tune picks numbers against its checklist.".into());
    let refused = lab.call(&TUNE, lab.on_project(json!({"device": "d", "how": "home", "knobs": ["Gain"]}))).await;
    assert_eq!((refused["state"].clone(), refused["code"].clone()), (json!("refused"), json!("refused")));
    assert!(refused["reason"].as_str().unwrap().starts_with("Start a judged run first"));
    assert_eq!(refused["history_ids"], json!([]));
    assert_eq!(refused["epoch"], json!(lab.epoch), "nothing changed: the refs still hold");

    lab.tune.writes.set(1);
    *lab.tune.answer.borrow_mut() = Err("Live stopped answering".into());
    let unsure = lab.call(&TUNE, lab.on_project(json!({"device": "d", "how": "home", "knobs": ["Gain"]}))).await;
    assert_eq!((unsure["state"].clone(), unsure["reason"].clone()), (json!("unsure"), json!("Live stopped answering")));
    assert_eq!(unsure["history_ids"].as_array().unwrap().len(), 2, "its row and the audit row: {unsure}");
    assert!(lab.ctx.link().unsure().is_some());

    let reaper = Lab::new(DawId::Reaper);
    let answer = reaper.call(&JUDGE, reaper.on_project(json!({"done": true}))).await;
    assert_eq!(answer["code"], "unsupported");
    assert!(reaper.judge.inputs.borrow().is_empty());
});

#[test]
fn the_schemas_are_lives_own_with_a_bounded_stretch_and_a_reference_that_is_never_a_path() {
    let judge = judge_schema();
    let reference = &judge["properties"]["goal"]["properties"]["reference"];
    assert!(reference.get("type").is_none() && reference["anyOf"].as_array().unwrap().len() == 2, "{reference}");
    assert_eq!(judge["required"], json!(["expected_project_key", "expected_epoch"]));
    assert_eq!(judge["oneOf"][0]["required"], json!(["goal", "from_beat", "beats"]));
    let native = judge_tool(&DawProfile::ABLETON).schema;
    assert_eq!(judge["properties"]["change"], native["properties"]["change"]);
    let tune = tune_schema();
    assert_eq!(tune["properties"]["how"], tune_tool(&DawProfile::ABLETON).schema["properties"]["how"]);
    assert_eq!(tune["required"], json!(["device", "how", "expected_project_key", "expected_epoch"]));
}
