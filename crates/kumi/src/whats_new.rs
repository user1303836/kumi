//! What changed in Kumi since the producer last opened it. The first start after an update shows a few
//! of the newest notes from Kumi's changelog, which is built in, so an installed Kumi and a checkout carry
//! their own; `/changelog` shows the rest. A fresh install shows nothing. The notes are for the producer:
//! they're shown in the conversation and never sent to the model.
//!
//! The version last shown is kept in `whats-new.json` beside `settings.json` (an older Kumi writing its
//! settings would drop a key it doesn't know). Going back to an older Kumi keeps the newer one there, so
//! updating again doesn't show the same notes twice.

use kumi_common::js::json::stringify;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
/// The most notes the first start after an update shows; `/changelog` has the rest.
pub const SHOWN: usize = 5;
/// How many releases `/changelog` shows when there's no update to show them since.
const LATEST: usize = 3;
pub const SEEN_FILE: &str = "whats-new.json";

/// A release's notes as the changelog has them, its headings' paragraphs about the bridge and testing left out.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub items: Vec<String>,
}

/// Notes to show: a heading, the notes (each with its release when there are several), and how many more
/// `/changelog` has.
#[derive(Clone, Debug, PartialEq)]
pub struct News {
    pub title: String,
    pub items: Vec<String>,
    pub more: usize,
    /// The version these are since: `/changelog` starts there.
    pub since: Option<String>,
}

fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().splitn(3, '.').map(|part| part.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// A note's words without the changelog's Markdown: code marks, bold and links' targets.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|at| open + at) else { break };
        let Some(end) = rest[close..].find(')').map(|at| close + at) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..close]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out.replace("**", "").replace('`', "")
}

/// Every release in `changelog`, newest first as it lists them. A release's notes are its top-level list
/// items, each with what's indented under it (its nested items run into it); the paragraphs between them
/// (which bridge it ships with, what it was tested on) are left out.
pub fn releases_in(changelog: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();
    let mut item: Option<String> = None;
    let finish = |releases: &mut Vec<Release>, item: &mut Option<String>| {
        if let (Some(release), Some(text)) = (releases.last_mut(), item.take()) {
            release.items.push(plain(&text));
        }
    };
    for line in changelog.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            finish(&mut releases, &mut item);
            let version = heading.split_whitespace().next().unwrap_or("").trim_matches(['[', ']']);
            if parse_version(version).is_some() {
                releases.push(Release { version: version.into(), items: vec![] });
            }
            continue;
        }
        if releases.is_empty() || line.trim().is_empty() {
            continue;
        }
        if let Some(text) = line.strip_prefix("- ") {
            finish(&mut releases, &mut item);
            item = Some(text.trim().to_string());
        } else if line.starts_with("  ") {
            if let Some(text) = item.as_mut() {
                text.push(' ');
                text.push_str(line.trim().trim_start_matches("- "));
            }
        } else {
            finish(&mut releases, &mut item);
        }
    }
    finish(&mut releases, &mut item);
    releases
}

pub fn releases() -> Vec<Release> {
    releases_in(CHANGELOG)
}

/// The releases after `seen`, up to and including `current`, newest first.
pub fn since(all: &[Release], seen: &str, current: &str) -> Vec<Release> {
    let (Some(seen), Some(current)) = (parse_version(seen), parse_version(current)) else { return vec![] };
    all.iter()
        .filter(|release| parse_version(&release.version).is_some_and(|version| version > seen && version <= current))
        .cloned()
        .collect()
}

/// `releases` as notes: at most `shown` of them (None: all), each named by its release when there are several.
pub fn news(releases: &[Release], title: String, since: Option<String>, shown: Option<usize>) -> Option<News> {
    let several = releases.len() > 1;
    let all: Vec<String> = releases
        .iter()
        .flat_map(|release| {
            release.items.iter().map(move |item| if several { format!("{} · {item}", release.version) } else { item.clone() })
        })
        .collect();
    if all.is_empty() {
        return None;
    }
    let count = shown.unwrap_or(all.len()).min(all.len());
    Some(News { title, more: all.len() - count, items: all.into_iter().take(count).collect(), since })
}

fn seen_file(settings_file: &str) -> PathBuf {
    Path::new(settings_file).parent().unwrap_or(Path::new(".")).join(SEEN_FILE)
}

fn read_seen(file: &Path) -> Option<String> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    value["seen"].as_str().filter(|seen| parse_version(seen).is_some()).map(str::to_string)
}

fn write_seen(file: &Path, version: &str) {
    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let _ = std::fs::write(file, format!("{}\n", stringify(&json!({"seen": version}))));
}

/// What to show as Kumi `current` starts, recording it as seen. `earlier`: this home has been used by a
/// Kumi before (one from before these notes kept a version, say), so a first look at the file isn't a
/// fresh install and shows this release's notes. `on`: the producer hasn't turned the notes off; off,
/// the version is still recorded, so turning them on later doesn't bring back old notes.
pub fn at_start(settings_file: &str, current: &str, earlier: bool, on: bool) -> Option<News> {
    at_start_from(&releases(), settings_file, current, earlier, on)
}

pub fn at_start_from(all: &[Release], settings_file: &str, current: &str, earlier: bool, on: bool) -> Option<News> {
    let file = seen_file(settings_file);
    let seen = read_seen(&file);
    let newer = match seen.as_deref() {
        Some(seen) => parse_version(current) > parse_version(seen),
        None => true,
    };
    if !newer {
        return None;
    }
    write_seen(&file, current);
    if !on {
        return None;
    }
    let (releases, since) = match seen {
        Some(seen) => (self::since(all, &seen, current), Some(seen)),
        None if earlier => (all.iter().filter(|release| release.version == current).cloned().collect(), None),
        None => return None,
    };
    let title = match &since {
        Some(seen) if releases.len() > 1 => format!("What's new in Kumi {current}, since {seen}"),
        _ => format!("What's new in Kumi {current}"),
    };
    news(&releases, title, since, Some(SHOWN))
}

/// `/changelog`: every note since `since` when this start showed an update, otherwise the latest releases'.
pub fn changelog(current: &str, since: Option<&str>) -> Option<News> {
    changelog_from(&releases(), current, since)
}

pub fn changelog_from(all: &[Release], current: &str, since: Option<&str>) -> Option<News> {
    let updated = since.map(|seen| self::since(all, seen, current)).filter(|releases| !releases.is_empty());
    let releases = updated.clone().unwrap_or_else(|| {
        all.iter().filter(|release| parse_version(&release.version) <= parse_version(current)).take(LATEST).cloned().collect()
    });
    let title = match (since.filter(|_| updated.is_some()), releases.as_slice()) {
        (Some(seen), _) => format!("What's new in Kumi {current}, since {seen}"),
        (None, [only]) => format!("What's new in Kumi {}", only.version),
        (None, [newest, .., oldest]) => format!("Kumi {} to {}", oldest.version, newest.version),
        (None, []) => return None,
    };
    news(&releases, title, since.map(str::to_string), None)
}

/// Where the notes not shown are, when some aren't.
pub fn more_line(news: &News) -> Option<String> {
    match (news.more, news.since.as_deref()) {
        (0, _) => None,
        (more, Some(since)) => Some(format!("and {more} more since {since}: /changelog")),
        (more, None) => Some(format!("and {more} more: /changelog")),
    }
}

/// Where the whole history is.
pub fn history_link() -> String {
    format!("{}/blob/main/CHANGELOG.md", env!("CARGO_PKG_REPOSITORY"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Changelog\n\nIntro.\n\n## 1.8.11 — 2026-10-06\n\nShips with bridge 1.0.84, which Live loads when it restarts.\n\nTested with Live 12.4 on macOS.\n\n### Bridge 1.0.84\n\n- Copying a Session clip into the Arrangement over another clip is refused, naming it, instead of\n  losing part of that clip.\n\n## 1.8.10 — 2026-10-06\n\n- Kumi sees the Set's **scale** (`D Dorian`), see [the guide](docs/en/KUMI_GUIDE.md).\n- Setup takes one command:\n  - signing in;\n  - the bridge.\n\n  Esc chats without Live for now.\n\n## 1.8.9 — 2026-10-05\n\n- Take lanes.\n";

    #[test]
    fn a_releases_notes_are_its_list_items_in_plain_words_without_its_paragraphs_or_nested_lists() {
        let parsed = releases_in(SAMPLE);
        assert_eq!(parsed.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["1.8.11", "1.8.10", "1.8.9"]);
        assert_eq!(
            parsed[0].items,
            ["Copying a Session clip into the Arrangement over another clip is refused, naming it, instead of losing part of that clip."]
        );
        assert_eq!(
            parsed[1].items,
            [
                "Kumi sees the Set's scale (D Dorian), see the guide.",
                "Setup takes one command: signing in; the bridge. Esc chats without Live for now."
            ]
        );
        // The real changelog parses: every release has notes, the newest first.
        let real = releases();
        assert!(real.len() > 20 && real.iter().all(|release| !release.items.is_empty()), "{real:?}");
        assert_eq!(real[0].version, kumi_runtime::KUMI_VERSION, "the changelog's newest release is this Kumi");
    }

    #[test]
    fn the_first_start_after_an_update_shows_the_newest_notes_once_and_a_fresh_install_none() {
        let all = releases_in(SAMPLE);
        let home = tempfile::tempdir().unwrap();
        let settings = home.path().join("settings.json").to_string_lossy().into_owned();
        // A fresh install: recorded, nothing shown.
        assert_eq!(at_start_from(&all, &settings, "1.8.9", false, true), None);
        // Updated across two releases: their notes, newest first, each named by its release.
        let news = at_start_from(&all, &settings, "1.8.11", true, true).unwrap();
        assert_eq!(news.title, "What's new in Kumi 1.8.11, since 1.8.9");
        assert_eq!(news.items.len(), 3);
        assert!(news.items[0].starts_with("1.8.11 · Copying a Session clip"));
        assert_eq!((news.more, news.since.as_deref()), (0, Some("1.8.9")));
        // Once.
        assert_eq!(at_start_from(&all, &settings, "1.8.11", true, true), None);
        // Back to an older Kumi and on again: nothing twice.
        assert_eq!(at_start_from(&all, &settings, "1.8.10", true, true), None);
        assert_eq!(at_start_from(&all, &settings, "1.8.11", true, true), None);
        // A home a Kumi used before the notes kept a version: this release's notes.
        let earlier = tempfile::tempdir().unwrap();
        let settings = earlier.path().join("settings.json").to_string_lossy().into_owned();
        let news = at_start_from(&all, &settings, "1.8.10", true, true).unwrap();
        assert_eq!(news.title, "What's new in Kumi 1.8.10");
        assert_eq!(news.items[0], "Kumi sees the Set's scale (D Dorian), see the guide.");
        // Turned off: recorded, not shown.
        let off = tempfile::tempdir().unwrap();
        let settings = off.path().join("settings.json").to_string_lossy().into_owned();
        write_seen(&seen_file(&settings), "1.8.9");
        assert_eq!(at_start_from(&all, &settings, "1.8.11", true, false), None);
        assert_eq!(read_seen(&seen_file(&settings)).as_deref(), Some("1.8.11"));
    }

    #[test]
    fn at_most_five_notes_show_at_the_start_and_changelog_has_them_all() {
        let mut changelog = String::from("# Changelog\n\n## 2.0.0 — 2026-10-07\n\n");
        for index in 0..8 {
            changelog.push_str(&format!("- Note {index}.\n"));
        }
        changelog.push_str("\n## 1.9.0 — 2026-10-06\n\n- Older.\n");
        let all = releases_in(&changelog);
        let home = tempfile::tempdir().unwrap();
        let settings = home.path().join("settings.json").to_string_lossy().into_owned();
        write_seen(&seen_file(&settings), "1.8.0");
        let news = at_start_from(&all, &settings, "2.0.0", true, true).unwrap();
        assert_eq!((news.items.len(), news.more), (SHOWN, 4));
        let full = changelog_from(&all, "2.0.0", news.since.as_deref()).unwrap();
        assert_eq!((full.items.len(), full.more), (9, 0));
        assert_eq!(full.title, "What's new in Kumi 2.0.0, since 1.8.0");
        // Without an update this start, the latest releases.
        let latest = changelog_from(&all, "2.0.0", None).unwrap();
        assert_eq!(latest.title, "Kumi 1.9.0 to 2.0.0");
        assert_eq!(latest.items.len(), 9);
    }
}
