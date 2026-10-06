//! Where each turn's time went: its model calls (time to first part and in all) with their reasoning
//! effort and service tier, its tools (and the slowest of them), its Live requests and the bytes it sent. One line per
//! turn in a local log (`timings.jsonl`), which `kumi report` shows, so every change to Kumi can be
//! measured before and after, and a slow answer says what it waited on.
//!
//! A turn runs on the session's one thread, and one turn runs at a time, so the turn being timed is
//! a thread-local the kernel, the model client and the Live client add to as they go.
use std::{cell::RefCell, collections::BTreeMap, path::Path, rc::Rc, time::Instant};

use kumi_common::{
    js::json,
    time::{iso_string, now_ms},
};
use serde_json::{json, Value};

use super::contracts::Usage;
use crate::ai::types::{CallOptions, Reasoning, StreamPart, StreamParts};
use crate::version::KUMI_VERSION;

const MAX_BYTES: u64 = 512 * 1024;
const KEEP_LINES: usize = 1000;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct TurnTiming {
    pub model: Option<String>,
    /// The reasoning effort its model calls asked for ("high"); none when the provider's default.
    pub effort: Option<String>,
    /// The service tier they asked for ("priority", as `/fast` does); none when the provider's default.
    pub tier: Option<String>,
    pub model_calls: u32,
    pub model_ms: u64,
    /// Each model call's wait for its first content: text, reasoning or a tool call (not the stream's
    /// start or its metadata, which come with the response's headers).
    pub first_part_ms: Vec<u64>,
    pub tools: u32,
    pub tool_ms: u64,
    /// Each tool's calls and their time. Calls that ran together each count their own.
    pub by_tool: BTreeMap<String, (u32, u64)>,
    /// Requests to Live's bridge, FOCUS's own reads while the turn ran left out.
    pub live_requests: u32,
    /// Request bodies sent to the model.
    pub sent_bytes: u64,
    /// The look at the files an older Kumi may be writing that the turn started (`FileSync`): its own
    /// time, beside the turn's, not part of it.
    pub files_ms: Option<u64>,
}

thread_local! {
    static ACTIVE: RefCell<Option<Rc<RefCell<TurnTiming>>>> = const { RefCell::new(None) };
}

tokio::task_local! {
    static BACKGROUND: ();
}

/// The turn being timed, from `begin` until `finish` (or until it's dropped).
pub struct Recorder {
    timing: Rc<RefCell<TurnTiming>>,
}

/// Start timing a turn; a turn already being timed is replaced.
pub fn begin() -> Recorder {
    let timing = Rc::new(RefCell::new(TurnTiming::default()));
    ACTIVE.with(|active| *active.borrow_mut() = Some(timing.clone()));
    Recorder { timing }
}

impl Recorder {
    /// The turn's numbers; timing stops.
    pub fn finish(self) -> TurnTiming {
        self.timing.borrow().clone()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            if active.as_ref().is_some_and(|current| Rc::ptr_eq(current, &self.timing)) {
                *active = None;
            }
        });
    }
}

/// Adds to the turn being timed; work done for something else (`background`) adds nothing.
fn with(add: impl FnOnce(&mut TurnTiming)) {
    if BACKGROUND.try_with(|_| ()).is_ok() {
        return;
    }
    ACTIVE.with(|active| {
        if let Some(timing) = active.borrow().as_ref() {
            add(&mut timing.borrow_mut());
        }
    });
}

/// One model call, from its request to the end of its stream.
pub struct ModelCall {
    began: Instant,
    first: Option<u64>,
}

pub fn model_call(model: &str) -> ModelCall {
    with(|timing| timing.model = Some(model.to_string()));
    ModelCall { began: Instant::now(), first: None }
}

impl ModelCall {
    /// Content arrived; the first sets the call's time to first part.
    pub fn part(&mut self) {
        if self.first.is_none() {
            self.first = Some(self.began.elapsed().as_millis() as u64);
        }
    }
}

impl Drop for ModelCall {
    fn drop(&mut self) {
        let total = self.began.elapsed().as_millis() as u64;
        let first = self.first;
        with(|timing| {
            timing.model_calls += 1;
            timing.model_ms += total;
            if let Some(first) = first {
                timing.first_part_ms.push(first);
            }
        });
    }
}

/// A model's parts, timed: the first content (text, reasoning or a tool call) sets the call's time to
/// first part, and the stream's end (its drop) the call's time in all. A reasoning item's start isn't
/// content: some providers send it before anything is thought.
pub fn timed(parts: StreamParts, mut call: ModelCall) -> StreamParts {
    use futures::StreamExt;
    Box::pin(parts.inspect(move |part| {
        if matches!(
            part,
            StreamPart::TextDelta { .. } | StreamPart::ReasoningDelta { .. } | StreamPart::ToolInputStart { .. } | StreamPart::ToolCall(_)
        ) {
            call.part();
        }
    }))
}

/// The effort and service tier a model call asks for, as its options carry them: `reasoningEffort`
/// (OpenAI's and the like, local models' too), `effort` (Anthropic's) or the call's own `reasoning`;
/// `serviceTier`. Called just before the request goes, once its provider has added what it adds.
pub fn effort(options: &CallOptions) {
    let find = |keys: &[&str]| {
        options
            .provider_options
            .as_ref()
            .and_then(|providers| providers.values().find_map(|options| keys.iter().find_map(|key| options.get(*key)?.as_str())))
            .map(str::to_string)
    };
    let effort = find(&["reasoningEffort", "effort"]).or_else(|| {
        options.reasoning.filter(|asked| *asked != Reasoning::ProviderDefault).and_then(|asked| json!(asked).as_str().map(str::to_string))
    });
    if let Some(effort) = effort {
        asked_effort(&effort);
    }
    if let Some(tier) = find(&["serviceTier"]) {
        with(|timing| timing.tier = Some(tier));
    }
}

/// The effort a model on this computer is asked for, which its provider adds as the request goes.
pub fn asked_effort(effort: &str) {
    with(|timing| timing.effort = Some(effort.to_string()));
}

/// One tool call's own time, by its tool's name.
pub fn tool_call(name: &str, elapsed_ms: u64) {
    with(|timing| {
        let (calls, ms) = timing.by_tool.entry(name.to_string()).or_default();
        *calls += 1;
        *ms += elapsed_ms;
    });
}

pub fn tool(elapsed_ms: u64) {
    with(|timing| {
        timing.tools += 1;
        timing.tool_ms += elapsed_ms;
    });
}

/// Calls that ran together: how many, and the time they took between them.
pub fn tools(count: u32, elapsed_ms: u64) {
    with(|timing| {
        timing.tools += count;
        timing.tool_ms += elapsed_ms;
    });
}

pub fn live_request() {
    with(|timing| timing.live_requests += 1);
}

/// Work done while a turn runs but not for it (FOCUS's and the transport clock's reads of Live, a
/// side question): its model calls, bytes and Live requests aren't the turn's.
pub async fn background<F: std::future::Future>(work: F) -> F::Output {
    BACKGROUND.scope((), work).await
}

pub fn sent(bytes: usize) {
    with(|timing| timing.sent_bytes += bytes as u64);
}

/// The turn being timed, for work it starts beside it that may end after it does.
pub fn current() -> Option<Turn> {
    ACTIVE.with(|active| active.borrow().clone()).map(Turn)
}
pub struct Turn(Rc<RefCell<TurnTiming>>);
impl Turn {
    /// The turn's look at an older Kumi's files took this long, beside it. It counts for this turn only:
    /// once the turn's line is written, for nothing.
    pub fn files(&self, ms: u64) {
        self.0.borrow_mut().files_ms = Some(ms);
    }
}

/// The log line for a finished turn; `stop` is how it ended (a `StopReason`, or "error").
pub fn line(timing: &TurnTiming, elapsed_ms: u64, stop: Value, usage: Option<&Usage>) -> Value {
    let mut line = json!({
        "at": iso_string(now_ms()),
        "kumi": KUMI_VERSION,
        "ms": elapsed_ms,
        "stop": stop,
        "modelCalls": timing.model_calls,
        "modelMs": timing.model_ms,
        "firstPartMs": timing.first_part_ms,
        "tools": timing.tools,
        "toolMs": timing.tool_ms,
        "liveRequests": timing.live_requests,
        "sentBytes": timing.sent_bytes,
    });
    if let Some(model) = &timing.model {
        line["model"] = json!(model);
    }
    if timing.model_calls > 0 {
        line["effort"] = json!(timing.effort.as_deref().unwrap_or("default"));
        if let Some(tier) = &timing.tier {
            line["tier"] = json!(tier);
        }
    }
    // The three tools that took longest, longest first: what a slow turn waited on.
    let mut slowest: Vec<_> = timing.by_tool.iter().collect();
    slowest.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    if !slowest.is_empty() {
        line["slowTools"] =
            json!(slowest.into_iter().take(3).map(|(tool, (calls, ms))| json!({"tool":tool,"calls":calls,"ms":ms})).collect::<Vec<_>>());
    }
    if let Some(ms) = timing.files_ms {
        line["filesMs"] = json!(ms);
    }
    if let Some(usage) = usage {
        line["inputTokens"] = json!(usage.input_tokens);
        line["cachedTokens"] = json!(usage.cache_read_tokens);
        line["outputTokens"] = json!(usage.output_tokens);
    }
    line
}

/// Append a line to the log; once it passes `MAX_BYTES`, keep its last `KEEP_LINES` (through a temporary
/// file, so the log is never left empty). Best effort: timing never gets in the way of a turn. Written in
/// place (one short line), so turns log in the order they end.
pub fn append(file: &Path, line: &Value) {
    let _ = write(file, json::stringify(line));
}

fn write(file: &Path, text: String) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(folder) = file.parent().filter(|folder| !folder.as_os_str().is_empty()) {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(folder)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(file)?.write_all(format!("{text}\n").as_bytes())?;
    if std::fs::metadata(file).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        let kept = std::fs::read_to_string(file)?;
        let lines: Vec<_> = kept.split('\n').filter(|line| !line.is_empty()).collect();
        let trimmed = file.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&trimmed, format!("{}\n", lines[lines.len().saturating_sub(KEEP_LINES)..].join("\n")))?;
        if let Err(error) = std::fs::rename(&trimmed, file) {
            let _ = std::fs::remove_file(&trimmed);
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_turn_adds_up_only_while_it_is_timed() {
        tool(5);
        live_request();
        let recorder = begin();
        {
            let mut call = model_call("openai/gpt-test");
            call.part();
            call.part();
        }
        tool(120);
        live_request();
        live_request();
        background(async {
            live_request();
            drop(model_call("openai/a-side-question"));
            sent(9999);
            tool(7);
        })
        .await;
        sent(2048);
        let timing = recorder.finish();
        assert_eq!(timing.model.as_deref(), Some("openai/gpt-test"));
        assert_eq!((timing.model_calls, timing.first_part_ms.len()), (1, 1));
        assert_eq!((timing.tools, timing.tool_ms), (1, 120));
        assert_eq!(timing.live_requests, 2, "FOCUS's own read is left out");
        assert_eq!(timing.model.as_deref(), Some("openai/gpt-test"), "a side question's call is left out");
        assert_eq!(timing.sent_bytes, 2048);
        // Nothing is timed after the turn.
        tool(9);
        let after = begin().finish();
        assert_eq!(after.tools, 0);
    }

    #[tokio::test]
    async fn a_turn_says_its_effort_its_tier_and_its_slowest_tools() {
        let asked = |provider: &str, options: Value| CallOptions {
            provider_options: Some(serde_json::Map::from_iter([(provider.to_string(), options)])),
            ..Default::default()
        };
        let recorder = begin();
        drop(model_call("openai-codex/gpt-test"));
        effort(&asked("openai", json!({"store":false,"reasoningEffort":"xhigh","serviceTier":"priority"})));
        effort(&CallOptions::default());
        for (name, ms) in
            [("live_discover", 40), ("watch_video", 31_000), ("make_changes", 900), ("live_discover", 60), ("search_web", 2_000)]
        {
            tool_call(name, ms);
        }
        background(async {
            tool_call("watch_video", 99_999);
            effort(&asked("anthropic", json!({"effort":"low"})));
        })
        .await;
        let line = line(&recorder.finish(), 40_000, json!("completed"), None);
        assert_eq!(
            (&line["effort"], &line["tier"]),
            (&json!("xhigh"), &json!("priority")),
            "a call asking nothing, or a side question's, leaves them"
        );
        assert_eq!(
            line["slowTools"],
            json!([{"tool":"watch_video","calls":1,"ms":31000},{"tool":"search_web","calls":1,"ms":2000},{"tool":"make_changes","calls":1,"ms":900}])
        );
        let turn = |calls: &[CallOptions]| {
            let recorder = begin();
            drop(model_call("test/model"));
            calls.iter().for_each(effort);
            tool_call("live_discover", 40);
            tool_call("live_discover", 60);
            super::line(&recorder.finish(), 500, json!("completed"), None)
        };
        let anthropic = turn(&[asked("anthropic", json!({"effort":"high"}))]);
        assert_eq!(anthropic["effort"], "high");
        assert_eq!(anthropic["slowTools"], json!([{"tool":"live_discover","calls":2,"ms":100}]));
        let own = turn(&[CallOptions { reasoning: Some(Reasoning::Minimal), ..Default::default() }]);
        assert_eq!(own["effort"], "minimal", "the call's own reasoning, when no provider option says");
        let left = turn(&[CallOptions { reasoning: Some(Reasoning::ProviderDefault), ..Default::default() }]);
        assert_eq!((&left["effort"], left.get("tier")), (&json!("default"), None));
        let quiet = super::line(&begin().finish(), 10, json!("completed"), None);
        assert!(quiet.get("effort").is_none() && quiet.get("slowTools").is_none(), "no model call, no effort: {quiet}");
    }

    #[test]
    fn a_turns_look_at_an_older_kumis_files_shows_as_its_own_time_on_that_turn_only() {
        let recorder = begin();
        current().unwrap().files(3);
        let line = line(&recorder.finish(), 2000, json!("completed"), None);
        assert_eq!((&line["filesMs"], &line["ms"]), (&json!(3), &json!(2000)));
        // A look that ends after its turn has: not the next turn's.
        let short = begin();
        let looking = current().unwrap();
        let first = super::line(&short.finish(), 10, json!("completed"), None);
        let next = begin();
        looking.files(5);
        assert!(first.get("filesMs").is_none() && super::line(&next.finish(), 10, json!("completed"), None).get("filesMs").is_none());
    }

    #[tokio::test]
    async fn the_first_part_is_the_first_content_not_the_streams_start() {
        use futures::{stream, StreamExt};
        let recorder = begin();
        let parts: StreamParts = Box::pin(
            stream::iter([
                StreamPart::StreamStart { warnings: Vec::new() },
                StreamPart::ResponseMetadata { id: None, timestamp: None, model_id: None },
            ])
            .chain(stream::once(async {
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                StreamPart::TextDelta { id: "t".into(), delta: "hi".into(), provider_metadata: None }
            })),
        );
        timed(parts, model_call("openai/gpt-test")).collect::<Vec<_>>().await;
        let timing = recorder.finish();
        assert_eq!(timing.model_calls, 1);
        assert!(timing.first_part_ms[0] >= 60, "{:?}", timing.first_part_ms);
    }
}
