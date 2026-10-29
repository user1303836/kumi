//! The palette tool, the same in every DAW: the session's palette (core/palette.rs) for the model. It lists and searches
//! the palette's sounds, says what a loose palette brings in (each sound's nearest in the producer's library, the ones
//! their own projects use first), and adds, takes out or excludes sounds by their paths. It works away from the DAW.

use crate::core::palette::{PaletteFile, Palettes};
use async_trait::async_trait;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_types::{
    contracts::{JsonObject, KernelTool, Reach, ToolResult},
    errors::RuntimeError,
};
use serde_json::{json, Value};
use std::rc::Rc;

pub const TOOL: &str = "palette";
/// What a list, a search or neighbours gives when it doesn't say.
const LISTED: usize = 50;

const DESCRIPTION: &str = concat!(
    "The producer's palette: the sounds they gathered to work with, which the turn's brief names. ",
    "list: its sounds (and those it excludes); search: its sounds with every one of words in their name, class or folder; ",
    "neighbours: what a loose palette brings in, each of its sounds' nearest in their library (or path's), the ones their own projects use first; ",
    "add and remove: sounds by their paths, only when the producer asks or agrees; exclude: sounds it must never bring in."
);

pub struct PaletteTool {
    palettes: Rc<Palettes>,
}

impl PaletteTool {
    pub fn new(palettes: Rc<Palettes>) -> Rc<dyn KernelTool> {
        Rc::new(PaletteTool { palettes })
    }

    async fn run(&self, input: &JsonObject) -> Result<Value, String> {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("list");
        let limit = input.get("limit").and_then(Value::as_u64).map_or(LISTED, |limit| (limit as usize).clamp(1, 200));
        let paths = || -> Result<Vec<String>, String> {
            let paths: Vec<String> = input
                .get("paths")
                .and_then(Value::as_array)
                .map(|paths| paths.iter().filter_map(|path| path.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            if paths.is_empty() {
                return Err(format!("{action} takes paths: the sounds' files"));
            }
            Ok(paths)
        };
        let failed = |error: RuntimeError| error.to_string();
        match action {
            "list" | "search" => {
                let Some(view) = self.palettes.view().await.map_err(failed)? else { return Ok(none()) };
                let words: Vec<String> =
                    input.get("words").and_then(Value::as_str).unwrap_or_default().split_whitespace().map(str::to_lowercase).collect();
                if action == "search" && words.is_empty() {
                    return Err("search takes words: what its sounds have in their name, class or folder".into());
                }
                // A sound's own folder, not every folder above it: a word in the home folder's name ("/Users/padma")
                // would find every sound.
                let has = |file: &&PaletteFile| {
                    let folder = std::path::Path::new(&file.path).parent().and_then(|folder| folder.file_name()).unwrap_or_default();
                    let text =
                        format!("{} {} {}", file.name, file.class.as_deref().unwrap_or_default(), folder.to_string_lossy()).to_lowercase();
                    words.iter().all(|word| text.contains(word.as_str()))
                };
                let found: Vec<&PaletteFile> = view.files.iter().filter(has).collect();
                let mut reply = json!({
                    "palette": view.name,
                    "mode": view.mode(),
                    "sounds": found.iter().take(limit).map(|file| described(file)).collect::<Vec<_>>(),
                });
                if found.len() > limit {
                    reply["more"] = json!(found.len() - limit);
                }
                if action == "list" && !view.excluded.is_empty() {
                    reply["excluded"] = json!(view.excluded.iter().map(described).collect::<Vec<_>>());
                }
                Ok(reply)
            }
            "neighbours" => {
                let Some(view) = self.palettes.view().await.map_err(failed)? else { return Ok(none()) };
                if view.reach == 0 {
                    return Ok(json!({
                        "palette": view.name,
                        "mode": view.mode(),
                        "say": "The palette is strict: it brings nothing in. The producer makes it loose with /palette loose.",
                    }));
                }
                let of = input.get("path").and_then(Value::as_str);
                let name = match of {
                    Some(path) => match view.files.iter().find(|file| file.path == path) {
                        Some(file) => Some(file.name.clone()),
                        None => return Err(format!("{path} isn't one of the palette's sounds: list gives their paths")),
                    },
                    None => None,
                };
                let reached = self.palettes.reached().await.map_err(failed)?;
                let found: Vec<_> = reached.iter().filter(|sound| name.as_ref().is_none_or(|name| sound.near.contains(name))).collect();
                Ok(json!({
                    "palette": view.name,
                    "mode": view.mode(),
                    "sounds": found.iter().take(limit).map(|sound| {
                        let mut each = json!({"name": sound.name, "path": sound.path, "near": sound.near, "usedInProjects": sound.used});
                        if let Some(class) = &sound.class {
                            each["class"] = json!(class);
                        }
                        each
                    }).collect::<Vec<_>>(),
                    "more": found.len().saturating_sub(limit),
                }))
            }
            "add" => Ok(json!({"said": self.palettes.add_paths(&paths()?).await.map_err(failed)?.words()})),
            "remove" => {
                let removed = self.palettes.remove_paths(&paths()?).await.map_err(failed)?;
                Ok(json!({"removed": removed}))
            }
            "exclude" => {
                let excluded = self.palettes.exclude_paths(&paths()?).await.map_err(failed)?;
                Ok(json!({"excluded": excluded}))
            }
            _ => Err("action is list, search, neighbours, add, remove or exclude".into()),
        }
    }
}

/// What a palette's file is, for the model.
fn described(file: &PaletteFile) -> Value {
    let mut each = json!({"name": file.name, "path": file.path});
    if let Some(class) = &file.class {
        each["class"] = json!(class);
    }
    if file.song {
        each["song"] = json!(true);
    }
    each
}

/// The session has no palette yet.
fn none() -> Value {
    json!({"palette": null, "say": "The producer has no palette in this session: Enter on /map, or /palette new and a name, makes one."})
}

#[async_trait(?Send)]
impl KernelTool for PaletteTool {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn input_schema(&self) -> JsonObject {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["action"],
            "properties": {
                "action": {"type": "string", "enum": ["list", "search", "neighbours", "add", "remove", "exclude"]},
                "words": {"type": "string", "description": "search: what its sounds have in their name, class or folder"},
                "path": {"type": "string", "description": "neighbours: one of the palette's sounds (each of them when left out)"},
                "paths": {
                    "type": "array",
                    "items": {"type": "string"},
                    "minItems": 1,
                    "maxItems": 200,
                    "description": "add, remove, exclude: the sounds' files"
                },
                "limit": {"type": "integer", "minimum": 1, "maximum": 200, "description": "list, search, neighbours: at most this many (50)"}
            }
        });
        schema.as_object().cloned().unwrap_or_default()
    }

    async fn execute(&self, input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(match self.run(&input).await {
            Ok(reply) => ToolResult { text: stringify(&reply), ..Default::default() },
            Err(why) => ToolResult { text: why, is_error: true, ..Default::default() },
        })
    }

    fn reach(&self) -> Reach {
        Reach::AwayFromDaw
    }
}
