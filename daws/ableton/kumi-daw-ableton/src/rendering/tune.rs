//! Live's side of tune: a device's knobs and how EQ Eight's bands are set for a fit, the scratch copies a search or a
//! probe hears (made, journalled and swept in Live), and Live's undo step around them. What tune does with them is
//! `Listening`'s.
use super::*;
use kumi_common::js::string::head;
use kumi_listening::{
    fit::{Band, Shape},
    knobs::Scale,
    renderer::{DeviceKnob, EqFit},
};
use std::path::Path;

const KNOBS_SCRIPT: &str = include_str!("../assets/device-knobs.py");
const COPIES_SCRIPT: &str = include_str!("../assets/tune-copies.py");
/// Seconds a make of copies may take before it stops itself (well inside the script's 20 s limit, so it can take back
/// what it made).
pub(super) const COPIES_BUDGET: f64 = 12.;

impl LiveRenderer {
    /// An EQ Eight's mode, as Live numbers it (0 Stereo, 1 L/R, 2 M/S); None when Live doesn't say.
    async fn eq_mode(&self, device: &str, signal: Signal) -> Result<Option<i64>, RuntimeError> {
        if !self.connection().has("live_run_python") {
            return Ok(None);
        }
        let long = self.connection().references.borrow().lengthen(&json!({"deviceRef":device}));
        let long = long["deviceRef"].as_str().unwrap_or(device).to_owned();
        let code = "result = int(obj.global_mode)";
        let read = self
            .connection()
            .call("live_run_python", object(json!({"code":code,"mode":"exec","ref":long,"timeoutMs":5000})), signal)
            .await?;
        let done = if read.is_error == Some(true) { None } else { super::super::context::payload(&read).ok() };
        Ok(done.filter(|done| done.get("ok") == Some(&Value::Bool(true))).and_then(|done| done.get("result")?.as_i64()))
    }

    /// A device's knobs with the text Live shows across each one's range, read in one call.
    pub(super) async fn device_knobs(&self, device: &str, signal: Signal) -> Result<Result<Vec<DeviceKnob>, String>, RuntimeError> {
        let long = self.connection().references.borrow().lengthen(&json!({"deviceRef":device}));
        let long = long["deviceRef"].as_str().unwrap_or(device).to_owned();
        let rows = self.rows("parameter", json!({"parent":long,"fields":["name"]}), signal.clone()).await?;
        let read = self
            .connection()
            .call("live_run_python", object(json!({"code":KNOBS_SCRIPT,"mode":"exec","ref":long,"timeoutMs":10000})), signal)
            .await?;
        let done = if read.is_error == Some(true) { None } else { super::super::context::payload(&read).ok() };
        let Some(listed) = done.filter(|done| done.get("ok") == Some(&Value::Bool(true))).and_then(|done| done.get("result").cloned())
        else {
            return Ok(Err("Kumi couldn't read the device's knobs; is it still there? Discover it again.".into()));
        };
        let listed = listed.as_array().cloned().unwrap_or_default();
        let mut knobs = vec![];
        for (row, read) in rows.iter().zip(&listed) {
            let name = read.get("name").and_then(Value::as_str).unwrap_or("");
            if row.get("name").and_then(Value::as_str) != Some(name) {
                continue;
            }
            let Some(reference) = row.get("ref").and_then(Value::as_str) else { continue };
            let number = |key: &str| read.get(key).and_then(Value::as_f64).unwrap_or(0.);
            let grid: Vec<(f64, String)> = read
                .get("grid")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|point| Some((point.get(0)?.as_f64()?, point.get(1)?.as_str()?.to_owned())))
                .collect();
            let items: Vec<String> = read
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect();
            knobs.push(DeviceKnob {
                reference: reference.into(),
                name: name.into(),
                raw: number("value"),
                min: number("min"),
                max: number("max"),
                scale: if items.is_empty() { Scale::read(&grid) } else { None },
                items,
            });
        }
        if knobs.is_empty() {
            return Ok(Err("Kumi couldn't read the device's knobs; is it still there? Discover it again.".into()));
        }
        Ok(Ok(knobs))
    }

    /// Sets knobs (by reference) to raw values in one step.
    pub(super) async fn set_knob_values(&self, device: &str, values: &[(&str, f64)], signal: Signal) -> Result<(), RuntimeError> {
        let values: Vec<Value> = values.iter().map(|(reference, raw)| json!({"parameterRef":reference,"value":raw})).collect();
        self.step("set_device_parameters", json!({"deviceRef":device,"values":values}), signal).await?;
        Ok(())
    }

    /// The knobs to set so an EQ Eight's bands are these: the ones off (or doing nothing) take them. Why not, when it
    /// can't be done.
    pub(super) async fn fit_eq_bands(
        &self,
        device: &str,
        knobs: &[DeviceKnob],
        bands: &[Band],
        signal: Signal,
    ) -> Result<Result<EqFit, String>, RuntimeError> {
        // EQ Eight's bands: the ones off (or doing nothing) take the fitted ones.
        let knob = |band: usize, what: &str| knobs.iter().find(|knob| knob.name == format!("{band} {what} A"));
        if knob(1, "Frequency").is_none() {
            return Ok(Err("fit sets an EQ Eight's bands: put an EQ Eight where the fix belongs, then tune it.".into()));
        }
        // fit writes the A curve: in L/R or M/S that's the left or mid channel's alone.
        match self.eq_mode(device, signal.clone()).await? {
            Some(0) => {}
            Some(_) => {
                return Ok(Err(
                    "This EQ Eight is in L/R or M/S mode, and fit writes only its A curve (the left or mid channel): set it to Stereo, or put a fresh EQ Eight where the fix belongs, then tune it."
                        .into(),
                ))
            }
            None => {
                return Ok(Err(
                    "Kumi couldn't read whether this EQ Eight is in Stereo, L/R or M/S mode, and fit writes only its A curve, which is the whole sound only in Stereo: home in on its bands instead (how: home)."
                        .into(),
                ))
            }
        }
        let unused: Vec<usize> = (1..=8)
            .filter(|band| {
                let on = knob(*band, "Filter On").is_none_or(|on| on.raw <= on.min);
                let flat = knob(*band, "Gain")
                    .and_then(|gain| gain.scale.as_ref().map(|scale| scale.shown(gain.raw).abs() < 0.05))
                    .unwrap_or(false);
                let bell = knob(*band, "Filter Type")
                    .is_some_and(|kind| kind.items.get(kind.raw.round() as usize).is_some_and(|item| item == "Bell"));
                on || (flat && bell)
            })
            .collect();
        if unused.len() < bands.len() {
            return Ok(Err(format!(
                "The EQ Eight has {} free bands and the fit needs {}; add another EQ Eight.",
                unused.len(),
                bands.len()
            )));
        }
        let mut values: Vec<(&DeviceKnob, f64)> = vec![];
        // The fit draws fixed-Q bells at full scale: Adaptive Q narrows deep cuts and Scale rescales every gain. On an
        // EQ Eight of the fit's own they're set so; one with bands of the producer's keeps them, and takes a fit only
        // when they already are.
        let adaptive = knobs.iter().find(|knob| knob.name == "Adaptive Q");
        let scale = knobs.iter().find(|knob| knob.name == "Scale");
        let adaptive_on = adaptive.is_some_and(|knob| knob.raw > knob.min);
        let scaled = scale.is_some_and(|knob| knob.scale.as_ref().is_none_or(|units| (units.shown(knob.raw) - 100.).abs() > 0.5));
        // The bands they'd reshape: on, a bell or a shelf, with gain (a cut filter has none, so a fresh EQ Eight's low
        // cut isn't one), on either curve (B is the right or side channel's, in L/R or M/S).
        let curve = |band: usize, what: &str, which: &str| knobs.iter().find(|knob| knob.name == format!("{band} {what} {which}"));
        let shaped = ["A", "B"].iter().any(|which| {
            (1..=8).any(|band| {
                let on = curve(band, "Filter On", which).is_some_and(|on| on.raw > on.min);
                let gained = curve(band, "Filter Type", which).is_some_and(|kind| {
                    kind.items.get(kind.raw.round() as usize).is_some_and(|item| item.contains("Bell") || item.contains("Shelf"))
                });
                let boosted = curve(band, "Gain", which)
                    .and_then(|gain| gain.scale.as_ref().map(|units| units.shown(gain.raw).abs() >= 0.05))
                    .unwrap_or(false);
                on && gained && boosted
            })
        });
        if shaped && (adaptive_on || scaled) {
            return Ok(Err(
                "This EQ Eight has bands with gain of its own, shaped by Adaptive Q or a Scale away from 100%; fit draws fixed-Q bands at full scale and won't reshape them. Put a fresh EQ Eight where the fix belongs, then tune it."
                    .into(),
            ));
        }
        // What else it sets is said with the bands.
        let mut settings = vec![];
        if let Some(adaptive) = adaptive.filter(|_| adaptive_on) {
            values.push((adaptive, adaptive.item("Off").unwrap_or(adaptive.min)));
            settings.push("Adaptive Q off");
        }
        if let Some(scale) = scale.filter(|_| scaled) {
            match &scale.scale {
                Some(units) => values.push((scale, units.raw(100.))),
                None => return Ok(Err("Kumi couldn't read this EQ Eight's Scale.".into())),
            }
            settings.push("Scale to 100%");
        }
        for (band, slot) in bands.iter().zip(&unused) {
            // Live 12 calls a band's width "Q" ("1 Q A"); older Live, "Resonance".
            let width = knob(*slot, "Q").or_else(|| knob(*slot, "Resonance"));
            let (Some(on), Some(kind), Some(hz), Some(db), Some(q)) =
                (knob(*slot, "Filter On"), knob(*slot, "Filter Type"), knob(*slot, "Frequency"), knob(*slot, "Gain"), width)
            else {
                return Ok(Err("Kumi couldn't find this EQ Eight's band knobs.".into()));
            };
            let shape = match band.shape {
                Shape::Bell => "Bell",
                Shape::LowShelf => "Low Shelf",
                Shape::HighShelf => "High Shelf",
            };
            let (Some(kind_raw), Some(hz_scale), Some(db_scale), Some(q_scale)) = (kind.item(shape), &hz.scale, &db.scale, &q.scale) else {
                return Ok(Err("Kumi couldn't read this EQ Eight's band units.".into()));
            };
            values.push((on, on.max));
            values.push((kind, kind_raw));
            values.push((hz, hz_scale.raw(band.hz)));
            values.push((db, db_scale.raw(band.db)));
            values.push((q, q_scale.raw(band.q)));
        }
        Ok(Ok(EqFit { values: values.into_iter().map(|(knob, raw)| (knob.reference.clone(), raw)).collect(), settings }))
    }

    /// Notes a search's scratch copies (`entry`: their prefix, the tracks there before, the copied track's name) with
    /// the Set they're in and this Kumi's process, or forgets them (None) once they're gone. Each Kumi keeps its own
    /// journal, so two never write over each other's entries.
    pub(super) fn note_copies(&self, prefix: &str, entry: Option<Value>) {
        let Some(own) = self.own_journal() else { return };
        let mut entries = read_journal(&own);
        entries.retain(|kept| kept["prefix"].as_str() != Some(prefix));
        if let Some(mut entry) = entry {
            entry["set"] = json!(self.connection().set.borrow().clone());
            entry["path"] = json!(self.history.remember.current().and_then(|project| project.path.clone()));
            entry["pid"] = json!(std::process::id());
            entries.push(entry);
        }
        write_journal(&own, &entries);
    }

    /// This Kumi's journal of scratch copies.
    fn own_journal(&self) -> Option<PathBuf> {
        self.copies_journal.as_ref().map(|base| PathBuf::from(format!("{}-{}.json", base.display(), std::process::id())))
    }

    /// Removes scratch copies a search left in this Set (cut off by a crash or a lost connection), once Live is back:
    /// the tracks that weren't there before it and carry its prefix. A track with the copied track's name that wasn't
    /// there before is named for the producer, not taken: it may be theirs. Another running Kumi's copies are its own
    /// to remove; an entry is forgotten only once nothing of it is left.
    pub(super) async fn sweep_copies(&self, identity: &str, path: Option<&str>, signal: Signal) {
        let Some(base) = &self.copies_journal else { return };
        for file in journals(base) {
            let mut entries = read_journal(&file);
            let mut changed = false;
            for entry in entries.clone() {
                let Some(prefix) = entry["prefix"].as_str().filter(|prefix| prefix.starts_with("Kumi · try ")) else { continue };
                let here = match entry["path"].as_str().filter(|saved| !saved.is_empty()) {
                    Some(saved) => Some(saved) == path,
                    None => entry["set"].as_str() == Some(identity),
                };
                let theirs = entry["pid"].as_f64().is_some_and(|pid| pid as u32 != std::process::id() && kumi_library::state::alive(pid));
                let in_use = self.copies_live.borrow().iter().any(|live| live == prefix);
                if !here || theirs || in_use {
                    continue;
                }
                let args = json!({"action":"drop","prefix":prefix,"before":entry["before"],"source":entry["source"]});
                let code = format!(
                    "import json\nARGS = json.loads({})\n{COPIES_SCRIPT}",
                    serde_json::to_string(&args.to_string()).unwrap_or_default()
                );
                let swept = self
                    .connection()
                    .call("live_run_python", object(json!({"code":code,"mode":"exec","timeoutMs":20000})), signal.clone())
                    .await;
                let Some(done) = swept
                    .ok()
                    .filter(|read| read.is_error != Some(true))
                    .and_then(|read| super::super::context::payload(&read).ok())
                    .filter(|done| done.get("ok") == Some(&Value::Bool(true)))
                    .map(|done| done["result"].clone())
                else {
                    continue;
                };
                if let Some(gone) = done["gone"].as_u64().filter(|gone| *gone > 0) {
                    self.tell(format!("Kumi removed {gone} scratch copies a cut-off search left in this Set."), None);
                }
                for name in done["newcomers"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    self.tell(
                        format!("A track named “{name}” appeared while Kumi's search was cut off. If it's a leftover copy, delete it."),
                        None,
                    );
                }
                if done["left"].as_u64() == Some(0) {
                    entries.retain(|kept| kept["prefix"].as_str() != Some(prefix));
                    changed = true;
                }
            }
            if changed {
                write_journal(&file, &entries);
            }
        }
    }

    /// Opens one Live undo step for Kumi's own work, when the bridge can, closing by itself after about `seconds`
    /// (the work's own length, with room): its id, to close it with. When the answer is lost, a step may be open with
    /// no id to close it by, so whatever step is open is closed then.
    pub(super) async fn open_undo_step(&self, label: &str, seconds: f64) -> Option<String> {
        if !self.connection().has("live_undo_step_begin") || !self.connection().has("live_undo_step_end") {
            return None;
        }
        let limit = (seconds * 1000.).clamp(30_000., 600_000.).round() as u64;
        let signal = abort::any([self.connection().lifetime.clone(), abort::timeout(10_000)]);
        match self.connection().call("live_undo_step_begin", object(json!({"label":label,"timeoutMs":limit})), signal).await {
            Ok(opened) => super::super::context::payload(&opened)
                .ok()?
                .get("stepId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned),
            Err(_) => {
                let signal = abort::any([self.connection().lifetime.clone(), abort::timeout(10_000)]);
                let _ = self.connection().call("live_undo_step_end", object(json!({})), signal).await;
                None
            }
        }
    }

    pub(super) async fn close_undo_step(&self, step: Option<String>) {
        if let Some(step) = step {
            let signal = abort::any([self.connection().lifetime.clone(), abort::timeout(10_000)]);
            let _ = self.connection().call("live_undo_step_end", object(json!({"stepId":step})), signal).await;
        }
    }

    /// Runs the scratch-copies script on a device's track.
    pub(super) async fn copies(&self, device: &str, args: Value, signal: Signal) -> Result<Result<Value, String>, RuntimeError> {
        let long = self.connection().references.borrow().lengthen(&json!({"deviceRef":device}));
        let long = long["deviceRef"].as_str().unwrap_or(device).to_owned();
        let code =
            format!("import json\nARGS = json.loads({})\n{COPIES_SCRIPT}", serde_json::to_string(&args.to_string()).unwrap_or_default());
        let read = self
            .connection()
            .call("live_run_python", object(json!({"code":code,"mode":"exec","ref":long,"timeoutMs":20000})), signal)
            .await?;
        let done = if read.is_error == Some(true) { None } else { super::super::context::payload(&read).ok() };
        match done {
            Some(done) if done.get("ok") == Some(&Value::Bool(true)) => Ok(Ok(done.get("result").cloned().unwrap_or(Value::Null))),
            Some(done) => Ok(Err(head(
                done.get("error").and_then(|error| error.get("message")).and_then(Value::as_str).unwrap_or("Live refused the copies"),
                300,
            ))),
            None => Ok(Err("Live refused the copies.".into())),
        }
    }
}

/// Every Kumi's journal of scratch copies beside `base` (`<base>-<process>.json`).
fn journals(base: &Path) -> Vec<PathBuf> {
    let (Some(folder), Some(stem)) = (base.parent(), base.file_name().and_then(|name| name.to_str())) else { return vec![] };
    let Ok(listed) = std::fs::read_dir(folder) else { return vec![] };
    listed
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&format!("{stem}-")) && name.ends_with(".json"))
        })
        .collect()
}

fn read_journal(file: &Path) -> Vec<Value> {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
}

/// Written whole beside itself and moved into place, only the producer's; gone once it holds nothing.
fn write_journal(file: &Path, entries: &[Value]) {
    if entries.is_empty() {
        let _ = std::fs::remove_file(file);
        return;
    }
    let _ = kumi_types::private::write(file, &serde_json::to_vec(entries).unwrap_or_default());
}
