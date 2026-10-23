//! M3's carried REAPER items (r-tracks): the tempo-marker wording, NIT-4 (an undo jump deeper than a look reads), NIT-7 (no
//! sleep on REAPER's main thread for a plug-in's meters) and N10 (routes made again in their place). NIT-5 is the host's
//! (`daw-mcp-server` changes.rs).

use std::time::{Duration, Instant};

use serde_json::json;

use super::tests_lab::*;
use super::volatile::{RELOOKS, RELOOK_EVERY};
use crate::reaper::fake::{self, FakeParam, FakeTempo};

// ----- the tempo-marker wording -----

fn tempo_markers() -> Vec<(f64, f64)> {
    state(|s| s.tempo_markers.iter().map(|m| (m.time, m.bpm)).collect())
}

#[test]
fn undoing_a_tempo_marker_delete_never_writes_over_another_marker_at_its_time_and_says_so() {
    let lab = Lab::new();
    state(|s| {
        s.tempo_markers = vec![
            FakeTempo { time: 0.0, bpm: 120.0, num: 4, denom: 4, linear: false },
            FakeTempo { time: 8.0, bpm: 90.0, num: -1, denom: -1, linear: false },
        ]
    });
    let (preview, applied) = lab.make("tempo-marker-delete", json!({"position": 8.0}), "Kumi: Deleted tempo marker (kt001)");
    assert_eq!(tempo_markers(), [(0.0, 120.0)]);
    // The producer puts a marker of their own at that time.
    state(|s| s.tempo_markers.push(FakeTempo { time: 8.0, bpm: 140.0, num: -1, denom: -1, linear: false }));
    let entries_before = entries();
    for anyway in [false, true] {
        let answer = if anyway {
            lab.undo_anyway("tempo-marker-delete", &preview, &applied, "Kumi: undo (kt002)")
        } else {
            lab.undo("tempo-marker-delete", &preview, &applied, "Kumi: undo (kt002)")
        };
        let (code, message) = answer.unwrap_err();
        assert_eq!(code, "changed", "anyway: {anyway}");
        assert!(message.contains("a different tempo marker is at") && message.contains("nothing was written"), "{message}");
        assert_eq!(tempo_markers(), [(0.0, 120.0), (8.0, 140.0)], "the producer's marker stays");
        assert_eq!(entries(), entries_before, "no point made");
    }
    // The one Kumi deleted, back (a retry after the first answer was lost): said, nothing written.
    state(|s| s.tempo_markers[1].bpm = 90.0);
    let again = lab.undo("tempo-marker-delete", &preview, &applied, "Kumi: undo (kt003)").unwrap();
    assert_eq!(again["display"]["note"], "It was back already: nothing was written.");
}

#[test]
fn undoing_a_tempo_marker_delete_leaves_it_out_once_the_tempo_map_changed_unless_told_to_put_it_back_anyway() {
    let lab = Lab::new();
    state(|s| {
        s.tempo_markers = vec![
            FakeTempo { time: 0.0, bpm: 120.0, num: 4, denom: 4, linear: false },
            FakeTempo { time: 16.0, bpm: 90.0, num: -1, denom: -1, linear: false },
        ]
    });
    let (preview, applied) = lab.make("tempo-marker-delete", json!({"position": 16.0}), "Kumi: Deleted tempo marker (kt011)");
    // An earlier tempo change: at 16 s the marker would sit at another bar.
    state(|s| s.tempo_markers.push(FakeTempo { time: 4.0, bpm: 140.0, num: -1, denom: -1, linear: false }));
    let (code, message) = lab.undo("tempo-marker-delete", &preview, &applied, "Kumi: undo (kt012)").unwrap_err();
    assert_eq!(code, "changed");
    assert!(message.contains("tempo map"), "{message}");
    assert_eq!(tempo_markers().len(), 2, "nothing made again");
    lab.undo_anyway("tempo-marker-delete", &preview, &applied, "Kumi: undo (kt013)").unwrap();
    assert_eq!(tempo_markers().len(), 3);
}

// ----- NIT-4: an undo jump deeper than a look reads -----

/// The producer's own edits, each a point, with the watch looking every few of them as REAPER's ticks would.
fn producer_edits(lab: &Lab, n: usize) {
    for k in 0..n {
        fake::producer_edit(&format!("Adjust track pan {k}"));
        if k % 40 == 0 {
            lab.tick();
        }
    }
    lab.tick();
}

#[test]
fn an_undo_jump_past_what_a_look_reads_is_told_and_so_is_a_redo_jump_back_over_it() {
    let lab = Lab::new();
    // Points of the producer's before Kumi's change, for the producer to jump back over too.
    producer_edits(&lab, 400);
    let desc = "Kumi: Kick volume −6 dB (kd3db)";
    lab.make("track-volume", json!({"ref": KICK, "volumeDb": -6.0}), desc);
    producer_edits(&lab, 300);
    assert_eq!(lab.events.told("undo"), Vec::<String>::new());
    // REAPER's Undo History window: 301 points back at once, Kumi's change among them, 300 points down.
    for _ in 0..301 {
        fake::producer_undo();
    }
    lab.tick();
    assert_eq!(lab.events.told("undo"), [desc]);
    // 300 more points back: Kumi's is far down the redo side, and nothing is told.
    for _ in 0..300 {
        fake::producer_undo();
    }
    lab.tick();
    assert!(lab.events.take().is_empty());
    // Then 600 forward at once, past it: applied again, 299 points down.
    for _ in 0..600 {
        fake::producer_redo();
    }
    lab.tick();
    assert_eq!(lab.events.told("redo"), [desc]);
}

// ----- NIT-7: no sleep on REAPER's main thread for a plug-in's meters -----

fn kick_fx_guids() -> Vec<String> {
    state(|s| s.tracks[1].fx.iter().map(|f| f.guid.clone()).collect())
}

#[test]
fn adding_a_plug_in_holds_reapers_main_thread_for_no_sleep() {
    let lab = Lab::new();
    state(|s| s.meter_params = true);
    let mut fastest = Duration::MAX;
    for n in 0..5 {
        let input = json!({"ref": KICK, "name": "ReaEQ"});
        let preview = lab.preview("fx-add", input.clone()).unwrap();
        let started = Instant::now();
        let applied = lab.apply("fx-add", &input, &preview, &format!("Kumi: Added ReaEQ (kq00{n})")).unwrap();
        fastest = fastest.min(started.elapsed());
        // The meters are still found: one by its name, one by its moving.
        assert_eq!(applied["applied"]["signature"]["volatile"], json!([4, 5]));
    }
    assert!(fastest < Duration::from_millis(10), "the fastest of five applies took {fastest:?}");
}

#[test]
fn a_plug_ins_parameter_names_are_read_once_not_on_every_change() {
    let lab = Lab::new();
    // Two plug-ins of one kind with 2,000 parameters each: what a look at one reads, names and values.
    let wide = |guid: &str| {
        let mut fx = fake::FakeFx::new(guid, "VST3: Wide (Maker)");
        fx.params = (0..2000).map(|param| FakeParam::new(&format!("Band {param}"), 0.5, "0")).collect();
        fx
    };
    state(|s| s.tracks[1].fx = vec![wide("{FX-WIDE-1}"), wide("{FX-WIDE-2}")]);
    let look = |guid: &str| {
        let before = state(|s| s.calls);
        lab.preview("fx-delete", json!({"ref": format!("r:fx:{guid}")})).unwrap();
        state(|s| s.calls) - before
    };
    let first = look("{FX-WIDE-1}");
    let second = look("{FX-WIDE-2}");
    assert!(second + 1500 < first, "the second plug-in's names were read again: {first} calls, then {second}");
}

/// The ticks through one whole watch of what Kumi looked at last.
fn watch_through(lab: &Lab, each: impl Fn(u8)) {
    for n in 0..RELOOKS {
        std::thread::sleep(RELOOK_EVERY + Duration::from_millis(2));
        each(n);
        lab.tick();
    }
}

#[test]
fn a_tick_looks_again_at_one_fx_at_a_time() {
    // Every FX due at once (up to 8 of 4,096 parameters each, every 100 ms) held REAPER's main thread (review R16).
    let lab = Lab::new();
    for n in 0..3 {
        lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}), &format!("Kumi: Added ReaEQ (ko00{n})"));
    }
    let before = super::volatile::looks_left();
    assert_eq!(before, [RELOOKS; 3]);
    std::thread::sleep(RELOOK_EVERY + Duration::from_millis(2));
    lab.tick();
    let after = super::volatile::looks_left();
    assert_eq!(after.iter().filter(|left| **left == RELOOKS - 1).count(), 1, "one FX looked at: {after:?}");
    lab.tick();
    lab.tick();
    assert_eq!(super::volatile::looks_left(), [RELOOKS - 1; 3], "the others in the next ticks");
}

#[test]
fn a_meter_found_moving_after_kumi_added_its_plug_in_does_not_refuse_the_undo_but_a_tweak_does() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}), "Kumi: Added ReaEQ (km001)");
    assert_eq!(applied["applied"]["signature"]["volatile"], json!([]));
    // A parameter that moves by itself once the plug-in plays, right after Kumi added it: the watch sees it move to its end.
    state(|s| s.tracks[1].fx[0].params[3].wobbles = true);
    watch_through(&lab, |_| {});
    // A real tweak is still seen.
    state(|s| s.tracks[1].fx[0].params[0].value = 0.9);
    let (code, _) = lab.undo("fx-add", &preview, &applied, "Kumi: undo (km002)").unwrap_err();
    assert_eq!(code, "changed");
    state(|s| s.tracks[1].fx[0].params[0].value = 0.5);
    lab.undo("fx-add", &preview, &applied, "Kumi: undo (km003)").unwrap();
    assert!(kick_fx_guids().is_empty(), "the moving parameter didn't refuse it");
}

#[test]
fn a_tweak_the_ticks_see_once_is_never_taken_for_a_meter() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}), "Kumi: Added ReaEQ (kt101)");
    // The producer turns a knob of the new plug-in at once, then leaves it.
    std::thread::sleep(Duration::from_millis(110));
    state(|s| s.tracks[1].fx[0].params[0].value = 0.9);
    for _ in 0..3 {
        lab.tick();
        std::thread::sleep(Duration::from_millis(110));
    }
    let (code, _) = lab.undo("fx-add", &preview, &applied, "Kumi: undo (kt102)").unwrap_err();
    assert_eq!(code, "changed", "the producer's tweak keeps the plug-in");
    assert_eq!(kick_fx_guids().len(), 1);
}

#[test]
fn a_knob_turned_through_most_of_the_watch_and_left_is_never_taken_for_a_meter() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}), "Kumi: Added ReaEQ (kd001)");
    // The producer drags a knob of the new plug-in for most of a second, then lets go.
    watch_through(&lab, |n| {
        if n + 2 < RELOOKS {
            state(|s| s.tracks[1].fx[0].params[0].value = 0.1 + 0.05 * f64::from(n));
        }
    });
    let (code, _) = lab.undo("fx-add", &preview, &applied, "Kumi: undo (kd002)").unwrap_err();
    assert_eq!(code, "changed", "the producer's knob keeps the plug-in");
    assert_eq!(kick_fx_guids().len(), 1);
}

#[test]
fn the_parameter_the_producer_touched_last_is_never_taken_for_a_meter() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}), "Kumi: Added ReaEQ (kh001)");
    // A drag longer than the watch: the knob moves to its end, and REAPER says it is the parameter touched last.
    state(|s| s.touched_fx = Some((1, -1, -1, 0, 0)));
    watch_through(&lab, |n| state(|s| s.tracks[1].fx[0].params[0].value = 0.1 + 0.05 * f64::from(n)));
    let (code, _) = lab.undo("fx-add", &preview, &applied, "Kumi: undo (kh002)").unwrap_err();
    assert_eq!(code, "changed", "the touched knob keeps the plug-in");
    assert_eq!(kick_fx_guids().len(), 1);
}

#[test]
fn a_parameter_that_starts_moving_after_the_watch_is_a_change_not_a_meter() {
    let lab = Lab::new();
    let (preview, applied) = lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}), "Kumi: Added ReaEQ (kl001)");
    watch_through(&lab, |_| {});
    // Later the producer sets up an LFO on a parameter: it moves by itself now, and that is their work.
    state(|s| s.tracks[1].fx[0].params[2].wobbles = true);
    let (code, _) = lab.undo("fx-add", &preview, &applied, "Kumi: undo (kl002)").unwrap_err();
    assert_eq!(code, "changed");
    assert_eq!(kick_fx_guids().len(), 1);
}
