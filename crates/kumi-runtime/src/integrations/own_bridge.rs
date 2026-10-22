//! The DAWs whose bridges are Kumi's own (REAPER's extension, Renoise's tool), through Kumi's host, `daw-mcp-server`:
//! one integration over the host's tools (`status`, `snapshot`, `read`, `focus`, `focus_body`), each DAW giving its
//! words, its refs and its instructions.
use crate::{
    core::{
        context::IntegrationContext,
        contracts::*,
        daw::DawProfile,
        errors::{FailureKind, KumiError, RuntimeError},
        fold::fold_tracks_with,
        phrases::{Intent, PhraseTrack},
        place::{is_project_id, unsaved, Place},
        plan::make_changes,
        project::{decide_project_with, ProjectRules},
        refs::{Book, Moved, RefScheme, Shift},
        view::{BodyKind, BodyRequest, DawFocus, FocusBody},
    },
    integrations::{
        none::no_access,
        own_catchup::{CatchUpWords, Remembered, WatchTool, CLOSE_MS},
        own_changes::{Ending, Link, OwnPlan},
        own_listening::{Flags, OwnListening},
        own_phrases::{PhraseLook, Phrased},
    },
    library::taste,
    mcp::client::{connect_mcp, BridgeDescriptor, McpEndpoint, Options},
    mcp::types::{CallToolResult, ContentBlock},
};
use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use futures::future::LocalBoxFuture;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::json::stringify,
    time::perf_now,
};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::LazyLock,
};

/// How long a call to Kumi's host waits.
pub(super) const CALL_MS: u64 = 30_000;
/// How often FOCUS's focus is read while the app shows it, unless the app says.
const FOCUS_MS: u64 = 150;
/// How often the focus is polled when nothing draws it (plain mode).
const UNDRAWN_FOCUS_MS: u64 = 1_000;
/// The most a page of the model's read asks for.
const MOST_ROWS: u64 = 200;

/// What one DAW gives the shared integration: its profile, how its host is started, its instructions, and the kinds
/// its read tool takes.
pub struct OwnDaw {
    pub profile: &'static DawProfile,
    pub descriptor: BridgeDescriptor,
    /// What `daw-mcp-server --daw` takes: "reaper", "renoise".
    pub host_flag: &'static str,
    pub instructions: &'static str,
    /// What a clear mechanical mistake looks like in this DAW, for the shared rules (`own_rules`): examples, in its words.
    pub mechanical: &'static str,
    /// The model's read tool: its name, what it says and the kinds it takes.
    pub read_tool: &'static str,
    pub read_description: &'static str,
    pub read_kinds: &'static [&'static str],
    /// The project row's word in the read tool ("project", "song").
    pub project_kind: &'static str,
    /// What the producer is looking at, from the host's `focus` (contract §4).
    pub fill_focus: fn(&Value) -> DawFocus,
    /// The bodies FOCUS can draw for this DAW, each as the host's `focus_body` kind with its ref, and read back.
    pub bodies: &'static [BodyKind],
    pub body_request: fn(&BodyRequest) -> Option<JsonObject>,
    pub to_body: fn(&BodyRequest, &Value) -> Option<FocusBody>,
    /// What Kumi can change in it, and how.
    pub changes: &'static super::own_changes::OwnChangeSet,
    /// M6: what catch-up and watch_me say of its projects, and how a watched routine is drafted as a recipe.
    pub catch_up: &'static CatchUpWords,
    /// How it makes the fixed phrases ("tempo 124", "mute drums") with no model call.
    pub phrases: &'static super::own_phrases::OwnPhrases,
    /// (#401) What its kept scripts are in, become, and take by their place (lint rules).
    pub scripts: &'static crate::core::scripts::backend::Words,
    /// (#401) Its facts for delivery to a spec: its master's row and fader, and what limits or raises the master.
    pub spec: &'static super::own_spec::OwnSpecFacts,
}

/// Starts Kumi's host for a DAW: the production one runs `daw-mcp-server`; a test's stand-in answers in its place.
pub type Connect = Rc<dyn Fn(Signal) -> LocalBoxFuture<'static, Result<Rc<dyn McpEndpoint>, RuntimeError>>>;

/// What the app gives a DAW's integration beside the neutral context.
#[derive(Clone)]
pub struct OwnOptions {
    pub context: IntegrationContext,
    /// Kumi's folder, where the host finds the bridge's secret and discovery files.
    pub kumi_home: PathBuf,
    pub connect: Option<Connect>,
}

/// One DAW's integration through Kumi's host.
pub struct OwnIntegration<S: RefScheme + 'static> {
    daw: &'static OwnDaw,
    options: OwnOptions,
    endpoint: Rc<RefCell<Option<Rc<dyn McpEndpoint>>>>,
    book: Rc<RefCell<Book<S>>>,
    /// The epoch the book's refs are from (the latest any answer showed).
    epoch: Rc<RefCell<Option<String>>>,
    connected: Rc<Cell<bool>>,
    closed: Rc<Cell<bool>>,
    watching: Cell<bool>,
    /// The track the producer has selected, as the last focus said: the timeline's page is centred on it.
    selected: Rc<RefCell<Option<String>>>,
    /// The project's identity as last observed: another one in the same tab (or another song) is another project.
    identity: Rc<RefCell<Option<String>>>,
    /// The layout the book's refs are at (Renoise's track shifts, each raising it): every request names it, so the
    /// bridge carries an older ref through the shifts it hasn't seen.
    layout: Rc<Cell<Option<u64>>>,
    /// Looks asked for again since the last one that read the project (a failed look asks for another, a few times).
    looks_again: Rc<Cell<u32>>,
    /// A look failed because the DAW was busy (a dialog): the next is asked for once the focus says it's free.
    look_when_free: Rc<Cell<bool>>,
    /// When the DAW last came back (or first answered): a DAW starting on its empty song and then loading the file
    /// isn't taken for a switch while it settles.
    back_at: Rc<Cell<Option<tokio::time::Instant>>>,
    /// The epoch each long ref was last read in: a read names its ref's, so a DAW whose refs hold across projects
    /// (REAPER's GUIDs, which a Save As copy in another tab shares) refuses one read in another project.
    seen_in: Rc<RefCell<HashMap<String, String>>>,
    /// Stops listening to the host's events.
    unlisten: RefCell<Option<Box<dyn Fn()>>>,
    /// The last look that read the project: its key and project, kept by a look that can't (as Live's are), so the
    /// conversation stays with the project while the DAW is busy or away.
    previous: RefCell<Option<(String, Option<ProjectRef>)>>,
    /// A project first seen unsaved: its identity and the key it was looked at under, which it keeps through its first
    /// save while it stays open in this run of the DAW (the conversation moves from the unsaved place to its own).
    first_unsaved: RefCell<Option<(String, String)>>,
    /// The last snapshot a look read: what a fixed phrase is made from, right after the look before it. A look that
    /// fails forgets it (the tracks may have moved since: Renoise's refs are places).
    looked: RefCell<Option<Value>>,
    /// The project's fingerprint from the last look that read it (`fingerprint_of`): kept while the DAW is busy or away.
    printed: RefCell<Option<Value>>,
    /// Kumi's changes in the DAW: HISTORY, plans and undo through the host's journal.
    changes: Rc<OwnPlan<S>>,
    /// Cancelled when the integration closes: a change under way is let go.
    lifetime: Signal,
    /// The tempo and saved file as last read (a wait in beats; the copy before a delete).
    tempo: Rc<Cell<Option<f64>>>,
    file: Rc<RefCell<Option<String>>>,
    /// Hosts started: one started again has lost its journal, so Kumi's changes before can't be undone through it.
    hosts: Cell<u32>,
    /// M6: when the project's snapshot is kept again, and what catch-up told of it.
    remembered: Rc<Remembered>,
    /// The task that keeps the snapshot again as edits settle is running.
    keeping: Cell<bool>,
    /// Listening (M5): the shared listening tools over the host's renders, captures and previews.
    listening: Rc<OwnListening<S>>,
    /// When the bridge last said what listening it can do (its status's `listening`).
    flags_at: Cell<Option<std::time::Instant>>,
    /// Each change of the DAW's playing the focus watch saw (when, playing), oldest first: whether the producer heard
    /// one of Kumi's changes.
    heard: Rc<RefCell<std::collections::VecDeque<(i64, bool)>>>,
    /// Pins from FOCUS where refs are positions (Renoise's): each by the refs it was pinned with (the TUI sends those every
    /// turn), carried through the track shifts since Kumi first saw it; None for one whose object was deleted.
    pins: Rc<RefCell<indexmap::IndexMap<(String, String), Option<Carried>>>>,
    /// Where refs are positions: the last track shifts Kumi followed (the epoch, the layout each came after, the shift), so
    /// a pin made from a FOCUS tree read a few shifts before Kumi first sees it is carried from the layout it was read at.
    shifts: Rc<RefCell<std::collections::VecDeque<(Option<String>, u64, Shift)>>>,
    /// Where each saved project was last seen, by the id its file carries.
    places: Places,
    /// (M7) The variations tool, where the DAW's profile offers it (core's, with the DAW's backend).
    variations: Option<Rc<dyn KernelTool>>,
    /// (#401) The sample map, where the DAW's profile offers it (core's, with the DAW's backend).
    mapping: Option<Rc<dyn KernelTool>>,
    /// (#401) Chopping, where the DAW's profile offers it (core's, with the DAW's backend).
    chop: Option<Rc<dyn KernelTool>>,
    /// (#401) Delivery, where the DAW's profile offers it (core's, with the DAW's backend).
    delivery: Option<Rc<dyn KernelTool>>,
    /// (#401) The routing doctor and builder, where the DAW's profile offers it (core's, with the DAW's backend).
    routing: Option<Rc<dyn KernelTool>>,
    /// (#401) Kept scripts, where the DAW's profile offers them (core's, with the shared backend).
    scripts: Option<Rc<dyn KernelTool>>,
    /// (#401) Delivery to a spec, where the DAW's profile offers it (core's, with the shared backend).
    spec: Option<Rc<dyn KernelTool>>,
    /// (#401) Cleaning up a jam, where the DAW's profile offers it (core's, with the DAW's backend).
    jam: Option<Rc<dyn KernelTool>>,
}

/// How REAPER and Renoise tell templates and a song's own folder, for core's rules on copies: a project in REAPER's
/// templates folder never keeps an id of its own; a copy in the same folder as its original is a version of the same song.
struct OwnRules;

impl ProjectRules for OwnRules {
    fn template_location(&self, path: &str) -> bool {
        std::path::Path::new(path).components().any(|part| part.as_os_str() == "ProjectTemplates")
    }
    fn project_folder(&self, path: &str) -> Option<PathBuf> {
        std::path::Path::new(path).parent().map(std::path::Path::to_path_buf)
    }
}

/// The most saved projects whose last place Kumi keeps for one DAW.
const MOST_PLACES: usize = 2000;

/// Where each saved project was last seen, by the id its file carries (REAPER's and Renoise's travel with the file, Save
/// As and all): a copy saved in another folder while its original is still there is a project of its own, by core's
/// rules ([`decide_project_with`]), so it never carries on the original's conversation, notes and goals. Kept in the
/// DAW's snapshots folder, through restarts.
struct Places {
    file: PathBuf,
    seen: RefCell<Option<indexmap::IndexMap<String, String>>>,
}

impl Places {
    fn new(kumi_home: &std::path::Path, daw: &str) -> Self {
        Self { file: kumi_home.join("bridges").join(daw).join("snapshots").join("places.json"), seen: RefCell::default() }
    }

    /// The project the file at `path` is, carrying the id `kept`: that id, or a copy's own (the one its path gives, the
    /// same each time). Only the original's own file, or where it moved, is kept as where that id was last seen.
    fn decide(&self, kept: &str, path: &str) -> String {
        let mut seen = self.seen.borrow_mut();
        // (on disk as [id, path] pairs, oldest first)
        let seen = seen.get_or_insert_with(|| {
            let pairs: Option<Vec<(String, String)>> = std::fs::read(&self.file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
            pairs.unwrap_or_default().into_iter().collect()
        });
        let (id, _) = decide_project_with(&OwnRules, Some(kept), path, seen.get(kept).map(String::as_str), false);
        if id == kept && seen.get(kept).map(String::as_str) != Some(path) && std::path::Path::new(path).exists() {
            seen.shift_remove(kept);
            seen.insert(kept.to_owned(), path.to_owned());
            if seen.len() > MOST_PLACES {
                seen.shift_remove_index(0);
            }
            // Best effort: a place that can't be kept is decided again from what this session saw.
            let part = self.file.with_extension("json.part");
            let written = self
                .file
                .parent()
                .map(std::fs::create_dir_all)
                .transpose()
                .ok()
                .and_then(|_| serde_json::to_vec(&seen.iter().collect::<Vec<_>>()).ok());
            if let Some(bytes) = written {
                if std::fs::write(&part, bytes).and_then(|_| std::fs::rename(&part, &self.file)).is_err() {
                    let _ = std::fs::remove_file(&part);
                }
            }
        }
        id
    }
}

/// A pinned node's refs as they are now, and the epoch and layout they're at: at any other, a shift went by uncarried.
#[derive(Clone, Debug)]
struct Carried {
    track_ref: String,
    node_ref: String,
    epoch: Option<String>,
    layout: Option<u64>,
}

/// How many pins an integration follows (the oldest goes first), and how many track shifts it keeps to carry a pin first
/// seen after them.
const PINS_KEPT: usize = 64;
const SHIFTS_KEPT: usize = 64;

/// Where a ref is after a shift: None for what was deleted (an empty ref stays empty).
fn carried_ref<S: RefScheme>(scheme: &S, shift: &Shift, reference: &str) -> Option<String> {
    if reference.is_empty() {
        return Some(String::new());
    }
    match scheme.moved(shift, reference) {
        Moved::Same => Some(reference.to_owned()),
        Moved::To(now) => Some(now),
        Moved::Gone => None,
    }
}

/// How often the snapshot keeper looks whether it's time.
const KEEPER_MS: u64 = 1_000;
/// How many changes of the DAW's playing an integration keeps.
const HEARD_KEPT: usize = 512;

/// How long what the bridge said it can do for listening is taken as standing (REAPER's audio may start or stop).
const FLAGS_FRESH: std::time::Duration = std::time::Duration::from_secs(20);

pub fn create_own_integration<S: RefScheme + 'static>(daw: &'static OwnDaw, options: OwnOptions) -> Rc<dyn Integration> {
    let link = Rc::new(Link {
        daw,
        endpoint: Rc::new(RefCell::new(None)),
        book: Rc::new(RefCell::new(Book::default())),
        epoch: Rc::default(),
        layout: Rc::default(),
        seen_in: Rc::default(),
        connected: Rc::default(),
        lifetime: Signal::new(),
        tempo: Rc::default(),
        file: Rc::default(),
        step_titles: RefCell::default(),
        outside_daw_undo: RefCell::default(),
        undo_unanswered: RefCell::default(),
        in_plan: RefCell::default(),
        maybe_redone: RefCell::default(),
        put_back_by_daw: RefCell::default(),
        project: Rc::default(),
        made_in: RefCell::default(),
    });
    let context = &options.context;
    // A change of Kumi's (or its undo) is kept in the project's snapshot once things settle.
    let remembered = Rc::new(Remembered::default());
    let on_change: Option<Rc<dyn Fn(ChangeRecord)>> = {
        let (remembered, on_change) = (Rc::downgrade(&remembered), context.on_change.clone());
        Some(Rc::new(move |record: ChangeRecord| {
            if let Some(remembered) = remembered.upgrade() {
                remembered.kumi_changed();
            }
            if let Some(on_change) = &on_change {
                on_change(record);
            }
        }))
    };
    let changes = OwnPlan::new(link.clone(), context.change_timeout_ms, on_change, context.on_action.clone());
    let listening = OwnListening::new(link.clone(), changes.clone(), context, &options.kumi_home);
    // (M7) the variations tool, where the DAW offers it: Renoise's patterns and phrases, REAPER's items (#401)
    let variations: Option<Rc<dyn KernelTool>> = match daw.profile.id.filter(|_| daw.profile.caps.variations) {
        Some(crate::core::daw::DawId::Renoise) => {
            let backend = super::renoise_variations::RenoiseBackend::new(changes.clone(), listening.renderer.clone());
            Some(crate::core::variations::tool::Variations::new(backend, options.kumi_home.clone()).tool())
        }
        Some(crate::core::daw::DawId::Reaper) => {
            let backend = super::reaper_variations::ReaperVariations::new(changes.clone(), listening.renderer.clone());
            Some(crate::core::variations::tool::Variations::new(backend, options.kumi_home.clone()).tool())
        }
        _ => None,
    };
    // (#401) the sample map, where the DAW offers it
    let mapping: Option<Rc<dyn KernelTool>> = (daw.profile.caps.sample_mapping && daw.profile.id == Some(crate::core::daw::DawId::Renoise))
        .then(|| {
            let backend = super::renoise_mapping::RenoiseMapping::new(changes.clone(), listening.renderer.clone());
            crate::core::mapping::tool::Mapping::new(backend).tool()
        });
    // (#401) chopping, where the DAW offers it: its lighter densities go to the variations tool as takes
    let chop: Option<Rc<dyn KernelTool>> = (daw.profile.caps.chop && daw.profile.id == Some(crate::core::daw::DawId::Renoise)).then(|| {
        let backend = super::renoise_chop::RenoiseChop::new(changes.clone(), listening.renderer.clone());
        crate::core::chop::tool::Chop::new(backend, variations.clone()).tool()
    });
    // (#401) delivery, where the DAW offers it
    let delivery: Option<Rc<dyn KernelTool>> =
        (daw.profile.caps.delivery && daw.profile.id == Some(crate::core::daw::DawId::Reaper)).then(|| {
            let backend = super::reaper_delivery::ReaperDelivery::new(changes.clone(), listening.renderer.clone());
            crate::core::delivery::tool::Delivery::new(backend).tool()
        });
    // (#401) routing, where the DAW offers it
    let routing: Option<Rc<dyn KernelTool>> =
        (daw.profile.caps.routing && daw.profile.id == Some(crate::core::daw::DawId::Reaper)).then(|| {
            let backend = super::reaper_routing::ReaperRouting::new(changes.clone());
            crate::core::routing::tool::Routing::new(backend).tool()
        });
    // (#401) kept scripts, where the DAW offers them: Kumi's copies in its folder, one a DAW
    let scripts: Option<Rc<dyn KernelTool>> = daw.profile.caps.scripts.then(|| {
        let backend = super::own_scripts::OwnScripts::new(changes.clone(), daw.scripts);
        crate::core::scripts::tool::Scripts::new(backend, options.kumi_home.join("scripts").join(daw.host_flag)).tool()
    });
    // (#401) delivery to a spec, where the DAW offers it
    let spec: Option<Rc<dyn KernelTool>> = daw.profile.caps.delivery_spec.then(|| {
        let backend = super::own_spec::OwnSpec::new(changes.clone(), listening.renderer.clone(), daw.spec);
        crate::core::spec::tool::DeliverySpec::new(backend).tool()
    });
    // (#401) cleaning up a jam, where the DAW offers it
    let jam: Option<Rc<dyn KernelTool>> = (daw.profile.caps.jam && daw.profile.id == Some(crate::core::daw::DawId::Renoise)).then(|| {
        let backend = super::renoise_jam::RenoiseJam::new(changes.clone());
        crate::core::jam::tool::Jam::new(backend).tool()
    });
    let kumi_home = options.kumi_home.clone();
    Rc::new(OwnIntegration::<S> {
        daw,
        options,
        endpoint: link.endpoint.clone(),
        book: link.book.clone(),
        epoch: link.epoch.clone(),
        connected: link.connected.clone(),
        closed: Rc::new(Cell::new(false)),
        watching: Cell::new(false),
        selected: Rc::default(),
        identity: link.project.clone(),
        layout: link.layout.clone(),
        seen_in: link.seen_in.clone(),
        back_at: Rc::default(),
        looks_again: Rc::default(),
        look_when_free: Rc::default(),
        unlisten: RefCell::default(),
        previous: RefCell::default(),
        first_unsaved: RefCell::default(),
        looked: RefCell::default(),
        printed: RefCell::default(),
        changes,
        lifetime: link.lifetime.clone(),
        tempo: link.tempo.clone(),
        file: link.file.clone(),
        hosts: Cell::new(0),
        remembered,
        keeping: Cell::new(false),
        listening,
        flags_at: Cell::new(None),
        heard: Rc::default(),
        pins: Rc::default(),
        shifts: Rc::default(),
        places: Places::new(&kumi_home, daw.host_flag),
        variations,
        mapping,
        chop,
        delivery,
        routing,
        scripts,
        spec,
        jam,
    })
}

/// The markers a look shows; more are read.
const MARKERS_SHOWN: usize = 30;
/// How long a DAW that just came back may take to load the file it was opened with (it starts on its empty song).
const SETTLE_MS: u64 = 3_000;
/// A look that ran into the DAW's own change (a file loading, a song switched mid-read) is taken again this often,
/// this far apart.
const LOOK_TRIES: u32 = 20;
const LOOK_GAP_MS: u64 = 250;
/// A look that failed while the DAW was there asks the session for another this often, a second apart.
const LOOKS_AGAIN: u32 = 20;
/// The most a read's page can hold for the model: the kernel cuts a tool's result at 64 KB.
const PAGE_TEXT_BYTES: usize = 60 * 1024;
/// The most pages a fixed phrase reads of a list (tracks, the sequence, patterns) before it leaves the request to the
/// model.
const PHRASE_PAGES: usize = 40;
/// A turn's short ref (`track:3`): the shape of a name this turn gave.
pub(super) static SHORT_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z]+:[0-9]+$").unwrap());

/// A call's arguments (`args`, in order or by name) with this turn's short refs anywhere in them made long: they sit under no
/// ref-named field, so the book's own walk passes them by.
pub(super) fn lengthen_anywhere<S: RefScheme>(book: &Book<S>, value: &Value) -> Value {
    match value {
        Value::String(text) if SHORT_REF.is_match(text) => book.lengthen(&json!({"ref": text}))["ref"].clone(),
        Value::Array(items) => Value::Array(items.iter().map(|item| lengthen_anywhere(book, item)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(key, item)| (key.clone(), lengthen_anywhere(book, item))).collect()),
        other => other.clone(),
    }
}

/// What a call answered (`result`, `out`) with the DAW's refs anywhere in it made this turn's short ones.
pub(super) fn shorten_anywhere<S: RefScheme>(book: &mut Book<S>, value: &Value) -> Value {
    match value {
        Value::String(text) if book.scheme.kind_of(text).is_some() => json!(book.short_ref(text)),
        Value::Array(items) => Value::Array(items.iter().map(|item| shorten_anywhere(book, item)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(key, item)| (key.clone(), shorten_anywhere(book, item))).collect()),
        other => other.clone(),
    }
}

/// Why a host tool answered an error: its words, and whether the DAW is there at all (it quit, or its bridge did).
pub(super) struct Refused {
    pub(super) why: String,
    pub(super) there: bool,
    /// Busy for now (a dialog, a load, a render): worth waiting for.
    pub(super) busy: bool,
    /// The host's code for it (`stale`, `changed`, `failed`…).
    pub(super) code: Option<String>,
    /// The DAW answered, and couldn't say how it ended (an undo it answered `uncertain`), rather than not answering.
    pub(super) answered: bool,
}

/// A host call's outcome, with a failure that came of the call's own signal (the caller stopped, or its time ran out) as
/// Aborted: the MCP client reports both as plain errors, and neither means the host is gone.
pub(super) fn stopped_or<T>(outcome: Result<T, RuntimeError>, signal: &Signal) -> Result<T, RuntimeError> {
    match outcome {
        Err(_) if signal.is_cancelled() => Err(RuntimeError::Aborted),
        other => other,
    }
}

/// The JSON a host tool answered with, or why it couldn't.
pub(super) fn answer_of(result: &CallToolResult) -> Result<Value, Refused> {
    let text = result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
    if result.is_error == Some(true) {
        return Err(Refused {
            why: value.get("error").and_then(Value::as_str).map(str::to_owned).unwrap_or(text),
            there: value.get("connected") != Some(&Value::Bool(false)),
            // The bridge's busy, or the host's own word that the DAW didn't answer in time (a dialog may be open).
            busy: value.get("code").and_then(Value::as_str) == Some("busy") || value.get("busy") == Some(&Value::Bool(true)),
            code: value.get("code").and_then(Value::as_str).map(str::to_owned),
            answered: value.get("answered") == Some(&Value::Bool(true)),
        });
    }
    Ok(value)
}

/// Why the DAW may answer late, in words: REAPER's `busy` code.
fn busy_words(code: &str, profile: &DawProfile) -> String {
    match code {
        "modal" => "a dialog is open".into(),
        "loading" => format!("a {} is loading", profile.words.place),
        "rendering" => "it's rendering".into(),
        "working" => "it's still running a long request (a plug-in loading, or a dialog that request opened)".into(),
        other => other.into(),
    }
}

impl<S: RefScheme + 'static> OwnIntegration<S> {
    fn set_connected(&self, connected: bool) {
        if self.connected.replace(connected) != connected || !connected {
            let state = if connected { ConnectionState::Connected } else { ConnectionState::Disconnected };
            (self.options.context.on_connection)(state, None);
        }
    }

    async fn endpoint(&self, signal: Signal) -> Result<Rc<dyn McpEndpoint>, RuntimeError> {
        if let Some(endpoint) = self.endpoint.borrow().clone() {
            return Ok(endpoint);
        }
        let endpoint = match &self.options.connect {
            Some(connect) => connect(signal).await?,
            None => {
                let args = vec![
                    "--daw".into(),
                    self.daw.host_flag.into(),
                    "--kumi-home".into(),
                    self.options.kumi_home.to_string_lossy().into_owned(),
                ];
                connect_mcp(Options { signal, args, timeout_ms: Some(CALL_MS), bridge: Some(self.daw.descriptor), ..Default::default() })
                    .await?
            }
        };
        *self.endpoint.borrow_mut() = Some(endpoint.clone());
        // A host started again has a new journal: Kumi's changes before it can't be undone through it.
        if self.hosts.replace(self.hosts.get() + 1) > 0 {
            self.changes.retire_all(Ending::Restart);
            self.remembered.host_restarted();
        }
        // Tracks inserted, removed or moved where refs are positions: the book's refs follow what they named.
        let (book, layout, epoch, pins) = (self.book.clone(), self.layout.clone(), self.epoch.clone(), self.pins.clone());
        let shifts = self.shifts.clone();
        let changes = Rc::downgrade(&self.changes);
        let listening = Rc::downgrade(&self.listening);
        let unlisten = endpoint.on_live_event(Rc::new(move |event| {
            // Scratch copies an earlier session left were removed: the producer is told.
            if event.get("name").and_then(Value::as_str) == Some("scratch_swept") {
                if let (Some(listening), Some(data)) = (listening.upgrade(), event.get("data")) {
                    listening.swept(data);
                }
                return;
            }
            // The producer undid (or redid) Kumi's changes with the DAW's own undo: HISTORY follows.
            if event.get("name").and_then(Value::as_str) == Some("undone_in_daw") {
                if let (Some(changes), Some(data)) = (changes.upgrade(), event.get("data")) {
                    changes.undone_in_daw(data);
                }
                return;
            }
            // Events were dropped on the way: a track shift among them is unknown, so positional refs are read again.
            if event.get("name").and_then(Value::as_str) == Some("events_lost") {
                if book.borrow().scheme.positional() {
                    let mut book = book.borrow_mut();
                    book.clear_names();
                    book.cursors.clear();
                    layout.set(None);
                }
                return;
            }
            if event.get("name").and_then(Value::as_str) != Some("tracks") {
                return;
            }
            let data = event.get("data").cloned().unwrap_or(Value::Null);
            let places = |key: &str| -> Vec<usize> {
                data.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|n| n as usize).collect()
            };
            // Another epoch (a swap, notifications that don't add up): the names go and the event's layout is taken.
            if data.get("epoch").and_then(Value::as_str).is_some_and(|now| epoch.borrow().as_deref().is_some_and(|before| before != now)) {
                follow_epoch(&book, &epoch, &layout, &data);
                return;
            }
            let Some(now) = data.get("layout").and_then(Value::as_u64) else { return };
            match layout.get() {
                // The next shift: the names follow it.
                Some(before) if now == before + 1 => {
                    let shift = Shift { tracks_made: places("made"), tracks_gone: places("gone"), ..Shift::default() };
                    book.borrow_mut().shift(&shift);
                    layout.set(Some(now));
                    {
                        let mut shifts = shifts.borrow_mut();
                        if shifts.len() >= SHIFTS_KEPT {
                            shifts.pop_front();
                        }
                        shifts.push_back((epoch.borrow().clone(), before, shift.clone()));
                    }
                    // The pins at the layout before follow what they named; one whose object went is told so.
                    let book = book.borrow();
                    let moved = |reference: &str| carried_ref(&book.scheme, &shift, reference);
                    for pin in pins.borrow_mut().values_mut() {
                        let Some(carried) = pin.as_mut().filter(|carried| carried.layout == Some(before)) else { continue };
                        match (moved(&carried.track_ref), moved(&carried.node_ref)) {
                            (Some(track_ref), Some(node_ref)) => {
                                (carried.track_ref, carried.node_ref, carried.layout) = (track_ref, node_ref, Some(now));
                            }
                            _ => *pin = None,
                        }
                    }
                }
                // One the book is already past (taken from an answer, or after events were lost): nothing to move.
                Some(before) if now <= before => {}
                // A gap: shifts in between are unknown, so the names go.
                Some(_) => {
                    if book.borrow().scheme.positional() {
                        let mut book = book.borrow_mut();
                        book.clear_names();
                        book.cursors.clear();
                    }
                    layout.set(Some(now));
                }
                // No layout yet (names retired): this one is where the book is.
                None => layout.set(Some(now)),
            }
        }));
        // The host went away (it exited, or its pipe closed): the next call starts another.
        let (held, gone) = (self.endpoint.clone(), Rc::downgrade(&endpoint));
        let undisconnect = endpoint.on_disconnect(Rc::new(move || {
            let same = held.borrow().as_ref().zip(gone.upgrade()).is_some_and(|(now, it)| Rc::ptr_eq(now, &it));
            if same {
                held.borrow_mut().take();
            }
        }));
        let unlisten: Box<dyn Fn()> = Box::new(move || {
            unlisten();
            undisconnect();
        });
        if let Some(before) = self.unlisten.replace(Some(unlisten)) {
            before();
        }
        Ok(endpoint)
    }

    /// A host tool's answer: Ok with its JSON, Err with why (the host's words) when it answered an error.
    /// The host stays through a failed call (the caller's stop, a time limit): only its going away (`on_disconnect`) lets
    /// the next call start another, whose journal is empty.
    async fn call(&self, tool: &str, args: Value, signal: Signal) -> Result<Result<Value, Refused>, RuntimeError> {
        let endpoint = self.endpoint(signal.clone()).await?;
        let args = args.as_object().cloned().unwrap_or_default();
        let bounded = abort::any([signal, abort::timeout(CALL_MS)]);
        stopped_or(endpoint.call(tool, args, bounded.clone()).await.map(|result| answer_of(&result)), &bounded)
    }

    /// A pin as the model sees it, in the turn's short refs. Where refs are positions, its refs are carried through the
    /// track shifts since Kumi first saw it, so they name what was pinned where it is now; once a shift went by that Kumi
    /// couldn't follow (a gap in the layouts, another epoch), or what was pinned was deleted, the pin is told by its name
    /// only, and no ref of it lands on whatever sits at its old place.
    fn pin_now(&self, pin: &PinnedNode) -> Value {
        // (where its refs were read is Kumi's to know, not the model's)
        let told = PinnedNode { layout: None, ..pin.clone() };
        if !self.book.borrow().scheme.positional() {
            return self.book.borrow_mut().shorten(&serde_json::to_value(&told).unwrap_or(Value::Null));
        }
        let place = self.pin_place(pin);
        let pin = &told;
        let mut book = self.book.borrow_mut();
        let on = pin.track.as_deref().map(|track| format!(" on “{track}”")).unwrap_or_default();
        let told = |gone: String| json!({"gone": gone, "name": pin.name, "node": pin.node, "track": pin.track});
        match place {
            Ok(carried) => {
                let mut now = pin.clone();
                (now.track_ref, now.r#ref) = (carried.track_ref, carried.node_ref);
                book.shorten(&serde_json::to_value(now).unwrap_or(Value::Null))
            }
            Err(false) => told(format!(
                "The producer pinned “{}”{on} in FOCUS, and {} has renumbered its tracks since in a way Kumi couldn't follow: find it by its name before acting on it, and say so.",
                pin.name, self.daw.profile.words.app
            )),
            Err(true) => told(format!("The producer pinned “{}”{on} in FOCUS, and it was deleted since: say so, and ask what they mean.", pin.name)),
        }
    }

    /// Where a pin's object is now, where refs are positions: else whether it was deleted (true) or a shift went by that Kumi
    /// couldn't carry it through (false). First seen, its refs are at the layout its FOCUS tree was read at (the pin says
    /// it), carried through the shifts Kumi followed since; a pin that doesn't say is never taken to be where its refs say.
    fn pin_place(&self, pin: &PinnedNode) -> Result<Carried, bool> {
        let (epoch, layout) = (self.epoch.borrow().clone(), self.layout.get());
        let key = (pin.track_ref.clone(), pin.r#ref.clone());
        let known = self.pins.borrow().get(&key).cloned();
        match known {
            Some(Some(carried)) if carried.epoch == epoch && carried.layout == layout => return Ok(carried),
            Some(Some(_)) => return Err(false),
            Some(None) => return Err(true),
            None => {}
        }
        let of_epoch = epoch.as_deref().is_some_and(|now| pin.r#ref.split_once(':').is_some_and(|(of, _)| of == now));
        let (Some(from), Some(to)) = (pin.layout, layout) else { return Err(false) };
        if !of_epoch || from > to {
            return Err(false);
        }
        let first = {
            let book = self.book.borrow();
            let shifts = self.shifts.borrow();
            let (mut track_ref, mut node_ref) = (pin.track_ref.clone(), pin.r#ref.clone());
            let mut gone = false;
            for at in from..to {
                let Some((_, _, shift)) = shifts.iter().find(|(of, before, _)| *before == at && *of == epoch) else { return Err(false) };
                match (carried_ref(&book.scheme, shift, &track_ref), carried_ref(&book.scheme, shift, &node_ref)) {
                    (Some(track), Some(node)) => (track_ref, node_ref) = (track, node),
                    _ => {
                        gone = true;
                        break;
                    }
                }
            }
            (!gone).then(|| Carried { track_ref, node_ref, epoch, layout })
        };
        let mut pins = self.pins.borrow_mut();
        if pins.len() >= PINS_KEPT {
            pins.shift_remove_index(0);
        }
        pins.insert(key, first.clone());
        first.ok_or(true)
    }

    /// Refs a new epoch made stale go: a positional DAW's refs carry their epoch; a stable DAW's never change.
    fn see_epoch(&self, epoch: Option<&str>) {
        let Some(epoch) = epoch else { return };
        let changed = self.epoch.borrow().as_deref().is_some_and(|before| before != epoch);
        if changed {
            self.layout.set(None);
            let mut book = self.book.borrow_mut();
            // Refs that hold through anything (REAPER's GUIDs) keep their names; positions retire with their epoch.
            if !book.scheme.positional() {
                book.cursors.clear();
                drop(book);
                *self.epoch.borrow_mut() = Some(epoch.to_owned());
                return;
            }
            book.refs.clear();
            book.known.clear();
            book.cursors.clear();
            book.clear_names();
        }
        *self.epoch.borrow_mut() = Some(epoch.to_owned());
    }

    /// Reads the host's focus while the app shows FOCUS, telling the app what changed: what the producer looks at and
    /// the transport (for the beat light, every read while it plays).
    fn watch_focus(&self) {
        let (on_focus, on_transport) = (self.options.context.on_focus.clone(), self.options.context.on_transport.clone());
        if self.watching.replace(true) || (on_focus.is_none() && on_transport.is_none()) {
            return;
        }
        let (endpoint, closed, daw, selected) = (self.endpoint.clone(), self.closed.clone(), self.daw, self.selected.clone());
        let (connected, identity, on_connection) =
            (self.connected.clone(), self.identity.clone(), self.options.context.on_connection.clone());
        let back_at = self.back_at.clone();
        let (book, layout, book_epoch) = (self.book.clone(), self.layout.clone(), self.epoch.clone());
        let (look_when_free, look_again) = (self.look_when_free.clone(), self.options.context.on_look_again.clone());
        let remembered = self.remembered.clone();
        let heard = self.heard.clone();
        let drawn = if self.options.context.focus_drawn { FOCUS_MS } else { UNDRAWN_FOCUS_MS };
        let period = self.options.context.focus_interval_ms.unwrap_or(drawn).max(50);
        tokio::task::spawn_local(async move {
            let (mut last, mut last_transport, mut last_epoch): (Option<Value>, Option<Value>, Option<String>) = (None, None, None);
            let lose_focus = |last: &mut Option<Value>| {
                if last.take().is_some() {
                    if let Some(on_focus) = &on_focus {
                        on_focus(None);
                    }
                }
            };
            while !closed.get() {
                tokio::time::sleep(std::time::Duration::from_millis(period)).await;
                let Some(host) = endpoint.borrow().clone() else { continue };
                let read = host.call("focus", JsonObject::new(), abort::timeout(2_000)).await.ok().map(|result| answer_of(&result));
                if closed.get() {
                    break;
                }
                let focus = match read {
                    Some(Ok(focus)) if focus.is_object() => focus,
                    // The DAW went away (it quit, or its bridge did): the session says so, and FOCUS empties with it.
                    Some(Err(Refused { there: false, .. })) => {
                        if connected.replace(false) {
                            on_connection(ConnectionState::Disconnected, Some(DisconnectCause::Live));
                        }
                        // (what changes before it's back is told as changed while it was away)
                        remembered.went_away();
                        // A DAW started again is another process: its project's identity is found afresh.
                        identity.borrow_mut().take();
                        last_epoch = None;
                        lose_focus(&mut last);
                        continue;
                    }
                    _ => {
                        lose_focus(&mut last);
                        continue;
                    }
                };
                // The DAW's change counter: the project's snapshot is kept again once the edits settle.
                remembered.noted(focus.get("changed"));
                // Back after it went away: the session looks again, once the DAW has loaded what it opened.
                if !connected.replace(true) {
                    back_at.set(Some(tokio::time::Instant::now()));
                    on_connection(ConnectionState::Connected, None);
                }
                // Another project in the same tab, or another song: the epoch moved, and the project's identity with it.
                // An epoch no event told of (a device or pattern edit in Renoise): the book follows it.
                follow_epoch(&book, &book_epoch, &layout, &focus);
                // Free again after a look failed because it was busy: the session looks again now.
                if focus.get("busy").is_none_or(Value::is_null) && look_when_free.replace(false) {
                    if let Some(look_again) = &look_again {
                        look_again();
                    }
                }
                let epoch = focus.get("epoch").and_then(Value::as_str).map(str::to_owned);
                if last_epoch.is_some() && epoch != last_epoch {
                    let mut args = JsonObject::new();
                    args.insert("kind".into(), daw.project_kind.into());
                    let row = host.call("read", args, abort::timeout(2_000)).await.ok().and_then(|result| answer_of(&result).ok());
                    let now = row.as_ref().and_then(|row| row.pointer("/items/0/identity")).and_then(Value::as_str).map(str::to_owned);
                    let before = identity.borrow().clone();
                    if now.is_some() && before.is_some() && now != before {
                        *identity.borrow_mut() = now;
                        on_connection(ConnectionState::Disconnected, Some(DisconnectCause::Set));
                        on_connection(ConnectionState::Connected, None);
                    }
                }
                last_epoch = epoch;
                *selected.borrow_mut() = focus.pointer("/selection/trackRef").and_then(Value::as_str).map(str::to_owned);
                let seen = json!({"view": focus.get("view"), "selection": focus.get("selection"), "epoch": focus.get("epoch"), "busy": focus.get("busy")});
                if last.as_ref() != Some(&seen) {
                    if let Some(on_focus) = &on_focus {
                        let mut filled = (daw.fill_focus)(&focus);
                        filled.busy = focus.get("busy").and_then(Value::as_str).map(|code| busy_words(code, daw.profile));
                        on_focus(Some(filled));
                    }
                    last = Some(seen);
                }
                let transport = focus.get("transport").cloned();
                let playing = transport.as_ref().and_then(|t| t.get("playing")) == Some(&Value::Bool(true));
                {
                    let mut heard = heard.borrow_mut();
                    if heard.back().is_none_or(|(_, was)| *was != playing) {
                        heard.push_back((chrono::Utc::now().timestamp_millis(), playing));
                        if heard.len() > HEARD_KEPT {
                            heard.pop_front();
                        }
                    }
                }
                if playing || transport != last_transport {
                    if let Some(on_transport) = &on_transport {
                        on_transport(crate::integrations::own_views::transport(&focus, perf_now()));
                    }
                    last_transport = transport;
                }
            }
        });
    }

    /// The project's snapshot: a read the DAW's own change ran into (a file loading, a song switched mid-read) is taken
    /// again, a moment later.
    async fn look(&self, signal: &Signal) -> Result<Result<Value, Refused>, RuntimeError> {
        for _ in 0..LOOK_TRIES {
            match self.call("snapshot", json!({}), signal.clone()).await? {
                Err(refused) if refused.there && matches!(refused.code.as_deref(), Some("epoch-changed" | "changed")) => {
                    signal.check()?;
                    tokio::time::sleep(std::time::Duration::from_millis(LOOK_GAP_MS)).await;
                }
                other => return Ok(other),
            }
        }
        self.call("snapshot", json!({}), signal.clone()).await
    }

    /// A look that failed while the DAW is there (busy, loading, changing): the project and the DAW's tools stay, so the
    /// conversation does; and Kumi looks again by itself, since no turn may come to.
    fn unread(&self, why: &str, busy: bool) -> Observation {
        let mut observation = self.no_access(None);
        let app = self.daw.profile.words.app;
        observation.revision = None;
        observation.label = format!("{app} — couldn't look just now: {why}");
        observation.tools = self.tools();
        observation.instructions = self.instructions();
        observation.context = stringify(&json!({
            "observedAt": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "daw": app,
            "look": format!("{app} couldn't be read just now ({why}); Kumi looks again, and {} reads it meanwhile.", self.daw.read_tool),
        }));
        // Busy (a dialog): asked for again once the focus says it's free, not every second meanwhile.
        if busy && self.watching.get() {
            self.look_when_free.set(true);
        } else {
            self.look_again_later();
        }
        observation
    }

    /// Asks the session for another look a second from now (a few times at most since the last good one).
    fn look_again_later(&self) {
        let Some(look_again) = self.options.context.on_look_again.clone() else { return };
        if self.looks_again.get() >= LOOKS_AGAIN {
            return;
        }
        self.looks_again.set(self.looks_again.get() + 1);
        let (closed, connected) = (self.closed.clone(), self.connected.clone());
        tokio::task::spawn_local(async move {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if !closed.get() && connected.get() {
                look_again();
            }
        });
    }

    /// M6: keeps the project's snapshot again as the edits settle (`Remembered::due`), while the integration lasts.
    fn keep_snapshots(&self) {
        if self.keeping.replace(true) {
            return;
        }
        let (remembered, endpoint, closed) = (self.remembered.clone(), self.endpoint.clone(), self.closed.clone());
        let (connected, lifetime) = (self.connected.clone(), self.lifetime.clone());
        tokio::task::spawn_local(async move {
            while !closed.get() {
                tokio::time::sleep(std::time::Duration::from_millis(KEEPER_MS)).await;
                if closed.get() {
                    break;
                }
                if !connected.get() || !remembered.due() {
                    continue;
                }
                let Some(host) = endpoint.borrow().clone() else { continue };
                remembered.clone().save(host, abort::any([lifetime.clone(), abort::timeout(CALL_MS)])).await;
            }
        });
    }

    /// M6, after a look at the project: catch-up for one it hasn't run for (another project, or the DAW back after it
    /// went away), else the snapshot kept again when the last one is old.
    fn remember_look(&self, identity: Option<&str>, name: &str) {
        let Some(identity) = identity.filter(|identity| !identity.is_empty()) else { return };
        let Some(host) = self.endpoint.borrow().clone() else { return };
        let remembered = self.remembered.clone();
        let signal = abort::any([self.lifetime.clone(), abort::timeout(60_000)]);
        if remembered.should_catch_up(identity) {
            let on_catch_up = self.options.context.on_catch_up.clone();
            tokio::task::spawn_local(remembered.catch_up(
                host,
                identity.to_owned(),
                name.to_owned(),
                self.daw.catch_up,
                on_catch_up,
                signal,
            ));
        } else if remembered.stale() {
            tokio::task::spawn_local(remembered.save(host, signal));
        }
    }

    /// The model's tools: the read, the DAW's changes with their plan and undo, and the listening tools it can run now.
    fn tools(&self) -> Vec<Rc<dyn KernelTool>> {
        let mut tools = vec![self.read_tool()];
        tools.extend(self.changes.tools());
        // M6: learning a routine the producer does by hand, for a recipe.
        tools.push(Rc::new(WatchTool {
            endpoint: self.endpoint.clone(),
            words: self.daw.catch_up,
            app: self.daw.profile.words.app,
            on_watch: self.options.context.on_watch.clone(),
        }));
        tools.extend(self.listening.tools());
        // M7: the variations loop, where the DAW offers it.
        if let Some(variations) = &self.variations {
            tools.push(variations.clone());
        }
        // #401: the sample map, where the DAW offers it.
        if let Some(mapping) = &self.mapping {
            tools.push(mapping.clone());
        }
        if let Some(chop) = &self.chop {
            tools.push(chop.clone());
        }
        if let Some(delivery) = &self.delivery {
            tools.push(delivery.clone());
        }
        if let Some(routing) = &self.routing {
            tools.push(routing.clone());
        }
        if let Some(scripts) = &self.scripts {
            tools.push(scripts.clone());
        }
        if let Some(spec) = &self.spec {
            tools.push(spec.clone());
        }
        if let Some(jam) = &self.jam {
            tools.push(jam.clone());
        }
        tools
    }

    /// What the bridge can do for listening, as its status says (read again once it's been a while).
    async fn read_flags(&self, signal: &Signal) {
        if self.flags_at.get().is_some_and(|at| at.elapsed() < FLAGS_FRESH) {
            return;
        }
        if let Ok(Ok(status)) = self.call("status", json!({}), signal.clone()).await {
            self.listening.renderer.set_flags(Flags::of(&status));
            if let Some(swept) = status.pointer("/listening/swept") {
                self.listening.swept(swept);
            }
            self.flags_at.set(Some(std::time::Instant::now()));
        }
    }

    fn instructions(&self) -> String {
        instructions_of(self.daw)
    }

    fn read_tool(&self) -> Rc<dyn KernelTool> {
        Rc::new(ReadTool::<S> {
            daw: self.daw,
            endpoint: self.endpoint.borrow().clone(),
            book: self.book.clone(),
            layout: self.layout.clone(),
            epoch: self.epoch.clone(),
            seen_in: self.seen_in.clone(),
            description: read_description(self.daw, &self.book.borrow().scheme),
        })
    }

    /// The snapshot once a DAW that just came back has loaded what it opened: it starts on an empty unsaved song and
    /// loads the file a moment later, and that first song is no switch. A song still unsaved after a while is one.
    async fn settled(&self, mut snapshot: Value, signal: &Signal) -> Result<Value, RuntimeError> {
        let unsaved = |snapshot: &Value| snapshot.pointer("/project/path").and_then(Value::as_str).is_none_or(str::is_empty);
        let Some(back) = self.back_at.get() else { return Ok(snapshot) };
        while unsaved(&snapshot) && back.elapsed() < std::time::Duration::from_millis(SETTLE_MS) {
            signal.check()?;
            tokio::time::sleep(std::time::Duration::from_millis(LOOK_GAP_MS)).await;
            match self.look(signal).await {
                Ok(Ok(now)) => snapshot = now,
                Err(error) if error.is_aborted() => return Err(error),
                _ => break,
            }
        }
        self.back_at.set(None);
        Ok(snapshot)
    }

    /// Every track of the project the look read: its rows, or every page of the read when it didn't list them all. None
    /// when they can't all be read now (the project changed since, or it's too big to page through for a phrase).
    async fn every_track(&self, snapshot: &Value, signal: &Signal) -> Result<Option<Vec<Value>>, RuntimeError> {
        let more = snapshot.get("truncated").is_some_and(|truncated| !truncated.is_null())
            || snapshot.get("moreTracks") == Some(&Value::Bool(true));
        if !more {
            return Ok(Some(snapshot.get("tracks").and_then(Value::as_array).cloned().unwrap_or_default()));
        }
        self.every_row(json!({"kind": "track", "limit": MOST_ROWS}), snapshot, signal).await
    }

    /// The sequence's slots, each its first beat and length in beats (its pattern's lines over the song's lines per beat).
    async fn slots(&self, snapshot: &Value, signal: &Signal) -> Result<Option<Vec<(f64, f64)>>, RuntimeError> {
        let lpb =
            ["/transport/lpb", "/project/lpb"].iter().find_map(|at| snapshot.pointer(at).and_then(Value::as_f64)).filter(|lpb| *lpb > 0.0);
        let Some(lpb) = lpb else { return Ok(None) };
        let Some(sequence) = self.every_row(json!({"kind": "sequence"}), snapshot, signal).await? else { return Ok(None) };
        let Some(patterns) = self.every_row(json!({"kind": "pattern"}), snapshot, signal).await? else { return Ok(None) };
        let lines: HashMap<&str, f64> =
            patterns.iter().filter_map(|row| Some((row.get("ref")?.as_str()?, row.get("lines")?.as_f64()?))).collect();
        let mut at = 0.0;
        let mut slots = Vec::with_capacity(sequence.len());
        for slot in &sequence {
            let Some(lines) = slot.get("patternRef").and_then(Value::as_str).and_then(|pattern| lines.get(pattern)) else {
                return Ok(None);
            };
            slots.push((at, lines / lpb));
            at += lines / lpb;
        }
        Ok(Some(slots))
    }

    /// Every row of a read, page by page, from the project the look read; None when the project changed since, a page is
    /// refused, or there are more than a phrase pages through.
    async fn every_row(&self, args: Value, snapshot: &Value, signal: &Signal) -> Result<Option<Vec<Value>>, RuntimeError> {
        let mut rows = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..PHRASE_PAGES {
            let mut ask = args.clone();
            if let Some(cursor) = &cursor {
                ask["cursor"] = cursor.clone().into();
            }
            let Ok(page) = self.call("read", ask, signal.clone()).await? else { return Ok(None) };
            if page.get("epoch") != snapshot.get("epoch") {
                return Ok(None);
            }
            rows.extend(page.get("items").and_then(Value::as_array).into_iter().flatten().cloned());
            match page.get("nextCursor").and_then(Value::as_str) {
                Some(next) => cursor = Some(next.to_owned()),
                None => return Ok(Some(rows)),
            }
        }
        Ok(None)
    }

    fn no_access(&self, why: Option<&str>) -> Observation {
        self.looked.borrow_mut().take();
        // The last project read keeps its key: a busy or quit DAW isn't another project.
        let (key, project) = self.previous.borrow().clone().unwrap_or_else(|| (unsaved(self.id()).into(), None));
        let mut observation = no_access(self.daw.profile, self.daw.instructions, &key, Utc::now(), project);
        if let Some(why) = why {
            observation.label = format!("{} — {why}", observation.label);
        }
        observation
    }

    fn id(&self) -> crate::core::daw::DawId {
        self.daw.profile.id.expect("a DAW with a bridge of Kumi's own is known")
    }

    /// The observation from the host's snapshot.
    fn observation(&self, snapshot: &Value, pinned: Option<&PinnedNode>) -> Observation {
        self.see_epoch(snapshot.get("epoch").and_then(Value::as_str));
        note_epoch(&self.seen_in, &self.book.borrow().scheme, snapshot);
        if self.layout.get().is_none() {
            self.layout.set(snapshot.get("layout").and_then(Value::as_u64));
        }
        let profile = self.daw.profile;
        let project = snapshot.get("project").cloned().unwrap_or(Value::Null);
        let name = project.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).unwrap_or("untitled").to_owned();
        let path = project.get("path").and_then(Value::as_str).filter(|path| !path.is_empty());
        // Saved: a file and an id (Renoise names an unsaved song by an id of its own until its first save keeps it), as
        // the host's snapshots are kept. Every unsaved project of the DAW shares its unsaved place, as Live's Sets do.
        // A copy of another project carries that one's id: it's a project of its own by core's rules (`Places`).
        let saved = project
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| is_project_id(id))
            .zip(path)
            .map(|(id, path)| self.places.decide(id, path));
        let saved = saved.as_deref();
        let place = match saved {
            Some(id) => Place::Saved { daw: self.id(), id: id.into() }.text(),
            None => unsaved(self.id()).into(),
        };
        let mut book = self.book.borrow_mut();
        let rows = snapshot.get("tracks").and_then(Value::as_array).into_iter().flatten();
        // Each track is named in the project's order before any ref in a row (a group's, a send's) names one out of it.
        for reference in rows.clone().filter_map(|row| row.get("ref").and_then(Value::as_str)) {
            book.short_ref(reference);
        }
        let tracks: Vec<JsonObject> = rows.filter_map(|row| book.shorten(row).as_object().cloned()).collect();
        let selected: Vec<String> =
            snapshot.pointer("/selection/trackRef").and_then(Value::as_str).map(|track| book.short_ref(track)).into_iter().collect();
        let selection = book.shorten(snapshot.get("selection").unwrap_or(&Value::Null));
        let markers = book.shorten(snapshot.get("markers").unwrap_or(&Value::Null));
        drop(book);
        let note = format!("Some tracks are folded to fit; {} reads them whole.", self.daw.read_tool);
        let read_tool = self.daw.read_tool;
        let folded = fold_tracks_with(
            &tracks,
            |track| track.get("ref").and_then(Value::as_str).is_some_and(|reference| selected.iter().any(|s| s == reference)),
            None,
            &note,
            |left| format!("{left} more tracks aren't listed; {read_tool} kind track (with cursor) reads them"),
        );
        let mut context = json!({
            "observedAt": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "daw": profile.words.app,
        });
        context[profile.words.place] = shown_project(&project, snapshot.get("transport"));
        context["transport"] = snapshot.get("transport").cloned().unwrap_or(Value::Null);
        context["tracks"] = json!(folded.tracks);
        context["selection"] = selection;
        // (m7) How the producer set the DAW up (REAPER's new-item defaults, extensions and render settings; Renoise's usual
        // pattern length, groups and tools), read each look: the instructions say to respect it.
        if let Some(setup) = snapshot.get("setup").filter(|setup| setup.is_object()) {
            context["setup"] = shown_setup(setup);
        }
        if let Some(folded_note) = folded.folded {
            context["folded"] = json!(folded_note);
        }
        if let Some(more) = folded.more_tracks {
            context["moreTracks"] = json!(more);
        }
        // A project bigger than the snapshot (REAPER names the total; Renoise says only that there are more): the model
        // is told, and how to read the rest.
        let listed = context["tracks"].as_array().map_or(0, Vec::len);
        let total = snapshot.pointer("/truncated/tracks/total").and_then(Value::as_u64);
        if let Some(total) = total.filter(|total| *total as usize > listed) {
            context["moreTracks"] = json!(format!(
                "{} tracks in all, {listed} listed here; {read_tool} kind track pages through every one",
                crate::notation::thousands(total as usize)
            ));
        } else if snapshot.get("moreTracks") == Some(&Value::Bool(true)) {
            context["moreTracks"] = json!(format!("Not every track is listed here; {read_tool} kind track pages through every one"));
        }
        // Markers within the look's budget: the rest are read. REAPER's snapshot holds some of them, and names how many
        // there are in all.
        if let Some(all) = markers.as_array() {
            let total =
                snapshot.pointer("/truncated/markers/total").and_then(Value::as_u64).map_or(0, |total| total as usize).max(all.len());
            let shown = all.len().min(MARKERS_SHOWN);
            context["markers"] = json!(all[..shown]);
            if total > shown {
                context["moreMarkers"] = json!(format!(
                    "{} more markers; {read_tool} kind marker pages through every one",
                    crate::notation::thousands(total - shown)
                ));
            }
        } else if !markers.is_null() {
            context["markers"] = markers;
        }
        if let Some(latest) = self.changes.latest() {
            context["kumiChanges"] = latest;
        }
        // What the producer pinned in FOCUS (an FX, a DSP device), in the turn's short refs.
        if let Some(pin) = pinned {
            context["pinned"] = self.pin_now(pin);
        }
        let identity = project.get("identity").and_then(Value::as_str).map(str::to_owned);
        // M6: what changed while Kumi wasn't looking, while this project stays open.
        if let Some(since) = identity.as_deref().and_then(|identity| self.remembered.since(identity)) {
            context["sinceLastTime"] = Value::Object(since);
        }
        self.remember_look(identity.as_deref(), &name);
        *self.identity.borrow_mut() = identity.clone();
        let tempo = ["/transport/tempo", "/transport/bpm", "/project/tempo", "/project/bpm"]
            .iter()
            .find_map(|at| snapshot.pointer(at).and_then(Value::as_f64));
        self.tempo.set(tempo);
        if let Some(beats) = snapshot.pointer("/transport/beatsPerBar").and_then(Value::as_f64) {
            self.listening.renderer.set_beats_per_bar(beats);
        }
        *self.file.borrow_mut() = project.get("path").and_then(Value::as_str).filter(|path| !path.is_empty()).map(str::to_owned);
        // A saved project is its id, through a restart of the DAW; an unsaved one is this run's identity for it, and keeps
        // that key once saved while it stays open: the session then carries its conversation on into the saved place.
        let key = {
            let mut first_unsaved = self.first_unsaved.borrow_mut();
            match (&identity, first_unsaved.as_ref()) {
                (Some(now), Some((was, key))) if now == was => key.clone(),
                _ => {
                    *first_unsaved = None;
                    match saved {
                        Some(id) => format!("{}:{id}", self.daw.host_flag),
                        None => {
                            let key = identity
                                .as_ref()
                                .map(|identity| format!("{}:{identity}", self.daw.host_flag))
                                .unwrap_or_else(|| place.clone());
                            *first_unsaved = identity.clone().map(|identity| (identity, key.clone()));
                            key
                        }
                    }
                }
            }
        };
        *self.previous.borrow_mut() = Some((key.clone(), saved.map(|_| ProjectRef { id: place.clone(), name: name.clone() })));
        // KUMI_TRACE=1: which project each look took the DAW to be, so a restart's key can be checked without a turn.
        if let Some(trace) = &self.options.context.on_dispatch {
            trace(format!("look: {key}"));
        }
        self.looks_again.set(0);
        *self.looked.borrow_mut() = Some(snapshot.clone());
        *self.printed.borrow_mut() = Some(fingerprint_of(snapshot));
        Observation {
            key: key.clone(),
            revision: None,
            label: format!("{} — {name}", profile.words.app),
            context: stringify(&context),
            instructions: self.instructions(),
            tools: self.tools(),
            project: saved.map(|_| ProjectRef { id: place, name: name.clone() }),
            set: identity.clone(),
            tracks: Some(tracks.iter().filter_map(|track| track.get("name").and_then(Value::as_str).map(str::to_owned)).collect()),
            saved_at: None,
            // The project as the DAW names it, for the app to show (saved or not).
            identity: Some(ProjectIdentity {
                daw: self.id(),
                id: saved.map(str::to_owned),
                name,
                file: project.get("path").and_then(Value::as_str).filter(|path| !path.is_empty()).map(str::to_owned),
                saved_at: None,
                open_identity: identity,
            }),
        }
    }
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> Integration for OwnIntegration<S> {
    fn profile(&self) -> &DawProfile {
        self.daw.profile
    }
    async fn start(&self, signal: Signal) -> Result<(), RuntimeError> {
        signal.check()?;
        if self.closed.get() {
            return Err(RuntimeError::plain("Integration is closed"));
        }
        (self.options.context.on_connection)(ConnectionState::Connecting, None);
        let status = self.call("status", json!({}), signal).await?;
        if let Ok(found) = &status {
            self.listening.renderer.set_flags(Flags::of(found));
            self.flags_at.set(Some(std::time::Instant::now()));
        }
        self.set_connected(status.is_ok_and(|status| status.get("connected") != Some(&Value::Bool(false))));
        self.watch_focus();
        self.keep_snapshots();
        Ok(())
    }
    fn steered(&self) {
        self.listening.steered();
    }
    async fn settled(&self) {
        self.listening.settled().await;
    }
    fn has_hear(&self) -> bool {
        let flags = self.listening.renderer.flags();
        self.connected.get() && flags.known && flags.render
    }
    fn has_audition(&self) -> bool {
        self.has_hear()
    }
    fn has_audio_file(&self) -> bool {
        self.connected.get() && self.listening.renderer.flags().known
    }
    async fn hear(&self, request: &HearRequest, signal: Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        self.listening.hear(request, signal).await
    }
    async fn audition(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<AuditionResult, String>, RuntimeError> {
        self.listening.audition(request, signal).await
    }
    async fn audio_file(&self, named: &str, signal: Signal) -> Result<Option<String>, RuntimeError> {
        self.listening.audio_file(named, signal).await
    }
    fn has_goal(&self) -> bool {
        self.connected.get() && self.listening.goal_ready()
    }
    async fn goal(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        self.listening.goal(request, signal).await
    }
    fn can_show(&self, kind: BodyKind) -> bool {
        self.daw.bodies.contains(&kind)
    }
    fn has_undo(&self) -> bool {
        true
    }
    async fn undo(&self, id: Option<&str>, signal: Signal) -> Result<ChangeRecord, RuntimeError> {
        if self.closed.get() || !self.connected.get() {
            let app = self.daw.profile.words.app;
            return Err(KumiError::new(FailureKind::Request, format!("Kumi isn't connected to {app}, so it can't undo.")).into());
        }
        let outcome = self.changes.history.undo(id.unwrap_or("last"), signal, false).await?;
        outcome.record.ok_or_else(|| KumiError::new(FailureKind::Request, outcome.text).into())
    }
    fn first_heard(&self, since: i64) -> Option<i64> {
        // The DAW playing (the producer's playing, as the focus watch saw it): at once when it was playing then.
        let heard = self.heard.borrow();
        let by_playing = if heard.iter().rev().find(|(at, _)| *at <= since).is_some_and(|(_, playing)| *playing) {
            Some(since)
        } else {
            heard.iter().find(|(at, playing)| *at > since && *playing).map(|(at, _)| *at)
        };
        // Kumi's own listens played out loud.
        let by_kumi = self
            .listening
            .renderer
            .played
            .borrow()
            .iter()
            .filter_map(|(from, to)| match () {
                _ if *from <= since && since <= *to => Some(since),
                _ if *from > since => Some(*from),
                _ => None,
            })
            .min();
        by_playing.into_iter().chain(by_kumi).min()
    }
    fn fingerprint(&self) -> Option<Value> {
        self.printed.borrow().clone()
    }
    /// A line in the DAW's status bar (Renoise's, through the tool's show_status), where its profile says it has one.
    async fn show_message(&self, text: &str, signal: Signal) -> Result<bool, RuntimeError> {
        if !self.daw.profile.caps.status_line || self.closed.get() || !self.connected.get() {
            return Ok(false);
        }
        Ok(self.call("show_status", json!({"text": text}), signal).await?.is_ok())
    }
    fn has_stop_live(&self) -> bool {
        true
    }
    async fn stop_live(&self, signal: Signal) -> Result<bool, RuntimeError> {
        // What Kumi plays out loud stops too (a preview isn't the DAW's playback).
        if self.listening.renderer.flags().known {
            let _ = self.call("audition_stop", json!({}), signal.clone()).await;
        }
        Ok(self.changes.history.stop_everything(signal).await)
    }
    /// Whether the DAW records now, as its transport says: playing and recording (REAPER's record, Renoise's edit mode while
    /// it plays). None when it can't say.
    async fn recording(&self, signal: Signal) -> Result<Option<bool>, RuntimeError> {
        if self.closed.get() || !self.connected.get() {
            return Ok(None);
        }
        let Ok(focus) = self.call("focus", json!({}), signal).await? else { return Ok(None) };
        let on = |key: &str| focus.pointer(&format!("/transport/{key}")).and_then(Value::as_bool);
        Ok(on("playing").map(|playing| playing && on("recording") == Some(true)))
    }
    /// A fixed phrase as the DAW's change, in the project the session's look just read: one plan, so it lands in HISTORY
    /// with Kumi's undo and Kumi says what changed. A phrase the DAW has no such thing for is refused in its words; one
    /// that can't run as asked here goes to the model.
    async fn instant(&self, intent: &Intent, signal: Signal) -> Result<Option<InstantOutcome>, RuntimeError> {
        if self.closed.get() || !self.connected.get() {
            return Ok(None);
        }
        let Some(snapshot) = self.looked.borrow().clone() else { return Ok(None) };
        let Some(rows) = self.every_track(&snapshot, &signal).await? else { return Ok(None) };
        let slots = match intent {
            Intent::Loop { .. } if self.daw.phrases.loops_by_slot => match self.slots(&snapshot, &signal).await? {
                Some(slots) => Some(slots),
                None => return Ok(None),
            },
            _ => None,
        };
        // Each track by this turn's short ref, as the look named them; the master never counts, nor a send track by place.
        let named: Vec<(String, String, bool, bool)> = {
            let mut book = self.book.borrow_mut();
            rows.iter()
                .filter_map(|row| {
                    let kind = row.get("type").and_then(Value::as_str);
                    let master = row.get("master") == Some(&Value::Bool(true)) || kind == Some("master");
                    let name = row.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
                    Some((book.short_ref(row.get("ref")?.as_str()?), name, !master && kind != Some("send"), master))
                })
                .collect()
        };
        let tracks: Vec<PhraseTrack> =
            named.iter().map(|(reference, name, own, master)| PhraseTrack { reference, name, own: *own, master: *master }).collect();
        let none = Value::Null;
        let look = PhraseLook {
            tracks: &tracks,
            project: snapshot.get("project").unwrap_or(&none),
            transport: snapshot.get("transport").unwrap_or(&none),
            slots: slots.as_deref(),
        };
        match (self.daw.phrases.plan)(intent, &look) {
            Phrased::Model => Ok(None),
            Phrased::Refused(text) => Ok(Some(InstantOutcome { text, is_error: true })),
            Phrased::Steps(steps) => {
                let plan = json!({"steps": steps, "final": true}).as_object().cloned().unwrap_or_default();
                let result = make_changes(&self.changes, plan, signal).await?;
                Ok(Some(InstantOutcome {
                    text: result.reply.filter(|reply| !reply.is_empty()).unwrap_or(result.text),
                    is_error: result.is_error,
                }))
            }
        }
    }
    /// Where refs are places (Renoise's), the pin as it is carried to now, in the turn's short refs, which the book follows
    /// from here (a recipe line typed later still names what was pinned); None when it can't be carried or was deleted.
    fn pin_for_recipe(&self, pin: &PinnedNode) -> Option<PinnedNode> {
        if !self.book.borrow().scheme.positional() {
            return Some(pin.clone());
        }
        let carried = self.pin_place(pin).ok()?;
        let mut book = self.book.borrow_mut();
        let mut now = PinnedNode { layout: None, ..pin.clone() };
        now.track_ref = if carried.track_ref.is_empty() { String::new() } else { book.short_ref(&carried.track_ref) };
        now.r#ref = book.short_ref(&carried.node_ref);
        Some(now)
    }
    async fn focus_body(&self, request: &BodyRequest, signal: Signal) -> Result<Option<FocusBody>, RuntimeError> {
        let Some(mut args) = (self.daw.body_request)(request).filter(|_| self.daw.bodies.contains(&request.kind())) else {
            return Ok(None);
        };
        // The timeline's tracks around the one the producer works on, not the first of a big project's.
        if let (BodyRequest::Timeline, Some(track)) = (request, self.selected.borrow().clone()) {
            args.insert("around".into(), track.into());
        }
        if let Some(layout) = self.layout.get().filter(|_| names_tracks(&self.book.borrow().scheme, &args)) {
            args.insert("layout".into(), layout.into());
        }
        let args = Value::Object(args);
        Ok(match self.call("focus_body", args, signal).await? {
            Ok(body) => {
                follow_epoch(&self.book, &self.epoch, &self.layout, &body);
                (self.daw.to_body)(request, &body)
            }
            Err(_) => None,
        })
    }
    async fn observe(&self, signal: Signal, hints: Option<ObserveHints>) -> Result<Observation, RuntimeError> {
        let pinned = hints.as_ref().and_then(|hints| hints.pinned.as_ref());
        signal.check()?;
        self.changes.new_turn();
        // A match run's next round goes on with its answer's rounds; a new answer starts a judged run's changes afresh.
        self.listening.new_turn(hints.as_ref().and_then(|hints| hints.continuing) == Some(true));
        if self.closed.get() {
            return Err(RuntimeError::plain("Integration is closed"));
        }
        match self.look(&signal).await {
            Ok(Ok(mut snapshot)) => {
                // A snapshot older than a shift the book has followed is taken again, carried to now.
                for _ in 0..3 {
                    if layout_of(&self.book, &self.layout, snapshot.get("layout").and_then(Value::as_u64)).await != Layout::Older {
                        break;
                    }
                    match self.look(&signal).await? {
                        Ok(now) => snapshot = now,
                        Err(_) => break,
                    }
                }
                if !self.connected.get() {
                    self.back_at.set(Some(tokio::time::Instant::now()));
                }
                let snapshot = self.settled(snapshot, &signal).await?;
                self.set_connected(true);
                self.read_flags(&signal).await;
                Ok(self.observation(&snapshot, pinned))
            }
            // Busy (a dialog, a load): still there, and still connected. Kumi waits a little for it and looks once more
            // (as Live's #256); still busy, the look keeps the project, and the focus says why it waits.
            Ok(Err(refused)) if refused.there && refused.busy => {
                for _ in 0..20 {
                    signal.check()?;
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    let busy = matches!(self.call("status", json!({}), signal.clone()).await, Ok(Ok(status)) if status.get("busy").is_some_and(|b| b != &Value::Bool(false)));
                    if !busy {
                        break;
                    }
                }
                match self.look(&signal).await {
                    Ok(Ok(snapshot)) => Ok(self.observation(&snapshot, pinned)),
                    Ok(Err(refused)) if refused.there => Ok(self.unread(&refused.why, refused.busy)),
                    Ok(Err(refused)) => Ok(self.no_access(Some(&refused.why))),
                    Err(error) if error.is_aborted() => Err(error),
                    Err(_) => Ok(self.no_access(None)),
                }
            }
            Ok(Err(refused)) if refused.there => Ok(self.unread(&refused.why, refused.busy)),
            Ok(Err(refused)) => {
                if self.connected.replace(false) {
                    (self.options.context.on_connection)(ConnectionState::Disconnected, Some(DisconnectCause::Live));
                }
                self.identity.borrow_mut().take();
                self.remembered.went_away();
                Ok(self.no_access(Some(&refused.why)))
            }
            Err(error) if error.is_aborted() => Err(error),
            Err(_) => {
                self.set_connected(false);
                self.remembered.went_away();
                Ok(self.no_access(None))
            }
        }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        // The project as Kumi leaves it: what changes after is what the next catch-up tells (bounded, as Live's).
        let host = self.endpoint.borrow().clone();
        if let Some(host) = host.filter(|_| self.connected.get() && !self.closed.get()) {
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(CLOSE_MS),
                self.remembered.clone().save(host, abort::timeout(CLOSE_MS)),
            )
            .await;
        }
        self.closed.set(true);
        self.lifetime.cancel();
        // Kumi's undo goes with its host: each row says who can still take its change back, in the DAW's words.
        self.changes.retire_all(Ending::Restart);
        if let Some(unlisten) = self.unlisten.borrow_mut().take() {
            unlisten();
        }
        let endpoint = self.endpoint.borrow_mut().take();
        if let Some(endpoint) = endpoint {
            endpoint.close().await?;
        }
        Ok(())
    }
}

/// The model's read of the DAW: a kind of thing, under a parent, a page at a time, in the turn's short refs.
struct ReadTool<S: RefScheme> {
    daw: &'static OwnDaw,
    endpoint: Option<Rc<dyn McpEndpoint>>,
    book: Rc<RefCell<Book<S>>>,
    layout: Rc<Cell<Option<u64>>>,
    epoch: Rc<RefCell<Option<String>>>,
    seen_in: Rc<RefCell<HashMap<String, String>>>,
    /// The description, with which kinds are read under which parent.
    description: String,
}

/// An answer from another epoch than the book's: Renoise moves it for a device, sample, phrase, pattern, instrument or
/// sequence edit, and no event says so. Positional names are retired, and the book takes the answer's epoch and layout.
/// (A DAW whose refs hold across epochs keeps its names; `see_epoch` looks after it.)
pub(super) fn follow_epoch<S: RefScheme>(
    book: &RefCell<Book<S>>,
    epoch: &RefCell<Option<String>>,
    layout: &Cell<Option<u64>>,
    answer: &Value,
) {
    let Some(now) = answer.get("epoch").and_then(Value::as_str) else { return };
    if !book.borrow().scheme.positional() || epoch.borrow().as_deref() == Some(now) {
        return;
    }
    // Moved only for devices, patterns or the sequence from the epoch the book is in (`kept`): the tracks (and the kinds
    // `kept.kinds` names, when it does) are where they were, and the DAW takes their refs from before, so their names
    // stay and the layout goes on as the tracks events say; every other name goes.
    let kept = answer.get("kept").filter(|kept| {
        (kept.get("tracks") == Some(&Value::Bool(true)) || kept.get("kinds").is_some_and(Value::is_array))
            && kept.get("from").and_then(Value::as_str).is_some_and(|from| epoch.borrow().as_deref() == Some(from))
    });
    if let Some(kept) = kept {
        let kinds: Vec<&str> = match kept.get("kinds").and_then(Value::as_array) {
            Some(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
            None => vec!["track"],
        };
        let survives = |scheme: &S, reference: &str| scheme.kind_of(reference).is_some_and(|kind| kinds.contains(&kind));
        let mut book = book.borrow_mut();
        let gone: Vec<String> =
            book.refs.keys().chain(book.known.keys()).filter(|reference| !survives(&book.scheme, reference)).cloned().collect();
        for reference in gone {
            book.retire(&reference);
        }
        book.cursors.clear();
        book.keep_names(survives);
        drop(book);
        *epoch.borrow_mut() = Some(now.to_owned());
        return;
    }
    if epoch.borrow().is_some() {
        let mut book = book.borrow_mut();
        book.refs.clear();
        book.known.clear();
        book.cursors.clear();
        book.clear_names();
    }
    *epoch.borrow_mut() = Some(now.to_owned());
    layout.set(answer.get("layout").and_then(Value::as_u64));
}

/// Whether a request names something on a track (a track, a device or a parameter), the refs a track shift moves: only
/// those carry the layout they were made in.
pub(super) fn names_tracks<S: RefScheme>(scheme: &S, args: &JsonObject) -> bool {
    // Any string anywhere in it (a change's `group`, a list of refs), as the tool looks; a bare track number too.
    fn walk<S: RefScheme>(scheme: &S, value: &Value, depth: usize) -> bool {
        match value {
            Value::String(text) => matches!(scheme.kind_of(text), Some("track" | "device" | "param" | "ptrack" | "slot")),
            Value::Array(items) if depth < 8 => items.iter().any(|item| walk(scheme, item, depth + 1)),
            Value::Object(row) if depth < 8 => row.values().any(|item| walk(scheme, item, depth + 1)),
            _ => false,
        }
    }
    args.get("track").is_some_and(Value::is_number) || args.values().any(|value| walk(scheme, value, 0))
}

/// An answer's layout against the book's.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Layout {
    /// The same (or the DAW's refs aren't positions): its refs name what the book's names do.
    Same,
    /// Newer, and no tracks event brought the book there (one was lost): the book's names went, and it takes the answer's.
    Adopted,
    /// Older than the book (a shift landed while it was on its way): read it again, carried to the layout of now.
    Older,
}

/// Where an answer's layout stands against the book's. A tracks event on its way is waited for a moment first.
pub(super) async fn layout_of<S: RefScheme>(book: &RefCell<Book<S>>, layout: &Cell<Option<u64>>, answered: Option<u64>) -> Layout {
    let Some(answered) = answered.filter(|_| book.borrow().scheme.positional()) else { return Layout::Same };
    for _ in 0..4 {
        match layout.get() {
            None => {
                layout.set(Some(answered));
                return Layout::Same;
            }
            Some(now) if now == answered => return Layout::Same,
            Some(now) if now > answered => return Layout::Older,
            Some(_) => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
        }
    }
    let mut book = book.borrow_mut();
    book.clear_names();
    book.cursors.clear();
    layout.set(Some(answered));
    Layout::Adopted
}

/// Keeps the epoch an answer was read in for each long ref it names.
pub(super) fn note_epoch(seen_in: &RefCell<HashMap<String, String>>, scheme: &impl RefScheme, answer: &Value) {
    let Some(epoch) = answer.get("epoch").and_then(Value::as_str) else { return };
    let text = answer.to_string();
    let mut seen_in = seen_in.borrow_mut();
    for reference in scheme.refs_in(&text) {
        seen_in.insert(reference.to_owned(), epoch.to_owned());
    }
}

/// The project as the model sees it: its name and state, without Kumi's identity for it or the folders it's in (its file
/// name stays), and without what the transport tells once already.
fn shown_project(project: &Value, transport: Option<&Value>) -> Value {
    let mut shown = project.clone();
    if let Some(row) = shown.as_object_mut() {
        row.remove("identity");
        if let Some(path) = row.remove("path").and_then(|path| path.as_str().map(str::to_owned)).filter(|path| !path.is_empty()) {
            let file = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned();
            row.insert("file".into(), json!(file));
        }
        if transport.is_some_and(|transport| transport.get("playing").is_some()) {
            row.remove("playState");
        }
        if transport.is_some_and(|transport| transport.get("loop").is_some()) {
            row.remove("loop");
        }
    }
    shown
}

/// The instructions a DAW's turns carry: its own, the rules every DAW shares, its changes', and what catch-up's summary is.
pub fn instructions_of(daw: &OwnDaw) -> String {
    let place = daw.profile.words.place;
    format!(
        "{} {} {} If the observation has sinceLastTime, the {place} changed while Kumi wasn't running (a short, possibly incomplete summary); take it into account and mention it briefly when it matters.",
        daw.instructions,
        own_rules(daw),
        daw.changes.instructions
    )
}

/// The rules Kumi works by in every DAW whose bridge is its own (core's, the same everywhere), in the DAW's words; the DAW
/// gives only its facts (its setup's fields in its instructions, its mechanical mistakes in `OwnDaw::mechanical`). Two equal
/// routes are a mistake to fix; a route through FX is the producer's sound, so a choice to ask about.
pub fn own_rules(daw: &OwnDaw) -> String {
    let app = daw.profile.words.app;
    let mut rules = format!(
        "The look's setup is how this producer set {app} up: follow it rather than {app}'s factory defaults. When the producer reports a problem: a clear mechanical mistake with one obvious fix ({}) gets that fix as one change, said with what you did and how to take it back (Kumi's undo or {app}'s own); when the fix means choosing between the producer's own routes or settings (one route running through FX, two settings they may have meant), name them exactly and ask one question. Never ask only because nothing is selected. Never change {app}'s preferences, key bindings, mouse modifiers or theme. Describe each change in words, naming what it touched and where. Offer alternatives as separate takes, each undoable, and never make a whole song or part unasked.",
        daw.mechanical
    );
    // (#401, R35) after time away, what's new first
    rules.push_str(" After a catch-up, start from what changed since last time: say what's new, and propose or change only that, never what came before.");
    // (#401) a chore kept as a script: once it ran, and only once the producer said yes
    if daw.profile.caps.scripts {
        rules.push_str(" A chore the producer repeats can be kept as a script (scripts): list what's kept first; a new one runs once with run_lua, and is kept only once they said yes.");
    }
    rules
}

/// The setup as the model sees it: a render folder outside the project is named by its last folder, as the project's file is
/// (the folders it's in stay on this computer).
fn shown_setup(setup: &Value) -> Value {
    let mut shown = setup.clone();
    if let Some(folder) = shown.pointer_mut("/render/folder") {
        let absolute = |path: &&str| path.starts_with(['/', '~', '\\']) || matches!(path.get(1..3), Some(":\\" | ":/"));
        let last = folder
            .as_str()
            .filter(absolute)
            .map(|path| path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or_default().to_owned());
        if let Some(last) = last {
            *folder = json!(format!("…/{last}"));
        }
    }
    shown
}

/// A project's fingerprint, as Kumi logs it with the producer's reactions (Live's, from what a look reads): its name,
/// tempo and meter, how many tracks play each role and the words in their names. A look doesn't read the FX on each
/// track, so their kinds aren't in it.
fn fingerprint_of(snapshot: &Value) -> Value {
    let at = |paths: &[&str]| paths.iter().find_map(|at| snapshot.pointer(at).filter(|value| !value.is_null())).cloned();
    let meter = at(&["/transport/timeSignature", "/project/timeSignature"])
        .and_then(|signature| Some(format!("{}/{}", signature.get(0)?.as_u64()?, signature.get(1)?.as_u64()?)));
    let (mut roles, mut tokens) = (std::collections::BTreeMap::<&str, usize>::new(), Vec::<String>::new());
    for name in snapshot.get("tracks").and_then(Value::as_array).into_iter().flatten().filter_map(|track| track.get("name")?.as_str()) {
        if let Some(role) = taste::role_of(name, &[]) {
            *roles.entry(role.as_str()).or_default() += 1;
        }
        for word in taste::name_words(name) {
            if word.chars().any(|c| c.is_ascii_alphabetic()) && !tokens.contains(&word) && tokens.len() < 32 {
                tokens.push(word);
            }
        }
    }
    json!({
        "name": snapshot.pointer("/project/name"),
        "tempo": at(&["/transport/tempo", "/transport/bpm", "/project/tempo", "/project/bpm"]),
        "meter": meter,
        "scale": null,
        "roles": roles,
        "tokens": tokens,
    })
}

/// The read tool's description: what it reads, and for each kind read inside something, its parent.
fn read_description(daw: &OwnDaw, scheme: &impl RefScheme) -> String {
    let parents: Vec<String> = daw
        .read_kinds
        .iter()
        .filter_map(|kind| scheme.parents(kind).map(|parents| format!("{kind} under a {}", parents.join(" or "))))
        .collect();
    let line = if daw.read_kinds.contains(&"line") {
        " A pattern's lines need the pattern as parent and the track as trackRef; hidden: true reads the columns the track hides too."
    } else {
        ""
    };
    format!(
        "{} Pass ref alone to read one object again. Read inside something with its ref as parent: {}.{line}",
        daw.read_description,
        parents.join("; ")
    )
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> KernelTool for ReadTool<S> {
    fn name(&self) -> &str {
        self.daw.read_tool
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        let app = self.daw.profile.words.app;
        let mut schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "kind": {"type": "string", "enum": self.daw.read_kinds, "description": format!("What to read in {app} (or pass ref alone)")},
                "ref": {"type": "string", "description": "One object's ref, from this turn: reads it alone, as it is now"},
                "parent": {"type": "string", "description": "The ref of what it's in (a track's items, an FX's parameters…), from this turn"},
                "cursor": {"type": "string", "description": "The cursor an earlier page gave, for the next page"},
                "limit": {"type": "integer", "minimum": 1, "maximum": MOST_ROWS, "description": "How many rows (50 by default)"},
            },
        });
        // A REAPER API function that only reads, by name, with its arguments.
        if self.daw.read_kinds.contains(&"call") {
            schema["properties"]["fn"] = json!({"type": "string", "description": "For kind call: a REAPER API function that only reads (GetMediaTrackInfo_Value, TrackFX_GetParamName…)"});
            schema["properties"]["args"] = json!({"type": ["array", "object"], "description": "For kind call: its arguments, in order or by the SDK's names; tracks, items, takes and envelopes as refs"});
        }
        // A tracker's lines are read one track at a time.
        if self.daw.read_kinds.contains(&"line") {
            schema["properties"]["trackRef"] = json!({"type": "string", "description": "For kind line: the track (its ref, from this turn) whose lines of the parent pattern to read"});
            schema["properties"]["hidden"] = json!({"type": "boolean", "description": "For kind line: also read the note and effect columns the track hides (a row says hidden when they hold something)"});
        }
        schema.as_object().cloned().unwrap_or_default()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        signal.check()?;
        let Some(endpoint) = self.endpoint.clone() else {
            return Ok(ToolResult::error(self.daw.profile.vocab.cant_reach));
        };
        let args = self.book.borrow().lengthen(&Value::Object(input.clone()));
        let mut args = args.as_object().cloned().unwrap_or_default();
        if let Some(call_args) = args.get("args").cloned() {
            args.insert("args".into(), lengthen_anywhere(&self.book.borrow(), &call_args));
        }
        // A short ref this turn doesn't know (an earlier song's, or made up) is refused in Kumi's words, not the DAW's.
        for key in ["ref", "parent", "trackRef"] {
            let (Some(asked), Some(sent)) = (input.get(key).and_then(Value::as_str), args.get(key).and_then(Value::as_str)) else {
                continue;
            };
            if asked == sent && SHORT_REF.is_match(asked) {
                let place = self.daw.profile.words.place;
                return Ok(ToolResult::error(format!(
                    "{asked} isn't one of this turn's refs (it may be from before the {place} changed): read again for a current one"
                )));
            }
        }
        // The host's name for the track whose lines are read.
        if let Some(track) = args.remove("trackRef") {
            args.insert("track".into(), track);
        }
        // Only refs on a track are carried through track shifts: a request without one names no layout.
        if let Some(layout) = self.layout.get().filter(|_| names_tracks(&self.book.borrow().scheme, &args)) {
            args.insert("layout".into(), layout.into());
        }
        // Refs that hold across projects are read only in the project they came from: the DAW refuses another (`stale`).
        let primary = args.get("ref").or_else(|| args.get("parent")).and_then(Value::as_str).map(str::to_owned);
        let epoch = primary.and_then(|reference| self.seen_in.borrow().get(&reference).cloned());
        if let Some(epoch) = epoch.filter(|_| !self.book.borrow().scheme.positional()) {
            args.insert("epoch".into(), epoch.into());
        }
        // A page the model can take whole: past the kernel's cut its nextCursor would be lost, so fewer rows are asked for
        // (a few times at most, should the bridge not take fewer).
        let mut tries = 0;
        loop {
            let result = endpoint.call("read", args.clone(), abort::any([signal.clone(), abort::timeout(CALL_MS)])).await?;
            match answer_of(&result) {
                Ok(page) => {
                    follow_epoch(&self.book, &self.epoch, &self.layout, &page);
                    if layout_of(&self.book, &self.layout, page.get("layout").and_then(Value::as_u64)).await == Layout::Older {
                        if tries < 4 {
                            tries += 1;
                            continue;
                        }
                        // Still from before a shift the book follows: its refs would get the wrong names.
                        let app = self.daw.profile.words.app;
                        return Ok(ToolResult::error(format!("{app} changed while reading (tracks were added or removed): read it again")));
                    }
                    note_epoch(&self.seen_in, &self.book.borrow().scheme, &page);
                    let mut page = page;
                    // (a call's answer holds refs under no ref-named field)
                    for key in ["result", "out"] {
                        if let Some(value) = page.get(key).cloned() {
                            page[key] = shorten_anywhere(&mut self.book.borrow_mut(), &value);
                        }
                    }
                    let text = stringify(&self.book.borrow_mut().shorten(&page));
                    let rows = page.get("items").and_then(Value::as_array).map_or(0, Vec::len);
                    let asked = args.get("limit").and_then(Value::as_u64).map_or(rows, |limit| (limit as usize).min(rows));
                    tries += 1;
                    if text.len() > PAGE_TEXT_BYTES && asked > 1 && tries < 5 {
                        args.insert("limit".into(), (asked / 2).into());
                        continue;
                    }
                    return Ok(ToolResult::text(text));
                }
                // A ref from another project, or a list that moved between pages: what to do, in Kumi's words.
                Err(Refused { code: Some(code), .. }) if code == "stale" || code == "changed" => {
                    let words = self.daw.profile.words;
                    let what = input.get("ref").or_else(|| input.get("parent")).and_then(Value::as_str).unwrap_or("That ref");
                    let positional = self.book.borrow().scheme.positional();
                    return Ok(ToolResult::error(match code.as_str() {
                        "stale" if positional => {
                            format!("{what} is from before the {} changed (something was added, removed or opened): read again for current refs", words.place)
                        }
                        "stale" => format!(
                            "{what} was read in another {} than the one {} has open now: read again for current refs",
                            words.place, words.app
                        ),
                        _ => format!(
                            "{} changed while reading (something was added or removed): read it again from the start, without the cursor",
                            words.app
                        ),
                    }));
                }
                // The DAW's words, with its long refs in this turn's short ones.
                Err(refused) => {
                    let mut why = refused.why;
                    let mut book = self.book.borrow_mut();
                    for reference in book.scheme.refs_in(&why.clone()) {
                        let short = book.short_ref(reference);
                        why = why.replace(reference, &short);
                    }
                    return Ok(ToolResult::error(why));
                }
            }
        }
    }
    fn reach(&self) -> Reach {
        Reach::DawRead
    }
}
