//! The complete Live integration: shared session state, public tools and shutdown ordering.
use super::{
    actions::ACTIONS,
    arrange, audition,
    bridge_version::*,
    catalog,
    changes::CHANGES,
    command_tools::{CommandTools, CommandToolsOptions, SET_OPENING_MS},
    connection::LiveConnection,
    context::{object, payload},
    history::{result_text, History},
    instant, judge_tool, live_command,
    mutations::Mutations,
    observation::{ObservationHost, ObservedChange, Observer},
    options::{AbletonOptions, HandsSetup},
    parameters::Parameters,
    plugin_tool, project,
    remember::Remember,
    rendering::Rendering,
    samples,
    variations::LiveVariations,
    views,
    views::ViewHost,
    watch::Watch,
};
use crate::devices::tool::{device_tool, DeviceToolOptions, Made};
use crate::history::LiveHistory;
use async_trait::async_trait;
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use kumi_ai::timing;
use kumi_common::{
    abort::{self, Signal},
    js::{
        json::stringify,
        number::round,
        string::{head, utf16_len},
    },
};
use kumi_runtime::{
    core::{contracts::*, variations::tool::Variations},
    integrations::shared::steps,
    mcp::allowed_tools::CallOptions,
};
use kumi_types::errors::{FailureKind, KumiError, RuntimeError};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
    sync::LazyLock,
    time::Duration,
};

static DATA: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/public-tools.json")).unwrap());
type Closing = Shared<LocalBoxFuture<'static, Result<(), RuntimeError>>>;
pub struct Ableton {
    weak: Weak<Self>,
    pub connection: Rc<LiveConnection>,
    pub history: Rc<History>,
    pub observer: Rc<Observer>,
    pub mutations: Rc<Mutations>,
    pub rendering: Rc<Rendering>,
    commands: CommandTools,
    watch: Watch,
    options: Rc<AbletonOptions>,
    closing: RefCell<Option<Closing>>,
    /// The devices this conversation made, which making one again by its name replaces.
    made: Rc<RefCell<Made>>,
    /// Core's variations tool, with Live's backend: Session clips' takes.
    variations: Rc<dyn KernelTool>,
}
pub fn create_ableton_integration(options: AbletonOptions) -> Rc<dyn Integration> {
    Ableton::new(options)
}
/// Kumi's own folder: KUMI_HOME, or ~/.kumi.
fn kumi_home() -> std::path::PathBuf {
    std::env::var("KUMI_HOME")
        .ok()
        .filter(|home| !home.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home::home_dir().unwrap_or_default().join(".kumi"))
}
/// Where a device Kumi made is kept when making it again replaces it: in Kumi's own folder, out of Live's Browser.
fn replaced_devices() -> std::path::PathBuf {
    kumi_home().join("devices").join("replaced")
}
impl Ableton {
    pub fn new(options: AbletonOptions) -> Rc<Self> {
        let options = Rc::new(options);
        Rc::new_cyclic(|weak: &Weak<Self>| {
            let mut config = options.connection_options();
            let owner = weak.clone();
            config.on_retire = Some(Rc::new(move |note| {
                if let Some(owner) = owner.upgrade() {
                    owner.history.retire(note);
                }
            }));
            let owner = weak.clone();
            config.on_live_lost = Some(Rc::new(move || {
                if let Some(owner) = owner.upgrade() {
                    owner.rendering.reset_live();
                }
            }));
            let connection = LiveConnection::new(config);
            let remember = Remember::new(connection.clone(), options.project_store.clone(), options.on_catch_up.clone());
            let history = Rc::new(History::new(connection.clone(), remember.clone(), options.change_timeout_ms, options.on_change.clone()));
            let observer = Rc::new(Observer::new(connection.clone(), remember.clone()));
            let shown = Rc::downgrade(&observer);
            *history.on_shift.borrow_mut() = Some(Rc::new(move |shift| {
                if let Some(observer) = shown.upgrade() {
                    match shift {
                        Some(shift) => observer.shifted(shift),
                        None => observer.forget_devices(),
                    }
                }
            }));
            let parameters = Rc::new(Parameters::new(history.clone(), options.fast));
            let made = Rc::new(RefCell::new(Made::keeping(replaced_devices())));
            let mut mutations = Mutations::new(parameters, observer.clone(), options.clone());
            mutations.made = made.clone();
            let mutations = Rc::new(mutations);
            let step = mutations.clone();
            let clip = mutations.clone();
            let rendering = Rendering::new(
                history.clone(),
                observer.clone(),
                &options,
                Rc::new(move |name, args, signal| {
                    let step = step.clone();
                    async move { step.step(&name, args, signal).await }.boxed_local()
                }),
                Rc::new(move |name, signal| {
                    let clip = clip.clone();
                    async move { clip.clip_file(&name, signal).await }.boxed_local()
                }),
            );
            let action = mutations.clone();
            let commands = CommandTools::new(
                connection.clone(),
                history.clone(),
                remember,
                CommandToolsOptions {
                    hands: options.hands.as_ref().map(|hands| match hands {
                        HandsSetup::Disabled => HandsSetup::Disabled,
                        HandsSetup::Open(open) => HandsSetup::Open(open.clone()),
                    }),
                    user_library: options.user_library.clone(),
                    on_action: options.on_action.clone(),
                    front_live: None,
                },
                Rc::new(move |name, args, signal| {
                    let action = action.clone();
                    async move {
                        let kind = ACTIONS.iter().find(|kind| kind.tool == name).ok_or_else(|| RuntimeError::plain("Unknown action"))?;
                        let result = action.act(kind, args, signal, false).await;
                        Ok(ToolResult { text: result.text, is_error: result.is_error, ..Default::default() })
                    }
                    .boxed_local()
                }),
            );
            let watch = Watch::new(mutations.clone());
            let variations =
                Variations::new(LiveVariations::new(connection.clone(), mutations.clone(), history.clone(), observer.clone()), kumi_home())
                    .tool();
            Self {
                weak: weak.clone(),
                connection,
                history,
                observer,
                mutations,
                rendering,
                commands,
                watch,
                options,
                closing: RefCell::new(None),
                made,
                variations,
            }
        })
    }
    fn combined(&self, signal: Signal) -> Signal {
        abort::any([signal, self.connection.lifetime.clone()])
    }
    fn tool(&self, name: &str, description: &str, schema: JsonObject) -> Rc<dyn KernelTool> {
        Rc::new(LiveTool {
            owner: self.weak.upgrade().unwrap(),
            name: name.into(),
            description: description.into(),
            schema,
            deferred: catalog::deferred(name),
            listed_by: None,
        })
    }
    /// A change or action: with make_changes offered, a step it lists with its input.
    fn step_tool(&self, name: &str, description: &str, schema: JsonObject, planned: bool) -> Rc<dyn KernelTool> {
        Rc::new(LiveTool {
            owner: self.weak.upgrade().unwrap(),
            name: name.into(),
            description: description.into(),
            schema,
            deferred: catalog::deferred(name),
            listed_by: planned.then_some(catalog::PLAN_TOOL),
        })
    }
    fn static_tool(&self, name: &str) -> Rc<dyn KernelTool> {
        self.tool(name, DATA[name]["description"].as_str().unwrap(), DATA[name]["schema"].as_object().unwrap().clone())
    }
    async fn find_sounds(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let named: Vec<_> = input.get("folders").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        let folders: Option<Vec<_>> = named.iter().map(|name| samples::folder_path(name, None)).collect();
        let Some(mut folders) = folders else {
            return Ok(ToolResult::error("Name folders by their full path, such as ~/Samples or /Users/me/Music/Drums."));
        };
        let default = folders.is_empty();
        if default {
            folders = samples::default_sample_folders(None, None, None);
        }
        let words =
            input.get("words").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
        let limit = input
            .get("limit")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && n.fract() == 0.)
            .map(|n| n.clamp(1., 50.) as usize)
            .unwrap_or(20);
        let found = samples::find_samples(samples::FindSamplesOptions {
            folders,
            words,
            limit,
            random: input.get("random") == Some(&json!(true)),
            signal: Some(signal),
        })
        .await?;
        let mut bank = self.mutations.samples.samples.borrow_mut();
        for sample in &found.samples {
            bank.shift_remove(&sample.path);
            bank.insert(sample.path.clone(), sample.clone());
        }
        while bank.len() > 5000 {
            bank.shift_remove_index(0);
        }
        let listed: Vec<_> = found
            .samples
            .iter()
            .map(|s| {
                let mut row = object(&json!({"name":s.name,"path":s.path})).unwrap();
                if let Some(seconds) = s.seconds {
                    row.insert("seconds".into(), json!(seconds));
                }
                row.insert("kb".into(), json!(round(s.bytes as f64 / 1024.)));
                row
            })
            .collect();
        let mut result = object(&json!({"samples":listed,"matched":found.matched,"looked":found.scanned}))?;
        if found.partial {
            result.insert("partial".into(), json!(true));
        }
        if !found.missing.is_empty() {
            result.insert("missing".into(), json!(found.missing));
        }
        if default {
            result.insert("searched".into(), json!("the User Library, Live's Core Library and Factory Packs"));
        }
        Ok(ToolResult::text(stringify(&json!(result))))
    }
    async fn live_undo(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let redo = input.get("redo") == Some(&json!(true));
        let result = self
            .connection
            .call(
                if redo { "live_song_redo" } else { "live_song_undo" },
                object(
                    &json!({"confirmation":if redo{"redo-in-live"}else{"undo-in-live"},"idempotencyKey":uuid::Uuid::new_v4().to_string()}),
                )?,
                self.combined(signal),
            )
            .await?;
        if result.is_error == Some(true) {
            return Ok(ToolResult::error(result_text(&result)));
        }
        let done = payload(&result)?;
        let title = if done.get("done") == Some(&json!(true)) {
            if redo {
                "Redid in Live"
            } else {
                "Undid in Live"
            }
        } else if redo {
            "Nothing to redo in Live"
        } else {
            "Nothing to undo in Live"
        };
        if let Some(listener) = &self.options.on_action {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener(ActionEvent { title: title.into(), playing: None, recording: None })
            }));
        }
        Ok(ToolResult::text(stringify(&json!(done))))
    }
    async fn run_python(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let args = object(&self.connection.references.borrow().lengthen(&json!(input)))?;
        if let Err(error) = self.connection.references.borrow().require_fresh_references(&args) {
            return Ok(ToolResult::error(error.to_string()));
        }
        let result = self.connection.call("live_run_python", args, self.combined(signal)).await;
        {
            let mut refs = self.connection.references.borrow_mut();
            refs.invalidate();
            refs.clear_names();
        }
        self.connection.lease.set(self.connection.lease.get() + 1);
        let result = result?;
        if result.is_error == Some(true) {
            return Ok(ToolResult::error(result_text(&result)));
        }
        let done = payload(&result)?;
        fn register(value: &Value, refs: &mut super::references::References) {
            static LIVE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+:([a-z][a-z_]{0,31}):").unwrap());
            match value {
                Value::Array(items) => {
                    for item in items {
                        register(item, refs)
                    }
                }
                Value::Object(row) => {
                    if let Some(reference) = row.get("ref").and_then(Value::as_str).filter(|r| utf16_len(r) <= 256) {
                        if let Some(kind) =
                            LIVE_REF.captures(reference).filter(|_| row.get("type").is_some_and(Value::is_string)).map(|m| m[1].to_owned())
                        {
                            refs.refs.insert(reference.into(), if kind == "clip" { "session-clip".into() } else { kind.replace('_', "-") });
                            if kind == "track" {
                                if let Some(name) = row.get("name").and_then(Value::as_str) {
                                    refs.known.insert(reference.into(), TrackChip { name: head(name, 256), color: None });
                                }
                            }
                        }
                    }
                    for child in row.values() {
                        register(child, refs);
                    }
                }
                _ => {}
            }
        }
        let mut refs = self.connection.references.borrow_mut();
        if let Some(value) = done.get("result") {
            register(value, &mut refs);
        }
        let is_error = done.get("ok") != Some(&json!(true));
        Ok(ToolResult { text: stringify(&refs.shorten(&json!(done))), is_error, ..Default::default() })
    }
}

struct LiveTool {
    owner: Rc<Ableton>,
    name: String,
    description: String,
    schema: JsonObject,
    /// Not sent with every request (catalog.rs).
    deferred: bool,
    /// make_changes, which lists this change or action as a step with its input.
    listed_by: Option<&'static str>,
}
/// Live tools that only read Kumi's own library: they don't wait for Live.
const AWAY_FROM_LIVE: &[&str] = &["find_sounds"];

#[async_trait(?Send)]
impl KernelTool for LiveTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn reach(&self) -> Reach {
        if AWAY_FROM_LIVE.contains(&self.name.as_str()) {
            Reach::AwayFromDaw
        } else if self.name == "read_notes" || kumi_runtime::mcp::allowed_tools::MODEL_TOOLS.contains(&self.name.as_str()) {
            Reach::DawRead
        } else {
            Reach::DawWrite
        }
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        self.schema.clone()
    }
    fn stream(&self, signal: Signal, on_start: Rc<dyn Fn()>) -> Option<Box<dyn StreamingCall>> {
        // While a stopped answer's call is still putting Live back, changes wait for it in execute: none stream.
        (self.name == "make_changes" && !self.owner.rendering.is_busy()).then(|| self.owner.mutations.stream_changes(signal, on_start))
    }
    fn deferred(&self) -> bool {
        self.deferred
    }
    fn listed_by(&self) -> Option<&str> {
        self.listed_by
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let owner = &self.owner;
        // One Live tool call at a time, across answers too: a stopped answer's call still putting Live back (taking a
        // round back, closing a render) finishes before this one starts. Reading Kumi's library, away from Live, needs
        // no turn (several such calls run together).
        let _live = match AWAY_FROM_LIVE.contains(&self.name.as_str()) {
            true => None,
            false => Some(owner.rendering.hold(&signal).await?),
        };
        if let Some(kind) = CHANGES.iter().find(|k| k.tool == self.name) {
            let out = owner.mutations.change(kind, input, signal, false).await;
            return Ok(ToolResult { text: out.text, is_error: out.is_error, ..Default::default() });
        }
        if let Some(kind) = ACTIONS.iter().find(|k| k.tool == self.name) {
            let out = owner.mutations.act(kind, input, signal, false).await;
            return Ok(ToolResult { text: out.text, is_error: out.is_error, ..Default::default() });
        }
        // What may change any device without Live telling (Python run in Live, a command, a plug-in, an undo): the
        // next turn reads them all.
        let unseen = matches!(self.name.as_str(), "run_python" | "live_command" | "plugin" | "undo_change" | "undo_in_live");
        // What changes Live outside HISTORY: a judged round can't take it back, and says so.
        let command = input.get("command").and_then(Value::as_str).unwrap_or("");
        let outside = match self.name.as_str() {
            "run_python" => Some("Python run in Live"),
            "live_command" if !["save", "collect_all_and_save", "save_as", "export_audio", "export_midi_clip"].contains(&command) => {
                Some("a command of Live's menus")
            }
            "undo_in_live" => Some("Live's own undo"),
            _ => None,
        };
        if let Some(what) = outside {
            owner.history.outside.borrow_mut().push(what);
        }
        let result = match self.name.as_str() {
            "find_sounds" => owner.find_sounds(input, signal).await,
            "read_notes" => Ok(super::notes::read_notes(&input, &owner.connection, owner.observer.tempo.get(), signal).await),
            "make_changes" => owner.mutations.make_changes(input, signal).await,
            "watch_me" => owner.watch.execute(input, signal).await,
            "arrange" => arrange::arrange(input, &owner.mutations.arrange_host(), owner.combined(signal)).await,
            "undo_change" => kumi_runtime::integrations::shared::undo::undo_change(&owner.history, &input, signal).await,
            "audition" | "judge" | "tune" | "groove" | "form" | "sound" | "render" => {
                match owner.rendering.call(&self.name, input, signal).await {
                    Some(result) => result,
                    None => unreachable!("a listening tool"),
                }
            }
            "undo_in_live" => owner.live_undo(input, signal).await,
            "run_python" => owner.run_python(input, signal).await,
            "live_command" => {
                let finishing = input.get("final") == Some(&json!(true));
                match owner.commands.live_command(&input, signal.clone()).await {
                    // An answer that ends here has no use for a look at the Set it opened: the next request takes its own.
                    Ok(result) if finishing => Ok(finished_command(result, true)),
                    Ok(result) => Ok(finished_command(owner.look_in_opened_set(result, signal).await, false)),
                    other => other,
                }
            }
            "plugin" => owner.commands.plugin_tool(&input, signal).await,
            name if name == owner.variations.name() => owner.variations.execute(input, signal).await,
            _ => Ok(owner.connection.invoke(&self.name, input, signal).await),
        };
        if unseen {
            owner.observer.forget_devices();
        }
        result
    }
}
impl Ableton {
    /// After Live opened the Set this request asked for (new_set, open_set): a look at it now, as a turn's start
    /// takes, so the request carries on in it with current references. Every Live tool used to be refused until the
    /// producer's next message, so "start a new Set and save it as …" took two requests (#254). The look comes back
    /// with the command's answer; while Live is still loading the Set, it's tried again for a few seconds.
    pub async fn look_in_opened_set(&self, mut result: ToolResult, signal: Signal) -> ToolResult {
        let Ok(Value::Object(mut answer)) = serde_json::from_str::<Value>(&result.text) else { return result };
        if result.is_error || !(answer.contains_key("opened") || answer.contains_key("openedSet")) {
            return result;
        }
        for attempt in 0..6 {
            if attempt > 0 {
                tokio::select! { _ = signal.cancelled() => return result, _ = tokio::time::sleep(std::time::Duration::from_millis(1000)) => {} }
            }
            let Ok(observation) = self.observer.observe(self, signal.clone(), None).await else { continue };
            let Ok(context) = serde_json::from_str::<JsonObject>(&observation.context) else { continue };
            if observation.revision.as_deref() == Some("no-live") || !context.contains_key("set") {
                continue;
            }
            let mut now: JsonObject = ["set", "tracks", "folded", "moreTracks", "moreDevices"]
                .iter()
                .filter_map(|key| Some(((*key).into(), context.get(*key)?.clone())))
                .collect();
            // A big Set's tracks don't fit beside a command's answer: discovery reads them.
            if stringify(&Value::Object(now.clone())).len() > 24 * 1024 {
                now.retain(|key, _| key == "set");
                now.insert("tracks".into(), json!("More than fit here: discover them"));
            }
            answer.insert("now".into(), Value::Object(now));
            answer.insert(
                "note".into(),
                json!("Live has the Set open and Kumi read it: now is the Set as it is, with current references. Carry on in it."),
            );
            result.text = stringify(&Value::Object(answer));
            return result;
        }
        result
    }
}
/// A command that finishes the request on its own (a save, a new or opened Set), said by Kumi when the model gave
/// final: true, so no model call follows only to say "Saved." (#254). A dialog or question left to answer isn't
/// finished.
fn finished_command(mut result: ToolResult, finishing: bool) -> ToolResult {
    if !finishing || result.is_error {
        return result;
    }
    let Ok(answer) = serde_json::from_str::<JsonObject>(&result.text) else { return result };
    if answer.contains_key("dialog") || answer.contains_key("next") || answer.get("cancelled") == Some(&json!(true)) {
        return result;
    }
    let file =
        |path: &str| std::path::Path::new(path).file_name().map_or_else(|| path.to_owned(), |name| name.to_string_lossy().into_owned());
    let said = if let Some(saved) = answer.get("saved").and_then(Value::as_str) {
        if saved == "where it is" {
            "Saved the Set.".to_owned()
        } else {
            format!("Saved the Set as {}.", file(saved))
        }
    } else if let Some(opened) = answer.get("openedSet").and_then(Value::as_str) {
        format!("Opened {}.", file(opened))
    } else if answer.contains_key("opened") {
        "Opened a new Set.".to_owned()
    } else if let Some(pressed) = answer.get("pressed").and_then(Value::as_str) {
        format!("Done in Live: {pressed}.")
    } else {
        return result;
    };
    result.reply = Some(said);
    result
}
#[async_trait(?Send)]
impl ObservationHost for Ableton {
    fn reset_turn(&self, continuing: bool) {
        self.history.changes_this_turn.set(0);
        self.mutations.samples.picked.borrow_mut().clear();
        self.mutations.names.borrow_mut().clear();
        self.rendering.reset_turn(continuing);
    }
    fn changes(&self) -> Vec<ObservedChange> {
        self.history.observed()
    }
    fn definitions(&self) -> Vec<Rc<dyn KernelTool>> {
        let Some(tools) = self.connection.tools() else { return vec![] };
        let mut offered: Vec<_> = tools
            .list()
            .iter()
            .map(|t| {
                let mut schema = object(&json!(t.input_schema)).unwrap();
                if t.name == "live_discover" {
                    // Notes are read with read_notes, as notation: discovery doesn't offer them.
                    if let Some(kinds) = schema
                        .get_mut("properties")
                        .and_then(|p| p.get_mut("kind"))
                        .and_then(|k| k.get_mut("enum"))
                        .and_then(Value::as_array_mut)
                    {
                        kinds.retain(|kind| kind != "note");
                    }
                }
                self.tool(&t.name, t.description.as_deref().unwrap_or("Read current Live state"), schema)
            })
            .collect();
        offered.push(self.static_tool("find_sounds"));
        if tools.has("live_discover") {
            offered.push(self.static_tool("read_notes"));
        }
        if tools.has("live_browser_inspect") && tools.has("live_browser_load_preview") {
            let owner = self.weak.upgrade().unwrap();
            offered.push(device_tool(DeviceToolOptions {
                user_library: self.options.user_library.clone().unwrap_or_else(|| samples::user_library(None, None)),
                wait_ms: None,
                made: self.made.clone(),
                browser_sees: Rc::new(move |item, signal| {
                    let owner = owner.clone();
                    async move {
                        let Some(tools) = owner.connection.tools() else { return Ok(false) };
                        Ok(tools
                            .call("live_browser_inspect", object(&json!({"itemId":item}))?, signal, CallOptions::default())
                            .await
                            .is_ok_and(|r| r.is_error != Some(true)))
                    }
                    .boxed_local()
                }),
            }));
        }
        // make_changes runs every change and action as a step; those that aren't tools of their own get a line each in
        // its description, with their input.
        let planned = tools.has("live_undo");
        let mut steps: Vec<String> = Vec::new();
        let mut edits = Vec::new();
        for kind in CHANGES.iter().filter(|k| {
            k.internal != Some(true)
                && self.mutations.supported(k.since.as_deref())
                && ((k.always == Some(true) && k.input_schema.is_some() && tools.has("live_undo")) || k.available(|tool| tools.has(tool)))
        }) {
            let preview = tools.tool(&kind.preview).map(|t| object(&json!(t.input_schema)).unwrap());
            let schema = if kind.fallback_schema == Some(true) {
                preview.or_else(|| kind.input_schema.clone()).unwrap_or_default()
            } else {
                kind.input_schema.clone().unwrap_or_else(|| kind.schema(&preview.unwrap_or_default()))
            };
            edits.push(kind.tool.clone());
            if !kind.core {
                steps.push(catalog::step_line(&kind.tool, &kind.description, &schema));
            }
            offered.push(self.step_tool(&kind.tool, &kind.description, schema, planned));
        }
        let mut actions = Vec::new();
        for kind in ACTIONS.iter().filter(|k| self.mutations.supported(k.since.as_deref()) && tools.has(&k.preview) && tools.has(&k.apply))
        {
            actions.push(kind.tool.clone());
            let schema =
                kind.input_schema.clone().unwrap_or_else(|| object(&json!(tools.tool(&kind.preview).unwrap().input_schema)).unwrap());
            if !kind.core {
                steps.push(catalog::step_line(&kind.tool, &kind.description, &schema));
            }
            offered.push(self.step_tool(&kind.tool, &kind.description, schema, planned));
        }
        if planned && !edits.is_empty() {
            edits.extend(actions);
            let base = &DATA["make_changes"];
            let (description, schema) = steps::plan_tool(base["description"].as_str().unwrap(), &base["schema"], edits, &steps);
            offered.push(self.tool("make_changes", &description, schema));
        }
        if ["live_undo", "live_clip_duplicate_preview", "live_clip_duplicate_apply"].iter().all(|n| tools.has(n)) {
            offered.push(self.tool(arrange::ARRANGE_TOOL, &arrange::ARRANGE_DESCRIPTION, arrange::ARRANGE_SCHEMA.clone()));
        }
        if tools.has("live_undo") {
            offered.push(self.static_tool("undo_change"));
        }
        if ["live_project_snapshot_export", "live_project_snapshot_diff"].iter().all(|n| tools.has(n)) {
            offered.push(self.static_tool("watch_me"));
        }
        if self.mutations.supported(Some(RENDER_BRIDGE)) && tools.has("live_undo") && tools.has("live_recording_preview") {
            offered.push(self.tool(audition::AUDITION_TOOL, &audition::AUDITION_DESCRIPTION, audition::AUDITION_SCHEMA.clone()));
            offered.push(self.tool(judge_tool::JUDGE_TOOL, &judge_tool::JUDGE_DESCRIPTION, judge_tool::JUDGE_SCHEMA.clone()));
            offered.push(self.tool(judge_tool::TUNE_TOOL, &judge_tool::TUNE_DESCRIPTION, judge_tool::TUNE_SCHEMA.clone()));
            offered.push(self.tool(judge_tool::GROOVE_TOOL, &judge_tool::GROOVE_DESCRIPTION, judge_tool::GROOVE_SCHEMA.clone()));
            offered.push(self.tool(judge_tool::FORM_TOOL, &judge_tool::FORM_DESCRIPTION, judge_tool::FORM_SCHEMA.clone()));
            offered.push(self.tool(judge_tool::SOUND_TOOL, &judge_tool::SOUND_DESCRIPTION, judge_tool::SOUND_SCHEMA.clone()));
        }
        if self.mutations.supported(Some(FULL_CONTROL_BRIDGE)) && tools.has("live_render_offline") {
            offered.push(self.tool(audition::RENDER_TOOL, &audition::RENDER_DESCRIPTION, audition::RENDER_SCHEMA.clone()));
        }
        if self.mutations.supported(Some(FULL_CONTROL_BRIDGE)) && tools.has("live_song_undo") && tools.has("live_song_redo") {
            offered.push(self.static_tool("undo_in_live"));
        }
        if self.mutations.supported(Some(PYTHON_BRIDGE)) && tools.has("live_run_python") {
            offered.push(self.static_tool("run_python"));
        }
        if !matches!(self.options.hands, Some(HandsSetup::Disabled))
            && (self.options.hands.is_some() || cfg!(any(target_os = "macos", target_os = "windows")))
            && tools.has("live_selection_preview")
        {
            offered.push(self.tool(
                live_command::LIVE_COMMAND_TOOL,
                &live_command::LIVE_COMMAND_DESCRIPTION,
                live_command::LIVE_COMMAND_SCHEMA.clone(),
            ));
        }
        if tools.has("live_device_read") {
            offered.push(self.tool(plugin_tool::PLUGIN_TOOL, &plugin_tool::PLUGIN_DESCRIPTION, plugin_tool::PLUGIN_SCHEMA.clone()));
        }
        // Takes of a Session clip: read with read_notes, written, renamed and deleted as changes.
        let change = |name: &str| {
            CHANGES.iter().any(|k| {
                k.tool == name
                    && k.internal != Some(true)
                    && self.mutations.supported(k.since.as_deref())
                    && k.available(|tool| tools.has(tool))
            })
        };
        if tools.has("live_discover")
            && ["write_midi_clip", "delete_clip", "rename", "add_tracks_and_scenes"].iter().all(|name| change(name))
        {
            let variations = &self.variations;
            offered.push(self.tool(variations.name(), variations.description(), variations.input_schema()));
        }
        offered
    }
    async fn restore_after_crash(&self, identity: &str, path: Option<&str>, signal: Signal) -> Result<Option<String>, RuntimeError> {
        self.rendering.restore_after_crash(identity, path, signal).await
    }
}
#[async_trait(?Send)]
impl Integration for Ableton {
    fn profile(&self) -> &DawProfile {
        &DawProfile::ABLETON
    }
    async fn start(&self, signal: Signal) -> Result<(), RuntimeError> {
        self.connection.start(signal).await
    }
    async fn settled(&self) {
        self.rendering.settled().await
    }
    fn steered(&self) {
        self.rendering.steered();
    }
    fn conversation(&self, id: &str) {
        self.made.borrow_mut().conversation(id);
    }
    /// Willington's words, as its switch stands now (read again for each new model session).
    fn instructions_addendum(&self, offered: &dyn Fn(&str) -> bool) -> String {
        super::willington::willington_instructions(self.options.willington.as_ref().and_then(|on| on()), offered).to_string()
    }
    fn worker_tools(&self) -> Vec<Rc<dyn KernelTool>> {
        let Some(tools) = self.connection.tools() else { return vec![] };
        if !(tools.has("live_browser_inspect") && tools.has("live_browser_load_preview")) {
            return vec![];
        }
        // The file only: the conversation that loads it finds it in Live's Browser then.
        vec![device_tool(DeviceToolOptions {
            user_library: self.options.user_library.clone().unwrap_or_else(|| samples::user_library(None, None)),
            wait_ms: Some(0),
            made: self.made.clone(),
            browser_sees: Rc::new(|_, _| async { Ok(false) }.boxed_local()),
        })]
    }
    async fn observe(&self, signal: Signal, hints: Option<ObserveHints>) -> Result<Observation, RuntimeError> {
        self.observer.observe(self, signal, hints).await
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        let pending = self.closing.borrow().clone();
        let closing = if let Some(pending) = pending {
            pending
        } else {
            self.connection.prepare_close();
            self.history.remember.cancel_timer();
            let owner = self.weak.upgrade().unwrap();
            let closing = async move {
                if owner.history.remember.current().is_some_and(|p| p.path.as_ref().is_some_and(|p| !p.is_empty()))
                    && owner.options.project_store.is_some()
                    && owner.connection.available.get()
                    && !owner.connection.lost.get()
                {
                    let _ = tokio::time::timeout(Duration::from_millis(2500), owner.history.remember.save_now(Some(2000))).await;
                }
                let closing = owner.connection.close();
                owner.rendering.close().await;
                closing.await
            }
            .boxed_local()
            .shared();
            *self.closing.borrow_mut() = Some(closing.clone());
            closing
        };
        closing.await
    }
    fn fingerprint(&self) -> Option<Value> {
        self.observer.fingerprint()
    }
    fn first_heard(&self, since: i64) -> Option<i64> {
        self.connection.first_heard(since)
    }
    async fn undo(&self, id: Option<&str>, signal: Signal) -> Result<ChangeRecord, RuntimeError> {
        if !self.connection.started.get() || self.connection.closed.get() {
            return Err(KumiError::new(FailureKind::Request, "Kumi isn't connected to Live, so it can't undo.").into());
        }
        let _live = self.rendering.hold(&signal).await?;
        let outcome = self.history.undo(id.unwrap_or("last"), signal, false).await?;
        outcome.record.ok_or_else(|| KumiError::new(FailureKind::Request, outcome.text).into())
    }
    async fn audio_file(&self, named: &str, signal: Signal) -> Result<Option<String>, RuntimeError> {
        self.mutations.clip_file(named, signal).await
    }
    async fn stop_live(&self, signal: Signal) -> Result<bool, RuntimeError> {
        Ok(self.history.stop_everything(self.combined(signal)).await)
    }
    /// Never Live's default Set or a template: opening one of those again would change it.
    fn open_set(&self) -> Option<String> {
        let set = self.history.remember.current()?;
        set.path.clone().filter(|path| !set.unsaved && !path.is_empty() && !project::template_location(path))
    }
    fn has_open_place(&self) -> bool {
        true
    }
    /// The Set where Kumi last saw the project, through the system as a double-click would open it: Live asks about the
    /// open Set in its own dialog. Live drops Kumi while the Set loads, which reads as a Set Kumi asked for (#188).
    async fn open_place(&self, place: &str, signal: Signal) -> Result<Result<(), String>, RuntimeError> {
        let _ = signal;
        let path = match &self.options.project_store {
            Some(store) => store.set_path(place).await,
            None => None,
        };
        let Some(path) = path.filter(|path| !project::template_location(path)) else {
            return Ok(Err("Kumi doesn't know where that Set is.".into()));
        };
        if !std::path::Path::new(&path).is_file() {
            return Ok(Err(format!("That Set isn't at {path} any more.")));
        }
        let Some(open) = self.options.open_file.clone() else {
            return Ok(Err("Kumi can't open Sets in Live from here.".into()));
        };
        self.connection.expect_set_change(SET_OPENING_MS);
        if !open(path.clone()).await {
            self.connection.forget_set_change();
            return Ok(Err(format!("Live didn't take {path}.")));
        }
        Ok(Ok(()))
    }
    /// Live's status bar, through the bridge's `live_message` (never its modal form); outside Live's undo, and
    /// without Kumi's Live lock, so it never waits for a step under way.
    async fn show_message(&self, text: &str, signal: Signal) -> Result<bool, RuntimeError> {
        let text = head(text.trim(), 1024);
        // An older bridge would show it as a modal alert, holding Live: no line at all there.
        let line = at_least(self.connection.version().as_deref(), STATUS_LINE_BRIDGE);
        if text.is_empty() || !line || !self.connection.available() || !self.connection.has("live_message") {
            return Ok(false);
        }
        let shown =
            self.connection.call("live_message", views::object(json!({"text": text, "modal": false})), self.combined(signal)).await?;
        Ok(shown.is_error != Some(true))
    }
    /// Live records only while it plays: stopped, as the transport Kumi follows says, it isn't; otherwise its
    /// session-playback read says whether Session or Arrangement recording is on.
    async fn recording(&self, signal: Signal) -> Result<Option<bool>, RuntimeError> {
        if !self.connection.available() {
            return Ok(None);
        }
        if self.connection.playing() == Some(false) {
            return Ok(Some(false));
        }
        if !self.connection.has("live_discover") {
            return Ok(None);
        }
        let read = payload(
            &self
                .connection
                .call("live_discover", views::object(json!({"kind":"session-playback","limit":1})), self.combined(signal))
                .await?,
        )?;
        let transport =
            &read.get("items").and_then(|items| items.get(0)).and_then(|item| item.get("transport")).cloned().unwrap_or_default();
        let on = |key: &str| transport.get(key) == Some(&Value::Bool(true));
        Ok(Some(on("playing") && (on("sessionRecord") || on("arrangementRecord"))))
    }
    async fn instant(&self, intent: &kumi_runtime::core::phrases::Intent, signal: Signal) -> Result<Option<InstantOutcome>, RuntimeError> {
        let Some(plan) = instant::steps(intent, &self.observer.model(), self.observer.beats_per_bar.get()) else { return Ok(None) };
        // One Live call at a time, as every Live tool takes it; the plan says what it changed (final).
        let signal = self.combined(signal);
        let _live = self.rendering.hold(&signal).await?;
        let result = self.mutations.make_changes(object(&plan)?, signal).await?;
        Ok(Some(InstantOutcome { text: result.reply.filter(|reply| !reply.is_empty()).unwrap_or(result.text), is_error: result.is_error }))
    }
    async fn device_tree(&self, track: &str, signal: Signal) -> Result<Option<DeviceTree>, RuntimeError> {
        timing::background(views::device_tree(self.connection.as_ref(), track, self.combined(signal))).await
    }
    async fn clip_view(&self, slot: &str, signal: Signal) -> Result<Option<ClipView>, RuntimeError> {
        timing::background(views::clip_view(self.connection.as_ref(), slot, self.combined(signal))).await
    }
    async fn session_strip(&self, track: &str, scene: f64, signal: Signal) -> Result<Option<SessionStrip>, RuntimeError> {
        timing::background(views::session_strip(self.connection.as_ref(), track, scene, self.combined(signal))).await
    }
    async fn arrangement_strip(&self, signal: Signal) -> Result<Option<ArrangementStrip>, RuntimeError> {
        timing::background(views::arrangement_strip(self.connection.as_ref(), self.combined(signal))).await
    }
    async fn audition(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<AuditionResult, String>, RuntimeError> {
        let _live = self.rendering.hold(&signal).await?;
        self.rendering.audition(request, signal).await
    }
    async fn goal(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        let _live = self.rendering.hold(&signal).await?;
        self.rendering.open_goal(request, signal).await
    }
    async fn hear(&self, request: &HearRequest, signal: Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        let _live = self.rendering.hold(&signal).await?;
        self.rendering.hear_in_set(request, signal).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_that_finishes_the_request_is_said_by_kumi_when_final() {
        let said = |text: Value, finishing: bool| finished_command(ToolResult::text(stringify(&text)), finishing).reply;
        assert_eq!(
            said(json!({"pressed":"File › Save Live Set As…","saved":"C:/Music/SPEED HUGE Project/SPEED HUGE.als"}), true).as_deref(),
            Some("Saved the Set as SPEED HUGE.als.")
        );
        assert_eq!(said(json!({"pressed":"File › Save Live Set","saved":"where it is"}), true).as_deref(), Some("Saved the Set."));
        assert_eq!(said(json!({"pressed":"File › New Live Set","opened":"a new Set"}), true).as_deref(), Some("Opened a new Set."));
        assert_eq!(
            said(json!({"pressed":"File › Open Live Set…","openedSet":"C:/Music/Night.als"}), true).as_deref(),
            Some("Opened Night.als.")
        );
        // Without final, or with a dialog or question left to answer, the model is called as before.
        assert_eq!(said(json!({"pressed":"File › Save Live Set","saved":"where it is"}), false), None);
        assert_eq!(said(json!({"pressed":"File › New Live Set","dialog":{"open":true},"next":"Answer it"}), true), None);
        assert_eq!(finished_command(ToolResult::error("Live has it greyed out"), true).reply, None);
    }
}
