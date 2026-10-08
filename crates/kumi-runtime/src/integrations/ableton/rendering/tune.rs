//! Code picks the numbers for a change the model chose, and the judge decides it as a round: an EQ fitted to what was
//! measured (no trial listens), one knob homed in on a few listens at a time, or a few knobs searched in small CMA-ES
//! generations, each generation heard in one pass on scratch copies of the track. Each round logs its listens.

use super::super::connection::NO_CURRENT_LIVE;
use super::judge::JudgeHeard;
use super::rig::Window;
use super::*;
use crate::listening::{
    checklist::{plan_cut, Quantity, Target},
    cmaes::Cmaes,
    detect::ProblemKind,
    fit::{fit, Band, Limits, Shape},
    home::{Homed, Homing},
    judging::{candidate_cost, predict},
    knobs::{Scale, Unit},
    measure::{measure_file, MeasureOptions},
    probes::Probes,
    round::{Round, RoundKind},
};
use kumi_common::js::{number::to_string, string::head};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuneHow {
    /// An EQ calculated from what was measured.
    Fit,
    /// One knob, homed in on.
    Home,
    /// A few knobs that interact, searched in generations heard side by side.
    Search,
    /// One knob heard across its range in one pass, what it does saved, then set where it meets the target.
    Probe,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TuneRequest {
    pub device: String,
    pub how: TuneHow,
    /// The checklist item it's for (its id or label); the run's next target without one.
    pub target: Option<String>,
    pub knobs: Vec<String>,
    pub change: Option<String>,
}

/// A device's knob: where it is, its range, the values it lists (a switch or a menu), and its scale.
#[derive(Debug, Clone)]
pub(super) struct DeviceKnob {
    pub(super) reference: String,
    pub(super) name: String,
    pub(super) raw: f64,
    pub(super) min: f64,
    pub(super) max: f64,
    pub(super) items: Vec<String>,
    pub(super) scale: Option<Scale>,
}

impl DeviceKnob {
    /// The raw value of one of the values it lists.
    fn item(&self, wanted: &str) -> Option<f64> {
        let index = self.items.iter().position(|item| item.eq_ignore_ascii_case(wanted))?;
        let steps = (self.items.len().max(2) - 1) as f64;
        Some(self.min + index as f64 * (self.max - self.min) / steps)
    }
}

const KNOBS_SCRIPT: &str = include_str!("../assets/device-knobs.py");
const COPIES_SCRIPT: &str = include_str!("../assets/tune-copies.py");
/// Generations a search hears at most, each in one pass.
const SEARCH_GENERATIONS: u32 = 4;
/// The sample rate EQ curves are drawn at (the shape below 16 kHz doesn't depend on it).
const CURVE_RATE: f64 = 48_000.;
/// Probes a homing spends at most.
const HOME_PROBES: usize = 4;
/// Seconds a make of copies may take before it stops itself (well inside the script's 20 s limit, so it can take back
/// what it made).
pub(super) const COPIES_BUDGET: f64 = 12.;

impl Rendering {
    pub async fn tune(self: &Rc<Self>, request: &TuneRequest, original: Signal) -> Result<Result<Round, String>, RuntimeError> {
        if !self.available() {
            return Ok(Err(NO_CURRENT_LIVE.into()));
        }
        if self.rendering.get() {
            return Ok(Err("Kumi is already listening to something; wait for it.".into()));
        }
        let index = {
            let run = self.judge.borrow();
            let Some(run) = run.as_ref().filter(|run| run.rounds.last().is_some_and(|round| round.kind != RoundKind::Done)) else {
                return Ok(Err("Start a judged run first (judge with a goal): tune picks numbers against its checklist.".into()));
            };
            if let Some(why) = &run.ended {
                return Ok(Err(format!("That run is over ({why}): start a new one with a goal.")));
            }
            match &request.target {
                Some(named) => {
                    let named = named.trim().to_lowercase();
                    match run.checklist.items.iter().position(|item| item.id == named || item.label.to_lowercase() == named) {
                        Some(index) => index,
                        None => {
                            return Ok(Err(format!(
                                "“{named}” isn't on the checklist; its items are {}.",
                                run.checklist.items.iter().map(|item| item.id.as_str()).collect::<Vec<_>>().join(", ")
                            )))
                        }
                    }
                }
                None => match run.target {
                    Some(index) => index,
                    None => return Ok(Err("Every item is within tolerance: there's nothing to tune.".into())),
                },
            }
        };
        let signal = abort::any([original, self.connection().lifetime.clone()]);
        // Live changed under the run: its bars are heard again before any number is picked against them.
        if self.judge.borrow().as_ref().is_some_and(|run| run.stale) {
            match self.rebaseline(signal.clone()).await {
                // The device to tune may have gone with this answer's changes: the round asks for them again first.
                Ok(Ok((round, true))) => {
                    self.tell_judged(&round);
                    return Ok(Ok(round));
                }
                Ok(Ok((round, false))) => self.tell_judged(&round),
                Ok(Err(why)) => return Ok(Err(why)),
                Err(error) => {
                    signal.check()?;
                    return Ok(Err(head(&error.to_string(), 400)));
                }
            }
        }
        let outcome = async {
            // The target's excerpt, and how it sounds as things stand. With a change already made (a device just put in
            // place), its "before" can't be heard any more: then the bars whose before is known.
            let changed = {
                let run = self.judge.borrow();
                !self.applied_since(&run.as_ref().unwrap().checkpoint).is_empty()
            };
            let window = {
                let mut guard = self.judge.borrow_mut();
                let run = guard.as_mut().unwrap();
                run.target = Some(index);
                let wanted = self.excerpt_for(run);
                let known = run.excerpts.iter().any(|excerpt| excerpt.window == wanted && excerpt.state == run.state);
                if known || !changed {
                    run.window = wanted;
                }
                run.window
            };
            if let Err(why) = self.ensure_before(window, signal.clone()).await? {
                return Ok(Err(why));
            }
            let knobs = match self.device_knobs(&request.device, signal.clone()).await? {
                Ok(knobs) => knobs,
                Err(why) => return Ok(Err(why)),
            };
            // Knobs named by role: found on the device, or on another of its track in the agreed order.
            let names: Vec<String> = knobs.iter().map(|knob| knob.name.clone()).collect();
            let (request, knobs) = match self.knobs_by_role(request, &names, signal.clone()).await? {
                Ok(None) => (request.clone(), knobs),
                Ok(Some(found)) if found.device == request.device => (found, knobs),
                Ok(Some(found)) => match self.device_knobs(&found.device, signal.clone()).await? {
                    Ok(knobs) => (found, knobs),
                    Err(why) => return Ok(Err(why)),
                },
                Err(why) => return Ok(Err(why)),
            };
            match request.how {
                TuneHow::Fit => self.tune_fit(&request, index, window, &knobs, signal.clone()).await,
                TuneHow::Home => self.tune_home(&request, index, window, &knobs, signal.clone()).await,
                TuneHow::Search => self.tune_search(&request, index, window, &knobs, signal.clone()).await,
                TuneHow::Probe => self.tune_probe(&request, index, window, &knobs, signal.clone()).await,
            }
        }
        .await;
        match outcome {
            Ok(Ok(round)) => {
                self.tell_judged(&round);
                Ok(Ok(round))
            }
            Ok(Err(why)) => Ok(Err(why)),
            Err(error) => {
                signal.check()?;
                Ok(Err(head(&error.to_string(), 400)))
            }
        }
    }

    /// A device's knobs with the text Live shows across each one's range, read in one call.
    async fn device_knobs(&self, device: &str, signal: Signal) -> Result<Result<Vec<DeviceKnob>, String>, RuntimeError> {
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

    /// Sets knobs to raw values in one step.
    pub(super) async fn set_knobs(&self, device: &str, values: &[(&DeviceKnob, f64)], signal: Signal) -> Result<(), RuntimeError> {
        let values: Vec<Value> = values.iter().map(|(knob, raw)| json!({"parameterRef":knob.reference,"value":raw})).collect();
        self.step("set_device_parameters", json!({"deviceRef":device,"values":values}), signal).await?;
        Ok(())
    }

    /// The excerpt's sound before the change, measured again for its frames (what a fit predicts from).
    async fn before_heard(&self, window: Window, signal: Signal) -> Result<crate::listening::measure::Heard, RuntimeError> {
        let (file, start) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            let excerpt = run.excerpts.iter().find(|excerpt| excerpt.window == window && excerpt.state == run.state);
            excerpt
                .map(|excerpt| (excerpt.file.clone(), excerpt.start))
                .ok_or_else(|| RuntimeError::plain("Kumi lost what the excerpt sounded like."))?
        };
        let tempo = self.observer.tempo.get().unwrap_or(120.);
        measure_file(
            &file.to_string_lossy(),
            MeasureOptions { start: Some(start), seconds: Some(window.beats * 60. / tempo), signal: Some(signal) },
        )
        .await
        .map_err(|error| RuntimeError::plain(error.to_string()))
    }

    /// An EQ Eight's bands calculated from what was measured: the smallest cut that clears a resonance, or bands fitted
    /// to the balance gaps toward the reference. One listen, the judged one.
    async fn tune_fit(
        self: &Rc<Self>,
        request: &TuneRequest,
        index: usize,
        window: Window,
        knobs: &[DeviceKnob],
        signal: Signal,
    ) -> Result<Result<Round, String>, RuntimeError> {
        let (quantity, target, label, now, open_regions) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            let item = &run.checklist.items[index];
            (item.quantity.clone(), item.target, item.label.clone(), run.whole[index], run.checklist.region_points(&run.whole))
        };
        let (bands, predicted) = match quantity {
            Quantity::Problem { problem: ProblemKind::Resonance | ProblemKind::Harshness, low, high, steady: true, .. } => {
                let wanted = match target {
                    Target::AtMost { value } => value - 0.5,
                    _ => return Ok(Err(format!("{label} has no ceiling to cut it down to."))),
                };
                let heard = self.before_heard(window, signal.clone()).await?;
                let (band, predicted) = plan_cut(&heard, low, high, true, wanted, 4., CURVE_RATE);
                if band.db > -0.25 {
                    return Ok(Err(format!("No cut Kumi can make clears {label}: it may be the music's own (a held note); leave it.")));
                }
                (vec![band], Some(format!("{label} predicted {} → {} dB", now.map(to_string).unwrap_or("–".into()), to_string(round1(predicted)))))
            }
            Quantity::Problem { problem: ProblemKind::Resonance | ProblemKind::Harshness, .. } => {
                return Ok(Err(format!(
                    "{label} comes and goes: a static cut would dull it all the time. Home in on a de-esser's or a dynamic band's threshold instead (how: home)."
                )))
            }
            Quantity::Region { .. } if open_regions.iter().any(|(_, gap, _)| *gap != 0.) => {
                let limits = Limits { db: 6., q: (0.3, 4.), hz: (20., 20_000.), bands: 4, within: 0.5 };
                (fit(&open_regions, &limits, CURVE_RATE), None)
            }
            _ => {
                return Ok(Err(format!(
                    "fit calculates an EQ for a steady resonance or a balance gap toward the reference; for {label}, home in on a knob (how: home)."
                )))
            }
        };
        if bands.is_empty() {
            return Ok(Err(format!("{label} is close enough that no band is worth adding.")));
        }
        // EQ Eight's bands: the ones off (or doing nothing) take the fitted ones.
        let knob = |band: usize, what: &str| knobs.iter().find(|knob| knob.name == format!("{band} {what} A"));
        if knob(1, "Frequency").is_none() {
            return Ok(Err("fit sets an EQ Eight's bands: put an EQ Eight where the fix belongs, then tune it.".into()));
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
        // cut isn't one).
        let shaped = (1..=8).any(|band| {
            let on = knob(band, "Filter On").is_some_and(|on| on.raw > on.min);
            let gained = knob(band, "Filter Type").is_some_and(|kind| {
                kind.items.get(kind.raw.round() as usize).is_some_and(|item| item.contains("Bell") || item.contains("Shelf"))
            });
            let boosted =
                knob(band, "Gain").and_then(|gain| gain.scale.as_ref().map(|units| units.shown(gain.raw).abs() >= 0.05)).unwrap_or(false);
            on && gained && boosted
        });
        if shaped && (adaptive_on || scaled) {
            return Ok(Err(
                "This EQ Eight has bands with gain of its own, shaped by Adaptive Q or a Scale away from 100%; fit draws fixed-Q bands at full scale and won't reshape them. Put a fresh EQ Eight where the fix belongs, then tune it."
                    .into(),
            ));
        }
        if let Some(adaptive) = adaptive.filter(|_| adaptive_on) {
            values.push((adaptive, adaptive.item("Off").unwrap_or(adaptive.min)));
        }
        if let Some(scale) = scale.filter(|_| scaled) {
            match &scale.scale {
                Some(units) => values.push((scale, units.raw(100.))),
                None => return Ok(Err("Kumi couldn't read this EQ Eight's Scale.".into())),
            }
        }
        let mut said = vec![];
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
            said.push(describe_band(band));
        }
        self.set_knobs(&request.device, &values, signal.clone()).await?;
        let change = format!(
            "{}EQ fitted: {}{}",
            said_first(request),
            said.join(", "),
            predicted.map(|predicted| format!(" ({predicted})")).unwrap_or_default()
        );
        self.judge_round(Some(change), None, signal).await
    }

    /// One knob homed in on: each probe heard on the target's excerpt, the next sized by what the last ones did, until
    /// the target's aim is met or four probes are spent. The closest is judged as the round.
    async fn tune_home(
        self: &Rc<Self>,
        request: &TuneRequest,
        index: usize,
        window: Window,
        knobs: &[DeviceKnob],
        signal: Signal,
    ) -> Result<Result<Round, String>, RuntimeError> {
        let [named] = request.knobs.as_slice() else {
            return Ok(Err("home moves one knob: name it in knobs (one name, as the device shows it).".into()));
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
            return Ok(Err(format!("{} is a switch or a list, not a knob to home in on.", knob.name)));
        };
        let (aim, met, unit, label, start, before, checklist, changed, whole_all, before_all) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            let item = &run.checklist.items[index];
            let Some((aim, met)) = item.aim() else {
                return Ok(Err(format!("{} is a guard, not something to home in on.", item.label)));
            };
            let excerpt = run.excerpts.iter().find(|excerpt| excerpt.window == window && excerpt.state == run.state);
            let before = excerpt.and_then(|excerpt| excerpt.values[index]);
            let before_all = excerpt.map(|excerpt| excerpt.values.clone()).unwrap_or_default();
            let changed = !self.applied_since(&run.checkpoint).is_empty();
            (
                aim,
                met,
                item.unit.clone(),
                item.label.clone(),
                run.whole[index],
                before,
                run.checklist.clone(),
                changed,
                run.whole.clone(),
                before_all,
            )
        };
        let (Some(start), Some(before)) = (start, before) else {
            return Ok(Err(format!("Kumi hasn't measured {label} yet; judge the run again first.")));
        };
        let (low, high) = scale.range();
        let x0 = scale.perceptual(scale.shown(knob.raw));
        // What the knob measures where it is is known only when nothing changed since the last judged listen (a
        // limiter just put in place changes it): otherwise that's the first probe.
        // A dB knob moves a level in dB about one for one, to begin with.
        let slope = (scale.unit == Unit::Db && matches!(unit.as_str(), "LUFS" | "dB" | "dBTP")).then_some(1.);
        // Probes closer than one just-noticeable step sound the same: the knob's step, or the target's own when the knob
        // moves it one for one (a true peak's 0.2 dB).
        let resolution = match slope {
            Some(_) => scale.step().min(checklist.items[index].jnd),
            None => scale.step(),
        };
        let mut homing =
            Homing::new(aim, met, (scale.perceptual(low), scale.perceptual(high)), x0, (!changed).then_some(start), HOME_PROBES)
                .resolution(resolution);
        // What this knob was heard to do to this measure before (probed, or homed in on), as the first setting to try:
        // the excerpt's readings, so the aim moves by how far the whole stretch reads from the excerpt.
        let kind = self.device_kind(&request.device, signal.clone()).await;
        let measure = checklist.items[index].id.clone();
        let scope = self.judge.borrow().as_ref().and_then(|run| run.track.clone()).unwrap_or_else(|| "mix".into());
        let saved = kind.as_ref().and_then(|kind| Probes::home().load(kind, &knob.name, &measure, &scope));
        if let Some(response) = &saved {
            let now = if changed { response.at_knob(x0) } else { Some(before) };
            if let Some(guess) = now.and_then(|now| response.predict((x0, now), aim - (start - before))) {
                homing = homing.guess(guess);
            }
        }
        // Where the knob is now (it moves with each probe).
        let mut current = x0;
        let (track, focus) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            (run.track.clone(), run.focus.clone())
        };
        let mut heard_at: Vec<(f64, JudgeHeard)> = vec![];
        let probed: Result<Result<Homed, String>, RuntimeError> = async {
            loop {
                let at = match homing.next(slope) {
                    Ok(at) => at,
                    Err(why) => return Ok(Ok(why)),
                };
                self.set_knobs(&request.device, &[(knob, scale.raw(scale.from_perceptual(at)))], signal.clone()).await?;
                current = at;
                let heard = match self.judge_hear(track.as_deref(), focus.as_deref(), window, signal.clone()).await? {
                    Ok(heard) => heard,
                    Err(why) => return Ok(Err(why)),
                };
                let values = {
                    let mut guard = self.judge.borrow_mut();
                    guard.as_mut().unwrap().listens += 1;
                    heard.read(&checklist)
                };
                let Some(measured) = values[index] else { return Ok(Ok(Homed::Stuck)) };
                let reached = checklist.items[index].quantity.moved(start, before, measured);
                // A probe that makes anything else audibly worse is too far, however close it gets.
                let predicted = predict(&checklist, &whole_all, &before_all, &values);
                if checklist.verdict(Some(index), &whole_all, &predicted).hurt.is_empty() {
                    homing.heard(at, reached);
                } else {
                    homing.hurt(at, reached);
                }
                heard_at.push((at, heard));
            }
        }
        .await;
        // A homing that ends unjudged (nothing came through, Esc, Live gone) puts the knob back.
        let stop = match probed {
            Ok(Ok(stop)) => stop,
            Ok(Err(why)) => {
                let _ = self.set_knobs(&request.device, &[(knob, knob.raw)], self.cleanup()).await;
                return Ok(Err(why));
            }
            Err(error) => {
                let _ = self.set_knobs(&request.device, &[(knob, knob.raw)], self.cleanup()).await;
                return Err(error);
            }
        };
        // What each probe read on the excerpt, saved for the next time.
        if let Some(kind) = &kind {
            let heard: Vec<(f64, f64)> = heard_at.iter().filter_map(|(at, heard)| Some((*at, heard.read(&checklist)[index]?))).collect();
            let _ = Probes::home().add(kind, &knob.name, &measure, &scope, &heard, Some(resolution));
        }
        let (best, reached) = homing.best().unwrap_or((x0, start));
        let Some(position) = heard_at.iter().position(|(at, _)| *at == best) else {
            // Nothing heard beat where it was: the knob goes back and nothing is judged.
            self.set_knobs(&request.device, &[(knob, knob.raw)], signal.clone()).await?;
            return Ok(Err(format!(
                "Moving {} didn't bring {label} closer to {} in {} listens; it's back where it was. Try another knob or device.",
                knob.name,
                to_string(round1(aim)),
                homing.listens()
            )));
        };
        let (_, heard) = heard_at.swap_remove(position);
        // The knob goes to the best probe unless it's already there (a probe that read nothing may be the last set).
        if current != best {
            if let Err(error) = self.set_knobs(&request.device, &[(knob, scale.raw(scale.from_perceptual(best)))], signal.clone()).await {
                let _ = self.set_knobs(&request.device, &[(knob, knob.raw)], self.cleanup()).await;
                return Err(error);
            }
        }
        let shown = |at: f64| scale.text(scale.from_perceptual(at));
        let change = format!(
            "{}{} {} → {}, homed in over {} listens ({label} {} → {} {unit}{})",
            said_first(request),
            knob.name,
            shown(x0),
            shown(best),
            homing.listens(),
            to_string(round1(start)),
            to_string(round1(reached)),
            match stop {
                Homed::Met => "",
                Homed::Spent => "; its listens spent",
                Homed::Stuck => "; the knob went as far as it helps",
            }
        );
        self.judge_round(Some(change), Some(heard), signal).await
    }
}

impl Rendering {
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
                let theirs = entry["pid"].as_f64().is_some_and(|pid| pid as u32 != std::process::id() && crate::library::state::alive(pid));
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

    /// Several tracks heard quietly in one pass over `window`, each measured.
    pub(super) async fn hear_tracks(
        self: &Rc<Self>,
        names: &[String],
        window: Window,
        signal: Signal,
    ) -> Result<Result<IndexMap<String, crate::listening::measure::Heard>, String>, RuntimeError> {
        let tempo = self.observer.tempo.get().unwrap_or(120.);
        let candidates: Vec<AuditionCandidate> =
            names.iter().map(|name| AuditionCandidate { track: name.clone(), mix: None, label: None, clip: None }).collect();
        self.tell(format!("Hearing {} takes side by side", names.len()), Some(true));
        self.begin_rendering();
        let mut rig = None;
        let rendered: Result<IndexMap<String, Render>, RuntimeError> = async {
            rig = Some(self.open_rig(&candidates, Some(window.from), Some(window.beats), signal.clone()).await?);
            self.render_pass(rig.as_mut().unwrap(), signal.clone()).await
        }
        .await;
        if let Some(rig) = rig.as_mut() {
            self.close_rig(rig).await;
        }
        self.end_rendering();
        self.tell("Listened", Some(false));
        let files = rendered?;
        let mut heard = IndexMap::new();
        for name in names {
            let Some(render) = files.get(name) else { continue };
            let seconds = render.seconds.unwrap_or(window.beats * 60. / tempo);
            let measured = measure_file(
                &render.file,
                MeasureOptions { start: Some(render.start), seconds: Some(seconds), signal: Some(signal.clone()) },
            )
            .await
            .map_err(|error| RuntimeError::plain(error.to_string()))?;
            heard.insert(name.clone(), measured);
        }
        if heard.is_empty() {
            return Ok(Err("Nothing came through from the copies; is something playing on the track there?".into()));
        }
        Ok(Ok(heard))
    }

    /// A few knobs that interact, searched in small CMA-ES generations: each generation's candidates set on scratch
    /// copies of the track and heard side by side in one pass, scored on the target's gap, the guards and what the
    /// change costs. The best is set on the device and judged as the round.
    async fn tune_search(
        self: &Rc<Self>,
        request: &TuneRequest,
        index: usize,
        window: Window,
        knobs: &[DeviceKnob],
        signal: Signal,
    ) -> Result<Result<Round, String>, RuntimeError> {
        if !(2..=5).contains(&request.knobs.len()) {
            return Ok(Err("search tunes 2 to 5 knobs that interact; for one, home in on it (how: home).".into()));
        }
        let mut chosen: Vec<(&DeviceKnob, Scale)> = vec![];
        for named in &request.knobs {
            let wanted = named.trim().to_lowercase();
            let Some(knob) = knobs.iter().find(|knob| knob.name.to_lowercase() == wanted) else {
                return Ok(Err(format!("The device has no knob called “{named}”.")));
            };
            let Some(scale) = knob.scale.clone() else {
                return Ok(Err(format!("{} is a switch or a list; search tunes knobs.", knob.name)));
            };
            chosen.push((knob, scale));
        }
        let (scoped, checklist, before, whole) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            let before = run
                .excerpts
                .iter()
                .find(|excerpt| excerpt.window == window && excerpt.state == run.state)
                .map(|excerpt| excerpt.values.clone());
            (run.track.clone(), run.checklist.clone(), before, run.whole.clone())
        };
        let Some(scoped) = scoped else {
            return Ok(Err("search hears copies of one track: start the run on that track (judge with track), or home in on one knob at a time for the mix.".into()));
        };
        let Some(before) = before else {
            return Ok(Err("Kumi lost what the excerpt sounded like before; judge the run again.".into()));
        };
        if let Quantity::Problem { problem: ProblemKind::Masking, .. } = checklist.items[index].quantity {
            return Ok(Err(
                "Masking is measured against the focus track, which the copies don't play with: home in on one knob instead (how: home)."
                    .into(),
            ));
        }
        // The copies are of the device's own track, so it has to be the run's (refs read long: a short one is a counter).
        let run_track = self.scope_ref(Some(&scoped), signal.clone()).await.ok();
        if !run_track
            .is_some_and(|track| super::super::mutations::same_track(&self.connection().references.borrow(), &request.device, &track))
        {
            return Ok(Err(format!("search hears copies of the run's own track ({scoped}): tune a device on it, or home in on one knob.")));
        }
        // Where the knobs are now, as fractions of their perceptual ranges.
        let span = |scale: &Scale| {
            let (low, high) = scale.range();
            (scale.perceptual(low), scale.perceptual(high))
        };
        let fraction = |scale: &Scale, raw: f64| {
            let (low, high) = span(scale);
            if high > low {
                (scale.perceptual(scale.shown(raw)) - low) / (high - low)
            } else {
                0.5
            }
        };
        let raw_at = |scale: &Scale, at: f64| {
            let (low, high) = span(scale);
            scale.raw(scale.from_perceptual(low + at.clamp(0., 1.) * (high - low)))
        };
        let start: Vec<f64> = chosen.iter().map(|(knob, scale)| fraction(scale, knob.raw)).collect();
        // Where the knobs are now costs no listen: its gap as the run reads it. A search has to beat it.
        let standing = candidate_cost(&checklist, index, &before, &whole, &before, &start, &start);
        let mut search = Cmaes::new(&start, 0.2, None, crate::core::evolve::seeded(rand::random::<u32>()));
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
        // One Live undo step for the whole search: the copies made, set and dropped (and the listens between) undo as
        // one, to nothing.
        let copies =
            match self.open_copies(&request.device, search.lambda, ("Kumi: search", SEARCH_GENERATIONS, window), signal.clone()).await? {
                Ok(copies) => copies,
                Err(why) => return Ok(Err(why)),
            };
        // The search, then the copies go whatever happened.
        let searched: Result<Result<(), String>, RuntimeError> = async {
            for _ in 0..SEARCH_GENERATIONS {
                let points = search.ask();
                let mut values = serde_json::Map::new();
                for (name, point) in copies.names.iter().zip(&points) {
                    let knobs: serde_json::Map<String, Value> =
                        chosen.iter().zip(point).map(|((knob, scale), at)| (knob.name.clone(), json!(raw_at(scale, *at)))).collect();
                    values.insert(name.clone(), Value::Object(knobs));
                }
                if let Err(why) = self.set_copies(&copies, values, signal.clone()).await? {
                    return Ok(Err(why));
                }
                let heard = match self.hear_tracks(&copies.names, window, signal.clone()).await? {
                    Ok(heard) => heard,
                    Err(why) => return Ok(Err(why)),
                };
                self.judge.borrow_mut().as_mut().unwrap().listens += 1;
                let costs: Vec<f64> = copies
                    .names
                    .iter()
                    .zip(&points)
                    .map(|(name, point)| match heard.get(name) {
                        Some(heard) => {
                            candidate_cost(&checklist, index, &before, &whole, &heard_alone(checklist.read(heard, None)), point, &start)
                        }
                        None => f64::INFINITY,
                    })
                    .collect();
                search.tell(&points, &costs);
            }
            Ok(Ok(()))
        }
        .await;
        self.close_copies(copies).await;
        match searched? {
            Ok(()) => {}
            Err(why) => return Ok(Err(why)),
        }
        // Better than standing still by half a step (or half of what's left, near the target).
        if search.best.as_ref().is_none_or(|(_, cost)| !(*cost < standing - (standing / 2.).min(0.25))) {
            return Ok(Err(format!(
                "The search heard nothing better than where the knobs are (in {} generations of {}); they stay. Try other knobs or another device.",
                search.generation, search.lambda
            )));
        }
        let Some((best, _)) = search.best.clone() else {
            return Ok(Err("The search heard nothing to choose from.".into()));
        };
        let values: Vec<(&DeviceKnob, f64)> = chosen.iter().zip(&best).map(|((knob, scale), at)| (*knob, raw_at(scale, *at))).collect();
        self.set_knobs(&request.device, &values, signal.clone()).await?;
        let said: Vec<String> = chosen
            .iter()
            .zip(&best)
            .map(|((knob, scale), at)| {
                format!("{} {} → {}", knob.name, scale.text(scale.shown(knob.raw)), scale.text(scale.shown(raw_at(scale, *at))))
            })
            .collect();
        let change = format!(
            "{}{} on {scoped}, searched in {} generations of {} heard side by side",
            said_first(request),
            said.join(", "),
            search.generation,
            search.lambda
        );
        self.judge_round(Some(change), None, signal).await
    }
}

/// The model's own words for the change, as the log's lead-in.
pub(super) fn said_first(request: &TuneRequest) -> String {
    request.change.as_ref().map(|change| format!("{}. ", change.trim().trim_end_matches('.'))).unwrap_or_default()
}

fn describe_band(band: &Band) -> String {
    let hz = if band.hz >= 1000. { format!("{} kHz", to_string(round1(band.hz / 1000.))) } else { format!("{} Hz", band.hz.round()) };
    let shape = match band.shape {
        Shape::Bell => "bell",
        Shape::LowShelf => "low shelf",
        Shape::HighShelf => "high shelf",
    };
    format!("{} dB {shape} at {hz}, Q {}", to_string(round1(band.db)), to_string(round1(band.q)))
}

fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
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

/// Written whole beside itself and moved into place; gone once it holds nothing.
fn write_journal(file: &Path, entries: &[Value]) {
    if entries.is_empty() {
        let _ = std::fs::remove_file(file);
        return;
    }
    let temporary = file.with_extension("json.partial");
    if std::fs::write(&temporary, serde_json::to_vec(entries).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&temporary, file);
    }
}
