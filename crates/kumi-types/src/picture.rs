//! What a change looks like, before and after, for the producer to check by eye: the values that moved, an
//! equaliser's curve, a compressor's curve, a clip's notes, an automation lane, a channel's levels, a tracker's lines,
//! routes between tracks, an instrument's keyzones or items on the timeline. Plain,
//! DAW-neutral data: an integration fills it from its own records, and the terminal draws it knowing nothing of
//! the DAW. Kept small (at most [`MAX_BYTES`] as JSON), since saved conversations keep thousands of changes.

use serde::{Deserialize, Serialize};

use super::contracts::ClipNote;
use kumi_common::js::{
    number,
    string::{head, trim},
};
use serde_json::Value;

/// The most one change's picture takes as JSON: past it, lists are thinned until it fits.
pub const MAX_BYTES: usize = 4096;
const MAX_VALUES: usize = 16;
const MAX_BANDS: usize = 16;
const MAX_NOTES: usize = 200;
const MAX_POINTS: usize = 128;
const MAX_SENDS: usize = 12;
const MAX_NAME: usize = 64;
const MAX_TEXT: usize = 32;
const MAX_LINES: usize = 16;
const MAX_LINE: usize = 120;
const MAX_ROUTES: usize = 8;
const MAX_ZONES: usize = 32;
const MAX_ITEMS: usize = 32;

/// A change's picture, by kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChangePicture {
    /// Each value that moved: a device's parameters, the tempo.
    Values { values: Vec<PictureValue> },
    /// An equaliser's bands, before and after, with the values that moved.
    Eq {
        before: Vec<EqBand>,
        after: Vec<EqBand>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<PictureValue>,
    },
    /// A compressor's curve (level in against level out), before and after, with the values that moved.
    Dynamics {
        before: Dynamics,
        after: Dynamics,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<PictureValue>,
    },
    /// A clip's notes before and after, in beats from its start; `length` is the clip's. Each note is kept as
    /// `[start, pitch, duration, velocity]`. `partial`: the latest bars were left out to keep it small.
    Notes {
        length: f64,
        #[serde(with = "compact_notes")]
        before: Vec<ClipNote>,
        #[serde(with = "compact_notes")]
        after: Vec<ClipNote>,
        #[serde(default, skip_serializing_if = "is_false")]
        partial: bool,
    },
    /// An automation lane's points ([beat, value]) before and after, in the parameter's range.
    Automation {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        parameter: String,
        min: f64,
        max: f64,
        before: Vec<[f64; 2]>,
        after: Vec<[f64; 2]>,
        /// What it moves between and where, in the DAW's words ("−18 to 0 dB over bars 5–9"), for words in its place.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        words: Option<String>,
    },
    /// A channel's fader, pan and sends, before and after.
    Mix { before: MixLevels, after: MixLevels },
    /// A tracker's lines (a pattern's or a phrase's) before and after, each in the DAW's own notation; `more` lines
    /// changed past those kept.
    Lines {
        lines: Vec<PictureLine>,
        #[serde(default, skip_serializing_if = "is_zero")]
        more: usize,
    },
    /// Routes made, taken away or changed (sends, receives, outputs), each from one end to the other with the values
    /// that moved.
    Routes { routes: Vec<PictureRoute> },
    /// An instrument's keyzones before and after: each zone's notes (across) and velocities (up), with the values that
    /// moved.
    Zones {
        before: Vec<PictureZone>,
        after: Vec<PictureZone>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<PictureValue>,
    },
    /// Items on the timeline before and after (in seconds from the project's start), with the values that moved.
    Items {
        before: Vec<PictureItem>,
        after: Vec<PictureItem>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<PictureValue>,
    },
}

/// One line of a tracker before and after (its number as the DAW shows it: Renoise's from 0); an empty side is a line
/// that had nothing (or has nothing now).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PictureLine {
    pub line: u32,
    #[serde(default)]
    pub before: String,
    #[serde(default)]
    pub after: String,
}

/// What became of a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteChange {
    Made,
    Removed,
    Changed,
}

/// A route from one end (a track) to the other (a track, or an output), and what moved on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PictureRoute {
    pub from: String,
    pub to: String,
    pub change: RouteChange,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<PictureValue>,
}

/// One keyzone: the sample it plays (or none), its notes and velocities as `[low, high]`, and its base note. Velocities
/// a change didn't touch may be unknown: the zone is then drawn by its keys alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PictureZone {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub notes: [u8; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocities: Option<[u8; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<u8>,
}

/// One item on the timeline: where it starts and how long it is, its fades, and whether it's muted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PictureItem {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub start: f64,
    pub length: f64,
    #[serde(default, skip_serializing_if = "is_nothing")]
    pub fade_in: f64,
    #[serde(default, skip_serializing_if = "is_nothing")]
    pub fade_out: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
}

/// One value that moved, as numbers in its range and as the DAW shows it, and what was asked when the DAW set
/// something else.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PictureValue {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// "-3.0 dB" → "-1.5 dB".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_text: Option<String>,
    /// What was asked, when the DAW set something else: "2 dB" where it set "1.9 dB".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<String>,
}

/// One equaliser band.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqBand {
    pub shape: EqShape,
    pub freq: f64,
    #[serde(default)]
    pub gain_db: f64,
    #[serde(default = "default_q")]
    pub q: f64,
    #[serde(default = "on")]
    pub on: bool,
    /// A cut's steepness in dB per octave (12, 24, 48); none for the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slope: Option<f64>,
}

fn default_q() -> f64 {
    0.71
}

fn on() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EqShape {
    Bell,
    LowShelf,
    HighShelf,
    LowCut,
    HighCut,
    Notch,
}

/// A compressor's static curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dynamics {
    pub threshold_db: f64,
    /// 4 for 4:1; a limiter's is large.
    pub ratio: f64,
    #[serde(default)]
    pub knee_db: f64,
    #[serde(default)]
    pub makeup_db: f64,
}

/// A channel's levels: the fader (0–1 along its travel) and pan (-1 left to 1 right), each with how the DAW shows
/// it, and its sends.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixLevels {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan_text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<MixSend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solo: Option<bool>,
}

/// One send: its name (the return's) and its level, 0–1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixSend {
    pub name: String,
    pub level: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl ChangePicture {
    /// The picture within its bounds: lists capped, names and texts cut, numbers finite, and thinned until its JSON
    /// is at most [`MAX_BYTES`]. None when nothing drawable is left.
    pub fn bounded(self) -> Option<ChangePicture> {
        let mut picture = self.capped()?;
        for _ in 0..8 {
            if serde_json::to_vec(&picture).map_or(usize::MAX, |json| json.len()) <= MAX_BYTES {
                return Some(picture);
            }
            picture = picture.halved()?;
        }
        None
    }

    fn capped(self) -> Option<ChangePicture> {
        let values = |values: Vec<PictureValue>| values.into_iter().take(MAX_VALUES).filter_map(PictureValue::bounded).collect::<Vec<_>>();
        let bands = |bands: Vec<EqBand>| bands.into_iter().filter(EqBand::finite).take(MAX_BANDS).collect::<Vec<_>>();
        let picture = match self {
            ChangePicture::Values { values: list } => ChangePicture::Values { values: values(list) },
            ChangePicture::Eq { before, after, values: list } => {
                ChangePicture::Eq { before: bands(before), after: bands(after), values: values(list) }
            }
            ChangePicture::Dynamics { before, after, values: list } => {
                if !before.finite() || !after.finite() {
                    return None;
                }
                ChangePicture::Dynamics { before, after, values: values(list) }
            }
            ChangePicture::Notes { length, before, after, partial } => {
                let notes = |notes: Vec<ClipNote>| {
                    let mut notes: Vec<ClipNote> = notes.into_iter().filter(finite_note).collect();
                    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.total_cmp(&b.pitch)));
                    notes
                };
                let (before, after) = (notes(before), notes(after));
                let (before, after, cut) = whole_bars(before, after, MAX_NOTES);
                ChangePicture::Notes {
                    length: if length.is_finite() && length > 0. { length } else { 0. },
                    before,
                    after,
                    partial: partial || cut,
                }
            }
            ChangePicture::Automation { parameter, min, max, before, after, words } => {
                let points = |points: Vec<[f64; 2]>| {
                    let mut points: Vec<[f64; 2]> = points.into_iter().filter(|[at, value]| at.is_finite() && value.is_finite()).collect();
                    points.sort_by(|a, b| a[0].total_cmp(&b[0]));
                    thin(points, MAX_POINTS)
                };
                if !min.is_finite() || !max.is_finite() || max <= min {
                    return None;
                }
                ChangePicture::Automation {
                    parameter: cut(&parameter, MAX_NAME),
                    min,
                    max,
                    before: points(before),
                    after: points(after),
                    words: words.map(|words| cut(&words, MAX_LINE)),
                }
            }
            ChangePicture::Mix { before, after } => ChangePicture::Mix { before: before.bounded(), after: after.bounded() },
            ChangePicture::Lines { lines, more } => {
                let mut lines: Vec<PictureLine> = lines
                    .into_iter()
                    .map(|line| PictureLine { line: line.line, before: cut(&line.before, MAX_LINE), after: cut(&line.after, MAX_LINE) })
                    .filter(|line| line.before != line.after)
                    .collect();
                lines.sort_by_key(|line| line.line);
                let past = lines.len().saturating_sub(MAX_LINES);
                lines.truncate(MAX_LINES);
                ChangePicture::Lines { lines, more: more + past }
            }
            ChangePicture::Routes { routes } => ChangePicture::Routes {
                routes: routes
                    .into_iter()
                    .take(MAX_ROUTES)
                    .map(|route| PictureRoute {
                        from: cut(&route.from, MAX_NAME),
                        to: cut(&route.to, MAX_NAME),
                        change: route.change,
                        values: values(route.values),
                    })
                    .filter(|route| !route.from.is_empty() || !route.to.is_empty())
                    .collect(),
            },
            ChangePicture::Zones { before, after, values: list } => {
                let zones = |zones: Vec<PictureZone>| zones.into_iter().take(MAX_ZONES).map(PictureZone::bounded).collect::<Vec<_>>();
                ChangePicture::Zones { before: zones(before), after: zones(after), values: values(list) }
            }
            ChangePicture::Items { before, after, values: list } => {
                let items =
                    |items: Vec<PictureItem>| items.into_iter().filter_map(PictureItem::bounded).take(MAX_ITEMS).collect::<Vec<_>>();
                ChangePicture::Items { before: items(before), after: items(after), values: values(list) }
            }
        };
        (!picture.is_empty()).then_some(picture)
    }

    /// Half of each list (the endpoints kept), for a picture still too large.
    fn halved(self) -> Option<ChangePicture> {
        let half = |n: usize| (n / 2).max(1);
        let picture = match self {
            ChangePicture::Values { values } => ChangePicture::Values { values: values.iter().take(half(values.len())).cloned().collect() },
            ChangePicture::Eq { before, after, values } => {
                ChangePicture::Eq { before, after, values: values.iter().take(values.len() / 2).cloned().collect() }
            }
            ChangePicture::Dynamics { before, after, values } => {
                ChangePicture::Dynamics { before, after, values: values.iter().take(values.len() / 2).cloned().collect() }
            }
            ChangePicture::Notes { length, before, after, .. } => {
                let most = half(before.len().max(after.len()));
                let (before, after, _) = whole_bars(before, after, most);
                ChangePicture::Notes { length, before, after, partial: true }
            }
            ChangePicture::Automation { parameter, min, max, before, after, words } => ChangePicture::Automation {
                parameter,
                min,
                max,
                before: thin(before.clone(), half(before.len())),
                after: thin(after.clone(), half(after.len())),
                words,
            },
            ChangePicture::Mix { .. } => return None,
            ChangePicture::Lines { lines, more } => {
                let kept = half(lines.len());
                ChangePicture::Lines { more: more + lines.len() - kept, lines: lines.into_iter().take(kept).collect() }
            }
            ChangePicture::Routes { routes } => ChangePicture::Routes { routes: routes.iter().take(half(routes.len())).cloned().collect() },
            ChangePicture::Zones { before, after, values } => ChangePicture::Zones {
                before: before.iter().take(half(before.len())).cloned().collect(),
                after: after.iter().take(half(after.len())).cloned().collect(),
                values: values.iter().take(values.len() / 2).cloned().collect(),
            },
            ChangePicture::Items { before, after, values } => ChangePicture::Items {
                before: before.iter().take(half(before.len())).cloned().collect(),
                after: after.iter().take(half(after.len())).cloned().collect(),
                values: values.iter().take(values.len() / 2).cloned().collect(),
            },
        };
        (!picture.is_empty()).then_some(picture)
    }

    fn is_empty(&self) -> bool {
        match self {
            ChangePicture::Values { values } => values.is_empty(),
            ChangePicture::Eq { before, after, .. } => before.is_empty() && after.is_empty(),
            ChangePicture::Dynamics { .. } => false,
            ChangePicture::Notes { before, after, .. } => before.is_empty() && after.is_empty(),
            ChangePicture::Automation { before, after, .. } => before.is_empty() && after.is_empty(),
            ChangePicture::Mix { before, after } => before == &MixLevels::default() && after == &MixLevels::default(),
            ChangePicture::Lines { lines, .. } => lines.is_empty(),
            ChangePicture::Routes { routes } => routes.is_empty(),
            ChangePicture::Zones { before, after, .. } => before.is_empty() && after.is_empty(),
            ChangePicture::Items { before, after, .. } => before.is_empty() && after.is_empty(),
        }
    }
}

impl PictureValue {
    fn bounded(self) -> Option<PictureValue> {
        let finite = |n: Option<f64>| n.filter(|n| n.is_finite());
        let text = |t: Option<String>| t.map(|t| cut(&t, MAX_TEXT)).filter(|t| !t.is_empty());
        let value = PictureValue {
            name: cut(&self.name, MAX_NAME),
            from: finite(self.from),
            to: finite(self.to),
            min: finite(self.min),
            max: finite(self.max),
            from_text: text(self.from_text),
            to_text: text(self.to_text),
            asked: text(self.asked),
        };
        (!value.name.is_empty() && (value.to.is_some() || value.to_text.is_some())).then_some(value)
    }
}

impl PictureZone {
    fn bounded(self) -> PictureZone {
        let range = |[a, b]: [u8; 2]| [a.min(b).min(127), a.max(b).min(127)];
        PictureZone {
            name: cut(&self.name, MAX_NAME),
            notes: range(self.notes),
            velocities: self.velocities.map(range),
            base: self.base.map(|n| n.min(127)),
        }
    }
}

impl PictureItem {
    fn bounded(self) -> Option<PictureItem> {
        let fade = |n: f64| if n.is_finite() { n.clamp(0., self.length) } else { 0. };
        (self.start.is_finite() && self.length.is_finite() && self.length > 0.).then(|| PictureItem {
            name: cut(&self.name, MAX_NAME),
            start: self.start,
            length: self.length,
            fade_in: fade(self.fade_in),
            fade_out: fade(self.fade_out),
            muted: self.muted,
        })
    }
}

impl EqBand {
    fn finite(&self) -> bool {
        self.freq.is_finite() && self.freq > 0. && self.gain_db.is_finite() && self.q.is_finite() && self.q > 0.
    }
}

impl Dynamics {
    fn finite(&self) -> bool {
        self.threshold_db.is_finite()
            && self.ratio.is_finite()
            && self.ratio >= 1.
            && self.knee_db.is_finite()
            && self.makeup_db.is_finite()
    }
}

impl MixLevels {
    fn bounded(self) -> MixLevels {
        let finite = |n: Option<f64>| n.filter(|n| n.is_finite());
        let text = |t: Option<String>| t.map(|t| cut(&t, MAX_TEXT)).filter(|t| !t.is_empty());
        MixLevels {
            volume: finite(self.volume),
            volume_text: text(self.volume_text),
            pan: finite(self.pan),
            pan_text: text(self.pan_text),
            sends: self
                .sends
                .into_iter()
                .filter(|send| send.level.is_finite())
                .take(MAX_SENDS)
                .map(|send| MixSend { name: cut(&send.name, MAX_NAME), level: send.level, text: text(send.text) })
                .collect(),
            mute: self.mute,
            solo: self.solo,
        }
    }
}

/// At most `most` notes on each side, the latest bars left out (whole bars, the same ones on both sides) so what's
/// drawn is what was written up to there; true when some were. Notes come sorted by start.
fn whole_bars(before: Vec<ClipNote>, after: Vec<ClipNote>, most: usize) -> (Vec<ClipNote>, Vec<ClipNote>, bool) {
    let cutoff = |notes: &[ClipNote]| notes.get(most).map(|note| (note.start / 4.).floor() * 4.);
    let Some(end) = [cutoff(&before), cutoff(&after)].into_iter().flatten().reduce(f64::min) else { return (before, after, false) };
    let keep = |notes: Vec<ClipNote>| {
        let kept: Vec<ClipNote> = notes.iter().filter(|note| note.start < end).cloned().collect();
        // A bar alone holds more than fits: its first notes, then.
        if kept.is_empty() {
            notes.into_iter().take(most).collect()
        } else {
            kept
        }
    };
    (keep(before), keep(after), true)
}

fn is_false(flag: &bool) -> bool {
    !*flag
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_nothing(n: &f64) -> bool {
    *n == 0.
}

/// Notes as `[start, pitch, duration, velocity]`, to 3 decimals (about a third of their size as objects); objects
/// are read too.
mod compact_notes {
    use super::ClipNote;
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_json::Value;

    fn number(n: f64) -> Value {
        let rounded = (n * 1000.).round() / 1000.;
        if rounded.fract() == 0. && rounded.abs() < 1e15 {
            Value::from(rounded as i64)
        } else {
            Value::from(rounded)
        }
    }

    pub fn serialize<S: Serializer>(notes: &[ClipNote], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(notes.iter().map(|n| [number(n.start), number(n.pitch), number(n.duration), number(n.velocity)]))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<ClipNote>, D::Error> {
        let items: Vec<Value> = Vec::deserialize(deserializer)?;
        Ok(items
            .into_iter()
            .filter_map(|item| match item {
                Value::Array(note) => Some(ClipNote {
                    start: note.first()?.as_f64()?,
                    pitch: note.get(1)?.as_f64()?,
                    duration: note.get(2)?.as_f64()?,
                    velocity: note.get(3).and_then(Value::as_f64).unwrap_or(100.),
                }),
                other => serde_json::from_value(other).ok(),
            })
            .collect())
    }
}

/// A saved record's picture: one a later Kumi drew (a kind or a shape this one doesn't know) reads as none, so the
/// conversation it's in still loads.
pub fn lenient<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<ChangePicture>, D::Error> {
    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

fn finite_note(note: &ClipNote) -> bool {
    note.pitch.is_finite() && note.start.is_finite() && note.duration.is_finite() && note.duration > 0. && note.velocity.is_finite()
}

/// At most `most` of the items, spread evenly over them, the first and last kept.
fn thin<T: Clone>(items: Vec<T>, most: usize) -> Vec<T> {
    if items.len() <= most || most == 0 {
        return if most == 0 { Vec::new() } else { items };
    }
    if most == 1 {
        return vec![items[0].clone()];
    }
    let last = items.len() - 1;
    (0..most).map(|i| items[i * last / (most - 1)].clone()).collect()
}

/// At most `max` characters.
fn cut(text: &str, max: usize) -> String {
    text.trim().chars().take(max).collect()
}

// The framework a DAW's integration draws pictures with: a device's curves from its parameters, walked back over a run
// of changes, and what was asked beside what was set. The DAW reads its own devices' parameters (`CurveReader`).

/// A number as a change says it: rounded to `digits` places (2 by default), without trailing zeros.
pub fn format_number(value: f64, digits: Option<usize>) -> String {
    number::to_string(number::parse(&number::to_fixed(value, digits.unwrap_or(2))).unwrap_or(f64::NAN))
}

/// How a DAW reads an equaliser's and a compressor's curves from its devices' parameters.
pub trait CurveReader {
    /// An equaliser's bands, by its parameters; None for a device that isn't one.
    fn eq_bands(&self, rows: &[ParameterRow]) -> Option<Vec<EqBand>>;
    /// A compressor's curve, by its parameters; None for a device that isn't one.
    fn dynamics(&self, rows: &[ParameterRow]) -> Option<Dynamics>;
}

/// One of a device's parameters, as read: its name, value, range and how the DAW shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterRow {
    pub name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub text: Option<String>,
}

/// Each change's curve, from one read of its device made after the last of them (`changes` oldest first, each with
/// the values it moved). Walking back from the newest: a change's after is the state before the one that followed
/// it, and its before puts back the values it moved. None for a device that draws no curve.
pub fn curves(reader: &dyn CurveReader, read: &[ParameterRow], changes: &[(String, Vec<PictureValue>)]) -> Vec<(String, ChangePicture)> {
    let mut state = read.to_vec();
    let mut drawn = Vec::new();
    for (id, moved) in changes.iter().rev() {
        let before = with_priors(&state, moved);
        if let Some(curve) = curve_between(reader, &before, &state, moved) {
            drawn.push((id.clone(), curve));
        }
        state = before;
    }
    drawn.reverse();
    drawn
}

/// The curves one read gives the changes it was for (`ours`, oldest first, each with every value it moved). Only
/// changes still applied are in what was read: the others are left out, and get none. Changes made while it read
/// (`newer`) may be in it too: they're walked back first, and get theirs from a read of their own. None at all when a
/// moved parameter isn't on the device read: another device sits in its place.
pub fn curves_for(
    reader: &dyn CurveReader,
    read: &[ParameterRow],
    ours: &[(String, Vec<PictureValue>)],
    newer: &[(String, Vec<PictureValue>)],
    applied: impl Fn(&str) -> bool,
) -> Vec<(String, ChangePicture)> {
    let walk: Vec<(String, Vec<PictureValue>)> = ours.iter().chain(newer).filter(|(id, _)| applied(id)).cloned().collect();
    let names: std::collections::HashSet<&str> = read.iter().map(|row| row.name.as_str()).collect();
    if !walk.iter().all(|(_, values)| values.iter().all(|value| names.contains(value.name.as_str()))) {
        return Vec::new();
    }
    curves(reader, read, &walk).into_iter().filter(|(id, _)| ours.iter().any(|(ours, _)| ours == id)).collect()
}

/// The parameters with the values a change moved put back as they were.
fn with_priors(rows: &[ParameterRow], moved: &[PictureValue]) -> Vec<ParameterRow> {
    rows.iter()
        .map(|row| match moved.iter().find(|value| value.name == row.name) {
            Some(value) => ParameterRow { value: value.from.unwrap_or(row.value), text: value.from_text.clone(), ..row.clone() },
            None => row.clone(),
        })
        .collect()
}

fn curve_between(
    reader: &dyn CurveReader,
    before: &[ParameterRow],
    after: &[ParameterRow],
    moved: &[PictureValue],
) -> Option<ChangePicture> {
    if let (Some(was), Some(now)) = (reader.eq_bands(before), reader.eq_bands(after)) {
        return Some(ChangePicture::Eq { before: was, after: now, values: moved.to_vec() });
    }
    if let (Some(was), Some(now)) = (reader.dynamics(before), reader.dynamics(after)) {
        return Some(ChangePicture::Dynamics { before: was, after: now, values: moved.to_vec() });
    }
    None
}

/// What was asked, as words, when it isn't what the DAW set: a number off by more than rounding, or words (“2 dB”) whose
/// amount or unit differ from what Live shows.
pub fn asked_apart(asked: &Value, set: Option<f64>, shown: Option<&str>) -> Option<String> {
    match asked {
        Value::Number(number) => {
            let asked = number.as_f64()?;
            let set = set?;
            ((asked - set).abs() > 1e-4 * asked.abs().max(1.)).then(|| format_number(asked, None))
        }
        Value::String(text) => {
            let text = trim(text);
            let shown = shown?;
            let same = match (amount(text), amount(shown)) {
                (Some(a), Some(b)) => (a - b).abs() <= 0.005 * a.abs().max(b.abs()).max(1.) && unit(text) == unit(shown),
                _ => text.eq_ignore_ascii_case(trim(shown)),
            };
            (!same && !text.is_empty()).then(|| head(text, 32))
        }
        _ => None,
    }
}

/// The amount in words such as "-7.50 dB", "3.21 kHz" (3210), "4.00 : 1" (4), "18.0": None for "-inf dB" and words.
pub fn amount(text: &str) -> Option<f64> {
    let text = trim(text);
    let start = text.find(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.')?;
    let rest = &text[start..];
    let end = rest.char_indices().skip(1).find(|(_, c)| !(c.is_ascii_digit() || *c == '.')).map_or(rest.len(), |(at, _)| at);
    let number: f64 = rest[..end].parse().ok()?;
    let unit = trim(&rest[end..]).to_ascii_lowercase();
    let scaled = if unit.starts_with("khz") { number * 1000. } else { number };
    scaled.is_finite().then_some(scaled)
}

fn unit(text: &str) -> String {
    text.trim_start_matches(|c: char| c.is_ascii_digit() || "+-., ".contains(c)).to_ascii_lowercase().replace(' ', "")
}

/// Names an automation lane's parameter and draws the lane to the parameter's range, as discovery read them; a lane
/// with a point outside that range keeps its own.
pub fn name_lane(picture: &mut ChangePicture, name: &str, low: f64, high: f64) {
    if let ChangePicture::Automation { parameter, min, max, before, after, .. } = picture {
        name.clone_into(parameter);
        if before.iter().chain(after.iter()).all(|point| (low..=high).contains(&point[1])) {
            (*min, *max) = (low, high);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: f64) -> ClipNote {
        ClipNote { pitch: 60. + (start as i64 % 12) as f64, start, duration: 0.25, velocity: 100. }
    }

    #[test]
    fn a_picture_stays_within_its_bounds() {
        let many = ChangePicture::Notes {
            length: 512.,
            before: (0..2000).map(|i| note(i as f64 / 4.)).collect(),
            after: (0..2000).map(|i| note(i as f64 / 4.)).collect(),
            partial: false,
        };
        let bounded = many.bounded().unwrap();
        assert!(serde_json::to_vec(&bounded).unwrap().len() <= MAX_BYTES);
        let ChangePicture::Notes { before, after, partial, .. } = &bounded else { panic!() };
        assert!(*partial, "said to be partial");
        assert!(!before.is_empty() && !after.is_empty());
        assert_eq!(after.first().unwrap().start, 0., "the first notes stay");
        // Every note up to where it stops, at a bar's edge: none left out in between.
        let last = after.last().unwrap().start;
        assert_eq!(after.len(), (last * 4.) as usize + 1);
        assert_eq!(((last + 0.25) / 4.).fract(), 0., "it stops at the end of a bar");
        assert_eq!(before.len(), after.len(), "the same bars on both sides");
        let points = ChangePicture::Automation {
            parameter: "Cutoff".into(),
            min: 0.,
            max: 1.,
            before: vec![],
            after: (0..1000).map(|i| [i as f64, 0.5]).collect(),
            words: None,
        };
        let ChangePicture::Automation { after, .. } = points.bounded().unwrap() else { panic!() };
        assert!(after.len() <= MAX_POINTS);
    }

    #[test]
    fn lines_routes_zones_and_items_stay_within_their_bounds() {
        let many = ChangePicture::Lines {
            lines: (1..=40)
                .rev()
                .map(|n| PictureLine { line: n, before: "--- .. .. .. ..".into(), after: format!("C-4 {n:02} .. .. ..") })
                .collect(),
            more: 0,
        };
        let ChangePicture::Lines { lines, more } = many.bounded().unwrap() else { panic!() };
        assert_eq!((lines.len(), more), (MAX_LINES, 40 - MAX_LINES), "the first lines, and how many more");
        assert_eq!((lines[0].line, lines[15].line), (1, 16), "in order");
        let same = ChangePicture::Lines { lines: vec![PictureLine { line: 3, before: "OFF".into(), after: "OFF".into() }], more: 0 };
        assert_eq!(same.bounded(), None, "a line written as it was draws nothing");
        let first = ChangePicture::Lines { lines: vec![PictureLine { line: 0, before: "---".into(), after: "C-4".into() }], more: 0 };
        assert_eq!(first.clone().bounded(), Some(first), "a tracker's first line is its 00");
        let item = |length: f64, fade_out: f64| PictureItem { name: "Vox".into(), start: 1., length, fade_in: 0., fade_out, muted: false };
        let items = ChangePicture::Items { before: vec![item(f64::NAN, 0.)], after: vec![item(2., 9.)], values: vec![] };
        let ChangePicture::Items { before, after, .. } = items.bounded().unwrap() else { panic!() };
        assert!(before.is_empty(), "no length, no item");
        assert_eq!(after[0].fade_out, 2., "a fade is at most the item");
        let zone = PictureZone { name: String::new(), notes: [60, 48], velocities: Some([200, 0]), base: Some(130) };
        let ChangePicture::Zones { after, .. } =
            (ChangePicture::Zones { before: vec![], after: vec![zone], values: vec![] }).bounded().unwrap()
        else {
            panic!()
        };
        assert_eq!((after[0].notes, after[0].velocities, after[0].base), ([48, 60], Some([0, 127]), Some(127)));
        let route = |from: &str| PictureRoute { from: from.into(), to: "Reverb".into(), change: RouteChange::Changed, values: vec![] };
        let ChangePicture::Routes { routes } =
            (ChangePicture::Routes { routes: (0..20).map(|_| route("Bass")).collect() }).bounded().unwrap()
        else {
            panic!()
        };
        assert_eq!(routes.len(), MAX_ROUTES);
        let json = serde_json::to_value(ChangePicture::Routes { routes: vec![route("Bass")] }).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "routes", "routes": [{"from": "Bass", "to": "Reverb", "change": "changed"}]}));
    }

    #[test]
    fn what_cant_be_drawn_is_dropped() {
        let value = |name: &str, to: Option<f64>| PictureValue { name: name.into(), to, ..Default::default() };
        let picture = ChangePicture::Values { values: vec![value("Gain", Some(f64::NAN)), value("", Some(1.)), value("Drive", Some(0.5))] };
        assert_eq!(picture.bounded(), Some(ChangePicture::Values { values: vec![value("Drive", Some(0.5))] }));
        assert_eq!(ChangePicture::Values { values: vec![value("Gain", None)] }.bounded(), None);
        let automation =
            ChangePicture::Automation { parameter: String::new(), min: 1., max: 1., before: vec![[0., 1.]], after: vec![], words: None };
        assert_eq!(automation.bounded(), None, "an empty range draws nothing");
        let silly = Dynamics { threshold_db: -20., ratio: 0.5, knee_db: 0., makeup_db: 0. };
        assert_eq!(ChangePicture::Dynamics { before: silly.clone(), after: silly, values: vec![] }.bounded(), None);
    }

    #[test]
    fn records_without_a_picture_still_read() {
        let old: super::super::contracts::ChangeRecord = serde_json::from_value(serde_json::json!({
            "id":"c1","family":"tempo","title":"Tempo 120 → 124 BPM","from":120.0,"to":124.0,"state":"applied","at":1
        }))
        .unwrap();
        assert_eq!(old.picture, None);
        let eq = ChangePicture::Eq {
            before: vec![EqBand { shape: EqShape::Bell, freq: 1000., gain_db: 0., q: 1., on: true, slope: None }],
            after: vec![EqBand { shape: EqShape::Bell, freq: 1000., gain_db: -3., q: 1., on: true, slope: None }],
            values: vec![],
        };
        let json = serde_json::to_value(&eq).unwrap();
        assert_eq!(json["kind"], "eq");
        assert_eq!(json["after"][0]["gainDb"], -3.0);
        assert_eq!(serde_json::from_value::<ChangePicture>(json).unwrap(), eq);
    }

    #[test]
    fn notes_are_kept_compact_and_old_ones_still_read() {
        let picture = ChangePicture::Notes {
            length: 4.,
            before: vec![],
            after: vec![ClipNote { pitch: 60., start: 0.333_333, duration: 0.25, velocity: 100. }],
            partial: false,
        };
        let json = serde_json::to_value(&picture).unwrap();
        assert_eq!(json, serde_json::json!({"kind":"notes","length":4.0,"before":[],"after":[[0.333, 60, 0.25, 100]]}));
        let read: ChangePicture = serde_json::from_value(json).unwrap();
        let ChangePicture::Notes { after, .. } = read else { panic!() };
        assert_eq!((after[0].start, after[0].pitch), (0.333, 60.));
        // Notes as objects (a picture written another way) read too.
        let objects = serde_json::json!({"kind":"notes","length":4.0,"before":[{"pitch":60.0,"start":0.0,"duration":1.0,"velocity":90.0}],"after":[]});
        let ChangePicture::Notes { before, .. } = serde_json::from_value::<ChangePicture>(objects).unwrap() else { panic!() };
        assert_eq!(before[0].velocity, 90.);
    }

    #[test]
    fn a_picture_this_kumi_cant_read_leaves_its_record_readable() {
        let record = |picture: serde_json::Value| {
            serde_json::from_value::<super::super::contracts::ChangeRecord>(serde_json::json!({
                "id":"c1","family":"parameter","title":"t","state":"applied","at":1,"picture":picture
            }))
        };
        assert_eq!(record(serde_json::json!({"kind":"spectrum","bins":[1,2,3]})).unwrap().picture, None, "a kind from a later Kumi");
        let shape = serde_json::json!({"kind":"eq","before":[],"after":[{"shape":"tilt","freq":1000.0}]});
        assert_eq!(record(shape).unwrap().picture, None, "a shape from a later Kumi");
        assert!(record(serde_json::json!({"kind":"values","values":[{"name":"Gain","to":1.0}]})).unwrap().picture.is_some());
    }
}
