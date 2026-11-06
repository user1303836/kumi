//! `variations` (CC-24): the DAW's own variations tool through the runtime. `make` draws numbered takes V1, V2… of a
//! source and writes each as a new clip, item or pattern, never over the source; `change` rewrites named takes; `keep`
//! renames them kept; `list` reads them.
//!
//! What it may do here is narrower than in Kumi's app:
//! - `drop` and `extract` are refused (a take goes back through the undo of its own row), as are `force` and `listen`:
//!   the takes are never rendered here, which also keeps a dispatch's draw the one its preview drew (W1-b).
//! - A make takes at most [`MOST_TAKES`].
//! - The integration's tool has the plugin host's rules (its sets under the runtime's state, W1-e): a preview draws the
//!   takes without writing, and its dispatch (`expect_takes`) writes them only when a draw now comes to the same (W1-a);
//!   a take the producer changed since is never rewritten (24-r1); takes that don't all have an empty place are refused,
//!   never trimmed (24-r2).
//!
//! A write runs in `change`'s call scope: its takes' HISTORY rows are one change, and an outcome Kumi can't confirm is
//! unsure, with its audit row (W1-d).

use super::connection;
use super::ctx::{epoch_of, epoch_schema, Call, Ctx, Effect, JsonObject, OpError, OpFuture, Operation};
use super::history::{CallIdentity, Row};
use kumi_types::contracts::{KernelTool, ToolResult};
use kumi_types::errors::RuntimeError;
use serde_json::{json, Map, Value};

/// The most takes one call makes or names for the plugin.
pub const MOST_TAKES: u64 = 8;

/// The integration's tool, by its name.
const TOOL: &str = "variations";

/// The tool's own arguments the plugin may give.
const TOOL_ARGS: [&str; 13] =
    ["action", "set", "source", "anchors", "place", "count", "takes", "names", "novelty", "minDistance", "seed", "options", "at"];

/// What a dispatch of a make or change must carry.
const NO_PREVIEW: &str = "expect_takes is the takes_digest of this call's preview: preview it first.";

pub static VARIATIONS: Operation = Operation {
    name: "variations",
    description: "Variations of a clip, item or pattern: numbered takes V1, V2… each written as a new one, never over the \
                  source. make (source, count up to 8 or takes: recipes with direction, ops, lines or from), change (names \
                  of takes to rewrite), keep (names; at places one take into an empty place), list. make and change with \
                  `preview` draw the takes without writing (their seed, lines and takes_digest); the dispatch carries that \
                  seed and expect_takes, and is refused when the takes would come out differently. Made on the project key \
                  and epoch an observe gave (but list). Each take written is a HISTORY row of one change.",
    schema: variations_schema,
    effect: variations_effect,
    run: run_variations,
};

fn variations_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["action"], "properties": {
        "action": {"type": "string", "enum": ["make", "change", "keep", "list", "drop", "extract"]},
        "set": {"type": ["integer", "string"], "description": "Which set of takes: its number, or its source's name (the latest by default)"},
        "source": {"type": "object", "description": "What a make varies, by its ref from an observe (Live: clipRef; REAPER: itemRef; Renoise: patternRef and trackRef, phraseRef or instrumentRef)"},
        "anchors": {"type": "array", "maxItems": 512, "items": {"type": "integer", "minimum": 0}},
        "place": {"type": "string", "maxLength": 32},
        "count": {"type": "integer", "minimum": 1, "maximum": MOST_TAKES},
        "takes": {"type": "array", "maxItems": MOST_TAKES, "items": {"type": "object"}},
        "names": {"type": "array", "maxItems": 32, "items": {"type": "string", "maxLength": 32}, "description": "Takes: V3, or rest"},
        "novelty": {"type": "string", "enum": ["subtle", "clear", "bold"]},
        "minDistance": {"type": "number", "minimum": 0, "maximum": 1},
        "seed": {"type": "integer", "minimum": 0, "description": "The seed a preview gave, for its dispatch"},
        "options": {"type": "object"},
        "at": {"type": "object", "description": "keep: where one kept take goes (an empty place only)"},
        "force": {"type": "boolean"},
        "listen": {"type": "boolean"},
        "expected_project_key": {"type": "string", "minLength": 1, "maxLength": 64},
        "expected_epoch": epoch_schema(),
        "preview": {"type": "boolean", "description": "make, change or keep: say what it would do without changing anything (the host's approval)"},
        "expect_takes": {"type": "string", "pattern": "^[0-9a-f]{64}$", "description": "The takes_digest of this call's preview"}
    }})
}

fn action(args: &JsonObject) -> &str {
    args.get("action").and_then(Value::as_str).unwrap_or_default()
}

fn previewing(args: &JsonObject) -> bool {
    args.get("preview").and_then(Value::as_bool).unwrap_or(false)
}

/// What the plugin may not ask of variations, refused with its words before anything runs.
fn not_allowed(args: &JsonObject) -> Option<&'static str> {
    match action(args) {
        "drop" => return Some("Kumi for Hermes doesn't drop takes: undo the ones you don't want with kumi_undo and their HISTORY ids."),
        "extract" => return Some("Kumi for Hermes doesn't extract takes."),
        _ => {}
    }
    if args.get("force") == Some(&Value::Bool(true)) {
        return Some("Kumi for Hermes never overwrites a kept take.");
    }
    if args.get("listen") == Some(&Value::Bool(true)) {
        return Some("Kumi doesn't render takes to hear them here.");
    }
    None
}

/// A list, a preview and a refusal read; a make, change or keep writes.
fn variations_effect(args: &JsonObject) -> Effect {
    if not_allowed(args).is_some() || action(args) == "list" || previewing(args) {
        Effect::Read
    } else {
        Effect::Write
    }
}

fn run_variations(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(variations(ctx, call))
}

/// A call refused before anything was written.
fn refused(code: &str, reason: impl Into<String>) -> Value {
    json!({"state": "refused", "code": code, "reason": reason.into(), "history_ids": []})
}

fn refusal(error: OpError) -> Value {
    refused(error.code(), error.reason())
}

fn project_value(project: &connection::Project) -> Value {
    json!({"key": project.key, "epoch": project.epoch})
}

async fn variations(ctx: &Ctx, call: Call) -> Result<Value, OpError> {
    let args = &call.args;
    if let Some(why) = not_allowed(args) {
        return Ok(refused("not_allowed", why));
    }
    let action = action(args);
    let previewed = previewing(args);
    if let Err(error) = ctx.connection() {
        return Ok(refusal(error));
    }
    let project = if action == "list" {
        let Some(project) = ctx.link().project() else {
            return Ok(refused("not_connected", "Kumi hasn't looked at the project yet: observe first."));
        };
        project
    } else {
        let Some(key) = args.get("expected_project_key").and_then(Value::as_str) else {
            return Ok(refused("invalid", "expected_project_key is the project key an observe gave."));
        };
        let Some(epoch) = epoch_of(args, "expected_epoch") else {
            return Ok(refused("invalid", "expected_epoch is the epoch an observe gave: a whole number, or its digits."));
        };
        match ctx.link().expect(key, epoch) {
            Ok(project) => project,
            Err(error) => return Ok(refusal(error)),
        }
    };
    if matches!(action, "make" | "change") && !previewed && args.get("expect_takes").is_none() {
        return Ok(refused("invalid", NO_PREVIEW));
    }
    let Some(tool) = ctx.link().tools().into_iter().find(|tool| tool.name() == TOOL) else {
        let app = ctx.daw().profile().words.app;
        return Ok(refused("unsupported", format!("{app} offers Kumi no variations here.")));
    };
    // The tool's own arguments, as its schema takes them; never rendered to be heard (W1-b: a render that clipped would
    // draw a take again from another seed, so the takes written would no longer be the ones approved).
    let mut input: Map<String, Value> =
        args.iter().filter(|(key, _)| TOOL_ARGS.contains(&key.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    input.insert("listen".into(), json!(false));
    if let Some(problem) = kumi_agent::kernel::schema::problem(&Value::Object(tool.input_schema()), &Value::Object(input.clone())) {
        return Ok(refused("invalid", problem));
    }
    if previewed {
        input.insert("preview".into(), json!(true));
    }
    if let Some(expected) = args.get("expect_takes") {
        input.insert("expect_takes".into(), expected.clone());
    }
    if action == "list" || previewed {
        let ran = tool.execute(input, call.signal.clone()).await;
        let mut answer = read(ran);
        answer["project"] = project_value(&project);
        return Ok(answer);
    }
    let asked = super::mcp::digest(VARIATIONS.name, args);
    let identity = CallIdentity::of(&call.host, &asked);
    let history = ctx.history();
    history.begin_call();
    let ran = tool.execute(input, call.signal.clone()).await;
    let rows = history.end_call(identity, ctx.snapshots());
    let mut answer = outcome(ran, &rows);
    let state = answer["state"].as_str().unwrap_or("unsure").to_string();
    let mut ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    if state == "unsure" {
        let reason = answer["reason"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| "The DAW didn't confirm every take: look at the project before another change.".into());
        answer["reason"] = json!(reason);
        let title = format!("Variations ({action})");
        ids.push(history.audit(&title, &project.key, &reason, identity, ctx.snapshots()).id);
        ctx.link().note_unsure(VARIATIONS.name, &reason, "unsure");
    }
    // Every take this call wrote, its audit row included, is one change: "undo that" takes a whole make back (W1-d).
    history.one_change(&ids, ctx.snapshots());
    history.call_came_to(&ids, &state, ctx.snapshots());
    // Whatever was written, the refs an earlier look gave may no longer hold: the next call is made on a fresh look.
    if state != "refused" {
        ctx.link().advance();
        connection::publish_focus(&ctx.handle());
    }
    let now = ctx.link().project().unwrap_or(project);
    super::core::publish_status(ctx, false).await;
    answer["history_ids"] = json!(ids);
    answer["undo_available"] = json!(rows.iter().any(|row| row.undo_available));
    answer["project"] = project_value(&now);
    Ok(answer)
}

/// What a list or a preview answered: its own answer (a preview's state, or a refusal: nothing was written either way),
/// or, when the tool couldn't, refused `failed`.
fn read(ran: Result<ToolResult, RuntimeError>) -> Value {
    let said = match ran {
        Ok(result) if !result.is_error => serde_json::from_str::<Value>(&result.text).ok().filter(Value::is_object),
        Ok(result) => return refused("failed", result.text),
        Err(error) => return refused("failed", error.to_string()),
    };
    match said {
        Some(mut said) => {
            if said["state"] == "refused" {
                said["history_ids"] = json!([]);
            }
            connection::names_only_said(&mut said);
            said
        }
        None => refused("failed", "The DAW's variations answered something Kumi can't read."),
    }
}

/// What a make, change or keep came to, with the HISTORY rows it made: `applied` when every take asked for was written,
/// `partial` when some weren't, `refused` when nothing was written, `unsure` when Kumi can't tell what was (an error after
/// rows were made, or a row that's unsure). The tool's own answer, with its state.
fn outcome(ran: Result<ToolResult, RuntimeError>, rows: &[Row]) -> Value {
    let unsure = |reason: String| json!({"state": "unsure", "reason": reason});
    let said = match ran {
        Ok(result) if !result.is_error => serde_json::from_str::<Value>(&result.text).ok().filter(Value::is_object),
        Ok(result) if rows.is_empty() => return refused("failed", result.text),
        Ok(result) => return unsure(result.text),
        Err(error) if rows.is_empty() => return refused("failed", error.to_string()),
        Err(error) => return unsure(format!("The variations stopped: {error}")),
    };
    let Some(mut said) = said else {
        return if rows.is_empty() {
            refused("failed", "The DAW's variations answered something Kumi can't read.")
        } else {
            unsure("The DAW's variations answered something Kumi can't read: look at the project before another change.".into())
        };
    };
    connection::names_only_said(&mut said);
    if said["state"] == "refused" {
        if rows.is_empty() {
            said["history_ids"] = json!([]);
            return said;
        }
        said["state"] = json!("unsure");
        return said;
    }
    if rows.iter().any(|row| row.state == "unsure") {
        said["state"] = json!("unsure");
        return said;
    }
    let left: Vec<String> = ["skipped", "notes"]
        .iter()
        .flat_map(|field| said[*field].as_array().cloned().unwrap_or_default())
        .filter_map(|line| line.as_str().map(str::to_string))
        .collect();
    let state = match (rows.is_empty(), left.is_empty()) {
        (true, false) => {
            said["code"] = json!("failed");
            said["reason"] = json!(left.join("; "));
            "refused"
        }
        (false, false) => "partial",
        _ => "applied",
    };
    said["state"] = json!(state);
    said
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::ctx::{Host, Progress};
    use crate::serve::Args;
    use async_trait::async_trait;
    use kumi_agent::core::contracts::{Integration, Observation};
    use kumi_common::abort::Signal;
    use kumi_types::contracts::{ChangeFamily, ChangeRecord, ChangeState};
    use kumi_types::daw::DawId;
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

    /// The integration's variations tool, as the runtime reaches it: each call's input kept; each write records `writes`
    /// changes as Kumi's integration does (the runtime's HISTORY hears them), in `state`, then answers `answer`.
    struct Takes {
        ctx: RefCell<Weak<Ctx>>,
        inputs: RefCell<Vec<JsonObject>>,
        writes: Cell<usize>,
        state: Cell<ChangeState>,
        answer: RefCell<Result<Value, String>>,
        made: Cell<usize>,
    }

    #[async_trait(?Send)]
    impl KernelTool for Takes {
        fn name(&self) -> &str {
            "variations"
        }
        fn description(&self) -> &str {
            "Variations"
        }
        fn input_schema(&self) -> JsonObject {
            let schema = json!({"type": "object", "additionalProperties": false, "properties": {
                "action": {"type": "string"}, "source": {"type": "object"}, "count": {"type": "integer", "maximum": 16},
                "names": {"type": "array"}, "seed": {"type": "integer"}, "listen": {"type": "boolean"}, "at": {"type": "object"}}});
            schema.as_object().cloned().unwrap()
        }
        async fn execute(&self, input: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
            let reads = input.get("preview") == Some(&json!(true)) || input["action"] == "list";
            self.inputs.borrow_mut().push(input);
            if !reads {
                let ctx = self.ctx.borrow().upgrade().unwrap();
                let key = ctx.link().project().unwrap().key;
                for _ in 0..self.writes.get() {
                    self.made.set(self.made.get() + 1);
                    let record = ChangeRecord {
                        id: format!("c{}", self.made.get()),
                        family: ChangeFamily::Mixer,
                        title: format!("Wrote V{}", self.made.get()),
                        track: None,
                        from: None,
                        to: None,
                        range: None,
                        colors: None,
                        clip: None,
                        devices: None,
                        state: self.state.get(),
                        score: None,
                        note: None,
                        at: 1_700_000_000_000,
                        picture: None,
                        batch: None,
                    };
                    ctx.history().record(&record, &key, ctx.snapshots());
                }
            }
            Ok(match self.answer.borrow().clone() {
                Ok(answer) => ToolResult::text(answer.to_string()),
                Err(why) => ToolResult::error(why),
            })
        }
    }

    struct Lab {
        _dir: tempfile::TempDir,
        ctx: Rc<Ctx>,
        takes: Rc<Takes>,
        key: String,
        epoch: u64,
    }

    impl Lab {
        fn new() -> Lab {
            let dir = tempfile::tempdir().unwrap();
            let args = Args {
                state_dir: dir.path().join("state"),
                kumi_home: dir.path().join("home"),
                hermes_home: None,
                hermes_root: None,
                daw: DawId::Reaper,
                library_roots: vec![],
                media_roots: vec![],
                tools: BTreeMap::new(),
            };
            let ctx = Ctx::new(args, kumi_types::system::Env::new(), kumi_daw::io::Answers::default()).unwrap();
            let takes = Rc::new(Takes {
                ctx: RefCell::new(Rc::downgrade(&ctx)),
                inputs: RefCell::default(),
                writes: Cell::new(0),
                state: Cell::new(ChangeState::Applied),
                answer: RefCell::new(Ok(json!({}))),
                made: Cell::new(0),
            });
            ctx.link().connect_for_test(Rc::new(Daw));
            let project = ctx.link().saw(&Observation {
                key: "reaper:0123".into(),
                revision: None,
                label: "Night Drive".into(),
                context: String::new(),
                instructions: String::new(),
                tools: vec![takes.clone() as Rc<dyn KernelTool>],
                project: None,
                set: Some("1".into()),
                tracks: None,
                saved_at: None,
                identity: None,
                native_epoch: None,
            });
            Lab { _dir: dir, ctx, takes, key: project.key, epoch: project.epoch }
        }

        /// `args`, on the project and epoch the look gave.
        fn on_project(&self, mut args: Value) -> Value {
            args["expected_project_key"] = json!(self.key);
            args["expected_epoch"] = json!(self.epoch);
            args
        }

        async fn call(&self, args: Value) -> Value {
            let Value::Object(args) = args else { panic!("an object") };
            let host = Host { session_id: Some("chat-1".into()), tool_call_id: Some("call-1".into()), ..Host::default() };
            let call = Call { args, host, signal: Signal::new(), progress: Progress::none() };
            (VARIATIONS.run)(&self.ctx, call).await.unwrap()
        }
    }

    fn object(value: Value) -> JsonObject {
        value.as_object().cloned().unwrap()
    }

    macro_rules! case {
        ($name:ident, $body:expr) => {
            #[tokio::test(flavor = "current_thread")]
            async fn $name() {
                tokio::task::LocalSet::new().run_until($body).await;
            }
        };
    }

    case!(drop_extract_force_and_listen_are_refused_with_their_words_and_never_reach_the_daw, async {
        let lab = Lab::new();
        let said = [
            (
                json!({"action": "drop", "names": ["V2"]}),
                "Kumi for Hermes doesn't drop takes: undo the ones you don't want with kumi_undo and their HISTORY ids.",
            ),
            (json!({"action": "extract"}), "Kumi for Hermes doesn't extract takes."),
            (json!({"action": "change", "names": ["V1"], "force": true}), "Kumi for Hermes never overwrites a kept take."),
            (json!({"action": "make", "source": {}, "listen": true}), "Kumi doesn't render takes to hear them here."),
        ];
        for (args, words) in said {
            let args = lab.on_project(args);
            assert_eq!((VARIATIONS.effect)(&object(args.clone())), Effect::Read, "refused without the lease: {args}");
            let answer = lab.call(args).await;
            assert_eq!(answer, json!({"state": "refused", "code": "not_allowed", "reason": words, "history_ids": []}));
        }
        assert!(lab.takes.inputs.borrow().is_empty(), "nothing reached the DAW");
        let schema = variations_schema();
        assert_eq!(
            (schema["properties"]["count"]["maximum"].clone(), schema["properties"]["takes"]["maxItems"].clone()),
            (json!(8), json!(8))
        );
    });

    case!(a_preview_and_a_list_read_and_a_dispatch_carries_its_previews_digest_on_the_project_and_epoch_it_was_made, async {
        let lab = Lab::new();
        let make = json!({"action": "make", "source": {"itemRef": "r:item:{A}"}, "count": 2});
        assert_eq!((VARIATIONS.effect)(&object(json!({"action": "list"}))), Effect::Read);
        assert_eq!((VARIATIONS.effect)(&object(json!({"action": "make", "preview": true}))), Effect::Read);
        assert_eq!((VARIATIONS.effect)(&object(make.clone())), Effect::Write);
        let preview = json!({"state": "preview", "seed": 7, "source": "“Hit” on Foley", "takes": [{"take": "V1", "direction": "higher", "line": "V1 (higher): pitch +2"}],
            "skipped": [], "takes_digest": "a".repeat(64), "fits": 1});
        *lab.takes.answer.borrow_mut() = Ok(preview.clone());
        lab.takes.writes.set(1);
        let mut previewed = make.clone();
        previewed["preview"] = json!(true);
        let answer = lab.call(lab.on_project(previewed)).await;
        let mut expected = preview.clone();
        expected["project"] = json!({"key": lab.key, "epoch": lab.epoch});
        assert_eq!(answer, expected);
        let input = lab.takes.inputs.borrow()[0].clone();
        assert_eq!((input["listen"].clone(), input["preview"].clone()), (json!(false), json!(true)), "never rendered to be heard");
        assert!(!input.contains_key("expected_project_key") && !input.contains_key("expected_epoch"), "{input:?}");
        assert!(lab.ctx.history().rows(10).is_empty(), "a preview writes nothing");

        // A dispatch without its preview's digest, or on an earlier epoch, is refused before the DAW hears of it.
        let bare = lab.call(lab.on_project(make.clone())).await;
        assert_eq!(bare, json!({"state": "refused", "code": "invalid", "reason": NO_PREVIEW, "history_ids": []}));
        let mut late = lab.on_project(make.clone());
        late["expected_epoch"] = json!(lab.epoch - 1);
        late["expect_takes"] = json!("a".repeat(64));
        assert_eq!(lab.call(late).await["code"], "stale");
        let missing = lab.call(json!({"action": "make", "source": {}, "expect_takes": "a".repeat(64)})).await;
        assert_eq!(missing["code"], "invalid", "{missing}");
        assert_eq!(lab.takes.inputs.borrow().len(), 1);
        // (the tool's own schema: what it doesn't take is refused before it runs)
        let mut odd = lab.on_project(json!({"action": "make", "source": {}, "novelty": "bold"}));
        odd["expect_takes"] = json!("a".repeat(64));
        assert_eq!(lab.call(odd).await["code"], "invalid");

        let listed = lab.call(json!({"action": "list"})).await;
        assert_eq!(listed["project"]["key"], json!(lab.key), "{listed}");
        assert_eq!(lab.takes.inputs.borrow().len(), 2);
    });

    case!(a_makes_takes_are_its_rows_of_one_change_and_the_epoch_moves_on, async {
        let lab = Lab::new();
        lab.takes.writes.set(3);
        *lab.takes.answer.borrow_mut() = Ok(json!({"set": 1, "source": "“Hit” on Foley", "skipped": [], "takes": [
            {"take": "V1", "ref": "i1", "line": "V1 (higher): pitch +2 (c1)"},
            {"take": "V2", "ref": "i2", "line": "V2 (lower): pitch −2 (c2)"},
            {"take": "V3", "ref": "i3", "line": "V3 (faster): rate 1.1 (c3)"}]}));
        let digest = "b".repeat(64);
        let answer = lab
            .call(
                lab.on_project(
                    json!({"action": "make", "source": {"itemRef": "r:item:{A}"}, "count": 3, "seed": 7, "expect_takes": digest}),
                ),
            )
            .await;
        assert_eq!(answer["state"], "applied", "{answer}");
        let ids: Vec<String> = serde_json::from_value(answer["history_ids"].clone()).unwrap();
        assert_eq!(ids.len(), 3, "one row a take: {answer}");
        let rows: Vec<Row> = ids.iter().map(|id| lab.ctx.history().row(id).unwrap()).collect();
        let change = rows[0].change_id.clone();
        assert!(change.is_some() && rows.iter().all(|row| row.change_id == change), "one change: {rows:?}");
        assert!(rows.iter().all(|row| row.tool_call_id.as_deref() == Some("call-1") && row.call_outcome.as_deref() == Some("applied")));
        assert_eq!(answer["undo_available"], true);
        assert_eq!(answer["project"], json!({"key": lab.key, "epoch": lab.epoch + 1}), "the refs an earlier look gave no longer hold");
        let input = lab.takes.inputs.borrow()[0].clone();
        assert_eq!(
            (input["expect_takes"].clone(), input["seed"].clone(), input["listen"].clone()),
            (json!(digest), json!(7), json!(false))
        );
        assert!(!input.contains_key("preview"));
    });

    case!(a_take_not_made_is_partial_an_error_after_a_write_is_unsure_and_the_daws_refusal_writes_nothing, async {
        let lab = Lab::new();
        let make = |lab: &Lab| {
            let mut args = lab.on_project(json!({"action": "make", "source": {}, "count": 2}));
            args["expect_takes"] = json!("c".repeat(64));
            args
        };
        // The takes would come out differently now: the tool's refusal, nothing written, the epoch where it was.
        let stale = json!({"state": "refused", "code": "stale", "reason": "The takes would come out differently now: preview them again."});
        *lab.takes.answer.borrow_mut() = Ok(stale.clone());
        let answer = lab.call(make(&lab)).await;
        let mut expected = stale.clone();
        expected["history_ids"] = json!([]);
        expected["undo_available"] = json!(false);
        expected["project"] = json!({"key": lab.key, "epoch": lab.epoch});
        assert_eq!(answer, expected);

        lab.takes.writes.set(1);
        *lab.takes.answer.borrow_mut() =
            Ok(json!({"set": 1, "takes": [{"take": "V1", "ref": "i1", "line": "V1"}], "skipped": ["V2 (lower) wasn't made: too close"]}));
        let partial = lab.call(make(&lab)).await;
        assert_eq!((partial["state"].clone(), partial["history_ids"].as_array().unwrap().len()), (json!("partial"), 1), "{partial}");

        // The DAW stopped answering after a take was written: unsure, with its audit row, in the same change.
        let lab = Lab::new();
        lab.takes.writes.set(1);
        *lab.takes.answer.borrow_mut() = Err("the DAW stopped answering".into());
        let unsure = lab.call(make(&lab)).await;
        assert_eq!((unsure["state"].clone(), unsure["reason"].clone()), (json!("unsure"), json!("the DAW stopped answering")));
        let ids: Vec<String> = serde_json::from_value(unsure["history_ids"].clone()).unwrap();
        assert_eq!(ids.len(), 2, "the take's row and the audit row: {unsure}");
        let rows: Vec<Row> = ids.iter().map(|id| lab.ctx.history().row(id).unwrap()).collect();
        assert!(rows[0].change_id.is_some() && rows[0].change_id == rows[1].change_id);
        assert!(lab.ctx.link().unsure().is_some());
        // An error before anything was written failed: nothing to undo.
        let lab = Lab::new();
        *lab.takes.answer.borrow_mut() = Err("there are no takes yet: make some first".into());
        let failed = lab.call(lab.on_project(json!({"action": "keep", "names": ["V1"]}))).await;
        assert_eq!((failed["state"].clone(), failed["code"].clone()), (json!("refused"), json!("failed")), "{failed}");
        assert_eq!(failed["project"]["epoch"], json!(lab.epoch));
    });

    #[test]
    fn the_plugins_sets_are_kept_under_the_runtimes_state_never_the_kumi_home() {
        let state = std::path::Path::new("/state/kumi");
        let policy = connection::policy(state);
        assert_eq!(policy.variations_home.as_deref(), Some(state));
        assert!(!policy.persist_identity && !policy.menus, "the plugin's policy");
    }
}
