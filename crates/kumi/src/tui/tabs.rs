//! The right pane's lower half: tabs in a fixed corner, each tab's rows right under its name. A tab is a
//! module that says its title, an optional count, and its rows for a width; the panel owns the strip,
//! which tab is showing, each tab's scroll and the keyboard's place, and draws the window of rows with
//! a quiet "n more" at the ends. HISTORY is the first tab; later ones only need to be registered.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::screen::{Rect, Screen};
use super::style::{palette, Style};
use super::width::{text_width, truncate};
use super::wrap::Span;

/// One row of a tab: its text, what's at its right edge, and what choosing it does.
#[derive(Clone, Default)]
pub struct TabRow {
    pub spans: Vec<Span>,
    pub right: Option<Span>,
    /// Clicking the right edge, or Enter on the row, does this.
    pub action: Option<Rc<dyn Fn()>>,
    /// Rows of one thing (a change whose title takes two) share this, so "n more" counts things, not rows.
    pub item: Option<String>,
}

pub trait Tab {
    fn id(&self) -> &str;
    fn title(&self) -> &str;
    /// A dim count beside the title, when there's something to count.
    fn badge(&self) -> Option<i64> {
        None
    }
    /// The tab's rows at this width, first row on top: shared, so a tab that keeps them needn't copy them each frame.
    fn rows(&self, width: i32) -> Rc<[TabRow]>;
    /// What it says when it has no rows.
    fn empty(&self) -> Option<&str> {
        None
    }
}

/// A place on screen that reacts to a click.
#[derive(Clone)]
pub struct Hit {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub action: Rc<dyn Fn()>,
}

/// Told which tab was shown, by id.
pub type SwitchListener = Rc<dyn Fn(&str)>;

struct State {
    active: String,
    scrolls: HashMap<String, i32>,
    totals: HashMap<String, i32>,
    /// The keyboard's row in the active tab, while the keyboard is in the panel.
    cursor: Option<i32>,
    last_rows: Rc<[TabRow]>,
    last_height: i32,
}

struct Inner {
    tabs: Vec<Rc<dyn Tab>>,
    on_switch: Option<SwitchListener>,
    state: RefCell<State>,
}

/// A handle: clones share the one panel, as the strip's click actions need to.
#[derive(Clone)]
pub struct TabPanel {
    inner: Rc<Inner>,
}

impl TabPanel {
    pub fn new(tabs: Vec<Rc<dyn Tab>>, active: Option<&str>, on_switch: Option<SwitchListener>) -> TabPanel {
        assert!(!tabs.is_empty(), "a tab panel needs a tab");
        let active = match active {
            Some(id) if tabs.iter().any(|tab| tab.id() == id) => id.to_string(),
            _ => tabs[0].id().to_string(),
        };
        TabPanel {
            inner: Rc::new(Inner {
                tabs,
                on_switch,
                state: RefCell::new(State {
                    active,
                    scrolls: HashMap::new(),
                    totals: HashMap::new(),
                    cursor: None,
                    last_rows: Rc::default(),
                    last_height: 1,
                }),
            }),
        }
    }

    pub fn active_id(&self) -> String {
        self.inner.state.borrow().active.clone()
    }

    pub fn inside(&self) -> bool {
        self.inner.state.borrow().cursor.is_some()
    }

    /// The keyboard's row in the active tab, while the keyboard is in the panel.
    pub fn cursor(&self) -> Option<i32> {
        self.inner.state.borrow().cursor
    }

    pub fn set_cursor(&self, cursor: Option<i32>) {
        self.inner.state.borrow_mut().cursor = cursor;
    }

    /// Show a tab (by click on the strip, or the next one by key); with one tab, nothing changes.
    pub fn show(&self, id: &str) {
        {
            let mut state = self.inner.state.borrow_mut();
            if !self.inner.tabs.iter().any(|tab| tab.id() == id) || id == state.active {
                return;
            }
            state.active = id.to_string();
            state.cursor = state.cursor.map(|_| 0);
        }
        if let Some(on_switch) = &self.inner.on_switch {
            on_switch(id);
        }
    }

    pub fn next(&self) {
        let active = self.active_id();
        let index = self.inner.tabs.iter().position(|tab| tab.id() == active).unwrap_or(0);
        let next = self.inner.tabs[(index + 1) % self.inner.tabs.len()].id().to_string();
        self.show(&next);
    }

    /// Rows down (positive) or up; held within the tab's rows.
    pub fn scroll_by(&self, rows: i32) {
        let mut state = self.inner.state.borrow_mut();
        let active = state.active.clone();
        let most = (state.totals.get(&active).copied().unwrap_or(0) - state.last_height).max(0);
        let scroll = (state.scrolls.get(&active).copied().unwrap_or(0) + rows).clamp(0, most.max(0));
        state.scrolls.insert(active, scroll);
    }

    /// The keyboard comes in at the first row showing, or leaves.
    pub fn enter(&self) {
        let mut state = self.inner.state.borrow_mut();
        let scroll = state.scrolls.get(&state.active).copied().unwrap_or(0);
        state.cursor = Some((scroll + if scroll > 0 { 1 } else { 0 }).min((state.last_rows.len() as i32 - 1).max(0)));
    }

    pub fn leave(&self) {
        self.inner.state.borrow_mut().cursor = None;
    }

    /// Keys while the keyboard is in the panel: arrows and pages move, Enter does the row's action. True when used.
    pub fn key(&self, name: &str) -> bool {
        let action = {
            let mut state = self.inner.state.borrow_mut();
            let Some(cursor) = state.cursor else { return false };
            let count = state.last_rows.len() as i32;
            let shift = |state: &mut State, by: i32| {
                let cursor = (cursor + by).clamp(0, (count - 1).max(0));
                state.cursor = Some(cursor);
                // Kept in view, clear of the "n more" rows at the ends.
                let active = state.active.clone();
                let scroll = state.scrolls.get(&active).copied().unwrap_or(0);
                let height = state.last_height;
                if cursor < scroll + if scroll > 0 { 1 } else { 0 } {
                    state.scrolls.insert(active, (cursor - 1).max(0));
                } else if cursor > scroll + height - 2 && cursor < count - 1 {
                    state.scrolls.insert(active, cursor - height + 2);
                } else if cursor == count - 1 {
                    state.scrolls.insert(active, (count - height).max(0));
                }
            };
            match name {
                "up" => {
                    shift(&mut state, -1);
                    return true;
                }
                "down" => {
                    shift(&mut state, 1);
                    return true;
                }
                "pageup" => {
                    let by = -state.last_height;
                    shift(&mut state, by);
                    return true;
                }
                "pagedown" => {
                    let by = state.last_height;
                    shift(&mut state, by);
                    return true;
                }
                "enter" => state.last_rows.get(cursor as usize).and_then(|row| row.action.clone()),
                _ => return false,
            }
        };
        if let Some(action) = action {
            action();
        }
        true
    }

    /// The strip on the area's first row, then the active tab's rows filling the rest.
    pub fn draw(&self, screen: &mut Screen, area: Rect, hits: &mut Vec<Hit>) {
        let faint = Style::fg(palette::FAINT);
        let dim = Style::fg(palette::DIM);
        let accent = Style::fg(palette::ACCENT);
        let rule = Style::fg(palette::RULE);
        let cursor_band = Style::bg(palette::SELECTED);
        let active = self.active_id();
        let mut x = area.x;
        for (index, tab) in self.inner.tabs.iter().enumerate() {
            if index > 0 {
                x = screen.put(x, area.y, "   ", &faint);
            }
            let title = if tab.id() == active { accent } else { dim };
            let start = x;
            x = screen.put(x, area.y, tab.title(), &title);
            if let Some(count) = tab.badge().filter(|count| *count != 0) {
                x = screen.put(x, area.y, &format!(" {count}"), &faint);
            }
            let panel = self.clone();
            let id = tab.id().to_string();
            hits.push(Hit { x: start, y: area.y, width: x - start, action: Rc::new(move || panel.show(&id)) });
        }
        // A quiet rule to the edge: the strip reads as a heading over its rows, finished with one tab too.
        if x + 1 < area.x + area.width {
            screen.put(x + 1, area.y, &"─".repeat((area.x + area.width - x - 1) as usize), &rule);
        }
        let tab = self.inner.tabs.iter().find(|tab| tab.id() == active).expect("the active tab");
        let rows = tab.rows(area.width);
        let height = (area.height - 1).max(1);
        let total = rows.len() as i32;
        let mut state = self.inner.state.borrow_mut();
        // Rows that arrive at the top while scrolled down keep the view where it was.
        let before = state.totals.get(&active).copied();
        let scrolled = state.scrolls.get(&active).copied().unwrap_or(0);
        if let Some(before) = before {
            if total > before && scrolled > 0 {
                state.scrolls.insert(active.clone(), scrolled + total - before);
            }
            if total > before && state.cursor.is_some_and(|cursor| cursor > 0) {
                state.cursor = state.cursor.map(|cursor| cursor + total - before);
            }
        }
        state.totals.insert(active.clone(), total);
        state.last_rows = rows.clone();
        state.last_height = height;
        if rows.is_empty() {
            screen.put(area.x, area.y + 1, &truncate(tab.empty().unwrap_or("Nothing here yet"), area.width), &faint);
            return;
        }
        let scroll = state.scrolls.get(&active).copied().unwrap_or(0).clamp(0, (total - height).max(0));
        state.scrolls.insert(active.clone(), scroll);
        let cursor = state.cursor;
        drop(state);
        let above = scroll;
        let below = (total - scroll - height).max(0);
        // The ends say how much more there is, on a row of their own, counting things rather than rows.
        let first = if above > 0 { 1 } else { 0 };
        let last = if below > 0 { 1 } else { 0 };
        let shown = &rows[(scroll + first) as usize..((scroll + height - last).max(scroll + first) as usize).min(rows.len())];
        let things = |hidden: &[&TabRow]| -> usize {
            hidden
                .iter()
                .filter(|row| !row.spans.is_empty())
                .enumerate()
                .map(|(index, row)| row.item.clone().unwrap_or_else(|| format!("row {index}")))
                .collect::<HashSet<String>>()
                .len()
        };
        let visible: HashSet<&str> = shown.iter().filter_map(|row| row.item.as_deref()).collect();
        let outside = |row: &&TabRow| row.item.as_deref().is_none_or(|item| !visible.contains(item));
        let hidden_above: Vec<&TabRow> = rows[..(scroll + first) as usize].iter().filter(outside).collect();
        let hidden_below: Vec<&TabRow> =
            rows[((scroll + height - last).max(0) as usize).min(rows.len())..].iter().filter(outside).collect();
        let mut y = area.y + 1;
        if first > 0 {
            screen.put(area.x, y, &format!("↑ {} more", things(&hidden_above)), &faint);
            y += 1;
        }
        for (offset, row) in shown.iter().enumerate() {
            let index = scroll + first + offset as i32;
            if cursor == Some(index) {
                screen.fill(Rect { x: area.x - 1, y, width: area.width + 2, height: 1 }, &cursor_band);
            }
            let right = row.right.as_ref().map_or(0, |right| text_width(&right.text));
            let mut column = area.x;
            for span in &row.spans {
                let room = area.x + area.width - column - if right > 0 { right + 1 } else { 0 };
                if room <= 0 {
                    break;
                }
                column = screen.put(column, y, &truncate(&span.text, room), &span.style);
            }
            if let Some(right_span) = &row.right {
                let at = area.x + area.width - right;
                screen.put(at, y, &right_span.text, &right_span.style);
                if let Some(action) = &row.action {
                    hits.push(Hit { x: at, y, width: right, action: Rc::clone(action) });
                }
            }
            y += 1;
        }
        if last > 0 {
            screen.put(area.x, y, &format!("↓ {} more", things(&hidden_below)), &faint);
        }
    }
}
