//! How a patcher is laid out, as numbers: what the survey measures in well-made devices, and what Kumi's own layout
//! is compared with. The rules say what mustn't happen; these say what good patchers do.

use std::collections::HashMap;

use super::geometry::{inlet_point, outlet_point, segments_cross, Rect};
use super::{MaxBox, Patcher};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shape {
    pub boxes: usize,
    pub cords: usize,
    /// Cords drawn with bends (midpoints).
    pub bent: usize,
    /// Places where two cords cross.
    pub crossings: usize,
    /// Boxes whose left edge lines up with another box's.
    pub in_columns: usize,
    /// For each cord that runs down, the room between its box's bottom and the next box's top.
    pub down_gaps: Vec<f64>,
    /// For each box with another beside it to the right (overlapping it in height), the room between them.
    pub side_gaps: Vec<f64>,
}

/// A patcher's layout, as numbers (its own boxes and cords, not those of the patchers inside it).
pub fn shape(patcher: &Patcher) -> Shape {
    let placed: Vec<(&MaxBox, Rect)> = patcher.boxes.iter().filter_map(|item| Some((item, item.rect()?))).collect();
    let by_id: HashMap<&str, (&MaxBox, Rect)> = placed.iter().map(|(item, rect)| (item.id(), (*item, *rect))).collect();
    let mut shape = Shape { boxes: placed.len(), ..Shape::default() };
    let mut segments: Vec<(usize, [f64; 2], [f64; 2])> = Vec::new();
    for (index, cord) in patcher.cords.iter().enumerate() {
        let (Some((from, a)), Some((to, b))) = (by_id.get(cord.from.as_str()), by_id.get(cord.to.as_str())) else { continue };
        shape.cords += 1;
        if !cord.midpoints.is_empty() {
            shape.bent += 1;
        }
        let start = outlet_point(a, from.outlets().max(cord.outlet + 1), cord.outlet);
        let end = inlet_point(b, to.inlets().max(cord.inlet + 1), cord.inlet);
        if b.y > a.bottom() {
            shape.down_gaps.push(b.y - a.bottom());
        }
        let points: Vec<[f64; 2]> = std::iter::once(start).chain(cord.midpoints.iter().copied()).chain(std::iter::once(end)).collect();
        segments.extend(points.windows(2).map(|pair| (index, pair[0], pair[1])));
    }
    for (at, (cord, a, b)) in segments.iter().enumerate() {
        shape.crossings += segments[at + 1..].iter().filter(|(other, c, d)| other != cord && segments_cross(*a, *b, *c, *d)).count();
    }
    for (item, rect) in &placed {
        if placed.iter().any(|(other, other_rect)| other.id() != item.id() && (other_rect.x - rect.x).abs() < 1.0) {
            shape.in_columns += 1;
        }
        let beside = placed
            .iter()
            .filter(|(_, other)| other.x >= rect.right() && other.y < rect.bottom() && rect.y < other.bottom())
            .map(|(_, other)| other.x - rect.right())
            .fold(f64::INFINITY, f64::min);
        if beside.is_finite() {
            shape.side_gaps.push(beside);
        }
    }
    shape
}

/// The value a share `at` (0 to 1) of the numbers lie below: the median at 0.5.
pub fn quantile(numbers: &[f64], at: f64) -> Option<f64> {
    let mut sorted: Vec<f64> = numbers.iter().copied().filter(|n| n.is_finite()).collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_by(f64::total_cmp);
    Some(sorted[((sorted.len() - 1) as f64 * at.clamp(0.0, 1.0)).round() as usize])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::patch::NoFiles;
    use serde_json::json;

    #[test]
    fn a_patchers_shape_counts_bends_crossings_columns_and_gaps() {
        let patcher = Patcher::read(
            &json!({ "boxes": [
                { "box": { "id": "a", "maxclass": "newobj", "text": "a", "patching_rect": [0., 0., 40., 22.], "numinlets": 1, "numoutlets": 1 } },
                { "box": { "id": "b", "maxclass": "newobj", "text": "b", "patching_rect": [100., 0., 40., 22.], "numinlets": 1, "numoutlets": 1 } },
                { "box": { "id": "c", "maxclass": "newobj", "text": "c", "patching_rect": [0., 60., 40., 22.], "numinlets": 1, "numoutlets": 1 } },
                { "box": { "id": "d", "maxclass": "newobj", "text": "d", "patching_rect": [100., 60., 40., 22.], "numinlets": 1, "numoutlets": 1 } }
            ], "lines": [
                { "patchline": { "source": ["a", 0], "destination": ["d", 0] } },
                { "patchline": { "source": ["b", 0], "destination": ["c", 0], "midpoints": [103.5, 40., 3.5, 40.] } }
            ] }),
            &NoFiles,
        );
        let shape = shape(&patcher);
        assert_eq!((shape.boxes, shape.cords, shape.bent, shape.crossings, shape.in_columns), (4, 2, 1, 1, 4));
        assert_eq!(shape.down_gaps, [38., 38.]);
        assert_eq!(shape.side_gaps, [60., 60.]);
        assert_eq!(quantile(&[3., 1., 2.], 0.5), Some(2.));
    }
}
