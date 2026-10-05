use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use kumi::input::ByteListener;
use kumi::tui::keys::{InputEvent, InputParser, Modifiers, MouseAction, MouseButton, WheelDirection};
use kumi::tui::render::{Cursor, Renderer};
use kumi::tui::scheduler::FrameScheduler;
use kumi::tui::screen::{Rect, Screen};
use kumi::tui::style::{detect_color_depth, hex, palette, sgr, style_key, to16, to256, ColorDepth, Rgb, Style, StyleTable};
use kumi::tui::tty::{emergency_restorers, Tty, TtyInput, TtyOptions, TtyOutput, RESTORE};
use kumi::tui::width::{cell_width, text_width, truncate};
use kumi::tui::wrap::{wrap, Span};

#[path = "support/vt.rs"]
mod vt;
use vt::VirtualTerminal;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
}

fn same_as_screen(vt: &VirtualTerminal, screen: &Screen) {
    for y in 0..screen.height {
        for x in 0..screen.width {
            let cell = screen.at(x, y);
            assert_eq!(vt.chars[y as usize][x as usize], if cell.width == 0 { "" } else { cell.char }, "char at {x},{y}");
            assert_eq!(vt.styles[y as usize][x as usize], style_key(&cell.style), "style at {x},{y}");
        }
    }
}

async fn local<F: Future>(future: F) -> F::Output {
    tokio::task::LocalSet::new().run_until(future).await
}

#[test]
fn widths_follow_graphemes_ascii_1_cjk_and_emoji_2_combining_marks_join_their_letter() {
    assert_eq!(cell_width("a"), 1);
    assert_eq!(cell_width("主"), 2);
    assert_eq!(cell_width("🎹"), 2);
    assert_eq!(text_width("e\u{301}"), 1);
    assert_eq!(text_width("主旋律 シンセ"), 13);
    assert_eq!(truncate("IGNORE RULES: start playback", 12), "IGNORE RULE…");
    assert_eq!(truncate("主旋律シンセ", 7), "主旋律…");
    assert_eq!(truncate("short", 12), "short");
}

/// What `string-width` 8.1.0 answers under Node 25 (Unicode 17) for the glyphs Kumi draws, and the emoji rules around them.
#[test]
fn widths_match_string_width_for_the_glyphs_kumi_draws() {
    let one = [
        "⠋",
        "─",
        "●",
        "▾",
        "✓",
        "×",
        "…",
        "⌘",
        "→",
        "│",
        "a",
        "é",
        "e\u{301}",
        "ｰ",
        "ﾞ",
        "ﾟ",
        "≈",
        "♪",
        "◆",
        "◇",
        "▣",
        "▦",
        "∞",
        "□",
        "○",
        "▪",
        "■",
        "↩",
        "▶",
        "▬",
        "▼",
        "~",
        "▫",
        "≋",
        "♯",
        "⠁",
        "⣾",
        "▁",
        "▂",
        "▃",
        "▄",
        "▅",
        "▆",
        "▇",
        "█",
        "◐",
        "◓",
        "◑",
        "◒",
        "▷",
        "▖",
        "▘",
        "▝",
        "▗",
        "━",
        "┃",
        "┼",
        "•",
        "∙",
        "·",
        "▌",
        "░",
        "▒",
        "▓",
        "▀",
        "✔",
        "❤",
        "ᅡ",
        "ﬁ",
        "⌫",
        "⏎",
        "⚙",
        "✗",
        "✕",
        "⏸",
        "⏯",
        "⏺",
        "≡",
        "⚠",
        "ℹ",
        "©",
        "®",
        "™",
        "⬆",
        "⬇",
        "⬅",
        "➡",
        "↑",
        "↓",
        "←",
        "⇧",
        "⌥",
        "⌃",
        "␣",
        "#\u{fe0f}",
        "🇯",
        "👁\u{200d}🗨",
        "✈",
        "☺",
        "🎚",
        "🎛",
        "⏏",
        "⏭",
        "⏮",
        "🗒",
        "◼",
        "◻",
        "☑",
        "⏱",
        "⏲",
    ];
    for glyph in one {
        assert_eq!(cell_width(glyph), 1, "{glyph:?}");
    }
    let two = [
        "🎛️",
        "🎹",
        "안",
        "ｆ",
        "主",
        "シ",
        "✔️",
        "☕",
        "❤️",
        "❤️‍🔥",
        "👨‍👩‍👧",
        "🇯🇵",
        "👍🏽",
        "#️⃣",
        "가",
        "ᄀ",
        "☰",
        "⚙️",
        "🔍",
        "💡",
        "⚠️",
        "ℹ️",
        "🏽",
        "☕️",
        "👨‍💻",
        "🧑🏿‍🚀",
        "🏳️‍🌈",
        "👁️‍🗨️",
        "©️",
        "0️⃣",
        "\u{1100}\u{1161}",
        "🏴󠁧󠁢󠁥󠁮󠁧󠁿",
        "🤝🏽",
        "🫱🏻‍🫲🏿",
        "👩‍❤️‍👨",
        "⌚",
        "⌚️",
        "✈️",
        "☺️",
        "❌",
        "🔥",
        "🎵",
        "🎶",
        "🔊",
        "🎚️",
        "🎼",
        "🎧",
        "🎤",
        "🥁",
        "🎸",
        "⏩",
        "⏪",
        "🔁",
        "📂",
        "💾",
        "🧠",
        "⚡",
        "⚡️",
        "🚀",
        "🧪",
        "🪄",
        "🎯",
        "✨",
        "💬",
        "📝",
        "🗒️",
        "📌",
        "🔗",
        "⬛",
        "⬜",
        "🟩",
        "🟥",
        "◼️",
        "▪️",
        "▫️",
        "✅",
        "☑️",
        "⏱️",
        "⌛",
        "⏳",
    ];
    for glyph in two {
        assert_eq!(cell_width(glyph), 2, "{glyph:?}");
    }
    for glyph in ["\u{ad}", "\u{200b}", "\u{200d}", "\u{fe0f}", "\u{20e3}", "\u{301}", "\u{7}", "\u{1b}"] {
        assert_eq!(cell_width(glyph), 0, "{glyph:?}");
    }
    assert_eq!(text_width("a\u{200d}b"), 2);
}

#[test]
fn colour_depth_comes_from_the_environment_and_colours_degrade_to_256_and_16() {
    assert_eq!(detect_color_depth(&env(&[("COLORTERM", "truecolor")]), "darwin", "24.6.0"), ColorDepth::Truecolor);
    assert_eq!(
        detect_color_depth(&env(&[("TERM_PROGRAM", "Apple_Terminal"), ("TERM", "xterm-256color")]), "darwin", "24.6.0"),
        ColorDepth::Colors256
    );
    assert_eq!(detect_color_depth(&env(&[("TERM", "xterm-256color")]), "linux", "6.8.0"), ColorDepth::Colors256);
    assert_eq!(detect_color_depth(&env(&[]), "linux", "6.8.0"), ColorDepth::Colors16);
    assert_eq!(detect_color_depth(&env(&[("NO_COLOR", "1"), ("COLORTERM", "truecolor")]), "darwin", "24.6.0"), ColorDepth::None);
    assert_eq!(detect_color_depth(&env(&[("KUMI_COLOR", "16"), ("COLORTERM", "truecolor")]), "darwin", "24.6.0"), ColorDepth::Colors16);
    // Windows' console and Windows Terminal draw 24-bit colour without saying so; the oldest Windows 10 didn't.
    assert_eq!(detect_color_depth(&env(&[]), "win32", "10.0.19045"), ColorDepth::Truecolor);
    assert_eq!(detect_color_depth(&env(&[("WT_SESSION", "x"), ("TERM", "xterm-256color")]), "win32", "10.0.26100"), ColorDepth::Truecolor);
    assert_eq!(detect_color_depth(&env(&[]), "win32", "10.0.10586"), ColorDepth::Colors16);
    assert_eq!(detect_color_depth(&env(&[("NO_COLOR", "1")]), "win32", "10.0.19045"), ColorDepth::None);
    assert_eq!(detect_color_depth(&env(&[("KUMI_COLOR", "256")]), "win32", "10.0.19045"), ColorDepth::Colors256);
    assert_eq!(to256([255, 0, 0]), 196);
    assert_eq!(to256([128, 128, 128]), 244);
    assert_eq!(to16([250, 250, 250]), 15);
    let style = Style { fg: Some(palette::ACCENT), bg: Some(palette::GROUND), bold: true, ..Style::default() };
    assert_eq!(sgr(&style, ColorDepth::Truecolor), "\u{1b}[0;1;38;2;134;227;181;48;2;14;15;18m");
    let degraded = sgr(&style, ColorDepth::Colors256);
    assert!(regex::Regex::new(r"^\u{1b}\[0;1;38;5;[0-9]+;48;5;[0-9]+m$").unwrap().is_match(&degraded), "{degraded:?}");
    assert_eq!(sgr(&style, ColorDepth::None), "\u{1b}[0;1m");
    assert_eq!(hex("#86e3b5"), Ok([134, 227, 181]));
    assert!(hex("mint").is_err());
    assert_eq!(hex("#0e0f12"), Ok(palette::GROUND));
    assert_eq!(hex("#e7c88f"), Ok(palette::LESSON));
}

#[test]
fn the_screen_keeps_wide_characters_whole_and_lets_text_show_the_background_beneath_it() {
    let mut screen = Screen::new(8, 2);
    screen.fill(Rect::new(0, 0, 8, 2), &Style::bg(palette::SURFACE));
    let end = screen.put(1, 0, "a主b", &Style::fg(palette::TEXT));
    assert_eq!(end, 5);
    assert_eq!(screen.lines(), vec![" a主b   ", "        "]);
    assert_eq!(screen.at(2, 0).width, 2);
    assert_eq!(screen.at(3, 0).width, 0);
    assert_eq!(screen.at(1, 0).style, Style { fg: Some(palette::TEXT), bg: Some(palette::SURFACE), ..Style::default() });
    screen.put(3, 0, "x", &Style::default());
    assert_eq!(screen.lines()[0], " a xb   ", "writing into the second half blanks the first");
    screen.put(6, 1, "主主", &Style::default());
    assert_eq!(screen.lines()[1], "      主", "a wide character that does not fit is left out");
    screen.put_in(0, 1, "abcdef", &Style::default(), Rect::new(0, 1, 3, 1));
    assert_eq!(&screen.lines()[1][..4], "abc ");
    screen.put(0, 0, "\u{7}bell", &Style::default());
    assert_eq!(&screen.lines()[0][..5], " bell");
}

#[test]
fn the_renderers_output_reproduces_every_frame_exactly_redrawing_only_what_changed() {
    let mut seed = 7.0f64;
    let mut random = move || {
        seed = (seed * 1103515245.0 + 12345.0) % 2147483648.0;
        seed / 2147483648.0
    };
    let words = ["kick", "主旋律", "🎹", "Bass", " ", "EQ Eight", "シンセ", "·", "━━━", "e\u{301}"];
    let colours: [Rgb; 6] = [palette::GROUND, palette::SURFACE, palette::RAISED, palette::ACCENT, palette::TEXT, palette::FAINT];
    let table = Rc::new(StyleTable::new());
    let mut renderer = Renderer::new(ColorDepth::Truecolor);
    let mut vt = VirtualTerminal::new(24, 8);
    let mut screen = Screen::with_table(24, 8, Rc::clone(&table));
    vt.write(&renderer.frame(&screen, None));
    same_as_screen(&vt, &screen);
    for _round in 0..300 {
        let mut next = Screen::with_table(24, 8, Rc::clone(&table));
        next.copy_from(&screen);
        let changes = 1 + (random() * 5.0).floor() as i32;
        for _change in 0..changes {
            if random() < 0.3 {
                let rect = Rect::new(
                    (random() * 24.0).floor() as i32,
                    (random() * 8.0).floor() as i32,
                    1 + (random() * 10.0).floor() as i32,
                    1 + (random() * 3.0).floor() as i32,
                );
                let bg = colours[(random() * 6.0).floor() as usize];
                next.fill(rect, &Style::bg(bg));
            } else {
                let x = (random() * 26.0).floor() as i32 - 1;
                let y = (random() * 8.0).floor() as i32;
                let word = words[(random() * 10.0).floor() as usize];
                let fg = colours[(random() * 6.0).floor() as usize];
                let bg = if random() < 0.3 { Some(colours[(random() * 6.0).floor() as usize]) } else { None };
                let bold = random() < 0.2;
                next.put(x, y, word, &Style { fg: Some(fg), bg, bold, ..Style::default() });
            }
        }
        let output = renderer.frame(&next, None);
        vt.write(&output);
        same_as_screen(&vt, &next);
        screen = next;
    }
    assert_eq!(renderer.frame(&screen, None), "", "an unchanged frame writes nothing");
    let mut one = Screen::with_table(24, 8, Rc::clone(&table));
    one.copy_from(&screen);
    one.put(0, 0, "Z", &Style::fg(palette::BRIGHT));
    let small = renderer.frame(&one, None);
    assert!(small.len() < 120, "one changed cell should be a small update, got {} bytes", small.len());
    vt.write(&small);
    same_as_screen(&vt, &one);
}

#[test]
fn the_renderer_places_the_cursor_for_input_methods_and_redraws_everything_after_a_resize() {
    let mut renderer = Renderer::new(ColorDepth::Truecolor);
    let mut screen = Screen::new(10, 3);
    screen.put(0, 2, "type", &Style::fg(palette::TEXT));
    let mut vt = VirtualTerminal::new(10, 3);
    vt.write(&renderer.frame(&screen, Some(Cursor { x: 4, y: 2 })));
    assert_eq!(vt.x, 4);
    assert_eq!(vt.y, 2);
    assert!(vt.cursor_visible);
    assert_ne!(renderer.frame(&screen, Some(Cursor { x: 5, y: 2 })), "", "a moved cursor is an update");
    vt.write(&renderer.frame(&screen, None));
    assert!(!vt.cursor_visible);
    let bigger = Screen::new(12, 4);
    let output = renderer.frame(&bigger, None);
    assert!(output.contains("\u{1b}[2J"), "a new size clears and redraws");
    assert!(output.starts_with("\u{1b}[?2026h") && output.ends_with("\u{1b}[?2026l"), "frames are synchronized updates");
}

async fn parse(chunks: &[&str], delay_ms: f64) -> Vec<InputEvent> {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let parser = InputParser::with_escape_delay(Rc::new(move |event| sink.borrow_mut().push(event)), delay_ms);
    for chunk in chunks {
        parser.push(chunk);
    }
    tokio::time::sleep(Duration::from_secs_f64(delay_ms * 4.0 / 1000.0)).await;
    parser.dispose();
    events.take()
}

fn key(name: &str, mods: Modifiers) -> InputEvent {
    InputEvent::Key { name: name.to_string(), mods, repeat: false }
}

fn text(text: &str) -> InputEvent {
    InputEvent::Text { text: text.to_string() }
}

const NONE: Modifiers = Modifiers::NONE;
const CTRL: Modifiers = Modifiers::CTRL;
const ALT: Modifiers = Modifiers::ALT;
const SHIFT: Modifiers = Modifiers::SHIFT;
const ALT_SHIFT: Modifiers = Modifiers { ctrl: false, alt: true, shift: true };

#[tokio::test(start_paused = true)]
async fn keys_text_controls_arrows_with_modifiers_function_keys_and_alt_combinations() {
    local(async {
        assert_eq!(parse(&["hello 主旋律"], 5.0).await, vec![text("hello 主旋律")]);
        assert_eq!(
            parse(&["\r", "\u{3}", "\u{7f}", "\t", "\n"], 5.0).await,
            vec![key("enter", NONE), key("c", CTRL), key("backspace", NONE), key("tab", NONE), key("j", CTRL)]
        );
        assert_eq!(
            parse(&["\u{1b}[A\u{1b}[1;5C\u{1b}[1;2D\u{1b}OH\u{1b}[F"], 5.0).await,
            vec![key("up", NONE), key("right", CTRL), key("left", SHIFT), key("home", NONE), key("end", NONE)]
        );
        assert_eq!(
            parse(&["\u{1b}[3~\u{1b}[5~\u{1b}[6;3~\u{1b}[15~\u{1b}OP\u{1b}[Z"], 5.0).await,
            vec![key("delete", NONE), key("pageup", NONE), key("pagedown", ALT), key("f5", NONE), key("f1", NONE), key("tab", SHIFT)]
        );
        assert_eq!(
            parse(&["\u{1b}b\u{1b}B\u{1b}\r\u{1b}\u{7f}"], 5.0).await,
            vec![key("b", ALT), key("b", ALT_SHIFT), key("enter", ALT), key("backspace", ALT)]
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn keys_shift_enter_in_csi_u_and_modify_other_keys_forms_and_a_key_let_go_is_its_own_event() {
    local(async {
        assert_eq!(
            parse(&["\u{1b}[13;2u", "\u{1b}[27;2;13~", "\u{1b}[97;5u", "\u{1b}[97;5:3u", "\u{1b}[97;2u", "\u{1b}[97;2:3u"], 5.0).await,
            vec![
                key("enter", SHIFT),
                key("enter", SHIFT),
                key("a", CTRL),
                InputEvent::Release { name: "a".to_string(), mods: CTRL },
                text("A")
            ],
            "letting go of a letter types nothing"
        );
        assert_eq!(
            parse(&["\u{1b}[27u\u{1b}[99;5u\u{1b}[106;5u\u{1b}[13;3u"], 5.0).await,
            vec![key("escape", NONE), key("c", CTRL), key("j", CTRL), key("enter", ALT)],
            "with the kitty protocol on, Escape and Ctrl combinations arrive as CSI u"
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn keys_with_the_kitty_protocol_saying_so_a_held_key_repeats_and_is_let_go_in_every_form_a_key_takes() {
    let release = |name: &str, mods: Modifiers| InputEvent::Release { name: name.to_string(), mods };
    let repeat = |name: &str, mods: Modifiers| InputEvent::Key { name: name.to_string(), mods, repeat: true };
    local(async {
        // ctrl+t held, then let go: a press, its repeats, and the let-go.
        assert_eq!(
            parse(&["\u{1b}[116;5u", "\u{1b}[116;5:2u\u{1b}[116;5:2u", "\u{1b}[116;5:3u"], 5.0).await,
            vec![key("t", CTRL), repeat("t", CTRL), repeat("t", CTRL), release("t", CTRL)]
        );
        // Arrows, F1–F4 and the ~ keys say so after their modifiers; letting go never presses them again.
        assert_eq!(
            parse(&["\u{1b}[1;1:2A\u{1b}[1;1:3A\u{1b}[1;5:3C\u{1b}[1;1:3P\u{1b}[5;1:3~\u{1b}[3;1:2~\u{1b}[27;1:3u"], 5.0).await,
            vec![
                repeat("up", NONE),
                release("up", NONE),
                release("right", CTRL),
                release("f1", NONE),
                release("pageup", NONE),
                repeat("delete", NONE),
                release("escape", NONE)
            ]
        );
        // A letter held repeats as text, and an explicit press is a press.
        assert_eq!(parse(&["\u{1b}[97;1:2u\u{1b}[116;5:1u"], 5.0).await, vec![text("a"), key("t", CTRL)]);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn keys_a_lone_escape_waits_briefly_and_sequences_split_across_reads_still_decode() {
    local(async {
        assert_eq!(parse(&["\u{1b}"], 5.0).await, vec![key("escape", NONE)]);
        assert_eq!(parse(&["\u{1b}[", "1;5", "A"], 5.0).await, vec![key("up", CTRL)]);
        assert_eq!(parse(&["\u{1b}", "[B"], 5.0).await, vec![key("down", NONE)]);
        assert_eq!(parse(&["\u{1b}\u{1b}[A"], 5.0).await, vec![key("escape", NONE), key("up", NONE)]);
        assert_eq!(parse(&["\u{1b}[12;"], 5.0).await, vec![], "a truncated sequence is dropped, never typed");
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn escape_deadline_starts_at_input_even_when_its_task_is_not_polled() {
    let local = tokio::task::LocalSet::new();
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = Rc::new(tokio::sync::Notify::new());
    let sink = events.clone();
    let ready = received.clone();
    let parser = InputParser::new(Rc::new(move |event| {
        sink.borrow_mut().push(event);
        ready.notify_one();
    }));
    {
        let _entered = local.enter();
        parser.push("\x1b");
    }
    // The local task cannot run yet, but the original 25 ms deadline has passed.
    tokio::time::advance(Duration::from_millis(30)).await;
    local
        .run_until(async {
            tokio::time::timeout(Duration::from_millis(1), received.notified()).await.expect("Escape deadline was postponed");
        })
        .await;
    assert_eq!(*events.borrow(), vec![key("escape", NONE)]);
    parser.dispose();
}

#[tokio::test(start_paused = true)]
async fn pastes_bracketed_pastes_even_split_and_unbracketed_multi_line_reads_never_send_enter() {
    local(async {
        assert_eq!(
            parse(&["\u{1b}[200~line one\r\nline", " two\u{1b}[20", "1~x"], 5.0).await,
            vec![InputEvent::Paste { text: "line one\nline two".to_string() }, text("x")]
        );
        assert_eq!(parse(&["first\rsecond\r"], 5.0).await, vec![InputEvent::Paste { text: "first\nsecond\n".to_string() }]);
        assert_eq!(parse(&["typed\r"], 5.0).await, vec![text("typed"), key("enter", NONE)]);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn mouse_and_focus_sgr_presses_drags_wheel_and_focus_changes() {
    let mouse = |action, button, direction, x, y, mods| InputEvent::Mouse { action, button, direction, x, y, mods };
    local(async {
        let events =
            parse(&["\u{1b}[<0;10;5M\u{1b}[<32;11;5M\u{1b}[<0;11;5m\u{1b}[<64;3;4M\u{1b}[<65;3;4M\u{1b}[<18;1;1M\u{1b}[I\u{1b}[O"], 5.0)
                .await;
        assert_eq!(
            events,
            vec![
                mouse(MouseAction::Press, MouseButton::Left, None, 9, 4, NONE),
                mouse(MouseAction::Drag, MouseButton::Left, None, 10, 4, NONE),
                mouse(MouseAction::Release, MouseButton::Left, None, 10, 4, NONE),
                mouse(MouseAction::Wheel, MouseButton::None, Some(WheelDirection::Up), 2, 3, NONE),
                mouse(MouseAction::Wheel, MouseButton::None, Some(WheelDirection::Down), 2, 3, NONE),
                mouse(MouseAction::Press, MouseButton::Right, None, 0, 0, CTRL),
                InputEvent::Focus { focused: true },
                InputEvent::Focus { focused: false },
            ]
        );
    })
    .await;
}

#[test]
fn wrapping_keeps_words_and_styles_splits_long_words_and_honours_newlines() {
    let plain = Rc::new(Style::default());
    let bold = Rc::new(Style { bold: true, ..Style::default() });
    let text = |lines: Vec<Vec<Span>>| -> Vec<String> {
        lines.iter().map(|line| line.iter().map(|span| span.text.as_str()).collect::<String>()).collect()
    };
    assert_eq!(
        text(wrap(&[Span::new("The kick and bass are fighting around 200 Hz.", &plain)], 16)),
        vec!["The kick and", "bass are", "fighting around", "200 Hz."]
    );
    assert_eq!(text(wrap(&[Span::new("abcdefghij", &plain)], 4)), vec!["abcd", "efgh", "ij"]);
    assert_eq!(text(wrap(&[Span::new("one\n\n  indented", &plain)], 20)), vec!["one", "", "  indented"]);
    assert_eq!(text(wrap(&[Span::new("主旋律シンセ", &plain)], 5)), vec!["主旋", "律シ", "ンセ"]);
    let styled = wrap(&[Span::new("EQ ", &plain), Span::new("Eight band", &bold)], 8);
    let flags: Vec<Vec<bool>> = styled.iter().map(|line| line.iter().map(|span| Rc::ptr_eq(&span.style, &bold)).collect()).collect();
    assert_eq!(flags, vec![vec![false, true], vec![true]]);
    assert_eq!(text(wrap(&[], 10)), vec![""]);
}

#[tokio::test(start_paused = true)]
async fn the_scheduler_turns_many_requests_into_one_frame_and_animates_only_while_asked() {
    local(async {
        let draws = Rc::new(Cell::new(0));
        let counter = Rc::clone(&draws);
        let scheduler = FrameScheduler::new(Rc::new(move || counter.set(counter.get() + 1)), 5.0);
        for _ in 0..10 {
            scheduler.request();
        }
        scheduler.flush();
        assert_eq!(draws.get(), 1);
        scheduler.set_animating(true);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(draws.get() >= 4, "animation keeps drawing, got {}", draws.get());
        scheduler.set_animating(false);
        tokio::time::sleep(Duration::from_millis(15)).await;
        let settled = draws.get();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(draws.get(), settled, "no frames once nothing moves");
        scheduler.dispose();
    })
    .await;
}

/// A terminal input fed by the test: a PassThrough with `isTTY`, `isRaw` and `setRawMode`.
struct FakeInput {
    raw: Cell<bool>,
    listener: RefCell<Option<ByteListener>>,
}

impl FakeInput {
    fn write(&self, data: &str) {
        let listener = self.listener.borrow().clone();
        if let Some(listener) = listener {
            listener(data.as_bytes());
        }
    }
}

impl TtyInput for FakeInput {
    fn is_tty(&self) -> bool {
        true
    }
    fn is_raw(&self) -> bool {
        self.raw.get()
    }
    fn set_raw_mode(&self, enabled: bool) -> std::io::Result<()> {
        self.raw.set(enabled);
        Ok(())
    }
    fn resume(&self, listener: ByteListener) {
        *self.listener.borrow_mut() = Some(listener);
    }
    fn pause(&self) {
        *self.listener.borrow_mut() = None;
    }
}

/// A terminal output that keeps what's written: a Writable with `isTTY`, `columns` and `rows`.
struct FakeOutput {
    written: RefCell<String>,
    resize: RefCell<Option<Rc<dyn Fn()>>>,
}

impl FakeOutput {
    fn emit_resize(&self) {
        let listener = self.resize.borrow().clone();
        if let Some(listener) = listener {
            listener();
        }
    }
}

impl TtyOutput for FakeOutput {
    fn is_tty(&self) -> bool {
        true
    }
    fn columns(&self) -> Option<i32> {
        Some(100)
    }
    fn rows(&self) -> Option<i32> {
        Some(30)
    }
    fn write(&self, data: &str) {
        self.written.borrow_mut().push_str(data);
    }
    fn watch_resize(&self, listener: Rc<dyn Fn()>) {
        *self.resize.borrow_mut() = Some(listener);
    }
    fn unwatch_resize(&self) {
        *self.resize.borrow_mut() = None;
    }
}

#[test]
fn the_terminal_is_taken_over_and_always_given_back_even_twice() {
    let input = Rc::new(FakeInput { raw: Cell::new(false), listener: RefCell::new(None) });
    let output = Rc::new(FakeOutput { written: RefCell::new(String::new()), resize: RefCell::new(None) });
    let events = Rc::new(RefCell::new(Vec::new()));
    let resized = Rc::new(Cell::new(0));
    let exit_listeners = emergency_restorers();
    let tty = Tty::new(TtyOptions {
        input: input.clone(),
        output: output.clone(),
        on_input: Rc::new({
            let events = Rc::clone(&events);
            move |event| events.borrow_mut().push(event)
        }),
        on_resize: Rc::new({
            let resized = Rc::clone(&resized);
            move || resized.set(resized.get() + 1)
        }),
        mouse: true,
    });
    tty.start().unwrap();
    assert!(input.is_raw());
    let written = output.written.borrow().clone();
    assert!(written.starts_with("\u{1b}[?1049h"), "alternate screen first");
    assert!(
        written.contains("\u{1b}[?1006h")
            && written.contains("\u{1b}[?2004h")
            && written.contains("\u{1b}[?7l")
            && written.contains("\u{1b}[>3u"),
        "keys reported unambiguously, with repeats and let-gos"
    );
    assert_eq!((tty.size().columns, tty.size().rows), (100, 30));
    input.write("\u{1b}[A");
    output.emit_resize();
    assert_eq!(events.borrow().clone(), vec![key("up", NONE)]);
    assert_eq!(resized.get(), 1);
    assert_eq!(emergency_restorers(), exit_listeners + 1);
    tty.restore(false);
    assert!(output.written.borrow().ends_with(RESTORE));
    assert!(!input.is_raw());
    assert_eq!(emergency_restorers(), exit_listeners);
    let length = output.written.borrow().len();
    tty.restore(false);
    tty.write("late frame");
    assert_eq!(output.written.borrow().len(), length, "restoring twice and writing afterwards are no-ops");
}
