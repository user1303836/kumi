//! Transport, routing, clip, Arrangement and device change descriptions.
use super::{summaries::*, *};
use crate::integrations::ableton::more_changes::{bars, span};
fn text(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => number::to_string(n.as_f64().unwrap()),
        Some(Value::Array(rows)) => {
            rows.iter().map(|v| if v.is_null() { String::new() } else { text(Some(v)) }).collect::<Vec<_>>().join(",")
        }
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}
fn owner(value: Option<&Value>, track: &Lookup<'_>) -> Option<KnownTrack> {
    let reference = value?.as_str()?;
    let direct = track(value.unwrap());
    if direct.is_some() && reference.contains(":track:") {
        return direct;
    }
    static PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([0-9]+):(?:clip|arrangement_clip|device|chain|clip_slot|slot|drum_pad):([0-9]+)(?::|$)").unwrap());
    let captures = PATTERN.captures(reference)?;
    track(&json!(format!("{}:track:{}", &captures[1], &captures[2])))
}
fn with_track(title: impl Into<String>, track: Option<KnownTrack>) -> ChangeSummary {
    ChangeSummary { title: title.into(), track, ..Default::default() }
}
fn clip_name(track: &Option<KnownTrack>, audio: bool) -> String {
    let kind = if audio { "audio clip" } else { "clip" };
    track.as_ref().map(|t| format!("{} {kind}", t.name)).unwrap_or_else(|| if audio { "Audio clip" } else { "Clip" }.into())
}
/// What an Arrangement clip replaced where it landed, as dropping one in Live does: ", replacing bar 5 to bar 6 of
/// “Hats”, “Fill”" (a clip it covers whole goes by its name).
fn replacing(preview: &JsonObject) -> String {
    let replaced: Vec<String> = array(preview.get("replaces"))
        .iter()
        .map(|r| match (r.get("whole") == Some(&json!(true)), finite(r.get("from")), finite(r.get("to"))) {
            (false, Some(from), Some(to)) => format!("{} to {} of {}", bars(from), bars(to), quoted(r.get("name"), "a clip")),
            _ => quoted(r.get("name"), "a clip"),
        })
        .collect();
    if replaced.is_empty() {
        String::new()
    } else {
        format!(", replacing {}", replaced.join(", "))
    }
}
fn on(track: &Option<KnownTrack>) -> String {
    track.as_ref().map(|t| format!(" on {}", t.name)).unwrap_or_default()
}
fn index_name<'a>(n: f64, names: &[&'a str]) -> Option<&'a str> {
    (n >= 0.0 && n.fract() == 0.0).then(|| names.get(n as usize).copied()).flatten()
}
fn parts_text(parts: Vec<String>, fallback: &str) -> String {
    if parts.is_empty() {
        fallback.into()
    } else {
        parts.join(", ")
    }
}
fn capital(text: String) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().to_string() + chars.as_str()).unwrap_or_default()
}
fn boolean(parts: &mut Vec<String>, value: Option<&Value>, yes: &str, no: &str) {
    if let Some(b) = value.and_then(Value::as_bool) {
        parts.push(if b { yes } else { no }.into());
    }
}
fn percent(n: f64) -> String {
    number::to_string(number::round(n * 100.0))
}
fn rounded(n: f64) -> String {
    number::to_string(number::parse(&number::to_fixed(n, 2)).unwrap())
}
pub(super) fn more(
    kind: &ChangeKind,
    preview: &JsonObject,
    input: &JsonObject,
    track: &Lookup<'_>,
    applied: Option<&JsonObject>,
) -> Option<ChangeSummary> {
    let empty = JsonObject::new();
    let proposed = record(preview.get("proposed"));
    let prior = record(preview.get("prior"));
    let proposed_or_input = preview.get("proposed").filter(|v| !v.is_null()).map(|v| record(Some(v))).unwrap_or(input);
    let field = |key: &str| coalesce(proposed_or_input.get(key), input.get(key));
    let clip_track = || owner(coalesce(preview.get("clipRef"), input.get("clipRef")), track);
    let summary = match kind.tool.as_str() {
        "set_transport" => {
            let loop_prior = record(prior.get("loop"));
            let mut parts = vec![];
            let start = finite(proposed.get("loopStart")).or_else(|| finite(loop_prior.get("start")));
            let length = finite(proposed.get("loopLength")).or_else(|| finite(loop_prior.get("length")));
            if proposed.get("loopEnabled").is_some_and(Value::is_boolean)
                || proposed.contains_key("loopStart")
                || proposed.contains_key("loopLength")
            {
                let enabled =
                    proposed.get("loopEnabled").and_then(Value::as_bool).unwrap_or(loop_prior.get("enabled") == Some(&json!(true)));
                parts.push(match (enabled, start, length) {
                    (true, Some(start), Some(length)) => format!("loop {} for {}", bars(start), span(length)),
                    (true, _, _) => "loop on".into(),
                    _ => "loop off".into(),
                });
            }
            for (key, yes, no) in [
                ("metronome", "metronome on", "metronome off"),
                ("punchIn", "punch in on", "punch in off"),
                ("punchOut", "punch out on", "punch out off"),
            ] {
                boolean(&mut parts, proposed.get(key), yes, no);
            }
            if let Some(position) = finite(proposed.get("position")) {
                parts.push(format!("playhead to {}", bars(position)));
            }
            ChangeSummary::title(capital(parts_text(parts, "transport")))
        }
        "set_routing" => {
            let known = known(track, coalesce(preview.get("trackRef"), input.get("trackRef")));
            let mut parts = vec![];
            for (key, sub, prefix) in [("inputType", "inputSubRouting", "input from"), ("outputType", "outputSubRouting", "output to")] {
                if let Some(name) = label(proposed.get(key)).or_else(|| label(input.get(key))) {
                    let sub = label(coalesce(proposed.get(sub), input.get(sub))).map(|s| format!(" ({s})")).unwrap_or_default();
                    parts.push(format!("{prefix} {name}{sub}"));
                }
            }
            boolean(&mut parts, coalesce(proposed.get("arm"), input.get("arm")), "armed", "disarmed");
            if let Some(monitoring) = label(coalesce(proposed.get("monitoring"), input.get("monitoring"))) {
                parts.push(format!("monitoring {monitoring}"));
            }
            with_track(format!("{}: {}", known.as_ref().map(|t| t.name.as_str()).unwrap_or("Track"), parts_text(parts, "routing")), known)
        }
        "set_mixer_options" => {
            let known = known(track, coalesce(preview.get("trackRef"), input.get("trackRef")));
            let mut parts = vec![];
            boolean(&mut parts, field("trackActivator"), "switched on", "switched off");
            if let Some(assign) = finite(field("crossfadeAssign")) {
                parts.push(format!(
                    "crossfade {}",
                    index_name(assign, &["none", "A", "B"]).map(str::to_owned).unwrap_or_else(|| number::to_string(assign))
                ));
            }
            if let Some(mode) = finite(field("panningMode")) {
                parts.push(if mode == 1.0 { "split stereo pan" } else { "stereo pan" }.into());
            }
            if finite(field("crossfader")).is_some() {
                parts.push("crossfader moved".into());
            }
            with_track(
                format!("{}: {}", known.as_ref().map(|t| t.name.as_str()).unwrap_or("Track"), parts_text(parts, "mixer options")),
                known,
            )
        }
        "edit_rack_mapping" => {
            let kind = text(input.get("kind"));
            let known = owner(input.get("ref"), track);
            let title = if matches!(kind.as_str(), "key-zone" | "velocity-zone" | "selector-zone") {
                format!(
                    "Changed {} zone to {}–{}, fades {}–{}",
                    kind.replacen("-zone", "", 1),
                    text(coalesce(proposed.get("minimum"), input.get("minimum"))),
                    text(coalesce(proposed.get("maximum"), input.get("maximum"))),
                    text(coalesce(proposed.get("fadeMinimum"), input.get("fadeMinimum"))),
                    text(coalesce(proposed.get("fadeMaximum"), input.get("fadeMaximum")))
                )
            } else {
                let mac = finite(input.get("macroIndex"))
                    .map(|n| format!("Macro {}", number::to_string(n + 1.0)))
                    .unwrap_or_else(|| "macro".into());
                let name = text(input.get("name").filter(|v| !v.is_null()).or(Some(&json!(""))));
                match input.get("kind").and_then(Value::as_str) {
                    Some("macro-name") => format!("Renamed {mac} to “{name}”"),
                    Some("variation-name") => format!("Renamed variation to “{name}”"),
                    _ if input.get("mappingIndex") == Some(&Value::Null) => "Removed macro mapping".into(),
                    _ => finite(input.get("mappingIndex"))
                        .map(|n| format!("Mapped parameter to Macro {}", number::to_string(n + 1.0)))
                        .unwrap_or_else(|| "Changed macro mapping".into()),
                }
            };
            with_track(title, known)
        }
        "set_clip_follow_actions" => with_track("Changed clip Follow Actions", owner(input.get("clipRef"), track)),
        "set_clip" => {
            let mut parts = vec![];
            boolean(&mut parts, proposed.get("muted"), "muted", "unmuted");
            if finite(proposed.get("loopEnd")).is_some() || finite(proposed.get("loopStart")).is_some() {
                let start = finite(proposed.get("loopStart")).or_else(|| finite(prior.get("loopStart"))).unwrap_or(0.0);
                if let Some(end) = finite(proposed.get("loopEnd")).or_else(|| finite(prior.get("loopEnd"))) {
                    let was = finite(prior.get("loopEnd"))
                        .map(|end| format!(" (was {})", span(end - finite(prior.get("loopStart")).unwrap_or(0.0))))
                        .unwrap_or_default();
                    parts.push(format!("loop {}{was}", span(end - start)));
                }
            }
            if proposed.get("looping") != prior.get("looping") {
                boolean(&mut parts, proposed.get("looping"), "looping", "not looping");
            }
            if let Some(mode) = finite(proposed.get("launchMode")) {
                parts.push(format!("{} mode", index_name(mode, &["trigger", "gate", "toggle", "repeat"]).unwrap_or("launch")));
            }
            for (key, title) in [("launchQuantization", "launch quantization"), ("colorIndex", "colour")] {
                if finite(proposed.get(key)).is_some() {
                    parts.push(title.into());
                }
            }
            boolean(&mut parts, proposed.get("legato"), "legato on", "legato off");
            if let Some(n) = finite(proposed.get("velocityAmount")) {
                parts.push(format!("velocity {}%", percent(n)));
            }
            boolean(&mut parts, proposed.get("ramMode"), "RAM mode on", "RAM mode off");
            let known = clip_track();
            with_track(format!("{}: {}", clip_name(&known, false), parts_text(parts, "settings")), known)
        }
        "set_audio_clip" => {
            let mut parts = vec![];
            if let Some(n) = finite(field("pitchCoarse")) {
                parts.push(format!("pitch {}{} st", if n > 0.0 { "+" } else { "" }, number::to_string(n)));
            }
            for (key, title) in [("pitchFine", "fine pitch"), ("gain", "gain")] {
                if finite(field(key)).is_some() {
                    parts.push(title.into());
                }
            }
            boolean(&mut parts, field("warping"), "warped", "unwarped");
            if let Some(n) = finite(field("warpMode")) {
                parts.push(format!(
                    "{} mode",
                    index_name(n, &["Beats", "Tones", "Texture", "Re-Pitch", "Complex", "REX", "Complex Pro"]).unwrap_or("warp")
                ));
            }
            if finite(field("loopStart")).is_some() || finite(field("loopEnd")).is_some() {
                parts.push("loop".into());
            }
            let known = clip_track();
            with_track(format!("{}: {}", clip_name(&known, true), parts_text(parts, "settings")), known)
        }
        "edit_clip" => {
            let action = label(coalesce(preview.get("action"), input.get("action"))).unwrap_or_else(|| "edit".into());
            let length = finite(prior.get("length")).or_else(|| finite(prior.get("loopEnd")));
            let known = clip_track();
            let what = known.as_ref().map(|t| format!("{} clip", t.name)).unwrap_or_else(|| "clip".into());
            let title = match action.as_str() {
                "duplicate-loop" => format!(
                    "Doubled the loop of the {what}{}",
                    length.map(|n| format!(" ({} → {})", span(n), span(n * 2.0))).unwrap_or_default()
                ),
                "crop" => format!("Cropped the {what} to its loop"),
                "duplicate-region" => format!("Copied part of the {what} within it"),
                _ => format!("Moved the {what}'s play position"),
            };
            with_track(title, known)
        }
        "duplicate_clip" | "move_clip" => {
            let duplicate = kind.tool == "duplicate_clip";
            let dest = if duplicate { record(preview.get("destination")) } else { &empty };
            let at = finite(if duplicate {
                coalesce(dest.get("arrangementPosition"), input.get("arrangementPosition"))
            } else {
                input.get("position")
            });
            let scene = finite(coalesce(dest.get("targetSceneIndex"), input.get("targetSceneIndex")));
            let target = known(track, coalesce(dest.get("targetTrackRef"), input.get("targetTrackRef")));
            let known = owner(if duplicate { coalesce(preview.get("source"), input.get("clipRef")) } else { input.get("clipRef") }, track);
            let what = known.as_ref().map(|t| format!("the {} clip", t.name)).unwrap_or_else(|| "a clip".into());
            // An Arrangement move replaces what's in its new place, as dropping a clip in Live does.
            let replacing = replacing(preview);
            let title = if let Some(at) = at {
                format!(
                    "{} {what} to {}{}{replacing}",
                    if duplicate { "Copied" } else { "Moved" },
                    if duplicate { "the Arrangement at " } else { "" },
                    bars(at)
                )
            } else {
                format!(
                    "{} {what} to {}{}",
                    if duplicate { "Copied" } else { "Moved" },
                    target.as_ref().map(|t| t.name.as_str()).unwrap_or(if duplicate { "its track" } else { "another slot" }),
                    scene.map(|n| format!(", scene {}", number::to_string(n + 1.0))).unwrap_or_default()
                )
            };
            with_track(title, target.or(known))
        }
        "add_take_lane" => {
            let payload = record(preview.get("payload"));
            let known = known(track, coalesce(payload.get("trackRef"), input.get("trackRef")));
            with_track(
                format!("New take lane {}{}", quoted(coalesce(payload.get("name"), input.get("name")), ""), on(&known))
                    .replacen("  ", " ", 1),
                known,
            )
        }
        "add_arrangement_clip" => {
            let payload = record(preview.get("payload"));
            let known = known(track, coalesce(payload.get("trackRef"), input.get("trackRef")));
            let at = finite(coalesce(payload.get("position"), input.get("position")));
            // An audio clip goes by its file's name until it's given one, and is as long as the file.
            let file = coalesce(payload.get("filePath"), input.get("sample"))
                .and_then(Value::as_str)
                .and_then(|path| std::path::Path::new(path).file_stem()?.to_str().map(|stem| json!(stem)));
            let length = finite(coalesce(payload.get("length"), input.get("length"))).filter(|_| file.is_none());
            with_track(
                format!(
                    "New Arrangement {}clip {}{}{}{}{}",
                    if file.is_some() { "audio " } else { "" },
                    quoted(coalesce(payload.get("name"), input.get("name")).or(file.as_ref()), ""),
                    on(&known),
                    at.map(|n| format!(" at {}", bars(n))).unwrap_or_default(),
                    length.map(|n| format!(" ({})", span(n))).unwrap_or_default(),
                    // Laid over the clips it lands on, as Live does: each one named, with the bars it cut.
                    replacing(preview)
                )
                .replacen("  ", " ", 1),
                known,
            )
        }
        "change_notes" | "delete_notes" => {
            let remove = kind.tool == "delete_notes";
            let count = array(input.get(if remove { "noteIds" } else { "notes" })).len();
            let known = clip_track();
            with_track(
                format!("{}: {} {}", clip_name(&known, false), plural(count as f64, "note"), if remove { "deleted" } else { "changed" }),
                known,
            )
        }
        "edit_notes" => {
            let action = label(coalesce(preview.get("action"), input.get("action"))).unwrap_or_else(|| "edit".into());
            let count = finite(preview.get("notes"));
            let known = clip_track();
            let what = match action.as_str() {
                "quantize" => format!(
                    "notes quantized{}",
                    finite(input.get("grid"))
                        .filter(|n| *n != 0.0)
                        .map(|n| format!(" to 1/{}", number::to_string(number::round(4.0 / n))))
                        .unwrap_or_default()
                ),
                "quantize-pitch" => "notes moved to one pitch".into(),
                "select" => count.map(|n| format!("{} selected", plural(n, "note"))).unwrap_or_else(|| "notes selected".into()),
                "delete-range" => {
                    count.map(|n| format!("{} deleted in a range", plural(n, "note"))).unwrap_or_else(|| "notes deleted in a range".into())
                }
                "duplicate" => "notes duplicated".into(),
                _ => "notes edited".into(),
            };
            with_track(format!("{}: {what}", clip_name(&known, false)), known)
        }
        "transform_midi" => {
            let diff = record(preview.get("diff"));
            let transform = label(coalesce(preview.get("transform"), input.get("transform"))).unwrap_or_else(|| "transform".into());
            let changed = ["add", "update", "delete"].iter().map(|key| finite(diff.get(*key)).unwrap_or(0.0)).sum::<f64>();
            let semitones = finite(record(input.get("params")).get("semitones"));
            let name = match (transform.as_str(), semitones) {
                ("transpose", Some(n)) => format!("transposed {}{}", if n > 0.0 { "+" } else { "" }, number::to_string(n)),
                _ => transform.replace('-', " "),
            };
            let known = clip_track();
            with_track(
                format!(
                    "{}: {name}{}",
                    clip_name(&known, false),
                    if changed != 0.0 { format!(" ({})", plural(changed, "note")) } else { String::new() }
                ),
                known,
            )
        }
        "set_automation" => {
            let action = label(coalesce(preview.get("action"), input.get("action"))).unwrap_or_else(|| "automation".into());
            let known = owner(input.get("clipRef"), track);
            let what = match action.as_str() {
                "insert" => format!("automation drawn ({})", plural(array(input.get("points")).len() as f64, "point")),
                "insert-step" => "automation step drawn".into(),
                "create-envelope" => "automation lane added".into(),
                "delete-range" => "automation erased in a range".into(),
                "delete-envelope" => "automation lane removed".into(),
                _ => "automation edited".into(),
            };
            with_track(format!("{}: {what}", clip_name(&known, false)), known)
        }
        "change_structure" => {
            let action = label(input.get("action")).unwrap_or_default();
            let known = known(track, input.get("ref"));
            let name = known.as_ref().map(|t| quoted(Some(&json!(t.name)), "")).unwrap_or_default();
            match action.as_str() {
                "create-return" => ChangeSummary::title(trim(&format!("Added return track {}", quoted(input.get("name"), "")))),
                "delete-return" => ChangeSummary::title(trim(&format!("Deleted return track {name}"))),
                "duplicate-track" => with_track(trim(&format!("Duplicated track {name}")), known),
                _ => ChangeSummary::title("Duplicated a scene"),
            }
        }
        "set_scene" => {
            let mut parts = vec![];
            if let Some(tempo) = finite(proposed.get("tempo")).filter(|_| proposed.get("tempoEnabled") != Some(&json!(false))) {
                parts.push(format!("tempo {} BPM", rounded(tempo)));
            } else if proposed.get("tempoEnabled") == Some(&json!(false)) {
                parts.push("tempo off".into());
            }
            if let (Some(n), Some(d)) = (finite(proposed.get("signatureNumerator")), finite(proposed.get("signatureDenominator"))) {
                parts.push(format!("{}/{}", number::to_string(n), number::to_string(d)));
            }
            if finite(proposed.get("colorIndex")).is_some() {
                parts.push("colour".into());
            }
            ChangeSummary::title(format!("Scene: {}", parts_text(parts, "settings")))
        }
        "capture_scene" => ChangeSummary::title("Captured the playing clips into a new scene"),
        "switch_device" => {
            let known = owner(input.get("deviceRef"), track);
            with_track(
                format!(
                    "{} a device{}",
                    if input.get("enabled") == Some(&json!(true)) { "Switched on" } else { "Switched off" },
                    on(&known)
                ),
                known,
            )
        }
        "move_device" => {
            let known = owner(input.get("deviceRef"), track);
            let index = finite(input.get("index"));
            let at = match index {
                Some(0.0) => "to the start".into(),
                Some(-1.0) => "to the end".into(),
                _ => format!("to position {}", number::to_string(index.unwrap_or(0.0) + 1.0)),
            };
            with_track(format!("Moved a device {at}{}", on(&known)), known)
        }
        "move_device_to" => {
            let target = known(track, input.get("targetTrackRef"));
            with_track(format!("Moved a device to {}", target.as_ref().map(|t| t.name.as_str()).unwrap_or("a rack's chain")), target)
        }
        "delete_device" => {
            let device = record(preview.get("device"));
            let known = owner(coalesce(preview.get("ref"), input.get("ref")), track);
            with_track(
                format!(
                    "Deleted {}{}",
                    label(device.get("name")).unwrap_or_else(|| "a device".into()),
                    known.as_ref().map(|t| format!(" from {}", t.name)).unwrap_or_default()
                ),
                known,
            )
        }
        "set_chain" => {
            let mut parts = vec![];
            boolean(&mut parts, field("mute"), "muted", "unmuted");
            boolean(&mut parts, field("solo"), "soloed", "unsoloed");
            if finite(field("colorIndex")).is_some() || field("autoColor").is_some_and(Value::is_boolean) {
                parts.push("colour".into());
            }
            ChangeSummary::title(
                format!(
                    "{}chain {} {}",
                    label(preview.get("rackName")).map(|s| format!("{s} · ")).unwrap_or_default(),
                    quoted(preview.get("chainName"), ""),
                    parts_text(parts, "changed")
                )
                .replacen("  ", " ", 1),
            )
        }
        "set_song" => {
            let mut parts = vec![];
            let n = finite(proposed.get("signatureNumerator"));
            let d = finite(proposed.get("signatureDenominator"));
            if n.is_some() || d.is_some() {
                parts.push(format!(
                    "time signature {}/{} → {}/{}",
                    finite(prior.get("signatureNumerator")).map(number::to_string).unwrap_or_else(|| "?".into()),
                    finite(prior.get("signatureDenominator")).map(number::to_string).unwrap_or_else(|| "?".into()),
                    n.map(number::to_string).unwrap_or_else(|| text(prior.get("signatureNumerator"))),
                    d.map(number::to_string).unwrap_or_else(|| text(prior.get("signatureDenominator")))
                ));
            }
            if let Some(swing) = finite(proposed.get("swingAmount")) {
                parts.push(format!("swing {}% → {}%", percent(finite(prior.get("swingAmount")).unwrap_or(0.0)), percent(swing)));
            }
            for (key, title) in [("clipTriggerQuantization", "launch quantization"), ("midiRecordingQuantization", "record quantization")] {
                if finite(proposed.get(key)).is_some() {
                    parts.push(title.into());
                }
            }
            ChangeSummary::title(capital(parts_text(parts, "song settings")))
        }
        "set_scale" => {
            let name = finite(input.get("rootNote"))
                .and_then(|n| index_name(n, &["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]))
                .unwrap_or("");
            let text = format!("Scale {name} {}", label(input.get("scaleName")).unwrap_or_default());
            let pieces = text
                .split(|c: char| kumi_common::js::string::trim(&c.to_string()).is_empty())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            ChangeSummary::title(pieces.join(" "))
        }
        "set_groove" => ChangeSummary::title(
            if input.get("action").and_then(Value::as_str) == Some("set-amount") && finite(input.get("grooveAmount")).is_some() {
                format!("Groove amount {}%", percent(finite(input.get("grooveAmount")).unwrap()))
            } else {
                "Groove edited".into()
            },
        ),
        "replace_sample" => {
            let known = owner(input.get("deviceRef"), track);
            with_track(
                format!("Simpler{} now plays {}", on(&known), quoted(file_name(input.get("filePath")).as_ref(), "another sample")),
                known,
            )
        }
        "import_audio" => {
            let known = known(track, input.get("trackRef"));
            let scene = finite(input.get("sceneIndex"));
            let position = finite(input.get("position"));
            with_track(
                format!(
                    "Imported {}{}{}",
                    quoted(file_name(input.get("filePath")).as_ref(), "audio"),
                    on(&known),
                    scene
                        .map(|n| format!(", scene {}", number::to_string(n + 1.0)))
                        .or_else(|| position.map(|n| format!(" at {}", bars(n))))
                        .unwrap_or_default()
                ),
                known,
            )
        }
        "set_warp_markers" => {
            let known = owner(input.get("clipRef"), track);
            with_track(
                format!(
                    "{}: warp marker {}",
                    clip_name(&known, true),
                    match label(input.get("action")).as_deref() {
                        Some("add") => "added",
                        Some("delete") => "removed",
                        _ => "moved",
                    }
                ),
                known,
            )
        }
        "capture_midi" => ChangeSummary::title("Captured MIDI into a clip"),
        "set_device_details" => {
            let known = owner(coalesce(input.get("deviceRef"), input.get("ref")), track);
            with_track(format!("Device settings changed{}", on(&known)), known)
        }
        "use_looper" => ChangeSummary::title(format!("Looper: {}", label(input.get("action")).unwrap_or_else(|| "settings".into()))),
        "delete_clip" => {
            let clip = record(coalesce(preview.get("clip"), preview.get("target")));
            let known = clip_track();
            with_track(
                format!("Deleted clip {}{}", quoted(coalesce(clip.get("name"), preview.get("name")), ""), on(&known))
                    .replacen("  ", " ", 1),
                known,
            )
        }
        "delete_scene" => {
            let scene = record(coalesce(preview.get("scene"), preview.get("target")));
            ChangeSummary::title(trim(&format!(
                "Deleted scene {}",
                quoted(
                    coalesce(scene.get("name"), preview.get("name")),
                    &finite(scene.get("index")).map(|n| number::to_string(n + 1.0)).unwrap_or_default()
                )
            )))
        }
        "delete_track" => {
            let known = known(track, input.get("trackRef"));
            let row = record(preview.get("track"));
            let also = array(row.get("alsoDeletes")).len();
            let name = known.as_ref().map(|t| json!(t.name));
            ChangeSummary::title(
                format!(
                    "Deleted track {}{}",
                    quoted(coalesce(name.as_ref(), row.get("name")), ""),
                    if also > 0 { format!(" and the {} in it", plural(also as f64, "track")) } else { String::new() }
                )
                .replacen("  ", " ", 1),
            )
        }
        "delete_locator" => {
            let locator = record(coalesce(preview.get("locator"), preview.get("target")));
            let at = finite(coalesce(locator.get("position"), locator.get("time")));
            ChangeSummary::title(
                format!(
                    "Deleted locator {}{}",
                    quoted(locator.get("name"), ""),
                    at.map(|n| format!(" at {}", bars(n))).unwrap_or_default()
                )
                .replacen("  ", " ", 1),
            )
        }
        "write_arrangement_clip" => {
            let clips: Vec<_> = input
                .get("clips")
                .and_then(Value::as_array)
                .map(|rows| rows.iter().map(|v| record(Some(v))).collect())
                .unwrap_or_else(|| vec![input]);
            let partial = record(applied.and_then(|row| row.get("partial")));
            let first = clips.first().copied().unwrap_or(&empty);
            let known = known(track, first.get("trackRef"));
            let notes = clips.iter().map(|clip| array(clip.get("notes")).len()).sum::<usize>();
            let start = finite(first.get("start"));
            let length = finite(first.get("length"));
            let drawn = drawn_notes(array(first.get("notes")));
            let title = if let (Some(made), Some(of)) = (finite(partial.get("made")), finite(partial.get("of"))) {
                format!("{} of {} made; the others aren't there", number::to_string(made), plural(of, "new Arrangement clip"))
            } else if clips.len() > 1 {
                format!("{} · {}", plural(clips.len() as f64, "new Arrangement clip"), plural(notes as f64, "note"))
            } else {
                format!(
                    "New Arrangement clip {}{} · {}",
                    quoted(first.get("name"), ""),
                    start.map(|n| format!(" at {}", bars(n))).unwrap_or_default(),
                    plural(notes as f64, "note")
                )
                .replacen("  ", " ", 1)
            };
            let clip = if clips.len() == 1 && length.is_some_and(|n| n > 0.0) && !drawn.is_empty() {
                Some(ClipPicture { length: length.unwrap(), notes: drawn })
            } else {
                None
            };
            ChangeSummary { title, track: known, clip, ..Default::default() }
        }
        "clear_range" => {
            let known = known(track, input.get("trackRef"));
            let from = finite(input.get("fromBeat"));
            let to = finite(input.get("toBeat"));
            let removed = array(preview.get("removes")).len();
            let cut = array(preview.get("cuts")).len();
            let range = match (from, to) {
                (Some(from), Some(to)) => format!(" from {} to {}", bars(from), bars(to)),
                _ => String::new(),
            };
            let mut parts = vec![];
            if removed > 0 {
                parts.push(format!("{} gone", plural(removed as f64, "clip")));
            }
            if cut > 0 {
                parts.push(format!("{} cut", plural(cut as f64, "clip")));
            }
            with_track(
                format!(
                    "Cleared{range}{}{}",
                    on(&known),
                    if parts.is_empty() { String::new() } else { format!(" ({})", parts.join(", ")) }
                ),
                known,
            )
        }
        "edit_device" => {
            let device = record(coalesce(preview.get("device"), preview.get("target")));
            let known = owner(input.get("deviceRef"), track);
            let what = match input.get("action").and_then(Value::as_str) {
                Some("set") => format!(
                    "{} set",
                    text(input.get("setting").filter(|v| !v.is_null()).or(Some(&json!("a setting"))))
                        .rsplit('.')
                        .next()
                        .unwrap()
                        .replace('_', " ")
                ),
                Some("modulate") => "modulation amount set".into(),
                _ => text(input.get("action").filter(|v| !v.is_null()).or(Some(&json!("edited")))).replace('-', " "),
            };
            with_track(format!("{}: {what}", label(device.get("name")).unwrap_or_else(|| "Device".into())), known)
        }
        "duplicate_device" => {
            let device = record(coalesce(preview.get("device"), preview.get("target")));
            let known = owner(input.get("deviceRef"), track);
            with_track(format!("Duplicated {}{}", label(device.get("name")).unwrap_or_else(|| "a device".into()), on(&known)), known)
        }
        _ => return None,
    };
    Some(summary)
}
