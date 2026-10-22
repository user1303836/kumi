//! Sample mapping (M7, #401): a set of samples (files, or an instrument's own) laid out across the keys, built once for every
//! DAW. Core reads what each name says (`names`), hears each sample's pitch and loudness (`heard`), settles which octave
//! convention the names use by checking them against the pitch heard, and decides roots, zones, velocity layers and round
//! robins (`plan`); the tool (`tool`) answers the plan in the DAW's words and makes it as one change through the DAW's backend
//! (`backend`), which holds only the DAW's facts: its note names and range, and the change that makes an instrument.
pub mod backend;
pub mod heard;
pub mod names;
pub mod tool;

use names::{midi_of, Hints, Velocity, Written};
use std::collections::{BTreeMap, BTreeSet};

/// The most samples one mapping takes (a sampler's 120 keys).
pub const MOST_SAMPLES: usize = 120;
/// A pitch heard this sure (the share of voiced frames) places a sample, or checks its name.
const SURE_PITCH: f64 = 0.5;
/// Samples at one key closer in level than this are round robins, not velocity layers (dB).
const LAYER_SPREAD_DB: f64 = 3.0;

/// What a sample sounds like, heard from its audio: its pitch as a MIDI note (fractional), how sure, and its level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeardSample {
    pub pitch: Option<f64>,
    pub sure: f64,
    /// Loudness of its loudest 400 ms (K-weighted, dB).
    pub level_db: f64,
    pub peak_db: f64,
    pub seconds: f64,
}

/// One sample to map: its id (a file's path, or the DAW's ref), its name, what the name says and what was heard; for a sample
/// the DAW has, how far it's transposed and whether it plays as a key lets go (the note-off layer).
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub id: String,
    pub name: String,
    pub hints: Hints,
    pub heard: Option<HeardSample>,
    pub transpose: i32,
    pub note_off: bool,
}

impl Sample {
    pub fn new(id: impl Into<String>, name: impl Into<String>, heard: Option<HeardSample>) -> Self {
        let name = name.into();
        Sample { id: id.into(), hints: names::hints(&name), name, heard, transpose: 0, note_off: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// A multi-sample: each root's zone reaches halfway to its neighbours'.
    Keys,
    /// A key per sound.
    Drums,
}

/// The octave number the names give middle C (MIDI 60): 3, 4 (scientific pitch) or 5 (Renoise and the trackers); and how
/// that was settled.
#[derive(Debug, Clone, PartialEq)]
pub struct Convention {
    pub middle_c: i32,
    pub how: String,
}

/// The octave numbers names give middle C.
pub const MIDDLE_CS: [i32; 3] = [3, 4, 5];

/// What the producer (or the model) settled before the plan: the layout, the convention, roots by sample, GM drum keys, and the
/// DAW's note range and first key.
#[derive(Debug, Clone, Default)]
pub struct Asked {
    pub layout: Option<Layout>,
    pub middle_c: Option<i32>,
    pub roots: BTreeMap<String, i32>,
    pub gm: bool,
    /// The lowest and highest MIDI notes the DAW's sampler maps, and where a drum kit starts.
    pub lowest: i32,
    pub highest: i32,
    pub first_key: i32,
}

/// How a sample's root was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    Named,
    Heard,
    Given,
    Drum,
    Next,
}

impl Found {
    pub fn word(self) -> &'static str {
        match self {
            Found::Named => "its name",
            Found::Heard => "its pitch",
            Found::Given => "as asked",
            Found::Drum => "its GM key",
            Found::Next => "the next key",
        }
    }
}

/// A sample as it's placed: its root (the key it sounds as recorded), the base note that makes it so (its root and its
/// transpose), its zone, its velocity range and which round robin it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub id: String,
    pub name: String,
    pub root: i32,
    pub base: i32,
    pub zone: (i32, i32),
    pub velocity: (u8, u8),
    pub round_robin: Option<usize>,
    pub found: Found,
}

/// One instrument the plan makes: what tells it apart (an articulation; None for the whole set), whether its round robins
/// cycle, and its samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub part: Option<String>,
    pub cycle: bool,
    pub samples: Vec<Placed>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub layout: Layout,
    pub convention: Option<Convention>,
    pub instruments: Vec<Planned>,
    /// What Kumi noticed and decided (said with the plan).
    pub findings: Vec<String>,
    /// What only the producer can say; the plan stands without it.
    pub questions: Vec<String>,
    /// Samples the plan leaves out, and why.
    pub left_out: Vec<(String, String)>,
}

/// The convention the names use: middle C as C3, C4 or C5, by the pitch heard where both a note and a pitch are known (the
/// most that agree; on a tie, C5 for tracker spellings, else C4). None when nothing can tell (no named note, no pitch heard).
fn convention(samples: &[Sample], asked: Option<i32>, tracker: bool) -> Option<Convention> {
    let named: Vec<&Sample> = samples.iter().filter(|s| matches!(s.hints.note, Some(Written::Named { .. }))).collect();
    if named.is_empty() {
        return None;
    }
    if let Some(middle_c) = asked {
        return Some(Convention { middle_c, how: "as asked".into() });
    }
    let compared: Vec<f64> = named
        .iter()
        .filter_map(|s| {
            let heard = s.heard.filter(|heard| heard.sure >= SURE_PITCH)?.pitch?;
            Some(heard - midi_of(s.hints.note?, 4)? as f64)
        })
        .collect();
    if compared.is_empty() {
        return None;
    }
    // (each convention moves the names by whole octaves: count how many agree with each, the preferred first on a tie)
    let agree = |middle_c: i32| compared.iter().filter(|diff| (**diff - 12. * (4 - middle_c) as f64).abs() <= 1.0).count();
    let order: [i32; 3] = if tracker { [5, 4, 3] } else { [4, 3, 5] };
    let mut middle_c = order[0];
    for candidate in order {
        if agree(candidate) > agree(middle_c) {
            middle_c = candidate;
        }
    }
    let best = agree(middle_c);
    let how = if best * 2 > compared.len() {
        format!("the pitch heard agrees in {best} of {}", compared.len())
    } else {
        format!("the pitch heard agrees in only {best} of {}: check a few by ear", compared.len())
    };
    Some(Convention { middle_c, how })
}

/// Zero-padded numbers ("001") are a sample's MIDI note only when its pitch agrees (an octave either way), or, with nothing
/// heard, when the set's numbers don't count takes: a run from 000 or 001, gaps and all (as many numbers as half its span).
fn settle_padded(samples: &mut [Sample], findings: &mut Vec<String>) {
    let padded = |s: &Sample| match s.hints.note {
        Some(Written::Padded(n)) => Some(n),
        _ => None,
    };
    let heard = |s: &Sample| s.heard.filter(|heard| heard.sure >= SURE_PITCH).and_then(|heard| heard.pitch);
    let mut unheard: Vec<u8> = samples.iter().filter(|s| heard(s).is_none()).filter_map(padded).collect();
    unheard.sort_unstable();
    unheard.dedup();
    let run = match (unheard.first(), unheard.last()) {
        (Some(low), Some(high)) => *low <= 1 && (*high - *low) as usize + 1 <= 2 * unheard.len(),
        _ => false,
    };
    let mut dropped = 0;
    for sample in samples.iter_mut() {
        let Some(n) = padded(sample) else { continue };
        let keep = match heard(sample) {
            Some(pitch) => {
                let off = pitch - n as f64;
                (off - 12. * (off / 12.).round()).abs() <= 1.0
            }
            None => !run,
        };
        if !keep {
            sample.hints.note = None;
            dropped += 1;
        }
    }
    if dropped > 0 {
        findings.push(format!(
            "{dropped} name{} end in a zero-padded number that isn't the note heard, or that counts takes (001, 002…): not read as notes",
            if dropped == 1 { "" } else { "s" }
        ));
    }
}

/// The order of a velocity hint within one key's samples (softest first), or None.
fn velocity_order(velocity: Option<Velocity>) -> Option<(u8, u32)> {
    match velocity? {
        Velocity::Layer(n) => Some((0, n)),
        Velocity::Dynamic(level) => Some((1, level as u32)),
        Velocity::Value(value) => Some((2, value as u32)),
    }
}

/// Velocity ranges for `count` layers, softest first: split evenly, or at the midpoints between the values the names give.
fn velocity_ranges(values: &[Option<u8>]) -> Vec<(u8, u8)> {
    let count = values.len().max(1);
    if values.iter().all(Option::is_some) && count > 1 {
        let values: Vec<u32> = values.iter().map(|value| value.unwrap_or(0) as u32).collect();
        let mut out = Vec::new();
        for k in 0..count {
            let low = if k == 0 { 0 } else { (values[k - 1] + values[k]) / 2 + 1 };
            let high = if k + 1 == count { 127 } else { (values[k] + values[k + 1]) / 2 };
            out.push((low.min(127) as u8, high.clamp(low, 127) as u8));
        }
        return out;
    }
    (0..count)
        .map(|k| {
            let low = (128 * k / count) as u8;
            let high = (128 * (k + 1) / count - 1) as u8;
            (low, high)
        })
        .collect()
}

/// A key's samples in layers (softest first), each layer's samples its round robins; what was decided, in words.
fn layers_of(key: &[&Sample], at: &str) -> (Vec<Vec<usize>>, Vec<Option<u8>>, Vec<String>) {
    let mut said = Vec::new();
    if key.len() == 1 {
        return (vec![vec![0]], vec![None], said);
    }
    let orders: Vec<Option<(u8, u32)>> = key.iter().map(|s| velocity_order(s.hints.velocity)).collect();
    let mut layers: Vec<Vec<usize>> = Vec::new();
    let mut values: Vec<Option<u8>> = Vec::new();
    if orders.iter().any(Option::is_some) {
        // By the names' velocities; samples with the same one (or none) are that layer's round robins.
        let mut by: BTreeMap<Option<(u8, u32)>, Vec<usize>> = BTreeMap::new();
        for (k, order) in orders.iter().enumerate() {
            by.entry(*order).or_default().push(k);
        }
        if let Some(none) = by.remove(&None) {
            said.push(format!(
                "{} at {at} {} no velocity in the name: played with the softest layer",
                names_of(key, &none),
                if none.len() == 1 { "has" } else { "have" }
            ));
            by.entry(by.keys().next().copied().flatten()).or_default().extend(none);
        }
        for (order, members) in by {
            values.push(order.filter(|(kind, _)| *kind == 2).map(|(_, value)| value as u8));
            layers.push(members);
        }
        // The names' order should be the loudness order.
        let levels: Vec<Option<f64>> = layers
            .iter()
            .map(|members| {
                let heard: Vec<f64> = members.iter().filter_map(|k| key[*k].heard.map(|heard| heard.level_db)).collect();
                (!heard.is_empty()).then(|| heard.iter().sum::<f64>() / heard.len() as f64)
            })
            .collect();
        for k in 1..levels.len() {
            if let (Some(soft), Some(loud)) = (levels[k - 1], levels[k]) {
                if loud + 1.0 < soft {
                    said.push(format!(
                        "at {at}, {} is quieter than {} ({:.1} dB): its name puts it in the louder layer, and it stays there",
                        names_of(key, &layers[k]),
                        names_of(key, &layers[k - 1]),
                        soft - loud
                    ));
                }
            }
        }
    } else {
        // No velocity in the names: round robins when they name them or sound alike; layers by loudness otherwise.
        let named_robins = key.iter().filter(|s| s.hints.round_robin.is_some()).count();
        let levels: Vec<f64> = key.iter().map(|s| s.heard.map_or(0., |heard| heard.level_db)).collect();
        let heard_all = key.iter().all(|s| s.heard.is_some());
        let spread = levels.iter().copied().fold(f64::MIN, f64::max) - levels.iter().copied().fold(f64::MAX, f64::min);
        if named_robins == key.len() || !heard_all || spread < LAYER_SPREAD_DB {
            if named_robins < key.len() {
                let why = if heard_all { format!("within {spread:.1} dB of each other") } else { "not heard".to_string() };
                said.push(format!(
                    "{} share {at} with no velocity or round robin in their names ({why}): round robins",
                    names_of(key, &(0..key.len()).collect::<Vec<_>>())
                ));
            }
            let mut all: Vec<usize> = (0..key.len()).collect();
            all.sort_by_key(|k| key[*k].hints.round_robin.unwrap_or(u32::MAX));
            layers.push(all);
            values.push(None);
        } else {
            let mut order: Vec<usize> = (0..key.len()).collect();
            order.sort_by(|a, b| levels[*a].total_cmp(&levels[*b]));
            said.push(format!(
                "{} share {at} with no velocity in their names: layered by loudness, {spread:.1} dB apart",
                names_of(key, &order)
            ));
            for k in order {
                layers.push(vec![k]);
                values.push(None);
            }
        }
    }
    (layers, values, said)
}

fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

fn names_of(key: &[&Sample], members: &[usize]) -> String {
    let names: Vec<String> = members.iter().map(|k| quoted(&key[*k].name)).collect();
    names.join(", ")
}

/// Zones between neighbouring roots: each reaches halfway to the next (the lower one takes the middle key), the lowest from
/// the bottom of the range and the highest to its top.
fn zones(roots: &[i32], lowest: i32, highest: i32) -> BTreeMap<i32, (i32, i32)> {
    let mut out = BTreeMap::new();
    for (k, root) in roots.iter().enumerate() {
        let low = if k == 0 { lowest } else { (roots[k - 1] + root).div_euclid(2) + 1 };
        let high = if k + 1 == roots.len() { highest } else { (root + roots[k + 1]).div_euclid(2) };
        out.insert(*root, (low, high));
    }
    out
}

/// The plan for a set of samples: the layout (as asked, or a drum kit when most samples are drums and few have a pitch),
/// the convention, and every sample's place.
pub fn plan(samples: &[Sample], asked: &Asked, note_name: &dyn Fn(i32) -> String) -> Plan {
    let mut findings = Vec::new();
    let mut questions = Vec::new();
    let mut left_out = Vec::new();
    let mut samples = samples.to_vec();
    settle_padded(&mut samples, &mut findings);
    let samples = &samples[..];
    let pitched = samples
        .iter()
        .filter(|s| s.hints.note.is_some() || s.heard.is_some_and(|heard| heard.sure >= SURE_PITCH && heard.pitch.is_some()))
        .filter(|s| s.hints.drum.is_none())
        .count();
    let drums = samples.iter().filter(|s| s.hints.drum.is_some()).count();
    let layout = asked.layout.unwrap_or(if pitched * 2 > samples.len() && pitched >= drums { Layout::Keys } else { Layout::Drums });
    if asked.layout.is_none() {
        findings.push(match layout {
            Layout::Keys => {
                format!("{pitched} of {} samples have a note in the name or a clear pitch: laid across the keys", samples.len())
            }
            Layout::Drums => {
                format!("{} of {} samples have no note to play at: a key each, as a drum kit", samples.len() - pitched, samples.len())
            }
        });
    }
    let tracker = samples.iter().any(|s| matches!(s.hints.note, Some(Written::Named { .. })) && names::tracker_spelling(&s.name));
    let convention = convention(samples, asked.middle_c, tracker).or_else(|| {
        // (nothing heard to check: tracker spellings name middle C C-5)
        tracker.then(|| Convention { middle_c: 5, how: "tracker notes, where middle C is C-5: no pitch was heard to check".into() })
    });
    let named = samples.iter().any(|s| matches!(s.hints.note, Some(Written::Named { .. })));
    if named && convention.is_none() {
        questions.push(
            "Do the names put middle C at C3, C4 or C5? No pitch could be heard to check: planned as C4 (MIDI 60); say middleC if the names are an octave off".into(),
        );
    }
    let middle_c = convention.as_ref().map_or(4, |c| c.middle_c);
    let instruments = match layout {
        Layout::Keys => keys(samples, asked, middle_c, note_name, &mut findings, &mut left_out),
        Layout::Drums => drum_kit(samples, asked, note_name, &mut findings, &mut left_out),
    };
    Plan { layout, convention, instruments, findings, questions, left_out }
}

/// Why a sample isn't laid out with the rest: it plays as a key lets go.
fn released(sample: &Sample) -> Option<&'static str> {
    if sample.note_off {
        Some("it plays on the note-off layer, as a key lets go: left as it is")
    } else if sample.hints.release {
        Some("a release sample (it plays when a key lets go): map it on the note-off layer by hand")
    } else {
        None
    }
}

fn keys(
    samples: &[Sample],
    asked: &Asked,
    middle_c: i32,
    note_name: &dyn Fn(i32) -> String,
    findings: &mut Vec<String>,
    left_out: &mut Vec<(String, String)>,
) -> Vec<Planned> {
    // Each sample's root: as asked, by its name, else by its pitch.
    let mut rooted: Vec<(&Sample, i32, Found)> = Vec::new();
    for sample in samples {
        if let Some(why) = released(sample) {
            left_out.push((sample.name.clone(), why.into()));
            continue;
        }
        let heard = sample.heard.filter(|heard| heard.sure >= SURE_PITCH).and_then(|heard| heard.pitch);
        let (root, found) = if let Some(root) = asked.roots.get(&sample.id).or_else(|| asked.roots.get(&sample.name)) {
            (*root, Found::Given)
        } else if let Some(root) = sample.hints.note.and_then(|note| midi_of(note, middle_c)) {
            if let Some(heard) = heard {
                let off = heard - root as f64;
                // (an octave either way is the pitch tracker's as often as the name's)
                let wrapped = off - 12. * (off / 12.).round();
                if wrapped.abs() > 1.0 {
                    findings.push(format!(
                        "{} sounds nearer {} than {} (its name): mapped by its name",
                        quoted(&sample.name),
                        note_name(heard.round() as i32),
                        note_name(root)
                    ));
                }
            }
            (root, Found::Named)
        } else if let Some(heard) = heard {
            (heard.round() as i32, Found::Heard)
        } else {
            left_out.push((sample.name.clone(), "no note in its name and no pitch heard: give its root".into()));
            continue;
        };
        if root < asked.lowest || root > asked.highest {
            left_out.push((
                sample.name.clone(),
                format!("its root {} is outside the keys ({} to {})", note_name(root), note_name(asked.lowest), note_name(asked.highest)),
            ));
            continue;
        }
        rooted.push((sample, root, found));
    }
    // An instrument per articulation; samples whose names say none are a part of their own (an untagged sample may be any).
    let mut parts: BTreeMap<Option<&'static str>, Vec<usize>> = BTreeMap::new();
    for (k, (sample, _, _)) in rooted.iter().enumerate() {
        parts.entry(sample.hints.articulation).or_default().push(k);
    }
    if parts.len() > 1 {
        let named: Vec<&str> = parts.keys().map(|part| part.unwrap_or(PLAIN)).collect();
        findings.push(format!("{} parts ({}): an instrument each, since one key can't choose between them", named.len(), named.join(", ")));
    }
    let single = parts.len() <= 1;
    let mut out = Vec::new();
    for (part, members) in parts {
        let mut by_root: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
        for k in &members {
            by_root.entry(rooted[*k].1).or_default().push(*k);
        }
        let roots: Vec<i32> = by_root.keys().copied().collect();
        let spans = zones(&roots, asked.lowest, asked.highest);
        let mut placed = Vec::new();
        let mut cycle = false;
        for (root, at_root) in &by_root {
            let key: Vec<&Sample> = at_root.iter().map(|k| rooted[*k].0).collect();
            let (layers, values, said) = layers_of(&key, &note_name(*root));
            findings.extend(said);
            let ranges = velocity_ranges(&values);
            for (layer, members) in layers.iter().enumerate() {
                cycle |= members.len() > 1;
                for (robin, k) in members.iter().enumerate() {
                    let (sample, _, found) = rooted[at_root[*k]];
                    placed.push(Placed {
                        id: sample.id.clone(),
                        name: sample.name.clone(),
                        root: *root,
                        base: (*root + sample.transpose).clamp(asked.lowest, asked.highest),
                        zone: spans[root],
                        velocity: ranges[layer],
                        round_robin: (members.len() > 1).then_some(robin + 1),
                        found,
                    });
                }
            }
        }
        if !placed.is_empty() {
            out.push(Planned { part: if single { None } else { Some(part.unwrap_or(PLAIN).to_owned()) }, cycle, samples: placed });
        }
    }
    out
}

/// The part of samples whose names name no articulation.
pub const PLAIN: &str = "plain";

/// GM's percussion keys end here: sounds with no GM key of their own go above it.
const PAST_GM: i32 = 82;

/// The order drum roles come in a kit laid out key by key.
const ROLE_ORDER: [&str; 14] = [
    "kick",
    "snare",
    "rim",
    "clap",
    "closed hat",
    "pedal hat",
    "open hat",
    "tom",
    "crash",
    "ride",
    "percussion",
    "shaker",
    "tambourine",
    "cowbell",
];

/// A drum sample's sound without its velocity or round robin: samples sharing it are one key's layers.
fn sound_of(sample: &Sample) -> String {
    let words = names::words(&sample.name);
    let mut kept = Vec::new();
    let mut k = 0;
    while k < words.len() {
        if names::is_tag(&words[k]) {
            // ("Vel 64", "rr 2": the number goes with its tag)
            if words.get(k + 1).is_some_and(|next| next.bytes().all(|b| b.is_ascii_digit()))
                && !words[k].bytes().any(|b| b.is_ascii_digit())
            {
                k += 1;
            }
        } else {
            kept.push(words[k].to_lowercase());
        }
        k += 1;
    }
    kept.join(" ")
}

fn drum_kit(
    samples: &[Sample],
    asked: &Asked,
    note_name: &dyn Fn(i32) -> String,
    findings: &mut Vec<String>,
    left_out: &mut Vec<(String, String)>,
) -> Vec<Planned> {
    // One key per sound (its velocity layers and round robins share it), in role order, then by name.
    let mut sounds: BTreeMap<String, Vec<&Sample>> = BTreeMap::new();
    for sample in samples {
        if sample.note_off {
            left_out.push((sample.name.clone(), released(sample).unwrap_or_default().into()));
            continue;
        }
        sounds.entry(sound_of(sample)).or_default().push(sample);
    }
    let mut order: Vec<(String, Vec<&Sample>)> = sounds.into_iter().collect();
    let rank = |group: &[&Sample]| {
        group[0].hints.drum.and_then(|(role, _)| ROLE_ORDER.iter().position(|r| *r == role)).unwrap_or(ROLE_ORDER.len())
    };
    order.sort_by(|(a_name, a), (b_name, b)| rank(a).cmp(&rank(b)).then(a_name.cmp(b_name)));
    let mut taken = BTreeSet::new();
    let mut keyed: Vec<(i32, Found, Vec<&Sample>)> = Vec::new();
    // GM keys first, when asked: the first sound of each role on its key; the rest after.
    let mut rest = Vec::new();
    for (_, group) in order {
        let given = asked.roots.get(&group[0].id).or_else(|| asked.roots.get(&group[0].name)).copied();
        if let Some(key) = given.filter(|key| !taken.contains(key)) {
            taken.insert(key);
            keyed.push((key, Found::Given, group));
        } else if let Some(key) =
            asked.gm.then(|| group[0].hints.drum.map(|(_, note)| note as i32)).flatten().filter(|key| !taken.contains(key))
        {
            taken.insert(key);
            keyed.push((key, Found::Drum, group));
        } else {
            rest.push(group);
        }
    }
    let mut next = if asked.gm { asked.first_key.max(PAST_GM) } else { asked.first_key };
    if asked.gm && !rest.is_empty() {
        findings.push(format!("{} sounds have no GM key of their own: from {} up", rest.len(), note_name(next)));
    }
    for group in rest {
        while taken.contains(&next) {
            next += 1;
        }
        if next > asked.highest {
            for sample in group {
                left_out.push((sample.name.clone(), format!("past the top of the keys ({})", note_name(asked.highest))));
            }
            continue;
        }
        taken.insert(next);
        keyed.push((next, Found::Next, group));
    }
    keyed.sort_by_key(|(key, _, _)| *key);
    let mut placed = Vec::new();
    let mut cycle = false;
    for (key, found, group) in keyed {
        let (layers, values, said) = layers_of(&group, &note_name(key));
        findings.extend(said);
        let ranges = velocity_ranges(&values);
        for (layer, members) in layers.iter().enumerate() {
            cycle |= members.len() > 1;
            for (robin, k) in members.iter().enumerate() {
                let sample = group[*k];
                placed.push(Placed {
                    id: sample.id.clone(),
                    name: sample.name.clone(),
                    root: key,
                    base: (key + sample.transpose).clamp(asked.lowest, asked.highest),
                    zone: (key, key),
                    velocity: ranges[layer],
                    round_robin: (members.len() > 1).then_some(robin + 1),
                    found,
                });
            }
        }
    }
    if placed.is_empty() {
        return Vec::new();
    }
    vec![Planned { part: None, cycle, samples: placed }]
}

#[cfg(test)]
mod fake_backend_tests;
#[cfg(test)]
mod tests;
