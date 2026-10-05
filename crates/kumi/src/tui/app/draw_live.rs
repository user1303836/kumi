use super::super::{
    activity::{activity_glyph, activity_scene},
    icons::{icon, track_kind},
    screen::Screen,
    style::Style,
    tree::{tree_window, TreeRole},
    width::{text_width, truncate},
    wrap::wrap,
};
use super::drawing::{num, sp, spans, st};
use super::*;
impl TuiApp {
    fn click<F: Fn(TuiApp) + 'static>(&self, f: F) -> Rc<dyn Fn()> {
        let weak = Rc::downgrade(&self.0);
        Rc::new(move || {
            if let Some(a) = weak.upgrade() {
                f(TuiApp(a));
            }
        })
    }
    fn hit<F: Fn(TuiApp) + 'static>(&self, x: i32, y: i32, width: i32, f: F) {
        let action = self.click(f);
        self.0.state.borrow_mut().hits.push(Hit { x, y, width, action });
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
        let rows = self.tree_shown();
        let clip = if rows.is_none() { self.clip_shown() } else { None };
        let strip = if rows.is_none() && clip.is_none() { self.strip_kind() } else { None };
        let (session, arrangement) = {
            let state = self.0.state.borrow();
            (
                if strip == Some("session") {
                    state.strip.session.clone().filter(|s| state.focus.as_ref().and_then(|f| f.track_ref.as_ref()) == Some(&s.track_ref))
                } else {
                    None
                },
                if strip == Some("arrangement") { state.strip.arrangement.clone() } else { None },
            )
        };
        self.keep_tree_fresh(if rows.is_some() {
            Some("tree")
        } else if clip.is_some() {
            Some("clip")
        } else if session.is_some() {
            Some("session")
        } else if arrangement.is_some() {
            Some("arrangement")
        } else {
            strip
        });
        let mut now_at = 6;
        if let Some(rows) = rows {
            screen.put(x + 5, area.y + 1, " · Device", &st::FAINT);
            now_at =
                4 + self.draw_tree(screen, x, area.y + 2, width, &rows, (area.height - Self::bottom_height(area.height) - 10).clamp(2, 12));
        } else if let Some(clip) = clip {
            screen.put(x + 5, area.y + 1, " · Clip", &st::FAINT);
            now_at = 4 + self.draw_clip(screen, x, area.y + 2, width, &clip);
        } else if let Some(session) = session {
            screen.put(x + 5, area.y + 1, " · Session", &st::FAINT);
            now_at = 4 + self.draw_session(screen, x, area.y + 2, width, &session);
        } else if let Some(arrangement) = arrangement {
            screen.put(x + 5, area.y + 1, " · Arrangement", &st::FAINT);
            now_at = 4 + self.draw_arrangement(screen, x, area.y + 2, width, &arrangement);
        } else if !self.draw_focus_path(screen, x, area.y + 2, width, true) {
            for (index, (text, style)) in self.focus_lines().iter().enumerate() {
                screen.put(x, area.y + 2 + index as i32, &truncate(text, width), style);
            }
        }
        screen.put(x, area.y + now_at, &truncate("NOW", width), &st::LABEL);
        let now = self.now_line();
        if let Some(dot) = now.dot {
            let label = format!(" {}", now.label);
            let at = x + width - text_width(&label) - 1;
            screen.put(at, area.y + now_at, "●", &dot);
            screen.put(at + 1, area.y + now_at, &label, &st::DIM);
        }
        let moment = perf_now();
        if let Some((_, since)) = now.activity {
            spans(screen, x, area.y + now_at + 1, &super::super::activity::shimmer(&truncate(&now.detail, width), moment - since), None);
        } else {
            screen.put(x, area.y + now_at + 1, &truncate(&now.detail, width), &now.style);
        }
        let picture = self.flashing().and_then(|c| change_picture(&c, width, self.0.depth));
        if let Some(picture) = &picture {
            for (row, line) in picture.iter().take(2).enumerate() {
                spans(screen, x, area.y + now_at + 2 + row as i32, line, None);
            }
        } else if let Some((kind, since)) = now.activity {
            spans(screen, x, area.y + now_at + 2, &activity_scene(kind, moment - since, width.min(28)), None);
        }
        let bottom = Self::bottom_height(area.height);
        let top = area.y + area.height - bottom;
        let tabs_area = Rect::new(x, top, width, bottom - 1);
        self.0.state.borrow_mut().tabs_area = Some(tabs_area);
        let mut hits = vec![];
        self.0.tabs.draw(screen, tabs_area, &mut hits);
        self.0.state.borrow_mut().hits.extend(hits);
    }
    fn bottom_height(pane: i32) -> i32 {
        ((pane as f64 / 2.).floor() as i32).max(7).min((pane - 8).max(1))
    }
    fn track_header(&self, screen: &mut Screen, x: i32, y: i32, width: i32) -> FocusTrack {
        let track = self.0.state.borrow().focus.as_ref().and_then(|f| f.track.clone()).unwrap();
        let kind = track.kind.map(|k| serde_json::to_value(k).unwrap().as_str().unwrap().to_string());
        let mark = icon(track_kind(kind.as_deref()), self.0.icons, Some(chip_color(track.color.as_deref())));
        screen.put(x, y, &mark.text, &mark.style);
        screen.put(x + 3, y, &truncate(&track.name, (width - 3).max(1)), &st::TEXT);
        track
    }
    fn draw_clip(&self, screen: &mut Screen, x: i32, y: i32, width: i32, clip: &ClipView) -> i32 {
        let track = self.0.state.borrow().focus.as_ref().and_then(|f| f.track.clone()).unwrap();
        let kind = track.kind.map(|k| serde_json::to_value(k).unwrap().as_str().unwrap().to_string());
        let mark = icon(track_kind(kind.as_deref()), self.0.icons, Some(chip_color(track.color.as_deref())));
        screen.put(x, y, &mark.text, &mark.style);
        let clip_mark = icon(IconKind::MidiClip, self.0.icons, None);
        let mut column = screen.put(x + 3, y, &truncate(&track.name, (((width - 6) as f64 / 2.).floor() as i32).max(1)), &st::TEXT);
        column = screen.put(column, y, " › ", &st::FAINT);
        column = screen.put(column, y, &clip_mark.text, &clip_mark.style) + 1;
        screen.put(
            column,
            y,
            &truncate(if clip.name.is_empty() { "Untitled clip" } else { &clip.name }, (x + width - column).max(1)),
            &st::BRIGHT,
        );
        let picture = clip_picture(clip.length, &clip.notes, width.min(32), 4);
        if let Some(picture) = &picture {
            for (row, line) in picture.iter().enumerate() {
                spans(screen, x, y + 1 + row as i32, line, None);
            }
        }
        let bars = clip.length / 4.;
        let selected = clip.notes.iter().filter(|n| n.selected == Some(true)).count();
        let mut facts = vec![
            if bars.fract() == 0. {
                format!("{} {}", number::to_string(bars), if bars == 1. { "bar" } else { "bars" })
            } else {
                format!("{} beats", number::to_string(number::round(clip.length * 100.) / 100.))
            },
            format!("{} {}", clip.notes.len(), if clip.notes.len() == 1 { "note" } else { "notes" }),
        ];
        if selected > 0 {
            facts.push(format!("{selected} selected"));
        }
        let facts = facts.join(" · ");
        let picture_rows = if picture.is_some() { 4 } else { 0 };
        screen.put(
            x,
            y + 1 + picture_rows,
            &truncate(&if picture.is_some() { facts } else { format!("{facts} · empty") }, width),
            &st::FAINT,
        );
        2 + picture_rows
    }
    fn draw_session(&self, screen: &mut Screen, x: i32, y: i32, width: i32, strip: &SessionStrip) -> i32 {
        self.track_header(screen, x, y, width);
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
    fn draw_arrangement(&self, screen: &mut Screen, x: i32, y: i32, width: i32, strip: &ArrangementStrip) -> i32 {
        self.track_header(screen, x, y, width);
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
        spans(screen, x, y + 1, &line, None);
        let bar = |beats: f64| (beats / 4.).floor() + 1.;
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
        whole.push(format!("{} bars", number::to_string(bar(strip.length) - 1.)));
        screen.put(x, y + 2, &truncate(&where_.join(" · "), width), &st::DIM);
        screen.put(x, y + 3, &truncate(&whole.join(" · "), width), &st::FAINT);
        4
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
        self.track_header(screen, x, y, width);
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
            let pinned = self.0.state.borrow().pinned.as_ref().is_some_and(|p| p.pin.r#ref == item.r#ref);
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
            let name = truncate(
                &item.name,
                (x + width - column - text_width(&count) - if label.is_empty() { 0 } else { text_width(label) + 1 }).max(1),
            );
            column = screen.put(column, row, &name, &style);
            if !count.is_empty() {
                screen.put(column, row, &count, &st::FAINT);
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
        let last = self.0.state.borrow().changes.last().cloned();
        if area.height > 1 {
            if let Some(last) = last.filter(|c| !self.busy() && c.state == ChangeState::Applied) {
                let at = area.width - 6;
                screen.put(2, area.y + 1, &truncate(&format!("✓ {}", last.title), (at - 4).max(1)), &st::TEXT);
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
    pub(super) fn history_rows(&self, width: i32) -> Vec<TabRow> {
        let mut rows = vec![];
        let (state_kept, changes) = {
            let state = self.0.state.borrow();
            (state.kept.clone(), state.changes.clone())
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
                        let gone = forget().await.unwrap_or(false);
                        if !gone && !entry.borrow().forgotten {
                            entry.borrow_mut().forgotten = true;
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
                ..Default::default()
            });
        }
        if state_kept.len() > 3 {
            rows.push(TabRow {
                spans: vec![sp(format!("{} more kept · /memory", state_kept.len() - 3), st::FAINT)], ..Default::default()
            });
        }
        if !state_kept.is_empty() && !changes.is_empty() {
            rows.push(TabRow::default());
        }
        for (place, change) in changes.iter().rev().enumerate() {
            let heard = change.state == ChangeState::Heard;
            let action = if heard {
                change.score.map(|n| format!("{}%", number::to_string(n))).unwrap_or_else(|| "heard".into())
            } else {
                match change.state {
                    ChangeState::Applied => "undo",
                    ChangeState::Undone => "undone",
                    ChangeState::Kept => "kept",
                    ChangeState::Expired => "no undo",
                    _ => "check Live",
                }
                .into()
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
            let lines = wrap(&[sp(&change.title, title_style)], (width - text_width(&action) - 3).max(1))
                .into_iter()
                .map(|s| s.into_iter().map(|s| s.text).collect::<String>())
                .collect::<Vec<_>>();
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
            rows.push(TabRow {
                spans: vec![marker, sp(lines.first().map(String::as_str).unwrap_or(""), title_style)],
                right: Some(sp(action, action_style)),
                item: Some(format!("change {place}")),
                action: undo.clone(),
            });
            if lines.len() > 1 {
                rows.push(TabRow {
                    spans: vec![sp("  ", title_style), sp(lines[1..].join(" "), title_style)],
                    item: Some(format!("change {place}")),
                    action: undo,
                    ..Default::default()
                });
            }
        }
        rows
    }
    pub(super) fn goal_rows(&self, width: i32) -> Vec<TabRow> {
        let Some(goal) = self.0.state.borrow().goal.clone() else {
            return vec![];
        };
        let clean = |v: &Value, max| self.clean(&v.as_str().unwrap_or("").replace('\n', " "), max);
        let running = matches!(goal["state"].as_str(), Some("running" | "starting"));
        let elapsed = if running { perf_now() - goal["since"].as_f64().unwrap_or(0.) } else { goal["elapsedMs"].as_f64().unwrap_or(0.) };
        let mut rows = vec![];
        let mut line = |parts: Vec<super::super::wrap::Span>| {
            rows.extend(wrap(&parts, width).into_iter().map(|spans| TabRow { spans, ..Default::default() }))
        };
        line(vec![sp(clean(&goal["goal"], 300), st::TEXT)]);
        let why = clean(&goal["why"], 80);
        let state = match goal["state"].as_str().unwrap_or("") {
            "starting" => "setting up".into(),
            "running" => "searching".into(),
            "paused" => {
                format!("paused{} · /goal carries on", if !why.is_empty() && why != "paused" { format!(" · {why}") } else { String::new() })
            }
            _ => format!("done{}", if !why.is_empty() { format!(" · {why}") } else { String::new() }),
        };
        line(vec![sp(
            format!(
                "{state} · gen {} · {} heard · {} {} · {}",
                num(&goal["generation"]),
                num(&goal["rendered"]),
                num(&goal["candidates"]),
                if goal["candidates"].as_f64() == Some(1.) { "candidate" } else { "candidates" },
                helpers::clock_of(elapsed)
            ),
            st::DIM,
        )]);
        if let Some(best) = goal.get("best") {
            let trend = goal["trend"].as_array().cloned().unwrap_or_default();
            let trend = &trend[trend.len().saturating_sub((width - 16).max(4) as usize)..];
            let low = trend.iter().filter_map(Value::as_f64).fold(f64::INFINITY, f64::min);
            let high = trend.iter().filter_map(Value::as_f64).fold(f64::NEG_INFINITY, f64::max);
            let levels = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
            let spark = trend
                .iter()
                .map(|v| levels[if high > low { number::round((v.as_f64().unwrap_or(0.) - low) / (high - low) * 7.) as usize } else { 7 }])
                .collect::<String>();
            line(vec![
                sp(format!("{}%", num(&best["score"])), st::ACCENT),
                sp(
                    if goal.get("first").is_some_and(|v| v != &best["score"]) {
                        format!(" from {}%  ", num(&goal["first"]))
                    } else {
                        "  ".into()
                    },
                    st::DIM,
                ),
                sp(spark, st::ACCENT),
            ]);
            if goal["leader"].as_str().is_some_and(|s| !s.is_empty()) {
                line(vec![sp("best  ", st::FAINT), sp(clean(&goal["leader"], 200), st::TEXT)]);
            }
        }
        for (key, label, max, style) in [("idea", "tried  ", 200, st::DIM), ("bestTrack", "kept on  ", 80, st::TEXT)] {
            if goal[key].as_str().is_some_and(|s| !s.is_empty()) {
                line(vec![sp(label, st::FAINT), sp(clean(&goal[key], max), style)]);
            }
        }
        let tokens = self.tokens_used();
        if !tokens.is_empty() {
            line(vec![sp(tokens.strip_prefix(" · ").unwrap_or(&tokens), st::FAINT)]);
        }
        rows
    }
}
