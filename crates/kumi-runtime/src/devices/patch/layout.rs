//! Kumi's layout for the patchers it builds, so a producer who opens one can read it: rows from top to bottom in the
//! order messages and audio flow, each box under what feeds it, and cords that run down between the rows and never over
//! a box, bent at right angles where they move across (as the measured devices draw them). A cord that closes a loop
//! goes round the side. Boxes nothing is wired to (a buffer~, a dict, a comment) sit together to the right.
//!
//! It's the layered way of drawing a graph: set loops aside, give each box a row, order each row so few cords cross,
//! then slide boxes along their rows to line up under what feeds them. Audio is the spine: its cords count more than
//! messages when rows are ordered and boxes lined up.
//!
//! It's for the patchers Kumi builds, which make every order that matters explicit with a trigger (the standard's
//! order rule), so moving a box never changes what the patcher does. Max numbers a subpatcher's inlet and outlet
//! objects by where they sit, so they stay in their order, inlets in the top row and outlets in the bottom one.

use std::collections::{BTreeMap, HashMap, VecDeque};

use super::geometry::{port_x, Rect};
use super::Patcher;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spacing {
    /// The least room between one row and the next.
    pub row_gap: f64,
    /// The least room between boxes side by side.
    pub column_gap: f64,
    /// The room between cords running side by side.
    pub lane: f64,
    /// The room around the whole.
    pub margin: f64,
}

/// From the measured devices: about 30 pixels from a box down to the next at the median, and at least about 24
/// between boxes side by side.
pub const SPACING: Spacing = Spacing { row_gap: 30.0, column_gap: 24.0, lane: 8.0, margin: 24.0 };

/// Lays out a patcher and the patchers embedded in it.
pub fn arrange(patcher: &mut Patcher) {
    arrange_with(patcher, &SPACING);
}

pub fn arrange_with(patcher: &mut Patcher, spacing: &Spacing) {
    let size = patcher.fields.get("default_fontsize").and_then(serde_json::Value::as_f64).unwrap_or(12.0);
    for item in &mut patcher.boxes {
        if let Some(inner) = item.patcher.as_mut() {
            arrange_with(inner, spacing);
        }
        // A box as wide as its text, at least: Max clips what doesn't fit.
        if let (Some(rect), "newobj" | "message") = (item.rect(), item.maxclass()) {
            let size = item.fields.get("fontsize").and_then(serde_json::Value::as_f64).unwrap_or(size);
            let needed = text_width(item.text(), size);
            if rect.w < needed {
                item.set_rect(Rect::new(rect.x, rect.y, needed.ceil(), rect.h));
            }
        }
    }
    let index: HashMap<&str, usize> =
        patcher.boxes.iter().enumerate().filter(|(_, item)| item.rect().is_some()).map(|(at, item)| (item.id(), at)).collect();
    let wires: Vec<Wire> = patcher
        .cords
        .iter()
        .enumerate()
        .filter_map(|(cord, line)| {
            let from = *index.get(line.from.as_str())?;
            let weight = if patcher.boxes[from].sends_signal(line.outlet) { AUDIO } else { 1.0 };
            Some(Wire { cord, from, outlet: line.outlet, to: *index.get(line.to.as_str())?, inlet: line.inlet, weight })
        })
        .collect();
    // Boxes wired together are drawn together, each group beside the last, in the order the patcher lists them.
    let mut group: Vec<usize> = (0..patcher.boxes.len()).collect();
    fn root(group: &mut [usize], at: usize) -> usize {
        let mut at = at;
        while group[at] != at {
            group[at] = group[group[at]];
            at = group[at];
        }
        at
    }
    for wire in &wires {
        let (a, b) = (root(&mut group, wire.from), root(&mut group, wire.to));
        group[a.max(b)] = a.min(b);
    }
    let mut wired = vec![false; patcher.boxes.len()];
    for wire in &wires {
        wired[wire.from] = true;
        wired[wire.to] = true;
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (at, _) in wired.iter().enumerate().filter(|(_, wired)| **wired) {
        let top = root(&mut group, at);
        groups.entry(top).or_default().push(at);
    }
    let mut placed: HashMap<usize, (f64, f64)> = HashMap::new();
    let mut routes: HashMap<usize, Vec<[f64; 2]>> = HashMap::new();
    let mut left = spacing.margin;
    for members in groups.values() {
        let inside: Vec<&Wire> = wires.iter().filter(|wire| members.binary_search(&wire.from).is_ok()).collect();
        let drawn = draw(patcher, spacing, members, &inside, left);
        placed.extend(drawn.placed);
        routes.extend(drawn.routes);
        left = drawn.right + spacing.column_gap * 2.0;
    }
    let mut y = spacing.margin;
    for (at, item) in patcher.boxes.iter().enumerate() {
        if let (false, Some(rect)) = (wired[at], item.rect()) {
            placed.insert(at, (left, y));
            y += rect.h + spacing.row_gap / 2.0;
        }
    }
    for (at, (x, y)) in placed {
        let item = &mut patcher.boxes[at];
        if let Some(rect) = item.rect() {
            item.set_rect(Rect::new(x, y, rect.w, rect.h));
        }
    }
    for wire in &wires {
        patcher.cords[wire.cord].midpoints = routes.remove(&wire.cord).unwrap_or_default();
    }
}

/// About how wide Max draws a line of text in Arial Bold at `size` points, with a box's margins.
fn text_width(text: &str, size: f64) -> f64 {
    let at_ten: f64 = text
        .chars()
        .map(|c| match c {
            'i' | 'j' | 'l' | '.' | ',' | ':' | ';' | '\'' | '!' | '|' => 3.0,
            'f' | 't' | 'r' | ' ' | '(' | ')' | '[' | ']' | '-' | '1' => 4.0,
            'm' | 'w' | 'M' | 'W' => 9.0,
            c if c.is_uppercase() => 7.0,
            _ => 6.0,
        })
        .sum();
    at_ten * size / 10.0 + 12.0
}

/// A cord between two boxes that are laid out: their indexes in the patcher.
#[derive(Debug, Clone, Copy)]
struct Wire {
    cord: usize,
    from: usize,
    outlet: usize,
    to: usize,
    inlet: usize,
    weight: f64,
}

/// How much more an audio cord counts than a message cord when rows are ordered and boxes lined up.
const AUDIO: f64 = 3.0;

/// Where a box goes because of what it is: an inlet object in the top row, an outlet object in the bottom one, each
/// with its place among the others (Max numbers inlet and outlet objects by position, gen's in and out by argument).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pin {
    Free,
    Top(f64),
    Bottom(f64),
}

fn pin(item: &super::MaxBox) -> Pin {
    let along = item.rect().map_or(0.0, |rect| rect.x);
    let numbered = item.args().first().and_then(|arg| arg.parse::<f64>().ok());
    match item.class() {
        "inlet" => Pin::Top(along),
        "outlet" => Pin::Bottom(along),
        "in" | "in~" => Pin::Top(numbered.unwrap_or(along)),
        "out" | "out~" => Pin::Bottom(numbered.unwrap_or(along)),
        _ => Pin::Free,
    }
}

/// A box in a row, or a point where a long cord passes a row (no box: no width, one way in and out).
#[derive(Debug, Clone)]
struct Node {
    item: Option<usize>,
    w: f64,
    h: f64,
    inlets: usize,
    outlets: usize,
    layer: usize,
    x: f64,
    pin: Pin,
}

impl Node {
    fn outlet_offset(&self, outlet: usize) -> f64 {
        match self.item {
            Some(_) => port_x(&Rect::new(0.0, 0.0, self.w, self.h), self.outlets.max(outlet + 1), outlet),
            None => 0.0,
        }
    }

    fn inlet_offset(&self, inlet: usize) -> f64 {
        match self.item {
            Some(_) => port_x(&Rect::new(0.0, 0.0, self.w, self.h), self.inlets.max(inlet + 1), inlet),
            None => 0.0,
        }
    }
}

/// A step of a cord from one row to the next.
#[derive(Debug, Clone, Copy)]
struct Link {
    from: usize,
    outlet: usize,
    to: usize,
    inlet: usize,
    weight: f64,
}

struct Drawn {
    placed: HashMap<usize, (f64, f64)>,
    routes: HashMap<usize, Vec<[f64; 2]>>,
    right: f64,
}

/// Lays out one group of boxes wired together, from `left`.
fn draw(patcher: &Patcher, spacing: &Spacing, members: &[usize], wires: &[&Wire], left: f64) -> Drawn {
    let local: HashMap<usize, usize> = members.iter().enumerate().map(|(n, &at)| (at, n)).collect();
    let mut nodes: Vec<Node> = members
        .iter()
        .map(|&at| {
            let item = &patcher.boxes[at];
            let rect = item.rect().expect("a laid-out box");
            Node { item: Some(at), w: rect.w, h: rect.h, inlets: item.inlets(), outlets: item.outlets(), layer: 0, x: 0.0, pin: pin(item) }
        })
        .collect();
    let count = nodes.len();
    let edges: Vec<(usize, Link)> = wires
        .iter()
        .map(|wire| {
            (wire.cord, Link { from: local[&wire.from], outlet: wire.outlet, to: local[&wire.to], inlet: wire.inlet, weight: wire.weight })
        })
        .collect();
    let loops = loop_edges(count, &edges);
    let forward: Vec<usize> = (0..edges.len()).filter(|&e| !loops[e]).collect();
    layer(&mut nodes, &edges, &forward);
    // Each forward cord, row by row: a long one passes each row between through a point of its own.
    let mut chains: Vec<(usize, Vec<Link>)> = Vec::new();
    for &e in &forward {
        let (cord, link) = edges[e];
        let (top, bottom) = (nodes[link.from].layer, nodes[link.to].layer);
        let mut links = Vec::new();
        let mut from = (link.from, link.outlet);
        for layer in top + 1..bottom {
            nodes.push(Node { item: None, w: 0.0, h: 0.0, inlets: 1, outlets: 1, layer, x: 0.0, pin: Pin::Free });
            let point = nodes.len() - 1;
            links.push(Link { from: from.0, outlet: from.1, to: point, inlet: 0, weight: link.weight });
            from = (point, 0);
        }
        links.push(Link { from: from.0, outlet: from.1, to: link.to, inlet: link.inlet, weight: link.weight });
        chains.push((cord, links));
    }
    let links: Vec<Link> = chains.iter().flat_map(|(_, links)| links.iter().copied()).collect();
    let rows = order(&nodes, &links);
    place(&mut nodes, &rows, &links, spacing, left);
    // The gaps: 0 above the first row, k between rows k-1 and k, the last below the last row.
    let gaps = rows.len() + 1;
    let mut jogs: Vec<Jog> = Vec::new();
    for (chain, (_, links)) in chains.iter().enumerate() {
        for (step, link) in links.iter().enumerate() {
            let (a, b) = (&nodes[link.from], &nodes[link.to]);
            let (xa, xb) = (a.x + a.outlet_offset(link.outlet), b.x + b.inlet_offset(link.inlet));
            if (xa - xb).abs() >= 0.5 {
                jogs.push(Jog {
                    gap: b.layer,
                    share: (link.from, link.outlet),
                    left: xa.min(xb),
                    right: xa.max(xb),
                    owner: (chain, step),
                    track: 0,
                });
            }
        }
    }
    let right_of_boxes = nodes.iter().map(|node| node.x + node.w).fold(left, f64::max);
    let loop_list: Vec<(usize, Link)> = (0..edges.len()).filter(|&e| loops[e]).map(|e| edges[e]).collect();
    let mut sides = Vec::new();
    for (n, (_, link)) in loop_list.iter().enumerate() {
        let side = right_of_boxes + spacing.column_gap / 2.0 + n as f64 * spacing.lane;
        sides.push(side);
        let (from, to) = (&nodes[link.from], &nodes[link.to]);
        let out = from.x + from.outlet_offset(link.outlet);
        let into = to.x + to.inlet_offset(link.inlet);
        let owner = (usize::MAX, n);
        jogs.push(Jog { gap: from.layer + 1, share: (usize::MAX, n * 2), left: out.min(side), right: out.max(side), owner, track: 0 });
        jogs.push(Jog { gap: to.layer, share: (usize::MAX, n * 2 + 1), left: into.min(side), right: into.max(side), owner, track: 0 });
    }
    let tracks = tracks(&mut jogs, gaps, spacing.lane);
    // Each gap's height, then each row's top.
    let heights: Vec<f64> = (0..gaps)
        .map(|gap| {
            let needed = if tracks[gap] == 0 { 0.0 } else { (tracks[gap] + 1) as f64 * spacing.lane };
            if gap == 0 || gap == gaps - 1 {
                needed
            } else {
                needed.max(spacing.row_gap)
            }
        })
        .collect();
    let mut tops = Vec::with_capacity(rows.len());
    let mut y = spacing.margin + heights[0];
    for (row, members) in rows.iter().enumerate() {
        tops.push(y);
        y += members.iter().map(|&n| nodes[n].h).fold(0.0, f64::max) + heights[row + 1];
    }
    let gap_top = |gap: usize| -> f64 {
        if gap == 0 {
            spacing.margin
        } else {
            tops[gap - 1] + rows[gap - 1].iter().map(|&n| nodes[n].h).fold(0.0, f64::max)
        }
    };
    let track_y = |jog: &Jog| -> f64 {
        let (top, height, count) = (gap_top(jog.gap), heights[jog.gap], tracks[jog.gap]);
        let first = top + (height - (count as f64 - 1.0) * spacing.lane) / 2.0;
        first + jog.track as f64 * spacing.lane
    };
    let mut routes: HashMap<usize, Vec<[f64; 2]>> = HashMap::new();
    let by_owner: HashMap<(usize, usize), Vec<&Jog>> = jogs.iter().fold(HashMap::new(), |mut map, jog| {
        map.entry(jog.owner).or_default().push(jog);
        map
    });
    for (chain, (cord, links)) in chains.iter().enumerate() {
        let mut points = Vec::new();
        // The x the cord runs down at: it keeps it past a row it doesn't move across in, so its bends stay square.
        let mut along = links.first().map_or(0.0, |link| nodes[link.from].x + nodes[link.from].outlet_offset(link.outlet));
        for (step, link) in links.iter().enumerate() {
            if let Some(jog) = by_owner.get(&(chain, step)).and_then(|jogs| jogs.first()) {
                let y = track_y(jog);
                let to = nodes[link.to].x + nodes[link.to].inlet_offset(link.inlet);
                points.push([along, y]);
                points.push([to, y]);
                along = to;
            }
        }
        routes.insert(*cord, points);
    }
    for (n, (cord, link)) in loop_list.iter().enumerate() {
        let jogs = &by_owner[&(usize::MAX, n)];
        let (below, above) = (track_y(jogs[0]), track_y(jogs[1]));
        let (from, to) = (&nodes[link.from], &nodes[link.to]);
        let out = from.x + from.outlet_offset(link.outlet);
        let into = to.x + to.inlet_offset(link.inlet);
        routes.insert(*cord, vec![[out, below], [sides[n], below], [sides[n], above], [into, above]]);
    }
    let placed = nodes.iter().filter_map(|node| Some((node.item?, (node.x, tops[node.layer])))).collect();
    let right = sides.last().map_or(right_of_boxes, |side| side + spacing.lane);
    Drawn { placed, routes, right }
}

/// Which cords close a loop: those that reach back to a box still being followed, from the boxes nothing feeds first.
fn loop_edges(count: usize, edges: &[(usize, Link)]) -> Vec<bool> {
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut fed = vec![false; count];
    for (e, (_, link)) in edges.iter().enumerate() {
        out[link.from].push(e);
        fed[link.to] = true;
    }
    let mut state = vec![0u8; count];
    let mut loops = vec![false; edges.len()];
    for start in (0..count).filter(|&n| !fed[n]).chain(0..count) {
        if state[start] != 0 {
            continue;
        }
        state[start] = 1;
        let mut stack = vec![(start, 0usize)];
        while let Some(top) = stack.last_mut() {
            let node = top.0;
            if top.1 < out[node].len() {
                let e = out[node][top.1];
                top.1 += 1;
                let to = edges[e].1.to;
                match state[to] {
                    0 => {
                        state[to] = 1;
                        stack.push((to, 0));
                    }
                    1 => loops[e] = true,
                    _ => {}
                }
            } else {
                state[node] = 2;
                stack.pop();
            }
        }
    }
    loops
}

/// Each box's row: one below everything that feeds it, then a box nearer its destinations than its sources moved
/// down beside them, and empty rows closed up.
fn layer(nodes: &mut [Node], edges: &[(usize, Link)], forward: &[usize]) {
    let count = nodes.len();
    let mut ins = vec![0usize; count];
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); count];
    for &e in forward {
        ins[edges[e].1.to] += 1;
        out[edges[e].1.from].push(e);
    }
    let mut waiting = ins.clone();
    let mut queue: VecDeque<usize> = (0..count).filter(|&n| ins[n] == 0).collect();
    let mut order = Vec::with_capacity(count);
    while let Some(node) = queue.pop_front() {
        order.push(node);
        for &e in &out[node] {
            let to = edges[e].1.to;
            nodes[to].layer = nodes[to].layer.max(nodes[node].layer + 1);
            waiting[to] -= 1;
            if waiting[to] == 0 {
                queue.push_back(to);
            }
        }
    }
    for &node in order.iter().rev() {
        let Some(nearest) = out[node].iter().map(|&e| nodes[edges[e].1.to].layer).min() else { continue };
        if nearest > nodes[node].layer + 1 && (ins[node] == 0 || out[node].len() > ins[node]) {
            nodes[node].layer = nearest - 1;
        }
    }
    let deepest = nodes.iter().map(|node| node.layer).max().unwrap_or(0);
    for n in 0..count {
        match nodes[n].pin {
            Pin::Top(_) if ins[n] == 0 => nodes[n].layer = 0,
            Pin::Bottom(_) if out[n].is_empty() => nodes[n].layer = deepest,
            _ => {}
        }
    }
    let mut used: Vec<usize> = nodes.iter().map(|node| node.layer).collect();
    used.sort_unstable();
    used.dedup();
    for node in nodes.iter_mut() {
        node.layer = used.binary_search(&node.layer).expect("a used row");
    }
}

/// The rows, each ordered so few cords cross: boxes as the patcher lists them, then sweeps that put each box at the
/// average place of what it's wired to (inlets and outlets counted where they sit), keeping the order that crossed least.
fn order(nodes: &[Node], links: &[Link]) -> Vec<Vec<usize>> {
    let depth = nodes.iter().map(|node| node.layer).max().map_or(0, |deepest| deepest + 1);
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); depth];
    // A point a cord passes sits where the box it comes from sits, among the boxes as listed.
    let mut seed = vec![0.0f64; nodes.len()];
    for (n, node) in nodes.iter().enumerate() {
        seed[n] = node.item.map_or(f64::MAX, |item| item as f64);
    }
    for link in links {
        if nodes[link.to].item.is_none() {
            seed[link.to] = seed[link.from] + 0.5;
        }
    }
    for (n, node) in nodes.iter().enumerate() {
        rows[node.layer].push(n);
    }
    for row in &mut rows {
        row.sort_by(|a, b| seed[*a].total_cmp(&seed[*b]));
        keep_pins(row, nodes);
    }
    let mut best = rows.clone();
    let mut fewest = crossings(nodes, &rows, links);
    for sweep in 0..8 {
        let mut place = vec![0.0f64; nodes.len()];
        for row in &rows {
            for (at, &n) in row.iter().enumerate() {
                place[n] = at as f64;
            }
        }
        let downward = sweep % 2 == 0;
        let layers: Vec<usize> = if downward { (1..depth).collect() } else { (0..depth.saturating_sub(1)).rev().collect() };
        for layer in layers {
            let mut keys: HashMap<usize, (f64, f64)> = HashMap::new();
            for link in links {
                let (n, other, fraction) = if downward {
                    (link.to, link.from, (link.outlet as f64 + 0.5) / nodes[link.from].outlets.max(1) as f64)
                } else {
                    (link.from, link.to, (link.inlet as f64 + 0.5) / nodes[link.to].inlets.max(1) as f64)
                };
                if nodes[n].layer == layer {
                    let entry = keys.entry(n).or_insert((0.0, 0.0));
                    entry.0 += link.weight * (place[other] + fraction);
                    entry.1 += link.weight;
                }
            }
            let key = |n: usize| keys.get(&n).map_or(place[n], |(sum, weight)| sum / weight);
            rows[layer].sort_by(|a, b| key(*a).total_cmp(&key(*b)));
            keep_pins(&mut rows[layer], nodes);
            for (at, &n) in rows[layer].iter().enumerate() {
                place[n] = at as f64;
            }
        }
        let crossed = crossings(nodes, &rows, links);
        if crossed < fewest {
            fewest = crossed;
            best = rows.clone();
        }
    }
    best
}

/// Puts a row's inlet and outlet objects back in their own order, in the places the row's order gave them.
fn keep_pins(row: &mut [usize], nodes: &[Node]) {
    let key = |n: usize| match nodes[n].pin {
        Pin::Top(along) | Pin::Bottom(along) => Some(along),
        Pin::Free => None,
    };
    let places: Vec<usize> = (0..row.len()).filter(|&at| key(row[at]).is_some()).collect();
    let mut pinned: Vec<usize> = places.iter().map(|&at| row[at]).collect();
    pinned.sort_by(|a, b| key(*a).unwrap_or(0.0).total_cmp(&key(*b).unwrap_or(0.0)));
    for (at, n) in places.into_iter().zip(pinned) {
        row[at] = n;
    }
}

/// How many pairs of cords cross between neighbouring rows, as the rows are ordered.
fn crossings(nodes: &[Node], rows: &[Vec<usize>], links: &[Link]) -> usize {
    let mut place = vec![0.0f64; nodes.len()];
    for row in rows {
        for (at, &n) in row.iter().enumerate() {
            place[n] = at as f64;
        }
    }
    let mut by_row: HashMap<usize, Vec<(f64, f64)>> = HashMap::new();
    for link in links {
        let top = place[link.from] + (link.outlet as f64 + 0.5) / nodes[link.from].outlets.max(1) as f64;
        let bottom = place[link.to] + (link.inlet as f64 + 0.5) / nodes[link.to].inlets.max(1) as f64;
        by_row.entry(nodes[link.from].layer).or_default().push((top, bottom));
    }
    by_row
        .values()
        .map(|pairs| {
            let mut crossed = 0;
            for (at, a) in pairs.iter().enumerate() {
                crossed += pairs[at + 1..].iter().filter(|b| (a.0 - b.0) * (a.1 - b.1) < 0.0).count();
            }
            crossed
        })
        .sum()
}

/// Each node's x: the rows packed from `left`, then sweeps down and up sliding each box toward where its inlets line
/// up under what feeds it (or its outlets over what it feeds), keeping each row's order and the room between boxes.
fn place(nodes: &mut [Node], rows: &[Vec<usize>], links: &[Link], spacing: &Spacing, left: f64) {
    let room = |a: &Node, b: &Node| -> f64 {
        let gap = match (a.item, b.item) {
            (Some(_), Some(_)) => spacing.column_gap,
            (None, None) => spacing.lane,
            _ => spacing.column_gap / 2.0,
        };
        a.w + gap
    };
    for row in rows {
        let mut x = left;
        for (at, &n) in row.iter().enumerate() {
            nodes[n].x = x;
            if let Some(&next) = row.get(at + 1) {
                x += room(&nodes[n], &nodes[next]);
            }
        }
    }
    let mut into: Vec<Vec<Link>> = vec![Vec::new(); nodes.len()];
    let mut out: Vec<Vec<Link>> = vec![Vec::new(); nodes.len()];
    for link in links {
        into[link.to].push(*link);
        out[link.from].push(*link);
    }
    let sweeps: Vec<bool> = [true, false].repeat(4).into_iter().chain([true]).collect();
    for downward in sweeps {
        let layers: Vec<usize> = if downward { (1..rows.len()).collect() } else { (0..rows.len().saturating_sub(1)).rev().collect() };
        for layer in layers {
            let row = &rows[layer];
            let mut desired = Vec::with_capacity(row.len());
            let mut weights = Vec::with_capacity(row.len());
            for &n in row {
                let wanted: Vec<(f64, f64)> = if downward {
                    into[n]
                        .iter()
                        .map(|link| {
                            (
                                nodes[link.from].x + nodes[link.from].outlet_offset(link.outlet) - nodes[n].inlet_offset(link.inlet),
                                link.weight,
                            )
                        })
                        .collect()
                } else {
                    out[n]
                        .iter()
                        .map(|link| {
                            (nodes[link.to].x + nodes[link.to].inlet_offset(link.inlet) - nodes[n].outlet_offset(link.outlet), link.weight)
                        })
                        .collect()
                };
                match median(wanted) {
                    Some(x) => {
                        desired.push(x);
                        weights.push(if nodes[n].item.is_some() { 1.0 } else { 2.0 });
                    }
                    None => {
                        desired.push(nodes[n].x);
                        weights.push(0.01);
                    }
                }
            }
            let gaps: Vec<f64> = row.windows(2).map(|pair| room(&nodes[pair[0]], &nodes[pair[1]])).collect();
            for (&n, x) in row.iter().zip(settle(&desired, &weights, &gaps)) {
                nodes[n].x = x;
            }
        }
    }
    let leftmost = nodes.iter().map(|node| node.x).fold(f64::INFINITY, f64::min);
    if leftmost.is_finite() {
        for node in nodes.iter_mut() {
            node.x += left - leftmost;
        }
    }
}

/// The weighted median: the value at which half the weight lies on either side (the mean of the two middle values when
/// it falls between them).
fn median(mut numbers: Vec<(f64, f64)>) -> Option<f64> {
    if numbers.is_empty() {
        return None;
    }
    numbers.sort_by(|a, b| a.0.total_cmp(&b.0));
    let half = numbers.iter().map(|(_, weight)| weight).sum::<f64>() / 2.0;
    let mut below = 0.0;
    for (at, (value, weight)) in numbers.iter().enumerate() {
        below += weight;
        if below > half + 1e-9 {
            return Some(*value);
        }
        if (below - half).abs() <= 1e-9 {
            return Some(numbers.get(at + 1).map_or(*value, |next| (value + next.0) / 2.0));
        }
    }
    numbers.last().map(|(value, _)| *value)
}

/// Positions as near the desired ones as the order and the room between them allow (least squares, weighted): pooling
/// neighbours that would cross, as isotonic regression does.
fn settle(desired: &[f64], weights: &[f64], gaps: &[f64]) -> Vec<f64> {
    let mut offset = vec![0.0; desired.len()];
    for at in 1..desired.len() {
        offset[at] = offset[at - 1] + gaps[at - 1];
    }
    let mut pools: Vec<(f64, f64, usize)> = Vec::new();
    for at in 0..desired.len() {
        pools.push((weights[at], weights[at] * (desired[at] - offset[at]), 1));
        while pools.len() > 1 {
            let (w2, s2, n2) = pools[pools.len() - 1];
            let (w1, s1, n1) = pools[pools.len() - 2];
            if s1 / w1 <= s2 / w2 {
                break;
            }
            pools.truncate(pools.len() - 2);
            pools.push((w1 + w2, s1 + s2, n1 + n2));
        }
    }
    let mut placed = Vec::with_capacity(desired.len());
    for (weight, sum, count) in pools {
        for _ in 0..count {
            placed.push(sum / weight + offset[placed.len()]);
        }
    }
    placed
}

/// A cord's move across a gap between rows, at the height of its track.
#[derive(Debug, Clone)]
struct Jog {
    gap: usize,
    /// Cords leaving one outlet share a track, as a fan-out is drawn.
    share: (usize, usize),
    left: f64,
    right: f64,
    /// The cord's chain and step (or a loop's), to find its track again.
    owner: (usize, usize),
    track: usize,
}

/// Gives each jog a track in its gap, cords from one outlet together and spans that would overlap apart, and says how
/// many tracks each gap needs.
fn tracks(jogs: &mut [Jog], gaps: usize, lane: f64) -> Vec<usize> {
    let mut needed = vec![0usize; gaps];
    for (gap, needs) in needed.iter_mut().enumerate() {
        let mut spans: BTreeMap<(usize, usize), (f64, f64)> = BTreeMap::new();
        for jog in jogs.iter().filter(|jog| jog.gap == gap) {
            let span = spans.entry(jog.share).or_insert((jog.left, jog.right));
            span.0 = span.0.min(jog.left);
            span.1 = span.1.max(jog.right);
        }
        let mut ordered: Vec<((usize, usize), (f64, f64))> = spans.into_iter().collect();
        ordered.sort_by(|a, b| a.1 .0.total_cmp(&b.1 .0));
        let mut ends: Vec<f64> = Vec::new();
        let mut given: HashMap<(usize, usize), usize> = HashMap::new();
        for (share, (start, end)) in ordered {
            let track = match ends.iter().position(|&last| last + lane <= start) {
                Some(track) => track,
                None => {
                    ends.push(f64::NEG_INFINITY);
                    ends.len() - 1
                }
            };
            ends[track] = end;
            given.insert(share, track);
        }
        *needs = ends.len();
        for jog in jogs.iter_mut().filter(|jog| jog.gap == gap) {
            jog.track = given[&jog.share];
        }
    }
    needed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::patch::check::{check, CORD_OVER_BOX, CORD_UPWARD};
    use crate::devices::patch::{NoFiles, Patcher};
    use serde_json::{json, Value};

    fn newobj(id: &str, text: &str, width: f64, ins: usize, outs: usize) -> Value {
        json!({ "box": { "id": id, "maxclass": "newobj", "text": text, "patching_rect": [500., 500., width, 22.], "numinlets": ins, "numoutlets": outs, "outlettype": vec![""; outs] } })
    }

    fn cord(from: &str, outlet: usize, to: &str, inlet: usize) -> Value {
        json!({ "patchline": { "source": [from, outlet], "destination": [to, inlet] } })
    }

    fn laid_out(value: Value) -> Patcher {
        let mut patcher = Patcher::read(&value, &NoFiles);
        arrange(&mut patcher);
        patcher
    }

    fn rect(patcher: &Patcher, id: &str) -> Rect {
        patcher.find(id).and_then(|item| item.rect()).expect(id)
    }

    #[test]
    fn a_chain_runs_straight_down_each_box_under_the_one_that_feeds_it() {
        let patcher = laid_out(json!({ "boxes": [
            newobj("c", "print", 40., 1, 0), newobj("a", "metro 100", 70., 2, 1), newobj("b", "counter", 60., 5, 4)
        ], "lines": [cord("a", 0, "b", 0), cord("b", 0, "c", 0)] }));
        let (a, b, c) = (rect(&patcher, "a"), rect(&patcher, "b"), rect(&patcher, "c"));
        assert_eq!((a.x, b.x, c.x), (24., 24., 24.), "outlet 0 over inlet 0, so the cords are straight");
        assert!(a.bottom() < b.y && b.bottom() < c.y, "{a:?} {b:?} {c:?}");
        assert_eq!(b.y - a.bottom(), SPACING.row_gap);
        assert!(patcher.cords.iter().all(|cord| cord.midpoints.is_empty()));
        assert_eq!(check(&patcher).findings, Vec::new());
    }

    #[test]
    fn cords_that_move_across_bend_in_the_gaps_and_never_run_over_a_box() {
        // Three controls into one box's three inlets, a source beside them, a long cord past a row.
        let patcher = laid_out(json!({ "boxes": [
            newobj("sum", "pak 0 0 0", 140., 3, 1),
            newobj("x", "prepend x", 60., 1, 1), newobj("y", "prepend y", 60., 1, 1), newobj("z", "prepend z", 60., 1, 1),
            newobj("in", "notein", 60., 1, 3), newobj("mid", "+ 1", 40., 2, 1), newobj("out", "print", 40., 1, 0)
        ], "lines": [
            cord("x", 0, "sum", 2), cord("y", 0, "sum", 1), cord("z", 0, "sum", 0),
            cord("in", 0, "mid", 0), cord("mid", 0, "x", 0), cord("in", 1, "y", 0), cord("in", 2, "sum", 0), cord("sum", 0, "out", 0)
        ] }));
        let report = check(&patcher);
        assert_eq!(report.findings, Vec::new(), "{:#?}", patcher.to_value());
        for cord in &patcher.cords {
            for pair in cord.midpoints.windows(2) {
                assert!(pair[0][0] == pair[1][0] || pair[0][1] == pair[1][1], "bends at right angles: {:?}", cord.midpoints);
            }
        }
    }

    #[test]
    fn a_cord_that_closes_a_loop_goes_round_the_side_and_loose_boxes_sit_to_the_right() {
        let patcher = laid_out(json!({ "boxes": [
            newobj("a", "t b i", 50., 1, 2), newobj("b", "+ 1", 40., 2, 1), newobj("store", "buffer~ ---grains", 110., 1, 2)
        ], "lines": [cord("a", 1, "b", 0), cord("b", 0, "a", 0)] }));
        let report = check(&patcher);
        assert!(report.findings.iter().all(|finding| finding.rule != CORD_OVER_BOX && finding.rule != CORD_UPWARD), "{report:#?}");
        let back = &patcher.cords[1];
        assert_eq!(back.midpoints.len(), 4, "down, round the side, up, across: {:?}", back.midpoints);
        let side = back.midpoints[1][0];
        assert!(side > rect(&patcher, "a").right() && side > rect(&patcher, "b").right());
        assert!(rect(&patcher, "store").x > side, "the buffer~ sits apart, right of what's wired");
    }

    #[test]
    fn a_box_too_narrow_for_its_text_is_widened() {
        let patcher = laid_out(
            json!({ "default_fontsize": 10.0, "boxes": [newobj("a", "prepend kumi_output", 80., 1, 1), newobj("b", "+ 1", 40., 2, 1)],
            "lines": [cord("a", 0, "b", 0)] }),
        );
        assert!(rect(&patcher, "a").w > 100., "{:?}", rect(&patcher, "a"));
        assert_eq!(rect(&patcher, "b").w, 40., "wide enough already");
    }

    #[test]
    fn rows_settle_toward_least_squares_keeping_order_and_room() {
        assert_eq!(settle(&[10., 10.], &[1., 1.], &[20.]), vec![0., 20.], "two wanting one place share it");
        assert_eq!(settle(&[0., 100.], &[1., 1.], &[20.]), vec![0., 100.], "room enough: each where it wants");
        assert_eq!(settle(&[10., 10.], &[3., 1.], &[20.]), vec![5., 25.], "the heavier keeps nearer");
    }
}
