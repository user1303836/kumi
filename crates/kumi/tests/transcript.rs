use kumi::tui::{style::Style, transcript::*, wrap::Span};
use serde_json::{json, Value};
fn fixture() -> Value {
    serde_json::from_str(include_str!("support/transcript/reference.json")).unwrap()
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
    let empty = || Entry::Assistant { text: "".into(), status: AnswerStatus::Done, started_at: None, elapsed_ms: None, steps: vec![] };
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
