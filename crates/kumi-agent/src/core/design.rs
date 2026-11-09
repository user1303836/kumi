//! Sound design for the producer, the same in every DAW: cross, fuse or stack a group of their sounds into a new one,
//! or grow one toward another (kumi-design renders it), kept as a file in Kumi's own folder. The app's PALETTE and map
//! run it on what the producer picked; the `design_sound` tool runs it when they ask Kumi. Nothing in the DAW changes.

use crate::core::palette::Palettes;
use async_trait::async_trait;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_design::{fresh_seed, load_source, render, schema, source_seconds, Answer, Mode, Op, Options, Request, SourceRef, MAX_SEED, MIN_SOURCES};
use kumi_media::audio::DecodeOptions;
use kumi_types::{
    contracts::{JsonObject, KernelTool, Reach, ToolResult},
    errors::RuntimeError,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

pub const TOOL: &str = "design_sound";

/// A design made: its file and what the engine answered (the seed it used, its recipe, peaks and length).
#[derive(Debug, Clone)]
pub struct Made {
    pub path: PathBuf,
    pub answer: Answer,
}

/// A request for these files (each one's name its file's), the op and mode, the seed (a fresh one when None) and options.
pub fn request(op: Op, mode: Option<Mode>, files: &[String], seed: Option<u64>, options: Options) -> Request {
    let sources = files
        .iter()
        .map(|file| SourceRef {
            file: Some(file.clone()),
            name: Path::new(file).file_stem().map(|stem| stem.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .collect();
    Request { op, mode, sources, seed, options }
}

/// What a made sound's file is called: the op or mode, its first source, how many more, and the seed, so every
/// variation keeps a name of its own ("Blend - Kick 909 + 2 - 4821.wav").
pub fn file_name(request: &Request, seed: u64) -> String {
    let what = match (request.op, request.mode) {
        (Op::Grow, _) => "Grow",
        (_, None) => "Stack",
        (_, Some(mode)) => match mode {
            Mode::Blend => "Blend",
            Mode::Attack => "Attack",
            Mode::Resonate => "Resonate",
            Mode::Modulate => "Modulate",
            Mode::Texture => "Texture",
            Mode::Loop => "Loop",
            Mode::Smear => "Smear",
        },
    };
    let first: String = request
        .sources
        .first()
        .and_then(|s| s.name.clone())
        .unwrap_or_default()
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control())
        .take(40)
        .collect();
    let more = request.sources.len().saturating_sub(1);
    format!("{what} - {} + {more} - {seed}.wav", first.trim())
}

/// Renders the request (its sources files) into `folder`: decoded, designed on a blocking thread, written whole.
pub async fn make(mut request: Request, folder: &Path, signal: &Signal) -> Result<Made, String> {
    request.check().map_err(|e| e.0)?;
    let mut sources = Vec::with_capacity(request.sources.len());
    let seconds = source_seconds(request.sources.len());
    for source in &request.sources {
        let file = source.file.as_deref().ok_or("Each sound is a file.")?;
        let name = source.name.clone().unwrap_or_else(|| file.to_string());
        let decoded = load_source(Path::new(file), &name, seconds, &DecodeOptions::default(), Some(signal.clone()))
            .await
            .map_err(|e| format!("{name}: {}", e.0))?;
        sources.push(decoded);
    }
    let seed = *request.seed.get_or_insert_with(fresh_seed);
    let job = request.clone();
    let rendered = tokio::task::spawn_blocking(move || render(&job, seed, &sources, &mut |_| {}))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.0)?;
    if signal.is_cancelled() {
        return Err("Stopped.".into());
    }
    std::fs::create_dir_all(folder).map_err(|e| format!("Kumi couldn't make its folder for designed sounds: {e}"))?;
    let path = folder.join(file_name(&request, seed));
    let partial = path.with_extension("wav.part");
    std::fs::write(&partial, rendered.wav())
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| format!("Kumi couldn't write the sound: {e}"))?;
    Ok(Made { path, answer: Answer::new(&request, seed, &rendered, None) })
}

const DESCRIPTION: &str = concat!(
    "Make a new sound from 2 or more of the producer's sound files, as many as they picked, changing nothing in the DAW. ",
    "cross blend: one body morphed from all of them; cross attack: one's transient (attack_from) onto the blend of the rest; ",
    "cross resonate: some ring through the others (options.resonate.pairing: halves, in-order, interleave, bright-dark, dark-bright, hits-tones, tones-hits, one-all, all-one, chain; split names the exciters); ",
    "cross modulate: one carrier shaped by the others (options.modulate: carrier, kinds of phase, ring, am, fm, filter, gain, vocode, stages); ",
    "fuse texture: a loopable bed that moves between them (seconds); fuse loop: a beat of their hits or a melody of them repitched ",
    "(bpm: give the DAW's tempo; bars 1-8; options.loop: style, meter, grid, swing, density, fills, lean, key, scale, tuning and more); ",
    "fuse smear: a 4-16x granular stretch with a long diffuse tail (stretch); stack: printed as one hit, aligned and loudness-matched; ",
    "grow: the first file part of the way (options.at, 0 to 1) toward the second (exactly 2 files), its in-between sounds new. ",
    "options.steer pushes any design before it renders (bright, body, filter, noise, punch, length, drive, crush, width, space; per sound pitch, reverse, start, delay, pan, level). ",
    "A seed picks every choice: a reroll is the same call with another seed (or none), the same seed the same sound. ",
    "keep: put the made sound in the producer's palette (only when they ask or agree). Returns its file, which the palette, ",
    "map_samples and listen take, and a one-line recipe to tell them."
);

/// The model's way to the same designs: `design_sound`.
pub struct DesignTool {
    folder: PathBuf,
    palettes: Option<Rc<Palettes>>,
}

impl DesignTool {
    pub fn new(folder: PathBuf, palettes: Option<Rc<Palettes>>) -> Rc<dyn KernelTool> {
        Rc::new(DesignTool { folder, palettes })
    }

    /// One of the tool's arguments read as its type, or what's wrong with it.
    fn field<T: serde::de::DeserializeOwned>(input: &JsonObject, key: &str) -> Result<Option<T>, String> {
        input.get(key).cloned().map(serde_json::from_value).transpose().map_err(|e| format!("{key}: {e}"))
    }

    async fn run(&self, input: JsonObject, signal: &Signal) -> Result<Value, String> {
        let keep = input.get("keep").and_then(Value::as_bool).unwrap_or(false);
        let files: Vec<String> = input
            .get("files")
            .and_then(Value::as_array)
            .map(|f| f.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let op: Op = Self::field(&input, "op")?.ok_or("design_sound takes an op: cross, fuse, stack or grow")?;
        let mode: Option<Mode> = Self::field(&input, "mode")?;
        let options: Options = Self::field(&input, "options")?.unwrap_or_default();
        let seed = input.get("seed").and_then(Value::as_u64);
        let request = request(op, mode, &files, seed, options);
        let made = make(request, &self.folder, signal).await?;
        let file = made.path.to_string_lossy().into_owned();
        let mut reply = json!({
            "file": file,
            "recipe": made.answer.recipe,
            "seed": made.answer.seed,
            "seconds": made.answer.duration,
            "op": made.answer.op,
            "mode": made.answer.mode,
        });
        if keep {
            reply["palette"] = match &self.palettes {
                Some(palettes) => json!(palettes.add_paths(&[file]).await.map_err(|e| e.to_string())?.words()),
                None => json!("This Kumi keeps no palette."),
            };
        }
        Ok(reply)
    }
}

#[async_trait(?Send)]
impl KernelTool for DesignTool {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn input_schema(&self) -> JsonObject {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["op", "files"],
            "properties": {
                "op": {"type": "string", "enum": schema::ops()},
                "mode": {"type": "string", "enum": schema::modes(),
                    "description": "cross: blend, attack, resonate, modulate; fuse: texture, loop, smear; stack and grow: none"},
                "files": {"type": "array", "items": {"type": "string"}, "minItems": MIN_SOURCES,
                    "description": "The sounds' files, in order, as many as wanted (grow: exactly 2; attack_from and split count from 0)"},
                "seed": {"type": "integer", "minimum": 0, "maximum": MAX_SEED, "description": "The variation; leave it out for a new one"},
                "options": schema::options(),
                "keep": {"type": "boolean", "description": "Put the made sound in the producer's palette"}
            }
        });
        schema.as_object().cloned().unwrap_or_default()
    }

    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(match self.run(input, &signal).await {
            Ok(reply) => ToolResult { text: stringify(&reply), ..Default::default() },
            Err(why) => ToolResult { text: why, is_error: true, ..Default::default() },
        })
    }

    fn reach(&self) -> Reach {
        Reach::AwayFromDaw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &Path, hz: f64) {
        let (rate, samples) = (48_000u32, 24_000u32);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + samples * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        for (value, width) in [(16u32, 4), (1, 2), (1, 2), (rate, 4), (rate * 2, 4), (2, 2), (16, 2)] {
            bytes.extend_from_slice(&value.to_le_bytes()[..width]);
        }
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples * 2).to_le_bytes());
        for n in 0..samples {
            let t = n as f64 / rate as f64;
            bytes.extend_from_slice(&((0.7 * (std::f64::consts::TAU * hz * t).sin() * (-t * 8.0).exp() * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[tokio::test]
    async fn the_tool_makes_a_named_variation_and_says_its_seed() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<String> = [("Kick 01.wav", 60.0), ("Bell.wav", 880.0)]
            .iter()
            .map(|(name, hz)| {
                let path = dir.path().join(name);
                hit(&path, *hz);
                path.to_string_lossy().into_owned()
            })
            .collect();
        let tool = DesignTool::new(dir.path().join("made"), None);
        let call = |seed: u64| {
            let input = json!({"op": "cross", "mode": "resonate", "files": files, "seed": seed, "keep": true});
            tool.execute(input.as_object().unwrap().clone(), Signal::new())
        };
        let made = call(5).await.unwrap();
        assert!(!made.is_error, "{}", made.text);
        let reply: Value = serde_json::from_str(&made.text).unwrap();
        let file = PathBuf::from(reply["file"].as_str().unwrap());
        assert_eq!(file.file_name().unwrap(), "Resonate - Kick 01 + 1 - 5.wav");
        assert!(std::fs::metadata(&file).unwrap().len() > 44);
        assert_eq!(reply["seed"], 5);
        assert_eq!(reply["palette"], "This Kumi keeps no palette.");
        // Another seed is another file beside it; the first stays.
        let other: Value = serde_json::from_str(&call(6).await.unwrap().text).unwrap();
        assert_ne!(other["file"], reply["file"]);
        assert!(file.exists());
        // Refusals come back as the tool's error, in words.
        let one = json!({"op": "stack", "files": [files[0]]});
        let refused = tool.execute(one.as_object().unwrap().clone(), Signal::new()).await.unwrap();
        assert!(refused.is_error && refused.text.contains("2 or more"), "{}", refused.text);
    }
}
