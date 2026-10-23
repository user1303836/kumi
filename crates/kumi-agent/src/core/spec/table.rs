//! The specs a delivery meets: each one's loudness (integrated, BS.1770, or RMS for ACX) with the range it allows and the level
//! to aim for, its true-peak ceiling, a noise floor (ACX), the longest file, and what else it asks, in words.

/// A spec.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub id: &'static str,
    pub name: &'static str,
    /// Integrated loudness (LUFS): the range allowed and the level to aim for.
    pub integrated: Option<(f64, f64, f64)>,
    /// RMS (dB): the range allowed and the level to aim for (ACX).
    pub rms: Option<(f64, f64, f64)>,
    /// The highest true peak allowed, dBTP.
    pub ceiling: f64,
    /// The noise floor's most (dB RMS, ACX).
    pub noise_floor: Option<f64>,
    /// The longest a file may be (seconds).
    pub longest: Option<f64>,
    /// What else it asks, which a measure can't tell.
    pub also: &'static str,
}

pub const SPECS: &[Spec] = &[
    Spec {
        id: "acx",
        name: "ACX (Audible audiobooks)",
        integrated: None,
        rms: Some((-23., -18., -20.)),
        ceiling: -3.,
        noise_floor: Some(-60.),
        longest: Some(120. * 60.),
        also: "44.1 kHz, 192 kbps CBR MP3, every file mono or every file stereo; 0.5–1 s of room tone at the head and 1–5 s at the tail; one chapter a file",
    },
    Spec {
        id: "spotify",
        name: "Spotify",
        integrated: Some((-15., -13., -14.)),
        rms: None,
        ceiling: -1.,
        noise_floor: None,
        longest: None,
        also: "Spotify turns louder masters down to -14 LUFS; one louder than -14 is safer at -2 dBTP (lossy encoding)",
    },
    Spec {
        id: "apple-music",
        name: "Apple Music",
        integrated: Some((-17., -15., -16.)),
        rms: None,
        ceiling: -1.,
        noise_floor: None,
        longest: None,
        also: "Sound Check plays masters at about -16 LUFS",
    },
    Spec {
        id: "youtube",
        name: "YouTube",
        integrated: Some((-15., -13., -14.)),
        rms: None,
        ceiling: -1.,
        noise_floor: None,
        longest: None,
        also: "YouTube turns louder uploads down to about -14 LUFS",
    },
    Spec {
        id: "amazon-music",
        name: "Amazon Music",
        integrated: Some((-15., -13., -14.)),
        rms: None,
        ceiling: -2.,
        noise_floor: None,
        longest: None,
        also: "",
    },
    Spec { id: "tidal", name: "Tidal", integrated: Some((-15., -13., -14.)), rms: None, ceiling: -1., noise_floor: None, longest: None, also: "" },
    Spec { id: "deezer", name: "Deezer", integrated: Some((-16., -14., -15.)), rms: None, ceiling: -1., noise_floor: None, longest: None, also: "" },
    Spec {
        id: "podcast",
        name: "Podcasts (Apple's guidance)",
        integrated: Some((-17., -15., -16.)),
        rms: None,
        ceiling: -1.,
        noise_floor: None,
        longest: None,
        also: "-16 LUFS for stereo; a mono show aims at -19 LUFS",
    },
    Spec {
        id: "ebu-r128",
        name: "EBU R 128 (broadcast)",
        integrated: Some((-23.5, -22.5, -23.)),
        rms: None,
        ceiling: -1.,
        noise_floor: None,
        longest: None,
        also: "-23 LUFS ±0.5 LU for a finished programme (±1 LU live); the loudness range as the programme needs",
    },
    Spec {
        id: "atsc-a85",
        name: "ATSC A/85 (US broadcast)",
        integrated: Some((-26., -22., -24.)),
        rms: None,
        ceiling: -2.,
        noise_floor: None,
        longest: None,
        also: "-24 LKFS ±2 dB",
    },
    Spec {
        id: "netflix",
        name: "Netflix",
        integrated: Some((-29., -25., -27.)),
        rms: None,
        ceiling: -2.,
        noise_floor: None,
        longest: None,
        also: "Netflix measures dialogue-gated loudness (-27 LKFS ±2): Kumi measures the whole programme, which a mix with loud music or effects reads louder than its dialogue",
    },
];

/// The spec of this id or name ("ACX", "spotify", "EBU R128").
pub fn find(asked: &str) -> Option<&'static Spec> {
    let key = |text: &str| text.to_lowercase().chars().filter(char::is_ascii_alphanumeric).collect::<String>();
    let asked = key(asked);
    SPECS.iter().find(|spec| key(spec.id) == asked || key(spec.name).starts_with(&asked))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_is_found_by_its_id_or_its_name_and_each_aims_inside_its_range() {
        assert_eq!(find("ACX").unwrap().id, "acx");
        assert_eq!(find("EBU R128").unwrap().id, "ebu-r128");
        assert_eq!(find("apple music").unwrap().id, "apple-music");
        assert!(find("vinyl").is_none());
        for spec in SPECS {
            for (low, high, aim) in [spec.integrated, spec.rms].into_iter().flatten() {
                assert!(low < aim && aim < high, "{}", spec.id);
            }
            assert!(spec.integrated.is_some() != spec.rms.is_some(), "{}: one loudness", spec.id);
        }
    }
}
