//! Colours, text styles and the terminal escape codes that draw them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
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
    /// The address the text opens where the terminal allows (OSC 8), by `link_id`; 0 for none.
    pub link: u32,
}

impl Style {
    /// A style of a foreground colour only.
    pub const fn fg(rgb: Rgb) -> Style {
        Style { fg: Some(rgb), bg: None, bold: false, dim: false, italic: false, underline: false, inverse: false, link: 0 }
    }

    /// A style of a background colour only.
    pub const fn bg(rgb: Rgb) -> Style {
        Style { fg: None, bg: Some(rgb), bold: false, dim: false, italic: false, underline: false, inverse: false, link: 0 }
    }

    /// This style, its text opening `url`.
    pub fn linked(self, url: &str) -> Style {
        Style { link: link_id(url), ..self }
    }
}

thread_local! {
    /// The addresses linked text opens, numbered from 1 so a style can carry one and stay a copy.
    static LINKS: RefCell<(Vec<Rc<str>>, HashMap<Rc<str>, u32>)> = RefCell::default();
}

/// The number styles carry `url` by; the same address always has the same one.
pub fn link_id(url: &str) -> u32 {
    LINKS.with_borrow_mut(|(urls, ids)| {
        if let Some(id) = ids.get(url) {
            return *id;
        }
        let url: Rc<str> = url.into();
        urls.push(Rc::clone(&url));
        ids.insert(url, urls.len() as u32);
        urls.len() as u32
    })
}

/// The address link `id` opens; None for 0.
pub fn link_url(id: u32) -> Option<Rc<str>> {
    LINKS.with_borrow(|(urls, _)| id.checked_sub(1).and_then(|index| urls.get(index as usize).cloned()))
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
    /// Code in answers, by what each run is: keywords, strings and numbers (its comments are faint, the rest bright).
    pub const CODE_KEYWORD: Rgb = [0x7a, 0xa2, 0xf7];
    pub const CODE_STRING: Rgb = [0xb8, 0xd4, 0x7e];
    pub const CODE_NUMBER: Rgb = [0xe4, 0x8e, 0xe0];

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

/// The palette as a theme draws it. Screens are drawn in the palette's own colours (the dark theme's, above), and
/// the renderer writes each in the theme's colour for it; colours mixed from them (a shimmer, a fade, a picture's
/// edges) are mixed from the theme's. Every theme keeps what each colour means: the accent for done and applied, the
/// warning, the error, one colour per kind of memory, and the icons' tint for each family. Track colours come from
/// the DAW itself, in every theme (a light theme only deepens one too pale to read on it). An image's placeholder
/// cells carry its id in their colour, and Kumi's ids (counted from 1) stay far below the palette's lowest colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Its name in settings.json's `theme`.
    pub name: &'static str,
    pub ground: Rgb,
    pub surface: Rgb,
    pub raised: Rgb,
    pub selected: Rgb,
    pub rule: Rgb,
    pub track: Rgb,
    pub faint: Rgb,
    pub dim: Rgb,
    pub text: Rgb,
    pub bright: Rgb,
    pub accent: Rgb,
    pub pulse: Rgb,
    pub warn: Rgb,
    pub beat: Rgb,
    pub offbeat: Rgb,
    pub error: Rgb,
    pub note: Rgb,
    pub technique: Rgb,
    pub recipe: Rgb,
    pub lesson: Rgb,
    /// Code's keywords, strings and numbers.
    pub code_keyword: Rgb,
    pub code_string: Rgb,
    pub code_number: Rgb,
    /// The tint of audio's icons (the other families' tints are the palette's: MIDI's the note's, an instrument's the
    /// lesson's, a plug-in's the technique's).
    pub audio: Rgb,
}

impl Theme {
    /// Its colours in the palette's order, then audio's tint.
    const fn colors(&self) -> [Rgb; 24] {
        [
            self.ground,
            self.surface,
            self.raised,
            self.selected,
            self.rule,
            self.track,
            self.faint,
            self.dim,
            self.text,
            self.bright,
            self.accent,
            self.pulse,
            self.warn,
            self.beat,
            self.offbeat,
            self.error,
            self.note,
            self.technique,
            self.recipe,
            self.lesson,
            self.code_keyword,
            self.code_string,
            self.code_number,
            self.audio,
        ]
    }

    /// This theme's colour for `rgb`, one of the palette's; any other colour (a track's) as it is.
    pub fn of(&self, rgb: Rgb) -> Rgb {
        if self.name == DARK.name {
            return rgb;
        }
        DARK.colors().iter().position(|color| *color == rgb).map_or(rgb, |at| self.colors()[at])
    }

    /// `style` in this theme's colours.
    pub fn style(&self, style: &Style) -> Style {
        Style { fg: style.fg.map(|rgb| self.of(rgb)), bg: style.bg.map(|rgb| self.of(rgb)), ..*style }
    }

    /// Whether its ground is light (nearer white than black): a colour too pale to read on it is deepened.
    pub fn is_light(&self) -> bool {
        contrast(self.ground, [0, 0, 0]) > contrast(self.ground, [0xff, 0xff, 0xff])
    }

    /// `rgb` (a track's colour, a sound class's) as it reads on this theme's grounds: on a light theme, a colour too
    /// pale to read deepened just enough (3:1 against `raised`, the darkest ground words are drawn on); on a dark one,
    /// as it is.
    pub fn legible(&self, rgb: Rgb) -> Rgb {
        if !self.is_light() || contrast(rgb, self.raised) >= 3. {
            return rgb;
        }
        let deepened = |share: f64| rgb.map(|c| js::number::round(c as f64 * share) as u8);
        // The least deepening that reads, found by halves: `reads` always does, `pale` doesn't.
        let (mut reads, mut pale) = (0., 1.);
        for _ in 0..7 {
            let share = (reads + pale) / 2.;
            if contrast(deepened(share), self.raised) >= 3. {
                reads = share;
            } else {
                pale = share;
            }
        }
        deepened(reads)
    }
}

/// WCAG's contrast between two colours, 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let luminance = |rgb: Rgb| {
        let linear = |c: u8| {
            let c = c as f64 / 255.;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Kumi's own: light words on a near-black ground, a mint accent.
pub const DARK: Theme = Theme {
    name: "dark",
    ground: palette::GROUND,
    surface: palette::SURFACE,
    raised: palette::RAISED,
    selected: palette::SELECTED,
    rule: palette::RULE,
    track: palette::TRACK,
    faint: palette::FAINT,
    dim: palette::DIM,
    text: palette::TEXT,
    bright: palette::BRIGHT,
    accent: palette::ACCENT,
    pulse: palette::PULSE,
    warn: palette::WARN,
    beat: palette::BEAT,
    offbeat: palette::OFFBEAT,
    error: palette::ERROR,
    note: palette::NOTE,
    technique: palette::TECHNIQUE,
    recipe: palette::RECIPE,
    lesson: palette::LESSON,
    code_keyword: palette::CODE_KEYWORD,
    code_string: palette::CODE_STRING,
    code_number: palette::CODE_NUMBER,
    audio: super::icons::tints::AUDIO,
};

/// For a light terminal, made for it rather than turned over: dark words on a warm paper white, each step of the greys
/// as far apart as the dark theme's, words at 4.5:1 or more on every ground as dark's are, and every colour deep enough
/// to read on white (the accent a deeper mint, the warning amber rather than yellow, code's colours deeper too).
pub const LIGHT: Theme = Theme {
    name: "light",
    ground: [0xf7, 0xf7, 0xf4],
    surface: [0xef, 0xef, 0xeb],
    raised: [0xe6, 0xe7, 0xe2],
    selected: [0xcf, 0xe9, 0xdc],
    rule: [0xc3, 0xc7, 0xcc],
    track: [0xa9, 0xae, 0xb6],
    faint: [0x63, 0x68, 0x71],
    dim: [0x44, 0x4b, 0x54],
    text: [0x26, 0x2b, 0x31],
    bright: [0x0b, 0x0d, 0x10],
    accent: [0x14, 0x76, 0x4e],
    pulse: [0xa8, 0xdc, 0xc2],
    warn: [0x92, 0x5b, 0x0d],
    beat: [0xd1, 0x9a, 0x00],
    offbeat: [0xe9, 0xdf, 0xb8],
    error: [0xbb, 0x39, 0x2c],
    note: [0x2a, 0x6f, 0xc4],
    technique: [0x74, 0x48, 0xc9],
    recipe: [0xb8, 0x45, 0x7a],
    lesson: [0x96, 0x70, 0x1f],
    code_keyword: [0x3a, 0x4f, 0xb8],
    code_string: [0x4a, 0x6a, 0x10],
    code_number: [0x9c, 0x2f, 0x9a],
    audio: [0x1c, 0x80, 0x8c],
};

/// The dark theme with colours that stay apart for red-green colour blindness (from Okabe and Ito's set): the accent
/// sky blue, the warning orange, the error vermilion (a little lighter, to read at 4.5:1), the kinds of memory bluish
/// green, reddish purple, yellow and
/// blue, and audio's icons yellow (MIDI's, an instrument's and a plug-in's follow the note, the lesson and the
/// technique). Code's numbers are pink, which stays apart from its blue keywords as orchid doesn't.
pub const COLOR_BLIND: Theme = Theme {
    name: "colorblind",
    selected: [0x1d, 0x32, 0x44],
    accent: [0x56, 0xb4, 0xe9],
    pulse: [0x24, 0x46, 0x5c],
    warn: [0xe6, 0x9f, 0x00],
    beat: [0xf0, 0xe4, 0x42],
    offbeat: [0x4d, 0x48, 0x20],
    error: [0xd7, 0x65, 0x0b],
    note: [0x19, 0xb3, 0x8a],
    technique: [0xcc, 0x79, 0xa7],
    recipe: [0xf0, 0xe4, 0x42],
    lesson: [0x4c, 0x8f, 0xd6],
    code_number: [0xf0, 0xa8, 0xd8],
    audio: [0xf0, 0xe4, 0x42],
    ..DARK
};

/// Every theme, by its name in settings.json.
pub const THEMES: [&Theme; 3] = [&DARK, &LIGHT, &COLOR_BLIND];

thread_local! {
    /// The theme the screen is drawn in (Kumi's TUI keeps to one thread).
    static THEME: Cell<&'static Theme> = const { Cell::new(&DARK) };
}

/// The theme the screen is drawn in now: dark until the app sets the producer's.
pub fn theme() -> &'static Theme {
    THEME.with(Cell::get)
}

pub fn set_theme(theme: &'static Theme) {
    THEME.with(|current| current.set(theme));
}

/// The theme for a terminal's background (`system`): light on a light one, dark on a dark one or one not known.
pub fn for_background(background: Option<Rgb>) -> &'static Theme {
    match background {
        Some(background) if contrast(background, [0, 0, 0]) > contrast(background, [0xff, 0xff, 0xff]) => &LIGHT,
        _ => &DARK,
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

/// The colour depth once the terminal has named itself (`tui::probe`): one known to draw 24-bit colour does, where its
/// environment doesn't say so (over SSH, say). A depth the environment forces (NO_COLOR, KUMI_COLOR) stays, and tmux,
/// which answers for itself, keeps the environment's.
pub fn answered_color_depth(depth: ColorDepth, version: Option<&str>, env: &HashMap<String, String>) -> ColorDepth {
    let forced = env.get("NO_COLOR").is_some_and(|value| !value.is_empty())
        || env.get("KUMI_COLOR").and_then(|value| ColorDepth::parse(value)).is_some();
    let name = version.unwrap_or("").to_ascii_lowercase();
    let truecolor = ["kitty", "ghostty", "wezterm", "iterm2", "foot"].iter().any(|known| name.starts_with(known));
    if truecolor && !forced {
        ColorDepth::Truecolor
    } else {
        depth
    }
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

/// A stable key for comparing styles; a link adds its address.
pub fn style_key(style: &Style) -> String {
    let color = |rgb: Option<Rgb>| rgb.map_or_else(|| "-".to_string(), |rgb| format!("{},{},{}", rgb[0], rgb[1], rgb[2]));
    let flag = |on: bool| if on { 1 } else { 0 };
    format!(
        "{}|{}|{}{}{}{}{}{}",
        color(style.fg),
        color(style.bg),
        flag(style.bold),
        flag(style.dim),
        flag(style.italic),
        flag(style.underline),
        flag(style.inverse),
        link_url(style.link).map_or_else(String::new, |url| format!("|{url}"))
    )
}

/// Where cluster numbers start: past every character's code.
const CLUSTERS: u32 = 0x11_0000;

/// Interns styles so screen cells can hold a small number; id 0 is the terminal default. Graphemes are numbered here
/// too: a lone character by its own code, a cluster of them (a letter and its marks, an emoji sequence, a picture's
/// placeholder) by its place in the table, past every character's.
#[derive(Debug)]
pub struct StyleTable {
    ids: RefCell<HashMap<Style, u32>>,
    styles: RefCell<Vec<Style>>,
    /// The style asked for last, with its id: a row's cells mostly share one, and a frame asks per cell.
    last: Cell<(Style, u32)>,
    clusters: RefCell<Vec<Rc<str>>>,
    cluster_ids: RefCell<HashMap<Rc<str>, u32>>,
}

impl Default for StyleTable {
    fn default() -> Self {
        Self::new()
    }
}

impl StyleTable {
    pub fn new() -> StyleTable {
        StyleTable {
            ids: RefCell::new(HashMap::from([(Style::default(), 0)])),
            styles: RefCell::new(vec![Style::default()]),
            last: Cell::new((Style::default(), 0)),
            clusters: RefCell::default(),
            cluster_ids: RefCell::default(),
        }
    }

    pub fn id(&self, style: &Style) -> u32 {
        let last = self.last.get();
        if last.0 == *style {
            return last.1;
        }
        let known = self.ids.borrow().get(style).copied();
        let id = known.unwrap_or_else(|| {
            let mut styles = self.styles.borrow_mut();
            let id = styles.len() as u32;
            styles.push(*style);
            self.ids.borrow_mut().insert(*style, id);
            id
        });
        self.last.set((*style, id));
        id
    }

    pub fn style(&self, id: u32) -> Style {
        self.styles.borrow().get(id as usize).copied().unwrap_or_default()
    }

    /// The number a cell holds for `grapheme`; 0 for none (a wide character's second cell).
    pub fn grapheme(&self, grapheme: &str) -> u32 {
        let mut characters = grapheme.chars();
        match (characters.next(), characters.next()) {
            (None, _) => 0,
            (Some(character), None) => character as u32,
            _ => {
                if let Some(id) = self.cluster_ids.borrow().get(grapheme) {
                    return *id;
                }
                let mut clusters = self.clusters.borrow_mut();
                let cluster: Rc<str> = grapheme.into();
                let id = CLUSTERS + clusters.len() as u32;
                clusters.push(Rc::clone(&cluster));
                self.cluster_ids.borrow_mut().insert(cluster, id);
                id
            }
        }
    }

    /// The grapheme numbered `code`, onto `out`.
    pub fn push_grapheme(&self, code: u32, out: &mut String) {
        if code >= CLUSTERS {
            out.push_str(&self.clusters.borrow()[(code - CLUSTERS) as usize]);
        } else if let Some(character) = char::from_u32(code).filter(|_| code != 0) {
            out.push(character);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dark_theme_draws_the_palette_as_it_is() {
        for rgb in DARK.colors().into_iter().chain([[1, 2, 3], [0xff, 0x5c, 0x5c]]) {
            assert_eq!(DARK.of(rgb), rgb);
        }
        let style = Style { fg: Some(palette::ACCENT), bg: Some(palette::RAISED), bold: true, ..Style::default() };
        assert_eq!(DARK.style(&style), style);
    }

    #[test]
    fn each_theme_gives_every_colour_of_the_palette_its_own_and_leaves_a_tracks_alone() {
        for theme in THEMES {
            for (dark, own) in DARK.colors().into_iter().zip(theme.colors()) {
                assert_eq!(theme.of(dark), own, "{}", theme.name);
            }
            assert_eq!(theme.of([0xff, 0x5c, 0x5c]), [0xff, 0x5c, 0x5c], "a track's colour, in {}", theme.name);
        }
    }

    #[test]
    fn every_theme_keeps_the_colours_that_mean_something_apart() {
        for theme in THEMES {
            // Done and applied, the warning, the error, and each kind of memory: no two alike, nor like the words.
            let meanings = [theme.accent, theme.warn, theme.error, theme.note, theme.technique, theme.recipe, theme.lesson];
            for (at, a) in meanings.iter().enumerate() {
                for b in &meanings[at + 1..] {
                    assert_ne!(a, b, "{}", theme.name);
                }
                for ground in [theme.ground, theme.surface, theme.raised] {
                    assert!(contrast(*a, ground) >= 3., "{} {a:?} on {ground:?}: {:.1}", theme.name, contrast(*a, ground));
                }
            }
        }
    }

    #[test]
    fn every_theme_keeps_the_icons_families_apart() {
        // Each icon's tint is one the theme draws: audio's its own, the others the palette's.
        use super::super::icons::tints;
        for tint in [tints::AUDIO, tints::MIDI, tints::INSTRUMENT, tints::PLUGIN, tints::NEUTRAL, tints::QUIET] {
            assert!(DARK.colors().contains(&tint), "{tint:?}");
        }
        for theme in THEMES {
            // Audio, MIDI, an instrument, a plug-in.
            let families = [theme.audio, theme.of(tints::MIDI), theme.of(tints::INSTRUMENT), theme.of(tints::PLUGIN)];
            for (at, a) in families.iter().enumerate() {
                for b in &families[at + 1..] {
                    assert_ne!(a, b, "{}", theme.name);
                }
                for ground in [theme.ground, theme.surface, theme.raised] {
                    assert!(contrast(*a, ground) >= 3., "{} {a:?} on {ground:?}: {:.1}", theme.name, contrast(*a, ground));
                }
            }
        }
    }

    #[test]
    fn a_light_theme_deepens_a_colour_too_pale_to_read_on_it_and_a_dark_one_leaves_it() {
        let mut deepened = 0;
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    let rgb = [r as u8, g as u8, b as u8];
                    assert_eq!(DARK.legible(rgb), rgb);
                    assert_eq!(COLOR_BLIND.legible(rgb), rgb);
                    let light = LIGHT.legible(rgb);
                    // On every ground it's drawn on: the darkest is `raised`.
                    for ground in [LIGHT.ground, LIGHT.surface, LIGHT.raised] {
                        assert!(contrast(light, ground) >= 3., "{rgb:?} as {light:?} on {ground:?}: {:.2}", contrast(light, ground));
                    }
                    if light == rgb {
                        continue;
                    }
                    deepened += 1;
                    // Just enough: a little less deep wouldn't read.
                    let top = (0..3).max_by_key(|at| rgb[*at]).unwrap();
                    let share = light[top] as f64 / rgb[top] as f64;
                    let lighter = rgb.map(|c| (c as f64 * (share + 0.02)).round() as u8);
                    assert!(contrast(lighter, LIGHT.raised) < 3., "{rgb:?} as {light:?} is deeper than it needs");
                }
            }
        }
        assert!(deepened > 1000, "{deepened}");
    }

    #[test]
    fn an_images_id_is_never_taken_for_a_palette_colour() {
        // A placeholder carries its image's id in its colour, and Kumi counts ids from 1: the palette's lowest colour,
        // as an id, is one no session reaches.
        let lowest = DARK.colors().iter().map(|[r, g, b]| ((*r as u32) << 16) | ((*g as u32) << 8) | *b as u32).min().unwrap();
        assert!(lowest > 900_000, "{lowest}");
    }

    #[test]
    fn every_themes_words_read_at_small_texts_contrast_on_each_of_its_grounds() {
        // WCAG's 4.5:1 for small text: the greys words are written in, and the accent, the warning and the error, which
        // are words too ("undo", "check Live"). Marks (memory's glyphs, icons, a track's chip) need 3:1, tested above.
        for theme in THEMES {
            let words = [theme.faint, theme.dim, theme.text, theme.bright, theme.accent, theme.warn, theme.error];
            for (word, ground) in
                words.into_iter().flat_map(|word| [theme.ground, theme.surface, theme.raised].map(|ground| (word, ground)))
            {
                assert!(contrast(word, ground) >= 4.5, "{} {word:?} on {ground:?}: {:.2}", theme.name, contrast(word, ground));
            }
        }
        // And the steps between the light greys as far apart as dark's (dark's: 1.61 and 1.63).
        assert!(contrast(LIGHT.faint, LIGHT.dim) >= 1.55 && contrast(LIGHT.dim, LIGHT.text) >= 1.55);
    }

    #[test]
    fn every_theme_colours_code_apart_and_legible_on_its_band() {
        // Keywords, strings and numbers are words on the code's band (`raised`, the hardest of the grounds to read on in
        // both dark and light), beside the code's own bright words and its faint comments: each reads at 4.5:1, and no
        // two are alike.
        for theme in THEMES {
            let code = [theme.code_keyword, theme.code_string, theme.code_number];
            for color in code {
                for ground in [theme.ground, theme.surface, theme.raised] {
                    let reads = contrast(color, ground);
                    assert!(reads >= 4.5, "{} {color:?} on {ground:?}: {reads:.2}", theme.name);
                }
            }
            let shown = [theme.code_keyword, theme.code_string, theme.code_number, theme.bright, theme.faint];
            for (at, a) in shown.iter().enumerate() {
                for b in &shown[at + 1..] {
                    assert_ne!(a, b, "{}", theme.name);
                }
            }
        }
    }

    #[test]
    fn a_terminals_background_chooses_light_or_dark() {
        assert_eq!(for_background(Some([0xff, 0xff, 0xff])).name, "light");
        assert_eq!(for_background(Some([0xfd, 0xf6, 0xe3])).name, "light");
        assert_eq!(for_background(Some([0x1e, 0x1e, 0x1e])).name, "dark");
        assert_eq!(for_background(Some([0x80, 0x80, 0x80])).name, "light");
        assert_eq!(for_background(Some([0x70, 0x70, 0x70])).name, "dark");
        assert_eq!(for_background(None).name, "dark");
    }

    #[test]
    fn the_light_theme_is_made_for_a_light_ground() {
        assert!(LIGHT.is_light() && !DARK.is_light() && !COLOR_BLIND.is_light());
        // The words as far apart from the ground as the dark theme's are: the text near black.
        assert!(contrast(LIGHT.text, LIGHT.ground) >= 12., "{:.1}", contrast(LIGHT.text, LIGHT.ground));
        // Each step of the greys darker than the one under it, as each is lighter in the dark theme.
        let greys = [LIGHT.ground, LIGHT.surface, LIGHT.raised, LIGHT.rule, LIGHT.track, LIGHT.faint, LIGHT.dim, LIGHT.text, LIGHT.bright];
        let lightness = |rgb: Rgb| rgb.iter().map(|c| *c as u32).sum::<u32>();
        assert!(greys.windows(2).all(|pair| lightness(pair[0]) > lightness(pair[1])), "{greys:?}");
    }

    #[test]
    fn what_a_theme_mixes_is_never_taken_for_one_of_the_palettes_colours() {
        // Mixed colours (a shimmer, a fade, the beat light) are mixed from the theme's colours, and the renderer then
        // writes each of the palette's colours in the theme's: a mix that came out as one of the dark palette's would be
        // turned again. In every theme, any mix of two of its colours at the steps Kumi mixes in is either none of the
        // palette's colours, or one the theme writes as it is.
        for theme in THEMES {
            let colors = theme.colors();
            for from in colors {
                for to in colors {
                    for steps in [5, 6] {
                        for step in 0..=steps {
                            let amount = step as f64 / steps as f64;
                            let mixed: Rgb =
                                std::array::from_fn(|i| js::number::round(from[i] as f64 + (to[i] as f64 - from[i] as f64) * amount) as u8);
                            assert_eq!(theme.of(mixed), mixed, "{} mixes {from:?} and {to:?} at {amount} into {mixed:?}", theme.name);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_screen_is_drawn_in_the_theme_set_until_another_is() {
        assert_eq!(theme().name, "dark");
        set_theme(&LIGHT);
        assert_eq!(theme().of(palette::ACCENT), LIGHT.accent);
        set_theme(&DARK);
        assert_eq!(theme().of(palette::ACCENT), palette::ACCENT);
    }
}
