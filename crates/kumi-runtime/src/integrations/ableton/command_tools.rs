//! Live's own menu commands and the plug-in guide/wavetable glue.
use super::{
    connection::{LiveConnection, ReadError, NO_CURRENT_LIVE},
    context::{object, payload, ObservationError},
    history::{result_text, History},
    live_command::{find_item, shortcut, Command, Target, COMMANDS},
    options::HandsSetup,
    remember::Remember,
    views::ViewHost,
};
use crate::{
    audio::{
        audio_path,
        wavetable::{self, FromAudio, Keyframe, Shape, WavetableSpec},
    },
    core::{
        contracts::{ActionEvent, ChangeRecord, JsonObject, ToolResult},
        errors::RuntimeError,
    },
    hands::{self, Hands, HandsError, KeysOptions, MenuItem, MenuOptions, OpenHandsOptions, Track},
    mcp::allowed_tools::CallOptions,
    plugins::registry::{adapter_for, folder_for, plugin_guide, ExposedParameter},
};
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{
        json::stringify,
        string::{head, trim},
    },
};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    rc::Rc,
    sync::LazyLock,
    time::Duration,
};

mod conversions;

pub type CommandAction = Rc<dyn Fn(String, JsonObject, Signal) -> LocalBoxFuture<'static, Result<ToolResult, RuntimeError>>>;
/// Bringing Live's window forward: true when it came.
pub type FrontLive = Rc<dyn Fn() -> LocalBoxFuture<'static, bool>>;
#[derive(Default)]
pub struct CommandToolsOptions {
    pub hands: Option<HandsSetup>,
    pub user_library: Option<String>,
    pub on_action: Option<Rc<dyn Fn(ActionEvent)>>,
    /// How Live's window is brought forward (the Live that's open, by its app, when left out).
    pub front_live: Option<FrontLive>,
}
type HandsReady = Shared<LocalBoxFuture<'static, Option<Rc<dyn Hands>>>>;
pub struct CommandTools {
    connection: Rc<LiveConnection>,
    history: Rc<History>,
    remember: Rc<Remember>,
    options: CommandToolsOptions,
    act: CommandAction,
    hands_setup: RefCell<Option<HandsReady>>,
    menu_items: RefCell<Option<Vec<MenuItem>>>,
}
#[derive(Debug)]
enum CommandError {
    Observation(ObservationError),
    Hands(HandsError),
    Other(String),
}
impl From<ObservationError> for CommandError {
    fn from(e: ObservationError) -> Self {
        Self::Observation(e)
    }
}
impl From<HandsError> for CommandError {
    fn from(e: HandsError) -> Self {
        Self::Hands(e)
    }
}
impl From<RuntimeError> for CommandError {
    fn from(e: RuntimeError) -> Self {
        match e {
            RuntimeError::Observation(message) => Self::Observation(ObservationError(message)),
            other => Self::Other(other.to_string()),
        }
    }
}
impl From<std::io::Error> for CommandError {
    fn from(e: std::io::Error) -> Self {
        Self::Other(e.to_string())
    }
}
impl From<crate::audio::decode::AudioError> for CommandError {
    fn from(e: crate::audio::decode::AudioError) -> Self {
        Self::Other(e.to_string())
    }
}
impl From<ReadError> for CommandError {
    fn from(e: ReadError) -> Self {
        match e {
            ReadError::Observation(e) => e.into(),
            ReadError::Other(e) => e.into(),
        }
    }
}
fn strings(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
}
fn string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(v)) => v.iter().map(|v| if v.is_null() { String::new() } else { string(Some(v)) }).collect::<Vec<_>>().join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(v) => stringify(v),
    }
}
fn args(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}
async fn delay(ms: u64, signal: &Signal) -> Result<(), CommandError> {
    tokio::select! { biased; _=signal.cancelled()=>Err(CommandError::Other("Operation cancelled".into())), _=tokio::time::sleep(Duration::from_millis(ms))=>Ok(()) }
}
static SESSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]+):clip:([0-9]+):([0-9]+)$").unwrap());
static DEVICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]+):device:([0-9]+):").unwrap());
static LABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[\\/:*?"<>|]+"#).unwrap());
fn includes(title: &str, words: &[&str]) -> bool {
    words.iter().any(|word| title.contains(word))
}
impl CommandTools {
    pub fn new(
        connection: Rc<LiveConnection>,
        history: Rc<History>,
        remember: Rc<Remember>,
        options: CommandToolsOptions,
        act: CommandAction,
    ) -> Self {
        Self { connection, history, remember, options, act, hands_setup: RefCell::new(None), menu_items: RefCell::new(None) }
    }
    fn tell(&self, title: impl Into<String>) {
        if let Some(listener) = &self.options.on_action {
            let event = ActionEvent { title: title.into(), playing: None, recording: None };
            let _ = catch_unwind(AssertUnwindSafe(|| listener(event)));
        }
    }
    fn available(&self) -> bool {
        self.connection.available.get()
            && !self.connection.lost.get()
            && self.connection.tools().is_some()
            && self.connection.epoch.get().is_some()
    }
    async fn hands_ready(&self) -> Option<Rc<dyn Hands>> {
        if matches!(self.options.hands, Some(HandsSetup::Disabled)) {
            return None;
        }
        if self.hands_setup.borrow().is_none() {
            let future: LocalBoxFuture<'static, Option<Rc<dyn Hands>>> = match &self.options.hands {
                Some(HandsSetup::Open(open)) => {
                    let pending = open();
                    async move { pending.await.ok().flatten() }.boxed_local()
                }
                _ => {
                    let listener = self.options.on_action.clone();
                    async move {
                        hands::open_hands(OpenHandsOptions {
                            on_build: Some(Rc::new(move |title| {
                                if let Some(listener) = &listener {
                                    let _ = catch_unwind(AssertUnwindSafe(|| {
                                        listener(ActionEvent { title: title.into(), playing: None, recording: None })
                                    }));
                                }
                            })),
                            ..Default::default()
                        })
                        .await
                        .ok()
                        .flatten()
                    }
                    .boxed_local()
                }
            };
            *self.hands_setup.borrow_mut() = Some(future.shared());
        }
        let pending = self.hands_setup.borrow().as_ref().unwrap().clone();
        let hands = pending.await;
        if hands.is_none() {
            self.hands_setup.borrow_mut().take();
        }
        hands
    }
    async fn track_rows(&self, fields: Value, signal: &Signal) -> Result<Vec<JsonObject>, CommandError> {
        let mut rows = self.connection.rows("track", args(json!({"fields":fields})), signal.clone()).await?;
        rows.extend(self.connection.rows("return-track", args(json!({"fields":["name"]})), signal.clone()).await?);
        Ok(rows)
    }
    async fn track_names(&self, signal: &Signal) -> Result<Vec<String>, CommandError> {
        Ok(self
            .track_rows(json!(["name"]), signal)
            .await?
            .iter()
            .map(|r| r.get("name").and_then(Value::as_str).unwrap_or("").into())
            .collect())
    }
    async fn track_ref_of(&self, named: &str, signal: &Signal) -> Result<String, CommandError> {
        let rows = self.track_rows(json!(["name"]), signal).await?;
        rows.iter()
            .find(|r| r.get("ref").and_then(Value::as_str) == Some(named))
            .or_else(|| rows.iter().find(|r| r.get("name").and_then(Value::as_str) == Some(named)))
            .and_then(|r| r.get("ref"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ObservationError(format!("{named} isn't a track in this turn's discovery; discover it again.")).into())
    }
    fn lengthen(&self, reference: &str, field: &str) -> String {
        string(self.connection.references.borrow().lengthen(&json!({field:reference})).get(field))
    }
    fn modified(&self) -> Result<Option<std::time::SystemTime>, CommandError> {
        let path = self.remember.current().and_then(|p| p.path.clone());
        match path {
            Some(path) if std::path::Path::new(&path).exists() => Ok(Some(std::fs::metadata(path)?.modified()?)),
            _ => Ok(None),
        }
    }
    pub async fn live_command(&self, input: &JsonObject, original_signal: Signal) -> Result<ToolResult, RuntimeError> {
        let signal = abort::any([original_signal, self.connection.lifetime.clone()]);
        if !self.available() {
            return Ok(ToolResult::error(NO_CURRENT_LIVE));
        }
        // Live's conversions to MIDI go through its API where it has one: no menus, any clip, on any computer.
        if let Some(conversion) = input.get("command").and_then(Value::as_str).and_then(conversions::conversion) {
            if self.connection.has("live_run_python") {
                match self.convert(input, conversion, &signal).await {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) => {}
                    Err(error) => {
                        signal.check()?;
                        return Ok(ToolResult::error(match error {
                            CommandError::Hands(e) => e.to_string(),
                            CommandError::Observation(e) => e.to_string(),
                            CommandError::Other(e) => format!("Kumi couldn't convert it: {}", head(&e, 200)),
                        }));
                    }
                }
            }
        }
        let Some(hands) = self.hands_ready().await else {
            return Ok(ToolResult::error(if cfg!(target_os = "macos") {
                "Kumi can't use Live's menus on this Mac yet: its helper is built with Xcode's command line tools (run xcode-select --install), or comes with Kumi's next update."
            } else {
                "Kumi can't use Live's menus on this computer."
            }));
        };
        match self.run_command(input, &signal, hands.as_ref()).await {
            Ok(result) => Ok(result),
            Err(error) => {
                signal.check()?;
                Ok(ToolResult::error(match error {
                    CommandError::Hands(e) => e.to_string(),
                    CommandError::Observation(e) => e.to_string(),
                    CommandError::Other(e) => format!("Kumi couldn't use Live's menus: {}", head(&e, 200)),
                }))
            }
        }
    }
    async fn run_command(&self, input: &JsonObject, signal: &Signal, hands: &dyn Hands) -> Result<ToolResult, CommandError> {
        if !hands.trusted(false).await? {
            let _ = hands.trusted(true).await;
            return Ok(ToolResult::error("Kumi needs Accessibility access to use Live's menus. macOS just asked for it: in System Settings › Privacy & Security › Accessibility, turn on the app Kumi runs in (your terminal), then ask again. Tell the producer exactly that."));
        }
        if let Some(answer) = input.get("answer").and_then(Value::as_str) {
            if !hands.answer(answer, Some(signal.clone())).await?.ok {
                let open = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
                return Ok(ToolResult::error(if open.open {
                    format!(
                        "Live's dialog has no \"{answer}\" button; its buttons: {}.",
                        open.buttons.map(|v| v.join(", ")).filter(|v| !v.is_empty()).unwrap_or_else(|| "none Kumi can see".into())
                    )
                } else {
                    "Live has no dialog open.".into()
                }));
            }
            self.tell(format!("Pressed {answer} in Live's dialog"));
            delay(200, signal).await?;
            let next = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
            let mut out = args(json!({"pressed":answer}));
            if next.open {
                out.insert("dialog".into(), serde_json::to_value(next).unwrap());
            }
            return Ok(ToolResult::text(stringify(&Value::Object(out))));
        }
        let named_command = input.get("command").and_then(Value::as_str);
        let command = named_command.and_then(|name| COMMANDS.get(name));
        if let Some(name) = named_command.filter(|_| command.is_none()) {
            return Ok(ToolResult::error(format!("Kumi doesn't know the command {name}; name the menu item instead (menu).")));
        }
        let menu = strings(input.get("menu"));
        let keys = strings(input.get("keys"));
        if command.is_none() && menu.is_empty() && keys.is_empty() {
            return Ok(ToolResult::error("Give a command, a menu item (menu) or keys."));
        }
        let track = input.get("track").and_then(Value::as_str).filter(|s| !s.is_empty());
        let tracks = strings(input.get("tracks"));
        let clip = input.get("clip").and_then(Value::as_str).filter(|s| !s.is_empty());
        let target = command.map(|c| c.target).unwrap_or(Target::None);
        if matches!(target, Target::Track | Target::TrackOrClip) && track.is_none() && clip.is_none() && tracks.is_empty() {
            return Ok(ToolResult::error(format!("{} works on a track: give track.", named_command.unwrap())));
        }
        if target == Target::Tracks && tracks.len() < 2 && track.is_none() {
            return Ok(ToolResult::error("Give the tracks to group, side by side, first to last (tracks)."));
        }
        if target == Target::Clip && clip.is_none() {
            return Ok(ToolResult::error(format!(
                "{} works on a clip: give clip (its clipRef from this turn, or \"selected\" for the one selected in Live).",
                named_command.unwrap()
            )));
        }
        let before = self.track_names(signal).await?;
        let saved_before = self.modified()?;
        let mut what = String::new();
        let mut chosen = Vec::new();
        if clip == Some("selected") {
            what = " the selected clip".into();
        } else if let Some(clip) = clip.filter(|_| matches!(target, Target::Clip | Target::TrackOrClip | Target::None)) {
            let long = self.lengthen(clip, "clipRef");
            let session = SESSION.captures(&long);
            if session.is_none() && target == Target::Clip {
                return Ok(ToolResult::error("Kumi can't select a clip in the Arrangement for Live's own commands yet: ask the producer to click it, then use clip: \"selected\" (or work on a Session clip)."));
            }
            if session.is_some() {
                (self.act)("show".into(), args(json!({"action":"focus-view","view":"Session"})), signal.clone()).await?;
            }
            let slot = session.map(|m| format!("{}:clip_slot:{}:{}", &m[1], &m[2], &m[3]));
            if let Some(slot) = &slot {
                self.connection.references.borrow_mut().refs.entry(slot.clone()).or_insert_with(|| "clip-slot".into());
            }
            let mut select = args(json!({"detailClipRef":clip}));
            if let Some(slot) = slot {
                select.insert("slotRef".into(), json!(slot));
            }
            let selected = (self.act)("select".into(), select, signal.clone()).await?;
            if selected.is_error {
                return Ok(ToolResult::error(format!("Kumi couldn't select that clip in Live: {}", head(&selected.text, 300))));
            }
            what = " the clip".into();
        } else if track.is_some() || !tracks.is_empty() {
            let list = if tracks.is_empty() { vec![track.unwrap().to_owned()] } else { tracks };
            let all = self.track_rows(json!(["name", "kind", "isFrozen", "isVisible"]), signal).await?;
            let mut targets = Vec::new();
            for one in &list {
                let reference = self.track_ref_of(one, signal).await?;
                let index = all.iter().position(|r| r.get("ref").and_then(Value::as_str) == Some(reference.as_str()));
                let row = index.map(|i| &all[i]);
                let name = row.and_then(|r| r.get("name")).and_then(Value::as_str).unwrap_or(one);
                let frozen = row.and_then(|r| r.get("isFrozen")).and_then(Value::as_bool);
                let already = if named_command == Some("freeze_track") && frozen == Some(true) {
                    Some("is frozen already")
                } else if matches!(named_command, Some("unfreeze_track" | "flatten_track")) && frozen == Some(false) {
                    Some(if named_command == Some("flatten_track") {
                        "isn't frozen (Flatten works on a frozen track: freeze it first)"
                    } else {
                        "isn't frozen"
                    })
                } else if named_command == Some("ungroup_tracks")
                    && row.is_some()
                    && row.and_then(|r| r.get("kind")).and_then(Value::as_str) != Some("group")
                {
                    Some("isn't a group")
                } else {
                    None
                };
                if let Some(already) = already {
                    return Ok(ToolResult::error(format!("{name} {already}; nothing pressed.")));
                }
                if row.and_then(|r| r.get("isVisible")).and_then(Value::as_bool) == Some(false) {
                    return Ok(ToolResult::error(format!(
                        "{name} is inside a folded group, so Live's track headers don't show it: unfold the group, then ask again."
                    )));
                }
                targets.push(Track {
                    name: name.into(),
                    nth: Some(
                        all[..index.unwrap_or(0)].iter().filter(|r| r.get("name").and_then(Value::as_str) == Some(name)).count() as f64
                    ),
                });
            }
            let selected = hands.tracks(&targets, Some(signal.clone())).await.unwrap_or_else(|e| hands::HandsReply {
                ok: false,
                error: Some(e.to_string()),
                ..Default::default()
            });
            if !selected.ok && selected.error.as_deref() == Some("no-track") {
                let missing = selected
                    .fields
                    .get("missing")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items.iter().map(|v| if v.is_null() { String::new() } else { string(Some(v)) }).collect::<Vec<_>>().join(", ")
                    })
                    .unwrap_or_else(|| list.join(", "));
                return Ok(ToolResult::error(format!(
                    "Live's track headers don't show {missing}: is it inside a folded group? Unfold the group, then ask again."
                )));
            }
            if !selected.ok {
                if list.len() > 1 {
                    return Ok(ToolResult::error(format!(
                        "Kumi couldn't select several tracks in Live here ({}); select them in Live, then ask again.",
                        selected.error.as_deref().unwrap_or("undefined")
                    )));
                }
                let reference = self.track_ref_of(&list[0], signal).await?;
                let via = (self.act)("select".into(), args(json!({"trackRef":reference})), signal.clone()).await?;
                if via.is_error {
                    return Ok(ToolResult::error(format!("Kumi couldn't select {} in Live: {}", list[0], head(&via.text, 300))));
                }
            } else {
                self.tell(format!("Selected {}", targets.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")));
            }
            chosen = targets.iter().map(|t| t.name.clone()).collect();
            what = if list.len() > 1 { format!(" {} tracks", list.len()) } else { format!(" {}", targets[0].name) };
        }
        let (pressed, key) = if command.is_some() || !menu.is_empty() {
            if self.menu_items.borrow().is_none() {
                let items = hands.menus(Some(signal.clone())).await?;
                *self.menu_items.borrow_mut() = Some(items);
            }
            let mut item = self.find_menu(command, &menu);
            if item.is_none() {
                let items = hands.menus(Some(signal.clone())).await?;
                *self.menu_items.borrow_mut() = Some(items);
                item = self.find_menu(command, &menu);
            }
            let Some(item) = item else {
                return Ok(ToolResult::error(format!(
                    "Live's menus don't have “{}” here: it may need a newer Live, Live Suite, or something selected first.",
                    command.map(|c| c.titles[0].clone()).unwrap_or_else(|| menu.join(" › "))
                )));
            };
            let reply = hands
                .menu(
                    &item.path,
                    MenuOptions {
                        signal: Some(signal.clone()),
                        titles: command.map(|c| c.titles.clone()).unwrap_or_default(),
                        ..Default::default()
                    },
                )
                .await?;
            if !reply.ok {
                return Ok(ToolResult::error(if reply.error.as_deref() == Some("disabled") {
                    format!("Live has “{}” greyed out right now: it needs the right thing selected (and some commands need the Arrangement or Session view in front).",item.path.join(" › "))
                } else {
                    format!("Live didn't take “{}” ({}).", item.path.join(" › "), reply.error.as_deref().unwrap_or("undefined"))
                }));
            }
            let said = if matches!(named_command, Some("freeze_track" | "unfreeze_track")) {
                command.unwrap().titles[0].as_str()
            } else {
                reply
                    .fields
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| item.path.last().map(String::as_str).unwrap_or(""))
            };
            let mut path = item.path[..item.path.len().saturating_sub(1)].to_vec();
            path.push(said.into());
            (path.join(" › "), shortcut(&item))
        } else {
            let reply = hands.keys(&keys, KeysOptions { signal: Some(signal.clone()), ..Default::default() }).await?;
            if !reply.ok {
                return Ok(ToolResult::error(format!("Live didn't take those keys ({}).", reply.error.as_deref().unwrap_or("undefined"))));
            }
            (keys.join(", "), None)
        };
        let toggle = match named_command {
            Some("freeze_track") => Some(true),
            Some("unfreeze_track" | "flatten_track") => Some(false),
            _ => None,
        };
        if let Some(toggle) = toggle.filter(|_| !chosen.is_empty()) {
            self.tell(format!(
                "{}{what}",
                if toggle {
                    "Freezing"
                } else if named_command == Some("flatten_track") {
                    "Flattening"
                } else {
                    "Unfreezing"
                }
            ));
            for _ in 0..600 {
                let rows = self.connection.rows("track", args(json!({"fields":["name","isFrozen"]})), signal.clone()).await?;
                if chosen.iter().all(|name| {
                    rows.iter().any(|r| {
                        r.get("name").and_then(Value::as_str) == Some(name) && r.get("isFrozen").and_then(Value::as_bool) == Some(toggle)
                    })
                }) {
                    break;
                }
                let open = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
                if open.open && open.buttons.as_ref().is_some_and(|b| !b.is_empty()) {
                    break;
                }
                delay(100, signal).await?;
            }
        }
        if command
            .is_some_and(|c| includes(&c.titles[0], &["Bounce", "Convert", "Separate", "Slice", "Consolidate", "Flatten", "Paste Bounced"]))
        {
            delay(150, signal).await?;
            for _ in 0..1200 {
                let open = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
                if !open.open || open.buttons.as_ref().is_some_and(|b| !b.is_empty()) {
                    break;
                }
                delay(100, signal).await?;
            }
        }
        let title = command.map(|c| format!("{}{what}", c.done)).unwrap_or_else(|| format!("Pressed {pressed} in Live"));
        self.tell(&title);
        let asks = command.and_then(|c| c.dialog) == Some(true);
        let mut dialog = None;
        for _ in 0..if asks { 6 } else { 2 } {
            delay(if asks { 250 } else { 150 }, signal).await?;
            let open = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
            if open.open && open.buttons.as_ref().is_some_and(|b| !b.is_empty()) {
                dialog = Some(open);
                break;
            }
        }
        {
            let mut refs = self.connection.references.borrow_mut();
            refs.invalidate();
            refs.clear_names();
        }
        self.connection.lease.set(self.connection.lease.get() + 1);
        let mut after = self.track_names(signal).await?;
        for _ in 0..8 {
            if dialog.is_some()
                || !command.is_some_and(|c| {
                    c.target != Target::None && includes(&c.titles[0], &["Bounce", "Convert", "Separate", "Slice", "Group"])
                })
                || after != before
            {
                break;
            }
            delay(250, signal).await?;
            after = self.track_names(signal).await?;
        }
        let added: Vec<_> = after
            .iter()
            .filter(|name| {
                !before.contains(name) || after.iter().filter(|n| n == name).count() > before.iter().filter(|n| n == name).count()
            })
            .cloned()
            .collect();
        let removed: Vec<_> = before.iter().filter(|name| !after.contains(name)).cloned().collect();
        let saved = if let Some(before) = saved_before { self.modified()?.is_some_and(|after| after > before) } else { false };
        if command.is_some() || !added.is_empty() || !removed.is_empty() {
            let record:ChangeRecord=serde_json::from_value(json!({"id":format!("l{}",&uuid::Uuid::new_v4().to_string()[..8]),"family":"structure","title":title,"state":"kept","note":"Done with Live's own command: Live's undo (Cmd-Z) takes it back.","at":self.connection.now().timestamp_millis()})).unwrap();
            self.history.emit(&record);
        }
        let mut out = args(json!({"pressed":pressed}));
        if let Some(key) = key.filter(|s| !s.is_empty()) {
            out.insert("liveShortcut".into(), json!(key));
        }
        if !added.is_empty() {
            out.insert("newTracks".into(), json!(added));
        }
        if !removed.is_empty() {
            out.insert("goneTracks".into(), json!(removed));
        }
        if saved {
            out.insert("saved".into(), json!(true));
        }
        if let Some(dialog) = dialog {
            out.insert("dialog".into(), serde_json::to_value(dialog).unwrap());
            out.insert("next".into(), json!("Answer it with answer (the button's title), or tell the producer what it asks."));
        }
        out.insert("note".into(), json!("References from before are gone: discover again before using any."));
        Ok(ToolResult::text(stringify(&Value::Object(out))))
    }
    fn find_menu(&self, command: Option<&Command>, menu: &[String]) -> Option<MenuItem> {
        let items = self.menu_items.borrow();
        let items = items.as_ref()?;
        if let Some(command) = command {
            find_item(items, &command.titles).cloned()
        } else {
            items
                .iter()
                .find(|item| item.path.len() == menu.len() && item.path.iter().zip(menu).all(|(a, b)| a.to_lowercase() == b.to_lowercase()))
                .or_else(|| find_item(items, &[menu.last()?.clone()]))
                .cloned()
        }
    }
    /// All parameter pages, stopping on a repeated cursor exactly as the source integration does.
    pub async fn device_parameters(&self, device: &str, fields: &[&str], signal: Signal) -> Result<Vec<JsonObject>, ReadError> {
        let mut rows = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..10000 {
            let mut input = args(json!({"kind":"parameter","parent":device,"fields":fields,"limit":self.connection.page_limit()}));
            if let Some(cursor) = &cursor {
                input.insert("cursor".into(), json!(cursor));
            }
            let read = payload(&self.connection.call("live_discover", input, signal.clone()).await?)?;
            if let Some(items) = read.get("items").and_then(Value::as_array) {
                for item in items {
                    rows.push(object(item)?);
                }
            }
            cursor = read.get("nextCursor").and_then(Value::as_str).filter(|next| Some(*next) != cursor.as_deref()).map(str::to_owned);
            if cursor.as_deref().is_none_or(str::is_empty) {
                break;
            }
        }
        Ok(rows)
    }
    pub async fn plugin_tool(&self, input: &JsonObject, original_signal: Signal) -> Result<ToolResult, RuntimeError> {
        let signal = abort::any([original_signal, self.connection.lifetime.clone()]);
        if !self.available() {
            return Ok(ToolResult::error(NO_CURRENT_LIVE));
        }
        let Some(device) = input.get("device").and_then(Value::as_str) else {
            return Ok(ToolResult::error("Give the plug-in device (its deviceRef from this turn)."));
        };
        let long = self.lengthen(device, "deviceRef");
        if let Err(error) = self.connection.references.borrow().require_fresh_references(&args(json!({"deviceRef":long}))) {
            return Ok(ToolResult::error(error.to_string()));
        }
        match self.run_plugin(input, &long, &signal).await {
            Ok(result) => Ok(result),
            Err(error) => {
                signal.check()?;
                Ok(ToolResult::error(match error {
                    CommandError::Observation(e) => e.to_string(),
                    CommandError::Other(e) => format!("Kumi couldn't read that plug-in: {}", head(&e, 200)),
                    CommandError::Hands(e) => format!("Kumi couldn't read that plug-in: {}", head(&e.to_string(), 200)),
                }))
            }
        }
    }
    async fn run_plugin(&self, input: &JsonObject, long: &str, signal: &Signal) -> Result<ToolResult, CommandError> {
        let tools = self.connection.tools().unwrap();
        let mut name = String::new();
        if let Some(track) = DEVICE.captures(long) {
            let devices=tools.call("live_discover",args(json!({"kind":"device","parent":format!("{}:track:{}",&track[1],&track[2]),"fields":["name","className"],"limit":self.connection.page_limit(),"budget":self.connection.whole_budget()})),signal.clone(),CallOptions{host:true}).await?;
            if devices.is_error != Some(true) {
                let read = payload(&devices)?;
                let rows = read
                    .get("items")
                    .filter(|v| !v.is_null())
                    .map(|v| v.as_array().ok_or_else(|| CommandError::Other("(payload(...).items ?? []).map is not a function".into())))
                    .transpose()?;
                if let Some(rows) = rows {
                    let rows = rows.iter().map(object).collect::<Result<Vec<_>, _>>()?;
                    if let Some(item) = rows.iter().find(|r| r.get("ref").and_then(Value::as_str) == Some(long)) {
                        name = item.get("name").and_then(Value::as_str).unwrap_or("").into();
                    }
                }
            }
        }
        let adapter = adapter_for(&name, None);
        if input.get("action").and_then(Value::as_str) == Some("wavetable") {
            let spec = object(input.get("wavetable").filter(|v| !v.is_null()).unwrap_or(&json!({})))?;
            let label = spec
                .get("name")
                .and_then(Value::as_str)
                .map(trim)
                .filter(|s| !s.is_empty())
                .map(|s| head(&LABEL.replace_all(s, " "), 48))
                .unwrap_or_else(|| "Kumi Wavetable".into());
            let from = spec.get("from_audio").filter(|v| v.is_object() || v.is_array()).map(object).transpose()?;
            let from_audio = from.as_ref().and_then(|a| {
                a.get("file").and_then(Value::as_str).map(|file| FromAudio {
                    file: audio_path(file),
                    start: a.get("start").and_then(Value::as_f64),
                    seconds: a.get("seconds").and_then(Value::as_f64),
                })
            });
            let keyframes = if from_audio.is_some() {
                None
            } else {
                spec.get("keyframes")
                    .and_then(Value::as_array)
                    .map(|keys| keys.iter().map(raw_keyframe).collect::<Result<Vec<_>, _>>())
                    .transpose()?
            };
            let frames =
                wavetable::build_wavetable(&WavetableSpec { keyframes, count: spec.get("count").and_then(Value::as_f64), from_audio })
                    .await?;
            let folder = folder_for(adapter.and_then(|a| a.folders.as_ref()).and_then(|f| f.wavetables.as_ref()))
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(self.options.user_library.clone().unwrap_or_else(|| super::samples::user_library(None, None)))
                        .join("Kumi")
                        .join("Wavetables")
                });
            tokio::fs::create_dir_all(&folder).await?;
            let mut file = folder.join(format!("{label}.wav"));
            for index in 2..1000 {
                if !file.exists() {
                    break;
                }
                file = folder.join(format!("{label} {index}.wav"));
            }
            wavetable::write_wavetable(&file, &frames).await?;
            let made = file.file_stem().unwrap().to_string_lossy();
            self.tell(format!("Made the wavetable {made} ({} frames)", frames.len()));
            let next=adapter.map(|a|format!("It's in {}'s wavetable folder: in its window (set_device_details isEditorOpen opens it), the oscillator's wavetable menu lists it. Picking it there is a click for the producer; say where.",a.name)).unwrap_or_else(||"Load it in the synth's oscillator from that file (most read 2048-sample frames).".into());
            return Ok(ToolResult::text(stringify(&json!({"made":made,"file":file.to_string_lossy(),"frames":frames.len(),"next":next}))));
        }
        let read = tools
            .call("live_device_read", args(json!({"deviceRef":long,"what":"parameter-names"})), signal.clone(), CallOptions { host: true })
            .await?;
        if read.is_error == Some(true) {
            let text = result_text(&read);
            return Ok(ToolResult::error(if text.contains("only a plug-in") {
                "That isn't a plug-in: its parameters are all in discovery (kind parameter).".into()
            } else {
                format!("Live didn't list that plug-in's parameters: {}", head(&text, 200))
            }));
        }
        let read = payload(&read)?;
        let names = match read.get("names").filter(|v| !v.is_null()) {
            None => Vec::new(),
            Some(Value::Array(v)) => v.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            _ => return Err(CommandError::Other("(payload(...).names ?? []).filter is not a function".into())),
        };
        let exposed = self
            .device_parameters(long, &["ref", "name", "displayValue"], signal.clone())
            .await?
            .iter()
            .filter_map(|r| {
                let name = r.get("name").and_then(Value::as_str).filter(|s| *s != "Device On")?;
                Some(ExposedParameter {
                    name: name.into(),
                    reference: string(r.get("ref")),
                    display: r.get("displayValue").and_then(Value::as_str).map(str::to_owned),
                })
            })
            .collect::<Vec<_>>();
        let mut guide = plugin_guide(if name.is_empty() { "this plug-in" } else { &name }, adapter, &names, &exposed);
        let text = stringify(&Value::Object(guide.clone()));
        if text.len() <= 48 * 1024 {
            return Ok(ToolResult::text(text));
        }
        guide.get_mut("parameters").and_then(Value::as_object_mut).unwrap().insert("groups".into(), json!("too many to list"));
        Ok(ToolResult::text(stringify(&Value::Object(guide))))
    }
}
fn raw_keyframe(value: &Value) -> Result<Keyframe, CommandError> {
    if value.is_null() {
        return Err(CommandError::Other("Cannot read properties of null (reading 'harmonics')".into()));
    }
    let harmonics = value.get("harmonics").map(raw_harmonics).transpose()?.flatten();
    let shape = match value.get("shape").filter(|v| !v.is_null()) {
        None => Some(Shape::Sine),
        Some(Value::String(s)) => match s.as_str() {
            "sine" => Some(Shape::Sine),
            "saw" => Some(Shape::Saw),
            "square" => Some(Shape::Square),
            "triangle" => Some(Shape::Triangle),
            "pulse" => Some(Shape::Pulse),
            _ => None,
        },
        _ => None,
    };
    // An unknown shape has no harmonics in the source switch and produces silence.
    Ok(Keyframe {
        harmonics: if harmonics.is_none() && shape.is_none() { Some(vec![0.0]) } else { harmonics },
        shape,
        width: value.get("width").map(js_number),
    })
}
fn js_number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => kumi_common::js::number::parse(s).unwrap_or(f64::NAN),
        Value::Array(_) => kumi_common::js::number::parse(&string(Some(value))).unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

fn raw_harmonics(value: &Value) -> Result<Option<Vec<f64>>, CommandError> {
    Ok(match value {
        Value::Array(values) if !values.is_empty() => Some(values.iter().map(js_number).collect()),
        Value::String(value) if !value.is_empty() => {
            Some(value.encode_utf16().map(|unit| js_number(&json!(String::from_utf16_lossy(&[unit])))).collect())
        }
        Value::Object(value) => {
            let Some(length) = value.get("length") else {
                return Ok(None);
            };
            let truthy = match length {
                Value::Null => false,
                Value::Bool(v) => *v,
                Value::Number(v) => v.as_f64() != Some(0.0),
                Value::String(v) => !v.is_empty(),
                _ => true,
            };
            if !truthy {
                return Ok(None);
            }
            let length = js_number(length);
            // Array.from converts a spectrum's length with ToLength before reading its indexed properties.
            if length.floor() > u32::MAX as f64 {
                return Err(CommandError::Other("Invalid array length".into()));
            }
            let length = if length.is_nan() || length <= 0.0 { 0 } else { length.floor() as usize };
            let values =
                (0..length).map(|i| value.get(&i.to_string()).filter(|v| !v.is_null()).map(js_number).unwrap_or(0.0)).collect::<Vec<_>>();
            Some(if values.is_empty() { vec![0.0] } else { values })
        }
        _ => None,
    })
}
