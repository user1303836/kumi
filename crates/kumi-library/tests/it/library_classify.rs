//! Naming and XML cases.
use kumi_runtime::library::{classify::*, xml::*};
#[test]
fn names_say_class_kind_tempo_key_and_note() {
    assert_eq!(tokens("KickPunchy_01 F#m 128bpm"), vec!["kick", "punchy", "01", "f#m", "128", "bpm"]);
    let kick = name_hints("Drums/Kicks/Kick 808 Long.wav");
    assert_eq!(kick.class, Some(SoundClass::Kick));
    assert_eq!(kick.class_from, Some(ClassFrom::Name));
    let bass = name_hints("Loops/Bass Loop 128 Fmin.wav");
    assert_eq!((bass.class, bass.kind, bass.key.as_deref()), (Some(SoundClass::Bass), Some(SoundKind::Loop), Some("F minor")));
    assert_eq!(bass.tempos, vec![Tempo { bpm: 128.0, explicit: false }]);
    for file in ["Packs/Hats/HH_01.wav", "Hi Hats/Open Hi Hat 3.wav", "Loops/Hat Loop 128.wav"] {
        assert_eq!(name_hints(file).class, Some(SoundClass::Hat), "{file}");
    }
    let folder = name_hints("Splice/Snares/rimmy thing.wav");
    assert_eq!((folder.class, folder.class_from), (Some(SoundClass::Snare), Some(ClassFrom::Folder)));
    assert_eq!(name_hints("Drums/Rim/Wood Block Combo.wav").class, Some(SoundClass::Perc));
    assert_eq!(name_hints("Vocals/Vox Chop C#m 120bpm.wav").key.as_deref(), Some("C# minor"));
    for file in ["Loops/Drum Loop Kick Snare 90.wav", "Splice/SO_DT_120_drum_loop_kick_heavy.wav", "Loops/Top Loop 03 124bpm.wav"] {
        assert_eq!(name_hints(file).class, Some(SoundClass::Drums), "{file}");
    }
    assert_eq!(name_hints("FX/FX Guitar Chop C.aif").class, Some(SoundClass::Guitar));
    for (text, key) in [
        ("Am", Some("A minor")),
        ("I am here", None),
        ("Bbmaj7 stab", Some("A# major")),
        ("Pad F# minor", Some("F# minor")),
        ("Strings Dmin6", Some("D minor")),
    ] {
        assert_eq!(parse_key(text).as_deref(), key, "{text}");
    }
    assert_eq!(parse_note("Harpsichord Pluck C2"), Some(Note { name: "C2".into(), midi: 36 }));
    assert_eq!(parse_note("E-Perc Low"), None);
    assert_eq!(parse_tempo("Break 90 bpm"), vec![Tempo { bpm: 90.0, explicit: true }]);
    assert_eq!(tempo_from_length(2.0, None, 0.0), Some(120.0));
    assert_eq!(tempo_from_length(4.8, Some(100.0), 0.8), Some(100.0));
}
#[test]
fn live_tags_preserve_quoted_greater_than_and_decode_entities() {
    struct Seen(Vec<String>);
    impl TagHandler for Seen {
        fn open(&mut self, name: &str, attrs: &str, _empty: bool) {
            self.0.push(format!("{name}:{}", attribute(attrs, "Value").or_else(|| attribute(attrs, "Name")).unwrap_or_default()));
        }
        fn close(&mut self, name: &str) {
            self.0.push(format!("/{name}"));
        }
    }
    let mut seen = Seen(vec![]);
    let text = "<?xml version=\"1.0\"?><!-- note --><A><B Value='1 > 0' /><C UserName=\"x\" Name=\"R&amp;B &#x263A;\"></C><D";
    let end = scan_tags(text, &mut seen);
    assert_eq!(seen.0, vec!["A:", "B:1 > 0", "/B", "C:R&B ☺", "/C"]);
    assert_eq!(&text[end..], "<D");
}
