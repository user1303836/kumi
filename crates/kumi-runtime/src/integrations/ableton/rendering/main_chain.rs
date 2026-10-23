//! Live's Main chain as the judge's masking reads it: which of Main's devices change the mix in a way the focus element
//! (heard before Main) doesn't share, and the gain of the Utilities that only turn the level.
use crate::listening::checklist::round1;
pub use crate::listening::checklist::MainState;

/// One of Main's devices as masking needs it: its name and class, whether it's on, and (a Utility's) its parameters
/// by name and the text Live shows.
#[derive(Debug, Clone, PartialEq)]
pub struct MainDevice {
    pub name: String,
    pub class: String,
    pub on: bool,
    pub parameters: Vec<(String, String)>,
}

/// Main's chain, read: devices that are off, Spectrum and Tuner change nothing; a Utility that only turns the level is
/// a gain the focus is heard with too; anything else makes masking unreadable there.
pub fn main_chain(devices: &[MainDevice]) -> MainState {
    let mut state = MainState::default();
    for device in devices.iter().filter(|device| device.on) {
        if matches!(device.class.as_str(), "SpectrumAnalyzer" | "Tuner") || device.name.starts_with("Kumi Ears") {
            continue;
        }
        match utility_gain(device) {
            Some(gain) => state.gain += gain,
            None => state.unfair.push(device.name.clone()),
        }
    }
    state.gain = round1(state.gain);
    state
}

/// A Utility's gain (dB) when that's all it changes, read by name from what Live shows. Every parameter has to be one
/// Kumi knows, at its neutral setting: both channels as they are (no invert, mono or mute, Channel Mode Stereo),
/// Balance and Mid/Side Balance at the centre. Live 12.4 calls the gain Output; before, Gain beside a Stereo Width (at
/// 100 %). None for anything else: a parameter Kumi doesn't know, text it can't read, or no gain at all.
pub fn utility_gain(device: &MainDevice) -> Option<f64> {
    if !matches!(device.class.as_str(), "StereoGain" | "Utility") {
        return None;
    }
    let mut gain = None;
    for (name, shown) in &device.parameters {
        let shown = shown.trim();
        let number = || {
            let text: String = shown.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
            text.parse::<f64>().ok()
        };
        let neutral = match name.as_str() {
            "Output" | "Gain" => {
                gain = Some(number().filter(|_| !shown.starts_with("-inf"))?);
                true
            }
            "Device On" => shown == "On",
            // The bass's frequency matters only with Bass Mono on, a DC filter only below what's heard.
            "Bass Freq" | "DC Filter" => true,
            "Left Inv" | "Right Inv" | "Mono" | "Bass Mono" | "Mute" => shown == "Off",
            "Channel Mode" => shown == "Stereo",
            "Balance" => shown == "C",
            "Mid/Side Balance" => shown == "0",
            "Stereo Width" => shown.ends_with('%') && number().is_some_and(|width| (width - 100.).abs() < 0.5),
            _ => false,
        };
        if !neutral {
            return None;
        }
    }
    gain
}
