//! The layers tool, the same in every DAW: look renders the main break and its layers over a stretch and says, layer by layer,
//! how early or late it lands, whether its polarity fights the main's, and how loud it sits, and which compressor the group
//! has; align nudges a layer, invert flips one, and glue measures how much peak-to-loudness ratio the group's compressor takes
//! (the group heard without it first: before one is added, or with it bypassed) and moves its threshold until that's as asked.
use super::backend::{Compressor, LayersBackend};
use super::{compare, plr};
use crate::core::contracts::{JsonObject, KernelTool, Reach, Render, ToolResult};
use crate::core::errors::RuntimeError;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use serde_json::{json, Value};
use std::rc::Rc;

pub const TOOL: &str = "layers";

/// The longest stretch rendered (quarter notes), and the most tracks: the main and seven layers.
const MOST_BEATS: f64 = 64.;
const MOST_TRACKS: usize = 8;
/// The glue a break bus usually has (dB of peak-to-loudness ratio taken), and how near a pass must land.
const GLUE: f64 = 1.5;
const GLUE_NEAR: f64 = 0.5;
/// How far a threshold first moves for each dB of glue missing (later moves go by what the passes measured), the most one
/// move takes, and how far under the group's peak a threshold starts when it sits above it (dB).
const THRESHOLD_PER_DB: f64 = 2.;
const MOST_MOVE: f64 = 24.;
const UNDER_PEAK: f64 = 6.;
const GLUE_PASSES: usize = 4;

pub struct Layers<B: LayersBackend> {
    backend: B,
    description: String,
}

/// A render as one mono channel, and its rate.
async fn mono_of(render: &Render, signal: &Signal) -> Result<(Vec<f32>, f64), String> {
    let seconds = render.seconds.unwrap_or(f64::MAX / 4.);
    let (mono, rate, _) =
        crate::audio::decode::read_mono(&render.file, signal, render.start, seconds.min(MOST_BEATS * 2.)).await.map_err(|error| error.0)?;
    Ok((mono, rate))
}

fn stretch(input: &JsonObject) -> Result<(f64, f64), String> {
    let number = |key: &str| input.get(key).and_then(Value::as_f64);
    match number("fromBeat").zip(number("beats")) {
        Some((from, beats)) if from >= 0. && beats > 0. && beats <= MOST_BEATS => Ok((from, beats)),
        _ => Err(format!("fromBeat and beats: the stretch the layers play (at most {MOST_BEATS} beats)")),
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

fn one(value: f64) -> f64 {
    (value * 10.).round() / 10.
}

impl<B: LayersBackend + 'static> Layers<B> {
    pub fn new(backend: B) -> Rc<Self> {
        let description = description(backend.words());
        Rc::new(Layers { backend, description })
    }

    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(LayersTool { owner: self.clone() })
    }

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        match input.get("action").and_then(Value::as_str).unwrap_or("look") {
            "look" => self.look(&input, &signal).await,
            "align" => {
                let track = input.get("track").and_then(Value::as_str).ok_or("track: the layer to nudge")?;
                let ms = input
                    .get("ms")
                    .and_then(Value::as_f64)
                    .filter(|ms| ms.abs() <= 100. && *ms != 0.)
                    .ok_or("ms: how far to move it (earlier when negative), at most 100")?;
                let (from, beats) = stretch(&input)?;
                Ok(json!({"made": self.backend.nudge(track, ms, from, beats, &signal).await?, "next": "look again to hear it land"}))
            }
            "invert" => {
                let track = input.get("track").and_then(Value::as_str).ok_or("track: the layer whose polarity flips")?;
                Ok(json!({"made": self.backend.invert(track, &signal).await?, "next": "look again to hear it with the main"}))
            }
            "glue" => self.glue(&input, &signal).await,
            _ => Err("action is look (the default), align, invert or glue".into()),
        }
    }

    /// The tracks rendered over the stretch: each as mono, and the rate.
    async fn heard(&self, tracks: &[String], from: f64, beats: f64, signal: &Signal) -> Result<(Vec<Vec<f32>>, f64), String> {
        let renders = self.backend.render(tracks, from, beats, signal).await?;
        if renders.len() != tracks.len() {
            return Err(format!("{} tracks asked for, {} rendered", tracks.len(), renders.len()));
        }
        let mut monos = Vec::new();
        let mut rate = 48_000.;
        for render in &renders {
            let (mono, found) = mono_of(render, signal).await?;
            rate = found;
            monos.push(mono);
        }
        Ok((monos, rate))
    }

    /// The group's peak-to-loudness ratio and true peak over the stretch.
    async fn group_plr(&self, group: &str, from: f64, beats: f64, signal: &Signal) -> Result<(f64, f64), String> {
        let (monos, rate) = self.heard(&[group.to_owned()], from, beats, signal).await?;
        monos
            .first()
            .and_then(|mono| plr(mono, rate))
            .ok_or_else(|| "the group is too quiet or the stretch too short to measure".to_string())
    }

    async fn look(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let tracks = strings(input.get("tracks"));
        if tracks.len() < 2 || tracks.len() > MOST_TRACKS {
            return Err(format!("tracks: the main break first, then its layers ({MOST_TRACKS} at most)"));
        }
        let (from, beats) = stretch(input)?;
        let (monos, rate) = self.heard(&tracks, from, beats, signal).await?;
        let main = &monos[0];
        let mut rows = Vec::new();
        let mut next = Vec::new();
        for (track, layer) in tracks.iter().zip(&monos).skip(1) {
            let found = compare(main, layer, rate);
            let mut row = json!({"track": track, "paired": found.paired});
            if found.paired == 0 {
                row["says"] = json!("none of its hits lands near the main's: another rhythm, or the stretch misses it");
                rows.push(row);
                continue;
            }
            if let Some(offset) = found.offset_ms {
                let way = if offset > 0. { "late" } else { "early" };
                let strays = if found.strays.is_empty() { String::new() } else { format!(" ({} hits stray from it)", found.strays.len()) };
                row["offset"] =
                    json!(if offset.abs() < 0.5 { format!("in time{strays}") } else { format!("{:.1} ms {way}{strays}", offset.abs()) });
                if offset.abs() >= 0.5 {
                    next.push(format!("align {track} by {:+.1} ms", -offset));
                }
            }
            let flipped = found.inverted * 2 > found.paired;
            row["polarity"] = json!(if flipped {
                format!("inverted against the main on {} of {} hits: they cancel", found.inverted, found.paired)
            } else {
                "with the main".to_string()
            });
            if flipped {
                next.push(format!("invert {track}"));
            }
            if let Some(level) = found.level_db {
                row["level"] = json!(format!("{:+.1} dB against the main", one(level)));
            }
            rows.push(row);
        }
        let mut reply = json!({"layers": rows});
        if let Some(group) = input.get("group").and_then(Value::as_str) {
            reply["group"] = json!(match self.backend.compressor(group, false, signal).await? {
                Some(found) => format!(
                    "{} at {:.1} dB{}: glue measures what it takes",
                    found.name,
                    found.threshold,
                    if found.active { "" } else { " (bypassed)" }
                ),
                None => format!("no compressor: glue adds {} and sets it", self.backend.words().compressor),
            });
            next.push("glue the group".into());
        }
        reply["next"] = json!(if next.is_empty() {
            "the layers sit well: balance them by ear if the producer asks".to_string()
        } else {
            format!("propose, and do what the producer accepts: {}; balance by ear with them", next.join("; "))
        });
        Ok(reply)
    }

    async fn glue(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let group = input.get("group").and_then(Value::as_str).ok_or("group: the group the layers play in")?;
        let (from, beats) = stretch(input)?;
        let target = input.get("target").and_then(Value::as_f64).unwrap_or(GLUE).clamp(0.5, 6.);
        let mut made = Vec::new();
        // (the group heard without its compressor first: before one is added, or with it bypassed)
        let (reference, peak, mut compressor) = match self.backend.compressor(group, false, signal).await? {
            None => {
                let (reference, peak) = self.group_plr(group, from, beats, signal).await?;
                let added = self.backend.compressor(group, true, signal).await?.ok_or("the compressor wasn't added")?;
                made.push(json!(format!("added {}", added.name)));
                (reference, peak, added)
            }
            Some(found) if found.active => {
                made.push(self.backend.bypass(&found, true, signal).await?);
                let heard = self.group_plr(group, from, beats, signal).await;
                match self.backend.bypass(&found, false, signal).await {
                    Ok(on) => made.push(on),
                    Err(why) => return Err(format!("{why}: {} is still bypassed", found.name)),
                }
                let (reference, peak) = heard?;
                (reference, peak, found)
            }
            Some(found) => {
                let (reference, peak) = self.group_plr(group, from, beats, signal).await?;
                made.push(self.backend.bypass(&found, false, signal).await?);
                (reference, peak, Compressor { active: true, ..found })
            }
        };
        let mut passes = Vec::new();
        let mut before: Option<(f64, f64)> = None;
        let mut said = None;
        let mut glued = 0.;
        for pass in 0..=GLUE_PASSES {
            glued = reference - self.group_plr(group, from, beats, signal).await?.0;
            passes.push(json!({"threshold": one(compressor.threshold), "glue": one(glued)}));
            if (glued - target).abs() <= GLUE_NEAR || pass == GLUE_PASSES {
                break;
            }
            // (the next threshold: by how the glue moved over the last move, else a first guess; from under the peak)
            let slope = before.map(|(threshold, glue)| (glued - glue) / (compressor.threshold - threshold));
            if slope.is_some_and(|slope| slope.is_nan() || slope >= -0.02) {
                said = Some("the glue doesn't grow as the threshold falls (an attack that lets the hits through, or too gentle a ratio): set it by ear");
                break;
            }
            let step = slope.map_or(-(target - glued) * THRESHOLD_PER_DB, |slope| (target - glued) / slope);
            let mut threshold = (compressor.threshold + step.clamp(-MOST_MOVE, MOST_MOVE)).clamp(compressor.lowest, compressor.highest);
            if before.is_none() && glued < target && compressor.threshold > peak - UNDER_PEAK / 2. {
                threshold = threshold.min(peak - UNDER_PEAK).max(compressor.lowest);
            }
            if (threshold - compressor.threshold).abs() < 0.05 {
                said = Some(if glued < target {
                    "the threshold is at its lowest: raise the compressor's ratio for more"
                } else {
                    "the threshold is at its highest: lower the compressor's ratio for less"
                });
                break;
            }
            made.push(self.backend.set_threshold(&compressor, threshold, signal).await?);
            before = Some((compressor.threshold, glued));
            compressor.threshold = threshold;
        }
        let ok = (glued - target).abs() <= GLUE_NEAR;
        let mut reply = json!({
            "glue": format!("{} takes {:.1} dB of the group's peak-to-loudness ratio (aimed at {:.1})", compressor.name, glued, target),
            "ok": ok,
            "without": format!("{:.1} dB of peak-to-loudness ratio without it", reference),
            "threshold": format!("{:.1} dB", compressor.threshold),
            "passes": passes,
            "made": made,
        });
        if let Some(said) = said {
            reply["said"] = json!(said);
        }
        Ok(reply)
    }
}

fn description(words: &super::backend::Words) -> String {
    format!(
        "Layer breaks in {app}, hit by hit. action look (the default) renders the main break and its layers (tracks, the main first; 8 at most) over fromBeat and beats (at most 64), each as heard alone: each layer's hits paired with the main's, how early or late it lands (ms, and the hits that stray), whether its polarity fights the main's, and how loud it sits; with group, the group's compressor. align nudges a layer by ms ({nudge}); invert flips its polarity ({invert}); glue measures how much peak-to-loudness ratio the group's compressor ({compressor}) takes, the group heard without it first, and moves its threshold until that's target (1.5 dB by default). Propose what look finds; do what the producer accepts; balance by ear with them.",
        app = words.app,
        nudge = words.nudge,
        invert = words.invert,
        compressor = words.compressor
    )
}

fn schema() -> JsonObject {
    let value = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "action": {"type": "string", "enum": ["look", "align", "invert", "glue"]},
            "tracks": {"type": "array", "minItems": 2, "maxItems": MOST_TRACKS, "items": {"type": "string"}, "description": "The main break first, then its layers"},
            "group": {"type": "string", "description": "The group the layers play in"},
            "track": {"type": "string", "description": "The layer to align or invert"},
            "ms": {"type": "number", "minimum": -100, "maximum": 100},
            "fromBeat": {"type": "number", "minimum": 0},
            "beats": {"type": "number", "exclusiveMinimum": 0, "maximum": MOST_BEATS},
            "target": {"type": "number", "minimum": 0.5, "maximum": 6, "description": "The glue to aim at, dB of peak-to-loudness ratio taken"}
        }
    });
    value.as_object().cloned().unwrap_or_default()
}

struct LayersTool<B: LayersBackend> {
    owner: Rc<Layers<B>>,
}

#[async_trait(?Send)]
impl<B: LayersBackend + 'static> KernelTool for LayersTool<B> {
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
