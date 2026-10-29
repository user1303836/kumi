use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use kumi::input::ByteListener;
use kumi::tui::keys::{InputEvent, InputParser, Modifiers, MouseAction, MouseButton, Reply, WheelDirection};
use kumi::tui::render::{Cursor, Renderer};
use kumi::tui::scheduler::FrameScheduler;
use kumi::tui::screen::{Rect, Screen};
use kumi::tui::style::{
    answered_color_depth, detect_color_depth, hex, link_id, palette, sgr, style_key, to16, to256, ColorDepth, Rgb, Style, StyleTable,
};
use kumi::tui::tty::{emergency_restorers, Tty, TtyInput, TtyOptions, TtyOutput, RESTORE};
use kumi::tui::width::{cell_width, text_width, truncate};
use kumi::tui::wrap::{wrap, Span};

#[path = "../support/vt.rs"]
mod vt;
use vt::VirtualTerminal;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
}

fn same_as_screen(vt: &VirtualTerminal, screen: &Screen) {
    for y in 0..screen.height {
        for x in 0..screen.width {
            let cell = screen.at(x, y);
            assert_eq!(vt.chars[y as usize][x as usize], if cell.width == 0 { "" } else { cell.char.as_str() }, "char at {x},{y}");
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
fn a_theme_writes_the_palettes_colours_in_its_own_and_another_redraws_everything() {
    use kumi::tui::style::{set_theme, Theme, DARK, LIGHT};
    let table = Rc::new(StyleTable::new());
    let mut screen = Screen::with_table(12, 2, Rc::clone(&table));
    screen.fill(screen.bounds(), &Style::bg(palette::GROUND));
    screen.put(1, 0, "Ready", &Style::fg(palette::FAINT));
    screen.put(1, 1, "undo", &Style::fg(palette::ACCENT));
    // What the terminal shows, in a theme's colours: each cell's style written in them.
    let shown_in = |theme: &'static Theme, screen: &Screen| {
        let mut vt = VirtualTerminal::new(12, 2);
        let mut renderer = Renderer::new(ColorDepth::Truecolor);
        set_theme(theme);
        vt.write(&renderer.frame(screen, None));
        (vt, renderer)
    };
    let (dark, _) = shown_in(&DARK, &screen);
    same_as_screen(&dark, &screen);
    let (light, mut renderer) = shown_in(&LIGHT, &screen);
    assert_eq!(
        light.styles[0][1],
        style_key(&LIGHT.style(&Style { fg: Some(palette::FAINT), bg: Some(palette::GROUND), ..Style::default() }))
    );
    assert_eq!(
        light.styles[1][1],
        style_key(&LIGHT.style(&Style { fg: Some(palette::ACCENT), bg: Some(palette::GROUND), ..Style::default() }))
    );
    assert_ne!(light.styles[0][1], dark.styles[0][1]);
    // Unchanged in the same theme, nothing is written; in another, all of it again, in that one's colours.
    assert_eq!(renderer.frame(&screen, None), "");
    set_theme(&DARK);
    let again = renderer.frame(&screen, None);
    let mut vt = VirtualTerminal::new(12, 2);
    vt.write(&again);
    same_as_screen(&vt, &screen);
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
fn moved_bands_erased_blanks_and_links_reproduce_every_frame_exactly_and_full_repaint_writes_every_cell() {
    let mut seed = 11.0f64;
    let mut random = move || {
        seed = (seed * 1103515245.0 + 12345.0) % 2147483648.0;
        seed / 2147483648.0
    };
    let words = ["kick", "主旋律", "🎹", "Bass", "EQ Eight", "シンセ", "e\u{301}", "━━━", "·"];
    let colours: [Rgb; 4] = [palette::GROUND, palette::SURFACE, palette::ACCENT, palette::TEXT];
    let (width, height) = (30, 12);
    for full_repaint in [false, true] {
        let table = Rc::new(StyleTable::new());
        let mut renderer = Renderer::new(ColorDepth::Truecolor).full_repaint(full_repaint);
        let mut vt = VirtualTerminal::new(width, height);
        let mut screen = Screen::with_table(width, height, Rc::clone(&table));
        screen.fill(screen.bounds(), &Style::bg(palette::GROUND));
        vt.write(&renderer.frame(&screen, None));
        same_as_screen(&vt, &screen);
        let (mut scrolled, mut erased) = (0, 0);
        for _round in 0..400 {
            let mut next = Screen::with_table(width, height, Rc::clone(&table));
            next.copy_from(&screen);
            let pick = |random: &mut dyn FnMut() -> f64, count: usize| (random() * count as f64).floor() as usize;
            if random() < 0.4 {
                // A band moves up, as the conversation does while text streams in under it: new rows come in at its foot.
                let (top, bottom, by) = (pick(&mut random, 4), height as usize - pick(&mut random, 3), 1 + pick(&mut random, 3));
                if top + by < bottom {
                    let cells = |row: usize| row * width as usize;
                    next.codes.copy_within(cells(top + by)..cells(bottom), cells(top));
                    next.widths.copy_within(cells(top + by)..cells(bottom), cells(top));
                    next.styles.copy_within(cells(top + by)..cells(bottom), cells(top));
                    for y in (bottom - by) as i32..bottom as i32 {
                        next.fill(Rect::new(0, y, width, 1), &Style::bg(palette::GROUND));
                        let word = words[pick(&mut random, words.len())];
                        next.put(1 + pick(&mut random, 8) as i32, y, word, &Style::fg(colours[pick(&mut random, 4)]));
                    }
                }
            }
            for _change in 0..pick(&mut random, 4) {
                let (x, y) = (pick(&mut random, width as usize + 2) as i32 - 1, pick(&mut random, height as usize) as i32);
                let roll = random();
                if roll < 0.25 {
                    // A long run of blanks: a row cut short, or a gap in it.
                    let bg = colours[pick(&mut random, 2)];
                    next.fill(Rect::new(x, y, 8 + pick(&mut random, 24) as i32, 1), &Style::bg(bg));
                } else if roll < 0.35 {
                    let style = Style { underline: true, ..Style::fg(palette::ACCENT) }
                        .linked(["https://kumi-agent.com/a", "https://kumi-agent.com/b"][pick(&mut random, 2)]);
                    next.put(x, y, words[pick(&mut random, words.len())], &style);
                } else if roll < 0.45 {
                    // Blanks an erase doesn't leave: in a text colour, underlined, inverse.
                    let kinds = [
                        Style::fg(palette::TEXT),
                        Style { underline: true, ..Style::default() },
                        Style { inverse: true, ..Style::default() },
                    ];
                    let style = Style { bg: Some(palette::SURFACE), ..kinds[pick(&mut random, 3)] };
                    next.put(x, y, &" ".repeat(8 + pick(&mut random, 12)), &style);
                } else {
                    let bg = (random() < 0.3).then(|| colours[pick(&mut random, 4)]);
                    let style = Style { fg: Some(colours[pick(&mut random, 4)]), bg, bold: random() < 0.2, ..Style::default() };
                    next.put(x, y, words[pick(&mut random, words.len())], &style);
                }
            }
            let output = renderer.frame(&next, None);
            scrolled += usize::from(output.contains("M\u{1b}[r"));
            erased += usize::from(output.contains("\u{1b}[K") || output.contains("X\u{1b}"));
            vt.write(&output);
            same_as_screen(&vt, &next);
            // A link opened in a frame is closed before it ends.
            assert!(output.rfind("\u{1b}]8;").is_none_or(|at| output[at..].starts_with("\u{1b}]8;;")), "{output:?}");
            screen = next;
        }
        if full_repaint {
            assert_eq!((scrolled, erased), (0, 0), "every cell written, none moved or erased");
        } else {
            assert!(scrolled > 30 && erased > 30, "moved {scrolled} times, erased {erased}");
        }
        assert_eq!(renderer.frame(&screen, None), "", "an unchanged frame writes nothing");
    }
}

#[test]
fn text_streaming_under_a_header_moves_its_band_and_writes_only_the_row_that_came_in() {
    let table = Rc::new(StyleTable::new());
    let mut renderer = Renderer::new(ColorDepth::Truecolor);
    let (width, height) = (60, 20);
    let frame = |first: i32| {
        let mut screen = Screen::with_table(width, height, Rc::clone(&table));
        screen.fill(screen.bounds(), &Style::bg(palette::GROUND));
        screen.put(1, 0, "Kumi · the header", &Style::fg(palette::BRIGHT));
        for y in 2..17 {
            screen.put(2, y, &format!("line {} of the answer", first + y), &Style::fg(palette::TEXT));
        }
        screen.put(1, 18, "› the box", &Style::fg(palette::DIM));
        screen
    };
    let mut vt = VirtualTerminal::new(width, height);
    vt.write(&renderer.frame(&frame(0), None));
    for first in 1..6 {
        let next = frame(first);
        let output = renderer.frame(&next, None);
        vt.write(&output);
        same_as_screen(&vt, &next);
        // The row coming in comes in on the band's background, set before the move.
        assert!(output.contains("\u{1b}[0;48;2;14;15;18m\u{1b}[3;17r\u{1b}[3;1H\u{1b}[1M\u{1b}[r"), "{output:?}");
        assert_eq!(output.matches("line ").count(), 1, "only the row that came in: {output:?}");
        assert!(output.len() < 200, "{} bytes", output.len());
    }
    // Moved by more than a row at once, and with a row above the band changed too.
    let mut next = frame(9);
    next.put(1, 0, "Kumi · working", &Style::fg(palette::BRIGHT));
    let output = renderer.frame(&next, None);
    vt.write(&output);
    same_as_screen(&vt, &next);
    assert!(output.contains("\u{1b}[4M"), "{output:?}");
    // Where blanks aren't erased in the colours set, the rows come in on the terminal's own.
    let mut plain = Renderer::new(ColorDepth::Truecolor).erasing(false);
    let mut vt = VirtualTerminal::new(width, height);
    vt.write(&plain.frame(&frame(0), None));
    let output = plain.frame(&frame(1), None);
    vt.write(&output);
    same_as_screen(&vt, &frame(1));
    assert!(output.contains("\u{1b}[0m\u{1b}[3;17r"), "{output:?}");
}

#[test]
fn a_row_cut_short_is_erased_to_its_end_and_a_gap_in_one_go_but_blanks_that_show_are_written() {
    let table = Rc::new(StyleTable::new());
    let mut renderer = Renderer::new(ColorDepth::Truecolor);
    let (width, height) = (40, 3);
    let mut vt = VirtualTerminal::new(width, height);
    let mut screen = Screen::with_table(width, height, Rc::clone(&table));
    screen.fill(screen.bounds(), &Style::bg(palette::GROUND));
    for y in 0..height {
        screen.put(0, y, &"x".repeat(width as usize), &Style { bg: Some(palette::GROUND), ..Style::fg(palette::TEXT) });
    }
    vt.write(&renderer.frame(&screen, None));
    let mut next = Screen::with_table(width, height, Rc::clone(&table));
    next.copy_from(&screen);
    next.fill(Rect::new(5, 0, width - 5, 1), &Style::bg(palette::GROUND));
    next.fill(Rect::new(10, 1, 20, 1), &Style::bg(palette::SURFACE));
    next.put(10, 2, &" ".repeat(20), &Style { underline: true, ..Style::bg(palette::GROUND) });
    let output = renderer.frame(&next, None);
    vt.write(&output);
    same_as_screen(&vt, &next);
    assert!(output.contains("\u{1b}[1;6H\u{1b}[0;48;2;14;15;18m\u{1b}[K"), "the row's end, erased: {output:?}");
    assert!(output.contains("\u{1b}[20X"), "the gap, erased: {output:?}");
    assert!(output.contains(&format!("{}\u{1b}[0m", " ".repeat(20))), "underlined blanks, written: {output:?}");
    // Where the terminal erases in its own colours, blanks are written.
    let mut plain = Renderer::new(ColorDepth::Truecolor).erasing(false);
    plain.frame(&screen, None);
    let output = plain.frame(&next, None);
    assert!(!output.contains("\u{1b}[K") && !output.contains("X"), "{output:?}");
}

#[test]
fn linked_text_opens_its_address_and_the_link_closes_after_it() {
    let mut renderer = Renderer::new(ColorDepth::Truecolor);
    let mut screen = Screen::new(40, 2);
    let url = "https://kumi-agent.com/docs?a=1&b=2";
    let x = screen.put(1, 0, "the manual", &Style { underline: true, ..Style::fg(palette::ACCENT) }.linked(url));
    screen.put(x, 0, " after", &Style::fg(palette::TEXT));
    screen.put(1, 1, "例え", &Style::fg(palette::TEXT).linked("https://例え.jp/a b"));
    let output = renderer.frame(&screen, None);
    let opened = format!("\u{1b}]8;id=k{};{url}\u{1b}\\", link_id(url));
    let at = output.find(&opened).expect("the link opens");
    assert!(output[at..].find("the manual").unwrap() < output[at..].find("\u{1b}]8;;\u{1b}\\").unwrap(), "{output:?}");
    assert!(output.contains(";https://%E4%BE%8B%E3%81%88.jp/a%20b\u{1b}\\"), "an address past ASCII, percent-encoded: {output:?}");
    let mut vt = VirtualTerminal::new(40, 2);
    vt.write(&output);
    assert!(vt.styles[0][1].ends_with(&format!("|{url}")) && vt.styles[0][10].ends_with(&format!("|{url}")));
    assert!(!vt.styles[0][11].contains("https"), "{}", vt.styles[0][11]);
    let mut plain = Renderer::new(ColorDepth::Truecolor).linking(false);
    assert!(!plain.frame(&screen, None).contains("\u{1b}]8"));
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
async fn a_terminals_reply_is_never_typed_and_alt_keys_still_are() {
    local(async {
        // Kitty's graphics reply and a DCS report, whole or split across reads: nothing typed but what was typed after.
        // XTVERSION's report is read as the answer it is (`tui::probe` asks for it).
        assert_eq!(parse(&["\u{1b}_Gi=7;OK\u{1b}\\a"], 5.0).await, vec![text("a")]);
        assert_eq!(parse(&["\u{1b}_Gi=7;EN", "OENT:gone\u{1b}\\b"], 5.0).await, vec![text("b")]);
        assert_eq!(
            parse(&["\u{1b}P>|kitty(0.40)\u{1b}\\c"], 5.0).await,
            vec![InputEvent::Reply(Reply::Version("kitty(0.40)".into())), text("c")]
        );
        assert_eq!(parse(&["\u{1b}P1+r544e\u{7}d"], 5.0).await, vec![text("d")]);
        // Alt and a letter are keys as before, Alt-_ then G typed fast too.
        assert_eq!(parse(&["\u{1b}_"], 5.0).await, vec![key("_", ALT)]);
        assert_eq!(parse(&["\u{1b}_Gx"], 5.0).await, vec![key("_", ALT), text("Gx")]);
        assert_eq!(parse(&["\u{1b}P"], 5.0).await, vec![key("p", ALT_SHIFT)]);
    })
    .await;
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

#[tokio::test(start_paused = true)]
async fn an_animation_draws_at_its_own_pace_and_a_request_is_still_drawn_within_the_floor() {
    local(async {
        let draws = Rc::new(Cell::new(0));
        let counter = Rc::clone(&draws);
        let start = tokio::time::Instant::now();
        let clock = Box::new(move || start.elapsed().as_secs_f64() * 1000.0);
        let scheduler = FrameScheduler::with_clock(Rc::new(move || counter.set(counter.get() + 1)), 5.0, 50.0, clock);
        scheduler.set_animating(true);
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(draws.get(), 1, "the first frame comes at once");
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!((3..=4).contains(&draws.get()), "a frame every 50 ms while animating, not every 5: got {}", draws.get());
        // A key arrives between two animation frames: its frame comes within the floor, not at the next one.
        let before = draws.get();
        scheduler.request();
        tokio::time::sleep(Duration::from_millis(6)).await;
        assert_eq!(draws.get(), before + 1, "a request is drawn within 5 ms");
        scheduler.set_animating(false);
        tokio::time::sleep(Duration::from_millis(60)).await;
        let settled = draws.get();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(draws.get(), settled, "no frames once nothing moves");
        scheduler.dispose();
    })
    .await;
}

/// A terminal input fed by the test: a PassThrough with `isTTY`, `isRaw` and `setRawMode`.
struct FakeInput {
    raw: Cell<bool>,
    listener: RefCell<Option<ByteListener>>,
    /// How often what was waiting to be read was dropped.
    discarded: Cell<usize>,
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
    fn discard_pending(&self) {
        self.discarded.set(self.discarded.get() + 1);
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

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_key_in_the_lent_terminal_ends_the_wait_whole_and_kumi_closing_ends_it_too() {
    let input = Rc::new(FakeInput { raw: Cell::new(false), listener: RefCell::new(None), discarded: Cell::new(0) });
    let output = Rc::new(FakeOutput { written: RefCell::new(String::new()), resize: RefCell::new(None) });
    let events = Rc::new(RefCell::new(Vec::new()));
    let tty = Tty::new(TtyOptions {
        input: input.clone(),
        output: output.clone(),
        on_input: Rc::new({
            let events = Rc::clone(&events);
            move |event| events.borrow_mut().push(event)
        }),
        on_resize: Rc::new(|| {}),
        mouse: true,
        farewell: String::new(),
    });
    tty.start().unwrap();
    // Not lent: there's nothing to wait for.
    tty.wait_for_key().await;
    // Lent: an arrow's bytes, read in two, end the wait and go with it, raw as a program would read them.
    tty.hand_over(async {
        let wait = tty.wait_for_key();
        tokio::pin!(wait);
        assert!(futures::poll!(&mut wait).is_pending());
        assert!(input.is_raw(), "the key is read raw");
        input.write("\u{1b}[");
        input.write("A");
        wait.await;
        assert!(!input.is_raw(), "raw mode as the program found it");
    })
    .await;
    assert!(events.borrow().is_empty(), "the arrow was the wait's, not Kumi's");
    assert!(input.discarded.get() >= 2, "what was typed before and after went too");
    // Kumi closing while it waits ends the wait, and the terminal isn't taken again.
    tty.hand_over(async {
        let wait = tty.wait_for_key();
        tokio::pin!(wait);
        assert!(futures::poll!(&mut wait).is_pending());
        tty.restore(false);
        wait.await;
    })
    .await;
    assert!(!tty.lent() && !tty.is_active() && !input.is_raw());
}

#[tokio::test(flavor = "current_thread")]
async fn the_terminal_is_lent_to_a_program_and_taken_back_whole() {
    let input = Rc::new(FakeInput { raw: Cell::new(false), listener: RefCell::new(None), discarded: Cell::new(0) });
    let output = Rc::new(FakeOutput { written: RefCell::new(String::new()), resize: RefCell::new(None) });
    let events = Rc::new(RefCell::new(Vec::new()));
    let farewell = "\u{1b}]9;4;0\u{7}";
    let tty = Tty::new(TtyOptions {
        input: input.clone(),
        output: output.clone(),
        on_input: Rc::new({
            let events = Rc::clone(&events);
            move |event| events.borrow_mut().push(event)
        }),
        on_resize: Rc::new(|| {}),
        mouse: true,
        farewell: farewell.into(),
    });
    tty.start().unwrap();
    output.written.borrow_mut().clear();
    let ran = tty
        .hand_over(async {
            // The program finds the terminal as Kumi found it: every mode off, input not raw, and Kumi not reading it.
            assert_eq!(output.written.borrow().as_str(), RESTORE, "the farewell stays: Kumi is still running");
            assert!(!input.is_raw());
            input.write("x");
            assert!(tty.lent() && !tty.is_active());
            tty.write("a frame drawn meanwhile");
            "edited"
        })
        .await;
    assert_eq!(ran, "edited");
    assert!(events.borrow().is_empty(), "the program's keys never reach Kumi");
    let written = output.written.borrow().clone();
    let taken = written.strip_prefix(RESTORE).unwrap();
    assert!(taken.starts_with("\u{1b}[?1049h") && taken.contains("\u{1b}[>3u") && taken.contains("\u{1b}[?1006h"), "{taken:?}");
    assert!(input.is_raw() && tty.is_active() && !tty.lent());
    assert_eq!(input.discarded.get(), 1, "what was typed meanwhile was the program's");
    input.write("\u{1b}[A");
    assert_eq!(events.borrow().clone(), vec![key("up", NONE)]);
    // Kumi closing while it's lent gives the terminal back for good: the farewell goes, and the screen isn't taken again.
    output.written.borrow_mut().clear();
    tty.hand_over(async { tty.restore(false) }).await;
    assert_eq!(output.written.borrow().clone(), format!("{RESTORE}{farewell}"));
    assert!(!tty.is_active() && !tty.lent() && !input.is_raw());

    // With the mouse left to the terminal, the wheel's keys Kumi turned off are the program's again while it runs, and
    // off again once the terminal is taken back.
    let input = Rc::new(FakeInput { raw: Cell::new(false), listener: RefCell::new(None), discarded: Cell::new(0) });
    let output = Rc::new(FakeOutput { written: RefCell::new(String::new()), resize: RefCell::new(None) });
    let tty = Tty::new(TtyOptions {
        input: input.clone(),
        output: output.clone(),
        on_input: Rc::new(|_| {}),
        on_resize: Rc::new(|| {}),
        mouse: false,
        farewell: String::new(),
    });
    tty.start().unwrap();
    output.written.borrow_mut().clear();
    let given = format!("{RESTORE}\u{1b}[?1007h");
    tty.hand_over(async { assert_eq!(output.written.borrow().as_str(), given) }).await;
    let written = output.written.borrow().clone();
    let taken = written.strip_prefix(&given).unwrap();
    assert!(taken.contains("\u{1b}[?1007l") && !taken.contains("\u{1b}[?1006h"), "{taken:?}");
    tty.restore(false);
}

#[tokio::test(start_paused = true)]
async fn a_terminals_answers_are_read_as_answers_and_never_typed() {
    local(async {
        let answers = concat!(
            "\u{1b}P>|ghostty 1.1.3\u{1b}\\",
            "\u{1b}[?2026;2$y",
            "\u{1b}[?1u",
            "\u{1b}]10;rgb:dcdc/dcdc/dcdc\u{1b}\\",
            "\u{1b}]11;rgb:1e/1e/1e\u{7}",
            "\u{1b}[?62;22c",
        );
        assert_eq!(
            parse(&[answers], 5.0).await,
            [
                Reply::Version("ghostty 1.1.3".into()),
                Reply::Mode { mode: 2026, value: 2 },
                Reply::KittyKeys(1),
                Reply::Color { which: 10, rgb: Some([220, 220, 220]) },
                Reply::Color { which: 11, rgb: Some([30, 30, 30]) },
                Reply::Attributes("62;22".into()),
            ]
            .map(InputEvent::Reply)
        );
        // A late answer, split between reads, is never typed, and its BEL is never ctrl+g.
        assert_eq!(
            parse(&["x\u{1b}]11;rgb:ff", "ff/ffff/ffff\u{7}y"], 5.0).await,
            [text("x"), InputEvent::Reply(Reply::Color { which: 11, rgb: Some([255, 255, 255]) }), text("y")]
        );
        // An answer Kumi didn't ask for is read and dropped; Alt-] on its own is still Alt-], after the Escape delay.
        assert_eq!(parse(&["\u{1b}]4;1;rgb:00/00/00\u{1b}\\z"], 5.0).await, [text("z")]);
        assert_eq!(parse(&["\u{1b}]"], 5.0).await, [key("]", Modifiers::ALT)]);
        // An answer under way is waited for longer than the Escape delay between its reads (a slow SSH link), so its head
        // isn't dropped and its tail typed; past that wait, what came of it is dropped, never typed.
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&events);
        let parser = InputParser::with_escape_delay(Rc::new(move |event| sink.borrow_mut().push(event)), 5.0);
        parser.push("\u{1b}]11;rgb:ff");
        tokio::time::sleep(Duration::from_millis(100)).await;
        parser.push("ff/ffff/ffff\u{7}z");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(events.take(), [InputEvent::Reply(Reply::Color { which: 11, rgb: Some([255, 255, 255]) }), text("z")]);
        parser.push("\u{1b}P>|kit");
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(events.take().is_empty());
        parser.dispose();
    })
    .await;
}

/// A terminal that answers Kumi's questions as they're written: what's typed first, then its answers.
struct Answering {
    input: Rc<FakeInput>,
    answers: &'static str,
    written: RefCell<String>,
}

impl TtyOutput for Answering {
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
        if data.contains("\u{1b}[c") {
            self.input.write(self.answers);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn the_terminal_is_asked_once_its_last_answer_ends_the_wait_and_keys_typed_meanwhile_are_kept() {
    use kumi::tui::probe::{probe, Answers, QUERIES};
    local(async {
        let input = Rc::new(FakeInput { raw: Cell::new(false), listener: RefCell::new(None), discarded: Cell::new(0) });
        let output = Rc::new(Answering {
            input: input.clone(),
            answers: "hi\u{1b}P>|kitty(0.35.2)\u{1b}\\\u{1b}[?1u\u{1b}]11;rgb:0000/0000/0000\u{1b}\\\u{1b}[?62c",
            written: RefCell::default(),
        });
        let started = tokio::time::Instant::now();
        let probed = probe(&(input.clone() as Rc<dyn TtyInput>), &(output.clone() as Rc<dyn TtyOutput>), Duration::from_secs(5)).await;
        assert!(started.elapsed() < Duration::from_millis(100), "DA1's answer ended the wait");
        assert_eq!(output.written.borrow().as_str(), QUERIES);
        assert_eq!(
            probed.answers,
            Answers {
                version: Some("kitty(0.35.2)".into()),
                kitty_keys: true,
                background: Some([0, 0, 0]),
                complete: true,
                ..Answers::default()
            }
        );
        assert!(!input.is_raw() && input.listener.borrow().is_none(), "the terminal is put back as it was");
        // Read again as the screen starts, what was typed is typed, and the answers aren't.
        let events = Rc::new(RefCell::new(Vec::new()));
        let tty = Tty::new(TtyOptions {
            input: input.clone(),
            output: Rc::new(FakeOutput { written: RefCell::new(String::new()), resize: RefCell::new(None) }),
            on_input: Rc::new({
                let events = Rc::clone(&events);
                move |event| events.borrow_mut().push(event)
            }),
            on_resize: Rc::new(|| {}),
            mouse: true,
            farewell: String::new(),
        });
        tty.start().unwrap();
        tty.read_again(&probed.read);
        let typed: Vec<_> = events.borrow().iter().filter(|event| !matches!(event, InputEvent::Reply(_))).cloned().collect();
        assert_eq!(typed, [text("hi")]);
        tty.restore(false);
        // A terminal that doesn't answer is waited for no longer than the budget, and taken at its environment's word.
        let quiet = Rc::new(FakeInput { raw: Cell::new(false), listener: RefCell::new(None), discarded: Cell::new(0) });
        let started = tokio::time::Instant::now();
        let output: Rc<dyn TtyOutput> = Rc::new(FakeOutput { written: RefCell::new(String::new()), resize: RefCell::new(None) });
        let probed = probe(&(quiet.clone() as Rc<dyn TtyInput>), &output, Duration::from_millis(250)).await;
        assert!(started.elapsed() >= Duration::from_millis(250) && started.elapsed() < Duration::from_millis(300));
        assert_eq!(probed.answers, Answers::default());
        assert!(!quiet.is_raw());
    })
    .await;
}

#[test]
fn what_the_terminal_says_of_itself_comes_before_its_environment() {
    use kumi::tui::alerts::{detect_terminal_profile, NotifyChannel};
    use kumi::tui::probe::Answers;
    let env = |pairs: &[(&str, &str)]| pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect::<HashMap<_, _>>();
    let said =
        |version: &str| Answers { version: Some(version.into()), background: Some([30, 30, 30]), complete: true, ..Answers::default() };
    // Ghostty over SSH: TERM_PROGRAM doesn't arrive, so its environment names no terminal.
    let ssh = env(&[("TERM", "xterm-256color"), ("SSH_CONNECTION", "10.0.0.2 52000 10.0.0.9 22")]);
    let before = detect_terminal_profile(&ssh, "linux");
    assert!(!before.images && before.notify == NotifyChannel::None);
    let after = before.answered(&said("ghostty 1.1.3"), &ssh, "linux");
    assert!(after.images && after.progress && after.notify == NotifyChannel::Osc777);
    assert!(after.hyperlinks && !detect_terminal_profile(&ssh, "linux").hyperlinks, "links Ghostty opens itself, over SSH too");
    assert_eq!((after.version.as_deref(), after.background), (Some("ghostty 1.1.3"), Some([30, 30, 30])));
    assert_eq!(answered_color_depth(ColorDepth::Colors256, Some("ghostty 1.1.3"), &ssh), ColorDepth::Truecolor);
    let kitty = detect_terminal_profile(&ssh, "linux").answered(&said("kitty(0.35.2)"), &ssh, "linux");
    assert!(kitty.images && kitty.notify == NotifyChannel::Osc99);
    let iterm = detect_terminal_profile(&ssh, "linux").answered(&said("iTerm2 3.5.10"), &ssh, "linux");
    assert!(!iterm.images && iterm.notify == NotifyChannel::Osc9);
    // Its own selection is Option-drag, which the hint for copying says.
    assert!(iterm.iterm && !detect_terminal_profile(&ssh, "linux").iterm);
    // tmux answers for itself, even where its environment doesn't say tmux: no pictures, the system's notification,
    // and the environment's colour depth.
    let tmux = detect_terminal_profile(&ssh, "darwin").answered(&said("tmux 3.5a"), &ssh, "darwin");
    assert!(tmux.tmux && !tmux.images && !tmux.progress && tmux.notify == NotifyChannel::AppleScript);
    assert_eq!(answered_color_depth(ColorDepth::Colors256, Some("tmux 3.5a"), &ssh), ColorDepth::Colors256);
    // Inside a multiplexer, what answers for the terminal outside turns no pictures on.
    let screen = env(&[("TERM", "screen-256color")]);
    assert!(!detect_terminal_profile(&screen, "linux").answered(&said("kitty(0.35.2)"), &screen, "linux").images);
    // A name Kumi doesn't know, and no answer at all, leave the environment's word.
    let local = env(&[("TERM_PROGRAM", "iTerm.app"), ("TERM", "xterm-256color")]);
    let before = detect_terminal_profile(&local, "darwin");
    let after = before.clone().answered(&said("SomeTerm 1.0"), &local, "darwin");
    assert_eq!((after.notify, after.images, after.progress), (before.notify, before.images, before.progress));
    assert_eq!(before.clone().answered(&Answers::default(), &local, "darwin"), before);
    // A depth the environment forces stays; a terminal it keeps quiet stays quiet.
    assert_eq!(answered_color_depth(ColorDepth::None, Some("kitty(0.35.2)"), &env(&[("NO_COLOR", "1")])), ColorDepth::None);
    let dumb = env(&[("TERM", "dumb")]);
    let after = detect_terminal_profile(&dumb, "linux").answered(&said("kitty(0.35.2)"), &dumb, "linux");
    assert!(!after.status && !after.images && after.notify == NotifyChannel::None);
}

#[test]
fn the_terminal_is_taken_over_and_always_given_back_even_twice() {
    let input = Rc::new(FakeInput { raw: Cell::new(false), listener: RefCell::new(None), discarded: Cell::new(0) });
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
        farewell: String::new(),
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
