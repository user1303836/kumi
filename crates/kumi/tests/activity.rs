//! The activity half. The Transcript tests in that file
//! belong with `tui/transcript.rs` and are ported alongside it.

use std::collections::HashSet;

use kumi::tui::activity::{activity_glyph, activity_of, activity_scene, shimmer, Activity};
use kumi::tui::width::text_width;
use kumi::tui::wrap::Span;

fn text(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

#[test]
fn each_kind_of_task_has_its_own_animation_a_glyph_a_cell_wide_and_a_scene_as_wide_as_asked_that_moves() {
    assert_eq!(activity_of(Some("search_web")), Activity::Search);
    assert_eq!(activity_of(Some("read_web")), Activity::Read);
    assert_eq!(activity_of(Some("make_device")), Activity::Build);
    assert_eq!(activity_of(Some("live_discover")), Activity::Look);
    assert_eq!(activity_of(Some("set_mixer")), Activity::Change);
    assert_eq!(activity_of(Some("audition")), Activity::Listen);
    assert_eq!(activity_of(None), Activity::Think);
    let mut scenes = HashSet::new();
    for kind in Activity::ALL {
        for ms in [0.0, 130.0, 777.0, 2_400.0, 9_999.0] {
            assert_eq!(text_width(&activity_glyph(kind, ms, false).text), 1, "{} glyph at {ms}", kind.as_str());
            let plain = activity_glyph(kind, ms, true).text;
            assert!(plain.len() == 1 && (0x20..=0x7e).contains(&plain.as_bytes()[0]), "{}'s plain glyph is ASCII", kind.as_str());
        }
        let scene = |ms: f64| activity_scene(kind, ms, 20);
        for ms in [0.0, 250.0, 500.0, 900.0, 1_300.0] {
            assert_eq!(text_width(&text(&scene(ms))), 20, "{} scene is 20 cells: “{}”", kind.as_str(), text(&scene(ms)));
        }
        // What moves may be the light on it rather than its characters (a page's words, read one by one).
        let frames: Vec<String> = [0.0, 250.0, 500.0, 900.0, 1_300.0]
            .iter()
            .map(|ms| scene(*ms).iter().map(|span| format!("{}:{:?}", span.text, span.style.fg)).collect::<String>())
            .collect();
        assert!(frames.iter().collect::<HashSet<_>>().len() > 1, "{}'s scene moves", kind.as_str());
        scenes.insert(frames.join("|"));
    }
    assert_eq!(scenes.len(), Activity::ALL.len(), "no two kinds look the same");
}

#[test]
fn a_shimmer_keeps_the_words_and_passes_over_them() {
    let early = shimmer("reading a page", 100.0);
    let later = shimmer("reading a page", 900.0);
    assert_eq!(text(&early), "reading a page");
    assert_ne!(early.iter().map(|span| span.style.fg).collect::<Vec<_>>(), later.iter().map(|span| span.style.fg).collect::<Vec<_>>());
}
