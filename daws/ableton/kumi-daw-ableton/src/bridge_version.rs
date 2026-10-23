//! Minimum bridge releases for runtime features.
pub const FIXED_BRIDGE: &str = "1.0.34";
pub const ARRANGEMENT_BRIDGE: &str = "1.0.35";
pub const RENDER_BRIDGE: &str = "1.0.49";
pub const GOAL_BRIDGE: &str = "1.0.50";
pub const SCALE_BRIDGE: &str = "1.0.57";
pub const FULL_CONTROL_BRIDGE: &str = "1.0.58";
pub const PYTHON_BRIDGE: &str = "1.0.68";
pub const EARS_BRIDGE: &str = "1.0.73";
/// A message in passing is a line in Live's status bar (the Control Surface's own call). Older bridges showed it
/// with Application.show_on_the_fly_message, a modal alert in Live 12.4 that holds Live until it's dismissed.
pub const STATUS_LINE_BRIDGE: &str = "1.0.92";
/// Unknown versions count as later, as in the source. Components use decimal parseInt, not semver validation.
pub fn at_least(version: Option<&str>, minimum: &str) -> bool {
    let Some(version) = version.filter(|v| !v.is_empty()) else {
        return true;
    };
    let parse = |text: &str| {
        text.split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map(|part| {
                let part = kumi_common::js::string::trim_start(part);
                let end = part.bytes().take_while(u8::is_ascii_digit).count();
                if end == 0 {
                    0.
                } else {
                    part[..end].parse::<f64>().unwrap_or(0.)
                }
            })
            .collect::<Vec<_>>()
    };
    let have = parse(version);
    let need = parse(minimum);
    for at in 0..have.len().max(need.len()) {
        let diff = have.get(at).copied().unwrap_or(0.) - need.get(at).copied().unwrap_or(0.);
        if diff != 0. {
            return diff > 0.;
        }
    }
    true
}
