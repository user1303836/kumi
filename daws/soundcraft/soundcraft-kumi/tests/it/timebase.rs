//! Samples as quarter notes and as bars and beats, held to what SoundCraft 0.17.0 writes for the same positions.
use serde_json::{json, Value};
use soundcraft_kumi::timebase::{BarBeat, Timebase};

/// The demo session at 48 kHz, 104 bpm in 4/4, after `event.tempo {bpm: 140, at: 443077}` (it landed on tick 15360, bar
/// 5) and `event.meter {3/4, at: 886154}` (SoundCraft moved it to the bar line, tick 34560, bar 10).
fn changed() -> Value {
    json!({
        "sample_rate": 48000,
        "tempo": [{"bpm": 104.0, "tick": 0}, {"bpm": 140.0, "tick": 15360}],
        "meter": [{"denominator": 4, "numerator": 4, "tick": 0}, {"denominator": 4, "numerator": 3, "tick": 34560}],
    })
}

#[test]
fn a_position_reads_as_soundcraft_writes_it_in_bars_and_beats() {
    let timebase = Timebase::of(&changed()).unwrap();
    // (samples, and what `session.format_time {at, format: "bars_beats"}` answered for them)
    for (samples, written) in [
        (0, "1| 1| 000"),
        (27692, "1| 2| 000"),
        (443077, "5| 1| 000"),
        (500000, "5| 3| 736"),
        (886154, "10| 2| 517"),
        (1000000, "12| 2| 070"),
    ] {
        let BarBeat { bar, beat, ticks } = timebase.bar_beat(timebase.beats(f64::from(samples)));
        assert_eq!(format!("{bar}| {beat}| {ticks:03}"), written, "{samples} samples");
    }
}

#[test]
fn the_meter_and_tempo_are_the_ones_in_force_at_a_beat() {
    let timebase = Timebase::of(&changed()).unwrap();
    assert_eq!((timebase.time_signature(0.0), timebase.beats_a_bar(0.0)), ((4, 4), 4.0));
    assert_eq!((timebase.time_signature(37.5), timebase.beats_a_bar(37.5)), ((3, 4), 3.0), "past bar 10");
    assert_eq!((timebase.tempo(10.0), timebase.tempo(20.0)), (104.0, 140.0));
    assert!((timebase.beats(27692.0) - 1.0).abs() < 1e-4, "a beat at 104 bpm is 27692 samples at 48 kHz");
    let in_six_eight = Timebase::of(&json!({
        "sample_rate": 48000, "tempo": [{"bpm": 120.0, "tick": 0}], "meter": [{"denominator": 8, "numerator": 6, "tick": 0}],
    }))
    .unwrap();
    assert_eq!(
        in_six_eight.bar_beat(3.5),
        BarBeat { bar: 2, beat: 2, ticks: 0 },
        "a bar of 6/8 is three quarter notes, counted in eighths"
    );
}

#[test]
fn a_map_that_isnt_steps_from_the_start_has_no_timebase() {
    let with = |change: fn(&mut Value)| {
        let mut session = changed();
        change(&mut session);
        Timebase::of(&session).is_some()
    };
    assert!(with(|_| {}));
    assert!(!with(|s| s["tempo"][1]["ramp"] = json!(true)), "a ramp, from a later release");
    assert!(!with(|s| s["tempo"][0]["tick"] = json!(960)), "no tempo at the start");
    assert!(!with(|s| s["meter"][1]["tick"] = json!(35000)), "a meter off a bar line");
    assert!(!with(|s| s["tempo"][1]["tick"] = json!(0)), "two tempos at one tick");
    assert!(!with(|s| s["sample_rate"] = Value::Null));
    assert!(!with(|s| s["meter"] = json!([])));
}
