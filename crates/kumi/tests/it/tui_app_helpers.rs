use kumi::tui::{app::*, style::ColorDepth, wrap::Span};
use serde_json::{json, Value};
fn picture(rows: Option<Vec<Vec<Span>>>) -> Value {
    rows.map(|rows| {
        Value::Array(
            rows.into_iter()
                .map(|row| {
                    Value::Array(
                        row.into_iter()
                            .map(|s| {
                                let mut style = json!({});
                                if let Some(fg) = s.style.fg {
                                    style["fg"] = json!(fg);
                                }
                                if let Some(bg) = s.style.bg {
                                    style["bg"] = json!(bg);
                                }
                                if s.style.bold {
                                    style["bold"] = json!(true);
                                }
                                json!({"text":s.text,"style":style})
                            })
                            .collect(),
                    )
                })
                .collect(),
        )
    })
    .unwrap_or(Value::Null)
}
#[test]
fn focus_paths_changes_and_piano_rolls_match_source_whole_results() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("support/app/reference.json")).unwrap();
    for case in cases {
        let args = case["args"].as_array().unwrap();
        let value = match case["fn"].as_str().unwrap() {
            "isCommand" => json!(is_command(args[0].as_str().unwrap())),
            "chipColor" => json!(chip_color(args[0].as_str())),
            "focusPath" => json!(focus_path(&serde_json::from_value(args[0].clone()).unwrap())),
            "fitCrumbs" => {
                json!(fit_crumbs(&serde_json::from_value::<Vec<String>>(args[0].clone()).unwrap(), args[1].as_i64().unwrap() as i32))
            }
            "touchedNext" => json!(touched_next(
                serde_json::from_value::<Option<kumi_runtime::core::contracts::LiveFocus>>(args[0].clone()).unwrap().as_ref(),
                serde_json::from_value::<Option<kumi_runtime::core::contracts::LiveFocus>>(args[1].clone()).unwrap().as_ref(),
                serde_json::from_value(args[2].clone()).unwrap()
            )),
            "setNameFrom" => json!(set_name_from(args[0].as_str().unwrap())),
            "changePicture" => picture(change_picture(
                &serde_json::from_value(args[0].clone()).unwrap(),
                args[1].as_i64().unwrap() as i32,
                ColorDepth::parse(args[2].as_str().unwrap()).unwrap(),
            )),
            "clipPicture" => picture(clip_picture(
                args[0]["length"].as_f64().unwrap(),
                &serde_json::from_value::<Vec<_>>(args[0]["notes"].clone()).unwrap(),
                args[1].as_i64().unwrap() as i32,
                args[2].as_u64().unwrap() as usize,
            )),
            "catchUpText" => json!(catch_up_text(&serde_json::from_value(args[0].clone()).unwrap(), args[1].as_f64().unwrap())),
            other => panic!("unknown fixture {other}"),
        };
        assert_eq!(value, case["expected"], "{}({args:?})", case["fn"]);
    }
}
#[test]
fn voice_language_names_keep_the_source_display_names() {
    for (code, name) in
        [("ja", "Japanese"), ("auto", "any language"), ("en", "English"), ("iw", "Hebrew"), ("fil", "Filipino"), ("zz", "zz")]
    {
        assert_eq!(language_name(code), name);
    }
}
