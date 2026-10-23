//! The stems tool, the same in every DAW without a splitter of its own: renders a track over a stretch, splits it into drums,
//! bass and other with Kumi's own splitter, writes each as a sound file and puts them back beside the track.
use super::backend::StemsBackend;
use super::split::{split, BASS_HZ};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use kumi_types::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::rc::Rc;

pub const TOOL: &str = "stems";

/// The longest stretch split (quarter notes) and the longest it may last (seconds): a split holds the stretch's spectra.
const MOST_BEATS: f64 = 128.;
const MOST_SECONDS: f64 = 90.;

pub struct Stems<B: StemsBackend> {
    backend: B,
    description: String,
}

/// A file name of letters, digits, spaces, dashes and dots.
fn plain(name: &str) -> String {
    let kept: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '.') { c } else { '_' }).collect();
    kept.trim().chars().take(60).collect()
}

impl<B: StemsBackend + 'static> Stems<B> {
    pub fn new(backend: B) -> Rc<Self> {
        let description = description(backend.words());
        Rc::new(Stems { backend, description })
    }

    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(StemsTool { owner: self.clone() })
    }

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        let track = input.get("track").and_then(Value::as_str).ok_or("track: the track to split")?;
        let number = |key: &str| input.get(key).and_then(Value::as_f64);
        let (from, beats) = match number("fromBeat").zip(number("beats")) {
            Some((from, beats)) if from >= 0. && beats > 0. && beats <= MOST_BEATS => (from, beats),
            _ => return Err(format!("fromBeat and beats: the stretch to split (at most {MOST_BEATS} beats)")),
        };
        let (render, name) = self.backend.render(track, from, beats, &signal).await?;
        let seconds = render.seconds.filter(|seconds| *seconds > 0.).ok_or("the render didn't say how long it is")?;
        if seconds > MOST_SECONDS {
            return Err(format!("that stretch lasts {seconds:.0} s: Kumi splits {MOST_SECONDS:.0} s at most at once; split it in parts"));
        }
        let ([left, right], rate) =
            kumi_media::audio::decode::read_stereo(&render.file, &signal, render.start, seconds).await.map_err(|error| error.0)?;
        // (in Kumi's listening folder, which Kumi clears, and gone once placed: the DAW keeps its own copy)
        let folder = self.backend.folder();
        let base = format!("{} {}", plain(&name), kumi_common::time::now_ms());
        let stem_names = name.clone();
        // (the split and the writes off Kumi's own thread, so the conversation keeps going; Esc stops the wait)
        let writing = tokio::task::spawn_blocking(move || -> Result<Vec<(String, PathBuf)>, String> {
            let parts = split(&left, &right, rate);
            std::fs::create_dir_all(&folder).map_err(|error| format!("Kumi couldn't make its listening folder: {error}"))?;
            let mut files = Vec::new();
            for (stem, channels) in [("drums", &parts.drums), ("bass", &parts.bass), ("other", &parts.other)] {
                let path = folder.join(format!("{base} {stem}.wav"));
                std::fs::write(&path, kumi_media::audio::decode::float_wav(&channels[..], rate as u32))
                    .map_err(|error| format!("Kumi couldn't write the {stem} stem: {error}"))?;
                files.push((format!("{stem_names} {stem}"), path));
            }
            Ok(files)
        });
        let files = tokio::select! {
            files = writing => files.map_err(|error| error.to_string())??,
            _ = signal.cancelled() => return Err("stopped".into()),
        };
        let stems: Vec<(String, &std::path::Path)> = files.iter().map(|(stem, path)| (stem.clone(), path.as_path())).collect();
        let made =
            if signal.is_cancelled() { Err("stopped".into()) } else { self.backend.place(track, &stems, from, beats, &signal).await };
        for (_, path) in &files {
            let _ = std::fs::remove_file(path);
        }
        let made = made?;
        let named: Vec<&str> = files.iter().map(|(stem, _)| stem.as_str()).collect();
        Ok(json!({
            "stems": named,
            "made": made,
            "said": format!("split by Kumi's own splitter: drums (what hits), bass (what sustains under {BASS_HZ:.0} Hz) and other (the rest, voices too); together they play as the track did, beside it: mute the track to hear the stems alone"),
        }))
    }
}

fn description(words: &super::backend::Words) -> String {
    format!(
        "Split a track's stretch in {app} into stems with Kumi's own splitter: drums (what hits), bass (what sustains under 150 Hz) and other (the rest, voices too), which add up to the track. track, fromBeat and beats (at most 128 beats, 90 s). The stems go on {placed}. Kumi renders the track, so nothing plays meanwhile.",
        app = words.app,
        placed = words.placed
    )
}

fn schema() -> JsonObject {
    let value = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["track", "fromBeat", "beats"],
        "properties": {
            "track": {"type": "string", "description": "The track to split"},
            "fromBeat": {"type": "number", "minimum": 0},
            "beats": {"type": "number", "exclusiveMinimum": 0, "maximum": MOST_BEATS}
        }
    });
    value.as_object().cloned().unwrap_or_default()
}

struct StemsTool<B: StemsBackend> {
    owner: Rc<Stems<B>>,
}

#[async_trait(?Send)]
impl<B: StemsBackend + 'static> KernelTool for StemsTool<B> {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        &self.owner.description
    }

    fn input_schema(&self) -> JsonObject {
        schema()
    }

    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(match self.owner.run(input, signal).await {
            Ok(reply) => ToolResult { text: stringify(&reply), ..Default::default() },
            Err(why) => ToolResult { text: why, is_error: true, ..Default::default() },
        })
    }

    fn reach(&self) -> Reach {
        Reach::DawWrite
    }

    fn deferred(&self) -> bool {
        true
    }
}
