//! The scripts tool, the same in every DAW: a chunk linted; one the producer said yes to kept (Kumi's copy, and the DAW's own
//! action); what's kept listed; one run again by name (one step of the DAW's undo); one removed, its action too.
use super::backend::ScriptsBackend;
use super::lint::{lint, Finding};
use super::store::{line, slug, Saved, Store};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use kumi_types::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::rc::Rc;

pub const TOOL: &str = "scripts";

/// The most of a script's code (bytes).
const MOST_CODE: usize = 200_000;

pub struct Scripts<B: ScriptsBackend> {
    backend: B,
    store: Store,
    description: String,
}

fn flag(input: &JsonObject, key: &str) -> bool {
    input.get(key) == Some(&Value::Bool(true))
}

fn code_of(input: &JsonObject) -> Result<&str, String> {
    input
        .get("code")
        .and_then(Value::as_str)
        .filter(|code| !code.trim().is_empty() && code.len() <= MOST_CODE)
        .ok_or_else(|| "code: the chunk as it ran".to_string())
}

fn found_json(found: &[Finding]) -> Vec<Value> {
    found.iter().map(|finding| json!({"blocks": finding.blocks, "says": finding.says})).collect()
}

impl<B: ScriptsBackend + 'static> Scripts<B> {
    /// Kept in `dir` (Kumi's folder of this DAW's scripts).
    pub fn new(backend: B, dir: PathBuf) -> Rc<Self> {
        let description = description(backend.words());
        Rc::new(Scripts { backend, store: Store::new(dir), description })
    }

    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(ScriptsTool { owner: self.clone() })
    }

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        match input.get("action").and_then(Value::as_str).unwrap_or("list") {
            "list" => Ok(self.list()),
            "lint" => {
                let code = code_of(&input)?;
                let found = self.findings(code, flag(&input, "thisProject"));
                let blocks = found.iter().any(|finding| finding.blocks);
                Ok(json!({"findings": found_json(&found), "next": if blocks {
                    "fix what blocks it before keeping it"
                } else {
                    "ask the producer whether to keep it; save it once they said yes (confirmed: true)"
                }}))
            }
            "save" => self.save(&input, &signal).await,
            "run" => self.run_kept(&input, &signal).await,
            "remove" => self.remove(&input, &signal).await,
            _ => Err("action is list (the default), lint, save, run or remove".into()),
        }
    }

    fn findings(&self, code: &str, this_project: bool) -> Vec<Finding> {
        lint(code, self.backend.words(), self.backend.ran_cleanly(code), this_project)
    }

    fn named(&self, input: &JsonObject) -> Result<Saved, String> {
        let name = input.get("name").and_then(Value::as_str).unwrap_or("");
        self.store.get(name).ok_or_else(|| format!("no script named \u{201c}{name}\u{201d} is kept: list them"))
    }

    async fn save(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        if !flag(input, "confirmed") {
            return Err("save keeps a script only once the producer said yes to keeping it (confirmed: true): ask them first".into());
        }
        let name = line(input.get("name").and_then(Value::as_str).unwrap_or(""), "name", true)?;
        let mut description = line(input.get("description").and_then(Value::as_str).unwrap_or(""), "description", false)?;
        let code = code_of(input)?;
        let this_project = flag(input, "thisProject");
        let found = self.findings(code, this_project);
        let blocking: Vec<&str> = found.iter().filter(|finding| finding.blocks).map(|finding| finding.says.as_str()).collect();
        if !blocking.is_empty() {
            return Err(format!("not kept: {}", blocking.join("; ")));
        }
        let replace = flag(input, "replace");
        let slug = slug(&name)?;
        if !replace && self.store.get(&slug).is_some() {
            return Err(format!("a script named \u{201c}{name}\u{201d} is kept already: replace: true writes over it"));
        }
        if this_project {
            description = format!("{description} (for this project only)").trim().to_string();
        }
        let mut saved = Saved {
            name,
            slug,
            description,
            kept: chrono::Local::now().format("%Y-%m-%d").to_string(),
            action: None,
            code: code.to_string(),
        };
        let in_daw = input.get("inDaw") != Some(&Value::Bool(false));
        let installed = if in_daw { Some(self.backend.install(&saved, replace, signal).await?) } else { None };
        saved.action = installed.as_ref().map(|installed| installed.action.clone());
        if let Err(why) = self.store.save(&saved, replace) {
            // (an action in the DAW with no copy of Kumi's would be a script nobody lists: taken away again)
            if in_daw {
                let _ = self.backend.uninstall(&saved.slug, signal).await;
            }
            return Err(why);
        }
        let mut reply = json!({"kept": saved.name});
        if let Some(installed) = installed {
            reply["action"] = json!(installed.action);
            reply["file"] = json!(installed.file);
        }
        let notes: Vec<&str> = found.iter().map(|finding| finding.says.as_str()).collect();
        if !notes.is_empty() {
            reply["notes"] = json!(notes);
        }
        reply["next"] = json!("tell the producer where it is (a shortcut can be set for it there); run takes it by name next time");
        Ok(reply)
    }

    fn list(&self) -> Value {
        let kept = self.store.list();
        let rows: Vec<Value> = kept
            .iter()
            .map(|saved| json!({"name": saved.name, "description": saved.description, "kept": saved.kept, "action": saved.action}))
            .collect();
        json!({"scripts": rows, "count": kept.len()})
    }

    async fn run_kept(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let saved = self.named(input)?;
        let answer = self.backend.run(&saved.code, signal).await?;
        Ok(json!({"ran": saved.name, "answer": answer}))
    }

    async fn remove(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        if !flag(input, "confirmed") {
            return Err("remove takes a kept script away only once the producer said so (confirmed: true)".into());
        }
        let saved = self.named(input)?;
        if saved.action.is_some() {
            self.backend.uninstall(&saved.slug, signal).await?;
        }
        self.store.remove(&saved.slug)?;
        Ok(json!({"removed": saved.name}))
    }
}

fn description(words: &super::backend::Words) -> String {
    format!(
        "Keep a chore the producer repeats as a script to run again. Write it against the live session with run_lua and run it once (one step of {app}'s undo), so they see it work; then ask whether to keep it. Only once they said yes, action save (name, description, code as it ran, confirmed: true) keeps it: Kumi runs it again by name (action run, one step of the undo) and it becomes {action}. lint (and save) says what would stop it working next time, in another project: ids from this project (thisProject: true keeps one for this project only), Kumi's refs (this conversation's), a chunk that hasn't run here without an error; what it takes by its place is said. action list (the default) shows what's kept: look before writing a chore again. remove (confirmed: true) takes one away, its action too. inDaw: false keeps it for Kumi alone. Scripts are {language}.",
        app = words.app,
        action = words.action,
        language = words.language
    )
}

fn schema() -> JsonObject {
    let value = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "action": {"type": "string", "enum": ["list", "lint", "save", "run", "remove"]},
            "name": {"type": "string", "maxLength": 80},
            "description": {"type": "string", "maxLength": 500, "description": "What it does, in a line"},
            "code": {"type": "string", "minLength": 1, "maxLength": MOST_CODE},
            "confirmed": {"type": "boolean", "description": "The producer said yes to keeping (or removing) it"},
            "replace": {"type": "boolean"},
            "thisProject": {"type": "boolean"},
            "inDaw": {"type": "boolean"}
        }
    });
    value.as_object().cloned().unwrap_or_default()
}

struct ScriptsTool<B: ScriptsBackend> {
    owner: Rc<Scripts<B>>,
}

#[async_trait(?Send)]
impl<B: ScriptsBackend + 'static> KernelTool for ScriptsTool<B> {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        &self.owner.description
    }

    fn input_schema(&self) -> JsonObject {
        schema()
    }

    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(match self.owner.run(input, signal).await {
            Ok(reply) => ToolResult { text: stringify(&reply), ..Default::default() },
            Err(why) => ToolResult { text: why, is_error: true, ..Default::default() },
        })
    }

    fn reach(&self) -> Reach {
        Reach::DawWrite
    }

    fn deferred(&self) -> bool {
        true
    }
}
