//! What a change looks like, before and after, for the producer to check by eye: the values that moved, an
//! equaliser's curve, a compressor's curve, a clip's notes, an automation lane, a channel's levels, a tracker's lines,
//! routes between tracks, an instrument's keyzones, items on the timeline, the project's tracks, markers on the
//! timeline, a clip's warp markers or an audio clip's waveform. Plain,
//! DAW-neutral data: an integration fills it from its own records, and the terminal draws it knowing nothing of
//! the DAW. Kept small (at most [`MAX_BYTES`] as JSON), since saved conversations keep thousands of changes.

use serde::{Deserialize, Serialize};

use crate::contracts::ClipNote;
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
const MAX_TRACKS: usize = 24;
const MAX_MARKS: usize = 32;
const MAX_WARPS: usize = 64;
/// A waveform's columns: enough for a wide window, at about five bytes each.
pub const MAX_PEAKS: usize = 240;

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
    /// that moved; `more` routes past those kept.
    Routes {
        routes: Vec<PictureRoute>,
        #[serde(default, skip_serializing_if = "is_zero")]
        more: usize,
    },
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
    /// The project's tracks in their order, those a change made or took away marked (new tracks, a group, a copy, a
    /// deleted track); `more` tracks past those kept.
    Tracks {
        tracks: Vec<PictureTrack>,
        #[serde(default, skip_serializing_if = "is_zero")]
        more: usize,
    },
    /// Markers on the timeline (locators, cue points, markers) before and after, each where it is in beats from the
    /// start, with where the ones that moved are in the DAW's words.
    Marks {
        before: Vec<PictureMark>,
        after: Vec<PictureMark>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<PictureValue>,
    },
    /// A clip's warp (or stretch) markers before and after, each as `[beat, second]`: where it is in the clip, in beats,
    /// and the moment of the sample it holds there, in seconds. With the values that moved.
    Warp {
        before: Vec<[f64; 2]>,
        after: Vec<[f64; 2]>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<PictureValue>,
    },
    /// An audio clip's sample at a glance: its level along its `seconds` (each column's peak, 0 to 1), and the part
    /// of it that plays before and after (from and to, in seconds into the sample), with the values that moved.
    Wave {
        seconds: f64,
        #[serde(with = "compact_peaks")]
        peaks: Vec<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<[f64; 2]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<[f64; 2]>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<PictureValue>,
    },
}

/// What a change did to a track: made it, or took it away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrackMark {
    Made,
    Removed,
}

/// One of the project's tracks: its name, its colour ("#rrggbb"), and what the change did to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PictureTrack {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<TrackMark>,
}

/// A marker on the timeline: its name and where it is, in beats from the start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PictureMark {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub at: f64,
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
    /// The picture as a saved record keeps it: a notes picture's numbers to 3 decimals, as `compact_notes` writes them.
    /// One shown as it's made then reads as the same one read back from disk, and a note restated with fewer decimals
    /// (Live's triplet start 2.6666666666666665 against a patch's 2.6667) is the note it was.
    pub fn as_kept(mut self) -> ChangePicture {
        if let ChangePicture::Notes { before, after, .. } = &mut self {
            for note in before.iter_mut().chain(after.iter_mut()) {
                *note = compact_notes::kept(note);
            }
        }
        self
    }
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
            ChangePicture::Routes { routes, more } => {
                let routes: Vec<PictureRoute> = routes
                    .into_iter()
                    .map(|route| PictureRoute {
                        from: cut(&route.from, MAX_NAME),
                        to: cut(&route.to, MAX_NAME),
                        change: route.change,
                        values: values(route.values),
                    })
                    .filter(|route| !route.from.is_empty() || !route.to.is_empty())
                    .collect();
                let past = routes.len().saturating_sub(MAX_ROUTES);
                ChangePicture::Routes { routes: routes.into_iter().take(MAX_ROUTES).collect(), more: more + past }
            }
            ChangePicture::Zones { before, after, values: list } => {
                let zones = |zones: Vec<PictureZone>| zones.into_iter().take(MAX_ZONES).map(PictureZone::bounded).collect::<Vec<_>>();
                ChangePicture::Zones { before: zones(before), after: zones(after), values: values(list) }
            }
            ChangePicture::Items { before, after, values: list } => {
                let items =
                    |items: Vec<PictureItem>| items.into_iter().filter_map(PictureItem::bounded).take(MAX_ITEMS).collect::<Vec<_>>();
                ChangePicture::Items { before: items(before), after: items(after), values: values(list) }
            }
            ChangePicture::Tracks { tracks, more } => {
                let tracks: Vec<PictureTrack> = tracks.into_iter().map(PictureTrack::bounded).collect();
                let (tracks, past) = around_marks(tracks, MAX_TRACKS);
                ChangePicture::Tracks { tracks, more: more + past }
            }
            ChangePicture::Marks { before, after, values: list } => {
                let marks = |marks: Vec<PictureMark>| {
                    let mut marks: Vec<PictureMark> = marks
                        .into_iter()
                        .filter(|mark| mark.at.is_finite())
                        .map(|mark| PictureMark { name: cut(&mark.name, MAX_NAME), at: mark.at })
                        .collect();
                    marks.sort_by(|a, b| a.at.total_cmp(&b.at));
                    marks
                };
                let (before, after) = around_change(marks(before), marks(after), MAX_MARKS, |mark| mark.at);
                ChangePicture::Marks { before, after, values: values(list) }
            }
            ChangePicture::Warp { before, after, values: list } => {
                let markers = |markers: Vec<[f64; 2]>| {
                    let mut markers: Vec<[f64; 2]> =
                        markers.into_iter().filter(|[beat, second]| beat.is_finite() && second.is_finite()).collect();
                    markers.sort_by(|a, b| a[0].total_cmp(&b[0]));
                    markers
                };
                let (before, after) = around_change(markers(before), markers(after), MAX_WARPS, |marker| marker[0]);
                ChangePicture::Warp { before, after, values: values(list) }
            }
            ChangePicture::Wave { seconds, peaks, before, after, values: list } => {
                if !seconds.is_finite() || seconds <= 0. {
                    return None;
                }
                let peaks: Vec<f64> = peaks.into_iter().map(|peak| if peak.is_finite() { peak.clamp(0., 1.) } else { 0. }).collect();
                let span = |span: Option<[f64; 2]>| {
                    span.filter(|[from, to]| from.is_finite() && to.is_finite() && to > from)
                        .map(|[from, to]| [from.max(0.), to.min(seconds)])
                };
                ChangePicture::Wave {
                    seconds,
                    peaks: pooled(&peaks, MAX_PEAKS),
                    before: span(before),
                    after: span(after),
                    values: values(list),
                }
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
            ChangePicture::Routes { routes, more } => {
                let kept = half(routes.len());
                ChangePicture::Routes { more: more + routes.len() - kept, routes: routes.into_iter().take(kept).collect() }
            }
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
            ChangePicture::Tracks { tracks, more } => {
                let most = half(tracks.len());
                let (tracks, past) = around_marks(tracks, most);
                ChangePicture::Tracks { tracks, more: more + past }
            }
            ChangePicture::Marks { before, after, values } => {
                let most = half(before.len().max(after.len()));
                let (before, after) = around_change(before, after, most, |mark| mark.at);
                ChangePicture::Marks { before, after, values: values.iter().take(values.len() / 2).cloned().collect() }
            }
            ChangePicture::Warp { before, after, values } => {
                let most = half(before.len().max(after.len()));
                let (before, after) = around_change(before, after, most, |marker| marker[0]);
                ChangePicture::Warp { before, after, values: values.iter().take(values.len() / 2).cloned().collect() }
            }
            ChangePicture::Wave { seconds, peaks, before, after, values } => ChangePicture::Wave {
                seconds,
                peaks: pooled(&peaks, half(peaks.len())),
                before,
                after,
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
            ChangePicture::Routes { routes, .. } => routes.is_empty(),
            ChangePicture::Zones { before, after, .. } => before.is_empty() && after.is_empty(),
            ChangePicture::Items { before, after, .. } => before.is_empty() && after.is_empty(),
            ChangePicture::Tracks { tracks, .. } => tracks.is_empty(),
            ChangePicture::Marks { before, after, .. } => before.is_empty() && after.is_empty(),
            ChangePicture::Warp { before, after, .. } => before.is_empty() && after.is_empty(),
            ChangePicture::Wave { peaks, .. } => peaks.is_empty(),
        }
    }
}

impl PictureTrack {
    fn bounded(self) -> PictureTrack {
        PictureTrack { name: cut(&self.name, MAX_NAME), color: self.color.filter(|color| is_hex_color(color)), mark: self.mark }
    }
}

/// "#rrggbb".
fn is_hex_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// At most `most` tracks, the ones marked among them: those around the first marked one (from just before it), else the
/// first. How many were left out comes back too.
fn around_marks(tracks: Vec<PictureTrack>, most: usize) -> (Vec<PictureTrack>, usize) {
    if tracks.len() <= most {
        return (tracks, 0);
    }
    let first = tracks.iter().position(|track| track.mark.is_some()).unwrap_or(0);
    // (a track or two before the first marked one, for where it is)
    let start = first.saturating_sub(2).min(tracks.len() - most);
    let past = tracks.len() - most;
    (tracks.into_iter().skip(start).take(most).collect(), past)
}

/// At most `most` of each side's items, the same stretch of the timeline on both (each item where `at` says): from a little
/// before the first that the change made, moved or took away, so what changed stays, and what stayed reads as it was.
/// The items are in order, one at a time on each side.
fn around_change<T: PartialEq>(before: Vec<T>, after: Vec<T>, most: usize, at: impl Fn(&T) -> f64) -> (Vec<T>, Vec<T>) {
    if before.len() <= most && after.len() <= most {
        return (before, after);
    }
    let mut times: Vec<f64> = before.iter().chain(&after).map(&at).collect();
    times.sort_by(f64::total_cmp);
    times.dedup();
    let changed = before.iter().filter(|item| !after.contains(item)).chain(after.iter().filter(|item| !before.contains(item)));
    let first = changed.map(&at).fold(f64::INFINITY, f64::min);
    let start = times.iter().position(|time| *time >= first).unwrap_or(0).saturating_sub(2).min(times.len().saturating_sub(most.max(1)));
    let (low, high) = (times[start], times[(start + most.max(1) - 1).min(times.len() - 1)]);
    let keep = |items: Vec<T>| items.into_iter().filter(|item| (low..=high).contains(&at(item))).collect();
    (keep(before), keep(after))
}

/// At most `most` levels, each the highest of the ones it stands for: a waveform's peaks stay where they were.
fn pooled(peaks: &[f64], most: usize) -> Vec<f64> {
    if peaks.len() <= most || most == 0 {
        return if most == 0 { Vec::new() } else { peaks.to_vec() };
    }
    (0..most)
        .map(|column| {
            let (from, to) = (column * peaks.len() / most, ((column + 1) * peaks.len() / most).max(column * peaks.len() / most + 1));
            peaks[from..to].iter().copied().fold(0., f64::max)
        })
        .collect()
}

/// A waveform's peaks to 2 decimals (they're drawn a column at a time, a few rows high).
mod compact_peaks {
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_json::Value;

    pub fn serialize<S: Serializer>(peaks: &[f64], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(peaks.iter().map(|peak| {
            let rounded = (peak * 100.).round() / 100.;
            if rounded.fract() == 0. {
                Value::from(rounded as i64)
            } else {
                Value::from(rounded)
            }
        }))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<f64>, D::Error> {
        let items: Vec<Value> = Vec::deserialize(deserializer)?;
        Ok(items.iter().map(|item| item.as_f64().unwrap_or(0.)).collect())
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

    /// A note's number as it's written: to 3 decimals.
    fn three_places(n: f64) -> f64 {
        (n * 1000.).round() / 1000.
    }

    /// A note as it's written, each number to 3 decimals: what reading it back gives.
    pub(super) fn kept(note: &ClipNote) -> ClipNote {
        ClipNote {
            start: three_places(note.start),
            pitch: three_places(note.pitch),
            duration: three_places(note.duration),
            velocity: three_places(note.velocity),
        }
    }

    fn number(n: f64) -> Value {
        let rounded = three_places(n);
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

/// One picture for a change made alike on several tracks, from each track's name and its own picture: the routes made
/// when each is a route, else each value that moved, named by its track ("Drums · Volume"). None when a part is drawn
/// as more than values (a curve, notes, a lane).
pub fn across(parts: &[(String, ChangePicture)]) -> Option<ChangePicture> {
    if parts.iter().all(|(_, picture)| matches!(picture, ChangePicture::Routes { .. })) {
        let routes = parts
            .iter()
            .flat_map(|(_, picture)| match picture {
                ChangePicture::Routes { routes, .. } => routes.clone(),
                _ => Vec::new(),
            })
            .collect();
        let more = parts.iter().map(|(_, picture)| if let ChangePicture::Routes { more, .. } = picture { *more } else { 0 }).sum();
        return ChangePicture::Routes { routes, more }.bounded();
    }
    let mut values = Vec::new();
    for (track, picture) in parts {
        let named = |value: &PictureValue| PictureValue { name: format!("{track} · {}", value.name), ..value.clone() };
        match picture {
            ChangePicture::Values { values: list } => values.extend(list.iter().map(named)),
            ChangePicture::Routes { routes, .. } => {
                for route in routes {
                    values.extend(route.values.iter().map(named));
                }
            }
            ChangePicture::Mix { before, after } => values.extend(mix_values(before, after).iter().map(named)),
            _ => return None,
        }
    }
    ChangePicture::Values { values }.bounded()
}

/// A channel's levels that moved, as values: the fader and pan in their ranges, mute and solo, each send.
fn mix_values(before: &MixLevels, after: &MixLevels) -> Vec<PictureValue> {
    let mut values = Vec::new();
    let mut level = |name: &str, from: Option<f64>, to: Option<f64>, range: [f64; 2], texts: [&Option<String>; 2]| {
        if to.is_some() && (from != to || texts[0] != texts[1]) {
            values.push(PictureValue {
                name: name.into(),
                from,
                to,
                min: Some(range[0]),
                max: Some(range[1]),
                from_text: texts[0].clone(),
                to_text: texts[1].clone(),
                asked: None,
            });
        }
    };
    level("Volume", before.volume, after.volume, [0., 1.], [&before.volume_text, &after.volume_text]);
    level("Pan", before.pan, after.pan, [-1., 1.], [&before.pan_text, &after.pan_text]);
    for send in &after.sends {
        let was = before.sends.iter().find(|old| old.name == send.name);
        level(
            &format!("Send {}", send.name),
            was.map(|old| old.level),
            Some(send.level),
            [0., 1.],
            [&was.and_then(|old| old.text.clone()), &send.text],
        );
    }
    let words = |on: bool| if on { "on" } else { "off" };
    for (name, from, to) in [("Mute", before.mute, after.mute), ("Solo", before.solo, after.solo)] {
        if let Some(to) = to.filter(|to| from != Some(*to)) {
            values.push(PictureValue {
                name: name.into(),
                from_text: from.map(|from| words(from).into()),
                to_text: Some(words(to).into()),
                ..Default::default()
            });
        }
    }
    values
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
    fn a_kept_notes_picture_is_the_one_read_back_from_disk() {
        let made = ChangePicture::Notes {
            length: 4.,
            before: vec![ClipNote { start: 2.6666666666666665, pitch: 36., duration: 0.3333333333333333, velocity: 90.25049 }],
            after: vec![ClipNote { start: 2.6667, pitch: 36., duration: 0.3333, velocity: 110. }],
            partial: false,
        };
        let read_back: ChangePicture = serde_json::from_value(serde_json::to_value(&made).unwrap()).unwrap();
        assert_eq!(made.clone().as_kept(), read_back);
        // A triplet's start as Live keeps it and as a patch restates it: the same note, once kept.
        let ChangePicture::Notes { before, after, .. } = made.as_kept() else { unreachable!() };
        assert_eq!((before[0].start, before[0].duration), (after[0].start, after[0].duration));
        assert_eq!(before[0].velocity, 90.25);
        // Another kind of picture keeps its numbers as they are.
        let values = ChangePicture::Values { values: vec![] };
        assert_eq!(values.clone().as_kept(), values);
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
        let ChangePicture::Routes { routes, more } =
            (ChangePicture::Routes { routes: (0..20).map(|_| route("Bass")).collect(), more: 0 }).bounded().unwrap()
        else {
            panic!()
        };
        assert_eq!((routes.len(), more), (MAX_ROUTES, 20 - MAX_ROUTES), "the first, and how many more");
        let json = serde_json::to_value(ChangePicture::Routes { routes: vec![route("Bass")], more: 0 }).unwrap();
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
        let old: crate::contracts::ChangeRecord = serde_json::from_value(serde_json::json!({
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
    fn tracks_marks_warps_and_waves_stay_within_their_bounds() {
        let track = |n: usize, mark: Option<TrackMark>| PictureTrack { name: format!("Track {n}"), color: Some("#ff8800".into()), mark };
        // 60 tracks, two new ones deep in the list: the ones kept are around them, and the rest counted.
        let tracks: Vec<PictureTrack> = (0..60).map(|n| track(n, (n == 20 || n == 21).then_some(TrackMark::Made))).collect();
        let ChangePicture::Tracks { tracks, more } = (ChangePicture::Tracks { tracks, more: 0 }).bounded().unwrap() else { panic!() };
        assert_eq!((tracks.len(), more), (MAX_TRACKS, 60 - MAX_TRACKS));
        assert_eq!(tracks[2].name, "Track 20", "two tracks before the first new one, for where it is");
        assert!(tracks.iter().filter(|track| track.mark == Some(TrackMark::Made)).count() == 2);
        // Near the end, the last ones.
        let tracks: Vec<PictureTrack> = (0..60).map(|n| track(n, (n == 58).then_some(TrackMark::Made))).collect();
        let ChangePicture::Tracks { tracks, .. } = (ChangePicture::Tracks { tracks, more: 0 }).bounded().unwrap() else { panic!() };
        assert_eq!(tracks.last().unwrap().name, "Track 59");
        let odd = PictureTrack { name: "Bass".into(), color: Some("orange".into()), mark: None };
        let ChangePicture::Tracks { tracks, .. } = (ChangePicture::Tracks { tracks: vec![odd], more: 0 }).bounded().unwrap() else {
            panic!()
        };
        assert_eq!(tracks[0].color, None, "a colour is #rrggbb or none");
        let json = serde_json::to_value(ChangePicture::Tracks { tracks: vec![track(1, Some(TrackMark::Removed))], more: 0 }).unwrap();
        assert_eq!(json, serde_json::json!({"kind":"tracks","tracks":[{"name":"Track 1","color":"#ff8800","mark":"removed"}]}));

        let mark = |name: &str, at: f64| PictureMark { name: name.into(), at };
        let marks = ChangePicture::Marks { before: vec![mark("B", 16.), mark("A", 0.)], after: vec![mark("X", f64::NAN)], values: vec![] };
        let ChangePicture::Marks { before, after, .. } = marks.bounded().unwrap() else { panic!() };
        assert_eq!(before.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["A", "B"], "in their order on the timeline");
        assert!(after.is_empty(), "nowhere is no marker");

        // A marker moved among 500: the same stretch kept on both sides, around it, so the others still read as they were.
        let markers: Vec<[f64; 2]> = (0..500).map(|n| [n as f64, n as f64 / 2.]).collect();
        let mut moved = markers.clone();
        moved[300][0] = 300.5;
        let warp = ChangePicture::Warp { before: markers, after: moved, values: vec![] };
        let ChangePicture::Warp { before, after, .. } = warp.bounded().unwrap() else { panic!() };
        assert!(before.len() <= MAX_WARPS && after.len() <= MAX_WARPS);
        assert_eq!((before[0], after[0]), ([298., 149.], [298., 149.]), "from a little before the one moved");
        assert!(after.contains(&[300.5, 150.]) && before.contains(&[300., 150.]));
        assert_eq!(before.iter().filter(|marker| !after.contains(marker)).count(), 1, "only the one moved reads as moved");
        // A late new locator in a long Set stays.
        let many: Vec<PictureMark> = (0..100).map(|n| mark("L", n as f64 * 16.)).collect();
        let mut more = many.clone();
        more.push(mark("Coda", 1601.));
        let ChangePicture::Marks { after, .. } = (ChangePicture::Marks { before: many, after: more, values: vec![] }).bounded().unwrap()
        else {
            panic!()
        };
        assert!(after.iter().any(|mark| mark.name == "Coda"), "{after:?}");

        // A long sample's peaks, pooled: each column keeps the loudest of what it stands for.
        let mut peaks = vec![0.1; 10_000];
        peaks[5_000] = 0.9;
        let wave = ChangePicture::Wave { seconds: 300., peaks, before: Some([-1., 10.]), after: Some([20., 400.]), values: vec![] };
        let bounded = wave.bounded().unwrap();
        assert!(serde_json::to_vec(&bounded).unwrap().len() <= MAX_BYTES);
        let ChangePicture::Wave { peaks, before, after, .. } = &bounded else { panic!() };
        assert_eq!(peaks.len(), MAX_PEAKS);
        assert_eq!(peaks.iter().filter(|peak| **peak == 0.9).count(), 1, "the one loud moment is still there");
        assert_eq!((*before, *after), (Some([0., 10.]), Some([20., 300.])), "within the sample");
        let json = serde_json::to_value(&bounded).unwrap();
        assert_eq!(json["peaks"][0], serde_json::json!(0.1), "to 2 decimals");
        assert_eq!(serde_json::from_value::<ChangePicture>(json).unwrap(), bounded);
        let silent = ChangePicture::Wave { seconds: 0., peaks: vec![0.5], before: None, after: None, values: vec![] };
        assert_eq!(silent.bounded(), None, "no length, nothing to draw it along");
    }

    #[test]
    fn a_change_made_alike_on_several_tracks_is_one_picture_of_each_tracks_values() {
        let mix = |volume: f64, text: &str, mute: Option<bool>| MixLevels {
            volume: Some(volume),
            volume_text: Some(text.into()),
            mute,
            ..Default::default()
        };
        let parts = vec![
            ("Drums".to_string(), ChangePicture::Mix { before: mix(0.85, "0.0 dB", Some(false)), after: mix(0.7, "-4.5 dB", Some(false)) }),
            ("Bass".to_string(), ChangePicture::Mix { before: mix(0.85, "0.0 dB", Some(false)), after: mix(0.85, "0.0 dB", Some(true)) }),
        ];
        let ChangePicture::Values { values } = across(&parts).unwrap() else { panic!() };
        assert_eq!(values.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["Drums · Volume", "Bass · Mute"], "only what moved");
        assert_eq!((values[0].min, values[0].max, values[0].to_text.as_deref()), (Some(0.), Some(1.), Some("-4.5 dB")));
        assert_eq!((values[1].from_text.as_deref(), values[1].to_text.as_deref()), (Some("off"), Some("on")));
        let route = |from: &str| PictureRoute { from: from.into(), to: "Bus".into(), change: RouteChange::Changed, values: vec![] };
        let routes = vec![
            ("Drums".to_string(), ChangePicture::Routes { routes: vec![route("Drums")], more: 0 }),
            ("Perc".to_string(), ChangePicture::Routes { routes: vec![route("Perc")], more: 0 }),
        ];
        let ChangePicture::Routes { routes, .. } = across(&routes).unwrap() else { panic!() };
        assert_eq!(routes.len(), 2, "routes stay routes");
        let notes = ChangePicture::Notes { length: 4., before: vec![], after: vec![note(0.)], partial: false };
        assert_eq!(across(&[("Keys".to_string(), notes)]), None, "notes aren't values");
    }

    #[test]
    fn a_record_saved_before_runs_and_counted_routes_reads_and_one_saved_now_reads_back_the_same() {
        // As an older Kumi saved it: no run, routes without a count.
        let older: crate::contracts::ChangeRecord = serde_json::from_value(serde_json::json!({
            "id":"c1","family":"mixer","title":"Kick: output to Drum Bus","state":"applied","at":1,"track":{"name":"Kick"},
            "picture":{"kind":"routes","routes":[{"from":"Kick","to":"Drum Bus","change":"changed"}]}
        }))
        .unwrap();
        assert_eq!(older.batch, None);
        let Some(ChangePicture::Routes { routes, more }) = &older.picture else { panic!("{:?}", older.picture) };
        assert_eq!((routes.len(), *more), (1, 0));
        // As this one saves it: its run and a new kind, read back as they were.
        let mut now = older.clone();
        now.batch = Some("0123456789ab:set_routing".into());
        now.picture =
            Some(ChangePicture::Marks { before: vec![], after: vec![PictureMark { name: "Drop".into(), at: 64. }], values: vec![] });
        let saved = serde_json::to_value(&now).unwrap();
        assert_eq!(serde_json::from_value::<crate::contracts::ChangeRecord>(saved.clone()).unwrap(), now);
        // And without the run and the picture (what an older Kumi keeps of it, saving again), it's the record it was.
        let mut kept = saved;
        kept.as_object_mut().unwrap().retain(|key, _| !matches!(key.as_str(), "batch" | "picture"));
        let read: crate::contracts::ChangeRecord = serde_json::from_value(kept).unwrap();
        assert_eq!(read, crate::contracts::ChangeRecord { batch: None, picture: None, ..now });
    }

    #[test]
    fn a_picture_this_kumi_cant_read_leaves_its_record_readable() {
        let record = |picture: serde_json::Value| {
            serde_json::from_value::<crate::contracts::ChangeRecord>(serde_json::json!({
                "id":"c1","family":"parameter","title":"t","state":"applied","at":1,"picture":picture
            }))
        };
        assert_eq!(record(serde_json::json!({"kind":"spectrum","bins":[1,2,3]})).unwrap().picture, None, "a kind from a later Kumi");
        let shape = serde_json::json!({"kind":"eq","before":[],"after":[{"shape":"tilt","freq":1000.0}]});
        assert_eq!(record(shape).unwrap().picture, None, "a shape from a later Kumi");
        assert!(record(serde_json::json!({"kind":"values","values":[{"name":"Gain","to":1.0}]})).unwrap().picture.is_some());
    }
}
