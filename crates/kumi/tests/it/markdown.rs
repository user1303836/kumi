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
            "| Track | Device |",
            "| Bass | EQ Eight |",
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
        // Only markdown markers may go; a line without any comes back whole.
        assert!(text.len() * 10 >= line.len() * 9, "{} of {}", text.len(), line.len());
        if !line.contains(['*', '_', '`', '[']) {
            assert_eq!(text, line);
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
        let streamed = render_markdown_with(&whole[..end], 24, &base(), &mut cache);
        assert_eq!(rows_with_bands(&streamed), rows_with_bands(&render_markdown(&whole[..end], 24, &base())), "at {end} bytes");
        // Everything up to the last newline is kept; only the line after it is rendered per call.
        assert_eq!(cache.settled(), whole[..end].rfind('\n').map_or(0, |at| at + 1), "at {end} bytes");
    }
    // A text that changed before its end, or another width, starts over and still matches; so does one whose
    // characters no longer end where the kept text's lines did.
    let changed = "# Remix notes\n\nSome **bold** words\nlast line";
    assert_eq!(
        rows_with_bands(&render_markdown_with(changed, 24, &base(), &mut cache)),
        rows_with_bands(&render_markdown(changed, 24, &base()))
    );
    let mut shifted = MarkdownCache::default();
    render_markdown_with("ab\ncd", 24, &base(), &mut shifted);
    let other = "a€\nmore\nx";
    assert_eq!(
        rows_with_bands(&render_markdown_with(other, 24, &base(), &mut shifted)),
        rows_with_bands(&render_markdown(other, 24, &base()))
    );
    assert_eq!(
        rows_with_bands(&render_markdown_with(whole, 40, &base(), &mut cache)),
        rows_with_bands(&render_markdown(whole, 40, &base()))
    );
    // An open fence carries from the kept lines into the one still growing.
    let open = "before\n```\ncode so far";
    assert_eq!(rows_with_bands(&render_markdown_with(open, 24, &base(), &mut cache)), rows_with_bands(&render_markdown(open, 24, &base())));
    assert!(render_markdown_with(open, 24, &base(), &mut cache).last().unwrap().bg.is_some());
}
