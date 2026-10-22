//! The jam tool against a fake DAW (no DAW), on R35's jam: two parts from before, then six recorded (near copies, empties, an
//! exact copy, a fill). The look proposes only within the jam; the fix applies what's accepted, dropping slots last and highest
//! first; quantize moves notes by a strength.
use super::backend::{Edit, JamBackend, Words};
use super::tool::Jam;
use super::{Hit, Part, TrackPart};
use crate::core::contracts::KernelTool;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

static WORDS: Words = Words { app: "Fake", part: "pattern", slot: "slot", alias: "an alias" };

#[derive(Default)]
struct Fake {
    parts: Vec<Part>,
    done: RefCell<Vec<String>>,
}

#[async_trait(?Send)]
impl JamBackend for Rc<Fake> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn parts(&self, _tracks: &[String], _signal: &Signal) -> Result<Vec<Part>, String> {
        Ok(self.parts.clone())
    }

    fn note_name(&self, note: i64) -> String {
        format!("n{note}")
    }

    async fn rewrite(&self, part: &Part, track: &TrackPart, edits: &[Edit], _signal: &Signal) -> Result<(Value, Vec<String>), String> {
        let words: Vec<String> = edits
            .iter()
            .map(|edit| match edit {
                Edit::Remove(hit) => format!("-{}@{}", hit.note, hit.at),
                Edit::Move(hit, to) => format!("{}:{}>{}", hit.note, hit.at, to),
            })
            .collect();
        self.done.borrow_mut().push(format!("rewrite {} {} {}", part.reference, track.track, words.join(" ")));
        Ok((json!({"change": "c"}), vec![]))
    }

    async fn alias(&self, part: &str, track: &str, of: &str, _signal: &Signal) -> Result<Value, String> {
        self.done.borrow_mut().push(format!("alias {part} {track} {of}"));
        Ok(json!({"change": "c"}))
    }

    async fn replay(&self, slot: &str, of: &str, _signal: &Signal) -> Result<Value, String> {
        self.done.borrow_mut().push(format!("replay {slot} {of}"));
        Ok(json!({"change": "c"}))
    }

    async fn drop_slot(&self, slot: &str, _signal: &Signal) -> Result<Value, String> {
        self.done.borrow_mut().push(format!("drop {slot}"));
        Ok(json!({"change": "c"}))
    }
}

fn hit(at: f64, note: i64) -> Hit {
    Hit { at, note, velocity: 0.8, line: at.floor() as u32, col: 1 }
}

fn track(reference: &str, name: &str, hits: Vec<Hit>) -> TrackPart {
    let digest = hits.iter().map(|h| format!("{}:{}", h.at, h.note)).collect::<Vec<_>>().join(",");
    TrackPart { track: reference.into(), name: name.into(), digest, empty: hits.is_empty(), alias: false, hits }
}

fn part(n: usize, keys: Vec<Hit>, drums: Vec<Hit>) -> Part {
    Part {
        reference: format!("p{n}"),
        name: format!("Pattern {n:02}"),
        slots: vec![(n, format!("s{n}"))],
        lpb: 4,
        tracks: vec![track("t1", "Keys", keys), track("t2", "Drums", drums)],
    }
}

/// R35's song: patterns 1 and 2 from before; 3 and 5 copies of 1 with a note more on Keys; 4 and 7 empty; 6 the same as 1;
/// 8 a fill.
fn r35() -> Rc<Fake> {
    let keys = |note: i64| (0..8).map(|k| hit(k as f64 * 8., note)).collect::<Vec<_>>();
    let drums = || (0..16).map(|k| hit(k as f64 * 4., 36)).collect::<Vec<_>>();
    let with = |extra: f64| {
        let mut list = keys(60);
        list.push(hit(extra, 67));
        list.sort_by(|a, b| a.at.total_cmp(&b.at));
        list
    };
    Rc::new(Fake {
        parts: vec![
            part(1, keys(60), drums()),
            part(2, keys(64), drums()),
            part(3, with(12.), drums()),
            part(4, vec![], vec![]),
            part(5, with(20.), drums()),
            part(6, keys(60), drums()),
            part(7, vec![], vec![]),
            part(8, vec![], (48..64).map(|l| hit(l as f64, 36)).collect()),
        ],
        ..Fake::default()
    })
}

async fn run(tool: &Rc<dyn KernelTool>, input: Value) -> Result<Value, String> {
    let done = tool.execute(input.as_object().cloned().unwrap(), Signal::new()).await.unwrap();
    if done.is_error {
        return Err(done.text);
    }
    Ok(serde_json::from_str(&done.text).unwrap())
}

#[tokio::test(flavor = "current_thread")]
async fn r35s_jam_is_told_as_whats_new_and_cleaned_up_without_touching_what_came_before() {
    let fake = r35();
    let tool = Jam::new(fake.clone()).tool();
    assert!(run(&tool, json!({"action": "fix", "ids": ["all"]})).await.unwrap_err().starts_with("look first"));
    let jam: Vec<&str> = vec!["s3", "s4", "s5", "s6", "s7", "s8"];
    let looked = run(&tool, json!({"patterns": jam})).await.unwrap();
    assert_eq!(looked["jam"], "6 patterns, slots 03 to 08");
    let says: Vec<&str> = looked["proposals"].as_array().unwrap().iter().map(|p| p["says"].as_str().unwrap()).collect();
    assert!(says.iter().any(|s| s.starts_with("slot 04 plays Pattern 04, which is empty")), "{says:?}");
    assert!(says.iter().any(|s| s.starts_with("slot 06 plays Pattern 06, the same as Pattern 01")), "{says:?}");
    assert!(says.iter().any(|s| s.contains("\u{201c}Drums\u{201d} in Pattern 03 is the same as in Pattern 01")), "{says:?}");
    assert!(looked["said"].to_string().contains("Pattern 03 (slot 03) is Pattern 01 with 1 note more on \u{201c}Keys\u{201d}"), "{looked}");
    assert!(!says.iter().any(|s| s.contains("Pattern 02") && !s.contains("Pattern 01")), "nothing proposed of what came before");
    let fixed = run(&tool, json!({"action": "fix", "ids": ["all"]})).await.unwrap();
    let done = fake.done.borrow().clone();
    assert!(done.contains(&"replay s6 p1".to_string()), "{done:?}");
    let drops: Vec<&String> = done.iter().filter(|d| d.starts_with("drop")).collect();
    assert_eq!(drops, ["drop s7", "drop s4"], "the highest first");
    assert!(done.iter().position(|d| d.starts_with("drop")) > done.iter().position(|d| d.starts_with("replay")), "{done:?}");
    assert!(!done.iter().any(|d| d.contains(" p1 ") && d.starts_with("rewrite") || d.contains(" p2 ")), "{done:?}");
    assert!(fixed["next"].as_str().unwrap().starts_with("look again"));
    // The look is spent: a second fix asks for another.
    assert!(run(&tool, json!({"action": "fix", "ids": ["all"]})).await.unwrap_err().starts_with("look first"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_mistake_is_fixed_by_its_id_and_quantize_moves_notes_by_a_strength() {
    // (a swing on the offbeats, one note half a line late on line 4)
    let drums = vec![hit(0., 36), hit(2.25, 42), hit(4.5, 38), hit(6.25, 42), hit(8., 36), hit(10.25, 42), hit(12., 38), hit(14.25, 42)];
    let fake = Rc::new(Fake { parts: vec![part(1, vec![hit(0.3, 60)], drums)], ..Fake::default() });
    let tool = Jam::new(fake.clone()).tool();
    let looked = run(&tool, json!({})).await.unwrap();
    let mistake = looked["proposals"].as_array().unwrap().iter().find(|p| p["says"].as_str().unwrap().contains("off the grid")).unwrap();
    assert!(looked["said"].to_string().contains("has a feel (line 2 of each beat +0.25)"), "{looked}");
    run(&tool, json!({"action": "fix", "ids": [mistake["id"]]})).await.unwrap();
    assert_eq!(fake.done.borrow().last().unwrap(), "rewrite p1 t2 38:4.5>4", "to the grid");
    // Quantize at half strength, the feel kept: the keys' late note moves halfway to line 0.
    run(&tool, json!({})).await.unwrap();
    run(&tool, json!({"action": "fix", "quantize": {"strength": 0.5, "tracks": ["t1"]}})).await.unwrap();
    assert_eq!(fake.done.borrow().last().unwrap(), "rewrite p1 t1 60:0.3>0.15");
}
