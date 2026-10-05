use std::borrow::Cow;

use kumi_common::js::json::stringify;
use kumi_runtime::ai::types::{
    AssistantPart, DataContent, FileData, Message, ReasoningPart, TextPart, ToolCallPart, ToolPart, ToolResultContentItem,
    ToolResultOutput, ToolResultPart, UserPart,
};
use kumi_runtime::kernel::budget::{drop_earliest, fit, ContextBudget, OBSERVATION_MARKER, SHORTENED};
use serde::Serialize;
use serde_json::json;

fn observed(words: &str) -> String {
    format!("{words}{OBSERVATION_MARKER}\n{}\n</current_observation_untrusted>", "o".repeat(600))
}
fn user(text: &str) -> Message {
    Message::user_text(text)
}
fn said(text: &str) -> Message {
    Message::assistant_text(text)
}
fn options(value: serde_json::Value) -> Option<serde_json::Map<String, serde_json::Value>> {
    match value {
        serde_json::Value::Object(map) => Some(map),
        _ => unreachable!(),
    }
}
fn called(id: &str) -> Message {
    Message::Assistant {
        content: vec![
            AssistantPart::Reasoning(ReasoningPart {
                text: String::new(),
                provider_options: options(json!({"openai": {"itemId": format!("rs_{id}"), "reasoningEncryptedContent": "enc"}})),
            }),
            AssistantPart::ToolCall(ToolCallPart {
                tool_call_id: id.into(),
                tool_name: "live_discover".into(),
                input: json!({"kind": "track"}),
                provider_executed: None,
                provider_options: options(json!({"openai": {"itemId": format!("fc_{id}")}})),
            }),
        ],
        provider_options: None,
    }
}
fn result(id: &str, value: &str, error: bool) -> Message {
    Message::Tool {
        content: vec![ToolPart::ToolResult(ToolResultPart {
            tool_call_id: id.into(),
            tool_name: "live_discover".into(),
            output: if error { ToolResultOutput::error_text(value) } else { ToolResultOutput::text(value) },
            provider_options: None,
        })],
        provider_options: None,
    }
}
fn read(tag: &str, bytes: usize) -> String {
    format!("{{\"changed\":\"{tag}\",\"items\":[{}\"end\"]}}", "\"x\",".repeat(bytes.div_ceil(4)))
}
/// One turn: the producer asks, Kumi reads Live once, then answers.
fn exchange(tag: &str, bytes: usize) -> Vec<Message> {
    vec![
        user(&observed(&format!("ask {tag}"))),
        called(&format!("c_{tag}")),
        result(&format!("c_{tag}"), &read(tag, bytes), false),
        said(&format!("answer {tag}")),
    ]
}
fn size<T: Serialize + ?Sized>(value: &T) -> usize {
    stringify(&serde_json::to_value(value).unwrap()).len()
}
fn output_of(message: Option<&Message>) -> Option<String> {
    match message {
        Some(Message::Tool { content, .. }) => match content.first() {
            Some(ToolPart::ToolResult(part)) => match &part.output {
                ToolResultOutput::Text { value, .. } | ToolResultOutput::ErrorText { value, .. } => Some(value.clone()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}
fn text_of(message: Option<&Message>) -> Option<String> {
    match message {
        Some(Message::User { content, .. }) => match content.first() {
            Some(UserPart::Text(TextPart { text, .. })) => Some(text.clone()),
            _ => None,
        },
        _ => None,
    }
}
fn budget(clear_at: f64, limit: f64) -> ContextBudget {
    ContextBudget { clear_at, limit }
}
const CLEARED_TAIL: &str = "Kumi cleared the rest of this earlier result to save room; read Live again if you need it.]";

#[test]
fn under_the_budget_nothing_changes_down_to_the_same_arrays() {
    let history = [exchange("a", 3000), exchange("b", 3000)].concat();
    let turn = vec![user(&observed("now"))];
    let fitted = fit(&history, &turn, &budget(64.0 * 1024.0, 128.0 * 1024.0));
    assert!(matches!(fitted.history, Cow::Borrowed(_)));
    assert!(matches!(fitted.turn, Cow::Borrowed(_)));
}

#[test]
fn past_clear_at_earlier_turns_reads_shrink_to_their_opening_and_their_observations_go_the_turn_before_stays_whole() {
    let small = result("c_small", "{\"changed\":\"Tempo 120 → 124\",\"change\":\"c1\",\"state\":\"applied\"}", false);
    let refusal = result("c_refused", &"x".repeat(900), true);
    let history =
        [exchange("a", 3000), vec![called("c_small"), small.clone(), called("c_refused"), refusal.clone()], exchange("b", 3000)].concat();
    let turn = vec![user(&observed("now"))];
    let fitted = fit(&history, &turn, &budget(4096.0, 64.0 * 1024.0));
    let cleared = output_of(fitted.history.get(2)).unwrap();
    assert!(cleared.starts_with("{\"changed\":\"a\",\"items\":[\"x\","), "{cleared}");
    assert!(cleared.ends_with(CLEARED_TAIL), "{cleared}");
    assert!(cleared.len() < 400);
    assert_eq!(text_of(fitted.history.first()).as_deref(), Some("ask a"));
    // Small results and short refusals stay whole; calls and replay metadata are untouched.
    assert_eq!(fitted.history[5], small);
    assert_eq!(fitted.history[7], refusal);
    assert_eq!(fitted.history[1], history[1]);
    // The turn just before this one, and this turn's own observation, stay as they were.
    assert_eq!(&fitted.history[8..], &history[8..]);
    assert!(matches!(fitted.turn, Cow::Borrowed(_)));
    assert_eq!(fitted.history.len(), history.len());
}

#[test]
fn a_steered_turn_before_this_one_is_still_kept_whole() {
    let steered = vec![
        user(&observed("prev")),
        called("c_prev"),
        result("c_prev", &read("prev", 3000), false),
        user("also check the drums"),
        said("answer prev"),
    ];
    let history = [exchange("a", 3000), steered.clone()].concat();
    let now = [user(&observed("now"))];
    let fitted = fit(&history, &now, &budget(4096.0, 64.0 * 1024.0));
    assert!(output_of(fitted.history.get(2)).unwrap().contains("Kumi cleared the rest"), "the turn before it is cleared");
    assert_eq!(&fitted.history[4..], &steered[..], "the steered turn keeps its read and its observation");
}

#[test]
fn fitting_is_stable_once_cleared_the_same_conversation_comes_back_unchanged_so_prompt_caches_keep_working() {
    let budget = budget(4096.0, 64.0 * 1024.0);
    let history = [exchange("a", 3000), exchange("b", 3000), exchange("c", 3000)].concat();
    let turn = vec![user(&observed("now"))];
    let once = fit(&history, &turn, &budget);
    let twice = fit(&once.history, &once.turn, &budget);
    assert!(matches!(twice.history, Cow::Borrowed(_)));
    assert!(matches!(twice.turn, Cow::Borrowed(_)));
}

#[test]
fn past_the_limit_this_turns_older_reads_are_cleared_too_keeping_its_latest_results_and_its_observation() {
    let turn = vec![
        user(&observed("now")),
        called("t1"),
        result("t1", &read("t1", 12_000), false),
        called("t2"),
        result("t2", &read("t2", 12_000), false),
    ];
    let history = exchange("a", 3000);
    let fitted = fit(&history, &turn, &budget(4096.0, 32.0 * 1024.0));
    assert!(output_of(fitted.turn.get(2)).unwrap().contains("Kumi cleared the rest"));
    assert_eq!(fitted.turn[4], turn[4]);
    assert_eq!(text_of(fitted.turn.first()), text_of(turn.first()));
    assert!(size(&[fitted.history.as_ref(), fitted.turn.as_ref()].concat()) <= 32 * 1024);
}

#[test]
fn when_clearing_isnt_enough_the_earliest_exchanges_go_with_a_note_where_the_conversation_now_starts() {
    let history: Vec<Message> = (0..40)
        .flat_map(|index| {
            vec![user(&format!("question {index} {}", "w".repeat(400))), said(&format!("answer {index} {}", "w".repeat(400)))]
        })
        .collect();
    let turn = vec![user(&observed("now"))];
    let budget = budget(4096.0, 16.0 * 1024.0);
    let fitted = fit(&history, &turn, &budget);
    assert!(fitted.history.len() < history.len());
    assert!(size(&[fitted.history.as_ref(), fitted.turn.as_ref()].concat()) as f64 <= budget.limit * 0.75);
    assert!(matches!(fitted.history.first(), Some(Message::User { .. })));
    assert!(text_of(fitted.history.first()).unwrap().starts_with(SHORTENED));
    assert_eq!(fitted.history.last(), history.last());
    // The next request drops nothing more and doesn't note twice.
    let again = fit(&fitted.history, &fitted.turn, &budget);
    assert!(matches!(again.history, Cow::Borrowed(_)));
}

#[test]
fn if_no_earlier_exchange_fits_the_note_goes_on_this_turn() {
    let history = vec![user(&"w".repeat(20_000)), said(&"w".repeat(20_000))];
    let turn = vec![user(&observed("now"))];
    let fitted = fit(&history, &turn, &budget(4096.0, 16.0 * 1024.0));
    assert!(fitted.history.is_empty());
    assert_eq!(text_of(fitted.turn.first()).unwrap(), format!("{SHORTENED}{}", text_of(turn.first()).unwrap()));
}

#[test]
fn drop_earliest_keeps_whole_exchanges_from_the_end_starting_where_the_producer_spoke() {
    let messages = vec![user("1"), said("2"), result("3", "three", false), user("4"), said("5")];
    assert_eq!(drop_earliest(&messages, 10_000), &messages[..]);
    assert_eq!(drop_earliest(&messages, size(&messages[3..])), &messages[3..]);
    assert!(drop_earliest(&messages, size(&messages[3..]) - 1).is_empty());
}

#[test]
fn images_count_at_what_they_cost_a_model_not_their_size_past_the_most_a_request_carries_this_turns_earliest_are_put_away() {
    let image = |id: &str, bytes: usize| Message::Tool {
        content: vec![ToolPart::ToolResult(ToolResultPart {
            tool_call_id: id.into(),
            tool_name: "watch_video".into(),
            output: ToolResultOutput::Content {
                value: vec![
                    ToolResultContentItem::Text { text: format!("frames {id}"), provider_options: None },
                    ToolResultContentItem::File {
                        data: FileData::Data { data: DataContent::Bytes(vec![0; bytes]) },
                        media_type: "image/jpeg".into(),
                        filename: None,
                        provider_options: None,
                    },
                ],
                provider_options: None,
            },
            provider_options: None,
        })],
        provider_options: None,
    };
    // 30 frames of 200 kB each would be 6 MB as bytes; as a model's cost they fit the budget.
    let turn: Vec<Message> =
        std::iter::once(user(&observed("watch"))).chain((0..30).map(|index| image(&format!("i{index}"), 200_000))).collect();
    let fitted = fit(&[], &turn, &budget(160.0 * 1024.0, 400.0 * 1024.0));
    assert!(matches!(fitted.turn, Cow::Borrowed(_)));
    let many: Vec<Message> =
        std::iter::once(user(&observed("watch"))).chain((0..45).map(|index| image(&format!("i{index}"), 10))).collect();
    let trimmed = fit(&[], &many, &budget(160.0 * 1024.0, 400.0 * 1024.0)).turn.into_owned();
    let kept = trimmed
        .iter()
        .filter(|message| matches!(message, Message::Tool { content, .. } if content.iter().any(|part| matches!(part, ToolPart::ToolResult(part) if matches!(part.output, ToolResultOutput::Content { .. })))))
        .count();
    assert_eq!(kept, 40);
    assert_eq!(
        output_of(trimmed.get(1)).as_deref(),
        Some("frames i0\n[An image was shown here; it's no longer attached (the tool shows it again when asked).]")
    );
}
