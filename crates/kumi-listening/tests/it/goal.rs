//! What every DAW's goal rig does alike with a trial's render (`goal`).
use kumi_listening::goal::{heard_seconds, screen_pick, trial_key, HeardCache, ScreenPick};
use kumi_types::contracts::{AuditionRequest, Render};

#[test]
fn a_render_is_heard_as_long_as_it_says_else_for_its_part_at_the_tempo() {
    let render = |seconds| Render { file: "take.wav".into(), start: 0.5, seconds };
    assert_eq!(heard_seconds(&render(None), 8., 120.), 4.);
    assert_eq!(heard_seconds(&render(Some(1.5)), 8., 120.), 1.5, "Live stopped short");
}

/// At 120 BPM three seconds are 6 beats: half-second slices, 6 of them across.
fn slices(count: usize, loud: std::ops::Range<usize>) -> Vec<Option<f64>> {
    (0..count)
        .map(|at| {
            Some(if loud.contains(&at) {
                if at % 2 == 0 {
                    -10.
                } else {
                    -14.
                }
            } else {
                -30.
            })
        })
        .collect()
}

#[test]
fn a_part_under_six_seconds_isnt_screened() {
    assert_eq!(screen_pick(&slices(24, 0..0), 120., 11.9, 0.5), None);
    assert!(screen_pick(&slices(24, 0..0), 120., 12., 0.5).is_some(), "six seconds is long enough");
}

#[test]
fn a_long_part_is_screened_on_the_three_seconds_that_are_loudest_and_move_most() {
    let pick = screen_pick(&slices(24, 12..18), 120., 24., 0.5).unwrap();
    assert_eq!(pick, ScreenPick { offset: 11., beats: 6. }, "from slice 11, as the step up into the loud part counts as moving too");
}

#[test]
fn a_slice_nothing_was_measured_in_counts_as_silence() {
    let mut lufs = slices(24, 0..0);
    lufs[..6].fill(None);
    let pick = screen_pick(&lufs, 120., 24., 0.5).unwrap();
    assert_eq!(pick.offset, 5., "the last stretch to hold a step up from silence");
}

#[test]
fn a_screen_stays_inside_the_part() {
    // The loud stretch is 15 seconds into a reference longer than the 8 second part.
    let pick = screen_pick(&slices(40, 30..36), 120., 16., 0.5).unwrap();
    assert_eq!(pick, ScreenPick { offset: 10., beats: 6. }, "as late as a stretch fits: 16 beats less 6");
}

#[test]
fn a_reference_shorter_than_the_stretch_is_screened_from_its_start() {
    assert_eq!(screen_pick(&slices(3, 1..3), 120., 24., 0.5), Some(ScreenPick { offset: 0., beats: 6. }));
}

#[test]
fn a_screen_reads_the_reference_over_its_stretch_only() {
    let pick = ScreenPick { offset: 12., beats: 6. };
    let asked = AuditionRequest {
        candidates: vec![],
        from_beat: None,
        beats: None,
        reference: None,
        reference_from: Some(1.),
        reference_seconds: None,
        focus: None,
    };
    let request = pick.request(&asked, 120.);
    assert_eq!((request.reference_from, request.reference_seconds), (Some(7.), Some(3.)));
}

#[test]
fn a_trial_is_known_by_its_slot_whether_its_screened_and_its_values_to_four_places() {
    assert_eq!(trial_key("Bass", true, &[0.5, 0.25]), "Bass|screen|0.5000,0.2500");
    assert_eq!(trial_key("Bass", false, &[0.5]), "Bass|full|0.5000");
    assert_eq!(trial_key("Bass", false, &[0.12341]), trial_key("Bass", false, &[0.12344]));
    assert_ne!(trial_key("Bass", false, &[0.1234]), trial_key("Bass", false, &[0.1235]));
}

#[test]
fn what_was_heard_is_kept_for_the_last_two_thousand_trials() {
    let mut heard = HeardCache::default();
    for at in 0..2000 {
        heard.insert(format!("trial {at}"), at);
    }
    assert_eq!(heard.get("trial 0"), Some(0), "two thousand are kept");
    heard.insert("trial 2000".into(), 2000);
    assert!(!heard.contains("trial 0"), "the oldest goes");
    assert_eq!(heard.get("trial 1"), Some(1));
    assert_eq!(heard.get("trial 2000"), Some(2000));
}
