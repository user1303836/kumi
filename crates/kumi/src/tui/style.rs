//! Colours, text styles and the terminal escape codes that draw them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use kumi_common::js;

pub type Rgb = [u8; 3];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl Style {
    /// A style of a foreground colour only.
    pub const fn fg(rgb: Rgb) -> Style {
        Style { fg: Some(rgb), bg: None, bold: false, dim: false, italic: false, underline: false, inverse: false }
    }

    /// A style of a background colour only.
    pub const fn bg(rgb: Rgb) -> Style {
        Style { fg: None, bg: Some(rgb), bold: false, dim: false, italic: false, underline: false, inverse: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorDepth {
    Truecolor,
    Colors256,
    Colors16,
    None,
}

impl ColorDepth {
    /// The depth's name, as `KUMI_COLOR` spells it: "truecolor", "256", "16" or "none".
    pub fn as_str(self) -> &'static str {
        match self {
            ColorDepth::Truecolor => "truecolor",
            ColorDepth::Colors256 => "256",
            ColorDepth::Colors16 => "16",
            ColorDepth::None => "none",
        }
    }

    pub fn parse(name: &str) -> Option<ColorDepth> {
        match name {
            "truecolor" => Some(ColorDepth::Truecolor),
            "256" => Some(ColorDepth::Colors256),
            "16" => Some(ColorDepth::Colors16),
            "none" => Some(ColorDepth::None),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("not a #rrggbb colour: {0}")]
pub struct ColorError(pub String);

static HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$").unwrap());

pub fn hex(value: &str) -> Result<Rgb, ColorError> {
    let found = HEX.captures(value).ok_or_else(|| ColorError(value.to_string()))?;
    let channel = |index: usize| u8::from_str_radix(&found[index], 16).expect("two hex digits");
    Ok([channel(1), channel(2), channel(3)])
}

/// Kumi's palette (see docs/en/KUMI_TUI.md). Track colours come from Live itself.
pub mod palette {
    use super::Rgb;

    pub const GROUND: Rgb = [0x0e, 0x0f, 0x12];
    pub const SURFACE: Rgb = [0x14, 0x16, 0x1a];
    pub const RAISED: Rgb = [0x1c, 0x1f, 0x24];
    pub const SELECTED: Rgb = [0x1e, 0x3a, 0x2f];
    pub const RULE: Rgb = [0x3a, 0x3f, 0x47];
    pub const TRACK: Rgb = [0x4a, 0x50, 0x59];
    pub const FAINT: Rgb = [0x80, 0x86, 0x8f];
    pub const DIM: Rgb = [0xa6, 0xac, 0xb5];
    pub const TEXT: Rgb = [0xd6, 0xda, 0xdf];
    pub const BRIGHT: Rgb = [0xf4, 0xf6, 0xf8];
    pub const ACCENT: Rgb = [0x86, 0xe3, 0xb5];
    pub const PULSE: Rgb = [0x2f, 0x5a, 0x47];
    pub const WARN: Rgb = [0xe7, 0xb4, 0x5f];
    /// The beat light: Live's transport playing, lit on each beat (brightest on a bar's first), dark between.
    pub const BEAT: Rgb = [0xff, 0xe1, 0x4d];
    pub const OFFBEAT: Rgb = [0x4d, 0x44, 0x20];
    pub const ERROR: Rgb = [0xee, 0x84, 0x79];
    /// What Kumi keeps, by kind: notes, techniques, recipes, and its own lessons from matching.
    pub const NOTE: Rgb = [0x8c, 0xc8, 0xff];
    pub const TECHNIQUE: Rgb = [0xc7, 0xa6, 0xff];
    pub const RECIPE: Rgb = [0xf2, 0xa6, 0xc4];
    pub const LESSON: Rgb = [0xe7, 0xc8, 0x8f];

    /// The palette by name (`palette[name]` in the TypeScript), for colours chosen at run time.
    pub fn named(name: &str) -> Option<Rgb> {
        Some(match name {
            "ground" => GROUND,
            "surface" => SURFACE,
            "raised" => RAISED,
            "selected" => SELECTED,
            "rule" => RULE,
            "track" => TRACK,
            "faint" => FAINT,
            "dim" => DIM,
            "text" => TEXT,
            "bright" => BRIGHT,
            "accent" => ACCENT,
            "pulse" => PULSE,
            "warn" => WARN,
            "beat" => BEAT,
            "offbeat" => OFFBEAT,
            "error" => ERROR,
            "note" => NOTE,
            "technique" => TECHNIQUE,
            "recipe" => RECIPE,
            "lesson" => LESSON,
            _ => return None,
        })
    }
}

/// `process.platform`'s word for this system: "darwin", "linux", "win32" (or Rust's name elsewhere).
pub fn process_platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    }
}

/// `os.release()`: the kernel's release ("24.6.0"), or Windows' version ("10.0.19045").
pub fn os_release() -> String {
    #[cfg(unix)]
    {
        // SAFETY: utsname is plain memory that uname fills; a zeroed struct is a valid one to fill.
        let mut name: libc::utsname = unsafe { std::mem::zeroed() };
        if unsafe { libc::uname(&mut name) } == 0 {
            let release = unsafe { std::ffi::CStr::from_ptr(name.release.as_ptr()) };
            return release.to_string_lossy().into_owned();
        }
        String::new()
    }
    #[cfg(windows)]
    {
        #[repr(C)]
        struct OsVersionInfo {
            size: u32,
            major: u32,
            minor: u32,
            build: u32,
            platform: u32,
            service_pack: [u16; 128],
        }
        #[link(name = "ntdll")]
        extern "system" {
            fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
        }
        let mut info = OsVersionInfo {
            size: std::mem::size_of::<OsVersionInfo>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform: 0,
            service_pack: [0; 128],
        };
        // SAFETY: the struct is sized and laid out as RTL_OSVERSIONINFOW, which RtlGetVersion fills.
        if unsafe { RtlGetVersion(&mut info) } == 0 {
            return format!("{}.{}.{}", info.major, info.minor, info.build);
        }
        String::new()
    }
    #[cfg(not(any(unix, windows)))]
    {
        String::new()
    }
}

/// `detect_color_depth` for this process: its environment, platform and OS release.
pub fn detect_color_depth_here() -> ColorDepth {
    detect_color_depth(&kumi_common::env::vars(), process_platform(), &os_release())
}

static TERM_256: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-256(color)?$").unwrap());

/// Chooses the colour depth from the environment; `NO_COLOR` keeps bold and dim only. On Windows the
/// console has drawn 24-bit colour since Windows 10 build 14931, as Windows Terminal does, and neither
/// says so in the environment.
pub fn detect_color_depth(env: &HashMap<String, String>, platform: &str, release: &str) -> ColorDepth {
    if env.get("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return ColorDepth::None;
    }
    if let Some(forced) = env.get("KUMI_COLOR").and_then(|value| ColorDepth::parse(value)) {
        return forced;
    }
    let colorterm = env.get("COLORTERM").map(|value| value.to_lowercase()).unwrap_or_default();
    if colorterm == "truecolor" || colorterm == "24bit" {
        return ColorDepth::Truecolor;
    }
    if env.get("TERM_PROGRAM").is_some_and(|value| value == "Apple_Terminal") {
        return ColorDepth::Colors256;
    }
    let term = env.get("TERM").map(String::as_str).unwrap_or("");
    if term == "dumb" {
        return ColorDepth::None;
    }
    if platform == "win32" {
        // `Number(part)`: a missing part is 0, one that isn't a number compares false below.
        let part = |index: usize| release.split('.').nth(index).map_or(Some(0.0), js::number::parse);
        let (major, build) = (part(0), part(2));
        if major.is_some_and(|major| major >= 10.0) && build.is_some_and(|build| build >= 14931.0) {
            return ColorDepth::Truecolor;
        }
    }
    if TERM_256.is_match(term) {
        return ColorDepth::Colors256;
    }
    ColorDepth::Colors16
}

/// A stable key for comparing styles.
pub fn style_key(style: &Style) -> String {
    let color = |rgb: Option<Rgb>| rgb.map_or_else(|| "-".to_string(), |rgb| format!("{},{},{}", rgb[0], rgb[1], rgb[2]));
    let flag = |on: bool| if on { 1 } else { 0 };
    format!(
        "{}|{}|{}{}{}{}{}",
        color(style.fg),
        color(style.bg),
        flag(style.bold),
        flag(style.dim),
        flag(style.italic),
        flag(style.underline),
        flag(style.inverse)
    )
}

/// Interns styles so screen cells can hold a small number; id 0 is the terminal default.
#[derive(Debug)]
pub struct StyleTable {
    ids: RefCell<HashMap<Style, u32>>,
    styles: RefCell<Vec<Style>>,
}

impl Default for StyleTable {
    fn default() -> Self {
        Self::new()
    }
}

impl StyleTable {
    pub fn new() -> StyleTable {
        StyleTable { ids: RefCell::new(HashMap::from([(Style::default(), 0)])), styles: RefCell::new(vec![Style::default()]) }
    }

    pub fn id(&self, style: &Style) -> u32 {
        if let Some(id) = self.ids.borrow().get(style) {
            return *id;
        }
        let mut styles = self.styles.borrow_mut();
        let id = styles.len() as u32;
        styles.push(*style);
        self.ids.borrow_mut().insert(*style, id);
        id
    }

    pub fn style(&self, id: u32) -> Style {
        self.styles.borrow().get(id as usize).copied().unwrap_or_default()
    }
}

// The xterm palette for the 16 basic colours, used to find the nearest match.
const BASIC: [Rgb; 16] = [
    [0, 0, 0],
    [205, 0, 0],
    [0, 205, 0],
    [205, 205, 0],
    [0, 0, 238],
    [205, 0, 205],
    [0, 205, 205],
    [229, 229, 229],
    [127, 127, 127],
    [255, 0, 0],
    [0, 255, 0],
    [255, 255, 0],
    [92, 92, 255],
    [255, 0, 255],
    [0, 255, 255],
    [255, 255, 255],
];

fn distance(a: Rgb, b: Rgb) -> i64 {
    let square = |x: u8, y: u8| (x as i64 - y as i64).pow(2);
    square(a[0], b[0]) + square(a[1], b[1]) + square(a[2], b[2])
}

const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The index of the first entry nearest to the target, by `distance`; a tie keeps the earlier one.
fn nearest<T: Copy>(entries: &[T], distance: impl Fn(T) -> i64) -> usize {
    let mut best = 0;
    for (index, entry) in entries.iter().enumerate().skip(1) {
        if distance(*entry) < distance(entries[best]) {
            best = index;
        }
    }
    best
}

pub fn to256(rgb: Rgb) -> u8 {
    let level = |channel: u8| nearest(&CUBE, |step| (step as i64 - channel as i64).abs());
    let (r, g, b) = (level(rgb[0]), level(rgb[1]), level(rgb[2]));
    let cube: Rgb = [CUBE[r], CUBE[g], CUBE[b]];
    let average = js::number::round((rgb[0] as f64 + rgb[1] as f64 + rgb[2] as f64) / 3.0);
    let gray_index = js::number::round((average - 8.0) / 10.0).clamp(0.0, 23.0) as u8;
    let gray_value = 8 + gray_index * 10;
    let gray: Rgb = [gray_value, gray_value, gray_value];
    if distance(gray, rgb) < distance(cube, rgb) {
        232 + gray_index
    } else {
        16 + 36 * r as u8 + 6 * g as u8 + b as u8
    }
}

pub fn to16(rgb: Rgb) -> u8 {
    nearest(&BASIC, |basic| distance(basic, rgb)) as u8
}

/// The complete SGR sequence for a style: always starts from a reset, so output never inherits.
pub fn sgr(style: &Style, depth: ColorDepth) -> String {
    let mut codes: Vec<String> = vec!["0".to_string()];
    if style.bold {
        codes.push("1".to_string());
    }
    if style.dim {
        codes.push("2".to_string());
    }
    if style.italic {
        codes.push("3".to_string());
    }
    if style.underline {
        codes.push("4".to_string());
    }
    if style.inverse {
        codes.push("7".to_string());
    }
    let mut color = |rgb: Option<Rgb>, background: bool| {
        let Some(rgb) = rgb else { return };
        match depth {
            ColorDepth::None => {}
            ColorDepth::Truecolor => codes.push(format!("{};2;{};{};{}", if background { 48 } else { 38 }, rgb[0], rgb[1], rgb[2])),
            ColorDepth::Colors256 => codes.push(format!("{};5;{}", if background { 48 } else { 38 }, to256(rgb))),
            ColorDepth::Colors16 => {
                let index = to16(rgb);
                let base = if index < 8 {
                    if background {
                        40
                    } else {
                        30
                    }
                } else if background {
                    100
                } else {
                    90
                };
                codes.push((base + index % 8).to_string());
            }
        }
    };
    color(style.fg, false);
    color(style.bg, true);
    format!("\u{1b}[{}m", codes.join(";"))
}
