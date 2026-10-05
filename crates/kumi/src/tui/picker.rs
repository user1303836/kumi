//! A list to choose from in a panel above the input box: models grouped by provider, effort
//! levels, providers to sign in to. Typing filters it; headings stay with what they head.

use kumi_common::js;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteTone {
    Accent,
    Faint,
    Warn,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickerItem {
    pub label: String,
    /// Beside the label, dimmer: a model's description, where a key comes from.
    pub detail: Option<String>,
    /// At the right edge: "current", "signed in", "sign in".
    pub note: Option<String>,
    pub note_tone: Option<NoteTone>,
    /// A heading groups the items under it and can't be chosen.
    pub heading: bool,
    /// Shown but not choosable (a list still loading).
    pub inert: bool,
    pub value: Option<String>,
}

impl PickerItem {
    /// An item with a label and a value.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> PickerItem {
        PickerItem { label: label.into(), value: Some(value.into()), ..Default::default() }
    }

    pub fn heading(label: impl Into<String>) -> PickerItem {
        PickerItem { label: label.into(), heading: true, ..Default::default() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickerOptions {
    pub filterable: bool,
    pub hint: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Picker {
    pub title: String,
    pub options: PickerOptions,
    pub filter: String,
    items: Vec<PickerItem>,
    index: usize,
}

impl Picker {
    pub fn new(title: impl Into<String>, items: Vec<PickerItem>) -> Picker {
        Picker::with_options(title, items, PickerOptions::default())
    }

    pub fn with_options(title: impl Into<String>, items: Vec<PickerItem>, options: PickerOptions) -> Picker {
        let mut picker = Picker { title: title.into(), options, filter: String::new(), items, index: 0 };
        picker.index = picker.choosable().iter().position(|item| item.note.as_deref() == Some("current")).unwrap_or(0);
        picker
    }

    /// Swap the items (a list finished loading), keeping the chosen one when it's still there.
    pub fn set_items(&mut self, items: Vec<PickerItem>) {
        let chosen = self.selected().and_then(|item| item.value.clone());
        self.items = items;
        let at = chosen.and_then(|chosen| self.choosable().iter().position(|item| item.value.as_ref() == Some(&chosen)));
        self.index = at.unwrap_or_else(|| self.index.min(self.choosable().len().saturating_sub(1)));
    }

    /// What's shown: every item matching the filter, under its heading.
    pub fn visible(&self) -> Vec<&PickerItem> {
        let lower = self.filter.to_lowercase();
        let words: Vec<&str> = lower.split_whitespace().collect();
        if words.is_empty() {
            return self.items.iter().collect();
        }
        let mut shown: Vec<&PickerItem> = Vec::new();
        let mut heading: Option<&PickerItem> = None;
        for item in &self.items {
            if item.heading {
                heading = Some(item);
                continue;
            }
            let haystack = format!(
                "{} {} {} {}",
                heading.map(|heading| heading.label.as_str()).unwrap_or(""),
                item.label,
                item.detail.as_deref().unwrap_or(""),
                item.value.as_deref().unwrap_or("")
            )
            .to_lowercase();
            if !words.iter().all(|word| haystack.contains(word)) {
                continue;
            }
            if let Some(heading) = heading {
                if !shown.last().is_some_and(|last| std::ptr::eq(*last, heading))
                    && !shown.iter().any(|shown| std::ptr::eq(*shown, heading))
                {
                    shown.push(heading);
                }
            }
            shown.push(item);
        }
        shown
    }

    fn choosable(&self) -> Vec<&PickerItem> {
        self.visible().into_iter().filter(|item| !item.heading && !item.inert).collect()
    }

    pub fn selected(&self) -> Option<&PickerItem> {
        self.choosable().get(self.index).copied()
    }

    /// Put the selection on the item with `value`, when it's there.
    pub fn select(&mut self, value: Option<&str>) {
        let at = value.and_then(|value| self.choosable().iter().position(|item| item.value.as_deref() == Some(value)));
        if let Some(at) = at {
            self.index = at;
        }
    }

    pub fn r#move(&mut self, delta: i32) {
        let count = self.choosable().len() as i64;
        if count > 0 {
            self.index = (self.index as i64 + delta as i64 + count).rem_euclid(count) as usize;
        }
    }

    pub fn r#type(&mut self, text: &str) {
        if !self.options.filterable {
            return;
        }
        let typed: String = text.chars().filter(|character| !matches!(character, '\r' | '\n' | '\t')).collect();
        self.filter = js::string::head(&(self.filter.clone() + &typed), 40);
        self.index = 0;
    }

    pub fn erase(&mut self) {
        self.filter = js::string::slice(&self.filter, 0, Some(-1));
        self.index = 0;
    }
}
