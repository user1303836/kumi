//! A theme that follows the terminal (`system`, the setting's default): light on a light background, dark on a dark
//! one or one not known, and turning as the terminal says its colours changed. A theme the producer chose stays.
#[path = "../support/tui_app.rs"]
mod support;
use kumi::tui::alerts::TerminalProfile;
use kumi::tui::style::{Theme, DARK, LIGHT};
use std::rc::Rc;
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

/// The app on a terminal whose background is `background`, in `theme` (None: the terminal's).
async fn on(background: Option<[u8; 3]>, theme: Option<&'static Theme>) -> Harness {
    let h = Harness::with(120, 36, Rc::new(Control::default()), move |o| {
        o.terminal = Some(TerminalProfile { background, ..TerminalProfile::quiet() });
        o.theme = theme;
    });
    h.start().await;
    h.connect();
    h
}

/// The ground under the header's "Kumi", as the terminal shows it.
fn ground_of(h: &Harness) -> String {
    h.styles_of("Kumi")[0].split('|').nth(1).unwrap().to_string()
}

fn ground(theme: &Theme) -> String {
    theme.ground.map(|c| c.to_string()).join(",")
}

/// OSC 11's question, as Kumi asks the terminal its background.
const ASKED: &str = "\x1b]11;?\x1b\\";

case!(a_theme_that_follows_the_terminal_is_light_on_a_light_one_and_turns_as_its_background_does, async {
    let h = on(Some([0xfa, 0xfa, 0xfa]), None).await;
    assert_eq!(ground_of(&h), ground(&LIGHT));
    // The terminal says its look turned dark (mode 2031's report): its background is asked for, and that decides, as
    // the report can be the system's light or dark rather than the terminal's colours.
    let before = h.output.text.borrow().matches(ASKED).count();
    h.type_text("\x1b[?997;1n").await;
    assert_eq!(h.output.text.borrow().matches(ASKED).count(), before + 1, "the background asked for");
    assert_eq!(ground_of(&h), ground(&LIGHT), "nothing switched on the report alone");
    h.type_text("\x1b]11;rgb:1e1e/1e1e/1e1e\x07").await;
    assert_eq!(ground_of(&h), ground(&DARK));
    // A report of light over a background still dark (one dark theme, the system turned light): it stays dark.
    h.type_text("\x1b[?997;2n").await;
    h.type_text("\x1b]11;rgb:1e1e/1e1e/1e1e\x07").await;
    assert_eq!(ground_of(&h), ground(&DARK));
    h.type_text("\x1b[?997;2n\x1b]11;rgb:fafa/fafa/fafa\x07").await;
    assert_eq!(ground_of(&h), ground(&LIGHT));
    // None of it typed.
    assert!(!has(&h.screen(), "997") && !has(&h.screen(), "rgb"), "{}", h.screen().join("\n"));
    h.close().await;
    // A dark background, or one not known: dark.
    for background in [Some([0x1e, 0x1e, 0x1e]), None] {
        let h = on(background, None).await;
        assert_eq!(ground_of(&h), ground(&DARK), "{background:?}");
        h.close().await;
    }
});

case!(a_terminal_that_said_no_background_is_taken_at_its_report, async {
    let h = on(None, None).await;
    assert_eq!(ground_of(&h), ground(&DARK));
    h.type_text("\x1b[?997;2n").await;
    assert_eq!(ground_of(&h), ground(&LIGHT));
    assert!(!h.output.text.borrow().contains(ASKED), "it said no background before: none asked for");
    h.close().await;
});

case!(a_picture_open_in_history_is_drawn_and_uploaded_again_in_another_theme, async {
    // On a terminal that shows images: the picture open in HISTORY (and NOW's) goes up again, under new ids.
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| {
        o.terminal = Some(TerminalProfile::quiet());
        o.graphics = Some(kumi::tui::pictures::Graphics::Kitty);
    });
    h.start().await;
    h.connect();
    let note = |start: f64, pitch: f64, duration: f64| serde_json::json!([start, pitch, duration, 100.0]);
    h.emit(
        serde_json::json!({"type":"change","change":{"id":"c1","family":"clip","title":"Chords · 3 notes moved","state":"applied","at":1,
        "picture":{"kind":"notes","length":8.0,"before":[note(0.,60.,2.),note(2.,62.,2.)],"after":[note(0.,60.,1.),note(1.,67.,1.)]}}}),
    );
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    h.screen();
    // HISTORY's own: a clip's notes are 6 rows tall only there (NOW's and the dock's are shorter), and each upload says
    // its rows once.
    let history_uploads = |h: &Harness| h.output.text.borrow().matches(",r=6,").count();
    let before = history_uploads(&h);
    assert!(before >= 1, "HISTORY's picture went up");
    h.type_text("\x1b[?997;2n").await;
    h.screen();
    assert_eq!(ground_of(&h), ground(&LIGHT));
    // Each taken down, and HISTORY's drawn and uploaded afresh in the new theme's colours.
    assert!(h.output.text.borrow().contains("\x1b_Ga=d,d=I,i=1,"), "taken down");
    assert!(history_uploads(&h) > before, "HISTORY's picture up again: {before} then {}", history_uploads(&h));
    h.close().await;
});

case!(a_theme_the_producer_chose_stays_as_the_terminals_colours_change, async {
    let h = on(Some([0xfa, 0xfa, 0xfa]), Some(&DARK)).await;
    assert_eq!(ground_of(&h), ground(&DARK));
    h.type_text("\x1b[?997;2n").await;
    assert_eq!(ground_of(&h), ground(&DARK));
    h.close().await;
});
