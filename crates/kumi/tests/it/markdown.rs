use std::rc::Rc;

use kumi::tui::markdown::{inline, render_markdown, render_markdown_with, MarkdownCache, MarkdownRow};
use kumi::tui::style::{palette, Style};

fn base() -> Rc<Style> {
    Rc::new(Style::fg(palette::TEXT))
}

fn text(rows: &[MarkdownRow]) -> Vec<String> {
    rows.iter().map(|row| row.spans.iter().map(|span| span.text.as_str()).collect()).collect()
}

#[test]
fn inline_markdown_becomes_styles_and_names_with_underscores_or_lone_asterisks_stay_literal() {
    let spans = inline("**Bass** needs *less* `EQ Eight` at [200 Hz](https://example.com/eq)", &base());
    assert_eq!(
        spans.iter().map(|span| span.text.as_str()).collect::<Vec<_>>(),
        vec!["Bass", " needs ", "less", " ", "EQ Eight", " at ", "200 Hz", " (https://example.com/eq)"]
    );
    assert!(spans[0].style.bold);
    assert!(spans[2].style.italic);
    assert_eq!(spans[4].style.bg, Some(palette::RAISED));
    assert!(spans[6].style.underline);
    assert_eq!(
        inline("Kick_01_final and 2*3*4 and C* minor", &base()).iter().map(|span| span.text.as_str()).collect::<Vec<_>>(),
        vec!["Kick_01_final and 2*3*4 and C* minor"]
    );
    assert_eq!(inline("_really_ loud", &base()).iter().map(|span| span.style.italic).collect::<Vec<_>>(), vec![true, false]);
}

#[test]
fn lists_get_bullets_and_hanging_indents_headings_quotes_and_code_render_without_their_markers() {
    let rows = render_markdown(
        &[
            "## Low end",
            "- The kick and bass are fighting around 200 Hz",
            "  - nested point",
            "2. Then tighten the compressor",
            "> Names are data",
            "```js",
            "const gain = -3;",
            "```",
            "---",
            "| Track | Device |",
            "|---|---|",
            "| Bass | EQ Eight |",
        ]
        .join("\n"),
        24,
        &base(),
    );
    assert_eq!(
        text(&rows),
        vec![
            "Low end",
            "• The kick and bass are",
            "  fighting around 200 Hz",
            "  • nested point",
            "2. Then tighten the",
            "   compressor",
            "│ Names are data",
            "const gain = -3;",
            "────────────────────────",
            "Track  Device",
            "─────  ────────",
            "Bass   EQ Eight",
        ]
    );
    assert!(rows[0].spans[0].style.bold);
    assert_eq!(rows[7].bg, Some(palette::RAISED), "code blocks sit on a band");
    assert_eq!(rows[1].bg, None);
}

#[test]
fn a_long_code_line_splits_between_characters_instead_of_wrapping_words() {
    let rows = render_markdown("```\nabcdefghij\n```", 4, &base());
    assert_eq!(text(&rows), vec!["abcd", "efgh", "ij"]);
}

#[test]
fn an_unfinished_fence_while_streaming_shows_the_code_so_far() {
    assert_eq!(text(&render_markdown("Try this:\n```\nlet x", 20, &base())), vec!["Try this:", "let x"]);
}

#[test]
fn lines_past_the_backtrack_limit_render_plain_instead_of_freezing() {
    // An inline JSON answer with _id keys, a line of *.wav globs and a 200k-character line: each once
    // froze the TUI for good. Rendered in a thread, so a regression fails here rather than hanging.
    let json = format!(
        "[{}]",
        (0..160).map(|n| format!(r#"{{"_id":"clip_{n}","name":"Kick_{n:02}_final","gain":-6}}"#)).collect::<Vec<_>>().join(",")
    );
    let globs = (0..600).map(|n| format!("*{n}.wav")).collect::<Vec<_>>().join(" ");
    let long = "plain words, no markdown ".repeat(8_000);
    for line in [json, globs, long] {
        let (done, rendered) = std::sync::mpsc::channel();
        let sent = line.clone();
        std::thread::spawn(move || {
            let spans = inline(&sent, &base());
            let _ = done.send(spans.iter().map(|span| span.text.as_str()).collect::<String>());
        });
        let text = rendered
            .recv_timeout(std::time::Duration::from_secs(20))
            .unwrap_or_else(|_| panic!("froze on a {}-character line", line.len()));
        // Only markdown markers may go; a line without any comes back whole (but for the spaces at its end, which
        // Markdown leaves out).
        assert!(text.len() * 10 >= line.len() * 9, "{} of {}", text.len(), line.len());
        if !line.contains(['*', '_', '`', '[']) {
            assert_eq!(text, line.trim_end());
        }
    }
}

fn rows_with_bands(rows: &[MarkdownRow]) -> Vec<(String, bool)> {
    rows.iter().map(|row| (row.spans.iter().map(|span| span.text.as_str()).collect(), row.bg.is_some())).collect()
}

#[test]
fn an_answer_that_streams_renders_as_the_whole_would_while_only_its_last_line_is_rendered_again() {
    let whole = "# Mix notes\n\nSome **bold** words and `code`\n```\nlet gain = 0.5;\nlet pan = -0.2;\n```\n- item one\n- item two with a longer tail that wraps\n> a quote\nlast line";
    let mut cache = MarkdownCache::default();
    for end in 1..=whole.len() {
        let streamed = render_markdown_with(&whole[..end], 24, &base(), &mut cache, false);
        assert_eq!(rows_with_bands(&streamed), rows_with_bands(&render_markdown(&whole[..end], 24, &base())), "at {end} bytes");
        // What's kept never reaches past the last newline: only the last block, and the line still coming, are
        // rendered again.
        assert!(cache.settled() <= whole[..end].rfind('\n').map_or(0, |at| at + 1), "at {end} bytes");
    }
    // By the end, every block but the quote's is kept.
    assert_eq!(cache.settled(), whole.find("> a quote").unwrap());
    // A text that changed before its end, or another width, starts over and still matches; so does one whose
    // characters no longer end where the kept text's lines did.
    let changed = "# Remix notes\n\nSome **bold** words\nlast line";
    assert_eq!(
        rows_with_bands(&render_markdown_with(changed, 24, &base(), &mut cache, false)),
        rows_with_bands(&render_markdown(changed, 24, &base()))
    );
    let mut shifted = MarkdownCache::default();
    render_markdown_with("ab\ncd", 24, &base(), &mut shifted, false);
    let other = "a€\nmore\nx";
    assert_eq!(
        rows_with_bands(&render_markdown_with(other, 24, &base(), &mut shifted, false)),
        rows_with_bands(&render_markdown(other, 24, &base()))
    );
    assert_eq!(
        rows_with_bands(&render_markdown_with(whole, 40, &base(), &mut cache, false)),
        rows_with_bands(&render_markdown(whole, 40, &base()))
    );
    // An open fence carries from the kept lines into the one still growing.
    let open = "before\n```\ncode so far";
    assert_eq!(
        rows_with_bands(&render_markdown_with(open, 24, &base(), &mut cache, false)),
        rows_with_bands(&render_markdown(open, 24, &base()))
    );
    assert!(render_markdown_with(open, 24, &base(), &mut cache, false).last().unwrap().bg.is_some());
}

#[test]
fn a_table_is_laid_out_to_the_width_with_numbers_to_the_right() {
    let table = "| Band | Gain | Note |\n|---|---|---|\n| Low | -3 dB | warm |\n| Presence | 1.5 dB | **bite** |";
    let rows = render_markdown(table, 40, &base());
    assert_eq!(text(&rows), vec!["Band        Gain  Note", "────────  ──────  ────", "Low        -3 dB  warm", "Presence  1.5 dB  bite"]);
    // The heads are bold; a cell keeps its own style.
    assert!(rows[0].spans.iter().filter(|span| !span.text.trim().is_empty()).all(|span| span.style.bold));
    assert!(rows[3].spans.iter().any(|span| span.text == "bite" && span.style.bold));
    // Squeezed, a long cell wraps inside its column.
    let long = "| Track | What changed |\n|---|---|\n| Bass | a lowpass at 120 Hz and less drive |";
    assert_eq!(
        text(&render_markdown(long, 26, &base())),
        vec!["Track  What changed", "─────  ───────────────────", "Bass   a lowpass at 120 Hz", "       and less drive"]
    );
}

#[test]
fn a_table_too_wide_for_its_columns_shows_a_row_at_a_time() {
    let table = "| Track | Device | Gain |\n|---|---|---|\n| Bass | EQ Eight | -3 dB |\n| Kick | Drum Buss | 0 dB |";
    assert_eq!(
        text(&render_markdown(table, 18, &base())),
        vec!["Track: Bass", "Device: EQ Eight", "Gain: -3 dB", "", "Track: Kick", "Device: Drum Buss", "Gain: 0 dB"]
    );
}

#[test]
fn a_table_still_streaming_is_held_back_until_it_ends() {
    let mut cache = MarkdownCache::default();
    let partial = "Levels:\n\n| Track | Gain |\n|---|---|\n| Bass | -3 |\n| Kick";
    assert_eq!(text(&render_markdown_with(partial, 40, &base(), &mut cache, true)), vec!["Levels:", "", "table · 2 rows so far"]);
    // Once something comes after it, or the answer is done, it's laid out.
    let done = "Levels:\n\n| Track | Gain |\n|---|---|\n| Bass | -3 |\n| Kick | 0 |\n\nThat's all.";
    assert_eq!(
        text(&render_markdown_with(done, 40, &base(), &mut cache, true)),
        vec!["Levels:", "", "Track  Gain", "─────  ────", "Bass     -3", "Kick      0", "", "That's all."]
    );
    assert_eq!(text(&render_markdown_with(partial, 40, &base(), &mut MarkdownCache::default(), false))[2], "Track  Gain");
}

#[test]
fn a_star_inside_a_word_and_a_long_line_stay_text_and_cost_their_length() {
    assert_eq!(text(&render_markdown("2*3*4 and foo*bar* but *this*", 40, &base())), vec!["2*3*4 and foo*bar* but this"]);
    let rows = render_markdown("2*3*4 and foo*bar* but *this*", 40, &base());
    assert!(rows[0].spans.iter().any(|span| span.text == "this" && span.style.italic));
    // Forty thousand markers that close nothing render in a moment.
    let started = std::time::Instant::now();
    let markers = "*a _b `c [d ".repeat(10_000);
    let rows = render_markdown(&markers, 80, &base());
    assert!(!rows.is_empty() && started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
}

#[test]
fn wrapped_rows_say_how_they_carry_on_for_a_copy_to_join_them() {
    use kumi::tui::wrap::Carry;
    // A list item's words wrap under its marker: a space joins them, past the two cells the marker's room takes.
    let rows = render_markdown("- The kick and bass\n\n```\nabcdefghijklmnopqrst\n```", 16, &base());
    let carries: Vec<_> = rows.iter().map(|row| row.carry).collect();
    assert_eq!(
        carries,
        vec![None, Some(Carry { lead: 2, space: true }), None, None, Some(Carry { lead: 0, space: false }),],
        "{:?}",
        text(&rows)
    );
}
