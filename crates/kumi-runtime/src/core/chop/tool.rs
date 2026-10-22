//! The chop tool, the same in every DAW: plan (the hits, their kinds and the break's tempo), cut (at the hits of a density, in
//! one change), lay (the cuts written to play the recording as recorded, rendered and checked against it, the lighter densities
//! offered as variations takes). The DAW's backend reads, cuts, lays out and renders.
use super::backend::{ChopBackend, Key, Laid, Recording, Song, Step};
use super::hits::{hits, Hit};
use super::{beat_of, break_tempo, hit_words, on_grid, rolls, same_tempo, Density, Tempo};
use crate::audio::decode::read_mono;
use crate::core::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use crate::core::errors::RuntimeError;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use serde_json::{json, Value};
use std::rc::Rc;

pub const TOOL: &str = "chop";

/// The longest recording Kumi chops (seconds).
const MOST_SECONDS: f64 = 120.;
/// The most cuts one change makes (a DAW's own limit may be lower: the backend says).
pub const MOST_CUTS: usize = 64;
/// A cut and a hit this close are one (seconds).
const SAME_HIT: f64 = 0.005;
/// A hit of the render this close to where one should be is it (seconds).
const MATCH: f64 = 0.03;

pub struct Chop<B: ChopBackend> {
    backend: B,
    variations: Option<Rc<dyn KernelTool>>,
    description: String,
    schema: JsonObject,
}

/// A recording heard: its hits, which are a roll's, and the break's tempo.
struct Heard {
    mono: Vec<f32>,
    hits: Vec<Hit>,
    rolls: Vec<bool>,
    tempo: Tempo,
}

impl<B: ChopBackend + 'static> Chop<B> {
    /// The tool over the DAW's backend; `variations`, where the DAW has it, takes the lighter densities as takes.
    pub fn new(backend: B, variations: Option<Rc<dyn KernelTool>>) -> Rc<Self> {
        let words = backend.words();
        Rc::new(Chop { description: description(words, variations.is_some()), schema: schema(words), backend, variations })
    }

    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(ChopTool { owner: self.clone() })
    }

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        let reference = input.get("ref").and_then(Value::as_str).ok_or("ref: the recording to chop")?;
        let recording = self.backend.recording(reference, &signal).await?;
        let song = self.backend.song(&signal).await?;
        let heard = self.hear(&recording, &song, &input, &signal).await?;
        match input.get("action").and_then(Value::as_str).unwrap_or("plan") {
            "plan" => Ok(self.plan(&recording, &heard)),
            "cut" => self.cut(&recording, &song, &heard, &input, &signal).await,
            "lay" => self.lay(&recording, &song, &heard, &input, &signal).await,
            _ => Err("action is plan (the default), cut or lay".into()),
        }
    }

    async fn hear(&self, recording: &Recording, song: &Song, input: &JsonObject, signal: &Signal) -> Result<Heard, String> {
        let (mono, rate, seconds) = read_mono(&recording.file, signal, 0., MOST_SECONDS).await.map_err(|e| e.0)?;
        if seconds > MOST_SECONDS {
            return Err(format!("that recording is {:.0} seconds: Kumi chops at most {MOST_SECONDS:.0}", seconds));
        }
        let bars = input.get("bars").and_then(Value::as_u64).map(|bars| bars as u32);
        let bpm = input.get("bpm").and_then(Value::as_f64);
        let tempo = break_tempo(seconds, song.beats_per_bar, song.bpm, bars, bpm);
        let (found, mono) =
            tokio::task::spawn_blocking(move || (hits(&mono, rate), mono)).await.map_err(|_| "Kumi couldn't hear it".to_string())?;
        let rolls = rolls(&found, tempo.bpm);
        Ok(Heard { mono, hits: found, rolls, tempo })
    }

    fn plan(&self, recording: &Recording, heard: &Heard) -> Value {
        let lines: Vec<String> = heard.hits.iter().zip(&heard.rolls).map(|(hit, roll)| hit_words(hit, *roll)).collect();
        let counts: serde_json::Map<String, Value> = Density::ALL
            .iter()
            .map(|density| {
                let n = heard.hits.iter().zip(&heard.rolls).filter(|(hit, roll)| density.keeps(hit, **roll)).count();
                (density.name().to_string(), json!(format!("{n}: {}", density.words())))
            })
            .collect();
        let mut reply = json!({
            "recording": recording.name,
            "tempo": tempo_words(&heard.tempo),
            "hits": lines,
            "densities": counts,
        });
        if !recording.cuts.is_empty() {
            reply["alreadyCut"] =
                json!(format!("it has {} {}s now: cut with replace: true cuts it again", recording.cuts.len(), self.backend.words().cut));
        }
        reply["next"] = json!("action cut makes the cuts (density every by default), then lay writes them to play it");
        reply
    }

    async fn cut(&self, recording: &Recording, song: &Song, heard: &Heard, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let cut = self.backend.words().cut;
        let replace = input.get("replace").and_then(Value::as_bool).unwrap_or(false);
        if !recording.cuts.is_empty() && !replace {
            return Err(format!(
                "{} has {} {cut}s already: replace: true cuts it again at its hits",
                quoted(&recording.name),
                recording.cuts.len()
            ));
        }
        let density = density_of(input)?;
        let chosen: Vec<&Hit> =
            heard.hits.iter().zip(&heard.rolls).filter(|(hit, roll)| density.keeps(hit, **roll)).map(|(hit, _)| hit).collect();
        if chosen.is_empty() {
            return Err(format!("no hits of that density ({}) were found", density.words()));
        }
        // (on the song's grid when the break plays at its tempo: laid out in order, the cuts are the recording again)
        let unit = song.grid.filter(|_| same_tempo(heard.tempo.bpm, song.bpm));
        let mut frames: Vec<usize> =
            chosen.iter().map(|hit| unit.map_or(hit.frame, |unit| on_grid(hit.frame, recording.rate, song.bpm, unit))).collect();
        frames.dedup();
        // (a cut again keeps the cuts that stay where they are: only those that move are changes)
        let added = frames.iter().filter(|frame| !recording.cuts.contains(frame)).count();
        let removed = if replace { recording.cuts.iter().filter(|frame| !frames.contains(frame)).count() } else { 0 };
        if added + removed == 0 {
            return Ok(
                json!({"made": Value::Null, "said": format!("{} is cut at these hits already", quoted(&recording.name)), "cuts": frames.len()}),
            );
        }
        if added + removed > MOST_CUTS {
            return Err(format!(
                "that is {} {cut} changes, more than one change makes ({MOST_CUTS}): density main cuts {}",
                added + removed,
                heard.hits.iter().zip(&heard.rolls).filter(|(hit, roll)| Density::Main.keeps(hit, **roll)).count()
            ));
        }
        let answer = self.backend.cut(recording, &frames, replace, signal).await?;
        let keys = self.backend.keys(recording, signal).await.unwrap_or_default();
        let mut reply = json!({"made": answer, "density": density.words(), "cuts": frames.len()});
        if !keys.is_empty() {
            reply["keys"] = json!(self.key_lines(&keys, heard, recording.rate));
        }
        if unit.is_some() {
            reply["grid"] = json!("each cut sits where a note can start exactly, so laid out in order it plays the recording again");
        }
        reply["next"] = json!("action lay writes them to play it as recorded (or write notes on these keys to play them another way)");
        Ok(reply)
    }

    /// Each cut's key and what it starts with: "C#4: 0.000 s kick".
    fn key_lines(&self, keys: &[Key], heard: &Heard, rate: f64) -> Vec<String> {
        keys.iter()
            .map(|key| {
                let seconds = key.frame as f64 / rate;
                match nearest(heard, seconds) {
                    Some(k) => format!("{}: {}", key.note, hit_words(&heard.hits[k], heard.rolls[k])),
                    None => format!("{}: {seconds:.3} s", key.note),
                }
            })
            .collect()
    }

    async fn lay(&self, recording: &Recording, song: &Song, heard: &Heard, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let cut = self.backend.words().cut;
        let keys = self.backend.keys(recording, signal).await?;
        if keys.is_empty() {
            return Err(format!("{} has no {cut}s yet: cut it first (action cut)", quoted(&recording.name)));
        }
        let at = input.get("at").cloned().ok_or("at: where to lay the cuts out")?;
        let tempo = &heard.tempo;
        let steps: Vec<Step> =
            keys.iter().map(|key| Step { note: key.note.clone(), beat: beat_of(key.frame as f64 / recording.rate, tempo.bpm) }).collect();
        let beats = beat_of(recording.frames as f64 / recording.rate, tempo.bpm);
        let replace = input.get("replace").and_then(Value::as_bool).unwrap_or(false);
        let laid = self.backend.lay(recording, &at, &steps, beats, replace, signal).await?;
        let mut reply = json!({"made": laid.answer, "at": laid.words, "tempo": tempo_words(tempo)});
        if !same_tempo(tempo.bpm, song.bpm) {
            reply["note"] = json!(format!(
                "the song plays at {:.1} BPM, the break was played at {:.1}: each {cut} starts on its beat at the song's tempo, so {}",
                song.bpm,
                tempo.bpm,
                if song.bpm > tempo.bpm { "each is cut short by the next" } else { "gaps open between them" }
            ));
        }
        reply["heard"] = json!(self.check(recording, song, heard, &keys, &laid, signal).await);
        // The lighter densities as takes, each its own change, kept or dropped by number.
        if input.get("takes").and_then(Value::as_bool).unwrap_or(true) {
            if let Some(variations) = &self.variations {
                if let Some(takes) = self.takes(heard, &keys, &steps, &laid, recording.rate) {
                    let asked = json!({"action": "make", "source": laid.range, "takes": takes});
                    let done = variations.execute(asked.as_object().cloned().unwrap_or_default(), signal.clone()).await;
                    reply["takes"] = match done {
                        Ok(done) if !done.is_error => serde_json::from_str(&done.text).unwrap_or(json!(done.text)),
                        Ok(done) => json!(format!("the lighter densities weren't made as takes: {}", done.text)),
                        Err(error) => json!(format!("the lighter densities weren't made as takes: {error}")),
                    };
                }
            }
        }
        Ok(reply)
    }

    /// The takes of the lighter densities that differ from what was laid and from each other: each one's lines.
    fn takes(&self, heard: &Heard, keys: &[Key], steps: &[Step], laid: &Laid, rate: f64) -> Option<Vec<Value>> {
        let mut out = Vec::new();
        let mut seen: Vec<Vec<usize>> = vec![(0..steps.len()).collect()];
        for density in [Density::Main, Density::KickSnare] {
            let kept: Vec<usize> = keys
                .iter()
                .enumerate()
                .filter(|(_, key)| match nearest(heard, key.frame as f64 / rate) {
                    Some(k) => density.keeps(&heard.hits[k], heard.rolls[k]),
                    None => density == Density::Main,
                })
                .map(|(k, _)| k)
                .collect();
            if kept.is_empty() || seen.contains(&kept) {
                continue;
            }
            seen.push(kept.clone());
            let chosen: Vec<Step> = kept.iter().map(|k| steps[*k].clone()).collect();
            out.push(json!({"direction": density.words(), "lines": self.backend.take_lines(laid, &chosen)}));
        }
        (!out.is_empty()).then_some(out)
    }

    /// The laid-out cuts rendered and heard: how close each hit lands to where the recording has it, and, at the break's own
    /// tempo, how deep the render nulls against the recording.
    async fn check(&self, recording: &Recording, song: &Song, heard: &Heard, keys: &[Key], laid: &Laid, signal: &Signal) -> String {
        if let Some(why) = self.backend.cant_render() {
            return format!("not heard: {why}");
        }
        if song.playing {
            return format!("not heard: a render would stop {} playing", self.backend.words().app);
        }
        let scale = heard.tempo.bpm / song.bpm;
        let expected: Vec<f64> = keys.iter().map(|key| key.frame as f64 / recording.rate * scale).collect();
        let seconds = expected.last().copied().unwrap_or(0.) + 60. / song.bpm + 0.5;
        let (file, start) = match self.backend.render(laid, seconds, signal).await {
            Ok(rendered) => rendered,
            Err(why) => return format!("not heard: {why}"),
        };
        let (render, rate, _) = match read_mono(&file, signal, start, seconds + 1.).await {
            Ok(read) => read,
            Err(error) => return format!("not heard: {}", error.0),
        };
        let source = heard.mono.clone();
        let same = same_tempo(heard.tempo.bpm, song.bpm) && (rate - recording.rate).abs() < 0.5;
        let (found, null) = tokio::task::spawn_blocking(move || {
            let found = hits(&render, rate);
            let null = same.then(|| null_depth(&render, &source));
            (found, null)
        })
        .await
        .unwrap_or_default();
        let mut worst: f64 = 0.;
        let mut missed = 0;
        for at in &expected {
            match found.iter().map(|hit| (hit.seconds - at).abs()).fold(None, |best: Option<f64>, d| Some(best.map_or(d, |b| b.min(d)))) {
                Some(d) if d <= MATCH => worst = worst.max(d),
                _ => missed += 1,
            }
        }
        let mut words = if missed == 0 {
            format!("every hit lands within {:.1} ms of the recording's", worst * 1000.)
        } else {
            format!(
                "{} of {} hits land within {:.1} ms of the recording's; {missed} weren't heard apart",
                expected.len() - missed,
                expected.len(),
                worst * 1000.
            )
        };
        if let Some(Some((db, lag))) = null {
            words.push_str(&format!("; against the recording it nulls to {db:.0} dB"));
            if lag != 0 {
                words.push_str(&format!(" ({lag} frames late)"));
            }
        }
        words
    }
}

/// How deep a render nulls against the recording (dB of what's left, at the best gain and lag within 64 frames), or None when
/// either is silent.
pub fn null_depth(render: &[f32], source: &[f32]) -> Option<(f64, i64)> {
    let n = render.len().min(source.len());
    if n < 1024 {
        return None;
    }
    let mut best: Option<(f64, i64)> = None;
    for lag in -64i64..=64 {
        let pairs = (0..n).filter_map(|k| {
            let j = k as i64 - lag;
            (j >= 0 && (j as usize) < source.len()).then(|| (render[k] as f64, source[j as usize] as f64))
        });
        let (mut rs, mut ss, mut rr) = (0f64, 0f64, 0f64);
        for (r, s) in pairs {
            rs += r * s;
            ss += s * s;
            rr += r * r;
        }
        if ss < 1e-9 || rr < 1e-9 {
            continue;
        }
        let gain = rs / ss;
        let left = (rr - 2. * gain * rs + gain * gain * ss).max(1e-15);
        let db = 10. * (left / rr).log10();
        if best.is_none_or(|(kept, _)| db < kept) {
            best = Some((db, lag));
        }
    }
    best
}

fn nearest(heard: &Heard, seconds: f64) -> Option<usize> {
    heard
        .hits
        .iter()
        .enumerate()
        .map(|(k, hit)| (k, (hit.seconds - seconds).abs()))
        .filter(|(_, d)| *d <= SAME_HIT + 0.002)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(k, _)| k)
}

fn density_of(input: &JsonObject) -> Result<Density, String> {
    match input.get("density").and_then(Value::as_str) {
        None => Ok(Density::Every),
        Some(text) => Density::of(text).ok_or_else(|| "density is every, main (no ghosts or rolls) or kick-snare".to_string()),
    }
}

fn tempo_words(tempo: &Tempo) -> String {
    match tempo.bars {
        Some(bars) => format!("{bars} bar{} at {:.1} BPM ({})", if bars == 1 { "" } else { "s" }, tempo.bpm, tempo.how),
        None => format!("{:.1} BPM ({})", tempo.bpm, tempo.how),
    }
}

fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

fn description(words: &super::backend::Words, takes: bool) -> String {
    let cut = words.cut;
    let takes = if takes {
        " With takes (the default) the lighter densities become variations takes, each its own change, to keep or drop by number."
    } else {
        ""
    };
    format!(
        "Chop a drum recording at its real hits and lay the {cut}s out to play it: ref is {source}. action plan (the default) finds every hit to the frame (a kick, snare or hat; a ghost, far quieter than its kind's loudest; a roll) and the break's own tempo (whole bars from its length, nearest the song's: bars or bpm tell it otherwise): tell the producer and offer the densities: every (ghosts and rolls too), main (no ghosts or rolls), kick-snare. action cut makes a {cut} at each hit of a density in one change, each on the song's grid when the break plays at the song's tempo (replace: true cuts one already cut again); its answer lists each {cut}'s key and what it starts with. action lay writes the {cut}s at {place}, in order as recorded, each where it falls at the song's tempo (never over notes already there unless replace: true, which undo_change puts back), renders it and says how close its hits land to the recording's and how deep it nulls against it.{takes} Play them another way (a re-sequence) only when asked, writing notes on the keys cut lists.",
        source = words.source,
        place = words.place
    )
}

fn schema(words: &super::backend::Words) -> JsonObject {
    let at: Value = serde_json::from_str(words.at_schema).unwrap_or(json!({"type": "object"}));
    let value = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["ref"],
        "properties": {
            "action": {"type": "string", "enum": ["plan", "cut", "lay"]},
            "ref": {"type": "string", "description": words.source},
            "bars": {"type": "integer", "minimum": 1, "maximum": 64, "description": "How many bars the recording is"},
            "bpm": {"type": "number", "minimum": 20, "maximum": 999, "description": "The tempo it was played at"},
            "density": {"type": "string", "enum": ["every", "main", "kick-snare"]},
            "replace": {"type": "boolean"},
            "at": at,
            "takes": {"type": "boolean"}
        }
    });
    value.as_object().cloned().unwrap_or_default()
}

struct ChopTool<B: ChopBackend> {
    owner: Rc<Chop<B>>,
}

#[async_trait(?Send)]
impl<B: ChopBackend + 'static> KernelTool for ChopTool<B> {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        &self.owner.description
    }

    fn input_schema(&self) -> JsonObject {
        self.owner.schema.clone()
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
