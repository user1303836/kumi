//! The runtime core's operations (W1). `status` says what the runtime is and what it can reach; the DAW connection,
//! looks, reads, changes, undo, HISTORY and setup join it.

use super::ctx::{reads, Call, Ctx, OpError, OpFuture, Operation};
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

/// The runtime's status now, also written as `status.json` (`stale` there: whether the runtime has stopped).
pub async fn publish_status(ctx: &Ctx, stale: bool) -> Value {
    let status = status(ctx).await;
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
        "lease": "free",
    });
    if let Err(error) = ctx.snapshots().write("owner", &body) {
        super::say(&format!("kumi serve: owner.json not written: {error}"));
    }
}

/// What `status` answers (C1): `runtime` (its version, protocol and Kumi home), `daw`, `setup`, `connected`,
/// `project`, `owner` and `capabilities` (the operations offered, by name).
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
    json!({
        "runtime": {"version": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL, "home": home},
        "daw": ctx.daw().as_str(),
        "setup": {"component": {"installed": installed, "version": version, "bundled": bundled}, "daw_open": daw_open},
        "connected": false,
        "project": null,
        "owner": null,
        "capabilities": capabilities,
    })
}
