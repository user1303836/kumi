//! Kumi's icons for what's in a Set: a small silhouette, two cells wide, tinted by family (audio,
//! MIDI, instruments, plug-ins; a track in its own Live colour). Only characters every terminal Kumi
//! supports draws (box drawing, blocks and a few geometric shapes, no Braille, no pictures); where
//! even those don't show, a two-letter badge stands in. The tint goes on the icon only, never the
//! row's text. What follows an icon leaves one space after its two cells.

use std::collections::HashMap;

use super::style::{process_platform, Rgb, Style};
use super::wrap::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconKind {
    AudioEffect,
    MidiEffect,
    Instrument,
    DrumInstrument,
    Device,
    AudioRack,
    InstrumentRack,
    MidiRack,
    DrumRack,
    MaxAudio,
    MaxMidi,
    MaxInstrument,
    Plugin,
    Chain,
    DrumPad,
    AudioTrack,
    MidiTrack,
    GroupTrack,
    ReturnTrack,
    MainTrack,
    Scene,
    AudioClip,
    MidiClip,
    Locator,
    Sample,
    Preset,
    Groove,
    Tuning,
}

impl IconKind {
    /// Every kind, in the order the TypeScript lists them.
    pub const ALL: [IconKind; 28] = [
        IconKind::AudioEffect,
        IconKind::MidiEffect,
        IconKind::Instrument,
        IconKind::DrumInstrument,
        IconKind::Device,
        IconKind::AudioRack,
        IconKind::InstrumentRack,
        IconKind::MidiRack,
        IconKind::DrumRack,
        IconKind::MaxAudio,
        IconKind::MaxMidi,
        IconKind::MaxInstrument,
        IconKind::Plugin,
        IconKind::Chain,
        IconKind::DrumPad,
        IconKind::AudioTrack,
        IconKind::MidiTrack,
        IconKind::GroupTrack,
        IconKind::ReturnTrack,
        IconKind::MainTrack,
        IconKind::Scene,
        IconKind::AudioClip,
        IconKind::MidiClip,
        IconKind::Locator,
        IconKind::Sample,
        IconKind::Preset,
        IconKind::Groove,
        IconKind::Tuning,
    ];

    /// The kind's name as the TypeScript spelt it ("audio-effect").
    pub fn as_str(self) -> &'static str {
        match self {
            IconKind::AudioEffect => "audio-effect",
            IconKind::MidiEffect => "midi-effect",
            IconKind::Instrument => "instrument",
            IconKind::DrumInstrument => "drum-instrument",
            IconKind::Device => "device",
            IconKind::AudioRack => "audio-rack",
            IconKind::InstrumentRack => "instrument-rack",
            IconKind::MidiRack => "midi-rack",
            IconKind::DrumRack => "drum-rack",
            IconKind::MaxAudio => "max-audio",
            IconKind::MaxMidi => "max-midi",
            IconKind::MaxInstrument => "max-instrument",
            IconKind::Plugin => "plugin",
            IconKind::Chain => "chain",
            IconKind::DrumPad => "drum-pad",
            IconKind::AudioTrack => "audio-track",
            IconKind::MidiTrack => "midi-track",
            IconKind::GroupTrack => "group-track",
            IconKind::ReturnTrack => "return-track",
            IconKind::MainTrack => "main-track",
            IconKind::Scene => "scene",
            IconKind::AudioClip => "audio-clip",
            IconKind::MidiClip => "midi-clip",
            IconKind::Locator => "locator",
            IconKind::Sample => "sample",
            IconKind::Preset => "preset",
            IconKind::Groove => "groove",
            IconKind::Tuning => "tuning",
        }
    }
}

/// Soft tints near Kumi's palette, one per family.
pub mod tints {
    use super::Rgb;

    pub const AUDIO: Rgb = [0x7f, 0xd1, 0xc7];
    pub const MIDI: Rgb = [0x8c, 0xc8, 0xff];
    pub const INSTRUMENT: Rgb = [0xe7, 0xc8, 0x8f];
    pub const PLUGIN: Rgb = [0xc7, 0xa6, 0xff];
    pub const NEUTRAL: Rgb = [0xa6, 0xac, 0xb5];
    pub const QUIET: Rgb = [0x80, 0x86, 0x8f];
}

/// A kind's glyph, its tint (a track's is its own colour), and the badge that stands in for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icon {
    pub glyph: &'static str,
    pub tint: Rgb,
    pub badge: &'static str,
}

/// Each kind: its glyph, its tint (a track's is its own colour), and the badge that stands in for it.
pub fn icons(kind: IconKind) -> Icon {
    let icon = |glyph, tint, badge| Icon { glyph, tint, badge };
    match kind {
        IconKind::AudioEffect => icon("≈", tints::AUDIO, "FX"),
        IconKind::MidiEffect => icon("♪", tints::MIDI, "ME"),
        IconKind::Instrument => icon("◆", tints::INSTRUMENT, "IN"),
        IconKind::DrumInstrument => icon("●", tints::INSTRUMENT, "DI"),
        IconKind::Device => icon("◇", tints::NEUTRAL, "DV"),
        IconKind::AudioRack => icon("▣", tints::AUDIO, "AR"),
        IconKind::InstrumentRack => icon("▣", tints::INSTRUMENT, "IR"),
        IconKind::MidiRack => icon("▣", tints::MIDI, "MR"),
        IconKind::DrumRack => icon("▦", tints::INSTRUMENT, "DR"),
        IconKind::MaxAudio => icon("∞", tints::AUDIO, "MA"),
        IconKind::MaxMidi => icon("∞", tints::MIDI, "MM"),
        IconKind::MaxInstrument => icon("∞", tints::INSTRUMENT, "MX"),
        IconKind::Plugin => icon("□", tints::PLUGIN, "PL"),
        IconKind::Chain => icon("○", tints::QUIET, "CH"),
        IconKind::DrumPad => icon("▪", tints::INSTRUMENT, "PD"),
        IconKind::AudioTrack => icon("■", tints::NEUTRAL, "AT"),
        IconKind::MidiTrack => icon("■", tints::NEUTRAL, "MT"),
        IconKind::GroupTrack => icon("▣", tints::NEUTRAL, "GR"),
        IconKind::ReturnTrack => icon("↩", tints::NEUTRAL, "RT"),
        IconKind::MainTrack => icon("●", tints::NEUTRAL, "MN"),
        IconKind::Scene => icon("▶", tints::QUIET, "SC"),
        IconKind::AudioClip => icon("▬", tints::AUDIO, "AC"),
        IconKind::MidiClip => icon("▬", tints::MIDI, "MC"),
        IconKind::Locator => icon("▼", tints::QUIET, "LC"),
        IconKind::Sample => icon("~", tints::AUDIO, "SM"),
        IconKind::Preset => icon("▫", tints::NEUTRAL, "PR"),
        IconKind::Groove => icon("≋", tints::QUIET, "GV"),
        IconKind::Tuning => icon("♯", tints::QUIET, "TU"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconStyle {
    Glyphs,
    Badges,
}

/// `detect_icon_style` for this process: its environment and platform.
pub fn detect_icon_style_here() -> IconStyle {
    detect_icon_style(&std::env::vars().collect(), process_platform())
}

/// Glyphs, or badges where they may not show: KUMI_ICONS chooses outright; otherwise badges on the
/// Linux console, a dumb terminal, and the old Windows console. A Windows terminal that names itself
/// (Windows Terminal, ConEmu, and those that set TERM_PROGRAM: VS Code, WezTerm, mintty) has the glyphs.
pub fn detect_icon_style(env: &HashMap<String, String>, platform: &str) -> IconStyle {
    let get = |name: &str| env.get(name).map(String::as_str);
    match get("KUMI_ICONS") {
        Some("badges") => return IconStyle::Badges,
        Some("glyphs") => return IconStyle::Glyphs,
        _ => {}
    }
    if matches!(get("TERM"), Some("linux") | Some("dumb")) {
        return IconStyle::Badges;
    }
    if platform == "win32" && get("WT_SESSION").is_none() && get("TERM_PROGRAM").is_none() && get("ConEmuANSI").is_none() {
        return IconStyle::Badges;
    }
    IconStyle::Glyphs
}

/// A kind's icon as text two cells wide, and its style; `color` is a track's own colour.
pub fn icon(kind: IconKind, style: IconStyle, color: Option<Rgb>) -> Span {
    let found = icons(kind);
    let tint = color.unwrap_or(found.tint);
    match style {
        IconStyle::Badges => Span::styled(found.badge, Style { fg: Some(tint), bold: true, ..Style::default() }),
        IconStyle::Glyphs => Span::styled(format!("{} ", found.glyph), Style::fg(tint)),
    }
}

/// What the bridge says about a device row: its class, whether it holds chains or pads, and Live's device type when sent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceRow<'a> {
    pub class_name: Option<&'a str>,
    pub can_have_chains: Option<bool>,
    pub can_have_drum_pads: Option<bool>,
    pub device_type: Option<&'a str>,
}

impl<'a> DeviceRow<'a> {
    /// The fields from a JSON object, read as loosely as the TypeScript read them.
    pub fn from_value(value: &'a serde_json::Value) -> DeviceRow<'a> {
        DeviceRow {
            class_name: value.get("className").and_then(serde_json::Value::as_str),
            can_have_chains: value.get("canHaveChains").and_then(serde_json::Value::as_bool),
            can_have_drum_pads: value.get("canHaveDrumPads").and_then(serde_json::Value::as_bool),
            device_type: value.get("deviceType").and_then(serde_json::Value::as_str),
        }
    }
}

/// What kind of device a device row is, from what the bridge says about it (its class, and Live's device type when sent).
pub fn device_kind(row: &DeviceRow<'_>) -> IconKind {
    let cls = row.class_name.unwrap_or("");
    let kind = row.device_type.filter(|kind| matches!(*kind, "instrument" | "audio_effect" | "midi_effect"));
    if row.can_have_drum_pads == Some(true) || cls == "DrumGroupDevice" {
        return IconKind::DrumRack;
    }
    if row.can_have_chains == Some(true) || cls.ends_with("GroupDevice") {
        return if cls.starts_with("MidiEffect") || kind == Some("midi_effect") {
            IconKind::MidiRack
        } else if cls.starts_with("Instrument") || kind == Some("instrument") {
            IconKind::InstrumentRack
        } else {
            IconKind::AudioRack
        };
    }
    if cls.starts_with("MxDevice") {
        return if cls.contains("MidiEffect") {
            IconKind::MaxMidi
        } else if cls.contains("Instrument") {
            IconKind::MaxInstrument
        } else {
            IconKind::MaxAudio
        };
    }
    if cls.contains("Plugin") {
        return IconKind::Plugin;
    }
    if cls == "DrumCell" || cls == "DrumSampler" {
        return IconKind::DrumInstrument;
    }
    match kind {
        Some("instrument") => IconKind::Instrument,
        Some("audio_effect") => IconKind::AudioEffect,
        Some("midi_effect") => IconKind::MidiEffect,
        _ => IconKind::Device,
    }
}

/// A track's icon kind from the focus feed's or the bridge's words for it.
pub fn track_kind(kind: Option<&str>) -> IconKind {
    match kind {
        Some("midi") => IconKind::MidiTrack,
        Some("group") => IconKind::GroupTrack,
        Some("return") => IconKind::ReturnTrack,
        Some("main") => IconKind::MainTrack,
        _ => IconKind::AudioTrack,
    }
}
