//! What every DAW's goal rig does alike with a trial's render (`goal`).
use kumi_listening::goal::heard_seconds;
use kumi_types::contracts::Render;

#[test]
fn a_render_is_heard_as_long_as_it_says_else_for_its_part_at_the_tempo() {
    let render = |seconds| Render { file: "take.wav".into(), start: 0.5, seconds };
    assert_eq!(heard_seconds(&render(None), 8., 120.), 4.);
    assert_eq!(heard_seconds(&render(Some(1.5)), 8., 120.), 1.5, "Live stopped short");
}
