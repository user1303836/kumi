//! The jam tool, the same in every DAW: look sets what was recorded against what came before and proposes the clean-up, each
//! proposal with an id; fix applies the ones the producer accepts (and quantizes, by a strength), never touching a part outside
//! the jam. The DAW's backend reads the parts and writes the fixes.
use super::backend::{Edit, JamBackend};
use super::{feel, mistakes, quantized, Hit, Mistake, Part};
use crate::core::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use crate::core::errors::RuntimeError;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

pub const TOOL: &str = "jam";

/// A near copy differs by at most this many notes.
const NEAR: usize = 3;

/// What a look proposes.
#[derive(Debug, Clone)]
enum Proposal {
    /// A slot playing an empty part: dropped.
    Empty { slot: String, number: usize },
    /// A slot playing a part the same as an earlier one: it plays that one instead.
    Same { slot: String, of: String },
    /// A part's track the same as an earlier part's: an alias of it.
    Repeat { part: String, track: String, of: String },
    /// A note that reads as a mistake: taken away, or moved to the grid (with its track's feel).
    Mistake { part: String, track: String, mistake: Mistake, to: f64 },
}

/// The last look: the parts as read, the jam's parts (by ref), and the proposals by id.
struct Looked {
    parts: Vec<Part>,
    jam: Vec<String>,
    feel: BTreeMap<(String, String), BTreeMap<u32, f64>>,
    proposals: Vec<(String, Proposal)>,
}

pub struct Jam<B: JamBackend> {
    backend: B,
    looked: RefCell<Option<Looked>>,
    description: String,
}

impl<B: JamBackend + 'static> Jam<B> {
    pub fn new(backend: B) -> Rc<Self> {
        let description = description(backend.words());
        Rc::new(Jam { backend, looked: RefCell::new(None), description })
    }

    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(JamTool { owner: self.clone() })
    }

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        match input.get("action").and_then(Value::as_str).unwrap_or("look") {
            "look" => self.look(&input, &signal).await,
            "fix" => self.fix(&input, &signal).await,
            _ => Err("action is look (the default) or fix".into()),
        }
    }

    async fn look(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let words = self.backend.words();
        let tracks = strings(input.get("tracks"));
        let named = strings(input.get("patterns"));
        let whole = input.get("wholeSong") == Some(&Value::Bool(true));
        if named.is_empty() && !whole {
            return Err(format!(
                "patterns: the jam's {}s, the new ones since last time (the catch-up names them), by ref or by the slots that play them; or wholeSong: true when the producer asks for the whole song",
                words.part
            ));
        }
        let parts = self.backend.parts(&tracks, if whole { None } else { Some(&named) }, signal).await?;
        let in_jam =
            |part: &Part| whole || named.iter().any(|name| *name == part.reference || part.slots.iter().any(|(_, slot)| slot == name));
        // (a slot is dropped or replayed whole: only judged on every track, and never one holding more than its tracks' notes)
        let narrowed = !tracks.is_empty();
        let jam: Vec<String> = parts.iter().filter(|part| in_jam(part)).map(|part| part.reference.clone()).collect();
        if jam.is_empty() {
            return Err(format!("none of those is a {} the sequence plays: read the sequence again", words.part));
        }
        let mut proposals: Vec<(String, Proposal)> = Vec::new();
        let mut said: Vec<String> = Vec::new();
        let mut rows: Vec<Value> = Vec::new();
        let mut feels = BTreeMap::new();
        let mut counts = BTreeMap::<&str, usize>::new();
        let mut id = |kind: &'static str| {
            let n = counts.entry(kind).or_insert(0);
            *n += 1;
            format!("{kind}{n}")
        };
        // (each track's note classes across the jam: what tells a stray)
        let mut classes: BTreeMap<String, BTreeMap<i64, usize>> = BTreeMap::new();
        for part in parts.iter().filter(|part| jam.contains(&part.reference)) {
            for track in &part.tracks {
                for hit in &track.hits {
                    *classes.entry(track.track.clone()).or_default().entry(hit.note.rem_euclid(12)).or_insert(0) += 1;
                }
            }
        }
        for (k, part) in parts.iter().enumerate().filter(|(_, part)| jam.contains(&part.reference)) {
            let earlier = &parts[..k];
            let slots = part.slots.iter().map(|(n, _)| format!("{n:02}")).collect::<Vec<_>>().join(", ");
            if part.empty() && !narrowed && !part.holds_more {
                for (number, slot) in &part.slots {
                    let pid = id("e");
                    rows.push(json!({"id": pid, "says": format!("slot {number:02} plays {}, which is empty: drop the slot", part.name)}));
                    proposals.push((pid, Proposal::Empty { slot: slot.clone(), number: *number }));
                }
                continue;
            }
            let same = earlier.iter().find(|other| !narrowed && !part.holds_more && !other.holds_more && part.same_as(other));
            if let Some(same) = same {
                for (number, slot) in &part.slots {
                    let pid = id("s");
                    rows.push(json!({"id": pid, "says": format!("slot {number:02} plays {}, the same as {}: play {} there instead", part.name, same.name, same.name)}));
                    proposals.push((pid, Proposal::Same { slot: slot.clone(), of: same.reference.clone() }));
                }
                continue;
            }
            if let Some((other, how)) = earlier.iter().find_map(|other| part.near(other, NEAR).map(|how| (other, how))) {
                said.push(format!("{} (slot {slots}) is {} with {how}", part.name, other.name));
            }
            for track in part.tracks.iter().filter(|track| !track.empty && !track.alias) {
                if let Some(of) = earlier.iter().find(|other| {
                    other.tracks.iter().any(|theirs| theirs.track == track.track && theirs.digest == track.digest && !theirs.alias)
                }) {
                    let pid = id("a");
                    rows.push(json!({"id": pid, "says": format!("\u{201c}{}\u{201d} in {} is the same as in {}: make it {} of {}", track.name, part.name, of.name, words.alias, of.name)}));
                    proposals.push((
                        pid,
                        Proposal::Repeat { part: part.reference.clone(), track: track.track.clone(), of: of.reference.clone() },
                    ));
                    continue;
                }
                let track_feel = feel(&track.hits, part.lpb);
                let name = |note: i64| self.backend.note_name(note);
                for mistake in mistakes(&track.hits, part.lpb, &track_feel, &classes.get(&track.track).cloned().unwrap_or_default(), &name)
                {
                    let pid = id("m");
                    let class = (super::nearest(mistake.hit.at) as u32) % part.lpb.max(1);
                    let to = quantized(mistake.hit.at, 1., 1., track_feel.get(&class).copied());
                    let what = if mistake.kind == "timing" { "move it to the grid" } else { "take it away" };
                    rows.push(
                        json!({"id": pid, "says": format!("\u{201c}{}\u{201d} in {}: {}: {what}", track.name, part.name, mistake.says)}),
                    );
                    proposals.push((pid, Proposal::Mistake { part: part.reference.clone(), track: track.track.clone(), mistake, to }));
                }
                let late: Vec<String> = track_feel
                    .iter()
                    .filter(|(_, offset)| offset.abs() >= 0.1)
                    .map(|(class, offset)| format!("line {class} of each beat {offset:+.2}"))
                    .collect();
                if !late.is_empty() {
                    said.push(format!("\u{201c}{}\u{201d} in {} has a feel ({}): kept", track.name, part.name, late.join(", ")));
                }
                feels.insert((part.reference.clone(), track.track.clone()), track_feel);
            }
        }
        let slots: Vec<usize> =
            parts.iter().filter(|part| jam.contains(&part.reference)).flat_map(|part| part.slots.iter().map(|(n, _)| *n)).collect();
        let jam_words = match (slots.iter().min(), slots.iter().max()) {
            (Some(first), Some(last)) => format!("{} {}s, slots {first:02} to {last:02}", jam.len(), words.part),
            _ => format!("{} {}s", jam.len(), words.part),
        };
        // (what came before, left alone, said)
        let before: Vec<usize> =
            parts.iter().filter(|part| !jam.contains(&part.reference)).flat_map(|part| part.slots.iter().map(|(n, _)| *n)).collect();
        if !before.is_empty() {
            let listed: Vec<String> = before.iter().map(|n| format!("{n:02}")).collect();
            said.push(format!("left alone: slots {} (what came before)", listed.join(", ")));
        }
        if narrowed {
            said.push(format!(
                "only the tracks named were read: no slot is proposed to drop or to play another {} (that needs every track)",
                words.part
            ));
        }
        let mut reply = json!({"jam": jam_words, "proposals": rows});
        if !said.is_empty() {
            reply["said"] = json!(said);
        }
        reply["next"] = json!(if proposals.is_empty() {
            "nothing to clean up: say so; quantize (fix with quantize) only if the producer asks"
        } else {
            "tell the producer what's new and propose these; fix applies the ids they accept (\"all\" for every one), never what came before"
        });
        *self.looked.borrow_mut() = Some(Looked { parts, jam, feel: feels, proposals });
        Ok(reply)
    }

    async fn fix(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let Some(looked) = self.looked.borrow_mut().take() else {
            return Err("look first: fix applies what a look proposed".into());
        };
        let asked = strings(input.get("ids"));
        let all = asked.iter().any(|id| id == "all");
        let chosen: Vec<&(String, Proposal)> = looked.proposals.iter().filter(|(id, _)| all || asked.contains(id)).collect();
        if let Some(unknown) = asked.iter().find(|id| *id != "all" && !looked.proposals.iter().any(|(pid, _)| pid == *id)) {
            *self.looked.borrow_mut() = Some(looked);
            return Err(format!("{unknown} isn't one of the look's proposals"));
        }
        let quantize = input.get("quantize").and_then(Value::as_object);
        // (each track's edits together: its mistakes, and the quantize over its notes)
        let mut edits: BTreeMap<(String, String), Vec<Edit>> = BTreeMap::new();
        for (_, proposal) in &chosen {
            if let Proposal::Mistake { part, track, mistake, to } = proposal {
                let edit = if mistake.kind == "timing" { Edit::Move(mistake.hit.clone(), *to) } else { Edit::Remove(mistake.hit.clone()) };
                edits.entry((part.clone(), track.clone())).or_default().push(edit);
            }
        }
        if let Some(quantize) = quantize {
            let strength = quantize.get("strength").and_then(Value::as_f64).unwrap_or(0.5).clamp(0., 1.);
            let grid = quantize.get("grid").and_then(Value::as_f64).unwrap_or(1.).max(0.01);
            let keep_feel = quantize.get("keepFeel").and_then(Value::as_bool).unwrap_or(true);
            let only = strings(quantize.get("tracks"));
            for part in looked.parts.iter().filter(|part| looked.jam.contains(&part.reference)) {
                for track in
                    part.tracks.iter().filter(|track| !track.empty && !track.alias && (only.is_empty() || only.contains(&track.track)))
                {
                    let feel = looked.feel.get(&(part.reference.clone(), track.track.clone()));
                    let list = edits.entry((part.reference.clone(), track.track.clone())).or_default();
                    for hit in &track.hits {
                        if list.iter().any(|edit| matches!(edit, Edit::Remove(h) | Edit::Move(h, _) if same_hit(h, hit))) {
                            continue;
                        }
                        // (the feel of the grid line it goes to, not the one it's written near)
                        let class = ((super::nearest(hit.at / grid) * grid).round() as u32) % part.lpb.max(1);
                        let to = quantized(
                            hit.at,
                            grid,
                            strength,
                            if keep_feel { feel.and_then(|feel| feel.get(&class)).copied() } else { None },
                        );
                        if (to - hit.at).abs() > 0.01 {
                            list.push(Edit::Move(hit.clone(), to));
                        }
                    }
                }
            }
        }
        let mut made = Vec::new();
        let mut said = Vec::new();
        for ((part_ref, track_ref), list) in edits.iter().filter(|(_, list)| !list.is_empty()) {
            let Some(part) = looked.parts.iter().find(|part| part.reference == *part_ref) else { continue };
            let Some(track) = part.tracks.iter().find(|track| track.track == *track_ref) else { continue };
            let (answer, skipped) = self.backend.rewrite(part, track, list, signal).await?;
            made.push(answer);
            said.extend(skipped);
        }
        for (_, proposal) in &chosen {
            if let Proposal::Repeat { part, track, of } = proposal {
                made.push(self.backend.alias(part, track, of, signal).await?);
            }
        }
        for (_, proposal) in &chosen {
            if let Proposal::Same { slot, of, .. } = proposal {
                made.push(self.backend.replay(slot, of, signal).await?);
            }
        }
        // (the slots dropped last, the highest first: a drop moves the ones after it)
        let mut drops: Vec<(usize, &String)> = chosen
            .iter()
            .filter_map(|(_, p)| if let Proposal::Empty { slot, number } = p { Some((*number, slot)) } else { None })
            .collect();
        drops.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, slot) in drops {
            made.push(self.backend.drop_slot(slot, signal).await?);
        }
        let mut reply = json!({"made": made, "next": "look again before more fixes: the song moved"});
        if !said.is_empty() {
            reply["said"] = json!(said);
        }
        Ok(reply)
    }
}

fn same_hit(a: &Hit, b: &Hit) -> bool {
    a.line == b.line && a.col == b.col && a.note == b.note
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

fn description(words: &super::backend::Words) -> String {
    format!(
        "Clean up a jam in {app}. Start from what changed since last time (the catch-up's new {part}s: name them in patterns, by ref or by their slots) and never touch what came before. action look (the default) sets them against what came before and proposes, each with an id: slots playing an empty {part} to drop, or one the same as an earlier {part} to play that one instead; tracks the same as an earlier {part}'s to make {alias}; notes that read as mistakes rather than feel (a double trigger, a note far off the grid where its neighbours aren't, a stray the track plays nowhere else; a swing every beat shares is feel, and kept). It says which {part}s are near copies. Tell the producer what's new and propose; action fix applies the ids they accept (ids: [\"all\"] for every one), and quantize {{strength 0 to 1, grid in lines, keepFeel (true by default), tracks}} moves the jam's notes toward the grid when asked.",
        app = words.app,
        part = words.part,
        alias = words.alias
    )
}

fn schema() -> JsonObject {
    let value = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "action": {"type": "string", "enum": ["look", "fix"]},
            "patterns": {"type": "array", "items": {"type": "string"}, "description": "The jam's parts (by ref, or a slot that plays one): the new ones since last time"},
            "wholeSong": {"type": "boolean", "description": "The whole song is the jam (only when the producer asks)"},
            "tracks": {"type": "array", "items": {"type": "string"}},
            "ids": {"type": "array", "items": {"type": "string"}, "description": "The look's proposals to apply, or [\"all\"]"},
            "quantize": {"type": "object", "additionalProperties": false, "properties": {
                "strength": {"type": "number", "minimum": 0, "maximum": 1},
                "grid": {"type": "number", "exclusiveMinimum": 0, "description": "In lines"},
                "keepFeel": {"type": "boolean"},
                "tracks": {"type": "array", "items": {"type": "string"}}
            }}
        }
    });
    value.as_object().cloned().unwrap_or_default()
}

struct JamTool<B: JamBackend> {
    owner: Rc<Jam<B>>,
}

#[async_trait(?Send)]
impl<B: JamBackend + 'static> KernelTool for JamTool<B> {
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
