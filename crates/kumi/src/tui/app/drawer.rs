//! Below 100 columns, with 24 rows or more, the pane is a drawer under the conversation: a row of tabs (NOW, FOCUS,
//! then the tab panel's HISTORY, GOAL, JOBS and PALETTE) over the tab it shows, 6 to 10 rows by the window's height.
//! A change landing opens it to NOW, and Kumi turning the tabs (to GOAL) opens a folded one on them; what Kumi opened
//! folds back to its row once nothing new has landed for a while. A drawer the producer opened keeps their tab, and one
//! they closed stays folded until they open it. It never takes the keyboard. ctrl+\ (or a click on the tab showing)
//! opens and closes it. At other sizes ctrl+\ puts the pane away and brings it back: the side pane at full width, the
//! two-line strip below 24 rows.

use super::super::{
    activity::activity_glyph,
    pictures::Graphics,
    screen::Screen,
    style::{palette, Style},
    width::{text_width, truncate},
    wrap::Span,
};
use super::draw_live::{change_marks, FocusRoom};
use super::drawing::{sp, spans, st, NowLine};
use super::*;

/// The columns the side pane needs: below them, the drawer or the strip.
pub(super) const PANE_COLUMNS: i32 = 100;
/// The rows the drawer needs: below them, the two-line strip.
const DRAWER_ROWS: i32 = 24;
/// How long a drawer Kumi opened stays open once nothing new lands in it.
const QUIET_MS: u64 = 10_000;

/// What the drawer shows: NOW, FOCUS, or the tab panel's own tab (HISTORY, GOAL, JOBS or PALETTE).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum DrawerTab {
    #[default]
    Now,
    Focus,
    Tabs,
}

/// Who opened the drawer: Kumi (counted, so only its latest opening folds it), or the producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opened {
    Kumi(u64),
    Hand,
}

#[derive(Default)]
pub(super) struct Drawer {
    tab: DrawerTab,
    /// None while it's folded to its row of tabs.
    opened: Option<Opened>,
    openings: u64,
    /// The producer closed it: Kumi leaves it folded until they open it.
    closed: bool,
    /// What it followed last: the latest change's time, and how often the tab panel's tab changed.
    seen_change: Option<f64>,
    seen_switches: u64,
    /// Where it was drawn this frame, if it was: a drag that starts in it stays in it.
    pub(super) area: Option<Rect>,
}

impl Drawer {
    fn is_open(&self) -> bool {
        self.opened.is_some()
    }

    /// A change landed: it opens on NOW, as Kumi's. One the producer opened keeps their tab (HISTORY's count says so).
    fn change_landed(&mut self) -> Option<u64> {
        if self.opened == Some(Opened::Hand) {
            return None;
        }
        self.tab = DrawerTab::Now;
        self.kumi_opens()
    }

    /// The tab panel turned to another tab (GOAL, as a goal starts): a folded drawer opens on it, and one Kumi opened
    /// on the tabs stays open longer; one open on something else keeps showing that.
    fn tabs_turned(&mut self) -> Option<u64> {
        match self.opened {
            None => {
                self.tab = DrawerTab::Tabs;
                self.kumi_opens()
            }
            Some(Opened::Kumi(_)) if self.tab == DrawerTab::Tabs => self.kumi_opens(),
            _ => None,
        }
    }

    fn kumi_opens(&mut self) -> Option<u64> {
        self.openings += 1;
        self.opened = Some(Opened::Kumi(self.openings));
        Some(self.openings)
    }
}

/// Whether the pane is the drawer at this size.
pub(super) fn drawer_fits(columns: i32, rows: i32) -> bool {
    columns < PANE_COLUMNS && rows >= DRAWER_ROWS
}

impl TuiApp {
    /// The drawer follows what landed since the last frame: a new change opens it to NOW, and the tab panel turning to
    /// another tab (GOAL as a goal starts) opens a folded one on it. Only while it's on screen, the producer hasn't
    /// closed it and the keyboard isn't in it; otherwise what landed is only noted, so it doesn't open later for
    /// something long gone.
    pub(super) fn follow_drawer(&self, shown: bool) {
        let switches = self.0.tabs.switches();
        let keys = self.0.tabs.inside() || self.0.state.borrow().tree_cursor.is_some();
        let opening = {
            let mut state = self.0.state.borrow_mut();
            let latest = state.last_change.as_ref().map(|(_, at)| *at);
            let drawer = &mut state.drawer;
            let open = shown && !drawer.closed && !keys;
            let mut opening = None;
            if switches != drawer.seen_switches {
                drawer.seen_switches = switches;
                if open {
                    opening = drawer.tabs_turned();
                }
            }
            // (a change and the tabs at once: the change)
            if latest != drawer.seen_change {
                drawer.seen_change = latest;
                if open && latest.is_some() {
                    opening = drawer.change_landed().or(opening);
                }
            }
            opening
        };
        if let Some(opening) = opening {
            self.fold_later(opening);
        }
    }

    /// Kumi's opening folds once nothing new has landed for a while: a later opening, or the producer's, keeps it open.
    fn fold_later(&self, opening: u64) {
        let weak = Rc::downgrade(&self.0);
        // From when it opened, not from when the timer first runs; kept with the flashes' timers, which quitting aborts.
        let at = tokio::time::Instant::now() + Duration::from_millis(QUIET_MS);
        let timer = tokio::task::spawn_local(async move {
            tokio::time::sleep_until(at).await;
            if let Some(inner) = weak.upgrade() {
                let mut state = inner.state.borrow_mut();
                if state.drawer.opened == Some(Opened::Kumi(opening)) && !state.closing {
                    state.drawer.opened = None;
                    drop(state);
                    inner.scheduler.request();
                }
            }
        });
        let mut state = self.0.state.borrow_mut();
        state.flash_timers.retain(|timer| !timer.is_finished());
        state.flash_timers.push(timer);
    }

    /// ctrl+\: the drawer opens or closes; at other sizes the pane is put away or brought back. Put away with the
    /// keyboard in it, the keyboard goes back to the box.
    pub(super) fn toggle_pane(&self) {
        let size = self.0.tty.size();
        let shown = {
            let mut state = self.0.state.borrow_mut();
            if drawer_fits(size.columns, size.rows) {
                let open = !state.drawer.is_open();
                state.drawer.opened = open.then_some(Opened::Hand);
                state.drawer.closed = !open;
                open
            } else {
                state.pane_away = !state.pane_away;
                !state.pane_away
            }
        };
        if !shown {
            self.0.tabs.leave();
            self.0.state.borrow_mut().tree_cursor = None;
        }
        self.0.scheduler.request();
    }

    /// The producer goes into the pane (a click on a drawer's tab, Shift-Tab into the tabs, Tab into FOCUS's tree,
    /// `/goal` to see the goal): it shows that part, and the drawer stays open until they close it.
    pub(super) fn open_pane(&self, tab: DrawerTab) {
        let size = self.0.tty.size();
        let mut state = self.0.state.borrow_mut();
        if drawer_fits(size.columns, size.rows) {
            state.drawer.tab = tab;
            state.drawer.opened = Some(Opened::Hand);
            state.drawer.closed = false;
        } else {
            state.pane_away = false;
        }
    }

    /// A click on one of the drawer's tabs (`id`, the tab panel's): on the one showing it closes the drawer, as ctrl+\
    /// does; on another it opens there.
    fn click_drawer_tab(&self, tab: DrawerTab, id: Option<&str>) {
        let showing = self.0.state.borrow().drawer.is_open()
            && self.0.state.borrow().drawer.tab == tab
            && id.is_none_or(|id| id == self.0.tabs.active_id());
        if showing {
            self.toggle_pane();
            return;
        }
        self.open_pane(tab);
        if let Some(id) = id {
            self.0.tabs.show(id);
        }
    }

    /// The drawer's rows: its row of tabs folded, 6 to 10 open by the window's.
    pub(super) fn drawer_height(&self, rows: i32) -> i32 {
        if self.0.state.borrow().drawer.is_open() {
            (rows / 4).clamp(6, 10)
        } else {
            1
        }
    }

    /// The drawer at `area`: its row of tabs, then the tab it shows while it's open.
    pub(super) fn draw_drawer(&self, screen: &mut Screen, area: Rect) {
        self.0.state.borrow_mut().drawer.area = Some(area);
        screen.fill(area, &st::SURFACE);
        let (x, width) = (area.x + 2, area.width - 4);
        let (tab, open) = {
            let state = self.0.state.borrow();
            (state.drawer.tab, state.drawer.is_open())
        };
        // NOW's words without the change that just landed: the drawer shows it on a row of its own.
        let now = self.now_said(false);
        let body = Rect::new(x, area.y + 1, width, area.height - 1);
        let mut heading = None;
        match tab {
            _ if !open || body.height <= 0 => self.keep_tree_fresh(None),
            DrawerTab::Now => {
                self.keep_tree_fresh(None);
                self.draw_drawer_now(screen, body, &now);
            }
            DrawerTab::Focus => {
                let focus = self.draw_focus(screen, x, body.y, width, FocusRoom::Rows(body.height));
                heading = focus.busy.map(|said| (said, st::WARN)).or(focus.heading.map(|heading| (heading, st::FAINT)));
                // Bodies draw as tall as they are: what runs past the drawer is painted over, and can't be clicked.
                let below = Rect::new(0, area.y + area.height, screen.width, (screen.height - area.y - area.height).max(0));
                screen.fill(below, &st::GROUND);
                self.cover(below);
            }
            DrawerTab::Tabs => {
                self.keep_tree_fresh(None);
                self.0.state.borrow_mut().tabs_area = Some(body);
                let mut hits = vec![];
                self.0.tabs.draw_rows(screen, body, &mut hits);
                self.0.state.borrow_mut().hits.extend(hits);
            }
        }
        self.draw_drawer_strip(screen, x, area.y, width, tab, open, heading, &now);
    }

    /// The drawer's row of tabs: NOW, FOCUS (with its heading while it shows), then the tab panel's with their counts,
    /// each a click away; at its end what Kumi is at (working, a goal, a loop), or the key that opens and closes it.
    #[allow(clippy::too_many_arguments)]
    fn draw_drawer_strip(
        &self,
        screen: &mut Screen,
        x: i32,
        y: i32,
        width: i32,
        tab: DrawerTab,
        open: bool,
        heading: Option<(String, Style)>,
        now: &NowLine,
    ) {
        let titles = self.0.tabs.titles();
        let active = self.0.tabs.active_id();
        let counted = |badge: &Option<i64>| badge.map_or(0, |count| text_width(&format!(" {count}")));
        let named = text_width("NOW")
            + 3
            + text_width("FOCUS")
            + titles.iter().map(|(_, title, badge)| 3 + text_width(title) + counted(badge)).sum::<i32>();
        // The end, in what's left after the tabs and a rule's start.
        let room = width - named - 3;
        let end: Vec<Span> = match now.dot {
            Some(dot) if room >= 8 => vec![sp("● ", dot), sp(truncate(&now.label, room - 2), st::DIM)],
            None if room >= 14 => vec![sp(if open { "ctrl+\\ closes" } else { "ctrl+\\ opens" }, st::FAINT)],
            _ => vec![],
        };
        let ends = end.iter().map(|span| text_width(&span.text)).sum::<i32>();
        let room = width - named - ends - 3;
        let heading = heading.filter(|_| tab == DrawerTab::Focus && open && room >= 6);
        let style = |chosen: bool| if chosen { st::ACCENT } else { st::DIM };
        let start = x;
        let mut column = screen.put(x, y, "NOW", &style(tab == DrawerTab::Now));
        self.hit(start, y, column - start, |app| app.click_drawer_tab(DrawerTab::Now, None));
        column = screen.put(column, y, "   ", &st::FAINT);
        let start = column;
        column = screen.put(column, y, "FOCUS", &style(tab == DrawerTab::Focus));
        if let Some((words, style)) = &heading {
            column = screen.put(column, y, &truncate(words, room), style);
        }
        self.hit(start, y, column - start, |app| app.click_drawer_tab(DrawerTab::Focus, None));
        for (id, title, badge) in titles {
            column = screen.put(column, y, "   ", &st::FAINT);
            let start = column;
            column = screen.put(column, y, &title, &style(tab == DrawerTab::Tabs && id == active));
            if let Some(count) = badge {
                column = screen.put(column, y, &format!(" {count}"), &st::FAINT);
            }
            self.hit(start, y, column - start, move |app| app.click_drawer_tab(DrawerTab::Tabs, Some(&id)));
        }
        // A quiet rule to the end, as the pane's tabs have; the key's words there open and close it too.
        let at = x + width - ends;
        if at - column >= 3 {
            screen.put(column + 1, y, &"─".repeat((at - column - 2) as usize), &Style::fg(palette::RULE));
            spans(screen, at, y, &end, None);
            if now.dot.is_none() && ends > 0 {
                self.hit(at, y, ends, |app| app.toggle_pane());
            }
        }
    }

    /// NOW in the drawer: what Kumi is doing, then the latest change as HISTORY marks it (its undo too), with its
    /// picture under it as tall as the drawer has room for.
    fn draw_drawer_now(&self, screen: &mut Screen, body: Rect, now: &NowLine) {
        let (x, width) = (body.x, body.width);
        let mut y = body.y;
        if let Some((kind, since)) = now.activity {
            // One glyph turning beside steady words, as in the pane.
            let glyph = activity_glyph(kind, perf_now() - since, self.0.icons == IconStyle::Badges);
            screen.put(x, y, &glyph.text, &glyph.style);
            screen.put(x + 2, y, &truncate(&now.detail, (width - 2).max(1)), &now.style);
        } else {
            screen.put(x, y, &truncate(&now.detail, width), &now.style);
        }
        y += 1;
        let latest = self.0.state.borrow().records.changes().last().cloned();
        let Some(change) = latest.filter(|_| y < body.y + body.height) else { return };
        let (marker, title, action) = change_marks(&change);
        let column = screen.put(x, y, &marker.text, &marker.style);
        let room = (x + width - column - text_width(&action.text) - 1).max(1);
        screen.put(column, y, &truncate(&change.title, room), &title);
        let at = x + width - text_width(&action.text);
        screen.put(at, y, &action.text, &action.style);
        if change.state == ChangeState::Applied {
            let id = change.id.clone();
            self.hit(at, y, text_width(&action.text), move |app| {
                let id = id.clone();
                app.task(move |app| async move {
                    app.undo(Some(&id)).await;
                    Ok(())
                });
            });
        }
        y += 1;
        let left = body.y + body.height - y;
        if left <= 0 {
            return;
        }
        // A new clip, a device's place and a colour keep their own pictures; curves, lanes, values and levels are drawn
        // from the change's picture, a row left under it for what moved.
        let own = change.clip.is_some() || change.devices.is_some() || change.colors.is_some();
        let tall = if left >= 3 { (left - 1).min(6) } else { left };
        let drawn = (!own)
            .then(|| self.change_drawn(&change, width.max(4) as usize, tall as usize))
            .flatten()
            .map(|(drawn, _)| drawn.rows.into_iter().chain(drawn.lines).collect::<Vec<_>>());
        let cells = || (self.0.graphics != Graphics::Words).then(|| change_picture(&change, width, self.0.depth)).flatten();
        if let Some(picture) = drawn.filter(|rows| !rows.is_empty()).or_else(cells) {
            for (row, line) in picture.iter().take(left as usize).enumerate() {
                spans(screen, x, y + row as i32, line, None);
            }
        }
    }
}
