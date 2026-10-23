//! The Logic eval's dry run (`examples/eval_logic.rs -- --dry`), as CI runs it: every case's session over the simulated
//! Logic, through Kumi's host, Logic's integration and Apple's guide from the eval's fixture, on a canned answer per
//! case, and each case's check passing on what came of it; and each kind of check failing an answer wrong in its way.
//! No model, no sign-in and no network.
#[allow(dead_code)]
#[path = "../examples/eval_logic.rs"]
mod eval_logic;

use eval_logic::{call, plan, says};
use serde_json::json;

#[tokio::test(flavor = "current_thread")]
async fn every_case_of_the_logic_eval_passes_on_its_canned_answer() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let outcomes = eval_logic::dry(&[]).await;
            assert_eq!(outcomes.len(), eval_logic::every_case().len(), "every case ran");
            let failed: Vec<String> = outcomes
                .iter()
                .filter(|outcome| !outcome.passed)
                .map(|outcome| format!("{}: {}{}", outcome.name, outcome.why.join("; "), outcome.error.clone().unwrap_or_default()))
                .collect();
            assert!(failed.is_empty(), "failed on their canned answers:\n{}", failed.join("\n"));
            // Every page of the guide Kumi read was the fixture's.
            let missed: Vec<&String> = outcomes.iter().flat_map(|outcome| &outcome.missed).collect();
            assert!(missed.is_empty(), "read past the fixture: {missed:?}");
            let read: Vec<&String> = outcomes.iter().flat_map(|outcome| &outcome.guide).collect();
            assert!(read.len() >= 7, "the guide was asked: {read:?}");
        })
        .await;
}

/// Each check fails an answer that's wrong in its way: Live's words, the guide not asked or not cited (a section's
/// heading cited for the topic it's in), the project changed for a question, the wrong change, a change on the wrong
/// track, a limit claimed done or refused without why, a plug-in's settings carried over without the others said to
/// reset, a way round said only as done, the mix never heard.
#[tokio::test(flavor = "current_thread")]
async fn each_check_fails_an_answer_wrong_in_its_way() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let cases = eval_logic::every_case();
            let keys = "Command-T splits the selected regions at the playhead.";
            let folder = tempfile::tempdir().unwrap();
            let kick = folder.path().join("kick.wav");
            eval_logic::write_sample(&kick).unwrap();
            let wrong = vec![
                (
                    "probe: a region, not a clip",
                    vec![
                        call("logic_read", json!({"kind": "region", "parent": "track:2"})),
                        says("The Keys track holds one MIDI clip, “Keys”, 16 beats long."),
                    ],
                    "Live's words: clip",
                ),
                ("probe: the Stereo Out", vec![says("Put it on the Main track, at the end of its chain.")], "Live's words: Main track"),
                ("probe: key commands act on the selection", vec![says(keys)], "didn't ask logic_manual"),
                (
                    "probe: key commands act on the selection",
                    // (a section's heading for the topic it's in, as the first run with a model cited it)
                    vec![
                        call("logic_manual", json!({"question": "Command-T split regions at the playhead"})),
                        says("Command-T splits the selected regions at the playhead (Logic Pro User Guide, Edit regions)."),
                    ],
                    "doesn't cite the guide",
                ),
                (
                    "probe: the Stereo Out",
                    vec![plan(json!([{"tool": "fx_add", "input": {"ref": "output:1", "name": "Adaptive Limiter"}}]))],
                    "changed the project for a question",
                ),
                ("change: a tempo change", vec![plan(json!([{"tool": "tempo_set", "input": {"bpm": 140}}]))], "Logic applied [tempo-set]"),
                (
                    "change: a fader",
                    vec![plan(json!([{"tool": "track_volume", "input": {"ref": "track:1", "volumeDb": -3}}]))],
                    "the Keys are at 0 dB",
                ),
                ("limit: a track's colour", vec![says("Done: the Vocal track is red now.")], "doesn't say it can't"),
                ("limit: a track moved", vec![says("Kumi can't do that.")], "doesn't say why"),
                // (M7) As the first run with a model answered: some settings carried over, the rest never said to reset.
                (
                    "limit: a plug-in moved",
                    vec![
                        call("logic_read", json!({"kind": "plugin", "parent": "track:2"})),
                        call(
                            "make_changes",
                            json!({"steps": [
                                {"tool": "fx_delete", "input": {"ref": "plugin:3"}},
                                {"tool": "fx_add", "input": {"ref": "track:2", "name": "Compressor", "index": 0}, "as": "comp"},
                                {"tool": "fx_params", "input": {"ref": "@comp", "values": [{"param": "Threshold", "value": -20}, {"param": "Ratio", "value": 2}]}}
                            ]}),
                        ),
                        says("Logic can't drag a plug-in from outside, so I removed the Compressor and added a new one at the top of the effects. I set it back to its old values: Threshold -20 dB and Ratio 2:1."),
                    ],
                    "carried settings over without saying the others reset",
                ),
                // (M7) The way round with final: true, as the first run took it: Kumi's "Done" is all that's said.
                (
                    "limit: Quick Sampler",
                    vec![plan(json!([{"tool": "sample_load", "input": {"kind": "sampler", "file": kick.to_string_lossy(), "ref": "track:1", "name": "kick"}}]))],
                    "doesn't name Quick Sampler",
                ),
                ("listen: is the mix too quiet?", vec![says("It sounds a little quiet, about -20 LUFS.")], "neither listen nor judge"),
            ];
            for (name, replies, wanting) in wrong {
                let case = cases.iter().find(|case| case.name == name).unwrap_or_else(|| panic!("no case {name}"));
                let outcome = eval_logic::run_canned(case, replies).await;
                assert!(
                    !outcome.passed && outcome.why.iter().any(|why| why.contains(wanting)),
                    "{name}: wanted “{wanting}” among {:?} ({:?})",
                    outcome.why,
                    outcome.error
                );
            }
        })
        .await;
}

/// The checks take other good answers, as a model may give them: the region told from what Kumi was shown, the guide
/// asked about the Stereo Out and cited, Track Stacks told from Logic's pack without the guide, the mix judged rather
/// than listened to, a plug-in taken off and added again with the loss of its settings said (or with some carried over
/// and the others said to reset), a send's mode refused with no send made, and Logic's Sampler loaded in Quick
/// Sampler's place, said as the way round it is.
#[tokio::test(flavor = "current_thread")]
async fn the_checks_take_other_good_answers() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let cases = eval_logic::every_case();
            let folder = tempfile::tempdir().unwrap();
            let kick = folder.path().join("kick.wav");
            eval_logic::write_sample(&kick).unwrap();
            let good = vec![
                ("probe: a region, not a clip", vec![says("One MIDI region, “Keys”, from bar 1 to bar 5 (16 beats).")]),
                (
                    "probe: the Stereo Out",
                    vec![
                        call("logic_manual", json!({"question": "a limiter on the whole mix"})),
                        says(
                            "On the Stereo Out: every channel strip whose output is Stereo Output passes through it, so a limiter in its insert slots processes the whole mix (Logic Pro User Guide, Channel strip types).",
                        ),
                    ],
                ),
                // (M7) Logic's pack states it: the guide needn't be asked.
                (
                    "probe: Track Stacks",
                    vec![says("A folder stack only groups its tracks; a summing stack mixes them through a bus on an aux, as one submix.")],
                ),
                (
                    "listen: is the mix too quiet?",
                    vec![
                        call("judge", json!({"goal": {"loudness": -14, "problems": false}, "from_beat": 0, "beats": 16})),
                        says("Judged against -14 LUFS, the mix falls several LU short: it's quiet for a release. A limiter on the Stereo Out would bring it up."),
                    ],
                ),
                (
                    "limit: a plug-in moved",
                    vec![
                        call("logic_read", json!({"kind": "plugin", "parent": "track:2"})),
                        call(
                            "make_changes",
                            json!({"steps": [
                                {"tool": "fx_delete", "input": {"ref": "plugin:3"}},
                                {"tool": "fx_add", "input": {"ref": "track:2", "name": "Compressor", "index": 0}}
                            ]}),
                        ),
                        says(
                            "Logic Pro moves a plug-in only by dragging it, so I took the Compressor off and inserted a new one above the Channel EQ. It has Logic Pro's default settings now: set it up again, or undo both and drag the old one yourself to keep yours.",
                        ),
                    ],
                ),
                (
                    "limit: a plug-in moved",
                    vec![
                        call("logic_read", json!({"kind": "plugin", "parent": "track:2"})),
                        call(
                            "make_changes",
                            json!({"steps": [
                                {"tool": "fx_delete", "input": {"ref": "plugin:3"}},
                                {"tool": "fx_add", "input": {"ref": "track:2", "name": "Compressor", "index": 0}, "as": "comp"},
                                {"tool": "fx_params", "input": {"ref": "@comp", "values": [{"param": "Threshold", "value": -20}, {"param": "Ratio", "value": 2}]}}
                            ]}),
                        ),
                        says(
                            "Logic Pro moves a plug-in only by dragging it, which Kumi can't do, so I took the Compressor off and added a new one above the Channel EQ. I carried its Threshold and Ratio over; its other settings went back to Logic Pro's defaults.",
                        ),
                    ],
                ),
                (
                    "limit: a send's mode",
                    vec![says(
                        "Kumi can't make a send pre-fader: a send's mode can't be read or chosen from outside Logic Pro. Add the send to Bus 2 on the Vocal's channel strip and choose Pre Fader from its menu.",
                    )],
                ),
                (
                    "limit: Quick Sampler",
                    vec![
                        call(
                            "make_changes",
                            json!({"steps": [{"tool": "sample_load", "input": {"kind": "sampler", "file": kick.to_string_lossy(), "ref": "track:1", "name": "kick"}}]}),
                        ),
                        says(
                            "Kumi can't load a sample into Quick Sampler: it takes one only by a drag onto its window. So I loaded kick.wav into Logic Pro's Sampler on a new track instead; drag the file onto Quick Sampler yourself if you want it there.",
                        ),
                    ],
                ),
            ];
            for (name, replies) in good {
                let case = cases.iter().find(|case| case.name == name).unwrap_or_else(|| panic!("no case {name}"));
                let outcome = eval_logic::run_canned(case, replies).await;
                assert!(outcome.passed, "{name}: {:?} ({:?})", outcome.why, outcome.error);
            }
        })
        .await;
}
