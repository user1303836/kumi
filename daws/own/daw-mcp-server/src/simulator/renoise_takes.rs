//! What Kumi's variations make in the simulator's Renoise song (M7), answered as the tool answers: phrases made with their
//! lines (phrase-add's content), written, set and dropped; slots of variation patterns made and dropped; a phrase's lines and
//! `read {kind: "where"}`. A place is the made object's id (the simulator never moves what it made); the vectors' song has
//! three patterns in four slots, and one phrase in its first instrument, before anything made here.
use serde_json::{json, Value};
use std::collections::BTreeMap;

const PATTERNS: u64 = 3;
const SLOTS: u64 = 4;

fn phrases_before(instrument: u64) -> u64 {
    if instrument == 1 {
        1
    } else {
        0
    }
}

#[derive(Debug, Clone)]
struct Phrase {
    id: u64,
    name: String,
    lines: u64,
    lpb: u64,
    rows: Vec<Value>,
}

#[derive(Debug, Clone)]
struct Pattern {
    id: u64,
    name: String,
    /// Whether a slot plays it (a drop takes the slot; the pattern stays in the pool).
    slotted: bool,
}

#[derive(Default)]
pub struct Takes {
    next: u64,
    /// Places are of a generation: one from before `forget` (Renoise started again) is unknown, and found again by name.
    generation: u64,
    phrases: BTreeMap<u64, Vec<Phrase>>,
    patterns: Vec<Pattern>,
    /// What each drop took (by its desc): the phrases with their places, or the patterns' ids.
    dropped: BTreeMap<String, Value>,
}

type Answer = Result<Value, (String, String)>;

fn bad(message: &str) -> (String, String) {
    ("bad-params".into(), message.into())
}

/// The numbers of a ref of `kind` (`E:phrase:1.2` is [1, 2]).
fn numbers(text: Option<&str>, kind: &str) -> Option<Vec<u64>> {
    let rest = text?.split_once(':')?.1.strip_prefix(&format!("{kind}:"))?;
    Some(rest.split('.').filter_map(|n| n.parse().ok()).collect())
}

impl Takes {
    /// The kinds this answers.
    pub fn owns(kind: &str) -> bool {
        matches!(kind, "phrase-add" | "phrase-write" | "phrase-set" | "slot-variation" | "slots-drop" | "phrases-drop" | "pattern-name")
    }

    /// Renoise started again: every place from before is unknown (what was made is still there, by its name).
    pub fn forget(&mut self) {
        self.generation += 1;
    }

    fn place(&self, id: u64) -> Value {
        json!({"sim": id, "gen": self.generation})
    }

    /// A place of this generation's: its id; an older one is unknown.
    fn known(&self, place: &Value) -> Option<u64> {
        (place.get("gen").and_then(Value::as_u64) == Some(self.generation)).then(|| place.get("sim").and_then(Value::as_u64)).flatten()
    }

    /// The made phrases' rows of an instrument, after `before` (the vectors' rows), as a read of its phrases answers.
    pub fn phrase_rows(&self, epoch: u32, instrument: u64, before: Vec<Value>) -> Answer {
        let mut rows = before;
        for (k, phrase) in self.phrases.get(&instrument).into_iter().flatten().enumerate() {
            let p = phrases_before(instrument) + 1 + k as u64;
            rows.push(json!({"ref": format!("{epoch}:phrase:{instrument}.{p}"), "index": p, "name": phrase.name, "lines": phrase.lines, "lpb": phrase.lpb}));
        }
        Ok(json!({"items": rows, "epoch": epoch.to_string(), "layout": 0}))
    }

    /// The made patterns' rows after `before` (the vectors' three), as a read of the patterns answers.
    pub fn pattern_rows(&self, epoch: u32, before: Vec<Value>) -> Answer {
        let mut rows = before;
        for (k, pattern) in self.patterns.iter().enumerate() {
            let p = PATTERNS + 1 + k as u64;
            rows.push(json!({"ref": format!("{epoch}:pattern:{p}"), "number": p - 1, "name": pattern.name, "lines": 32, "aliases": {}}));
        }
        Ok(json!({"items": rows, "epoch": epoch.to_string(), "layout": 0}))
    }

    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn phrase_at(&self, instrument: u64, index: u64) -> Option<&Phrase> {
        let made = index.checked_sub(phrases_before(instrument) + 1)?;
        self.phrases.get(&instrument)?.get(made as usize)
    }

    fn phrase_index(&self, id: u64) -> Option<(u64, u64)> {
        self.phrases.iter().find_map(|(instrument, list)| {
            list.iter().position(|phrase| phrase.id == id).map(|k| (*instrument, phrases_before(*instrument) + 1 + k as u64))
        })
    }

    /// The pattern number (from 1) of a made pattern, and its slot (from 1) while it has one.
    fn pattern_index(&self, id: u64) -> Option<(u64, Option<u64>)> {
        let k = self.patterns.iter().position(|pattern| pattern.id == id)?;
        let slot = self.patterns[k].slotted.then(|| SLOTS + self.patterns[..k].iter().filter(|pattern| pattern.slotted).count() as u64 + 1);
        Some((PATTERNS + 1 + k as u64, slot))
    }

    /// A made phrase's lines, as the tool's `line` read of a phrase answers.
    pub fn phrase_lines(&self, epoch: u32, parent: &str) -> Option<Answer> {
        let n = numbers(Some(parent), "phrase")?;
        let phrase = self.phrase_at(n[0], n[1])?;
        Some(Ok(json!({"items": phrase.rows, "epoch": epoch.to_string(), "layout": 0, "lines": phrase.lines, "lpb": phrase.lpb})))
    }

    /// `read {kind: "where"}`: each item's ref and place now.
    pub fn locate(&self, epoch: u32, items: &[Value]) -> Answer {
        let mut out = Vec::new();
        for item in items {
            let row = if let Some(pattern) = item.get("pattern") {
                let found = match pattern {
                    Value::String(text) if text.contains(":seq:") => {
                        let s = numbers(Some(text), "seq").and_then(|n| n.first().copied()).unwrap_or(0);
                        // (the vectors' sequence plays 1, 2, 1, 3)
                        [1u64, 2, 1, 3].get(s.saturating_sub(1) as usize).map(|p| (*p, None))
                    }
                    Value::String(text) => numbers(Some(text), "pattern").and_then(|n| n.first().copied()).map(|p| (p, None)),
                    place => match self.known(place) {
                        Some(id) => Some((0, Some(id))),
                        None => {
                            out.push(json!({"unknown": true}));
                            continue;
                        }
                    },
                };
                match found {
                    Some((p, None)) if (1..=PATTERNS).contains(&p) => {
                        let slots: Vec<u64> =
                            [1u64, 2, 1, 3].iter().enumerate().filter(|(_, v)| **v == p).map(|(s, _)| s as u64 + 1).collect();
                        json!({"ref": format!("{epoch}:pattern:{p}"), "number": p - 1, "name": "", "lines": 32, "slots": slots,
                            "seqRef": format!("{epoch}:seq:{}", slots[0]), "place": {"sim": 0, "n": p, "gen": self.generation}})
                    }
                    Some((_, Some(0))) => {
                        let p = pattern.get("n").and_then(Value::as_u64).unwrap_or(1);
                        let slots: Vec<u64> =
                            [1u64, 2, 1, 3].iter().enumerate().filter(|(_, v)| **v == p).map(|(s, _)| s as u64 + 1).collect();
                        json!({"ref": format!("{epoch}:pattern:{p}"), "number": p - 1, "name": "", "lines": 32, "slots": slots,
                            "seqRef": format!("{epoch}:seq:{}", slots[0]), "place": {"sim": 0, "n": p, "gen": self.generation}})
                    }
                    Some((_, Some(id))) => match self.pattern_index(id) {
                        Some((p, slot)) => {
                            let name = &self.patterns[(p - PATTERNS - 1) as usize].name;
                            let slots: Vec<u64> = slot.into_iter().collect();
                            let mut row = json!({"ref": format!("{epoch}:pattern:{p}"), "number": p - 1, "name": name, "lines": 32, "slots": slots, "place": self.place(id)});
                            if let Some(slot) = slot {
                                row["seqRef"] = format!("{epoch}:seq:{slot}").into();
                            }
                            row
                        }
                        None => json!({"gone": true}),
                    },
                    _ => json!({"gone": true}),
                }
            } else if let Some(phrase) = item.get("phrase") {
                let found = match phrase {
                    Value::String(text) => numbers(Some(text), "phrase").map(|n| (n[0], n[1])),
                    place => match self.known(place) {
                        Some(id) => self.phrase_index(id),
                        None => {
                            out.push(json!({"unknown": true}));
                            continue;
                        }
                    },
                };
                match found.and_then(|(i, p)| self.phrase_at(i, p).map(|phrase| (i, p, phrase))) {
                    Some((i, p, phrase)) => {
                        json!({"ref": format!("{epoch}:phrase:{i}.{p}"), "index": p, "name": phrase.name, "lines": phrase.lines,
                        "lpb": phrase.lpb, "place": self.place(phrase.id)})
                    }
                    None => json!({"gone": true}),
                }
            } else if let Some(instrument) = item.get("instrument") {
                let i = match instrument {
                    Value::String(text) => numbers(Some(text), "instrument").and_then(|n| n.first().copied()),
                    place => place.get("instrument").and_then(Value::as_u64),
                };
                match i.filter(|i| (1..=2).contains(i)) {
                    Some(i) => {
                        let phrases = phrases_before(i) + self.phrases.get(&i).map_or(0, Vec::len) as u64;
                        json!({"ref": format!("{epoch}:instrument:{i}"), "index": i, "name": if i == 1 { "Kit" } else { "Pad" }, "phrases": phrases,
                            "place": {"instrument": i}})
                    }
                    None => json!({"gone": true}),
                }
            } else if let Some(track) = item.get("track").and_then(Value::as_str) {
                let t = numbers(Some(track), "track").and_then(|n| n.first().copied()).unwrap_or(0);
                json!({"ref": format!("{epoch}:track:{t}"), "index": t, "name": format!("Track {t}")})
            } else {
                return Err(bad("each item is {pattern}, {instrument}, {phrase} or {track}"));
            };
            out.push(row);
        }
        Ok(json!({"items": out, "epoch": epoch.to_string(), "layout": 0}))
    }

    /// A change of one of the kinds this owns. `lines` are the pattern tracks' lines the simulator keeps (by pattern and track):
    /// a variation's pattern gets its varied track's there, so `notes-write` and `line` reads work on it.
    pub fn change(
        &mut self,
        epoch: u32,
        method: &str,
        params: &Value,
        lines: &mut std::collections::HashMap<(u64, u64), Vec<Value>>,
        undo: &mut Vec<String>,
    ) -> Answer {
        let kind = params["kind"].as_str().unwrap_or_default();
        let input = &params["input"];
        let desc = params["desc"].as_str().unwrap_or_default().to_string();
        let answer = |mut value: Value| {
            value["epoch"] = epoch.to_string().into();
            value["layout"] = 0.into();
            Ok(value)
        };
        let target = json!({"ref": format!("{epoch}:song:1"), "kind": "song"});
        match (kind, method) {
            ("phrase-add", "change_preview") => {
                let i = numbers(input["ref"].as_str(), "instrument").and_then(|n| n.first().copied()).ok_or(bad("ref: an instrument"))?;
                answer(
                    json!({"target": target, "prior": {"phrases": 0}, "fence": {"instrument": i}, "display": {"title": format!("Added phrase “{}”", input["name"].as_str().unwrap_or(""))}}),
                )
            }
            ("phrase-add", "change_apply") => {
                let i = params["fence"]["instrument"].as_u64().unwrap_or(1);
                let id = self.id();
                let rows: Vec<Value> = input["content"].as_array().cloned().unwrap_or_default();
                let phrase = Phrase {
                    id,
                    name: input["name"].as_str().unwrap_or("").into(),
                    lines: input["lines"].as_u64().unwrap_or(32),
                    lpb: input["lpb"].as_u64().unwrap_or(4),
                    rows,
                };
                let list = self.phrases.entry(i).or_default();
                list.push(phrase);
                let p = phrases_before(i) + list.len() as u64;
                undo.push(desc.clone());
                answer(
                    json!({"applied": {"id": id}, "created": {"ref": format!("{epoch}:phrase:{i}.{p}"), "kind": "phrase", "place": self.place(id)},
                    "target": target, "display": {"title": format!("Added phrase “{}”", input["name"].as_str().unwrap_or(""))}, "undo": {"desc": desc, "entry": true}}),
                )
            }
            ("phrase-add", _) => {
                let id = params["applied"]["id"].as_u64().unwrap_or(0);
                for list in self.phrases.values_mut() {
                    list.retain(|phrase| phrase.id != id);
                }
                undo.push(desc);
                answer(json!({"restored": {"deleted": true}, "display": {"title": "Removed the phrase (put back)"}}))
            }
            ("phrase-write" | "phrase-set", "change_preview") => {
                let n = numbers(input["ref"].as_str(), "phrase").ok_or(bad("ref: a phrase"))?;
                // (as the tool's phrase-write: lines to write, at least one)
                if kind == "phrase-write" && input["lines"].as_array().is_none_or(Vec::is_empty) {
                    return Err(bad("lines: a list of {line, notes?, effects?, clear?}"));
                }
                let phrase = self.phrase_at(n[0], n[1]).ok_or(("gone".to_string(), "that phrase isn't there".to_string()))?;
                answer(
                    json!({"target": {"ref": input["ref"], "kind": "phrase"}, "prior": {"rows": phrase.rows, "lpb": phrase.lpb, "id": phrase.id},
                    "fence": {"id": phrase.id}, "display": {"title": format!("Wrote lines in “{}”", phrase.name)}}),
                )
            }
            ("phrase-write" | "phrase-set", "change_apply") => {
                let id = params["fence"]["id"].as_u64().unwrap_or(0);
                let (i, p) = self.phrase_index(id).ok_or(("gone".to_string(), "that phrase isn't there".to_string()))?;
                let k = (p - phrases_before(i) - 1) as usize;
                let phrase = &mut self.phrases.get_mut(&i).expect("its instrument")[k];
                if kind == "phrase-set" {
                    phrase.lpb = input["lpb"].as_u64().unwrap_or(phrase.lpb);
                    if let Some(name) = input["name"].as_str() {
                        phrase.name = name.into();
                    }
                } else {
                    phrase.rows = written(&phrase.rows, input["lines"].as_array().map(Vec::as_slice).unwrap_or(&[]));
                }
                undo.push(desc.clone());
                answer(
                    json!({"applied": {"id": id, "rows": phrase.rows, "lpb": phrase.lpb}, "display": {"title": format!("Wrote lines in “{}”", phrase.name)}, "undo": {"desc": desc, "entry": true}}),
                )
            }
            ("phrase-write" | "phrase-set", _) => {
                let id = params["prior"]["id"].as_u64().unwrap_or(0);
                let (i, p) = self.phrase_index(id).ok_or(("gone".to_string(), "that phrase isn't there".to_string()))?;
                let k = (p - phrases_before(i) - 1) as usize;
                let phrase = &mut self.phrases.get_mut(&i).expect("its instrument")[k];
                if params["applied"]["rows"] != json!(phrase.rows) && params["discard"] != true {
                    return Err(("changed".into(), "the phrase changed in Renoise since Kumi wrote it".into()));
                }
                phrase.rows = params["prior"]["rows"].as_array().cloned().unwrap_or_default();
                phrase.lpb = params["prior"]["lpb"].as_u64().unwrap_or(phrase.lpb);
                undo.push(desc);
                answer(json!({"restored": {"done": true}, "display": {"title": "Lines put back"}}))
            }
            // (a made pattern renamed: a kept take's mark)
            ("pattern-name", "change_preview") => {
                let p = numbers(input["ref"].as_str(), "pattern").and_then(|n| n.first().copied()).ok_or(bad("ref: a pattern"))?;
                let k = p
                    .checked_sub(PATTERNS + 1)
                    .filter(|k| (*k as usize) < self.patterns.len())
                    .ok_or(bad("the sim names only patterns its variations made"))? as usize;
                let pattern = &self.patterns[k];
                answer(json!({"target": {"ref": input["ref"], "kind": "pattern"}, "prior": {"name": pattern.name, "id": pattern.id},
                    "fence": {"id": pattern.id}, "display": {"title": format!("Renamed pattern “{}”", pattern.name)}}))
            }
            ("pattern-name", "change_apply") => {
                let id = params["fence"]["id"].as_u64().unwrap_or(0);
                let pattern = self
                    .patterns
                    .iter_mut()
                    .find(|pattern| pattern.id == id)
                    .ok_or(("gone".to_string(), "that pattern isn't there".to_string()))?;
                pattern.name = input["name"].as_str().unwrap_or("").into();
                undo.push(desc.clone());
                answer(
                    json!({"applied": {"id": id}, "display": {"title": format!("Renamed pattern “{}”", pattern.name)}, "undo": {"desc": desc, "entry": true}}),
                )
            }
            ("slot-variation", "change_preview") => {
                numbers(input["pattern"].as_str(), "pattern").ok_or(bad("pattern: a pattern"))?;
                numbers(input["track"].as_str(), "track").ok_or(bad("track: a track"))?;
                answer(json!({"target": {"ref": input["track"], "kind": "track"}, "prior": {"slots": 0}, "fence": {},
                    "display": {"title": format!("Added a variation “{}”", input["name"].as_str().unwrap_or(""))}}))
            }
            ("slot-variation", "change_apply") => {
                let source = numbers(input["pattern"].as_str(), "pattern").and_then(|n| n.first().copied()).unwrap_or(1);
                let track = numbers(input["track"].as_str(), "track").and_then(|n| n.first().copied()).unwrap_or(1);
                let (first, last) = (input["first"].as_u64().unwrap_or(0), input["last"].as_u64().unwrap_or(31));
                let id = self.id();
                self.patterns.push(Pattern { id, name: input["name"].as_str().unwrap_or("").into(), slotted: true });
                let (p, slot) = self.pattern_index(id).expect("just made");
                let mut rows: Vec<Value> = lines
                    .get(&(source, track))
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|row| row["line"].as_u64().is_some_and(|line| line < first || line > last))
                    .collect();
                rows.extend(input["content"].as_array().cloned().unwrap_or_default());
                rows.sort_by_key(|row| row["line"].as_u64());
                lines.insert((p, track), rows);
                undo.push(desc.clone());
                answer(json!({"applied": {"id": id}, "target": {"ref": input["track"], "kind": "track"},
                    "created": {"ref": format!("{epoch}:seq:{}", slot.unwrap_or(0)), "kind": "seq", "pattern": format!("{epoch}:pattern:{p}"), "place": self.place(id)},
                    "display": {"title": format!("Added a variation “{}”", input["name"].as_str().unwrap_or(""))}, "undo": {"desc": desc, "entry": true}}))
            }
            ("slot-variation", _) => {
                let id = params["applied"]["id"].as_u64().unwrap_or(0);
                if let Some(pattern) = self.patterns.iter_mut().find(|pattern| pattern.id == id) {
                    pattern.slotted = false;
                }
                undo.push(desc);
                answer(json!({"restored": {"done": true}, "display": {"title": "Removed the variation's slot (put back)"}}))
            }
            ("slots-drop" | "phrases-drop", "change_preview") => {
                let count = input["refs"].as_array().map_or(0, Vec::len);
                if count == 0 {
                    return Err(bad("refs: the takes to drop"));
                }
                answer(
                    json!({"target": target, "prior": {"deleted": false}, "fence": {}, "display": {"title": format!("Deleted {count}")}}),
                )
            }
            ("slots-drop", "change_apply") => {
                let mut ids = Vec::new();
                for reference in input["refs"].as_array().into_iter().flatten() {
                    let s = numbers(reference.as_str(), "seq").and_then(|n| n.first().copied()).ok_or(bad("refs: slots"))?;
                    let id = self
                        .patterns
                        .iter()
                        .map(|pattern| pattern.id)
                        .find(|id| self.pattern_index(*id).and_then(|(_, slot)| slot) == Some(s));
                    ids.push(id.ok_or(bad("the sim drops only slots its variations made"))?);
                }
                for id in &ids {
                    if let Some(pattern) = self.patterns.iter_mut().find(|pattern| pattern.id == *id) {
                        pattern.slotted = false;
                    }
                }
                self.dropped.insert(desc.clone(), json!(ids));
                undo.push(desc.clone());
                answer(
                    json!({"applied": {"deleted": true}, "display": {"title": format!("Deleted {} slots", ids.len()), "note": "Their patterns stay in the pool, unused."}, "undo": {"desc": desc, "entry": true}}),
                )
            }
            ("phrases-drop", "change_apply") => {
                let mut taken = Vec::new();
                for reference in input["refs"].as_array().into_iter().flatten() {
                    let n = numbers(reference.as_str(), "phrase").ok_or(bad("refs: phrases"))?;
                    let phrase = self.phrase_at(n[0], n[1]).ok_or(bad("the sim drops only phrases its variations made"))?;
                    taken.push((n[0], phrase.id));
                }
                for (instrument, id) in &taken {
                    if let Some(list) = self.phrases.get_mut(instrument) {
                        list.retain(|phrase| phrase.id != *id);
                    }
                }
                undo.push(desc.clone());
                answer(
                    json!({"applied": {"deleted": true}, "display": {"title": format!("Deleted {} phrases", taken.len())}, "undo": {"desc": desc, "entry": true}}),
                )
            }
            _ => Err(("daw-only".into(), "the sim doesn't put dropped takes back".into())),
        }
    }

    /// The sequence's rows (`rows`: the vectors' four slots of 8 beats), with a slot after them for each made pattern a slot
    /// plays.
    pub fn sequence(&self, epoch: u32, mut rows: Vec<Value>) -> Answer {
        let mut position = SLOTS as f64 * 8.;
        for pattern in self.patterns.iter().filter(|pattern| pattern.slotted) {
            let Some((p, Some(slot))) = self.pattern_index(pattern.id) else { continue };
            rows.push(json!({"ref": format!("{epoch}:seq:{slot}"), "pattern": p - 1, "patternRef": format!("{epoch}:pattern:{p}"),
                "positionBeats": position, "lengthBeats": 8, "mutedTracks": [], "section": null}));
            position += 8.;
        }
        Ok(json!({"items": rows, "epoch": epoch.to_string(), "layout": 0}))
    }

    /// The made phrases of an instrument: name and lines, in order.
    pub fn phrases(&self, instrument: u64) -> Vec<(String, Vec<Value>)> {
        self.phrases.get(&instrument).into_iter().flatten().map(|phrase| (phrase.name.clone(), phrase.rows.clone())).collect()
    }

    /// The made patterns that a slot plays: name and number (from 1).
    pub fn slotted(&self) -> Vec<(String, u64)> {
        self.patterns
            .iter()
            .filter(|pattern| pattern.slotted)
            .map(|pattern| (pattern.name.clone(), self.pattern_index(pattern.id).map_or(0, |(p, _)| p)))
            .collect()
    }
}

/// Lines after a write without merge: each line named is cleared first when it says so, and its cells written.
fn written(rows: &[Value], entries: &[Value]) -> Vec<Value> {
    let mut by: BTreeMap<u64, Value> = rows.iter().filter_map(|row| Some((row["line"].as_u64()?, row.clone()))).collect();
    for entry in entries {
        let Some(line) = entry["line"].as_u64() else { continue };
        if entry["clear"] == true {
            by.remove(&line);
        }
        let mut row = json!({"line": line});
        for key in ["notes", "effects"] {
            if let Some(cells) = entry.get(key).filter(|cells| cells.as_array().is_some_and(|cells| !cells.is_empty())) {
                row[key] = cells.clone();
            }
        }
        if row.get("notes").is_some() || row.get("effects").is_some() {
            by.insert(line, row);
        }
    }
    by.into_values().collect()
}
