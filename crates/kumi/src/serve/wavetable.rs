//! `wavetable`: Live's plug-in tool's wavetable writer (`plugin` with action wavetable) through the runtime. It draws
//! frames from keyframes, or cuts single cycles from a held note, and writes them as a new WAV into the plug-in's own
//! wavetable folder (Serum 2's, Vital's…), else the User Library's Kumi/Wavetables: never over a file that's there. A
//! file beside the Set, not a change in it, so it's not in HISTORY and takes no write lease (LibraryWrite), like a
//! preset or a device. The audio is a source the runtime resolves and copies aside for the call, never a path.

use super::change::one_line;
use super::connection::{names_only_said, names_only_text};
use super::ctx::{Call, Ctx, Effect, OpError, OpFuture, Operation};
use super::listen::{source_schema, stage_source, Source};
use kumi_daw_ableton::plugin_tool::PLUGIN_SCHEMA;
use kumi_types::daw::DawId;
use serde_json::{json, Map, Value};

const IN_HERMES: &str = "In Kumi for Hermes, set_device_details is a kumi_change step. The file stays where it is: an undo \
                         doesn't remove it, and it's not in HISTORY.";

pub static WAVETABLE: Operation = Operation {
    name: "wavetable",
    description: "Make a wavetable for a Live plug-in instance (by device_ref from the current look): frames morphing \
                  through keyframes (harmonics or shapes), or single cycles cut from a held note in an audio source. Kumi \
                  writes it as a new file into the plug-in's own wavetable folder (Serum 2's, Vital's…), else the User \
                  Library's Kumi/Wavetables, never over a file. Picking it in the plug-in's oscillator is the producer's click.",
    schema,
    effect: |_| Effect::LibraryWrite,
    run,
};

fn schema() -> Value {
    let mut wavetable = PLUGIN_SCHEMA["properties"]["wavetable"].clone();
    let from = &mut wavetable["properties"]["from_audio"];
    from["required"] = json!(["source"]);
    let properties = from["properties"].as_object_mut().expect("from_audio has properties");
    properties.remove("file");
    let mut source = source_schema();
    source["description"] = json!("The held note: a map sound, a file Kumi made, or an approved file");
    properties.insert("source".into(), source);
    wavetable["description"] = json!("What to draw: keyframes (with count), or from_audio");
    json!({"type": "object", "additionalProperties": false, "required": ["device_ref", "wavetable"], "properties": {
        "device_ref": {"type": "string", "minLength": 1, "maxLength": 256, "description": "The plug-in device's ref from the current look"},
        "wavetable": wavetable
    }})
}

fn invalid(reason: &str) -> OpError {
    OpError::Invalid { reason: reason.into() }
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        if ctx.daw() != DawId::Ableton {
            return Err(OpError::Refused {
                code: "unsupported",
                reason: "Kumi makes wavetables for Live's plug-ins here.".into(),
                owner: None,
            });
        }
        let device = call.args.get("device_ref").and_then(Value::as_str).filter(|device| !device.trim().is_empty());
        let (Some(device), Some(Value::Object(spec))) = (device, call.args.get("wavetable")) else {
            return Err(invalid("Give device_ref (a plug-in device from the current look) and wavetable."));
        };
        if call.args.len() != 2 {
            return Err(invalid("A wavetable takes device_ref and wavetable only."));
        }
        let mut spec: Map<String, Value> = spec.clone();
        ctx.connection()?;
        let last = ctx.link().project().ok_or_else(|| OpError::NotConnected { reason: "Observe the project first.".into() })?;
        ctx.link().expect(&last.key, last.epoch)?;
        let tools = ctx.link().tools();
        let tool = tools.iter().find(|tool| tool.name() == "plugin").ok_or_else(|| OpError::Refused {
            code: "unsupported",
            reason: "This Live offers Kumi no plug-in tool: update Kumi's part in Live with setup.".into(),
            owner: None,
        })?;
        // The held note, resolved and copied aside: the copy is what the integration reads, for as long as the call.
        let mut staged = None;
        if let Some(Value::Object(from)) = spec.get_mut("from_audio") {
            let source = from.remove("source").ok_or_else(|| invalid("from_audio needs its source."))?;
            let copy = stage_source(ctx, &Source::parse(&source)?, &call.signal).await?;
            from.insert("file".into(), json!(copy.path().to_string_lossy()));
            staged = Some(copy);
        }
        let Value::Object(input) = json!({"action": "wavetable", "device": device, "wavetable": spec}) else { unreachable!() };
        if let Some(problem) = kumi_agent::kernel::schema::problem(&Value::Object(tool.input_schema()), &Value::Object(input.clone())) {
            return Err(OpError::Invalid { reason: problem });
        }
        let ran = tool.execute(input, call.signal.clone()).await;
        drop(staged);
        let ran = ran.map_err(|error| OpError::Failed { reason: one_line(&names_only_text(&error.to_string()), 500) })?;
        if ran.is_error {
            // Words from require_fresh_references.
            let code = if ran.text.contains("must come from discovery") { "stale" } else { "failed" };
            return Err(OpError::Refused { code, reason: one_line(&names_only_text(&ran.text), 500), owner: None });
        }
        let mut made: Value = serde_json::from_str(&ran.text)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| OpError::Failed { reason: "The integration returned no structured wavetable answer.".into() })?;
        // Its file's name, never its folder's path: where it went is said in words (folder).
        names_only_said(&mut made);
        made["in_hermes"] = json!(IN_HERMES);
        Ok(made)
    })
}
