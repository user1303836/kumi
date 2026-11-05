//! `read` (C1): one of the DAW's own reads, bounded, or the whole input of a change step a plugin may use or of a
//! reader.
//!
//! Only the DAW's readers run: REAPER's and Renoise's own, Live's note reader and the reads its bridge offers a model.
//! Nothing else the integration offers does (making a device, finding sounds on disk), and a read that names an API
//! function (`fn`, `args`) is refused (C2: no code). A read runs only on the project the last look saw: the DAW
//! answering, and no return since (an own bridge's tab switch to another project is one). What it gives of the
//! producer's files is their names, never their folders. The steps are the DAW's plugin allowlist's, each schema as a
//! plugin's plan fills it; a step that places a file isn't one until the import step (CC-11) is in the runtime.

use super::change::{offered_steps, source_schema};
use super::connection::{names_only, names_only_text};
use super::ctx::{Call, Ctx, OpError, OpFuture, Operation};
use kumi_daw::plugin::{plugin_schema, PluginStep};
use kumi_types::contracts::{KernelTool, Reach, ToolRole};
use serde_json::{json, Map, Value};
use std::rc::Rc;

pub static READ: Operation = Operation {
    name: "read",
    description: "Reads the DAW, changing nothing. `data`: one of its readers (`tool`), with `query` its input as \
                  its schema says: rows with their refs, and a cursor for the next page. `schema`: the whole input of a \
                  change step or a reader (`tool`), or, without one, the steps a change may use and the readers. \
                  Observe first: the readers come with the look.",
    schema: read_schema,
    effect: super::ctx::reads,
    run: run_read,
};

/// The most of an answer's text: by default, the least asked for, and the most.
const MAX_CHARS: (u64, u64, u64) = (8000, 500, 20000);

fn read_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["action"], "properties": {
        "action": {"type": "string", "enum": ["data", "schema"]},
        "tool": {"type": "string", "minLength": 1, "maxLength": 64, "description": "data: the reader; schema: the step or reader"},
        "query": {"type": "object", "description": "data: the reader's input"},
        "max_chars": {"type": "integer", "minimum": MAX_CHARS.1, "maximum": MAX_CHARS.2, "description": "The most of the answer's text (8000 by default)"}
    }})
}

fn run_read(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(read(ctx, call))
}

/// One of the DAW's readers: REAPER's and Renoise's own, Live's note reader, and the reads Live's bridge offers a model.
/// (A DAW's reader joins this list with its integration.) It must also say it only reads the DAW.
fn reader(tool: &dyn KernelTool) -> bool {
    let name = tool.name();
    tool.reach() == Reach::DawRead
        && (matches!(name, "reaper_read" | "renoise_read" | "read_notes") || kumi_agent::mcp::allowed_tools::MODEL_TOOLS.contains(&name))
}

/// A field of a reader's input that names an API function to call, wherever it is.
fn names_a_function(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.iter().any(|(field, inner)| matches!(field.as_str(), "fn" | "args") || names_a_function(inner)),
        Value::Array(items) => items.iter().any(names_a_function),
        _ => false,
    }
}

async fn read(ctx: &Ctx, call: Call) -> Result<Value, OpError> {
    ctx.connection()?;
    let Some(last) = ctx.link().project() else {
        return Err(OpError::NotConnected { reason: "Kumi hasn't looked at the project yet: observe first.".into() });
    };
    let tools = ctx.link().tools();
    let named = call.args.get("tool").and_then(Value::as_str);
    let most = call.args.get("max_chars").and_then(Value::as_u64).unwrap_or(MAX_CHARS.0) as usize;
    match call.args.get("action").and_then(Value::as_str) {
        Some("schema") => schema(ctx, &tools, named, most),
        Some("data") => {
            let Some(name) = named else {
                return Err(OpError::Invalid { reason: "Name the reader to run (`tool`): read with action schema lists them.".into() });
            };
            let Some(tool) = tools.iter().find(|tool| tool.name() == name && reader(tool.as_ref())) else {
                return Err(OpError::Invalid {
                    reason: format!("{name} isn't one of this DAW's readers: read with action schema lists them."),
                });
            };
            let query = call.args.get("query").and_then(Value::as_object).cloned().unwrap_or_default();
            if query.get("kind").and_then(Value::as_str) == Some("call") || names_a_function(&Value::Object(query.clone())) {
                let reason = "A read that names an API function runs the DAW's code, which Kumi for Hermes doesn't: read by kind.";
                return Err(OpError::Refused { code: "not_allowed", reason: reason.into(), owner: None });
            }
            if let Some(problem) = kumi_agent::kernel::schema::problem(&Value::Object(tool.input_schema()), &Value::Object(query.clone())) {
                return Err(OpError::Invalid { reason: format!("query: {problem}") });
            }
            // The rows are the last look's project's only while it's the one open: the DAW answering, and no return
            // since (an own bridge reports a tab switch to another project as one), before the read and after it.
            let project = ctx.link().expect_project(&last.key)?;
            let ran = tool.execute(query, call.signal.clone()).await.map_err(|error| match ctx.link().expect_project(&last.key) {
                Err(away) => away,
                Ok(_) => OpError::Failed { reason: format!("The DAW didn't answer the read: {error}") },
            })?;
            ctx.link().expect_project(&last.key)?;
            if ran.is_error {
                return Err(OpError::Invalid { reason: super::core::bounded(&names_only_text(&ran.text), most) });
            }
            // An own bridge's page is the looked project's only when its epoch is the one the look kept: one that's
            // unknown either side (the reader busy at the look, say) isn't taken for it.
            if own_reader(name) {
                let page_epoch = serde_json::from_str::<Value>(&ran.text)
                    .ok()
                    .and_then(|rows| rows.get("epoch").and_then(Value::as_str).map(str::to_string));
                if !ctx.link().page_is_looked(page_epoch.as_deref()) {
                    let reason =
                        "Kumi can't tell these rows are the looked project's (another tab, say, or a look it couldn't pin): observe again.";
                    return Err(OpError::Refused { code: "stale", reason: reason.into(), owner: None });
                }
            }
            Ok(page(&ran.text, most, &project))
        }
        _ => Err(OpError::Invalid { reason: "action is data or schema.".into() }),
    }
}

/// One of Kumi's own bridges' readers, whose pages carry the bridge's epoch.
fn own_reader(name: &str) -> bool {
    matches!(name, "reaper_read" | "renoise_read")
}

/// The epoch an own bridge's reader gives now, read right after a look (its first kind, one row), so a later page of
/// another epoch is known for another state of the project. None when there's no own reader (Live: its Set switch is
/// a return) or it didn't answer with one; an own reader's page is then refused, never taken on trust.
pub(super) async fn page_epoch_now(tools: &[Rc<dyn KernelTool>], signal: kumi_common::abort::Signal) -> Option<String> {
    let reader = tools.iter().find(|tool| own_reader(tool.name()))?;
    let schema = reader.input_schema();
    let kind = schema.get("properties")?.get("kind")?.get("enum")?.as_array()?.first()?.as_str()?.to_string();
    let query: Map<String, Value> = [("kind".to_string(), json!(kind)), ("limit".to_string(), json!(1))].into_iter().collect();
    let ran = reader.execute(query, signal).await.ok().filter(|ran| !ran.is_error)?;
    serde_json::from_str::<Value>(&ran.text).ok()?.get("epoch")?.as_str().map(str::to_string)
}

/// A reader's answer as `read` gives it: names, not folders, and at most `most` characters, with the page's cursor
/// kept outside the text so a cut never loses it.
fn page(text: &str, most: usize, project: &super::connection::Project) -> Value {
    let mut answer = json!({"project_key": project.key, "epoch": project.epoch});
    let shown = match serde_json::from_str::<Value>(text) {
        Ok(mut rows) => {
            names_only(&mut rows);
            if let Some(cursor) =
                ["nextCursor", "next_cursor", "cursor"].iter().find_map(|name| rows.get(*name).filter(|value| !value.is_null()).cloned())
            {
                answer["cursor"] = cursor;
            }
            rows.to_string()
        }
        Err(_) => names_only_text(text),
    };
    answer["text"] = json!(super::core::bounded(&shown, most));
    answer
}

/// The steps a plugin's plan may use here: the plan tool offers them, and the DAW's allowlist has them. A step that
/// places a file isn't one until the import step (CC-11) is in the runtime: `change` refuses it.
fn allowed_steps(ctx: &Ctx, tools: &[Rc<dyn KernelTool>]) -> Vec<&'static PluginStep> {
    let listed = crate::daw::provider(ctx.daw()).map_or(&[][..], |provider| provider.plugin_steps());
    let offered =
        tools.iter().find(|tool| tool.role() == Some(ToolRole::Plan)).map(|plan| offered_steps(&plan.input_schema())).unwrap_or_default();
    listed.iter().filter(|step| step.file_field.is_none() && offered.iter().any(|name| name == step.tool)).collect()
}

/// A step's or reader's whole input; or, with none named, the steps a change may use and the readers, each with what
/// it does (while that fits in `most`; the names alone otherwise).
fn schema(ctx: &Ctx, tools: &[Rc<dyn KernelTool>], named: Option<&str>, most: usize) -> Result<Value, OpError> {
    let steps = allowed_steps(ctx, tools);
    let tool_of = |name: &str| tools.iter().find(|tool| tool.name() == name);
    if let Some(name) = named {
        if let Some(step) = steps.iter().find(|step| step.tool == name) {
            let Some(tool) = tool_of(name) else {
                return Err(OpError::Invalid { reason: format!("{name} takes no input of its own to describe.") });
            };
            let mut answer = json!({
                "tool": name,
                "kind": "step",
                "family": step.family,
                "description": tool.description(),
                "input_schema": plugin_schema(step, &tool.input_schema(), &source_schema()),
            });
            if !step.refused.is_empty() {
                let refused: Map<String, Value> = step.refused.iter().map(|(field, values)| (field.to_string(), json!(values))).collect();
                answer["refused_values"] = Value::Object(refused);
            }
            return Ok(answer);
        }
        if let Some(tool) = tools.iter().find(|tool| tool.name() == name && reader(tool.as_ref())) {
            return Ok(json!({"tool": name, "kind": "reader", "description": tool.description(), "input_schema": tool.input_schema()}));
        }
        let listed = crate::daw::provider(ctx.daw()).map_or(&[][..], |provider| provider.plugin_steps());
        if listed.iter().any(|step| step.tool == name && step.file_field.is_some()) {
            return Err(OpError::Invalid {
                reason: format!("{name} places a file, which needs the import step: not in this runtime yet."),
            });
        }
        return Err(OpError::Invalid {
            reason: format!(
                "{name} isn't a step a change may use here, nor one of this DAW's readers: read with action schema lists them."
            ),
        });
    }
    let listed = |with_what: bool| {
        let step_rows: Vec<Value> = steps
            .iter()
            .map(|step| {
                let mut row = json!({"tool": step.tool, "family": step.family});
                if let Some(tool) = tool_of(step.tool).filter(|_| with_what) {
                    row["what"] = json!(first_clause(tool.description()));
                }
                row
            })
            .collect();
        let reader_rows: Vec<Value> = tools
            .iter()
            .filter(|tool| reader(tool.as_ref()))
            .map(|tool| {
                if with_what {
                    json!({"tool": tool.name(), "what": first_clause(tool.description())})
                } else {
                    json!({"tool": tool.name()})
                }
            })
            .collect();
        json!({"steps": step_rows, "readers": reader_rows})
    };
    let full = listed(true);
    Ok(if full.to_string().chars().count() <= most { full } else { listed(false) })
}

/// What a tool does: its description up to the end of the first sentence, at most 100 characters.
fn first_clause(description: &str) -> String {
    let end = description.find(". ").unwrap_or(description.len());
    let clause = description[..end].trim_end_matches('.');
    match clause.char_indices().nth(100) {
        Some((at, _)) => format!("{}…", &clause[..at]),
        None => clause.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_names_files_not_folders_and_keeps_its_cursor_through_a_cut() {
        let project = super::super::connection::Project { key: "p-1".into(), name: "Song".into(), epoch: 7, seen_at: 0 };
        let rows = json!({
            "items": [
                {"ref": "r:item:{4F2A}", "name": "Vox", "source": "/Users/me/Music/Takes/Vox take 2.wav"},
                {"trackRef": "/odd/ref", "filePath": "C:\\Samples\\Kick.wav", "project": "\\\\server\\share\\Song.RPP"}
            ],
            "nextCursor": "c-2",
            "padding": "x".repeat(2000)
        });
        let answer = page(&rows.to_string(), 600, &project);
        let text = answer["text"].as_str().unwrap();
        assert!(!text.contains("/Users/") && !text.contains("Samples\\\\") && !text.contains("server"), "{text}");
        assert!(text.contains("Vox take 2.wav") && text.contains("Kick.wav") && text.contains("/odd/ref"), "{text}");
        assert_eq!(answer["cursor"], "c-2", "lifted out of the text");
        assert!(text.chars().count() <= 600, "{}", text.chars().count());
        let plain = page("Opened /Users/me/Songs/Demo.RPP \"C:\\Temp\\x.wav\" here", 600, &project);
        assert_eq!(plain["text"], "Opened Demo.RPP \"x.wav\" here");
    }

    #[test]
    fn a_read_naming_an_api_function_anywhere_is_spotted() {
        assert!(names_a_function(&json!({"kind": "track", "where": [{"fn": "GetTrackName"}]})));
        assert!(names_a_function(&json!({"args": []})));
        assert!(!names_a_function(&json!({"kind": "track", "parent": "r:track:1", "limit": 20})));
    }
}
