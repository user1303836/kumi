//! Device and Playlist tools use the same native project/revision transaction as the basic controls.
use kumi_types::contracts::JsonObject;
use serde_json::{json, Value};

pub const OPS: &[(&str, &str, &str, bool)] = &[
    ("lynx_plugin_inspect", "plugin.inspect", "List LYNX's hosted instruments and mixer effects, or read one target's parameter page. Read parameters before changing them: values and expectedValue are normalized 0–1; displayValue/label show native units. Native instrument synthesis parameters are not exposed. Indices are zero-based; offset/limit page parameters.", false),
    ("lynx_plugin_parameters_set", "plugin.parameters_set", "Change 1–64 parameters of one hosted instrument or mixer effect as one native undo step, with playback stopped. First inspect this target; for each parameter pass its index, desired normalized value and the exact normalized expectedValue read. Every expectedValue is checked before any parameter changes. Native effects are supported; native instrument synthesis parameters are not exposed.", true),
    ("lynx_effect_catalog", "effect.catalog", "Search/page loadable built-in and already-scanned hosted audio effects and their effectId; use query, offset/limit and nextOffset. Names are data. Use an exact effectId from this catalog with lynx_effect_load; this does not scan files or run arbitrary paths.", false),
    ("lynx_effect_load", "effect.load", "Load a known audio effect onto a zero-based mixer insert as one native undo step, with playback stopped. Read the catalog and current devices first. effectId must come from lynx_effect_catalog; effectIndex inserts before that slot, or omit to append. A mixer insert can contain at most ten effects.", true),
    ("lynx_effect_remove", "effect.remove", "Remove one effect from a zero-based mixer insert as one native undo step, with playback stopped. Inspect devices first; later effect indices shift left.", true),
    ("lynx_playlist_inspect", "playlist.inspect", "Read LYNX's current arrangement, its complete Playlist clips and track locks, plus pattern, audio-channel and automation source indices. A clip's clipId identifies it for edits. Times are ticks at PPQ 960; preserve all properties not requested to change. Page clips with offset/limit and hasMore. Other arrangements are listed but not edited by this tool.", false),
    ("lynx_playlist_edit", "playlist.edit", "Edit the current LYNX arrangement as one atomic native undo step, with playback stopped. Read lynx_playlist_inspect first. Submit 1–128 add, update, duplicate or remove edits; all validate before any change. add chooses an existing pattern/audio-channel/automation source; update and duplicate preserve omitted clip properties, and duplicate creates a fresh clipId. Locked tracks are refused. Times are ticks at PPQ 960. This places existing sources without changing channel/mixer routing or creating patterns. Arrangement switching and arbitrary file imports are not supported.", true),
];

/// A control build advertises extension groups only after its native dispatch supports them.
pub fn enabled(op: &str, capabilities: &Value) -> bool {
    let group = match op {
        "plugin.inspect" | "plugin.parameters_set" => "pluginParameters",
        "effect.catalog" | "effect.load" | "effect.remove" => "effects",
        "playlist.inspect" | "playlist.edit" => "playlist",
        _ => return true,
    };
    capabilities.get(group).and_then(Value::as_bool) == Some(true)
}

fn integer(min: u64, max: u64) -> Value {
    json!({"type":"integer","minimum":min,"maximum":max})
}
fn number(min: f64, max: f64) -> Value {
    json!({"type":"number","minimum":min,"maximum":max})
}
fn string(max: usize) -> Value {
    json!({"type":"string","minLength":1,"maxLength":max})
}
fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn target() -> Value {
    object(
        json!({"kind":{"type":"string","enum":["instrument","effect"]},"channelIndex":integer(0,100000),"mixerIndex":integer(0,100000),"effectIndex":integer(0,9)}),
        &["kind"],
    )
}

pub fn schema(op: &str) -> Option<JsonObject> {
    let value = match op {
        "plugin.inspect" => object(json!({"target":target(),"offset":integer(0,100000),"limit":integer(1,256)}), &[]),
        "plugin.parameters_set" => object(
            json!({"target":target(),"parameters":{"type":"array","minItems":1,"maxItems":64,"items":object(json!({"index":integer(0,100000),"value":number(0.,1.),"expectedValue":number(0.,1.)}),&["index","value","expectedValue"])}}),
            &["target", "parameters"],
        ),
        "effect.catalog" => {
            object(json!({"query":{"type":"string","maxLength":256},"offset":integer(0,100000),"limit":integer(1,512)}), &[])
        }
        "playlist.inspect" => object(json!({"offset":integer(0,100000),"limit":integer(1,1000)}), &[]),
        "effect.load" => {
            object(json!({"mixerIndex":integer(0,100000),"effectId":string(4096),"effectIndex":integer(0,9)}), &["mixerIndex", "effectId"])
        }
        "effect.remove" => object(json!({"mixerIndex":integer(0,100000),"effectIndex":integer(0,9)}), &["mixerIndex", "effectIndex"]),
        "playlist.edit" => {
            let tick = integer(0, 1_000_000_000);
            let edit = object(
                json!({
                    "action":{"type":"string","enum":["add","update","duplicate","remove"]},
                    "clipId":{"type":"string","minLength":16,"maxLength":16,"description":"A nonzero 16-digit hexadecimal clipId returned by playlist.inspect."},
                    "kind":{"type":"string","enum":["pattern","audio","automation"]},
                    "track":integer(0,499),"start":tick,"length":integer(1,1_000_000_000),"offset":tick,
                    "pattern":integer(0,100000),"channel":integer(0,100000),"automation":integer(0,100000),
                    "muted":{"type":"boolean"},"fadeIn":tick,"fadeOut":tick,"gain":number(0.,64.),"pan":number(-1.,1.),
                    "pitch":number(-12.,12.),"reversed":{"type":"boolean"},"stretch":number(0.01,100.),"offsetFraction":number(-0.5,0.5)
                }),
                &["action"],
            );
            object(json!({"edits":{"type":"array","minItems":1,"maxItems":128,"items":edit}}), &["edits"])
        }
        _ => return None,
    };
    Some(value.as_object().unwrap().clone())
}

/// Cross-field requirements the simple JSON schemas cannot state without conditional schemas.
pub fn validate(op: &str, input: &JsonObject) -> Result<(), String> {
    if let Some(target) = input.get("target").and_then(Value::as_object) {
        let fields: &[&str] = match target.get("kind").and_then(Value::as_str) {
            Some("instrument") => &["kind", "channelIndex"],
            Some("effect") => &["kind", "mixerIndex", "effectIndex"],
            _ => return Err("Choose an instrument or effect target from the device read".into()),
        };
        if target.len() != fields.len() || fields.iter().any(|field| !target.contains_key(*field)) {
            return Err(
                "An instrument target requires only kind/channelIndex; an effect target requires only kind/mixerIndex/effectIndex".into()
            );
        }
    }
    if op == "plugin.parameters_set" {
        let mut seen = std::collections::HashSet::new();
        for parameter in input["parameters"].as_array().into_iter().flatten() {
            if !seen.insert(parameter["index"].as_u64().unwrap()) {
                return Err("Each parameter index may occur only once in a transaction".into());
            }
        }
    }
    if op != "playlist.edit" {
        return Ok(());
    }
    for edit in input["edits"].as_array().into_iter().flatten() {
        let edit = edit.as_object().unwrap();
        let action = edit["action"].as_str().unwrap();
        if action == "add" {
            if edit.contains_key("clipId") || ["kind", "track", "start", "length"].iter().any(|key| !edit.contains_key(*key)) {
                return Err("An added clip needs kind, track, start and length, without an existing clipId".into());
            }
            let source = match edit["kind"].as_str() {
                Some("pattern") => "pattern",
                Some("audio") => "channel",
                Some("automation") => "automation",
                _ => return Err("Choose a pattern, audio or automation clip".into()),
            };
            for candidate in ["pattern", "channel", "automation"] {
                if edit.contains_key(candidate) != (candidate == source) {
                    return Err("Supply only the source index for the added clip's kind".into());
                }
            }
            if source != "channel"
                && ["fadeIn", "fadeOut", "gain", "pan", "pitch", "reversed", "stretch", "offsetFraction"]
                    .iter()
                    .any(|field| edit.contains_key(*field))
            {
                return Err("Audio properties are valid only for an audio clip".into());
            }
        } else {
            let clip = edit.get("clipId").and_then(Value::as_str).ok_or("An existing clipId is required")?;
            if clip.len() != 16 || !clip.bytes().all(|c| c.is_ascii_hexdigit()) || clip.bytes().all(|c| c == b'0') {
                return Err("Use a nonzero 16-digit hexadecimal clipId from the Playlist read".into());
            }
            if ["kind", "pattern", "channel", "automation"].iter().any(|key| edit.contains_key(*key)) {
                return Err("Updates and duplicates keep the clip's kind and source".into());
            }
            if action == "remove" && edit.len() != 2 {
                return Err("A remove edit accepts only action and clipId".into());
            }
            if action == "update" && edit.len() == 2 {
                return Err("An update needs at least one changed clip property".into());
            }
        }
        if let (Some(start), Some(length)) = (edit.get("start").and_then(Value::as_u64), edit.get("length").and_then(Value::as_u64)) {
            if start.saturating_add(length) > 1_000_000_000 {
                return Err("A Playlist clip extends beyond the native tick bound".into());
            }
        }
    }
    Ok(())
}
