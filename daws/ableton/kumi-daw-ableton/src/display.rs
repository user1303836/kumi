//! Parameter display text: the reader is shared (kumi-types' display); the script that maps a device's
//! display values in Live stays here.
pub use kumi_types::display::*;

pub const DISPLAY_MAP_SCRIPT: &str = include_str!("assets/display-map.py");
/// Finds, in Live, the fader, pan and send values at which Live shows given labels ("-6.0 dB", "25L"): a track's
/// mixer as `obj`, the labels in `ARGS`. Reads Live's own `str_for_value`, never sets a value.
pub const MIXER_DISPLAY_SCRIPT: &str = include_str!("assets/mixer-display.py");
