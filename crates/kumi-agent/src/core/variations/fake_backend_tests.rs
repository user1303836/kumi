//! The variations tool against a fake DAW of sixteen steps, with no DAW at all: what core owns, the same for every backend
//! (numbering across makes, keep, change, drop and "rest", the checks and their warnings, the sets kept across a restart,
//! takes found again by their names, and when takes are rendered and drawn again).
use super::backend::{Rendered, Rewritten, Set, Source, Undroppable, VariationsBackend, Words};
use super::heard::Heard;
use super::tool::{names, Variations};
use super::{Material, Rng};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_types::contracts::{JsonObject, KernelTool};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

const STEPS: usize = 16;
const BEAT: [u8; STEPS] = [9, 0, 0, 5, 0, 0, 7, 0, 9, 0, 0, 5, 0, 0, 7, 0];

/// Sixteen steps, each a level 0 to 9 (0 plays nothing).
#[derive(Debug, Clone, PartialEq)]
struct Steps(Vec<u8>);

impl Material for Steps {
    fn distance(&self, other: &Self) -> f64 {
        self.0.iter().zip(&other.0).filter(|(a, b)| a != b).count() as f64 / STEPS as f64
    }

    fn silent(&self) -> bool {
        self.0.iter().all(|level| *level == 0)
    }
}

/// The fake DAW: its takes by id (name and steps), the changes it made, whether it plays, the renders it made and how many of
/// the next takes rendered clip. `generation` moves when it starts again: places from before are unknown then. `unreadable`: a
/// take whose reads fail.
#[derive(Default)]
struct Daw {
    takes: RefCell<BTreeMap<u64, (String, Steps)>>,
    next: Cell<u64>,
    changes: Cell<usize>,
    playing: Cell<bool>,
    /// The DAW can't say whether it plays (its focus read fails); it starts playing once it made this many renders (0: never).
    focus_fails: Cell<bool>,
    plays_after: Cell<usize>,
    renders: Cell<usize>,
    clips: Cell<usize>,
    /// Where the source peaks in a render, as a take that doesn't clip does (−6 dBFS by default), and where a take that clips
    /// peaks (0 dBFS by default).
    source_peak: Cell<Option<f64>>,
    clip_peak: Cell<Option<f64>>,
    /// The DAW can't turn a take down; what it was asked to turn down (dB) and the take it was.
    gainless: Cell<bool>,
    quieted: RefCell<Vec<(f64, Steps)>>,
    generation: Cell<u64>,
    unreadable: Cell<Option<u64>>,
    /// Kept sets' keys as their sources' places give them now (a position the producer moved).
    moved: RefCell<BTreeMap<String, String>>,
    /// Why a take stays out of a drop, by its id.
    undroppable: RefCell<BTreeMap<u64, Undroppable>>,
}

impl Daw {
    fn change(&self) -> String {
        self.changes.set(self.changes.get() + 1);
        format!("c{}", self.changes.get())
    }

    fn names(&self) -> Vec<String> {
        self.takes.borrow().values().map(|(name, _)| name.clone()).collect()
    }
}

struct Fake(Rc<Daw>);

static WORDS: Words = Words {
    app: "Fake",
    sources: "{steps: sixteen levels 0–9, name}",
    anchors: "steps every take keeps",
    ops: "flip {amount}: flips that share of the steps",
    lines: "{step, level}",
    places: "the end",
    place_names: &["end"],
    at: "nothing",
    options: "none",
    extract: None,
    hears: None,
    rewrites: None,
    drops: None,
    idioms: "",
    directions: &["a few flipped", "some flipped", "more flipped", "many flipped"],
    source_schema: r#"{"type": "object"}"#,
    op_schema: r#"{"type": "object"}"#,
    options_schema: r#"{"type": "object"}"#,
    at_schema: r#"{"type": "object"}"#,
};

fn id_of(row: &Value) -> u64 {
    row["id"].as_u64().unwrap_or(0)
}

#[async_trait(?Send)]
impl VariationsBackend for Fake {
    type Material = Steps;
    type Data = ();

    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn project_key(&self, _: &Signal) -> Option<String> {
        Some("song 1".into())
    }

    async fn source(&self, input: &JsonObject, _: &Signal) -> Result<Source<Steps, ()>, String> {
        let source = input.get("source").cloned().unwrap_or(Value::Null);
        let material = source["steps"].as_array().map(|steps| Steps(steps.iter().map(|level| level.as_u64().unwrap_or(0) as u8).collect()));
        let name = source["name"].as_str().unwrap_or("Beat");
        let key = source["key"].as_str().map_or_else(|| name.to_lowercase(), str::to_owned);
        Ok(Source { label: format!("steps of “{name}”"), short: name.into(), key, material, data: () })
    }

    async fn key_now(&self, source: &Source<Steps, ()>, _: &Signal) -> Option<String> {
        self.0.moved.borrow().get(&source.key).cloned()
    }

    fn direction_ops(&self, _: &Source<Steps, ()>, direction: &str) -> Vec<Value> {
        let amount = match direction {
            "a few flipped" => 0.15,
            "some flipped" => 0.2,
            "more flipped" => 0.25,
            _ => 0.3,
        };
        vec![json!({"op": "flip", "amount": amount})]
    }

    fn check_ops(&self, _: &Source<Steps, ()>, ops: &[Value]) -> Result<(), String> {
        match ops.iter().find(|op| op["op"] != "flip") {
            Some(op) => Err(format!("{op}: op is flip")),
            None => Ok(()),
        }
    }

    fn apply_ops(&self, ops: &[Value], factor: f64, take: &mut Steps, _: &Steps, rng: &mut Rng) -> Result<Vec<String>, String> {
        let mut said = Vec::new();
        for op in ops {
            let share = op["amount"].as_f64().unwrap_or(0.2) * factor;
            let at = rng.choose(STEPS, ((share * STEPS as f64).round() as usize).clamp(1, STEPS));
            for step in &at {
                take.0[*step] = if take.0[*step] == 0 { 5 } else { 0 };
            }
            said.push(format!("flipped steps {at:?}"));
        }
        Ok(said)
    }

    fn written(&self, _: &Set<Self>, lines: &[Value], _: &Steps) -> Result<Steps, String> {
        let mut steps = Steps(vec![0; STEPS]);
        for line in lines {
            let step = line["step"].as_u64().filter(|step| (*step as usize) < STEPS).ok_or("step is 0 to 15")? as usize;
            steps.0[step] = line["level"].as_u64().unwrap_or(5).min(9) as u8;
        }
        Ok(steps)
    }

    fn empty(&self, _: &Set<Self>) -> Steps {
        Steps(vec![0; STEPS])
    }

    fn anchors_kept(&self, _: &Steps, _: &Steps) -> Result<(), String> {
        Ok(())
    }

    fn judge(&self, _: Option<&Steps>, _: &Steps, _: &[Value], _: f64) -> Vec<String> {
        vec![]
    }

    fn digest(&self, material: &Steps) -> String {
        format!("{:?}", material.0)
    }

    /// Each step that plays a level lower (as low as 1).
    fn quieter(&self, take: &Steps, db: f64) -> Result<Steps, String> {
        if self.0.gainless.get() {
            return Err("Fake can't turn a take down".into());
        }
        self.0.quieted.borrow_mut().push((db, take.clone()));
        Ok(Steps(take.0.iter().map(|level| if *level > 1 { level - 1 } else { *level }).collect()))
    }

    async fn write_new(
        &self,
        set: &Set<Self>,
        number: usize,
        material: &Steps,
        _: &Signal,
    ) -> Result<(String, Value, String), (String, bool)> {
        let daw = &self.0;
        let id = daw.next.get() + 1;
        daw.next.set(id);
        let [name, _] = names(number, &set.source.short);
        daw.takes.borrow_mut().insert(id, (name, material.clone()));
        Ok((daw.change(), json!({"id": id, "gen": daw.generation.get()}), format!("take:{id}")))
    }

    async fn find(&self, set: &mut Set<Self>, numbers: &[usize], _: &Signal) -> Result<Vec<Value>, String> {
        let daw = &self.0;
        let takes = daw.takes.borrow();
        let short = set.source.short.clone();
        let mut rows = Vec::new();
        for number in numbers {
            let Some(take) = set.take_mut(*number) else {
                rows.push(Value::Null);
                continue;
            };
            let row = if take.place["gen"].as_u64() == Some(daw.generation.get()) {
                let id = take.place["id"].as_u64().unwrap_or(0);
                if takes.contains_key(&id) {
                    json!({"ref": format!("take:{id}"), "id": id})
                } else {
                    json!({"gone": true})
                }
            } else {
                // (a place from before the DAW started again: the take by the name Kumi gave it)
                let wanted = names(*number, &short);
                match takes.iter().find(|(_, (name, _))| wanted.contains(name)) {
                    Some((id, _)) => {
                        take.place = json!({"id": id, "gen": daw.generation.get()});
                        json!({"ref": format!("take:{id}"), "id": id})
                    }
                    None => json!({}),
                }
            };
            rows.push(row);
        }
        Ok(rows)
    }

    async fn current(&self, _: &Set<Self>, row: &Value, _: &Signal) -> Result<Steps, String> {
        if self.0.unreadable.get() == Some(id_of(row)) {
            return Err("the DAW was busy".into());
        }
        self.0.takes.borrow().get(&id_of(row)).map(|(_, steps)| steps.clone()).ok_or("gone".into())
    }

    async fn rewrite(&self, _: &Set<Self>, row: &Value, _: &Steps, after: &Steps, _: &Signal) -> Result<Rewritten<Steps>, (String, bool)> {
        let daw = &self.0;
        match daw.takes.borrow_mut().get_mut(&id_of(row)) {
            Some((_, steps)) => *steps = after.clone(),
            None => return Err(("gone".into(), false)),
        }
        Ok((daw.change(), None))
    }

    async fn mark_kept(&self, set: &Set<Self>, number: usize, row: &Value, _: &Signal) -> Result<Option<String>, String> {
        let daw = &self.0;
        let [name, kept] = names(number, &set.source.short);
        {
            let mut takes = daw.takes.borrow_mut();
            let Some((now, _)) = takes.get_mut(&id_of(row)) else { return Err("gone".into()) };
            if *now != name {
                return Ok(None);
            }
            *now = kept;
        }
        Ok(Some(daw.change()))
    }

    async fn place(&self, _: &mut Set<Self>, _: usize, _: &Value, _: &JsonObject, _: &Signal) -> Result<Value, String> {
        Ok(json!("at the end"))
    }

    async fn undroppable(&self, _: &Set<Self>, _: usize, row: &Value, _: &Signal) -> Option<Undroppable> {
        self.0.undroppable.borrow().get(&id_of(row)).cloned()
    }

    async fn drop(&self, _: &Set<Self>, rows: &[(usize, Value)], _: &Signal) -> Result<(String, Value), String> {
        let daw = &self.0;
        for (_, row) in rows {
            daw.takes.borrow_mut().remove(&id_of(row));
        }
        Ok((daw.change(), json!({"display": {"title": format!("Deleted {} takes", rows.len())}})))
    }

    fn cant_render(&self) -> Option<String> {
        None
    }

    async fn playing(&self, _: &Signal) -> Option<bool> {
        let daw = &self.0;
        let started = daw.plays_after.get() > 0 && daw.renders.get() >= daw.plays_after.get();
        (!daw.focus_fails.get()).then_some(daw.playing.get() || started)
    }

    async fn render(&self, set: &mut Set<Self>, numbers: &[usize], with_source: bool, _: &Signal) -> Result<Rendered, String> {
        let daw = &self.0;
        daw.renders.set(daw.renders.get() + 1);
        let heard = |steps: &Steps, peak: f64| Heard {
            peak,
            level: -18.,
            onsets: steps.0.iter().enumerate().filter(|(_, level)| **level > 0).map(|(step, _)| step as f64 * 0.125).collect(),
        };
        let peak = daw.source_peak.get().unwrap_or(-6.);
        let source = set.source.material.as_ref().filter(|_| with_source).map(|steps| heard(steps, peak));
        let mut takes = BTreeMap::new();
        for number in numbers {
            let Some(take) = set.take(*number) else { continue };
            let clips = daw.clips.get() > 0;
            if clips {
                daw.clips.set(daw.clips.get() - 1);
            }
            takes.insert(*number, Ok(heard(&take.material, if clips { daw.clip_peak.get().unwrap_or(0.) } else { peak })));
        }
        Ok((source, takes))
    }

    fn short(&self, long: &str) -> String {
        long.into()
    }

    fn end_note(&self, _: &Set<Self>) -> Option<String> {
        None
    }

    async fn extract(&self, _: &JsonObject, _: &Signal) -> Result<Value, String> {
        Err("no extract".into())
    }

    fn material_json(&self, material: &Steps) -> Value {
        json!(material.0)
    }

    fn material_of(&self, value: &Value) -> Option<Steps> {
        Some(Steps(value.as_array()?.iter().map(|level| level.as_u64().unwrap_or(0) as u8).collect()))
    }

    fn data_json(&self, _: &()) -> Value {
        Value::Null
    }

    fn data_of(&self, _: &Value) -> Option<()> {
        Some(())
    }
}

/// Kumi (another run of it, when called again) with the fake DAW and Kumi's folder `home`.
fn kumi(daw: &Rc<Daw>, home: &std::path::Path) -> Rc<dyn KernelTool> {
    Variations::new(Fake(daw.clone()), home.into()).tool()
}

async fn run(tool: &Rc<dyn KernelTool>, value: Value) -> Result<Value, String> {
    let done = tool.execute(value.as_object().cloned().unwrap(), Signal::new()).await.unwrap();
    if done.is_error {
        Err(done.text)
    } else {
        Ok(serde_json::from_str(&done.text).unwrap())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn takes_are_numbered_kept_changed_and_dropped_by_core_alone() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    let made = run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 4, "seed": 1})).await.unwrap();
    let takes = made["takes"].as_array().unwrap();
    assert_eq!(takes.len(), 4, "{made}");
    assert!(takes.iter().all(|take| take["line"].as_str().unwrap().contains(" \u{b7} heard: peak \u{2212}6.0 dBFS")), "{made}");
    assert_eq!(daw.changes.get(), 4, "each take its own change");
    assert_eq!(daw.renders.get(), 1, "one render for every take");
    assert_eq!(daw.names(), ["V1 \u{b7} Beat", "V2 \u{b7} Beat", "V3 \u{b7} Beat", "V4 \u{b7} Beat"]);

    let kept = run(&tool, json!({"action": "keep", "names": ["V1", "V3"]})).await.unwrap();
    assert_eq!(kept["rest"], json!(["V2", "V4"]), "{kept}");
    assert_eq!(daw.names(), ["V1 \u{b7} Beat \u{b7} kept", "V2 \u{b7} Beat", "V3 \u{b7} Beat \u{b7} kept", "V4 \u{b7} Beat"]);
    let before = daw.takes.borrow().clone();
    let changed = run(&tool, json!({"action": "change", "names": ["rest"]})).await.unwrap();
    assert_eq!(changed["changed"].as_array().unwrap().len(), 2, "{changed}");
    let after = daw.takes.borrow().clone();
    assert_eq!((&after[&1], &after[&3]), (&before[&1], &before[&3]), "the kept takes are as they were");
    assert!(after[&2] != before[&2] && after[&4] != before[&4], "the rest changed");
    assert!(run(&tool, json!({"action": "change", "names": ["V1"]})).await.unwrap_err().contains("V1 is kept"));
    let changes = daw.changes.get();
    let dropped = run(&tool, json!({"action": "drop"})).await.unwrap();
    assert_eq!(dropped["dropped"], json!(["V2", "V4"]), "{dropped}");
    assert_eq!(daw.changes.get(), changes + 1, "the rest go in one change");
    assert!(run(&tool, json!({"action": "drop"})).await.unwrap_err().contains("names no take"), "\"rest\" never reaches a kept take");

    // (the same source again goes on numbering; another source starts set 2, and set 1 is still there by number and by name)
    let more = run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 1, "seed": 2})).await.unwrap();
    assert_eq!((&more["set"], &more["takes"][0]["take"]), (&json!(1), &json!("V5")), "{more}");
    let other = run(&tool, json!({"action": "make", "source": {"steps": BEAT, "name": "Fill"}, "count": 1, "seed": 3})).await.unwrap();
    assert_eq!((&other["set"], &other["takes"][0]["take"]), (&json!(2), &json!("V1")), "{other}");
    assert_eq!(run(&tool, json!({"action": "list", "set": 1})).await.unwrap()["takes"].as_array().unwrap().len(), 3, "V1, V3, V5");
    assert_eq!(run(&tool, json!({"action": "list", "set": "beat"})).await.unwrap()["set"], 1);
}

#[tokio::test(flavor = "current_thread")]
async fn kept_takes_outlive_a_restart_and_takes_are_found_by_the_names_kumi_gave_them() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 3, "seed": 4, "listen": false})).await.unwrap();
    run(&tool, json!({"action": "keep", "names": ["V2"]})).await.unwrap();
    // (the producer renames V3: Kumi can't tell it's V3 once its place is lost)
    daw.takes.borrow_mut().get_mut(&3).unwrap().0 = "my fill".into();
    drop(tool);

    // (Kumi again, and the DAW started again: its places from before are unknown)
    daw.generation.set(1);
    let again = kumi(&daw, home.path());
    let listed = run(&again, json!({"action": "list"})).await.unwrap();
    assert!(listed["takes"][1].as_str().unwrap().ends_with("(kept)"), "{listed}");
    let dropped = run(&again, json!({"action": "drop"})).await.unwrap();
    assert_eq!(dropped["dropped"], json!(["V1"]), "found by its name: {dropped}");
    assert!(dropped["skipped"][0].as_str().unwrap().contains("can't find V3"), "{dropped}");
    assert_eq!(daw.names(), ["V2 \u{b7} Beat \u{b7} kept", "my fill"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_take_the_producer_edited_or_deleted_is_left_out_of_a_drop() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 3, "seed": 5, "listen": false})).await.unwrap();
    daw.takes.borrow_mut().get_mut(&1).unwrap().1 .0[15] = 9;
    daw.takes.borrow_mut().remove(&2);
    let dropped = run(&tool, json!({"action": "drop"})).await.unwrap();
    assert_eq!(dropped["dropped"], json!(["V3"]), "{dropped}");
    assert!(dropped["skipped"][0].as_str().unwrap().contains("V1 was edited in Fake"), "{dropped}");
    assert!(dropped["skipped"][1].as_str().unwrap().contains("V2 isn't in the Fake any more"), "{dropped}");
    assert_eq!(daw.names(), ["V1 \u{b7} Beat"]);
    // (named, an edited take goes all the same, and the answer says it was edited)
    let named = run(&tool, json!({"action": "drop", "names": ["V1"]})).await.unwrap();
    assert_eq!((named["dropped"].clone(), named["edited"].clone()), (json!(["V1"]), json!(["V1"])), "{named}");
    assert!(daw.names().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_named_take_goes_past_an_edit_the_backend_sees_but_never_past_its_use_elsewhere() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 3, "seed": 5, "listen": false})).await.unwrap();
    daw.undroppable.borrow_mut().insert(1, Undroppable::Edited("was edited on its other tracks".into()));
    daw.undroppable.borrow_mut().insert(2, Undroppable::InUse("plays in 2 slots now".into()));
    let rest = run(&tool, json!({"action": "drop"})).await.unwrap();
    assert_eq!(rest["dropped"], json!(["V3"]), "{rest}");
    assert!(rest["skipped"][0].as_str().unwrap().contains("name it to drop it anyway"), "{rest}");
    let named = run(&tool, json!({"action": "drop", "names": ["V1", "V2"]})).await.unwrap();
    assert_eq!((named["dropped"].clone(), named["edited"].clone()), (json!(["V1"]), json!(["V1"])), "{named}");
    assert!(named["skipped"][0].as_str().unwrap().starts_with("V2 plays in 2 slots now"), "{named}");
    assert_eq!(daw.names(), ["V2 \u{b7} Beat"]);
}

#[tokio::test(flavor = "current_thread")]
async fn lines_the_model_writes_are_written_with_a_warning_and_operator_takes_meet_the_floors() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    let same: Vec<Value> =
        BEAT.iter().enumerate().filter(|(_, level)| **level > 0).map(|(step, level)| json!({"step": step, "level": level})).collect();
    let made = run(
        &tool,
        json!({"action": "make", "source": {"steps": BEAT}, "minDistance": 0.9, "listen": false, "takes": [{"direction": "as I wrote it", "lines": same}]}),
    )
    .await
    .unwrap();
    assert!(made["takes"][0]["line"].as_str().unwrap().contains("written as asked, though"), "{made}");
    assert_eq!(daw.changes.get(), 1);
    let refused = run(
        &tool,
        json!({"action": "make", "source": {"steps": BEAT}, "minDistance": 0.9, "listen": false, "takes": [{"ops": [{"op": "flip", "amount": 0.01}]}]}),
    )
    .await
    .unwrap();
    assert!(
        refused["takes"].as_array().unwrap().is_empty() && refused["skipped"][0].as_str().unwrap().contains("wasn't made"),
        "{refused}"
    );
    assert_eq!(daw.changes.get(), 1, "nothing written");
}

#[tokio::test(flavor = "current_thread")]
async fn nothing_renders_while_the_daw_plays_and_a_take_that_clips_is_drawn_again_where_the_daw_cant_turn_it_down() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    daw.playing.set(true);
    let made = run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 1, "seed": 6})).await.unwrap();
    assert!(made["heard"][0].as_str().unwrap().starts_with("Fake is playing"), "{made}");
    assert!(!made["takes"][0]["line"].as_str().unwrap().contains("heard:"), "{made}");
    assert_eq!(daw.renders.get(), 0, "a render would stop the playback");

    daw.playing.set(false);
    daw.gainless.set(true);
    daw.clips.set(1);
    let made = run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 1, "seed": 7})).await.unwrap();
    let line = made["takes"][0]["line"].as_str().unwrap();
    assert!(line.contains("(drawn again after the render: it clips (peak 0.0 dBFS))"), "{made}");
    assert!(line.contains(" \u{b7} heard: peak \u{2212}6.0 dBFS"), "heard again, fine now: {made}");
    assert_eq!(daw.changes.get(), 3, "the first take, the second, and its rewrite");
    assert_eq!(daw.renders.get(), 2, "the take, then the take drawn again");

    // (lines the model wrote aren't drawn again: the take says why it wasn't turned down either)
    daw.clips.set(1);
    let written =
        run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "takes": [{"lines": [{"step": 2, "level": 9}]}]})).await.unwrap();
    assert!(
        written["takes"][0]["line"]
            .as_str()
            .unwrap()
            .ends_with(" · heard: it clips (peak 0.0 dBFS) (not turned down: Fake can't turn a take down)"),
        "{written}"
    );
}

/// (#407, P31) Takes of a hot source that clip where the source doesn't are turned down to the source's peak, not drawn again
/// (re-rolls can't cure a hot source): each as drawn but its level, in one more change, heard again, its line saying so. They
/// stay Kumi's: a change takes one, and a drop of the rest takes the other, neither taken for edited since Kumi wrote it.
#[tokio::test(flavor = "current_thread")]
async fn takes_that_clip_where_a_hot_source_doesnt_are_turned_down_to_its_peak_and_stay_kumis() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    daw.source_peak.set(Some(-0.3));
    daw.clip_peak.set(Some(1.1));
    daw.clips.set(2);
    let made = run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 2, "seed": 7})).await.unwrap();
    for (take, change) in made["takes"].as_array().unwrap().iter().zip(["c3", "c4"]) {
        let line = take["line"].as_str().unwrap();
        let said = format!(
            " · turned down 1.4 dB to the source's peak ({change}): its render clipped (peak 1.1 dBFS) · heard: peak \u{2212}0.3 dBFS"
        );
        assert!(line.contains(&said), "{line}");
        assert!(!line.contains("drawn again"), "{line}");
    }
    let quieted = daw.quieted.borrow().clone();
    assert_eq!(quieted.iter().map(|(db, _)| *db).collect::<Vec<_>>(), [1.4, 1.4]);
    for (id, (_, drawn)) in [1, 2].into_iter().zip(&quieted) {
        let lower: Vec<u8> = drawn.0.iter().map(|level| if *level > 1 { level - 1 } else { *level }).collect();
        assert_eq!(daw.takes.borrow()[&id].1 .0, lower, "V{id} as drawn, a level lower");
    }
    assert_eq!(daw.changes.get(), 4, "two takes, each turned down");
    assert_eq!(daw.renders.get(), 2, "the takes, then the takes turned down");

    let changed = run(&tool, json!({"action": "change", "names": ["V1"], "listen": false})).await.unwrap();
    assert_eq!(changed["changed"].as_array().unwrap().len(), 1, "{changed}");
    let dropped = run(&tool, json!({"action": "drop"})).await.unwrap();
    assert_eq!(dropped["dropped"], json!(["V1", "V2"]), "{dropped}");
    assert!(dropped["skipped"].as_array().unwrap().is_empty() && dropped.get("edited").is_none(), "{dropped}");

    // (lines the model wrote are turned down too: their level, not their lines)
    daw.clips.set(1);
    let written =
        run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "takes": [{"lines": [{"step": 2, "level": 9}]}]})).await.unwrap();
    assert!(written["takes"][0]["line"].as_str().unwrap().contains(" · turned down 1.4 dB to the source's peak"), "{written}");
}

/// (#400 review notes) Numbers once given to takes of a name never come back, though Kumi let go of the set that gave them (it
/// holds 8): the takes may still be in the project.
#[tokio::test(flavor = "current_thread")]
async fn a_takes_number_never_comes_back_after_its_set_is_let_go() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    let first = run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 2, "seed": 6, "listen": false})).await.unwrap();
    assert_eq!(first["takes"][1]["take"], "V2", "{first}");
    for k in 0..8 {
        let name = format!("Other {k}");
        run(&tool, json!({"action": "make", "source": {"steps": BEAT, "name": name}, "count": 1, "seed": 7, "listen": false}))
            .await
            .unwrap();
    }
    let again =
        run(&tool, json!({"action": "make", "source": {"steps": BEAT, "key": "beat again"}, "count": 1, "seed": 8, "listen": false}))
            .await
            .unwrap();
    assert_eq!(again["takes"][0]["take"], "V3", "after V2, whose set Kumi let go of: {again}");
}

/// (#400 review notes) A set whose source moved (its position in its key) goes on when a make names that source where it is now,
/// and a source now at its old position starts a set of its own.
#[tokio::test(flavor = "current_thread")]
async fn a_set_goes_on_from_where_its_source_moved_to() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    let first = run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 1, "seed": 6, "listen": false})).await.unwrap();
    assert_eq!(first["takes"][0]["take"], "V1", "{first}");
    daw.moved.borrow_mut().insert("beat".into(), "beat@2".into());
    let on = run(&tool, json!({"action": "make", "source": {"steps": BEAT, "key": "beat@2"}, "count": 1, "seed": 7, "listen": false}))
        .await
        .unwrap();
    assert_eq!(on["takes"][0]["take"], "V2", "the same set goes on: {on}");
    let other = run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 1, "seed": 8, "listen": false})).await.unwrap();
    assert_eq!(other["takes"][0]["take"], "V3", "a set of its own, its numbers after the other's: {other}");
    assert_ne!(other["set"], on["set"], "{other} {on}");
}

/// (#400 review notes) Nothing renders when the DAW can't say whether it plays, and a take turned down after its render isn't
/// rendered again once the producer pressed play.
#[tokio::test(flavor = "current_thread")]
async fn nothing_renders_when_the_daw_cant_say_it_plays_or_starts_playing_between_rounds() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    daw.focus_fails.set(true);
    let made = run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 1, "seed": 6})).await.unwrap();
    assert!(made["heard"][0].as_str().unwrap().starts_with("Kumi couldn't tell whether Fake is playing"), "{made}");
    assert_eq!(daw.renders.get(), 0);

    daw.focus_fails.set(false);
    daw.clips.set(1);
    daw.plays_after.set(1);
    let made = run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 1, "seed": 7})).await.unwrap();
    assert!(made["heard"].to_string().contains("Fake plays now (or couldn't say whether it does)"), "{made}");
    assert!(made["takes"][0]["line"].as_str().unwrap().contains("turned down 6.0 dB to the source's peak"), "{made}");
    assert_eq!(daw.renders.get(), 1, "the take turned down isn't rendered while the DAW plays");
}

/// (review W9) A make on the same source goes on with the source as it is now: a take is drawn from the producer's edit.
#[tokio::test(flavor = "current_thread")]
async fn a_make_on_the_same_source_draws_from_the_source_as_it_is_now() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 1, "seed": 8, "listen": false})).await.unwrap();
    let edited: Vec<u8> = BEAT.iter().map(|level| if *level == 0 { 3 } else { 0 }).collect();
    let more = run(&tool, json!({"action": "make", "source": {"steps": edited}, "count": 1, "seed": 9, "listen": false})).await.unwrap();
    assert_eq!((&more["set"], &more["takes"][0]["take"]), (&json!(1), &json!("V2")), "the same set: {more}");
    let take = daw.takes.borrow()[&2].1.clone();
    assert!(take.distance(&Steps(edited.clone())) <= 0.5, "drawn from the edit, not the source as it was: {take:?}");
}

/// (review W11) A take that can't be read in a change is left and said; the takes rewritten before it stay Kumi's.
#[tokio::test(flavor = "current_thread")]
async fn a_take_that_cant_be_read_in_a_change_is_left_and_the_others_stay_told() {
    let home = tempfile::tempdir().unwrap();
    let daw = Rc::new(Daw::default());
    let tool = kumi(&daw, home.path());
    run(&tool, json!({"action": "make", "source": {"steps": BEAT}, "count": 4, "seed": 10, "listen": false})).await.unwrap();
    daw.unreadable.set(Some(3));
    let changed = run(&tool, json!({"action": "change", "names": ["rest"], "listen": false})).await.unwrap();
    assert_eq!(changed["changed"].as_array().unwrap().len(), 3, "{changed}");
    assert!(changed["skipped"][0].as_str().unwrap().contains("V3 wasn't changed: Kumi couldn't read it"), "{changed}");
    daw.unreadable.set(None);
    let dropped = run(&tool, json!({"action": "drop"})).await.unwrap();
    assert_eq!(dropped["dropped"], json!(["V1", "V2", "V3", "V4"]), "none of them taken for edited: {dropped}");
}
