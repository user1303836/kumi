//! Live's side of the judge: Main's chain as masking reads it, the faders and tracks it asks about, the gain stage
//! rebalancing turns and the changes in HISTORY it counts. The judged run itself is `Listening`'s.
use super::super::display::parse_display;
use super::main_chain::{main_chain, MainDevice};
use super::*;
use crate::history::Audible;
use kumi_common::js::{number::to_string, string::head};
use kumi_listening::{checklist::MainState, judging::Placed};

/// Where rebalancing turns the level: a gain knob, and whether a limiter comes after it (then peaks stay put).
#[derive(Debug, Clone)]
pub(super) struct GainStage {
    device: String,
    parameter: String,
    label: String,
    pub(super) limited: bool,
    /// The knob turns the other way: a maximizing Limiter's Threshold, lowered to make it louder.
    inverted: bool,
}

impl LiveRenderer {
    /// Main's chain as masking needs it, read from Live as it is now: what on it moves the mix but not the focus, and
    /// the level its Utilities add. Main's fader isn't in it: the mix is heard before the fader. A chain Kumi can't read
    /// counts as one it can't read masking through.
    pub(super) async fn main_state(self: &Rc<Self>, signal: Signal) -> MainState {
        let read = async {
            let (main, _) = self.main_volume(signal.clone()).await?;
            let rows = self.rows("device", json!({"parent":main,"fields":["name","className","enabled"]}), signal.clone()).await?;
            let mut devices = vec![];
            for row in &rows {
                let text = |key: &str| row.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                let (class, on) = (text("className"), row.get("enabled") != Some(&Value::Bool(false)));
                let name = Some(text("name")).filter(|name| !name.is_empty()).unwrap_or_else(|| class.clone());
                let mut parameters = vec![];
                if on && matches!(class.as_str(), "StereoGain" | "Utility") {
                    let read =
                        self.rows("parameter", json!({"parent":row.get("ref"),"fields":["name","displayValue"]}), signal.clone()).await?;
                    for parameter in &read {
                        let field = |key: &str| parameter.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                        parameters.push((field("name"), field("displayValue")));
                    }
                }
                devices.push(MainDevice { name, class, on, parameters });
            }
            Ok::<_, RuntimeError>(main_chain(&devices))
        };
        read.await.unwrap_or_else(|_| MainState { unfair: vec!["Main's chain, which Kumi couldn't read".into()], gain: 0. })
    }

    /// A track's fader, dB as Live shows it (−inf reads as −120; None when Kumi can't read it).
    pub(super) async fn fader_of(&self, track: &str, signal: Signal) -> Option<f64> {
        let tracks = self.rows("track", json!({"fields":["name","mixer"]}), signal).await.ok()?;
        let row = tracks.iter().find(|row| row.get("name").and_then(Value::as_str) == Some(track))?;
        let shown = row.get("mixer")?.get("volumeDisplay")?.as_str()?;
        if shown.trim_start().starts_with("-inf") {
            return Some(-120.);
        }
        parse_display(shown).filter(|reading| reading.unit == "db" && reading.value.is_finite()).map(|reading| reading.value)
    }

    /// A track's name, from its ref or its name.
    pub(super) async fn track_name(&self, named: &str, signal: Signal) -> Option<String> {
        let long = self.connection().references.borrow().lengthen(&json!({"trackRef":named}));
        let long = long["trackRef"].as_str().unwrap_or(named).to_owned();
        let tracks = self.rows("track", json!({"fields":["name"]}), signal).await.ok()?;
        tracks
            .iter()
            .find(|row| {
                let reference = row.get("ref").and_then(Value::as_str);
                reference == Some(named) || reference == Some(long.as_str()) || row.get("name").and_then(Value::as_str) == Some(named)
            })
            .and_then(|row| row.get("name").and_then(Value::as_str).map(str::to_owned))
    }

    /// Turns the level by `gain` dB where rebalancing goes: the gain of the last Limiter on Main (or the run's track),
    /// else the last Utility's, else a Utility Kumi puts at the end. Found again each time (devices move between
    /// rounds). Says what it turned, and whether a limiter holds the peaks after it.
    pub(super) async fn rebalance(self: &Rc<Self>, gain: f64, scope: Option<&str>, signal: Signal) -> Result<(String, bool), String> {
        let stage = self.gain_stage(scope, signal.clone()).await.map_err(|error| head(&error.to_string(), 200))?;
        let read = self
            .rows("parameter", json!({"parent":stage.device,"fields":["name","displayValue"]}), signal.clone())
            .await
            .map_err(|error| head(&error.to_string(), 200))?;
        let shown = read
            .iter()
            .find(|row| row.get("ref").and_then(Value::as_str) == Some(stage.parameter.as_str()))
            .and_then(|row| row.get("displayValue").and_then(Value::as_str).map(str::to_owned))
            .ok_or("Kumi couldn't read the gain knob")?;
        let now = parse_display(&shown)
            .filter(|reading| reading.unit == "db")
            .map(|reading| reading.value)
            .ok_or("the gain knob doesn't show dB")?;
        let gain = if stage.inverted { -gain } else { gain };
        let wanted = if now.is_finite() { now + gain } else { gain };
        self.step(
            "set_device_parameter",
            json!({"deviceRef":stage.device,"parameterRef":stage.parameter,"value":format!("{} dB", to_string((wanted * 10.).round() / 10.))}),
            signal,
        )
        .await
        .map_err(|error| head(&error.to_string(), 200))?;
        Ok((stage.label, stage.limited))
    }

    /// The run's track (Main without one), by ref.
    pub(super) async fn scope_ref(self: &Rc<Self>, track: Option<&str>, signal: Signal) -> Result<String, RuntimeError> {
        match track {
            Some(track) => {
                let tracks = self.rows("track", json!({"fields":["name"]}), signal).await?;
                tracks
                    .iter()
                    .find(|row| {
                        row.get("ref").and_then(Value::as_str) == Some(track) || row.get("name").and_then(Value::as_str) == Some(track)
                    })
                    .and_then(|row| row.get("ref").and_then(Value::as_str).map(str::to_owned))
                    .ok_or_else(|| observation(format!("{track} isn't a track in this Set now.")))
            }
            None => Ok(self.main_volume(signal).await?.0),
        }
    }

    /// The devices on the run's track (Main without one), in order.
    pub(super) async fn chain_now(self: &Rc<Self>, track: Option<&str>, signal: Signal) -> Result<Vec<Placed>, RuntimeError> {
        let parent = self.scope_ref(track, signal.clone()).await?;
        let rows = self.rows("device", json!({"parent":parent,"fields":["name","className","objectIdentity"]}), signal).await?;
        Ok(rows
            .iter()
            .zip(names(&rows))
            .map(|(row, name)| Placed {
                identity: identity(row),
                name,
                reference: row.get("ref").and_then(Value::as_str).unwrap_or("").to_string(),
            })
            .collect())
    }

    /// How many devices the changes made (loaded or duplicated), as HISTORY records them.
    pub(super) fn created_by(&self, changes: &[(String, String)]) -> usize {
        let entries = self.history.entries.borrow();
        changes
            .iter()
            .filter(|(id, _)| {
                entries.get(id).is_some_and(|entry| {
                    let entry = entry.borrow();
                    entry.record.family == ChangeFamily::Device
                        && (entry.record.title.starts_with("Loaded ") || entry.record.title.starts_with("Duplicated "))
                })
            })
            .count()
    }

    /// Where rebalancing turns the level as the chain stands, without adding anything: the last Limiter that's on, else
    /// a Utility last of those on. None when there's neither (rebalancing then puts a Utility at the end).
    pub(super) async fn find_gain_stage(self: &Rc<Self>, scope: Option<&str>, signal: Signal) -> Result<Option<GainStage>, RuntimeError> {
        let track = self.scope_ref(scope, signal.clone()).await?;
        let rows = self.rows("device", json!({"parent":track,"fields":["name","className","enabled"]}), signal.clone()).await?;
        let Some((device, limited)) = gain_device(&rows) else { return Ok(None) };
        self.gain_knob(&device, limited, signal).await.map(Some)
    }

    async fn gain_stage(self: &Rc<Self>, scope: Option<&str>, signal: Signal) -> Result<GainStage, RuntimeError> {
        if let Some(stage) = self.find_gain_stage(scope, signal.clone()).await? {
            return Ok(stage);
        }
        let track = self.scope_ref(scope, signal.clone()).await?;
        self.step("load_device", json!({"itemId":"audio_effects/Utility","trackRef":track}), signal.clone()).await?;
        let rows = self.rows("device", json!({"parent":track,"fields":["name","className"]}), signal.clone()).await?;
        let device = rows.last().cloned().ok_or_else(|| observation("The Utility Kumi added didn't appear."))?;
        self.gain_knob(&device, false, signal).await
    }

    /// A gain stage's knob: its Gain, Input Gain or Output. A Limiter that's maximizing (Live 12.4's Maximize On) does
    /// nothing with its Input Gain: its Threshold drives it, lowered to make it louder.
    async fn gain_knob(self: &Rc<Self>, device: &JsonObject, limited: bool, signal: Signal) -> Result<GainStage, RuntimeError> {
        let device_ref =
            device.get("ref").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| observation("A device without a ref."))?;
        let parameters = self.rows("parameter", json!({"parent":device_ref,"fields":["name","value"]}), signal).await?;
        let named = |name: &str| parameters.iter().find(|row| row.get("name").and_then(Value::as_str) == Some(name));
        let maximizing =
            limited && named("Maximize On").and_then(|row| row.get("value")).and_then(Value::as_f64).is_some_and(|on| on >= 0.5);
        let names: Vec<&str> = parameters.iter().filter_map(|row| row.get("name").and_then(Value::as_str)).collect();
        let found = gain_parameter(limited, maximizing, &names).and_then(named);
        let parameter = found
            .and_then(|row| row.get("ref").and_then(Value::as_str).map(str::to_owned))
            .ok_or_else(|| observation("Kumi couldn't find the gain knob."))?;
        let name = device.get("name").and_then(Value::as_str).unwrap_or("Utility").to_string();
        // Said by the level it turns: a maximizing Limiter's threshold goes the other way.
        let label = if maximizing { format!("{name} level (its Threshold, maximizing)") } else { format!("{name} gain") };
        Ok(GainStage { device: device_ref, parameter, label, limited, inverted: maximizing })
    }

    /// Kumi's changes in HISTORY now (applied ones), to tell a round's own from what came before.
    pub(super) fn applied_ids(&self) -> Vec<String> {
        self.history
            .entries
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.borrow().record.state == ChangeState::Applied)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The applied changes since a checkpoint, oldest first: their ids and titles.
    pub(super) fn applied_since(&self, checkpoint: &[String]) -> Vec<(String, String)> {
        self.history
            .entries
            .borrow()
            .iter()
            .filter(|(id, entry)| !checkpoint.contains(id) && entry.borrow().record.state == ChangeState::Applied)
            .map(|(id, entry)| (id.clone(), entry.borrow().record.title.clone()))
            .collect()
    }

    /// Each track's Arrangement clips that play, by track name (the first 64 tracks): where each starts and ends, in beats.
    pub(super) async fn arrangement_clips(&self, signal: Signal) -> Vec<(String, Vec<(f64, f64)>)> {
        let Ok(tracks) = self.rows("track", json!({"fields":["name"]}), signal.clone()).await else { return vec![] };
        let mut found = vec![];
        for track in tracks.iter().take(64) {
            let (Some(reference), Some(name)) = (track.get("ref").and_then(Value::as_str), track.get("name").and_then(Value::as_str))
            else {
                continue;
            };
            let Ok(clips) = self
                .rows("arrangement-clip", json!({"parent":reference,"fields":["start","endTime","length","muted"]}), signal.clone())
                .await
            else {
                continue;
            };
            let spans: Vec<(f64, f64)> = clips
                .iter()
                // A muted clip doesn't play.
                .filter(|clip| clip.get("muted").and_then(Value::as_bool) != Some(true))
                .filter_map(|clip| {
                    let start = clip.get("start").and_then(Value::as_f64)?;
                    // Where it stops playing: a looped clip's length is its loop's, not how far it runs.
                    let end = match clip.get("endTime").and_then(Value::as_f64) {
                        Some(end) => end,
                        None => start + clip.get("length").and_then(Value::as_f64)?,
                    };
                    Some((start, end))
                })
                .collect();
            if !spans.is_empty() {
                found.push((name.to_string(), spans));
            }
        }
        found
    }

    /// The long ref of a track (from its ref or its name), as HISTORY keeps refs: positions.
    pub(super) async fn track_ref_long(self: &Rc<Self>, named: &str, signal: Signal) -> Option<String> {
        let short = self.scope_ref(Some(named), signal).await.ok();
        short.map(|short| {
            let long = self.connection().references.borrow().lengthen(&json!({ "ref": short }));
            long["ref"].as_str().unwrap_or(&short).to_owned()
        })
    }

    /// Kumi's applied track renames not among `before`, oldest first: the renamed track's ref, the name it had and its
    /// new name.
    pub(super) fn renames_since(&self, before: &[String]) -> Vec<(String, String, String)> {
        self.history
            .entries
            .borrow()
            .iter()
            .filter(|(id, _)| !before.contains(id))
            .filter_map(|(_, entry)| {
                let entry = entry.borrow();
                let applied = matches!(entry.record.state, ChangeState::Applied | ChangeState::Kept);
                let restore = entry.restore.as_ref().filter(|restore| applied && restore.field == "name")?;
                Some((restore.reference.clone(), restore.value.clone()?, entry.record.track.as_ref()?.name.clone()))
            })
            .collect()
    }

    /// Whether a change can change what's heard (a rename, a colour, a locator, the transport or an empty track can't);
    /// None when HISTORY has no such change.
    pub(super) fn audible_change(&self, id: &str) -> Option<bool> {
        self.history.entries.borrow().get(id).map(|entry| entry.borrow().audible())
    }

    /// Kumi's undo of one change; why not, when Live wouldn't. Taking back goes on after Esc (what was changed is put back),
    /// with the cleanup's own time limit.
    pub(super) async fn undo_change(&self, id: &str) -> Result<(), String> {
        match self.history.undo(id, self.cleanup(), false).await {
            Ok(undone) if !undone.is_error => Ok(()),
            Ok(undone) => Err(head(&undone.text, 120)),
            Err(error) => Err(head(&error.to_string(), 120)),
        }
    }

    /// Live's identities for the devices these changes made (loaded or duplicated), as HISTORY recorded them.
    pub(super) fn devices_made_by(&self, changes: &[(String, String)]) -> Vec<String> {
        let entries = self.history.entries.borrow();
        changes.iter().filter_map(|(id, _)| entries.get(id).and_then(|entry| entry.borrow().created.clone())).collect()
    }
}

/// The knob rebalancing turns on its device, by name among the device's parameters: a Limiter's Input Gain (an older
/// Live's Gain), or its Threshold while it's maximizing; a Utility's Output (an older Live's Gain). A Limiter's Output
/// comes after its ceiling: turning that would let the peaks past it.
pub fn gain_parameter<'a>(limiter: bool, maximizing: bool, names: &[&'a str]) -> Option<&'a str> {
    let wanted: &[&str] = match (limiter, maximizing) {
        (true, true) => &["Threshold"],
        (true, false) => &["Input Gain", "Gain"],
        (false, _) => &["Output", "Gain"],
    };
    wanted.iter().find_map(|want| names.iter().find(|name| *name == want).copied())
}

/// The device rebalancing turns, from a chain's rows (with `className` and `enabled`): the last Limiter that's on
/// (peaks then stay put), else a Utility that's the last device on. Live's own, by class: a plug-in or a rack named
/// "Limiter" or "Utility" isn't one. One that's off turns nothing: without either, rebalancing puts a Utility at the
/// end.
pub fn gain_device(rows: &[JsonObject]) -> Option<(JsonObject, bool)> {
    let on = |row: &JsonObject| row.get("enabled") != Some(&Value::Bool(false));
    let class = |row: &JsonObject| row.get("className").and_then(Value::as_str).unwrap_or("").to_owned();
    if let Some(at) = rows.iter().rposition(|row| on(row) && class(row) == "Limiter") {
        return Some((rows[at].clone(), true));
    }
    rows.iter()
        .rev()
        .find(|row| on(row))
        .filter(|row| matches!(class(row).as_str(), "StereoGain" | "Utility"))
        .map(|row| (row.clone(), false))
}

/// A device row's identity in Live (empty when the bridge doesn't say).
fn identity(row: &JsonObject) -> String {
    match row.get("objectIdentity") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// Devices by the name Live shows (its class when it has none).
fn names(rows: &[JsonObject]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .or_else(|| row.get("className").and_then(Value::as_str))
                .unwrap_or("Device")
                .to_owned()
        })
        .collect()
}
