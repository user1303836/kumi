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
    hands::{self, Hands, HandsError, KeysOptions, MenuItem, MenuOptions, OpenHandsOptions, Toggle, ToggleSet, Track},
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
    cell::{Cell, RefCell},
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    rc::Rc,
    sync::LazyLock,
    time::Duration,
};

mod conversions;

pub type CommandAction = Rc<dyn Fn(String, JsonObject, Signal) -> LocalBoxFuture<'static, Result<ToolResult, RuntimeError>>>;
/// How long a Set Kumi asked for may take to open (Live drops Kumi while it loads).
const SET_OPENING_MS: i64 = 120_000;
/// Keys and menu items that put another Set in place of the open one.
const SET_KEYS: &[&str] = &["ctrl+n", "cmd+n", "ctrl+o", "cmd+o"];
fn switches_set(path: &[String]) -> bool {
    path.iter().any(|title| {
        let title = title.trim().to_lowercase();
        ["new live set", "open live set", "open recent"].iter().any(|start| title.starts_with(start))
    })
}
/// A Set switch Kumi asked for, expected only while this lives: a switch cancelled, refused or never seen
/// leaves a Set the producer opens afterwards theirs (#188).
struct ExpectedSwitch<'a>(&'a LiveConnection);
impl<'a> ExpectedSwitch<'a> {
    fn new(connection: &'a LiveConnection) -> Self {
        connection.expect_set_change(SET_OPENING_MS);
        Self(connection)
    }
    /// Still expected past this call: Kumi handed Live's dialog back mid-switch, and the model's `answer` (Yes
    /// to a replace prompt, say) may finish it. A Cancel through `answer`, or the time running out, ends it.
    fn handed_back(self) {
        std::mem::forget(self);
    }
}
impl Drop for ExpectedSwitch<'_> {
    fn drop(&mut self) {
        self.0.forget_set_change();
    }
}
/// Live's progress windows (Freeze…, Bounce…) offer only Cancel: they close on their own, with nothing to
/// answer. Read through Win32 since #187, their Cancel shows, and a wait that stopped at any button stopped at
/// them.
fn in_progress(dialog: &hands::Dialog) -> bool {
    dialog.open && dialog.buttons.as_ref().is_some_and(|buttons| buttons.len() == 1 && button_title(&buttons[0]) == "cancel")
}
/// Where Live may put a Set saved as `path`, each with when it was last written: there in a project folder
/// (one with "Ableton Project Info"), else in a new project folder beside it, "<name> Project", or
/// "<name>-1 Project" and on when that one exists (seen in Live 12.4). Read before the save and after: the
/// one whose time changed is where it went, whatever the clocks (Windows stamps files from a coarser one).
fn set_places(path: &std::path::Path) -> Vec<(PathBuf, Option<std::time::SystemTime>)> {
    let (Some(folder), Some(name), Some(file)) = (path.parent(), path.file_stem(), path.file_name()) else { return vec![] };
    let name = name.to_string_lossy();
    std::iter::once(path.to_path_buf())
        .chain(
            std::iter::once(format!("{name} Project"))
                .chain((1..=20).map(|n| format!("{name}-{n} Project")))
                .map(|project| folder.join(project).join(file)),
        )
        .map(|place| {
            let written = std::fs::metadata(&place).and_then(|m| m.modified()).ok();
            (place, written)
        })
        .collect()
}
/// A dialog button's title as compared: no & marks, straight apostrophes, no trailing dots, lower case.
fn button_title(title: &str) -> String {
    title.replace('&', "").replace('\u{2019}', "'").trim().trim_end_matches(['.', '\u{2026}']).to_lowercase()
}
/// Live's Separate Stems dialog (Live 12.3 on): each stem's toggle by its accessibility id, the same in every
/// language, and by its name, as Live 12.4 reads them on Windows.
const STEMS: [(&str, &str, &str); 4] = [
    ("vocals", "VocalsCheckControl", "Vocals"),
    ("drums", "DrumsCheckControl", "Drums"),
    ("bass", "BassCheckControl", "Bass"),
    ("others", "OthersCheckControl", "Others"),
];
/// Merge to Single Track: Live takes it with two or three stems only, and greys it out (keeping its state)
/// with one or four.
const MERGE_STEMS: [&str; 3] = ["MergeStems.MergeStemsCheckControl", "Merge Stems", "Merge to Single Track"];
/// Quality Mode: on for High Quality, off for High Speed.
const HIGH_QUALITY: [&str; 2] = ["HighQualityCheckControl", "Quality Mode"];
const SEPARATE: [&str; 2] = ["Separate", "SeparateButton"];
/// How long Kumi waits on Live's work after pressing a dialog's button: Separate Stems at High Quality takes
/// minutes for a whole song (rounds of 250 ms).
const WORK_ROUNDS: u32 = 20 * 60 * 4;
/// A toggle's name as Live gives it, split at its first comma: what it is ("Vocals") and what it does
/// ("Include or exclude the Vocals stem.").
fn toggle_parts(name: &str) -> (&str, Option<&str>) {
    match name.split_once(", ") {
        Some((short, about)) if !short.trim().is_empty() && !about.trim().is_empty() => (short.trim(), Some(about.trim())),
        _ => (name.trim(), None),
    }
}
/// The toggle one of `names` stands for among a dialog's: by its id, its whole name or its name before the
/// comma, in any case, the first name that finds one.
fn toggle_named<'a>(toggles: &'a [Toggle], names: &[String]) -> Option<&'a Toggle> {
    names.iter().find_map(|name| {
        let wanted = button_title(name);
        toggles.iter().find(|toggle| {
            toggle.id.as_deref().is_some_and(|id| id.to_lowercase() == wanted)
                || button_title(&toggle.name) == wanted
                || button_title(toggle_parts(&toggle.name).0) == wanted
        })
    })
}
/// A dialog as the model sees it: each toggle by its name, on or off, enabled false when Live has it greyed
/// out, and what it does when Live says.
fn shown(dialog: &hands::Dialog) -> Value {
    let mut value = serde_json::to_value(dialog).unwrap();
    if let Some(toggles) = &dialog.toggles {
        value["toggles"] = toggles
            .iter()
            .map(|toggle| {
                let (name, about) = toggle_parts(&toggle.name);
                let mut shown = json!({"name":name,"on":toggle.on});
                if !toggle.enabled {
                    shown["enabled"] = json!(false);
                }
                if let Some(about) = about {
                    shown["about"] = json!(about);
                }
                shown
            })
            .collect();
    }
    value
}
/// What the model is told to do with a dialog Kumi hands back.
fn next_for(dialog: &hands::Dialog) -> &'static str {
    if dialog.toggles.as_ref().is_some_and(|toggles| !toggles.is_empty()) {
        "Set its toggles by name with toggles and press its button with answer, both in one call, or tell the producer what it asks."
    } else {
        "Answer it with answer (the button's title), or tell the producer what it asks."
    }
}
/// A toggle asked for: the names that may stand for it (an id first, when Kumi knows it), on or off.
struct Wanted {
    names: Vec<String>,
    on: bool,
}
impl Wanted {
    fn said(&self) -> &str {
        self.names.last().map(String::as_str).unwrap_or("")
    }
}
/// What a command's dialog gets, given with the command: its toggles set, then its button pressed. For
/// separate_stems, `stems` says what Separate Stems is asked for (Kumi reports what it separated).
struct Fill {
    toggles: Vec<Wanted>,
    answer: Option<Vec<String>>,
    stems: Option<Vec<&'static str>>,
}
/// How a command's dialog went: filled, pressed and Live's work seen through (the tracks after, and what to
/// say of it); handed back (Live asked something else, or Kumi couldn't fill it), with what to do next; or no
/// dialog came.
enum Filled {
    Done(Vec<String>, JsonObject),
    Back(hands::Dialog, String),
    NoDialog,
}
/// A dialog a command handed back to the model for Live's work after it (Separate Stems, a bounce): the
/// model's answer finishes the command, so Kumi waits for that work, says what it made, and keeps it in
/// HISTORY under the command's title only then.
struct Handed {
    before: Vec<String>,
    title: String,
}
/// What Live's work after a press came to: the tracks after, a dialog Live asked instead, and whether its
/// progress window came up.
struct Worked {
    after: Vec<String>,
    asked: Option<hands::Dialog>,
    busy: bool,
}
/// The toggles input gives, in its order: each name, on (true) or off (false).
fn wanted_toggles(input: &JsonObject) -> Result<Vec<Wanted>, String> {
    let Some(given) = input.get("toggles").filter(|v| !v.is_null()) else { return Ok(vec![]) };
    let Some(given) = given.as_object() else {
        return Err("toggles names each toggle with true (on) or false (off): {\"Vocals\": true}.".into());
    };
    given
        .iter()
        .map(|(name, on)| match on.as_bool() {
            Some(on) => Ok(Wanted { names: vec![name.clone()], on }),
            None => Err(format!("toggles takes true (on) or false (off) for {name}.")),
        })
        .collect()
}
/// The tracks Live made and the ones it took away, by name: a name there more often than before counts as made.
fn changed_tracks(before: &[String], after: &[String]) -> (Vec<String>, Vec<String>) {
    let added = after
        .iter()
        .filter(|name| !before.contains(name) || after.iter().filter(|n| n == name).count() > before.iter().filter(|n| n == name).count())
        .cloned()
        .collect();
    let removed = before.iter().filter(|name| !after.contains(name)).cloned().collect();
    (added, removed)
}
/// Separate Stems filled as separate_stems asks: the stems (all four when not given), Merge to Single Track
/// (off unless asked, for two or three), Quality Mode when given (else as the producer last left it), then
/// Separate. Refused before anything is pressed when Live couldn't do it.
fn stems_fill(input: &JsonObject) -> Result<Fill, String> {
    let given = match input.get("stems").filter(|v| !v.is_null()) {
        None => STEMS.iter().map(|(stem, _, _)| stem.to_string()).collect(),
        Some(Value::Array(stems)) => stems.iter().map(|v| v.as_str().unwrap_or("").trim().to_lowercase()).collect::<Vec<_>>(),
        Some(_) => return Err("stems lists the stems to make: [\"vocals\", \"drums\"].".into()),
    };
    let mut stems = Vec::new();
    for stem in &given {
        let Some((known, _, _)) = STEMS.iter().find(|(known, _, _)| known == stem) else {
            return Err(format!("Live separates vocals, drums, bass and others; not “{stem}”."));
        };
        if !stems.contains(known) {
            stems.push(*known);
        }
    }
    if stems.is_empty() {
        return Err("Give at least one stem: vocals, drums, bass or others.".into());
    }
    stems.sort_by_key(|stem| STEMS.iter().position(|(known, _, _)| known == stem));
    let merge = input.get("merge").and_then(Value::as_bool);
    let merging = (2..=3).contains(&stems.len());
    if merge == Some(true) && !merging {
        return Err(format!("Live merges two or three stems into one track; with {} leave merge out.", stems.len()));
    }
    let quality = match input.get("quality").and_then(Value::as_str).map(|q| q.trim().to_lowercase().replace('_', " ")) {
        None => None,
        Some(q) if q == "high quality" => Some(true),
        Some(q) if q == "high speed" => Some(false),
        Some(q) => return Err(format!("quality is high quality or high speed, not “{q}”.")),
    };
    // Each stem asked for goes on before the others go off, so the dialog never has none on (Live greys out
    // Separate then).
    let mut toggles: Vec<Wanted> = STEMS
        .iter()
        .filter(|(stem, _, _)| stems.contains(stem))
        .chain(STEMS.iter().filter(|(stem, _, _)| !stems.contains(stem)))
        .map(|(stem, id, name)| Wanted { names: vec![id.to_string(), name.to_string()], on: stems.contains(stem) })
        .collect();
    if merging {
        toggles.push(Wanted { names: MERGE_STEMS.iter().map(|n| n.to_string()).collect(), on: merge == Some(true) });
    }
    if let Some(high) = quality {
        toggles.push(Wanted { names: HIGH_QUALITY.iter().map(|n| n.to_string()).collect(), on: high });
    }
    Ok(Fill { toggles, answer: Some(SEPARATE.iter().map(|n| n.to_string()).collect()), stems: Some(stems) })
}
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
    /// A Set switch set_file handed back a dialog for, which the model's answer can finish or cancel.
    switch_handed_back: Cell<bool>,
    /// A command's dialog handed back to the model, whose answer starts Live's work (Separate Stems).
    handed: RefCell<Option<Handed>>,
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
        Self {
            connection,
            history,
            remember,
            options,
            act,
            hands_setup: RefCell::new(None),
            menu_items: RefCell::new(None),
            switch_handed_back: Cell::new(false),
            handed: RefCell::new(None),
        }
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
        track_ref_in(&self.track_rows(json!(["name"]), signal).await?, named)
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
        let toggles = match wanted_toggles(input) {
            Ok(toggles) => toggles,
            Err(why) => return Ok(ToolResult::error(why)),
        };
        let answer = input.get("answer").and_then(Value::as_str);
        let named_command = input.get("command").and_then(Value::as_str);
        let menu = strings(input.get("menu"));
        let keys = strings(input.get("keys"));
        // An answer or toggles on their own are for the dialog Live has open; given with a command, for the
        // dialog that command opens.
        if named_command.is_none() && menu.is_empty() && keys.is_empty() && (answer.is_some() || !toggles.is_empty()) {
            return self.answer_dialog(answer, &toggles, signal, hands).await;
        }
        self.handed.borrow_mut().take();
        let command = named_command.and_then(|name| COMMANDS.get(name));
        if let Some(name) = named_command.filter(|_| command.is_none()) {
            return Ok(ToolResult::error(format!("Kumi doesn't know the command {name}; name the menu item instead (menu).")));
        }
        if let (Some(name), Some(command)) = (named_command, command) {
            if matches!(name, "save_as" | "new_set" | "open_set") || (name == "save" && input.contains_key("path")) {
                return self.set_file(name, command, input, signal, hands).await;
            }
        }
        if command.is_none() && menu.is_empty() && keys.is_empty() {
            return Ok(ToolResult::error("Give a command, a menu item (menu) or keys."));
        }
        let stems_given = ["stems", "quality", "merge"].iter().any(|key| input.get(*key).is_some_and(|v| !v.is_null()));
        if stems_given && named_command != Some("separate_stems") {
            return Ok(ToolResult::error("stems, quality and merge are for separate_stems."));
        }
        // What goes into the dialog the command opens: Separate Stems' stems, or the toggles and answer given.
        let fill = if named_command == Some("separate_stems") && (stems_given || (toggles.is_empty() && answer.is_none())) {
            match stems_fill(input) {
                Ok(fill) => Some(fill),
                Err(why) => return Ok(ToolResult::error(why)),
            }
        } else if !toggles.is_empty() || answer.is_some() {
            Some(Fill { toggles, answer: answer.map(|a| vec![a.to_owned()]), stems: None })
        } else {
            None
        };
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
                // Found in the tracks just read, not in another read of them for each target.
                let reference = track_ref_in(&all, one)?;
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
            // A new Set or another opened: Live drops Kumi while it loads, and this request carries on (#188).
            let switching = switches_set(&item.path);
            if switching {
                self.connection.expect_set_change(SET_OPENING_MS);
            }
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
                if switching {
                    self.connection.forget_set_change();
                }
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
            let switching = keys.iter().any(|combo| SET_KEYS.contains(&combo.to_lowercase().replace(' ', "").as_str()));
            if switching {
                self.connection.expect_set_change(SET_OPENING_MS);
            }
            let reply = hands.keys(&keys, KeysOptions { signal: Some(signal.clone()), ..Default::default() }).await?;
            if !reply.ok {
                if switching {
                    self.connection.forget_set_change();
                }
                return Ok(ToolResult::error(match reply.error.as_deref() {
                    Some("not-front") => "Kumi couldn't bring Live to the front, so the keys weren't pressed: another window may be holding the front (a dialog or a full-screen app).".into(),
                    Some("keys-refused") => "Windows refused Kumi's key presses, so nothing was pressed: another app may be blocking input.".into(),
                    error => format!("Live didn't take those keys ({}).", error.unwrap_or("undefined")),
                }));
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
                if open.open && open.buttons.as_ref().is_some_and(|b| !b.is_empty()) && !in_progress(&open) {
                    break;
                }
                delay(100, signal).await?;
            }
        }
        // The dialog the command opens, filled and answered as asked, and Live's work after it seen through.
        let (mut finished, mut dialog, mut next, mut said) = (None, None, None, JsonObject::new());
        if let Some(fill) = &fill {
            match self.fill(hands, fill, &before, signal).await? {
                Filled::Done(after, report) => (finished, said) = (Some(after), report),
                Filled::Back(open, why) => (dialog, next) = (Some(open), Some(why)),
                Filled::NoDialog => {}
            }
        }
        let filled = finished.is_some() || dialog.is_some();
        let works = command.is_some_and(|c| {
            includes(&c.titles[0], &["Bounce", "Convert", "Separate", "Slice", "Consolidate", "Flatten", "Paste Bounced"])
        });
        if works && !filled {
            delay(150, signal).await?;
            for _ in 0..1200 {
                let open = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
                if !open.open || (open.buttons.as_ref().is_some_and(|b| !b.is_empty()) && !in_progress(&open)) {
                    break;
                }
                delay(100, signal).await?;
            }
        }
        let title = command.map(|c| format!("{}{what}", c.done)).unwrap_or_else(|| format!("Pressed {pressed} in Live"));
        let asks = command.and_then(|c| c.dialog) == Some(true);
        let looks = if filled {
            0
        } else if asks {
            6
        } else {
            2
        };
        for _ in 0..looks {
            delay(if asks { 250 } else { 150 }, signal).await?;
            let open = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
            if open.open && open.buttons.as_ref().is_some_and(|b| !b.is_empty()) && !in_progress(&open) {
                dialog = Some(open);
                break;
            }
        }
        // Live asks before its work: the work, and saying it's done, come with the model's answer.
        let handing = works && dialog.is_some();
        if !handing {
            self.tell(&title);
        }
        {
            let mut refs = self.connection.references.borrow_mut();
            refs.invalidate();
            refs.clear_names();
        }
        self.connection.lease.set(self.connection.lease.get() + 1);
        let mut after = match finished.take() {
            Some(after) => after,
            None => self.track_names(signal).await?,
        };
        for _ in 0..8 {
            if dialog.is_some()
                || filled
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
        let (added, removed) = changed_tracks(&before, &after);
        let saved = if let Some(before) = saved_before { self.modified()?.is_some_and(|after| after > before) } else { false };
        if (command.is_some() || !added.is_empty() || !removed.is_empty()) && !handing {
            self.record_command(&title);
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
        out.extend(said);
        if let Some(dialog) = dialog {
            out.insert("dialog".into(), shown(&dialog));
            out.insert("next".into(), json!(next.unwrap_or_else(|| next_for(&dialog).to_owned())));
        }
        // The model's answer starts Live's work: Kumi sees it through then, and says what it made.
        if handing {
            *self.handed.borrow_mut() = Some(Handed { before, title });
        }
        out.insert("note".into(), json!("References from before are gone: discover again before using any."));
        Ok(ToolResult::text(stringify(&Value::Object(out))))
    }
    /// HISTORY's entry for what Live's own command did: kept, for Live's undo to take back.
    fn record_command(&self, title: &str) {
        let record: ChangeRecord = serde_json::from_value(json!({"id":format!("l{}",&uuid::Uuid::new_v4().to_string()[..8]),"family":"structure","title":title,"state":"kept","note":"Done with Live's own command: Live's undo (Cmd-Z) takes it back.","at":self.connection.now().timestamp_millis()})).unwrap();
        self.history.emit(&record);
    }
    /// The dialog a command opened, filled as asked: its toggles set, its button pressed, and Live's work
    /// after it seen through (Separate Stems: its progress window, then the stems' tracks).
    async fn fill(&self, hands: &dyn Hands, fill: &Fill, before: &[String], signal: &Signal) -> Result<Filled, CommandError> {
        // Live puts its dialog up a moment after the menu, and its controls a moment after that.
        let mut dialog = None;
        for _ in 0..20 {
            delay(250, signal).await?;
            let open = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
            let controls = open.buttons.as_ref().is_some_and(|b| !b.is_empty()) || open.toggles.as_ref().is_some_and(|t| !t.is_empty());
            if open.open && controls && !in_progress(&open) {
                dialog = Some(open);
                break;
            }
        }
        let Some(mut dialog) = dialog else { return Ok(Filled::NoDialog) };
        if !fill.toggles.is_empty() {
            match self.set_toggles(hands, &dialog, &fill.toggles, signal).await? {
                Ok(set) => dialog = set,
                Err(why) => {
                    let now = hands.dialog(Some(signal.clone())).await.ok().filter(|now| now.open).unwrap_or(dialog);
                    let next = format!("{why} Nothing in it was pressed. {}", next_for(&now));
                    return Ok(Filled::Back(now, next));
                }
            }
        }
        let mut report = JsonObject::new();
        if let Some(stems) = &fill.stems {
            let toggles = dialog.toggles.clone().unwrap_or_default();
            report.insert("stems".into(), json!(stems));
            if let Some(high) = toggle_named(&toggles, &HIGH_QUALITY.map(String::from)) {
                report.insert("quality".into(), json!(if high.on { "high quality" } else { "high speed" }));
            }
            // Greyed out, Merge Stems keeps its state but Live doesn't merge.
            if toggle_named(&toggles, &MERGE_STEMS.map(String::from)).is_some_and(|merge| merge.on && merge.enabled) {
                report.insert("merged".into(), json!(true));
            }
        }
        let Some(answer) = &fill.answer else {
            let next = next_for(&dialog).to_owned();
            return Ok(Filled::Back(dialog, next));
        };
        // The button by the first of its names Live has: its id stands in when its title is in another language.
        let (mut pressed, mut greyed) = (None, false);
        for name in answer {
            let reply = hands.answer(name, Some(signal.clone())).await?;
            if reply.ok {
                pressed = Some(reply.fields.get("pressed").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(name).to_owned());
                break;
            }
            greyed |= reply.error.as_deref() == Some("disabled");
        }
        let Some(pressed) = pressed else {
            let button = answer.first().map(String::as_str).unwrap_or("");
            let why = if greyed {
                format!("Live has “{button}” greyed out right now, so nothing in it was pressed.")
            } else {
                format!("Live's dialog has no “{button}” button, so nothing in it was pressed.")
            };
            let next = format!("{why} {}", next_for(&dialog));
            return Ok(Filled::Back(dialog, next));
        };
        report.insert("answered".into(), json!(pressed));
        if button_title(&pressed) == "cancel" {
            return Ok(Filled::Done(self.track_names(signal).await?, report));
        }
        match &fill.stems {
            Some(stems) => self.tell(format!("Separating stems: {}", stems.join(", "))),
            None => self.tell(format!("Pressed {pressed} in Live's dialog")),
        }
        let worked = self.finish_work(hands, before, Some(&dialog), signal).await?;
        Ok(match worked.asked {
            Some(asked) => {
                let next = next_for(&asked).to_owned();
                Filled::Back(asked, next)
            }
            None => Filled::Done(worked.after, report),
        })
    }
    /// The toggles of Live's open dialog set as asked, in order: Ok with the dialog as it is after, or what to
    /// tell the model (a toggle it doesn't have, one Live kept as it was).
    async fn set_toggles(
        &self,
        hands: &dyn Hands,
        dialog: &hands::Dialog,
        wanted: &[Wanted],
        signal: &Signal,
    ) -> Result<Result<hands::Dialog, String>, CommandError> {
        let toggles = dialog.toggles.clone().unwrap_or_default();
        let mut set = Vec::new();
        for one in wanted {
            let Some(toggle) = toggle_named(&toggles, &one.names) else {
                return Ok(Err(if toggles.is_empty() {
                    "Live's dialog has no toggles.".to_owned()
                } else {
                    format!(
                        "Live's dialog has no toggle “{}”; its toggles: {}.",
                        one.said(),
                        toggles.iter().map(|t| toggle_parts(&t.name).0).collect::<Vec<_>>().join(", ")
                    )
                }));
            };
            set.push(ToggleSet { name: toggle.name.clone(), id: toggle.id.clone(), on: one.on });
        }
        let reply = hands.toggles(&set, Some(signal.clone())).await?;
        if !reply.ok {
            return Ok(Err(match reply.error.as_deref() {
                Some("no-dialog") => "Live has no dialog open.".to_owned(),
                error => format!("Kumi couldn't set the dialog's toggles ({}).", error.unwrap_or("undefined")),
            }));
        }
        let after: Vec<Toggle> = reply.fields.get("toggles").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        for (one, asked) in wanted.iter().zip(&set) {
            let Some(now) = after.iter().find(|t| if asked.id.is_some() { t.id == asked.id } else { t.name == asked.name }) else {
                return Ok(Err(format!("Live's dialog lost its toggle “{}” while Kumi set it.", one.said())));
            };
            if now.on != asked.on {
                let state = if now.on { "on" } else { "off" };
                return Ok(Err(if now.enabled {
                    format!("Live kept “{}” {state} when Kumi set it.", toggle_parts(&now.name).0)
                } else {
                    format!("Live has “{}” greyed out right now, so it stayed {state}.", toggle_parts(&now.name).0)
                }));
            }
        }
        let mut dialog = dialog.clone();
        dialog.toggles = Some(after);
        Ok(Ok(dialog))
    }
    /// Live's work after a press, seen through: its progress window waited out (Separate Stems' comes up about
    /// 2 s after Separate in Live 12.4), then the tracks as they are. A question or an error Live asks instead
    /// comes back as the dialog; the dialog just answered, while it closes, doesn't.
    async fn finish_work(
        &self,
        hands: &dyn Hands,
        before: &[String],
        answered: Option<&hands::Dialog>,
        signal: &Signal,
    ) -> Result<Worked, CommandError> {
        let (mut busy, mut quiet, mut closing, mut seen) = (false, 0, 0, None);
        for _ in 0..WORK_ROUNDS {
            delay(250, signal).await?;
            let open = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
            if open.open && answered.is_some_and(|d| d.title == open.title && d.buttons == open.buttons) && closing < 8 {
                closing += 1;
                continue;
            }
            // A progress window, or one still putting its buttons up.
            if open.open && (in_progress(&open) || open.buttons.as_ref().is_none_or(Vec::is_empty)) {
                (busy, quiet) = (true, 0);
                continue;
            }
            if open.open {
                return Ok(Worked { after: self.track_names(signal).await?, asked: Some(open), busy });
            }
            let after = self.track_names(signal).await?;
            // Live's new tracks come in one go: read the same twice, they're all there.
            if after != before {
                if seen.as_ref() == Some(&after) {
                    return Ok(Worked { after, asked: None, busy });
                }
                seen = Some(after);
                continue;
            }
            quiet += 1;
            // Nothing new: done once its progress window has been gone a second, or when none came in 8 s.
            if (busy && quiet >= 4) || quiet >= 32 {
                return Ok(Worked { after, asked: None, busy });
            }
        }
        Ok(Worked { after: self.track_names(signal).await?, asked: None, busy })
    }
    /// The dialog Live has open answered: its toggles set as asked, then the button pressed. When a command
    /// handed it back for Live's work after it, the work is seen through and what it made said.
    async fn answer_dialog(
        &self,
        answer: Option<&str>,
        toggles: &[Wanted],
        signal: &Signal,
        hands: &dyn Hands,
    ) -> Result<ToolResult, CommandError> {
        let mut set = None;
        if !toggles.is_empty() {
            let open = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
            if !open.open {
                return Ok(ToolResult::error("Live has no dialog open."));
            }
            set = match self.set_toggles(hands, &open, toggles, signal).await? {
                Ok(dialog) => Some(dialog),
                Err(why) => return Ok(ToolResult::error(why)),
            };
            let said = toggles.iter().map(|t| format!("{} {}", t.said(), if t.on { "on" } else { "off" })).collect::<Vec<_>>();
            self.tell(format!("Set {} in Live's dialog", said.join(", ")));
        }
        let answer = match (answer, set) {
            (Some(answer), _) => answer,
            (None, Some(set)) => return Ok(ToolResult::text(stringify(&json!({"dialog":shown(&set)})))),
            (None, None) => return Ok(ToolResult::error("Give answer (a button's title) or toggles.")),
        };
        // A command's dialog, whose answer starts Live's work: read before it's pressed, to tell it from what
        // Live shows after.
        let handed = self.handed.borrow_mut().take();
        let asked = match &handed {
            Some(_) => hands.dialog(Some(signal.clone())).await.ok(),
            None => None,
        };
        let answered = hands.answer(answer, Some(signal.clone())).await?;
        if !answered.ok {
            *self.handed.borrow_mut() = handed;
            if answered.error.as_deref() == Some("disabled") {
                return Ok(ToolResult::error(format!("Live has “{answer}” greyed out right now, so nothing was pressed.")));
            }
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
        // Windows says No where macOS says Don't Save: what was pressed is said as Live says it.
        let pressed = answered.fields.get("pressed").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(answer).to_owned();
        self.tell(format!("Pressed {pressed} in Live's dialog"));
        // A switch Kumi handed back stays Kumi's until it's cancelled here.
        if self.switch_handed_back.get() && button_title(&pressed) == "cancel" {
            self.switch_handed_back.set(false);
            self.connection.forget_set_change();
        }
        let mut out = args(json!({"pressed":pressed}));
        if let Some(handed) = handed.filter(|_| button_title(&pressed) != "cancel") {
            let worked = self.finish_work(hands, &handed.before, asked.as_ref(), signal).await?;
            {
                let mut refs = self.connection.references.borrow_mut();
                refs.invalidate();
                refs.clear_names();
            }
            self.connection.lease.set(self.connection.lease.get() + 1);
            let (added, removed) = changed_tracks(&handed.before, &worked.after);
            if !added.is_empty() {
                out.insert("newTracks".into(), json!(added));
            }
            if !removed.is_empty() {
                out.insert("goneTracks".into(), json!(removed));
            }
            match worked.asked {
                Some(next) => {
                    out.insert("dialog".into(), shown(&next));
                    out.insert("next".into(), json!(next_for(&next)));
                    // Its answer still finishes the command.
                    *self.handed.borrow_mut() = Some(handed);
                }
                // Done: what Live worked on, or made, is the command's, in HISTORY too.
                None if worked.busy || !added.is_empty() || !removed.is_empty() => {
                    self.tell(&handed.title);
                    self.record_command(&handed.title);
                }
                None => {}
            }
            out.insert("note".into(), json!("References from before are gone: discover again before using any."));
            return Ok(ToolResult::text(stringify(&Value::Object(out))));
        }
        delay(200, signal).await?;
        let next = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
        if next.open {
            out.insert("dialog".into(), shown(&next));
        }
        Ok(ToolResult::text(stringify(&Value::Object(out))))
    }
    /// The Set's file through Live's own commands: save it under a name (save_as, or save with a path for a
    /// Set never saved), start a new one (new_set) or open one (open_set). Live answers with dialogs Kumi's
    /// hands fill in: its "Save changes?" prompt (save_current) and the Save and Open dialogs (path) (#189).
    /// A new or opened Set replaces the open one, and Live drops Kumi while it loads; this request carries
    /// on, waiting until Live is back with it (#188).
    async fn set_file(
        &self,
        name: &str,
        command: &Command,
        input: &JsonObject,
        signal: &Signal,
        hands: &dyn Hands,
    ) -> Result<ToolResult, CommandError> {
        let save_current = input.get("save_current").and_then(Value::as_str);
        let path = match (name, input.get("path").and_then(Value::as_str).map(trim).filter(|p| !p.is_empty())) {
            ("save_as" | "open_set", None) => {
                return Ok(ToolResult::error(format!("{name} needs path: the Set's file, as a full path (….als).")));
            }
            (_, Some(given)) => {
                let mut path = PathBuf::from(given);
                if !path.is_absolute() {
                    return Ok(ToolResult::error(format!(
                        "path is a full path, such as C:\\Music\\My Set.als or /Users/me/Music/My Set.als; {given} isn't."
                    )));
                }
                if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("als")) {
                    path = PathBuf::from(format!("{}.als", path.display()));
                }
                if name == "open_set" && !path.is_file() {
                    return Ok(ToolResult::error(format!("There's no Set at {}.", path.display())));
                }
                if name != "open_set" && !path.parent().is_some_and(|folder| folder.is_dir()) {
                    return Ok(ToolResult::error(format!(
                        "The folder {} doesn't exist.",
                        path.parent().map(|f| f.display().to_string()).unwrap_or_default()
                    )));
                }
                Some(path)
            }
            (_, None) => None,
        };
        // Kumi's hands fill Live's Save and Open dialogs on Windows only; on a Mac nothing is pressed.
        if path.is_some() && crate::system::platform() == "darwin" {
            return Ok(ToolResult::error(format!(
                "On a Mac, Kumi can't fill in Live's Save and Open dialogs yet, so nothing was pressed. Ask the producer to {} in Live.",
                match name {
                    "open_set" => "open the Set (File › Open Live Set…)",
                    "new_set" => "save the open Set (File › Save Live Set As…), then ask for the new Set again",
                    _ => "save it there (File › Save Live Set As…)",
                }
            )));
        }
        if self.menu_items.borrow().is_none() {
            let items = hands.menus(Some(signal.clone())).await?;
            *self.menu_items.borrow_mut() = Some(items);
        }
        let Some(item) = self.find_menu(Some(command), &[]) else {
            return Ok(ToolResult::error(format!("Live's menus don't have “{}” here.", command.titles[0])));
        };
        let switching = matches!(name, "new_set" | "open_set");
        let mut expected = switching.then(|| ExpectedSwitch::new(&self.connection));
        self.switch_handed_back.set(false);
        let hand_back = |expected: &mut Option<ExpectedSwitch>| {
            if let Some(expected) = expected.take() {
                expected.handed_back();
                self.switch_handed_back.set(true);
            }
        };
        // The dialog the path is for: the Open dialog for open_set; a Save dialog for the rest (new_set's path
        // is where the open Set, never saved, is saved first).
        let wanted = if name == "open_set" { "open" } else { "save" };
        let existed = path.as_ref().is_some_and(|p| p.is_file());
        // Where the Set may be written, read before anything is pressed.
        let places = path.as_deref().map(set_places).unwrap_or_default();
        let reply = hands
            .menu(&item.path, MenuOptions { signal: Some(signal.clone()), titles: command.titles.clone(), ..Default::default() })
            .await?;
        if !reply.ok {
            return Ok(ToolResult::error(if reply.error.as_deref() == Some("disabled") {
                format!("Live has “{}” greyed out right now.", item.path.join(" › "))
            } else {
                format!("Live didn't take “{}” ({}).", item.path.join(" › "), reply.error.as_deref().unwrap_or("undefined"))
            }));
        }
        let pressed = item.path.join(" › ");
        let mut out = args(json!({"pressed":pressed}));
        let (mut filled, mut away, mut quiet, mut blank) = (false, false, 0, 0);
        let rounds = if switching { SET_OPENING_MS / 250 } else { 120 };
        for round in 0..rounds {
            delay(250, signal).await?;
            if switching {
                // Live's Remote Script goes quiet while another Set loads. Without a focus feed to notice (plain
                // lines, a script), the bridge is asked each second, as the feed would.
                if !away && !self.connection.lost.get() && round % 4 == 3 {
                    if let Ok(status) = self.connection.read_status(abort::timeout(1500)).await {
                        if status.get("connected") == Some(&Value::Bool(false)) {
                            self.connection.lose_live();
                        }
                    }
                }
                away |= self.connection.lost.get();
                if away && self.connection.is_back() {
                    let opened = path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "a new Set".into());
                    self.tell(format!("{} ({opened})", command.done));
                    out.insert(if name == "new_set" { "opened".into() } else { "openedSet".into() }, json!(opened));
                    out.insert(
                        "note".into(),
                        json!("Live has the Set open now. References from before are gone: read the Set again before changing anything."),
                    );
                    return Ok(ToolResult::text(stringify(&Value::Object(out))));
                }
                if away {
                    continue;
                }
            }
            let dialog = hands.dialog(Some(signal.clone())).await.unwrap_or_default();
            // A dialog read before its controls are up (Live's Save dialog can take a second or two): looked at
            // again, for up to 5 s.
            if dialog.open
                && dialog.file.is_none()
                && dialog.buttons.as_ref().is_none_or(Vec::is_empty)
                && dialog.words.as_ref().is_none_or(Vec::is_empty)
                && blank < 20
            {
                blank += 1;
                continue;
            }
            if dialog.open && in_progress(&dialog) {
                continue;
            }
            if dialog.open {
                quiet = 0;
                let words = dialog.words.clone().unwrap_or_default().join(" ").to_lowercase();
                if words.contains("save changes") || words.contains("before closing") {
                    let Some(answer) = save_current else {
                        // Answered by hand, the Open dialog that follows would be left for no one to fill.
                        out.insert("dialog".into(), shown(&dialog));
                        out.insert("next".into(), json!("Live asks whether to save the open Set first: answer Cancel, ask the producer, then run this again with save_current (yes saves it, no discards it)."));
                        hand_back(&mut expected);
                        return Ok(ToolResult::text(stringify(&Value::Object(out))));
                    };
                    // The prompt's own button: Yes / No on Windows, Save / Don't Save on a Mac.
                    let (titles, usual): (&[&str], &str) = match answer {
                        "yes" => (&["save", "yes"], if crate::system::platform() == "darwin" { "Save" } else { "Yes" }),
                        "no" => (&["don't save", "no", "discard"], if crate::system::platform() == "darwin" { "Don't Save" } else { "No" }),
                        _ => (&["cancel"], "Cancel"),
                    };
                    let buttons = dialog.buttons.clone().unwrap_or_default();
                    let button = titles
                        .iter()
                        .find_map(|title| buttons.iter().find(|button| button_title(button) == *title))
                        .cloned()
                        .unwrap_or_else(|| usual.to_owned());
                    let answered = hands.answer(&button, Some(signal.clone())).await?;
                    if !answered.ok {
                        out.insert("dialog".into(), shown(&dialog));
                        out.insert("next".into(), json!("Kumi couldn't answer it: answer it yourself with answer, by its button's title."));
                        hand_back(&mut expected);
                        return Ok(ToolResult::text(stringify(&Value::Object(out))));
                    }
                    if answer == "cancel" {
                        out.insert("cancelled".into(), json!(true));
                        self.tell("Live kept the Set open");
                        return Ok(ToolResult::text(stringify(&Value::Object(out))));
                    }
                    continue;
                }
                // The path goes only into the dialog it's for: a path to open typed into the Save dialog of the
                // open Set would save that Set over it (#189).
                if let Some(path) = path.as_ref().filter(|_| !filled && dialog.file.as_deref() == Some(wanted)) {
                    let reply = hands.file(&path.to_string_lossy(), wanted, Some(signal.clone())).await;
                    if reply.as_ref().is_ok_and(|reply| reply.ok) {
                        filled = true;
                        continue;
                    }
                }
                // Something else: a file for a Set never saved, replacing a file, an error, a question Kumi has
                // no answer for.
                let saving = dialog.file.as_deref() == Some("save")
                    || (dialog.file.is_none() && dialog.title.as_deref().is_some_and(|t| t.to_lowercase().contains("save")));
                // Opening a Set, any Save dialog is for the open one, never saved: Live asks for it after its Open
                // dialog took the path and Yes answered its prompt (Live 12.4's order).
                let unsaved = saving && (name == "open_set" || (path.is_none() && !filled));
                // Windows asks before a save writes over a file: that's the producer's to decide. Its prompt is
                // known by its words, or, once the path given was already a file, as any two-button question
                // after the fill (Ja / Nein, Oui / Non: its buttons are in Windows' language).
                let question = dialog.buttons.as_ref().is_some_and(|buttons| buttons.len() == 2);
                let replacing = filled
                    && wanted == "save"
                    && ((existed && question)
                        || ["already exists", "replace", "既に存在", "置き換え", "已存在", "替换"].iter().any(|w| words.contains(w)));
                out.insert("dialog".into(), shown(&dialog));
                out.insert("next".into(), json!(if unsaved {
                    "Live wants a file for the open Set, which was never saved, before it opens another: answer Cancel, save it with save_as and a path, then ask again; or give save_current no to discard it.".to_owned()
                } else if replacing {
                    format!("A file is already at {}: ask the producer. Yes replaces it; No keeps it (then save under another path).", path.as_ref().map(|p| p.display().to_string()).unwrap_or_default())
                } else {
                    "Answer it with answer (the button's title), or tell the producer what it asks.".to_owned()
                }));
                hand_back(&mut expected);
                return Ok(ToolResult::text(stringify(&Value::Object(out))));
            }
            if switching {
                continue;
            }
            // Saved: the file is written (or rewritten), in a project folder Live made when that's where it went,
            // or a Set saved before saves where it is.
            let written = path.as_deref().and_then(|p| {
                set_places(p)
                    .into_iter()
                    .zip(&places)
                    .find(|((_, now), (_, then))| now.is_some() && now != then)
                    .map(|((place, _), _)| place)
            });
            quiet += 1;
            // A Set saved before is saved where it is, without a dialog: path is for one never saved.
            let in_place = name == "save" && !filled && quiet >= 8;
            if written.is_some() || in_place {
                let saved = written.as_ref().filter(|_| !in_place).map(|p| p.display().to_string()).unwrap_or_else(|| "where it is".into());
                self.tell(format!("{} {saved}", command.done));
                out.insert("saved".into(), json!(saved));
                if written.as_ref().zip(path.as_ref()).is_some_and(|(written, asked)| written != asked) {
                    out.insert(
                        "note".into(),
                        json!("Live keeps a Set in a project folder: it made one beside the path given, and saved the Set in it."),
                    );
                }
                if in_place {
                    out.insert(
                        "note".into(),
                        json!("This Set was saved before, so Live saved it where it is, without a dialog, and the path wasn't used: save_as saves it at a path. Kumi can't see that file to check it."),
                    );
                }
                return Ok(ToolResult::text(stringify(&Value::Object(out))));
            }
        }
        Ok(ToolResult::error(if switching {
            "Live hasn't opened the Set yet: it may still be loading, or showing something Kumi can't see. Check Live.".to_owned()
        } else {
            "Live didn't confirm the save in time: check Live.".to_owned()
        }))
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
            // A frame holds FRAME / 2 - 1 harmonics and only those are synthesized: read no more of a spectrum, so a
            // length in the billions can't ask for gigabytes.
            let length = length.min(wavetable::FRAME / 2 - 1);
            let values =
                (0..length).map(|i| value.get(&i.to_string()).filter(|v| !v.is_null()).map(js_number).unwrap_or(0.0)).collect::<Vec<_>>();
            Some(if values.is_empty() { vec![0.0] } else { values })
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spectrum_longer_than_a_frame_holds_reads_what_it_holds() {
        // Only the first FRAME / 2 - 1 harmonics are synthesized; a length up to u32's range reads just those.
        let keyframe = raw_keyframe(&json!({"harmonics":{"length":u32::MAX,"0":1,"1022":0.5,"1023":0.25}})).unwrap();
        let harmonics = keyframe.harmonics.unwrap();
        assert_eq!((harmonics.len(), harmonics[0], harmonics[1], harmonics[1022]), (1023, 1.0, 0.0, 0.5));
        assert_eq!(raw_harmonics(&json!({"length":3,"1":2})).unwrap(), Some(vec![0.0, 2.0, 0.0]), "a short one as it is");
        assert!(matches!(raw_harmonics(&json!({"length":4294967296.0})), Err(CommandError::Other(_))), "past u32's range, refused");
    }
}
/// A track's ref, by its ref or else its name, among `rows` (a read of the Set's tracks).
fn track_ref_in(rows: &[JsonObject], named: &str) -> Result<String, CommandError> {
    rows.iter()
        .find(|r| r.get("ref").and_then(Value::as_str) == Some(named))
        .or_else(|| rows.iter().find(|r| r.get("name").and_then(Value::as_str) == Some(named)))
        .and_then(|r| r.get("ref"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ObservationError(format!("{named} isn't a track in this turn's discovery; discover it again.")).into())
}
