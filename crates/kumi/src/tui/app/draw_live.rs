use super::super::pictures::Graphics;
use super::super::{
    activity::activity_glyph,
    icons::{icon, track_kind},
    screen::Screen,
    style::Style,
    tree::{tree_window, TreeRole},
    width::{text_width, truncate, truncate_with},
    wrap::wrap,
};
use super::drawing::{sp, spans, st};
use super::*;
/// What FOCUS drew: the DAW's word for its body (" · Device"), why the DAW may be stale (" · Live is busy: …"), and the
/// rows the body took; no rows when FOCUS says only where the producer is.
pub(super) struct FocusDrawn {
    pub heading: Option<String>,
    pub busy: Option<String>,
    pub rows: Option<i32>,
}
/// How tall FOCUS's body may be: in the pane, so many rows for the device tree and for a DAW's own body; in the drawer,
/// all of it in so many rows, whatever heads it (the tree's track, a tracker's title and track names).
pub(super) enum FocusRoom {
    Pane { tree: i32, own: i32 },
    Rows(i32),
}
impl FocusRoom {
    fn most(&self, kind: BodyKind) -> i32 {
        match (self, kind) {
            (FocusRoom::Pane { tree, .. }, BodyKind::Tree) => *tree,
            (FocusRoom::Pane { own, .. }, _) => *own,
            (FocusRoom::Rows(rows), BodyKind::Tree) => rows - 1,
            (FocusRoom::Rows(rows), BodyKind::Tracker) => rows - 2,
            (FocusRoom::Rows(rows), _) => *rows,
        }
    }
}
impl TuiApp {
    fn click<F: Fn(TuiApp) + 'static>(&self, f: F) -> Rc<dyn Fn()> {
        let weak = Rc::downgrade(&self.0);
        Rc::new(move || {
            if let Some(a) = weak.upgrade() {
                f(TuiApp(a));
            }
        })
    }
    pub(super) fn hit<F: Fn(TuiApp) + 'static>(&self, x: i32, y: i32, width: i32, f: F) {
        let action = self.click(f);
        self.0.state.borrow_mut().hits.push(Hit { x, y, width, action });
    }
    /// A panel or menu drawn over `area` hides what's under it: what was there can't be clicked through it, and a copy
    /// there takes the panel's own cells, not the conversation's rows (nor joins the row below it to them).
    pub(super) fn cover(&self, area: Rect) {
        let under =
            |hit: &Hit| hit.y >= area.y && hit.y < area.y + area.height && hit.x < area.x + area.width && hit.x + hit.width > area.x;
        let mut state = self.0.state.borrow_mut();
        state.hits.retain(|hit| !under(hit));
        let covered = area.y..area.y + area.height;
        state.text_rows.retain(|row| !covered.contains(&row.y));
        // Tabs under it (the drawer's, below 100 columns) aren't the wheel's either: it goes to what covers them.
        if state.tabs_area.is_some_and(|tabs| {
            tabs.y < area.y + area.height && tabs.y + tabs.height > area.y && tabs.x < area.x + area.width && tabs.x + tabs.width > area.x
        }) {
            state.tabs_area = None;
        }
        if let Some(below) = state.text_rows.iter_mut().find(|row| row.y == covered.end) {
            below.carries = None;
        }
    }
    fn draw_focus_path(&self, screen: &mut Screen, x: i32, y: i32, width: i32, with_context: bool) -> bool {
        let state = self.0.state.borrow();
        let Some(focus) = state.focus.as_ref().filter(|_| state.connection == ConnectionState::Connected) else {
            return false;
        };
        let Some(track) = &focus.track else {
            return false;
        };
        let path = focus_path(focus);
        let room = (width - 2).max(1);
        let inline = path.value.as_ref().is_some_and(|v| text_width(&format!("{} · {v}", path.crumbs.join(" › "))) <= room);
        let parts = fit_crumbs(&path.crumbs, room - if inline { text_width(&format!(" · {}", path.value.as_ref().unwrap())) } else { 0 });
        let mut column = screen.put(x, y, "■", &Style::fg(chip_color(track.color.as_deref())));
        column = screen.put(column, y, " ", &st::TEXT);
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                column = screen.put(column, y, " › ", &st::FAINT);
            }
            column = screen.put(column, y, part, &if index == parts.len() - 1 && parts.len() > 1 { st::BRIGHT } else { st::TEXT });
        }
        if inline {
            screen.put(column, y, &format!(" · {}", path.value.as_ref().unwrap()), &st::BRIGHT);
        }
        let second = [if inline { None } else { path.value.as_deref() }, Some(path.context.as_str())]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        if with_context && !second.is_empty() {
            screen.put(x + 2, y + 1, &truncate(&second, (width - 2).max(1)), &st::DIM);
        }
        true
    }
    pub(super) fn draw_pane(&self, screen: &mut Screen, area: Rect) {
        screen.fill(area, &st::SURFACE);
        let x = area.x + 2;
        let width = area.width - 4;
        screen.put(x, area.y + 1, &truncate("FOCUS", width), &st::LABEL);
        let bottom = Self::bottom_height(area.height);
        let room = FocusRoom::Pane { tree: (area.height - bottom - 10).clamp(2, 12), own: (area.height - bottom - 8).clamp(3, 16) };
        let focus = self.draw_focus(screen, x, area.y + 2, width, room);
        if let Some(said) = &focus.busy {
            screen.put(x + 5, area.y + 1, &truncate(said, (width - 5).max(1)), &st::WARN);
        } else if let Some(heading) = &focus.heading {
            screen.put(x + 5, area.y + 1, heading, &st::FAINT);
        }
        let now_at = focus.rows.map_or(6, |rows| 4 + rows);
        screen.put(x, area.y + now_at, &truncate("NOW", width), &st::LABEL);
        let now = self.now_line();
        if let Some(dot) = now.dot {
            let label = format!(" {}", now.label);
            let at = x + width - text_width(&label) - 1;
            screen.put(at, area.y + now_at, "●", &dot);
            screen.put(at + 1, area.y + now_at, &label, &st::DIM);
        }
        if let Some((kind, since)) = now.activity {
            // One glyph turning beside steady words, as on a step's row: enough to show Kumi is at work.
            let glyph = activity_glyph(kind, perf_now() - since, self.0.icons == IconStyle::Badges);
            screen.put(x, area.y + now_at + 1, &glyph.text, &glyph.style);
            screen.put(x + 2, area.y + now_at + 1, &truncate(&now.detail, (width - 2).max(1)), &now.style);
        } else {
            screen.put(x, area.y + now_at + 1, &truncate(&now.detail, width), &now.style);
        }
        if let Some(change) = self.flashing() {
            // A new clip, a device's place and a colour keep their own pictures; curves, lanes, values and levels are
            // drawn from the change's picture.
            let own = change.clip.is_some() || change.devices.is_some() || change.colors.is_some();
            let drawn = (!own).then(|| self.change_drawn(&change, width.max(4) as usize, 2)).flatten().map(|(drawn, _)| {
                if drawn.rows.is_empty() {
                    drawn.lines
                } else {
                    drawn.rows
                }
            });
            let cells = || (self.0.graphics != Graphics::Words).then(|| change_picture(&change, width, self.0.depth)).flatten();
            if let Some(picture) = drawn.filter(|rows| !rows.is_empty()).or_else(cells) {
                for (row, line) in picture.iter().take(2).enumerate() {
                    spans(screen, x, area.y + now_at + 2 + row as i32, line, None);
                }
            }
        }
        let top = area.y + area.height - bottom;
        let tabs_area = Rect::new(x, top, width, bottom - 1);
        self.0.state.borrow_mut().tabs_area = Some(tabs_area);
        let mut hits = vec![];
        self.0.tabs.draw(screen, tabs_area, &mut hits);
        self.0.state.borrow_mut().hits.extend(hits);
    }
    /// FOCUS's body, as the pane and the drawer show it: drawn at (x, y) in `width`, as tall as `room` lets the device
    /// tree and a DAW's own body be, and read again as the DAW changes while it shows.
    pub(super) fn draw_focus(&self, screen: &mut Screen, x: i32, y: i32, width: i32, room: FocusRoom) -> FocusDrawn {
        let rows = self.tree_shown();
        let clip = if rows.is_none() { self.clip_shown() } else { None };
        let own = if rows.is_none() && clip.is_none() { self.own_shown() } else { None };
        let strip = if rows.is_none() && clip.is_none() && own.is_none() { self.strip_kind() } else { None };
        let (grid, timeline) = {
            let state = self.0.state.borrow();
            (
                if strip == Some(BodyKind::Grid) {
                    state.strip.grid.clone().filter(|s| state.focus.as_ref().and_then(|f| f.track_ref.as_ref()) == Some(&s.track_ref))
                } else {
                    None
                },
                if strip == Some(BodyKind::Timeline) { state.strip.timeline.clone() } else { None },
            )
        };
        self.keep_tree_fresh(if rows.is_some() {
            Some(BodyKind::Tree)
        } else if clip.is_some() {
            Some(BodyKind::Notes)
        } else if let Some(own) = &own {
            Some(match own {
                FocusBody::Tracker(_) => BodyKind::Tracker,
                FocusBody::Matrix(_) => BodyKind::Matrix,
                FocusBody::Keyzones(_) => BodyKind::Keyzones,
                FocusBody::Mixer(_) => BodyKind::Mixer,
                _ => BodyKind::Routing,
            })
        } else if let Some(asked) = (rows.is_none() && clip.is_none()).then(|| self.own_request()).flatten() {
            // A body of the DAW's own asked for and not read yet (or refused, as Renoise refuses while Kumi renders): it's
            // still read again, so it comes back.
            Some(asked.kind())
        } else if grid.is_some() {
            Some(BodyKind::Grid)
        } else if timeline.is_some() {
            Some(BodyKind::Timeline)
        } else {
            strip
        });
        // Each body's heading is the DAW's word for it.
        let heading = |kind| format!(" · {}", (crate::daw::view().heading)(kind));
        let (heading, rows) = if let Some(rows) = rows {
            (Some(heading(BodyKind::Tree)), Some(self.draw_tree(screen, x, y, width, &rows, room.most(BodyKind::Tree))))
        } else if let Some(clip) = clip {
            (Some(heading(BodyKind::Notes)), Some(self.draw_notes(screen, x, y, width, &clip)))
        } else if let Some(grid) = grid {
            (Some(heading(BodyKind::Grid)), Some(self.draw_grid(screen, x, y, width, &grid)))
        } else if let Some(own) = own {
            let most = |kind| room.most(kind);
            let (kind, drawn) = match &own {
                FocusBody::Tracker(grid) => (BodyKind::Tracker, self.draw_tracker(screen, x, y, width, grid, most(BodyKind::Tracker))),
                FocusBody::Matrix(matrix) => (BodyKind::Matrix, self.draw_matrix(screen, x, y, width, matrix, most(BodyKind::Matrix))),
                FocusBody::Keyzones(map) => (BodyKind::Keyzones, self.draw_keyzones(screen, x, y, width, map, most(BodyKind::Keyzones))),
                FocusBody::Routing(routing) => {
                    (BodyKind::Routing, self.draw_routing(screen, x, y, width, routing, most(BodyKind::Routing)))
                }
                FocusBody::Mixer(mixer) => (BodyKind::Mixer, self.draw_mixer(screen, x, y, width, mixer, most(BodyKind::Mixer))),
                _ => (BodyKind::Routing, 0),
            };
            // (a tracker grid that isn't a pattern, Renoise's phrase, says its own word)
            let named =
                if let FocusBody::Tracker(TrackerGrid { heading: Some(word), .. }) = &own { Some(format!(" · {word}")) } else { None };
            (Some(named.unwrap_or_else(|| heading(kind))), Some(drawn))
        } else if let Some(timeline) = timeline {
            (Some(heading(BodyKind::Timeline)), Some(self.draw_timeline(screen, x, y, width, &timeline)))
        } else {
            if !self.draw_focus_path(screen, x, y, width, true) {
                for (index, (text, style)) in self.focus_lines().iter().enumerate() {
                    screen.put(x, y + index as i32, &truncate(text, width), style);
                }
            }
            (None, None)
        };
        // A busy DAW (a dialog is open): FOCUS keeps what it showed, and says why it may be stale.
        let busy = self.0.state.borrow().focus.as_ref().and_then(|focus| focus.busy.clone());
        let busy = busy.map(|why| format!(" · {} is busy: {why}", crate::daw::words().app));
        FocusDrawn { heading, busy, rows }
    }
    fn bottom_height(pane: i32) -> i32 {
        ((pane as f64 / 2.).floor() as i32).max(7).min((pane - 8).max(1))
    }
    /// The focused track's chip and name over a body; where the focus names no track (REAPER's routing window for a track
    /// that isn't the selected one), the name the body gives, with a plain chip; none when neither says.
    fn track_header(&self, screen: &mut Screen, x: i32, y: i32, width: i32, named: Option<&str>) -> Option<FocusTrack> {
        let focused = self.0.state.borrow().focus.as_ref().and_then(|f| f.track.clone());
        let track = focused.or_else(|| named.map(|name| FocusTrack { name: name.to_owned(), color: None, kind: None }))?;
        let kind = track.kind.map(|k| serde_json::to_value(k).unwrap().as_str().unwrap().to_string());
        let mark = icon(track_kind(kind.as_deref()), self.0.icons, Some(chip_color(track.color.as_deref())));
        screen.put(x, y, &mark.text, &mark.style);
        screen.put(x + 3, y, &truncate(&track.name, (width - 3).max(1)), &st::TEXT);
        Some(track)
    }
    fn draw_notes(&self, screen: &mut Screen, x: i32, y: i32, width: i32, clip: &NoteView) -> i32 {
        let track = self.0.state.borrow().focus.as_ref().and_then(|f| f.track.clone()).unwrap();
        let kind = track.kind.map(|k| serde_json::to_value(k).unwrap().as_str().unwrap().to_string());
        let mark = icon(track_kind(kind.as_deref()), self.0.icons, Some(chip_color(track.color.as_deref())));
        screen.put(x, y, &mark.text, &mark.style);
        let clip_mark = icon(IconKind::MidiClip, self.0.icons, None);
        let mut column = screen.put(x + 3, y, &truncate(&track.name, (((width - 6) as f64 / 2.).floor() as i32).max(1)), &st::TEXT);
        column = screen.put(column, y, " › ", &st::FAINT);
        column = screen.put(column, y, &clip_mark.text, &clip_mark.style) + 1;
        // An unnamed one in the DAW's word: Live's clip, REAPER's take.
        let untitled = match self.0.state.borrow().focus.as_ref().and_then(|focus| focus.item.as_ref()).map(|item| item.kind) {
            Some(kind) if crate::daw::current().id() != kumi_types::daw::DawId::Ableton => format!("Untitled {}", kind.word()),
            _ => "Untitled clip".to_string(),
        };
        screen.put(
            column,
            y,
            &truncate(if clip.name.is_empty() { &untitled } else { &clip.name }, (x + width - column).max(1)),
            &st::BRIGHT,
        );
        // Braille may not show on a basic console: the bars and notes are said below.
        let picture = (self.0.graphics != Graphics::Words).then(|| clip_picture(clip.length, &clip.notes, width.min(32), 4)).flatten();
        if let Some(picture) = &picture {
            for (row, line) in picture.iter().enumerate() {
                spans(screen, x, y + 1 + row as i32, line, None);
            }
        }
        let bars = clip.length / clip.beats_per_bar.filter(|beats| *beats > 0.).unwrap_or(4.);
        let selected = clip.notes.iter().filter(|n| n.selected == Some(true)).count();
        let mut facts = vec![
            if bars.fract() == 0. {
                format!("{} {}", number::to_string(bars), if bars == 1. { "bar" } else { "bars" })
            } else {
                format!("{} beats", number::to_string(number::round(clip.length * 100.) / 100.))
            },
            // (every note the clip has: the view holds the first of a big one's)
            match clip.note_count.unwrap_or(0).max(clip.notes.len()) {
                1 => "1 note".to_owned(),
                count => format!("{count} notes"),
            },
        ];
        if selected > 0 {
            facts.push(format!("{selected} selected"));
        }
        let facts = facts.join(" · ");
        let picture_rows = if picture.is_some() { 4 } else { 0 };
        screen.put(
            x,
            y + 1 + picture_rows,
            &truncate(&if clip.notes.is_empty() { format!("{facts} · empty") } else { facts }, width),
            &st::FAINT,
        );
        // Its controller lanes (REAPER's CC lanes), a row each: the lane's values across the take, or in words.
        let mut row = y + 2 + picture_rows;
        const LANES_SHOWN: usize = 3;
        for lane in clip.lanes.iter().take(LANES_SHOWN) {
            let name = format!("{:<9}", truncate(&lane.name, 9));
            let column = screen.put(x, row, &name, &st::DIM);
            if self.0.graphics == Graphics::Words || clip.length <= 0. {
                let (low, high) = lane.points.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
                let points = lane.points.len();
                let range =
                    if points > 0 { format!(" · {}–{}", number::to_string(low), number::to_string(high)) } else { String::new() };
                let said = format!("{points} {}{range}", if points == 1 { "point" } else { "points" });
                screen.put(column + 1, row, &truncate(&said, (x + width - column - 1).max(1)), &st::FAINT);
            } else {
                screen.put(
                    column + 1,
                    row,
                    &controller_line(lane, clip.length, (x + width - column - 1).clamp(4, 32) as usize),
                    &st::ACCENT,
                );
            }
            row += 1;
        }
        if clip.lanes.len() > LANES_SHOWN {
            let more = clip.lanes.len() - LANES_SHOWN;
            screen.put(x, row, &truncate(&format!("and {more} more {}", if more == 1 { "lane" } else { "lanes" }), width), &st::FAINT);
            row += 1;
        }
        row - y
    }
    fn draw_grid(&self, screen: &mut Screen, x: i32, y: i32, width: i32, strip: &LauncherStrip) -> i32 {
        self.track_header(screen, x, y, width, None);
        for (index, slot) in strip.slots.iter().enumerate() {
            let row = y + 1 + index as i32;
            if slot.index == strip.scene {
                screen.fill(Rect::new(x - 1, row, width + 2, 1), &st::RAISED);
            }
            let mut column = screen.put(x, row, &format!("{:>3}", number::to_string(slot.index as f64 + 1.)), &st::FAINT) + 1;
            let Some(clip) = &slot.clip else {
                screen.put(column, row, "·", &st::FAINT);
                continue;
            };
            let mark = icon(if clip.audio { IconKind::AudioClip } else { IconKind::MidiClip }, self.0.icons, None);
            column = screen.put(column, row, &mark.text, &mark.style) + 1;
            let label = if slot.queued == Some(true) {
                "queued"
            } else if slot.playing == Some(true) {
                "playing"
            } else {
                ""
            };
            screen.put(
                column,
                row,
                &truncate(
                    if clip.name.is_empty() { "Untitled clip" } else { &clip.name },
                    (x + width - column - if label.is_empty() { 0 } else { text_width(label) + 1 }).max(1),
                ),
                &if slot.playing == Some(true) {
                    st::ACCENT
                } else if !clip.name.is_empty() {
                    st::TEXT
                } else {
                    st::DIM
                },
            );
            if !label.is_empty() {
                screen.put(x + width - text_width(label), row, label, &if slot.playing == Some(true) { st::ACCENT } else { st::FAINT });
            }
        }
        1 + strip.slots.len() as i32
    }
    fn draw_timeline(&self, screen: &mut Screen, x: i32, y: i32, width: i32, strip: &TimelineStrip) -> i32 {
        self.track_header(screen, x, y, width, None);
        let cells = width.max(8);
        let span = strip.locators.iter().map(|l| l.position + 4.).fold(strip.length.max(strip.position + 4.), f64::max);
        let at = |beats: f64| (beats / span * cells as f64).floor().clamp(0., (cells - 1) as f64) as usize;
        let mut line = vec![sp("─", st::FAINT); cells as usize];
        if let Some(loop_) = &strip.r#loop {
            for cell in &mut line[at(loop_.start)..=at(loop_.start + loop_.length)] {
                *cell = sp("━", if loop_.enabled { st::DIM } else { st::FAINT });
            }
        }
        for locator in &strip.locators {
            line[at(locator.position)] = sp("┼", st::DIM);
        }
        line[at(strip.position)] = sp("┃", st::ACCENT);
        // The time selection, as REAPER shades it.
        if let Some([start, end]) = strip.time_selection {
            for cell in &mut line[at(start)..=at(end)] {
                *cell = sp(cell.text.clone(), Style { bg: st::SELECTED.bg, ..*cell.style });
            }
        }
        spans(screen, x, y + 1, &line, None);
        let bar = |beats: f64| strip.bar_at(beats);
        let past = strip.locators.iter().filter(|l| l.position <= strip.position).max_by(|a, b| a.position.total_cmp(&b.position));
        let mut where_ = vec![format!("bar {}", number::to_string(bar(strip.position)))];
        if strip.playing {
            where_.push("playing".into());
        }
        if let Some(past) = past.filter(|l| !l.name.is_empty()) {
            where_.push(format!("after {}", past.name));
        }
        let mut whole = vec![];
        if let Some(loop_) = strip.r#loop.as_ref().filter(|l| l.enabled) {
            whole.push(format!("loop {}–{}", number::to_string(bar(loop_.start)), number::to_string(bar(loop_.start + loop_.length))));
        }
        if let Some([start, end]) = strip.time_selection {
            whole.push(format!("selection {}–{}", number::to_string(bar(start)), number::to_string(bar(end))));
        }
        whole.push(format!("{} bars", number::to_string(bar(strip.length) - 1.)));
        screen.put(x, y + 2, &truncate(&where_.join(" · "), width), &st::DIM);
        screen.put(x, y + 3, &truncate(&whole.join(" · "), width), &st::FAINT);
        4 + self.draw_lanes(screen, x, y + 4, width, strip, span)
    }
    /// A timeline's tracks (REAPER's arrange view), each with its items as bars on the strip's scale: the selected
    /// track's lanes first in view, as many as fit.
    fn draw_lanes(&self, screen: &mut Screen, x: i32, y: i32, width: i32, strip: &TimelineStrip, span: f64) -> i32 {
        let Some(lanes) = strip.lanes.as_ref().filter(|lanes| !lanes.is_empty()) else { return 0 };
        let name_width = (width / 3).clamp(6, 16);
        let cells = (width - name_width - 1).max(4);
        let at = |beats: f64| (beats / span * cells as f64).floor().clamp(0., (cells - 1) as f64) as usize;
        // The selected track by its ref (two tracks can share a name), by name where the DAW gives no refs.
        let (selected_ref, selected_name) = {
            let state = self.0.state.borrow();
            let focus = state.focus.as_ref();
            (focus.and_then(|f| f.track_ref.clone()), focus.and_then(|f| f.track.as_ref().map(|t| t.name.clone())))
        };
        // What REAPER draws: tracks the producer hid, and those in a folder collapsed all the way, aren't.
        let visible: Vec<usize> = (0..lanes.len()).filter(|at| !lanes[*at].hidden).collect();
        if visible.is_empty() {
            return 0;
        }
        let shown = visible.len().min(8);
        let first = visible
            .iter()
            .position(|at| lanes[*at].track_ref.is_some() && lanes[*at].track_ref == selected_ref)
            .or_else(|| {
                selected_name.and_then(|name| visible.iter().position(|at| lanes[*at].track_ref.is_none() && lanes[*at].name == name))
            })
            .map(|at| at.saturating_sub(shown / 2).min(visible.len() - shown))
            .unwrap_or(0);
        for (row, lane) in visible[first..first + shown].iter().map(|at| &lanes[*at]).enumerate() {
            let row = y + row as i32;
            let indent = "  ".repeat(lane.depth.min(4) as usize);
            let sign = if lane.collapsed { "▸ " } else { "" };
            screen.put(x, row, &truncate(&format!("{indent}{sign}{}", lane.name), name_width), &st::DIM);
            let mut line = vec![sp(" ", st::FAINT); cells as usize];
            for item in &lane.items {
                let style = if item.selected {
                    st::ACCENT
                } else if item.muted {
                    st::FAINT
                } else {
                    st::TEXT
                };
                for cell in &mut line[at(item.start)..=at(item.start + item.length.max(0.))] {
                    *cell = sp("▬", style);
                }
            }
            spans(screen, x + name_width + 1, row, &line, None);
        }
        // The DAW sends a page of lanes, says where it starts and how many tracks it has in all: the rest are above or
        // below.
        let (top, bottom) = (visible[first], visible[first + shown - 1]);
        let above = strip.lanes_from.unwrap_or(0) + top;
        let total = strip.track_count.unwrap_or(0).max(strip.lanes_from.unwrap_or(0) + lanes.len());
        let below = total.saturating_sub(strip.lanes_from.unwrap_or(0) + bottom + 1);
        let hidden = lanes[top..=bottom].iter().filter(|lane| lane.hidden).count();
        let mut more = match (above, below) {
            (0, 0) => vec![],
            (0, below) => vec![format!("{below} more tracks below")],
            (above, 0) => vec![format!("{above} more tracks above")],
            (above, below) => vec![format!("{above} more tracks above"), format!("{below} below")],
        };
        if hidden > 0 {
            more.push(format!("{hidden} hidden"));
        }
        if more.is_empty() {
            return shown as i32;
        }
        let more = more.join(" · ");
        screen.put(x, y + shown as i32, &truncate(&more, width), &st::FAINT);
        shown as i32 + 1
    }
    /// A tracker's pattern or phrase around its cursor: line numbers (beats brighter), then each track's cells, the
    /// focused track first, as many tracks as fit.
    fn draw_tracker(&self, screen: &mut Screen, x: i32, y: i32, width: i32, grid: &TrackerGrid, most: i32) -> i32 {
        let mut title = format!(
            "{} · {} lines",
            if grid.name.is_empty() { grid.heading.as_deref().unwrap_or("Pattern") } else { &grid.name },
            grid.lines
        );
        for fact in &grid.facts {
            title.push_str(&format!(" · {fact}"));
        }
        screen.put(x, y, &truncate(&title, width), &st::TEXT);
        let focused = self.0.state.borrow().focus.as_ref().and_then(|f| f.track.as_ref().map(|t| t.name.clone()));
        let mut order: Vec<&TrackerTrack> = grid.tracks.iter().collect();
        if let Some(at) = focused.and_then(|name| order.iter().position(|track| track.name == name)) {
            let track = order.remove(at);
            order.insert(0, track);
        }
        let number_width = 4;
        let mut column = x + number_width;
        let mut shown: Vec<(&TrackerTrack, i32, i32)> = vec![];
        for track in order {
            let cell = track.cells.iter().map(|cell| text_width(cell)).max().unwrap_or(3).max(3).min(width - number_width);
            if column + cell > x + width {
                break;
            }
            shown.push((track, column, cell));
            column += cell + 1;
        }
        for (track, at, cell) in &shown {
            screen.put(*at, y + 1, &truncate(&track.name, *cell), &Style::fg(chip_color(track.color.as_deref())));
        }
        let lines = shown.iter().map(|(track, _, _)| track.cells.len()).max().unwrap_or(0);
        let rows = lines.min(most.max(1) as usize);
        // The rows around the cursor (or the playhead when following): the window the DAW sends is wider than the pane.
        let centre = grid.cursor.or(grid.playing).map(|line| line.saturating_sub(grid.from));
        let first = centre.map_or(0, |at| at.saturating_sub(rows / 2).min(lines - rows));
        let lpb = grid.lpb.filter(|lpb| *lpb >= 1.).unwrap_or(4.) as usize;
        for (row, index) in (first..first + rows).enumerate() {
            let line = grid.from + index;
            let y = y + 2 + row as i32;
            if grid.cursor == Some(line) {
                screen.fill(Rect::new(x - 1, y, width + 2, 1), &st::RAISED);
            }
            let style = if grid.playing == Some(line) {
                st::ACCENT
            } else if line % lpb == 0 {
                st::DIM
            } else {
                st::FAINT
            };
            // (as the tracker numbers it: 00 the first line)
            screen.put(x, y, &format!("{:>3}", format!("{:02}", line)), &style);
            for (track, at, cell) in &shown {
                let text = track.cells.get(index).map(String::as_str).unwrap_or("");
                screen.put(
                    *at,
                    y,
                    &truncate(text, *cell),
                    &if text.chars().all(|c| matches!(c, '.' | '-' | ' ')) { st::FAINT } else { st::TEXT },
                );
            }
        }
        2 + rows as i32
    }
    /// A tracker's sequence as its matrix draws it: each slot's number and pattern, then each track's part of it (a
    /// block in the slot's colour, dim where muted; a dot where empty; an alias as the pattern it plays), sections
    /// between, the current slot raised with its pattern's name.
    fn draw_matrix(&self, screen: &mut Screen, x: i32, y: i32, width: i32, matrix: &SequenceMatrix, most: i32) -> i32 {
        // (the slot's number, then its pattern's: "  3 02  ")
        let lead = 8;
        let column = 4;
        let fit = ((width - lead) / column).max(1) as usize;
        let mut head = " ".repeat(lead as usize);
        for track in matrix.tracks.iter().take(fit) {
            // Padded by width, not characters: a wide name (CJK) takes two cells a character.
            let name = truncate_with(track, column - 1, "");
            head.push_str(&format!("{name}{}", " ".repeat((column - text_width(&name)).max(1) as usize)));
        }
        screen.put(x, y, &truncate(&head, width), &st::DIM);
        let rows = (most.max(2) - 1) as usize;
        let current = matrix.current.unwrap_or(0);
        let first = current.saturating_sub(rows / 2).min(matrix.slots.len().saturating_sub(rows));
        let shown = matrix.tracks.len().min(fit);
        let mut row = 1;
        for (index, slot) in matrix.slots.iter().enumerate().skip(first) {
            if row as usize > rows {
                break;
            }
            if let Some(section) = &slot.section {
                if row as usize >= rows {
                    break;
                }
                screen.put(x, y + row, &truncate(&format!("— {section}"), width), &st::FAINT);
                row += 1;
            }
            let here = Some(index) == matrix.current;
            if here {
                screen.fill(Rect::new(x - 1, y + row, width + 2, 1), &st::RAISED);
            }
            screen.put(x, y + row, &format!("{:>3} ", matrix.first + index + 1), &st::FAINT);
            screen.put(x + 4, y + row, &format!("{:02}", slot.pattern), &if here { st::TEXT } else { st::DIM });
            for track in 0..shown {
                let muted = slot.muted.contains(&track);
                let color = slot.colors.iter().find(|(at, _)| *at == track).map(|(_, color)| color.as_str());
                let (text, style) = if let Some((_, source)) = slot.aliases.iter().find(|(at, _)| *at == track) {
                    (format!("→{source:02}"), if muted { st::FAINT } else { st::DIM })
                } else if slot.empty.contains(&track) {
                    ("·".to_owned(), st::FAINT)
                } else if muted {
                    ("░░░".to_owned(), st::FAINT)
                } else {
                    ("███".to_owned(), color.map_or(st::TEXT, |color| Style::fg(chip_color(Some(color)))))
                };
                screen.put(x + lead + track as i32 * column, y + row, &text, &style);
            }
            // The current slot's pattern by its name, where there's room.
            let after = x + lead + shown as i32 * column;
            if let Some(name) = slot.name.as_deref().filter(|_| here && x + width - after > 4) {
                screen.put(after, y + row, &truncate(name, x + width - after), &st::DIM);
            }
            row += 1;
        }
        row
    }
    /// An instrument's samples on the keyboard: a map of its keyzones (keys across, velocities up), each zone's notes and
    /// velocities, then its macros and phrases with their keys.
    fn draw_keyzones(&self, screen: &mut Screen, x: i32, y: i32, width: i32, map: &KeyzoneMap, most: i32) -> i32 {
        let profile = crate::daw::profile();
        let mut title = if map.name.is_empty() { "Instrument".to_owned() } else { map.name.clone() };
        match map.plays_through.as_deref() {
            Some("plugin") => title.push_str(" · plug-in"),
            Some("midi") => title.push_str(" · MIDI"),
            _ => {}
        }
        if let Some(overlap) = map.overlap.as_deref().filter(|overlap| *overlap != "all") {
            title.push_str(&format!(" · overlapping zones: {overlap}"));
        }
        screen.put(x, y, &truncate(&title, width), &st::TEXT);
        let mut row = y + 1;
        // Each zone's letter, in the map and in the list under it.
        let letter = |k: usize| match k {
            0..=25 => char::from(b'A' + k as u8),
            26..=51 => char::from(b'a' + (k - 26) as u8),
            _ => '+',
        };
        let phrase_line = !map.phrases.is_empty();
        let macro_line = !map.macros.is_empty();
        // The keyzone map: keys across (the zones' keys, an octave at least), velocities up in four bands, two or one, with
        // the keys under them; each cell is the zone that plays there, ▒ where zones overlap, · where none does. The zones'
        // names come first, so each letter in it is named below: the map takes the rows they leave (under the title, less
        // the facts line), beside the macros and phrases lines, else one band in their place.
        let left = most - 2 - map.zones.len() as i32;
        let fits = |room: i32| [4, 2, 1].into_iter().find(|bands| (*bands < 4 || most >= 12) && room > *bands);
        let bands: i32 = if map.zones.is_empty() || width < 12 {
            0
        } else {
            fits(left - phrase_line as i32 - macro_line as i32).or(fits(left).map(|_| 1)).unwrap_or(0)
        };
        if bands > 0 {
            let lo = map.zones.iter().map(|zone| zone.notes.0).min().unwrap_or(0) as i32;
            let hi = map.zones.iter().map(|zone| zone.notes.1).max().unwrap_or(119) as i32;
            let short = (12 - (hi - lo + 1)).max(0);
            let lo = (lo - short / 2).max(0);
            let hi = (lo + 11).max(hi).min(119);
            let keys = hi - lo + 1;
            let per = (keys + width - 1) / width;
            let columns = (keys + per - 1) / per;
            for band in 0..bands {
                let (top, bottom) = (127 - band * 128 / bands, 128 - (band + 1) * 128 / bands);
                let cells: String = (0..columns)
                    .map(|column| {
                        let (first, last) = (lo + column * per, (lo + column * per + per - 1).min(hi));
                        let mut playing = map.zones.iter().enumerate().filter(|(_, zone)| {
                            zone.notes.0 as i32 <= last
                                && zone.notes.1 as i32 >= first
                                && zone.velocities.0 as i32 <= top
                                && zone.velocities.1 as i32 >= bottom
                        });
                        match (playing.next(), playing.next()) {
                            (None, _) => '·',
                            (Some((k, _)), None) => letter(k),
                            _ => '▒',
                        }
                    })
                    .collect();
                screen.put(x, row, &cells, &st::DIM);
                row += 1;
            }
            let (from, to) = (profile.note_name(lo as u8), profile.note_name(hi as u8));
            let gap = (columns - text_width(&from) - text_width(&to)).max(1) as usize;
            screen.put(x, row, &truncate(&format!("{from}{}{to}", " ".repeat(gap)), width), &st::FAINT);
            row += 1;
        }
        let rows = map.zones.len().min((y + most - row - 1).max(0) as usize);
        for (k, zone) in map.zones.iter().take(rows).enumerate() {
            let notes = if zone.notes.0 == zone.notes.1 {
                profile.note_name(zone.notes.0)
            } else {
                format!("{}–{}", profile.note_name(zone.notes.0), profile.note_name(zone.notes.1))
            };
            let velocities =
                if zone.velocities == (0, 127) { String::new() } else { format!(" · vel {}–{}", zone.velocities.0, zone.velocities.1) };
            let off = if zone.note_off { " · note-off" } else { "" };
            let group = zone.mute_group.map(|group| format!(" · mute group {group}")).unwrap_or_default();
            let slice = if zone.slice { " · slice" } else { "" };
            let line = format!("{} {notes:<9} {}{velocities}{off}{group}{slice}", letter(k), zone.sample);
            screen.put(x, row, &truncate(&line, width), &st::DIM);
            row += 1;
        }
        let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut facts = vec![count(map.zones.len(), "zone", "zones")];
        if rows < map.zones.len() {
            facts[0].push_str(&format!(" ({} not shown)", map.zones.len() - rows));
        }
        if !map.macros.is_empty() {
            facts.push(count(map.macros.len(), "macro", "macros"));
        }
        if !map.phrases.is_empty() {
            facts.push(count(map.phrases.len(), "phrase", "phrases"));
        }
        if let Some(n) = map.modsets.filter(|n| *n > 0) {
            facts.push(count(n, "modulation set", "modulation sets"));
        }
        if let Some(n) = map.chains.filter(|n| *n > 0) {
            facts.push(count(n, "FX chain", "FX chains"));
        }
        screen.put(x, row, &truncate(&facts.join(" · "), width), &st::FAINT);
        row += 1;
        // Its macros by name, with where each stands (Renoise's 0–100 %).
        if !map.macros.is_empty() && row < y + most {
            let shown: Vec<String> = map
                .macros
                .iter()
                .map(|(name, value)| format!("{name} {}%", number::to_string(number::round((value * 100.).clamp(0., 100.)))))
                .collect();
            screen.put(x, row, &truncate(&format!("macros: {}", shown.join(" · ")), width), &st::FAINT);
            row += 1;
        }
        // The phrases, with their keys in the keymap (and the one program mode plays).
        if phrase_line && row < y + most {
            let shown: Vec<String> = map
                .phrases
                .iter()
                .enumerate()
                .map(|(k, name)| {
                    let mut text = if map.program == Some(k) { format!("▸{name}") } else { name.clone() };
                    if let Some(Some((low, high))) = map.phrase_keys.get(k) {
                        text.push_str(&format!(" {}–{}", profile.note_name(*low), profile.note_name(*high)));
                    }
                    text
                })
                .collect();
            let how = match map.phrase_playback.as_deref() {
                Some("off") => "phrases off: ",
                Some("program") => "phrases by program: ",
                Some("keymap") => "phrases by keymap: ",
                _ => "",
            };
            screen.put(x, row, &truncate(&format!("{how}{}", shown.join(" · ")), width), &st::FAINT);
            row += 1;
        }
        row - y
    }
    /// A track's sends and receives, with their levels and where they tap the track.
    fn draw_routing(&self, screen: &mut Screen, x: i32, y: i32, width: i32, routing: &RoutingView, most: i32) -> i32 {
        self.track_header(screen, x, y, width, routing.name.as_deref());
        let rows = routing.routes.len().min((most.max(3) - 2 - routing.midi_out.is_some() as i32).max(1) as usize);
        for (row, route) in routing.routes.iter().take(rows).enumerate() {
            let arrow = match route.kind {
                RouteKind::Send => "→",
                RouteKind::Receive => "←",
                RouteKind::Hardware => "⇥",
            };
            let mut line = format!("{arrow} {}", route.other);
            if let Some(db) = route.volume_db {
                // (dB with a true minus, as the DAW's titles write it)
                line.push_str(&format!(" · {} dB", number::to_string(number::round(db * 10.) / 10.).replace('-', "−")));
            }
            if let Some(tap) = &route.tap {
                line.push_str(&format!(" · {tap}"));
            }
            screen.put(x, y + 1 + row as i32, &truncate(&line, width), &if route.muted { st::FAINT } else { st::DIM });
        }
        let mut row = y + 1 + rows as i32;
        // Its MIDI out of the DAW (REAPER's hardware MIDI output), with the routes.
        if let Some(out) = &routing.midi_out {
            screen.put(x, row, &truncate(&format!("⇥ MIDI to {out}"), width), &st::DIM);
            row += 1;
        }
        let parent = match routing.parent_send {
            Some(true) => "feeds its parent",
            Some(false) => "doesn't feed its parent",
            None => "",
        };
        if !parent.is_empty() {
            screen.put(x, row, &truncate(parent, width), &st::FAINT);
            row += 1;
        }
        row - y
    }
    /// The mixer: a row per track's strip around the selected one (raised): its chip and name, its peak level now where
    /// the DAW meters it, its fader in dB, its pan, and whether it's muted, soloed or armed. A DAW without meters says so.
    fn draw_mixer(&self, screen: &mut Screen, x: i32, y: i32, width: i32, mixer: &MixerView, most: i32) -> i32 {
        const LEVELS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
        let note = !mixer.levels;
        let rows = (most - note as i32).max(1) as usize;
        let selected = mixer.strips.iter().position(|strip| strip.selected).unwrap_or(0);
        let first = selected.saturating_sub(rows / 2).min(mixer.strips.len().saturating_sub(rows));
        let db_text = |db: f64| {
            if db <= -100. {
                "−∞".to_string()
            } else {
                let text = number::to_fixed(db, 1);
                text.strip_prefix('-').map_or(text.clone(), |rest| format!("−{rest}"))
            }
        };
        let pan_text = |pan: f64| {
            let percent = (pan.abs() * 100.).round() as i64;
            match percent {
                0 => "C".to_string(),
                n if pan < 0. => format!("{n}L"),
                n => format!("{n}R"),
            }
        };
        let mut row = y;
        for strip in mixer.strips.iter().skip(first).take(rows) {
            if strip.selected {
                screen.fill(Rect::new(x - 1, row, width + 2, 1), &st::RAISED);
            }
            let kind = strip.kind.map(|k| serde_json::to_value(k).unwrap().as_str().unwrap().to_string());
            let mark = icon(track_kind(kind.as_deref()), self.0.icons, Some(chip_color(strip.color.as_deref())));
            screen.put(x, row, &mark.text, &mark.style);
            // What's on the right: the level, the fader, the pan and the flags, in fixed columns.
            let flags = [strip.mute.as_ref().map(|_| "M"), strip.solo.then_some("S"), strip.armed.then_some("●")]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("");
            let right = format!(
                "{:>6} {:>4} {:<3}",
                strip.volume_db.map(db_text).unwrap_or_default(),
                strip.pan.map(pan_text).unwrap_or_default(),
                flags
            );
            let meter = if mixer.levels { 2 } else { 0 };
            let at = x + width - text_width(&right);
            let name_room = (at - meter - (x + 3) - 1).max(1);
            screen.put(x + 3, row, &truncate(&strip.name, name_room), &if strip.mute.is_some() { st::FAINT } else { st::TEXT });
            if let Some([left, right_db]) = strip.peak_db.filter(|_| mixer.levels) {
                let loudest = left.max(right_db);
                let level = (((loudest + 60.) / 60.).clamp(0., 1.) * 7.).round() as usize;
                let style = if loudest > -1. {
                    st::WARN
                } else if loudest > -60. {
                    st::ACCENT
                } else {
                    st::FAINT
                };
                screen.put(at - 2, row, if loudest <= -60. { "·" } else { LEVELS[level] }, &style);
            }
            screen.put(at, row, &right, &st::DIM);
            row += 1;
        }
        if note {
            screen.put(x, row, &truncate(&format!("{} gives no meters", crate::daw::words().app), width), &st::FAINT);
            row += 1;
        }
        row - y
    }
    pub(super) fn draw_held(&self, screen: &mut Screen, x: i32, mut y: i32, width: i32, rows: i32) {
        let state = self.0.state.borrow();
        let shown = if state.held.len() > rows as usize { &state.held[state.held.len() - (rows - 1) as usize..] } else { &state.held };
        if state.held.len() > rows as usize {
            screen.put(x, y, &format!("  and {} more waiting · alt+↑ takes the last back", state.held.len() - shown.len()), &st::FAINT);
            y += 1;
        }
        for item in shown {
            let when = if item.when == "now" && (state.current.is_some() || state.pending_turn) {
                "at the next step"
            } else {
                "after this answer"
            };
            let words = sanitize_text(&item.text, &state.secrets)
                .split(|c: char| string::trim(&c.to_string()).is_empty())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let column = screen.put(x, y, "↳ ", &st::FAINT);
            screen.put(column, y, &truncate(&words, (width - text_width(when) - 4).max(1)), &st::DIM);
            screen.put(x + width - text_width(when), y, when, &st::FAINT);
            y += 1;
        }
    }
    /// The files that go with the next message: name, kind and size, each with × to take it back.
    pub(super) fn draw_attachments(&self, screen: &mut Screen, x: i32, y: i32, width: i32) {
        let files = self.0.state.borrow().attachments.clone();
        let mut column = screen.put(x, y, "with ", &st::FAINT);
        for (index, file) in files.iter().enumerate() {
            let details = format!(" · {} · {}", super::super::attach::kind_of(file), super::super::attach::size_of(file.bytes));
            let left = files.len() - index;
            let more = if left > 1 { format!("   +{} more", left - 1) } else { String::new() };
            let need = text_width(&file.name).min(32) + text_width(&details) + 2 + text_width(&more);
            if index > 0 && column + need > x + width {
                screen.put(column, y, &format!("+{left} more"), &st::FAINT);
                return;
            }
            column = screen.put(column, y, &truncate(&file.name, 32), &st::BRIGHT);
            column = screen.put(column, y, &details, &st::DIM);
            let at = column + 1;
            screen.put(at, y, "×", &st::FAINT);
            let path = file.path.clone();
            self.hit(at, y, 1, move |app| {
                app.0.state.borrow_mut().attachments.retain(|a| a.path != path);
                app.0.scheduler.request();
            });
            column = at + 4;
        }
    }
    pub(super) fn draw_pin(&self, screen: &mut Screen, x: i32, y: i32, width: i32) {
        let pin = self.0.state.borrow().pinned.clone().unwrap();
        let mark = icon(pin.kind, self.0.icons, None);
        let mut column = screen.put(x, y, &mark.text, &mark.style) + 1;
        let room = (x + width - column - 3).max(1);
        let mut crumbs =
            if !pin.pin.trail.is_empty() { pin.pin.trail } else { pin.pin.track.into_iter().filter(|s| !s.is_empty()).collect() };
        crumbs.push(pin.pin.name);
        let parts = fit_crumbs(&crumbs, room);
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                column = screen.put(column, y, " › ", &st::FAINT);
            }
            column = screen.put(column, y, part, &if index == parts.len() - 1 { st::BRIGHT } else { st::DIM });
        }
        let at = column + 2;
        screen.put(at, y, "×", &st::FAINT);
        self.hit(at, y, 1, |app| app.unpin());
    }
    fn draw_tree(&self, screen: &mut Screen, x: i32, y: i32, width: i32, rows: &[TreeRow], most: i32) -> i32 {
        let track = self.track_header(screen, x, y, width, None);
        // A chain that isn't the track's own (REAPER's take FX) says whose it is beside the track.
        let owner = self.0.state.borrow().tree.as_ref().and_then(|tree| tree.owner.clone());
        if let (Some(owner), Some(track)) = (owner, track) {
            let at = x + 3 + text_width(&truncate(&track.name, (width - 3).max(1)));
            if at + 4 < x + width {
                screen.put(at, y, &truncate(&format!(" · {owner}"), x + width - at), &st::DIM);
            }
        }
        let focused = rows.iter().any(|r| r.role == TreeRole::Focus);
        let cursor = self.0.state.borrow().tree_cursor;
        let keep = cursor
            .map(|c| c as isize)
            .unwrap_or_else(|| rows.iter().position(|r| r.role == TreeRole::Focus).map(|n| n as isize).unwrap_or(-1));
        let view = tree_window(rows, most as usize, keep);
        let cut_above = usize::from(view.above > 0);
        let cut_below = usize::from(view.below > 0);
        let visible = &view.rows[cut_above..view.rows.len().saturating_sub(cut_below)];
        let mut row = y + 1;
        if cut_above > 0 {
            screen.put(x, row, &format!("  {} more", view.above + 1), &st::FAINT);
            row += 1;
        }
        for item in visible {
            let index = rows.iter().position(|r| r == item).unwrap();
            let cursor = cursor == Some(index);
            if cursor || item.role == TreeRole::Focus {
                screen.fill(Rect::new(x - 1, row, width + 2, 1), &if cursor { st::SELECTED } else { st::RAISED });
            }
            let mut column = screen.put(x, row, &item.prefix, &st::FAINT);
            let mark = icon(item.kind, self.0.icons, None);
            column = screen.put(column, row, &mark.text, &mark.style) + 1;
            let pinned = self.0.state.borrow().pinned.as_ref().is_some_and(|p| p.row == item.r#ref);
            let label = if pinned { "pinned" } else { "" };
            let style = if item.role == TreeRole::Focus {
                st::ACCENT
            } else if pinned {
                st::BRIGHT
            } else if !focused || item.role == TreeRole::Path {
                st::TEXT
            } else {
                st::DIM
            };
            let count = item.count.filter(|n| *n > 0).map(|n| format!(" ({n})")).unwrap_or_default();
            let note = item.note.as_ref().map(|note| format!(" · {note}")).unwrap_or_default();
            let room = (x + width - column - text_width(&count) - if label.is_empty() { 0 } else { text_width(label) + 1 }).max(1);
            // The name gives way first (a plug-in's format and maker go, then its end), so what the DAW says of it stays.
            let named = (room - text_width(&note)).max(room.min(8));
            let name = if text_width(&item.name) <= named { item.name.clone() } else { truncate(short_name(&item.name), named) };
            column = screen.put(column, row, &name, &style);
            if !count.is_empty() {
                column = screen.put(column, row, &count, &st::FAINT);
            }
            // What the DAW says of it (bypassed, offline, parallel), where there's room.
            let left = room - text_width(&name);
            if !note.is_empty() && left > 4 {
                screen.put(column, row, &truncate(&note, left), &st::FAINT);
            }
            if !label.is_empty() {
                let at = x + width - text_width(label);
                screen.put(at, row, label, &st::FAINT);
                self.hit(at, row, text_width(label), |app| app.unpin());
            }
            let item = item.clone();
            self.hit(x, row, width, move |app| {
                app.0.state.borrow_mut().tree_cursor = None;
                app.pin(&item);
            });
            row += 1;
        }
        if cut_below > 0 {
            screen.put(x, row, &format!("  {} more", view.below + 1), &st::FAINT);
            row += 1;
        }
        row - y
    }
    pub(super) fn draw_dock(&self, screen: &mut Screen, area: Rect) {
        if area.height <= 0 {
            return;
        }
        screen.fill(area, &st::SURFACE);
        let focus = self.focus_lines().remove(0);
        let now = self.now_line();
        if !self.draw_focus_path(screen, 2, area.y, (area.width - 16).max(1), false) {
            screen.put(2, area.y, &truncate(&focus.0, (area.width - 16).max(1)), &focus.1);
        }
        if let Some(dot) = now.dot {
            let right = format!(" {}", now.label);
            let at = area.width - 2 - text_width(&right) - 1;
            screen.put(at, area.y, "●", &dot);
            screen.put(at + 1, area.y, &right, &st::DIM);
        }
        let last = self.0.state.borrow().records.changes().last().cloned();
        if area.height > 1 {
            if let Some(last) = last.filter(|c| !self.busy() && c.state == ChangeState::Applied) {
                let at = area.width - 6;
                // A curve, a lane or notes in one row beside it, where there's room.
                let room = ((at - 4) / 3).clamp(0, 16);
                let picture = (room >= 8)
                    .then(|| self.change_drawn(&last, room as usize, 1))
                    .flatten()
                    .and_then(|(drawn, _)| drawn.rows.into_iter().next());
                let title_room = if picture.is_some() { at - 5 - room } else { at - 4 };
                screen.put(2, area.y + 1, &truncate(&format!("✓ {}", last.title), title_room.max(1)), &st::TEXT);
                if let Some(picture) = picture {
                    spans(screen, at - 1 - room, area.y + 1, &picture, None);
                }
                screen.put(at, area.y + 1, "undo", &st::ACCENT);
                self.hit(at, area.y + 1, 4, move |app| {
                    let id = last.id.clone();
                    app.task(move |app| async move {
                        app.undo(Some(&id)).await;
                        Ok(())
                    });
                });
            } else if let Some((kind, since)) = now.activity {
                let glyph = activity_glyph(kind, perf_now() - since, self.0.icons == IconStyle::Badges);
                screen.put(2, area.y + 1, &glyph.text, &glyph.style);
                screen.put(4, area.y + 1, &truncate(&now.detail, area.width - 6), &now.style);
            } else {
                screen.put(2, area.y + 1, &truncate(&now.detail, area.width - 4), &now.style);
            }
        }
    }
    pub(super) fn history_rows(&self, width: i32) -> Rc<[TabRow]> {
        // Made again only when the changes or what's kept change, at another width, or as a row opens or closes: not
        // every frame.
        let (revision, expanded) = {
            let state = self.0.state.borrow();
            if let Some((made, at, open, rows)) = &state.history_rows {
                if *made == state.records.revision() && *at == width && *open == state.expanded {
                    return rows.clone();
                }
            }
            (state.records.revision(), state.expanded.clone())
        };
        let mut image = None;
        let mut rows = vec![];
        let (state_kept, changes) = {
            let state = self.0.state.borrow();
            (state.records.kept().to_vec(), state.records.changes().to_vec())
        };
        let kept = state_kept.iter().rev().take(3);
        for entry in kept {
            let e = entry.borrow();
            let action = if e.forgotten {
                None
            } else {
                let entry = entry.clone();
                Some(self.click(move |app| {
                    let entry = entry.clone();
                    app.task(move |app| async move {
                        let forget = entry.borrow().forget.clone();
                        // A forget that failed is said as it failed (by the task), and the row still offers it.
                        let gone = app.forget(forget).await?;
                        if !gone && !entry.borrow().forgotten {
                            app.0.state.borrow_mut().records.set_forgotten(&entry);
                            app.notice("That was already gone.", NoticeTone::Info);
                        }
                        app.0.scheduler.request();
                        Ok(())
                    });
                }))
            };
            rows.push(TabRow {
                spans: vec![
                    sp(format!("{} ", e.what.glyph()), Style::fg(e.what.color())),
                    sp(&e.title, if e.forgotten { st::FAINT } else { st::TEXT }),
                ],
                right: Some(sp(if e.forgotten { "forgotten" } else { "forget" }, if e.forgotten { st::FAINT } else { st::ACCENT })),
                action,
                // Every row says what it is: rows opening or closing around it leave the keyboard and the view on it.
                item: Some(format!("kept {}", e.title)),
                ..Default::default()
            });
        }
        if state_kept.len() > 3 {
            rows.push(TabRow {
                spans: vec![sp(format!("{} more kept · /memory", state_kept.len() - 3), st::FAINT)],
                item: Some("kept more".into()),
                ..Default::default()
            });
        }
        if !state_kept.is_empty() && !changes.is_empty() {
            rows.push(TabRow { item: Some("kept end".into()), ..Default::default() });
        }
        for change in changes.iter().rev() {
            let (marker, title_style, action) = change_marks(change);
            let lines = wrap(&[sp(&change.title, title_style)], (width - text_width(&action.text) - 3).max(1))
                .into_iter()
                .map(|s| s.into_iter().map(|s| s.text).collect::<String>())
                .collect::<Vec<_>>();
            let undo = if change.state == ChangeState::Applied {
                let id = change.id.clone();
                Some(self.click(move |app| {
                    let id = id.clone();
                    app.task(move |app| async move {
                        app.undo(Some(&id)).await;
                        Ok(())
                    });
                }))
            } else {
                None
            };
            // Enter (or a click on the words) opens the change's picture and closes it again; the label undoes it.
            let open = expanded.as_deref() == Some(change.id.as_str());
            let toggle: Rc<dyn Fn()> = {
                let id = change.id.clone();
                self.click(move |app| {
                    let mut state = app.0.state.borrow_mut();
                    state.expanded = if state.expanded.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                })
            };
            // → and Space open the picture, ← closes it, and `u` undoes only an opened change: typed otherwise, it's a
            // word for the box. With nothing left to do, ←, → and Space stay in the tabs rather than leave them.
            let stay: Rc<dyn Fn()> = Rc::new(|| {});
            let keys: Vec<(&'static str, Rc<dyn Fn()>)> = if open {
                let close = self.click(|app| app.0.state.borrow_mut().expanded = None);
                let mut keys = vec![("left", close), ("right", stay.clone()), ("space", stay)];
                keys.extend(undo.clone().map(|undo| ("u", undo)));
                keys
            } else {
                vec![("right", toggle.clone()), ("space", toggle.clone()), ("left", stay)]
            };
            let item = Some(change.id.clone());
            rows.push(TabRow {
                spans: vec![marker, sp(lines.first().map(String::as_str).unwrap_or(""), title_style)],
                right: Some(action),
                item: item.clone(),
                action: undo.clone(),
                open: Some(toggle.clone()),
                keys: keys.clone(),
                ..Default::default()
            });
            if lines.len() > 1 {
                rows.push(TabRow {
                    spans: vec![sp("  ", title_style), sp(lines[1..].join(" "), title_style)],
                    item: item.clone(),
                    action: undo.clone(),
                    open: Some(toggle.clone()),
                    keys: keys.clone(),
                    ..Default::default()
                });
            }
            if open {
                let cols = (width - 2).max(4) as usize;
                let extra = |spans: Vec<crate::tui::wrap::Span>| TabRow {
                    spans: [vec![sp("  ", st::FAINT)], spans].concat(),
                    item: item.clone(),
                    action: undo.clone(),
                    open: Some(toggle.clone()),
                    keys: keys.clone(),
                    ..Default::default()
                };
                match self.change_drawn(change, cols, self.picture_height(change)) {
                    Some((drawn, key)) => {
                        image = key;
                        rows.extend(drawn.rows.into_iter().chain(drawn.lines).map(extra));
                    }
                    // A change from before pictures (or one without its own): its value bar, swatch or notes, as NOW drew them.
                    None => match (self.0.graphics != Graphics::Words).then(|| change_picture(change, cols as i32, self.0.depth)).flatten()
                    {
                        Some(lines) => rows.extend(lines.into_iter().map(extra)),
                        None => rows.push(extra(vec![sp("No picture for this change", st::FAINT)])),
                    },
                }
            }
        }
        let rows: Rc<[TabRow]> = rows.into();
        let mut state = self.0.state.borrow_mut();
        state.history_rows = Some((revision, width, expanded, rows.clone()));
        state.history_image = image;
        drop(state);
        rows
    }
    /// The /goal objective: what it is, where it stands against its budget, the last check and what's next.
    fn objective_rows(&self, objective: &Timed<ObjectiveStatus>, width: i32) -> Vec<TabRow> {
        let clean = |text: &str, max| self.clean(&text.replace('\n', " "), max);
        let status = &objective.status;
        let running = status.state == ObjectiveState::Running;
        let elapsed = if running { perf_now() - objective.since } else { status.elapsed_ms as f64 };
        let mut rows = vec![];
        let mut line = |parts: Vec<super::super::wrap::Span>| {
            rows.extend(wrap(&parts, width).into_iter().map(|spans| TabRow { spans, ..Default::default() }))
        };
        line(vec![sp(clean(&status.objective, 300), st::TEXT)]);
        let state = match (status.state, status.verdict) {
            (ObjectiveState::Running, _) => "working".to_string(),
            (ObjectiveState::Done, Some(Verdict::Complete)) => "met".into(),
            (ObjectiveState::Done, _) => "stopped".into(),
            (ObjectiveState::Paused, Some(Verdict::Blocked)) => "blocked · /goal resume once it's dealt with".into(),
            (ObjectiveState::Paused, Some(Verdict::Budget)) => "out of budget · /goal resume carries on".into(),
            (ObjectiveState::Paused, Some(Verdict::Stuck)) => "stuck · /goal edit, or resume".into(),
            (ObjectiveState::Paused, None | Some(Verdict::Complete | Verdict::Continue)) => "paused · /goal resume carries on".into(),
        };
        line(vec![sp(
            format!(
                "{state} · turn {} of {} · {} of {}",
                status.turns,
                status.turn_budget,
                helpers::clock_of(elapsed),
                helpers::clock_of(status.budget_ms as f64)
            ),
            st::DIM,
        )]);
        let reason = clean(status.reason.as_deref().unwrap_or(""), 300);
        if !reason.is_empty() {
            let by = if status.measured { "measured" } else { "checked" };
            line(vec![
                sp(format!("{by}: "), st::DIM),
                sp(reason, if status.verdict == Some(Verdict::Complete) { st::ACCENT } else { st::TEXT }),
            ]);
        }
        let next = clean(status.next.as_deref().unwrap_or(""), 300);
        if !next.is_empty() {
            line(vec![sp("next: ", st::DIM), sp(next, st::TEXT)]);
        }
        rows
    }
    pub(super) fn goal_rows(&self, width: i32) -> Vec<TabRow> {
        if let Some(objective) = self.0.state.borrow().objective.clone() {
            return self.objective_rows(&objective, width);
        }
        let Some(goal) = self.0.state.borrow().goal.clone() else {
            return vec![];
        };
        let clean = |text: &str, max| self.clean(&text.replace('\n', " "), max);
        let status = &goal.status;
        let running = matches!(status.state, GoalPhase::Running | GoalPhase::Starting);
        let elapsed = if running { perf_now() - goal.since } else { status.elapsed_ms as f64 };
        let mut rows = vec![];
        let mut line = |parts: Vec<super::super::wrap::Span>| {
            rows.extend(wrap(&parts, width).into_iter().map(|spans| TabRow { spans, ..Default::default() }))
        };
        line(vec![sp(clean(&status.goal, 300), st::TEXT)]);
        let why = clean(status.why.as_deref().unwrap_or(""), 80);
        let state = match status.state {
            GoalPhase::Starting => "setting up".into(),
            GoalPhase::Running => "searching".into(),
            GoalPhase::Paused => {
                format!("paused{} · /loop carries on", if !why.is_empty() && why != "paused" { format!(" · {why}") } else { String::new() })
            }
            GoalPhase::Done => format!("done{}", if !why.is_empty() { format!(" · {why}") } else { String::new() }),
        };
        line(vec![sp(
            format!(
                "{state} · gen {} · {} heard · {} {} · {}",
                status.generation,
                status.rendered,
                status.candidates,
                if status.candidates == 1 { "candidate" } else { "candidates" },
                helpers::clock_of(elapsed)
            ),
            st::DIM,
        )]);
        if let Some(best) = &status.best {
            let trend = &status.trend[status.trend.len().saturating_sub((width - 16).max(4) as usize)..];
            let low = trend.iter().copied().fold(f64::INFINITY, f64::min);
            let high = trend.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let levels = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
            let spark = trend
                .iter()
                .map(|v| levels[if high > low { number::round((v - low) / (high - low) * 7.) as usize } else { 7 }])
                .collect::<String>();
            line(vec![
                sp(format!("{}%", number::to_string(best.score)), st::ACCENT),
                sp(
                    match status.first.filter(|first| *first != best.score) {
                        Some(first) => format!(" from {}%  ", number::to_string(first)),
                        None => "  ".into(),
                    },
                    st::DIM,
                ),
                sp(spark, st::ACCENT),
            ]);
            if let Some(leader) = status.leader.as_deref().filter(|leader| !leader.is_empty()) {
                line(vec![sp("best  ", st::FAINT), sp(clean(leader, 200), st::TEXT)]);
            }
        }
        for (text, label, max, style) in [(&status.idea, "tried  ", 200, st::DIM), (&status.best_track, "kept on  ", 80, st::TEXT)] {
            if let Some(text) = text.as_deref().filter(|text| !text.is_empty()) {
                line(vec![sp(label, st::FAINT), sp(clean(text, max), style)]);
            }
        }
        let tokens = self.tokens_used();
        if !tokens.is_empty() {
            line(vec![sp(tokens.strip_prefix(" · ").unwrap_or(&tokens), st::FAINT)]);
        }
        rows
    }
}

/// How HISTORY marks a change: its glyph (the track's chip where it has one), its title's style, and the word at its
/// end (undo, undone, a match's score…) in that word's style.
pub(super) fn change_marks(change: &ChangeRecord) -> (super::super::wrap::Span, Style, super::super::wrap::Span) {
    let heard = change.state == ChangeState::Heard;
    let action = if heard {
        change.score.map(|n| format!("{}%", number::to_string(n))).unwrap_or_else(|| "heard".into())
    } else {
        match change.state {
            ChangeState::Applied => "undo".into(),
            ChangeState::Undone => "undone".into(),
            ChangeState::Kept => "kept".into(),
            ChangeState::Expired => "no undo".into(),
            _ => format!("check {}", crate::daw::words().app),
        }
    };
    let action_style = if heard {
        st::DIM
    } else if change.state == ChangeState::Applied {
        st::ACCENT
    } else if matches!(change.state, ChangeState::Undone | ChangeState::Expired) {
        st::FAINT
    } else {
        st::WARN
    };
    let title_style = if heard {
        st::DIM
    } else if matches!(change.state, ChangeState::Undone | ChangeState::Expired) {
        st::FAINT
    } else {
        st::TEXT
    };
    let marker = if heard {
        sp("♪ ", st::FAINT)
    } else if matches!(change.state, ChangeState::Undone | ChangeState::Expired) {
        sp("○ ", st::FAINT)
    } else if change.state == ChangeState::Unsure {
        sp("● ", st::WARN)
    } else if let Some(track) = &change.track {
        sp("■ ", Style::fg(chip_color(track.color.as_deref())))
    } else {
        sp("✓ ", st::ACCENT)
    };
    (marker, title_style, sp(action, action_style))
}

/// A plug-in's name without what REAPER writes around it: its format ("VST3: ") and its maker ("(Cockos)").
fn short_name(name: &str) -> &str {
    const FORMATS: &[&str] = &["VST", "VSTi", "VST3", "VST3i", "AU", "AUi", "CLAP", "CLAPi", "LV2", "LV2i", "DX", "DXi", "JS"];
    let Some((_, name)) = name.split_once(": ").filter(|(format, _)| FORMATS.contains(format)) else { return name };
    match name.rfind(" (") {
        Some(at) if at > 0 && name.ends_with(')') => &name[..at],
        _ => name,
    }
}
