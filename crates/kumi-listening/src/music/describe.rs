//! What songs share, in words: a profile as one short sentence the model can lean on when the producer left the
//! style open (how loud, how bright, how wide their lows are, how squashed, how much contrast, room and echo). Fixed
//! thresholds, so one profile always reads the same.

use crate::checklist::{number, Profile, Spread};

/// Integrated loudness (LUFS) at or above which songs read as very loud, loud and moderate; below the last, quiet.
const LOUDNESS: [(f64, &str); 3] = [(-8., "very loud"), (-11., "loud"), (-15., "moderate")];
/// Tilt (dB an octave against pink, 100 Hz to 10 kHz) at or above which they read as very bright, bright and balanced,
/// then at or above which warm; below, dark.
const BRIGHTNESS: [(f64, &str); 4] = [(1.5, "very bright"), (0.5, "bright"), (-1., "balanced"), (-2.5, "warm")];
/// Side below 120 Hz against mid (dB) at or under which the lows read as mono, then as mostly mono; above, wide.
const MONO_LOWS: f64 = -25.;
const MOSTLY_MONO_LOWS: f64 = -12.;
/// Peak to loudness (dB) under which the dynamics read as squashed, firm and open; at or above the last, very open.
const DYNAMICS: [(f64, &str); 3] = [(8.5, "squashed"), (11., "firm"), (14., "open")];
/// Loudness range (LU) at or above which sections contrast a lot, and at or under which they hold one level.
const CONTRAST: f64 = 9.;
const STEADY: f64 = 3.;
/// Reverb decay (s) at or above which the room reads as long, then as some; below, dry.
const LONG_REVERB: f64 = 1.8;
const SOME_REVERB: f64 = 0.8;
/// An echo's fall a repeat (dB) under which its echoes are heard (60 is none).
const ECHOES: f64 = 30.;
/// Onsets a bar under which the music reads as sparse, and at or above which as busy; between, steady.
const SPARSE: f64 = 4.;
const BUSY: f64 = 10.;
/// A swing ratio at or above which it reads as swung; under, straight.
const SWUNG: f64 = 1.25;
/// How far ahead of or behind the beat (ms) it sits, at least, to read as pushed or laid back.
const PUSHED: f64 = 8.;

fn spread(value: &Spread, unit: &str) -> String {
    format!("{} {unit}, {} to {}", number(value.mid), number(value.low), number(value.high))
}

fn word(value: f64, words: &[(f64, &'static str)], below: &'static str) -> &'static str {
    words.iter().find(|(from, _)| value >= *from).map_or(below, |(_, word)| word)
}

/// A profile in one sentence: `Around 128 BPM in A minor, busy (12 onsets a bar), swung (1.5), loud (about -9 LUFS,
/// -11 to -7), warm (tilt -1.4 dB/oct), mono lows, ...`.
pub fn describe(profile: &Profile) -> String {
    let mut parts = vec![];
    match (&profile.tempo, &profile.key) {
        (Some(tempo), Some(key)) => parts.push(format!("around {} BPM in {key}", number(tempo.mid.round()))),
        (Some(tempo), None) => parts.push(format!("around {} BPM", number(tempo.mid.round()))),
        (None, Some(key)) => parts.push(format!("in {key}")),
        (None, None) => {}
    }
    if let Some(busy) = &profile.busyness {
        let word = if busy.mid < SPARSE {
            "sparse"
        } else if busy.mid >= BUSY {
            "busy"
        } else {
            "steady"
        };
        parts.push(format!("{word} ({} onsets a bar)", number(busy.mid.round())));
    }
    if let Some(groove) = &profile.groove {
        parts.push(match groove.swing_ratio.mid >= SWUNG {
            true => format!("swung ({})", number(groove.swing_ratio.mid)),
            false => "straight".into(),
        });
        if groove.push_ms.mid.abs() >= PUSHED {
            parts.push(match groove.push_ms.mid < 0. {
                true => format!("ahead of the beat ({} ms)", number(groove.push_ms.mid)),
                false => format!("laid back (+{} ms)", number(groove.push_ms.mid)),
            });
        }
    }
    if let Some(loudness) = &profile.integrated {
        parts.push(format!("{} (about {})", word(loudness.mid, &LOUDNESS, "quiet"), spread(loudness, "LUFS")));
    }
    parts.push(format!("{} (tilt {} dB/oct)", word(profile.tilt.mid, &BRIGHTNESS, "dark"), number(profile.tilt.mid)));
    if let Some(lows) = &profile.low_width {
        parts.push(
            if lows.mid <= MONO_LOWS {
                "mono lows"
            } else if lows.mid <= MOSTLY_MONO_LOWS {
                "mostly mono lows"
            } else {
                "wide lows"
            }
            .into(),
        );
    }
    if let Some(plr) = &profile.plr {
        let dynamics = DYNAMICS.iter().find(|(under, _)| plr.mid < *under).map_or("very open", |(_, word)| word);
        parts.push(format!("{dynamics} dynamics (peak to loudness {} dB)", number(plr.mid)));
    }
    if let Some(range) = profile.range.filter(|range| range.mid >= CONTRAST || range.mid <= STEADY) {
        parts.push(match range.mid >= CONTRAST {
            true => format!("big contrast between sections ({} LU)", number(range.mid)),
            false => format!("one level throughout ({} LU)", number(range.mid)),
        });
    }
    if let Some(decay) = &profile.sound.decay_time {
        parts.push(if decay.mid >= LONG_REVERB {
            format!("long reverb ({} s)", number(decay.mid))
        } else if decay.mid >= SOME_REVERB {
            format!("some reverb ({} s)", number(decay.mid))
        } else {
            "dry".into()
        });
    }
    if let (Some(time), Some(falls)) = (&profile.sound.echo_time, &profile.sound.echo_falls) {
        if falls.mid < ECHOES {
            parts.push(format!("echoes at {} ms", number(time.mid)));
        }
    }
    let sentence = parts.join(", ");
    let mut chars = sentence.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}
