//! Logic's labels in the producer's language. The bridge matches elements by what Logic calls them ("Control Bar",
//! "Tracks", "Tempo"), and Logic calls them so only in English: the other languages' words are in the `.lproj` tables
//! of the installed bundle (`Logic Pro.app/Contents/Frameworks/*.framework/Versions/A/Resources/<language>.lproj/
//! Localizable.strings`, the app's own `Contents/Resources/<language>.lproj` beside them), keyed by the English
//! string, sometimes with a context after a `#` (`Mute#acc`) or a prefix before `|||` (`StrTabBtnLabel|||Tracks`).
//! [`Strings`] loads the tables of the language Logic runs in and answers every translation a key family has, so a
//! label matches whichever of them Logic used; in English nothing is loaded and a label matches itself. The tables
//! are UTF-16 text or binary property lists, so `plutil` reads them: it is on every Mac.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// The languages Logic ships, as their `.lproj` folders are named, and the `AppleLanguages` prefixes that pick them.
const LANGUAGES: &[(&str, &[&str])] = &[
    ("en", &["en"]),
    ("de", &["de"]),
    ("fr", &["fr"]),
    ("es", &["es"]),
    ("it", &["it"]),
    ("ja", &["ja"]),
    ("ko", &["ko"]),
    ("pt", &["pt"]),
    ("zh_CN", &["zh-Hans", "zh-CN"]),
    ("zh_TW", &["zh-Hant", "zh-TW", "zh-HK"]),
];

/// The translations of Logic's English labels, for one language.
#[derive(Debug, Clone, Default)]
pub struct Strings {
    /// The `.lproj` name the tables came from; `en` for none.
    pub language: String,
    /// English → every translation its key family has.
    translations: HashMap<String, Vec<String>>,
}

/// The English key of a table entry: `Mute#acc` → `Mute`, `StrTabBtnLabel|||Tracks` → `Tracks`.
pub fn english_key(key: &str) -> &str {
    let key = key.rsplit_once("|||").map_or(key, |(_, rest)| rest);
    key.split_once('#').map_or(key, |(english, _)| english)
}

impl Strings {
    /// No translations: Logic in English.
    pub fn english() -> Strings {
        Strings { language: "en".into(), translations: HashMap::new() }
    }

    /// Translations given directly, for tests.
    pub fn with(language: &str, pairs: &[(&str, &str)]) -> Strings {
        let mut strings = Strings { language: language.into(), translations: HashMap::new() };
        for (english, translated) in pairs {
            strings.add(english, translated);
        }
        strings
    }

    fn add(&mut self, key: &str, translated: &str) {
        let english = english_key(key);
        if translated.is_empty() || translated == english {
            return;
        }
        let list = self.translations.entry(english.to_string()).or_default();
        if !list.iter().any(|known| known == translated) {
            list.push(translated.to_string());
        }
    }

    /// Whether a label Logic shows is the English one, or one of its translations.
    pub fn is(&self, label: &str, english: &str) -> bool {
        label == english || self.translations.get(english).is_some_and(|list| list.iter().any(|t| t == label))
    }

    /// Whether a label starts with the English word or one of its translations (for labels Logic composes, such as
    /// `Track 2 “Keys”`).
    pub fn starts_with(&self, label: &str, english: &str) -> bool {
        label.starts_with(english) || self.translations.get(english).is_some_and(|list| list.iter().any(|t| label.starts_with(t.as_str())))
    }

    /// The translations of an English label, the English one first.
    pub fn candidates(&self, english: &str) -> Vec<String> {
        let mut out = vec![english.to_string()];
        if let Some(list) = self.translations.get(english) {
            out.extend(list.iter().cloned());
        }
        out
    }

    /// How many English keys have a translation.
    pub fn len(&self) -> usize {
        self.translations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.translations.is_empty()
    }

    /// The tables of the language Logic runs in, from its app bundle: nothing for English, or for a language whose
    /// tables aren't in the bundle.
    pub fn load(bundle: &Path, language: &str) -> Strings {
        let mut strings = Strings { language: language.into(), translations: HashMap::new() };
        if language == "en" {
            return strings;
        }
        for table in tables(bundle, language) {
            if let Some(entries) = read_table(&table) {
                for (key, value) in entries {
                    strings.add(&key, &value);
                }
            }
        }
        strings
    }
}

/// The `Localizable.strings` tables of a language in the bundle: each framework's, and the app's own.
fn tables(bundle: &Path, language: &str) -> Vec<PathBuf> {
    let lproj = format!("{language}.lproj");
    let mut found = vec![];
    let app_table = bundle.join("Contents/Resources").join(&lproj).join("Localizable.strings");
    if app_table.is_file() {
        found.push(app_table);
    }
    let frameworks = bundle.join("Contents/Frameworks");
    let Ok(entries) = std::fs::read_dir(&frameworks) else { return found };
    let mut names: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    names.sort();
    for framework in names {
        let table = framework.join("Versions/A/Resources").join(&lproj).join("Localizable.strings");
        if table.is_file() {
            found.push(table);
        }
    }
    found
}

/// A `.strings` table (UTF-16 text or a binary plist) as JSON, through `plutil`.
fn read_table(path: &Path) -> Option<Vec<(String, String)>> {
    let output = Command::new(crate::tools::PLUTIL).args(["-convert", "json", "-o", "-"]).arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value: Value = serde_json::from_slice(&output.stdout).ok()?;
    let entries = value.as_object()?;
    Some(entries.iter().filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string()))).collect())
}

/// The language Logic runs in, as its `.lproj` is named: the first of the user's languages (Logic's own
/// `AppleLanguages`, else the system's) that Logic ships, `en` when none is.
pub fn language_of(preferred: &[String]) -> String {
    for wanted in preferred {
        let wanted = wanted.replace('_', "-");
        for (lproj, prefixes) in LANGUAGES {
            if prefixes.iter().any(|prefix| wanted == *prefix || wanted.starts_with(&format!("{prefix}-"))) {
                return lproj.to_string();
            }
        }
    }
    "en".into()
}

/// The user's preferred languages, from the app's own preferences first, then the system's.
#[cfg(target_os = "macos")]
pub fn preferred_languages(bundle_id: &str) -> Vec<String> {
    for args in [vec!["read", bundle_id, "AppleLanguages"], vec!["read", "-g", "AppleLanguages"]] {
        let Ok(output) = Command::new(crate::tools::DEFAULTS).args(&args).output() else { continue };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let languages: Vec<String> = text
            .lines()
            .map(|line| line.trim().trim_end_matches(',').trim_matches('"').to_string())
            .filter(|line| !line.is_empty() && line != "(" && line != ")")
            .collect();
        if !languages.is_empty() {
            return languages;
        }
    }
    vec![]
}

#[cfg(not(target_os = "macos"))]
pub fn preferred_languages(_bundle_id: &str) -> Vec<String> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_matches_the_english_or_any_translation_of_its_key_family() {
        let strings = Strings::with(
            "de",
            &[
                ("Control Bar#acc", "Steuerungsleiste"),
                ("Mute", "Stummschalten"),
                ("Mute#acc", "Ton aus"),
                ("StrTabBtnLabel|||Tracks", "Spuren"),
                ("Tempo", "Tempo"),
            ],
        );
        assert!(strings.is("Control Bar", "Control Bar"));
        assert!(strings.is("Steuerungsleiste", "Control Bar"));
        assert!(strings.is("Ton aus", "Mute") && strings.is("Stummschalten", "Mute") && strings.is("Mute", "Mute"));
        assert!(!strings.is("Solo", "Mute"));
        assert!(strings.is("Spuren", "Tracks"));
        assert!(strings.starts_with("Spuren-Header", "Tracks"));
        assert_eq!(strings.candidates("Mute"), ["Mute", "Stummschalten", "Ton aus"]);
        // A translation equal to the English adds nothing; English is empty.
        assert_eq!(strings.candidates("Tempo"), ["Tempo"]);
        assert!(Strings::english().is_empty());
        assert!(Strings::english().is("Tempo", "Tempo"));
        assert_eq!(english_key("Mute#acc"), "Mute");
        assert_eq!(english_key("StrTabBtnLabel|||Tracks"), "Tracks");
        assert_eq!(english_key("Plain"), "Plain");
    }

    #[test]
    fn the_language_is_the_first_preferred_one_logic_ships() {
        assert_eq!(language_of(&["en-US".into(), "ja-US".into()]), "en");
        assert_eq!(language_of(&["nl-NL".into(), "de-DE".into()]), "de");
        assert_eq!(language_of(&["zh-Hans-CN".into()]), "zh_CN");
        assert_eq!(language_of(&["zh-Hant-TW".into()]), "zh_TW");
        assert_eq!(language_of(&["pt_BR".into()]), "pt");
        assert_eq!(language_of(&["nl".into()]), "en");
        assert_eq!(language_of(&[]), "en");
    }

    #[test]
    fn tables_are_found_in_the_bundle_and_read_through_plutil() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Logic Pro.app");
        let table = bundle.join("Contents/Frameworks/Logic.framework/Versions/A/Resources/de.lproj");
        std::fs::create_dir_all(&table).unwrap();
        std::fs::write(
            table.join("Localizable.strings"),
            "/* a comment */\n\"Control Bar#acc\" = \"Steuerungsleiste\";\n\"Tempo\" = \"Tempo\";\n",
        )
        .unwrap();
        let own = bundle.join("Contents/Resources/de.lproj");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("Localizable.strings"), "\"Mixer\" = \"Mischpult\";\n").unwrap();
        assert_eq!(tables(&bundle, "de").len(), 2);
        assert!(tables(&bundle, "fr").is_empty());
        assert!(Strings::load(&bundle, "en").is_empty());
        if cfg!(target_os = "macos") {
            let strings = Strings::load(&bundle, "de");
            assert!(strings.is("Steuerungsleiste", "Control Bar"));
            assert!(strings.is("Mischpult", "Mixer"));
            assert_eq!(strings.len(), 2);
        }
    }
}
