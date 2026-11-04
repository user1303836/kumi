//! A pooled MIDI take: REAPER keeps one set of MIDI for every item in the pool, so a change to one lands on them all (and its undo
//! puts them all back: lab, 7.82). What Kumi says of it, which the tool's answer and the HISTORY row carry.

use serde_json::Value;

use super::Ctx;
use crate::reaper::{Item, Take};

pub(super) const WILL_APPLY: &str = "REAPER will apply this to";
pub(super) const APPLIED: &str = "REAPER applied this to";
pub(super) const PUT_BACK: &str = "REAPER put this back in";

/// A source's pool, of its item's chunk: the take's by its place among the item's takes, each take after the first opening with
/// a `TAKE` line of the item's own.
fn pool_in(chunk: &str, take: usize) -> Option<&str> {
    let (mut depth, mut at) = (0, 0);
    for line in chunk.lines().map(str::trim) {
        if line.starts_with('<') {
            depth += 1;
        } else if line == ">" {
            depth -= 1;
        } else if depth == 1 && (line == "TAKE" || line.starts_with("TAKE ")) {
            at += 1;
        } else if at == take {
            if let Some(pool) = line.strip_prefix("POOLEDEVTS ") {
                return Some(pool.trim());
            }
        }
    }
    None
}

fn pooled(ctx: &Ctx, take: Take) -> bool {
    ctx.reaper.take_source_type(take).is_some_and(|kind| kind.eq_ignore_ascii_case("MIDIPOOL"))
}

/// The pool of a take of `item`, at its place.
fn pool_of(ctx: &Ctx, item: Item, take: Take) -> Option<String> {
    let at = (0..ctx.reaper.take_count(item)).position(|k| ctx.reaper.take_at(item, k) == Some(take))?;
    pool_in(&ctx.reaper.item_chunk(item)?, at).map(str::to_owned)
}

/// How many items of the project hold this pool: only pooled takes' items' chunks are read, so a project without pools pays nothing
/// and one with pools pays for those. None when it can't be said.
fn items_in(ctx: &Ctx, take: Take) -> Option<usize> {
    let reaper = ctx.reaper;
    let pool = pool_of(ctx, reaper.take_item(take)?, take)?;
    let mut items = 0;
    for track in (0..reaper.track_count(ctx.proj)).filter_map(|t| reaper.track_at(ctx.proj, t)) {
        for item in (0..reaper.track_item_count(track)).filter_map(|i| reaper.track_item(track, i)) {
            let holds = (0..reaper.take_count(item))
                .filter_map(|k| reaper.take_at(item, k))
                .any(|other| pooled(ctx, other) && pool_of(ctx, item, other).as_deref() == Some(pool.as_str()));
            items += usize::from(holds);
        }
    }
    Some(items)
}

/// An answer's words with a pooled take's said: `done` ("REAPER applied this to") all its pool's items, by how many when that can be
/// told. A take that isn't pooled's answer as it is.
pub(super) fn said(ctx: &Ctx, take: Take, done: &str, mut display: Value) -> Value {
    if !pooled(ctx, take) {
        return display;
    }
    let words = match items_in(ctx, take) {
        // (the only one left in its pool: it reaches nothing else)
        Some(0 | 1) => return display,
        Some(items) => format!("{done} all {items} items in this take's MIDI pool."),
        None => format!("{done} every item in this take's MIDI pool."),
    };
    display["note"] = match display.get("note").and_then(Value::as_str) {
        Some(note) => format!("{note} {words}"),
        None => words,
    }
    .into();
    display
}

#[cfg(test)]
mod tests {
    use super::pool_in;

    /// A take's own pool, by its place, past another take's source and FX of their own.
    #[test]
    fn a_takes_pool_is_read_at_its_place_in_its_items_chunk() {
        let chunk = "<ITEM\nNAME \"a\"\n<SOURCE MIDIPOOL\nPOOLEDEVTS {A}\n>\nTAKE\nNAME \"b\"\n<SOURCE WAVE\nFILE \"b.wav\"\n>\nTAKE SEL\n<SOURCE MIDIPOOL\nPOOLEDEVTS {C}\n>\n>";
        assert_eq!(pool_in(chunk, 0), Some("{A}"));
        assert_eq!(pool_in(chunk, 1), None, "an audio take has none");
        assert_eq!(pool_in(chunk, 2), Some("{C}"));
        assert_eq!(pool_in(chunk, 3), None);
    }
}
