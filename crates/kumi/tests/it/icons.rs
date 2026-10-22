use std::collections::{HashMap, HashSet};

use kumi::tui::icons::{detect_icon_style, device_kind, icon, icons, track_kind, DeviceRow, IconKind, IconStyle};
use kumi::tui::width::text_width;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
}

#[test]
fn every_icon_is_exactly_two_cells_as_a_glyph_or_a_badge_and_each_badge_is_its_own() {
    let kinds = IconKind::ALL;
    for kind in kinds {
        assert_eq!(text_width(&icon(kind, IconStyle::Glyphs, None).text), 2, "{}", kind.as_str());
        assert_eq!(text_width(&icon(kind, IconStyle::Badges, None).text), 2, "{}", kind.as_str());
        let badge = icon(kind, IconStyle::Badges, None).text;
        assert!(badge.len() == 2 && badge.bytes().all(|byte| byte.is_ascii_uppercase()), "{}", kind.as_str());
        // Box drawing, blocks, geometric shapes and a few symbols: nothing from Braille or the emoji ranges.
        let code = icons(kind).glyph.chars().next().unwrap() as u32;
        assert!(!(0x2800..=0x28ff).contains(&code) && code < 0x1f000, "{} uses a character every terminal draws", kind.as_str());
    }
    assert_eq!(kinds.iter().map(|kind| icons(*kind).badge).collect::<HashSet<_>>().len(), kinds.len());
}

#[test]
fn the_tint_is_on_the_icon_only_and_a_tracks_is_its_own_colour() {
    assert_eq!(*icon(IconKind::MidiTrack, IconStyle::Glyphs, Some([200, 40, 40])).style, kumi::tui::style::Style::fg([200, 40, 40]));
    assert!(!icon(IconKind::AudioEffect, IconStyle::Glyphs, None).style.bold);
}

#[test]
fn badges_stand_in_where_glyphs_may_not_show_chosen_outright_or_on_the_linux_console_and_the_old_windows_console() {
    assert_eq!(detect_icon_style(&env(&[("TERM_PROGRAM", "ghostty")]), "darwin"), IconStyle::Glyphs);
    assert_eq!(detect_icon_style(&env(&[("WT_SESSION", "x")]), "win32"), IconStyle::Glyphs, "Windows Terminal");
    assert_eq!(detect_icon_style(&env(&[("TERM_PROGRAM", "vscode")]), "win32"), IconStyle::Glyphs);
    assert_eq!(detect_icon_style(&env(&[("TERM_PROGRAM", "WezTerm")]), "win32"), IconStyle::Glyphs, "any terminal that names itself");
    assert_eq!(
        detect_icon_style(&env(&[("TERM_PROGRAM", "mintty"), ("TERM", "xterm")]), "win32"),
        IconStyle::Glyphs,
        "Git Bash's own window"
    );
    assert_eq!(detect_icon_style(&env(&[]), "win32"), IconStyle::Badges, "the old console");
    assert_eq!(detect_icon_style(&env(&[("TERM", "linux")]), "linux"), IconStyle::Badges);
    assert_eq!(detect_icon_style(&env(&[("KUMI_ICONS", "badges"), ("TERM_PROGRAM", "ghostty")]), "darwin"), IconStyle::Badges);
    assert_eq!(detect_icon_style(&env(&[("KUMI_ICONS", "glyphs")]), "win32"), IconStyle::Glyphs);
}

#[test]
fn a_devices_kind_comes_from_its_class_its_chains_and_pads_and_lives_device_type_when_the_bridge_sends_it() {
    let row = |class_name: &'static str, chains: Option<bool>, pads: Option<bool>, device_type: Option<&'static str>| DeviceRow {
        class_name: Some(class_name),
        can_have_chains: chains,
        can_have_drum_pads: pads,
        device_type,
    };
    assert_eq!(device_kind(&row("DrumGroupDevice", Some(true), Some(true), None)), IconKind::DrumRack);
    assert_eq!(device_kind(&row("InstrumentGroupDevice", Some(true), None, None)), IconKind::InstrumentRack);
    assert_eq!(device_kind(&row("AudioEffectGroupDevice", Some(true), None, None)), IconKind::AudioRack);
    assert_eq!(device_kind(&row("MidiEffectGroupDevice", Some(true), None, None)), IconKind::MidiRack);
    assert_eq!(device_kind(&row("MxDeviceMidiEffect", None, None, None)), IconKind::MaxMidi);
    assert_eq!(device_kind(&row("MxDeviceInstrument", None, None, None)), IconKind::MaxInstrument);
    assert_eq!(device_kind(&row("MxDeviceAudioEffect", None, None, None)), IconKind::MaxAudio);
    assert_eq!(device_kind(&row("PluginDevice", None, None, None)), IconKind::Plugin);
    assert_eq!(device_kind(&row("AuPluginDevice", None, None, None)), IconKind::Plugin);
    assert_eq!(device_kind(&row("Saturator", None, None, Some("audio_effect"))), IconKind::AudioEffect);
    assert_eq!(device_kind(&row("Operator", None, None, Some("instrument"))), IconKind::Instrument);
    assert_eq!(device_kind(&row("Arpeggiator", None, None, Some("midi_effect"))), IconKind::MidiEffect);
    assert_eq!(device_kind(&row("Saturator", None, None, None)), IconKind::Device, "without Live's type, a plain device");
    assert_eq!(
        device_kind(&DeviceRow::from_value(&serde_json::json!({ "className": "DrumGroupDevice", "canHaveDrumPads": true }))),
        IconKind::DrumRack
    );
    assert_eq!(track_kind(Some("group")), IconKind::GroupTrack);
    assert_eq!(track_kind(None), IconKind::AudioTrack);
}
