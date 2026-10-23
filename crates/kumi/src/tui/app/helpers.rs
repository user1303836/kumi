//! Pure focus and change pictures used by the full-screen app.
use crate::tui::{
    style::{hex, palette, ColorDepth, Rgb, Style},
    width::{text_width, truncate},
    wrap::Span,
};
use kumi_agent::core::contracts::*;
use kumi_common::{
    js::{number, string},
    time::since,
};
use kumi_types::contracts::*;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::LazyLock};

pub fn is_command(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('/') else { return false };
    let letters = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
    letters > 0 && (letters == rest.len() || rest[letters..].chars().next().is_some_and(|c| string::trim(&c.to_string()).is_empty()))
}

pub fn chip_color(color: Option<&str>) -> Rgb {
    let Some(rgb) = color.and_then(|s| hex(s).ok()) else { return palette::DIM };
    let luminance = (0.2126 * rgb[0] as f64 + 0.7152 * rgb[1] as f64 + 0.0722 * rgb[2] as f64) / 255.;
    if luminance >= 0.3 {
        rgb
    } else {
        rgb.map(|c| number::round(c as f64 + (255. - c as f64) * 0.45) as u8)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusPath {
    pub crumbs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub context: String,
}
pub fn focus_path(focus: &DawFocus) -> FocusPath {
    let focus = filled(crate::daw::view(), focus);
    let editor = focus.editor_view.as_ref().map(|view| view.kind);
    let mut crumbs = vec![];
    let mut value = None;
    if let Some(track) = &focus.track {
        crumbs.push(track.name.clone());
    }
    let item = focus.item.as_ref().filter(|item| item.name.is_some());
    if let (Some(ViewKind::NoteEditor), Some(item)) = (editor, item) {
        crumbs.push(item.name.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| format!("Untitled {}", item.kind.word())));
    } else if editor == Some(ViewKind::DeviceChain) && focus.device.as_ref().is_some_and(|s| !s.is_empty()) {
        crumbs.push(focus.device.clone().unwrap());
        if let Some(p) = focus.parameter.as_ref().filter(|p| p.owner.as_ref().is_none_or(|s| s.is_empty()) || p.owner == focus.device) {
            crumbs.push(p.name.clone());
            value = p.value.clone().filter(|s| !s.is_empty());
        }
    } else if let Some(section) = focus.section.as_ref().and_then(|section| section.name.as_ref()).filter(|s| !s.is_empty()) {
        crumbs.push(section.clone());
    }
    // The views in the DAW's own words.
    let mut context: Vec<String> = [&focus.main_view, &focus.editor_view].into_iter().flatten().map(|view| view.label.clone()).collect();
    if let Some(n) = focus.selected_notes.filter(|n| *n != 0) {
        context.push(format!("{n} {} selected", if n == 1 { "note" } else { "notes" }));
    }
    FocusPath { crumbs, value, context: context.join(" · ") }
}
pub fn fit_crumbs(crumbs: &[String], width: i32) -> Vec<String> {
    if crumbs.is_empty() || text_width(&crumbs.join(" › ")) <= width {
        return crumbs.to_vec();
    }
    let first = truncate(&crumbs[0], 12);
    if crumbs.len() > 2 {
        let collapsed = vec![crumbs[0].clone(), "…".into(), crumbs.last().unwrap().clone()];
        if text_width(&collapsed.join(" › ")) <= width {
            return collapsed;
        }
        return vec![first.clone(), "…".into(), truncate(crumbs.last().unwrap(), (width - text_width(&first) - 6).max(1))];
    }
    if crumbs.len() == 2 {
        vec![first.clone(), truncate(&crumbs[1], (width - text_width(&first) - 3).max(1))]
    } else {
        vec![truncate(&crumbs[0], width)]
    }
}
/// What the producer touched last in the DAW, for which body FOCUS shows (the names are Live's, as kept).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Touched {
    /// The device chain: the tree.
    #[serde(rename = "device")]
    Tree,
    /// The item in the note editor: its notes.
    #[serde(rename = "clip")]
    Notes,
    /// The launcher grid.
    #[serde(rename = "session")]
    Grid,
    /// The timeline (where an item in the note editor is read as the timeline's too).
    #[serde(rename = "arrangement")]
    Timeline,
    /// A tracker's pattern or phrase editor.
    #[serde(rename = "tracker")]
    Tracker,
    /// A tracker's pattern matrix.
    #[serde(rename = "matrix")]
    Matrix,
    /// An instrument's keyzones.
    #[serde(rename = "keyzones")]
    Keyzones,
    /// A track's routing.
    #[serde(rename = "routing")]
    Routing,
    /// The mixer's strips.
    #[serde(rename = "mixer")]
    Mixer,
}
pub fn touched_next(before: Option<&DawFocus>, after: Option<&DawFocus>, was: Option<Touched>) -> Option<Touched> {
    let view = crate::daw::view();
    let after = filled(view, after?);
    let editor = |focus: &DawFocus| focus.editor_view.as_ref().map(|view| view.kind);
    let main = |focus: &DawFocus| focus.main_view.as_ref().map(|view| view.kind);
    let index = |focus: &DawFocus| focus.section.as_ref().and_then(|section| section.index);
    let item = |focus: &DawFocus| focus.item.as_ref().map(|item| (item.item_ref.clone(), item.name.clone())).unwrap_or_default();
    let on_timeline = main(&after) == Some(ViewKind::Timeline);
    // A DAW whose note editor over the timeline shows a take's notes (REAPER) reads them, not the timeline.
    let notes = if on_timeline && !view.timeline_notes { Touched::Timeline } else { Touched::Notes };
    // A tracker's views, an instrument and a track's routing have bodies of their own (a tracker's pattern stays
    // under its DSP frame).
    let own_of = |focus: &DawFocus| match (main(focus), editor(focus)) {
        (_, Some(ViewKind::Routing)) => Some(Touched::Routing),
        (Some(ViewKind::Mixer), _) | (_, Some(ViewKind::Mixer)) => Some(Touched::Mixer),
        (Some(ViewKind::PatternEditor), _) => Some(Touched::Tracker),
        (Some(ViewKind::Sequencer), _) => Some(Touched::Matrix),
        (Some(ViewKind::Instrument), _) => Some(Touched::Keyzones),
        _ => None,
    };
    let own = own_of(&after);
    let shown = own.unwrap_or(if on_timeline { Touched::Timeline } else { Touched::Grid });
    let Some(before) = before.map(|before| filled(view, before)) else {
        return Some(match editor(&after) {
            Some(ViewKind::NoteEditor) => notes,
            Some(ViewKind::DeviceChain) if own.is_none() => Touched::Tree,
            _ => shown,
        });
    };
    // A pane beside the main view (Renoise's pattern matrix) shows when the producer moves in it, not playback.
    let pane_open = |kind: ViewKind| after.panes.iter().flatten().any(|pane| pane.kind == kind);
    let producer_moved = index(&after) != index(&before) && after.section_follows != Some(true);
    // Beside a body of its own, a device chain shows for another device on the same track: moving to another track
    // is moving in the tracker.
    let same_track = own.is_none() || after.track_ref == before.track_ref;
    // Coming into a view with a body of its own, or leaving one, shows what's there now.
    let own_moved = (own.is_some() || own_of(&before).is_some()) && own != own_of(&before);
    Some(if after.main_view != before.main_view || own_moved {
        shown
    } else if index(&after) != index(&before) && main(&after) == Some(ViewKind::Launcher) {
        Touched::Grid
    } else if editor(&after) == Some(ViewKind::NoteEditor)
        && (editor(&before) != Some(ViewKind::NoteEditor) || item(&after) != item(&before))
    {
        notes
    } else if pane_open(ViewKind::Sequencer) && producer_moved {
        Touched::Matrix
    } else if editor(&after) == Some(ViewKind::DeviceChain)
        && (editor(&before) != Some(ViewKind::DeviceChain)
            || (same_track && (after.device != before.device || after.chain != before.chain || after.track_ref != before.track_ref)))
    {
        Touched::Tree
    } else if editor(&after) != editor(&before)
        && !editor(&after).is_some_and(|kind| matches!(kind, ViewKind::NoteEditor | ViewKind::DeviceChain))
    {
        // The editor went away, or to one with no body of its own: what the view shows (REAPER's arrange after its MIDI
        // editor or an FX window).
        shown
    } else if let Some(own) = own.filter(|_| item(&after) != item(&before) || after.track_ref != before.track_ref) {
        own
    } else {
        was.unwrap_or(shown)
    })
}
/// The project's name an observation event gives, as the app shows it: the DAW's name for it, or the label's words
/// when an older event has none.
pub fn observed_name(event: &serde_json::Value) -> Option<String> {
    event["name"].as_str().and_then(shown_name).or_else(|| set_name_from(event["label"].as_str().unwrap_or("")))
}
/// A project's name as the app shows it: trimmed, and none for one that's empty or runs over lines.
pub fn shown_name(name: &str) -> Option<String> {
    if name.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    let name = string::trim(name);
    (!name.is_empty()).then(|| name.into())
}
pub fn set_name_from(label: &str) -> Option<String> {
    let (name, suffix) = label.strip_prefix("Current open Set: ")?.rsplit_once(" — ")?;
    if suffix.contains('—') || name.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    let name = string::trim(name);
    (!name.is_empty()).then(|| name.into())
}
/// English display names for every two/three-letter language accepted by voice settings,
/// extracted from the reference runtime's ICU data; unknown codes retain their spelling.
pub fn language_name(code: &str) -> String {
    static NAMES: LazyLock<HashMap<String, String>> = LazyLock::new(|| serde_json::from_str(include_str!("language-names.json")).unwrap());
    if code == "auto" {
        "any language".into()
    } else {
        NAMES.get(&code.to_ascii_lowercase()).cloned().unwrap_or_else(|| code.into())
    }
}
pub fn catch_up_text(catch: &CatchUp, now: f64) -> String {
    let when = since(catch.last_seen_at as f64, now);
    let more = if catch.more != 0 { format!("; and {} more", catch.more) } else { String::new() };
    if catch.after_reconnect == Some(true) {
        format!("While {} was away, {} changed: {}{more}.", crate::daw::words().app, catch.set, catch.lines.join("; "))
    } else if catch.lines.is_empty() {
        format!("Nothing changed in {} since you were last here, {when}.", catch.set)
    } else {
        format!("Since you were last here ({when}): {}{more}.", catch.lines.join("; "))
    }
}
#[allow(dead_code)] // Used by the app orchestration port in progress.
pub(super) fn clock_of(ms: f64) -> String {
    let whole = (ms / 1000.).floor().max(0.) as u64;
    let (hours, minutes, seconds) = (whole / 3600, whole % 3600 / 60, whole % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}
#[allow(dead_code)]
pub(super) fn elapsed(ms: f64) -> String {
    format!("{}s", number::to_fixed(ms.max(0.) / 1000., 1))
}
#[allow(dead_code)]
pub(super) fn mix_rgb(from: Rgb, to: Rgb, amount: f64) -> Rgb {
    std::array::from_fn(|i| number::round(from[i] as f64 + (to[i] as f64 - from[i] as f64) * amount) as u8)
}
#[allow(dead_code)]
pub(super) fn chunk(text: &str, width: usize) -> Vec<String> {
    text.encode_utf16().collect::<Vec<_>>().chunks(width.max(1)).map(String::from_utf16_lossy).collect()
}
fn span(text: impl Into<String>, color: Rgb) -> Span {
    Span::styled(text, Style::fg(color))
}
pub fn change_picture(change: &ChangeRecord, width: i32, depth: ColorDepth) -> Option<Vec<Vec<Span>>> {
    if let Some(clip) = &change.clip {
        return clip_picture(
            clip.length,
            &clip.notes.iter().map(|n| ClipViewNote { note: n.clone(), selected: None }).collect::<Vec<_>>(),
            width,
            2,
        );
    }
    if let Some(devices) = &change.devices {
        return devices_picture(devices, width);
    }
    if let Some(colors) = &change.colors {
        if matches!(depth, ColorDepth::Colors16 | ColorDepth::None) {
            return None;
        }
        let mut row = vec![];
        if let Some(from) = colors.from.as_ref().filter(|s| !s.is_empty()) {
            row.extend([span("████", chip_color(Some(from))), span(" → ", palette::FAINT)]);
        }
        row.push(span("████", chip_color(Some(&colors.to))));
        return Some(vec![row]);
    }
    let (from, to, [min, max]) = (change.from?, change.to?, change.range?);
    if max <= min {
        return None;
    }
    let cells = ((width - 3) as f64 / 2.).floor().clamp(4., 12.) as usize;
    let bar = |value: f64| {
        let filled = number::round(((value - min) / (max - min)).clamp(0., 1.) * cells as f64) as usize;
        "█".repeat(filled) + &"░".repeat(cells - filled)
    };
    Some(vec![vec![span(bar(from), palette::FAINT), span(" → ", palette::FAINT), span(bar(to), palette::ACCENT)]])
}
fn device_row(devices: &[String], lit: Option<f64>, room: i32) -> Vec<Span> {
    let names: Vec<_> = devices.iter().map(|s| truncate(if s.is_empty() { "Device" } else { s }, 16)).collect();
    if names.is_empty() {
        return vec![span("empty", palette::FAINT)];
    }
    let end = (names.len() - 1) as f64;
    let cost = |from: f64, to: f64| {
        names[from as usize..((to + 1.) as usize).min(names.len())].iter().map(|s| text_width(s) as f64).sum::<f64>()
            + 3. * (to - from)
            + if from > 0. { 4. } else { 0. }
            + if to < end { 4. } else { 0. }
    };
    let mut first = lit.filter(|n| *n >= 0. && *n < names.len() as f64).unwrap_or(end);
    let mut last = first;
    loop {
        let mut grew = false;
        if last < end && cost(first, last + 1.) <= room as f64 {
            last += 1.;
            grew = true;
        }
        if first > 0. && cost(first - 1., last) <= room as f64 {
            first -= 1.;
            grew = true;
        }
        if !grew {
            break;
        }
    }
    let mut spans = vec![];
    if first > 0. {
        spans.extend([span("…", palette::FAINT), span(" → ", palette::FAINT)]);
    }
    for (offset, name) in names[(first as usize).min(names.len())..((last + 1.) as usize).min(names.len())].iter().enumerate() {
        if offset > 0 {
            spans.push(span(" → ", palette::FAINT));
        }
        spans.push(span(name, if Some(first + offset as f64) == lit { palette::ACCENT } else { palette::DIM }));
    }
    if last < end {
        spans.extend([span(" → ", palette::FAINT), span("…", palette::FAINT)]);
    }
    spans
}
fn devices_picture(placement: &DevicePlacement, width: i32) -> Option<Vec<Vec<Span>>> {
    let chains = placement.chains.as_deref().unwrap_or_default();
    if chains.is_empty() {
        return placement.devices.as_ref().filter(|v| !v.is_empty()).map(|v| vec![device_row(v, placement.index, width)]);
    }
    let focus = placement.chain.unwrap_or(0.).clamp(0., (chains.len() - 1) as f64);
    assert!(focus.fract() == 0., "a chain picture needs an existing chain");
    let focus = focus as usize;
    let shown = if chains.len() == 1 {
        vec![focus]
    } else if focus < chains.len() - 1 {
        vec![focus, focus + 1]
    } else {
        vec![focus - 1, focus]
    };
    let more = chains.len() - shown.len();
    let name_width = shown.iter().map(|i| text_width(&chains[*i].name)).max().unwrap_or(0).min(10);
    Some(
        shown
            .iter()
            .enumerate()
            .map(|(line, index)| {
                let glyph = if shown.len() == 1 {
                    "╶ "
                } else if line == 0 {
                    "╭ "
                } else {
                    "╰ "
                };
                let name = truncate(&chains[*index].name, name_width);
                let tail = if line == shown.len() - 1 && more > 0 { format!("  +{more}") } else { String::new() };
                let room = (width - 2 - name_width - 2 - tail.len() as i32).max(8);
                let mut row = vec![
                    span(glyph, palette::FAINT),
                    span(
                        format!("{name}{}  ", " ".repeat((name_width - text_width(&name)).max(0) as usize)),
                        if *index == focus {
                            if placement.index.is_none() {
                                palette::ACCENT
                            } else {
                                palette::TEXT
                            }
                        } else {
                            palette::FAINT
                        },
                    ),
                ];
                row.extend(device_row(&chains[*index].devices, (*index == focus).then_some(placement.index).flatten(), room));
                if !tail.is_empty() {
                    row.push(span(tail, palette::FAINT));
                }
                row
            })
            .collect(),
    )
}
/// A controller lane across a take as one row of blocks, low to high: each cell holds the value the lane has there
/// (a lane holds its last value until the next point); blank before its first point.
pub fn controller_line(lane: &ControllerLane, length: f64, cells: usize) -> String {
    const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let mut points = lane.points.clone();
    points.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let centred = lane.min < 0.;
    (0..cells)
        .map(|cell| {
            let at = (cell as f64 + 1.) / cells as f64 * length;
            match points.iter().take_while(|point| point[0] < at).last() {
                // A lane centred on 0 (pitch bend): the centre a line, bars above it up from there and below it down.
                Some([_, value]) if centred => match value / lane.max.max(1.) {
                    level if level.abs() < 0.01 => '─',
                    level if level > 0. => LEVELS[4 + (level.min(1.) * 3.).round() as usize],
                    level => LEVELS[3 - ((-level).min(1.) * 3.).round() as usize],
                },
                Some([_, value]) => LEVELS[(((value - lane.min) / (lane.max - lane.min).max(1.)).clamp(0., 1.) * 7.).round() as usize],
                None => ' ',
            }
        })
        .collect()
}
pub fn clip_picture(length: f64, notes: &[ClipViewNote], width: i32, rows: usize) -> Option<Vec<Vec<Span>>> {
    if length <= 0. || notes.is_empty() {
        return None;
    }
    let cells = width.clamp(8, 32) as usize;
    let (columns, lanes) = (cells * 2, rows * 4);
    let mut pitches: Vec<f64> = notes.iter().map(|n| n.note.pitch).collect();
    pitches.sort_by(|a, b| b.total_cmp(a));
    pitches.dedup();
    let (high, low) = (pitches[0], *pitches.last().unwrap());
    let top = ((lanes as f64 - 1. - (high - low)) / 2.).floor();
    let lane = |pitch: f64| {
        if high - low < lanes as f64 {
            top + high - pitch
        } else if pitches.len() <= lanes {
            number::round(pitches.iter().position(|p| *p == pitch).unwrap() as f64 * (lanes - 1) as f64 / (pitches.len() - 1) as f64)
        } else {
            number::round((high - pitch) * (lanes - 1) as f64 / (high - low))
        }
    };
    let marking = notes.iter().any(|n| n.selected == Some(true));
    let mut dots = vec![vec![0f64; columns]; lanes];
    for note in notes {
        let n = &note.note;
        let first = (n.start / length * columns as f64).floor().clamp(0., (columns - 1) as f64) as usize;
        let last = (((n.start + n.duration) / length * columns as f64).ceil() - 1.).min((columns - 1) as f64).max(first as f64) as usize;
        let row = &mut dots[lane(n.pitch) as usize];
        let weight = if marking {
            if note.selected == Some(true) {
                127.
            } else {
                1.
            }
        } else {
            n.velocity
        };
        for value in &mut row[first..=last] {
            *value = value.max(weight);
        }
    }
    const BRAILLE: [[u32; 2]; 4] = [[1, 8], [2, 16], [4, 32], [64, 128]];
    Some(
        (0..rows)
            .map(|text_row| {
                let mut spans: Vec<Span> = vec![];
                for cell in 0..cells {
                    let (mut bits, mut loudest) = (0, 0f64);
                    for dy in 0..4 {
                        for dx in 0..2 {
                            let velocity = dots[text_row * 4 + dy][cell * 2 + dx];
                            if velocity != 0. {
                                bits |= BRAILLE[dy][dx];
                                loudest = loudest.max(velocity);
                            }
                        }
                    }
                    let style = Style::fg(if loudest >= 64. { palette::ACCENT } else { palette::DIM });
                    let text = char::from_u32(0x2800 + bits).unwrap();
                    if let Some(last) = spans.last_mut().filter(|s| *s.style == style) {
                        last.text.push(text);
                    } else {
                        spans.push(Span::styled(text.to_string(), style));
                    }
                }
                spans
            })
            .collect(),
    )
}
/// A wait in words: "2h 13m", "13m", "45s".
pub fn span_words(ms: f64) -> String {
    let seconds = (ms / 1000.).ceil().max(0.) as u64;
    let (hours, minutes) = (seconds / 3600, (seconds % 3600) / 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}
