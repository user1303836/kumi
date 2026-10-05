//! In-memory sound, preset and Set search, including measured similarity and reasons for each hit.
pub use super::classify::tokens;
use super::{
    classify::{parse_key, ClassFrom, SoundClass, SoundKind, CLASSES},
    features::{VECTOR_GROUPS, VECTOR_LENGTH},
    learn::{extension, PresetEntry, SetEntry, SoundEntry},
    sets::{SetSummary, SetTrack},
    sources::{basename, Source, SEP},
    store::unpack_vector,
    taste::{colour_name, track_role},
};
use kumi_common::js::{
    number::{round, to_fixed, to_string},
    string::{locale_compare_numeric_base, trim},
};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    path::Path,
    sync::LazyLock,
};
fn plain_word(word: &str) -> String {
    word.to_lowercase().chars().filter(|c| *c != '-' && *c != '_' && !kumi_common::js::string::trim(&c.to_string()).is_empty()).collect()
}
pub fn class_for_word(word: &str) -> Option<SoundClass> {
    static WORDS: LazyLock<HashMap<String, SoundClass>> = LazyLock::new(|| {
        let mut words = HashMap::new();
        for class in CLASSES {
            words.insert(class.to_string(), class);
            words.insert(format!("{class}s"), class);
        }
        for (word, class) in [
            ("hihat", "hat"),
            ("hihats", "hat"),
            ("hh", "hat"),
            ("bd", "kick"),
            ("kik", "kick"),
            ("sd", "snare"),
            ("vox", "vocal"),
            ("vocals", "vocal"),
            ("voice", "vocal"),
            ("percussion", "perc"),
            ("crash", "cymbal"),
            ("ride", "cymbal"),
            ("808", "bass"),
            ("sub", "bass"),
            ("piano", "keys"),
            ("chord", "keys"),
            ("chords", "keys"),
            ("riser", "fx"),
            ("sfx", "fx"),
            ("impact", "fx"),
            ("atmos", "texture"),
            ("ambience", "texture"),
            ("drum", "drums"),
            ("break", "drums"),
            ("breaks", "drums"),
        ] {
            words.insert(word.into(), SoundClass::parse(class).unwrap());
        }
        words
    });
    WORDS.get(&plain_word(word)).copied()
}
#[derive(Clone, Copy)]
enum Measure {
    Brightness,
    Flatness,
    Attack,
    Decay,
    Seconds,
    Loudness,
    Width,
    Low,
    High,
    Onsets,
}
#[derive(Clone, Copy)]
struct Descriptor {
    measure: Measure,
    high: bool,
    says: &'static str,
}
fn descriptor(word: &str) -> Option<Descriptor> {
    let (measure, high, says) = match plain_word(word).as_str() {
        "dark" => (Measure::Brightness, false, "dark"),
        "warm" => (Measure::Brightness, false, "warm"),
        "muffled" => (Measure::Brightness, false, "muffled"),
        "mellow" => (Measure::Brightness, false, "mellow"),
        "dull" => (Measure::Brightness, false, "dull"),
        "deep" => (Measure::Low, true, "deep"),
        "bright" => (Measure::Brightness, true, "bright"),
        "crisp" => (Measure::Brightness, true, "crisp"),
        "airy" => (Measure::High, true, "airy"),
        "sharp" => (Measure::Attack, false, "sharp"),
        "harsh" => (Measure::Brightness, true, "harsh"),
        "shiny" => (Measure::Brightness, true, "shiny"),
        "punchy" => (Measure::Attack, false, "punchy"),
        "snappy" => (Measure::Decay, false, "snappy"),
        "tight" => (Measure::Decay, false, "tight"),
        "short" => (Measure::Seconds, false, "short"),
        "long" => (Measure::Seconds, true, "long"),
        "boomy" => (Measure::Decay, true, "boomy"),
        "big" => (Measure::Decay, true, "big"),
        "soft" => (Measure::Attack, true, "soft"),
        "slow" => (Measure::Attack, true, "slow"),
        "dusty" => (Measure::Flatness, true, "dusty"),
        "lofi" => (Measure::Flatness, true, "lo-fi"),
        "gritty" => (Measure::Flatness, true, "gritty"),
        "dirty" => (Measure::Flatness, true, "dirty"),
        "crunchy" => (Measure::Flatness, true, "crunchy"),
        "noisy" => (Measure::Flatness, true, "noisy"),
        "distorted" => (Measure::Flatness, true, "distorted"),
        "clean" => (Measure::Flatness, false, "clean"),
        "pure" => (Measure::Flatness, false, "pure"),
        "wide" => (Measure::Width, true, "wide"),
        "stereo" => (Measure::Width, true, "wide"),
        "mono" => (Measure::Width, false, "mono"),
        "narrow" => (Measure::Width, false, "narrow"),
        "loud" => (Measure::Loudness, true, "loud"),
        "quiet" => (Measure::Loudness, false, "quiet"),
        "heavy" => (Measure::Low, true, "heavy"),
        "fat" => (Measure::Low, true, "fat"),
        "thick" => (Measure::Low, true, "thick"),
        "thin" => (Measure::Low, false, "thin"),
        "busy" => (Measure::Onsets, true, "busy"),
        "sparse" => (Measure::Onsets, false, "sparse"),
        _ => return None,
    };
    Some(Descriptor { measure, high, says })
}
pub fn is_descriptor(word: &str) -> bool {
    descriptor(word).is_some()
}
fn measure_of(entry: &SoundEntry, measure: Measure) -> Option<f64> {
    match measure {
        Measure::Brightness => entry.brightness,
        Measure::Flatness => entry.flatness,
        Measure::Attack => entry.attack,
        Measure::Decay => entry.decay,
        Measure::Seconds => entry.seconds,
        Measure::Loudness => entry.loudness,
        Measure::Width => entry.width,
        Measure::Low => entry.low,
        Measure::High => entry.high,
        Measure::Onsets => entry.onsets,
    }
}
fn measure_words(measure: Measure, value: f64) -> String {
    match measure {
        Measure::Brightness => format!(
            "brightness {}",
            if value >= 1000. { format!("{} kHz", to_fixed(value / 1000., 1)) } else { format!("{} Hz", to_string(round(value))) }
        ),
        Measure::Flatness => format!("noise {}%", to_string(round(value * 100.))),
        Measure::Attack => format!("attack {} ms", to_string(round(value))),
        Measure::Decay => format!("decay {} ms", to_string(round(value))),
        Measure::Seconds => format!("{} s", to_fixed(value, if value < 10. { 2 } else { 1 })),
        Measure::Loudness => format!("{} LUFS", to_string(round(value))),
        Measure::Width => format!("width {}%", to_string(round(value * 100.))),
        Measure::Low => format!("low end {}%", to_string(round(value * 100.))),
        Measure::High => format!("highs {}%", to_string(round(value * 100.))),
        Measure::Onsets => format!("{} hits/s", to_fixed(value, 1)),
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LikeSound {
    pub vector: Vec<f64>,
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
    pub brightness: f64,
    pub attack: f64,
    pub seconds: f64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundQuery {
    #[serde(default)]
    pub words: Vec<String>,
    #[serde(default)]
    pub like: Option<LikeSound>,
    #[serde(default)]
    pub kind: Option<SoundKind>,
    #[serde(default)]
    pub classes: Vec<SoundClass>,
    #[serde(default)]
    pub bpm: Option<f64>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub min_seconds: Option<f64>,
    #[serde(default)]
    pub max_seconds: Option<f64>,
    #[serde(default)]
    pub folders: Option<Vec<String>>,
    #[serde(default)]
    pub random: bool,
    pub limit: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundHit {
    pub entry: SoundEntry,
    pub name: String,
    #[serde(rename = "where")]
    pub r#where: String,
    pub score: f64,
    pub why: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closeness: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult<T> {
    pub hits: Vec<T>,
    pub matched: usize,
}
fn relative_key(key: &str) -> Option<String> {
    let mut parts = key.split(' ');
    let root = parts.next()?;
    let quality = parts.next()?;
    let notes = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let index = notes.iter().position(|n| *n == root)?;
    match quality {
        "minor" => Some(format!("{} major", notes[(index + 3) % 12])),
        "major" => Some(format!("{} minor", notes[(index + 9) % 12])),
        _ => None,
    }
}
struct Prepared {
    name: String,
    lower: String,
    where_: String,
    hay: String,
    folders: Vec<String>,
    vector: Vec<f32>,
}
fn prepared(entry: &SoundEntry, source: &Source) -> Prepared {
    let relative = &entry.path[source.path.len() + 1..];
    let mut folders: Vec<_> = relative.split(['\\', '/']).map(str::to_owned).collect();
    folders.pop();
    let name = basename(&entry.path);
    let ext = extension(&entry.path);
    let name = name.strip_suffix(&ext).unwrap_or(&name).to_owned();
    let all = std::iter::once(source.label.clone()).chain(folders).collect::<Vec<_>>();
    Prepared {
        lower: name.to_lowercase(),
        name,
        where_: all.join(" / "),
        hay: format!("{}/{relative}", source.label).to_lowercase(),
        folders: all.iter().map(|s| s.to_lowercase()).collect(),
        vector: entry.vector.as_deref().map(unpack_vector).unwrap_or_default(),
    }
}
pub struct SoundIndex {
    pub entries: Vec<SoundEntry>,
    prepared: Vec<Prepared>,
    vectors: Vec<f32>,
    has_vector: Vec<bool>,
    weights: [f32; VECTOR_LENGTH],
    mean: [f32; VECTOR_LENGTH],
    scale: [f32; VECTOR_LENGTH],
}
impl SoundIndex {
    pub fn new(entries: impl IntoIterator<Item = SoundEntry>, sources: &[Source]) -> Self {
        let mut roots: Vec<_> = sources.iter().filter(|s| Path::new(&s.path).exists()).collect();
        roots.sort_by_key(|s| std::cmp::Reverse(s.path.encode_utf16().count()));
        let rows: Vec<_> = entries
            .into_iter()
            .filter_map(|entry| {
                roots.iter().find(|root| entry.path.starts_with(&format!("{}{SEP}", root.path))).map(|source| {
                    let prepared = prepared(&entry, source);
                    (entry, prepared)
                })
            })
            .collect();
        Self::from_rows(rows)
    }
    fn from_rows(rows: Vec<(SoundEntry, Prepared)>) -> Self {
        let count = rows.len();
        let (entries, prepared): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
        let mut index = Self {
            entries,
            prepared,
            vectors: vec![0.; count * VECTOR_LENGTH],
            has_vector: vec![false; count],
            weights: [0.; VECTOR_LENGTH],
            mean: [0.; VECTOR_LENGTH],
            scale: [1.; VECTOR_LENGTH],
        };
        let mut at = 0;
        for part in VECTOR_GROUPS {
            for _ in 0..part.count {
                index.weights[at] = (part.weight * if part.count > 3 { 3. / part.count as f64 } else { 1. }).sqrt() as f32;
                at += 1;
            }
        }
        let (mut squares, mut sums) = ([0_f64; VECTOR_LENGTH], [0_f64; VECTOR_LENGTH]);
        let mut measured = 0;
        for (row, prepared) in index.prepared.iter().enumerate() {
            let vector = &prepared.vector;
            if vector.len() != VECTOR_LENGTH {
                continue;
            }
            index.has_vector[row] = true;
            measured += 1;
            for dimension in 0..VECTOR_LENGTH {
                let value = vector[dimension] as f64;
                sums[dimension] += value;
                squares[dimension] += value * value;
            }
        }
        for dimension in 0..VECTOR_LENGTH {
            let average = sums[dimension] / measured.max(1) as f64;
            index.mean[dimension] = average as f32;
            index.scale[dimension] = (index.weights[dimension] as f64
                / 1e-3_f64.max((squares[dimension] / measured.max(1) as f64 - average * average).max(0.).sqrt()))
                as f32;
        }
        for (row, prepared) in index.prepared.iter().enumerate() {
            if !index.has_vector[row] {
                continue;
            }
            for dimension in 0..VECTOR_LENGTH {
                index.vectors[row * VECTOR_LENGTH + dimension] =
                    ((prepared.vector[dimension] as f64 - index.mean[dimension] as f64) * index.scale[dimension] as f64) as f32;
            }
        }
        index
    }
    pub async fn build(entries: impl IntoIterator<Item = SoundEntry>, sources: &[Source]) -> Self {
        let mut roots: Vec<_> = sources.iter().collect();
        roots.sort_by_key(|s| std::cmp::Reverse(s.path.encode_utf16().count()));
        let mut rows = vec![];
        let mut count = 0;
        for entry in entries {
            if let Some(root) = roots.iter().find(|s| entry.path.starts_with(&format!("{}{SEP}", s.path))) {
                rows.push((prepared(&entry, root), entry, root.path.clone()));
            }
            count += 1;
            if count % 2000 == 0 {
                tokio::task::yield_now().await;
            }
        }
        if count % 2000 != 0 {
            tokio::task::yield_now().await;
        }
        let present: HashSet<_> = roots.iter().filter(|s| Path::new(&s.path).exists()).map(|s| s.path.as_str()).collect();
        // If a longer source vanished while yielding, the constructor uses the next containing source.
        Self::from_rows(
            rows.into_iter()
                .filter_map(|(made, entry, root)| {
                    if present.contains(root.as_str()) {
                        Some((entry, made))
                    } else {
                        roots.iter().find(|s| present.contains(s.path.as_str()) && entry.path.starts_with(&format!("{}{SEP}", s.path))).map(
                            |s| {
                                let made = prepared(&entry, s);
                                (entry, made)
                            },
                        )
                    }
                })
                .collect(),
        )
    }
    pub fn size(&self) -> usize {
        self.entries.len()
    }
    pub fn measured(&self) -> usize {
        self.has_vector.iter().filter(|flag| **flag).count()
    }
    pub fn holds(&self, folder: &str) -> bool {
        self.entries.iter().any(|e| e.path.starts_with(&format!("{folder}{SEP}")))
    }
    pub fn vector_of(&self, path: &str, size: Option<u64>) -> Option<&SoundEntry> {
        self.entries.iter().find(|e| e.path == path && size.is_none_or(|s| e.size == s))
    }
    fn normalized(&self, vector: &[f64]) -> [f32; VECTOR_LENGTH] {
        std::array::from_fn(|d| ((vector.get(d).copied().unwrap_or(0.) - self.mean[d] as f64) * self.scale[d] as f64) as f32)
    }
    pub fn search(&self, query: &SoundQuery) -> SearchResult<SoundHit> {
        let words = words(&query.words);
        let describing: Vec<_> = words.iter().filter_map(|word| descriptor(word)).collect();
        let naming: Vec<_> = words.into_iter().filter(|word| !is_descriptor(word)).collect();
        let classes: Vec<_> = naming.iter().map(|word| class_for_word(word)).collect();
        let key = query.key.as_ref().filter(|s| !s.is_empty()).map(|s| parse_key(s).unwrap_or_else(|| s.clone()));
        let relative = key.as_deref().and_then(relative_key);
        let folders = query.folders.as_ref().map(|folders| folders.iter().map(|f| format!("{f}{SEP}")).collect::<Vec<_>>());
        let like = query.like.as_ref().map(|like| self.normalized(&like.vector));
        let spread = (2. * self.weights.iter().map(|w| *w as f64 * *w as f64).sum::<f64>()).sqrt();
        let (mut rows, mut scores, mut closeness) = (vec![], vec![], HashMap::new());
        for (row, entry) in self.entries.iter().enumerate() {
            if query.kind.is_some_and(|k| entry.kind != Some(k))
                || (!query.classes.is_empty() && entry.r#class.is_none_or(|c| !query.classes.contains(&c)))
                || query.min_seconds.is_some_and(|min| entry.seconds.is_none_or(|s| s < min))
                || query.max_seconds.is_some_and(|max| entry.seconds.is_none_or(|s| s > max))
                || folders.as_ref().is_some_and(|f| !f.iter().any(|f| entry.path.starts_with(f)))
            {
                continue;
            }
            let mut score = 0.;
            if let Some(bpm) = query.bpm {
                let Some(actual) = entry.bpm else { continue };
                let off = (actual / bpm).log2().abs();
                if off < 0.03 {
                    score += 6.;
                } else if (off - 1.).abs() < 0.03 {
                    score += 2.;
                } else {
                    continue;
                }
            }
            if let Some(key) = &key {
                if entry.key.as_ref() == Some(key) {
                    score += 6.;
                } else if entry.key.is_some() && entry.key == relative {
                    score += 3.;
                } else if entry.key.is_none() && entry.note.as_ref().is_some_and(|note| key.starts_with(&format!("{} ", note_root(note)))) {
                    score += 4.;
                } else {
                    continue;
                }
            }
            let made = &self.prepared[row];
            let mut missing = false;
            for (word, class) in naming.iter().zip(&classes) {
                let in_name = if made.lower.starts_with(word) {
                    10.
                } else if made.lower.contains(word) {
                    5.
                } else {
                    0.
                };
                let as_class = class.is_some() && entry.r#class == *class;
                if in_name == 0. && !as_class && !made.hay.contains(word) {
                    missing = true;
                    break;
                }
                score += in_name
                    + if made.folders.iter().any(|folder| folder == word || folder == &format!("{word}s")) { 3. } else { 0. }
                    + if as_class { 8. } else { 0. };
            }
            if missing {
                continue;
            }
            if let Some(like) = like {
                if !self.has_vector[row] || query.like.as_ref().and_then(|l| l.path.as_ref()) == Some(&entry.path) {
                    continue;
                }
                let distance = (0..VECTOR_LENGTH)
                    .map(|d| {
                        let delta = self.vectors[row * VECTOR_LENGTH + d] as f64 - like[d] as f64;
                        delta * delta
                    })
                    .sum::<f64>();
                let close = 100. * (-2. * (distance.sqrt() / spread).powi(2)).exp();
                score += close;
                closeness.insert(row, close);
            }
            rows.push(row);
            scores.push(score);
        }
        let ranks: Vec<HashMap<usize, f64>> = describing
            .iter()
            .map(|describe| {
                let mut values: Vec<_> = rows.iter().filter_map(|row| measure_of(&self.entries[*row], describe.measure)).collect();
                values.sort_by(|a, b| compare(*a, *b));
                let mut rank = HashMap::new();
                if values.is_empty() {
                    return rank;
                }
                for (index, row) in rows.iter().enumerate() {
                    let Some(value) = measure_of(&self.entries[*row], describe.measure) else { continue };
                    let low = values.partition_point(|v| *v < value);
                    let place = low as f64 / values.len().saturating_sub(1).max(1) as f64;
                    let place = if describe.high { place } else { 1. - place };
                    scores[index] += place * 12.;
                    rank.insert(*row, place);
                }
                rank
            })
            .collect();
        let mut order: Vec<_> = (0..rows.len()).collect();
        let chosen = if query.random {
            order.shuffle(&mut rand::rng());
            order.truncate(query.limit);
            order
        } else {
            let mut chosen = top_by(order, query.limit, |a, b| compare(scores[*b], scores[*a]).then(a.cmp(b)));
            chosen.sort_by(|a, b| {
                compare(scores[*b], scores[*a])
                    .then_with(|| locale_compare_numeric_base(&self.prepared[rows[*a]].name, &self.prepared[rows[*b]].name))
            });
            chosen
        };
        SearchResult {
            matched: rows.len(),
            hits: chosen
                .into_iter()
                .map(|at| {
                    let row = rows[at];
                    let entry = &self.entries[row];
                    let made = &self.prepared[row];
                    let mut why = vec![];
                    if let (Some(bpm), Some(actual)) = (query.bpm, entry.bpm) {
                        why.push(format!(
                            "{} BPM{}",
                            to_string(actual),
                            if (actual / bpm).log2().abs() < 0.03 { "" } else { " (half or double)" }
                        ));
                    }
                    if let Some(key) = &key {
                        why.push(if entry.key.as_ref() == Some(key) {
                            key.clone()
                        } else if entry.key.is_some() && entry.key == relative {
                            format!("{} (relative key)", entry.key.as_ref().unwrap())
                        } else {
                            format!("note {}", entry.note.as_deref().unwrap_or("undefined"))
                        });
                    }
                    for (word, class) in naming.iter().zip(&classes) {
                        if made.lower.contains(word) {
                            why.push(format!("“{word}” in its name"));
                        } else if made.hay.contains(word) {
                            why.push(format!("in a “{word}” folder"));
                        } else if class.is_some() && entry.r#class == *class {
                            why.push(format!(
                                "a {} by its {}",
                                entry.r#class.unwrap(),
                                if entry.class_from == Some(ClassFrom::Sound) { "sound" } else { "folder" }
                            ));
                        }
                    }
                    for (describe, rank) in describing.iter().zip(&ranks) {
                        if let Some(value) = measure_of(entry, describe.measure) {
                            if rank.get(&row).copied().unwrap_or(0.) >= 0.6 {
                                why.push(format!("{}: {}", describe.says, measure_words(describe.measure, value)));
                            }
                        }
                    }
                    let close = closeness.get(&row).copied();
                    if let (Some(close), Some(like)) = (close, &query.like) {
                        why.insert(0, format!("{}% like {}{}", to_string(round(close)), like.name, differences(entry, like)));
                    }
                    SoundHit {
                        entry: entry.clone(),
                        name: made.name.clone(),
                        r#where: made.where_.clone(),
                        score: round(scores[at] * 10.) / 10.,
                        why,
                        closeness: close.map(round),
                    }
                })
                .collect(),
        }
    }
}
fn words(words: &[String]) -> Vec<String> {
    words.iter().map(|w| trim(w).to_lowercase()).filter(|s| !s.is_empty()).collect()
}
fn compare(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}
fn note_root(note: &str) -> &str {
    let without = note.trim_end_matches(|c: char| c.is_ascii_digit());
    if without.len() != note.len() {
        without.strip_suffix('-').unwrap_or(without)
    } else {
        note
    }
}
fn differences(entry: &SoundEntry, like: &LikeSound) -> String {
    let mut notes = vec![];
    if let Some(brightness) = entry.brightness.filter(|_| like.brightness > 0.) {
        let ratio = brightness / like.brightness;
        if ratio > 1.3 {
            notes.push("brighter");
        } else if ratio < 0.77 {
            notes.push("darker");
        }
    }
    if let Some(attack) = entry.attack.filter(|a| (*a - like.attack).abs() > 10_f64.max(like.attack)) {
        notes.push(if attack > like.attack { "a slower attack" } else { "a faster attack" });
    }
    if let Some(seconds) = entry.seconds.filter(|_| like.seconds > 0.) {
        let ratio = seconds / like.seconds;
        if ratio > 1.6 {
            notes.push("longer");
        } else if ratio < 0.6 {
            notes.push("shorter");
        }
    }
    if notes.is_empty() {
        String::new()
    } else {
        format!(" ({})", notes.join(", "))
    }
}
fn top_by<T>(mut items: Vec<T>, limit: usize, compare: impl Fn(&T, &T) -> Ordering) -> Vec<T> {
    if limit == 0 {
        return vec![];
    }
    if items.len() <= limit.saturating_mul(4) {
        items.sort_by(compare);
        items.truncate(limit);
        return items;
    }
    let mut kept = vec![];
    for item in items {
        if kept.len() < limit {
            kept.push(item);
            if kept.len() == limit {
                kept.sort_by(&compare);
            }
            continue;
        }
        if compare(&item, &kept[limit - 1]) != Ordering::Less {
            continue;
        }
        let mut low = 0;
        let mut high = limit - 1;
        while low < high {
            let middle = (low + high) >> 1;
            if compare(&item, &kept[middle]) == Ordering::Less {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        kept.insert(low, item);
        kept.pop();
    }
    kept
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PresetQuery {
    #[serde(default)]
    pub words: Vec<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    pub limit: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetHit {
    pub entry: PresetEntry,
    pub score: f64,
    pub why: Vec<String>,
}
pub fn search_presets<'a>(entries: impl IntoIterator<Item = &'a PresetEntry>, query: &PresetQuery) -> SearchResult<PresetHit> {
    let words = words(&query.words);
    let device = query.device.as_ref().map(|d| trim(d).to_lowercase()).filter(|d| !d.is_empty());
    let mut found = vec![];
    for entry in entries {
        if query.category.as_ref().is_some_and(|category| {
            !category.is_empty() && entry.category.map(|c| serde_json::to_value(c).unwrap()) != Some(json!(category))
        }) {
            continue;
        }
        let device_name = entry.device.as_deref().unwrap_or("").to_lowercase();
        if device.as_ref().is_some_and(|d| {
            !device_name.contains(d) && !entry.inside.as_ref().is_some_and(|inside| inside.iter().any(|i| i.to_lowercase().contains(d)))
        }) {
            continue;
        }
        let name = entry.name.to_lowercase();
        let hay = format!(
            "{}/{}/{} {} {}",
            entry.source,
            entry.folder,
            entry.name,
            entry.device.as_deref().unwrap_or(""),
            entry.about.as_deref().unwrap_or("")
        )
        .to_lowercase();
        let mut score = if entry.source == "User Library" { 2. } else { 0. };
        let mut why = vec![];
        let mut missing = false;
        for word in &words {
            if !hay.contains(word) {
                missing = true;
                break;
            }
            if name.starts_with(word) {
                score += 10.;
                why.push(format!("“{word}” in its name"));
            } else if name.contains(word) {
                score += 5.;
                why.push(format!("“{word}” in its name"));
            } else if device_name.contains(word) {
                score += 4.;
                why.push(format!("a {} preset", entry.device.as_deref().unwrap_or("undefined")));
            } else {
                why.push(format!("“{word}” in its folder or notes"));
            }
        }
        if missing {
            continue;
        }
        if let Some(device) = &device {
            score += if device_name == *device { 6. } else { 3. };
            why.push(if device_name.contains(device) {
                format!("a {} preset", entry.device.as_deref().unwrap_or("undefined"))
            } else {
                format!("a rack with {}", query.device.as_deref().unwrap())
            });
        }
        found.push(PresetHit { entry: entry.clone(), score, why });
    }
    SearchResult {
        matched: found.len(),
        hits: top_by(found, query.limit, |a, b| {
            compare(b.score, a.score).then_with(|| locale_compare_numeric_base(&a.entry.name, &b.entry.name))
        }),
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetQuery {
    #[serde(default)]
    pub words: Vec<String>,
    #[serde(default)]
    pub min_tempo: Option<f64>,
    #[serde(default)]
    pub max_tempo: Option<f64>,
    #[serde(default)]
    pub key: Option<String>,
    pub limit: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetHit {
    pub entry: SetEntry,
    pub score: f64,
    pub why: Vec<String>,
}
fn set_words(set: &SetSummary) -> Vec<(String, String)> {
    let all: Vec<_> = set.tracks.iter().chain(&set.returns).chain(set.main.iter()).collect();
    let mut words = vec![("its name".into(), set.name.clone())];
    words.extend(all.iter().map(|t| (format!("track “{}”", t.name), t.name.clone())));
    for track in &all {
        for device in &track.devices {
            let where_ = format!(
                "“{}” has {}",
                track.name,
                device
                    .preset
                    .as_ref()
                    .filter(|p| !p.is_empty())
                    .map(|p| format!("{p} ({})", device.name))
                    .unwrap_or_else(|| device.name.clone())
            );
            for text in
                std::iter::once(&device.name).chain(device.preset.iter()).chain(device.inside.iter().flatten()).filter(|s| !s.is_empty())
            {
                words.push((where_.clone(), text.clone()));
            }
        }
    }
    for track in &all {
        for plugin in track.plugins.iter().flatten() {
            words.push((format!("“{}” has {plugin}", track.name), plugin.clone()));
        }
    }
    for track in &all {
        for file in &track.samples {
            let name = basename(file);
            words.push((format!("“{}” plays {name}", track.name), name));
        }
    }
    words
}
pub fn search_sets<'a>(entries: impl IntoIterator<Item = &'a SetEntry>, query: &SetQuery) -> SearchResult<SetHit> {
    let words = words(&query.words);
    let key = query.key.as_ref().filter(|k| !k.is_empty()).map(|k| parse_key(k).unwrap_or_else(|| k.clone()));
    let mut found = vec![];
    for entry in entries {
        let Some(set) = &entry.set else { continue };
        if query.min_tempo.is_some_and(|min| set.tempo.is_none_or(|t| t < min))
            || query.max_tempo.is_some_and(|max| set.tempo.is_none_or(|t| t > max))
            || key.as_ref().is_some_and(|k| set.key.as_ref() != Some(k))
        {
            continue;
        }
        let texts = if words.is_empty() { vec![] } else { set_words(set) };
        let (mut score, mut why, mut missing) = (0., vec![], false);
        for word in &words {
            let Some((where_, _)) = texts.iter().find(|(_, text)| text.to_lowercase().contains(word)) else {
                missing = true;
                break;
            };
            score += if where_ == "its name" {
                10.
            } else if where_.starts_with("track") {
                6.
            } else {
                3.
            };
            if !why.contains(where_) {
                why.push(if where_ == "its name" { format!("“{word}” in its name") } else { where_.clone() });
            }
        }
        if !missing {
            found.push(SetHit { entry: entry.clone(), score, why });
        }
    }
    SearchResult {
        matched: found.len(),
        hits: top_by(found, query.limit, |a, b| compare(b.score, a.score).then(b.entry.mtime.cmp(&a.entry.mtime))),
    }
}
pub fn describe_track(track: &SetTrack) -> Value {
    let mut row = json!({"name":track.name,"kind":track.kind,"devices":track.devices.iter().map(|device|{
        let mut text=device.preset.as_ref().filter(|p|!p.is_empty()).map(|p|format!("{p} ({})",device.name)).unwrap_or_else(||device.name.clone());
        if let Some(plugin)=device.plugin.as_ref().filter(|p|!p.is_empty()&&p.as_str()!="Max for Live"){text.push_str(&format!(" [{plugin}]"));}
        if let Some(inside)=device.inside.as_ref().filter(|i|!i.is_empty()){text.push_str(&format!(": {}",inside.join(", ")));}text
    }).collect::<Vec<_>>(),"clips":track.clips});
    if let Some(role) = track_role(track) {
        row["role"] = json!(role);
    }
    if let Some(group) = track.group.as_ref().filter(|g| !g.is_empty()) {
        row["group"] = json!(group);
    }
    if let Some(color) = track.color {
        row["colour"] = json!(format!("{} (colour {})", colour_name(color).unwrap_or_else(|| "colour".into()), to_string(color)));
    }
    if !track.samples.is_empty() {
        row["samples"] = json!(&track.samples[..track.samples.len().min(12)]);
    }
    if track.frozen == Some(true) {
        row["frozen"] = json!(true);
    }
    row
}
