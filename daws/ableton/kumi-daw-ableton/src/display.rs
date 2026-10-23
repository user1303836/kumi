//! Parameter display text: the reader is shared (kumi-types' display); the script that maps a device's
//! display values in Live stays here.
pub use kumi_types::display::*;

pub const DISPLAY_MAP_SCRIPT: &str = include_str!("assets/display-map.py");
