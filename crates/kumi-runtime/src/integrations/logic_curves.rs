//! (M6) Logic Pro's equaliser's and compressor's curves, for HISTORY's pictures, from the rows its plug-in window's
//! Controls View lists (the bridge's `param` read): Channel EQ's eight bands (Low Cut, Low Shelf, Peak 1 to 4, High
//! Shelf, High Cut, each its On/Off, Frequency, Gain or Slope and Q-Factor, as the M4b scenes read them: "75.0 Hz",
//! "+3.0 dB", "0.71", "12 dB/Oct") and the Compressor's static curve from four of the 25 rows its Controls View lists,
//! as the bridge's read-param-compressor scene read them on Logic Pro 12.3.1 (Scratch's Keys): Threshold ("-17.5 dB"),
//! Ratio ("3.9:1"), Knee ("0.7", a value the view shows with no control, so its row has the text alone) and Make Up
//! ("0.0 dB"). A change of one of these parameters is drawn as Live's EQ and compressor changes are: before and after.
use super::own_bridge::OwnCurves;
use crate::core::picture::{amount, CurveReader, Dynamics, EqBand, EqShape, ParameterRow};

/// Channel EQ's bands as its Controls View names them, each with its shape and whether it has a gain (a cut has a slope).
const BANDS: [(&str, EqShape, bool); 8] = [
    ("Low Cut", EqShape::LowCut, false),
    ("Low Shelf", EqShape::LowShelf, true),
    ("Peak 1", EqShape::Bell, true),
    ("Peak 2", EqShape::Bell, true),
    ("Peak 3", EqShape::Bell, true),
    ("Peak 4", EqShape::Bell, true),
    ("High Shelf", EqShape::HighShelf, true),
    ("High Cut", EqShape::HighCut, false),
];
/// The Compressor's make-up gain, as its Controls View names it (never a bare "Gain": the Gain plug-in's, which the
/// listening's gain stage turns often, is no compressor's, and the Compressor's own "Gain" is its side chain filter's).
const MAKE_UP: &str = "Make Up";
/// A unitless Knee (Logic shows "0.7"; it runs 0 to 1, hard to soft) drawn as up to this much soft knee.
const SOFT_KNEE_DB: f64 = 20.0;

/// Logic's Channel EQ and Compressor, by their parameters' names and the words Logic shows for them.
pub struct LogicCurves;

fn find<'a>(rows: &'a [ParameterRow], name: &str) -> Option<&'a ParameterRow> {
    rows.iter().find(|row| row.name == name)
}

/// What a row shows as a number ("+3.0 dB" is 3, "2.5 kHz" 2500), else its value.
fn shown(row: &ParameterRow) -> f64 {
    row.text.as_deref().and_then(amount).unwrap_or(row.value)
}

/// Whether a switch is on: its text ("on", "off"), else its value.
fn on(row: &ParameterRow) -> bool {
    match row.text.as_deref().map(|text| text.trim().to_ascii_lowercase()) {
        Some(text) if text == "on" => true,
        Some(text) if text == "off" => false,
        _ => row.value >= 0.5,
    }
}

impl CurveReader for LogicCurves {
    fn eq_bands(&self, rows: &[ParameterRow]) -> Option<Vec<EqBand>> {
        // Logic's own words say what it is: Channel EQ's frequencies read in Hz.
        let in_hz = |row: &ParameterRow| row.text.as_deref().is_some_and(|text| text.trim().to_ascii_lowercase().ends_with("hz"));
        if !["Low Shelf Frequency", "Peak 1 Frequency", "High Shelf Frequency"].iter().all(|name| find(rows, name).is_some_and(in_hz)) {
            return None;
        }
        let bands: Vec<EqBand> = BANDS
            .iter()
            .filter_map(|(band, shape, gains)| {
                let get = |what: &str| find(rows, &format!("{band} {what}"));
                let freq = get("Frequency").filter(|row| in_hz(row)).map(shown)?;
                let gain_db = if *gains { get("Gain").map(shown).unwrap_or(0.0) } else { 0.0 };
                let q = get("Q-Factor").map(shown).filter(|q| q.is_finite() && *q > 0.0).unwrap_or(0.71);
                let slope = (!gains).then(|| get("Slope").and_then(|row| row.text.as_deref()).and_then(amount).unwrap_or(12.0));
                Some(EqBand { shape: *shape, freq, gain_db, q, on: get("On/Off").is_none_or(on), slope })
            })
            .collect();
        (!bands.is_empty()).then_some(bands)
    }
    fn dynamics(&self, rows: &[ParameterRow]) -> Option<Dynamics> {
        // A compressor's threshold reads in dB and its ratio as "2.0:1".
        let threshold =
            find(rows, "Threshold").filter(|row| row.text.as_deref().is_some_and(|text| text.to_ascii_lowercase().contains("db")))?;
        let ratio = find(rows, "Ratio").filter(|row| row.text.as_deref().is_some_and(|text| text.contains(':')))?;
        let ratio = match ratio.text.as_deref().map(str::trim) {
            Some(text) if text.starts_with('∞') || text.to_ascii_lowercase().starts_with("inf") => 100.0,
            _ => shown(ratio),
        };
        let knee_db = find(rows, "Knee").map_or(0.0, |knee| match knee.text.as_deref() {
            Some(text) if text.to_ascii_lowercase().contains("db") => shown(knee),
            _ => (shown(knee).clamp(0.0, 1.0)) * SOFT_KNEE_DB,
        });
        let makeup_db = find(rows, MAKE_UP).map(shown).unwrap_or(0.0);
        Some(Dynamics { threshold_db: shown(threshold), ratio: ratio.max(1.0), knee_db, makeup_db })
    }
}

/// Whether a parameter of this name may be Channel EQ's or the Compressor's: worth a read of its plug-in's others for
/// the curve (their own names then say whether the plug-in is one).
pub fn curve_parameter(name: &str) -> bool {
    BANDS.iter().any(|(band, _, _)| {
        name.strip_prefix(band)
            .and_then(|rest| rest.strip_prefix(' '))
            .is_some_and(|what| matches!(what, "On/Off" | "Frequency" | "Gain" | "Q-Factor" | "Slope"))
    }) || ["Threshold", "Ratio", "Knee", MAKE_UP].contains(&name)
}

/// What Logic gives the shared integration for its changes' curves.
pub const CURVES: OwnCurves = OwnCurves { reader: &LogicCurves, curve_parameter };

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::picture::{curves, PictureValue};

    fn row(name: &str, value: f64, text: &str) -> ParameterRow {
        ParameterRow { name: name.into(), value, min: 0.0, max: 1050.0, text: Some(text.into()) }
    }

    /// Channel EQ as the M4b scenes read it (Vocal's, its defaults), with Low Shelf Gain moved.
    fn channel_eq(low_shelf_gain: &str) -> Vec<ParameterRow> {
        let mut rows = vec![
            row("Low Cut On/Off", 0.0, "off"),
            row("Low Cut Frequency", 0.0, "20.0 Hz"),
            ParameterRow { name: "Low Cut Slope".into(), value: 0.0, min: 0.0, max: 0.0, text: Some("12 dB/Oct".into()) },
            row("Low Cut Q-Factor", 36.0, "0.71"),
            row("Low Shelf On/Off", 1.0, "on"),
            row("Low Shelf Frequency", 200.0, "75.0 Hz"),
            row("Low Shelf Gain", 240.0, low_shelf_gain),
            row("Low Shelf Q-Factor", 40.0, "1.00"),
        ];
        for (n, freq) in [(1, "100 Hz"), (2, "250 Hz"), (3, "1040 Hz"), (4, "2500 Hz")] {
            rows.extend([
                row(&format!("Peak {n} On/Off"), 1.0, "on"),
                row(&format!("Peak {n} Frequency"), 300.0, freq),
                row(&format!("Peak {n} Gain"), 240.0, "0.0 dB"),
                row(&format!("Peak {n} Q-Factor"), 33.0, "0.60"),
            ]);
        }
        rows.extend([
            row("High Shelf On/Off", 1.0, "on"),
            row("High Shelf Frequency", 900.0, "7500 Hz"),
            row("High Shelf Gain", 240.0, "0.0 dB"),
            row("High Shelf Q-Factor", 40.0, "1.00"),
            row("High Cut On/Off", 0.0, "off"),
            row("High Cut Frequency", 1050.0, "20000 Hz"),
            ParameterRow { name: "High Cut Slope".into(), value: 0.0, min: 0.0, max: 0.0, text: Some("24 dB/Oct".into()) },
            row("High Cut Q-Factor", 36.0, "0.71"),
            row("Master  Gain", 240.0, "0.0 dB"),
        ]);
        rows
    }

    #[test]
    fn channel_eqs_bands_are_read_from_what_its_controls_view_shows() {
        let bands = LogicCurves.eq_bands(&channel_eq("+3.0 dB")).expect("Channel EQ");
        assert_eq!(bands.len(), 8);
        assert_eq!((bands[0].shape, bands[0].on, bands[0].slope, bands[0].freq), (EqShape::LowCut, false, Some(12.0), 20.0));
        assert_eq!((bands[1].shape, bands[1].freq, bands[1].gain_db, bands[1].q, bands[1].on), (EqShape::LowShelf, 75.0, 3.0, 1.0, true));
        assert_eq!((bands[4].shape, bands[4].freq), (EqShape::Bell, 1040.0));
        assert_eq!((bands[7].shape, bands[7].slope, bands[7].freq), (EqShape::HighCut, Some(24.0), 20000.0));
        // Another plug-in whose rows aren't an EQ's in Hz (a Gain, the Piano) draws none.
        assert!(LogicCurves.eq_bands(&[row("Gain", 960.0, "0.0 dB")]).is_none());
        assert!(LogicCurves.dynamics(&channel_eq("0.0 dB")).is_none());
    }

    /// The Compressor's Controls View as the bridge's read-param-compressor scene read it on Logic Pro 12.3.1 (Scratch's
    /// Keys), each row as the integration takes it from the read (a row with no value, a menu's or a value shown with no
    /// control, keeps its text): name, value, range and text, with its Threshold as given.
    fn compressor(threshold: (f64, &str)) -> Vec<ParameterRow> {
        const ROWS: [(&str, Option<f64>, Option<(f64, f64)>, &str); 25] = [
            ("Threshold", Some(65.0), Some((0.0, 100.0)), "-17.5 dB"),
            ("Ratio", Some(34.0), Some((0.0, 85.0)), "3.9:1"),
            ("Attack", Some(45.0), Some((0.0, 100.0)), "35.0 ms"),
            ("Release", Some(50.0), Some((0.0, 119.0)), "91.0 ms"),
            ("Make Up", Some(40.0), Some((0.0, 110.0)), "0.0 dB"),
            ("Knee", None, None, "0.7"),
            ("Peak/RMS", None, None, "RMS"),
            ("Auto Gain", None, None, "-12 dB"),
            ("Distortion", None, None, "Off"),
            ("Circuit Type", None, None, "Studio VCA"),
            ("Side Chain Detection", Some(0.0), Some((0.0, 1.0)), "Max"),
            ("Limiter Threshold", Some(100.0), Some((0.0, 100.0)), "0.0 dB"),
            ("Limiter On", Some(0.0), Some((0.0, 1.0)), "off"),
            ("Auto Release", Some(1.0), Some((0.0, 1.0)), "on"),
            ("State", None, None, "Off"),
            ("Mode", None, None, "LP"),
            ("Frequency", Some(105.0), Some((0.0, 123.0)), "7040.0 Hz"),
            ("Q", Some(6.0), Some((0.0, 20.0)), "1.00"),
            ("Gain", None, None, "0.0 dB"),
            ("Mix", Some(118.0), Some((0.0, 200.0)), "59.0 %"),
            ("Gain Reduction", None, None, "0.0 dB"),
            ("Input Gain", Some(60.0), Some((0.0, 120.0)), "0.0 dB"),
            ("Output Gain", Some(58.0), Some((0.0, 120.0)), "-1.0 dB"),
            ("Display Select", Some(0.0), Some((0.0, 1.0)), "Meter"),
            ("View Select", None, None, "Output"),
        ];
        ROWS.iter()
            .map(|(name, value, range, text)| {
                let (value, text) = if *name == "Threshold" { (Some(threshold.0), threshold.1) } else { (*value, *text) };
                let (min, max) = range.unwrap_or((0.0, 0.0));
                ParameterRow { name: (*name).into(), value: value.unwrap_or(0.0), min, max, text: Some(text.into()) }
            })
            .collect()
    }

    #[test]
    fn the_compressors_curve_is_its_threshold_ratio_knee_and_make_up_as_its_controls_view_shows_them() {
        let rows = compressor((65.0, "-17.5 dB"));
        // Knee "0.7" of a run from 0 to 1 is 14 dB of soft knee; the side chain filter's Gain is no make-up.
        assert_eq!(LogicCurves.dynamics(&rows), Some(Dynamics { threshold_db: -17.5, ratio: 3.9, knee_db: 14.0, makeup_db: 0.0 }));
        assert!(LogicCurves.eq_bands(&rows).is_none(), "its Frequency is the side chain filter's, not a band's");
        assert!(LogicCurves.dynamics(&rows[..1]).is_none(), "no ratio, no compressor");
        assert!(
            LogicCurves.dynamics(&[row("Threshold", 400.0, "-20.0 dB"), row("Ratio", 10.0, "3")]).is_none(),
            "a ratio without its ':1'"
        );
        // A ratio Logic shows as infinite is a limiter's.
        let limiting: Vec<ParameterRow> = rows
            .iter()
            .cloned()
            .map(|row| if row.name == "Ratio" { ParameterRow { text: Some("∞:1".into()), ..row } } else { row })
            .collect();
        assert_eq!(LogicCurves.dynamics(&limiting).map(|curve| curve.ratio), Some(100.0));
    }

    #[test]
    fn a_change_of_the_compressors_threshold_draws_its_curve_before_and_after() {
        // Kumi's fx_param took the Threshold from -17.5 dB to -24.0 dB (raw 65 to 52) on Logic Pro 12.3.1.
        let moved = PictureValue {
            name: "Threshold".into(),
            from: Some(65.0),
            to: Some(52.0),
            from_text: Some("-17.5 dB".into()),
            to_text: Some("-24.0 dB".into()),
            ..Default::default()
        };
        let drawn = curves(&LogicCurves, &compressor((52.0, "-24.0 dB")), &[("c2".into(), vec![moved])]);
        let [(id, crate::core::picture::ChangePicture::Dynamics { before, after, values })] = drawn.as_slice() else { panic!("{drawn:?}") };
        assert_eq!((id.as_str(), before.threshold_db, after.threshold_db, values.len()), ("c2", -17.5, -24.0, 1));
        assert_eq!((before.ratio, after.ratio, after.knee_db, after.makeup_db), (3.9, 3.9, 14.0, 0.0));
        for name in ["Threshold", "Ratio", "Knee", "Make Up"] {
            assert!(curve_parameter(name), "{name}");
        }
        for name in ["Limiter Threshold", "Gain", "Input Gain", "Output Gain", "Mix", "Make Up Gain"] {
            assert!(!curve_parameter(name), "{name}");
        }
    }

    #[test]
    fn a_change_of_channel_eqs_gain_draws_its_curve_before_and_after() {
        let moved = PictureValue {
            name: "Low Shelf Gain".into(),
            from: Some(240.0),
            to: Some(270.0),
            from_text: Some("0.0 dB".into()),
            to_text: Some("+3.0 dB".into()),
            ..Default::default()
        };
        let drawn = curves(&LogicCurves, &channel_eq("+3.0 dB"), &[("c1".into(), vec![moved])]);
        let [(id, crate::core::picture::ChangePicture::Eq { before, after, values })] = drawn.as_slice() else { panic!("{drawn:?}") };
        assert_eq!((id.as_str(), before[1].gain_db, after[1].gain_db, values.len()), ("c1", 0.0, 3.0, 1));
        for name in ["Low Shelf Gain", "Peak 3 Frequency", "High Cut Slope", "Threshold", "Ratio", "Make Up"] {
            assert!(curve_parameter(name), "{name}");
        }
        for name in ["Master  Gain", "Analyzer On/Off", "Attack", "Main Volume", "Peak 5 Gain", "Gain"] {
            assert!(!curve_parameter(name), "{name}");
        }
    }
}
