//! A judged round's moves on a pretend Live: what a lost reading means, peaks judged at the loudness rebalancing
//! brings back, two takes brought to one loudness, rebalancing heard and judged again, and a round taken back without
//! touching the producer's own devices.
use async_trait::async_trait;
use kumi_runtime::listening::{
    checklist::{Change, Checklist, Goal, Item, Quantity, Role, Target},
    detect,
    judging::{decide, matched, predict, processing, rebalance, removable, take_back, Listen, Placed, RoundHost, Unheard},
    listener::{Choice, Opinion},
    measure::{measure_samples, Heard},
};
use std::cell::{Cell, RefCell};

const RATE: f64 = 48_000.;

fn item(id: &str, quantity: Quantity, target: Target, role: Role, jnd: f64) -> Item {
    Item { id: id.into(), label: id.into(), role, unit: String::new(), quantity, target, jnd, fix: None }
}

/// Loudness to hold at −14, a true-peak ceiling and brightness to work toward, and clipping guarded.
fn checklist() -> Checklist {
    Checklist {
        items: vec![
            item("loudness", Quantity::Integrated, Target::Exactly { value: -14., within: 0.5 }, Role::Target, 1.),
            item("true peak", Quantity::TruePeak, Target::AtMost { value: -1. }, Role::Target, 0.2),
            item("brightness", Quantity::Tilt, Target::Exactly { value: 0., within: 0.3 }, Role::Target, 0.3),
            item("clipping", Quantity::Clipped, Target::AtMost { value: 0. }, Role::Guard, 1.),
        ],
    }
}
const LOUDNESS: usize = 0;
const PEAK: usize = 1;
const BRIGHTNESS: usize = 2;
const CLIPPING: usize = 3;

fn heard(samples: &[f64]) -> Heard {
    let samples: Vec<f32> = samples.iter().map(|s| *s as f32).collect();
    measure_samples(&samples, &samples, RATE)
}

#[test]
fn a_reading_lost_after_a_change_is_never_within_tolerance() {
    let checklist = checklist();
    let before = vec![Some(-14.), Some(-2.), Some(-1.), Some(0.)];
    // The change left silence: loudness and peaks read nothing.
    let after = vec![None, None, Some(-1.), Some(0.)];
    let verdict = checklist.verdict(Some(PEAK), &before, &after);
    assert!(!verdict.kept && verdict.why.contains("couldn't be read"), "{verdict:?}");
    assert_eq!((verdict.rows[LOUDNESS].change, verdict.rows[PEAK].change), (Change::Worse, Change::Worse));
    assert!(verdict.hurt.contains(&"loudness".to_string()), "silence counts even for loudness: {verdict:?}");
    // A whole stretch that can't be read now is open, not met.
    assert_eq!(checklist.next(&[None, Some(-2.), Some(0.), Some(0.)]), Some(LOUDNESS));
    // An item the excerpt doesn't read (the part isn't playing in those bars) keeps its value; one it read and lost
    // is lost.
    let whole = vec![Some(-14.), Some(-2.), Some(-1.), Some(0.)];
    let predicted = predict(&checklist, &whole, &[Some(-13.), None, Some(-1.), Some(0.)], &[None, None, Some(-0.5), Some(0.)]);
    assert_eq!(predicted, vec![None, Some(-2.), Some(-0.5), Some(0.)]);
}

#[test]
fn what_a_listen_cant_read_comes_off_the_checklist_and_guards_alone_are_no_goal() {
    let mut checklist = checklist();
    let mut values = vec![Some(-14.), Some(-2.), None, Some(0.)];
    assert_eq!(checklist.drop_unreadable(&mut values), ["brightness"]);
    assert_eq!((checklist.items.len(), values.len()), (3, 3));
    assert!(checklist.has_targets());
    let guards = Checklist { items: vec![item("clipping", Quantity::Clipped, Target::AtMost { value: 0. }, Role::Guard, 1.)] };
    assert!(!guards.has_targets());
}

#[test]
fn peaks_are_judged_at_the_loudness_rebalancing_brings_back() {
    let checklist = checklist();
    // Over the ceiling, and the change turns the last Utility down 2 dB: peaks drop, but so does loudness, and
    // rebalancing would put the 2 dB right back on that Utility.
    let before = vec![Some(-14.), Some(0.5), Some(-1.), Some(0.)];
    let after = vec![Some(-16.), Some(-1.5), Some(-1.), Some(0.)];
    let gain = checklist.rebalance(&before, &after).unwrap();
    assert_eq!(gain, 2.);
    assert!(checklist.verdict(Some(PEAK), &before, &after).kept, "as heard, the peaks look better");
    let level = checklist.at_level(&before, &after, gain);
    assert_eq!(level[PEAK], Some(0.5));
    let verdict = checklist.verdict(Some(PEAK), &before, &level);
    assert!(!verdict.kept && verdict.why.contains("didn't improve"), "{verdict:?}");
    // With a limiter after the gain, its ceiling holds the peaks: no shift.
    assert_eq!(checklist.at_level(&before, &after, 0.)[PEAK], Some(-1.5));
    // Clipping can't be told until it's heard: it reads as before until then.
    let clipped = vec![Some(-16.), Some(-1.5), Some(-1.), Some(40.)];
    assert_eq!(checklist.at_level(&before, &clipped, 2.)[CLIPPING], Some(0.));
}

#[test]
fn the_quieter_take_is_never_raised_to_meet_the_other() {
    // The change made it 3 dB louder: that take goes down 3 dB.
    assert_eq!(matched(Some(-14.), Some(-11.)), (0., -3.));
    // The change made it 3 dB quieter: the take before goes down instead of this one going up.
    assert_eq!(matched(Some(-14.), Some(-17.)), (-3., 0.));
    assert_eq!(matched(None, Some(-17.)), (0., 0.));
}

#[test]
fn a_part_that_doesnt_play_reads_no_masking_rather_than_none() {
    let tone: Vec<f64> = (0..(4. * RATE) as usize).map(|n| 0.05 * (2. * std::f64::consts::PI * 2000. * n as f64 / RATE).sin()).collect();
    let mix = heard(&tone);
    assert_eq!(detect::masking_share(&heard(&vec![0.; tone.len()]), &mix), None);
    assert_eq!(detect::masking_share(&heard(&tone), &mix), Some(0.));
}

#[test]
fn processing_has_to_earn_its_devices() {
    let checklist = checklist();
    let before = vec![Some(-14.), Some(0.), Some(-1.), Some(0.)];
    let after = vec![Some(-14.), Some(-0.8), Some(-1.), Some(0.)];
    let verdict = checklist.verdict(Some(PEAK), &before, &after);
    // 0.8 dB is four steps of 0.2: enough for one device, not for ten.
    assert_eq!(processing(&verdict, Some("true peak"), 1), None);
    assert!(processing(&verdict, Some("true peak"), 10).is_some_and(|why| why.contains("processing costs")));
}

/// One entry in pretend HISTORY.
struct Entry {
    id: String,
    title: String,
    applied: bool,
    undoable: bool,
    /// The gain a rebalance step turned.
    turned: f64,
    /// Live's identity for the device it made.
    created: Option<String>,
}

/// Live as far as a round can tell: a gain stage, HISTORY, a chain, and what the excerpt reads at each gain.
struct Pretend {
    gain: Cell<f64>,
    history: RefCell<Vec<Entry>>,
    chain: RefCell<Vec<Placed>>,
    deleted: RefCell<Vec<String>>,
    /// Listens to come that won't come through.
    unheard: Cell<usize>,
    /// The gain stage can't be turned (why).
    stuck_gain: Option<String>,
    /// Kumi's undo of a rebalance step fails.
    stuck_rebalance: bool,
    sound: Box<dyn Fn(f64) -> Vec<Option<f64>>>,
}

impl Pretend {
    fn new(sound: impl Fn(f64) -> Vec<Option<f64>> + 'static) -> Self {
        Self {
            gain: Cell::new(0.),
            history: RefCell::new(vec![]),
            chain: RefCell::new(vec![]),
            deleted: RefCell::new(vec![]),
            unheard: Cell::new(0),
            stuck_gain: None,
            stuck_rebalance: false,
            sound: Box::new(sound),
        }
    }
    fn change(&self, title: &str, undoable: bool) {
        self.made_one(title, undoable, None);
    }
    /// A change that made a device, Live's identity for it recorded.
    fn made_one(&self, title: &str, undoable: bool, created: Option<&str>) {
        let id = format!("c{}", self.history.borrow().len() + 1);
        let created = created.map(str::to_owned);
        self.history.borrow_mut().push(Entry { id, title: title.into(), applied: true, undoable, turned: 0., created });
    }
    fn listen(&self) -> Listen {
        let values = (self.sound)(self.gain.get());
        Listen { loudness: values[LOUDNESS], values, file: "take.wav".into(), start: 0. }
    }
}

fn device(identity: &str, name: &str) -> Placed {
    Placed { identity: identity.into(), name: name.into(), reference: format!("device:{identity}") }
}

#[async_trait(?Send)]
impl RoundHost for Pretend {
    async fn turn(&self, gain: f64) -> Result<String, String> {
        if let Some(why) = &self.stuck_gain {
            return Err(why.clone());
        }
        let id = format!("r{}", self.history.borrow().len() + 1);
        let undoable = !self.stuck_rebalance;
        self.history.borrow_mut().push(Entry {
            id,
            title: format!("Utility gain {gain} dB"),
            applied: true,
            undoable,
            turned: gain,
            created: None,
        });
        self.gain.set(self.gain.get() + gain);
        Ok("Utility gain".into())
    }
    async fn hear(&self) -> Result<Listen, Unheard> {
        if self.unheard.get() > 0 {
            self.unheard.set(self.unheard.get() - 1);
            return Err(Unheard { why: "nothing came through".into(), stopped: false });
        }
        Ok(self.listen())
    }
    fn applied(&self) -> Vec<String> {
        self.history.borrow().iter().filter(|entry| entry.applied).map(|entry| entry.id.clone()).collect()
    }
    fn applied_since(&self, mark: &[String]) -> Vec<(String, String)> {
        self.history
            .borrow()
            .iter()
            .filter(|entry| entry.applied && !mark.contains(&entry.id))
            .map(|entry| (entry.id.clone(), entry.title.clone()))
            .collect()
    }
    async fn undo(&self, id: &str) -> Result<(), String> {
        let mut history = self.history.borrow_mut();
        let entry = history.iter_mut().find(|entry| entry.id == id).unwrap();
        if !entry.undoable {
            return Err("Live wouldn't".into());
        }
        entry.applied = false;
        self.gain.set(self.gain.get() - entry.turned);
        Ok(())
    }
    fn made(&self, changes: &[(String, String)]) -> Vec<String> {
        let history = self.history.borrow();
        changes.iter().filter_map(|(id, _)| history.iter().find(|entry| &entry.id == id)?.created.clone()).collect()
    }
    fn stopped(&self) -> bool {
        false
    }
    async fn limited(&self) -> bool {
        false
    }
    async fn chain(&self) -> Result<Vec<Placed>, String> {
        Ok(self.chain.borrow().clone())
    }
    async fn delete(&self, reference: &str) -> bool {
        self.chain.borrow_mut().retain(|device| device.reference != reference);
        self.deleted.borrow_mut().push(reference.into());
        true
    }
}

#[tokio::test]
async fn a_kept_change_is_rebalanced_and_judged_again_as_it_ends_up() {
    let checklist = checklist();
    // Over the ceiling; a limiter brings the peaks down and loudness up 2 dB.
    let whole = vec![Some(-14.), Some(0.), Some(-1.), Some(0.)];
    let live = Pretend::new(|gain| vec![Some(-12. + gain), Some(-3. + gain), Some(-1.), Some(0.)]);
    live.change("Loaded Limiter on Main", true);
    let excerpt = live.listen();
    let predicted = predict(&checklist, &whole, &whole, &excerpt.values);
    let gain = checklist.rebalance(&whole, &predicted).unwrap();
    let verdict = checklist.verdict(Some(PEAK), &whole, &checklist.at_level(&whole, &predicted, gain));
    assert!(verdict.kept, "{verdict:?}");
    let settled = rebalance(&live, &checklist, Some(PEAK), &whole, predicted, excerpt, gain, verdict).await;
    assert!(settled.verdict.kept, "{:?}", settled.verdict);
    assert_eq!(live.gain.get(), -2.);
    assert_eq!(settled.whole[LOUDNESS], Some(-14.));
    assert_eq!(settled.whole[PEAK], Some(-5.));
    assert_eq!((settled.listens, settled.rebalanced.as_deref()), (1, Some("Utility gain -2 dB")));
}

#[tokio::test]
async fn a_rebalance_that_clips_takes_the_round_back() {
    let checklist = checklist();
    // A cut that brightens the mix but takes 3 dB of loudness with it; bringing those 3 dB back clips downstream.
    let whole = vec![Some(-14.), Some(-3.), Some(-1.5), Some(0.)];
    let live = Pretend::new(|gain| vec![Some(-17. + gain), Some(-5.5 + gain), Some(-0.2), Some(if gain > 2.5 { 300. } else { 0. })]);
    live.change("EQ Eight · Low Shelf Gain 0 → −6 dB", true);
    let excerpt = live.listen();
    let predicted = predict(&checklist, &whole, &whole, &excerpt.values);
    let gain = checklist.rebalance(&whole, &predicted).unwrap();
    assert_eq!(gain, 3.);
    let verdict = checklist.verdict(Some(BRIGHTNESS), &whole, &checklist.at_level(&whole, &predicted, gain));
    assert!(verdict.kept, "before it's heard, the clipping can't be told: {verdict:?}");
    let settled = rebalance(&live, &checklist, Some(BRIGHTNESS), &whole, predicted, excerpt, gain, verdict).await;
    assert!(!settled.verdict.kept, "{:?}", settled.verdict);
    assert!(
        settled.verdict.why.contains("clipping") && settled.verdict.why.ends_with("once loudness was matched"),
        "{:?}",
        settled.verdict
    );
    // Taken back: the change and the rebalance both.
    let said = take_back(&live, &[]).await;
    assert!(said.starts_with("taken back: EQ Eight") && said.contains("Utility gain 3 dB"), "{said}");
    assert_eq!(live.gain.get(), 0.);
}

#[tokio::test]
async fn a_rebalance_nobody_heard_is_taken_back_and_the_round_isnt_kept() {
    let checklist = checklist();
    let whole = vec![Some(-14.), Some(0.), Some(-1.), Some(0.)];
    let live = Pretend::new(|gain| vec![Some(-12. + gain), Some(-3. + gain), Some(-1.), Some(0.)]);
    live.change("Loaded Limiter on Main", true);
    let excerpt = live.listen();
    let predicted = predict(&checklist, &whole, &whole, &excerpt.values);
    let gain = checklist.rebalance(&whole, &predicted).unwrap();
    let verdict = checklist.verdict(Some(PEAK), &whole, &checklist.at_level(&whole, &predicted, gain));
    live.unheard.set(1);
    let settled = rebalance(&live, &checklist, Some(PEAK), &whole, predicted, excerpt, gain, verdict).await;
    assert_eq!(live.gain.get(), 0., "the step nobody heard was taken back");
    assert!(settled.rebalanced.as_deref().is_some_and(|said| said.contains("couldn't hear the rebalance")), "{settled:?}");
    assert_eq!(settled.listens, 0);
    // Its loudness never got matched, so the round can't be judged fairly: not kept.
    assert!(
        !settled.verdict.kept && settled.verdict.why.contains("its loudness couldn't be matched (it's 2 dB louder"),
        "{:?}",
        settled.verdict
    );
}

#[tokio::test]
async fn a_device_live_wont_undo_goes_only_when_its_one_the_round_made() {
    // Main's chain (the run's); what the round made, by Live's identity for it.
    let stuck = |load: &str, made: &str, chain: Vec<Placed>| {
        let live = Pretend::new(|_| vec![]);
        live.made_one(load, false, Some(made));
        live.change("Compressor · Threshold −20 → −30 dB", true);
        *live.chain.borrow_mut() = chain;
        live
    };
    // The round's Compressor is on Main and Live won't undo it: it goes, the producer's Glue doesn't.
    let live = stuck(
        "Loaded Compressor on Main",
        "9",
        vec![device("1", "EQ Eight"), device("7", "Glue Compressor"), device("2", "Limiter"), device("9", "Compressor")],
    );
    let said = take_back(&live, &[]).await;
    assert!(said.contains("so Kumi removed Compressor instead"), "{said}");
    assert_eq!(*live.deleted.borrow(), ["device:9"]);
    // A mix run: the stuck EQ was loaded on Bass, and Main's new device is the producer's Glue. Nothing on Main goes.
    let live = stuck("Loaded EQ Eight on Bass", "5", vec![device("1", "EQ Eight"), device("2", "Limiter"), device("8", "Glue Compressor")]);
    let said = take_back(&live, &[]).await;
    assert!(said.ends_with("undo it yourself") && live.deleted.borrow().is_empty(), "{said}");
    // A bridge that doesn't say identities: nothing is removed.
    let live = Pretend::new(|_| vec![]);
    live.made_one("Loaded Compressor on Main", false, None);
    *live.chain.borrow_mut() = vec![device("9", "Compressor")];
    let said = take_back(&live, &[]).await;
    assert!(said.ends_with("undo it yourself") && live.deleted.borrow().is_empty(), "{said}");
    // A device Live doesn't say the identity of is never one.
    assert!(removable(&[device("", "Compressor")], &["".to_string()]).is_empty());
    // Nothing applied: nothing to take back.
    let live = Pretend::new(|_| vec![]);
    assert!(take_back(&live, &[]).await.starts_with("nothing in HISTORY"));
}

#[tokio::test]
async fn a_round_whose_loudness_cant_be_matched_isnt_kept() {
    let checklist = checklist();
    let whole = vec![Some(-14.), Some(0.), Some(-1.), Some(0.)];
    let setup = || {
        let live = Pretend::new(|gain| vec![Some(-12. + gain), Some(-3. + gain), Some(-1.), Some(0.)]);
        live.change("Loaded Limiter on Main", true);
        live
    };
    let judged = |live: &Pretend| {
        let excerpt = live.listen();
        let predicted = predict(&checklist, &whole, &whole, &excerpt.values);
        let gain = checklist.rebalance(&whole, &predicted).unwrap();
        let verdict = checklist.verdict(Some(PEAK), &whole, &checklist.at_level(&whole, &predicted, gain));
        (excerpt, predicted, gain, verdict)
    };
    // The gain stage can't be turned: 2 dB louder can't be judged against the louder take.
    let mut live = setup();
    live.stuck_gain = Some("the gain knob doesn't show dB".into());
    let (excerpt, predicted, gain, verdict) = judged(&live);
    let settled = rebalance(&live, &checklist, Some(PEAK), &whole, predicted, excerpt, gain, verdict).await;
    assert!(
        !settled.verdict.kept && settled.verdict.why.contains("its loudness couldn't be matched (it's 2 dB louder"),
        "{:?}",
        settled.verdict
    );
    // A step nobody heard that Live wouldn't take back: Live is in a state nobody heard.
    let mut live = setup();
    live.stuck_rebalance = true;
    live.unheard.set(1);
    let (excerpt, predicted, gain, verdict) = judged(&live);
    let settled = rebalance(&live, &checklist, Some(PEAK), &whole, predicted, excerpt, gain, verdict).await;
    assert!(!settled.verdict.kept && settled.verdict.why.contains("Live wouldn't take it back"), "{:?}", settled.verdict);
}

#[test]
fn the_checklists_own_goal_still_reads_the_way_it_did() {
    // A goal built from a listen: what it reads at once is all readable (nothing to drop) and has targets.
    let tone: Vec<f64> = (0..(6. * RATE) as usize).map(|n| 0.2 * (2. * std::f64::consts::PI * 440. * n as f64 / RATE).sin()).collect();
    let first = heard(&tone);
    let mut checklist = Checklist::new(&Goal { loudness: Some(-14.), ..Default::default() }, &first, &[]);
    let mut values = checklist.read(&first, None);
    assert!(checklist.drop_unreadable(&mut values).is_empty());
    assert!(checklist.has_targets());
}

#[test]
fn clipping_says_when() {
    // Two bursts over full scale, a second and three seconds in.
    let samples: Vec<f64> = (0..(4. * RATE) as usize)
        .map(|n| {
            let at = n as f64 / RATE;
            let level = if (1.0..1.2).contains(&at) || (3.0..3.1).contains(&at) { 1.3 } else { 0.3 };
            level * (2. * std::f64::consts::PI * 220. * at).sin()
        })
        .collect();
    let heard = heard(&samples);
    assert_eq!(heard.measures.clipped_at.len(), 2, "{:?}", heard.measures.clipped_at);
    assert!((heard.measures.clipped_at[0][0] - 1.).abs() < 0.05 && (heard.measures.clipped_at[1][0] - 3.).abs() < 0.05);
    let found = detect::peaks(&heard, None);
    let clipping = found.iter().find(|problem| problem.id == "clipping").unwrap();
    assert_eq!(clipping.at.len(), 2);
    assert!(clipping.what.contains("at 0:01, 0:03"), "{}", clipping.what);
}

/// An excerpt heard after a change, reading `after`.
fn heard_after(after: Vec<Option<f64>>) -> Listen {
    Listen { loudness: after[LOUDNESS], values: after, file: "after.wav".into(), start: 0. }
}

#[tokio::test]
async fn a_round_is_decided_whole_kept_and_rebalanced_or_taken_back() {
    let checklist = checklist();
    let whole = vec![Some(-14.), Some(0.), Some(-1.), Some(0.)];
    // Kept: a limiter brings the peaks down and loudness up 2 dB, and rebalancing brings it back.
    let live = Pretend::new(|gain| vec![Some(-12. + gain), Some(-3. + gain), Some(-1.), Some(0.)]);
    live.change("Loaded Limiter on Main", true);
    let after = live.listen();
    let decided = decide(&live, &checklist, Some(PEAK), &whole, &whole, after, None, 1, &[]).await;
    assert!(decided.verdict.kept, "{:?}", decided.verdict);
    assert_eq!((live.gain.get(), decided.whole[LOUDNESS], decided.listens), (-2., Some(-14.), 1));
    // Silence after the change: every reading lost, not kept, and taken back.
    let live = Pretend::new(|_| vec![None, None, None, None]);
    live.change("Utility · Mute on", true);
    let silent = heard_after(vec![None, None, None, None]);
    let decided = decide(&live, &checklist, Some(PEAK), &whole, &whole, silent, None, 0, &[]).await;
    assert!(!decided.verdict.kept && decided.verdict.why.contains("couldn't be read"), "{:?}", decided.verdict);
    assert!(decided.verdict.why.contains("taken back: Utility · Mute on"), "{:?}", decided.verdict);
    assert!(live.history.borrow().iter().all(|entry| !entry.applied));
    // The listening model hears it distorted both ways and prefers it before: not kept, whatever the meters say.
    let live = Pretend::new(|gain| vec![Some(-14. + gain), Some(-1.5 + gain), Some(-1.), Some(0.)]);
    live.change("Saturator · Drive 0 → 12 dB", true);
    let after = live.listen();
    let opinion = Opinion { closer: Some(Choice::Before), new_problems: vec!["distorted".into()], said: "first / second".into() };
    let decided = decide(&live, &checklist, Some(PEAK), &whole, &whole, after, Some(&opinion), 0, &[]).await;
    assert!(
        !decided.verdict.kept && decided.verdict.why.starts_with("the listening model heard it distorted both ways"),
        "{:?}",
        decided.verdict
    );
    // Three devices for a fifth of a step: the processing costs more than it earns.
    let live = Pretend::new(|gain| vec![Some(-14. + gain), Some(-1.04 + gain), Some(-1.), Some(0.)]);
    live.change("Loaded Compressor on Main", true);
    let after = live.listen();
    let edge = vec![Some(-14.), Some(-0.8), Some(-1.), Some(0.)];
    let decided = decide(&live, &checklist, Some(PEAK), &edge, &edge, after, None, 3, &[]).await;
    assert!(!decided.verdict.kept && decided.verdict.why.contains("less than the processing costs"), "{:?}", decided.verdict);
}
