//! /map: the producer's sounds and songs as two maps, full screen in place of the conversation until it closes. Arrows
//! go to the nearest point that way, shift and the arrows pan, + and − zoom (about the point chosen), a click picks a
//! point, / filters, space plays, a point plays as the mouse rests on it (h turns that off and on), enter offers what
//! to ask Kumi, tab switches maps and esc closes. Zoomed out on a big
//! map it draws density, busier places brighter; points, then their names, show as it zooms in. The points are an
//! image where the terminal shows images and braille elsewhere, with the point chosen and the names as text over
//! them; a console that shows neither lists the groups with their counts and examples instead.
//!
//! Each map is turned so its long side runs across the screen and fitted to it. Its points grow as it zooms in (a
//! typical sound a little bigger than the rest), a soft light shows where they gather, and where the map is drawn in
//! cells a zoom or a move glides.

use std::rc::Rc;
use std::time::{Duration, Instant};

use super::pictures::{cells, kitty, scene::Shape, Graphics, Raster, Scene};
use super::render::Cursor;
use super::screen::{Rect, Screen};
use super::style::{palette, Rgb, Style};
use super::width::{text_width, truncate};
use super::wrap::Span;
use crate::map::{asked, Action, Kind, Layer, MapData, Point, ACTIONS};
use kumi_listening::music::tool::grouped;

/// How far in the map zooms at most, and how much each + or − zooms.
const DEEPEST: f64 = 2048.;
const STEP: f64 = 1.5;
/// More points than this in view are drawn as density.
const DENSE: usize = 3000;
/// Names show beside the points when this many are in view, or fewer.
const NAMED: usize = 30;
/// The nearest points named under the map.
const NEIGHBOURS: usize = 4;
/// How much of the area the whole map fills, zoomed out: a margin round it.
const FILL: f64 = 0.94;
/// The widest name beside a point, in cells.
const NAME_CELLS: i32 = 18;
/// An image's pixels in a cell, and the most it has.
const CELL_PIXELS: (f64, f64) = (8., 16.);
const MOST_PIXELS: (f64, f64) = (1600., 800.);
/// Points grow as the map zooms in: this part of the room they have on screen, up to this radius (in dots).
const GROW: f64 = 0.4;
const MOST_RADIUS: f64 = 5.;
/// The most of the way to its nearest a point's radius takes: two near points' discs touch, not run together.
const TOUCHING: f64 = 0.45;
/// How typical a point is: how many others have it among their nearest this many on the map.
const NEAREST: usize = 8;
/// A map fitted to the screen: past this share of its points' distances from its middle, a stray's distance beyond it
/// grows as a log, this part of that distance at a time; and a map at most this many times as wide as it's tall.
const STRAYS: f64 = 0.985;
const STRAY_EASE: f64 = 0.12;
const WIDEST: f64 = 4.;
/// The light where points gather: at most this share of their colour, none under the last.
const LIGHT: f64 = 0.26;
const FAINTEST: f64 = 0.03;
/// How long a zoom or a move glides.
const GLIDE: Duration = Duration::from_millis(150);
/// How near a click or the mouse comes to a point to be on it, in dots.
const REACH: f64 = 8.;
/// A point plays as the mouse rests on it: this much of it (seconds), faded out over this long when that cuts it short.
const PREVIEW: f64 = 2.;
const PREVIEW_FADE: f64 = 0.2;

/// What a key asks of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Asked {
    Nothing,
    /// Play this stretch of a file.
    Play {
        path: String,
        from: f64,
        seconds: f64,
        name: String,
    },
    Stop,
    /// The mouse came to rest on a point (hover plays on): play this stretch of it, faded out over `fade` seconds,
    /// once it has rested a moment.
    Hover {
        path: String,
        from: f64,
        seconds: f64,
        fade: f64,
        name: String,
    },
    /// The mouse left the point it was on (or came to the one Space plays, or Tab or the filter took that point away): a
    /// hover's play stops, and one about to start doesn't.
    Leave,
    /// Whether points play as the mouse rests on them, changed with h: to remember.
    HoverPlays(bool),
    /// These words in the input box, the map closed.
    Put(String),
    Close,
}

/// Where each map is: what the filter keeps, the point chosen, and what the area shows.
#[derive(Clone, Debug)]
struct View {
    filter: String,
    /// The points the filter keeps, by their place in the layer.
    shown: Vec<u32>,
    chosen: Option<u32>,
    /// The chosen point's nearest, by their place in the layer.
    near: Vec<u32>,
    /// The map's point in the middle of the area, and how far in it is (1: all of it).
    centre: (f64, f64),
    zoom: f64,
    /// How wide the map is against its height: its x runs from 0 to this, its y from 0 to 1.
    aspect: f64,
    /// Where the list is, on a console that lists the groups.
    row: usize,
}

impl View {
    fn new(layer: &Layer, aspect: f64) -> View {
        let mut view =
            View { filter: String::new(), shown: vec![], chosen: None, near: vec![], centre: (aspect / 2., 0.5), zoom: 1., aspect, row: 0 };
        view.filter_by(layer, "");
        view
    }

    /// Keeps the points whose name, folder, tags or group have every word of `filter` (any case).
    fn filter_by(&mut self, layer: &Layer, filter: &str) {
        let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
        let has = |text: &str| {
            let text = text.to_lowercase();
            words.iter().map(|word| text.contains(word.as_str())).collect::<Vec<bool>>()
        };
        let (places, tags): (Vec<Vec<bool>>, Vec<Vec<bool>>) =
            (layer.places.iter().map(|p| has(p)).collect(), layer.tags.iter().map(|t| has(t)).collect());
        let groups: Vec<Vec<bool>> = layer.groups.iter().map(|group| has(&group.name)).collect();
        self.filter = filter.trim().to_string();
        self.shown = layer
            .points
            .iter()
            .enumerate()
            .filter(|(_, point)| {
                words.is_empty() || {
                    let name = point.name.to_lowercase();
                    words.iter().enumerate().all(|(at, word)| {
                        places[point.place as usize][at]
                            || tags[point.tags as usize][at]
                            || groups[point.group as usize][at]
                            || name.contains(word.as_str())
                    })
                }
            })
            .map(|(at, _)| at as u32)
            .collect();
        // The point chosen stays where the filter keeps it; else the nearest kept one is.
        let from = self.chosen.map(|at| &layer.points[at as usize]).map_or(self.centre, |point| (point.x as f64, point.y as f64));
        if self.chosen.is_none_or(|at| self.shown.binary_search(&at).is_err()) {
            self.choose(layer, nearest(layer, &self.shown, from, None));
        }
    }

    fn choose(&mut self, layer: &Layer, chosen: Option<u32>) {
        self.chosen = chosen;
        self.near = chosen.map(|at| neighbours(layer, &self.shown, at)).unwrap_or_default();
    }

    fn clamp(&mut self) {
        let middle = (self.aspect / 2., 0.5);
        if self.zoom <= 1. {
            (self.zoom, self.centre) = (1., middle);
        } else if !(self.centre.0.is_finite() && self.centre.1.is_finite()) {
            // Worked out against an area that has nothing in it: the middle.
            self.centre = middle;
        } else {
            self.centre = (self.centre.0.clamp(0., self.aspect), self.centre.1.clamp(0., 1.));
        }
    }

    /// Zooms by `by`, keeping `anchor` (a point of the map) where it is on the screen.
    fn zoom_by(&mut self, by: f64, anchor: (f64, f64)) {
        let zoom = (self.zoom * by).clamp(1., DEEPEST);
        let kept = self.zoom / zoom;
        self.centre = (anchor.0 + (self.centre.0 - anchor.0) * kept, anchor.1 + (self.centre.1 - anchor.1) * kept);
        self.zoom = zoom;
        self.clamp();
    }
}

/// Map units to dots in the area: two across a cell and four down, square on most terminals.
#[derive(Clone, Copy, Debug)]
struct Frame {
    width: f64,
    height: f64,
    scale: f64,
    centre: (f64, f64),
}

impl Frame {
    fn new(area: Rect, view: &View) -> Frame {
        Frame::showing(area, view.aspect, view.zoom, view.centre)
    }

    /// The area showing a map `aspect` wide (and 1 tall), `zoom` far in, about `centre`.
    fn showing(area: Rect, aspect: f64, zoom: f64, centre: (f64, f64)) -> Frame {
        let (width, height) = (area.width as f64 * 2., area.height as f64 * 4.);
        Frame { width, height, scale: (width / aspect).min(height) * FILL * zoom, centre }
    }

    /// Where a point of the map falls, in dots from the area's top left.
    fn dot(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (self.width / 2. + (x - self.centre.0) * self.scale, self.height / 2. - (y - self.centre.1) * self.scale)
    }

    /// The point of the map under a dot.
    fn at(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (self.centre.0 + (x - self.width / 2.) / self.scale, self.centre.1 - (y - self.height / 2.) / self.scale)
    }

    fn shows(&self, (x, y): (f64, f64)) -> bool {
        x >= 0. && y >= 0. && x < self.width && y < self.height
    }
}

fn spot(point: &Point) -> (f64, f64) {
    (point.x as f64, point.y as f64)
}

/// A map as it's drawn: how wide it is against its height, how far each point is from its nearest on it, and how
/// typical each is (how many others have it among their NEAREST nearest there, against the median: 0.7 to 1.7).
#[derive(Clone, Debug, Default)]
struct Fit {
    aspect: f64,
    apart: Vec<f32>,
    typical: Vec<f32>,
}

/// Both maps made ready to draw: worked out once as /map opens, off Kumi's thread.
pub struct Ready {
    data: MapData,
    fits: [Fit; 2],
}

/// The maps made ready to draw: each turned so its long side runs across (its places' main direction), strays brought
/// in, and fitted to its own box in place of a square (x from 0 to its aspect, y from 0 to 1), with how far each point
/// is from its nearest there and how typical it is.
pub fn ready(mut data: MapData) -> Ready {
    let fits = [fitted(&mut data.sounds), fitted(&mut data.songs)];
    Ready { data, fits }
}

/// One map turned, its strays brought in and fitted to its box (its points' places changed in place).
fn fitted(layer: &mut Layer) -> Fit {
    let points = &mut layer.points;
    let count = points.len();
    if count == 0 {
        return Fit { aspect: 1., ..Fit::default() };
    }
    let sum = points.iter().fold((0., 0.), |sum, point| (sum.0 + point.x as f64, sum.1 + point.y as f64));
    let mean = (sum.0 / count as f64, sum.1 / count as f64);
    let (mut xx, mut xy, mut yy) = (0., 0., 0.);
    for point in points.iter() {
        let (dx, dy) = (point.x as f64 - mean.0, point.y as f64 - mean.1);
        (xx, xy, yy) = (xx + dx * dx, xy + dx * dy, yy + dy * dy);
    }
    // Turned back by its main direction's angle.
    let angle = 0.5 * (2. * xy).atan2(xx - yy);
    let (cos, sin) = (angle.cos(), angle.sin());
    let mut places: Vec<(f64, f64)> = points
        .iter()
        .map(|point| {
            let (dx, dy) = (point.x as f64 - mean.0, point.y as f64 - mean.1);
            (dx * cos + dy * sin, dy * cos - dx * sin)
        })
        .collect();
    let far: Vec<f64> = places.iter().map(|(x, y)| x.hypot(*y)).collect();
    let mut sorted = far.clone();
    sorted.sort_by(f64::total_cmp);
    let edge = sorted[((count - 1) as f64 * STRAYS) as usize];
    let ease = STRAY_EASE * edge;
    if ease > 0. {
        for (place, far) in places.iter_mut().zip(&far) {
            if *far > edge {
                let kept = (edge + ease * ((far - edge) / ease).ln_1p()) / far;
                *place = (place.0 * kept, place.1 * kept);
            }
        }
    }
    let (mut low, mut high) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
    for (x, y) in &places {
        (low, high) = ((low.0.min(*x), low.1.min(*y)), (high.0.max(*x), high.1.max(*y)));
    }
    let (wide, tall) = (high.0 - low.0, high.1 - low.1);
    let span = tall.max(wide / WIDEST);
    for (point, (x, y)) in points.iter_mut().zip(&places) {
        // A map flatter than WIDEST allows sits in the middle of the height.
        (point.x, point.y) =
            if span > 0. { (((x - low.0) / span) as f32, ((y - low.1) / span + (1. - tall / span) / 2.) as f32) } else { (0.5, 0.5) };
    }
    let aspect = if span > 0. { (wide / span).max(1e-3) } else { 1. };
    let (apart, named) = nearest_named(points, aspect);
    let mut sorted = named.clone();
    sorted.sort_unstable();
    let median = sorted[count / 2] as f64;
    let typical = named.iter().map(|named| ((*named as f64 + 1.) / (median + 1.)).sqrt().clamp(0.7, 1.7) as f32).collect();
    Fit { aspect, apart, typical }
}

/// Each point's distance to its nearest other on a map `aspect` wide, and how many others have it among their NEAREST
/// nearest: a grid of about two points a cell, searched ring by ring out from each point's cell.
fn nearest_named(points: &[Point], aspect: f64) -> (Vec<f32>, Vec<u32>) {
    let count = points.len();
    let across = ((count as f64 / 2. * aspect).sqrt().ceil() as usize).clamp(1, 4096);
    let down = ((count as f64 / 2. / aspect).sqrt().ceil() as usize).clamp(1, 4096);
    let (cell_width, cell_height) = (aspect / across as f64, 1. / down as f64);
    let cell =
        |point: &Point| (((point.x as f64 / cell_width) as usize).min(across - 1), ((point.y as f64 / cell_height) as usize).min(down - 1));
    // The points cell by cell: each cell's start in `order`, then its points.
    let mut starts = vec![0usize; across * down + 1];
    for point in points {
        let (x, y) = cell(point);
        starts[y * across + x + 1] += 1;
    }
    for at in 1..starts.len() {
        starts[at] += starts[at - 1];
    }
    let (mut next, mut order) = (starts.clone(), vec![0u32; count]);
    for (at, point) in points.iter().enumerate() {
        let (x, y) = cell(point);
        order[next[y * across + x]] = at as u32;
        next[y * across + x] += 1;
    }
    let (step, wanted) = (cell_width.min(cell_height), NEAREST.min(count - 1));
    let (mut apart, mut named) = (vec![0f32; count], vec![0u32; count]);
    let mut best: Vec<(f64, u32)> = Vec::with_capacity(NEAREST + 1);
    for (at, point) in points.iter().enumerate() {
        let (cx, cy) = cell(point);
        let (cx, cy) = (cx as isize, cy as isize);
        best.clear();
        for ring in 0..across.max(down) as isize {
            for y in (cy - ring).max(0)..=(cy + ring).min(down as isize - 1) {
                for x in (cx - ring).max(0)..=(cx + ring).min(across as isize - 1) {
                    // The ring's edge only: what's inside it was searched.
                    if (x - cx).abs() != ring && (y - cy).abs() != ring {
                        continue;
                    }
                    let c = y as usize * across + x as usize;
                    for other in &order[starts[c]..starts[c + 1]] {
                        if *other as usize == at {
                            continue;
                        }
                        let there = &points[*other as usize];
                        let d = (there.x as f64 - point.x as f64).powi(2) + (there.y as f64 - point.y as f64).powi(2);
                        if best.len() < NEAREST || d < best[NEAREST - 1].0 {
                            let place = best.partition_point(|(seen, _)| *seen <= d);
                            best.insert(place, (d, *other));
                            best.truncate(NEAREST);
                        }
                    }
                }
            }
            // Past this ring every point is at least `ring` cells away.
            if best.len() == wanted && (wanted < NEAREST || best[wanted - 1].0.sqrt() <= ring as f64 * step) {
                break;
            }
        }
        apart[at] = best.first().map_or(0., |(d, _)| d.sqrt() as f32);
        for (_, other) in &best {
            named[*other as usize] += 1;
        }
    }
    (apart, named)
}

/// How typical point `at` is (1 when the map has no word on it).
fn typical(fit: &Fit, at: u32) -> f64 {
    fit.typical.get(at as usize).map_or(1., |typical| *typical as f64)
}

/// How big point `at` draws, in dots, when the points in view draw `radius` (before how typical each is) at `scale`
/// dots a map unit: by how typical it is, but under half the way to its nearest, so two never run into one blob.
fn radius_of(fit: &Fit, at: u32, radius: f64, scale: f64) -> f64 {
    let room = fit.apart.get(at as usize).map_or(f64::INFINITY, |apart| (*apart as f64 * scale * TOUCHING).max(0.5));
    (radius * typical(fit, at)).min(room)
}

/// The kept point nearest `to` (other than `besides`).
fn nearest(layer: &Layer, shown: &[u32], to: (f64, f64), besides: Option<u32>) -> Option<u32> {
    shown
        .iter()
        .copied()
        .filter(|at| Some(*at) != besides)
        .map(|at| {
            let (x, y) = spot(&layer.points[at as usize]);
            ((x - to.0).powi(2) + (y - to.1).powi(2), at)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, at)| at)
}

/// The point nearest `from` that way (x right, y up): within 60° of it, the closest along it, what's off to the side
/// counting double.
fn toward(layer: &Layer, shown: &[u32], from: u32, (ux, uy): (f64, f64)) -> Option<u32> {
    let (x, y) = spot(&layer.points[from as usize]);
    shown
        .iter()
        .copied()
        .filter(|at| *at != from)
        .filter_map(|at| {
            let (qx, qy) = spot(&layer.points[at as usize]);
            let (dx, dy) = (qx - x, qy - y);
            let along = dx * ux + dy * uy;
            let aside = (dx * uy - dy * ux).abs();
            (along > 0. && aside <= along * 3f64.sqrt()).then_some((along + 2. * aside, at))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, at)| at)
}

/// The kept points nearest a point, nearest first.
fn neighbours(layer: &Layer, shown: &[u32], of: u32) -> Vec<u32> {
    let (x, y) = spot(&layer.points[of as usize]);
    let mut near: Vec<(f64, u32)> = Vec::with_capacity(NEIGHBOURS + 1);
    for &at in shown.iter().filter(|at| **at != of) {
        let (qx, qy) = spot(&layer.points[at as usize]);
        let distance = (qx - x).powi(2) + (qy - y).powi(2);
        if near.len() < NEIGHBOURS || distance < near[NEIGHBOURS - 1].0 {
            let place = near.partition_point(|(d, _)| *d <= distance);
            near.insert(place, (distance, at));
            near.truncate(NEIGHBOURS);
        }
    }
    near.into_iter().map(|(_, at)| at).collect()
}

fn mix(under: Rgb, over: Rgb, share: f64) -> Rgb {
    std::array::from_fn(|c| (under[c] as f64 + (over[c] as f64 - under[c] as f64) * share).round() as u8)
}

/// The kept points in the frame, by their place in the layer.
fn in_view(layer: &Layer, shown: &[u32], frame: &Frame) -> Vec<u32> {
    shown.iter().copied().filter(|at| frame.shows(frame.dot(spot(&layer.points[*at as usize])))).collect()
}

/// How big the points in view draw, in dots (each then by how typical it is): GROW of the room they have on screen, the
/// geometric mean of the median distance from each to its nearest on the map and of the room each would have spread
/// evenly, from half a dot up to MOST_RADIUS.
fn grown(fit: &Fit, in_view: &[u32], frame: &Frame) -> f64 {
    let mut apart: Vec<f32> = in_view.iter().filter_map(|at| fit.apart.get(*at as usize).copied()).collect();
    if apart.is_empty() {
        return 0.5;
    }
    let middle = apart.len() / 2;
    let median = *apart.select_nth_unstable_by(middle, f32::total_cmp).1 as f64 * frame.scale;
    let even = (frame.width * frame.height / in_view.len() as f64).sqrt();
    (GROW * (median * even).sqrt()).clamp(0.5, MOST_RADIUS)
}

/// What `scene` drew: its shapes, the points it named, whether it drew density, and how big its points are (in dots,
/// before each one's typical share).
struct Drawn {
    scene: Scene,
    named: Vec<u32>,
    dense: bool,
    radius: f64,
}

/// The points in view as shapes in the area: a dot each where few are in view, as big as they have room for; where many
/// are, a dot for each patch of them (`patch` dots across), brighter the busier it is and in its commonest group's
/// colour.
fn scene(layer: &Layer, fit: &Fit, in_view: &[u32], frame: &Frame, patch: f64, steps: Option<f64>) -> Drawn {
    let thousandths = |dots: f64| dots / frame.height * 1000.;
    if in_view.len() <= DENSE {
        let radius = grown(fit, in_view, frame);
        let shapes = in_view
            .iter()
            .map(|at| {
                let point = &layer.points[*at as usize];
                let (x, y) = frame.dot(spot(point));
                Shape::Dot {
                    x: x / frame.width,
                    y: 1. - y / frame.height,
                    radius: thousandths(radius_of(fit, *at, radius, frame.scale)),
                    color: layer.groups[point.group as usize].color,
                    alpha: 0.9,
                }
            })
            .collect();
        let named = if in_view.len() <= NAMED { in_view.to_vec() } else { vec![] };
        return Drawn { scene: Scene { shapes, lines: false }, named, dense: false, radius };
    }
    let (columns, rows) = ((frame.width / patch).ceil() as usize, (frame.height / patch).ceil() as usize);
    // Each patch's count, and its commonest group (a running vote: the group most of its points are in, when one is).
    let mut patches = vec![(0u32, 0u16, 0u32); columns * rows];
    for at in in_view {
        let point = &layer.points[*at as usize];
        let (x, y) = frame.dot(spot(point));
        let kept = &mut patches[((y / patch) as usize).min(rows - 1) * columns + ((x / patch) as usize).min(columns - 1)];
        kept.0 += 1;
        if kept.2 == 0 {
            (kept.1, kept.2) = (point.group, 1);
        } else if kept.1 == point.group {
            kept.2 += 1;
        } else {
            kept.2 -= 1;
        }
    }
    let busiest = (patches.iter().map(|p| p.0).max().unwrap_or(1) as f64 + 1.).ln();
    let radius = thousandths(patch * 0.72);
    let shapes = patches
        .iter()
        .enumerate()
        .filter(|(_, (count, _, _))| *count > 0)
        .map(|(at, (count, group, _))| {
            let level = 0.3 + 0.7 * (*count as f64 + 1.).ln() / busiest;
            // In cells the levels are few, so a cell's commonest colour is its commonest group at its brightness.
            let level = steps.map_or(level, |steps| (level * steps).round() / steps);
            let (x, y) = (((at % columns) as f64 + 0.5) * patch, ((at / columns) as f64 + 0.5) * patch);
            let color = mix(palette::GROUND, layer.groups[*group as usize].color, level);
            Shape::Dot { x: x / frame.width, y: 1. - y / frame.height, radius, color, alpha: 1. }
        })
        .collect();
    Drawn { scene: Scene { shapes, lines: false }, named: vec![], dense: true, radius: 0.5 }
}

/// The soft light where points gather, a cell of the area at a time: each cell's points in view in their colours,
/// blurred about as far across as down on screen (a cell is twice as tall as it's wide), at up to LIGHT of their colour
/// where they're busiest; none where it's fainter than FAINTEST.
fn light(layer: &Layer, in_view: &[u32], frame: &Frame, cols: usize, rows: usize) -> Vec<Option<(Rgb, f64)>> {
    if cols == 0 || rows == 0 {
        return vec![];
    }
    let mut sums = vec![[0f64; 4]; cols * rows];
    for at in in_view {
        let point = &layer.points[*at as usize];
        let (x, y) = frame.dot(spot(point));
        let color = layer.groups[point.group as usize].color;
        let sum = &mut sums[((y / 4.) as usize).min(rows - 1) * cols + ((x / 2.) as usize).min(cols - 1)];
        *sum = [sum[0] + color[0] as f64, sum[1] + color[1] as f64, sum[2] + color[2] as f64, sum[3] + 1.];
    }
    let sums = blur(&blur(&sums, cols, rows, (1, 0), 1.8, 5), cols, rows, (0, 1), 0.9, 3);
    let busiest = sums.iter().map(|sum| sum[3]).fold(0., f64::max);
    sums.iter()
        .map(|sum| {
            let share = LIGHT * (sum[3] / busiest).powf(0.75);
            (busiest > 0. && share >= FAINTEST).then(|| ([0, 1, 2].map(|c| (sum[c] / sum[3]).round().clamp(0., 255.) as u8), share))
        })
        .collect()
}

/// Cells' sums blurred one way (`way`: a cell across, or one down), a Gaussian `sigma` cells wide out to `reach` cells.
fn blur(sums: &[[f64; 4]], cols: usize, rows: usize, way: (isize, isize), sigma: f64, reach: isize) -> Vec<[f64; 4]> {
    let weights: Vec<f64> = (-reach..=reach).map(|k| (-((k * k) as f64) / (2. * sigma * sigma)).exp()).collect();
    let mut out = vec![[0f64; 4]; sums.len()];
    for row in 0..rows as isize {
        for col in 0..cols as isize {
            let mut sum = [0f64; 4];
            for (k, weight) in (-reach..=reach).zip(&weights) {
                let (c, r) = (col + k * way.0, row + k * way.1);
                if c >= 0 && r >= 0 && c < cols as isize && r < rows as isize {
                    let from = sums[r as usize * cols + c as usize];
                    (0..4).for_each(|i| sum[i] += from[i] * weight);
                }
            }
            out[row as usize * cols + col as usize] = sum;
        }
    }
    out
}

/// Where a glide shows a map `t` of the way (0 to 1, eased out) from one zoom and centre to another: the zoom on a log
/// scale, the centre along the path that keeps the zoom's anchor still on screen.
fn between(from: (f64, (f64, f64)), to: (f64, (f64, f64)), t: f64) -> (f64, (f64, f64)) {
    if t >= 1. {
        return to;
    }
    let eased = 1. - (1. - t.max(0.)).powi(3);
    let zoom = (from.0.ln() + (to.0.ln() - from.0.ln()) * eased).exp();
    let along = if (to.0 - from.0).abs() < 1e-12 { eased } else { (1. / from.0 - 1. / zoom) / (1. / from.0 - 1. / to.0) };
    let ((fx, fy), (tx, ty)) = (from.1, to.1);
    (zoom, (fx + (tx - fx) * along, fy + (ty - fy) * along))
}

/// A picture drawn for the area, kept until what it shows changes.
struct Picture {
    key: String,
    /// Braille rows; for an image, the placeholder rows that show it under `id`.
    rows: Vec<Vec<Span>>,
    image: Option<Raster>,
    /// The id an image's placeholder rows were made for.
    id: Option<u32>,
    /// The points named beside them.
    named: Vec<u32>,
    dense: bool,
    /// An image's size in pixels.
    pixels: Option<(f64, f64)>,
    /// How big its points are, in dots (before each one's typical share).
    radius: f64,
}

/// A zoom or a move gliding on a map (by its tab), from the zoom and centre it was shown at, since `started`.
#[derive(Clone, Copy, Debug)]
struct Glide {
    tab: usize,
    zoom: f64,
    centre: (f64, f64),
    started: Instant,
}

pub struct MapView {
    graphics: Graphics,
    data: Option<Rc<MapData>>,
    /// Each map as it's drawn (how wide it is, how far apart its points sit and how typical each is), by tab.
    fits: Vec<Fit>,
    tab: usize,
    views: Vec<View>,
    /// The filter being typed.
    prompt: Option<String>,
    /// The actions offered for the point chosen, and which is chosen.
    menu: Option<usize>,
    /// What plays (the point, on its map), and words for the point's row (a sound that wouldn't play).
    playing: Option<(usize, u32)>,
    said: Option<String>,
    /// Points play as the mouse rests on them (h turns it off and on); the point the mouse rests on (on its map); and
    /// whether what plays began that way (a hover's play stops as the mouse leaves; Space's plays on).
    hover: bool,
    hovered: Option<(usize, u32)>,
    from_hover: bool,
    /// Whether zooms and moves glide (where the map is drawn in cells), and the one gliding.
    gliding: bool,
    glide: Option<Glide>,
    /// Bumped whenever what the picture shows changes: pan, zoom, filter, a new map.
    version: u64,
    picture: Option<Picture>,
    /// Where the map was drawn last: a click is read against it.
    area: Rect,
}

impl MapView {
    /// The map, before its data is read; `hover`: whether points play as the mouse rests on them.
    pub fn new(graphics: Graphics, hover: bool) -> MapView {
        MapView {
            graphics,
            data: None,
            fits: vec![],
            tab: 0,
            views: vec![],
            prompt: None,
            menu: None,
            playing: None,
            said: None,
            hover,
            hovered: None,
            from_hover: false,
            gliding: false,
            glide: None,
            version: 0,
            picture: None,
            area: Rect::default(),
        }
    }

    /// The same map with its zooms and moves gliding where it's drawn in cells (an image would go up again each step).
    pub fn gliding(mut self, on: bool) -> MapView {
        self.gliding = on && self.graphics == Graphics::Cells;
        self
    }

    /// The map's data, read and made ready to draw: each map zoomed out with the point nearest its middle chosen. The
    /// songs' map comes first when there are no sounds on one yet.
    pub fn loaded(&mut self, ready: Ready) {
        let Ready { data, fits } = ready;
        self.views = vec![View::new(&data.sounds, fits[0].aspect), View::new(&data.songs, fits[1].aspect)];
        self.fits = fits.into();
        self.tab = usize::from(data.sounds.points.is_empty() && !data.songs.points.is_empty());
        self.data = Some(Rc::new(data));
        self.version += 1;
    }

    /// The zoom and centre the map on show is drawn at now: its view's, or partway along a glide to it.
    fn shown(&self) -> (f64, (f64, f64)) {
        let Some(view) = self.views.get(self.tab) else { return (1., (0.5, 0.5)) };
        let to = (view.zoom, view.centre);
        match self.glide.filter(|glide| glide.tab == self.tab) {
            Some(glide) => between((glide.zoom, glide.centre), to, glide.started.elapsed().as_secs_f64() / GLIDE.as_secs_f64()),
            None => to,
        }
    }

    /// The view moved from where it was drawn (`was`): it glides there, when moves glide.
    fn glide_from(&mut self, was: (f64, (f64, f64))) {
        let Some(view) = self.views.get(self.tab) else { return };
        if self.gliding && was != (view.zoom, view.centre) {
            self.glide = Some(Glide { tab: self.tab, zoom: was.0, centre: was.1, started: Instant::now() });
        }
    }

    /// Whether the map is still gliding: it's drawn again soon.
    pub fn moving(&self) -> bool {
        self.glide.is_some_and(|glide| glide.tab == self.tab && glide.started.elapsed() < GLIDE)
    }

    fn layer(&self) -> Option<&Layer> {
        self.data.as_ref().map(|data| if self.tab == 0 { &data.sounds } else { &data.songs })
    }

    /// The point Space plays and Enter asks about: the one chosen, or in a list the chosen group's first example.
    fn current(&self) -> Option<u32> {
        let (layer, view) = (self.layer()?, self.views.get(self.tab)?);
        if self.graphics == Graphics::Words {
            return groups_by_size(layer).get(view.row).and_then(|group| layer.groups[*group].examples.first().copied());
        }
        view.chosen
    }

    /// Whatever played has ended (or didn't start, and why).
    pub fn played(&mut self, said: Option<String>) {
        (self.playing, self.from_hover) = (None, false);
        self.said = said;
    }

    /// A hover's play begins (the mouse rested on its point long enough): the point the mouse is on plays, and shows so.
    /// False when the mouse is on no point any more: nothing plays.
    pub fn hover_plays(&mut self) -> bool {
        if self.hovered.is_none() {
            return false;
        }
        (self.playing, self.from_hover, self.said) = (self.hovered, true, None);
        true
    }

    /// The mouse left the point it was on, or hover plays were turned off: whether a hover's play was on, which stops.
    pub fn hover_ends(&mut self) -> bool {
        let was = self.from_hover && self.playing.is_some();
        if was {
            self.playing = None;
        }
        self.from_hover = false;
        was
    }

    pub fn on_key(&mut self, name: &str, shift: bool) -> Asked {
        if let Some(prompt) = self.prompt.as_mut() {
            match name {
                "escape" => self.prompt = None,
                "backspace" => {
                    prompt.pop();
                }
                "enter" => {
                    let filter = self.prompt.take().unwrap_or_default();
                    self.refilter(&filter);
                    // The point the mouse rests on, filtered out: its ring goes, and its hover ends.
                    if self.hovered.is_some_and(|(tab, at)| !self.views[tab].shown.contains(&at)) {
                        self.hovered = None;
                        return Asked::Leave;
                    }
                }
                _ => {}
            }
            return Asked::Nothing;
        }
        if let Some(chosen) = self.menu {
            match name {
                "escape" => self.menu = None,
                "left" | "up" => self.menu = Some((chosen + ACTIONS.len() - 1) % ACTIONS.len()),
                "right" | "down" | "tab" => self.menu = Some((chosen + 1) % ACTIONS.len()),
                "enter" => return self.act(ACTIONS[chosen].0),
                _ => {}
            }
            return Asked::Nothing;
        }
        let way = match name {
            "up" => (0., 1.),
            "down" => (0., -1.),
            "left" => (-1., 0.),
            "right" => (1., 0.),
            "escape" => return Asked::Close,
            "space" => return self.play(),
            "tab" if self.data.is_some() => {
                self.tab = 1 - self.tab;
                // A hover on the map left behind ends: one about to play doesn't, and one playing stops.
                let hovering = self.hovered.is_some() || self.from_hover;
                (self.said, self.hovered, self.version) = (None, None, self.version + 1);
                return if hovering { Asked::Leave } else { Asked::Nothing };
            }
            "enter" if self.current().is_some() => {
                self.menu = Some(0);
                return Asked::Nothing;
            }
            _ => return Asked::Nothing,
        };
        if self.graphics == Graphics::Words {
            self.list_by(-way.1 as isize);
        } else if shift {
            self.pan(way);
        } else {
            self.go(way);
        }
        Asked::Nothing
    }

    /// Typed characters, one at a time (they can come together): space plays, + and − zoom, / starts a filter (and
    /// what follows is its words), h turns hover plays off and on, 1 to 3 choose an action, q closes.
    pub fn on_text(&mut self, text: &str) -> Asked {
        for character in text.chars() {
            let asked = self.on_character(character);
            if asked != Asked::Nothing {
                return asked;
            }
        }
        Asked::Nothing
    }

    fn on_character(&mut self, character: char) -> Asked {
        if let Some(prompt) = self.prompt.as_mut() {
            // A pasted line break, or a tab, is a space in the filter's words.
            prompt.push(if character.is_control() { ' ' } else { character });
            return Asked::Nothing;
        }
        if self.menu.is_some() {
            return match character.to_digit(10).map(|number| number as usize) {
                Some(number) if (1..=ACTIONS.len()).contains(&number) => self.act(ACTIONS[number - 1].0),
                _ => Asked::Nothing,
            };
        }
        match character {
            ' ' => return self.play(),
            '+' | '=' => self.zoom_by(STEP, None),
            '-' | '_' => self.zoom_by(1. / STEP, None),
            '/' if self.data.is_some() => self.prompt = Some(String::new()),
            'h' if self.data.is_some() => {
                self.hover = !self.hover;
                if !self.hover {
                    self.hovered = None;
                }
                return Asked::HoverPlays(self.hover);
            }
            'q' => return Asked::Close,
            _ => {}
        }
        Asked::Nothing
    }

    /// A click picks the point nearest it (or the group of a list's row).
    pub fn on_click(&mut self, x: i32, y: i32) -> Asked {
        let area = self.area;
        if self.menu.is_some() || self.prompt.is_some() || !inside(area, x, y) {
            return Asked::Nothing;
        }
        let Some(data) = self.data.clone() else { return Asked::Nothing };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        if self.graphics == Graphics::Words {
            let view = &mut self.views[self.tab];
            let row = (y - area.y) as usize + list_top(view.row, area.height);
            if row < groups_by_size(layer).len() {
                view.row = row;
            }
            return Asked::Nothing;
        }
        let picked = self.under(x, y);
        if picked.is_some() {
            self.views[self.tab].choose(layer, picked);
            self.menu = None;
        }
        Asked::Nothing
    }

    /// The mouse moved: come to rest on a point (within reach, where points are drawn as points, not density), it
    /// plays once it has rested a moment; off it, a hover's play stops. Nothing while hover plays are off, in the
    /// actions or the filter, over density, or on a console's list.
    pub fn on_move(&mut self, x: i32, y: i32) -> Asked {
        let points = self.picture.as_ref().is_some_and(|picture| !picture.dense);
        let over = (self.hover && points && self.menu.is_none() && self.prompt.is_none()).then(|| self.under(x, y)).flatten();
        let now = over.map(|at| (self.tab, at));
        if now == self.hovered {
            return Asked::Nothing;
        }
        self.hovered = now;
        match over.zip(self.layer()) {
            // The point Space plays: it plays on, whole, and no hover of it starts (nor one about to).
            Some(_) if self.playing == now && !self.from_hover => Asked::Leave,
            Some((at, layer)) => {
                let point = &layer.points[at as usize];
                let cut = point.seconds as f64 > PREVIEW;
                Asked::Hover {
                    path: point.path.clone(),
                    from: point.from as f64,
                    seconds: if cut { PREVIEW } else { point.seconds as f64 },
                    fade: if cut { PREVIEW_FADE } else { 0. },
                    name: point.name.clone(),
                }
            }
            None => Asked::Leave,
        }
    }

    /// The point within reach of a cell of the map's area: none outside it, or on a console's list.
    fn under(&self, x: i32, y: i32) -> Option<u32> {
        let area = self.area;
        if self.graphics == Graphics::Words || !inside(area, x, y) {
            return None;
        }
        let (layer, view, fit) = (self.layer()?, self.views.get(self.tab)?, self.fits.get(self.tab)?);
        let frame = Frame::new(area, view);
        let dot = (((x - area.x) * 2 + 1) as f64, ((y - area.y) * 4 + 2) as f64);
        let radius = self.picture.as_ref().map_or(0., |picture| picture.radius);
        nearest(layer, &view.shown, frame.at(dot), None).filter(|at| {
            let (px, py) = frame.dot(spot(&layer.points[*at as usize]));
            (px - dot.0).hypot(py - dot.1) <= REACH.max(radius_of(fit, *at, radius, frame.scale) + 1.)
        })
    }

    /// The wheel zooms about the point of the map under it.
    pub fn on_wheel(&mut self, up: bool, x: i32, y: i32) -> Asked {
        let area = self.area;
        if self.graphics != Graphics::Words && self.data.is_some() && inside(area, x, y) {
            let frame = Frame::new(area, &self.views[self.tab]);
            let anchor = frame.at((((x - area.x) * 2 + 1) as f64, ((y - area.y) * 4 + 2) as f64));
            self.zoom_by(if up { STEP } else { 1. / STEP }, Some(anchor));
        }
        Asked::Nothing
    }

    fn refilter(&mut self, filter: &str) {
        let Some(data) = self.data.clone() else { return };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        let view = &mut self.views[self.tab];
        view.filter_by(layer, filter);
        view.row = 0;
        self.version += 1;
        self.follow();
    }

    /// To the nearest point that way.
    fn go(&mut self, way: (f64, f64)) {
        let Some(data) = self.data.clone() else { return };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        let view = &mut self.views[self.tab];
        let next = match view.chosen {
            Some(from) => toward(layer, &view.shown, from, way),
            None => nearest(layer, &view.shown, view.centre, None),
        };
        if next.is_some() {
            view.choose(layer, next);
            self.follow();
        }
    }

    /// Keeps the point chosen in view, moving the map as little as it takes.
    fn follow(&mut self) {
        let Some(data) = self.data.clone() else { return };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        let was = self.shown();
        let (area, view) = (self.area, &mut self.views[self.tab]);
        let Some(at) = view.chosen.filter(|_| area.width > 0 && area.height > 0) else { return };
        let frame = Frame::new(area, view);
        let (x, y) = frame.dot(spot(&layer.points[at as usize]));
        let inside = |value: f64, size: f64| value.clamp(size * 0.08, size * 0.92);
        let (dx, dy) = (x - inside(x, frame.width), y - inside(y, frame.height));
        let before = view.centre;
        view.centre = (view.centre.0 + dx / frame.scale, view.centre.1 - dy / frame.scale);
        // Zoomed out, the whole map shows: nothing moves.
        view.clamp();
        if view.centre != before {
            self.version += 1;
            self.glide_from(was);
        }
    }

    /// Shift and an arrow: a quarter of the view that way (nothing before the map has been drawn at a size that shows it).
    fn pan(&mut self, (ux, uy): (f64, f64)) {
        let (area, was) = (self.area, self.shown());
        let Some(view) = self.views.get_mut(self.tab).filter(|_| area.width > 0 && area.height > 0) else { return };
        let frame = Frame::new(area, view);
        view.centre = (view.centre.0 + ux * frame.width / frame.scale / 4., view.centre.1 + uy * frame.height / frame.scale / 4.);
        view.clamp();
        self.version += 1;
        self.glide_from(was);
    }

    fn zoom_by(&mut self, by: f64, anchor: Option<(f64, f64)>) {
        let Some(data) = self.data.clone() else { return };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        let was = self.shown();
        let view = &mut self.views[self.tab];
        let anchor = anchor.or_else(|| view.chosen.map(|at| spot(&layer.points[at as usize]))).unwrap_or(view.centre);
        view.zoom_by(by, anchor);
        self.version += 1;
        self.glide_from(was);
    }

    /// Up or down a console's list of groups.
    fn list_by(&mut self, by: isize) {
        let Some(layer) = self.layer() else { return };
        let groups = groups_by_size(layer).len();
        let view = &mut self.views[self.tab];
        view.row = (view.row as isize + by).clamp(0, groups.saturating_sub(1) as isize) as usize;
    }

    fn play(&mut self) -> Asked {
        let (Some(at), Some(layer)) = (self.current(), self.layer()) else { return Asked::Nothing };
        // Space again stops what Space played; over a hover's preview of the point, it plays the point whole.
        if self.playing == Some((self.tab, at)) && !self.from_hover {
            self.playing = None;
            return Asked::Stop;
        }
        let point = &layer.points[at as usize];
        let asked =
            Asked::Play { path: point.path.clone(), from: point.from as f64, seconds: point.seconds as f64, name: point.name.clone() };
        (self.playing, self.from_hover, self.said) = (Some((self.tab, at)), false, None);
        asked
    }

    fn act(&mut self, action: Action) -> Asked {
        self.menu = None;
        match (self.current(), self.layer()) {
            (Some(at), Some(layer)) => Asked::Put(asked(layer.kind, &layer.points[at as usize], action)),
            _ => Asked::Nothing,
        }
    }

    /// The whole screen: the maps' names, the legend, the map (or its list), the point chosen and what it's near, and
    /// the keys. `upload` puts an image up under a key (once) and says the id it shows under. The cursor, while the
    /// filter is typed.
    pub fn draw(&mut self, screen: &mut Screen, upload: &mut dyn FnMut(&str, &Raster, usize, usize) -> u32) -> Option<Cursor> {
        if self.glide.is_some_and(|glide| glide.started.elapsed() >= GLIDE) {
            self.glide = None;
        }
        let (columns, rows) = (screen.width, screen.height);
        if columns < 40 || rows < 12 {
            screen.put(1, 0, &truncate("Make this window bigger for the map (esc closes it)", columns - 2), &Style::fg(palette::DIM));
            return None;
        }
        self.area = Rect::new(1, 2, columns - 2, rows - 5);
        self.draw_header(screen);
        let Some(data) = self.data.clone() else {
            centred(screen, self.area, "Reading your map…", Style::fg(palette::DIM));
            self.draw_keys(screen);
            return None;
        };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        if self.graphics != Graphics::Words {
            self.draw_legend(screen, layer);
        }
        if let Some(why) = &layer.empty {
            centred(screen, self.area, why, Style::fg(palette::DIM));
        } else if self.views[self.tab].shown.is_empty() {
            let words = format!("Nothing on this map has “{}”: / changes the filter.", self.views[self.tab].filter);
            centred(screen, self.area, &words, Style::fg(palette::DIM));
        } else if self.graphics == Graphics::Words {
            self.draw_list(screen, layer);
        } else {
            self.draw_map(screen, layer, upload);
        }
        self.draw_point(screen, layer);
        self.draw_keys(screen)
    }

    fn draw_header(&self, screen: &mut Screen) {
        let mut x = screen.put(1, 0, "Map", &Style { bold: true, ..Style::fg(palette::BRIGHT) }) + 2;
        let counts = self.data.as_ref().map(|data| [data.sounds.points.len(), data.songs.points.len()]);
        for (tab, name) in ["Sounds", "Songs"].into_iter().enumerate() {
            let text = match counts {
                Some(counts) => format!(" {name} {} ", grouped(counts[tab])),
                None => format!(" {name} "),
            };
            let style = if tab == self.tab {
                Style { bold: true, bg: Some(palette::SELECTED), ..Style::fg(palette::BRIGHT) }
            } else {
                Style::fg(palette::DIM)
            };
            x = screen.put(x, 0, &text, &style) + 1;
        }
        if let Some(view) = self.views.get(self.tab) {
            let mut right = String::new();
            if !view.filter.is_empty() {
                right = format!("/ {}: {} shown", truncate(&view.filter, 24), grouped(view.shown.len()));
            }
            if view.zoom > 1. && self.graphics != Graphics::Words {
                right = [right, format!("×{}", (view.zoom * 10.).round() / 10.)]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ");
            }
            let at = screen.width - 1 - text_width(&right);
            if at > x + 1 {
                screen.put(at, 0, &right, &Style::fg(palette::ACCENT));
            }
        }
    }

    /// Each group in its colour, the biggest first, as many as fit.
    fn draw_legend(&self, screen: &mut Screen, layer: &Layer) {
        let mut x = 1;
        for group in groups_by_size(layer).into_iter().map(|at| &layer.groups[at]) {
            let count = grouped(group.count);
            let width = 2 + text_width(&group.name) + 1 + text_width(&count) + 2;
            if x + width > screen.width - 1 {
                break;
            }
            x = screen.put(x, 1, "●", &Style::fg(group.color));
            x = screen.put(x + 1, 1, &group.name, &Style::fg(palette::DIM));
            x = screen.put(x + 1, 1, &count, &Style::fg(palette::FAINT)) + 2;
        }
    }

    fn draw_map(&mut self, screen: &mut Screen, layer: &Layer, upload: &mut dyn FnMut(&str, &Raster, usize, usize) -> u32) {
        // Where a zoom or a move glides, each step is a picture of its own.
        let (zoom, centre) = self.shown();
        let (area, view, fit) = (self.area, &self.views[self.tab], &self.fits[self.tab]);
        let (cols, rows) = (area.width as usize, area.height as usize);
        let key = format!("map {} {} {cols}x{rows} {zoom} {} {}", self.tab, self.version, centre.0, centre.1);
        let frame = Frame::showing(area, view.aspect, zoom, centre);
        if self.picture.as_ref().is_none_or(|picture| picture.key != key) {
            let image = self.graphics == Graphics::Kitty;
            let in_view = in_view(layer, &view.shown, &frame);
            // A patch of density a braille dot across (four pixels of an image); in braille its brightness in steps.
            let drawn = scene(layer, fit, &in_view, &frame, 1., (!image).then_some(6.));
            let light = light(layer, &in_view, &frame, cols, rows);
            let Drawn { scene, named, dense, radius } = drawn;
            let picture = if image {
                let shrink = (MOST_PIXELS.0 / (cols as f64 * CELL_PIXELS.0)).min(MOST_PIXELS.1 / (rows as f64 * CELL_PIXELS.1)).min(1.);
                let (width, height) = ((cols as f64 * CELL_PIXELS.0 * shrink) as u32, (rows as f64 * CELL_PIXELS.1 * shrink) as u32);
                // The light under the points, smooth from cell to cell.
                let mut raster = Raster::new(width.max(1), height.max(1));
                raster.glow(&light, cols, rows);
                raster.paint(&scene);
                let pixels = Some((raster.width as f64, raster.height as f64));
                Picture { key: key.clone(), rows: vec![], image: Some(raster), id: None, named, dense, pixels, radius }
            } else {
                // The light behind each cell's dots, fading as the points grow: big ones are their own light, and a cell's
                // light round each is a box.
                let mut cells = cells::braille(&scene, cols, rows);
                let fade = (1.5 - radius / 2.).clamp(0., 1.);
                for (spans, lights) in cells.iter_mut().zip(light.chunks(cols)) {
                    for (span, light) in spans.iter_mut().zip(lights) {
                        if let Some((color, share)) = light.filter(|(_, share)| share * fade >= FAINTEST) {
                            span.style = Rc::new(Style { bg: Some(mix(palette::GROUND, color, share * fade)), ..*span.style });
                        }
                    }
                }
                Picture { key: key.clone(), rows: cells, image: None, id: None, named, dense, pixels: None, radius }
            };
            self.picture = Some(picture);
        }
        let picture = self.picture.as_mut().expect("drawn");
        // An image's placeholder rows are made again only when it's up under another id (taken down unseen, say): a cell
        // each, thousands a frame.
        if let Some(raster) = &picture.image {
            let id = upload(&key, raster, cols, rows);
            if picture.id != Some(id) {
                picture.rows = kitty::cells(id, cols, rows);
                picture.id = Some(id);
            }
        }
        let picture = &*picture;
        for (row, spans) in picture.rows.iter().enumerate() {
            let mut x = area.x;
            for span in spans {
                x = screen.put(x, area.y + row as i32, &span.text, &span.style);
            }
        }
        // Names beside their points where there's room, the chosen one's first; then the chosen point itself.
        let (view, fit) = (&self.views[self.tab], &self.fits[self.tab]);
        // A name starts a cell clear of its point: past the half of it right of its cell (a disc about the cell's right
        // dot reaches half a cell further).
        let gap = |at: u32| ((1. + radius_of(fit, at, picture.radius, frame.scale)) / 2.).floor() as i32 + 2;
        // The cell each dot is drawn in: an image's pixel, or braille's dot (rounded as braille rounds it).
        let cell = |at: u32| {
            let (x, y) = frame.dot(spot(&layer.points[at as usize]));
            let (x, y) = match picture.pixels {
                Some((width, height)) => {
                    ((x / frame.width * (width - 1.)) / (width / cols as f64), (y / frame.height * (height - 1.)) / (height / rows as f64))
                }
                None => ((x / frame.width * (frame.width - 1.)).round() / 2., (y / frame.height * (frame.height - 1.)).round() / 4.),
            };
            (area.x + x as i32, area.y + y as i32)
        };
        let index = |x: i32, y: i32| {
            (x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height)
                .then(|| ((y - area.y) * area.width + x - area.x) as usize)
        };
        // The cells the named points and the names take.
        let mut taken = vec![false; cols * rows];
        for at in &picture.named {
            let (x, y) = cell(*at);
            if let Some(at) = index(x, y) {
                taken[at] = true;
            }
        }
        let chosen = view.chosen.filter(|at| frame.shows(frame.dot(spot(&layer.points[*at as usize]))));
        let order = chosen.into_iter().chain(picture.named.iter().copied().filter(|at| Some(*at) != chosen));
        if !picture.dense {
            for at in order.take(if picture.named.is_empty() { 1 } else { usize::MAX }) {
                let (x, y) = cell(at);
                let gap = gap(at);
                let name = truncate(&layer.points[at as usize].name, NAME_CELLS.min(area.x + area.width - x - gap));
                let width = text_width(&name);
                // The cells between the point and its name too: a name never runs into another point.
                let free: Option<Vec<usize>> = (x + 1..x + gap + width).map(|column| index(column, y).filter(|at| !taken[*at])).collect();
                let Some(free) = free.filter(|_| width >= 3) else { continue };
                for at in free {
                    taken[at] = true;
                }
                let color = if Some(at) == chosen { palette::BRIGHT } else { palette::DIM };
                screen.put(x + gap, y, &name, &Style { bg: Some(palette::GROUND), ..Style::fg(color) });
            }
        }
        if let Some(at) = chosen {
            let (x, y) = cell(at);
            let color = layer.groups[layer.points[at as usize].group as usize].color;
            screen.put(x, y, "◉", &Style { bold: true, bg: Some(palette::SELECTED), ..Style::fg(color) });
        }
        // The point the mouse rests on: ringed, its name beside it (over whatever was there), ▶ while it plays.
        let hovered = self.hovered.filter(|(tab, _)| *tab == self.tab).map(|(_, at)| at);
        if let Some(at) = hovered.filter(|at| !picture.dense && frame.shows(frame.dot(spot(&layer.points[*at as usize])))) {
            let (x, y) = cell(at);
            let point = &layer.points[at as usize];
            if Some(at) != chosen {
                let color = layer.groups[point.group as usize].color;
                screen.put(x, y, "○", &Style { bold: true, ..Style::fg(color) });
            }
            let playing = if self.from_hover && self.playing == Some((self.tab, at)) { "▶ " } else { "" };
            let gap = gap(at);
            let name = truncate(&format!("{playing}{}", point.name), (NAME_CELLS + 2).min(area.x + area.width - x - gap));
            if text_width(&name) >= 3 {
                screen.put(x + gap, y, &name, &Style { bg: Some(palette::GROUND), ..Style::fg(palette::BRIGHT) });
            }
        }
    }

    /// A console's list: each group, the biggest first, with its count and examples.
    fn draw_list(&self, screen: &mut Screen, layer: &Layer) {
        let (area, view) = (self.area, &self.views[self.tab]);
        let groups = groups_by_size(layer);
        let name_width = groups.iter().map(|at| text_width(&layer.groups[*at].name)).max().unwrap_or(0).min(28);
        let top = list_top(view.row, area.height);
        for (line, at) in groups.iter().enumerate().skip(top).take(area.height as usize) {
            let group = &layer.groups[*at];
            let y = area.y + (line - top) as i32;
            let chosen = line == view.row;
            let ground = if chosen { Some(palette::SELECTED) } else { None };
            if chosen {
                screen.fill(Rect::new(area.x, y, area.width, 1), &Style { bg: ground, ..Style::default() });
            }
            let examples: Vec<&str> = group.examples.iter().map(|point| layer.points[*point as usize].name.as_str()).collect();
            let mut x = screen.put(
                area.x + 1,
                y,
                &truncate(&group.name, name_width),
                &Style { bg: ground, bold: chosen, ..Style::fg(palette::TEXT) },
            );
            x = x.max(area.x + 1 + name_width) + 2;
            let count = grouped(group.count);
            x = screen.put(x + 7 - text_width(&count), y, &count, &Style { bg: ground, ..Style::fg(palette::DIM) }) + 2;
            screen.put(
                x,
                y,
                &truncate(&examples.join(" · "), area.x + area.width - x - 1),
                &Style { bg: ground, ..Style::fg(palette::FAINT) },
            );
        }
    }

    /// The point chosen (its name, what it is, its folder), and under it what it's near or what to ask Kumi.
    fn draw_point(&self, screen: &mut Screen, layer: &Layer) {
        let (y, width) = (screen.height - 3, screen.width - 2);
        let Some(at) = self.current() else { return };
        let point = &layer.points[at as usize];
        let group = &layer.groups[point.group as usize];
        let words = self.graphics == Graphics::Words;
        let mut x = if words { 1 } else { screen.put(1, y, "●", &Style::fg(group.color)) + 1 };
        x = screen.put(x, y, &truncate(&point.name, (width / 2).max(12)), &Style { bold: true, ..Style::fg(palette::BRIGHT) });
        let what = match layer.kind {
            Kind::Sounds => [layer.tags[point.tags as usize].clone(), length(point.seconds as f64)].join(" · "),
            Kind::Songs => group.name.clone(),
        };
        x = screen.put(x + 2, y, &truncate(&what, (width - x).max(0)), &Style::fg(palette::DIM));
        let place = &layer.places[point.place as usize];
        if x + 4 < width {
            x = screen.put(x + 2, y, &truncate(place, width - x - 2), &Style::fg(palette::FAINT));
        }
        if self.playing == Some((self.tab, at)) && x + 12 < width {
            screen.put(x + 2, y, if words { "playing" } else { "▶ playing" }, &Style::fg(palette::ACCENT));
        } else if let Some(said) = self.said.as_ref().filter(|_| x + 12 < width) {
            screen.put(x + 2, y, &truncate(said, width - x - 2), &Style::fg(palette::WARN));
        }
        let y = y + 1;
        if let Some(chosen) = self.menu {
            let mut x = screen.put(1, y, "Ask Kumi:", &Style::fg(palette::FAINT)) + 1;
            for (number, (_, words)) in ACTIONS.iter().enumerate() {
                let style = if number == chosen {
                    Style { bold: true, bg: Some(palette::SELECTED), ..Style::fg(palette::BRIGHT) }
                } else {
                    Style::fg(palette::TEXT)
                };
                x = screen.put(x + 1, y, &format!(" {} {words} ", number + 1), &style);
            }
        } else if self.graphics != Graphics::Words {
            let view = &self.views[self.tab];
            let names: Vec<&str> = view.near.iter().map(|at| layer.points[*at as usize].name.as_str()).collect();
            if !names.is_empty() {
                let x = screen.put(1, y, "Near", &Style::fg(palette::FAINT)) + 2;
                screen.put(x, y, &truncate(&names.join(" · "), width - x), &Style::fg(palette::DIM));
            }
        }
    }

    /// The keys, or the filter being typed (with the cursor).
    fn draw_keys(&self, screen: &mut Screen) -> Option<Cursor> {
        let y = screen.height - 1;
        if let Some(prompt) = &self.prompt {
            let x = screen.put(1, y, "/ ", &Style::fg(palette::ACCENT));
            let end = screen.put(x, y, &truncate(prompt, screen.width - x - 30), &Style::fg(palette::BRIGHT));
            screen.put(end + 2, y, "enter keeps it · esc leaves it", &Style::fg(palette::FAINT));
            return Some(Cursor { x: end, y });
        }
        let other = if self.tab == 0 { "songs" } else { "sounds" };
        let keys = if self.menu.is_some() {
            "← → or 1–3 choose · enter puts it in the box to send · esc goes back".to_string()
        } else if self.graphics == Graphics::Words {
            format!("up and down choose · space plays · enter asks Kumi · / filters · tab {other} · esc closes")
        } else {
            let hover = if self.hover { "plays" } else { "quiet" };
            format!(
                "arrows move · shift+arrows pan · + − zoom · / filters · space plays · h hover {hover} · enter asks Kumi · tab {other} · esc closes"
            )
        };
        screen.put(1, y, &truncate(&keys, screen.width - 2), &Style::fg(palette::FAINT));
        None
    }
}

/// The layer's groups that have points, the biggest first.
fn groups_by_size(layer: &Layer) -> Vec<usize> {
    let mut groups: Vec<usize> = (0..layer.groups.len()).filter(|at| layer.groups[*at].count > 0).collect();
    groups.sort_by_key(|at| std::cmp::Reverse(layer.groups[*at].count));
    groups
}

/// Whether a cell is in the area.
fn inside(area: Rect, x: i32, y: i32) -> bool {
    x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height
}

/// The first row a list shows, so its chosen row shows.
fn list_top(row: usize, height: i32) -> usize {
    row.saturating_sub(height.max(1) as usize - 1)
}

/// "0.8 s", "12 s", "1:05".
fn length(seconds: f64) -> String {
    match seconds {
        s if s < 10. => format!("{:.1} s", s),
        s if s < 60. => format!("{} s", s.round()),
        s => format!("{}:{:02}", (s / 60.).floor(), (s % 60.).floor()),
    }
}

/// Words in the middle of an area, wrapped to fit.
fn centred(screen: &mut Screen, area: Rect, words: &str, style: Style) {
    let lines = super::wrap::wrap(&[Span::styled(words, style)], (area.width - 4).clamp(10, 72));
    let top = area.y + (area.height - lines.len() as i32) / 2;
    for (at, line) in lines.iter().enumerate() {
        let text: String = line.iter().map(|span| span.text.as_str()).collect();
        screen.put(area.x + (area.width - text_width(&text)) / 2, top + at as i32, &text, &style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::Group;

    fn layer(points: &[(f32, f32, u16)]) -> Layer {
        Layer {
            kind: Kind::Sounds,
            points: points
                .iter()
                .enumerate()
                .map(|(at, (x, y, group))| Point {
                    id: at as i64,
                    x: *x,
                    y: *y,
                    group: *group,
                    name: format!("p{at}"),
                    place: 0,
                    tags: 0,
                    path: format!("/s/p{at}.wav"),
                    from: 0.,
                    seconds: 1.,
                })
                .collect(),
            groups: vec![
                Group { name: "kick".into(), color: [255, 0, 0], count: 0, examples: vec![] },
                Group { name: "pad".into(), color: [0, 0, 255], count: 0, examples: vec![] },
            ],
            places: vec!["Samples / Drums".into()],
            tags: vec!["kick one-shot".into()],
            empty: None,
        }
    }

    #[test]
    fn an_arrow_goes_to_the_nearest_point_that_way() {
        // From the middle: one up and a little right, one far right, one right and a little down, one behind.
        let layer = layer(&[(0.5, 0.5, 0), (0.52, 0.7, 0), (0.95, 0.5, 0), (0.58, 0.45, 0), (0.3, 0.5, 0)]);
        let all: Vec<u32> = (0..5).collect();
        assert_eq!(toward(&layer, &all, 0, (1., 0.)), Some(3), "the nearer one right, though a little down");
        assert_eq!(toward(&layer, &all, 0, (0., 1.)), Some(1));
        assert_eq!(toward(&layer, &all, 0, (-1., 0.)), Some(4));
        assert_eq!(toward(&layer, &all, 0, (0., -1.)), Some(3), "down: only the one a little down, within 60°");
        assert_eq!(toward(&layer, &all, 2, (1., 0.)), None, "nothing further right");
        assert_eq!(neighbours(&layer, &all, 0), [3, 4, 1, 2]);
    }

    #[test]
    fn zoomed_out_a_big_map_is_density_and_busier_places_are_brighter() {
        // 4,000 points in one corner, 4 in the other.
        let mut points: Vec<(f32, f32, u16)> =
            (0..4000).map(|n| (0.1 + (n % 40) as f32 / 4000., 0.1 + (n / 40) as f32 / 4000., 0)).collect();
        points.extend([(0.9, 0.9, 1), (0.901, 0.9, 1), (0.9, 0.901, 1), (0.901, 0.901, 1)]);
        let layer = layer(&points);
        let view = View::new(&layer, 1.);
        let frame = Frame::new(Rect::new(0, 0, 80, 30), &view);
        let Drawn { scene: drawn, named, dense, .. } =
            scene(&layer, &Fit::default(), &in_view(&layer, &view.shown, &frame), &frame, 1., None);
        assert!(dense && named.is_empty());
        assert!(drawn.shapes.len() < 100, "a dot a patch, not a point: {}", drawn.shapes.len());
        let brightness = |shape: &Shape| match shape {
            Shape::Dot { color, .. } => color.iter().map(|c| *c as u32).sum::<u32>(),
            _ => 0,
        };
        let (busy, quiet): (Vec<&Shape>, Vec<&Shape>) =
            drawn.shapes.iter().partition(|shape| matches!(shape, Shape::Dot { x, .. } if *x < 0.5));
        assert!(busy.iter().map(|s| brightness(s)).max() > quiet.iter().map(|s| brightness(s)).max());
        assert!(quiet.iter().all(|shape| matches!(shape, Shape::Dot { color, .. } if color[2] > color[0])), "in the pads' blue");
        // Zoomed in on the quiet corner: points, named.
        let mut near = view.clone();
        near.zoom_by(64., (0.9, 0.9));
        let frame = Frame::new(Rect::new(0, 0, 80, 30), &near);
        let Drawn { scene: drawn, named, dense, .. } =
            scene(&layer, &Fit::default(), &in_view(&layer, &near.shown, &frame), &frame, 1., None);
        assert!(!dense);
        assert_eq!((drawn.shapes.len(), named.len()), (4, 4));
    }

    #[test]
    fn the_filter_keeps_what_has_every_word_and_the_choice_moves_to_what_it_keeps() {
        let mut layer = layer(&[(0.1, 0.1, 0), (0.5, 0.5, 1), (0.9, 0.9, 1)]);
        layer.places.push("Splice / Pads".into());
        layer.points[2].place = 1;
        let mut view = View::new(&layer, 1.);
        assert_eq!(view.chosen, Some(1), "the one nearest the middle");
        view.filter_by(&layer, "PAD splice");
        assert_eq!(view.shown, [2]);
        assert_eq!(view.chosen, Some(2));
        view.filter_by(&layer, "kick drums");
        assert_eq!(view.shown, [0, 1], "every point's tags say kick here, and two are in Drums");
        assert_eq!(view.chosen, Some(1), "the kept one nearest");
        view.filter_by(&layer, "nothing");
        assert!(view.shown.is_empty() && view.chosen.is_none());
    }

    #[test]
    fn panned_before_the_map_has_room_to_show_nothing_moves_and_it_shows_once_it_has() {
        let data = MapData { sounds: layer(&[(0.5, 0.5, 0), (0.6, 0.55, 1)]), songs: layer(&[]) };
        let mut map = MapView::new(Graphics::Cells, true);
        map.loaded(ready(data));
        // A window too small for the map: it says so and draws none of it, so it has no area yet.
        let mut screen = Screen::new(30, 10);
        assert!(map.draw(&mut screen, &mut |_, _, _, _| 0).is_none());
        assert_eq!(map.on_text("+"), Asked::Nothing);
        map.on_key("right", true);
        let centre = map.views[0].centre;
        assert!(centre.0.is_finite() && centre.1.is_finite(), "{centre:?}");
        // Big enough: the map shows, its points with it.
        let mut screen = Screen::new(100, 30);
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert!(screen.lines().iter().any(|line| line.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c))), "points drawn");
        // A centre that's no number goes back to the middle.
        map.views[0].centre = (f64::NAN, 0.5);
        map.views[0].clamp();
        assert_eq!(map.views[0].centre, (map.views[0].aspect / 2., 0.5));
    }

    #[test]
    fn zooming_keeps_its_anchor_where_it_was_and_zoomed_out_the_whole_map_shows() {
        let layer = layer(&[(0.2, 0.3, 0)]);
        let mut view = View::new(&layer, 1.);
        let area = Rect::new(0, 0, 100, 40);
        let before = Frame::new(area, &view).dot((0.2, 0.3));
        view.zoom_by(4., (0.2, 0.3));
        let after = Frame::new(area, &view).dot((0.2, 0.3));
        assert!((before.0 - after.0).abs() < 1e-9 && (before.1 - after.1).abs() < 1e-9);
        view.zoom_by(0.1, (0.9, 0.9));
        assert_eq!((view.zoom, view.centre), (1., (0.5, 0.5)));
        assert_eq!(length(0.84), "0.8 s");
        assert_eq!(length(65.), "1:05");
    }

    /// The cell a point is drawn in, the map drawn at its area's size.
    fn cell_of(map: &MapView, at: u32) -> (i32, i32) {
        let (layer, view) = (map.layer().unwrap(), &map.views[map.tab]);
        let (x, y) = Frame::new(map.area, view).dot(spot(&layer.points[at as usize]));
        (map.area.x + (x / 2.) as i32, map.area.y + (y / 4.) as i32)
    }

    #[test]
    fn the_mouse_resting_on_a_point_hears_it_and_leaving_stops_what_that_started() {
        let mut sounds = layer(&[(0.5, 0.5, 0), (0.9, 0.9, 1)]);
        sounds.points[1].seconds = 30.;
        let mut map = MapView::new(Graphics::Cells, true);
        map.loaded(ready(MapData { sounds, songs: layer(&[]) }));
        map.draw(&mut Screen::new(100, 30), &mut |_, _, _, _| 0);
        let (x, y) = cell_of(&map, 0);
        let hover = map.on_move(x, y);
        assert!(matches!(&hover, Asked::Hover { seconds, fade, name, .. } if *seconds == 1. && *fade == 0. && name == "p0"), "{hover:?}");
        assert_eq!(map.on_move(x + 1, y), Asked::Nothing, "still on it");
        map.hover_plays();
        assert_eq!(map.playing, Some((0, 0)));
        let mut screen = Screen::new(100, 30);
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert!(screen.lines().iter().any(|line| line.contains("▶ p0")), "{}", screen.lines().join("\n"));
        // Off it: a hover's play stops.
        assert_eq!(map.on_move(map.area.x, map.area.y), Asked::Leave);
        assert!(map.hover_ends() && map.playing.is_none());
        assert_eq!(map.on_move(map.area.x + 1, map.area.y), Asked::Nothing, "still off it");
        // A long sound: two seconds of it, faded out.
        let (x, y) = cell_of(&map, 1);
        assert!(matches!(map.on_move(x, y), Asked::Hover { seconds, fade, .. } if seconds == PREVIEW && fade == PREVIEW_FADE));
        // Space's play goes on as the mouse leaves.
        assert!(matches!(map.on_text(" "), Asked::Play { .. }));
        assert_eq!(map.on_move(map.area.x, map.area.y), Asked::Leave);
        assert!(!map.hover_ends() && map.playing.is_some());
        // h: hover plays off, the keys say so, and the mouse resting on a point asks nothing; on again.
        assert_eq!(map.on_text("h"), Asked::HoverPlays(false));
        assert_eq!(map.on_move(x, y), Asked::Nothing);
        let mut screen = Screen::new(100, 30);
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert!(screen.lines()[29].contains("h hover quiet"), "{}", screen.lines()[29]);
        assert_eq!(map.on_text("h"), Asked::HoverPlays(true));
        assert!(matches!(map.on_move(x, y), Asked::Hover { .. }));
    }

    #[test]
    fn space_over_a_hovers_preview_plays_the_point_whole_and_tab_or_the_filter_end_a_hover() {
        let mut sounds = layer(&[(0.5, 0.5, 0), (0.9, 0.9, 1)]);
        sounds.points[0].seconds = 30.;
        let mut map = MapView::new(Graphics::Cells, true);
        map.loaded(ready(MapData { sounds, songs: layer(&[]) }));
        map.draw(&mut Screen::new(100, 30), &mut |_, _, _, _| 0);
        // The mouse rests on the point chosen and its preview plays: Space plays it whole, and again stops it.
        let chosen = map.views[0].chosen.unwrap();
        let (x, y) = cell_of(&map, chosen);
        assert!(matches!(map.on_move(x, y), Asked::Hover { .. }));
        assert!(map.hover_plays());
        assert!(matches!(map.on_text(" "), Asked::Play { .. }), "Space plays it whole, not a stop");
        assert_eq!((map.playing, map.from_hover), (Some((0, chosen)), false));
        assert_eq!(map.on_text(" "), Asked::Stop);
        // The mouse coming back to the point Space plays starts no hover of it.
        assert!(matches!(map.on_text(" "), Asked::Play { .. }));
        assert_eq!(map.on_move(map.area.x, map.area.y), Asked::Leave);
        assert!(!map.hover_ends());
        assert_eq!(map.on_move(x, y), Asked::Leave, "Space's point plays on, whole");
        assert_eq!((map.playing, map.from_hover), (Some((0, chosen)), false));
        // Tab while the mouse rests on a point: its hover ends (none about to play, nothing to play when it would).
        map.played(None);
        assert_eq!(map.on_move(map.area.x, map.area.y), Asked::Leave);
        assert!(matches!(map.on_move(x, y), Asked::Hover { .. }));
        assert_eq!(map.on_key("tab", false), Asked::Leave);
        assert!(!map.hover_plays(), "a hover waiting as Tab came plays nothing");
        assert_eq!(map.on_key("tab", false), Asked::Nothing, "no hover on, nothing to end");
        // A preview playing as the filter hides its point: it ends, and the point's ring goes.
        map.draw(&mut Screen::new(100, 30), &mut |_, _, _, _| 0);
        let (x, y) = cell_of(&map, chosen);
        assert!(matches!(map.on_move(x, y), Asked::Hover { .. }));
        assert!(map.hover_plays());
        map.on_text("/p1");
        assert_eq!(map.on_key("enter", false), Asked::Leave);
        assert!(map.hover_ends() && map.hovered.is_none());
    }

    #[test]
    fn over_density_the_mouse_hears_nothing() {
        let points: Vec<(f32, f32, u16)> = (0..4000).map(|n| (0.1 + (n % 40) as f32 / 4000., 0.1 + (n / 40) as f32 / 4000., 0)).collect();
        let mut map = MapView::new(Graphics::Cells, true);
        map.loaded(ready(MapData { sounds: layer(&points), songs: layer(&[]) }));
        map.draw(&mut Screen::new(100, 30), &mut |_, _, _, _| 0);
        let chosen = map.views[0].chosen.unwrap();
        let (x, y) = cell_of(&map, chosen);
        assert_eq!(map.on_move(x, y), Asked::Nothing, "density, not a point");
        // Zoomed in on them: points, which play.
        for _ in 0..12 {
            map.on_text("+");
        }
        map.draw(&mut Screen::new(100, 30), &mut |_, _, _, _| 0);
        let (x, y) = cell_of(&map, chosen);
        assert!(matches!(map.on_move(x, y), Asked::Hover { .. }));
    }

    #[test]
    fn a_map_is_turned_to_run_across_and_fitted_to_its_box_with_its_strays_brought_in() {
        // Points along a rising diagonal: turned to run across, as wide as a map may be against its height.
        let mut points: Vec<(f32, f32, u16)> = (0..200).map(|n| (0.2 + n as f32 * 0.003, 0.2 + n as f32 * 0.002, 0)).collect();
        let mut line = layer(&points);
        let fit = fitted(&mut line);
        assert!((fit.aspect - WIDEST).abs() < 1e-6, "the long side runs across: {}", fit.aspect);
        let (xs, ys): (Vec<f32>, Vec<f32>) = line.points.iter().map(|point| (point.x, point.y)).unzip();
        assert!(xs.iter().all(|x| (0. ..=fit.aspect as f32 + 1e-4).contains(x)) && ys.iter().all(|y| (*y - 0.5).abs() < 1e-4));
        assert!(xs.iter().any(|x| *x < 1e-4) && xs.iter().any(|x| (*x - fit.aspect as f32).abs() < 1e-3), "it fills its box across");
        // One stray far off it: brought in, nearer the line's middle than it was against the line's length.
        points.push((0.95, 0.05, 1));
        let mut layer = layer(&points);
        let fit = fitted(&mut layer);
        assert!(layer.points.iter().all(|point| (0. ..=fit.aspect as f32 + 1e-4).contains(&point.x) && (0. ..=1.).contains(&point.y)));
        let middle = (layer.points[100].x, layer.points[100].y);
        let stray = (layer.points[200].x - middle.0).hypot(layer.points[200].y - middle.1);
        let length = (layer.points[199].x - layer.points[0].x).hypot(layer.points[199].y - layer.points[0].y);
        let (was, was_length) = ((0.95f32 - 0.5).hypot(0.05 - 0.4), (0.597f32).hypot(0.398));
        assert!(stray / length < was / was_length, "{stray} of {length}, against {was} of {was_length}");
    }

    #[test]
    fn points_grow_as_the_map_zooms_in_and_a_stray_draws_smaller_than_a_typical_one() {
        // A grid of points, and one far from the rest.
        let mut points: Vec<(f32, f32, u16)> = (0..900).map(|n| (0.2 + (n % 30) as f32 * 0.02, 0.2 + (n / 30) as f32 * 0.02, 0)).collect();
        points.push((0.97, 0.97, 1));
        let mut layer = layer(&points);
        let fit = fitted(&mut layer);
        let area = Rect::new(0, 0, 120, 40);
        let mut view = View::new(&layer, fit.aspect);
        let mut radii = vec![];
        for _ in 0..8 {
            let frame = Frame::new(area, &view);
            radii.push(grown(&fit, &in_view(&layer, &view.shown, &frame), &frame));
            view.zoom_by(2., spot(&layer.points[465]));
        }
        assert!(radii.windows(2).all(|pair| pair[1] >= pair[0]) && radii[0] < radii[3], "{radii:?}");
        assert_eq!(*radii.last().unwrap(), MOST_RADIUS, "{radii:?}");
        assert!(radii.iter().all(|radius| (0.5..=MOST_RADIUS).contains(radius)));
        // The stray: few have it among their nearest.
        assert!(typical(&fit, 900) < typical(&fit, 465), "{} against {}", typical(&fit, 900), typical(&fit, 465));
        assert!((0.7..=1.7).contains(&typical(&fit, 465)));
    }

    #[test]
    fn the_light_shows_where_points_gather_and_none_far_from_them() {
        let points: Vec<(f32, f32, u16)> = (0..400).map(|n| (0.1 + (n % 20) as f32 * 0.005, 0.8 + (n / 20) as f32 * 0.005, 1)).collect();
        let layer = layer(&points);
        let view = View::new(&layer, 1.);
        let frame = Frame::new(Rect::new(0, 0, 60, 20), &view);
        let lit = light(&layer, &in_view(&layer, &view.shown, &frame), &frame, 60, 20);
        let at = |x: f64, y: f64| lit[((y / 4.) as usize).min(19) * 60 + ((x / 2.) as usize).min(59)];
        let (x, y) = frame.dot((0.15, 0.85));
        let (color, share) = at(x, y).expect("lit where they gather");
        assert!(color[2] > color[0] && share > FAINTEST && share <= LIGHT, "in the pads' blue: {color:?} at {share}");
        let (x, y) = frame.dot((0.9, 0.1));
        assert!(at(x, y).is_none(), "none far from them");
    }

    #[test]
    fn a_glide_eases_to_where_the_map_goes_keeping_the_zooms_anchor_still() {
        let area = Rect::new(0, 0, 100, 30);
        let (from, anchor) = ((2., (0.4, 0.6)), (0.45, 0.62));
        let mut view = View::new(&layer(&[]), 1.);
        (view.zoom, view.centre) = (from.0, from.1);
        let before = Frame::new(area, &view).dot(anchor);
        view.zoom_by(8., anchor);
        let to = (view.zoom, view.centre);
        assert_eq!(between(from, to, 0.), from);
        assert_eq!(between(from, to, 1.), to);
        let mut zooms = vec![];
        for t in [0.1, 0.3, 0.5, 0.8] {
            let (zoom, centre) = between(from, to, t);
            let now = Frame::showing(area, 1., zoom, centre).dot(anchor);
            assert!((now.0 - before.0).abs() < 1e-6 && (now.1 - before.1).abs() < 1e-6, "{t}: {now:?} against {before:?}");
            zooms.push(zoom);
        }
        assert!(zooms.windows(2).all(|pair| pair[0] < pair[1]) && zooms[0] > from.0 && zooms[3] < to.0, "{zooms:?}");
        // A move alone: the centre eases along, the zoom stays.
        let (zoom, centre) = between((4., (0.2, 0.5)), (4., (0.6, 0.5)), 0.5);
        assert!(zoom == 4. && centre.0 > 0.4 && centre.0 < 0.6, "{centre:?}");
    }

    #[test]
    fn a_zoom_glides_where_the_map_is_drawn_in_cells_and_not_where_its_an_image() {
        let sounds = layer(&[(0.3, 0.4, 0), (0.6, 0.55, 1), (0.5, 0.7, 0)]);
        let mut cells = MapView::new(Graphics::Cells, true).gliding(true);
        cells.loaded(ready(MapData { sounds: sounds.clone(), songs: layer(&[]) }));
        cells.draw(&mut Screen::new(100, 30), &mut |_, _, _, _| 0);
        assert!(!cells.moving());
        cells.on_text("+");
        assert!(cells.moving(), "gliding to the new zoom");
        let shown = cells.shown();
        assert!(shown.0 < cells.views[0].zoom, "drawn short of it as it starts: {shown:?}");
        let mut image = MapView::new(Graphics::Kitty, true).gliding(true);
        image.loaded(ready(MapData { sounds, songs: layer(&[]) }));
        image.draw(&mut Screen::new(100, 30), &mut |_, _, _, _| 1);
        image.on_text("+");
        assert!(!image.moving() && image.shown().0 == image.views[0].zoom, "an image goes straight there");
    }
}
