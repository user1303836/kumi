//! History v0 in Kumi's changes: what a change cuts or deletes is read in Live first (one Python call), kept with the
//! change and in the Set's history.db, and Kumi's undo makes it again inside one Live undo step, after checking it
//! can. A check that fails changes nothing and says why (and what clears it); an undo Live stops partway is taken back
//! with one Live undo only when the track shows it's Live's last. Every call fits what python.run takes. Without
//! Python in Live, changes stay as they were: Live's own undo takes them back.
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
        snapshots::{Captured, SetHistory, CODE_LIMIT},
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

type Reply = Rc<dyn Fn(&str, &Value) -> Value>;
/// Live's Python, by the script's ARGS: Some to answer an op, None for the usual answer.
type Python = Rc<dyn Fn(&Value) -> Option<Value>>;

/// A bridge that answers each tool from `reply` (its payload), and Python in Live by the script's ARGS. Like Live's
/// python.run, it takes no more than CODE_LIMIT of code.
struct Bridge {
    tools: Vec<&'static str>,
    calls: RefCell<Vec<(String, Value)>>,
    code: RefCell<Vec<usize>>,
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
        let text = |payload: Value| {
            Ok(serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&payload)}],"isError":false})).unwrap())
        };
        if name == "live_run_python" {
            let code = args["code"].as_str().unwrap();
            self.code.borrow_mut().push(code.len());
            if code.len() > CODE_LIMIT {
                return text(json!({"ok":false,"result":null,"stdout":"","error":{"type":"ValueError",
                    "message":"Python arguments require code, eval/exec mode and timeoutMs from 1 to 30000"}}));
            }
        }
        // Python in Live: the snapshot script's ARGS, as the call's args.
        let args = if name == "live_run_python" { script_args(args["code"].as_str().unwrap()) } else { Value::Object(args) };
        self.calls.borrow_mut().push((name.into(), args.clone()));
        let payload = (self.reply)(name, &args);
        text(match (name, payload.get("raise")) {
            ("live_run_python", Some(why)) => json!({"ok":false,"result":null,"stdout":"","error":{"type":"ValueError","message":why}}),
            ("live_run_python", None) => json!({"ok":true,"result":payload,"stdout":"","error":null}),
            _ => payload,
        })
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
    "live_song_undo",
    "live_undo_step_begin",
    "live_undo_step_end",
    "live_clip_delete_preview",
    "live_clip_delete_apply",
    "live_clip_clear_range_preview",
    "live_clip_clear_range_apply",
    "live_arrangement_clip_preview",
    "live_arrangement_clip_apply",
    "live_clip_move_preview",
    "live_clip_move_apply",
];
const VERSE: &str = "7:arrangement_clip:0:1";
const FILL: &str = "7:arrangement_clip:0:5";
const TRACK: &str = "7:track:0";

/// "Verse", as Kumi keeps it: a MIDI clip at beats 8 to 16 of the first track.
fn verse() -> Value {
    json!({"identity":"live:501","track":"live:100","trackName":"Keys","trackId":"01J9KEYS","where":{"start":8.0,"end":16.0},
        "hash":"verse-hash","notesHash":"verse-notes",
        "leaf":{"kind":"midi","name":"Verse","color":0,"muted":false,"looping":true,"loop":[0.0,8.0],"markers":[0.0,8.0],
            "signature":[4,4],"span":[8.0,16.0],"notes":[[60,0.0,1.0,100.0,false,1.0,0.0,64.0]]}})
}
/// What the track holds, as the state op says it.
fn held(name: &str) -> Value {
    json!({"clips":[[name,8.0,16.0,"digest"]],"slots":[]})
}

struct Live {
    integration: Rc<Ableton>,
    bridge: Rc<Bridge>,
}
async fn live(tools: &[&'static str], reply: Reply, projects: Option<&std::path::Path>) -> Live {
    let bridge = Rc::new(Bridge { tools: tools.to_vec(), calls: RefCell::new(vec![]), code: RefCell::new(vec![]), reply });
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
        book.refs.insert(FILL.into(), "arrangement_clip".into());
        book.refs.insert(TRACK.into(), "track".into());
    }
    *integration.history.remember.current.borrow_mut() = Some(Rc::new(project()));
    Live { integration, bridge }
}
fn project() -> CurrentProject {
    CurrentProject {
        identity: "set-1".into(),
        path: Some("/songs/Night Drive.als".into()),
        name: "Night Drive".into(),
        project: Some("0123456789abcdef0123456789abcdef".into()),
        unsaved: false,
    }
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
    fn python(&self, op: &str) -> Vec<Value> {
        self.bridge
            .calls
            .borrow()
            .iter()
            .filter(|(name, args)| name == "live_run_python" && args["op"] == op)
            .map(|(_, args)| args.clone())
            .collect()
    }
    fn state(&self) -> ChangeState {
        self.integration.history.entries.borrow().values().last().unwrap().borrow().record.state
    }
    async fn undo(&self) -> (String, bool) {
        let undone = self.integration.history.undo("last", Signal::new(), false).await.unwrap();
        (undone.text, undone.is_error)
    }
}
/// The bridge's answers for a deletion, a cleared range, a new clip over another and a move; Live's Python through
/// `python` first (the made clips, a state, all notes in, by default).
fn bridge_reply(python: Python) -> Reply {
    Rc::new(move |name, args| {
        match name {
        "live_clip_delete_preview" => json!({"transactionId":"tx-delete","confirmation":"apply","target":{"ref":VERSE,"name":"Verse"}}),
        "live_clip_delete_apply" => {
            json!({"transactionId":"tx-delete","state":"applied","deleted":VERSE,"kept":"Kumi can't bring this back; Live's undo can."})
        }
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
        // "Fill" moved to beats 10 to 12 on its own track, over the middle of "Verse"; a copy says so in its payload.
        "live_clip_move_preview" => {
            let mut payload = json!({"ref":FILL,"position":10,"expectedObjectIdentity":"live:600"});
            if args["keepSource"] == true {
                payload["keepSource"] = json!(true);
            }
            json!({"transactionId":"tx-move","confirmation":"apply","impact":"moves-clip-replacing","payload":payload,
                "replaces":[{"name":"Verse","start":10,"end":12}],"kept":"Kumi can't bring back what the move replaces; Live's undo can."})
        }
        "live_clip_move_apply" => {
            json!({"transactionId":"tx-move","state":"applied","created":{"ref":"7:arrangement_clip:0:3","objectIdentity":"live:601"}})
        }
        "live_undo" => json!({"transactionId":"tx-create","state":"undone"}),
        "live_song_undo" => json!({"done":true}),
        "live_undo_step_begin" => json!({"open":true,"stepId":"undo_step_1"}),
        "live_undo_step_end" => json!({"closed":true,"stepId":"undo_step_1"}),
        "live_run_python" => python(args).unwrap_or_else(|| match args["op"].as_str() {
            // What the change will cut, and after it (read over the whole of "Verse"), the two pieces Live left of it.
            Some("capture") if args["from"].as_f64() == Some(8.0) => json!({"clips":[
                {"identity":"live:501","track":"live:100","trackName":"Keys","where":{"start":8.0,"end":10.0},"hash":"left-hash","leaf":{"name":"Verse"}},
                {"identity":"live:502","track":"live:100","trackName":"Keys","where":{"start":12.0,"end":16.0},"hash":"right-hash","leaf":{"name":"Verse"}}]}),
            Some("capture") => json!({"clips":[verse()]}),
            Some("restore") if args["check"] == true => json!({"checked":true}),
            Some("restore") => made_of(args),
            Some("state") => held("Verse"),
            Some("notes") => json!({"added":args["notes"].as_array().map_or(0, Vec::len),"exact":true}),
            _ => json!({}),
        }),
        _ => json!({}),
    }
    })
}
/// A restore's answer: every clip it was given, made.
fn made_of(args: &Value) -> Value {
    let made: Vec<Value> = args["clips"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(n, clip)| json!({"identity":format!("live:{}", 700 + n),"where":clip["where"],"name":clip["leaf"]["name"]}))
        .collect();
    let removed: Vec<Value> = args["remnants"].as_array().into_iter().flatten().map(|r| r["name"].clone()).collect();
    json!({"removed":removed,"made":made,"partial":[]})
}
/// Equal as they'd travel (the bridge writes 8.0 as 8), whatever their keys' order.
fn same(a: &Value, b: &Value) -> bool {
    let plain = |value: &Value| kumi_store::history::canonical(&serde_json::from_str(&stringify(value)).unwrap());
    plain(a) == plain(b)
}
/// The calls after the change's apply.
fn after_apply(live: &Live, apply: &str) -> Vec<String> {
    let calls = live.calls();
    calls[calls.iter().position(|c| c == apply).unwrap() + 1..].to_vec()
}
/// What Kumi's undo does when it goes as planned, after the change's apply.
const UNDO: &[&str] = &["python restore (check)", "python state", "live_undo_step_begin", "python restore", "live_undo_step_end"];

#[tokio::test(flavor = "current_thread")]
async fn a_deleted_clip_is_kept_and_kumis_undo_makes_it_again_in_one_live_step() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let projects = tempfile::tempdir().unwrap();
            let live = live(TOOLS, bridge_reply(Rc::new(|_| None)), Some(projects.path())).await;
            let (text, is_error) = live.change("delete_clip", json!({"clipRef":VERSE})).await;
            assert!(!is_error, "{text}");
            // Read before it went: the deletion is Kumi's to take back, not Live's alone, and its answer says so.
            assert_eq!(live.calls(), ["live_clip_delete_preview", "python capture", "live_clip_delete_apply"]);
            assert_eq!(live.state(), ChangeState::Applied, "{text}");
            let answer: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(answer["live"]["kept"], "Kumi's undo brings back \u{201c}Verse\u{201d}.", "{text}");
            let (text, is_error) = live.undo().await;
            assert!(!is_error, "{text}");
            assert!(text.contains("\"broughtBack\":\"\u{201c}Verse\u{201d}\""), "{text}");
            assert_eq!(live.state(), ChangeState::Undone);
            // Checked first, what the track holds read, then made again inside one Live undo step.
            assert_eq!(after_apply(&live, "live_clip_delete_apply"), UNDO);
            let restore = &live.python("restore")[1];
            assert_eq!(
                (restore["track"].clone(), restore["trackName"].clone(), restore["trackId"].clone(), restore["remnants"].clone()),
                (json!("live:100"), json!("Keys"), json!("01J9KEYS"), json!([]))
            );
            assert!(same(&restore["clips"][0]["where"], &json!({"start":8,"end":16})), "{restore}");
            assert!(same(&restore["clips"][0]["leaf"], &verse()["leaf"]), "{restore}");
            // The check reads neither notes nor settings.
            assert_eq!(live.python("restore")[0]["clips"][0]["leaf"], json!({"kind":"midi","name":"Verse"}));
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
            // A session that no longer holds it reads it back from there.
            let store: Rc<dyn ProjectStore> = create_project_store(projects.path());
            let again = SetHistory::default().kept_leaf(&object, Some(&store), Some(&project())).await.unwrap();
            assert!(same(&again, &verse()["leaf"]), "{again}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_clip_in_the_way_leaves_the_change_for_kumis_undo_once_its_moved() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let refusal = Rc::new(RefCell::new(json!("\u{201c}Fill\u{201d} is in \u{201c}Verse\u{201d}'s place now")));
            let said = refusal.clone();
            let python: Python = Rc::new(move |args| (args["check"] == true).then(|| json!({"raise":*said.borrow()})));
            let live = live(TOOLS, bridge_reply(python), None).await;
            live.change("delete_clip", json!({"clipRef":VERSE})).await;
            let (text, is_error) = live.undo().await;
            assert!(is_error);
            assert_eq!(
                text,
                "Kumi can't bring back \u{201c}Verse\u{201d} yet: \u{201c}Fill\u{201d} is in \u{201c}Verse\u{201d}'s place now. Move or delete \u{201c}Fill\u{201d}, then undo again."
            );
            // Nothing changed, and the change is still Kumi's to undo: only the check ran.
            assert_eq!(live.state(), ChangeState::Applied);
            assert_eq!(after_apply(&live, "live_clip_delete_apply"), ["python restore (check)"]);
            // What the producer can't clear (the piece Live left, edited since) leaves it to Live's undo.
            *refusal.borrow_mut() = json!("\u{201c}Verse\u{201d} changed in Live since");
            let (text, _) = live.undo().await;
            assert_eq!(
                text,
                "Kumi can't bring back \u{201c}Verse\u{201d}: \u{201c}Verse\u{201d} changed in Live since. It left the change as it is; Live's own undo (Cmd-Z in Live) can take it back."
            );
            assert_eq!(live.state(), ChangeState::Kept);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_cleared_range_puts_back_what_it_cut_after_the_pieces_live_left() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let live = live(TOOLS, bridge_reply(Rc::new(|_| None)), None).await;
            let (text, is_error) = live.change("clear_range", json!({"trackRef":TRACK,"fromBeat":10,"toBeat":12})).await;
            assert!(!is_error, "{text}");
            assert_eq!(live.calls(), ["live_clip_clear_range_preview", "python capture", "live_clip_clear_range_apply", "python capture"]);
            let reads = live.python("capture");
            assert_eq!(reads[0], json!({"op":"capture","track":TRACK,"from":10,"to":12}));
            // What Live left is read over the whole of what it cut.
            assert!(same(&reads[1], &json!({"track":TRACK,"from":8,"to":16,"except":[],"op":"capture"})), "{}", reads[1]);
            let (text, is_error) = live.undo().await;
            assert!(!is_error, "{text}");
            assert_eq!(
                live.python("restore")[1]["remnants"],
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
            let live = live(&tools, bridge_reply(Rc::new(|_| None)), None).await;
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
            let live = live(TOOLS, bridge_reply(Rc::new(|_| None)), None).await;
            let (text, is_error) =
                live.change("add_arrangement_clip", json!({"trackRef":TRACK,"position":10,"length":2,"name":"Fill"})).await;
            assert!(!is_error, "{text}");
            assert_eq!(live.state(), ChangeState::Applied, "{text}");
            let reads = live.python("capture");
            // What it lands on, read before; what Live left of it after, the new clip aside.
            assert!(same(&reads[0], &json!({"op":"capture","track":TRACK,"from":10,"to":12})), "{}", reads[0]);
            assert!(same(&reads[1], &json!({"op":"capture","track":TRACK,"from":8,"to":16,"except":["live:900"]})), "{}", reads[1]);
            let (text, is_error) = live.undo().await;
            assert!(!is_error, "{text}");
            // After the apply: the track's clips read again (what the new clip cut), and what Live left of "Verse". The
            // undo deletes the new clip (the bridge's undo) and makes "Verse" whole, in one Live undo step; the check
            // before it leaves the new clip out of the way, since the undo deletes it.
            assert_eq!(
                after_apply(&live, "live_arrangement_clip_apply"),
                [
                    "live_discover",
                    "python capture",
                    "python restore (check)",
                    "python state",
                    "live_undo_step_begin",
                    "live_undo",
                    "python restore",
                    "live_undo_step_end"
                ]
            );
            assert_eq!(live.python("restore")[0]["leaving"], json!(["live:900"]));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_move_that_replaces_part_of_a_clip_is_moved_back_and_that_clip_made_whole() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let live = live(TOOLS, bridge_reply(Rc::new(|_| None)), None).await;
            // keepSource false is a move, not a copy: Kumi's to undo.
            let (text, is_error) = live.change("move_clip", json!({"clipRef":FILL,"position":10,"keepSource":false})).await;
            assert!(!is_error, "{text}");
            assert_eq!(live.state(), ChangeState::Applied, "{text}");
            let answer: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(answer["live"]["kept"], "Kumi's undo brings back \u{201c}Verse\u{201d}.", "{text}");
            let reads = live.python("capture");
            // The span the preview says it replaces, the moving clip aside; after, what Live left, found by the moved
            // clip's ref and leaving it out.
            assert!(same(&reads[0], &json!({"op":"capture","track":FILL,"from":10,"to":12,"except":["live:600"]})), "{}", reads[0]);
            assert!(
                same(&reads[1], &json!({"op":"capture","track":"7:arrangement_clip:0:3","from":8,"to":16,"except":["live:601"]})),
                "{}",
                reads[1]
            );
            let (text, is_error) = live.undo().await;
            assert!(!is_error, "{text}");
            assert_eq!(live.state(), ChangeState::Undone);
            // The move goes back through the bridge, then "Verse" is made whole, in one Live undo step.
            assert_eq!(
                after_apply(&live, "live_clip_move_apply"),
                [
                    "python capture",
                    "python restore (check)",
                    "python state",
                    "live_undo_step_begin",
                    "live_undo",
                    "python restore",
                    "live_undo_step_end"
                ]
            );
            assert_eq!(live.python("restore")[0]["leaving"], json!(["live:601"]));
            // A copy lands where its source isn't: Live's to undo, read nothing.
            let reads_before = live.python("capture").len();
            live.change("move_clip", json!({"clipRef":FILL,"position":10,"keepSource":true})).await;
            assert_eq!(live.python("capture").len(), reads_before);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_undo_live_stops_partway_is_taken_back_with_one_live_undo_right_after_its_step() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Kumi's answer says it removed a piece before Live stopped: its step changed the Set.
            let python: Python = Rc::new(move |args| match args["op"].as_str() {
                Some("restore") if args["check"] != true => {
                    Some(json!({"removed":["Verse"],"made":[],"partial":[],"error":"RuntimeError: Live refused"}))
                }
                _ => None,
            });
            let live = live(TOOLS, bridge_reply(python), None).await;
            live.change("delete_clip", json!({"clipRef":VERSE})).await;
            let (text, is_error) = live.undo().await;
            assert!(is_error);
            assert_eq!(
                text,
                "Kumi's undo didn't finish (Live refused), so it took back what it did: the change is as it was. Undo again, or Live's own undo (Cmd-Z in Live) can take it back."
            );
            assert_eq!(live.state(), ChangeState::Applied);
            // Live's undo once, right after the step closed, then the track read: back as it was.
            let tail = after_apply(&live, "live_clip_delete_apply");
            assert_eq!(tail[tail.len() - 3..], ["live_undo_step_end", "live_song_undo", "python state"]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_undo_live_stops_partway_is_left_to_the_producer_when_kumi_cant_tell() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // What the track held can't be read, so Kumi can't tell its step is Live's last: Live's undo is left alone.
            let python: Python = Rc::new(|args| match args["op"].as_str() {
                Some("restore") if args["check"] != true => {
                    Some(json!({"removed":["Verse"],"made":[],"partial":[],"error":"TimeoutError: python.run"}))
                }
                Some("state") => Some(json!({"raise":"RuntimeError: can't read"})),
                _ => None,
            });
            let one = live(TOOLS, bridge_reply(python), None).await;
            one.change("delete_clip", json!({"clipRef":VERSE})).await;
            let (text, is_error) = one.undo().await;
            assert!(is_error);
            assert_eq!(text, "Kumi's undo didn't finish (Live took too long). Cmd-Z in Live once puts back what it changed.");
            assert_eq!(one.state(), ChangeState::Unsure);
            assert!(!one.calls().contains(&"live_song_undo".to_owned()));
            // Read before, but not after a step that says it changed nothing: Kumi can't tell, so it's left alone too.
            let reads = Rc::new(Cell::new(0));
            let count = reads.clone();
            let python: Python = Rc::new(move |args| match args["op"].as_str() {
                Some("restore") if args["check"] != true => {
                    Some(json!({"removed":[],"made":[],"partial":[],"error":"TimeoutError: python.run"}))
                }
                Some("state") => {
                    count.set(count.get() + 1);
                    Some(if count.get() == 1 { held("Fill") } else { json!({"raise":"RuntimeError: can't read"}) })
                }
                _ => None,
            });
            let unread = live(TOOLS, bridge_reply(python), None).await;
            unread.change("delete_clip", json!({"clipRef":VERSE})).await;
            let (text, is_error) = unread.undo().await;
            assert!(is_error);
            assert_eq!(text, "Kumi's undo didn't finish (Live took too long). Cmd-Z in Live once puts back what it changed.");
            assert_eq!(unread.state(), ChangeState::Unsure);
            assert_eq!(reads.get(), 2, "read before, and once after");
            assert!(!unread.calls().contains(&"live_song_undo".to_owned()), "Live's undo isn't pressed blind");
            // Live's undo once, but not back as it was: the producer is told to check Live.
            let reads = Rc::new(Cell::new(0));
            let count = reads.clone();
            let python: Python = Rc::new(move |args| match args["op"].as_str() {
                Some("restore") if args["check"] != true => {
                    Some(json!({"removed":["Verse"],"made":[],"partial":[],"error":"RuntimeError: Live refused"}))
                }
                Some("state") => {
                    count.set(count.get() + 1);
                    Some(held(&count.get().to_string()))
                }
                _ => None,
            });
            let two = live(TOOLS, bridge_reply(python), None).await;
            two.change("delete_clip", json!({"clipRef":VERSE})).await;
            let (text, _) = two.undo().await;
            assert!(
                text.ends_with(
                    "something else may have changed in Live meanwhile. Check Live, where Cmd-Z and Shift-Cmd-Z step through it."
                ),
                "{text}"
            );
            assert_eq!(two.state(), ChangeState::Unsure);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_big_clear_goes_back_in_calls_python_run_takes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Forty clips of 300 notes each: more than one call takes.
            let clips: Vec<Value> = (0..40)
                .map(|n| {
                    let mut clip = verse();
                    clip["identity"] = json!(format!("live:{}", 1000 + n));
                    clip["where"] = json!({"start":(n * 4) as f64,"end":(n * 4 + 4) as f64});
                    clip["leaf"]["name"] = json!(format!("Clip {n}"));
                    clip["leaf"]["notes"] = json!((0..300)
                        .map(|i| json!([36 + i % 48, i as f64 * 0.0125, 0.25, 100.0, false, 1.0, 0.0, 64.0]))
                        .collect::<Vec<_>>());
                    clip
                })
                .collect();
            let python: Python = Rc::new(move |args| match args["op"].as_str() {
                Some("capture") if args.get("except").is_some() => Some(json!({"clips":[]})),
                Some("capture") => Some(json!({"clips":clips})),
                _ => None,
            });
            let live = live(TOOLS, bridge_reply(python), None).await;
            let (text, is_error) = live.change("clear_range", json!({"trackRef":TRACK,"fromBeat":0,"toBeat":160})).await;
            assert!(!is_error, "{text}");
            let (text, is_error) = live.undo().await;
            assert!(!is_error, "{text}");
            let restores: Vec<Value> = live.python("restore").into_iter().filter(|args| args["check"] != true).collect();
            assert!(restores.len() > 1, "{}", restores.len());
            assert_eq!(restores.iter().map(|args| args["clips"].as_array().unwrap().len()).sum::<usize>(), 40);
            assert!(live.bridge.code.borrow().iter().all(|size| *size <= CODE_LIMIT), "{:?}", live.bridge.code.borrow());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_big_clips_notes_follow_in_more_calls_and_the_last_checks_them() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut big = verse();
            big["leaf"]["notes"] =
                json!((0..6000).map(|i| json!([36 + i % 48, i as f64 * 0.0125, 0.25, 100.0, false, 1.0, 0.0, 64.0])).collect::<Vec<_>>());
            let python: Python =
                Rc::new(move |args| (args["op"] == "capture" && args.get("clips").is_some()).then(|| json!({"clips":[big]})));
            let live = live(TOOLS, bridge_reply(python), None).await;
            live.change("delete_clip", json!({"clipRef":VERSE})).await;
            let (text, is_error) = live.undo().await;
            assert!(!is_error, "{text}");
            let first = live.python("restore").into_iter().find(|args| args["check"] != true).unwrap();
            assert_eq!(first["clips"][0]["notesToCome"], true);
            let notes = live.python("notes");
            assert!(notes.len() > 1, "{}", notes.len());
            let sent = first["clips"][0]["leaf"]["notes"].as_array().unwrap().len()
                + notes.iter().map(|args| args["notes"].as_array().unwrap().len()).sum::<usize>();
            assert_eq!(sent, 6000);
            // Only the last one checks the clip holds them all.
            assert!(notes[..notes.len() - 1].iter().all(|args| args["notesHash"].is_null()));
            assert_eq!(notes.last().unwrap()["notesHash"], "verse-notes");
            assert!(live.bridge.code.borrow().iter().all(|size| *size <= CODE_LIMIT));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn what_kumis_undo_wont_bring_back_is_said_before_and_after() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let take: Value = json!({"identity":"live:501","track":"live:100","trackName":"Vox","where":{"start":8.0,"end":16.0},"hash":"take-hash",
                "leaf":{"kind":"audio","name":"Take","file":"/samples/take.wav","partial":["fades"]}});
            let python: Python = Rc::new(move |args| match args["op"].as_str() {
                Some("capture") if args.get("clips").is_some() => Some(json!({"clips":[take]})),
                Some("restore") if args["check"] != true => Some(json!({"removed":[],"made":[{"identity":"live:700","name":"Take"}],
                    "partial":[{"name":"Take","missing":["its fades"]}]})),
                _ => None,
            });
            let live = live(TOOLS, bridge_reply(python), None).await;
            let (text, _) = live.change("delete_clip", json!({"clipRef":VERSE})).await;
            let answer: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                answer["live"]["kept"],
                "Kumi's undo brings back \u{201c}Take\u{201d}, \u{201c}Take\u{201d} without its fades (Live doesn't give Kumi those). Live's own undo (Cmd-Z in Live), right away, brings back all of it."
            );
            let (text, is_error) = live.undo().await;
            assert!(!is_error, "{text}");
            let undone: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                undone["notExactly"],
                "\u{201c}Take\u{201d} without its fades: Live doesn't give Kumi those, or take them back. Cmd-Z in Live twice, right away, takes this undo back and then the change, bringing all of it back."
            );
        })
        .await;
}

#[test]
fn a_clip_too_big_for_one_call_stays_lives_to_undo() {
    let mut big: Captured = serde_json::from_value(verse()).unwrap();
    assert!(big.fits());
    // Its notes can follow in more calls; settings this big can't.
    big.leaf["notes"] = json!(vec![json!([60, 0.0, 1.0, 100.0, false, 1.0, 0.0, 64.0]); 20_000]);
    assert!(big.fits());
    big.leaf["warpMarkers"] = json!(vec![json!([1.0, 0.5]); 10_000]);
    assert!(!big.fits());
}

#[tokio::test(flavor = "current_thread")]
async fn what_an_unsaved_set_kept_is_written_once_its_saved_and_its_ops_carry_on() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let projects = tempfile::tempdir().unwrap();
            let store: Rc<dyn ProjectStore> = create_project_store(projects.path());
            let history = SetHistory::default();
            let clip: Captured = serde_json::from_value(verse()).unwrap();
            let unsaved = CurrentProject { path: None, project: None, unsaved: true, ..project() };
            history.keep(Some(&store), Some(&unsaved), &[clip.clone()], "Deleted Verse", json!({}), 1);
            // The producer saves: the next observation finds the project id.
            history.identified(Some(&store), Some(&project()));
            history.keep(Some(&store), Some(&project()), &[clip], "Deleted Verse again", json!({}), 2);
            let path = projects.path().join("0123456789abcdef0123456789abcdef/history.db");
            let mut ops = vec![];
            for _ in 0..300 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                ops = kumi_store::read_only(&path, |c| kumi_store::history::recent_ops(c, 10)).unwrap_or_default();
                if ops.len() == 2 {
                    break;
                }
            }
            assert_eq!(ops.len(), 2, "both ops written under the project");
            assert_eq!(ops[1].parent, None);
            assert_eq!(ops[0].parent.as_deref(), Some(ops[1].id.as_str()), "the saved Set's op follows the unsaved one's");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_undo_live_stops_without_saying_what_it_did_is_left_alone_when_nothing_changed() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Live stopped the call with no answer of Kumi's: the track, read again, is as it was.
            let python: Python =
                Rc::new(|args| (args["op"] == "restore" && args["check"] != true).then(|| json!({"raise":"RuntimeError: boom"})));
            let live = live(TOOLS, bridge_reply(python), None).await;
            live.change("delete_clip", json!({"clipRef":VERSE})).await;
            let (text, _) = live.undo().await;
            assert_eq!(
                text,
                "Kumi couldn't bring back \u{201c}Verse\u{201d} (Live said: boom); nothing changed. Undo again, or Live's own undo (Cmd-Z in Live) can take it back."
            );
            assert_eq!(live.state(), ChangeState::Applied);
            let tail = after_apply(&live, "live_clip_delete_apply");
            assert_eq!(tail[tail.len() - 2..], ["live_undo_step_end", "python state"]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_clip_live_cant_make_at_its_length_leaves_the_change_to_lives_undo() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Drawn out past its file, seen at capture: the change stays Live's to undo, as before.
            let mut long = verse();
            long["overFile"] = json!(true);
            let python: Python = Rc::new(move |args| (args["op"] == "capture").then(|| json!({"clips":[long]})));
            let one = live(TOOLS, bridge_reply(python), None).await;
            let (text, _) = one.change("delete_clip", json!({"clipRef":VERSE})).await;
            assert_eq!(one.state(), ChangeState::Kept, "{text}");
            let answer: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(answer["live"]["kept"], "Kumi can't bring this back; Live's undo can.");
            // Found only when the stand-in comes up short: refused before anything in the Set changed.
            let python: Python = Rc::new(|args| {
                (args["op"] == "restore" && args["check"] != true)
                    .then(|| json!({"raise":"Live can't make \u{201c}Verse\u{201d} again at its length: the clip is longer than its file"}))
            });
            let two = live(TOOLS, bridge_reply(python), None).await;
            two.change("delete_clip", json!({"clipRef":VERSE})).await;
            let (text, is_error) = two.undo().await;
            assert!(is_error);
            assert_eq!(
                text,
                "Kumi can't bring back \u{201c}Verse\u{201d}: Live can't make \u{201c}Verse\u{201d} again at its length: the clip is longer than its file. It left the change as it is; Live's own undo (Cmd-Z in Live) can take it back, one press later than before: Kumi's try left an empty step on top of Live's undo history."
            );
            assert_eq!(two.state(), ChangeState::Kept);
            assert!(!two.calls().contains(&"live_song_undo".to_owned()), "Kumi's empty step stays in Live's history");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_renamed_track_says_how_to_clear_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let python: Python = Rc::new(|args| {
                (args["check"] == true).then(|| json!({"raise":"\u{201c}Pad\u{201d} isn't the track it was (renamed, or another in its place)"}))
            });
            let live = live(TOOLS, bridge_reply(python), None).await;
            live.change("delete_clip", json!({"clipRef":VERSE})).await;
            let (text, _) = live.undo().await;
            assert_eq!(
                text,
                "Kumi can't bring back \u{201c}Verse\u{201d} yet: \u{201c}Pad\u{201d} isn't the track it was (renamed, or another in its place). If it's \u{201c}Keys\u{201d} renamed, name it \u{201c}Keys\u{201d} again, then undo again; if another track took its place, Live's own undo (Cmd-Z in Live) can take the change back."
            );
            assert_eq!(live.state(), ChangeState::Applied);
        })
        .await;
}
