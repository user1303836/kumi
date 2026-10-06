//! History v0 in Kumi's changes: what a change cuts or deletes is read in Live first (one Python call), kept with the
//! change and in the Set's history.db, and Kumi's undo makes it again inside one Live undo step, after checking it
//! can; a check that fails changes nothing and says why. Without Python in Live, changes stay as they were: Live's own
//! undo takes them back.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{
        contracts::{ChangeState, JsonObject},
        errors::RuntimeError,
    },
    integrations::ableton::{
        changes::CHANGES,
        integration::Ableton,
        options::AbletonOptions,
        project::{create_project_store, ProjectStore},
        remember::CurrentProject,
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

type Reply = Rc<dyn Fn(&str, &Value) -> Value>;

/// A bridge that answers each tool from `reply` (its payload), and Python in Live by the script's ARGS.
struct Bridge {
    tools: Vec<&'static str>,
    calls: RefCell<Vec<(String, Value)>>,
    reply: Reply,
}
#[async_trait(?Send)]
impl McpEndpoint for Bridge {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"bridge","version":"1.0.82"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(
            json!({"tools":self.tools.iter().map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()}),
        )
        .unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        // Python in Live: the snapshot script's ARGS, as the call's args.
        let args = if name == "live_run_python" { script_args(args["code"].as_str().unwrap()) } else { Value::Object(args) };
        self.calls.borrow_mut().push((name.into(), args.clone()));
        let payload = (self.reply)(name, &args);
        let payload = match (name, payload.get("raise")) {
            ("live_run_python", Some(why)) => json!({"ok":false,"result":null,"stdout":"","error":{"type":"ValueError","message":why}}),
            ("live_run_python", None) => json!({"ok":true,"result":payload,"stdout":"","error":null}),
            _ => payload,
        };
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&payload)}],"isError":false})).unwrap())
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}
/// The ARGS a script carries: `ARGS = json.loads("…")`.
fn script_args(code: &str) -> Value {
    let line = code.lines().find(|line| line.starts_with("ARGS = json.loads(")).unwrap();
    let literal = &line["ARGS = json.loads(".len()..line.len() - 1];
    serde_json::from_str(&serde_json::from_str::<String>(literal).unwrap()).unwrap()
}

const TOOLS: &[&str] = &[
    "live_discover",
    "live_run_python",
    "live_undo",
    "live_undo_step_begin",
    "live_undo_step_end",
    "live_clip_delete_preview",
    "live_clip_delete_apply",
    "live_clip_clear_range_preview",
    "live_clip_clear_range_apply",
    "live_arrangement_clip_preview",
    "live_arrangement_clip_apply",
];
const VERSE: &str = "7:arrangement_clip:0:1";
const TRACK: &str = "7:track:0";

/// "Verse", as Kumi keeps it: a MIDI clip at beats 8 to 16 of the first track.
fn verse() -> Value {
    json!({"identity":"live:501","track":"live:100","where":{"start":8.0,"end":16.0},"hash":"verse-hash",
        "leaf":{"kind":"midi","name":"Verse","color":0,"muted":false,"looping":true,"loop":[0.0,8.0],"markers":[0.0,8.0],
            "signature":[4,4],"span":[8.0,16.0],"notes":[[60,0.0,1.0,100.0,false,1.0,0.0,64.0]]}})
}

struct Live {
    integration: Rc<Ableton>,
    bridge: Rc<Bridge>,
}
async fn live(tools: &[&'static str], reply: Reply, projects: Option<&std::path::Path>) -> Live {
    let bridge = Rc::new(Bridge { tools: tools.to_vec(), calls: RefCell::new(vec![]), reply });
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    let out = bridge.clone();
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = out.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    options.change_timeout_ms = Some(500);
    if let Some(projects) = projects {
        let store: Rc<dyn ProjectStore> = create_project_store(projects);
        options.project_store = Some(store);
    }
    let integration = Ableton::new(options);
    let connection = integration.connection.clone();
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    connection.epoch.set(Some(7.0));
    {
        let mut book = connection.references.borrow_mut();
        book.refs.insert(VERSE.into(), "arrangement_clip".into());
        book.refs.insert(TRACK.into(), "track".into());
    }
    *integration.history.remember.current.borrow_mut() = Some(Rc::new(CurrentProject {
        identity: "set-1".into(),
        path: Some("/songs/Night Drive.als".into()),
        name: "Night Drive".into(),
        project: Some("0123456789abcdef0123456789abcdef".into()),
        unsaved: false,
    }));
    Live { integration, bridge }
}
impl Live {
    async fn change(&self, tool: &str, input: Value) -> (String, bool) {
        let kind = CHANGES.iter().find(|kind| kind.tool == tool).unwrap();
        let outcome = self.integration.mutations.change(kind, input.as_object().cloned().unwrap(), Signal::new(), true).await;
        (outcome.text, outcome.is_error)
    }
    fn calls(&self) -> Vec<String> {
        self.bridge
            .calls
            .borrow()
            .iter()
            .map(|(name, args)| match args.get("op").and_then(Value::as_str) {
                Some(op) if name == "live_run_python" => {
                    format!("python {op}{}", if args["check"] == true { " (check)" } else { "" })
                }
                _ => name.clone(),
            })
            .collect()
    }
    fn state(&self) -> ChangeState {
        self.integration.history.entries.borrow().values().last().unwrap().borrow().record.state
    }
}
/// The bridge's answers for a deletion and a cleared range, Live's Python by op: `restore` answers a restore (a
/// payload, or {"raise": why} for Live's refusal).
fn bridge_reply(restore: Rc<dyn Fn(&Value) -> Value>) -> Reply {
    Rc::new(move |name, args| match name {
        "live_clip_delete_preview" => json!({"transactionId":"tx-delete","confirmation":"apply","target":{"ref":VERSE,"name":"Verse"}}),
        "live_clip_delete_apply" => json!({"transactionId":"tx-delete","state":"applied"}),
        "live_clip_clear_range_preview" => json!({"transactionId":"tx-clear","confirmation":"apply","trackRef":TRACK,"trackName":"Keys",
            "fromBeat":10,"toBeat":12,"removes":[],"cuts":[{"ref":VERSE,"name":"Verse","start":8,"end":16}]}),
        "live_clip_clear_range_apply" => json!({"transactionId":"tx-clear","state":"applied","removed":[]}),
        // A new MIDI clip at beats 10 to 12, over the middle of "Verse" (8 to 16).
        "live_discover" => json!({"kind":"arrangement-clip","epoch":7,"truncated":false,"revision":"r1",
            "items":[{"ref":VERSE,"parentRef":TRACK,"trackRef":TRACK,"name":"Verse","start":8,"endTime":16,"length":8,"objectIdentity":"live:501"}]}),
        "live_arrangement_clip_preview" => {
            json!({"transactionId":"tx-create","confirmation":"apply","trackRef":TRACK,"trackName":"Keys","position":10,"length":2,"name":"Fill"})
        }
        "live_arrangement_clip_apply" => {
            json!({"transactionId":"tx-create","state":"applied","result":{"ref":"7:arrangement_clip:0:2","objectIdentity":"live:900","start":10,"length":2}})
        }
        "live_undo" => json!({"transactionId":"tx-create","state":"undone"}),
        "live_undo_step_begin" => json!({"open":true,"stepId":"undo_step_1"}),
        "live_undo_step_end" => json!({"closed":true,"stepId":"undo_step_1"}),
        "live_run_python" => match args["op"].as_str() {
            // What the change will cut, and after a clear, the two pieces Live left of it.
            Some("capture") if args.get("except").is_some() || args["from"] == json!(8.0) => json!({"clips":[
                {"identity":"live:501","track":"live:100","where":{"start":8.0,"end":10.0},"hash":"left-hash","leaf":{"name":"Verse"}},
                {"identity":"live:502","track":"live:100","where":{"start":12.0,"end":16.0},"hash":"right-hash","leaf":{"name":"Verse"}}]}),
            Some("capture") => json!({"clips":[verse()]}),
            Some("restore") => restore(args),
            _ => json!({}),
        },
        _ => json!({}),
    })
}
/// Equal as they'd travel (the bridge writes 8.0 as 8), whatever their keys' order.
fn same(a: &Value, b: &Value) -> bool {
    let plain = |value: &Value| kumi_store::history::canonical(&serde_json::from_str(&stringify(value)).unwrap());
    plain(a) == plain(b)
}
fn made() -> Value {
    json!({"made":[{"identity":"live:777","where":{"start":8.0,"end":16.0},"name":"Verse"}],"partial":[]})
}
/// The calls after the change's apply.
fn after_apply(live: &Live, apply: &str) -> Vec<String> {
    let calls = live.calls();
    calls[calls.iter().position(|c| c == apply).unwrap() + 1..].to_vec()
}

#[tokio::test(flavor = "current_thread")]
async fn a_deleted_clip_is_kept_and_kumis_undo_makes_it_again_in_one_live_step() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let projects = tempfile::tempdir().unwrap();
            let restores = Rc::new(RefCell::new(vec![]));
            let seen = restores.clone();
            let reply = bridge_reply(Rc::new(move |args| {
                seen.borrow_mut().push(args.clone());
                if args["check"] == true {
                    json!({"checked":true})
                } else {
                    made()
                }
            }));
            let live = live(TOOLS, reply, Some(projects.path())).await;
            let (text, is_error) = live.change("delete_clip", json!({"clipRef":VERSE})).await;
            assert!(!is_error, "{text}");
            // Read before it went: the deletion is Kumi's to take back, not Live's alone.
            assert_eq!(live.calls(), ["live_clip_delete_preview", "python capture", "live_clip_delete_apply"]);
            assert_eq!(live.state(), ChangeState::Applied, "{text}");
            let undone = live.integration.history.undo("last", Signal::new(), false).await.unwrap();
            assert!(!undone.is_error, "{}", undone.text);
            assert!(undone.text.contains("\"broughtBack\":\"\u{201c}Verse\u{201d}\""), "{}", undone.text);
            assert_eq!(live.state(), ChangeState::Undone);
            // Checked first, then made again inside one Live undo step.
            assert_eq!(
                after_apply(&live, "live_clip_delete_apply"),
                ["python restore (check)", "live_undo_step_begin", "python restore", "live_undo_step_end"]
            );
            let restore = &restores.borrow()[1];
            assert_eq!((restore["track"].clone(), restore["remnants"].clone()), (json!("live:100"), json!([])));
            assert!(same(&restore["clips"][0]["where"], &json!({"start":8,"end":16})), "{restore}");
            assert!(same(&restore["clips"][0]["leaf"], &verse()["leaf"]), "{restore}");
            // The Set's history keeps the clip, and an op saying what happened to it.
            let path = projects.path().join("0123456789abcdef0123456789abcdef/history.db");
            let mut ops = vec![];
            for _ in 0..300 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                ops = kumi_store::read_only(&path, |c| kumi_store::history::recent_ops(c, 10)).unwrap_or_default();
                if !ops.is_empty() {
                    break;
                }
            }
            assert_eq!(ops.len(), 1, "an op for the deletion");
            assert_eq!(ops[0].kind, "cut");
            let object = ops[0].view["objects"][0].as_str().unwrap().to_owned();
            let (kind, leaf) = kumi_store::read_only(&path, |c| kumi_store::history::object(c, &object)).unwrap().unwrap();
            assert_eq!(kind, "clip");
            assert!(same(&leaf, &verse()["leaf"]), "{leaf}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_undo_that_cant_make_the_clip_again_changes_nothing_and_says_why() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let reply = bridge_reply(Rc::new(|_| json!({"raise":"\u{201c}Fill\u{201d} is in \u{201c}Verse\u{201d}'s place now"})));
            let live = live(TOOLS, reply, None).await;
            live.change("delete_clip", json!({"clipRef":VERSE})).await;
            let undone = live.integration.history.undo("last", Signal::new(), false).await.unwrap();
            assert!(undone.is_error);
            assert_eq!(
                undone.text,
                "Kumi can't bring back \u{201c}Verse\u{201d} (\u{201c}Fill\u{201d} is in \u{201c}Verse\u{201d}'s place now), so it left the change as it is; Live's own undo (Cmd-Z in Live) can take it back."
            );
            assert_eq!(live.state(), ChangeState::Kept);
            // Only the check ran: no undo step, nothing made.
            assert_eq!(after_apply(&live, "live_clip_delete_apply"), ["python restore (check)"]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_cleared_range_puts_back_what_it_cut_after_the_pieces_live_left() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let restores = Rc::new(RefCell::new(vec![]));
            let seen = restores.clone();
            let reply = bridge_reply(Rc::new(move |args| {
                seen.borrow_mut().push(args.clone());
                if args["check"] == true {
                    json!({"checked":true})
                } else {
                    made()
                }
            }));
            let live = live(TOOLS, reply, None).await;
            let (text, is_error) = live.change("clear_range", json!({"trackRef":TRACK,"fromBeat":10,"toBeat":12})).await;
            assert!(!is_error, "{text}");
            assert_eq!(live.calls(), ["live_clip_clear_range_preview", "python capture", "live_clip_clear_range_apply", "python capture"]);
            let reads: Vec<Value> = live.bridge.calls.borrow().iter().filter(|(name, _)| name == "live_run_python").map(|(_, args)| args.clone()).collect();
            assert_eq!(reads[0], json!({"op":"capture","track":TRACK,"from":10,"to":12}));
            // What Live left is read over the whole of what it cut.
            assert!(same(&reads[1], &json!({"track":TRACK,"from":8,"to":16,"except":[],"op":"capture"})), "{}", reads[1]);
            let undone = live.integration.history.undo("last", Signal::new(), false).await.unwrap();
            assert!(!undone.is_error, "{}", undone.text);
            let restore = &restores.borrow()[1];
            assert_eq!(
                restore["remnants"],
                json!([{"identity":"live:501","hash":"left-hash","name":"Verse"},{"identity":"live:502","hash":"right-hash","name":"Verse"}])
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn without_python_in_live_a_deletion_stays_lives_to_take_back() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let tools: Vec<&'static str> = TOOLS.iter().copied().filter(|tool| *tool != "live_run_python").collect();
            let live = live(&tools, bridge_reply(Rc::new(|_| json!({}))), None).await;
            let (text, _) = live.change("delete_clip", json!({"clipRef":VERSE})).await;
            assert_eq!(live.calls(), ["live_clip_delete_preview", "live_clip_delete_apply"]);
            assert_eq!(live.state(), ChangeState::Kept, "{text}");
            let note = live.integration.history.entries.borrow().values().last().unwrap().borrow().record.note.clone().unwrap_or_default();
            assert!(note.contains("Live's own undo can"), "{note}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_clip_laid_over_another_is_taken_back_and_that_one_made_whole() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let restores = Rc::new(RefCell::new(vec![]));
            let seen = restores.clone();
            let reply = bridge_reply(Rc::new(move |args| {
                seen.borrow_mut().push(args.clone());
                if args["check"] == true {
                    json!({"checked":true})
                } else {
                    made()
                }
            }));
            let live = live(TOOLS, reply, None).await;
            let (text, is_error) =
                live.change("add_arrangement_clip", json!({"trackRef":TRACK,"position":10,"length":2,"name":"Fill"})).await;
            assert!(!is_error, "{text}");
            assert_eq!(live.state(), ChangeState::Applied, "{text}");
            let reads: Vec<Value> =
                live.bridge.calls.borrow().iter().filter(|(name, _)| name == "live_run_python").map(|(_, args)| args.clone()).collect();
            // What it lands on, read before; what Live left of it after, the new clip aside.
            assert!(same(&reads[0], &json!({"op":"capture","track":TRACK,"from":10,"to":12})), "{}", reads[0]);
            assert!(same(&reads[1], &json!({"op":"capture","track":TRACK,"from":8,"to":16,"except":["live:900"]})), "{}", reads[1]);
            let undone = live.integration.history.undo("last", Signal::new(), false).await.unwrap();
            assert!(!undone.is_error, "{}", undone.text);
            // After the apply: the track's clips read again (what the new clip cut), and what Live left of "Verse". The
            // undo deletes the new clip (the bridge's undo) and makes "Verse" whole, in one Live undo step; the check
            // before it leaves the new clip out of the way, since the undo deletes it.
            assert_eq!(
                after_apply(&live, "live_arrangement_clip_apply"),
                [
                    "live_discover",
                    "python capture",
                    "python restore (check)",
                    "live_undo_step_begin",
                    "live_undo",
                    "python restore",
                    "live_undo_step_end"
                ]
            );
            assert_eq!(restores.borrow()[0]["leaving"], json!(["live:900"]));
        })
        .await;
}
