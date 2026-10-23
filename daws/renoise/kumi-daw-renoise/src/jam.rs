//! Renoise's backend for cleaning up a jam (#401): only Renoise's facts. Its parts are the patterns the sequence plays, read
//! with their sequencer tracks' lines (the song's LPB); a track's notes are removed or moved with notes_write (a moved note keeps
//! its column, its delay saying where in the line it starts), a track becomes an alias with pattern_alias, a slot plays another
//! pattern with slot_pattern, and a slot goes with slot_delete. Core's tool does the rest.
use crate::lines::{note_text, Lines};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_daw_own::changes::OwnPlan;
use kumi_runtime::core::jam::backend::{Edit, JamBackend, Words};
use kumi_runtime::core::jam::{Hit, Part, TrackPart};
use kumi_runtime::core::refs::RefScheme;
use serde_json::{json, Value};
use std::rc::Rc;

pub static WORDS: Words = Words { app: "Renoise", part: "pattern", slot: "slot", alias: "an alias (it plays that pattern's lines)" };

/// Pages of rows read (200 a page; a pattern holds up to 512 lines).
const PAGES: usize = 8;
/// The most patterns a look reads, and the pages of patterns read with their prints (16 a page).
const MOST_PATTERNS: usize = 256;
const PRINT_PAGES: usize = 64;

pub struct RenoiseJam<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
}

impl<S: RefScheme + 'static> RenoiseJam<S> {
    pub fn new(plan: Rc<OwnPlan<S>>) -> Self {
        RenoiseJam { plan }
    }

    fn short(&self, long: &str) -> String {
        self.plan.link().book.borrow_mut().short_ref(long)
    }

    async fn rows(&self, args: Value, signal: &Signal) -> Result<Vec<Value>, String> {
        self.plan.rows_through(args, PAGES, signal).await
    }

    /// A pattern track's lines now (its refs long).
    async fn lines(&self, pattern: &str, track: &str, length: u32, lpb: u32, signal: &Signal) -> Result<Lines, String> {
        let rows = self.rows(json!({"kind": "line", "parent": pattern, "track": track, "hidden": true, "limit": 200}), signal).await?;
        Ok(Lines::from_rows(&rows, 0, length.max(1) - 1, lpb))
    }
}

fn hits_of(lines: &Lines) -> Vec<Hit> {
    lines.note_cells().into_iter().map(|(line, col, at, note, velocity)| Hit { at, note: note as i64, velocity, line, col }).collect()
}

/// A moved note's line and delay (256ths of a line), within the pattern.
fn line_and_delay(to: f64, length: u32) -> (u32, u8) {
    let to = to.clamp(0., length as f64 - 1. / 256.);
    let mut line = to.floor() as u32;
    let mut delay = ((to - line as f64) * 256.).round() as u32;
    if delay >= 256 {
        line += 1;
        delay = 0;
    }
    (line.min(length - 1), delay as u8)
}

/// The edits applied to `lines`: what couldn't be (a cell taken where a note goes), in words.
fn apply(lines: &mut Lines, edits: &[Edit], track: &str) -> Vec<String> {
    let mut said = Vec::new();
    let length = lines.length;
    for edit in edits {
        let (hit, to) = match edit {
            Edit::Remove(hit) => (hit, None),
            Edit::Move(hit, to) => (hit, Some(*to)),
        };
        let found = lines.rows.get(&hit.line).and_then(|row| {
            row.notes.iter().position(|note| note.col == hit.col && crate::lines::note_value(&note.note) == Some(hit.note as u8))
        });
        let Some(at) = found else { continue };
        let target = to.map(|to| line_and_delay(to, length));
        if let Some((line, _)) = target {
            let taken = line != hit.line && lines.rows.get(&line).is_some_and(|row| row.notes.iter().any(|note| note.col == hit.col));
            if taken {
                said.push(format!(
                    "{} on \u{201c}{track}\u{201d} stays on line {:02}: column {} of line {line:02} holds a note already",
                    crate::lines::note_text(hit.note.clamp(0, 119) as u8),
                    hit.line,
                    hit.col
                ));
                continue;
            }
        }
        let Some(row) = lines.rows.get_mut(&hit.line) else { continue };
        let mut note = row.notes.remove(at);
        if row.notes.is_empty() && row.effects.is_empty() {
            lines.rows.remove(&hit.line);
        }
        if let Some((line, delay)) = target {
            note.delay = delay;
            let row = lines.rows.entry(line).or_default();
            row.notes.push(note);
            row.notes.sort_by_key(|n| n.col);
        }
    }
    said
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> JamBackend for RenoiseJam<S> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn parts(&self, tracks: &[String], hits_for: Option<&[String]>, signal: &Signal) -> Result<Vec<Part>, String> {
        let song = self.plan.read_through(json!({"kind": "song"}), None, signal).await?;
        let lpb = song.pointer("/items/0/lpb").and_then(Value::as_u64).unwrap_or(4) as u32;
        let wanted: Vec<String> = tracks.iter().map(|track| self.plan.lengthen(track)).collect::<Result<_, _>>()?;
        let track_rows = self.rows(json!({"kind": "track", "limit": 200}), signal).await?;
        // (sequencer tracks, by their number in the song from 1: what a pattern's prints and aliases are keyed by)
        let all: Vec<(usize, String, String)> = track_rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.get("type").and_then(Value::as_str) == Some("sequencer"))
            .filter_map(|(k, row)| {
                Some((k + 1, row.get("ref")?.as_str()?.to_string(), row.get("name").and_then(Value::as_str).unwrap_or("").to_string()))
            })
            .collect();
        let sequencer: Vec<&(usize, String, String)> =
            all.iter().filter(|(_, reference, _)| wanted.is_empty() || wanted.contains(reference)).collect();
        // (every pattern's tracks fingerprinted by the tool, its lines not read: those are read for the jam's patterns only)
        let patterns = self.plan.rows_through(json!({"kind": "pattern", "prints": true, "limit": 16}), PRINT_PAGES, signal).await?;
        let sequence = self.rows(json!({"kind": "sequence", "limit": 200}), signal).await?;
        let named = |slot: &str, pattern: &str| hits_for.is_none_or(|named| named.iter().any(|n| n == slot || n == pattern));
        let mut jam: Vec<String> = Vec::new();
        for slot in &sequence {
            let (Some(slot_ref), Some(pattern_ref)) =
                (slot.get("ref").and_then(Value::as_str), slot.get("patternRef").and_then(Value::as_str))
            else {
                continue;
            };
            if named(&self.short(slot_ref), &self.short(pattern_ref)) && !jam.iter().any(|p| p == pattern_ref) {
                jam.push(pattern_ref.to_string());
            }
        }
        let mut parts: Vec<Part> = Vec::new();
        for (s, slot) in sequence.iter().enumerate() {
            let (Some(slot_ref), Some(pattern_ref)) =
                (slot.get("ref").and_then(Value::as_str), slot.get("patternRef").and_then(Value::as_str))
            else {
                continue;
            };
            let short = self.short(pattern_ref);
            if let Some(part) = parts.iter_mut().find(|part| part.reference == short) {
                part.slots.push((s + 1, self.short(slot_ref)));
                continue;
            }
            if parts.len() >= MOST_PATTERNS {
                break;
            }
            let row =
                patterns.iter().find(|row| row.get("ref").and_then(Value::as_str) == Some(pattern_ref)).cloned().unwrap_or(Value::Null);
            let number = row.get("number").and_then(Value::as_u64).unwrap_or(0);
            let name = row
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map_or_else(|| format!("Pattern {number:02}"), |name| format!("Pattern {number:02} \u{201c}{name}\u{201d}"));
            let length = row.get("lines").and_then(Value::as_u64).unwrap_or(64) as u32;
            let prints = row.get("prints").and_then(Value::as_object).cloned().unwrap_or_default();
            // (automation, or lines on a track that isn't a sequencer track: the master's or a send's commands)
            let holds_more = row.get("automation").and_then(Value::as_u64).unwrap_or(0) > 0
                || prints.keys().any(|key| !all.iter().any(|(number, _, _)| number.to_string() == *key));
            let read_hits = jam.iter().any(|p| p == pattern_ref);
            let mut part_tracks = Vec::new();
            for (number, reference, track_name) in &sequencer {
                let print = prints.get(&number.to_string()).and_then(Value::as_str).unwrap_or("").to_string();
                let alias = print.starts_with("alias of");
                let notes = print.split_once(" notes").and_then(|(count, _)| count.parse::<usize>().ok()).unwrap_or(0);
                let hits = if read_hits && !alias && !print.is_empty() {
                    hits_of(&self.lines(pattern_ref, reference, length, lpb, signal).await?)
                } else {
                    Vec::new()
                };
                part_tracks.push(TrackPart {
                    track: self.short(reference),
                    name: track_name.clone(),
                    empty: print.is_empty(),
                    digest: print,
                    alias,
                    notes,
                    hits,
                });
            }
            parts.push(Part {
                reference: short,
                name,
                slots: vec![(s + 1, self.short(slot_ref))],
                lines: length,
                lpb,
                tracks: part_tracks,
                holds_more,
            });
        }
        Ok(parts)
    }

    fn note_name(&self, note: i64) -> String {
        note_text(note.clamp(0, 119) as u8)
    }

    async fn rewrite(&self, part: &Part, track: &TrackPart, edits: &[Edit], signal: &Signal) -> Result<(Value, Vec<String>), String> {
        let (pattern, track_ref) = (self.plan.lengthen(&part.reference)?, self.plan.lengthen(&track.track)?);
        let length = self
            .rows(json!({"kind": "pattern", "limit": 200}), signal)
            .await?
            .iter()
            .find(|row| row.get("ref").and_then(Value::as_str) == Some(pattern.as_str()))
            .and_then(|row| row.get("lines").and_then(Value::as_u64))
            .ok_or_else(|| format!("{} isn't in the song any more: look again", part.name))? as u32;
        let before = self.lines(&pattern, &track_ref, length, part.lpb, signal).await?;
        if hits_of(&before) != track.hits {
            return Err(format!("\u{201c}{}\u{201d} in {} changed since the look: look again", track.name, part.name));
        }
        let mut after = before.clone();
        let said = apply(&mut after, edits, &track.name);
        let lines = after.rewrite(&before, 0);
        if lines.is_empty() {
            return Ok((json!({"changed": "nothing to write"}), said));
        }
        let input = json!({"patternRef": part.reference, "trackRef": track.track, "lines": lines});
        let outcome = self.plan.change_by("notes_write", input.as_object().cloned().unwrap_or_default(), signal.clone()).await;
        let answer: Value = serde_json::from_str(&outcome.text).unwrap_or_else(|_| json!({"said": outcome.text}));
        if outcome.is_error || outcome.stops {
            return Err(answer.get("changed").and_then(Value::as_str).map(str::to_owned).unwrap_or(outcome.text));
        }
        Ok((answer, said))
    }

    async fn alias(&self, part: &str, track: &str, of: &str, signal: &Signal) -> Result<Value, String> {
        self.change("pattern_alias", json!({"ref": part, "trackRef": track, "aliasRef": of}), signal).await
    }

    async fn replay(&self, slot: &str, of: &str, signal: &Signal) -> Result<Value, String> {
        self.change("slot_pattern", json!({"ref": slot, "patternRef": of}), signal).await
    }

    async fn drop_slot(&self, slot: &str, signal: &Signal) -> Result<Value, String> {
        self.change("slot_delete", json!({"ref": slot}), signal).await
    }
}

impl<S: RefScheme + 'static> RenoiseJam<S> {
    async fn change(&self, tool: &str, input: Value, signal: &Signal) -> Result<Value, String> {
        let outcome = self.plan.change_by(tool, input.as_object().cloned().unwrap_or_default(), signal.clone()).await;
        let answer: Value = serde_json::from_str(&outcome.text).unwrap_or_else(|_| json!({"said": outcome.text}));
        if outcome.is_error || outcome.stops {
            return Err(answer.get("changed").and_then(Value::as_str).map(str::to_owned).unwrap_or(outcome.text));
        }
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::CHANGES;
    use kumi_runtime::kernel::schema::problem;

    fn lines() -> Lines {
        let rows = vec![
            json!({"line": 4, "notes": [{"col": 1, "note": "C-4", "instrument": "00", "delay": "80"}]}),
            json!({"line": 8, "notes": [{"col": 1, "note": "D-4", "instrument": "00"}]}),
            json!({"line": 9, "notes": [{"col": 1, "note": "E-4", "instrument": "00"}]}),
        ];
        Lines::from_rows(&rows, 0, 15, 4)
    }

    #[test]
    fn a_moved_note_keeps_its_column_and_a_delay_and_a_taken_cell_leaves_it_where_it_was() {
        let mut moved = lines();
        let hits = hits_of(&moved);
        assert_eq!((hits[0].line, hits[0].at), (4, 4.5));
        let said =
            apply(&mut moved, &[Edit::Move(hits[0].clone(), 4.0), Edit::Remove(hits[1].clone()), Edit::Move(hits[2].clone(), 8.0)], "Keys");
        let now = hits_of(&moved);
        // (Renoise's C-4 is 48: C-5 is MIDI's 60)
        assert_eq!(now.iter().map(|h| (h.line, h.at, h.note)).collect::<Vec<_>>(), [(4, 4.0, 48), (8, 8.0, 52)]);
        assert!(said.is_empty(), "{said:?}");
        // Onto a taken cell: it stays.
        let mut taken = lines();
        let hits = hits_of(&taken);
        let said = apply(&mut taken, &[Edit::Move(hits[2].clone(), 8.0)], "Keys");
        assert_eq!(said.len(), 1, "{said:?}");
        assert_eq!(hits_of(&taken).len(), 3);
        assert_eq!(line_and_delay(4.999, 16), (5, 0));
        assert_eq!(line_and_delay(4.25, 16), (4, 64));
    }

    #[test]
    fn every_fix_fits_its_renoise_kind() {
        let before = lines();
        let mut after = before.clone();
        let hits = hits_of(&before);
        apply(&mut after, &[Edit::Remove(hits[1].clone())], "Keys");
        for (tool, input) in [
            ("notes_write", json!({"patternRef": "pattern:1", "trackRef": "track:1", "lines": after.rewrite(&before, 0)})),
            ("pattern_alias", json!({"ref": "pattern:3", "trackRef": "track:2", "aliasRef": "pattern:1"})),
            ("slot_pattern", json!({"ref": "seq:6", "patternRef": "pattern:1"})),
            ("slot_delete", json!({"ref": "seq:4"})),
        ] {
            let kind = CHANGES.kinds.iter().find(|kind| kind.tool == tool).unwrap_or_else(|| panic!("{tool}"));
            assert_eq!(problem(&Value::Object(kind.input_schema()), &input), None, "{tool}: {input}");
        }
    }
}
