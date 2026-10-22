//! Cleaning up a jam (#401), built once for every DAW: what was recorded (the parts named, the new ones since last time) set
//! against what came before, and what to propose (`look`): parts that are empty, the same as an earlier one, or nearly; tracks
//! that repeat an earlier part's track exactly (an alias plays it); and notes that read as mistakes rather than feel (a double
//! trigger, a note far off the grid where its neighbours aren't, a stray outside what the track plays), the feel told apart by
//! where each beat position's notes sit. Then `fix` applies what the producer accepts, and quantizes toward the grid by a
//! strength, keeping the feel if asked. The DAW's backend (`backend`) reads its parts and writes the fixes.
pub mod backend;
pub mod tool;

#[cfg(test)]
mod fake_backend_tests;

use std::collections::BTreeMap;

/// A note as the analysis reads it: where it starts (lines from its part's start, delay included), its note number, its
/// velocity (0 to 1), and where it's written (line, column), for a fix to find it.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub at: f64,
    pub note: i64,
    pub velocity: f64,
    pub line: u32,
    pub col: u8,
}

/// A track's part of a pattern: its ref and name, what it holds (the same digest is the same lines), how many notes, and its
/// hits (read only for the jam's parts).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackPart {
    pub track: String,
    pub name: String,
    pub digest: String,
    pub empty: bool,
    /// It plays another part's lines already (an alias).
    pub alias: bool,
    pub notes: usize,
    pub hits: Vec<Hit>,
}

/// A part: its ref and name, the slots that play it (their number, from 1, and ref), its lines per beat, its tracks, and
/// whether it holds more than their lines (automation, or lines on tracks not read: a master's or a send's commands), which
/// no slot-level change may drop.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub reference: String,
    pub name: String,
    pub slots: Vec<(usize, String)>,
    pub lpb: u32,
    pub tracks: Vec<TrackPart>,
    pub holds_more: bool,
}

impl Part {
    pub fn empty(&self) -> bool {
        self.tracks.iter().all(|track| track.empty)
    }

    fn track(&self, reference: &str) -> Option<&TrackPart> {
        self.tracks.iter().find(|track| track.track == reference)
    }

    /// The same lines as `other` on every track.
    pub fn same_as(&self, other: &Part) -> bool {
        !self.empty()
            && self.tracks.len() == other.tracks.len()
            && self.tracks.iter().all(|track| other.track(&track.track).is_some_and(|theirs| theirs.digest == track.digest))
    }

    /// How it differs from `other` when only by a few notes (at most `most`), in words; None when more, or the same.
    pub fn near(&self, other: &Part, most: usize) -> Option<String> {
        if self.empty() || other.empty() || self.same_as(other) {
            return None;
        }
        let mut said = Vec::new();
        let mut count = 0;
        for track in &self.tracks {
            let theirs = other.track(&track.track)?;
            if track.digest == theirs.digest {
                continue;
            }
            // (told by how many notes each has: the lines of what came before aren't read)
            let notes = |n: usize| if n == 1 { "1 note".to_string() } else { format!("{n} notes") };
            if track.notes == theirs.notes {
                count += 1;
                said.push(format!("other notes or effects on \u{201c}{}\u{201d}", track.name));
            } else if track.notes > theirs.notes {
                count += track.notes - theirs.notes;
                said.push(format!("{} more on \u{201c}{}\u{201d}", notes(track.notes - theirs.notes), track.name));
            } else {
                count += theirs.notes - track.notes;
                said.push(format!("{} fewer on \u{201c}{}\u{201d}", notes(theirs.notes - track.notes), track.name));
            }
        }
        (count <= most && !said.is_empty()).then(|| said.join(", "))
    }
}

/// The grid line nearest a position (lines), a tie going to the line it's written on (half a line late is late, not early).
pub fn nearest(at: f64) -> f64 {
    (at - 0.5).ceil()
}

/// Where a beat position's notes sit off the grid, by position within the beat (line within it): the median offset, in
/// lines, where three or more notes say so. A late offbeat everywhere is a swing: the feel, not a mistake.
pub fn feel(hits: &[Hit], lpb: u32) -> BTreeMap<u32, f64> {
    let mut by_class: BTreeMap<u32, Vec<f64>> = BTreeMap::new();
    for hit in hits {
        let at = nearest(hit.at);
        by_class.entry((at as u32) % lpb.max(1)).or_default().push(hit.at - at);
    }
    by_class
        .into_iter()
        .filter(|(_, offsets)| offsets.len() >= 3)
        .map(|(class, mut offsets)| {
            offsets.sort_by(f64::total_cmp);
            (class, offsets[offsets.len() / 2])
        })
        .collect()
}

/// A note that reads as a mistake: its kind and what to say.
#[derive(Debug, Clone, PartialEq)]
pub struct Mistake {
    pub hit: Hit,
    pub kind: &'static str,
    pub says: String,
}

/// How far off its class's feel a note may sit before it's a mistake (lines).
const LOOSE: f64 = 0.35;
/// Two hits of one note this close are a double trigger (lines).
const DOUBLE: f64 = 0.25;

/// The mistakes among a track's hits: double triggers, notes far off the grid where their neighbours aren't, and strays (on a
/// pitched track, a note class played once that the rest never plays). `classes`: how often each note class plays on the
/// track across the parts looked at.
pub fn mistakes(
    hits: &[Hit],
    lpb: u32,
    feel: &BTreeMap<u32, f64>,
    classes: &BTreeMap<i64, usize>,
    name: &dyn Fn(i64) -> String,
) -> Vec<Mistake> {
    let mut out = Vec::new();
    let mut sorted: Vec<&Hit> = hits.iter().collect();
    sorted.sort_by(|a, b| a.at.total_cmp(&b.at));
    for (k, hit) in sorted.iter().enumerate() {
        if let Some(earlier) = sorted[..k].iter().rev().find(|other| other.note == hit.note && hit.at - other.at < DOUBLE) {
            out.push(Mistake {
                hit: (*hit).clone(),
                kind: "double",
                says: format!("{} twice at line {:02}: a double trigger", name(hit.note), earlier.line),
            });
            continue;
        }
        let grid = nearest(hit.at);
        let expected = feel.get(&((grid as u32) % lpb.max(1))).copied().unwrap_or(0.);
        let off = hit.at - grid - expected;
        if off.abs() > LOOSE {
            out.push(Mistake {
                hit: (*hit).clone(),
                kind: "timing",
                says: format!(
                    "{} at line {:02} sits {:.2} lines off the grid, where its neighbours don't",
                    name(hit.note),
                    hit.line,
                    hit.at - grid
                ),
            });
            continue;
        }
        let pitched = classes.len() >= 5;
        let class = hit.note.rem_euclid(12);
        if pitched && classes.get(&class).copied().unwrap_or(0) == 1 {
            out.push(Mistake {
                hit: (*hit).clone(),
                kind: "stray",
                says: format!("{} at line {:02}: a note the track plays nowhere else", name(hit.note), hit.line),
            });
        }
    }
    out
}

/// Where a note goes quantized: toward the nearest `grid` (lines) by `strength` (0 to 1), its class's feel kept when asked.
pub fn quantized(at: f64, grid: f64, strength: f64, feel: Option<f64>) -> f64 {
    let grid = grid.max(1e-3);
    let target = nearest(at / grid) * grid + feel.unwrap_or(0.);
    at + (target - at) * strength.clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(at: f64, note: i64) -> Hit {
        Hit { at, note, velocity: 0.8, line: at.floor() as u32, col: 1 }
    }

    #[test]
    fn a_late_offbeat_everywhere_is_feel_and_one_far_off_note_is_a_mistake() {
        // (lpb 4: the offbeats, lines 2 and 6, a quarter line late each time; one note on line 4 sits half a line late)
        let hits = vec![hit(0., 36), hit(2.25, 42), hit(4.5, 38), hit(6.25, 42), hit(8., 36), hit(10.25, 42), hit(12., 38), hit(14.25, 42)];
        let feel = feel(&hits, 4);
        assert_eq!(feel.get(&2), Some(&0.25), "the swing");
        let classes = BTreeMap::new();
        let found = mistakes(&hits, 4, &feel, &classes, &|n| n.to_string());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].kind, found[0].hit.at), ("timing", 4.5));
        // A double trigger.
        let doubled = vec![hit(0., 36), hit(0.1, 36), hit(4., 36)];
        assert_eq!(mistakes(&doubled, 4, &BTreeMap::new(), &classes, &|n| n.to_string())[0].kind, "double");
    }

    #[test]
    fn a_stray_is_a_class_a_pitched_track_plays_once_and_quantizing_has_a_strength() {
        let notes = [60, 62, 64, 65, 67, 60, 62, 64, 65, 67, 61];
        let hits: Vec<Hit> = notes.iter().enumerate().map(|(k, n)| hit(k as f64 * 2., *n)).collect();
        let mut classes = BTreeMap::new();
        for n in notes {
            *classes.entry(n % 12).or_insert(0) += 1;
        }
        let found = mistakes(&hits, 4, &BTreeMap::new(), &classes, &|n| n.to_string());
        assert_eq!(found.iter().map(|m| (m.kind, m.hit.note)).collect::<Vec<_>>(), [("stray", 61)]);
        assert_eq!(quantized(4.3, 1., 0.5, None), 4.15);
        assert_eq!(quantized(4.3, 1., 1., Some(0.25)), 4.25, "the feel kept");
    }

    #[test]
    fn parts_are_the_same_or_near_by_their_tracks() {
        let track = |digest: &str, hits: Vec<Hit>| TrackPart {
            track: "t1".into(),
            name: "Keys".into(),
            digest: digest.into(),
            empty: hits.is_empty(),
            alias: false,
            notes: hits.len(),
            hits,
        };
        let part =
            |tracks: Vec<TrackPart>| Part { reference: "p".into(), name: String::new(), slots: vec![], lpb: 4, tracks, holds_more: false };
        let a = part(vec![track("x", vec![hit(0., 60), hit(8., 62)])]);
        let b = part(vec![track("x", vec![hit(0., 60), hit(8., 62)])]);
        let c = part(vec![track("y", vec![hit(0., 60), hit(8., 62), hit(12., 67)])]);
        assert!(a.same_as(&b) && !a.same_as(&c));
        assert_eq!(c.near(&a, 3).as_deref(), Some("1 note more on \u{201c}Keys\u{201d}"));
        assert!(part(vec![track("", vec![])]).empty());
    }
}
