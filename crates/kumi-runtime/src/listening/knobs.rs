//! Knob units: a parameter's raw range read through the text Live shows across it, so code picking numbers moves in
//! units a listener hears evenly (dB, octaves, log-time) and one step is about one just-noticeable difference.

use crate::integrations::ableton::display::parse_display;
use kumi_common::js::number::to_string;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Db,
    Hz,
    Seconds,
    Percent,
    Ratio,
    Semitones,
    Plain,
}

impl Unit {
    fn of(unit: &str) -> Unit {
        match unit {
            "db" => Unit::Db,
            "hz" => Unit::Hz,
            "s" => Unit::Seconds,
            "%" => Unit::Percent,
            "ratio" => Unit::Ratio,
            "st" => Unit::Semitones,
            _ => Unit::Plain,
        }
    }
    /// Searched on a log scale: octaves for frequency, log-time, and ratios.
    fn logarithmic(self) -> bool {
        matches!(self, Unit::Hz | Unit::Seconds | Unit::Ratio)
    }
}

/// The quietest level a dB knob is searched at (its "-inf" end stands in as this).
const FLOOR_DB: f64 = -70.;

/// A knob's scale: its raw values against what Live shows for them, in one unit, monotone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scale {
    pub unit: Unit,
    /// (raw, shown) in raw order.
    points: Vec<(f64, f64)>,
}

impl Scale {
    /// The scale of a parameter from Live's text across its range (raw value and text), when its text reads as one
    /// unit and moves one way. None for a switch, a list or text that doesn't read as numbers.
    pub fn read(grid: &[(f64, String)]) -> Option<Scale> {
        let read: Vec<(f64, f64, String)> =
            grid.iter().filter_map(|(raw, text)| parse_display(text).map(|shown| (*raw, shown.value, shown.unit))).collect();
        // The unit most of the range shows in.
        let mut counts: Vec<(String, usize)> = vec![];
        for (_, _, unit) in &read {
            match counts.iter_mut().find(|(seen, _)| seen == unit) {
                Some((_, count)) => *count += 1,
                None => counts.push((unit.clone(), 1)),
            }
        }
        let (unit, count) = counts.into_iter().max_by_key(|(_, count)| *count)?;
        if count * 2 < grid.len().max(2) {
            return None;
        }
        let kind = Unit::of(&unit);
        let mut points: Vec<(f64, f64)> = read
            .into_iter()
            .filter(|(_, _, seen)| *seen == unit)
            .map(|(raw, shown, _)| {
                (raw, if kind == Unit::Db && shown == f64::NEG_INFINITY { FLOOR_DB } else { shown.max(FLOOR_DB.min(shown)) })
            })
            .filter(|(raw, shown)| raw.is_finite() && shown.is_finite())
            .collect();
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        points.dedup_by(|a, b| a.0 == b.0);
        if kind.logarithmic() && points.iter().any(|(_, shown)| *shown <= 0.) {
            points.retain(|(_, shown)| *shown > 0.);
        }
        // Text rounds, so neighbours may show the same; the scale must not turn back.
        let rising = points.last()?.1 >= points.first()?.1;
        let mut kept: Vec<(f64, f64)> = vec![];
        for point in points {
            match kept.last() {
                Some(last) if (rising && point.1 < last.1) || (!rising && point.1 > last.1) => return None,
                Some(last) if point.1 == last.1 => {}
                _ => kept.push(point),
            }
        }
        if kept.len() < 2 || kept.first()?.1 == kept.last()?.1 {
            return None;
        }
        Some(Scale { unit: kind, points: kept })
    }
    /// A scale straight from numbers: raw equals shown (a parameter whose text is its number).
    pub fn linear(unit: Unit, min: f64, max: f64) -> Scale {
        Scale { unit, points: vec![(min, min), (max, max)] }
    }
    /// The lowest and highest values shown, in the unit.
    pub fn range(&self) -> (f64, f64) {
        let (a, b) = (self.points.first().unwrap().1, self.points.last().unwrap().1);
        (a.min(b), a.max(b))
    }
    /// The raw range.
    pub fn raw_range(&self) -> (f64, f64) {
        (self.points.first().unwrap().0, self.points.last().unwrap().0)
    }
    /// Where a value sits for a listener: dB as it is, octaves, log-time, log-ratio, a fraction for percent.
    pub fn perceptual(&self, shown: f64) -> f64 {
        match self.unit {
            unit if unit.logarithmic() => shown.max(1e-9).log2(),
            Unit::Percent => shown / 100.,
            _ => shown,
        }
    }
    pub fn from_perceptual(&self, at: f64) -> f64 {
        match self.unit {
            unit if unit.logarithmic() => at.exp2(),
            Unit::Percent => at * 100.,
            _ => at,
        }
    }
    /// About one just-noticeable difference, in perceptual units.
    pub fn step(&self) -> f64 {
        match self.unit {
            Unit::Db => 1.,
            Unit::Hz => 1. / 6.,
            Unit::Seconds => 0.26,
            Unit::Ratio => 0.25,
            Unit::Percent => 0.05,
            Unit::Semitones => 0.5,
            Unit::Plain => {
                let (lo, hi) = self.range();
                (hi - lo).abs() / 20.
            }
        }
    }
    /// What a raw value shows, in the unit.
    pub fn shown(&self, raw: f64) -> f64 {
        let points: Vec<(f64, f64)> = self.points.iter().map(|(raw, shown)| (*raw, self.perceptual(*shown))).collect();
        self.from_perceptual(interpolate(&points, raw))
    }
    /// The raw value that shows a value, in the unit (clamped to the range).
    pub fn raw(&self, shown: f64) -> f64 {
        let (lo, hi) = self.range();
        let at = self.perceptual(shown.clamp(lo, hi));
        let mut points: Vec<(f64, f64)> = self.points.iter().map(|(raw, shown)| (self.perceptual(*shown), *raw)).collect();
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        interpolate(&points, at)
    }
    /// A value as text Live reads back in this unit.
    pub fn text(&self, shown: f64) -> String {
        let rounded = |value: f64, places: i32| {
            let scale = 10f64.powi(places);
            to_string((value * scale).round() / scale)
        };
        match self.unit {
            Unit::Db => format!("{} dB", rounded(shown, 2)),
            Unit::Hz => format!("{} Hz", rounded(shown, 1)),
            Unit::Seconds => format!("{} ms", rounded(shown * 1000., 2)),
            Unit::Percent => format!("{} %", rounded(shown, 2)),
            Unit::Ratio => format!("{} : 1", rounded(shown, 2)),
            Unit::Semitones => format!("{} st", rounded(shown, 2)),
            Unit::Plain => rounded(shown, 3),
        }
    }
}

/// Piecewise-linear, clamped at the ends; `points` sorted by x.
fn interpolate(points: &[(f64, f64)], x: f64) -> f64 {
    let (first, last) = (points[0], points[points.len() - 1]);
    if x <= first.0 {
        return first.1;
    }
    if x >= last.0 {
        return last.1;
    }
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if x >= a.0 && x <= b.0 {
            return if b.0 == a.0 { a.1 } else { a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0) };
        }
    }
    last.1
}
