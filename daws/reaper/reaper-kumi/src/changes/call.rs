//! A REAPER API function that changes the project, called by name (see reaper/generic.rs), as a change: one undo block
//! named by the host, so REAPER's undo takes it back whole. What the function did is REAPER's to know, not Kumi's, so
//! Kumi's own undo is refused (`daw-only`): REAPER's undo can.

use serde_json::{json, Value};

use super::{Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, CallRefs};
use crate::reaper::generic;

const NOTE: &str = "Only REAPER's own undo can take this back (Edit > Undo).";

/// The function an api-call names, and its arguments: one that changes the project and can be called by name.
fn plan(input: &Value) -> Done<(usize, &'static str, Value)> {
    let name = input.get("fn").and_then(Value::as_str).ok_or_else(|| bad("fn: the name of a REAPER API function"))?;
    let index = generic::function(name).map_err(bad)?;
    let args = input.get("args").cloned().unwrap_or(Value::Array(vec![]));
    if let Some(why) = generic::denied(index, &args) {
        return Err(bad(why));
    }
    if let Some(why) = generic::uncallable(index) {
        return Err(bad(format!("{name} can't be called by name: {why} (run-lua can call it)")));
    }
    if !matches!(args, Value::Array(_) | Value::Object(_)) {
        return Err(bad("args: a list of the function's arguments in order, or an object of them by name"));
    }
    if generic::reads_only(index, &args) {
        return Err(bad(format!("{name} only reads: read it with read {{kind: \"call\", fn, args}}, which changes nothing")));
    }
    Ok((index, generic::name_of(index), args))
}

fn title(name: &str) -> String {
    format!("Called {name}")
}

pub fn preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let (index, name, args) = plan(input)?;
    let handles = CallRefs { reads: ctx.reads, reaper: ctx.reaper, proj: ctx.proj };
    // (the arguments are checked, the refs found, without calling REAPER)
    ctx.reaper.call(index, &args, &handles, true).map_err(bad)?;
    Ok(Preview {
        target: json!({"kind": "call", "fn": name}),
        prior: json!({}),
        fence: json!({"fn": name}),
        display: json!({"title": title(name), "note": NOTE}),
    })
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (index, name, args) = plan(input)?;
    if fence["fn"] != name {
        return Err(bad("the function isn't the one previewed"));
    }
    let handles = CallRefs { reads: ctx.reads, reaper: ctx.reaper, proj: ctx.proj };
    let (result, out) = ctx.reaper.call(index, &args, &handles, false).map_err(bad)?;
    // (REAPER counts a block's changes only as it ends, so whether the function changed anything isn't known here: the
    // block is always closed as a write, REAPER's undo covers whatever it did, the producer's next edit never joins it,
    // and the answer's undo.entry says whether REAPER made a point of it)
    Ok(Applied {
        applied: json!({"fn": name, "result": result, "out": out}),
        display: json!({"title": title(name), "note": NOTE}),
        ..Default::default()
    })
}

pub fn restore(_ctx: &Ctx, _restore: &Restore) -> Done<Restored> {
    Err(("daw-only".into(), "Kumi can't take back what an API call did: REAPER's own undo can (Edit > Undo)".into()))
}
