use kumi::tui::{style::Style, transcript::*, wrap::Span};
use serde_json::{json, Value};
fn fixture() -> Value {
    serde_json::from_str(include_str!("../support/transcript/reference.json")).unwrap()
}
fn style(s: &Style) -> Value {
    let mut v = json!({});
    if let Some(fg) = s.fg {
        v["fg"] = json!(fg)
    }
    if let Some(bg) = s.bg {
        v["bg"] = json!(bg)
    }
    for (k, b) in [("bold", s.bold), ("dim", s.dim), ("italic", s.italic), ("underline", s.underline), ("inverse", s.inverse)] {
        if b {
            v[k] = json!(true)
        }
    }
    v
}
fn span(s: &Span) -> Value {
    json!({"text":s.text,"style":style(&s.style)})
}
fn normalize(value: Value) -> Value {
    serde_json::from_str(&kumi_common::js::json::stringify(&value)).unwrap()
}
fn rows(rows: &[Row]) -> Value {
    normalize(json!(rows
        .iter()
        .map(|r| {
            let mut v = json!({"spans":r.spans.iter().map(span).collect::<Vec<_>>()});
            if let Some(band) = &r.band {
                v["band"] = json!({"bg":band.bg,"width":band.width});
            }
            if let Some(trailing) = &r.trailing {
                v["trailing"] = span(trailing);
            }
            if let Some(live) = &r.live {
                v["live"] = match live {
                    LiveRow::Step { activity, since, label, doing } => {
                        let mut v = json!({"kind":"step","activity":activity.as_str(),"since":since,"label":label});
                        if let Some(doing) = doing {
                            v["doing"] = json!(doing);
                        }
                        v
                    }
                    LiveRow::Header { label, since } => {
                        let mut v = json!({"kind":"header","label":label});
                        if let Some(since) = since {
                            v["since"] = json!(since);
                        }
                        v
                    }
                };
            }
            v
        })
        .collect::<Vec<_>>()))
}
#[test]
fn all_entry_kinds_match_complete_source_rows_at_three_widths() {
    let f = fixture();
    for (index, case) in f["cases"].as_array().unwrap().iter().enumerate() {
        let mut t = Transcript::default();
        t.add(serde_json::from_value(case["entry"].clone()).unwrap());
        assert_eq!(
            rows(&t.rows(case["width"].as_i64().unwrap() as i32, case["now"].as_f64().unwrap())),
            case["rows"],
            "case {index}: {}",
            case["entry"]
        );
    }
}
#[test]
fn repeated_steps_match_source_folding_frames_deadlines_and_cache() {
    let f = fixture();
    let mut t = Transcript::default();
    t.add(serde_json::from_value(f["original"].clone()).unwrap());
    for case in f["folding"].as_array().unwrap() {
        let now = case["now"].as_f64().unwrap();
        assert_eq!(normalize(json!(t.change_at(now))), case.get("before").cloned().unwrap_or(Value::Null), "before {now}");
        assert_eq!(rows(&t.rows(60, now)), case["rows"], "rows {now}");
        assert_eq!(normalize(json!(t.change_at(now))), case.get("after").cloned().unwrap_or(Value::Null), "after {now}");
        assert_eq!(t.laid_out, case["laidOut"].as_u64().unwrap() as usize);
    }
}
#[test]
fn half_block_picture_colors_match_source_area_averages() {
    for case in fixture()["pictures"].as_array().unwrap() {
        let picture = serde_json::from_value(case["picture"].clone()).unwrap();
        let result = picture_rows(&picture, case["cells"].as_u64().unwrap() as usize);
        assert_eq!(json!(result.iter().map(|r| r.iter().map(span).collect::<Vec<_>>()).collect::<Vec<_>>()), case["rows"]);
    }
}
#[test]
fn labels_match_source_and_fallbacks() {
    for case in fixture()["labels"].as_array().unwrap() {
        let tool = case["tool"].as_str().unwrap();
        assert_eq!(step_label(tool), case["label"]);
        assert_eq!(doing_label(Some(tool), "fallback"), case["doing"]);
    }
    assert_eq!(doing_label(None, "fallback"), "fallback");
}
fn step(label: &str, ended_at: Option<f64>) -> Step {
    Step {
        id: format!("{label}:{ended_at:?}"),
        label: label.into(),
        state: StepState::Done,
        ms: Some(300.),
        ended_at,
        ..Default::default()
    }
}
fn lines(t: &mut Transcript, now: f64) -> Vec<String> {
    t.rows(60, now).iter().map(|r| r.spans.iter().map(|s| s.text.as_str()).collect::<String>().trim_end().to_string()).collect()
}
#[test]
fn original_repeated_step_scenario_folds_later_arrivals() {
    let mut t = Transcript::default();
    let e = t.add(Entry::Assistant {
        text: "".into(),
        status: AnswerStatus::Running,
        started_at: Some(0.),
        elapsed_ms: None,
        thinking: None,
        footer: None,
        steps: vec![
            step("searched the web", Some(900.)),
            step("read a page", Some(1000.)),
            step("read a page", Some(1100.)),
            step("read a page", Some(1200.)),
            Step { state: StepState::Error, ..step("made a device", Some(1300.)) },
            step("made a device", Some(1400.)),
        ],
    });
    assert_eq!(lines(&mut t, 4199.).iter().filter(|s| s.contains("read a page")).count(), 3);
    assert_eq!(t.change_at(1300.), Some(4200.));
    assert_eq!(t.change_at(4300.), Some(4300.));
    assert_eq!(lines(&mut t, 5300.).iter().filter(|s| s.contains("read a page")).collect::<Vec<_>>(), ["│ ✓ read a page ×3"]);
    assert!(t.rows(60, 5300.).iter().any(|r| r.trailing.as_ref().is_some_and(|s| s.text == "0.9s")));
    assert_eq!(t.change_at(5300.), None);
    if let Entry::Assistant { steps, .. } = &mut *e.borrow_mut() {
        steps.insert(4, step("read a page", Some(10000.)));
    }
    t.touch(&e);
    assert_eq!(lines(&mut t, 10500.).iter().filter(|s| s.contains("read a page")).count(), 2);
    assert_eq!(lines(&mut t, 14000.).iter().filter(|s| s.contains("read a page")).collect::<Vec<_>>(), ["│ ✓ read a page ×4"]);
}
#[test]
fn original_running_and_resumed_steps_scenario() {
    let mut t = Transcript::default();
    t.add(Entry::Assistant {
        text: "".into(),
        status: AnswerStatus::Running,
        started_at: Some(0.),
        elapsed_ms: None,
        thinking: None,
        footer: None,
        steps: vec![
            step("read a page", Some(100.)),
            Step { state: StepState::Running, tool: Some("read_web".into()), started_at: Some(200.), ..step("read a page", None) },
        ],
    });
    assert_eq!(t.rows(60, 10000.).iter().filter(|r| matches!(r.live, Some(LiveRow::Step { .. }))).count(), 1);
    let mut t = Transcript::default();
    t.add(Entry::Assistant {
        text: "Done.".into(),
        status: AnswerStatus::Done,
        started_at: None,
        elapsed_ms: None,
        thinking: None,
        footer: None,
        steps: vec![step("looked at your Set", None), step("looked at your Set", None), step("made changes", None)],
    });
    let shown = lines(&mut t, 5.);
    assert!(shown.contains(&"│ ✓ looked at your Set ×2".into()) && shown.contains(&"│ ✓ made changes".into()));
}
#[test]
fn transcript_insertion_mutation_and_cached_layouts() {
    let mut t = Transcript::default();
    assert!(t.is_empty());
    let first = t.add(Entry::User { text: "a".into() });
    let last = t.add(Entry::Notice { text: "c".into(), tone: NoticeTone::Info });
    let mid = t.insert_before(Entry::Divider { text: "b".into() }, Some(&last));
    t.rows(40, 0.);
    assert_eq!(t.laid_out, 3);
    t.rows(40, 100.);
    assert_eq!(t.laid_out, 3);
    *first.borrow_mut() = Entry::User { text: "changed".into() };
    t.touch(&first);
    t.rows(40, 100.);
    assert_eq!(t.laid_out, 4);
    t.remove(&mid);
    assert_eq!(t.entries.len(), 2);
    t.rows(20, 100.);
    assert_eq!(t.laid_out, 6);
    t.clear();
    assert!(t.is_empty());
    assert!(t.rows(40, 100.).is_empty());
}
#[test]
fn a_frames_layout_borrows_the_cached_rows_with_the_same_gaps() {
    let mut t = Transcript::default();
    // An answer that hasn't said anything yet has no rows: what follows it gets no gap before it.
    let empty = || Entry::Assistant {
        text: "".into(),
        status: AnswerStatus::Done,
        started_at: None,
        elapsed_ms: None,
        steps: vec![],
        thinking: None,
        footer: None,
    };
    t.add(empty());
    t.add(Entry::User { text: "a".into() });
    t.add(empty());
    t.add(Entry::Notice { text: "c".into(), tone: NoticeTone::Info });
    assert!(kumi::tui::transcript::entry_rows(&empty(), 40, 0.).is_empty());
    // The rows as each entry's were once gathered, copied one after another.
    let mut gathered = vec![];
    for entry in &t.entries {
        if !gathered.is_empty() {
            gathered.push(Row::default());
        }
        gathered.extend(kumi::tui::transcript::entry_rows(&entry.borrow(), 40, 0.));
    }
    let layout = t.layout(40, 0.);
    assert_eq!(layout.len(), gathered.len());
    assert_eq!(
        layout.iter().map(|row| format!("{row:?}")).collect::<Vec<_>>(),
        gathered.iter().map(|row| format!("{row:?}")).collect::<Vec<_>>()
    );
    // The next frame lays out nothing again and copies nothing: each entry's rows are the cache's own.
    let laid = t.laid_out;
    let again = t.layout(40, 0.);
    assert_eq!(t.laid_out, laid);
    let shared = layout.iter().zip(again.iter()).filter(|(a, b)| std::ptr::eq(*a, *b)).count();
    assert_eq!(shared, layout.iter().filter(|row| !row.spans.is_empty()).count());
}
#[test]
fn printed_for_the_scrollback_keeps_requests_answers_and_a_line_for_each_run_of_steps() {
    let mut t = Transcript::default();
    t.add(Entry::Notice { text: "Continuing your conversation from yesterday.".into(), tone: NoticeTone::Info });
    t.add(Entry::User { text: "what's on the bass?\nand the kick?".into() });
    let mut failed = step("listened", Some(3.));
    failed.state = StepState::Error;
    t.add(Entry::Assistant {
        text: "**Bass** has an EQ Eight.\n".into(),
        steps: vec![step("looked at your Set", Some(1.)), step("looked at your Set", Some(2.)), failed],
        status: AnswerStatus::Done,
        elapsed_ms: Some(900.),
        started_at: None,
        thinking: None,
        footer: None,
    });
    t.add(Entry::Assistant {
        text: String::new(),
        steps: vec![],
        status: AnswerStatus::Stopped,
        elapsed_ms: None,
        started_at: None,
        thinking: None,
        footer: None,
    });
    let all = printed(&t.entries, PRINTED_MOST, false);
    assert_eq!(
        all,
        "\x1b[1m› what's on the bass?\x1b[22m\n\x1b[1m  and the kick?\x1b[22m\n\n**Bass** has an EQ Eight.\n\
         \x1b[2m  ✓ looked at your Set ×2\x1b[22m\n\x1b[2m  × listened\x1b[22m\n\n\x1b[2mstopped\x1b[22m\n\n"
    );
    // Past the most it keeps, the end, from a line's start, after a line that says so; a cut inside a character is safe.
    for most in [40, 41, 42, 43] {
        let end = printed(&t.entries, most, false);
        assert!(end.starts_with("\x1b[2m… the start of this conversation is left out here\x1b[22m\n\n"), "{end:?}");
        assert!(all.ends_with(end.split_once("here\x1b[22m\n\n").unwrap().1), "{end:?}");
    }
    assert_eq!(printed(&[], PRINTED_MOST, false), "");
    // Where the terminal's symbols may not show, the marks are plain.
    assert_eq!(
        printed(&t.entries, PRINTED_MOST, true),
        "\x1b[1m> what's on the bass?\x1b[22m\n\x1b[1m  and the kick?\x1b[22m\n\n**Bass** has an EQ Eight.\n\
         \x1b[2m  + looked at your Set x2\x1b[22m\n\x1b[2m  x listened\x1b[22m\n\n\x1b[2mstopped\x1b[22m\n\n"
    );
    assert!(printed(&t.entries, 40, true).starts_with("\x1b[2m... the start of this conversation is left out here"));
}

#[test]
fn a_reasonings_headline_is_its_latest_title_or_its_last_line_without_markdown() {
    assert_eq!(headline(""), "");
    assert_eq!(headline("\n\n  \n"), "");
    assert_eq!(headline("**Weighing the kick**\n\nThe sub rings 60 ms past each hit."), "Weighing the kick");
    // Each part of a summary begins with its own: the latest one's.
    assert_eq!(headline("**Weighing the kick**\n\nThe sub rings.\n\n**Checking the `sidechain`**\n\nThe ratio"), "Checking the sidechain");
    // Untitled, as a model's own thinking runs: its last line that says anything.
    assert_eq!(headline("The kick sits at 55 Hz.\nThe bass at 48 Hz,\n"), "The bass at 48 Hz,");
    assert_eq!(headline("## Planning the change\n- set the ratio"), "Planning the change");
    assert_eq!(headline("- **set** the `ratio`   to 4:1\n\n"), "set the ratio to 4:1");
    // A sign isn't a list's mark.
    assert_eq!(headline("-3 dB on the low shelf should do it"), "-3 dB on the low shelf should do it");
    assert_eq!(headline("> - *quoted*, listed"), "*quoted*, listed");
}

fn heard(bands: Vec<f64>) -> Entry {
    Entry::Heard { file: "mix.wav".into(), summary: "−12.1 LUFS".into(), bands, compared: None }
}
fn texts(rows: &[Row]) -> Vec<String> {
    rows.iter().map(|row| row.spans.iter().map(|span| span.text.as_str()).collect::<String>()).collect()
}
/// A darker balance, as the analysis gives it: each band's share of the whole, dB.
const DARK_MIX: [f64; 10] = [-8., -5., -7., -9., -10., -12., -13., -16., -18., -22.];

#[test]
fn one_file_heard_is_an_octave_analyzer_where_pink_noise_draws_flat() {
    // Pink noise has the same energy in every octave: each band's share is its width in octaves over all ten's.
    let octaves = |band: &kumi_media::audio::analyze::Band| (band.to / band.from).log2();
    let all: f64 = kumi_media::audio::BANDS.iter().map(octaves).sum();
    let pink = kumi_media::audio::BANDS.iter().map(|band| 10. * (octaves(band) / all).log10()).collect();
    let lines = texts(&entry_rows(&heard(pink), 75, 0.));
    assert_eq!(lines[..2], ["Heard mix.wav · −12.1 LUFS", "energy per octave, dB under the loudest band"]);
    // Every bar as high, half a row into the top one (0 dB is its middle), and as wide as its band on the log scale:
    // sub (1.6 octaves) wider than bass (1), bass than bite (0.6).
    assert!(lines[2].starts_with("   0 "), "{lines:#?}");
    let tops: Vec<&str> = lines[2][5..].split('─').filter(|run| !run.is_empty()).collect();
    assert_eq!(tops.len(), 10, "{lines:#?}");
    assert!(tops.iter().all(|run| run.chars().all(|c| c == '▄')), "{tops:?}");
    let wide = |run: &str| run.chars().count();
    assert!(wide(tops[0]) > wide(tops[1]) && wide(tops[1]) > wide(tops[7]), "{tops:?}");
    assert!(lines[3..7].iter().all(|line| line.contains('█') && !line.contains('▄')), "{lines:#?}");
    assert!(lines[4].starts_with(" −10 ") && lines[6].starts_with(" −20 "), "{lines:#?}");
    // The Hz scale, and the tilt: none for pink noise.
    assert!(lines[7].starts_with("     ─") && lines[7].contains('┬'), "{lines:#?}");
    assert!(["50", "100", "200", "500", "1k", "2k", "5k", "10k Hz"].iter().all(|hz| lines[8].contains(hz)), "{lines:#?}");
    assert_eq!(lines[9..], ["tilt 0.0 dB per octave · pink noise is 0, darker is below"]);
}

#[test]
fn a_band_under_the_analyzers_scale_keeps_a_stub_and_the_tilt_says_how_dark() {
    let lines = texts(&entry_rows(&heard(DARK_MIX.to_vec()), 75, 0.));
    assert_eq!(lines.last().unwrap(), "tilt −1.4 dB per octave · pink noise is 0, darker is below");
    assert!(!lines[6].contains('▁'), "{lines:#?}");
    // Air 60 dB under the rest: under the scale, a stub on the bottom row (before the gridline's last cell).
    let mut quiet = DARK_MIX;
    quiet[9] = -60.;
    let lines = texts(&entry_rows(&heard(quiet.to_vec()), 75, 0.));
    assert!(lines[6].trim_end_matches('─').ends_with('▁'), "{lines:#?}");
}

#[test]
fn the_analyzer_needs_the_ten_bands_and_room_for_them() {
    let title = ["Heard mix.wav · −12.1 LUFS"];
    assert_eq!(texts(&entry_rows(&heard(DARK_MIX.to_vec()), 34, 0.)), title);
    assert!(entry_rows(&heard(DARK_MIX.to_vec()), 35, 0.).len() > 9);
    assert_eq!(texts(&entry_rows(&heard(vec![-15.; 31]), 75, 0.)), title);
    assert_eq!(texts(&entry_rows(&heard(vec![]), 75, 0.)), title);
    assert_eq!(texts(&entry_rows(&heard(vec![f64::NEG_INFINITY; 10]), 75, 0.)), title);
    // However wide the conversation, the analyzer keeps to 78 cells.
    let wide = texts(&entry_rows(&heard(DARK_MIX.to_vec()), 140, 0.));
    assert!(wide[2..9].iter().all(|line| line.chars().count() <= 78), "{wide:#?}");
}

#[test]
fn the_analyzers_bars_are_mixed_again_in_another_theme() {
    use kumi::tui::style::{set_theme, DARK, LIGHT};
    let mut t = Transcript::default();
    t.add(heard(DARK_MIX.to_vec()));
    let bar = |t: &mut Transcript| {
        t.rows(75, 0.).iter().flat_map(|row| row.spans.iter()).find(|span| span.text.contains('█')).and_then(|span| span.style.fg).unwrap()
    };
    // The accent half over the ground.
    assert_eq!(bar(&mut t), [0x4a, 0x79, 0x64]);
    let laid = t.laid_out;
    set_theme(&LIGHT);
    let light = bar(&mut t);
    set_theme(&DARK);
    assert_eq!(t.laid_out, laid + 1, "laid out again in the light theme");
    assert_ne!(light, [0x4a, 0x79, 0x64]);
    assert_eq!(bar(&mut t), [0x4a, 0x79, 0x64]);
}
