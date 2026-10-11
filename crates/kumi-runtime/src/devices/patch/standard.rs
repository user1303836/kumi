//! The standard Kumi holds Max patchers to, from `max-standard/standard.json`: each rule's level, and what measuring
//! well-made devices found for it.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde_json::Value;

/// How much a broken rule matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Kumi says nothing.
    Off,
    /// Kumi mentions it when asked to look a patcher over.
    Advice,
    /// Kumi says so.
    Warn,
    /// Kumi doesn't make a device that breaks it.
    Error,
}

impl Level {
    pub fn parse(text: &str) -> Option<Level> {
        match text {
            "off" => Some(Level::Off),
            "advice" => Some(Level::Advice),
            "warn" => Some(Level::Warn),
            "error" => Some(Level::Error),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Advice => "advice",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

#[derive(Debug, Default)]
pub struct Standard {
    levels: HashMap<String, Level>,
}

static STANDARD: LazyLock<Standard> = LazyLock::new(|| {
    Standard::from_value(&serde_json::from_str(include_str!("../../../../../max-standard/standard.json")).expect("standard.json"))
});

/// The standard Kumi carries.
pub fn standard() -> &'static Standard {
    &STANDARD
}

impl Standard {
    pub fn from_value(value: &Value) -> Standard {
        let levels = value["rules"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(rule, entry)| Some((rule.clone(), Level::parse(entry["level"].as_str()?)?)))
            .collect();
        Standard { levels }
    }

    /// A rule's level; a rule the standard doesn't list is off.
    pub fn level(&self, rule: &str) -> Level {
        self.levels.get(rule).copied().unwrap_or(Level::Off)
    }

    /// Every rule the standard lists.
    pub fn rules(&self) -> impl Iterator<Item = &str> {
        self.levels.keys().map(String::as_str)
    }
}
