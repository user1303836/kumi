//! A picture as shapes in a unit square (x right, y up): a change's (an equaliser's response, a compressor's curve, a
//! clip's notes, an automation lane, an instrument's keyzones, items on the timeline), or the map's points. The
//! rasterizers (RGBA for terminal graphics, braille and half-blocks for cells) draw these; nothing here knows the DAW.

use std::f64::consts::PI;

use kumi_types::contracts::{ChangePicture, ClipNote};
use kumi_types::picture::{Dynamics, EqBand, EqShape, PictureItem, PictureZone};

use super::super::style::{palette, Rgb, Theme};

/// One thing drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// Joined points, `width` in thousandths of the picture's height.
    Line { points: Vec<(f64, f64)>, color: Rgb, alpha: f64, width: f64 },
    /// A filled box between two corners.
    Rect { x0: f64, y0: f64, x1: f64, y1: f64, color: Rgb, alpha: f64 },
    /// A filled disc at (x, y), `radius` in thousandths of the picture's height.
    Dot { x: f64, y: f64, radius: f64, color: Rgb, alpha: f64 },
}

/// What a picture draws, and whether it reads as lines (curves: braille in cells) or areas (notes: half-blocks).
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub shapes: Vec<Shape>,
    pub lines: bool,
}

impl Scene {
    /// The scene in `theme`'s colours.
    pub fn in_theme(mut self, theme: &Theme) -> Scene {
        for shape in &mut self.shapes {
            let (Shape::Line { color, .. } | Shape::Rect { color, .. } | Shape::Dot { color, .. }) = shape;
            *color = theme.of(*color);
        }
        self
    }
}

/// What came before, drawn under what's there now.
pub const GHOST: Rgb = palette::FAINT;
/// What's there now.
pub const NOW: Rgb = palette::ACCENT;
/// Guides: 0 dB, the 1:1 line.
pub const GUIDE: Rgb = palette::RULE;

/// The shapes for a picture that draws as one: curves, notes and lanes; None for values and levels (drawn as bars).
pub fn scene_of(picture: &ChangePicture) -> Option<Scene> {
    match picture {
        ChangePicture::Eq { before, after, .. } => Some(eq_scene(before, after)),
        ChangePicture::Dynamics { before, after, .. } => Some(dynamics_scene(before, after)),
        ChangePicture::Notes { length, before, after, .. } => notes_scene(*length, before, after),
        ChangePicture::Automation { min, max, before, after, .. } => automation_scene(*min, *max, before, after),
        ChangePicture::Zones { before, after, .. } => zones_scene(before, after),
        ChangePicture::Items { before, after, .. } => items_scene(before, after),
        ChangePicture::Values { .. } | ChangePicture::Mix { .. } | ChangePicture::Lines { .. } | ChangePicture::Routes { .. } => None,
    }
}

/// Keyzones as boxes, keys across (an octave at least) and velocities up: where each was, ghosted, and where it is. A
/// zone whose velocities aren't known (a change that didn't touch them) is a band across its keys, claiming none.
fn zones_scene(before: &[PictureZone], after: &[PictureZone]) -> Option<Scene> {
    let all = || before.iter().chain(after);
    let (low, high) = all().fold((u8::MAX, 0), |(low, high), zone| (low.min(zone.notes[0]), high.max(zone.notes[1])));
    if low > high {
        return None;
    }
    let (low, high) = (low as f64, high as f64 + 1.);
    let span = (high - low).max(12.);
    let low = (low - ((span - (high - low)) / 2.).floor()).max(0.);
    let rect = |zone: &PictureZone, color: Rgb, alpha: f64| Shape::Rect {
        x0: ((zone.notes[0] as f64 - low) / span).clamp(0., 1.),
        y0: zone.velocities.map_or(0.4, |[low, _]| low as f64 / 128.),
        x1: ((zone.notes[1] as f64 + 1. - low) / span).clamp(0., 1.),
        y1: zone.velocities.map_or(0.6, |[_, high]| (high as f64 + 1.) / 128.),
        color,
        alpha,
    };
    let mut shapes: Vec<Shape> = before.iter().filter(|zone| !after.contains(zone)).map(|zone| rect(zone, GHOST, 0.45)).collect();
    shapes.extend(after.iter().map(|zone| if before.contains(zone) { rect(zone, palette::DIM, 0.8) } else { rect(zone, NOW, 0.9) }));
    Some(Scene { shapes, lines: false })
}

/// Items as bars on one timeline: before above, ghosted, after below, in colour; a fade slopes in from the bar's edge.
fn items_scene(before: &[PictureItem], after: &[PictureItem]) -> Option<Scene> {
    let all = || before.iter().chain(after);
    let start = all().map(|item| item.start).fold(f64::INFINITY, f64::min);
    let end = all().map(|item| item.start + item.length).fold(f64::NEG_INFINITY, f64::max);
    if !(end > start) {
        return None;
    }
    // A little room either side, so an item's edges show.
    let margin = (end - start) * 0.05;
    let (start, span) = (start - margin, end - start + 2. * margin);
    let x = |at: f64| ((at - start) / span).clamp(0., 1.);
    let mut shapes = Vec::new();
    for (items, (y0, y1), color) in [(before, (0.55, 0.95), GHOST), (after, (0.05, 0.45), NOW)] {
        for item in items {
            let alpha = if item.muted {
                0.35
            } else if color == GHOST {
                0.6
            } else {
                1.
            };
            shapes.push(Shape::Rect { x0: x(item.start), y0, x1: x(item.start + item.length), y1, color, alpha });
            if item.fade_in > 0. {
                shapes.push(Shape::Line {
                    points: vec![(x(item.start), y0), (x(item.start + item.fade_in), y1)],
                    color: GUIDE,
                    alpha: 1.,
                    width: 10.,
                });
            }
            if item.fade_out > 0. {
                let end = item.start + item.length;
                shapes.push(Shape::Line { points: vec![(x(end - item.fade_out), y1), (x(end), y0)], color: GUIDE, alpha: 1., width: 10. });
            }
        }
    }
    Some(Scene { shapes, lines: false })
}

const LOW_HZ: f64 = 20.;
const HIGH_HZ: f64 = 20_000.;
const POINTS: usize = 160;

fn eq_scene(before: &[EqBand], after: &[EqBand]) -> Scene {
    let frequencies: Vec<f64> = (0..POINTS).map(|i| LOW_HZ * (HIGH_HZ / LOW_HZ).powf(i as f64 / (POINTS - 1) as f64)).collect();
    let was: Vec<f64> = frequencies.iter().map(|f| response_db(before, *f)).collect();
    let now: Vec<f64> = frequencies.iter().map(|f| response_db(after, *f)).collect();
    // ±12 dB, or more to fit the largest boost or cut (up to ±30).
    let most = was.iter().chain(&now).map(|db| db.abs()).filter(|db| db.is_finite()).fold(0., f64::max);
    let range = ((most / 6.).ceil() * 6.).clamp(12., 30.);
    let y = |db: f64| (0.5 + db.clamp(-range, range) / (2. * range)).clamp(0., 1.);
    let curve = |dbs: &[f64]| dbs.iter().enumerate().map(|(i, db)| (i as f64 / (POINTS - 1) as f64, y(*db))).collect::<Vec<_>>();
    Scene {
        shapes: vec![
            Shape::Line { points: vec![(0., 0.5), (1., 0.5)], color: GUIDE, alpha: 1., width: 6. },
            Shape::Line { points: curve(&was), color: GHOST, alpha: 0.7, width: 14. },
            Shape::Line { points: curve(&now), color: NOW, alpha: 1., width: 22. },
        ],
        lines: true,
    }
}

/// The bands' response at `frequency`, in dB: each band a biquad (RBJ's cookbook) at 96 kHz, cuts cascaded for their
/// slope (12 dB per octave each).
pub fn response_db(bands: &[EqBand], frequency: f64) -> f64 {
    bands.iter().filter(|band| band.on).map(|band| band_db(band, frequency)).sum()
}

const RATE: f64 = 96_000.;

fn band_db(band: &EqBand, frequency: f64) -> f64 {
    let w0 = 2. * PI * band.freq.clamp(1., RATE * 0.49) / RATE;
    let (cos, sin) = (w0.cos(), w0.sin());
    let q = band.q.max(0.05);
    let alpha = sin / (2. * q);
    let gain = 10f64.powf(band.gain_db / 40.);
    let root = 2. * gain.sqrt() * alpha;
    let (b, a) = match band.shape {
        EqShape::Bell => ([1. + alpha * gain, -2. * cos, 1. - alpha * gain], [1. + alpha / gain, -2. * cos, 1. - alpha / gain]),
        EqShape::Notch => ([1., -2. * cos, 1.], [1. + alpha, -2. * cos, 1. - alpha]),
        EqShape::LowCut => ([(1. + cos) / 2., -(1. + cos), (1. + cos) / 2.], [1. + alpha, -2. * cos, 1. - alpha]),
        EqShape::HighCut => ([(1. - cos) / 2., 1. - cos, (1. - cos) / 2.], [1. + alpha, -2. * cos, 1. - alpha]),
        EqShape::LowShelf => (
            [
                gain * ((gain + 1.) - (gain - 1.) * cos + root),
                2. * gain * ((gain - 1.) - (gain + 1.) * cos),
                gain * ((gain + 1.) - (gain - 1.) * cos - root),
            ],
            [(gain + 1.) + (gain - 1.) * cos + root, -2. * ((gain - 1.) + (gain + 1.) * cos), (gain + 1.) + (gain - 1.) * cos - root],
        ),
        EqShape::HighShelf => (
            [
                gain * ((gain + 1.) + (gain - 1.) * cos + root),
                -2. * gain * ((gain - 1.) + (gain + 1.) * cos),
                gain * ((gain + 1.) + (gain - 1.) * cos - root),
            ],
            [(gain + 1.) - (gain - 1.) * cos + root, 2. * ((gain - 1.) - (gain + 1.) * cos), (gain + 1.) - (gain - 1.) * cos - root],
        ),
    };
    let sections = match band.shape {
        EqShape::LowCut | EqShape::HighCut => (band.slope.unwrap_or(12.) / 12.).round().clamp(1., 8.),
        _ => 1.,
    };
    let w = 2. * PI * frequency / RATE;
    let magnitude = |c: [f64; 3]| {
        let real = c[0] + c[1] * w.cos() + c[2] * (2. * w).cos();
        let imaginary = c[1] * w.sin() + c[2] * (2. * w).sin();
        (real * real + imaginary * imaginary).sqrt()
    };
    let db = 20. * (magnitude(b) / magnitude(a)).max(1e-9).log10();
    db * sections
}

const LOWEST_DB: f64 = -60.;

fn dynamics_scene(before: &Dynamics, after: &Dynamics) -> Scene {
    // From 60 dB below full scale, or lower to show a threshold set lower.
    let low = (before.threshold_db.min(after.threshold_db) - 12.).clamp(-96., LOWEST_DB);
    let at = |db: f64| ((db - low) / -low).clamp(0., 1.);
    let curve = |dynamics: &Dynamics| {
        (0..=POINTS)
            .map(|i| {
                let input = low + -low * i as f64 / POINTS as f64;
                (at(input), at(output_db(dynamics, input)))
            })
            .collect::<Vec<_>>()
    };
    Scene {
        shapes: vec![
            Shape::Line { points: vec![(0., 0.), (1., 1.)], color: GUIDE, alpha: 1., width: 6. },
            Shape::Line { points: curve(before), color: GHOST, alpha: 0.7, width: 14. },
            Shape::Line { points: curve(after), color: NOW, alpha: 1., width: 22. },
        ],
        lines: true,
    }
}

/// A level through the compressor: unchanged below the knee, the ratio above it, a smooth bend through it, then the
/// makeup gain.
pub fn output_db(dynamics: &Dynamics, input: f64) -> f64 {
    let (threshold, ratio, knee) = (dynamics.threshold_db, dynamics.ratio.max(1.), dynamics.knee_db.max(0.));
    let over = input - threshold;
    let compressed = if knee > 0. && over.abs() <= knee / 2. {
        input + (1. / ratio - 1.) * (over + knee / 2.).powi(2) / (2. * knee)
    } else if over > 0. {
        threshold + over / ratio
    } else {
        input
    };
    compressed + dynamics.makeup_db
}

fn notes_scene(length: f64, before: &[ClipNote], after: &[ClipNote]) -> Option<Scene> {
    let all = || before.iter().chain(after);
    let end = all().map(|note| note.start + note.duration).fold(length.max(0.), f64::max);
    if end <= 0. || all().next().is_none() {
        return None;
    }
    let (low, high) = all().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), note| (low.min(note.pitch), high.max(note.pitch)));
    // At least an octave tall, so a few notes don't fill it.
    let span = (high - low + 1.).max(12.);
    let low = low - ((span - (high - low + 1.)) / 2.).floor();
    let same =
        |a: &ClipNote, b: &ClipNote| a.pitch == b.pitch && (a.start - b.start).abs() < 1e-6 && (a.duration - b.duration).abs() < 1e-6;
    let rect = |note: &ClipNote, color: Rgb, alpha: f64| Shape::Rect {
        x0: note.start / end,
        y0: (note.pitch - low) / span,
        x1: ((note.start + note.duration) / end).min(1.),
        y1: (note.pitch - low + 1.) / span,
        color,
        alpha,
    };
    let mut shapes = Vec::new();
    // Gone: ghosted; kept: dim; new or moved: in colour, on top.
    shapes.extend(before.iter().filter(|old| !after.iter().any(|new| same(old, new))).map(|note| rect(note, GHOST, 0.45)));
    for note in after {
        let kept = before.iter().any(|old| same(old, note));
        shapes.push(if kept { rect(note, palette::DIM, 0.8) } else { rect(note, NOW, 1.) });
    }
    Some(Scene { shapes, lines: false })
}

fn automation_scene(min: f64, max: f64, before: &[[f64; 2]], after: &[[f64; 2]]) -> Option<Scene> {
    if max <= min || (before.is_empty() && after.is_empty()) {
        return None;
    }
    let end = before.iter().chain(after).map(|p| p[0]).fold(0., f64::max).max(4.);
    let lane = |points: &[[f64; 2]]| {
        let mut points: Vec<[f64; 2]> = points.to_vec();
        points.sort_by(|a, b| a[0].total_cmp(&b[0]));
        let mut line: Vec<(f64, f64)> = points.iter().map(|[at, value]| (at / end, ((value - min) / (max - min)).clamp(0., 1.))).collect();
        // A lane holds its first and last values out to the clip's edges.
        if let (Some(first), Some(last)) = (line.first().copied(), line.last().copied()) {
            line.insert(0, (0., first.1));
            line.push((1., last.1));
        }
        line
    };
    let mut shapes = Vec::new();
    if !before.is_empty() {
        shapes.push(Shape::Line { points: lane(before), color: GHOST, alpha: 0.7, width: 14. });
    }
    if !after.is_empty() {
        shapes.push(Shape::Line { points: lane(after), color: NOW, alpha: 1., width: 22. });
    }
    Some(Scene { shapes, lines: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(shape: EqShape, freq: f64, gain_db: f64, q: f64) -> EqBand {
        EqBand { shape, freq, gain_db, q, on: true, slope: None }
    }

    fn close(a: f64, b: f64, by: f64) -> bool {
        (a - b).abs() <= by
    }

    #[test]
    fn a_bell_peaks_at_its_gain_and_fades_away_from_it() {
        let bell = [band(EqShape::Bell, 1000., 6., 1.)];
        assert!(close(response_db(&bell, 1000.), 6., 0.01), "{}", response_db(&bell, 1000.));
        assert!(response_db(&bell, 100.).abs() < 0.3);
        assert!(response_db(&bell, 10_000.).abs() < 0.3);
        let cut = [band(EqShape::Bell, 250., -4.5, 2.)];
        assert!(close(response_db(&cut, 250.), -4.5, 0.01));
    }

    #[test]
    fn shelves_reach_their_gain_beyond_their_frequency_and_half_of_it_at_it() {
        let low = [band(EqShape::LowShelf, 200., 6., 0.71)];
        assert!(close(response_db(&low, 20.), 6., 0.2), "{}", response_db(&low, 20.));
        assert!(close(response_db(&low, 200.), 3., 0.05), "{}", response_db(&low, 200.));
        assert!(response_db(&low, 10_000.).abs() < 0.2);
        let high = [band(EqShape::HighShelf, 5000., -6., 0.71)];
        assert!(close(response_db(&high, 20_000.), -6., 0.3), "{}", response_db(&high, 20_000.));
        assert!(close(response_db(&high, 5000.), -3., 0.05), "{}", response_db(&high, 5000.));
        assert!(response_db(&high, 100.).abs() < 0.2);
    }

    #[test]
    fn cuts_fall_by_their_slope_and_off_bands_do_nothing() {
        let cut = [EqBand { slope: Some(12.), ..band(EqShape::LowCut, 100., 0., 0.71) }];
        assert!(close(response_db(&cut, 100.), -3., 0.1), "{}", response_db(&cut, 100.));
        assert!(close(response_db(&cut, 25.), -24., 1.), "two octaves below, 12 dB each: {}", response_db(&cut, 25.));
        let steep = [EqBand { slope: Some(48.), ..band(EqShape::LowCut, 100., 0., 0.71) }];
        assert!(response_db(&steep, 50.) < response_db(&cut, 50.) - 15.);
        let off = [EqBand { on: false, ..band(EqShape::Bell, 1000., 12., 1.) }];
        assert_eq!(response_db(&off, 1000.), 0.);
        let notch = [band(EqShape::Notch, 1000., 0., 4.)];
        assert!(response_db(&notch, 1000.) < -40.);
    }

    #[test]
    fn a_compressor_leaves_quiet_levels_and_bends_loud_ones_by_its_ratio() {
        let hard = Dynamics { threshold_db: -20., ratio: 4., knee_db: 0., makeup_db: 0. };
        assert_eq!(output_db(&hard, -40.), -40.);
        assert_eq!(output_db(&hard, 0.), -15.);
        let soft = Dynamics { knee_db: 10., makeup_db: 3., ..hard.clone() };
        assert_eq!(output_db(&soft, -40.), -37.);
        // At the threshold, halfway through a 10 dB knee: 0.75 × 5² / 20 below the input, then the makeup.
        assert!(close(output_db(&soft, -20.), -20. - 0.9375 + 3., 1e-9));
        assert!(close(output_db(&soft, 0.), -12., 1e-9));
    }

    #[test]
    fn notes_gone_kept_and_new_each_draw_their_own_way() {
        let note = |pitch: f64, start: f64| ClipNote { pitch, start, duration: 1., velocity: 100. };
        let before = [note(60., 0.), note(64., 1.)];
        let after = [note(60., 0.), note(65., 1.)];
        let scene = notes_scene(4., &before, &after).unwrap();
        let colors: Vec<Rgb> =
            scene.shapes.iter().map(|shape| if let Shape::Rect { color, .. } = shape { *color } else { panic!() }).collect();
        assert_eq!(colors, [GHOST, palette::DIM, NOW], "64 gone, 60 kept, 65 new");
        let Shape::Rect { x0, x1, .. } = &scene.shapes[2] else { panic!() };
        assert_eq!((*x0, *x1), (0.25, 0.5));
        assert!(!scene.lines);
    }

    #[test]
    fn keyzones_draw_keys_across_and_velocities_up() {
        let zone =
            |notes: [u8; 2], velocities: [u8; 2]| PictureZone { name: String::new(), notes, velocities: Some(velocities), base: None };
        let scene = zones_scene(&[zone([48, 59], [0, 127])], &[zone([60, 71], [64, 127])]).unwrap();
        let [Shape::Rect { x0, color: was, .. }, Shape::Rect { x0: now_x0, y0, y1, color: now, .. }] = scene.shapes.as_slice() else {
            panic!("{:?}", scene.shapes)
        };
        assert_eq!((*was, *now), (GHOST, NOW));
        assert_eq!((*x0, *now_x0), (0., 0.5), "two octaves across, the new zone in the upper one");
        assert_eq!((*y0, *y1), (0.5, 1.), "the loud half");
        assert!(zones_scene(&[], &[]).is_none());
        // Velocities the change didn't touch: a band across the keys, not the whole height.
        let keys_only = PictureZone { name: String::new(), notes: [48, 59], velocities: None, base: None };
        let scene = zones_scene(&[], &[keys_only]).unwrap();
        let [Shape::Rect { y0, y1, .. }] = scene.shapes.as_slice() else { panic!() };
        assert!(*y0 > 0. && *y1 < 1., "{y0}..{y1}");
    }

    #[test]
    fn items_draw_before_above_and_after_below_with_their_fades() {
        let item = |start: f64, fade_out: f64| PictureItem { name: String::new(), start, length: 4., fade_in: 0., fade_out, muted: false };
        let scene = items_scene(&[item(10., 0.)], &[item(14., 1.)]).unwrap();
        let rects: Vec<(f64, f64, Rgb)> = scene
            .shapes
            .iter()
            .filter_map(|shape| if let Shape::Rect { x0, y0, color, .. } = shape { Some((*x0, *y0, *color)) } else { None })
            .collect();
        assert_eq!(rects.len(), 2);
        assert!(rects[0].0 < rects[1].0 && rects[0].1 > rects[1].1, "moved later, drawn below: {rects:?}");
        assert_eq!((rects[0].2, rects[1].2), (GHOST, NOW));
        assert!(scene.shapes.iter().any(|shape| matches!(shape, Shape::Line { color, .. } if *color == GUIDE)), "the fade out");
    }

    #[test]
    fn an_automation_lane_holds_its_ends_out_to_the_edges() {
        let scene = automation_scene(0., 1., &[], &[[2., 0.5], [6., 1.]]).unwrap();
        let [Shape::Line { points, color, .. }] = scene.shapes.as_slice() else { panic!() };
        assert_eq!(*color, NOW);
        assert_eq!(points.as_slice(), [(0., 0.5), (2. / 6., 0.5), (1., 1.), (1., 1.)]);
        assert!(automation_scene(1., 1., &[[0., 1.]], &[]).is_none());
    }
}
