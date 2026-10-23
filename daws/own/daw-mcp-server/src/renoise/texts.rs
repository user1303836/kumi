//! How a Renoise parameter's value reads as text, and text as a value (M4 n-devices): the kinds of text Renoise 3.5.4 shows
//! for its native devices' parameters, as measured in the lab (devices.rs says each parameter's). A device inside
//! a Doofer or Splitter is no Lua object, so Renoise can't read text for it: Kumi does, from these.

/// One parameter's kind of text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Text {
    /// A whole number, shown as it is.
    Int,
    /// One word per value from the parameter's minimum, by steps of 1 ("Sin", "Saw", …).
    Choices(&'static [&'static str]),
    /// The value itself, with its unit ("100 Hz", "4.00").
    Identity(&'static str),
    /// The value as a percentage ("50.000 %" for 0.5).
    Percent,
    /// A linear gain in decibels ("-6.02 dB" for 0.5, "-INF dB" for 0).
    Db,
    /// A pan: "50 L" (0), "Center" (0.5), "50 R" (1).
    Pan,
    /// A curve measured at points (value, the number shown), and words shown at some values ("Off", "-INF"): read
    /// between the points along straight lines, so it can land a little off.
    Table(&'static str, &'static [(f64, f64)], &'static [(&'static str, f64)]),
    /// Renoise's text for it wasn't measured: values only.
    None,
}

/// The number a text begins with, and the rest (its unit), lower-cased: "−6 dB" is (-6, "db"), "1,000 Hz" (1000, "hz").
fn number_and_unit(text: &str) -> Option<(f64, String)> {
    let text = text.trim().replace('\u{2212}', "-");
    let mut end = 0;
    for (i, c) in text.char_indices() {
        if c.is_ascii_digit() || c == '.' || c == ',' || ((c == '-' || c == '+') && i == 0) {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    let number: f64 = text[..end].replace(',', "").parse().ok()?;
    Some((number, text[end..].trim().to_lowercase()))
}

/// A number given in `unit` as a number in `wanted` units (Hz and kHz, ms and s); none when the units don't agree.
fn in_unit(number: f64, unit: &str, wanted: &str) -> Option<f64> {
    let wanted = wanted.to_lowercase();
    if unit.is_empty() || unit == wanted {
        return Some(number);
    }
    match (unit, wanted.as_str()) {
        ("hz", "khz") => Some(number / 1000.0),
        ("khz", "hz") => Some(number * 1000.0),
        ("s", "ms") => Some(number * 1000.0),
        ("ms", "s") => Some(number / 1000.0),
        _ => None,
    }
}

impl Text {
    /// The value a text means (`min` is the parameter's minimum, where a word list starts), or none when it reads as
    /// nothing this parameter shows.
    pub fn value(&self, text: &str, min: f64) -> Option<f64> {
        let plain = text.trim().to_lowercase().replace('\u{2212}', "-");
        match self {
            Text::Int => plain.parse::<f64>().ok().filter(|n| n.fract() == 0.0),
            Text::Choices(words) => {
                if let Some(i) = words.iter().position(|w| w.to_lowercase() == plain) {
                    return Some(min + i as f64);
                }
                let starts: Vec<usize> =
                    words.iter().enumerate().filter(|(_, w)| w.to_lowercase().starts_with(&plain)).map(|(i, _)| i).collect();
                (starts.len() == 1 && !plain.is_empty()).then(|| min + starts[0] as f64)
            }
            Text::Identity(unit) => number_and_unit(&plain).and_then(|(n, u)| in_unit(n, &u, unit)),
            Text::Percent => number_and_unit(&plain).filter(|(_, u)| u.is_empty() || u == "%").map(|(n, _)| n / 100.0),
            Text::Db => {
                if plain.starts_with("-inf") {
                    return Some(0.0);
                }
                number_and_unit(&plain).filter(|(_, u)| u.is_empty() || u == "db").map(|(n, _)| 10f64.powf(n / 20.0))
            }
            Text::Pan => {
                if plain == "center" || plain == "centre" || plain == "c" {
                    return Some(0.5);
                }
                let (n, side) = number_and_unit(&plain)?;
                match side.as_str() {
                    "l" if (0.0..=50.0).contains(&n) => Some(0.5 - n / 100.0),
                    "r" if (0.0..=50.0).contains(&n) => Some(0.5 + n / 100.0),
                    _ => None,
                }
            }
            Text::Table(unit, points, words) => {
                if let Some((_, v)) = words.iter().find(|(w, _)| w.to_lowercase() == plain) {
                    return Some(*v);
                }
                let (n, u) = number_and_unit(&plain)?;
                let x = in_unit(n, &u, unit)?;
                points.windows(2).find_map(|pair| {
                    let ((v0, x0), (v1, x1)) = (pair[0], pair[1]);
                    let (lo, hi) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
                    if x < lo || x > hi {
                        return None;
                    }
                    Some(if x1 == x0 { v0 } else { v0 + (x - x0) / (x1 - x0) * (v1 - v0) })
                })
            }
            Text::None => None,
        }
    }

    /// Whether a value read from text can land a little off what Renoise would read (a curve read between points).
    pub fn approximate(&self) -> bool {
        matches!(self, Text::Table(..))
    }

    /// A value as Renoise shows it (between a curve's points, about).
    pub fn show(&self, value: f64, min: f64) -> String {
        match self {
            Text::Int => format!("{}", value.round() as i64),
            Text::Choices(words) => {
                words.get((value - min).round().max(0.0) as usize).map_or_else(|| format!("{value}"), |w| w.to_string())
            }
            Text::Identity(unit) => with_unit(trim(value), unit),
            Text::Percent => format!("{} %", trim(value * 100.0)),
            Text::Db => {
                if value <= 0.0 {
                    "-INF dB".into()
                } else {
                    format!("{} dB", trim(20.0 * value.log10()))
                }
            }
            Text::Pan => {
                let side = ((value - 0.5) * 100.0).round();
                if side == 0.0 {
                    "Center".into()
                } else if side < 0.0 {
                    format!("{} L", -side)
                } else {
                    format!("{side} R")
                }
            }
            Text::Table(unit, points, words) => {
                if let Some((w, _)) = words.iter().find(|(_, v)| (v - value).abs() < 1e-9) {
                    return w.to_string();
                }
                let shown = points.windows(2).find_map(|pair| {
                    let ((v0, x0), (v1, x1)) = (pair[0], pair[1]);
                    (value >= v0.min(v1) && value <= v0.max(v1))
                        .then(|| if v1 == v0 { x0 } else { x0 + (value - v0) / (v1 - v0) * (x1 - x0) })
                });
                shown.map_or_else(|| format!("{value}"), |x| format!("≈ {}", with_unit(trim(x), unit)))
            }
            Text::None => trim(value),
        }
    }

    /// Two texts this parameter shows, for a refusal's words.
    pub fn example(&self, min: f64, max: f64) -> String {
        match self {
            Text::Choices(words) => words.iter().take(8).copied().collect::<Vec<_>>().join(", "),
            _ => format!("{} to {}", self.show(min, min), self.show(max, min)),
        }
    }
}

fn with_unit(number: String, unit: &str) -> String {
    if unit.is_empty() {
        number
    } else {
        format!("{number} {unit}")
    }
}

fn trim(value: f64) -> String {
    let text = format!("{:.3}", value);
    let text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    if text == "-0" {
        "0".into()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renoise::devices::{by_name, text_of};

    fn param(device: &str, name: &str) -> (usize, &'static crate::renoise::devices::Param) {
        let d = by_name(device).unwrap();
        let k = d.params.iter().position(|p| p.name == name).unwrap();
        (k + 1, &d.params[k])
    }

    #[test]
    fn texts_read_as_renoise_shows_them_for_each_kind() {
        // A gain in dB (the Gainer's Volume is linear 0..4).
        let (_, gain) = param("Gainer", "Gain");
        assert!((gain.text.value("-6 dB", gain.min).unwrap() - 0.501_187).abs() < 1e-5);
        assert!((gain.text.value("\u{2212}6.02", gain.min).unwrap() - 0.5).abs() < 1e-3);
        assert_eq!(gain.text.value("-INF dB", gain.min), Some(0.0));
        assert_eq!(gain.text.value("loud", gain.min), None);
        assert_eq!(gain.text.show(1.0, gain.min), "0 dB");
        // A pan.
        let (_, pan) = param("Gainer", "Panning");
        assert_eq!(pan.text.value("Center", pan.min), Some(0.5));
        assert_eq!(pan.text.value("25 L", pan.min), Some(0.25));
        assert_eq!(pan.text.value("50 r", pan.min), Some(1.0));
        assert_eq!(pan.text.show(0.75, pan.min), "25 R");
        // The value itself with its unit, Hz and kHz both.
        let (_, freq) = param("EQ 5", "Frequency 2");
        assert_eq!(freq.text, Text::Identity("Hz"));
        assert_eq!(freq.text.value("1000 Hz", freq.min), Some(1000.0));
        assert_eq!(freq.text.value("2.5 kHz", freq.min), Some(2500.0));
        assert_eq!(freq.text.value("3 dB", freq.min), None, "another unit");
        // Words, from the minimum.
        let (k, kind) = param("*LFO", "Type");
        let Text::Choices(words) = kind.text else { panic!("{:?}", kind.text) };
        assert_eq!(kind.text.value(words[2], kind.min), Some(kind.min + 2.0));
        assert_eq!(text_of("LfoDevice", k), Some(&kind.text));
        // A percentage.
        let (_, amp) = param("*LFO", "Amplitude");
        assert_eq!(amp.text, Text::Percent);
        assert_eq!(amp.text.value("25 %", amp.min), Some(0.25));
        assert_eq!(amp.text.show(0.5, amp.min), "50 %");
    }

    #[test]
    fn a_curve_reads_between_its_points_and_says_it_may_land_a_little_off() {
        let (_, cutoff) = param("Analog Filter", "Cutoff");
        let Text::Table(unit, points, _) = cutoff.text else { panic!("{:?}", cutoff.text) };
        assert_eq!(unit, "kHz");
        assert!(cutoff.text.approximate());
        // A measured point reads back exactly; between two it lands between their values; out of the curve is nothing.
        let (v, x) = points[5];
        assert!((cutoff.text.value(&format!("{x} kHz"), cutoff.min).unwrap() - v).abs() < 1e-9);
        let one_k = cutoff.text.value("1 kHz", cutoff.min).unwrap();
        let in_hz = cutoff.text.value("1000 Hz", cutoff.min).unwrap();
        assert!((one_k - in_hz).abs() < 1e-12);
        assert!(one_k > cutoff.min && one_k < cutoff.max);
        assert_eq!(cutoff.text.value("90 kHz", cutoff.min), None);
        assert!(cutoff.text.show(one_k, cutoff.min).starts_with("≈ 1 kHz"), "{}", cutoff.text.show(one_k, cutoff.min));
    }

    #[test]
    fn every_measured_parameter_has_a_kind_of_text_and_its_points_run_one_way() {
        let mut unknown = 0;
        for device in crate::renoise::devices::DEVICES {
            for p in device.params {
                match p.text {
                    Text::None => unknown += 1,
                    Text::Table(_, points, _) => {
                        assert!(points.len() >= 2, "{} {}", device.name, p.name);
                        let up = points.windows(2).all(|w| w[1].1 > w[0].1);
                        let down = points.windows(2).all(|w| w[1].1 < w[0].1);
                        assert!(up || down, "{} {}: its curve runs both ways", device.name, p.name);
                    }
                    Text::Choices(words) => assert!(!words.is_empty(), "{} {}", device.name, p.name),
                    _ => {}
                }
            }
        }
        assert!(unknown <= 8, "{unknown} parameters read as values only");
    }
}
