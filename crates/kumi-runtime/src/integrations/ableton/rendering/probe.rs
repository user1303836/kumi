//! Probing a device: one knob heard across its range in one pass, on scratch copies of the track each with the knob
//! at one setting. What it does to every checklist measure is saved (so homing that knob later starts where the
//! response says), and the knob goes where its response meets the target, judged as the round.

use super::rig::Window;
use super::tune::{said_first, DeviceKnob, TuneRequest, COPIES_BUDGET};
use super::*;
use crate::listening::{
    checklist::Quantity,
    detect::ProblemKind,
    judging::predict,
    probes::{Probes, Response},
    round::Round,
};
use kumi_common::js::{number::to_string, string::head};

/// Settings a probe hears across a knob's range, in one pass.
const PROBE_POINTS: usize = 7;

/// Scratch copies of a device's track, made for one search or probe and taken away after it, in one Live undo step.
pub(super) struct Copies {
    device: String,
    prefix: String,
    known: Value,
    step: Option<String>,
    /// The copies' track names, in order.
    pub names: Vec<String>,
    position: u64,
}

impl Rendering {
    /// Makes `count` copies of a device's track. The tracks there before are noted first (by Live's identity), so a
    /// crash or a lost connection still leaves the copies to be swept.
    pub(super) async fn open_copies(
        &self,
        device: &str,
        count: usize,
        label: &str,
        signal: Signal,
    ) -> Result<Result<Copies, String>, RuntimeError> {
        let tag = uuid::Uuid::new_v4().to_string()[..4].to_owned();
        let prefix = format!("Kumi · try {tag}");
        let step = self.open_undo_step(label).await;
        let known = match self.copies(device, json!({"action":"before"}), signal.clone()).await {
            Ok(Ok(known)) => known,
            Ok(Err(why)) => {
                self.close_undo_step(step).await;
                return Ok(Err(why));
            }
            Err(error) => {
                self.close_undo_step(step).await;
                return Err(error);
            }
        };
        self.note_copies(&prefix, Some(json!({"prefix":prefix,"before":known["before"],"source":known["source"]})));
        self.copies_live.borrow_mut().push(prefix.clone());
        let mut copies = Copies { device: device.into(), prefix, known, step, names: vec![], position: 0 };
        let made = self.copies(device, json!({"action":"make","count":count,"prefix":copies.prefix,"budget":COPIES_BUDGET}), signal).await;
        match made {
            Ok(Ok(made)) => {
                copies.position = made.get("position").and_then(Value::as_u64).unwrap_or(0);
                copies.names = made
                    .get("names")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                Ok(Ok(copies))
            }
            Ok(Err(why)) => {
                self.close_copies(copies).await;
                Ok(Err(why))
            }
            Err(error) => {
                self.close_copies(copies).await;
                Err(error)
            }
        }
    }

    /// Sets each copy's knobs (by copy name, knob name and raw value).
    pub(super) async fn set_copies(
        &self,
        copies: &Copies,
        values: serde_json::Map<String, Value>,
        signal: Signal,
    ) -> Result<Result<(), String>, RuntimeError> {
        Ok(self.copies(&copies.device, json!({"action":"set","position":copies.position,"values":values}), signal).await?.map(|_| ()))
    }

    /// Takes the copies away, whatever happened (a make cut off partway takes its own back), and closes the undo step.
    pub(super) async fn close_copies(&self, copies: Copies) {
        let dropped = self
            .copies(
                &copies.device,
                json!({"action":"drop","prefix":copies.prefix,"before":copies.known["before"],"source":copies.known["source"]}),
                self.cleanup(),
            )
            .await;
        self.close_undo_step(copies.step).await;
        self.copies_live.borrow_mut().retain(|live| *live != copies.prefix);
        match dropped {
            // Forgotten only once nothing of them is left.
            Ok(Ok(done)) if done["left"].as_u64() == Some(0) => self.note_copies(&copies.prefix, None),
            Ok(Ok(done)) => self.tell(
                format!("{} of Kumi's scratch copies are still in the Set; it'll remove them when Live reconnects.", done["left"]),
                None,
            ),
            Ok(Err(why)) => {
                self.tell(format!("Kumi couldn't remove its scratch copies ({why}); it'll try again when Live reconnects."), None)
            }
            Err(error) => self.tell(
                format!(
                    "Kumi couldn't remove its scratch copies ({}); it'll try again when Live reconnects.",
                    head(&error.to_string(), 160)
                ),
                None,
            ),
        }
    }

    /// What kind of device it is, as its probes are saved: Live's class for its own devices, the name for a plug-in
    /// or a Max device.
    pub(super) async fn device_kind(&self, device: &str, signal: Signal) -> Option<String> {
        let long = self.connection().references.borrow().lengthen(&json!({"deviceRef":device}));
        let long = long["deviceRef"].as_str().unwrap_or(device).to_owned();
        let code = "result = [str(obj.class_name), str(obj.name)]";
        let read = self
            .connection()
            .call("live_run_python", object(json!({"code":code,"mode":"exec","ref":long,"timeoutMs":5000})), signal)
            .await
            .ok()?;
        if read.is_error == Some(true) {
            return None;
        }
        let done = super::super::context::payload(&read).ok()?;
        let pair = done.get("result")?.as_array()?;
        let (class, name) = (pair.first()?.as_str()?, pair.get(1)?.as_str()?);
        let own = !class.is_empty() && !class.starts_with("Plugin") && !class.starts_with("Mx");
        Some(if own { class.to_owned() } else { name.to_owned() }).filter(|kind| !kind.is_empty())
    }

    /// One knob heard across its range in one pass (scratch copies of the run's track, the knob at a setting on each),
    /// what it does to each checklist measure saved, then the knob set where its response meets the target (never
    /// past a setting that made something else worse) and judged.
    pub(super) async fn tune_probe(
        self: &Rc<Self>,
        request: &TuneRequest,
        index: usize,
        window: Window,
        knobs: &[DeviceKnob],
        signal: Signal,
    ) -> Result<Result<Round, String>, RuntimeError> {
        let [named] = request.knobs.as_slice() else {
            return Ok(Err("probe hears one knob across its range: name it in knobs (one name, as the device shows it).".into()));
        };
        let wanted = named.trim().to_lowercase();
        let Some(knob) = knobs
            .iter()
            .find(|knob| knob.name.to_lowercase() == wanted)
            .or_else(|| knobs.iter().find(|knob| knob.name.to_lowercase().contains(&wanted)))
        else {
            return Ok(Err(format!(
                "The device has no knob called “{named}”; it has {}.",
                knobs.iter().take(40).map(|knob| knob.name.as_str()).collect::<Vec<_>>().join(", ")
            )));
        };
        let Some(scale) = knob.scale.clone() else {
            return Ok(Err(format!("{} is a switch or a list, not a knob to probe.", knob.name)));
        };
        let (scoped, checklist, before, whole, changed) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            let before = run
                .excerpts
                .iter()
                .find(|excerpt| excerpt.window == window && excerpt.state == run.state)
                .map(|excerpt| excerpt.values.clone());
            (run.track.clone(), run.checklist.clone(), before, run.whole.clone(), !self.applied_since(&run.checkpoint).is_empty())
        };
        let item = checklist.items[index].clone();
        let Some((aim, met)) = item.aim() else {
            return Ok(Err(format!("{} is a guard, not something to probe toward.", item.label)));
        };
        let Some(scoped) = scoped else {
            return Ok(Err(
                "probe hears copies of one track: start the run on that track (judge with track), or home in on the knob (how: home)."
                    .into(),
            ));
        };
        let Some(before) = before else {
            return Ok(Err("Kumi lost what the excerpt sounded like before; judge the run again.".into()));
        };
        if let Quantity::Problem { problem: ProblemKind::Masking, .. } = item.quantity {
            return Ok(Err(
                "Masking is measured against the focus track, which the copies don't play with: home in on the knob instead (how: home)."
                    .into(),
            ));
        }
        let run_track = self.scope_ref(Some(&scoped), signal.clone()).await.ok();
        if !run_track
            .is_some_and(|track| super::super::mutations::same_track(&self.connection().references.borrow(), &request.device, &track))
        {
            return Ok(Err(format!("probe hears copies of the run's own track ({scoped}): probe a device on it, or home in on the knob.")));
        }
        // The settings: evenly across the knob's range in its perceptual units (dB, octaves, log-time).
        let (low, high) = scale.range();
        let (low, high) = (scale.perceptual(low), scale.perceptual(high));
        let x0 = scale.perceptual(scale.shown(knob.raw));
        let grid: Vec<f64> = (0..PROBE_POINTS).map(|k| low + (high - low) * k as f64 / (PROBE_POINTS - 1) as f64).collect();
        let kind = self.device_kind(&request.device, signal.clone()).await;
        // What the copies can't hear (the focus track isn't with them) reads as it did before.
        let heard_alone = |values: Vec<Option<f64>>| -> Vec<Option<f64>> {
            values
                .into_iter()
                .zip(&before)
                .zip(&checklist.items)
                .map(|((value, was), item)| match item.quantity {
                    Quantity::Problem { problem: ProblemKind::Masking, .. } => *was,
                    _ => value,
                })
                .collect()
        };
        let copies = match self.open_copies(&request.device, PROBE_POINTS, "Kumi: probe", signal.clone()).await? {
            Ok(copies) => copies,
            Err(why) => return Ok(Err(why)),
        };
        let probed: Result<Result<Vec<(f64, Vec<Option<f64>>)>, String>, RuntimeError> = async {
            let mut values = serde_json::Map::new();
            for (name, at) in copies.names.iter().zip(&grid) {
                values.insert(name.clone(), json!({ knob.name.clone(): scale.raw(scale.from_perceptual(*at)) }));
            }
            if let Err(why) = self.set_copies(&copies, values, signal.clone()).await? {
                return Ok(Err(why));
            }
            let heard = match self.hear_tracks(&copies.names, window, signal.clone()).await? {
                Ok(heard) => heard,
                Err(why) => return Ok(Err(why)),
            };
            self.judge.borrow_mut().as_mut().unwrap().listens += 1;
            Ok(Ok(copies
                .names
                .iter()
                .zip(&grid)
                .filter_map(|(name, at)| Some((*at, heard_alone(checklist.read(heard.get(name)?, None)))))
                .collect()))
        }
        .await;
        self.close_copies(copies).await;
        let probed = match probed? {
            Ok(probed) => probed,
            Err(why) => return Ok(Err(why)),
        };
        // Saved for every measure it moved, so homing any of them later starts from it.
        if let Some(kind) = &kind {
            let probes = Probes::home();
            for (measure, item) in checklist.items.iter().enumerate() {
                let points: Vec<(f64, f64)> = probed.iter().filter_map(|(at, values)| Some((*at, values[measure]?))).collect();
                if points.len() >= 2 {
                    let _ = probes.add(kind, &knob.name, &item.id, &points, scale.step());
                }
            }
        }
        // The target's response, and the settings that made something else audibly worse.
        let response = Response {
            knob: knob.name.clone(),
            measure: item.id.clone(),
            points: probed.iter().filter_map(|(at, values)| Some((*at, values[index]?))).collect(),
            at: 0,
        };
        if response.points.len() < 2 {
            return Ok(Err(format!(
                "Nothing came through to read {} from the copies; is something playing on {scoped} there?",
                item.label
            )));
        }
        let hurt: Vec<f64> = probed
            .iter()
            .filter(|(_, values)| !checklist.verdict(Some(index), &whole, &predict(&checklist, &whole, &before, values)).hurt.is_empty())
            .map(|(at, _)| *at)
            .collect();
        // The run reads the whole stretch; the copies, the excerpt: the aim moved by how far the two differ.
        let (Some(start), Some(here)) = (whole[index], before[index]) else {
            return Ok(Err(format!("Kumi hasn't measured {} yet; judge the run again first.", item.label)));
        };
        let aim_here = aim - (start - here);
        // Where the knob is now reads as the excerpt did, when nothing changed since; else as the response says.
        let now = if changed { response.at_knob(x0).unwrap_or(here) } else { here };
        let Some(mut target) = response.predict((x0, now), aim_here) else {
            return Ok(Err("The probe heard too little to go by.".into()));
        };
        // Never past a setting that hurt something else, on the way from where the knob is.
        let (mut floor, mut ceiling) = (low, high);
        for at in &hurt {
            if *at > x0 {
                ceiling = ceiling.min(*at - (high - low) / (PROBE_POINTS as f64 * 4.));
            } else if *at < x0 {
                floor = floor.max(*at + (high - low) / (PROBE_POINTS as f64 * 4.));
            }
        }
        target = target.clamp(floor.min(ceiling), ceiling.max(floor));
        let reached = response.at_knob(target).map(|reading| reading + (now - response.at_knob(x0).unwrap_or(now)) + (start - here));
        let gap_now = item.gap(Some(start));
        if reached.is_none_or(|reached| item.gap(Some(reached)) >= gap_now) || (target - x0).abs() < scale.step() {
            let readings: Vec<String> = response
                .points
                .iter()
                .map(|(at, value)| format!("{} → {}", scale.text(scale.from_perceptual(*at)), to_string(round1(*value))))
                .collect();
            return Ok(Err(format!(
                "{} doesn't bring {} closer across its range ({}); it stays where it is. Try another knob or device.",
                knob.name,
                item.label,
                readings.join(", ")
            )));
        }
        self.set_knobs(&request.device, &[(knob, scale.raw(scale.from_perceptual(target)))], signal.clone()).await?;
        let met_said = reached.is_some_and(|reached| (met.0..=met.1).contains(&reached));
        let change = format!(
            "{}{} {} → {}, from its response heard across its range ({} settings side by side): {} {} → {} {} predicted{}",
            said_first(request),
            knob.name,
            scale.text(scale.from_perceptual(x0)),
            scale.text(scale.from_perceptual(target)),
            response.points.len(),
            item.label,
            to_string(round1(start)),
            to_string(round1(reached.unwrap_or(start))),
            item.unit,
            if met_said { "" } else { " (as near as it gets)" }
        );
        self.judge_round(Some(change), None, signal).await
    }
}

fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}
