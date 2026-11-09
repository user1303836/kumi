//! `sound`, `audition`, `groove`, `form` and `render`: Live's own listening tools (Kumi's), through the runtime, as `judge` and
//! `tune` are (see judge.rs for what every call shares: Live only, on the project key and epoch an observe gave, its
//! rows one change in HISTORY). What each adds here:
//! - `sound` measures 1–8 tracks heard side by side in one Kumi Ears pass, from_beat on (8 bars unless beats says).
//! - `audition` scores up to 8 candidates against a reference in one pass. Its reference is a clip ref or a source the
//!   runtime resolves (never a path), heard for at most 60 s. Session-clip candidates are played from temporary
//!   Arrangement copies that Live's integration journals and takes away as the pass closes. It's said in HISTORY as a
//!   heard row, as natively.
//! - `groove` judges a part's feel on its notes, against a MIDI clip's: it never plays (no `audio` stem, no audio-file
//!   reference, so nothing to transcribe and no model). `apply`, and a round taking a change back, are changes in
//!   HISTORY, undone as natively.
//! - `form` reads the song's form bar by bar: the one listen that plays the whole Arrangement (the owner's call), up
//!   to 15 minutes in all with every other condition of the plugin host's listening; a stop ends it at once. Its
//!   reference is as an audition's.
//! - `render` renders an audio track's own clips offline, at most 60 s of them; the file Live wrote is kept as the
//!   runtime's own audio (an artifact_id) and Live's copy deleted: no path comes back.
//!
//! Every pass keeps to the plugin host's listening (live_pass.rs): Kumi Ears alone, Live stopped and not recording, a
//! minute a pass with its lead-in (form: the whole Arrangement, 15 minutes at most), transport and loop put back,
//! captures in the call's own folder.

use super::ctx::{Call, Ctx, Effect, JsonObject, OpFuture, Operation};
use super::judge::{listened, refusal, refused, tools_input};
use super::listen::{source_schema, Source};
use super::live_pass::{beats_per_bar, readable_clip, readable_reference, whole_song_refusal, window_refusal, Capturing, MOST_SECONDS};
use kumi_common::abort::Signal;
use kumi_listening::tools::{audition_tool, form_tool, groove_tool, render_tool, sound_tool};
use kumi_types::contracts::ToolResult;
use kumi_types::daw::DawProfile;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub static SOUND: Operation = Operation {
    name: "sound",
    description: "Kumi measures sounds in Live: 1–8 tracks heard side by side in one Kumi Ears pass, from from_beat (8 bars \
                  unless beats says; Live plays at most 60 seconds, a bar before and half a bar after included). Each sound's \
                  attack, decay, sustain, brightness, pitch drop, modulation, warmth, width, noise floor and tail; its effects \
                  (reverb time, echoes, swings, pumping); its problems; a kit verdict; how it ducks under or plays against \
                  `against`; cents from `key`. Nothing in the Set changes. Made on the project key and epoch an observe gave.",
    schema: sound_schema,
    effect: |_| Effect::Playback,
    run: run_sound,
};

pub static AUDITION: Operation = Operation {
    name: "audition",
    description: "Kumi plays a part in Live once and hears up to 8 candidate tracks after their devices (or the whole mix, \
                  alone), scoring each 0–100 against a reference with its gaps: the producer hears the first candidate, Kumi \
                  the others on their own. The part is from_beat and beats in the Arrangement (8 beats unless said; Live plays \
                  at most 60 seconds with a bar before and half a bar after), or a Session clip per candidate, played from \
                  temporary Arrangement copies Kumi removes after. The reference is an audio clip's clip ref or a source (a \
                  map sound, a file Kumi made, an approved file), heard for at most 60 seconds. Made on the project key and \
                  epoch an observe gave.",
    schema: audition_schema,
    effect: |_| Effect::Playback,
    run: run_audition,
};

pub static GROOVE: Operation = Operation {
    name: "groove",
    description: "Kumi shapes a MIDI part's feel toward a reference's, judged on the notes with no listening: timing per \
                  lane, swing, accents, syncopation, fills. Start a run with clip (the part's clip ref) and reference (a \
                  MIDI clip's clip ref); then change (judge ONE change you made with kumi_change: kept or taken back by Kumi), \
                  apply (Kumi moves the notes toward the reference, amount 0–1, and judges that), and done. apply and a take-back \
                  are changes in HISTORY that kumi_undo takes back. Made on the project key and epoch an observe gave.",
    schema: groove_schema,
    effect: groove_effect,
    run: run_groove,
};

pub static FORM: Operation = Operation {
    name: "form",
    description: "Kumi plays the Arrangement through Kumi Ears and reads its form bar by bar: energy over the song, \
                  sections (role and letter), turns, intro and outro, the first hook, which tracks play where, form \
                  problems; with a reference (an audio clip's clip ref or a source), its form and the differences. It hears \
                  the whole Arrangement by default (from_beat and beats for part of it), so it plays as long as the song: \
                  at most 15 minutes, with Live stopped and not recording, everything put back after; a stop ends it at once. \
                  Made on the project key and epoch an observe gave.",
    schema: form_schema,
    effect: |_| Effect::Playback,
    run: run_form,
};

pub static RENDER: Operation = Operation {
    name: "render",
    description: "Kumi renders an audio track's own clips (its recordings, a bounce) between from_beat and from_beat + beats \
                  to a file, offline and silent: the raw audio before the track's devices, at most 60 seconds of it. The file \
                  is kept as Kumi's audio and given as artifact_id with its measures: hear or compare it with kumi_listen \
                  (source artifact_id). Made on the project key and epoch an observe gave.",
    schema: render_schema,
    effect: |_| Effect::Playback,
    run: run_render,
};

fn run_sound(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(listened(ctx, call, "sound"))
}

fn run_audition(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(listened(ctx, call, "audition"))
}

fn run_groove(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(listened(ctx, call, "groove"))
}

fn run_form(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(listened(ctx, call, "form"))
}

fn run_render(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(listened(ctx, call, "render"))
}

/// The runtime's own fields beside the tool's: the project key and epoch an observe gave.
fn bound(schema: &mut Map<String, Value>) {
    let properties = schema["properties"].as_object_mut().expect("a tool schema has properties");
    properties.insert("expected_project_key".into(), json!({"type": "string", "minLength": 1, "maxLength": 64}));
    properties.insert("expected_epoch".into(), super::ctx::epoch_schema());
    let required = schema.entry("required").or_insert_with(|| json!([])).as_array_mut().expect("required is a list");
    required.extend([json!("expected_project_key"), json!("expected_epoch")]);
}

fn properties(schema: &mut Map<String, Value>) -> &mut Map<String, Value> {
    schema["properties"].as_object_mut().expect("a tool schema has properties")
}

fn sound_schema() -> Value {
    let mut schema = sound_tool(&DawProfile::ABLETON).schema;
    // Where it listens is said, never the playhead's place.
    schema["required"].as_array_mut().expect("sound requires tracks").push(json!("from_beat"));
    bound(&mut schema);
    Value::Object(schema)
}

/// A reference as the runtime takes one: an audio clip in the Set by its clip ref, or a source it resolves.
fn reference_schema(what: &str) -> Value {
    json!({"description": format!("{what}: an audio clip in the Set by its clip ref, or a source (a map sound, a file Kumi made, an approved file)"),
    "anyOf": [
        {"type": "object", "additionalProperties": false, "required": ["clip_ref"],
            "properties": {"clip_ref": {"type": "string", "minLength": 1, "maxLength": 256}}},
        source_schema()
    ]})
}

fn audition_schema() -> Value {
    let mut schema = audition_tool(&DawProfile::ABLETON).schema;
    let fields = properties(&mut schema);
    fields.insert("reference".into(), reference_schema("What to match"));
    fields["reference_seconds"]["maximum"] = json!(MOST_SECONDS);
    fields["reference_seconds"]["description"] = json!("How much of the reference to hear, seconds (60 at most, and when left out)");
    bound(&mut schema);
    Value::Object(schema)
}

fn groove_schema() -> Value {
    let mut schema = groove_tool(&DawProfile::ABLETON).schema;
    let fields = properties(&mut schema);
    // MIDI clip references only: no stem to hear, no audio file to transcribe.
    fields.remove("audio");
    fields.remove("reference_tempo");
    fields["reference"]["description"] = json!("Starts a run: the reference's MIDI clip (its clip ref)");
    bound(&mut schema);
    Value::Object(schema)
}

fn form_schema() -> Value {
    let mut schema = form_tool(&DawProfile::ABLETON).schema;
    let fields = properties(&mut schema);
    fields.insert("reference".into(), reference_schema("A song to compare the form with"));
    fields["beats"]["description"] = json!("How long, in beats (to the song's last clip when left out): at most 15 minutes at the tempo");
    bound(&mut schema);
    Value::Object(schema)
}

fn render_schema() -> Value {
    let mut schema = render_tool(&DawProfile::ABLETON).schema;
    properties(&mut schema)["beats"]["description"] = json!("How long, in beats: at most 60 seconds at the Set's tempo");
    bound(&mut schema);
    Value::Object(schema)
}

/// A groove run's change or apply may move the part's notes (or take a change back); a start or done reads them.
fn groove_effect(args: &JsonObject) -> Effect {
    if args.contains_key("change") || args.get("apply") == Some(&Value::Bool(true)) {
        Effect::Write
    } else {
        Effect::Read
    }
}

/// What a sound, audition, groove or render call asks the integration's tool, refused (with its words) when it can't be:
/// its stretch fitted to the plugin host's minute, and an audition's reference resolved.
pub(super) async fn input(
    ctx: &Ctx,
    name: &str,
    args: &JsonObject,
    call: &Call,
    capturing: &Capturing,
) -> Result<(Map<String, Value>, Option<super::artifacts::StagedArtifact>), Value> {
    let mut input = tools_input(args);
    let number = |key: &str| args.get(key).and_then(Value::as_f64);
    match name {
        "sound" => {
            let from = number("from_beat").ok_or_else(|| refused("invalid", "sound listens from from_beat (8 bars unless beats says)."))?;
            let beats = number("beats").unwrap_or(8. * beats_per_bar(ctx));
            if let Some(error) = window_refusal(ctx, from, beats) {
                return Err(refusal(error));
            }
            Ok((input, None))
        }
        "form" => {
            if let Some(beats) = number("beats") {
                if let Some(error) = whole_song_refusal(ctx, number("from_beat").unwrap_or(0.), beats) {
                    return Err(refusal(error));
                }
            }
            let mut staged = None;
            if let Some(reference) = input.get_mut("reference") {
                let (resolved, copy) = resolved_reference(ctx, reference, call, capturing).await?;
                staged = copy;
                *reference = json!(resolved);
            }
            Ok((input, staged))
        }
        "audition" => {
            if let Some(from) = number("from_beat") {
                if let Some(error) = window_refusal(ctx, from, number("beats").unwrap_or(8.)) {
                    return Err(refusal(error));
                }
            }
            let mut staged = None;
            if let Some(reference) = input.get_mut("reference") {
                let (resolved, copy) = resolved_reference(ctx, reference, call, capturing).await?;
                staged = copy;
                *reference = json!(resolved);
                // Its excerpt is a minute at most: the whole file is never heard.
                let seconds = number("reference_seconds").unwrap_or(MOST_SECONDS);
                if !(seconds > 0. && seconds <= MOST_SECONDS) {
                    return Err(refused("invalid", "reference_seconds is more than 0 and 60 at most."));
                }
                input.insert("reference_seconds".into(), json!(seconds));
            } else if args.contains_key("reference_seconds") || args.contains_key("reference_from_seconds") {
                return Err(refused("invalid", "reference_seconds and reference_from_seconds go with a reference."));
            }
            Ok((input, staged))
        }
        "groove" => {
            if let Some(key) = ["audio", "reference_tempo"].into_iter().find(|key| args.contains_key(*key)) {
                return Err(refused(
                    "unsupported",
                    format!("{key} isn't offered here: a groove's reference is a MIDI clip's clip ref, judged on its notes."),
                ));
            }
            if let Some(reference) = args.get("reference").and_then(Value::as_str) {
                if names_a_file(reference) {
                    return Err(refused("invalid", "A groove's reference is a MIDI clip's clip ref, never a file."));
                }
            }
            Ok((input, None))
        }
        "render" => {
            let (Some(_), Some(beats)) = (number("from_beat"), number("beats")) else {
                return Err(refused("invalid", "Give the track, from_beat and beats."));
            };
            let tempo = ctx
                .link()
                .connection()
                .and_then(|connection| connection.transport())
                .and_then(|transport| transport.tempo)
                .filter(|tempo| *tempo > 0.)
                .ok_or_else(|| refused("refused", "Kumi doesn't know the Set's tempo yet: observe, then render."))?;
            let seconds = beats * 60. / tempo;
            if !(seconds <= MOST_SECONDS) {
                return Err(refused(
                    "window_too_long",
                    format!("{beats} beats at {tempo} BPM is {seconds:.1} seconds: Kumi renders at most 60 seconds at a time here."),
                ));
            }
            Ok((input, None))
        }
        _ => Err(refused("unsupported", format!("Kumi offers no listening tool named {name} here."))),
    }
}

/// A reference for Live's listening to read: a clip ref (decoded aside when its file is compressed) or a source staged
/// and made readable, never a path a call gave. The staged copy lives as long as what's returned.
async fn resolved_reference(
    ctx: &Ctx,
    reference: &Value,
    call: &Call,
    capturing: &Capturing,
) -> Result<(String, Option<super::artifacts::StagedArtifact>), Value> {
    if let Some(clip) = reference.get("clip_ref").and_then(Value::as_str) {
        return Ok((readable_clip(ctx, clip, capturing.folder(), &call.signal).await.map_err(refusal)?, None));
    }
    let source = Source::parse(reference).map_err(refusal)?;
    let (path, copy) = readable_reference(ctx, &source, capturing.folder(), &call.signal).await.map_err(refusal)?;
    Ok((path.to_string_lossy().into_owned(), Some(copy)))
}

/// Whether a groove reference names a file rather than a clip: a path's separators, a home-relative start, an audio
/// or MIDI file's extension, or something there on disk.
fn names_a_file(reference: &str) -> bool {
    let extension = Path::new(reference).extension().map(|extension| extension.to_string_lossy().to_lowercase());
    reference.contains(['/', '\\'])
        || reference.starts_with('~')
        || extension.is_some_and(|extension| {
            ["mid", "midi"].contains(&extension.as_str()) || kumi_media::audio::AUDIO_EXTENSIONS.contains(&format!(".{extension}").as_str())
        })
        || Path::new(reference).exists()
}

/// Whether `file` is what Kumi's Live extension writes a render as: `<ms>-<token>.wav` (or AIFF) in its `renders`
/// folder. Anything else Live's side answers is never read, kept or deleted.
fn an_extension_render(file: &Path) -> bool {
    let name = file.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let Some((stem, extension)) = name.rsplit_once('.') else { return false };
    let Some((ms, token)) = stem.split_once('-') else { return false };
    file.is_absolute()
        && file.parent().and_then(Path::file_name).is_some_and(|parent| parent == "renders")
        && ["wav", "aif", "aiff"].contains(&extension.to_lowercase().as_str())
        && !ms.is_empty()
        && ms.bytes().all(|b| b.is_ascii_digit())
        && !token.is_empty()
        && token.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// What a render came to, its file kept as the runtime's own audio: measured, saved as an artifact (its id and measures
/// said) and Live's copy deleted. No path is said.
pub(super) async fn kept_render(
    ctx: &Ctx,
    capturing: &Capturing,
    ran: Result<ToolResult, RuntimeError>,
    signal: &Signal,
) -> Result<ToolResult, RuntimeError> {
    let result = match ran {
        Ok(result) if !result.is_error => result,
        other => return other,
    };
    let Some(mut said) = serde_json::from_str::<Value>(&result.text).ok().filter(Value::is_object) else {
        return Ok(ToolResult::error("Kumi's render answered something the runtime can't read."));
    };
    let Some(file) = said.as_object_mut().and_then(|said| said.remove("file")).and_then(|file| file.as_str().map(PathBuf::from)) else {
        return Ok(ToolResult::error("Kumi's render didn't say which file it wrote."));
    };
    if !an_extension_render(&file) {
        return Ok(ToolResult::error("Kumi's render wrote somewhere the runtime doesn't read from; nothing was kept."));
    }
    let extension = file.extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_else(|| "wav".into());
    let copy = capturing.folder().join(format!("render.{extension}"));
    let copied = tokio::fs::copy(&file, &copy).await;
    // Live's copy goes whatever comes of keeping it.
    let _ = tokio::fs::remove_file(&file).await;
    if let Err(error) = copied {
        return Ok(ToolResult::error(format!("Kumi couldn't keep the render: {error}")));
    }
    let kept = async {
        let staged = ctx.artifacts().stage_scratch(ctx, capturing.scratch(), &copy, signal).await?;
        let measured = super::listen::measure(ctx, staged, Path::new(&format!("render.{extension}")), signal).await?;
        super::listen::retain(ctx, measured, None, signal, None).await
    }
    .await;
    match kept {
        Ok(kept) => {
            for key in ["artifact_id", "metrics", "verdict_text"] {
                said[key] = kept[key].clone();
            }
            Ok(ToolResult::text(said.to_string()))
        }
        Err(error) => Ok(ToolResult::error(format!("Kumi couldn't keep the render: {}", error.reason()))),
    }
}

#[cfg(test)]
#[path = "listening_tests.rs"]
mod tests;
