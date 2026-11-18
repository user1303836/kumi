//! FOCUS reads a focus in neutral terms, which Live's presentation fills from Live's wire form: when Live's integration
//! sends it, and again when the app reads it. Over the reference's focuses (touchedNext's and focusPath's), filling twice
//! is filling once, the neutral fields say what Live's say one for one, Live's own are left as they were, and what the
//! app makes of a filled focus is what it made of Live's.

use kumi::tui::app::{focus_path, touched_next};
use kumi_agent::core::contracts::{
    filled, BodyKind, DawFocus, FocusItem, FocusSection, FocusView, ItemKind, LiveDetail, LiveView, SectionKind, ViewKind,
};
use serde_json::{json, Value};

fn live() -> &'static kumi_agent::core::view::ViewPresentation {
    kumi::daw::registry::view(kumi_types::daw::DawId::Ableton).unwrap()
}

/// The neutral fields cleared: Live's wire form alone.
fn wire(focus: &DawFocus) -> DawFocus {
    DawFocus { main_view: None, editor_view: None, item: None, section: None, ..focus.clone() }
}

fn check(focus: &DawFocus, label: &str) {
    let once = filled(live(), focus);
    assert_eq!(filled(live(), &once), once, "{label}: filling twice is filling once");
    assert_eq!(wire(&once), wire(focus), "{label}: Live's fields are left as they were");
    let round: DawFocus = serde_json::from_value(serde_json::to_value(&once).unwrap()).unwrap();
    assert_eq!(round, once, "{label}: a filled focus reads back as written");
    // One for one.
    let view = |kind, label: &str| FocusView { kind, label: label.into() };
    assert_eq!(
        once.main_view,
        focus.view.map(|main| match main {
            LiveView::Session => view(ViewKind::Launcher, "Session"),
            LiveView::Arrangement => view(ViewKind::Timeline, "Arrangement"),
        }),
        "{label}"
    );
    assert_eq!(
        once.editor_view,
        focus.detail.map(|detail| match detail {
            LiveDetail::Clip => view(ViewKind::NoteEditor, "Clip view"),
            LiveDetail::Device => view(ViewKind::DeviceChain, "Device view"),
        }),
        "{label}"
    );
    assert_eq!(
        once.item,
        (focus.slot_ref.is_some() || focus.clip.is_some()).then(|| FocusItem {
            kind: ItemKind::Clip,
            item_ref: focus.slot_ref.clone(),
            name: focus.clip.clone()
        }),
        "{label}"
    );
    assert_eq!(
        once.section,
        (focus.scene_index.is_some() || focus.scene.is_some()).then(|| FocusSection {
            kind: SectionKind::Scene,
            index: focus.scene_index,
            name: focus.scene.clone()
        }),
        "{label}"
    );
}

#[test]
fn filling_a_focus_twice_is_filling_it_once_and_the_app_reads_it_as_it_read_lives() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("../support/app/reference.json")).unwrap();
    let (mut touched, mut paths, mut focuses) = (0, 0, 0);
    for (index, case) in cases.iter().enumerate() {
        let args = case["args"].as_array().unwrap();
        let label = format!("case {index} ({})", case["fn"]);
        match case["fn"].as_str().unwrap() {
            "touchedNext" => {
                let before: Option<DawFocus> = serde_json::from_value(args[0].clone()).unwrap();
                let after: Option<DawFocus> = serde_json::from_value(args[1].clone()).unwrap();
                for focus in [&before, &after].into_iter().flatten() {
                    check(focus, &label);
                    focuses += 1;
                }
                // The app keeps focuses filled: what it makes of them is what the reference says of Live's.
                let was = serde_json::from_value(args[2].clone()).unwrap();
                let fill = |focus: &Option<DawFocus>| focus.as_ref().map(|focus| filled(live(), focus));
                assert_eq!(json!(touched_next(fill(&before).as_ref(), fill(&after).as_ref(), was)), case["expected"], "{label}");
                touched += 1;
            }
            "focusPath" => {
                let focus: DawFocus = serde_json::from_value(args[0].clone()).unwrap();
                check(&focus, &label);
                assert_eq!(json!(focus_path(&filled(live(), &focus))), case["expected"], "{label}");
                focuses += 1;
                paths += 1;
            }
            _ => {}
        }
    }
    assert_eq!(touched, 432, "touchedNext's cases");
    assert!(paths > 0 && focuses > 400, "{paths} focus paths, {focuses} focuses");
}

#[test]
fn live_says_its_bodies_in_its_own_words() {
    let heading = live().heading;
    assert_eq!(
        [BodyKind::Tree, BodyKind::Notes, BodyKind::Grid, BodyKind::Timeline].map(heading),
        ["Device", "Clip", "Session", "Arrangement"]
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "disagrees with Live's")]
fn a_neutral_field_that_disagrees_with_lives_is_caught() {
    let focus = DawFocus {
        view: Some(LiveView::Session),
        main_view: Some(FocusView { kind: ViewKind::Timeline, label: "Arrangement".into() }),
        ..DawFocus::default()
    };
    filled(live(), &focus);
}
