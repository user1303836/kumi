//! Capabilities the producer needed that Kumi or Live does not offer, logged locally for developers.
use crate::core::{store_client::StoreClient, store_import::parse_gaps};
use async_trait::async_trait;
use kumi_common::{
    abort::Signal,
    js::{json, string},
    time::{iso_string, now_ms},
};
use kumi_store::gaps;
use kumi_types::daw::{DawId, DawProfile};
use kumi_types::version::KUMI_VERSION;
use kumi_types::{
    contracts::{with_final, JsonObject, KernelTool, ToolResult},
    errors::RuntimeError,
    notes::suspect_note,
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    rc::Rc,
};
use tokio::io::AsyncWriteExt;
pub const GAP_TOOL: &str = "note_gap";
kumi_types::daw_text!(
    pub GAP_GUIDANCE, gap_guidance:
    "When a request needs something Kumi's tools or ", app, "'s scripting don't offer, take the way round first (another tool, a plan of several, a recording, a device you make) and do it, cleanup included; then note the gap with note_gap beside your answer (final: true once nothing is left to do), and say in a sentence what you did instead."
);
const MAX_BYTES: u64 = 256 * 1024;
const KEEP_LINES: usize = 500;
kumi_types::daw_text!(
    #[allow(dead_code)]
    DESCRIPTION, description:
    "When the producer asks for something you can't do because Kumi's tools or ", app, "'s API lack it (a device setting scripts can't reach, an operation no tool offers), note it here for Kumi's developers, then tell the producer and offer the way round. ",
    "Not for things you chose not to do, or that failed for another reason. The producer doesn't see this, and it isn't a memory."
);
fn clean(value: Option<&Value>) -> String {
    let text: String = value
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .map(|c| if c <= '\u{1f}' || ('\u{7f}'..='\u{9f}').contains(&c) || c == '\u{feff}' { ' ' } else { c })
        .collect();
    string::trim(&string::head(&text.split_whitespace().collect::<Vec<_>>().join(" "), 300)).to_string()
}
/// The gap tool, logging to Kumi's database when there is one, otherwise to `file`.
pub fn gap_tools(file: impl Into<PathBuf>, store: Option<StoreClient>) -> Vec<Rc<dyn KernelTool>> {
    gap_tools_for(DawProfile::ABLETON, file, store)
}

/// `gap_tools`, saying the DAW's words.
pub fn gap_tools_for(profile: DawProfile, file: impl Into<PathBuf>, store: Option<StoreClient>) -> Vec<Rc<dyn KernelTool>> {
    vec![Rc::new(GapTool { file: file.into(), store, description: description(&profile), profile })]
}
struct GapTool {
    file: PathBuf,
    store: Option<StoreClient>,
    description: String,
    profile: DawProfile,
}
#[async_trait(?Send)]
impl KernelTool for GapTool {
    fn name(&self) -> &str {
        GAP_TOOL
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        with_final(json!({"type":"object","additionalProperties":false,"required":["missing"],"properties":{
        "missing":{"type":"string","minLength":1,"maxLength":300,"description":format!("What's missing, as a capability (\"{}\")", self.profile.vocab.missing_example)},
        "asked":{"type":"string","maxLength":300,"description":"What the producer asked for that needed it"},
        "workaround":{"type":"string","maxLength":300,"description":"What you did or suggested instead"}
    }}).as_object().unwrap().clone())
    }
    async fn execute(&self, input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        let missing = clean(input.get("missing"));
        if missing.is_empty() {
            return Ok(ToolResult::error("Say what's missing."));
        }
        let mut entry = json!({"at":iso_string(now_ms()),"kumi":KUMI_VERSION,"missing":missing});
        // The DAW it was missing in, when it isn't Live (never written for Live; kumi.db's gaps keep no DAW yet).
        if let Some(daw) = self.profile.id.filter(|daw| *daw != DawId::Ableton) {
            entry["daw"] = json!(daw);
        }
        for key in ["asked", "workaround"] {
            let text = clean(input.get(key));
            if !text.is_empty() {
                entry[key] = Value::String(text);
            }
        }
        if entry.as_object().unwrap().values().any(|v| suspect_note(v.as_str().unwrap_or(""))) {
            return Ok(ToolResult::error("That holds something that reads as a secret, so it wasn't logged."));
        }
        if let Some(store) = &self.store {
            let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
            let gap = gaps::Gap {
                kumi_version: KUMI_VERSION.into(),
                missing: missing.clone(),
                asked: text("asked"),
                workaround: text("workaround"),
                at: now_ms(),
            };
            store.write(move |c| gaps::add(c, &gap)).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
            return Ok(ToolResult { text: json::stringify(&json!({"noted":missing})), reply: Some(String::new()), ..Default::default() });
        }
        let folder = self.file.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(folder).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&self.file).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
        file.write_all(format!("{}\n", json::stringify(&entry)).as_bytes()).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
        file.flush().await.map_err(|e| RuntimeError::plain(e.to_string()))?;
        drop(file);
        // Trimming is best effort: the entry is already logged either way.
        if tokio::fs::metadata(&self.file).await.is_ok_and(|m| m.len() > MAX_BYTES) {
            if let Ok(text) = tokio::fs::read_to_string(&self.file).await {
                let lines: Vec<_> = text.split('\n').filter(|line| !line.is_empty()).collect();
                let kept = &lines[lines.len().saturating_sub(KEEP_LINES)..];
                let _ = tokio::fs::write(&self.file, format!("{}\n", kept.join("\n"))).await;
            }
        }
        Ok(ToolResult { text: json::stringify(&json!({"noted":missing})), reply: Some(String::new()), ..Default::default() })
    }
}

/// The gap log as `kumi report` shows it, written nowhere: the gaps Kumi's database at `db` has logged,
/// with any an older Kumi logged in `file` that it hasn't read in yet, oldest first, each as the gap
/// log writes it (one JSON line). Blocking.
pub fn logged_gaps(db: &Path, file: &Path) -> Result<Vec<String>, kumi_store::StoreError> {
    let mut logged = kumi_store::read_only(db, gaps::all)?;
    if let Ok(text) = std::fs::read_to_string(file) {
        let known: HashSet<(i64, String)> = logged.iter().map(|gap| (gap.at, gap.missing.clone())).collect();
        logged.extend(parse_gaps(&text).into_iter().filter(|gap| !known.contains(&(gap.at, gap.missing.clone()))));
        logged.sort_by_key(|gap| gap.at);
    }
    Ok(logged
        .into_iter()
        .map(|gap| {
            let mut entry = json!({"at":iso_string(gap.at),"kumi":gap.kumi_version,"missing":gap.missing});
            if let Some(asked) = gap.asked {
                entry["asked"] = Value::String(asked);
            }
            if let Some(workaround) = gap.workaround {
                entry["workaround"] = Value::String(workaround);
            }
            json::stringify(&entry)
        })
        .collect())
}
