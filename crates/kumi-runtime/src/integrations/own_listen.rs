//! (M5) Listening in a DAW whose bridge is Kumi's own and which hears through a hook of its own (Logic Pro's: a part
//! played and captured, or bounced, answered when it is done): the DAW gives `OwnDaw::listening`, a hook that builds,
//! over Kumi's link to the DAW's host (`ListenLink`: its calls, its refs, Kumi's changes and HISTORY), what the
//! listening tools meet the DAW through (a `Renderer`) and what the integration answers `hear` and `audio_file` with
//! (`OwnHearing`). With it, the integration offers the listening tools (judge, tune, sound, form, groove, audition,
//! render) and their engine follows the turns. REAPER and Renoise hear through the host's listening jobs instead
//! (`own_listening`).
use super::own_bridge::{answer_of, follow_epoch, stopped_or};
use super::own_changes::{Link, OwnPlan};
use crate::{
    core::{
        context::IntegrationContext,
        contracts::{ActionEvent, ChangeRecord, ChangeState, HearRequest, HeardTake, JsonObject, KernelTool, ToolResult},
        daw::DawProfile,
        errors::RuntimeError,
        plan::PlanHost,
        refs::RefScheme,
    },
    listening::{
        engine::Listening,
        renderer::Renderer,
        tools::{
            audition_tool, form_tool, groove_tool, judge_tool, render_tool, sound_tool, tune_tool, ToolDef, AUDITION_TOOL, FORM_TOOL,
            GROOVE_TOOL, JUDGE_TOOL, RENDER_TOOL, SOUND_TOOL, TUNE_TOOL,
        },
    },
};
use async_trait::async_trait;
use kumi_common::{
    abort::{self, Signal},
    js::string::head,
};
use serde_json::{json, Value};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
};

/// Why the DAW's host refused a call: its words, its code, whether the DAW is busy (a dialog, playing), and whether it
/// is there at all.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub why: String,
    pub code: Option<String>,
    pub busy: bool,
    pub there: bool,
}

/// What a DAW's listening reaches through the integration: the DAW's host, the turn's refs, and Kumi's changes in it.
#[async_trait(?Send)]
pub trait ListenLink {
    /// The DAW's words, and what it can do.
    fn profile(&self) -> &'static DawProfile;
    /// Whether Kumi is connected to the DAW now.
    fn connected(&self) -> bool;
    /// Ends when the integration closes.
    fn lifetime(&self) -> Signal;
    /// The project's tempo as the last look read it.
    fn tempo(&self) -> Option<f64>;
    /// The epoch the turn's refs are from (the latest any answer showed).
    fn epoch(&self) -> Option<String>;
    /// One of the host's tools, given `ms` to answer (a pass lasts as long as its part plays): its JSON, or why the host
    /// refused; Err when the host isn't there or the call was stopped.
    async fn call(&self, tool: &str, args: Value, ms: u64, signal: Signal) -> Result<Result<Value, Refusal>, RuntimeError>;
    /// A ref the model gave (this turn's short one) made long; anything else as it is.
    fn lengthen(&self, named: &str) -> String;
    /// A long ref as the model sees it this turn.
    fn short_ref(&self, long: &str) -> String;
    /// The DAW's words with its long refs in them made the turn's short ones.
    fn in_short_refs(&self, why: &str) -> String;
    /// One of the DAW's changes, by its tool's name and input, through Kumi's journal: kept in HISTORY as the model's
    /// are, so Kumi's undo takes it back. Its answer, or why it wasn't made.
    async fn change(&self, tool: &str, input: JsonObject, signal: Signal) -> Result<Value, String>;
    /// Kumi's changes in HISTORY now (applied ones), oldest first.
    fn applied(&self) -> Vec<String>;
    /// Those not in `mark`: ids and titles.
    fn applied_since(&self, mark: &[String]) -> Vec<(String, String)>;
    /// The tool that made a change (None: one HISTORY holds without its tool); None when HISTORY has no such change.
    fn made_by(&self, id: &str) -> Option<Option<String>>;
    /// How many times the DAW changed outside HISTORY (its own undo of Kumi's changes), and since a count, in words.
    fn outside(&self) -> usize;
    fn outside_since(&self, mark: usize) -> Vec<&'static str>;
    /// Kumi's undo of one change; why not, when it couldn't.
    async fn undo(&self, id: &str, signal: Signal) -> Result<(), String>;
    /// Puts a line in HISTORY (an audition heard).
    fn emit(&self, record: ChangeRecord);
    /// Says what Kumi is doing, as the app shows it; `playing` when the DAW plays or stops for it.
    fn tell(&self, title: &str, playing: Option<bool>);
    /// How long one change may take.
    fn change_ms(&self) -> u64;
}

/// What the integration answers through a DAW's listening, besides the listening tools.
#[async_trait(?Send)]
pub trait OwnHearing {
    /// Hears tracks or the mix as the listen tool asks (a file each), or why not.
    async fn hear(&self, request: &HearRequest, signal: Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError>;
    /// The audio file a name stands for (a region's, in the DAW's words); None when it names none.
    async fn audio_file(&self, named: &str, signal: Signal) -> Result<Option<String>, RuntimeError>;
    /// What a look read of the project (its tempo, its meter), kept for what the tools ask between looks.
    fn seen(&self, snapshot: &Value);
    /// A listening tool as the DAW describes it: the shared definition, or its own where it hears otherwise.
    fn describe(&self, tool: &str, shared: ToolDef) -> ToolDef {
        let _ = tool;
        shared
    }
    /// The integration closes: what the listening left behind goes (its folder).
    async fn close(&self) {}
}

/// A DAW's listening, as its hook builds it.
pub struct OwnEars {
    pub renderer: Rc<dyn Renderer>,
    pub hearing: Rc<dyn OwnHearing>,
}

/// Builds a DAW's listening over Kumi's link to its host.
pub type ListenHook = fn(Rc<dyn ListenLink>) -> OwnEars;

/// Kumi's link to the DAW's host as the listening reaches it: the integration's link and change engine, and where the app
/// is told what Kumi does.
pub(super) struct OwnListenLink<S: RefScheme + 'static> {
    pub(super) link: Rc<Link<S>>,
    pub(super) plan: Rc<OwnPlan<S>>,
    pub(super) on_action: Option<Rc<dyn Fn(ActionEvent)>>,
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> ListenLink for OwnListenLink<S> {
    fn profile(&self) -> &'static DawProfile {
        self.link.daw.profile
    }
    fn connected(&self) -> bool {
        self.link.connected.get()
    }
    fn lifetime(&self) -> Signal {
        self.link.lifetime.clone()
    }
    fn tempo(&self) -> Option<f64> {
        self.link.tempo.get()
    }
    fn epoch(&self) -> Option<String> {
        self.link.epoch.borrow().clone()
    }
    async fn call(&self, tool: &str, args: Value, ms: u64, signal: Signal) -> Result<Result<Value, Refusal>, RuntimeError> {
        let Some(endpoint) = self.link.endpoint.borrow().clone() else {
            return Err(RuntimeError::plain(self.link.daw.profile.vocab.cant_reach));
        };
        let args = args.as_object().cloned().unwrap_or_default();
        let bounded = abort::any([signal, self.link.lifetime.clone(), abort::timeout(ms)]);
        Ok(match stopped_or(endpoint.call(tool, args, bounded.clone()).await.map(|result| answer_of(&result)), &bounded)? {
            Ok(answer) => {
                // A track or region changed since the turn's refs were read: the book follows the answer's epoch.
                follow_epoch(&self.link.book, &self.link.epoch, &self.link.layout, &answer);
                Ok(answer)
            }
            Err(refused) => Err(Refusal { why: refused.why, code: refused.code, busy: refused.busy, there: refused.there }),
        })
    }
    fn lengthen(&self, named: &str) -> String {
        self.link.book.borrow().lengthen(&json!({"ref": named}))["ref"].as_str().unwrap_or(named).to_owned()
    }
    fn short_ref(&self, long: &str) -> String {
        self.link.book.borrow_mut().short_ref(long)
    }
    fn in_short_refs(&self, why: &str) -> String {
        let mut said = why.to_owned();
        let mut book = self.link.book.borrow_mut();
        for reference in book.scheme.refs_in(why) {
            let short = book.short_ref(reference);
            said = said.replace(reference, &short);
        }
        said
    }
    async fn change(&self, tool: &str, input: JsonObject, signal: Signal) -> Result<Value, String> {
        let Some(kind) = self.plan.change_kind(tool) else {
            return Err(format!("Kumi can't make {tool} changes in {}", self.link.daw.profile.words.app));
        };
        let outcome = PlanHost::change(&*self.plan, kind, input, signal, false).await;
        if outcome.is_error {
            return Err(outcome.text);
        }
        Ok(serde_json::from_str(&outcome.text).unwrap_or(Value::String(outcome.text)))
    }
    fn applied(&self) -> Vec<String> {
        let entries = self.plan.history.entries.borrow();
        entries.iter().filter(|(_, entry)| entry.borrow().record.state == ChangeState::Applied).map(|(id, _)| id.clone()).collect()
    }
    fn applied_since(&self, mark: &[String]) -> Vec<(String, String)> {
        let entries = self.plan.history.entries.borrow();
        entries
            .iter()
            .filter(|(id, entry)| !mark.contains(id) && entry.borrow().record.state == ChangeState::Applied)
            .map(|(id, entry)| (id.clone(), entry.borrow().record.title.clone()))
            .collect()
    }
    fn made_by(&self, id: &str) -> Option<Option<String>> {
        self.plan.history.entries.borrow().get(id).map(|entry| entry.borrow().tool.clone())
    }
    fn outside(&self) -> usize {
        self.plan.history.outside.borrow().len()
    }
    fn outside_since(&self, mark: usize) -> Vec<&'static str> {
        self.plan.history.outside_since(mark)
    }
    async fn undo(&self, id: &str, signal: Signal) -> Result<(), String> {
        match self.plan.history.undo(id, signal, false).await {
            Ok(undone) if !undone.is_error => Ok(()),
            Ok(undone) => Err(head(&undone.text, 120)),
            Err(error) => Err(head(&error.to_string(), 120)),
        }
    }
    fn emit(&self, record: ChangeRecord) {
        self.plan.history.emit(&record);
    }
    fn tell(&self, title: &str, playing: Option<bool>) {
        if let Some(tell) = &self.on_action {
            let _ = catch_unwind(AssertUnwindSafe(|| tell(ActionEvent { title: title.into(), playing, recording: None })));
        }
    }
    fn change_ms(&self) -> u64 {
        self.plan.history.timeout_ms
    }
}

/// The listening tools, in the order Live offers them: the audition goes with every request, the rest wait behind
/// find_tools.
const TOOLS: [&str; 7] = [AUDITION_TOOL, JUDGE_TOOL, TUNE_TOOL, GROOVE_TOOL, FORM_TOOL, SOUND_TOOL, RENDER_TOOL];

/// A DAW's listening in its integration: the tools' engine over its `Renderer`, what answers `hear` and `audio_file`,
/// the link it reaches the DAW through, and the tools as the DAW describes them.
pub(super) struct HookListening {
    pub(super) engine: Rc<Listening>,
    pub(super) hearing: Rc<dyn OwnHearing>,
    link: Rc<dyn ListenLink>,
    tools: Vec<(&'static str, ToolDef)>,
}

impl HookListening {
    /// The DAW's listening, built by its hook over `link`, its engine told what the app follows (rounds, auditions).
    pub(super) fn new(hook: ListenHook, link: Rc<dyn ListenLink>, context: &IntegrationContext) -> Self {
        let ears = hook(link.clone());
        let engine = Listening::new(
            ears.renderer,
            context.on_judge.clone(),
            context.on_audition.clone(),
            context.listener.clone(),
            context.references.clone(),
        );
        let profile = link.profile();
        let tools = TOOLS
            .iter()
            .map(|name| {
                let shared = match *name {
                    AUDITION_TOOL => audition_tool(profile),
                    JUDGE_TOOL => judge_tool(profile),
                    TUNE_TOOL => tune_tool(profile),
                    GROOVE_TOOL => groove_tool(profile),
                    FORM_TOOL => form_tool(profile),
                    SOUND_TOOL => sound_tool(profile),
                    _ => render_tool(profile),
                };
                (*name, ears.hearing.describe(name, shared))
            })
            .collect();
        Self { engine, hearing: ears.hearing, link, tools }
    }

    /// The listening tools, each run by the engine.
    pub(super) fn tools(&self) -> Vec<Rc<dyn KernelTool>> {
        self.tools
            .iter()
            .map(|(name, def)| Rc::new(ListeningTool { engine: self.engine.clone(), name, def: def.clone() }) as Rc<dyn KernelTool>)
            .collect()
    }

    /// A new answer: a judged run's changes count from the changes applied now (a match run's next round goes on).
    pub(super) fn new_turn(&self, continuing: bool) {
        if !continuing {
            self.engine.new_answer(self.link.applied());
        }
    }

    /// The producer's words came into the answer under way: what changes from here may be theirs.
    pub(super) fn steered(&self) {
        self.engine.steered(self.link.applied());
    }
}

/// One of the listening tools, as the DAW describes it.
struct ListeningTool {
    engine: Rc<Listening>,
    name: &'static str,
    def: ToolDef,
}

#[async_trait(?Send)]
impl KernelTool for ListeningTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        &self.def.description
    }
    fn input_schema(&self) -> JsonObject {
        self.def.schema.clone()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        match self.engine.call(self.name, input, signal).await {
            Some(result) => result,
            None => Ok(ToolResult::error(format!("{} isn't a listening tool", self.name))),
        }
    }
    /// The audition goes with every request, as Live's does; the rest wait behind find_tools.
    fn deferred(&self) -> bool {
        self.name != AUDITION_TOOL
    }
}
