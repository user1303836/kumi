//! A picture for every change: what a `ChangePicture` (DAW-neutral data) looks like on this terminal. An image where
//! the terminal shows them (Kitty's graphics, through placeholder cells), braille and half-blocks in cells
//! elsewhere, words on a basic console; and under it, each value that moved, with what was asked when the DAW set
//! something else. A tracker's lines and a route are words everywhere: the DAW's own text says them best.

pub mod cells;
pub mod kitty;
pub mod raster;
pub mod scene;

use kumi_types::contracts::ChangePicture;
use kumi_types::picture::{MixLevels, PictureLine, PictureRoute, PictureValue, RouteChange};

use super::alerts::TerminalProfile;
use super::style::{palette, ColorDepth, Rgb, Style};
use super::width::{text_width, truncate};
use super::wrap::Span;
pub use raster::Raster;
pub use scene::{scene_of, Scene};

/// How this terminal shows pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Graphics {
    /// Images through Kitty's Unicode placeholders (kitty, Ghostty), at 24-bit colour only: the image's id travels in
    /// the placeholders' colour.
    Kitty,
    /// Braille and half-blocks in colour.
    Cells,
    /// Words only: a console whose symbols may not show, or no colour at all.
    Words,
}

/// How pictures show here: images where the terminal's profile says it shows Kitty's placeholders (at 24-bit colour
/// only: an image's id travels in its placeholders' colour), cells elsewhere or when the producer keeps them there
/// (`KUMI_PICTURES=cells`), words where symbols may not show or there's no colour.
pub fn graphics_for(profile: &TerminalProfile, depth: ColorDepth, glyphs: bool, cells_only: bool) -> Graphics {
    if !glyphs || depth == ColorDepth::None {
        Graphics::Words
    } else if profile.images && depth == ColorDepth::Truecolor && !cells_only {
        Graphics::Kitty
    } else {
        Graphics::Cells
    }
}

/// A picture laid out for the screen.
#[derive(Clone, Debug, Default)]
pub struct Drawn {
    /// The picture's rows: placeholders, braille or half-blocks (none for values alone, or in words).
    pub rows: Vec<Vec<Span>>,
    /// Each value that moved, under it.
    pub lines: Vec<Vec<Span>>,
    /// For terminal graphics: the image the rows' placeholders show, uploaded first under its id.
    pub image: Option<Raster>,
}

/// The picture at `cols` × `rows` cells. `id` names an image (Kitty), `ground` is the colour under the picture.
pub fn draw(picture: &ChangePicture, cols: usize, rows: usize, graphics: Graphics, id: u32, ground: Rgb) -> Drawn {
    let cols = cols.max(4);
    let lines = lines_of(picture, cols);
    if graphics == Graphics::Words {
        return Drawn { rows: Vec::new(), lines, image: None };
    }
    let Some(scene) = scene_of(picture) else {
        // Values and levels are bars, the same everywhere.
        return Drawn { rows: Vec::new(), lines: bars_of(picture, cols).unwrap_or(lines), image: None };
    };
    if rows == 0 {
        return Drawn { rows: Vec::new(), lines, image: None };
    }
    // Drawn in the theme's colours: an image's pixels and the half-blocks' edges are mixed from them.
    let theme = super::style::theme();
    let (scene, ground) = (scene.in_theme(theme), theme.of(ground));
    match graphics {
        Graphics::Kitty => {
            let image = Raster::draw(&scene, (cols * 8).min(1600) as u32, (rows * 16).min(800) as u32);
            Drawn { rows: kitty::cells(id, cols, rows), lines, image: Some(image) }
        }
        _ if scene.lines => Drawn { rows: cells::braille(&scene, cols, rows), lines, image: None },
        _ => Drawn { rows: cells::half_blocks(&scene, cols, rows, ground), lines, image: None },
    }
}

fn styled(text: impl Into<String>, color: Rgb) -> Span {
    Span::styled(text, Style::fg(color))
}

/// "−3.0 dB → −1.5 dB", or the numbers when the DAW gave no words.
fn values_text(value: &PictureValue) -> String {
    let words = |text: &Option<String>, number: Option<f64>| {
        text.clone().or_else(|| number.map(|n| kumi_common::js::number::to_string((n * 1000.).round() / 1000.)))
    };
    match (words(&value.from_text, value.from), words(&value.to_text, value.to)) {
        (Some(from), Some(to)) if from != to => format!("{from} → {to}"),
        (_, Some(to)) => to,
        (from, None) => from.unwrap_or_default(),
    }
}

/// Each value in words: its name, before → after, and what was asked when it isn't what was set (on a line of its
/// own when they don't fit on one).
fn value_lines(value: &PictureValue, cols: usize) -> Vec<Vec<Span>> {
    let line = vec![styled(format!("{} ", value.name), palette::DIM), styled(values_text(value), palette::TEXT)];
    let Some(asked) = &value.asked else { return vec![fit(line, cols)] };
    let asked_words = format!("asked {asked}");
    if line.iter().map(|span| text_width(&span.text) as usize).sum::<usize>() + 3 + text_width(&asked_words) as usize <= cols {
        return vec![fit([line, vec![styled(format!(" · {asked_words}"), palette::WARN)]].concat(), cols)];
    }
    vec![fit(line, cols), fit(vec![styled(format!("  {asked_words}"), palette::WARN)], cols)]
}

/// A tracker's line before and after, in the DAW's text: " 09 E-4 01 40 .. .. → OFF .. .. .. ..", or on two rows
/// where that doesn't fit (the line before, then what it is now under it).
fn tracker_lines(line: &PictureLine, cols: usize) -> Vec<Vec<Span>> {
    // (as the tracker shows it: two digits at least, 00 the first line)
    let number = format!("{:>3} ", format!("{:02}", line.line));
    let side = |text: &str| if text.is_empty() { "(empty)".to_string() } else { text.to_string() };
    let (before, after) = (side(&line.before), side(&line.after));
    let one = text_width(&number) + text_width(&before) + 3 + text_width(&after);
    if one as usize <= cols {
        return vec![vec![
            styled(number, palette::FAINT),
            styled(before, palette::DIM),
            styled(" → ", palette::FAINT),
            styled(after, palette::TEXT),
        ]];
    }
    vec![
        fit(vec![styled(number, palette::FAINT), styled(before, palette::DIM)], cols),
        fit(vec![styled("  → ", palette::FAINT), styled(after, palette::TEXT)], cols),
    ]
}

/// A route's ends, and whether Kumi made it or took it away: "Bass → Reverb · new".
fn route_line(route: &PictureRoute, cols: usize) -> Vec<Span> {
    let mut line = vec![styled(route.from.clone(), palette::TEXT), styled(" → ", palette::FAINT), styled(route.to.clone(), palette::TEXT)];
    match route.change {
        RouteChange::Made => line.push(styled(" · new", palette::ACCENT)),
        RouteChange::Removed => line.push(styled(" · taken away", palette::FAINT)),
        RouteChange::Changed => {}
    }
    fit(line, cols)
}

/// The words of a picture: each value that moved, or what changed in notes and lanes.
pub fn lines_of(picture: &ChangePicture, cols: usize) -> Vec<Vec<Span>> {
    match picture {
        ChangePicture::Values { values }
        | ChangePicture::Eq { values, .. }
        | ChangePicture::Dynamics { values, .. }
        | ChangePicture::Zones { values, .. }
        | ChangePicture::Items { values, .. } => values.iter().flat_map(|value| value_lines(value, cols)).collect(),
        ChangePicture::Lines { lines, more } => {
            let mut rows: Vec<Vec<Span>> = lines.iter().flat_map(|line| tracker_lines(line, cols)).collect();
            if *more > 0 {
                rows.push(fit(
                    vec![styled(format!("    and {more} more {}", if *more == 1 { "line" } else { "lines" }), palette::FAINT)],
                    cols,
                ));
            }
            rows
        }
        ChangePicture::Routes { routes } => routes
            .iter()
            .flat_map(|route| {
                std::iter::once(route_line(route, cols)).chain(
                    route
                        .values
                        .iter()
                        .flat_map(|value| value_lines(value, cols.saturating_sub(2)))
                        .map(|line| [vec![styled("  ", palette::TEXT)], line].concat()),
                )
            })
            .collect(),
        ChangePicture::Notes { before, after, .. } => {
            let same = |a: &kumi_types::contracts::ClipNote, b: &kumi_types::contracts::ClipNote| {
                a.pitch == b.pitch && (a.start - b.start).abs() < 1e-6 && (a.duration - b.duration).abs() < 1e-6
            };
            let new = after.iter().filter(|note| !before.iter().any(|old| same(old, note))).count();
            let gone = before.iter().filter(|note| !after.iter().any(|now| same(now, note))).count();
            let plural = |n: usize| format!("{n} note{}", if n == 1 { "" } else { "s" });
            let mut line = vec![styled(
                if before.is_empty() { plural(after.len()) } else { format!("{} → {}", plural(before.len()), plural(after.len())) },
                palette::TEXT,
            )];
            if new > 0 && !before.is_empty() {
                line.push(styled(format!(" · {new} new"), palette::ACCENT));
            }
            if gone > 0 {
                line.push(styled(format!(" · {gone} gone"), palette::FAINT));
            }
            vec![fit(line, cols)]
        }
        ChangePicture::Automation { parameter, before, after, words, .. } => {
            let plural = |n: usize| format!("{n} point{}", if n == 1 { "" } else { "s" });
            let what = if parameter.is_empty() { String::new() } else { format!("{parameter} · ") };
            let mut lines = vec![fit(vec![styled(format!("{what}{} → {}", before.len(), plural(after.len())), palette::TEXT)], cols)];
            // What it moves between and where, where the DAW said ("−18 to 0 dB over bars 5–9").
            if let Some(words) = words {
                lines.push(fit(vec![styled(words.clone(), palette::DIM)], cols));
            }
            lines
        }
        ChangePicture::Mix { before, after } => {
            mix_words(before, after).into_iter().map(|line| fit(vec![styled(line, palette::TEXT)], cols)).collect()
        }
    }
}

fn mix_words(before: &MixLevels, after: &MixLevels) -> Vec<String> {
    let pair = |a: &Option<String>, b: &Option<String>| match (a, b) {
        (Some(a), Some(b)) if a != b => Some(format!("{a} → {b}")),
        _ => None,
    };
    let mut lines = Vec::new();
    if let Some(volume) = pair(&before.volume_text, &after.volume_text) {
        lines.push(format!("Volume {volume}"));
    }
    if let Some(pan) = pair(&before.pan_text, &after.pan_text) {
        lines.push(format!("Pan {pan}"));
    }
    for (index, send) in after.sends.iter().enumerate() {
        if let Some(text) = pair(&before.sends.get(index).and_then(|s| s.text.clone()), &send.text) {
            lines.push(format!("Send {} {text}", send.name));
        }
    }
    for (what, was, now) in [("muted", before.mute, after.mute), ("soloed", before.solo, after.solo)] {
        if let (Some(was), Some(now)) = (was, now) {
            if was != now {
                lines.push(if now { what.to_string() } else { format!("un{what}") });
            }
        }
    }
    lines
}

/// Values and levels as bars: each a track with its new place filled and its old one marked, then its words.
fn bars_of(picture: &ChangePicture, cols: usize) -> Option<Vec<Vec<Span>>> {
    let width = (cols / 4).clamp(4, 14);
    let bar = |from: Option<f64>, to: f64, min: f64, max: f64| -> Vec<Span> {
        let place = |v: f64| ((v - min) / (max - min)).clamp(0., 1.);
        let to = (place(to) * width as f64).round() as usize;
        let from = from.map(|f| ((place(f) * width as f64).round() as usize).min(width.saturating_sub(1)));
        (0..width)
            .map(|cell| match (cell < to, Some(cell) == from && from != Some(to.saturating_sub(1))) {
                (true, true) => styled("█", palette::DIM),
                (true, false) => styled("█", palette::ACCENT),
                (false, true) => styled("▏", palette::DIM),
                (false, false) => styled("─", palette::RULE),
            })
            .collect()
    };
    let value_bars = |values: &[PictureValue], cols: usize| -> Vec<Vec<Span>> {
        values
            .iter()
            .flat_map(|value| {
                let mut line = match (value.to, value.min, value.max) {
                    (Some(to), Some(min), Some(max)) if max > min => bar(value.from, to, min, max),
                    _ => Vec::new(),
                };
                let indent = if line.is_empty() { 0 } else { width + 1 };
                if !line.is_empty() {
                    line.push(styled(" ", palette::TEXT));
                }
                let mut words = value_lines(value, cols.saturating_sub(indent)).into_iter();
                line.extend(words.next().unwrap_or_default());
                // What was asked, when it took a line of its own, sits under the words.
                std::iter::once(line).chain(words.map(move |more| [vec![styled(" ".repeat(indent), palette::TEXT)], more].concat()))
            })
            .collect()
    };
    match picture {
        ChangePicture::Values { values } => Some(value_bars(values, cols)),
        // Each route's ends, then its values as bars under them.
        ChangePicture::Routes { routes } => Some(
            routes
                .iter()
                .flat_map(|route| {
                    std::iter::once(route_line(route, cols)).chain(
                        value_bars(&route.values, cols.saturating_sub(2))
                            .into_iter()
                            .map(|line| [vec![styled("  ", palette::TEXT)], line].concat()),
                    )
                })
                .collect(),
        ),
        ChangePicture::Mix { before, after } => {
            let mut lines = Vec::new();
            if let Some(to) = after.volume {
                let mut line = bar(before.volume, to, 0., 1.);
                line.push(styled(format!(" Volume {}", after.volume_text.clone().unwrap_or_default()), palette::TEXT));
                lines.push(fit(line, cols));
            }
            if let Some(to) = after.pan {
                let mut line = bar(before.pan, to, -1., 1.);
                line.push(styled(format!(" Pan {}", after.pan_text.clone().unwrap_or_default()), palette::TEXT));
                lines.push(fit(line, cols));
            }
            for (index, send) in after.sends.iter().enumerate() {
                let mut line = bar(before.sends.get(index).map(|s| s.level), send.level, 0., 1.);
                line.push(styled(format!(" Send {} {}", send.name, send.text.clone().unwrap_or_default()), palette::TEXT));
                lines.push(fit(line, cols));
            }
            lines.extend(
                mix_words(before, after)
                    .into_iter()
                    .filter(|w| !w.starts_with("Volume") && !w.starts_with("Pan") && !w.starts_with("Send"))
                    .map(|w| vec![styled(w, palette::TEXT)]),
            );
            Some(lines)
        }
        _ => None,
    }
}

/// A line cut to `cols` cells, its last span ending in an ellipsis when it had to be.
fn fit(spans: Vec<Span>, cols: usize) -> Vec<Span> {
    let mut used = 0;
    let mut out = Vec::new();
    for span in spans {
        let width = text_width(&span.text) as usize;
        if used + width <= cols {
            used += width;
            out.push(span);
            continue;
        }
        let room = cols.saturating_sub(used);
        if room > 0 {
            out.push(Span { text: truncate(&span.text, room as i32), style: span.style.clone() });
        }
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_types::picture::{EqBand, EqShape};

    fn text(rows: &[Vec<Span>]) -> Vec<String> {
        rows.iter().map(|row| row.iter().map(|span| span.text.as_str()).collect()).collect()
    }

    #[test]
    fn images_need_a_terminal_that_shows_them_24_bit_colour_and_symbols() {
        let shows = TerminalProfile { images: true, ..TerminalProfile::quiet() };
        let truecolor = ColorDepth::Truecolor;
        assert_eq!(graphics_for(&shows, truecolor, true, false), Graphics::Kitty);
        assert_eq!(graphics_for(&shows, ColorDepth::Colors256, true, false), Graphics::Cells, "the id needs 24-bit colour");
        assert_eq!(graphics_for(&shows, truecolor, true, true), Graphics::Cells, "KUMI_PICTURES=cells");
        assert_eq!(graphics_for(&TerminalProfile::quiet(), truecolor, true, false), Graphics::Cells);
        assert_eq!(graphics_for(&shows, truecolor, false, false), Graphics::Words, "a console without the symbols");
        assert_eq!(
            graphics_for(&TerminalProfile::quiet(), ColorDepth::Colors16, true, false),
            Graphics::Cells,
            "braille shows in 16 colours"
        );
        assert_eq!(graphics_for(&TerminalProfile::quiet(), ColorDepth::None, true, false), Graphics::Words);
    }

    #[test]
    fn an_eq_draws_as_an_image_braille_or_words() {
        let band = |gain_db: f64| EqBand { shape: EqShape::Bell, freq: 1000., gain_db, q: 1., on: true, slope: None };
        let moved = PictureValue {
            name: "3 Gain A".into(),
            from: Some(0.),
            to: Some(-4.),
            from_text: Some("0.00 dB".into()),
            to_text: Some("-4.00 dB".into()),
            asked: Some("-4.5 dB".into()),
            ..Default::default()
        };
        let picture = ChangePicture::Eq { before: vec![band(0.)], after: vec![band(-4.)], values: vec![moved] };
        let drawn = draw(&picture, 60, 6, Graphics::Kitty, 9, palette::GROUND);
        assert_eq!((drawn.rows.len(), drawn.rows[0].len()), (6, 60));
        let image = drawn.image.unwrap();
        assert_eq!((image.width, image.height), (480, 96));
        assert_eq!(text(&drawn.lines), ["3 Gain A 0.00 dB → -4.00 dB · asked -4.5 dB"]);
        let cells = draw(&picture, 60, 6, Graphics::Cells, 9, palette::GROUND);
        assert!(cells.image.is_none());
        assert!(text(&cells.rows).iter().any(|row| row.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c))), "braille");
        let words = draw(&picture, 60, 6, Graphics::Words, 9, palette::GROUND);
        assert!(words.rows.is_empty());
        assert_eq!(text(&words.lines), text(&cells.lines));
    }

    #[test]
    fn values_and_levels_are_bars_with_their_words() {
        let value = PictureValue {
            name: "Tempo".into(),
            from: Some(120.),
            to: Some(124.),
            min: Some(20.),
            max: Some(999.),
            from_text: Some("120 BPM".into()),
            to_text: Some("124 BPM".into()),
            asked: None,
        };
        let drawn = draw(&ChangePicture::Values { values: vec![value] }, 40, 6, Graphics::Kitty, 1, palette::GROUND);
        assert!(drawn.rows.is_empty() && drawn.image.is_none());
        let [line] = text(&drawn.lines).try_into().unwrap();
        assert!(line.ends_with(" Tempo 120 BPM → 124 BPM"), "{line}");
        assert_eq!(text_width(&line), 10 + 1 + text_width("Tempo 120 BPM → 124 BPM"));
        let mix = ChangePicture::Mix {
            before: MixLevels { volume: Some(0.85), volume_text: Some("0.0 dB".into()), ..Default::default() },
            after: MixLevels { volume: Some(0.7), volume_text: Some("-4.5 dB".into()), mute: Some(true), ..Default::default() },
        };
        let lines = text(&draw(&mix, 40, 6, Graphics::Cells, 1, palette::GROUND).lines);
        assert!(lines[0].ends_with(" Volume -4.5 dB"), "{lines:?}");
        assert_eq!(text(&lines_of(&mix, 40)), ["Volume 0.0 dB → -4.5 dB"]);
    }

    #[test]
    fn a_lane_says_its_parameter_and_its_points() {
        let said = |parameter: &str, words: Option<&str>| ChangePicture::Automation {
            parameter: parameter.into(),
            min: -15.,
            max: 15.,
            before: vec![],
            after: vec![[0., 0.], [4., -8.]],
            words: words.map(str::to_owned),
        };
        let lane = |parameter: &str| said(parameter, None);
        assert_eq!(text(&lines_of(&lane("3 Gain A"), 40)), ["3 Gain A · 0 → 2 points"]);
        assert_eq!(text(&lines_of(&lane(""), 40)), ["0 → 2 points"]);
        // (review U20) What it moves between and where, where the DAW said.
        let volume = said("Volume", Some("−8 to 0 dB over bars 5–9"));
        assert_eq!(text(&lines_of(&volume, 60)), ["Volume · 0 → 2 points", "−8 to 0 dB over bars 5–9"]);
    }

    #[test]
    fn a_trackers_lines_read_before_and_after_and_fold_where_they_dont_fit() {
        let line = PictureLine { line: 9, before: "E-4 01 40 .. ..".into(), after: "OFF .. .. .. ..".into() };
        let picture = ChangePicture::Lines { lines: vec![line], more: 2 };
        assert_eq!(text(&lines_of(&picture, 60)), [" 09 E-4 01 40 .. .. → OFF .. .. .. ..", "    and 2 more lines"]);
        assert_eq!(text(&lines_of(&picture, 24)), [" 09 E-4 01 40 .. ..", "  → OFF .. .. .. ..", "    and 2 more lines"]);
        let drawn = draw(&picture, 60, 6, Graphics::Cells, 1, palette::GROUND);
        assert!(drawn.rows.is_empty() && drawn.image.is_none(), "words everywhere");
        assert_eq!(text(&drawn.lines), text(&lines_of(&picture, 60)));
    }

    #[test]
    fn a_route_says_its_ends_then_its_values_as_bars() {
        let volume = PictureValue {
            name: "Volume".into(),
            from: Some(-12.),
            to: Some(-6.),
            min: Some(-60.),
            max: Some(12.),
            from_text: Some("−12 dB".into()),
            to_text: Some("−6 dB".into()),
            asked: None,
        };
        let route = |change| ChangePicture::Routes {
            routes: vec![PictureRoute { from: "Bass".into(), to: "Reverb".into(), change, values: vec![volume.clone()] }],
        };
        let lines = text(&draw(&route(RouteChange::Changed), 40, 6, Graphics::Cells, 1, palette::GROUND).lines);
        assert_eq!(lines[0], "Bass → Reverb");
        assert!(lines[1].starts_with("  ") && lines[1].ends_with(" Volume −12 dB → −6 dB"), "{lines:?}");
        assert_eq!(text(&lines_of(&route(RouteChange::Made), 40)), ["Bass → Reverb · new", "  Volume −12 dB → −6 dB"]);
        assert_eq!(text(&lines_of(&route(RouteChange::Removed), 40))[0], "Bass → Reverb · taken away");
    }

    #[test]
    fn long_words_are_cut_to_the_width() {
        let value = PictureValue {
            name: "A very long parameter name indeed".into(),
            to: Some(1.),
            to_text: Some("100 %".into()),
            ..Default::default()
        };
        let lines = lines_of(&ChangePicture::Values { values: vec![value] }, 12);
        assert!(text_width(&text(&lines)[0]) <= 12);
        assert!(text(&lines)[0].ends_with('…'));
    }
}
