//! Renoise's backend for layering breaks (#401): only Renoise's facts. A track renders alone by Renoise's solo, as the
//! producer hears it soloed (through its group's devices and the master's); a layer is nudged by its track delay (-100 to 100
//! ms, the whole track: a sequencer track's only); its polarity flips in its samples, since Renoise has no polarity switch on a
//! track or device: every sample with audio of the instruments it plays (a slice follows its sample), one sample_invert each,
//! refused when another track plays one of them; the glue's compressor is a Bus Compressor on the group, added at 2:1 when it
//! has none, bypassed by device_bypass and its threshold (dB) set by device_param; the master's dynamics are its active
//! Maximizer, Compressor, Bus Compressor or Gate 2, or a plug-in named as a limiter, maximizer, compressor or clipper. Core's
//! tool does the rest.
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_listening::renderer::Window;
use kumi_runtime::core::contracts::Render;
use kumi_runtime::core::layers::backend::{Compressor, Dynamics, LayersBackend, Words};
use kumi_runtime::core::refs::RefScheme;
use kumi_runtime::integrations::own_changes::OwnPlan;
use kumi_runtime::integrations::own_listening::OwnRenderer;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::rc::Rc;

pub static WORDS: Words = Words {
    app: "Renoise",
    nudge: "the track's delay, -100 to 100 ms, which moves the whole track",
    invert: "the samples of the instruments it plays, flipped, when no other track plays them",
    compressor: "a Bus Compressor at 2:1 on the group",
};

/// Pages of rows read (200 a page), and of the note count by track (each about a second of Renoise's time).
const PAGES: usize = 10;
const COUNT_PAGES: usize = 60;
/// Renoise's Bus Compressor, and its Ratio's value that reads 2:1 (3.5.4, the lab's inventory of native devices).
const BUS_COMPRESSOR: &str = "Audio/Effects/Native/Bus Compressor";
const RATIO_2_TO_1: f64 = 0.525;
/// Renoise's own dynamics devices, and the words a plug-in's name holds when it's one (lowercase).
const NATIVE_DYNAMICS: [&str; 4] = ["Maximizer", "Compressor", "Bus Compressor", "Gate 2"];
const DYNAMICS_WORDS: [&str; 5] = ["limit", "maxim", "compress", "clip", "loud"];

/// Which tracks play which instruments: each track's number (from 1) and the instruments its notes name (from 0).
type Players = BTreeMap<String, Vec<u64>>;

pub struct RenoiseLayers<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
    renderer: Rc<OwnRenderer<S>>,
}

fn quoted(text: &str) -> String {
    format!("\u{201c}{text}\u{201d}")
}

/// Whether a device row is a dynamics device that's on: a limiter, maximizer, compressor or gate.
fn dynamics(row: &Value) -> bool {
    let path = row.get("path").and_then(Value::as_str).unwrap_or("");
    let name = row.get("name").and_then(Value::as_str).unwrap_or("").to_lowercase();
    let native = path.strip_prefix("Audio/Effects/Native/").is_some_and(|device| NATIVE_DYNAMICS.contains(&device));
    let plugin = row.get("plugin") == Some(&Value::Bool(true)) && DYNAMICS_WORDS.iter().any(|word| name.contains(word));
    row.get("active") == Some(&Value::Bool(true)) && (native || plugin)
}

fn name_of(row: &Value) -> String {
    quoted(row.get("name").and_then(Value::as_str).unwrap_or(""))
}

impl<S: RefScheme + 'static> RenoiseLayers<S> {
    pub fn new(plan: Rc<OwnPlan<S>>, renderer: Rc<OwnRenderer<S>>) -> Self {
        RenoiseLayers { plan, renderer }
    }

    /// One change by its tool: what Renoise answered, or why not.
    async fn change(&self, tool: &str, input: Value, signal: &Signal) -> Result<Value, String> {
        let outcome = self.plan.change_by(tool, input.as_object().cloned().unwrap_or_default(), signal.clone()).await;
        let answer: Value = serde_json::from_str(&outcome.text).unwrap_or_else(|_| json!({"said": outcome.text}));
        if outcome.is_error || outcome.stops {
            return Err(answer.get("changed").and_then(Value::as_str).map(str::to_owned).unwrap_or(outcome.text));
        }
        Ok(answer)
    }

    async fn rows(&self, args: Value, signal: &Signal) -> Result<Vec<Value>, String> {
        self.plan.rows_through(args, PAGES, signal).await
    }

    /// The track a ref names: its long ref and its row.
    async fn track(&self, reference: &str, signal: &Signal) -> Result<(String, Value), String> {
        let long = self.plan.lengthen(reference)?;
        if !long.contains(":track:") {
            return Err(format!("{reference} isn't a track"));
        }
        let answer = self.plan.read_through(json!({"ref": long}), None, signal).await?;
        let row = answer.pointer("/items/0").cloned().ok_or_else(|| format!("{reference} isn't in the song any more: read again"))?;
        Ok((long, row))
    }

    /// Which tracks play which instruments, and whether every pattern was counted.
    async fn players(&self, signal: &Signal) -> Result<(Players, bool), String> {
        let mut players = Players::new();
        let mut cursor: Option<String> = None;
        for _ in 0..COUNT_PAGES {
            let mut asked = json!({"kind": "tidy-notes", "byTrack": true});
            if let Some(cursor) = &cursor {
                asked["cursor"] = cursor.clone().into();
            }
            let page = self.plan.read_through(asked, None, signal).await?;
            for (track, counts) in page.get("tracks").and_then(Value::as_object).into_iter().flatten() {
                let mine = players.entry(track.clone()).or_default();
                for instrument in counts.as_object().into_iter().flatten().filter_map(|(key, _)| key.parse::<u64>().ok()) {
                    if !mine.contains(&instrument) {
                        mine.push(instrument);
                    }
                }
            }
            match page.get("nextCursor").and_then(Value::as_str) {
                Some(next) => cursor = Some(next.to_owned()),
                None => return Ok((players, true)),
            }
        }
        Ok((players, false))
    }

    /// The group's Bus Compressor row, if it has one (the last).
    async fn bus_compressor(&self, group: &str, signal: &Signal) -> Result<Option<Value>, String> {
        let devices = self.rows(json!({"kind": "device", "parent": group, "limit": 200}), signal).await?;
        Ok(devices.into_iter().rev().find(|device| device.get("path").and_then(Value::as_str) == Some(BUS_COMPRESSOR)))
    }
}

/// Why a layer's samples can't be flipped, in words with the way out, or None: an instrument another track plays too (with
/// those tracks' names), or one that plays no samples.
fn unflippable(track: &str, mine: &[u64], players: &Players, instruments: &[Value], names: &BTreeMap<String, String>) -> Option<String> {
    for instrument in mine {
        let row = instruments.iter().find(|row| row.get("index").and_then(Value::as_u64) == Some(*instrument))?;
        let others: Vec<&str> = players
            .iter()
            .filter(|(number, theirs)| number.as_str() != track && theirs.contains(instrument))
            .map(|(number, _)| names.get(number).map_or("another track", String::as_str))
            .collect();
        if !others.is_empty() {
            return Some(format!(
                "{} plays on {} too: flipping its samples would flip them there as well. Copy the instrument for the layer (instrument_duplicate), point the layer's notes at the copy, then flip it",
                name_of(row),
                others.join(", ")
            ));
        }
        if row.get("kind").and_then(Value::as_str) != Some("sample") {
            return Some(format!(
                "{} plays through a plug-in or MIDI, not samples: Renoise can't flip its polarity; flip it in the plug-in, or on a polarity plug-in on the track",
                name_of(row)
            ));
        }
    }
    None
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> LayersBackend for RenoiseLayers<S> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn render(&self, tracks: &[String], from: f64, beats: f64, signal: &Signal) -> Result<Vec<Render>, String> {
        if self.renderer.listening_now() {
            return Err("Kumi is listening to something else just now".into());
        }
        if let Some(why) = self.renderer.why_not() {
            return Err(why);
        }
        let mut sources = Vec::new();
        for (k, track) in tracks.iter().enumerate() {
            sources.push(json!({"id": format!("t{k}"), "what": "track", "ref": self.plan.lengthen(track)?, "stage": "post-fader"}));
        }
        let _listening = self.renderer.begin_listen();
        let rendered = self.renderer.render_sources(sources, Window { from, beats }, signal).await;
        self.renderer.put_tempo_back().await;
        let files = match rendered {
            Ok(Ok((files, _))) => files,
            Ok(Err(why)) => return Err(why),
            Err(error) => return Err(error.to_string()),
        };
        tracks
            .iter()
            .enumerate()
            .map(|(k, track)| {
                files.get(&format!("t{k}")).map(|(render, _)| render.clone()).ok_or_else(|| format!("{track}'s render made no file"))
            })
            .collect()
    }

    async fn nudge(&self, track: &str, ms: f64, _from: f64, _beats: f64, signal: &Signal) -> Result<Value, String> {
        let (long, row) = self.track(track, signal).await?;
        let kind = row.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "sequencer" {
            return Err(format!("{} is a {kind} track: Renoise has a delay on a sequencer track only; nudge its members", name_of(&row)));
        }
        let now = row.get("outputDelay").and_then(Value::as_f64).unwrap_or(0.);
        let to = ((now + ms) * 100.).round() / 100.;
        if to.abs() > 100. {
            return Err(format!(
                "{}'s delay is {now:+.1} ms already: {ms:+.1} more is past Renoise's 100 ms; move the main break the other way instead",
                name_of(&row)
            ));
        }
        self.change("track_delay", json!({"ref": long, "delayMs": to}), signal).await
    }

    async fn invert(&self, track: &str, signal: &Signal) -> Result<Value, String> {
        let (_, row) = self.track(track, signal).await?;
        let number = row.get("index").and_then(Value::as_u64).ok_or("the track's number wasn't read")?.to_string();
        let (players, whole) = self.players(signal).await?;
        if !whole {
            return Err("the song is too big to tell which tracks play an instrument, so Kumi won't flip one: flip the layer's samples in Renoise's sample editor".into());
        }
        let mine = players.get(&number).cloned().unwrap_or_default();
        if mine.is_empty() {
            return Err(format!("{} plays no notes: nothing to flip", name_of(&row)));
        }
        let instruments = self.rows(json!({"kind": "instrument", "limit": 200}), signal).await?;
        let tracks = self.rows(json!({"kind": "track", "limit": 200}), signal).await?;
        let names: BTreeMap<String, String> =
            tracks.iter().filter_map(|row| Some((row.get("index").and_then(Value::as_u64)?.to_string(), name_of(row)))).collect();
        if let Some(why) = unflippable(&number, &mine, &players, &instruments, &names) {
            return Err(why);
        }
        let mut flipped = Vec::new();
        let mut made = Vec::new();
        for instrument in &mine {
            let Some(reference) = instruments
                .iter()
                .find(|row| row.get("index").and_then(Value::as_u64) == Some(*instrument))
                .and_then(|row| row.get("ref").and_then(Value::as_str))
            else {
                continue;
            };
            let samples = self.rows(json!({"kind": "sample", "parent": reference, "limit": 200}), signal).await?;
            // (a slice plays its sample's audio: the sample flips it)
            for sample in samples
                .iter()
                .filter(|sample| sample.get("slice").is_none() && sample.get("frames").and_then(Value::as_u64).unwrap_or(0) > 0)
            {
                let reference = sample.get("ref").and_then(Value::as_str).ok_or("a sample without a ref")?;
                match self.change("sample_invert", json!({"ref": reference}), signal).await {
                    Ok(answer) => {
                        made.push(answer);
                        flipped.push(name_of(sample));
                    }
                    Err(why) if flipped.is_empty() => return Err(why),
                    Err(why) => {
                        return Err(format!("{why} (flipped already: {}; undo_change flips each back)", flipped.join(", ")));
                    }
                }
            }
        }
        if flipped.is_empty() {
            return Err(format!("the instruments {} plays have no audio to flip", name_of(&row)));
        }
        Ok(json!({"flipped": flipped, "made": made}))
    }

    async fn compressor(&self, group: &str, add: bool, signal: &Signal) -> Result<Option<Compressor>, String> {
        let (long, row) = self.track(group, signal).await?;
        if row.get("type").and_then(Value::as_str) != Some("group") {
            return Err(format!("{} isn't a group: glue works on the group the layers play in", name_of(&row)));
        }
        let device = match self.bus_compressor(&long, signal).await? {
            Some(device) => device,
            None if add => {
                self.change("device_add", json!({"ref": long, "path": BUS_COMPRESSOR}), signal).await?;
                let device = self.bus_compressor(&long, signal).await?.ok_or("Renoise didn't add the Bus Compressor")?;
                let reference = device.get("ref").and_then(Value::as_str).ok_or("the Bus Compressor has no ref")?;
                self.change("device_param", json!({"ref": reference, "param": "Ratio", "value": RATIO_2_TO_1}), signal).await?;
                device
            }
            None => return Ok(None),
        };
        let reference = device.get("ref").and_then(Value::as_str).ok_or("the Bus Compressor has no ref")?.to_owned();
        let params = self.rows(json!({"kind": "param", "parent": reference, "limit": 200}), signal).await?;
        let threshold = params
            .iter()
            .find(|param| param.get("name").and_then(Value::as_str) == Some("Threshold"))
            .ok_or("the Bus Compressor's threshold wasn't read")?;
        let number = |key: &str| threshold.get(key).and_then(Value::as_f64);
        let name = device.get("displayName").and_then(Value::as_str).filter(|name| !name.is_empty()).unwrap_or("Bus Compressor");
        Ok(Some(Compressor {
            reference,
            name: name.to_owned(),
            threshold: number("value").ok_or("the Bus Compressor's threshold wasn't read")?,
            lowest: number("min").unwrap_or(-60.),
            highest: number("max").unwrap_or(0.),
            active: device.get("active").and_then(Value::as_bool).unwrap_or(true),
        }))
    }

    async fn set_threshold(&self, compressor: &Compressor, db: f64, signal: &Signal) -> Result<Value, String> {
        let value = ((db.clamp(compressor.lowest, compressor.highest)) * 10.).round() / 10.;
        self.change("device_param", json!({"ref": compressor.reference, "param": "Threshold", "value": value}), signal).await
    }

    async fn master_dynamics(&self, signal: &Signal) -> Result<Vec<Dynamics>, String> {
        let tracks = self.rows(json!({"kind": "track", "limit": 200}), signal).await?;
        let master = tracks
            .iter()
            .find(|row| row.get("type").and_then(Value::as_str) == Some("master"))
            .and_then(|row| row.get("ref").and_then(Value::as_str))
            .ok_or("the master's track row wasn't found")?;
        let devices = self.rows(json!({"kind": "device", "parent": master, "limit": 200}), signal).await?;
        Ok(devices
            .iter()
            .filter(|row| dynamics(row))
            .filter_map(|row| {
                let name = row
                    .get("displayName")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .or_else(|| row.get("name").and_then(Value::as_str))?;
                Some(Dynamics { reference: row.get("ref").and_then(Value::as_str)?.to_owned(), name: name.to_owned() })
            })
            .collect())
    }

    async fn bypass(&self, reference: &str, bypass: bool, signal: &Signal) -> Result<Value, String> {
        self.change("device_bypass", json!({"ref": reference, "bypass": bypass}), signal).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layer_whose_instrument_plays_elsewhere_or_through_a_plugin_isnt_flipped() {
        let players: Players = [("2".into(), vec![0, 3]), ("3".into(), vec![1]), ("5".into(), vec![3])].into_iter().collect();
        let names: BTreeMap<String, String> =
            [("2".into(), quoted("Main")), ("3".into(), quoted("Layer")), ("5".into(), quoted("Fill"))].into_iter().collect();
        let instruments = vec![
            json!({"index": 0, "name": "Kit", "kind": "sample"}),
            json!({"index": 1, "name": "Clap", "kind": "sample"}),
            json!({"index": 2, "name": "Synth", "kind": "plugin"}),
            json!({"index": 3, "name": "Kick", "kind": "sample"}),
        ];
        assert_eq!(unflippable("3", &[1], &players, &instruments, &names), None, "only the layer plays Clap");
        let why = unflippable("2", &[0, 3], &players, &instruments, &names).unwrap();
        assert!(why.starts_with("\u{201c}Kick\u{201d} plays on \u{201c}Fill\u{201d} too"), "{why}");
        assert!(why.contains("instrument_duplicate"), "{why}");
        let why = unflippable("3", &[2], &players, &instruments, &names).unwrap();
        assert!(why.starts_with("\u{201c}Synth\u{201d} plays through a plug-in"), "{why}");
    }

    #[test]
    fn a_masters_limiter_maximizer_or_compressor_that_is_on_is_dynamics_and_its_eq_isnt() {
        let row =
            |path: &str, name: &str, plugin: bool, active: bool| json!({"path": path, "name": name, "plugin": plugin, "active": active});
        assert!(dynamics(&row("Audio/Effects/Native/Maximizer", "Maximizer", false, true)));
        assert!(dynamics(&row("Audio/Effects/Native/Bus Compressor", "Bus Compressor", false, true)));
        assert!(!dynamics(&row("Audio/Effects/Native/Maximizer", "Maximizer", false, false)), "bypassed");
        assert!(!dynamics(&row("Audio/Effects/Native/EQ 10", "EQ 10", false, true)));
        assert!(dynamics(&row("Audio/Effects/AU/aufx:lmtr:Fake", "Pro Limiter", true, true)));
        assert!(!dynamics(&row("Audio/Effects/AU/aufx:rvb2:Fake", "Big Room", true, true)));
    }

    #[test]
    fn every_change_fits_its_renoise_kind() {
        use crate::changes::CHANGES;
        use kumi_runtime::kernel::schema::problem;
        let schema_of = |tool: &str| {
            Value::Object(CHANGES.kinds.iter().find(|kind| kind.tool == tool).unwrap_or_else(|| panic!("{tool}")).input_schema())
        };
        for (tool, input) in [
            ("track_delay", json!({"ref": "1:track:2", "delayMs": -4.25})),
            ("sample_invert", json!({"ref": "1:sample:2.1"})),
            ("device_add", json!({"ref": "1:track:4", "path": BUS_COMPRESSOR})),
            ("device_param", json!({"ref": "1:device:4.2", "param": "Ratio", "value": RATIO_2_TO_1})),
            ("device_param", json!({"ref": "1:device:4.2", "param": "Threshold", "value": -18.5})),
            ("device_bypass", json!({"ref": "1:device:4.2", "bypass": true})),
        ] {
            assert_eq!(problem(&schema_of(tool), &input), None, "{tool} {input}");
        }
    }
}
