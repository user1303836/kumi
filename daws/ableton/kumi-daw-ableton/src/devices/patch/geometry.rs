//! Where things sit in a patcher: a box's rectangle, its inlets and outlets, and the segments a cord is drawn with.

use serde_json::{json, Value};

/// How wide Max draws an inlet or an outlet; a cord meets it in the middle.
pub const PORT: f64 = 7.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    /// A rectangle as Max saves one: [x, y, width, height].
    pub fn of(value: &Value) -> Option<Rect> {
        let numbers: Vec<f64> = value.as_array()?.iter().filter_map(Value::as_f64).collect();
        match numbers[..] {
            [x, y, w, h] if [x, y, w, h].iter().all(|n| n.is_finite()) && w >= 0. && h >= 0. => Some(Rect { x, y, w, h }),
            _ => None,
        }
    }

    pub fn to_value(self) -> Value {
        json!([self.x, self.y, self.w, self.h])
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// Whether the two share more than an edge.
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.right() && other.x < self.right() && self.y < other.bottom() && other.y < self.bottom()
    }

    /// Whether the segment from `a` to `b` runs through the rectangle's inside, `inset` in from each edge: a cord
    /// that only touches an edge, or ends on one, doesn't count.
    pub fn crossed_by(&self, a: [f64; 2], b: [f64; 2], inset: f64) -> bool {
        let (left, top, right, bottom) = (self.x + inset, self.y + inset, self.right() - inset, self.bottom() - inset);
        if left >= right || top >= bottom {
            return false;
        }
        // Liang–Barsky: clip the segment's span t in 0..1 against each edge.
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let (mut from, mut to) = (0.0f64, 1.0f64);
        for (p, q) in [(-dx, a[0] - left), (dx, right - a[0]), (-dy, a[1] - top), (dy, bottom - a[1])] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    from = from.max(r);
                } else {
                    to = to.min(r);
                }
                if from > to {
                    return false;
                }
            }
        }
        to - from > 1e-9
    }
}

/// Where a cord meets the `index`th of a box's `count` inlets or outlets: Max spreads them from the box's left edge to
/// its right, the first at the left.
pub fn port_x(rect: &Rect, count: usize, index: usize) -> f64 {
    let spread = if count > 1 { (rect.w - PORT).max(0.0) * index.min(count - 1) as f64 / (count - 1) as f64 } else { 0.0 };
    rect.x + spread + PORT / 2.0
}

/// Where a cord leaves a box's outlet: on its bottom edge.
pub fn outlet_point(rect: &Rect, count: usize, index: usize) -> [f64; 2] {
    [port_x(rect, count, index), rect.bottom()]
}

/// Where a cord enters a box's inlet: on its top edge.
pub fn inlet_point(rect: &Rect, count: usize, index: usize) -> [f64; 2] {
    [port_x(rect, count, index), rect.y]
}

/// Whether two segments cross each other: each one's ends lie on either side of the other (touching or running along
/// one another isn't crossing).
pub fn segments_cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let side = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    side(a, b, c) * side(a, b, d) < 0.0 && side(c, d, a) * side(c, d, b) < 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_segment_crosses_a_box_only_through_its_inside() {
        let area = Rect::new(100., 100., 60., 22.);
        assert!(area.crossed_by([90., 111.], [200., 111.], 1.), "straight through");
        assert!(area.crossed_by([130., 0.], [130., 300.], 1.), "top to bottom");
        assert!(!area.crossed_by([90., 100.], [200., 100.], 1.), "along the top edge");
        assert!(!area.crossed_by([130., 0.], [130., 100.], 1.), "ending on the top edge, as a cord into an inlet does");
        assert!(!area.crossed_by([0., 0.], [90., 300.], 1.), "beside it");
        assert!(area.crossed_by([95., 95.], [165., 127.], 1.), "corner to corner");
    }

    #[test]
    fn ports_spread_from_the_left_edge_to_the_right() {
        let area = Rect::new(40., 20., 107., 22.);
        assert_eq!(outlet_point(&area, 1, 0), [43.5, 42.]);
        assert_eq!(inlet_point(&area, 3, 0), [43.5, 20.]);
        assert_eq!(inlet_point(&area, 3, 1), [93.5, 20.]);
        assert_eq!(inlet_point(&area, 3, 2), [143.5, 20.]);
        assert!(segments_cross([0., 0.], [10., 10.], [0., 10.], [10., 0.]));
        assert!(!segments_cross([0., 0.], [10., 0.], [10., 0.], [20., 5.]), "sharing an end");
    }
}
