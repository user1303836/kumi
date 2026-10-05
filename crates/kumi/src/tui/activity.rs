//! What Kumi is doing, as motion: each kind of task has its own small animation, so a step at work
//! reads as searching, reading, building or listening at a glance. A one-cell glyph for a step's row,
//! a wider scene for NOW, and a shimmer that passes over the words of whatever is under way.

use std::rc::Rc;

use kumi_common::js;

use super::style::{palette, Rgb, Style};
use super::wrap::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Activity {
    Think,
    Search,
    Read,
    Look,
    Build,
    Change,
    Listen,
    Watch,
    Play,
    Record,
    Code,
}

impl Activity {
    pub const ALL: [Activity; 11] = [
        Activity::Think,
        Activity::Search,
        Activity::Read,
        Activity::Look,
        Activity::Build,
        Activity::Change,
        Activity::Listen,
        Activity::Watch,
        Activity::Play,
        Activity::Record,
        Activity::Code,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Activity::Think => "think",
            Activity::Search => "search",
            Activity::Read => "read",
            Activity::Look => "look",
            Activity::Build => "build",
            Activity::Change => "change",
            Activity::Listen => "listen",
            Activity::Watch => "watch",
            Activity::Play => "play",
            Activity::Record => "record",
            Activity::Code => "code",
        }
    }
}

fn kind_of(tool: &str) -> Option<Activity> {
    Some(match tool {
        "search_web"
        | "search_conversations"
        | "live_browser_search"
        | "find_sounds"
        | "find_presets"
        | "my_sets"
        | "live_browser_roots"
        | "live_browser_inspect" => Activity::Search,
        "read_web" | "live_manual" => Activity::Read,
        "server_status"
        | "live_status"
        | "live_discover"
        | "live_snapshot"
        | "live_note_read"
        | "live_song_state"
        | "live_performance_read"
        | "live_key_estimate"
        | "live_take_lane_read"
        | "live_warp_marker_read"
        | "live_arrangement_automation_read"
        | "watch_me"
        | "select"
        | "show" => Activity::Look,
        "make_device" | "arrange" => Activity::Build,
        "listen" | "audition" => Activity::Listen,
        "watch_video" => Activity::Watch,
        "play" | "fire_scene" | "launch_clip" | "jump_to_locator" => Activity::Play,
        "record" | "capture_midi" => Activity::Record,
        "run_python" => Activity::Code,
        _ => return None,
    })
}

/// The kind of task a tool is; the rest change the Set.
pub fn activity_of(tool: Option<&str>) -> Activity {
    let Some(tool) = tool.filter(|tool| !tool.is_empty()) else { return Activity::Think };
    kind_of(tool).unwrap_or(if tool.starts_with("live_") { Activity::Look } else { Activity::Change })
}

/// Each kind's one-cell frames, shown about ten a second.
fn glyphs(kind: Activity) -> &'static [&'static str] {
    match kind {
        // Dots turning: thinking it over.
        Activity::Think => &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
        // A dot circling, like a radar's sweep.
        Activity::Search => &["⠁", "⠈", "⠐", "⠠", "⢀", "⡀", "⠄", "⠂"],
        // Eyes going down a page, line by line.
        Activity::Read => &["⠉", "⠒", "⠤", "⣀", "⣀", "⠀"],
        // A gap going round: looking all over the Set.
        Activity::Look => &["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"],
        // Filling up from the bottom: something being built.
        Activity::Build => &["⡀", "⣀", "⣄", "⣤", "⣦", "⣶", "⣷", "⣿", "⣿", "⠀"],
        // A fader going up and down.
        Activity::Change => &["⣀", "⠤", "⠒", "⠉", "⠒", "⠤"],
        // A level meter.
        Activity::Listen => &["▁", "▄", "▂", "▆", "▃", "▇", "▅", "▂"],
        // A reel turning.
        Activity::Watch => &["◐", "◓", "◑", "◒"],
        Activity::Play => &["▶", "▶", "▷", "▷"],
        Activity::Record => &["●", "●", "○", "○"],
        // A block going round: code running.
        Activity::Code => &["▖", "▘", "▝", "▗"],
    }
}

/// The same, in characters every console font has (the old Windows console, where Kumi's icons are badges too).
fn plain_glyphs(kind: Activity) -> &'static [&'static str] {
    match kind {
        Activity::Think => &["|", "/", "-", "\\"],
        Activity::Search => &[".", "o", "O", "o"],
        Activity::Read => &["-", "=", "-", " "],
        Activity::Look => &["<", "^", ">", "v"],
        Activity::Build => &[".", ":", "|", "#", " "],
        Activity::Change => &["-", "=", "+", "="],
        Activity::Listen => &[".", ":", "|", ":"],
        Activity::Watch => &["o", "O"],
        Activity::Play => &[">", " "],
        Activity::Record => &["*", " "],
        Activity::Code => &["_", " "],
    }
}

const FRAME_MS: f64 = 100.0;

/// `Math.floor(value) % count` for a non-negative value, as JavaScript does it.
fn frame(value: f64, count: usize) -> usize {
    ((value.floor() as i64).rem_euclid(count as i64)) as usize
}

/// The glyph for `kind` at `ms` into it, and its style (recording's in red); `plain` for consoles without the glyphs.
pub fn activity_glyph(kind: Activity, ms: f64, plain: bool) -> Span {
    let frames = if plain { plain_glyphs(kind) } else { glyphs(kind) };
    let text = frames[frame(ms.max(0.0) / FRAME_MS, frames.len())];
    Span::styled(text, Style::fg(if kind == Activity::Record { palette::ERROR } else { palette::ACCENT }))
}

/// `from` toward `to` by `amount` (0–1), in six steps so the terminal's style table stays small.
pub fn blend(from: Rgb, to: Rgb, amount: f64) -> Rgb {
    blend_steps(from, to, amount, 6)
}

/// `blend` in `steps` steps.
pub fn blend_steps(from: Rgb, to: Rgb, amount: f64, steps: u32) -> Rgb {
    let step = js::number::round(amount.clamp(0.0, 1.0) * steps as f64) / steps as f64;
    let channel = |index: usize| js::number::round(from[index] as f64 + (to[index] as f64 - from[index] as f64) * step) as u8;
    [channel(0), channel(1), channel(2)]
}

/// Spans of one character each into as few spans as their styles allow.
fn joined(cells: Vec<Span>) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for cell in cells {
        match spans.last_mut() {
            Some(last) if same(&last.style, &cell.style) => last.text.push_str(&cell.text),
            _ => spans.push(Span::styled(cell.text, *cell.style)),
        }
    }
    spans
}

fn same(a: &Style, b: &Style) -> bool {
    a.fg == b.fg && a.bg == b.bg && a.bold == b.bold
}

/// `text` with a band of light passing over it, left to right, every couple of seconds: the words
/// of what's under way, so they never sit still while Kumi works.
pub fn shimmer(text: &str, ms: f64) -> Vec<Span> {
    shimmer_with(text, ms, palette::DIM, palette::BRIGHT)
}

/// `shimmer` between colours of one's own.
pub fn shimmer_with(text: &str, ms: f64, base: Rgb, peak: Rgb) -> Vec<Span> {
    let characters: Vec<char> = text.chars().collect();
    let span = characters.len() as f64 + 8.0;
    let center = ((ms / 1800.0) % 1.0) * span - 4.0;
    joined(
        characters
            .iter()
            .enumerate()
            .map(|(index, character)| {
                let distance = (index as f64 - center).abs();
                Span::styled(character.to_string(), Style::fg(blend(base, peak, (1.0 - distance / 3.5).max(0.0))))
            })
            .collect(),
    )
}

/// A smooth value 0–1 for each cell and moment: a level meter's bars, say.
fn wave(cell: f64, ms: f64) -> f64 {
    0.5 + 0.22 * (ms / 170.0 + cell * 1.7).sin() + 0.18 * (ms / 97.0 + cell * 0.9 + 1.0).sin() + 0.1 * (ms / 53.0 + cell * 2.3).sin()
}

const BARS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// NOW's picture of what's under way, `width` cells wide: a scanner looking back and forth for a
/// search, a highlight going word by word for a page, tracks being looked over, blocks stacking up
/// for a device, a knob finding its place for a change, a level meter for listening, a playhead for
/// a video or Live playing, a line being typed for code, and a slow wave while thinking.
pub fn activity_scene(kind: Activity, ms: f64, width: i32) -> Vec<Span> {
    let cells = width.max(6) as i64;
    let t = ms.max(0.0);
    let faint = Rc::new(Style::fg(palette::RULE));
    let dim = Rc::new(Style::fg(palette::FAINT));
    let lit = Rc::new(Style::fg(palette::ACCENT));
    let mut out: Vec<Span> = Vec::new();
    let floor = |value: f64| value.floor() as i64;
    match kind {
        Activity::Search => {
            // A scanner: a bright head going back and forth, its tail fading behind it.
            let period = 1600.0;
            let phase = (t % period) / period;
            let forward = phase < 0.5;
            let head = js::number::round(if forward { phase * 2.0 } else { 2.0 - phase * 2.0 } * (cells - 1) as f64) as i64;
            for cell in 0..cells {
                let behind = if forward { head - cell } else { cell - head };
                if cell == head {
                    out.push(Span::new("●", &lit));
                } else if behind > 0 && behind <= 3 {
                    out.push(Span::new(["•", "∙", "·"][(behind - 1) as usize], if behind == 1 { &lit } else { &dim }));
                } else {
                    out.push(Span::new("·", &faint));
                }
            }
        }
        Activity::Read => {
            // Words of a line, read one after another; then the next line.
            let line = floor(t / 2400.0);
            let mut words: Vec<i64> = Vec::new();
            let (mut used, mut index) = (0i64, 0i64);
            while used < cells {
                let length = 2 + (line * 7 + index * 5).rem_euclid(4);
                words.push(length.min(cells - used));
                used += length + 1;
                index += 1;
            }
            let reading = floor((t % 2400.0) / (2400.0 / words.len() as f64));
            let mut used = 0;
            for (index, length) in words.iter().enumerate() {
                if used >= cells {
                    continue;
                }
                let index = index as i64;
                out.push(Span::new(
                    "▬".repeat(*length as usize),
                    if index == reading {
                        &lit
                    } else if index < reading {
                        &dim
                    } else {
                        &faint
                    },
                ));
                used += length;
                if used < cells {
                    out.push(Span::new(" ", &faint));
                    used += 1;
                }
            }
        }
        Activity::Look => {
            // The Set's tracks side by side, looked over one by one.
            let tracks = ((cells + 1) / 2).max(3);
            let at = floor(t / 140.0) % (tracks + 3);
            for track in 0..tracks {
                let distance = at - track;
                out.push(Span::new(
                    "▌",
                    if distance == 0 {
                        &lit
                    } else if distance == 1 {
                        &dim
                    } else {
                        &faint
                    },
                ));
                if track < tracks - 1 {
                    out.push(Span::new(" ", &faint));
                }
            }
        }
        Activity::Build => {
            // Blocks stacking up, one after another, until it's built; then again.
            let per_cell = 120.0;
            let total = cells as f64 * per_cell + 700.0;
            let at = t % total;
            for cell in 0..cells {
                let level = floor((at - cell as f64 * per_cell) / (per_cell / 8.0));
                if level <= 0 {
                    out.push(Span::new("▁", &faint));
                } else {
                    let style = if at > cells as f64 * per_cell {
                        &lit
                    } else if level >= 7 {
                        &dim
                    } else {
                        &lit
                    };
                    out.push(Span::new(BARS[level.min(7) as usize], style));
                }
            }
        }
        Activity::Change => {
            // A knob gliding to a new place, filled up to it, again and again.
            let hop = 900.0;
            let from = settle(floor(t / hop) - 1, cells);
            let to = settle(floor(t / hop), cells);
            let ease = ((t % hop) / 450.0).min(1.0);
            let smooth = ease * ease * (3.0 - 2.0 * ease);
            let knob = js::number::round(from as f64 + (to - from) as f64 * smooth) as i64;
            for cell in 0..cells {
                let (text, style) = if cell == knob {
                    ("●", &lit)
                } else if cell < knob {
                    ("━", &dim)
                } else {
                    ("─", &faint)
                };
                out.push(Span::new(text, style));
            }
        }
        Activity::Listen => {
            for cell in 0..cells {
                let level = (js::number::round(wave(cell as f64, t) * 7.0) as i64).clamp(0, 7);
                out.push(Span::new(BARS[level as usize], if level >= 5 { &lit } else { &dim }));
            }
        }
        Activity::Watch | Activity::Play | Activity::Record => {
            // A playhead moving along, ticks on the beats behind it.
            let lead = if kind == Activity::Record { "● " } else { "▶ " };
            if kind == Activity::Record {
                let fg = if floor(t / 500.0) % 2 != 0 { palette::ERROR } else { blend(palette::ERROR, palette::SURFACE, 0.5) };
                out.push(Span::styled(lead, Style::fg(fg)));
            } else {
                out.push(Span::new(lead, &lit));
            }
            let length = (cells - 2).max(2);
            let head = floor(t / 180.0) % length;
            for cell in 0..length {
                let text = if cell == head {
                    "┃"
                } else if cell < head {
                    if kind == Activity::Watch {
                        "━"
                    } else if cell % 4 == 0 {
                        "┼"
                    } else {
                        "─"
                    }
                } else {
                    "─"
                };
                out.push(Span::new(
                    text,
                    if cell == head {
                        &lit
                    } else if cell < head {
                        &dim
                    } else {
                        &faint
                    },
                ));
            }
        }
        Activity::Code => {
            // A line being typed, a block cursor blinking at its end; then the next line.
            let written = floor(t / 90.0) % (cells + 6);
            let end = written.min(cells - 1);
            for cell in 0..cells {
                if cell < end {
                    out.push(Span::new(if (cell * 7) % 5 == 3 { " " } else { "▪" }, &dim));
                } else if cell == end {
                    out.push(Span::new(if floor(t / 400.0) % 2 != 0 { " " } else { "▌" }, &lit));
                } else {
                    out.push(Span::new(" ", &faint));
                }
            }
        }
        Activity::Think => {
            // A slow wave of dots: thinking.
            for cell in 0..cells {
                let level = 0.5 + 0.5 * (t / 260.0 - cell as f64 * 0.55).sin();
                let text = if level > 0.85 {
                    "●"
                } else if level > 0.6 {
                    "•"
                } else if level > 0.35 {
                    "∙"
                } else {
                    "·"
                };
                out.push(Span::new(
                    text,
                    if level > 0.85 {
                        &lit
                    } else if level > 0.35 {
                        &dim
                    } else {
                        &faint
                    },
                ));
            }
        }
    }
    // Every scene is exactly as wide as asked, so what's beside it stays put.
    let used: i64 = out.iter().map(|span| span.text.chars().count() as i64).sum();
    if used < cells {
        out.push(Span::new(" ".repeat((cells - used) as usize), &faint));
    }
    joined(out)
}

/// Where the change scene's knob settles on its `n`th hop: somewhere new each time, the same each run.
fn settle(n: i64, cells: i64) -> i64 {
    let seed = ((n + 3) as f64 * 12.9898).sin() * 43758.5453;
    ((seed - seed.floor()) * (cells - 2) as f64).floor() as i64 + 1
}
