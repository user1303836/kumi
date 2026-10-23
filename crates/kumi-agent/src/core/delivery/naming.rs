//! Names for delivered files: the convention a project already uses (read from its regions' names: a stem, a separator and a
//! zero-padded number, "Footstep_Gravel_07"), numbered names in it, and what's wrong with a list of names for a game or a
//! delivery (spaces, characters a file system or an engine refuses, two names the same but for case, overlong ones).
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// A numbered naming convention: the stem, what separates it from the number, how many digits the number has, and the
/// highest number used so far.
#[derive(Debug, Clone, PartialEq)]
pub struct Convention {
    pub stem: String,
    pub separator: String,
    pub pad: usize,
    pub highest: usize,
}

/// The longest name Kumi proposes for a delivered file (game engines and file systems differ; this is safe for all).
pub const MOST_CHARS: usize = 64;

static NUMBERED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*?)([_\- .]?)([0-9]{1,4})$").unwrap());

/// The convention most of these names follow (the stem the most names share), or None when none is numbered.
pub fn convention(names: &[String]) -> Option<Convention> {
    let mut by_stem: BTreeMap<(String, String, usize), Vec<usize>> = BTreeMap::new();
    for name in names {
        let Some(caps) = NUMBERED.captures(name.trim()) else { continue };
        let stem = caps[1].to_string();
        if stem.is_empty() {
            continue;
        }
        let number: usize = caps[3].parse().unwrap_or(0);
        by_stem.entry((stem, caps[2].to_string(), caps[3].len())).or_default().push(number);
    }
    let ((stem, separator, pad), numbers) = by_stem.into_iter().max_by_key(|(_, numbers)| numbers.len())?;
    Some(Convention { stem, separator, pad, highest: numbers.into_iter().max().unwrap_or(0) })
}

/// The `n`th name in a convention ("Footstep_Gravel_07").
pub fn numbered(stem: &str, separator: &str, pad: usize, n: usize) -> String {
    format!("{stem}{separator}{n:0pad$}")
}

/// What's wrong with these names as file names for a delivery, in words: none when nothing.
pub fn problems(names: &[String]) -> Vec<String> {
    static BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[<>:"/\\|?*\x00-\x1f]"#).unwrap());
    let mut out = Vec::new();
    let with_spaces: Vec<&String> = names.iter().filter(|name| name.contains(' ')).collect();
    if !with_spaces.is_empty() {
        out.push(format!(
            "{} name{} spaces ({}): game pipelines usually refuse them",
            with_spaces.len(),
            if with_spaces.len() == 1 { " has" } else { "s have" },
            quoted(with_spaces[0])
        ));
    }
    if let Some(bad) = names.iter().find(|name| BAD.is_match(name)) {
        out.push(format!("{} has a character file systems refuse", quoted(bad)));
    }
    if let Some(empty) = names.iter().position(|name| name.trim().is_empty()) {
        out.push(format!("name {} is empty", empty + 1));
    }
    if let Some(long) = names.iter().find(|name| name.chars().count() > MOST_CHARS) {
        out.push(format!("{} is longer than {MOST_CHARS} characters", quoted(long)));
    }
    let mut seen: BTreeMap<String, &String> = BTreeMap::new();
    for name in names {
        if let Some(first) = seen.insert(name.to_lowercase(), name) {
            out.push(format!("{} and {} are one file on a drive that ignores case", quoted(first), quoted(name)));
            break;
        }
    }
    out
}

fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn the_convention_is_the_numbered_stem_most_names_share() {
        let found = convention(&names(&["Footstep_Gravel_01", "Footstep_Gravel_02", "Footstep_Gravel_07", "Ambience", "Door_1"])).unwrap();
        assert_eq!(found, Convention { stem: "Footstep_Gravel".into(), separator: "_".into(), pad: 2, highest: 7 });
        assert_eq!(numbered(&found.stem, &found.separator, found.pad, 8), "Footstep_Gravel_08");
        assert_eq!(convention(&names(&["Intro", "Verse"])), None);
        assert_eq!(convention(&names(&["UI-Click-003"])).unwrap().pad, 3);
    }

    #[test]
    fn problems_are_spaces_bad_characters_case_twins_and_overlong_names() {
        assert!(problems(&names(&["Footstep_Gravel_01", "Footstep_Gravel_02"])).is_empty());
        let found = problems(&names(&["Foot step 01", "door?", "Door?", ""]));
        assert!(found.iter().any(|p| p.contains("spaces")), "{found:?}");
        assert!(found.iter().any(|p| p.contains("file systems refuse")), "{found:?}");
        assert!(found.iter().any(|p| p.contains("ignores case")), "{found:?}");
        assert!(found.iter().any(|p| p.contains("is empty")), "{found:?}");
        assert!(problems(&[("x".repeat(70))]).iter().any(|p| p.contains("longer than 64")));
    }
}
