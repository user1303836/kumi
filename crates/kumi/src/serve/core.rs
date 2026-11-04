//! The runtime core's operations (W1). `status` says what the runtime is and what it can reach; the DAW connection,
//! looks, reads, changes, undo, HISTORY and setup join it.

use super::ctx::{reads, Call, Ctx, OpFuture, Operation};
use super::{registry, PROTOCOL};
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

/// What `status` answers (C1): `runtime`, `daw`, `setup`, `connected`, `project`, `owner` and `capabilities` (the
/// operations offered, by name).
pub async fn status(ctx: &Ctx) -> Value {
    let (installed, version, bundled, daw_open) = match crate::daw::provider(ctx.daw()) {
        Some(provider) => {
            let config = provider.bridge_config(ctx.env());
            let version = config.as_deref().and_then(|config| provider.installed_bridge_version(config, ctx.env()));
            (config.is_some(), version, provider.bundled_bridge_version(), provider.app_open(ctx.env()).await)
        }
        None => (false, None, None, false),
    };
    json!({
        "runtime": {"version": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL},
        "daw": ctx.daw().as_str(),
        "setup": {"component": {"installed": installed, "version": version, "bundled": bundled}, "daw_open": daw_open},
        "connected": false,
        "project": null,
        "owner": null,
        "capabilities": registry::names(),
    })
}
