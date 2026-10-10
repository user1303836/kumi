//! What Kumi's checks know of Max's objects beyond what a patcher says about them, from `max-standard/objects.json`:
//! aliases, which inlets make an object send, which objects send later, which name something every copy of a device
//! shares. A class it doesn't list follows Max's convention, its left inlet the only hot one.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use serde_json::Value;

/// Which of an object's inlets make it send.
#[derive(Debug, Clone, PartialEq)]
enum Inlets {
    All,
    Hot(Vec<usize>),
    Cold(Vec<usize>),
}

/// What an object is for, as a patcher's colours can show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Dsp,
    Storage,
    Timing,
    Wireless,
}

#[derive(Debug, Default)]
pub struct Catalog {
    aliases: HashMap<String, String>,
    inlets: HashMap<String, Inlets>,
    deferred: HashSet<String>,
    audio_with_messages: HashSet<String>,
    named: HashSet<String>,
    roles: HashMap<String, Role>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    Catalog::from_value(&serde_json::from_str(include_str!("../../../../../max-standard/objects.json")).expect("objects.json"))
});

/// The catalog Kumi carries.
pub fn catalog() -> &'static Catalog {
    &CATALOG
}

fn names(value: &Value) -> impl Iterator<Item = String> + '_ {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string)
}

impl Catalog {
    pub fn from_value(value: &Value) -> Catalog {
        let mut catalog = Catalog::default();
        for (alias, class) in value["aliases"].as_object().into_iter().flatten() {
            if let Some(class) = class.as_str() {
                catalog.aliases.insert(alias.clone(), class.to_string());
            }
        }
        let indexes = |value: &Value| value.as_array().into_iter().flatten().filter_map(Value::as_u64).map(|n| n as usize).collect();
        for (class, inlets) in value["inlets"].as_object().into_iter().flatten() {
            let inlets = match inlets {
                Value::String(all) if all == "all" => Inlets::All,
                Value::Object(map) if map.contains_key("hot") => Inlets::Hot(indexes(&map["hot"])),
                Value::Object(map) if map.contains_key("cold") => Inlets::Cold(indexes(&map["cold"])),
                _ => continue,
            };
            catalog.inlets.insert(class.clone(), inlets);
        }
        catalog.deferred = names(&value["deferred"]["classes"]).collect();
        catalog.audio_with_messages = names(&value["audio_with_messages"]["classes"]).collect();
        catalog.named = names(&value["named"]["classes"]).collect();
        for (role, key) in [(Role::Dsp, "dsp"), (Role::Storage, "storage"), (Role::Timing, "timing"), (Role::Wireless, "wireless")] {
            for class in names(&value["roles"][key]) {
                catalog.roles.insert(class, role);
            }
        }
        catalog
    }

    /// The class an alias stands for ("t" is trigger), or the class itself.
    pub fn canonical<'a>(&'a self, class: &'a str) -> &'a str {
        self.aliases.get(class).map(String::as_str).unwrap_or(class)
    }

    /// Whether a message into `inlet` makes the object send (a cold inlet only keeps it).
    pub fn hot(&self, class: &str, inlet: usize) -> bool {
        match self.inlets.get(self.canonical(class)) {
            Some(Inlets::All) => true,
            Some(Inlets::Hot(hot)) => hot.contains(&inlet),
            Some(Inlets::Cold(cold)) => !cold.contains(&inlet),
            None => inlet == 0,
        }
    }

    /// Whether the object decides its hot inlets itself (code, a subpatcher), so no inlet of it is known to be cold.
    pub fn all_hot(&self, class: &str) -> bool {
        matches!(self.inlets.get(self.canonical(class)), Some(Inlets::All))
    }

    /// Whether the object sends what reaches it later, out of the chain of messages that reached it.
    pub fn defers(&self, class: &str) -> bool {
        self.deferred.contains(self.canonical(class))
    }

    /// Whether a message into the object goes no further as a message: an audio object (named with ~) that doesn't
    /// answer one at once.
    pub fn ends_messages(&self, class: &str) -> bool {
        let class = self.canonical(class);
        class.ends_with('~') && !self.audio_with_messages.contains(class)
    }

    /// Whether the object's first argument names something every patcher using the name shares.
    pub fn names_something(&self, class: &str) -> bool {
        self.named.contains(self.canonical(class))
    }

    pub fn role(&self, class: &str) -> Option<Role> {
        self.roles.get(self.canonical(class)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_left_inlet_is_hot_unless_the_catalog_says_otherwise() {
        let catalog = catalog();
        assert!(catalog.hot("+", 0) && !catalog.hot("+", 1), "Max's convention");
        assert!(!catalog.hot("gate", 0) && catalog.hot("gate", 1), "gate's data comes in on the right");
        assert!(!catalog.hot("switch", 0) && catalog.hot("switch", 2));
        assert!(catalog.hot("pak", 2) && catalog.all_hot("pak"));
        assert!(catalog.hot("counter", 3) && !catalog.hot("counter", 1));
        assert_eq!(catalog.canonical("t"), "trigger");
        assert!(catalog.defers("del") && catalog.defers("qlim") && !catalog.defers("speedlim"));
        assert!(catalog.ends_messages("*~") && !catalog.ends_messages("snapshot~") && !catalog.ends_messages("prepend"));
        assert!(catalog.names_something("s") && catalog.names_something("buffer~") && !catalog.names_something("pv"));
        assert_eq!(catalog.role("buffer~"), Some(Role::Storage));
        assert_eq!(catalog.role("prepend"), None);
    }
}
