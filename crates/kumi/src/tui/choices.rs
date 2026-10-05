//! The options an answer ends on when Kumi asks the producer to pick one, so one key can answer.

use std::sync::LazyLock;

use kumi_common::js::string::utf16_len;
use regex::Regex;

/// Longest option offered as a one-key answer (UTF-16 units); past it, the list is prose, not options.
const LONGEST: usize = 100;

/// An answer's options: a question ("?" or Japanese and Chinese "？"), then a numbered list (2 to 9
/// short items, "1." or "1)", or "1．" and "1、" with no space needed, in order) with nothing after it.
/// None when the answer doesn't end that way.
pub fn choices(answer: &str) -> Option<Vec<String>> {
    static ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([1-9])(?:[.)]\s+|[．、]\s*)(\S.*)$").unwrap());
    let mut items = Vec::new();
    let mut question = None;
    for line in answer.lines().rev().map(str::trim).filter(|line| !line.is_empty()) {
        match ITEM.captures(line) {
            Some(item) if question.is_none() => items.push((item[1].parse::<usize>().ok()?, plain(&item[2]))),
            _ => {
                question = Some(line);
                break;
            }
        }
    }
    items.reverse();
    let numbered = items.iter().enumerate().all(|(index, (n, _))| *n == index + 1);
    let short = items.iter().all(|(_, text)| !text.is_empty() && utf16_len(text) <= LONGEST);
    (question?.contains(['?', '？']) && (2..=9).contains(&items.len()) && numbered && short)
        .then(|| items.into_iter().map(|(_, text)| text).collect())
}

/// An option without its Markdown emphasis.
fn plain(text: &str) -> String {
    text.replace("**", "").replace("__", "").replace('`', "").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_ending_on_a_short_numbered_list_offers_its_options() {
        assert_eq!(
            choices("I found two basses.\n\nWhich one should get the sidechain?\n\n1. **Sub Bass**\n2) `Reese`\n"),
            Some(vec!["Sub Bass".to_string(), "Reese".to_string()])
        );
        assert_eq!(choices("Which track?\n1. Bass\n\n2. Lead\n\n3. A new track").map(|c| c.len()), Some(3));
        assert_eq!(
            choices("どちらのベースにサイドチェインをかけますか？\n1．サブベース\n2．リース"),
            Some(vec!["サブベース".to_string(), "リース".to_string()])
        );
        assert_eq!(choices("要给哪一条贝斯加侧链？\n1、低音\n2、Reese"), Some(vec!["低音".to_string(), "Reese".to_string()]));
    }

    #[test]
    fn anything_else_offers_nothing() {
        for answer in [
            "Done: added a compressor.",
            "Here's what changed:\n1. Tempo 120 → 124\n2. Swing 10%",
            "Which one?\n1. Bass",
            "Which one?\n1. Bass\n3. Lead",
            "Which one?\n1.5 dB\n2.5 dB",
            "Which one?\n1. Bass\n2. Lead\n\nOr tell me another.",
            &format!("Which one?\n1. {}\n2. Lead", "x".repeat(101)),
        ] {
            assert_eq!(choices(answer), None, "{answer}");
        }
    }
}
