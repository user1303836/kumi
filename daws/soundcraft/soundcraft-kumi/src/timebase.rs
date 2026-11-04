//! `timebase`: SoundCraft's positions, which are samples, as quarter notes and as bars and beats, through its tempo and
//! meter maps (`session.inspect`'s `tempo` and `meter`, placed in ticks at 960 a quarter note).
//!
//! SoundCraft 0.17.0's maps change in steps, and that's all this reads: a map with anything else in it (a ramp, from a
//! later release) has no timebase, rather than a wrong one.
use serde_json::{Map, Value};

/// Ticks a quarter note.
pub const PPQ: f64 = 960.0;

/// A tempo from `tick` on, which starts at `sample`.
struct Tempo {
    tick: f64,
    bpm: f64,
    sample: f64,
}

/// A meter from `tick` (a bar line) on, which starts bar `bar` (from 1).
struct Meter {
    tick: f64,
    numerator: u32,
    denominator: u32,
    bar: i64,
}

impl Meter {
    fn ticks_a_beat(&self) -> f64 {
        PPQ * 4.0 / f64::from(self.denominator)
    }

    fn ticks_a_bar(&self) -> f64 {
        self.ticks_a_beat() * f64::from(self.numerator)
    }
}

/// A position as SoundCraft writes it in bars and beats ("10| 2| 517"): its bar and its beat in the bar (both from 1,
/// in the meter's beats), and the ticks past that beat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarBeat {
    pub bar: i64,
    pub beat: i64,
    pub ticks: i64,
}

/// A session's tempo and meter maps.
pub struct Timebase {
    rate: f64,
    tempos: Vec<Tempo>,
    meters: Vec<Meter>,
}

impl Timebase {
    /// The session's (`session.inspect`), when its maps change in steps only and each starts at tick 0. A meter SoundCraft
    /// sets is on a bar line; one a file brings off a bar line starts the next bar, the part-bar before it counted whole,
    /// as SoundCraft counts it.
    pub fn of(session: &Value) -> Option<Timebase> {
        let rate = session["sample_rate"].as_f64().filter(|rate| *rate > 0.0)?;
        let mut tempos: Vec<Tempo> = vec![];
        for tempo in steps(session, "tempo", &["tick", "bpm"])? {
            let (tick, bpm) = (tempo.get("tick")?.as_f64()?, tempo.get("bpm")?.as_f64().filter(|bpm| *bpm > 0.0)?);
            let sample = match tempos.last() {
                None => (tick == 0.0).then_some(0.0)?,
                Some(before) => {
                    (tick > before.tick).then_some(())?;
                    before.sample + (tick - before.tick) / PPQ * 60.0 / before.bpm * rate
                }
            };
            tempos.push(Tempo { tick, bpm, sample });
        }
        let mut meters: Vec<Meter> = vec![];
        for meter in steps(session, "meter", &["tick", "numerator", "denominator"])? {
            let tick = meter.get("tick")?.as_f64()?;
            let part = |key: &str| meter.get(key)?.as_u64().filter(|part| *part > 0).and_then(|part| u32::try_from(part).ok());
            let (numerator, denominator) = (part("numerator")?, part("denominator")?);
            let bar = match meters.last() {
                None => (tick == 0.0).then_some(1)?,
                Some(before) => {
                    let bars = (tick - before.tick) / before.ticks_a_bar();
                    (bars > 0.0).then_some(())?;
                    before.bar + bars.ceil() as i64
                }
            };
            meters.push(Meter { tick, numerator, denominator, bar });
        }
        (!tempos.is_empty() && !meters.is_empty()).then_some(Timebase { rate, tempos, meters })
    }

    /// Quarter notes from the session's start to `samples`.
    pub fn beats(&self, samples: f64) -> f64 {
        let tempo = self.tempos.iter().rev().find(|tempo| tempo.sample <= samples).unwrap_or(&self.tempos[0]);
        tempo.tick / PPQ + (samples - tempo.sample) / self.rate * tempo.bpm / 60.0
    }

    /// The bar, beat and ticks at `beats` quarter notes, counted as SoundCraft counts them (the tick rounded).
    pub fn bar_beat(&self, beats: f64) -> BarBeat {
        let tick = (beats * PPQ).round();
        let meter = self.meter(tick);
        let into = tick - meter.tick;
        let bars = (into / meter.ticks_a_bar()).floor();
        let into_bar = into - bars * meter.ticks_a_bar();
        let beat = (into_bar / meter.ticks_a_beat()).floor();
        BarBeat { bar: meter.bar + bars as i64, beat: beat as i64 + 1, ticks: (into_bar - beat * meter.ticks_a_beat()) as i64 }
    }

    /// The meter at `beats`: its numerator and denominator.
    pub fn time_signature(&self, beats: f64) -> (u32, u32) {
        let meter = self.meter((beats * PPQ).round());
        (meter.numerator, meter.denominator)
    }

    /// Quarter notes a bar at `beats`.
    pub fn beats_a_bar(&self, beats: f64) -> f64 {
        self.meter((beats * PPQ).round()).ticks_a_bar() / PPQ
    }

    /// The tempo at `beats` (the tick rounded, as the other lookups have it: a playhead on a change has the new tempo).
    pub fn tempo(&self, beats: f64) -> f64 {
        let tick = (beats * PPQ).round();
        self.tempos.iter().rev().find(|tempo| tempo.tick <= tick).unwrap_or(&self.tempos[0]).bpm
    }

    fn meter(&self, tick: f64) -> &Meter {
        self.meters.iter().rev().find(|meter| meter.tick <= tick).unwrap_or(&self.meters[0])
    }
}

/// A map's entries, if each has only the fields a step has.
fn steps<'a>(session: &'a Value, key: &str, fields: &[&str]) -> Option<Vec<&'a Map<String, Value>>> {
    session
        .get(key)?
        .as_array()?
        .iter()
        .map(|entry| entry.as_object().filter(|entry| entry.keys().all(|key| fields.contains(&key.as_str()))))
        .collect()
}
