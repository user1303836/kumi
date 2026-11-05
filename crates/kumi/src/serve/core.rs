//! The runtime core's operations (W1). `status` says what the runtime is and what it can reach; the DAW connection,
//! looks, reads, changes, undo, HISTORY and setup join it.

use super::connection::{self, Project};
use super::ctx::{reads, Call, Ctx, Effect, JsonObject, OpError, OpFuture, Operation};
use super::PROTOCOL;
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
            let reason = format!("This runtime works with {}, not {daw}: change the DAW in the plugin's settings.", ctx.daw().as_str());
            return Err(OpError::Refused { code: "other_daw", reason, owner: None });
        }
        let connection = connection::connect(ctx, call.signal.clone()).await?;
        // A first look, for the project's key and epoch: a DAW that doesn't answer yet leaves it for observe.
        if let Ok(observation) = connection.integration.observe(call.signal.clone(), None).await {
            ctx.link().saw(&observation);
            let epoch = super::read::page_epoch_now(&observation.tools, call.signal.clone()).await;
            ctx.link().saw_page_epoch(epoch);
            connection::publish_focus(&ctx.handle());
        }
        Ok(publish_status(ctx, false).await)
    })
}

pub static OBSERVE: Operation = Operation {
    name: "observe",
    description: "Looks at the open project now: `project` gives its full description with the refs a change names, \
                  `focus` what the producer has selected and the transport, `catch_up` what changed since the last look. \
                  Every answer has the project's key and epoch, which a change names. Writes nothing into the DAW.",
    schema: observe_schema,
    effect: reads,
    run: run_observe,
};

/// The longest `text` an observation gives when it isn't told (`max_chars`), and the bounds of what it may be told.
const MAX_CHARS: (u64, u64, u64) = (4000, 200, 20000);

fn observe_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["scope"], "properties": {
        "scope": {"type": "string", "enum": ["focus", "catch_up", "project"]},
        "max_chars": {"type": "integer", "minimum": MAX_CHARS.1, "maximum": MAX_CHARS.2, "description": "The longest text to answer with (4000 if left out)"}
    }})
}

fn run_observe(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        let connection = ctx.connection()?;
        let observation = connection
            .integration
            .observe(call.signal.clone(), None)
            .await
            .map_err(|error| OpError::Failed { reason: format!("Kumi couldn't look at the project: {error}") })?;
        let project = ctx.link().saw(&observation);
        ctx.link().clear_unsure();
        let epoch = super::read::page_epoch_now(&observation.tools, call.signal.clone()).await;
        ctx.link().saw_page_epoch(epoch);
        let (focus, transport, catch_up) = (connection.focus(), connection.transport(), connection.take_catch_up());
        let scope = call.args.get("scope").and_then(Value::as_str).unwrap_or("project");
        let text = match scope {
            "focus" => focus_text(&project, focus.as_ref(), transport.as_ref()),
            "catch_up" if catch_up.is_empty() => format!("Nothing has changed in {} since the last look.", project.name),
            "catch_up" => catch_up
                .iter()
                .flat_map(|seen| seen.lines.iter().cloned().chain((seen.more > 0).then(|| format!("…and {} more", seen.more))))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => observation.context.clone(),
        };
        let most = call.args.get("max_chars").and_then(Value::as_u64).unwrap_or(MAX_CHARS.0);
        let refs = connection::refs(&connection, call.signal.clone()).await;
        ctx.link().keep_refs(&refs);
        connection::publish_focus(&ctx.handle());
        publish_status(ctx, false).await;
        Ok(json!({
            "text": bounded(&text, most as usize),
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

/// What the producer has selected and the transport, in a few lines.
fn focus_text(
    project: &Project,
    focus: Option<&kumi_agent::core::view::DawFocus>,
    transport: Option<&kumi_agent::core::view::Transport>,
) -> String {
    let mut lines = vec![format!("{} (epoch {})", project.name, project.epoch)];
    match focus {
        Some(focus) => lines.push(format!("Selected: {}", connection::focus_value(focus))),
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
    let body = json!({
        "profile": owner.profile_id,
        "session_id": owner.session_id,
        "pid": std::process::id(),
        "since": ctx.started(),
        "heartbeat": kumi_common::time::now_ms(),
        "running": running,
        "lease": if ctx.lease().held() { "held" } else { "free" },
    });
    if let Err(error) = ctx.snapshots().write("owner", &body) {
        super::say(&format!("kumi serve: owner.json not written: {error}"));
    }
}

/// What `status` answers (C1): `runtime` (its version, protocol and Kumi home), `daw`, `setup`, `connected`,
/// `project` (`{name, key, epoch}` once seen), `owner` and `capabilities` (the operations offered, by name).
pub async fn status(ctx: &Ctx) -> Value {
    let (installed, version, bundled, daw_open) = match crate::daw::provider(ctx.daw()) {
        Some(provider) => {
            let config = provider.bridge_config(ctx.env());
            let version = config.as_deref().and_then(|config| provider.installed_bridge_version(config, ctx.env()));
            (config.is_some(), version, provider.bundled_bridge_version(), provider.app_open(ctx.env()).await)
        }
        None => (false, None, None, false),
    };
    // The process's own, which every part of Kumi reads: `main` set it before anything else ran.
    let home = std::env::var_os("KUMI_HOME").map(|home| home.to_string_lossy().into_owned());
    let capabilities: Vec<&str> = ctx.offered().iter().map(|operation| operation.name).collect();
    let connected = ctx.link().connection().is_some_and(|connection| connection.answering());
    json!({
        "runtime": {"version": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL, "home": home},
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
