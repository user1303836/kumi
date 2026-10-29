//! Tools the model doesn't see up front. Every request carries the core tools and two of the kernel's own:
//! find_tools, whose description lists each deferred tool by name with what it's for, and which gives a
//! tool's whole description and input; and call_tool, which runs one. A deferred tool called by its own name
//! runs the same way. It's the same for every provider, so the deferred definitions stay out of every
//! request (and its cached prefix) wherever the model runs.

use std::collections::HashMap;
use std::rc::Rc;

use crate::core::text::cut;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use kumi_common::js::string::trim;
use serde_json::{json, Value};

use super::schema;
use kumi_ai::types::FunctionTool;
use kumi_types::contracts::{JsonObject, KernelTool, ToolResult};
use kumi_types::errors::RuntimeError;

pub const FIND_TOOLS: &str = "find_tools";
pub const CALL_TOOL: &str = "call_tool";
/// The most find_tools' list of deferred tools takes: every name, and what each is for while there's room.
const INDEX_BYTES: usize = 3 * 1024;
/// The most tools one search gives back.
const FOUND: usize = 8;
const MAX_NAMES: usize = 16;

/// The tool a call runs and its input, or why it can't run (the call's result).
pub type Resolved = Result<(Rc<dyn KernelTool>, JsonObject), String>;

/// The deferred tools, in the order they were given.
pub struct Shelf {
    tools: Vec<Rc<dyn KernelTool>>,
    by_name: HashMap<String, usize>,
}

impl Shelf {
    /// None when no tool is deferred: the request carries every tool, as before.
    pub fn new(tools: Vec<Rc<dyn KernelTool>>) -> Option<Rc<Shelf>> {
        if tools.is_empty() {
            return None;
        }
        let by_name = tools.iter().enumerate().map(|(index, tool)| (tool.name().to_string(), index)).collect();
        Some(Rc::new(Shelf { tools, by_name }))
    }

    pub fn has(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    fn get(&self, name: &str) -> Option<&Rc<dyn KernelTool>> {
        self.by_name.get(name).map(|index| &self.tools[*index])
    }

    /// The two tools every request carries in place of the deferred ones.
    pub fn tools(self: &Rc<Self>) -> Vec<Rc<dyn KernelTool>> {
        vec![Rc::new(FindTools { shelf: self.clone(), description: self.find_description() }), Rc::new(CallTool)]
    }

    /// find_tools' description: what it does, then every deferred tool as a signature (its fields, `?` after one it
    /// can leave out) with what it's for, so a call whose input is plain needs no lookup; tools that a plan tool lists
    /// with their input are left to it.
    fn find_description(&self) -> String {
        let mut listed: Vec<(&str, Vec<String>, String)> = Vec::new();
        let mut by: Vec<&str> = Vec::new();
        for tool in &self.tools {
            match tool.listed_by() {
                Some(plan) => {
                    if !by.contains(&plan) {
                        by.push(plan);
                    }
                }
                None => listed.push((tool.name(), schema::fields(&tool.input_schema()), clause(tool.description()))),
            }
        }
        let elsewhere =
            if by.is_empty() { String::new() } else { format!(" Besides these, every step {} lists is one too.", by.join(" and ")) };
        // Every name with its fields, and as much of what each is for as fits.
        let mut index = String::new();
        for (room, most) in [(90, 8), (64, 8), (48, 6), (36, 6), (28, 5), (20, 4), (0, 4), (0, 0)] {
            index = listed
                .iter()
                .map(|(name, fields, what)| {
                    let signature = if fields.len() > most { format!("{}, …", fields[..most].join(", ")) } else { fields.join(", ") };
                    let call = if most == 0 { (*name).to_string() } else { format!("{name}({signature})") };
                    if room == 0 || what.is_empty() {
                        call
                    } else {
                        format!("{call}: {}", words(what, room))
                    }
                })
                .collect::<Vec<_>>()
                .join("; ");
            if index.len() <= INDEX_BYTES {
                break;
            }
        }
        format!(
            "Your other tools, sent only when you need one. Each is listed as name(its fields; `?`: may be left out): what it's for. When a tool's fields are plain, run it straight away with call_tool, or by its name; otherwise ask by names (or a few words) and find_tools gives its whole description and input. They are: {index}.{elsewhere}"
        )
    }

    /// The tools a find_tools call asks for, by name and by words, as the model reads them.
    fn find(&self, input: &JsonObject) -> ToolResult {
        let names: Vec<String> = input
            .get("names")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).map(|name| trim(name).to_string()).collect())
            .unwrap_or_default();
        let words = input.get("words").and_then(Value::as_str).map(trim).unwrap_or("").to_lowercase();
        if names.is_empty() && words.is_empty() {
            return ToolResult::error("Ask for tools by names (from the list in find_tools' description) or by a few words.");
        }
        let mut found: Vec<usize> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        for name in names.iter().take(MAX_NAMES) {
            match self.by_name.get(name.as_str()) {
                Some(index) if !found.contains(index) => found.push(*index),
                Some(_) => {}
                None => unknown.push(name.clone()),
            }
        }
        if !words.is_empty() {
            let wanted: Vec<&str> = words.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|word| word.len() > 1).collect();
            let mut scored: Vec<(usize, usize)> = self
                .tools
                .iter()
                .enumerate()
                .filter(|(index, _)| !found.contains(index))
                .map(|(index, tool)| {
                    let name = tool.name().to_lowercase();
                    let about = tool.description().to_lowercase();
                    // A word in the name counts most; one in the description counts too.
                    let score = wanted.iter().map(|word| if name.contains(word) { 3 } else { usize::from(about.contains(word)) }).sum();
                    (index, score)
                })
                .filter(|(_, score)| *score > 0)
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            found.extend(scored.into_iter().take(FOUND).map(|(index, _)| index));
        }
        let tools: Vec<Value> = found
            .iter()
            .map(|index| {
                let tool = &self.tools[*index];
                let mut entry =
                    json!({"name": tool.name(), "description": tool.description(), "input": Value::Object(tool.input_schema())});
                if let Some(plan) = tool.listed_by() {
                    entry["alsoAStepOf"] = json!(plan);
                }
                entry
            })
            .collect();
        let mut answer = json!({"tools": tools});
        if !unknown.is_empty() {
            answer["unknown"] = json!(unknown);
        }
        if tools.is_empty() {
            answer["note"] = json!("Nothing matched: try other words, or a name from find_tools' list.");
        } else {
            answer["run"] = json!("Run one with call_tool {\"name\": …, \"input\": {…}}, or by its name, with this input.");
        }
        ToolResult { text: stringify(&answer), is_error: tools.is_empty(), ..Default::default() }
    }

    /// The deferred tool a call runs and its input: call_tool's own, or a deferred tool called by its name.
    /// Err says what's wrong, as the call's result.
    pub fn resolve(&self, name: &str, input: Option<&JsonObject>) -> Option<Resolved> {
        if name == CALL_TOOL {
            return Some(self.unwrap(input));
        }
        let tool = self.get(name)?;
        let input = input.cloned().unwrap_or_default();
        Some(checked(tool, input))
    }

    fn unwrap(&self, input: Option<&JsonObject>) -> Resolved {
        let input = input.ok_or_else(|| "call_tool's input must be a JSON object: {\"name\": …, \"input\": {…}}.".to_string())?;
        let Some(name) = input.get("name").and_then(Value::as_str).map(trim).filter(|name| !name.is_empty()) else {
            return Err("call_tool needs `name`: a tool from find_tools' list.".into());
        };
        let inner = match input.get("input") {
            None | Some(Value::Null) => JsonObject::new(),
            Some(Value::Object(inner)) => inner.clone(),
            Some(_) => return Err(format!("call_tool's `input` must be the object {name} takes.")),
        };
        if name == CALL_TOOL {
            return Err("call_tool runs another tool, not itself.".into());
        }
        match self.get(name) {
            Some(tool) => checked(tool, inner),
            None => Err(format!("No tool is called {}; find_tools' description lists them.", cut(name, 64))),
        }
    }
}

/// A deferred tool's input, checked against its own schema first.
fn checked(tool: &Rc<dyn KernelTool>, input: JsonObject) -> Resolved {
    match schema::problem(&Value::Object(tool.input_schema()), &Value::Object(input.clone())) {
        Some(problem) => Err(format!("{}: {problem} find_tools gives its whole input.", tool.name())),
        None => Ok((tool.clone(), input)),
    }
}

/// What a tool is for, from its description: up to the end of its first sentence or clause.
fn clause(description: &str) -> String {
    let text = trim(description);
    let end = [". ", ": ", "; ", " (", " — "].iter().filter_map(|stop| text.find(stop)).min().unwrap_or(text.len());
    trim(text[..end].trim_end_matches('.')).to_string()
}

/// At most `room` bytes of `text`, cut at a word.
fn words(text: &str, room: usize) -> String {
    if text.len() <= room {
        return text.to_string();
    }
    let kept = cut(text, room);
    match kept.rfind(' ') {
        Some(at) if at > room / 2 => format!("{}…", &kept[..at]),
        _ => format!("{kept}…"),
    }
}

/// The name and input a call shows as: the tool call_tool ran, or the call itself.
pub fn shown<'a>(name: &'a str, input: &'a Value) -> (&'a str, &'a Value) {
    if name == CALL_TOOL {
        if let Some(inner) = input.get("name").and_then(Value::as_str) {
            return (inner, input.get("input").unwrap_or(&Value::Null));
        }
    }
    (name, input)
}

struct FindTools {
    shelf: Rc<Shelf>,
    description: String,
}

#[async_trait(?Send)]
impl KernelTool for FindTools {
    fn name(&self) -> &str {
        FIND_TOOLS
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object","additionalProperties":false,"properties":{
            "names":{"type":"array","maxItems":MAX_NAMES,"items":{"type":"string","minLength":1,"maxLength":64},"description":"Tools by name, from the list above"},
            "words":{"type":"string","maxLength":200,"description":"What you need, in a few words, when you don't know the name"}
        }})
        .as_object()
        .expect("schema")
        .clone()
    }
    async fn execute(&self, input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(self.shelf.find(&input))
    }
}

/// Never run itself: the kernel runs the tool it names (see `Shelf::resolve`).
struct CallTool;

#[async_trait(?Send)]
impl KernelTool for CallTool {
    fn name(&self) -> &str {
        CALL_TOOL
    }
    fn description(&self) -> &str {
        "Run one of the tools find_tools lists, with its input: the same as calling it by its name."
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object","additionalProperties":false,"required":["name","input"],"properties":{
            "name":{"type":"string","minLength":1,"maxLength":64,"description":"A tool from find_tools' list"},
            "input":{"type":"object","description":"Its input, as find_tools gives it"}
        }})
        .as_object()
        .expect("schema")
        .clone()
    }
    async fn execute(&self, _input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(ToolResult::error("call_tool runs the tool it names."))
    }
}

/// The spec a tool is sent as.
pub fn spec(tool: &Rc<dyn KernelTool>) -> FunctionTool {
    FunctionTool::new(tool.name(), tool.description(), Value::Object(tool.input_schema()))
}
