//! `setup_daw` (C8): Kumi's part in the runtime's DAW for its Kumi home (`--kumi-home`), as the host's setup puts it
//! there. A dry run reads what's there and plans the step, writing nothing at all (not even the lease's file), and gives
//! the plan's `digest`; a real one takes the DAW endpoint's lease and carries out the plan the DAW's crate makes again
//! under the lease, only if it's still the one approved (`expected`, that digest). It refuses rather than overwrite
//! another Kumi's part, downgrade a newer one, or write into an adopted installation; it never asks, never waits for
//! the DAW, neither starts nor quits it, and needs no connection to it.
//!
//! Its effect is `Setup`, which isn't one of the adapter's writes: those record the call in flight and audit a broken
//! one in a project's HISTORY, and setup has no project. So it takes the lease itself, for a real install only.
use super::ctx::{Call, Ctx, Effect, JsonObject, OpError, OpFuture, Operation};
use kumi_daw::component::{ApplyError, ComponentPlan, PartState};
use kumi_daw::io::DawIo;
use kumi_daw::process::Run;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

pub static SETUP_DAW: Operation = Operation {
    name: "setup_daw",
    description: "Kumi's part in this runtime's DAW (Live's Remote Script with Kumi Ears, REAPER's extension, Renoise's tool), as \
                  the host's setup puts it there: with dry_run, what's there and what putting it in would write, every path \
                  named, and the plan's digest; without, the plan whose digest is `expected` carried out (refused when it \
                  isn't that plan any more). It never asks, waits for, starts or quits the DAW, and leaves another Kumi's part, a newer one \
                  and an adopted installation as they are.",
    schema,
    effect,
    run,
};

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "daw": {"enum": ["ableton", "reaper", "renoise"]},
            "dry_run": {"type": "boolean"},
            "expected": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
        },
        "required": ["daw", "dry_run"],
        "additionalProperties": false,
    })
}

fn effect(args: &JsonObject) -> Effect {
    if dry(args) {
        Effect::Read
    } else {
        Effect::Setup
    }
}

fn dry(args: &JsonObject) -> bool {
    args.get("dry_run") == Some(&Value::Bool(true))
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        let Some(provider) = crate::daw::provider(ctx.daw()) else {
            let reason = format!("Kumi doesn't work with {} here.", ctx.daw().as_str());
            return Err(OpError::Refused { code: "unsupported", reason, owner: None });
        };
        let app = provider.profile().words.app;
        if call.args.get("daw").and_then(Value::as_str) != Some(ctx.daw().as_str()) {
            let reason = format!("This runtime serves {app} only: setting up another DAW takes the runtime for it.");
            return Err(OpError::Refused { code: "other_daw", reason, owner: None });
        }
        let mut io = DawIo::new(ctx.env().clone());
        io.answers = ctx.answers().clone();
        if let Some(run) = ctx.answers().get::<Run>() {
            io.run = run.clone();
        }
        let home = ctx.paths().kumi_home.clone();
        let adopted = adopted(ctx);
        let failed = |reason: String| OpError::Failed { reason };
        if dry(&call.args) {
            let plan = provider.component_plan(&io, &home).map_err(failed)?;
            return Ok(reply(ctx, app, &plan, adopted, None));
        }
        // A real install carries out the plan the producer approved, named by its digest: never one nobody saw.
        let Some(expected) = call.args.get("expected").and_then(Value::as_str).map(str::to_string) else {
            let reason = "A real install names the plan it carries out: expected, the digest of the dry run that was approved.";
            return Err(OpError::Invalid { reason: reason.into() });
        };
        if adopted {
            let reason = format!(
                "Kumi for Hermes uses the Kumi installation at {} as it is, and never changes its part in {app}: run {} there.",
                home.display(),
                fix(ctx)
            );
            return Err(OpError::Refused { code: "adopted_home", reason, owner: Some(json!({"kumi_home": home})) });
        }
        let _held = ctx.lease().take(&call.host).await?;
        let plan = provider.component_plan(&io, &home).map_err(failed)?;
        if expected != digest(&plan) {
            let reason = format!(
                "What's in {app} changed since this step was approved, so nothing was installed: plan it again and approve what it shows now."
            );
            return Err(OpError::Refused { code: "changed", reason, owner: None });
        }
        match plan.state {
            PartState::OtherOwner => {
                let reason = format!(
                    "Kumi's part in {app} belongs to another Kumi on this computer: set {app} up for Hermes, or adopt that installation, first."
                );
                return Err(OpError::Refused { code: "owned_elsewhere", reason, owner: Some(json!({"kumi_home": plan.owner})) });
            }
            PartState::Newer => {
                let reason = format!(
                    "Kumi's part in {app} ({}) is newer than this runtime's: update Kumi for Hermes to match it. Kumi never puts an older one in.",
                    plan.version.as_deref().unwrap_or("newer")
                );
                return Err(OpError::Refused { code: "newer_installed", reason, owner: None });
            }
            _ if plan.paths.is_empty() => return Ok(reply(ctx, app, &plan, false, None)),
            _ => {}
        }
        match provider.component_apply(&io, &home, &plan).await {
            Ok(()) => {}
            Err(ApplyError::InUse(reason)) => return Err(OpError::Refused { code: "in_use", reason, owner: None }),
            Err(ApplyError::Changed(reason)) => return Err(OpError::Refused { code: "changed", reason, owner: None }),
            Err(ApplyError::Failed(reason)) => {
                super::say(&format!("kumi serve: setting up {app} stopped"));
                return Err(failed(reason));
            }
        }
        // What was done, as the plan it carried out names it (the paths written, what the producer does next), and how
        // each part stands now.
        let mut answer = reply(ctx, app, &plan, false, Some(kumi_common::time::now_ms()));
        if let Ok(now) = provider.component_plan(&io, &home) {
            answer["state"] = json!(now.state);
            answer["parts"] = json!(now.parts);
        }
        Ok(answer)
    })
}

/// Whether `--kumi-home` is an installation setup adopted: a Kumi home outside the host's plugin data. One that isn't
/// there yet can't be: adoption names a home that is.
fn adopted(ctx: &Ctx) -> bool {
    let home = &ctx.paths().kumi_home;
    let plugins = ctx.paths().state_dir.parent().map(Path::to_path_buf).unwrap_or_default();
    home.exists() && !super::connection::inside(home, &plugins)
}

/// The command that sets up Kumi's part in an adopted installation's own Kumi.
fn fix(ctx: &Ctx) -> String {
    format!("{} --daw {} bridge", *kumi_types::command::KUMI, ctx.daw().as_str())
}

/// The plan's digest, over what the producer approves of it: how it stands, the version, every path it writes and each
/// part. A real install refuses a plan whose digest isn't the one approved (C8: changed targets need a new approval).
fn digest(plan: &ComponentPlan) -> String {
    let approved = json!({"state": plan.state, "version": plan.version, "paths": plan.paths, "parts": plan.parts});
    hex::encode(Sha256::digest(approved.to_string()))
}

/// The result C8 gives: the plan as the host shows it, with its digest. In an adopted installation it writes nothing,
/// so it names no path, and says what does instead.
fn reply(ctx: &Ctx, app: &str, plan: &ComponentPlan, adopted: bool, installed_at: Option<i64>) -> Value {
    let (paths, effects, after) = match adopted {
        false => (plan.paths.clone(), plan.effects.clone(), plan.after),
        true if plan.paths.is_empty() => (vec![], plan.effects.clone(), plan.after),
        true => {
            let said = format!(
                "Kumi for Hermes uses the Kumi installation at {} as it is: to bring its part in {app} up to date, run {} there",
                ctx.paths().kumi_home.display(),
                fix(ctx)
            );
            (vec![], vec![said], kumi_daw::component::After::None)
        }
    };
    let mut reply = json!({
        "daw": ctx.daw().as_str(),
        "component": plan.component,
        "version": plan.version,
        "paths": paths,
        "state": plan.state,
        "owner": (plan.state == PartState::OtherOwner).then(|| json!({"kumi_home": plan.owner})),
        "effects": effects,
        "after": after,
        "parts": plan.parts,
        "keep_identity": plan.keep_identity,
        "digest": digest(plan),
    });
    if let Some(at) = installed_at {
        reply["installed_at"] = json!(at);
    }
    reply
}
