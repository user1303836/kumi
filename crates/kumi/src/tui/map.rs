//! /map: the producer's sounds and songs as two maps, full screen in place of the conversation until it closes. Arrows
//! go to the nearest point that way, shift and the arrows pan, + and − zoom (about the point chosen), a click picks a
//! point, / filters, space plays, a point plays as the mouse comes to it (h turns that off and on), tab switches maps
//! and esc closes. Zoomed out on a big
//! map it draws density, busier places brighter; points, then their names, show as it zooms in. The points are an
//! image where the terminal shows images and braille elsewhere, with the point chosen and the names as text over
//! them; a console that shows neither lists the groups with their counts and examples instead.
//!
//! Each map is turned so its long side runs across the screen and fitted to it. Its points grow as it zooms in (a
//! typical sound a little bigger than the rest), a soft light shows where they gather, and where the map is drawn in
//! cells a zoom or a move glides.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use kumi_design::{Mode, Op};

use super::keymap::Keymap;
use super::pictures::{cells, kitty, scene::Shape, Graphics, Raster, Scene};
use super::render::Cursor;
use super::screen::{Rect, Screen};
use super::style::{palette, Rgb, Style};
use super::width::{text_width, truncate};
use super::wrap::Span;
use crate::map::{gathered, Kind, Layer, MapData, Point};
use kumi_agent::core::palette::{Gathered, MOST_GATHERED};
use kumi_listening::music::tool::grouped;

/// How far in the map zooms at most, and how much each + or − zooms.
const DEEPEST: f64 = 2048.;
const STEP: f64 = 1.5;
/// More points than this in view are drawn as density.
const DENSE: usize = 3000;
/// Names show beside the points when this many are in view, or fewer.
const NAMED: usize = 30;
/// The nearest points named under the map, and what g brings of each point picked.
const NEIGHBOURS: usize = 4;
/// Each point's nearest on the map kept for g: the first NEIGHBOURS of them the filter keeps.
const NEAREST_KEPT: usize = 8;
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
/// From how many points a map's nearest are searched on more than one core.
const PARALLEL_FROM: usize = 20_000;
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
/// A point plays as the mouse comes to it: this much of it (seconds), faded out over this long when that cuts it short.
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
    /// The mouse came to a point (hover plays on): play this stretch of it at once, faded out over `fade` seconds.
    Hover {
        path: String,
        from: f64,
        seconds: f64,
        fade: f64,
        name: String,
    },
    /// The mouse left the point it was on (or came to the one Space plays, or Tab or the filter took that point away): a
    /// hover's play stops.
    Leave,
    /// Whether points play as the mouse comes to them, changed with h: to remember.
    HoverPlays(bool),
    /// These points into the session's palette (Enter: the ones picked, or the one chosen).
    Gather(Vec<Gathered>),
    /// d and a way's key: a sound made from these files (the picks, as many as the way takes).
    Design {
        op: Op,
        mode: Option<Mode>,
        files: Vec<String>,
    },
    /// n: the sounds that sound most like these files (the picks), to pick on the map as well.
    Near(Vec<String>),
    /// A sound dragged to a place of its own, or moved sounds put back (shift+r): every move on the maps now, to keep.
    Moved(Moves),
    /// r, once a sound was made here: another variation of it.
    Reroll,
    /// k: the sound made into the palette.
    Keep,
    /// p: the sound made, played again.
    PlayMade,
    Close,
}

/// The places of the sounds (and songs) dragged to places of their own, by each map's id (`MAP_IDS`) and each one's file:
/// where it was put as the map's data has its places (`Turn::back`), kept between sessions.
pub type Moves = BTreeMap<String, BTreeMap<String, [f64; 2]>>;

/// Each map's id, by its tab, as `Moves` keeps them.
const MAP_IDS: [&str; 2] = ["sounds", "songs"];

/// The most picks more like these starts from, and how many it brings of each.
const MOST_NEAR: usize = 16;
const NEAR_EACH: usize = 4;

/// The sounds made on the map this run that it lists (1 to 9 pick them).
const MOST_MADE: usize = 9;

/// A sound made on the map this run: its number in the app, its name and file, and whether it's picked to make the next
/// one from.
#[derive(Clone, Debug, PartialEq)]
struct MadeSound {
    id: u64,
    name: String,
    path: String,
    picked: bool,
}

/// A left press on the map: its cell, whether it adds to the picks (shift or ctrl held), the point it went down on,
/// which a drag moves, and whether it went down on no point at all (let go there, the picks go).
#[derive(Clone, Copy, Debug)]
struct Press {
    x: i32,
    y: i32,
    adds: bool,
    grabbed: Option<u32>,
    empty: bool,
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
    /// The points picked (a box dragged, x, ctrl and a click, a, g), by their place in the layer.
    picked: BTreeSet<u32>,
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
        let mut view = View {
            filter: String::new(),
            shown: vec![],
            chosen: None,
            near: vec![],
            picked: BTreeSet::new(),
            centre: (aspect / 2., 0.5),
            zoom: 1.,
            aspect,
            row: 0,
        };
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

/// A map as it's drawn: how wide it is against its height, how far each point is from its nearest on it, how typical
/// each is (how many others have it among their NEAREST nearest there, against the median: 0.7 to 1.7), and how its
/// places as read were turned and fitted into its box.
#[derive(Clone, Debug, Default)]
struct Fit {
    aspect: f64,
    apart: Vec<f32>,
    typical: Vec<f32>,
    turn: Turn,
}

/// How a map's places as read became its places as drawn. Similarity's (`fitted_on`): moved to its middle, turned by
/// its main direction's angle, its strays brought in, and scaled into its box. An arrangement's: its places ([0, 1], y
/// down) in a box `aspect` wide, y up. A sound dragged to a place is kept as read, so it lands at the same place among
/// its neighbours however the map is fitted next.
#[derive(Clone, Copy, Debug)]
enum Turn {
    Fitted {
        mean: (f64, f64),
        cos: f64,
        sin: f64,
        /// Past `edge` from the middle a place's distance grows as a log, `ease` of it at a time.
        edge: f64,
        ease: f64,
        low: (f64, f64),
        /// The box's height in places as read (none: every place is one), and how far up a flat map sits in it.
        span: f64,
        lift: f64,
    },
    Boxed {
        aspect: f64,
    },
}

impl Default for Turn {
    fn default() -> Turn {
        Turn::Boxed { aspect: 1. }
    }
}

impl Turn {
    /// A place as read, as the map draws it.
    fn forward(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let (mean, cos, sin, edge, ease, low, span, lift) = match *self {
            Turn::Boxed { aspect } => return (x * aspect, 1. - y),
            Turn::Fitted { mean, cos, sin, edge, ease, low, span, lift } => (mean, cos, sin, edge, ease, low, span, lift),
        };
        if span <= 0. {
            return (0.5, 0.5);
        }
        let (dx, dy) = (x - mean.0, y - mean.1);
        let (mut px, mut py) = (dx * cos + dy * sin, dy * cos - dx * sin);
        let far = px.hypot(py);
        if ease > 0. && far > edge {
            let kept = (edge + ease * ((far - edge) / ease).ln_1p()) / far;
            (px, py) = (px * kept, py * kept);
        }
        ((px - low.0) / span, (py - low.1) / span + lift)
    }

    /// A place as the map draws it, as read: `forward` undone.
    fn back(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let (mean, cos, sin, edge, ease, low, span, lift) = match *self {
            Turn::Boxed { aspect } => return (x / aspect, 1. - y),
            Turn::Fitted { mean, cos, sin, edge, ease, low, span, lift } => (mean, cos, sin, edge, ease, low, span, lift),
        };
        if span <= 0. {
            return mean;
        }
        let (mut px, mut py) = (x * span + low.0, (y - lift) * span + low.1);
        let far = px.hypot(py);
        if ease > 0. && far > edge {
            // Far out past the strays the log's undoing grows fast: a place dragged off the map stays within reach.
            let read = edge + ease * ((far - edge) / ease).min(30.).exp_m1();
            (px, py) = (px * read / far, py * read / far);
        }
        (px * cos - py * sin + mean.0, px * sin + py * cos + mean.1)
    }
}

/// How wide an arrangement's box is against its height: a circle's (keys, length) square, the rest wider, as a screen.
const ARRANGED_ASPECT: f64 = 1.6;

/// One way a map is laid out as it's drawn (by similarity, or one of the sounds' arrangements): its id as `Moves` keeps
/// it ("sounds", "sounds/character"), its arrangement's id ("similarity" for none), each point's place on it (the moves
/// placed), its fit (how far apart and how typical its points are, worked out the first time it shows), the words it
/// writes where they belong, and where it put each point moved, for shift+r.
#[derive(Clone, Debug, Default)]
struct Layout {
    id: String,
    arrangement: &'static str,
    places: Vec<(f32, f32)>,
    fit: Fit,
    labels: Vec<(String, (f64, f64))>,
    home: HashMap<u32, (f32, f32)>,
}

/// Each map's layouts: by similarity (fitted, its points' places in place), then each of its arrangements.
fn laid_out(layer: &mut Layer, id: &str) -> Vec<Layout> {
    let fit = fitted(layer);
    let places = layer.points.iter().map(|point| (point.x, point.y)).collect();
    let mut layouts = vec![Layout { id: id.into(), arrangement: "similarity", places, fit, ..Layout::default() }];
    for (k, arrangement) in layer.arrangements.iter().enumerate() {
        let aspect = if matches!(arrangement.id, "keys" | "length") { 1. } else { ARRANGED_ASPECT };
        let turn = Turn::Boxed { aspect };
        let at = |(x, y): (f32, f32)| {
            let (x, y) = turn.forward((x as f64, y as f64));
            (x as f32, y as f32)
        };
        layouts.push(Layout {
            id: format!("{id}/{}", arrangement.id),
            arrangement: arrangement.id,
            places: layer.points.iter().map(|point| at(point.place(Some(k)))).collect(),
            fit: Fit { aspect, turn, ..Fit::default() },
            labels: arrangement.labels.iter().map(|label| (label.text.clone(), turn.forward((label.x as f64, label.y as f64)))).collect(),
            home: HashMap::new(),
        });
    }
    layouts
}

/// Both maps as they were made ready to draw, kept after /map closes (`MapView::kept`): their data (its colours as read,
/// which the theme draws in its own as they show again), and each map's fit.
pub struct KeptMap {
    data: Rc<MapData>,
    layouts: [Vec<Layout>; 2],
    colors_read: [Vec<Rgb>; 2],
}

/// Both maps made ready to draw: worked out once as /map opens, off Kumi's thread.
pub struct Ready {
    data: MapData,
    layouts: [Vec<Layout>; 2],
}

/// The maps made ready to draw: each turned so its long side runs across (its places' main direction), strays brought
/// in, and fitted to its own box in place of a square (x from 0 to its aspect, y from 0 to 1), with how far each point
/// is from its nearest there and how typical it is.
pub fn ready(mut data: MapData) -> Ready {
    let layouts = [laid_out(&mut data.sounds, MAP_IDS[0]), laid_out(&mut data.songs, MAP_IDS[1])];
    Ready { data, layouts }
}

/// One map turned, its strays brought in and fitted to its box (its points' places changed in place).
fn fitted(layer: &mut Layer) -> Fit {
    fitted_on(layer, None)
}

/// `fitted`, its nearest searched on `threads` (None: as many as a map that big takes).
fn fitted_on(layer: &mut Layer, threads: Option<usize>) -> Fit {
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
    let turn = Turn::Fitted { mean, cos, sin, edge, ease, low, span, lift: if span > 0. { (1. - tall / span) / 2. } else { 0. } };
    let (apart, typical) = closeness(points, aspect, threads);
    Fit { aspect, apart, typical, turn }
}

/// How far each point is from its nearest on a map `aspect` wide, and how typical it is (how many others have it among
/// their NEAREST nearest, against the median: 0.7 to 1.7), searched on `threads` (None: as many as a map that big takes).
fn closeness(points: &[Point], aspect: f64, threads: Option<usize>) -> (Vec<f32>, Vec<f32>) {
    if points.is_empty() {
        return (vec![], vec![]);
    }
    let (apart, named) = match threads {
        Some(threads) => nearest_named_on(points, aspect, threads),
        None => nearest_named(points, aspect),
    };
    let mut sorted = named.clone();
    sorted.sort_unstable();
    let median = sorted[points.len() / 2] as f64;
    let typical = named.iter().map(|named| ((*named as f64 + 1.) / (median + 1.)).sqrt().clamp(0.7, 1.7) as f32).collect();
    (apart, typical)
}

/// Each point's distance to its nearest other on a map `aspect` wide, and how many others have it among their NEAREST
/// nearest: a grid of about two points a cell, searched ring by ring out from each point's cell.
fn nearest_named(points: &[Point], aspect: f64) -> (Vec<f32>, Vec<u32>) {
    // A big map's points are searched on some of the cores: at most half, as a DAW playing beside Kumi keeps the rest.
    let threads =
        if points.len() < PARALLEL_FROM { 1 } else { std::thread::available_parallelism().map_or(1, |cores| (cores.get() / 2).max(1)) };
    nearest_named_on(points, aspect, threads)
}

/// `nearest_named` on `threads` threads, each a run of the points: each point's search is the same wherever it runs, and
/// the counts add up the same, so it gives the same on any number of them.
fn nearest_named_on(points: &[Point], aspect: f64, threads: usize) -> (Vec<f32>, Vec<u32>) {
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
    // Each point's nearest, `from` to `to`: its distance to the nearest, and the counts it adds to those it names.
    let search = |from: usize, to: usize| {
        let mut apart = vec![0f32; to - from];
        let mut named = vec![0u32; count];
        let mut best: Vec<(f64, u32)> = Vec::with_capacity(NEAREST + 1);
        for (at, point) in points.iter().enumerate().take(to).skip(from) {
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
            apart[at - from] = best.first().map_or(0., |(d, _)| d.sqrt() as f32);
            for (_, other) in &best {
                named[*other as usize] += 1;
            }
        }
        (apart, named)
    };
    let runs = threads.clamp(1, count);
    let size = count.div_ceil(runs);
    let parts: Vec<(Vec<f32>, Vec<u32>)> = if runs == 1 {
        vec![search(0, count)]
    } else {
        std::thread::scope(|scope| {
            let searching: Vec<_> = (0..runs)
                .map(|run| {
                    let search = &search;
                    scope.spawn(move || search((run * size).min(count), ((run + 1) * size).min(count)))
                })
                .collect();
            searching.into_iter().map(|run| run.join().expect("a map's search")).collect()
        })
    };
    let mut apart = Vec::with_capacity(count);
    let mut named = vec![0u32; count];
    for (part_apart, part_named) in parts {
        apart.extend(part_apart);
        for (total, part) in named.iter_mut().zip(part_named) {
            *total += part;
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

/// Each point's NEAREST_KEPT nearest others on the map, nearest first (u32::MAX where there are fewer): a grid of about
/// two points a cell over the points' box, searched ring by ring out from each point's cell.
fn nearest_lists(points: &[Point]) -> Vec<u32> {
    let count = points.len();
    let mut out = vec![u32::MAX; count * NEAREST_KEPT];
    if count < 2 {
        return out;
    }
    let (mut low, mut high) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
    for point in points {
        let (x, y) = spot(point);
        (low, high) = ((low.0.min(x), low.1.min(y)), (high.0.max(x), high.1.max(y)));
    }
    let (wide, tall) = ((high.0 - low.0).max(1e-9), (high.1 - low.1).max(1e-9));
    let across = ((count as f64 / 2. * wide / tall).sqrt().ceil() as usize).clamp(1, 4096);
    let down = ((count as f64 / 2. * tall / wide).sqrt().ceil() as usize).clamp(1, 4096);
    let (cell_width, cell_height) = (wide / across as f64, tall / down as f64);
    let cell = |point: &Point| {
        let (x, y) = spot(point);
        ((((x - low.0) / cell_width) as usize).min(across - 1), (((y - low.1) / cell_height) as usize).min(down - 1))
    };
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
    let (step, wanted) = (cell_width.min(cell_height), NEAREST_KEPT.min(count - 1));
    let mut best: Vec<(f64, u32)> = Vec::with_capacity(NEAREST_KEPT + 1);
    for (at, point) in points.iter().enumerate() {
        let ((cx, cy), (px, py)) = (cell(point), spot(point));
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
                        let (qx, qy) = spot(&points[*other as usize]);
                        let d = (qx - px).powi(2) + (qy - py).powi(2);
                        if best.len() < NEAREST_KEPT || d < best[NEAREST_KEPT - 1].0 {
                            let place = best.partition_point(|(seen, _)| *seen <= d);
                            best.insert(place, (d, *other));
                            best.truncate(NEAREST_KEPT);
                        }
                    }
                }
            }
            // Past this ring every point is at least `ring` cells away.
            if best.len() == wanted && (wanted < NEAREST_KEPT || best[wanted - 1].0.sqrt() <= ring as f64 * step) {
                break;
            }
        }
        for (slot, (_, other)) in out[at * NEAREST_KEPT..].iter_mut().zip(&best) {
            *slot = *other;
        }
    }
    out
}

fn mix(under: Rgb, over: Rgb, share: f64) -> Rgb {
    // (between the theme's colours for them: its ground, under a point)
    let theme = super::style::theme();
    let (under, over) = (theme.of(under), theme.of(over));
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

/// How long a change of layout glides.
const MORPH: Duration = Duration::from_millis(450);

/// A change of layout gliding on the sounds' map: each point's place, and the map's aspect, it glides from, since
/// `started`.
#[derive(Clone, Debug)]
struct Morph {
    from: Vec<(f32, f32)>,
    aspect: f64,
    started: Instant,
}

/// The axis words of an arrangement's map (left, right, top, bottom), by its id: the UI's words, not its data's.
fn axes(arrangement: &str) -> [&'static str; 4] {
    match arrangement {
        "character" => ["dark", "bright", "punchy", "soft"],
        "texture" => ["tonal", "noisy", "high", "low"],
        "tempo" => ["slower", "faster", "", ""],
        _ => ["", "", "", ""],
    }
}

/// An arrangement's name as the header and the keys say it.
fn layout_name(arrangement: &str) -> String {
    let mut name = arrangement.to_string();
    if let Some(first) = name.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    name
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
    /// Each map's layouts as they're drawn (by similarity, then the sounds' arrangements), by tab; the one the sounds'
    /// map shows; whether a change of layout glides from one to the other, and the one gliding.
    layouts: [Vec<Layout>; 2],
    laid: usize,
    morphs: bool,
    morph: Option<Morph>,
    tab: usize,
    views: Vec<View>,
    /// The filter being typed.
    prompt: Option<String>,
    /// What plays (the point, on its map), and words for the point's row (a sound that wouldn't play).
    playing: Option<(usize, u32)>,
    said: Option<String>,
    /// Points play as the mouse comes to them (h turns it off and on); the point the mouse is on (on its map); and
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
    /// Each map's classes' or styles' colours as its data has them, which the theme draws in its own (`rethemed`).
    colors_read: [Vec<Rgb>; 2],
    /// Where the map was drawn last: a click is read against it.
    area: Rect,
    /// Whether Enter puts points into the session's palette (the session keeps palettes), and the palette's files (their
    /// paths), marked on the map.
    gathers: bool,
    palette: HashSet<String>,
    /// What a loose palette brings in (their paths), its halo on the map.
    reached: HashSet<String>,
    /// The palette's points on each map, and those it brings in, by their place in its layer.
    marked: [Vec<u32>; 2],
    halo: [Vec<u32>; 2],
    /// Where the left button went down, while that may become a box dragged (or the point it went down on dragged); the
    /// box, corner to corner, once it has; and whether the point has moved.
    pressed: Option<Press>,
    boxed: Option<((i32, i32), (i32, i32))>,
    dragging: bool,
    /// Where the middle button went down and the map's centre then, while it's held: the map follows it.
    panning: Option<((i32, i32), (f64, f64))>,
    /// The sounds dragged to places of their own, kept between sessions (where each layout put each one first is the
    /// layout's, for shift+r).
    moves: Moves,
    /// What putting points into the palette did, said under the map until the next key; and how many were picked when
    /// Enter put in only the first MOST_GATHERED.
    told: Option<String>,
    capped: Option<usize>,
    /// Whether d makes a sound from the picks (this Kumi keeps a folder for them); d was pressed and a way's key is
    /// next; and what the sound made here last is, said under the map (its recipe), which r, k and p work on.
    designs: bool,
    making: bool,
    made: Option<String>,
    /// The sounds made on the map this run, the oldest first (at most MOST_MADE), listed under it to make the next one
    /// from; and where each one's name was drawn in that row (its first and last cell), for a click.
    made_sounds: Vec<MadeSound>,
    made_cells: Vec<(i32, i32)>,
    /// What more like these brought last (picked as well, marked till the picks go), by their place on the sounds' map.
    found: BTreeSet<u32>,
    /// Each point's NEAREST_KEPT nearest on each map (u32::MAX for none), worked out the first time g wants them.
    nearest: [Option<Rc<Vec<u32>>>; 2],
    /// The keys the footer names: Kumi's own, or the app's (`keyed`).
    keymap: Keymap,
}

impl MapView {
    /// The map, before its data is read; `hover`: whether points play as the mouse comes to them.
    pub fn new(graphics: Graphics, hover: bool) -> MapView {
        MapView {
            graphics,
            data: None,
            layouts: Default::default(),
            laid: 0,
            morphs: false,
            morph: None,
            tab: 0,
            views: vec![],
            prompt: None,
            playing: None,
            said: None,
            hover,
            hovered: None,
            from_hover: false,
            gliding: false,
            glide: None,
            version: 0,
            picture: None,
            colors_read: [vec![], vec![]],
            area: Rect::default(),
            gathers: false,
            palette: HashSet::new(),
            reached: HashSet::new(),
            marked: [vec![], vec![]],
            halo: [vec![], vec![]],
            pressed: None,
            boxed: None,
            dragging: false,
            panning: None,
            moves: Moves::new(),
            told: None,
            capped: None,
            designs: false,
            making: false,
            made: None,
            made_sounds: vec![],
            made_cells: vec![],
            found: BTreeSet::new(),
            nearest: [None, None],
            keymap: Keymap::default(),
        }
    }

    /// The same map, its footer naming the keys `keymap` has.
    pub fn keyed(mut self, keymap: &Keymap) -> MapView {
        self.keymap = keymap.clone();
        self
    }

    /// The same map, Enter putting points into the session's palette.
    pub fn gathering(mut self, on: bool) -> MapView {
        self.gathers = on;
        self
    }

    /// The same map, d making a sound from the picks.
    pub fn designing(mut self, on: bool) -> MapView {
        self.designs = on;
        self
    }

    /// What the sound made from the picks is (its recipe), or how its making went, said under the map.
    pub fn made(&mut self, words: Option<String>) {
        self.made = words;
    }

    /// A sound made on the map (its number in the app and its file): listed under the map, to make the next one from.
    /// Past MOST_MADE the oldest one not picked goes.
    pub fn made_sound(&mut self, id: u64, path: String) {
        if self.made_sounds.iter().any(|made| made.id == id) {
            return;
        }
        let name = std::path::Path::new(&path).file_stem().map_or_else(|| path.clone(), |stem| stem.to_string_lossy().into_owned());
        self.made_sounds.push(MadeSound { id, name, path, picked: false });
        if self.made_sounds.len() > MOST_MADE {
            let gone = self.made_sounds.iter().position(|made| !made.picked).unwrap_or(0);
            self.made_sounds.remove(gone);
        }
    }

    /// How many sounds the next make is from: the sounds made that are picked, and the sounds picked on the map (on its
    /// sounds' map).
    fn sources(&self) -> usize {
        let picked = if self.tab == 0 { self.views.first().map_or(0, |view| view.picked.len()) } else { 0 };
        self.made_sounds.iter().filter(|made| made.picked).count() + picked
    }

    /// d: the picked sounds (two or more: sounds made, and picks on the sounds' map) wait for a way's key.
    fn design_menu(&mut self) -> Asked {
        let made = self.made_sounds.iter().any(|made| made.picked);
        if self.tab != 0 && !made {
            self.told = Some("Sounds are made from sounds: tab goes to them.".into());
        } else if self.sources() < kumi_design::MIN_SOURCES {
            self.told = Some("Pick two or more sounds to make one from them (x, drag a box, or a sound made's number).".into());
        } else {
            self.making = true;
        }
        Asked::Nothing
    }

    /// The files to make from, every one: the sounds made that are picked, then the sounds picked on the map.
    fn picked_files(&self) -> Vec<String> {
        let made = self.made_sounds.iter().filter(|made| made.picked).map(|made| made.path.clone());
        let picked: Vec<String> = match self.data.as_ref().filter(|_| self.tab == 0) {
            Some(data) => self.views[0].picked.iter().map(|at| data.sounds.points[*at as usize].path.clone()).collect(),
            None => vec![],
        };
        made.chain(picked).collect()
    }

    /// n: more like the sounds picked (the first MOST_NEAR; none picked, the one chosen), from the library.
    fn near(&mut self) -> Asked {
        let Some(data) = self.data.clone() else { return Asked::Nothing };
        if self.tab != 0 {
            self.told = Some("More like these finds sounds: tab goes to them.".into());
            return Asked::Nothing;
        }
        let view = &self.views[0];
        let from: Vec<u32> = if view.picked.is_empty() { view.chosen.into_iter().collect() } else { view.picked.iter().copied().collect() };
        if from.is_empty() {
            return Asked::Nothing;
        }
        self.told = Some(format!("Finding more like {}…", if from.len() == 1 { "this".to_string() } else { format!("these {}", grouped(from.len())) }));
        Asked::Near(from.into_iter().take(MOST_NEAR).map(|at| data.sounds.points[at as usize].path.clone()).collect())
    }

    /// The limit more like these asks for, from `count` files.
    pub fn near_limit(count: usize) -> u32 {
        (count.clamp(1, MOST_NEAR) * NEAR_EACH).max(8) as u32
    }

    /// What more like these found (their files), picked on the sounds' map as well as what was, and marked; or why it
    /// found none.
    pub fn near_found(&mut self, found: Result<Vec<String>, String>) {
        let Some(data) = self.data.clone() else { return };
        let paths = match found {
            Ok(paths) => paths,
            Err(why) => {
                self.told = Some(why);
                return;
            }
        };
        let wanted: HashSet<&str> = paths.iter().map(String::as_str).collect();
        let view = &mut self.views[0];
        self.found = (0..data.sounds.points.len() as u32)
            .filter(|at| wanted.contains(data.sounds.points[*at as usize].path.as_str()) && view.shown.binary_search(at).is_ok())
            .collect();
        view.picked.extend(self.found.iter().copied());
        self.told = Some(match self.found.len() {
            0 => "Nothing more like these on the map as it's filtered.".to_string(),
            1 => "Picked 1 more like these (✦): space plays the one chosen, enter adds them.".to_string(),
            count => format!("Picked {} more like these (✦): space plays the one chosen, enter adds them.", grouped(count)),
        });
        self.version += 1;
    }

    /// How many are picked: on the map on show, and of the sounds made.
    fn picks(&self) -> usize {
        self.views.get(self.tab).map_or(0, |view| view.picked.len()) + self.made_sounds.iter().filter(|made| made.picked).count()
    }

    /// Every pick let go: on the map on show, and of the sounds made.
    fn let_go_picks(&mut self) {
        if let Some(view) = self.views.get_mut(self.tab) {
            view.picked.clear();
        }
        self.found.clear();
        for made in &mut self.made_sounds {
            made.picked = false;
        }
        self.version += 1;
    }

    /// A sound made picked to make the next one from, or let go (its number in the list, from 1).
    fn toggle_made(&mut self, number: usize) -> bool {
        let Some(made) = number.checked_sub(1).and_then(|at| self.made_sounds.get_mut(at)) else { return false };
        made.picked = !made.picked;
        true
    }

    /// The session's palette's files, and what a loose one brings in (by their paths), marked on the map.
    pub fn in_palette(&mut self, paths: HashSet<String>, reached: HashSet<String>) {
        (self.palette, self.reached) = (paths, reached);
        self.mark_palette();
    }

    /// Which points are the palette's, and which it brings in, on each map: worked out once, not each frame.
    fn mark_palette(&mut self) {
        let Some(data) = self.data.clone() else { return };
        let on = |layer: &Layer, paths: &HashSet<String>| -> Vec<u32> {
            if paths.is_empty() {
                return vec![];
            }
            (0..layer.points.len() as u32).filter(|at| paths.contains(&layer.points[*at as usize].path)).collect()
        };
        self.marked = [on(&data.sounds, &self.palette), on(&data.songs, &self.palette)];
        self.halo = [on(&data.sounds, &self.reached), on(&data.songs, &self.reached)];
    }

    /// What putting points into the palette did, said under the map (with how many were left out, when Enter put in only
    /// the first MOST_GATHERED).
    pub fn told(&mut self, words: String) {
        self.told = Some(match self.capped.take() {
            Some(picked) => format!("{words} The first {} of {} picked went in.", grouped(MOST_GATHERED), grouped(picked)),
            None => words,
        });
    }

    /// Enter: the points picked (or, with none, the one chosen) into the session's palette, and the picks let go.
    fn gather(&mut self) -> Asked {
        if !self.gathers {
            return Asked::Nothing;
        }
        let Some(data) = self.data.clone() else { return Asked::Nothing };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        let picked = std::mem::take(&mut self.views[self.tab].picked);
        let points: Vec<u32> = if picked.is_empty() { self.current().into_iter().collect() } else { picked.into_iter().collect() };
        if points.is_empty() {
            return Asked::Nothing;
        }
        self.version += 1;
        // A palette is what to work with: as the browser's map, at most MOST_GATHERED at once, and told so.
        self.capped = (points.len() > MOST_GATHERED).then_some(points.len());
        Asked::Gather(points.into_iter().take(MOST_GATHERED).map(|at| gathered(layer, at)).collect())
    }

    /// A point picked, or let go if it was.
    fn toggle(&mut self, at: u32) {
        let picked = &mut self.views[self.tab].picked;
        if !picked.remove(&at) {
            picked.insert(at);
        }
        self.version += 1;
    }

    /// Every point the filter keeps, picked.
    fn pick_all(&mut self) {
        let view = &mut self.views[self.tab];
        view.picked.extend(view.shown.iter().copied());
        self.version += 1;
    }

    /// Each point picked brings its NEIGHBOURS nearest on the map that the filter keeps (of its NEAREST_KEPT nearest,
    /// worked out once a map, so a big pick grows in a moment).
    fn grow(&mut self) {
        let Some(data) = self.data.clone() else { return };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        let nearest = self.nearest[self.tab].get_or_insert_with(|| Rc::new(nearest_lists(&layer.points))).clone();
        let view = &mut self.views[self.tab];
        let near: Vec<u32> = view
            .picked
            .iter()
            .flat_map(|at| {
                let row = &nearest[*at as usize * NEAREST_KEPT..(*at as usize + 1) * NEAREST_KEPT];
                row.iter().copied().filter(|other| *other != u32::MAX && view.shown.binary_search(other).is_ok()).take(NEIGHBOURS)
            })
            .collect();
        view.picked.extend(near);
        self.version += 1;
    }

    /// The left button held and moved: a point it went down on goes with it; else, once off the cell it went down on, a
    /// box is dragged from there.
    pub fn on_drag(&mut self, x: i32, y: i32) -> Asked {
        let Some(press) = self.pressed.filter(|_| self.graphics != Graphics::Words) else { return Asked::Nothing };
        let area = self.area;
        let clamped = (x.clamp(area.x, area.x + area.width - 1), y.clamp(area.y, area.y + area.height - 1));
        if let Some(at) = press.grabbed {
            if self.dragging || (x, y) != (press.x, press.y) {
                self.dragging = true;
                self.move_to(at, clamped);
            }
        } else if self.boxed.is_some() || (x, y) != (press.x, press.y) {
            self.boxed = Some(((press.x, press.y), clamped));
        }
        Asked::Nothing
    }

    /// The middle button went down on the map: while it's held, the map follows the mouse (`on_pan`).
    pub fn on_middle(&mut self, x: i32, y: i32) -> Asked {
        self.told = None;
        if self.graphics != Graphics::Words && self.data.is_some() && self.prompt.is_none() && inside(self.area, x, y) {
            self.glide = None;
            self.panning = Some(((x, y), self.views[self.tab].centre));
        }
        Asked::Nothing
    }

    /// The middle button held and moved: the map moves with the mouse, picking nothing.
    pub fn on_pan(&mut self, x: i32, y: i32) -> Asked {
        let Some(((from_x, from_y), centre)) = self.panning else { return Asked::Nothing };
        let (area, view) = (self.area, &mut self.views[self.tab]);
        let scale = Frame::new(area, view).scale;
        view.centre = (centre.0 - ((x - from_x) * 2) as f64 / scale, centre.1 + ((y - from_y) * 4) as f64 / scale);
        view.clamp();
        self.version += 1;
        Asked::Nothing
    }

    /// A button let go: a point dragged stays where it was let go (and is kept there); a box dragged picks what's in it
    /// (in place of what was picked; with shift or ctrl, as well).
    pub fn on_release(&mut self, _x: i32, _y: i32) -> Asked {
        if self.panning.take().is_some() {
            return Asked::Nothing;
        }
        let (pressed, boxed, dragged) = (self.pressed.take(), self.boxed.take(), std::mem::take(&mut self.dragging));
        if let Some(at) = pressed.and_then(|press| press.grabbed).filter(|_| dragged) {
            return self.keep_move(at);
        }
        // A click on no point, without a drag: the picks go.
        if let (Some(press), None) = (pressed, boxed) {
            if press.empty && !press.adds {
                self.let_go_picks();
            }
            return Asked::Nothing;
        }
        let (Some(press), Some(((x0, y0), (x1, y1)))) = (pressed, boxed) else { return Asked::Nothing };
        let Some(data) = self.data.clone() else { return Asked::Nothing };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        let (area, view) = (self.area, &mut self.views[self.tab]);
        let frame = Frame::new(area, view);
        // The box's corners on the map: from the first cell's top left to the last one's bottom right.
        let corner = |x: i32, y: i32| frame.at((((x - area.x) * 2) as f64, ((y - area.y) * 4) as f64));
        let (a, b) = (corner(x0.min(x1), y0.min(y1)), corner(x0.max(x1) + 1, y0.max(y1) + 1));
        let (left, right, low, high) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
        if !press.adds {
            view.picked.clear();
        }
        view.picked.extend(view.shown.iter().copied().filter(|at| {
            let (x, y) = spot(&layer.points[*at as usize]);
            x >= left && x <= right && y >= low && y <= high
        }));
        self.version += 1;
        Asked::Nothing
    }

    /// The point a press went down on, which a drag would move: the one drawn in that cell, or well inside its disc.
    /// None over density, or on a console's list.
    fn grabbed(&self, x: i32, y: i32) -> Option<u32> {
        let picture = self.picture.as_ref().filter(|picture| !picture.dense)?;
        let at = self.under(x, y)?;
        let (layer, view, fit) = (self.layer()?, self.views.get(self.tab)?, self.fit(self.tab)?);
        let (area, frame) = (self.area, Frame::new(self.area, view));
        let (px, py) = frame.dot(spot(&layer.points[at as usize]));
        let dot = (((x - area.x) * 2 + 1) as f64, ((y - area.y) * 4 + 2) as f64);
        let cell = (area.x + (px / 2.) as i32, area.y + (py / 4.) as i32);
        let well_inside = (px - dot.0).hypot(py - dot.1) <= radius_of(fit, at, picture.radius, frame.scale) - 1.5;
        (cell == (x, y) || well_inside).then_some(at)
    }

    /// A point dragged to a cell of the map: drawn there at once (where the map first put it remembered).
    fn move_to(&mut self, at: u32, (x, y): (i32, i32)) {
        let (area, tab) = (self.area, self.tab);
        let Some(view) = self.views.get(tab) else { return };
        let place = Frame::new(area, view).at((((x - area.x) * 2 + 1) as f64, ((y - area.y) * 4 + 2) as f64));
        let place = (place.0 as f32, place.1 as f32);
        if let Some(layout) = self.layout_mut(tab) {
            let was = layout.places[at as usize];
            layout.home.entry(at).or_insert(was);
            layout.places[at as usize] = place;
        }
        let Some(data) = self.data.as_mut() else { return };
        let data = Rc::make_mut(data);
        let layer = if tab == 0 { &mut data.sounds } else { &mut data.songs };
        let point = &mut layer.points[at as usize];
        (point.x, point.y) = place;
        self.version += 1;
    }

    /// A point let go where it was dragged: its place kept by its file, as the map's data has places, and the point
    /// chosen's nearest and each one's nearest for g worked out again.
    fn keep_move(&mut self, at: u32) -> Asked {
        let (Some(data), Some(layout)) = (self.data.clone(), self.layout(self.tab)) else { return Asked::Nothing };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        let point = &layer.points[at as usize];
        let (x, y) = layout.fit.turn.back(spot(point));
        self.moves.entry(layout.id.clone()).or_default().insert(point.path.clone(), [x, y]);
        self.nearest[self.tab] = None;
        let view = &mut self.views[self.tab];
        view.choose(layer, view.chosen);
        Asked::Moved(self.moves.clone())
    }

    /// shift+r: the moved points picked (none picked: every one moved on this map) back where the map put them.
    fn unmove(&mut self) -> Asked {
        let tab = self.tab;
        let picked = &self.views[tab].picked;
        let Some(layout) = self.layout(tab) else { return Asked::Nothing };
        let back: Vec<u32> = layout.home.keys().copied().filter(|at| picked.is_empty() || picked.contains(at)).collect();
        if back.is_empty() {
            self.told = Some(if picked.is_empty() { "Nothing on this map was moved." } else { "None of the points picked was moved." }.into());
            return Asked::Nothing;
        }
        let laid = if tab == 0 { self.laid } else { 0 };
        let (Some(data), Some(layout)) = (self.data.as_mut(), self.layouts[tab].get_mut(laid)) else { return Asked::Nothing };
        let layer = if tab == 0 { &mut Rc::make_mut(data).sounds } else { &mut Rc::make_mut(data).songs };
        let moved = self.moves.entry(layout.id.clone()).or_default();
        for at in &back {
            let point = &mut layer.points[*at as usize];
            if let Some(home) = layout.home.remove(at) {
                (point.x, point.y) = home;
                layout.places[*at as usize] = home;
            }
            moved.remove(&point.path);
        }
        if moved.is_empty() {
            self.moves.remove(&layout.id);
        }
        self.told = Some(match back.len() {
            1 => "1 point is back where the map put it.".to_string(),
            count => format!("{} points are back where the map put them.", grouped(count)),
        });
        let layer = if tab == 0 { &data.sounds } else { &data.songs };
        self.nearest[tab] = None;
        let view = &mut self.views[tab];
        view.choose(layer, view.chosen);
        self.version += 1;
        Asked::Moved(self.moves.clone())
    }

    /// The same map with the sounds dragged to places of their own (kept from before) there.
    pub fn moved(mut self, moves: Moves) -> MapView {
        self.moves = moves;
        self
    }

    /// The moves kept, each point moved placed where it was put on each layout (where the layout put it remembered, for
    /// shift+r), and each map's points where its layout by similarity has them.
    fn place_moved(&mut self) {
        let Some(data) = self.data.as_mut() else { return };
        let data = Rc::make_mut(data);
        for (layer, layouts) in [&mut data.sounds, &mut data.songs].into_iter().zip(self.layouts.iter_mut()) {
            for layout in layouts.iter_mut() {
                let Some(moved) = self.moves.get(&layout.id) else { continue };
                for (at, point) in layer.points.iter().enumerate() {
                    if let Some([x, y]) = moved.get(&point.path) {
                        let was = layout.places[at];
                        layout.home.entry(at as u32).or_insert(was);
                        let (x, y) = layout.fit.turn.forward((*x, *y));
                        layout.places[at] = (x as f32, y as f32);
                    }
                }
            }
            if let Some(similarity) = layouts.first() {
                for (point, (x, y)) in layer.points.iter_mut().zip(&similarity.places) {
                    (point.x, point.y) = (*x, *y);
                }
            }
        }
    }

    /// The same map with a change of layout gliding from one to the other.
    pub fn morphing(mut self, on: bool) -> MapView {
        self.morphs = on;
        self
    }

    /// l (shift+l: back): the sounds' map laid out the next way (by similarity, then each of its arrangements), its
    /// points gliding there where layouts glide, the map zoomed out.
    fn arrange(&mut self, by: isize) -> Asked {
        let count = self.layouts[0].len();
        if self.tab != 0 || count < 2 {
            self.told = Some("Only the sounds' map has other layouts.".into());
            return Asked::Nothing;
        }
        let from = self.layouts[0][self.laid].places.clone();
        let aspect = self.views[0].aspect;
        self.laid = (self.laid as isize + by).rem_euclid(count as isize) as usize;
        let view = &mut self.views[0];
        (view.aspect, view.zoom, view.centre) = (self.layouts[0][self.laid].fit.aspect, 1., (self.layouts[0][self.laid].fit.aspect / 2., 0.5));
        self.glide = None;
        self.morph = Some(Morph { from, aspect, started: Instant::now() });
        if !self.morphs {
            self.morph_step(true);
        }
        let hovering = self.hovered.take().is_some() || self.from_hover;
        if hovering { Asked::Leave } else { Asked::Nothing }
    }

    /// A change of layout gliding: each point (and the map's aspect) eased along from where it was to its place on the
    /// layout shown (`land`: there at once). Once there, the layout's fit is worked out if it never was, and the point
    /// chosen's nearest again.
    fn morph_step(&mut self, land: bool) {
        let Some(morph) = self.morph.as_ref() else { return };
        let t = if land { 1. } else { (morph.started.elapsed().as_secs_f64() / MORPH.as_secs_f64()).min(1.) };
        let eased = 1. - (1. - t).powi(3);
        let (Some(data), Some(layout)) = (self.data.as_mut(), self.layouts[0].get(self.laid)) else { return };
        let layer = &mut Rc::make_mut(data).sounds;
        for ((point, from), to) in layer.points.iter_mut().zip(&morph.from).zip(&layout.places) {
            point.x = from.0 + (to.0 - from.0) * eased as f32;
            point.y = from.1 + (to.1 - from.1) * eased as f32;
        }
        let aspect = morph.aspect + (layout.fit.aspect - morph.aspect) * eased;
        let view = &mut self.views[0];
        (view.aspect, view.centre) = (aspect, (aspect / 2., 0.5));
        self.version += 1;
        if t < 1. {
            return;
        }
        self.morph = None;
        let layout = &mut self.layouts[0][self.laid];
        if layout.fit.apart.is_empty() {
            (layout.fit.apart, layout.fit.typical) = closeness(&layer.points, layout.fit.aspect, None);
        }
        self.nearest[0] = None;
        let Some(data) = self.data.clone() else { return };
        let view = &mut self.views[0];
        view.choose(&data.sounds, view.chosen);
    }

    /// The same map with its zooms and moves gliding where it's drawn in cells (an image would go up again each step).
    pub fn gliding(mut self, on: bool) -> MapView {
        self.gliding = on && self.graphics == Graphics::Cells;
        self
    }

    /// The map's data, read and made ready to draw: each map zoomed out with the point nearest its middle chosen. The
    /// songs' map comes first when there are no sounds on one yet.
    pub fn loaded(&mut self, ready: Ready) {
        let Ready { data, layouts } = ready;
        // The classes' and styles' colours as read, for each theme's own (`rethemed`).
        let colors_read = [&data.sounds, &data.songs].map(|layer| layer.groups.iter().map(|group| group.color).collect());
        self.shown_from(KeptMap { data: Rc::new(data), layouts, colors_read });
    }

    /// A map kept from the last time /map was open, still the same (`KeptMap`): shown as it was first shown, without
    /// reading or fitting it again.
    pub fn reloaded(&mut self, kept: KeptMap) {
        self.shown_from(kept);
    }

    fn shown_from(&mut self, kept: KeptMap) {
        let KeptMap { data, layouts, colors_read } = kept;
        (self.colors_read, self.layouts, self.laid, self.morph) = (colors_read, layouts, 0, None);
        self.data = Some(data);
        self.place_moved();
        let data = self.data.clone().expect("given");
        let aspect = |tab: usize| self.layouts[tab].first().map_or(1., |layout| layout.fit.aspect);
        self.views = vec![View::new(&data.sounds, aspect(0)), View::new(&data.songs, aspect(1))];
        self.tab = usize::from(data.sounds.points.is_empty() && !data.songs.points.is_empty());
        self.version += 1;
        self.mark_palette();
        self.rethemed();
    }

    /// What to keep of the map as it closes, for /map again while what it was read from stays the same: its data, its
    /// fits and its colours as read. None when it never came.
    pub fn kept(self) -> Option<KeptMap> {
        Some(KeptMap { data: self.data?, layouts: self.layouts, colors_read: self.colors_read })
    }

    /// Each class's and style's colour as it reads on the theme's ground (on light, one too pale deepened), from the
    /// colours as read: as the map's data comes, and again as the theme changes while it's open.
    pub fn rethemed(&mut self) {
        let Some(data) = self.data.as_mut() else { return };
        let data = Rc::make_mut(data);
        let theme = super::style::theme();
        for (layer, read) in [&mut data.sounds, &mut data.songs].into_iter().zip(&self.colors_read) {
            for (group, color) in layer.groups.iter_mut().zip(read) {
                group.color = theme.legible(*color);
            }
        }
        self.picture = None;
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

    /// Whether the map is still gliding (a zoom, a move, a change of layout): it's drawn again soon.
    pub fn moving(&self) -> bool {
        self.glide.is_some_and(|glide| glide.tab == self.tab && glide.started.elapsed() < GLIDE) || self.morph.is_some()
    }

    /// The layout a map shows (by its tab).
    fn layout(&self, tab: usize) -> Option<&Layout> {
        self.layouts.get(tab).and_then(|layouts| layouts.get(if tab == 0 { self.laid } else { 0 }))
    }

    fn layout_mut(&mut self, tab: usize) -> Option<&mut Layout> {
        let laid = if tab == 0 { self.laid } else { 0 };
        self.layouts.get_mut(tab).and_then(|layouts| layouts.get_mut(laid))
    }

    /// How the layout a map shows is drawn (by its tab).
    fn fit(&self, tab: usize) -> Option<&Fit> {
        self.layout(tab).map(|layout| &layout.fit)
    }

    fn layer(&self) -> Option<&Layer> {
        self.data.as_ref().map(|data| if self.tab == 0 { &data.sounds } else { &data.songs })
    }

    /// The point Space plays: the one chosen, or in a list the chosen group's first example.
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

    /// The mouse left the point it was on, or hover plays were turned off: whether a hover's play was on, which stops.
    pub fn hover_ends(&mut self) -> bool {
        let was = self.from_hover && self.playing.is_some();
        if was {
            self.playing = None;
        }
        self.from_hover = false;
        was
    }

    /// While its filter is typed: what's typed goes into it, and backspace, enter and esc are the filter's
    /// (`Scope::MapFilter`).
    pub fn filtering(&self) -> bool {
        self.prompt.is_some()
    }

    /// A character typed into the filter (a pasted line break, or a tab, is a space in its words).
    pub fn type_filter(&mut self, character: char) {
        self.told = None;
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.push(if character.is_control() { ' ' } else { character });
        }
    }

    /// Does what the first of a key's actions that applies does (`ids`, as the key registry has them in the map or its
    /// filter); one that doesn't apply just now lets the key go on to the next.
    pub fn act(&mut self, ids: Vec<&'static str>) -> Asked {
        self.told = None;
        let picks = self.data.is_some() && self.graphics != Graphics::Words;
        // A way's key comes right after d, or d is let go.
        let making = std::mem::take(&mut self.making);
        for id in ids {
            let (way, pan) = match id {
                "map.let-go" if making => return Asked::Nothing,
                "map.design" if picks && self.designs => return self.design_menu(),
                id if making && id.starts_with("map.make-") => {
                    let Some((op, mode)) = way_of(id) else { continue };
                    let files = self.picked_files();
                    return Asked::Design { op, mode, files };
                }
                id if MADE_KEYS.contains(&id) => {
                    let number = MADE_KEYS.iter().position(|key| *key == id).map_or(0, |at| at + 1);
                    if !self.toggle_made(number) {
                        continue;
                    }
                    return Asked::Nothing;
                }
                "map.reroll" if !making && self.made.is_some() => return Asked::Reroll,
                "map.keep" if !making && self.made.is_some() => return Asked::Keep,
                "map.again" if self.made.is_some() => return Asked::PlayMade,
                "map.filter-erase" => {
                    let Some(prompt) = self.prompt.as_mut() else { continue };
                    prompt.pop();
                    return Asked::Nothing;
                }
                "map.filter-keep" => {
                    let Some(filter) = self.prompt.take() else { continue };
                    self.refilter(&filter);
                    // The point the mouse is on, filtered out: its ring goes, and its hover ends.
                    if self.hovered.is_some_and(|(tab, at)| !self.views[tab].shown.contains(&at)) {
                        self.hovered = None;
                        return Asked::Leave;
                    }
                    return Asked::Nothing;
                }
                "map.filter-drop" => {
                    if self.prompt.take().is_none() {
                        continue;
                    }
                    return Asked::Nothing;
                }
                // Esc lets a box being dragged go (a point dragged stays where it is till the button's let go), then the
                // points picked, then closes the map.
                "map.let-go" if self.boxed.is_some() || self.pressed.is_some_and(|press| press.grabbed.is_none()) => {
                    (self.pressed, self.boxed) = (None, None);
                    return Asked::Nothing;
                }
                "map.unmove" if picks => return self.unmove(),
                "map.near" if picks => return self.near(),
                "map.arrange" if picks => return self.arrange(1),
                "map.arrange-back" if picks => return self.arrange(-1),
                "map.let-go" if self.picks() > 0 => {
                    self.let_go_picks();
                    return Asked::Nothing;
                }
                "map.close" => return Asked::Close,
                "map.gather" => return self.gather(),
                "map.play" => return self.play(),
                "map.switch" if self.data.is_some() => {
                    self.tab = 1 - self.tab;
                    // A hover on the map left behind ends: what it plays stops.
                    let hovering = self.hovered.is_some() || self.from_hover;
                    (self.said, self.hovered, self.version) = (None, None, self.version + 1);
                    return if hovering { Asked::Leave } else { Asked::Nothing };
                }
                "map.zoom-in" => {
                    self.zoom_by(STEP, None);
                    return Asked::Nothing;
                }
                "map.zoom-out" => {
                    self.zoom_by(1. / STEP, None);
                    return Asked::Nothing;
                }
                "map.filter" if self.data.is_some() => {
                    self.prompt = Some(String::new());
                    return Asked::Nothing;
                }
                // Picking: the point chosen picked or let go, every point the filter keeps, each picked one's nearest.
                "map.pick" if picks => {
                    if let Some(at) = self.views[self.tab].chosen {
                        self.toggle(at);
                    }
                    return Asked::Nothing;
                }
                "map.pick-all" if picks => {
                    self.pick_all();
                    return Asked::Nothing;
                }
                "map.grow" if picks => {
                    self.grow();
                    return Asked::Nothing;
                }
                "map.hover" if self.data.is_some() => {
                    self.hover = !self.hover;
                    if !self.hover {
                        self.hovered = None;
                    }
                    return Asked::HoverPlays(self.hover);
                }
                "map.up" => ((0., 1.), false),
                "map.down" => ((0., -1.), false),
                "map.left" => ((-1., 0.), false),
                "map.right" => ((1., 0.), false),
                "map.pan-up" => ((0., 1.), true),
                "map.pan-down" => ((0., -1.), true),
                "map.pan-left" => ((-1., 0.), true),
                "map.pan-right" => ((1., 0.), true),
                _ => continue,
            };
            if self.graphics == Graphics::Words {
                self.list_by(-way.1 as isize);
            } else if pan {
                self.pan(way);
            } else {
                self.go(way);
            }
            return Asked::Nothing;
        }
        Asked::Nothing
    }

    /// A key with Kumi's own keys, as the app hands it on (`act`): for tests.
    #[doc(hidden)]
    pub fn on_key(&mut self, name: &str, shift: bool) -> Asked {
        use super::{keymap, keys::Modifiers};
        let scope = if self.filtering() { keymap::Scope::MapFilter } else { keymap::Scope::Map };
        let chord = keymap::Chord::new(name, Modifiers { shift, ..Modifiers::NONE });
        self.act(keymap::Keymap::default().actions(scope, &chord))
    }

    /// Typed characters with Kumi's own keys, one at a time, as the app hands them on: for tests.
    #[doc(hidden)]
    pub fn on_text(&mut self, text: &str) -> Asked {
        use super::{keymap, keys::InputEvent};
        for typed in text.chars() {
            if self.filtering() {
                self.type_filter(typed);
                continue;
            }
            let chord = keymap::Chord::of(&InputEvent::text(&typed.to_string())).expect("one character");
            let asked = self.act(keymap::Keymap::default().actions(keymap::Scope::Map, &chord));
            if asked != Asked::Nothing {
                return asked;
            }
        }
        Asked::Nothing
    }

    /// A click chooses the point nearest it (or the group of a list's row); with shift or ctrl (`adds`) it picks it, or
    /// lets it go. The button held and moved from there drags the point it went down on, or a box (`on_drag`).
    pub fn on_click(&mut self, x: i32, y: i32, adds: bool) -> Asked {
        self.told = None;
        let area = self.area;
        // A sound made, clicked in its row under the map: picked to make the next one from, or let go.
        if self.prompt.is_none() && !self.made_cells.is_empty() && y == area.y + area.height {
            if let Some(at) = self.made_cells.iter().position(|(first, last)| (*first..=*last).contains(&x)) {
                self.toggle_made(at + 1);
            }
            return Asked::Nothing;
        }
        if self.prompt.is_some() || !inside(area, x, y) {
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
        if let Some(at) = picked.filter(|_| adds) {
            self.toggle(at);
        }
        if picked.is_some() {
            self.views[self.tab].choose(layer, picked);
        }
        let grabbed = if adds { None } else { self.grabbed(x, y) };
        (self.pressed, self.dragging) = (Some(Press { x, y, adds, grabbed, empty: picked.is_none() }), false);
        Asked::Nothing
    }

    /// The mouse moved: come to a point (within reach, where points are drawn as points, not density), it plays at
    /// once, and shows so; off it, a hover's play stops. Nothing while hover plays are off, in the filter, over density,
    /// or on a console's list.
    pub fn on_move(&mut self, x: i32, y: i32) -> Asked {
        let points = self.picture.as_ref().is_some_and(|picture| !picture.dense);
        let over = (self.hover && points && self.prompt.is_none()).then(|| self.under(x, y)).flatten();
        let now = over.map(|at| (self.tab, at));
        if now == self.hovered {
            return Asked::Nothing;
        }
        self.hovered = now;
        let Some((at, layer)) = over.zip(self.layer()) else { return Asked::Leave };
        // The point Space plays: it plays on, whole, and no hover of it starts.
        if self.playing == now && !self.from_hover {
            return Asked::Leave;
        }
        let point = &layer.points[at as usize];
        let cut = point.seconds as f64 > PREVIEW;
        let asked = Asked::Hover {
            path: point.path.clone(),
            from: point.from as f64,
            seconds: if cut { PREVIEW } else { point.seconds as f64 },
            fade: if cut { PREVIEW_FADE } else { 0. },
            name: point.name.clone(),
        };
        (self.playing, self.from_hover, self.said) = (now, true, None);
        asked
    }

    /// The point within reach of a cell of the map's area: none outside it, or on a console's list.
    fn under(&self, x: i32, y: i32) -> Option<u32> {
        let area = self.area;
        if self.graphics == Graphics::Words || !inside(area, x, y) {
            return None;
        }
        let (layer, view, fit) = (self.layer()?, self.views.get(self.tab)?, self.fit(self.tab)?);
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

    /// The whole screen: the maps' names, the legend, the map (or its list), the point chosen and what it's near, and
    /// the keys. `upload` puts an image up under a key (once) and says the id it shows under. The cursor, while the
    /// filter is typed.
    pub fn draw(&mut self, screen: &mut Screen, upload: &mut dyn FnMut(&str, &Raster, usize, usize) -> u32) -> Option<Cursor> {
        if self.glide.is_some_and(|glide| glide.started.elapsed() >= GLIDE) {
            self.glide = None;
        }
        self.morph_step(false);
        let (columns, rows) = (screen.width, screen.height);
        if columns < 40 || rows < 12 {
            let words = self.keymap.with_key(
                "map.close",
                "Make this window bigger for the map ({} closes it)",
                "Make this window bigger for the map",
            );
            screen.put(1, 0, &truncate(&words, columns - 2), &Style::fg(palette::DIM));
            return None;
        }
        // The sounds made on the map take a row above the point's.
        let strip = i32::from(!self.made_sounds.is_empty());
        self.area = Rect::new(1, 2, columns - 2, rows - 5 - strip);
        self.draw_header(screen);
        self.draw_made(screen);
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
        let laid_out = self.tab == 0 && self.layouts[0].len() > 1 && self.graphics != Graphics::Words;
        if let Some(layout) = self.layout(self.tab).filter(|_| laid_out) {
            x = screen.put(x + 1, 0, &format!("by {}", layout_name(layout.arrangement)), &Style::fg(palette::DIM)) + 1;
        }
        if let Some(view) = self.views.get(self.tab) {
            let mut right = String::new();
            if !view.filter.is_empty() {
                right = format!("/ {}: {} shown", truncate(&view.filter, 24), grouped(view.shown.len()));
            }
            if !view.picked.is_empty() {
                right = [format!("{} picked", grouped(view.picked.len())), right]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ");
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
        let (area, view, fit) = (self.area, &self.views[self.tab], self.fit(self.tab).expect("laid out"));
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
        // (The layout by its fields: the picture is borrowed.)
        let layout = &self.layouts[self.tab][if self.tab == 0 { self.laid } else { 0 }];
        let (view, fit) = (&self.views[self.tab], &layout.fit);
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
        // An arrangement's words where they belong (keys, tempos, lengths, classes, folders), and its axes' at the edges.
        if layout.arrangement != "similarity" {
            let style = Style { bg: Some(palette::GROUND), ..Style::fg(palette::FAINT) };
            for (text, place) in &layout.labels {
                let (x, y) = frame.dot(*place);
                let width = text_width(text);
                let (x, y) = (area.x + (x / 2.) as i32 - width / 2, area.y + (y / 4.) as i32);
                if y >= area.y && y < area.y + area.height && x >= area.x && x + width <= area.x + area.width {
                    screen.put(x, y, text, &style);
                }
            }
            let [left, right, top, bottom] = axes(layout.arrangement);
            let middle = area.y + area.height / 2;
            screen.put(area.x, middle, left, &style);
            screen.put(area.x + area.width - text_width(right), middle, right, &style);
            screen.put(area.x + (area.width - text_width(top)) / 2, area.y, top, &style);
            screen.put(area.x + (area.width - text_width(bottom)) / 2, area.y + area.height - 1, bottom, &style);
        }
        // What a loose palette brings in (·), the palette's points (•), and the points picked (●) over them.
        let shown = |at: &u32| frame.shows(frame.dot(spot(&layer.points[*at as usize])));
        for at in self.halo[self.tab].iter().filter(|at| !view.picked.contains(at)).filter(|at| shown(at)) {
            let (x, y) = cell(*at);
            screen.put(x, y, "·", &Style::fg(palette::ACCENT));
        }
        for at in self.marked[self.tab].iter().filter(|at| !view.picked.contains(at)).filter(|at| shown(at)) {
            let (x, y) = cell(*at);
            screen.put(x, y, "•", &Style { bold: true, ..Style::fg(palette::ACCENT) });
        }
        for at in view.picked.iter().filter(|at| shown(at)) {
            let (x, y) = cell(*at);
            let found = self.tab == 0 && self.found.contains(at);
            screen.put(x, y, if found { "✦" } else { "●" }, &Style { bold: true, ..Style::fg(if found { palette::ACCENT } else { palette::BRIGHT }) });
        }
        if let Some(at) = chosen {
            let (x, y) = cell(at);
            let color = layer.groups[layer.points[at as usize].group as usize].color;
            screen.put(x, y, "◉", &Style { bold: true, bg: Some(palette::SELECTED), ..Style::fg(color) });
        }
        // The point the mouse is on: ringed, its name beside it (over whatever was there), ▶ while it plays.
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
        // The box being dragged, its edge on the cells it spans.
        if let Some(((x0, y0), (x1, y1))) = self.boxed {
            let (left, right, top, bottom) = (x0.min(x1), x0.max(x1), y0.min(y1), y0.max(y1));
            let style = Style::fg(palette::ACCENT);
            for x in left..=right {
                screen.put(x, top, "─", &style);
                screen.put(x, bottom, "─", &style);
            }
            for y in top..=bottom {
                screen.put(left, y, "│", &style);
                screen.put(right, y, "│", &style);
            }
            for (x, y, corner) in [(left, top, "┌"), (right, top, "┐"), (left, bottom, "└"), (right, bottom, "┘")] {
                screen.put(x, y, corner, &style);
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

    /// The sounds made on the map, numbered (the key that picks each), the picked ones marked: the row above the point's.
    fn draw_made(&mut self, screen: &mut Screen) {
        self.made_cells.clear();
        if self.made_sounds.is_empty() {
            return;
        }
        let (y, end) = (screen.height - 4, screen.width - 1);
        let mut x = screen.put(1, y, "Made", &Style::fg(palette::FAINT)) + 2;
        for (at, made) in self.made_sounds.iter().enumerate() {
            let text = format!("{} {}{}", at + 1, if made.picked { "✓ " } else { "" }, made.name);
            let text = truncate(&text, (end - x).min(32));
            if text_width(&text) < 5 {
                break;
            }
            let style = if made.picked { Style { bold: true, ..Style::fg(palette::ACCENT) } } else { Style::fg(palette::DIM) };
            let after = screen.put(x, y, &text, &style);
            self.made_cells.push((x, after - 1));
            x = after + 2;
        }
    }

    /// The point chosen (its name, what it is, its folder), and under it what it's near.
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
        if let Some(told) = self.told.as_ref().or(self.made.as_ref()) {
            screen.put(1, y, &truncate(told, width), &Style::fg(palette::ACCENT));
        } else if self.graphics != Graphics::Words {
            let view = &self.views[self.tab];
            let names: Vec<&str> = view.near.iter().map(|at| layer.points[*at as usize].name.as_str()).collect();
            if !names.is_empty() {
                let x = screen.put(1, y, "Near", &Style::fg(palette::FAINT)) + 2;
                screen.put(x, y, &truncate(&names.join(" · "), width - x), &Style::fg(palette::DIM));
            }
        }
    }

    /// The keys, or the filter being typed (with the cursor): as the keymap has them, Kumi's own arrows and zoom keys
    /// written in words ("arrows", "+ −").
    fn draw_keys(&self, screen: &mut Screen) -> Option<Cursor> {
        let y = screen.height - 1;
        if let Some(prompt) = &self.prompt {
            let x = screen.put(1, y, "/ ", &Style::fg(palette::ACCENT));
            let end = screen.put(x, y, &truncate(prompt, screen.width - x - 30), &Style::fg(palette::BRIGHT));
            let keys = self.keymap.hint(&[(&["map.filter-keep"], "keeps it"), (&["map.filter-drop"], "leaves it")]);
            screen.put(end + 2, y, &keys, &Style::fg(palette::FAINT));
            return Some(Cursor { x: end, y });
        }
        let other = if self.tab == 0 { "songs" } else { "sounds" };
        let picked = self.views.get(self.tab).map_or(0, |view| view.picked.len());
        if self.making {
            let making = format!("make from {} picks:", grouped(self.sources()));
            let mut parts: Vec<(&[&str], &str, &str)> = vec![(&[], "", &making)];
            parts.extend(MAKES.iter().map(|(id, words)| (std::slice::from_ref(id), "", *words)));
            parts.push((&["map.let-go"], "", "cancels"));
            let keys = self.keymap.hint_written(&parts);
            screen.put(1, y, &truncate(&keys, screen.width - 2), &Style::fg(palette::FAINT));
            return None;
        }
        let gather: (&[&str], &str, &str) = (&["map.gather"], "", "adds to the palette");
        let mut parts: Vec<(&[&str], &str, &str)> = vec![];
        let (adds, hover);
        let more = if picked == 1 { "more like this".to_string() } else { format!("more like these ({})", grouped(picked.min(MOST_NEAR))) };
        let laid = format!("lays out: {}", self.layout(0).map_or("similarity", |layout| layout.arrangement));
        let clears = format!("clears {}", if self.picks() == 1 { "the pick".into() } else { format!("{} picks", grouped(self.picks())) });
        if self.graphics == Graphics::Words {
            parts.push((&["map.up", "map.down"], "up and down", "choose"));
            parts.push((&["map.play"], "", "plays"));
            if self.gathers {
                parts.push(gather);
            }
            parts.push((&["map.filter"], "", "filters"));
            parts.push((&["map.switch"], "", other));
            parts.push((&["map.close"], "", "closes"));
        } else if picked > 0 && self.gathers {
            adds = format!("adds {} to the palette", if picked == 1 { "1 point".into() } else { format!("{} points", grouped(picked)) });
            parts.push((&["map.gather"], "", &adds));
            if self.designs && self.sources() >= kumi_design::MIN_SOURCES {
                parts.push((&["map.design"], "", "makes a sound of them"));
            }
            if self.tab == 0 {
                parts.push((&["map.near"], "", &more));
            }
            parts.push((&[], "", "drag picks (shift adds)"));
            parts.push((&["map.pick"], "", "picks"));
            parts.push((&["map.pick-all"], "", "all"));
            parts.push((&["map.grow"], "", "grows"));
            parts.push((&["map.let-go"], "", &clears));
        } else {
            hover = format!("hover {}", if self.hover { "plays" } else { "quiet" });
            if self.made.is_some() {
                parts.push((&["map.reroll"], "", "another"));
                parts.push((&["map.keep"], "", "keeps it"));
                parts.push((&["map.again"], "", "plays it"));
            }
            if !self.made_sounds.is_empty() {
                parts.push((&MADE_KEYS, "1–9", "pick sounds made"));
                if self.designs && self.sources() >= kumi_design::MIN_SOURCES {
                    parts.push((&["map.design"], "", "makes a sound of them"));
                }
                if self.picks() > 0 {
                    parts.push((&["map.let-go"], "", &clears));
                }
            }
            parts.push((&["map.up", "map.down", "map.left", "map.right"], "arrows", "move"));
            parts.push((&["map.pan-up", "map.pan-down", "map.pan-left", "map.pan-right"], "shift+arrows", "pan"));
            parts.push((&["map.zoom-in", "map.zoom-out"], "+ −", "zoom"));
            if self.tab == 0 && self.layouts[0].len() > 1 {
                parts.push((&["map.arrange"], "", &laid));
            }
            if self.gathers {
                // A drag picks whatever x is.
                parts.push(if self.keymap.key("map.pick").is_some() {
                    (&["map.pick"], "", "drag or {} picks")
                } else {
                    (&[], "", "drag picks")
                });
                parts.push(gather);
            }
            parts.push((&["map.filter"], "", "filters"));
            parts.push((&["map.play"], "", "plays"));
            parts.push((&["map.hover"], "", &hover));
            if self.layout(self.tab).is_some_and(|layout| !layout.home.is_empty()) {
                parts.push((&["map.unmove"], "", "puts moved points back"));
            }
            parts.push((&[], "", "middle-drag pans"));
            parts.push((&["map.switch"], "", other));
            parts.push((&["map.close"], "", "closes"));
        }
        let keys = self.keymap.hint_written(&parts);
        screen.put(1, y, &truncate(&keys, screen.width - 2), &Style::fg(palette::FAINT));
        None
    }
}

/// The keys that pick the sounds made, by their numbers.
const MADE_KEYS: [&str; MOST_MADE] = [
    "map.made-one",
    "map.made-two",
    "map.made-three",
    "map.made-four",
    "map.made-five",
    "map.made-six",
    "map.made-seven",
    "map.made-eight",
    "map.made-nine",
];

/// The ways to make a sound after d, by their keys' actions, as the footer names them.
const MAKES: [(&str, &str); 8] = [
    ("map.make-blend", "blend"),
    ("map.make-attack", "attack"),
    ("map.make-resonate", "resonate"),
    ("map.make-modulate", "modulate"),
    ("map.make-texture", "texture"),
    ("map.make-loop", "loop"),
    ("map.make-smear", "smear"),
    ("map.make-stack", "stack"),
];

/// The op and mode a way's key makes.
fn way_of(id: &str) -> Option<(Op, Option<Mode>)> {
    Some(match id {
        "map.make-blend" => (Op::Cross, Some(Mode::Blend)),
        "map.make-attack" => (Op::Cross, Some(Mode::Attack)),
        "map.make-resonate" => (Op::Cross, Some(Mode::Resonate)),
        "map.make-modulate" => (Op::Cross, Some(Mode::Modulate)),
        "map.make-texture" => (Op::Fuse, Some(Mode::Texture)),
        "map.make-loop" => (Op::Fuse, Some(Mode::Loop)),
        "map.make-smear" => (Op::Fuse, Some(Mode::Smear)),
        "map.make-stack" => (Op::Stack, None),
        _ => return None,
    })
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
                    arranged: vec![],
                })
                .collect(),
            groups: vec![
                Group { name: "kick".into(), color: [255, 0, 0], count: 0, examples: vec![] },
                Group { name: "pad".into(), color: [0, 0, 255], count: 0, examples: vec![] },
            ],
            places: vec!["Samples / Drums".into()],
            tags: vec!["kick one-shot".into()],
            empty: None,
            arrangements: vec![],
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
    fn the_mouse_coming_to_a_point_hears_it_at_once_and_leaving_stops_what_that_started() {
        let mut sounds = layer(&[(0.5, 0.5, 0), (0.9, 0.9, 1)]);
        sounds.points[1].seconds = 30.;
        let mut map = MapView::new(Graphics::Cells, true);
        map.loaded(ready(MapData { sounds, songs: layer(&[]) }));
        map.draw(&mut Screen::new(100, 30), &mut |_, _, _, _| 0);
        let (x, y) = cell_of(&map, 0);
        let hover = map.on_move(x, y);
        assert!(matches!(&hover, Asked::Hover { seconds, fade, name, .. } if *seconds == 1. && *fade == 0. && name == "p0"), "{hover:?}");
        assert_eq!((map.playing, map.from_hover), (Some((0, 0)), true), "at once, with no rest first");
        assert_eq!(map.on_move(x + 1, y), Asked::Nothing, "still on it");
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
        // h: hover plays off, the keys say so, and the mouse on a point asks nothing; on again.
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
        // The mouse comes to the point chosen and its preview plays: Space plays it whole, and again stops it.
        let chosen = map.views[0].chosen.unwrap();
        let (x, y) = cell_of(&map, chosen);
        assert!(matches!(map.on_move(x, y), Asked::Hover { .. }));
        assert!(matches!(map.on_text(" "), Asked::Play { .. }), "Space plays it whole, not a stop");
        assert_eq!((map.playing, map.from_hover), (Some((0, chosen)), false));
        assert_eq!(map.on_text(" "), Asked::Stop);
        // The mouse coming back to the point Space plays starts no hover of it.
        assert!(matches!(map.on_text(" "), Asked::Play { .. }));
        assert_eq!(map.on_move(map.area.x, map.area.y), Asked::Leave);
        assert!(!map.hover_ends());
        assert_eq!(map.on_move(x, y), Asked::Leave, "Space's point plays on, whole");
        assert_eq!((map.playing, map.from_hover), (Some((0, chosen)), false));
        // Tab while the mouse is on a point: what its hover plays stops.
        map.played(None);
        assert_eq!(map.on_move(map.area.x, map.area.y), Asked::Leave);
        assert!(matches!(map.on_move(x, y), Asked::Hover { .. }));
        assert_eq!(map.on_key("tab", false), Asked::Leave);
        assert!(map.hover_ends() && map.playing.is_none());
        assert_eq!(map.on_key("tab", false), Asked::Nothing, "no hover on, nothing to end");
        // A preview playing as the filter hides its point: it ends, and the point's ring goes.
        map.draw(&mut Screen::new(100, 30), &mut |_, _, _, _| 0);
        let (x, y) = cell_of(&map, chosen);
        assert!(matches!(map.on_move(x, y), Asked::Hover { .. }));
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

    /// A map of `points` that puts points into the palette, drawn once (100 × 30).
    fn gathering(points: &[(f32, f32, u16)]) -> MapView {
        let mut map = MapView::new(Graphics::Cells, true).gathering(true);
        map.loaded(ready(MapData { sounds: layer(points), songs: layer(&[]) }));
        map.draw(&mut Screen::new(100, 30), &mut |_, _, _, _| 0);
        map
    }

    fn names(asked: &Asked) -> Vec<String> {
        match asked {
            Asked::Gather(files) => files.iter().map(|file| file.name.clone()).collect(),
            other => panic!("not a gathering: {other:?}"),
        }
    }

    #[test]
    fn x_a_and_g_pick_points_enter_puts_them_into_the_palette_and_esc_lets_them_go_first() {
        // A row of points across the middle.
        let points: Vec<(f32, f32, u16)> = (0..9).map(|n| (0.1 + n as f32 * 0.1, 0.5, (n % 2) as u16)).collect();
        let mut map = gathering(&points);
        // Nothing picked: Enter puts the point chosen in (p4, the middle one).
        assert_eq!(names(&map.on_key("enter", false)), ["p4"]);
        // x picks the point chosen, and again lets it go; the header counts them.
        map.on_text("x");
        let mut screen = Screen::new(100, 30);
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert!(screen.lines()[0].contains("1 picked"), "{}", screen.lines()[0]);
        assert!(screen.lines()[29].starts_with(" enter adds 1 point to the palette"), "{}", screen.lines()[29]);
        map.on_text("x");
        assert!(map.views[0].picked.is_empty());
        // g: each picked one brings its nearest (four on the map).
        map.on_text("x");
        map.on_text("g");
        assert_eq!(map.views[0].picked, BTreeSet::from([2, 3, 4, 5, 6]));
        // Enter puts the picked ones in, in their order, and lets them go.
        assert_eq!(names(&map.on_key("enter", false)), ["p2", "p3", "p4", "p5", "p6"]);
        assert!(map.views[0].picked.is_empty());
        // a picks all the filter keeps: Esc lets them go, and only then closes the map.
        map.on_text("/p1");
        map.on_key("enter", false);
        map.on_text("a");
        assert_eq!(map.views[0].picked, BTreeSet::from([1]));
        assert_eq!(map.on_key("escape", false), Asked::Nothing);
        assert!(map.views[0].picked.is_empty());
        assert_eq!(map.on_key("escape", false), Asked::Close);
        // A map whose session keeps no palettes: Enter does nothing, and the keys don't offer it.
        let mut plain = MapView::new(Graphics::Cells, true);
        plain.loaded(ready(MapData { sounds: layer(&points), songs: layer(&[]) }));
        let mut screen = Screen::new(100, 30);
        plain.draw(&mut screen, &mut |_, _, _, _| 0);
        assert_eq!(plain.on_key("enter", false), Asked::Nothing);
        assert!(!screen.lines()[29].contains("palette"), "{}", screen.lines()[29]);
    }

    #[test]
    fn the_keys_under_the_map_are_the_keymaps_with_kumis_own_arrows_in_words() {
        let points: Vec<(f32, f32, u16)> = (0..9).map(|n| (0.1 + n as f32 * 0.1, 0.5, (n % 2) as u16)).collect();
        let footer = |map: &mut MapView| {
            let mut screen = Screen::new(200, 30);
            map.draw(&mut screen, &mut |_, _, _, _| 0);
            screen.lines()[29].trim().to_string()
        };
        assert_eq!(
            footer(&mut gathering(&points)),
            "arrows move · shift+arrows pan · + − zoom · drag or x picks · enter adds to the palette · / filters · space plays · \
             h hover plays · middle-drag pans · tab songs · esc closes"
        );
        // keys.json's keys say themselves; a drag still picks with x given none.
        let (keymap, _) =
            Keymap::new(Some(&serde_json::json!({ "map.left": ["h"], "map.hover": ["v"], "map.pick": [], "map.close": ["f10"] })));
        assert_eq!(
            footer(&mut gathering(&points).keyed(&keymap)),
            "↑ ↓ h → move · shift+arrows pan · + − zoom · drag picks · enter adds to the palette · / filters · space plays · \
             v hover plays · middle-drag pans · tab songs · f10 closes"
        );
    }

    #[test]
    fn enter_puts_in_at_most_what_the_palette_takes_at_once_and_says_how_many_were_picked() {
        let points: Vec<(f32, f32, u16)> = (0..MOST_GATHERED + 5).map(|n| ((n % 100) as f32 / 100., (n / 100) as f32 / 101., 0)).collect();
        let mut map = gathering(&points);
        map.on_text("a");
        let Asked::Gather(files) = map.on_key("enter", false) else { panic!("a gathering") };
        assert_eq!(files.len(), MOST_GATHERED);
        map.told("Added 10,000 sounds to Palette 1.".into());
        assert_eq!(map.told.as_deref(), Some("Added 10,000 sounds to Palette 1. The first 10,000 of 10,005 picked went in."));
        // g on all of them is a lookup, not a search each.
        map.on_text("a");
        let started = std::time::Instant::now();
        map.on_text("g");
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn each_points_nearest_on_the_map_are_found_by_its_grid() {
        let points = [(0.1f32, 0.1f32), (0.12, 0.1), (0.5, 0.5), (0.9, 0.9), (0.13, 0.11)];
        let layer = layer(&points.map(|(x, y)| (x, y, 0)));
        let nearest = nearest_lists(&layer.points);
        assert_eq!(&nearest[..4], [1, 4, 2, 3], "p0's, nearest first");
        assert_eq!(nearest[3 * NEAREST_KEPT], 2, "p3's nearest is the middle one");
        assert!(nearest[..NEAREST_KEPT].iter().skip(4).all(|at| *at == u32::MAX), "four others, no more");
    }

    #[test]
    fn a_box_dragged_picks_whats_in_it_ctrl_adds_and_a_ctrl_click_picks_one() {
        let points = [(0.2, 0.2, 0), (0.25, 0.25, 0), (0.8, 0.8, 1), (0.82, 0.78, 1), (0.5, 0.5, 0)];
        let mut map = gathering(&points);
        let (a, b, c) = (cell_of(&map, 0), cell_of(&map, 1), cell_of(&map, 3));
        // From just past the first point to just past the second: both, drawn as a box while it's dragged.
        map.on_click(a.0 - 1, a.1 + 1, false);
        map.on_drag(b.0 + 1, b.1 - 1);
        let mut screen = Screen::new(100, 30);
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert!(screen.lines().iter().any(|line| line.contains('┌')) && screen.lines().iter().any(|line| line.contains('┘')));
        map.on_release(b.0 + 1, b.1 - 1);
        assert_eq!(map.views[0].picked, BTreeSet::from([0, 1]));
        // Another box in place of it; with ctrl as well as it.
        map.on_click(c.0 - 2, c.1 - 2, false);
        map.on_drag(c.0 + 2, c.1 + 2);
        map.on_release(c.0 + 2, c.1 + 2);
        assert_eq!(map.views[0].picked, BTreeSet::from([2, 3]));
        map.on_click(a.0 - 1, a.1 + 1, true);
        map.on_drag(b.0 + 1, b.1 - 1);
        map.on_release(b.0 + 1, b.1 - 1);
        assert_eq!(map.views[0].picked, BTreeSet::from([0, 1, 2, 3]));
        // A click without moving picks nothing; with ctrl it picks the point (or lets it go).
        let middle = cell_of(&map, 4);
        map.on_click(middle.0, middle.1, false);
        map.on_release(middle.0, middle.1);
        assert_eq!(map.views[0].picked.len(), 4);
        map.on_click(middle.0, middle.1, true);
        map.on_release(middle.0, middle.1);
        assert!(map.views[0].picked.contains(&4));
        map.on_click(middle.0, middle.1, true);
        map.on_release(middle.0, middle.1);
        assert!(!map.views[0].picked.contains(&4));
        // Esc lets a box being dragged go, nothing picked by it.
        map.on_click(a.0 - 1, a.1 + 1, false);
        map.on_drag(c.0 + 2, c.1 + 2);
        assert_eq!(map.on_key("escape", false), Asked::Nothing);
        map.on_release(c.0 + 2, c.1 + 2);
        assert_eq!(map.views[0].picked.len(), 4);
    }

    #[test]
    fn the_palettes_points_and_the_picked_ones_are_marked_and_what_gathering_did_is_said() {
        let mut map = gathering(&[(0.3, 0.3, 0), (0.7, 0.7, 1), (0.5, 0.5, 0)]);
        map.in_palette(HashSet::from(["/s/p0.wav".to_string()]), HashSet::from(["/s/p2.wav".to_string()]));
        map.views[0].picked.insert(1);
        map.version += 1;
        map.told("Added 2 sounds to Drums.".into());
        let mut screen = Screen::new(100, 30);
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        // Each mark on its point's cell: as braille rounds a dot, that's the cell `cell_of` works out or one beside it.
        let (in_palette, picked, reached) = (cell_of(&map, 0), cell_of(&map, 1), cell_of(&map, 2));
        let marked = |lines: &[String], (x, y): (i32, i32), mark: char| {
            (y - 1..=y + 1).any(|row| {
                (x - 1..=x + 1).any(|column| lines.get(row as usize).and_then(|line| line.chars().nth(column as usize)) == Some(mark))
            })
        };
        let lines = screen.lines();
        assert!(marked(&lines, in_palette, '•') && marked(&lines, picked, '●'), "{}", lines.join("\n"));
        assert!(screen.lines()[28].contains("Added 2 sounds to Drums."), "{}", screen.lines()[28]);
        // The chosen point (p2, the middle one) is drawn over its halo; with none chosen, the halo shows.
        assert!(marked(&lines, reached, '◉'), "{}", lines.join("\n"));
        map.views[0].chosen = None;
        let mut halo = Screen::new(100, 30);
        map.draw(&mut halo, &mut |_, _, _, _| 0);
        assert!(marked(&halo.lines(), reached, '·'), "{}", halo.lines().join("\n"));
        map.views[0].chosen = Some(2);
        // The next key: said no more.
        map.on_text("+");
        let mut screen = Screen::new(100, 30);
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert!(!screen.lines()[28].contains("Added"));
    }

    /// A big map's nearest are searched on several threads and come out as on one: each point's distance to its nearest
    /// and how many others name it, on a map with tight clusters and points on the very same spot.
    #[test]
    fn a_maps_nearest_are_the_same_on_one_thread_or_several() {
        let mut points = vec![];
        let mut seed = 7_u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f32 / (1u64 << 31) as f32
        };
        for at in 0..25_000 {
            let (x, y) = match at % 5 {
                // The same spot as the point before.
                0 if at > 0 => (points.last().map_or(0.5, |p: &(f32, f32, u16)| p.0), points.last().map_or(0.5, |p: &(f32, f32, u16)| p.1)),
                // A tight cluster.
                1 | 2 => (0.3 + next() * 0.01, 0.6 + next() * 0.01),
                _ => (next(), next()),
            };
            points.push((x, y, (at % 7) as u16));
        }
        let layer = layer(&points);
        let one = nearest_named_on(&layer.points, 1.4, 1);
        for threads in [3, 8] {
            assert_eq!(nearest_named_on(&layer.points, 1.4, threads), one, "{threads} threads");
        }
        assert_eq!(one.1.iter().map(|named| *named as usize).sum::<usize>(), 25_000 * NEAREST, "every point names its nearest");
    }

    /// A fixed map of 30,000 points (tight clusters, points on the same spot), fitted: every point's place, the map's
    /// aspect, each point's distance to its nearest and how typical it is, as one digest of their exact bits.
    fn fitted_digest(fit: impl Fn(&mut Layer) -> Fit) -> u64 {
        let mut points = vec![];
        let mut seed = 11_u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f32 / (1u64 << 31) as f32
        };
        for at in 0..30_000 {
            let (x, y) = match at % 6 {
                0 if at > 0 => (points.last().map_or(0.5, |p: &(f32, f32, u16)| p.0), points.last().map_or(0.5, |p: &(f32, f32, u16)| p.1)),
                1 | 2 => (0.2 + next() * 0.02, 0.7 + next() * 0.015),
                _ => (next(), next() * 0.8),
            };
            points.push((x, y, (at % 9) as u16));
        }
        let mut layer = layer(&points);
        let made = fit(&mut layer);
        let mut digest = 0xcbf29ce484222325_u64;
        let mut add = |bits: u64| {
            for byte in bits.to_le_bytes() {
                digest = (digest ^ byte as u64).wrapping_mul(0x100000001b3);
            }
        };
        for point in &layer.points {
            add(point.x.to_bits() as u64);
            add(point.y.to_bits() as u64);
        }
        add(made.aspect.to_bits());
        made.apart.iter().chain(&made.typical).for_each(|value| add(value.to_bits() as u64));
        digest
    }

    /// The fit is main's, point for point, however many threads its search runs on.
    #[test]
    fn a_big_maps_fit_is_the_same_as_before_on_any_number_of_threads() {
        for threads in [None, Some(1), Some(3), Some(8)] {
            assert_eq!(fitted_digest(|layer| fitted_on(layer, threads)), MAINS_FIT, "{threads:?} threads");
        }
    }
    /// What main's fit made of the fixed map (its digest, from main's tui/map.rs before this search ran on threads).
    const MAINS_FIT: u64 = 13_404_431_125_891_730_567;
    #[test]
    fn shift_and_a_box_adds_its_sounds_to_the_first_boxs() {
        let points = [(0.2, 0.2, 0), (0.25, 0.25, 0), (0.8, 0.8, 1), (0.82, 0.78, 1), (0.5, 0.5, 0)];
        let mut map = gathering(&points);
        let (a, b, c) = (cell_of(&map, 0), cell_of(&map, 1), cell_of(&map, 3));
        map.on_click(a.0 - 1, a.1 + 1, false);
        map.on_drag(b.0 + 1, b.1 - 1);
        map.on_release(b.0 + 1, b.1 - 1);
        // A second box with shift held (the app hands shift on as adding, as ctrl): one chunk, then another.
        map.on_click(c.0 - 2, c.1 - 2, true);
        map.on_drag(c.0 + 2, c.1 + 2);
        map.on_release(c.0 + 2, c.1 + 2);
        assert_eq!(map.views[0].picked, BTreeSet::from([0, 1, 2, 3]));
    }

    #[test]
    fn a_middle_drag_moves_the_view_and_picks_nothing() {
        let points: Vec<(f32, f32, u16)> = (0..9).map(|n| (0.1 + n as f32 * 0.1, 0.5, (n % 2) as u16)).collect();
        let mut map = gathering(&points);
        map.on_text("x");
        let picked = map.views[0].picked.clone();
        for _ in 0..4 {
            map.on_text("+");
        }
        let (centre, area) = (map.views[0].centre, map.area);
        let (x, y) = (area.x + area.width / 2, area.y + area.height / 2);
        map.on_middle(x, y);
        map.on_pan(x - 10, y + 3);
        map.on_release(x - 10, y + 3);
        let moved = map.views[0].centre;
        assert!(moved.0 > centre.0 && moved.1 > centre.1, "dragged left and down, the map shows what's right and up: {centre:?} → {moved:?}");
        assert_eq!(map.views[0].picked, picked, "nothing picked or let go");
        assert!(map.boxed.is_none() && map.pressed.is_none());
        // The point under the mouse went with it: a third of the way there and back, it's where it was.
        map.on_middle(x, y);
        map.on_pan(x + 10, y - 3);
        map.on_release(x + 10, y - 3);
        let back = map.views[0].centre;
        assert!((back.0 - centre.0).abs() < 1e-9 && (back.1 - centre.1).abs() < 1e-9, "{centre:?} {back:?}");
    }

    #[test]
    fn a_dragged_sound_stays_where_it_was_put_after_a_reload_and_shift_r_puts_it_back() {
        // Points scattered so the map is turned and its strays brought in: a moved one comes back through all that.
        let points: Vec<(f32, f32, u16)> =
            (0..40).map(|n| ((n * 37 % 100) as f32 / 100., (n * 61 % 100) as f32 / 140. + 0.1 * (n % 3) as f32, 0)).collect();
        let data = MapData { sounds: layer(&points), songs: layer(&[]) };
        let open = |moves: Moves| {
            let mut map = MapView::new(Graphics::Cells, true).gathering(true).moved(moves);
            map.loaded(ready(data.clone()));
            map.draw(&mut Screen::new(100, 30), &mut |_, _, _, _| 0);
            map
        };
        let mut map = open(Moves::new());
        let at = 7;
        let home = spot(&map.layer().unwrap().points[at]);
        let (x, y) = cell_of(&map, at as u32);
        let to = (map.area.x + 3, map.area.y + 2);
        assert_ne!((x, y), to);
        map.on_click(x, y, false);
        map.on_drag(to.0, to.1);
        let Asked::Moved(moves) = map.on_release(to.0, to.1) else { panic!("a move kept") };
        assert_eq!(cell_of(&map, at as u32), to, "drawn where it was let go");
        let put = spot(&map.layer().unwrap().points[at]);
        assert!(moves["sounds"].contains_key("/s/p7.wav"), "kept by its file: {moves:?}");
        // Opened again with the moves kept: there, and the other points where they were.
        let mut again = open(moves);
        let there = spot(&again.layer().unwrap().points[at]);
        assert!((there.0 - put.0).abs() < 1e-4 && (there.1 - put.1).abs() < 1e-4, "{put:?} {there:?}");
        assert_eq!(spot(&again.layer().unwrap().points[3]), spot(&map.layer().unwrap().points[3]));
        // shift+r: back where the map put it, and nothing kept.
        let Asked::Moved(moves) = again.on_text("R") else { panic!("the moves put back") };
        assert!(moves.is_empty(), "{moves:?}");
        assert_eq!(spot(&again.layer().unwrap().points[at]), home);
        assert_eq!(again.on_text("R"), Asked::Nothing, "nothing left to put back");
        // A drag that starts beside a point draws a box: the point stays.
        let (x, y) = cell_of(&again, 3);
        let before = spot(&again.layer().unwrap().points[3]);
        again.on_click(x - 1, y + 1, false);
        again.on_drag(x + 4, y + 2);
        assert!(again.boxed.is_some());
        assert_eq!(again.on_release(x + 4, y + 2), Asked::Nothing);
        assert_eq!(spot(&again.layer().unwrap().points[3]), before);
    }

    #[test]
    fn a_make_from_sounds_made_names_their_files_with_every_pick_and_no_cap() {
        let points: Vec<(f32, f32, u16)> = (0..30).map(|n| ((n % 10) as f32 / 10., (n / 10) as f32 / 3., 0)).collect();
        let mut map = gathering(&points).designing(true);
        map.made_sound(4, "/d/Blend - p0 + 1 - 42.wav".into());
        map.made_sound(5, "/d/Blend - p0 + 1 - 43.wav".into());
        map.made_sound(5, "/d/Blend - p0 + 1 - 43.wav".into());
        let mut screen = Screen::new(140, 30);
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert!(screen.lines()[26].contains("Made  1 Blend - p0 + 1 - 42  2 Blend - p0 + 1 - 43"), "{}", screen.lines()[26]);
        // The two made, picked by their numbers: d and b makes a blend of them.
        map.on_text("1");
        map.on_text("2");
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert!(screen.lines()[26].contains("1 ✓ Blend"), "{}", screen.lines()[26]);
        assert!(screen.lines()[29].contains("d makes a sound of them") && screen.lines()[29].contains("esc clears 2 picks"));
        map.on_text("d");
        let asked = map.on_text("b");
        assert_eq!(
            asked,
            Asked::Design {
                op: Op::Cross,
                mode: Some(Mode::Blend),
                files: vec!["/d/Blend - p0 + 1 - 42.wav".into(), "/d/Blend - p0 + 1 - 43.wav".into()]
            }
        );
        // With every point picked too: all 32, none left out.
        map.on_text("a");
        map.on_text("d");
        let Asked::Design { files, .. } = map.on_text("k") else { panic!("a stack") };
        assert_eq!(files.len(), 32);
        assert_eq!(files[..2], ["/d/Blend - p0 + 1 - 42.wav", "/d/Blend - p0 + 1 - 43.wav"]);
        // Esc lets every pick go, made ones too; a sound made comes off the list as the tenth comes.
        assert_eq!(map.on_key("escape", false), Asked::Nothing);
        assert_eq!(map.picks(), 0);
        for id in 6..13 {
            map.made_sound(id, format!("/d/{id}.wav"));
        }
        map.on_text("1");
        map.made_sound(13, "/d/13.wav".into());
        assert_eq!(map.made_sounds.len(), MOST_MADE);
        assert_eq!(map.made_sounds.iter().map(|made| made.id).collect::<Vec<_>>(), [4, 6, 7, 8, 9, 10, 11, 12, 13]);
        assert!(map.made_sounds[0].picked, "the oldest not picked went, the picked one stays");
    }

    #[test]
    fn a_click_on_no_point_lets_the_picks_go() {
        let points = [(0.2, 0.2, 0), (0.25, 0.25, 0), (0.8, 0.8, 1)];
        let mut map = gathering(&points);
        map.on_text("a");
        assert_eq!(map.views[0].picked.len(), 3);
        // Ctrl or shift and a click there keeps them.
        let (x, y) = (map.area.x + map.area.width - 2, map.area.y + 1);
        map.on_click(x, y, true);
        map.on_release(x, y);
        assert_eq!(map.views[0].picked.len(), 3);
        map.on_click(x, y, false);
        map.on_release(x, y);
        assert!(map.views[0].picked.is_empty());
    }

    #[test]
    fn l_lays_the_sounds_out_another_way_with_every_point_on_its_place_there() {
        use crate::map::{Arrangement, Label};
        let mut sounds = layer(&[(0.1, 0.2, 0), (0.6, 0.4, 1), (0.9, 0.9, 0), (0.3, 0.8, 1)]);
        sounds.arrangements = vec![
            Arrangement { id: "character", labels: vec![] },
            Arrangement { id: "keys", labels: vec![Label { text: "C".into(), x: 0.5, y: 0.05 }] },
        ];
        for (n, point) in sounds.points.iter_mut().enumerate() {
            point.arranged = vec![[n as f32 / 4., 1. - n as f32 / 4.], [0.2 + n as f32 / 5., 0.5]];
        }
        let arranged: Vec<Vec<[f32; 2]>> = sounds.points.iter().map(|point| point.arranged.clone()).collect();
        let mut map = MapView::new(Graphics::Cells, true);
        map.loaded(ready(MapData { sounds, songs: layer(&[]) }));
        let draw = |map: &mut MapView| {
            let mut screen = Screen::new(120, 30);
            map.draw(&mut screen, &mut |_, _, _, _| 0);
            screen.lines()
        };
        let spots = |map: &MapView| map.layer().unwrap().points.iter().map(spot).collect::<Vec<_>>();
        let lines = draw(&mut map);
        assert!(lines[0].contains("by Similarity") && lines[29].contains("l lays out: similarity"), "{}\n{}", lines[0], lines[29]);
        let similarity = spots(&map);
        // Character, then keys: every point on its place there (x across the box, y up), the map named in the header and
        // the keys, its axes' and data's words drawn.
        for (k, aspect, name, word) in [(0, ARRANGED_ASPECT, "character", "bright"), (1, 1., "keys", "C")] {
            assert_eq!(map.on_text("l"), Asked::Nothing);
            for (at, (x, y)) in spots(&map).into_iter().enumerate() {
                let [ax, ay] = arranged[at][k];
                assert!((x - ax as f64 * aspect).abs() < 1e-6 && (y - (1. - ay as f64)).abs() < 1e-6, "{name}: {at} at {x}, {y}");
            }
            assert_eq!(map.views[0].aspect, aspect);
            let lines = draw(&mut map);
            assert!(lines[29].contains(&format!("lays out: {name}")), "{}", lines[29]);
            assert!(lines.iter().any(|line| line.contains(word)), "{word}:\n{}", lines.join("\n"));
        }
        // Round to similarity again, where every point was; shift+l goes back the other way.
        map.on_text("l");
        assert_eq!(spots(&map), similarity);
        map.on_text("L");
        assert!(draw(&mut map)[0].contains("by Keys"));
        // The songs' map has one layout.
        map.on_key("tab", false);
        map.on_text("l");
        assert_eq!(map.told.as_deref(), Some("Only the sounds' map has other layouts."));
    }

    #[test]
    fn n_asks_for_more_like_the_picks_and_picks_what_it_finds_as_well() {
        let points: Vec<(f32, f32, u16)> = (0..9).map(|n| (0.1 + n as f32 * 0.1, 0.5, (n % 2) as u16)).collect();
        let mut map = gathering(&points);
        map.on_text("x");
        map.on_key("right", false);
        map.on_text("x");
        let picked = map.views[0].picked.clone();
        assert_eq!(picked.len(), 2);
        let mut screen = Screen::new(140, 30);
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert!(screen.lines()[29].contains("n more like these (2)"), "{}", screen.lines()[29]);
        let Asked::Near(paths) = map.on_text("n") else { panic!("more like these") };
        let want: Vec<String> = picked.iter().map(|at| format!("/s/p{at}.wav")).collect();
        assert_eq!(paths, want);
        // What it found is picked as well and marked; Esc lets it all go.
        map.near_found(Ok(vec!["/s/p0.wav".into(), "/s/p8.wav".into(), "/elsewhere.wav".into()]));
        assert_eq!(map.views[0].picked, picked.iter().copied().chain([0, 8]).collect());
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert!(screen.lines().iter().any(|line| line.contains('✦')));
        assert!(map.told.as_deref().is_some_and(|told| told.starts_with("Picked 2 more like these")));
        map.on_key("escape", false);
        assert!(map.views[0].picked.is_empty() && map.found.is_empty());
        // Why it found none is said.
        map.on_text("x");
        map.on_text("n");
        map.near_found(Err("Kumi hasn't learned how that sound sounds yet.".into()));
        assert_eq!(map.told.as_deref(), Some("Kumi hasn't learned how that sound sounds yet."));
    }
}
