//! /map: the producer's sounds and songs as two maps, full screen in place of the conversation until it closes. Arrows
//! go to the nearest point that way, shift and the arrows pan, + and − zoom (about the point chosen), a click picks a
//! point, / filters, space plays, a point plays as the mouse comes to it (h turns that off and on), tab switches maps
//! and esc closes. Zoomed out on a big
//! map it draws density, busier places brighter; points, then their names, show as it zooms in. The points are an
//! image where the terminal shows images and braille elsewhere, with the point chosen and the names as text over
//! them; a console that shows neither lists the groups with their counts and examples instead.

use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;

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
    /// The points picked (a box dragged, x, ctrl and a click, a, g), by their place in the layer.
    picked: BTreeSet<u32>,
    /// The map's point in the middle of the area, and how far in it is (1: all of it).
    centre: (f64, f64),
    zoom: f64,
    /// Where the list is, on a console that lists the groups.
    row: usize,
}

impl View {
    fn new(layer: &Layer) -> View {
        let mut view = View {
            filter: String::new(),
            shown: vec![],
            chosen: None,
            near: vec![],
            picked: BTreeSet::new(),
            centre: (0.5, 0.5),
            zoom: 1.,
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
        if self.zoom <= 1. {
            (self.zoom, self.centre) = (1., (0.5, 0.5));
        } else if !(self.centre.0.is_finite() && self.centre.1.is_finite()) {
            // Worked out against an area that has nothing in it: the middle.
            self.centre = (0.5, 0.5);
        } else {
            self.centre = (self.centre.0.clamp(0., 1.), self.centre.1.clamp(0., 1.));
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
        let (width, height) = (area.width as f64 * 2., area.height as f64 * 4.);
        Frame { width, height, scale: width.min(height) * FILL * view.zoom, centre: view.centre }
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
    std::array::from_fn(|c| (under[c] as f64 + (over[c] as f64 - under[c] as f64) * share).round() as u8)
}

/// The points in view as shapes in the area: a dot each where few are in view (bigger where they're named); where many
/// are, a dot for each patch of them (`patch` dots across), brighter the busier it is and in its commonest group's
/// colour. With the points it named, if any, and whether it drew density.
fn scene(layer: &Layer, view: &View, frame: &Frame, patch: f64, steps: Option<f64>) -> (Scene, Vec<u32>, bool) {
    let in_view: Vec<u32> = view.shown.iter().copied().filter(|at| frame.shows(frame.dot(spot(&layer.points[*at as usize])))).collect();
    let thousandths = |dots: f64| dots / frame.height * 1000.;
    if in_view.len() <= DENSE {
        let named = in_view.len() <= NAMED;
        let radius = thousandths(if named { 0.9 } else { 0.6 });
        let shapes = in_view
            .iter()
            .map(|at| {
                let point = &layer.points[*at as usize];
                let (x, y) = frame.dot(spot(point));
                Shape::Dot {
                    x: x / frame.width,
                    y: 1. - y / frame.height,
                    radius,
                    color: layer.groups[point.group as usize].color,
                    alpha: 0.9,
                }
            })
            .collect();
        return (Scene { shapes, lines: false }, if named { in_view } else { vec![] }, false);
    }
    let (columns, rows) = ((frame.width / patch).ceil() as usize, (frame.height / patch).ceil() as usize);
    // Each patch's count, and its commonest group (a running vote: the group most of its points are in, when one is).
    let mut patches = vec![(0u32, 0u16, 0u32); columns * rows];
    for at in &in_view {
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
    (Scene { shapes, lines: false }, vec![], true)
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
}

pub struct MapView {
    graphics: Graphics,
    data: Option<Rc<MapData>>,
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
    /// Bumped whenever what the picture shows changes: pan, zoom, filter, a new map.
    version: u64,
    picture: Option<Picture>,
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
    /// Where the left button went down (and whether with ctrl), while that may become a box dragged; the box, corner to
    /// corner, once it has.
    pressed: Option<(i32, i32, bool)>,
    boxed: Option<((i32, i32), (i32, i32))>,
    /// What putting points into the palette did, said under the map until the next key; and how many were picked when
    /// Enter put in only the first MOST_GATHERED.
    told: Option<String>,
    capped: Option<usize>,
    /// Each point's NEAREST_KEPT nearest on each map (u32::MAX for none), worked out the first time g wants them.
    nearest: [Option<Rc<Vec<u32>>>; 2],
}

impl MapView {
    /// The map, before its data is read; `hover`: whether points play as the mouse comes to them.
    pub fn new(graphics: Graphics, hover: bool) -> MapView {
        MapView {
            graphics,
            data: None,
            tab: 0,
            views: vec![],
            prompt: None,
            playing: None,
            said: None,
            hover,
            hovered: None,
            from_hover: false,
            version: 0,
            picture: None,
            area: Rect::default(),
            gathers: false,
            palette: HashSet::new(),
            reached: HashSet::new(),
            marked: [vec![], vec![]],
            halo: [vec![], vec![]],
            pressed: None,
            boxed: None,
            told: None,
            capped: None,
            nearest: [None, None],
        }
    }

    /// The same map, Enter putting points into the session's palette.
    pub fn gathering(mut self, on: bool) -> MapView {
        self.gathers = on;
        self
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

    /// The left button held and moved: once off the cell it went down on, a box is dragged from there.
    pub fn on_drag(&mut self, x: i32, y: i32) -> Asked {
        let Some((from_x, from_y, _)) = self.pressed.filter(|_| self.graphics != Graphics::Words) else { return Asked::Nothing };
        if self.boxed.is_some() || (x, y) != (from_x, from_y) {
            let area = self.area;
            let clamped = (x.clamp(area.x, area.x + area.width - 1), y.clamp(area.y, area.y + area.height - 1));
            self.boxed = Some(((from_x, from_y), clamped));
        }
        Asked::Nothing
    }

    /// The left button let go: a box dragged picks what's in it (in place of what was picked; with ctrl, as well).
    pub fn on_release(&mut self, _x: i32, _y: i32) -> Asked {
        let (pressed, boxed) = (self.pressed.take(), self.boxed.take());
        let (Some((_, _, ctrl)), Some(((x0, y0), (x1, y1)))) = (pressed, boxed) else { return Asked::Nothing };
        let Some(data) = self.data.clone() else { return Asked::Nothing };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        let (area, view) = (self.area, &mut self.views[self.tab]);
        let frame = Frame::new(area, view);
        // The box's corners on the map: from the first cell's top left to the last one's bottom right.
        let corner = |x: i32, y: i32| frame.at((((x - area.x) * 2) as f64, ((y - area.y) * 4) as f64));
        let (a, b) = (corner(x0.min(x1), y0.min(y1)), corner(x0.max(x1) + 1, y0.max(y1) + 1));
        let (left, right, low, high) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
        if !ctrl {
            view.picked.clear();
        }
        view.picked.extend(view.shown.iter().copied().filter(|at| {
            let (x, y) = spot(&layer.points[*at as usize]);
            x >= left && x <= right && y >= low && y <= high
        }));
        self.version += 1;
        Asked::Nothing
    }

    /// The map's data, read: each map zoomed out with the point nearest its middle chosen. The songs' map comes first
    /// when there are no sounds on one yet.
    pub fn loaded(&mut self, data: MapData) {
        self.views = vec![View::new(&data.sounds), View::new(&data.songs)];
        self.tab = usize::from(data.sounds.points.is_empty() && !data.songs.points.is_empty());
        self.data = Some(Rc::new(data));
        self.version += 1;
        self.mark_palette();
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

    pub fn on_key(&mut self, name: &str, shift: bool) -> Asked {
        self.told = None;
        if let Some(prompt) = self.prompt.as_mut() {
            match name {
                "escape" => self.prompt = None,
                "backspace" => {
                    prompt.pop();
                }
                "enter" => {
                    let filter = self.prompt.take().unwrap_or_default();
                    self.refilter(&filter);
                    // The point the mouse is on, filtered out: its ring goes, and its hover ends.
                    if self.hovered.is_some_and(|(tab, at)| !self.views[tab].shown.contains(&at)) {
                        self.hovered = None;
                        return Asked::Leave;
                    }
                }
                _ => {}
            }
            return Asked::Nothing;
        }
        let way = match name {
            "up" => (0., 1.),
            "down" => (0., -1.),
            "left" => (-1., 0.),
            "right" => (1., 0.),
            // Esc lets a box being dragged go, then the points picked, then closes the map.
            "escape" if self.pressed.is_some() || self.boxed.is_some() => {
                (self.pressed, self.boxed) = (None, None);
                return Asked::Nothing;
            }
            "escape" if self.views.get(self.tab).is_some_and(|view| !view.picked.is_empty()) => {
                self.views[self.tab].picked.clear();
                self.version += 1;
                return Asked::Nothing;
            }
            "escape" => return Asked::Close,
            "enter" => return self.gather(),
            "space" => return self.play(),
            "tab" if self.data.is_some() => {
                self.tab = 1 - self.tab;
                // A hover on the map left behind ends: what it plays stops.
                let hovering = self.hovered.is_some() || self.from_hover;
                (self.said, self.hovered, self.version) = (None, None, self.version + 1);
                return if hovering { Asked::Leave } else { Asked::Nothing };
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
    /// what follows is its words), h turns hover plays off and on, q closes.
    pub fn on_text(&mut self, text: &str) -> Asked {
        self.told = None;
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
        match character {
            ' ' => return self.play(),
            '+' | '=' => self.zoom_by(STEP, None),
            '-' | '_' => self.zoom_by(1. / STEP, None),
            '/' if self.data.is_some() => self.prompt = Some(String::new()),
            // Picking: the point chosen picked or let go, every point the filter keeps, each picked one's nearest.
            'x' if self.data.is_some() && self.graphics != Graphics::Words => {
                if let Some(at) = self.views[self.tab].chosen {
                    self.toggle(at);
                }
            }
            'a' if self.data.is_some() && self.graphics != Graphics::Words => self.pick_all(),
            'g' if self.data.is_some() && self.graphics != Graphics::Words => self.grow(),
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

    /// A click chooses the point nearest it (or the group of a list's row); with ctrl it picks it, or lets it go. The
    /// button held and moved from there drags a box (`on_drag`).
    pub fn on_click(&mut self, x: i32, y: i32, ctrl: bool) -> Asked {
        self.told = None;
        let area = self.area;
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
        if let Some(at) = picked.filter(|_| ctrl) {
            self.toggle(at);
        }
        if picked.is_some() {
            self.views[self.tab].choose(layer, picked);
        }
        self.pressed = Some((x, y, ctrl));
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
        let (layer, view) = (self.layer()?, self.views.get(self.tab)?);
        let frame = Frame::new(area, view);
        let dot = (((x - area.x) * 2 + 1) as f64, ((y - area.y) * 4 + 2) as f64);
        nearest(layer, &view.shown, frame.at(dot), None).filter(|at| {
            let (px, py) = frame.dot(spot(&layer.points[*at as usize]));
            (px - dot.0).hypot(py - dot.1) <= REACH
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
        }
    }

    /// Shift and an arrow: a quarter of the view that way (nothing before the map has been drawn at a size that shows it).
    fn pan(&mut self, (ux, uy): (f64, f64)) {
        let area = self.area;
        let Some(view) = self.views.get_mut(self.tab).filter(|_| area.width > 0 && area.height > 0) else { return };
        let frame = Frame::new(area, view);
        view.centre = (view.centre.0 + ux * frame.width / frame.scale / 4., view.centre.1 + uy * frame.height / frame.scale / 4.);
        view.clamp();
        self.version += 1;
    }

    fn zoom_by(&mut self, by: f64, anchor: Option<(f64, f64)>) {
        let Some(data) = self.data.clone() else { return };
        let layer = if self.tab == 0 { &data.sounds } else { &data.songs };
        let view = &mut self.views[self.tab];
        let anchor = anchor.or_else(|| view.chosen.map(|at| spot(&layer.points[at as usize]))).unwrap_or(view.centre);
        view.zoom_by(by, anchor);
        self.version += 1;
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
        let (area, view) = (self.area, &self.views[self.tab]);
        let (cols, rows) = (area.width as usize, area.height as usize);
        let key = format!("map {} {} {cols}x{rows}", self.tab, self.version);
        let frame = Frame::new(area, view);
        if self.picture.as_ref().is_none_or(|picture| picture.key != key) {
            let image = self.graphics == Graphics::Kitty;
            // A patch of density a braille dot across (four pixels of an image); in braille its brightness in steps.
            let (scene, named, dense) = scene(layer, view, &frame, 1., (!image).then_some(6.));
            let picture = if image {
                let shrink = (MOST_PIXELS.0 / (cols as f64 * CELL_PIXELS.0)).min(MOST_PIXELS.1 / (rows as f64 * CELL_PIXELS.1)).min(1.);
                let (width, height) = ((cols as f64 * CELL_PIXELS.0 * shrink) as u32, (rows as f64 * CELL_PIXELS.1 * shrink) as u32);
                let image = Some(Raster::draw(&scene, width, height));
                Picture { key: key.clone(), rows: vec![], image, id: None, named, dense, pixels: Some((width as f64, height as f64)) }
            } else {
                Picture { key: key.clone(), rows: cells::braille(&scene, cols, rows), image: None, id: None, named, dense, pixels: None }
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
        let view = &self.views[self.tab];
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
                let name = truncate(&layer.points[at as usize].name, NAME_CELLS.min(area.x + area.width - x - 2));
                let width = text_width(&name);
                // The cell between the point and its name too: a name never runs into another point.
                let free: Option<Vec<usize>> = (x + 1..x + 2 + width).map(|column| index(column, y).filter(|at| !taken[*at])).collect();
                let Some(free) = free.filter(|_| width >= 3) else { continue };
                for at in free {
                    taken[at] = true;
                }
                let color = if Some(at) == chosen { palette::BRIGHT } else { palette::DIM };
                screen.put(x + 2, y, &name, &Style { bg: Some(palette::GROUND), ..Style::fg(color) });
            }
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
            screen.put(x, y, "●", &Style { bold: true, ..Style::fg(palette::BRIGHT) });
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
            let name = truncate(&format!("{playing}{}", point.name), (NAME_CELLS + 2).min(area.x + area.width - x - 2));
            if text_width(&name) >= 3 {
                screen.put(x + 2, y, &name, &Style { bg: Some(palette::GROUND), ..Style::fg(palette::BRIGHT) });
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
        if let Some(told) = &self.told {
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
        let picked = self.views.get(self.tab).map_or(0, |view| view.picked.len());
        let keys = if self.graphics == Graphics::Words {
            let gathers = if self.gathers { "enter adds to the palette · " } else { "" };
            format!("up and down choose · space plays · {gathers}/ filters · tab {other} · esc closes")
        } else if picked > 0 && self.gathers {
            format!(
                "enter adds {} to the palette · drag picks (ctrl adds) · x picks · a all · g grows · esc lets go",
                if picked == 1 { "1 point".into() } else { format!("{} points", grouped(picked)) }
            )
        } else {
            let hover = if self.hover { "plays" } else { "quiet" };
            let gathers = if self.gathers { "drag or x picks · enter adds to the palette · " } else { "" };
            format!(
                "arrows move · shift+arrows pan · + − zoom · {gathers}/ filters · space plays · h hover {hover} · tab {other} · esc closes"
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
        let view = View::new(&layer);
        let frame = Frame::new(Rect::new(0, 0, 80, 30), &view);
        let (drawn, named, dense) = scene(&layer, &view, &frame, 1., None);
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
        let (drawn, named, dense) = scene(&layer, &near, &Frame::new(Rect::new(0, 0, 80, 30), &near), 1., None);
        assert!(!dense);
        assert_eq!((drawn.shapes.len(), named.len()), (4, 4));
    }

    #[test]
    fn the_filter_keeps_what_has_every_word_and_the_choice_moves_to_what_it_keeps() {
        let mut layer = layer(&[(0.1, 0.1, 0), (0.5, 0.5, 1), (0.9, 0.9, 1)]);
        layer.places.push("Splice / Pads".into());
        layer.points[2].place = 1;
        let mut view = View::new(&layer);
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
        map.loaded(data);
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
        assert_eq!(map.views[0].centre, (0.5, 0.5));
    }

    #[test]
    fn zooming_keeps_its_anchor_where_it_was_and_zoomed_out_the_whole_map_shows() {
        let layer = layer(&[(0.2, 0.3, 0)]);
        let mut view = View::new(&layer);
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
        map.loaded(MapData { sounds, songs: layer(&[]) });
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
        map.loaded(MapData { sounds, songs: layer(&[]) });
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
        map.loaded(MapData { sounds: layer(&points), songs: layer(&[]) });
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

    /// A map of `points` that puts points into the palette, drawn once (100 × 30).
    fn gathering(points: &[(f32, f32, u16)]) -> MapView {
        let mut map = MapView::new(Graphics::Cells, true).gathering(true);
        map.loaded(MapData { sounds: layer(points), songs: layer(&[]) });
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
        plain.loaded(MapData { sounds: layer(&points), songs: layer(&[]) });
        let mut screen = Screen::new(100, 30);
        plain.draw(&mut screen, &mut |_, _, _, _| 0);
        assert_eq!(plain.on_key("enter", false), Asked::Nothing);
        assert!(!screen.lines()[29].contains("palette"), "{}", screen.lines()[29]);
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
        let (in_palette, picked, reached) = (cell_of(&map, 0), cell_of(&map, 1), cell_of(&map, 2));
        let at = |(x, y): (i32, i32)| screen.lines()[y as usize].chars().nth(x as usize).unwrap();
        assert_eq!((at(in_palette), at(picked)), ('•', '●'));
        // The chosen point (p2, the middle one) is drawn over its halo; let it go and the halo shows.
        assert_eq!(at(reached), '◉');
        map.views[0].chosen = None;
        let mut screen = Screen::new(100, 30);
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert_eq!(screen.lines()[reached.1 as usize].chars().nth(reached.0 as usize), Some('·'));
        map.views[0].chosen = Some(2);
        assert!(screen.lines()[28].contains("Added 2 sounds to Drums."), "{}", screen.lines()[28]);
        // The next key: said no more.
        map.on_text("+");
        let mut screen = Screen::new(100, 30);
        map.draw(&mut screen, &mut |_, _, _, _| 0);
        assert!(!screen.lines()[28].contains("Added"));
    }
}
