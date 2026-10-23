//! What would stop a kept script working the next time, in another project: ids from this project (GUIDs), Kumi's refs (they
//! are this conversation's), what it takes by its place there (the DAW's rules), and a chunk that never ran here cleanly.
use super::backend::Words;
use regex::Regex;
use std::sync::LazyLock;

/// A finding: whether it stops the script being kept, and what it says.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub blocks: bool,
    pub says: String,
}

static GUID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\}").unwrap());
/// One of Kumi's refs in a string ("track:3"; a long one holds a GUID).
static REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"["']([a-z]+:[0-9]+)["']"#).unwrap());

/// The findings for `code`: `ran` says whether it ran in this session without an error, `this_project` that the producer
/// keeps it for this project only (its ids then stand).
pub fn lint(code: &str, words: &Words, ran: bool, this_project: bool) -> Vec<Finding> {
    let mut out = Vec::new();
    if !ran {
        out.push(Finding {
            blocks: true,
            says: "it hasn't run in this session without an error: run it once with run_lua first, so the producer sees it work before it's kept".into(),
        });
    }
    if let Some(id) = GUID.find(code).filter(|_| !this_project) {
        out.push(Finding {
            blocks: true,
            says: format!(
                "it names {} by its id in this project: in another one it finds nothing. Find what to work on by the selection or by name (or keep it with thisProject: true, for this project only)",
                id.as_str()
            ),
        });
    }
    if let Some(found) = REF.captures(code) {
        out.push(Finding {
            blocks: true,
            says: format!("\u{201c}{}\u{201d} is one of Kumi's refs, this conversation's: a script can't follow it. Find it by the selection or by name", &found[1]),
        });
    }
    for (pattern, says) in words.places {
        if Regex::new(pattern).is_ok_and(|re| re.is_match(code)) {
            out.push(Finding { blocks: false, says: (*says).to_string() });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    static WORDS: Words =
        Words { app: "Fake", language: "Lua", action: "a menu entry", places: &[(r"track\(\d+\)", "takes a track by its place")] };

    #[test]
    fn ids_refs_and_a_chunk_that_never_ran_stop_a_script_and_places_are_said() {
        assert!(lint("for t in tracks() do end", &WORDS, true, false).is_empty());
        let never = lint("return 1", &WORDS, false, false);
        assert!(never[0].blocks && never[0].says.starts_with("it hasn't run"), "{never:?}");
        let id = "local t = get('{0AF3C7B1-1E0A-4C59-9F0B-0D3F5C1B2A99}')";
        assert!(lint(id, &WORDS, true, false)[0].says.contains("by its id in this project"));
        assert!(lint(id, &WORDS, true, true).is_empty(), "kept for this project, its ids stand");
        let reference = lint("local t = find(\"track:3\")", &WORDS, true, false);
        assert!(reference[0].blocks && reference[0].says.starts_with("\u{201c}track:3\u{201d} is one of Kumi's refs"));
        let place = lint("song:track(2).name = 'x'", &WORDS, true, false);
        assert_eq!(place, vec![Finding { blocks: false, says: "takes a track by its place".into() }]);
    }
}
