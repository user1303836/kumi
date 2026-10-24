//! Code picks the numbers for a change the model chose, and the judge decides it as a round: an EQ fitted to what was
//! measured (no trial listens), one knob homed in on a few listens at a time, or a few knobs searched in small CMA-ES
//! generations, each generation heard in one pass on scratch copies of the track. Each round logs its listens.

use super::{judge::JudgeHeard, masking_items, Listening};
use crate::{
    checklist::{masking_unfair_in, plan_cut, Checklist, Item, Quantity, Target, REGIONS},
    cmaes::Cmaes,
    detect::ProblemKind,
    fit::{fit, Band, Limits, Shape},
    heard_settings::{SearchKind, Setting, SettingsHeard},
    home::{Homed, Homing},
    judging::{candidate_cost, predict},
    knobs::{Scale, Unit},
    measure::{measure_file, Heard, MeasureOptions, THIRDS},
    probes::Probes,
    renderer::{DeviceKnob, Window},
    round::{Round, RoundKind},
    tools::{TuneHow, TuneRequest},
    warm,
};
use indexmap::IndexMap;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{number::to_string, string::head},
};
use kumi_types::{contracts::AuditionCandidate, errors::RuntimeError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::rc::Rc;

/// Generations a search hears at most, each in one pass.
const SEARCH_GENERATIONS: u32 = 4;
/// The sample rate EQ curves are drawn at (the shape below 16 kHz doesn't depend on it).
const CURVE_RATE: f64 = 48_000.;
/// Probes a homing spends at most.
const HOME_PROBES: usize = 4;

impl Listening {
    /// Sets knobs to raw values in one step.
    pub(crate) async fn set_knobs(&self, device: &str, values: &[(&DeviceKnob, f64)], signal: Signal) -> Result<(), RuntimeError> {
        let values: Vec<(&str, f64)> = values.iter().map(|(knob, raw)| (knob.reference.as_str(), *raw)).collect();
        self.host.set_knobs(device, &values, signal).await
    }

    /// Several tracks heard in one pass over `window`, each measured; `what` says what's heard, as it plays.
    pub(crate) async fn hear_tracks(
        self: &Rc<Self>,
        names: &[String],
        window: Window,
        what: &str,
        signal: Signal,
    ) -> Result<Result<IndexMap<String, Heard>, String>, RuntimeError> {
        Ok(self
            .hear_takes(names, window, what, signal)
            .await?
            .map(|heard| heard.into_iter().map(|(name, (heard, _))| (name, heard)).collect()))
    }

    /// `hear_tracks`, each take with how long it runs before `window.from` (seconds): a pass that records keeps a short
    /// lead before the part, one that taps the signal measures from the part itself.
    pub(crate) async fn hear_takes(
        self: &Rc<Self>,
        names: &[String],
        window: Window,
        what: &str,
        signal: Signal,
    ) -> Result<Result<IndexMap<String, (Heard, f64)>, String>, RuntimeError> {
        let tempo = self.host.tempo().unwrap_or(120.);
        let candidates: Vec<AuditionCandidate> =
            names.iter().map(|name| AuditionCandidate { track: name.clone(), mix: None, label: None, clip: None }).collect();
        self.tell(format!("{} {} · {what}", self.hearing(), self.bars_of(window)), Some(true));
        let rendering = self.host.flag().begin();
        let pass = self.host.hear_pass(&candidates, window, false, signal.clone()).await;
        drop(rendering);
        self.tell("Listened", Some(false));
        let longest = pass.lead;
        let files = pass.rendered?.files;
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
            // The lead is cut short only where the capture starts less than that before the part (the song's start).
            let lead = if longest == 0. {
                0.
            } else if render.start > 0. {
                longest
            } else {
                (window.from * 60. / tempo).min(longest)
            };
            heard.insert(name.clone(), (measured, lead));
        }
        if heard.is_empty() {
            return Ok(Err("Nothing came through; is something playing on those tracks there?".into()));
        }
        Ok(Ok(heard))
    }

    pub async fn tune(self: &Rc<Self>, request: &TuneRequest, original: Signal) -> Result<Result<Round, String>, RuntimeError> {
        let tuned = self.tune_now(request, original).await;
        // The takes the run no longer stores go, however the call ended.
        self.prune_kept().await;
        tuned
    }

    async fn tune_now(self: &Rc<Self>, request: &TuneRequest, original: Signal) -> Result<Result<Round, String>, RuntimeError> {
        if let Some(why) = self.host.refusal() {
            return Ok(Err(why));
        }
        if self.host.flag().is_on() {
            return Ok(Err("Kumi is already listening to something; wait for it.".into()));
        }
        self.follow_renames();
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
        let signal = abort::any([original, self.host.lifetime()]);
        // Live changed under the run: its bars are heard again before any number is picked against them.
        if self.judge.borrow().as_ref().is_some_and(|run| run.out_of_date()) {
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
            // The run's window moves only once its "before" is heard: a listen that fails or is stopped leaves the run
            // where it was.
            let (window, known) = {
                let mut guard = self.judge.borrow_mut();
                let run = guard.as_mut().unwrap();
                run.target = Some(index);
                let wanted = self.excerpt_for(run);
                let known = |window: Window| run.excerpts.iter().any(|excerpt| excerpt.window == window && excerpt.state == run.state);
                let window = if known(wanted) || !changed { wanted } else { run.window };
                (window, known(window))
            };
            // Heard now, a "before" would hold the change already made, which then goes unjudged.
            if changed && !known {
                return Ok(Err(
                    format!("A change is already in {}, and Kumi doesn't know how these bars sounded before it: judge it first (judge with change), or take it back, then tune.", self.host.profile().words.app),
                ));
            }
            if let Err(why) = self.ensure_before(window, signal.clone()).await? {
                return Ok(Err(why));
            }
            self.judge.borrow_mut().as_mut().unwrap().window = window;
            let knobs = match self.host.knobs(&request.device, signal.clone()).await? {
                Ok(knobs) => knobs,
                Err(why) => return Ok(Err(why)),
            };
            // Knobs named by role: found on the device, or on another of its track in the agreed order. An EQ band goes
            // nearest where the target sits in frequency.
            let near = self.judge.borrow().as_ref().and_then(|run| target_hz(&run.checklist.items.get(index)?.quantity));
            let (request, knobs) = match self.knobs_by_role(request, &knobs, near, signal.clone()).await? {
                Ok(None) => (request.clone(), knobs),
                Ok(Some(found)) if found.device == request.device => (found, knobs),
                Ok(Some(found)) => match self.host.knobs(&found.device, signal.clone()).await? {
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

    /// The excerpt's sound before the change, measured again for its frames (what a fit predicts from).
    async fn before_heard(&self, window: Window, signal: Signal) -> Result<crate::measure::Heard, RuntimeError> {
        let (file, start) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            let excerpt = run.excerpts.iter().find(|excerpt| excerpt.window == window && excerpt.state == run.state);
            excerpt
                .map(|excerpt| (excerpt.file.clone(), excerpt.start))
                .ok_or_else(|| RuntimeError::plain("Kumi lost what the excerpt sounded like."))?
        };
        let tempo = self.host.tempo().unwrap_or(120.);
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
        let (quantity, target, label, now, open_regions, excerpt) = {
            let run = self.judge.borrow();
            let run = run.as_ref().unwrap();
            let item = &run.checklist.items[index];
            let excerpt = run
                .excerpts
                .iter()
                .find(|excerpt| excerpt.window == window && excerpt.state == run.state)
                .and_then(|excerpt| excerpt.values[index]);
            (item.quantity.clone(), item.target, item.label.clone(), run.whole[index], run.checklist.region_points(&run.whole), excerpt)
        };
        let (bands, predicted) = match quantity {
            Quantity::Problem { problem: ProblemKind::Resonance | ProblemKind::Harshness, low, high, steady: true, .. } => {
                let wanted = match target {
                    Target::AtMost { value } => value - 0.5,
                    _ => return Ok(Err(format!("{label} has no ceiling to cut it down to."))),
                };
                // The cut is planned on the excerpt where it stands out most, which reads over the whole stretch: aimed
                // as far over the target, so the whole lands on it rather than under.
                let wanted = match (now, excerpt) {
                    (Some(whole), Some(excerpt)) => wanted + (excerpt - whole),
                    _ => wanted,
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
        // The bands go on the DAW's equaliser: the ones it isn't using take the fitted ones.
        let fitted = match self.host.fit_eq(&request.device, knobs, &bands, signal.clone()).await? {
            Ok(fitted) => fitted,
            Err(why) => return Ok(Err(why)),
        };
        let said: Vec<String> = bands.iter().map(describe_band).collect();
        let values: Vec<(&str, f64)> = fitted.values.iter().map(|(reference, raw)| (reference.as_str(), *raw)).collect();
        self.host.set_knobs(&request.device, &values, signal.clone()).await?;
        let settings = fitted.settings;
        let change = format!(
            "{}EQ fitted: {}{}{}",
            said_first(request),
            said.join(", "),
            if settings.is_empty() { String::new() } else { format!("; {}", settings.join(", ")) },
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
        let kind = self.host.device_kind(&request.device, signal.clone()).await;
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
                let what = format!("{} at {}", knob.name, scale.text(scale.from_perceptual(at)));
                let heard = match self.judge_hear(track.as_deref(), focus.as_deref(), window, &what, signal.clone()).await? {
                    Ok(heard) => heard,
                    Err(why) => return Ok(Err(why)),
                };
                let mut values = {
                    let mut guard = self.judge.borrow_mut();
                    guard.as_mut().unwrap().listens += 1;
                    heard.read(&checklist)
                };
                // Main's chain moves the mix but not the focus (heard before it): masking can't be read across it, so
                // no probe goes by it. As a guard it reads as it was; as the target, the homing stops.
                if !heard.main_unfair.is_empty() {
                    let masking = masking_items(&checklist);
                    if masking.contains(&index) {
                        let focus = focus.as_deref().unwrap_or("the focus");
                        return Ok(Err(masking_unfair_in(self.host.profile(), focus, &heard.main_unfair, heard.premix_missed.as_deref())
                            .unwrap_or_default()));
                    }
                    for at in masking {
                        values[at] = before_all.get(at).copied().flatten();
                    }
                }
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
                let _ = self.set_knobs(&request.device, &[(knob, knob.raw)], self.host.cleanup()).await;
                return Ok(Err(why));
            }
            Err(error) => {
                let _ = self.set_knobs(&request.device, &[(knob, knob.raw)], self.host.cleanup()).await;
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
            // Nothing heard beat where it was: the knob goes back (Esc or not) and nothing is judged.
            self.set_knobs(&request.device, &[(knob, knob.raw)], self.host.cleanup()).await?;
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
                let _ = self.set_knobs(&request.device, &[(knob, knob.raw)], self.host.cleanup()).await;
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

impl Listening {
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
        if !self.host.on_track(&request.device, &scoped, signal.clone()).await {
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
        let mut search = Cmaes::new(&start, 0.2, None, crate::cmaes::seeded(rand::random::<u32>()));
        // What the search hears is handed out, a generation at a time, for Kumi to remember what worked on this device:
        // who the device is (never its place), what the search aims at and how the track sounded before it.
        let device = match self.remembers() {
            true => self.host.device_identity(&request.device, signal.clone()).await,
            false => None,
        };
        let run = uuid::Uuid::new_v4().to_string();
        let target = aimed_at(&checklist.items[index]);
        let knobs_were: serde_json::Map<String, Value> =
            fractions(&chosen, &start).into_iter().map(|(name, at)| (name, json!(at))).collect();
        let was = json!({"values": readings(&checklist, &before), "knobs": knobs_were});
        // What worked on this device before, from the past searches nearest this one: heard in its first generation.
        let starts = match (&device, &self.memory) {
            (Some(device), Some(memory)) => {
                let past = memory.bests(self.host.profile().id, &device.name, &[SearchKind::Tune]).await;
                let names: Vec<String> = chosen.iter().map(|(knob, _)| knob.name.clone()).collect();
                let item = &checklist.items[index];
                let sounds: Vec<(String, f64, f64)> =
                    checklist.items.iter().zip(&before).filter_map(|(item, value)| Some((item.id.clone(), (*value)?, item.jnd))).collect();
                let aim = item.aim().map(|(aim, _)| (aim, item.jnd));
                warm::starts(&past, &warm::Now { knobs: &names, start: &start, target: &target, aim, sounds: &sounds })
            }
            _ => vec![],
        };
        search.inject(&starts.iter().map(|start| start.point.clone()).collect::<Vec<_>>());
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
        let copies = match self
            .host
            .open_copies(&request.device, search.lambda, "Kumi: search", SEARCH_GENERATIONS, window, signal.clone())
            .await?
        {
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
                if let Err(why) = self.host.set_copies(&copies, values, signal.clone()).await? {
                    return Ok(Err(why));
                }
                let what = format!("Kumi hears {} settings on copies of {scoped} only it hears", copies.names.len());
                let heard = match self.hear_tracks(&copies.names, window, &what, signal.clone()).await? {
                    Ok(heard) => heard,
                    Err(why) => return Ok(Err(why)),
                };
                self.judge.borrow_mut().as_mut().unwrap().listens += 1;
                let read: Vec<Option<Vec<Option<f64>>>> =
                    copies.names.iter().map(|name| heard.get(name).map(|heard| heard_alone(checklist.read(heard, None)))).collect();
                let costs: Vec<f64> = read
                    .iter()
                    .zip(&points)
                    .map(|(values, point)| match values {
                        Some(values) => candidate_cost(&checklist, index, &before, &whole, values, point, &start),
                        None => f64::INFINITY,
                    })
                    .collect();
                let generation = search.generation + 1;
                search.tell(&points, &costs);
                if let Some(device) = &device {
                    // Each setting heard (one that came through, and was read on its target), a remembered one said.
                    let settings = points
                        .iter()
                        .zip(&read)
                        .zip(&costs)
                        .filter(|(_, cost)| cost.is_finite())
                        .filter_map(|((point, values), cost)| {
                            let measured = Value::Object(readings(&checklist, values.as_ref()?));
                            let remembered = starts
                                .iter()
                                .filter(|_| generation == 1)
                                .find(|start| start.point.iter().zip(point).all(|(a, b)| a.clamp(0., 1.) == *b))
                                .map(|start| start.remembered.id.clone());
                            Some(Setting {
                                knobs: fractions(&chosen, point),
                                measured,
                                cost: *cost,
                                generation: Some(generation),
                                remembered,
                            })
                        })
                        .collect();
                    self.heard(SettingsHeard::Heard {
                        run: run.clone(),
                        kind: SearchKind::Tune,
                        daw: self.host.profile().id,
                        device: device.clone(),
                        track: Some(scoped.clone()),
                        target: target.clone(),
                        before: Some(was.clone()),
                        settings,
                    });
                }
            }
            Ok(Ok(()))
        }
        .await;
        self.host.close_copies(copies).await;
        // Better than standing still by half a step (or half of what's left, near the target), as the judge counts better.
        let better = search.best.as_ref().is_some_and(|(_, cost)| *cost < standing - (standing / 2.).min(0.5));
        if device.is_some() {
            self.heard(SettingsHeard::Ended { run, kept: better && matches!(searched, Ok(Ok(()))) });
        }
        match searched? {
            Ok(()) => {}
            Err(why) => return Ok(Err(why)),
        }
        if !better {
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
            "{}{} on {scoped}, searched in {} generations of {} heard side by side{}",
            said_first(request),
            said.join(", "),
            search.generation,
            search.lambda,
            started_from(&starts)
        );
        self.judge_round(Some(change), None, signal).await
    }
}

/// What a search started from, as its log says it: where the nearest remembered setting worked, and when.
fn started_from(starts: &[warm::Start]) -> String {
    let Some(first) = starts.first() else { return String::new() };
    let day = chrono::DateTime::from_timestamp_millis(first.remembered.at)
        .map(|at| format!(" on {}", at.with_timezone(&chrono::Local).format("%-d %b")))
        .unwrap_or_default();
    let track = first.remembered.track.as_deref().map(|track| format!(" on {track}")).unwrap_or_default();
    let more = if starts.len() > 1 { format!(" (and {} more)", starts.len() - 1) } else { String::new() };
    format!("; started from what worked{track}{day}{more}")
}

/// The knobs at a point of a search, by name: each the fraction of its perceptual range it's set to.
fn fractions(chosen: &[(&DeviceKnob, Scale)], point: &[f64]) -> IndexMap<String, f64> {
    chosen.iter().zip(point).map(|((knob, _), at)| (knob.name.clone(), at.clamp(0., 1.))).collect()
}

/// A listen's readings, by checklist item: the ones it read.
fn readings(checklist: &Checklist, values: &[Option<f64>]) -> serde_json::Map<String, Value> {
    checklist
        .items
        .iter()
        .zip(values)
        .filter_map(|(item, value)| Some((item.id.clone(), json!(value.filter(|value| value.is_finite())?))))
        .collect()
}

/// What a search aims at, as Kumi remembers it: the item's kind (what it reads; a reference's own vector left out, a
/// fingerprint of it kept as `towards`), and what it wants (its target, and where a knob turned for it should take it).
fn aimed_at(item: &Item) -> Value {
    let mut target = serde_json::to_value(&item.quantity).unwrap_or_else(|_| json!({}));
    if let Some(object) = target.as_object_mut() {
        if let Some(to) = object.remove("to") {
            let towards = hex::encode(Sha256::digest(to.to_string().as_bytes()));
            object.insert("towards".into(), json!(towards[..16]));
        }
        object.insert("item".into(), json!(item.id));
        object.insert("unit".into(), json!(item.unit));
        object.insert("wanted".into(), serde_json::to_value(item.target).unwrap_or(Value::Null));
        if let Some((aim, _)) = item.aim() {
            object.insert("aim".into(), json!(aim));
        }
    }
    target
}

/// The model's own words for the change, as the log's lead-in.
pub(super) fn said_first(request: &TuneRequest) -> String {
    request.change.as_ref().map(|change| format!("{}. ", change.trim().trim_end_matches('.'))).unwrap_or_default()
}

/// Where a checklist item sits in frequency (Hz): a balance region's middle, or a problem's band; None for the rest.
fn target_hz(quantity: &Quantity) -> Option<f64> {
    match quantity {
        Quantity::Region { region } => {
            let (_, low, high) = REGIONS[*region];
            Some((THIRDS[low] * THIRDS[high]).sqrt())
        }
        Quantity::Problem { low, high, .. } => Some((low * high).sqrt()),
        _ => None,
    }
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
