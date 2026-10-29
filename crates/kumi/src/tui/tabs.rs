//! The right pane's lower half: tabs in a fixed corner, each tab's rows right under its name. A tab is a
//! module that says its title, an optional count, and its rows for a width; the panel owns the strip,
//! which tab is showing, each tab's scroll and the keyboard's place, and draws the window of rows with
//! a quiet "n more" at the ends. HISTORY is the first tab; later ones only need to be registered. Below 100 columns
//! the drawer puts the same tabs on a strip of its own, after NOW and FOCUS, over the rows.

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
    /// Enter, or a click on the row's words, does this instead when the row has it: a job opens (its right edge stops
    /// it), a change opens or closes its picture.
    pub open: Option<Rc<dyn Fn()>>,
    /// Other keys on the row (a letter, an arrow, Space), and what each does.
    pub keys: Vec<(&'static str, Rc<dyn Fn()>)>,
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
    /// Whether it's on the strip: a tab can wait off it until it has something (JOBS, until the first job).
    fn shown(&self) -> bool {
        true
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
    /// The rows last drawn, by the tab's id: the first one shown and the one after the last, the ends' "n more" aside.
    shown: Option<(String, i32, i32)>,
    /// Counts the times the tab showing changed, by a click, a key or Kumi.
    switches: u64,
}

impl State {
    /// The first row last drawn for this tab, when the rows last drawn were its.
    fn first_shown(&self, id: &str) -> Option<i32> {
        self.shown.as_ref().filter(|(shown, ..)| shown == id).map(|(_, first, _)| *first)
    }
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
                    shown: None,
                    switches: 0,
                }),
            }),
        }
    }

    /// The tab showing: the one chosen, or the first on the strip while that one is off it.
    pub fn active_id(&self) -> String {
        let active = self.inner.state.borrow().active.clone();
        if self.inner.tabs.iter().any(|tab| tab.id() == active && tab.shown()) {
            return active;
        }
        self.inner.tabs.iter().find(|tab| tab.shown()).map(|tab| tab.id().to_string()).unwrap_or(active)
    }

    pub fn inside(&self) -> bool {
        self.inner.state.borrow().cursor.is_some()
    }

    /// Every tab's title in the strip's order, those waiting off it too (the help names them).
    pub fn titles(&self) -> Vec<String> {
        self.inner.tabs.iter().map(|tab| tab.title().to_string()).collect()
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
            if !self.inner.tabs.iter().any(|tab| tab.id() == id && tab.shown()) || id == state.active {
                return;
            }
            state.switches += 1;
            state.active = id.to_string();
            state.cursor = state.cursor.map(|_| 0);
        }
        if let Some(on_switch) = &self.inner.on_switch {
            on_switch(id);
        }
    }

    /// How many times the tab showing changed: a drawer that holds the tabs among other things can follow them.
    pub fn switches(&self) -> u64 {
        self.inner.state.borrow().switches
    }

    /// The tabs on the strip, in order: each one's id, title and count.
    pub fn titles(&self) -> Vec<(String, String, Option<i64>)> {
        self.inner
            .tabs
            .iter()
            .filter(|tab| tab.shown())
            .map(|tab| (tab.id().to_string(), tab.title().to_string(), tab.badge().filter(|count| *count != 0)))
            .collect()
    }

    pub fn next(&self) {
        let active = self.active_id();
        let shown: Vec<&Rc<dyn Tab>> = self.inner.tabs.iter().filter(|tab| tab.shown()).collect();
        let index = shown.iter().position(|tab| tab.id() == active).unwrap_or(0);
        let next = shown[(index + 1) % shown.len().max(1)].id().to_string();
        self.show(&next);
    }

    /// Rows down (positive) or up; held within the tab's rows.
    pub fn scroll_by(&self, rows: i32) {
        let active = self.active_id();
        let mut state = self.inner.state.borrow_mut();
        let most = (state.totals.get(&active).copied().unwrap_or(0) - state.last_height).max(0);
        let scroll = (state.scrolls.get(&active).copied().unwrap_or(0) + rows).clamp(0, most.max(0));
        state.scrolls.insert(active, scroll);
    }

    /// The keyboard comes in at the first row showing, or leaves.
    pub fn enter(&self) {
        let active = self.active_id();
        let mut state = self.inner.state.borrow_mut();
        let scroll = state.scrolls.get(&active).copied().unwrap_or(0);
        let first = state.first_shown(&active).unwrap_or(scroll + if scroll > 0 { 1 } else { 0 });
        state.cursor = Some(first.min((state.last_rows.len() as i32 - 1).max(0)));
    }

    pub fn leave(&self) {
        self.inner.state.borrow_mut().cursor = None;
    }

    /// Keys while the keyboard is in the panel: ↑↓ and pages move; Enter opens a row that opens (a job, a change's
    /// picture) or does the row's action; another key is the row's own, if it has it (`keys`). True when used.
    pub fn key(&self, name: &str) -> bool {
        let active = self.active_id();
        let action = {
            let mut state = self.inner.state.borrow_mut();
            let Some(cursor) = state.cursor else { return false };
            let count = state.last_rows.len() as i32;
            let shift = |state: &mut State, by: i32| {
                let cursor = (cursor + by).clamp(0, (count - 1).max(0));
                state.cursor = Some(cursor);
                // Kept in view, clear of the "n more" rows at the ends: a row among those last shown already is.
                if matches!(&state.shown, Some((id, first, end)) if *id == active && (*first..*end).contains(&cursor)) {
                    return;
                }
                let active = active.clone();
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
                "enter" => state.last_rows.get(cursor as usize).and_then(|row| row.open.clone().or_else(|| row.action.clone())),
                key => match state.last_rows.get(cursor as usize).and_then(|row| row.keys.iter().find(|(name, _)| *name == key)) {
                    Some((_, action)) => Some(action.clone()),
                    None => return false,
                },
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
        let active = self.active_id();
        let mut x = area.x;
        for (index, tab) in self.inner.tabs.iter().filter(|tab| tab.shown()).enumerate() {
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
        self.draw_rows(screen, Rect { x: area.x, y: area.y + 1, width: area.width, height: area.height - 1 }, hits);
    }

    /// The active tab's rows filling the area, with a quiet "n more" at the ends; a strip of its own drawn elsewhere
    /// (the drawer's) shows which tab this is.
    pub fn draw_rows(&self, screen: &mut Screen, area: Rect, hits: &mut Vec<Hit>) {
        let faint = Style::fg(palette::FAINT);
        let cursor_band = Style::bg(palette::SELECTED);
        let active = self.active_id();
        let tab = self.inner.tabs.iter().find(|tab| tab.id() == active).expect("the active tab");
        let rows = tab.rows(area.width);
        let height = area.height.max(1);
        let total = rows.len() as i32;
        let mut state = self.inner.state.borrow_mut();
        // Rows that arrive at the top while scrolled down keep the view where it was. Rows of a thing stay put by it:
        // a change that opens adds rows under it, not above.
        let before = state.totals.get(&active).copied();
        let scrolled = state.scrolls.get(&active).copied().unwrap_or(0);
        // The things (rows' `item`s) at the top of the view and under the keyboard's row, in the rows last drawn.
        let previous = state.last_rows.clone();
        let item_at = |at: i32| previous.get(at.max(0) as usize).and_then(|row| row.item.clone());
        let top_row = state.first_shown(&active).unwrap_or(scrolled + if scrolled > 0 { 1 } else { 0 });
        let (top, under) = (item_at(top_row), state.cursor.and_then(item_at));
        let find = |item: &Option<String>| item.as_ref().and_then(|item| rows.iter().position(|row| row.item.as_ref() == Some(item)));
        if let Some(before) = before.filter(|before| *before != total) {
            match find(&top) {
                Some(at) if scrolled > 0 => {
                    state.scrolls.insert(active.clone(), (at as i32 - 1).max(0));
                }
                _ if total > before && scrolled > 0 => {
                    state.scrolls.insert(active.clone(), scrolled + total - before);
                }
                _ => {}
            }
            match (state.cursor, find(&under)) {
                (Some(_), Some(at)) => state.cursor = Some(at as i32),
                (Some(cursor), None) if total > before && cursor > 0 => state.cursor = Some(cursor + total - before),
                _ => {}
            }
        }
        state.totals.insert(active.clone(), total);
        state.last_rows = rows.clone();
        state.last_height = height;
        if rows.is_empty() {
            state.shown = Some((active, 0, 0));
            screen.put(area.x, area.y, &truncate(tab.empty().unwrap_or("Nothing here yet"), area.width), &faint);
            return;
        }
        let scroll = state.scrolls.get(&active).copied().unwrap_or(0).clamp(0, (total - height).max(0));
        state.scrolls.insert(active.clone(), scroll);
        let cursor = state.cursor;
        drop(state);
        let above = scroll;
        let below = (total - scroll - height).max(0);
        let things = |hidden: &[&TabRow]| -> usize {
            hidden
                .iter()
                .filter(|row| !row.spans.is_empty())
                .enumerate()
                .map(|(index, row)| row.item.clone().unwrap_or_else(|| format!("row {index}")))
                .collect::<HashSet<String>>()
                .len()
        };
        // The ends say how much more there is, on a row of their own, counting things rather than rows. Past an end with
        // no other thing beyond it (only more of one in view: an open change's picture), that row shows the next one.
        let ends = |first: i32, last: i32| {
            let shown = &rows[(scroll + first) as usize..((scroll + height - last).max(scroll + first) as usize).min(rows.len())];
            let visible: HashSet<&str> = shown.iter().filter_map(|row| row.item.as_deref()).collect();
            let outside = |row: &&TabRow| row.item.as_deref().is_none_or(|item| !visible.contains(item));
            let hidden_above: Vec<&TabRow> = rows[..(scroll + first) as usize].iter().filter(outside).collect();
            let hidden_below: Vec<&TabRow> =
                rows[((scroll + height - last).max(0) as usize).min(rows.len())..].iter().filter(outside).collect();
            (shown, things(&hidden_above), things(&hidden_below))
        };
        let (first, last) = (i32::from(above > 0), i32::from(below > 0));
        let (_, more_above, more_below) = ends(first, last);
        let (first, last) = (if more_above > 0 { first } else { 0 }, if more_below > 0 { last } else { 0 });
        let (shown, more_above, more_below) = ends(first, last);
        self.inner.state.borrow_mut().shown = Some((active, scroll + first, scroll + first + shown.len() as i32));
        let mut y = area.y;
        if first > 0 {
            screen.put(area.x, y, &format!("↑ {more_above} more"), &faint);
            y += 1;
        }
        for (offset, row) in shown.iter().enumerate() {
            let index = scroll + first + offset as i32;
            if cursor == Some(index) {
                screen.fill(Rect { x: area.x - 1, y, width: area.width + 2, height: 1 }, &cursor_band);
            }
            let right = row.right.as_ref().map_or(0, |right| text_width(&right.text));
            if let Some(open) = &row.open {
                hits.push(Hit {
                    x: area.x,
                    y,
                    width: (area.width - if right > 0 { right + 1 } else { 0 }).max(1),
                    action: Rc::clone(open),
                });
            }
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
            screen.put(area.x, y, &format!("↓ {more_below} more"), &faint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// Twenty rows named by the tab's id ("a0" to "a19"), on the strip while `shown` says.
    struct Rows {
        id: &'static str,
        shown: Rc<Cell<bool>>,
    }

    impl Tab for Rows {
        fn id(&self) -> &str {
            self.id
        }
        fn title(&self) -> &str {
            self.id
        }
        fn rows(&self, _: i32) -> Rc<[TabRow]> {
            (0..20)
                .map(|n| TabRow { spans: vec![Span::styled(format!("{}{n}", self.id), Style::default())], ..Default::default() })
                .collect()
        }
        fn shown(&self) -> bool {
            self.shown.get()
        }
    }

    /// A change whose picture is open, in eight rows, then (with `older`) a change before it.
    struct Opened {
        older: bool,
    }

    impl Tab for Opened {
        fn id(&self) -> &str {
            "history"
        }
        fn title(&self) -> &str {
            "HISTORY"
        }
        fn rows(&self, _: i32) -> Rc<[TabRow]> {
            let row = |text: String, item: &str| TabRow {
                spans: vec![Span::styled(text, Style::default())],
                item: Some(item.into()),
                ..Default::default()
            };
            let older = self.older.then(|| row("older".into(), "c0"));
            [row("change".into(), "c1")].into_iter().chain((0..8).map(|n| row(format!("picture {n}"), "c1"))).chain(older).collect()
        }
    }

    #[test]
    fn an_end_with_only_more_of_what_shows_past_it_shows_its_next_row_not_a_count_of_none() {
        let draw = |panel: &TabPanel| {
            let mut screen = Screen::new(20, 5);
            panel.draw_rows(&mut screen, Rect::new(0, 0, 20, 5), &mut vec![]);
            screen.lines()
        };
        let alone = TabPanel::new(vec![Rc::new(Opened { older: false })], None, None);
        let lines = draw(&alone);
        assert!(lines[0].starts_with("change") && lines[4].starts_with("picture 3"), "{lines:?}");
        // Scrolled into the picture: no ends at all.
        alone.scroll_by(3);
        let lines = draw(&alone);
        assert!(lines[0].starts_with("picture 2") && lines[4].starts_with("picture 6"), "{lines:?}");
        // Another change past it is counted.
        let lines = draw(&TabPanel::new(vec![Rc::new(Opened { older: true })], None, None));
        assert!(lines[3].starts_with("picture 2") && lines[4].starts_with("↓ 1 more"), "{lines:?}");
    }

    #[test]
    fn the_keyboard_comes_in_at_the_first_row_shown_and_moves_through_what_shows_before_it_scrolls() {
        let draw = |panel: &TabPanel| {
            let mut screen = Screen::new(20, 5);
            panel.draw_rows(&mut screen, Rect::new(0, 0, 20, 5), &mut vec![]);
            screen.lines()
        };
        let panel = TabPanel::new(vec![Rc::new(Opened { older: false })], None, None);
        draw(&panel);
        // Scrolled into the picture, with no ends: the first row showing is picture 2, the change's fourth row.
        panel.scroll_by(3);
        draw(&panel);
        panel.enter();
        assert_eq!(panel.cursor(), Some(3));
        for _ in 0..4 {
            assert!(panel.key("down"));
        }
        assert_eq!(panel.cursor(), Some(7));
        let lines = draw(&panel);
        assert!(lines[0].starts_with("picture 2") && lines[4].starts_with("picture 6"), "the view stays: {lines:?}");
        // One more goes past what shows: the view follows.
        assert!(panel.key("down"));
        assert!(draw(&panel).iter().any(|line| line.starts_with("picture 7")));
    }

    #[test]
    fn the_wheel_and_the_keyboard_move_the_tab_showing_while_the_chosen_one_is_off_the_strip() {
        let jobs = Rc::new(Cell::new(true));
        let tabs: Vec<Rc<dyn Tab>> =
            vec![Rc::new(Rows { id: "a", shown: Rc::new(Cell::new(true)) }), Rc::new(Rows { id: "b", shown: jobs.clone() })];
        let panel = TabPanel::new(tabs, Some("b"), None);
        let draw = |panel: &TabPanel| {
            let mut screen = Screen::new(20, 7);
            panel.draw(&mut screen, Rect::new(0, 0, 20, 7), &mut vec![]);
            screen.lines()
        };
        // The chosen tab leaves the strip (JOBS between its last job and the next): the first tab shows in its place.
        jobs.set(false);
        assert!(draw(&panel)[1].starts_with("a0"));
        panel.scroll_by(3);
        let lines = draw(&panel);
        assert!(lines[1].starts_with("↑ 4 more") && lines[2].starts_with("a4"), "{lines:?}");
        panel.enter();
        assert_eq!(panel.cursor(), Some(4), "the keyboard comes in at the first row showing");
        assert!(panel.key("pagedown"));
        let lines = draw(&panel);
        assert!(lines.iter().any(|line| line.starts_with("a10")), "the keyboard's row in view: {lines:?}");
    }
}
