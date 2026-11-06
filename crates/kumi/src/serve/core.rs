//! The runtime core's operations (W1). `status` says what the runtime is and what it can reach; the DAW connection,
//! looks, reads, changes, undo, HISTORY and setup join it.

use super::connection::{self, Project};
use super::ctx::{reads, Call, Ctx, Effect, JsonObject, OpError, OpFuture, Operation};
use super::PROTOCOL;
use kumi_agent::core::contracts::ObserveHints;
use kumi_types::daw::DawId;
use serde_json::{json, Value};

pub static STATUS: Operation = Operation {
    name: "status",
    description: "What the runtime is and what it reaches: its version and protocol, the DAW it works with, whether Kumi's \
                  part is installed in that DAW and the DAW is open, whether the runtime is connected and to which project, \
                  who holds the DAW for writing, and the operations it offers. Changes nothing.",
    schema: no_arguments,
    effect: reads,
    run: run_status,
};

fn no_arguments() -> Value {
    json!({"type": "object", "additionalProperties": false, "properties": {}})
}

fn run_status(ctx: &Ctx, _call: Call) -> OpFuture<'_> {
    Box::pin(async move { Ok(publish_status(ctx, false).await) })
}

pub static CONNECT: Operation = Operation {
    name: "connect",
    description: "Connects the runtime to the DAW through Kumi's part in it, and takes a first look at the open project. \
                  Writes nothing into the DAW or the project. Answers as status does.",
    schema: connect_schema,
    effect: connects,
    run: run_connect,
};

fn connect_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "properties": {
        "daw": {"type": "string", "enum": ["ableton", "reaper", "renoise"], "description": "The DAW, when you'd check it's the one this runtime works with"}
    }})
}

fn connects(_: &JsonObject) -> Effect {
    Effect::Connect
}

fn run_connect(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        if let Some(daw) = call.args.get("daw").and_then(Value::as_str).filter(|daw| *daw != ctx.daw().as_str()) {
            let asked = DawId::parse(daw).map_or(daw, |asked| asked.profile().words.app);
            let reason = format!(
                "Kumi's runtime works in {}, not {asked}: set Kumi's DAW to {asked} in Hermes's settings for the Kumi plugin. It takes effect once the chat that started this runtime ends.",
                ctx.daw().profile().words.app
            );
            return Err(OpError::Refused { code: "other_daw", reason, owner: None });
        }
        let connection = connection::connect(ctx, call.signal.clone()).await?;
        // A first look, for the project's key and epoch: a DAW that doesn't answer yet, or a look that couldn't read the
        // project, leaves it for observe.
        if let Some(observation) = connection.integration.observe(call.signal.clone(), every_track()).await.ok().filter(read_the_project) {
            ctx.link().saw(&observation);
            connection::publish_focus(&ctx.handle());
        }
        Ok(publish_status(ctx, false).await)
    })
}

pub static OBSERVE: Operation = Operation {
    name: "observe",
    description: "Looks at the open project now: `project` gives the whole turn's look (what's selected and what \"this\" \
                  means, Kumi's changes and what changed since the last look first, then the steps a change may use, one \
                  line each, and the tracks with their refs, folded round the one in focus), `focus` what the producer \
                  has selected and the transport, `catch_up` what changed since the last look. Every answer has the \
                  project's key and epoch, which a change names. Writes nothing into the DAW.",
    schema: observe_schema,
    effect: reads,
    run: run_observe,
};

/// The longest `text` an observation gives when it isn't told (`max_chars`), and the bounds of what it may be told: room
/// for a whole turn's look (worklist item 3).
const MAX_CHARS: (u64, u64, u64) = (12000, 200, 20000);

fn observe_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["scope"], "properties": {
        "scope": {"type": "string", "enum": ["focus", "catch_up", "project"]},
        "max_chars": {"type": "integer", "minimum": MAX_CHARS.1, "maximum": MAX_CHARS.2, "description": format!("The longest text to answer with ({} if left out)", MAX_CHARS.0)}
    }})
}

/// What serve asks of a look: every track the DAW has, Live's returns and Main after its tracks, so an approval names
/// Main and FOCUS's Main is one the look shows (Kumi's own look leaves them out).
fn every_track() -> Option<ObserveHints> {
    Some(ObserveHints { every_track: true, ..ObserveHints::default() })
}

fn run_observe(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        let connection = ctx.connection()?;
        let observation = connection
            .integration
            .observe(call.signal.clone(), every_track())
            .await
            .map_err(|error| OpError::Failed { reason: format!("Kumi couldn't look at the project: {error}") })?;
        // A look that couldn't read the project (the DAW gone, busy or not answering) is no project: it's said in the
        // DAW's words, and the last project read stays the one a plan names.
        if !read_the_project(&observation) {
            let app = ctx.daw().profile().words.app;
            let why = observation.label.strip_prefix("Inference-only — ").unwrap_or(&observation.label);
            return Err(OpError::Failed {
                reason: format!(
                    "Kumi can't see the project in {app} right now ({why}). It looks again by itself once {app} answers; nothing from an earlier look is current."
                ),
            });
        }
        let project = ctx.link().saw(&observation);
        ctx.link().clear_unsure();
        let scope = call.args.get("scope").and_then(Value::as_str).unwrap_or("project");
        // A look at FOCUS leaves what changed for the look that asks for it (catch_up, or the whole project).
        let catch_up = if scope == "focus" { connection.catch_up() } else { connection.take_catch_up() };
        let (focus, transport) = (connection.focus(), connection.transport());
        let most = call.args.get("max_chars").and_then(Value::as_u64).unwrap_or(MAX_CHARS.0) as usize;
        // The context as JSON, read once: what the look says, and the names it gives the refs a plan's steps take.
        let fields = match serde_json::from_str::<Value>(&observation.context) {
            Ok(Value::Object(fields)) => Some(fields),
            _ => None,
        };
        let names = fields.as_ref().map(connection::look_names).unwrap_or_default();
        let volumes = fields.as_ref().map(connection::look_volumes).unwrap_or_default();
        let text = match scope {
            "focus" => focus_text(&project, focus.as_ref(), transport.as_ref()),
            "catch_up" if catch_up.is_empty() => format!("Nothing has changed in {} since the last look.", project.name),
            "catch_up" => catch_up
                .iter()
                .flat_map(|seen| seen.lines.iter().cloned().chain((seen.more > 0).then(|| format!("…and {} more", seen.more))))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => match fields {
                Some(fields) => {
                    let tools = ctx.link().tools();
                    super::turn::whole_turn(fields, &super::read::turn_steps(ctx, &tools), &super::read::reader_names(&tools), most)
                }
                // A context that isn't a JSON object is said as it is.
                None => observation.context.clone(),
            },
        };
        let refs = connection::refs(&connection, call.signal.clone()).await;
        ctx.link().keep_refs(&refs, names);
        ctx.link().keep_volumes(volumes);
        connection::publish_focus(&ctx.handle());
        publish_status(ctx, false).await;
        Ok(json!({
            "text": bounded(&text, most),
            "project_key": project.key,
            "epoch": project.epoch,
            "project": project_value(&project),
            "focus": focus.as_ref().map(connection::focus_value),
            "transport": transport.as_ref().map(connection::transport_value),
            "catch_up": catch_up,
            "refs": refs,
            "age_ms": kumi_common::time::now_ms() - project.seen_at,
        }))
    })
}

/// Whether a look read the project: one that couldn't (the DAW gone, busy or not answering) has neither the project's
/// identity nor the open project's, only the DAW's words for why.
fn read_the_project(observation: &kumi_agent::core::contracts::Observation) -> bool {
    observation.identity.is_some() || observation.set.is_some()
}

/// What the producer has selected and the transport, in a few lines.
fn focus_text(
    project: &Project,
    focus: Option<&kumi_agent::core::view::DawFocus>,
    transport: Option<&kumi_agent::core::view::Transport>,
) -> String {
    let mut lines = vec![format!("{} (epoch {})", project.name, project.epoch)];
    match focus {
        Some(focus) => lines.push(format!("Selected: {}", focus_words(&connection::focus_value(focus)))),
        None => lines.push("Selected: nothing the DAW has said yet".into()),
    }
    if let Some(transport) = transport {
        let state = if transport.playing { "playing" } else { "stopped" };
        let tempo = transport.tempo.map(|tempo| format!(" at {tempo} BPM")).unwrap_or_default();
        let beat = transport.beat.map(|beat| format!(", beat {beat}")).unwrap_or_default();
        lines.push(format!("Transport: {state}{tempo}{beat}"));
    }
    lines.join("\n")
}

/// The focus in words, from what `focus` gives (names only, refs as the DAW's): the track, device, parameter, item and
/// section with their refs, then the views. A name is data, quoted on one line; an empty one is said to have none.
fn focus_words(focus: &Value) -> String {
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let quote = |name: &str| format!("\"{}\"", super::change::one_line(name, 60).replace('"', "'"));
    let named = |what: &str, name: Option<String>| match name.filter(|name| !name.trim().is_empty()) {
        Some(name) => format!("{what} {}", quote(&name)),
        None => format!("{what} with no name"),
    };
    let with_ref = |said: String, reference: Option<String>| match reference {
        Some(reference) => format!("{said} (ref {reference})"),
        None => said,
    };
    // A kind is the DAW's word too: one bounded line, its dashes spaces.
    let word = |value: &Value, key: &str| text(value, key).map(|kind| super::change::one_line(&kind.replace('-', " "), 20));
    let mut parts = Vec::new();
    if let Some(track) = focus.get("track") {
        let said = named("track", text(track, "name"));
        parts.push(match (text(focus, "trackRef"), word(track, "kind")) {
            (Some(reference), Some(kind)) => format!("{said} (ref {reference}, {kind})"),
            (Some(reference), None) => format!("{said} (ref {reference})"),
            (None, Some(kind)) => format!("{said} ({kind})"),
            (None, None) => said,
        });
    }
    if focus.get("device").is_some() || focus.get("deviceRef").is_some() {
        let mut said = with_ref(named("device", text(focus, "device")), text(focus, "deviceRef"));
        if let Some(chain) = text(focus, "chain") {
            said.push_str(&format!(" in {}", named("chain", Some(chain))));
        }
        parts.push(said);
    }
    if let Some(parameter) = focus.get("parameter") {
        let mut said = named("parameter", text(parameter, "name"));
        if let Some(value) = text(parameter, "value") {
            said.push_str(&format!(" at {}", super::change::one_line(&value, 40)));
        }
        if let Some(owner) = text(parameter, "owner").filter(|owner| !owner.trim().is_empty()) {
            said.push_str(&format!(" on {}", quote(&owner)));
        }
        parts.push(said);
    }
    if let Some(item) = focus.get("item") {
        let kind = word(item, "kind").unwrap_or_else(|| "item".into());
        let mut said = with_ref(named(&kind, text(item, "name")), text(item, "itemRef"));
        if let Some(notes) = focus.get("selectedNotes").and_then(Value::as_u64) {
            said.push_str(&format!(", {notes} notes selected"));
        }
        parts.push(said);
    }
    if let Some(section) = focus.get("section") {
        let kind = word(section, "kind").unwrap_or_else(|| "section".into());
        let at = section.get("index").and_then(Value::as_f64).map(|index| format!(" {}", index as i64 + 1)).unwrap_or_default();
        parts.push(match text(section, "name").filter(|name| !name.trim().is_empty()) {
            Some(name) => format!("{kind}{at} {}", quote(&name)),
            None => format!("{kind}{at}"),
        });
    }
    if let Some(line) = focus.get("line").and_then(Value::as_u64) {
        parts.push(format!("line {line}"));
    }
    let label = |key: &str| focus.get(key).and_then(|view| text(view, "label")).map(|label| super::change::one_line(&label, 40));
    let mut views = Vec::new();
    if let Some(main) = label("mainView") {
        views.push(format!("view {main}"));
    }
    if let Some(editor) = label("editorView") {
        views.push(format!("editor {editor}"));
    }
    if let Some(panes) = focus.get("panes").and_then(Value::as_array).filter(|panes| !panes.is_empty()) {
        let open: Vec<String> =
            panes.iter().filter_map(|pane| text(pane, "label")).map(|label| super::change::one_line(&label, 40)).collect();
        views.push(format!("also open {}", open.join(", ")));
    }
    if focus.get("browser") == Some(&Value::Bool(true)) {
        views.push("the browser open".into());
    }
    let mut said = if parts.is_empty() { "nothing".to_owned() } else { parts.join("; ") };
    if !views.is_empty() {
        said.push_str(&format!(". In {}", views.join(", ")));
    }
    if let Some(busy) = text(focus, "busy") {
        said.push_str(&format!(". The DAW is busy ({}), so this may be stale", super::change::one_line(&busy, 80)));
    }
    said
}

/// `text` cut to at most `most` characters, saying how much more there was (the saying included).
pub(super) fn bounded(text: &str, most: usize) -> String {
    let total = text.chars().count();
    if total <= most {
        return text.to_string();
    }
    let said = |more: usize| format!("\n… ({more} more characters: ask again with a larger max_chars, or read what you need)");
    let room = most.saturating_sub(said(total).chars().count());
    let kept: String = text.chars().take(room).collect();
    format!("{kept}{}", said(total - room))
}

fn project_value(project: &Project) -> Value {
    json!({"name": project.name, "key": project.key, "epoch": project.epoch})
}

pub static JOB: Operation = Operation {
    name: "job",
    description: "A background job another call started (a library scan, say): its state (running, done, failed, \
                  cancelled, or interrupted when the runtime stopped first), its progress, and its result once done. \
                  Changes nothing.",
    schema: job_schema,
    effect: reads,
    run: run_job,
};

fn job_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["job_id"], "properties": {
        "job_id": {"type": "string", "minLength": 1, "maxLength": 64, "description": "The id the call that started it answered with"}
    }})
}

fn run_job(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        let id = call.args.get("job_id").and_then(Value::as_str).unwrap_or_default();
        ctx.jobs().get(id, &ctx.paths().state_dir).ok_or_else(|| OpError::Invalid { reason: format!("there's no job {id}") })
    })
}

/// The runtime's status now, also written as `status.json` (`stale` there: whether the runtime has stopped). Once the
/// runtime is stopping, only the stop is written: a late write from before (a connection change) never undoes it.
pub async fn publish_status(ctx: &Ctx, stale: bool) -> Value {
    let status = status(ctx).await;
    if !stale && ctx.shutdown().is_cancelled() {
        return status;
    }
    let mut snapshot = status.clone();
    snapshot["stale"] = json!(stale);
    if let Err(error) = ctx.snapshots().write("status", &snapshot) {
        super::say(&format!("kumi serve: status.json not written: {error}"));
    }
    status
}

/// The runtime is stopping: `status.json` and `owner.json` say so, unless another runtime of the profile wrote them
/// since (they're that one's to say); then every snapshot is on disk.
pub async fn publish_stopped(ctx: &Ctx) {
    if ctx.snapshots().is_mine("status") {
        publish_status(ctx, true).await;
    }
    if ctx.snapshots().is_mine("owner") {
        publish_owner(ctx, false);
    }
    ctx.snapshots().flush();
}

/// `owner.json` (C3): who runs the runtime (its profile, session and process), since when, its heartbeat (ms; a
/// reader takes one older than 10 s for a runtime gone), whether it's running, and the DAW's write lease as it
/// stands for it (`free` until the runtime takes the lease).
pub fn publish_owner(ctx: &Ctx, running: bool) {
    let owner = ctx.snapshots().owner();
    let mut body = json!({
        "profile": owner.profile_id,
        "session_id": owner.session_id,
        "pid": std::process::id(),
        "since": ctx.started(),
        "heartbeat": kumi_common::time::now_ms(),
        "running": running,
        "lease": if ctx.lease().held() { "held" } else { "free" },
    });
    // A stop that waits on a call's own cleanup (a track listen's render and restoration): until when, so the host
    // tells a slow stop from a hung one.
    if let Some(until) = ctx.settling_until() {
        body["settling_until_ms"] = json!(until);
    }
    if let Err(error) = ctx.snapshots().write("owner", &body) {
        super::say(&format!("kumi serve: owner.json not written: {error}"));
    }
}

/// What `status` answers (C1): `runtime` (its version, protocol and generation, which its HISTORY rows carry; no path
/// on this computer), `daw`, `setup`, `connected`, `project` (`{name, key, epoch}` once seen), `owner` and
/// `capabilities` (the operations offered, by name).
pub async fn status(ctx: &Ctx) -> Value {
    let (installed, version, bundled, daw_open) = match crate::daw::provider(ctx.daw()) {
        Some(provider) => {
            let config = provider.bridge_config(ctx.env());
            let version = config.as_deref().and_then(|config| provider.installed_bridge_version(config, ctx.env()));
            (config.is_some(), version, provider.bundled_bridge_version(), provider.app_open(ctx.env()).await)
        }
        None => (false, None, None, false),
    };
    let capabilities: Vec<&str> = ctx.offered().iter().map(|operation| operation.name).collect();
    let connected = ctx.link().connection().is_some_and(|connection| connection.answering());
    json!({
        "runtime": {"version": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL, "generation": ctx.history().generation()},
        "daw": ctx.daw().as_str(),
        "setup": {"component": {"installed": installed, "version": version, "bundled": bundled}, "daw_open": daw_open},
        "connected": connected,
        "project": ctx.link().project().as_ref().map(project_value),
        "owner": ctx.lease().held().then(|| json!({"pid": std::process::id(), "profile": ctx.snapshots().owner().profile_id})),
        "unsure": ctx.link().unsure(),
        "quarantine": quarantined(&ctx.paths().state_dir.join("quarantine")),
        "capabilities": capabilities,
    })
}

/// How often the quarantine is counted again: `status` is asked often, and the count walks a folder tree.
const RECOUNT: std::time::Duration = std::time::Duration::from_secs(10);

/// What the quarantine keeps (CC-2: files a listen couldn't put back safely, kept for the producer, never deleted by
/// the runtime): how many and their bytes, counted again at most every [`RECOUNT`].
fn quarantined(folder: &std::path::Path) -> Value {
    type Counted = Option<(std::path::PathBuf, std::time::Instant, Value)>;
    thread_local! {
        static COUNTED: std::cell::RefCell<Counted> = const { std::cell::RefCell::new(None) };
    }
    COUNTED.with(|counted| {
        if let Some((path, at, value)) = counted.borrow().as_ref() {
            if path == folder && at.elapsed() < RECOUNT {
                return value.clone();
            }
        }
        let (mut items, mut bytes) = (0u64, 0u64);
        let mut folders = vec![folder.to_path_buf()];
        while let Some(folder) = folders.pop() {
            let Ok(entries) = std::fs::read_dir(&folder) else { continue };
            for entry in entries.filter_map(Result::ok) {
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    folders.push(entry.path());
                } else {
                    items += 1;
                    bytes += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                }
            }
        }
        let value = json!({"items": items, "bytes": bytes});
        *counted.borrow_mut() = Some((folder.to_path_buf(), std::time::Instant::now(), value.clone()));
        value
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FOCUS in words keeps every ref the model acts on, says a track with no name has none (never `""`), shows only a
    /// file's name, and never prints the focus as JSON.
    #[test]
    fn the_focus_is_said_in_words_with_its_refs() {
        let focus = json!({
            "track": {"name": "", "kind": "audio"}, "trackRef": "r:track:{AAAA}",
            "item": {"kind": "take", "itemRef": "r:take:{T1}", "name": "/Users/me/Samples/kick.wav"},
            "mainView": {"kind": "timeline", "label": "Arrange"},
        });
        let mut focus = focus;
        connection::names_only(&mut focus);
        let said = focus_words(&focus);
        assert!(said.contains("r:track:{AAAA}") && said.contains("r:take:{T1}"), "{said}");
        assert!(said.contains("no name") && !said.contains("\"\""), "{said}");
        assert!(said.contains("kick.wav") && !said.contains("/Users"), "{said}");
        assert!(!said.contains("\"track\""), "no JSON: {said}");
        assert_eq!(focus_words(&json!({})), "nothing");
    }
}
