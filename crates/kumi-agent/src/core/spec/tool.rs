//! The delivery_spec tool, the same in every DAW: the specs listed; the mix rendered offline and measured against one (each
//! line passing, or missing by how much); and a fix: the master fader moved by the gain the spec needs when the peaks allow,
//! then measured again. Measure, process, measure again: a spec is met only by a check after the last change.
use super::backend::SpecBackend;
use super::levels::{Leveller, Levels};
use super::table::{find, Spec, SPECS};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use kumi_types::contracts::{JsonObject, KernelTool, Reach, Render, ToolResult};
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::rc::Rc;

pub const TOOL: &str = "delivery_spec";

/// Headroom kept under a ceiling (dB): a true peak measured at the ceiling can pass it once encoded.
const MARGIN: f64 = 0.2;
/// The most times a fix moves the fader (a limiter on the master answers a move by less than it was).
const PASSES: usize = 3;

/// What a render measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub integrated: Option<f64>,
    pub true_peak: f64,
    pub range: Option<f64>,
    pub levels: Levels,
    pub seconds: f64,
}

/// A render measured: its loudness and peaks (BS.1770, 8× true peak), and its RMS and noise floor as the file streams by.
pub async fn measure(render: &Render, signal: &Signal) -> Result<Measured, String> {
    let options =
        kumi_listening::measure::MeasureOptions { start: Some(render.start), seconds: render.seconds, signal: Some(signal.clone()) };
    let heard = kumi_listening::measure::measure_file(&render.file, options).await.map_err(|error| error.0)?;
    let measures = heard.measures;
    let seconds = render.seconds.unwrap_or(measures.seconds);
    let mut source = kumi_media::audio::decode::open_audio_to(&render.file, Some(signal.clone()), Some(render.start + seconds + 1.))
        .await
        .map_err(|error| error.0)?;
    let rate = source.sample_rate;
    source.seek(render.start * rate);
    let mut leveller = Leveller::new(rate);
    let wanted = (seconds * rate) as usize;
    let mut done = 0;
    while done < wanted {
        let Some(block) = source.read(65536.min(wanted - done)).await.map_err(|error| error.0)? else { break };
        let left = &block[0];
        leveller.push(left, block.get(1).unwrap_or(left));
        done += left.len();
        tokio::task::yield_now().await;
    }
    let _ = source.close().await;
    Ok(Measured {
        integrated: measures.integrated,
        true_peak: measures.true_peak,
        range: measures.range,
        levels: leveller.finish(),
        seconds,
    })
}

fn one(value: f64) -> f64 {
    (value * 10.).round() / 10.
}

/// Each line of a spec against what was measured: what, measured, wanted, and whether it passes.
pub fn lines(spec: &Spec, measured: &Measured) -> Vec<Value> {
    let mut out = Vec::new();
    let mut line = |what: &str, value: Option<f64>, unit: &str, wanted: String, ok: bool| {
        let shown = value.map_or_else(|| "not measured".to_string(), |value| format!("{} {unit}", one(value)));
        out.push(json!({"what": what, "measured": shown, "wanted": wanted, "ok": ok}));
    };
    if let Some((low, high, _)) = spec.integrated {
        let ok = measured.integrated.is_some_and(|value| (low..=high).contains(&value));
        line("integrated loudness", measured.integrated, "LUFS", format!("{low} to {high} LUFS"), ok);
    }
    if let Some((low, high, _)) = spec.rms {
        let ok = (low..=high).contains(&measured.levels.rms);
        line("RMS", Some(measured.levels.rms), "dB", format!("{low} to {high} dB"), ok);
    }
    line("true peak", Some(measured.true_peak), "dBTP", format!("at most {} dBTP", spec.ceiling), measured.true_peak <= spec.ceiling);
    if let Some(most) = spec.noise_floor {
        let ok = measured.levels.noise_floor.is_some_and(|floor| floor <= most);
        line("noise floor", measured.levels.noise_floor, "dB", format!("at most {most} dB RMS"), ok);
    }
    if let Some(longest) = spec.longest {
        line("length", Some(measured.seconds / 60.), "min", format!("at most {} min", longest / 60.), measured.seconds <= longest);
    }
    out
}

/// The gain the spec's loudness needs (dB), aimed at its level.
fn gain(spec: &Spec, measured: &Measured) -> Option<f64> {
    gains(spec, measured).map(|(aim, _)| aim)
}

/// The gains to the spec's aim and to the inside of its range's nearer edge (dB).
fn gains(spec: &Spec, measured: &Measured) -> Option<(f64, f64)> {
    let (low, high, aim, value) = match (spec.integrated, spec.rms) {
        (Some((low, high, aim)), _) => (low, high, aim, measured.integrated?),
        (_, Some((low, high, aim))) => (low, high, aim, measured.levels.rms),
        _ => return None,
    };
    let edge = if value < low { low + 0.1 } else { high - 0.1 };
    Some((aim - value, edge - value))
}

pub struct DeliverySpec<B: SpecBackend> {
    backend: B,
    description: String,
}

impl<B: SpecBackend + 'static> DeliverySpec<B> {
    pub fn new(backend: B) -> Rc<Self> {
        let description = description(backend.words());
        Rc::new(DeliverySpec { backend, description })
    }

    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(SpecTool { owner: self.clone() })
    }

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        match input.get("action").and_then(Value::as_str).unwrap_or("check") {
            "list" => Ok(json!({"specs": SPECS.iter().map(|spec| json!({"spec": spec.id, "name": spec.name})).collect::<Vec<_>>()})),
            "check" => {
                let spec = spec_of(&input)?;
                let measured = measure(&self.backend.render_mix(stretch_of(&input), &signal).await?, &signal).await?;
                Ok(self.report(spec, &measured))
            }
            "fix" => self.fix(spec_of(&input)?, &input, &signal).await,
            _ => Err("action is check (the default), fix or list".into()),
        }
    }

    fn report(&self, spec: &Spec, measured: &Measured) -> Value {
        let lines = lines(spec, measured);
        let ok = lines.iter().all(|line| line["ok"] == true);
        let mut reply = json!({"spec": spec.name, "ok": ok, "lines": lines});
        if let Some(range) = measured.range {
            reply["loudnessRange"] = json!(format!("{} LU", one(range)));
        }
        if measured.levels.silence > 0. {
            reply["silence"] =
                json!(format!("{} s of digital silence (ACX and many specs ask for room tone instead)", one(measured.levels.silence)));
        }
        if !spec.also.is_empty() {
            reply["also"] = json!(spec.also);
        }
        if let Some(gain) = gain(spec, measured).filter(|gain| !ok && gain.abs() >= 0.1) {
            reply["gain"] = json!(format!("{:+.1} dB", gain));
            reply["next"] = json!("fix sets the master fader by that gain and measures again, if the peaks allow");
        }
        reply
    }

    async fn fix(&self, spec: &Spec, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let words = self.backend.words();
        let stretch = stretch_of(input);
        let mut measured = measure(&self.backend.render_mix(stretch, signal).await?, signal).await?;
        let before = self.report(spec, &measured);
        let mut changes = Vec::new();
        let mut said = Vec::new();
        for _ in 0..PASSES {
            // (the loudness first, to the level the spec aims at; with it in range, a peak past the ceiling comes down)
            let lines = lines(spec, &measured);
            let failing = |what: &str| lines.iter().any(|line| line["what"] == what && line["ok"] == false);
            let step = if failing("integrated loudness") || failing("RMS") {
                let Some((aim, bottom)) = gains(spec, &measured) else { break };
                // (down never hurts the peaks or the floor; up only as far as both leave room, the aim first, else as near it
                // as they allow within the range)
                let peak_room = spec.ceiling - MARGIN - measured.true_peak;
                let floor_room = match (spec.noise_floor, measured.levels.noise_floor) {
                    (Some(most), Some(floor)) => most - MARGIN - floor,
                    _ => f64::INFINITY,
                };
                let room = peak_room.min(floor_room);
                if aim <= 0. || aim <= room {
                    aim
                } else if bottom <= room {
                    let by = if floor_room < peak_room { "noise floor" } else { "true peak" };
                    said.push(format!("aimed lower in the range ({room:+.1} dB, not {aim:+.1}): the {by} leaves no more room"));
                    room
                } else if floor_room < peak_room {
                    said.push(format!(
                        "the noise floor would pass {} dB before the loudness reaches {}'s range: noise reduction comes first (the producer's call, a tool they trust), then fix again",
                        spec.noise_floor.unwrap_or(0.),
                        spec.name
                    ));
                    break;
                } else {
                    said.push(format!(
                        "the fader can raise it {:+.1} dB at most before its true peak passes {} dBTP, and the spec needs {bottom:+.1} at least: limit the master's peaks first ({}), then fix again",
                        peak_room.max(0.),
                        spec.ceiling,
                        words.limiter
                    ));
                    break;
                }
            } else if failing("true peak") {
                spec.ceiling - MARGIN - measured.true_peak
            } else {
                break;
            };
            let now = self.backend.master_db(signal).await?;
            let wanted = now + step;
            if wanted > words.fader_most {
                said.push(format!(
                    "the master fader goes to {:+} dB at most, and this needs {:+.1}: {}",
                    words.fader_most, wanted, words.gainer
                ));
                break;
            }
            let answer = self.backend.set_master_db(wanted, signal).await?;
            changes.push(answer.get("change").cloned().unwrap_or(answer));
            measured = measure(&self.backend.render_mix(stretch, signal).await?, signal).await?;
        }
        if let Some(floor) = measured.levels.noise_floor.filter(|floor| spec.noise_floor.is_some_and(|most| *floor > most)) {
            said.push(format!("the noise floor is {} dB, past what {} allows: taking noise away is the producer's call (a noise reduction they trust), not a fader's", one(floor), spec.name));
        }
        let after = self.report(spec, &measured);
        let mut reply = json!({"spec": spec.name, "before": before, "after": after, "changes": changes});
        if !said.is_empty() {
            reply["said"] = json!(said);
        }
        reply["ok"] = after["ok"].clone();
        Ok(reply)
    }
}

fn spec_of(input: &JsonObject) -> Result<&'static Spec, String> {
    let asked = input.get("spec").and_then(Value::as_str).ok_or("spec: which one (list names them)")?;
    find(asked)
        .ok_or_else(|| format!("{asked} isn't a spec Kumi knows: {}", SPECS.iter().map(|spec| spec.id).collect::<Vec<_>>().join(", ")))
}

fn stretch_of(input: &JsonObject) -> Option<(f64, f64)> {
    let number = |key: &str| input.get(key).and_then(Value::as_f64);
    number("fromBeat").zip(number("beats")).filter(|(from, beats)| *from >= 0. && *beats > 0.)
}

fn description(words: &super::backend::Words) -> String {
    format!(
        "Deliver to a loudness spec: ACX (audiobooks), streaming (Spotify, Apple Music, YouTube, Amazon Music, Tidal, Deezer), podcasts, broadcast (EBU R 128, ATSC A/85, Netflix); action list names them. action check (the default) renders {app}'s whole mix offline (or fromBeat and beats) and measures it against the spec: integrated loudness (BS.1770), true peak, RMS and noise floor (ACX), length; each line passes or says what it measured, with the gain the spec needs. action fix moves the master fader by that gain when the peaks allow, and measures again (a few times, if the master's own processing answers a move by less); when the peaks don't allow, it says so and what limiting needs first. Never say a spec is met without a check after the last change.",
        app = words.app
    )
}

fn schema() -> JsonObject {
    let ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
    let value = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "action": {"type": "string", "enum": ["check", "fix", "list"]},
            "spec": {"type": "string", "description": format!("One of {}", ids.join(", "))},
            "fromBeat": {"type": "number", "minimum": 0},
            "beats": {"type": "number", "exclusiveMinimum": 0}
        }
    });
    value.as_object().cloned().unwrap_or_default()
}

struct SpecTool<B: SpecBackend> {
    owner: Rc<DeliverySpec<B>>,
}

#[async_trait(?Send)]
impl<B: SpecBackend + 'static> KernelTool for SpecTool<B> {
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
