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

case!(a_theme_that_follows_the_terminal_is_light_on_a_light_one_and_turns_as_its_colours_do, async {
    let h = on(Some([0xfa, 0xfa, 0xfa]), None).await;
    assert_eq!(ground_of(&h), ground(&LIGHT));
    // The terminal turned dark, then light again (mode 2031's reports): the whole screen is drawn anew in each.
    h.type_text("\x1b[?997;1n").await;
    assert_eq!(ground_of(&h), ground(&DARK));
    h.type_text("\x1b[?997;2n").await;
    assert_eq!(ground_of(&h), ground(&LIGHT));
    // A background it says again is followed too, and none of it is typed.
    h.type_text("\x1b]11;rgb:1e1e/1e1e/1e1e\x07").await;
    assert_eq!(ground_of(&h), ground(&DARK));
    assert!(!has(&h.screen(), "997") && !has(&h.screen(), "rgb"), "{}", h.screen().join("\n"));
    h.close().await;
    // A dark background, or one not known: dark.
    for background in [Some([0x1e, 0x1e, 0x1e]), None] {
        let h = on(background, None).await;
        assert_eq!(ground_of(&h), ground(&DARK), "{background:?}");
        h.close().await;
    }
});

case!(a_theme_the_producer_chose_stays_as_the_terminals_colours_change, async {
    let h = on(Some([0xfa, 0xfa, 0xfa]), Some(&DARK)).await;
    assert_eq!(ground_of(&h), ground(&DARK));
    h.type_text("\x1b[?997;2n").await;
    assert_eq!(ground_of(&h), ground(&DARK));
    h.close().await;
});
