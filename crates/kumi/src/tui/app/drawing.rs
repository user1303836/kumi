use super::super::{
    activity::{activity_glyph, activity_of, shimmer, Activity},
    editor::EditorLayout,
    logo::*,
    render::Cursor,
    screen::Screen,
    style::{palette, Style},
    transcript::{doing_label, LiveRow, Row},
    width::{text_width, truncate},
    wrap::Span,
};
use super::*;
use crate::text::library_line;
use kumi_ai::failure::reset_words;
use kumi_common::time::since;
pub(super) mod st {
    use super::{palette, Style};
    pub const GROUND: Style = Style::bg(palette::GROUND);
    pub const SURFACE: Style = Style::bg(palette::SURFACE);
    pub const RAISED: Style = Style::bg(palette::RAISED);
    pub const SELECTED: Style = Style::bg(palette::SELECTED);
    pub const BRIGHT: Style = Style::fg(palette::BRIGHT);
    pub const TITLE: Style = Style { bold: true, ..BRIGHT };
    pub const TEXT: Style = Style::fg(palette::TEXT);
    pub const DIM: Style = Style::fg(palette::DIM);
    pub const FAINT: Style = Style::fg(palette::FAINT);
    pub const LABEL: Style = Style { bold: true, ..FAINT };
    pub const ACCENT: Style = Style::fg(palette::ACCENT);
    pub const WARN: Style = Style::fg(palette::WARN);
}
pub(super) fn sp(text: impl Into<String>, style: Style) -> Span {
    Span::styled(text, style)
}
pub(super) fn spans(screen: &mut Screen, mut x: i32, y: i32, spans: &[Span], clip: Option<Rect>) -> i32 {
    for span in spans {
        x = if let Some(clip) = clip {
            screen.put_in(x, y, &span.text, &span.style, clip)
        } else {
            screen.put(x, y, &span.text, &span.style)
        };
    }
    x
}
/// How long a step away from Live runs before NOW says ctrl-b can move it to the background.
const BACKGROUND_HINT_MS: f64 = 20_000.;
pub(super) struct NowLine {
    pub dot: Option<Style>,
    pub label: String,
    pub detail: String,
    pub style: Style,
    pub activity: Option<(Activity, f64)>,
}
impl TuiApp {
    pub(super) fn draw(&self) {
        // A panic Kumi caught (a worker's) gave the terminal back: take it again, and draw all of it.
        if self.0.tty.recover() {
            self.0.renderer.borrow_mut().invalidate();
            self.0.state.borrow_mut().status_sent = None;
        }
        if !self.0.tty.is_active() {
            return;
        }
        let size = self.0.tty.size();
        let (columns, rows) = (size.columns, size.rows);
        let kept = self.0.screen.borrow_mut().take();
        let mut screen = kept
            .filter(|kept| kept.width == columns && kept.height == rows && Rc::ptr_eq(&kept.table, &self.0.table))
            .unwrap_or_else(|| Screen::with_table(columns, rows, self.0.table.clone()));
        // Every cell painted over: nothing of the last frame stays.
        screen.fill(screen.bounds(), &st::GROUND);
        self.0.state.borrow_mut().hits.clear();
        if columns < 24 || rows < 8 {
            // No pane to keep fresh here, nor during setup or beside the dock below.
            self.keep_tree_fresh(None);
            screen.put(1, 0, &truncate("Make this window bigger for Kumi", columns - 2), &st::DIM);
            self.present(screen, None);
            return;
        }
        if self.setup_active() {
            self.keep_tree_fresh(None);
            let cursor = self.draw_setup(&mut screen, columns, rows).filter(|_| !self.0.state.borrow().closing);
            self.present(screen, cursor);
            return;
        }
        let (pane, left) = Self::layout_for(columns);
        self.draw_header(&mut screen, columns);
        let layout = self.0.state.borrow().editor.layout((left - 6).max(1));
        let visible = layout.rows.len().min(5) as i32;
        let box_height = visible + 2;
        let box_top = rows - 1 - box_height;
        let dock = if pane != 0 { 0 } else { 2 };
        let (chip, files, waiting) = {
            let state = self.0.state.borrow();
            (i32::from(state.pinned.is_some()), i32::from(!state.attachments.is_empty()), state.held.len().min(3) as i32)
        };
        let area = Rect::new(0, 2, left, (box_top - dock - 3 - chip - files - waiting).max(1));
        self.0.state.borrow_mut().page = (area.height - 2).max(1);
        self.draw_conversation(&mut screen, area);
        if pane != 0 {
            self.draw_pane(&mut screen, Rect::new(left, 1, pane, rows - 1));
        } else {
            self.keep_tree_fresh(None);
            self.draw_dock(&mut screen, Rect::new(0, box_top - dock - 1, columns, dock));
        }
        if chip != 0 {
            self.draw_pin(&mut screen, 3, box_top - dock - 2, left - 6);
        }
        if files != 0 {
            self.draw_attachments(&mut screen, 3, box_top - dock - 2 - chip, left - 6);
        }
        let no_panel = self.0.state.borrow().panel.is_none();
        if waiting != 0 && no_panel && self.menu().is_empty() {
            self.draw_held(&mut screen, 3, box_top - dock - 1 - chip - files - waiting, left - 6, waiting);
        }
        let mut cursor = Some(self.draw_composer(&mut screen, Rect::new(1, box_top, left - 2, box_height), &layout, visible));
        let panel = self.0.state.borrow().panel.clone();
        if let Some(panel) = panel {
            cursor = self.draw_panel(&mut screen, &panel, box_top, left);
        } else {
            self.draw_menu(&mut screen, box_top, left);
        }
        if self.0.state.borrow().closing {
            cursor = None;
        }
        self.present(screen, cursor);
    }
    /// The frame out to the terminal; its screen is kept to draw the next one on.
    fn present(&self, screen: Screen, cursor: Option<Cursor>) {
        let frame = self.0.renderer.borrow_mut().frame(&screen, cursor);
        // Images go up before the cells that show them, inside the frame's synchronized update so a picture taken down
        // and its cells change at once; those no longer shown are taken down.
        self.touch_images();
        let images = self.0.state.borrow_mut().images.writes();
        let sync = "\u{1b}[?2026h";
        match frame.strip_prefix(sync) {
            Some(rest) if !images.is_empty() => self.0.tty.write(&format!("{sync}{images}{rest}")),
            _ => {
                self.0.tty.write(&images);
                self.0.tty.write(&frame);
            }
        }
        *self.0.screen.borrow_mut() = Some(screen);
        // Between frames, the terminal's status follows what was drawn.
        if !self.0.state.borrow().closing {
            self.show_status();
        }
    }
    fn status_line(&self) -> (Style, String) {
        let state = self.0.state.borrow();
        let daw = crate::daw::words().app;
        // A chat without the DAW starts disconnected and stays so, unless setup connects it mid-session.
        if matches!(state.connection, ConnectionState::Disconnected | ConnectionState::Error) {
            (st::WARN, format!("{daw} not connected"))
        } else if state.connection == ConnectionState::Connecting {
            (st::FAINT, format!("connecting to {daw}…"))
        } else if state.focus.as_ref().is_some_and(|focus| focus.busy.is_some()) {
            // There, but answering late (a dialog is open): not "not connected".
            (st::WARN, format!("{daw} busy"))
        } else {
            (st::ACCENT, daw.to_string())
        }
    }
    fn draw_header(&self, screen: &mut Screen, columns: i32) {
        let (dot, status) = self.status_line();
        let mut start = columns - 2 - text_width(&format!("● {status}"));
        if let Some((text, dot)) = self.beat_light() {
            if start - text_width(&format!("● {text}")) - 3 > 12 {
                let at = start - 3 - text_width(&format!("● {text}"));
                screen.put(at, 0, "●", &dot);
                screen.put(at + 1, 0, &format!(" {text}"), &st::FAINT);
                start = at;
            }
        }
        let mut x = screen.put(2, 0, "Kumi", &st::TITLE);
        // Background jobs at work: a count the model's name gives way to.
        let running = self.running_jobs();
        if running > 0 {
            let jobs = format!(
                "{}{running} {}",
                if self.0.icons == IconStyle::Badges { "" } else { "◐ " },
                if running == 1 { "job" } else { "jobs" }
            );
            let at = start - 3 - text_width(&jobs);
            if at - x > 4 {
                screen.put(at, 0, &jobs, &st::ACCENT);
                start = at;
            }
        }
        let mut end = start - 2;
        let set_name = self.0.state.borrow().set_name.clone();
        if let Some(model) = self.model_label() {
            let text = truncate(&model, 36);
            let at = start - 3 - text_width(&text);
            let named = x + set_name.as_ref().map(|n| text_width(n).min(16) + 5).unwrap_or(0);
            if at - named >= 2 {
                let chosen = self.0.options.models.as_ref().is_some_and(|m| m.current().model.is_some());
                let fallback = self.0.options.controller.answering_fallback().is_some();
                screen.put(at, 0, &text, &if chosen && !fallback { st::FAINT } else { st::WARN });
                end = at - 3;
                // Near a plan's limit: how much is used and when it resets, before the model.
                if let Some((plan, nearly)) = self.plan_line() {
                    let at = end - text_width(&plan);
                    if at - named >= 2 {
                        screen.put(at, 0, &plan, &if nearly { st::WARN } else { st::DIM });
                        end = at - 3;
                    }
                }
            }
        }
        if let Some(name) = set_name.filter(|s| !s.is_empty()) {
            x = screen.put(x, 0, "  ·  ", &st::FAINT);
            screen.put(x, 0, &truncate(&name, (end - x).max(0)), &st::TEXT);
        }
        let live = columns - 2 - text_width(&format!("● {status}"));
        screen.put(live, 0, "●", &dot);
        screen.put(live + 1, 0, &format!(" {status}"), &st::DIM);
    }
    fn draw_conversation(&self, screen: &mut Screen, area: Rect) {
        let x = area.x + 3;
        let width = (area.width - 5).max(1);
        let now = perf_now();
        let (rows, next) = {
            let mut state = self.0.state.borrow_mut();
            let rows = state.transcript.layout(width, now);
            let next = state.transcript.change_at(now);
            (rows, next)
        };
        if let Some(next) = next {
            self.wake_at(next, now);
        }
        if rows.is_empty() {
            self.draw_welcome(screen, x, area.y + 2, width, area.height - 2);
            return;
        }
        let (start, scroll) = {
            let mut state = self.0.state.borrow_mut();
            let total = rows.len() as i32;
            if state.scroll > 0 && total != state.last_total {
                state.scroll = state.scroll.saturating_add(total - state.last_total).max(0);
            }
            state.last_total = total;
            state.scroll = state.scroll.min((total - area.height).max(0));
            ((total - area.height - state.scroll).max(0) as usize, state.scroll)
        };
        for (index, row) in rows.iter().skip(start).take(area.height.max(0) as usize).enumerate() {
            self.draw_row(screen, row, x, area.y + index as i32, width, area, now);
        }
        if scroll > 0 {
            let hint = "newer below · page down";
            screen.fill(Rect::new(area.x, area.y + area.height - 1, area.width, 1), &st::GROUND);
            screen.put(area.x + area.width - 2 - text_width(hint), area.y + area.height - 1, hint, &st::FAINT);
        }
    }
    fn draw_row(&self, screen: &mut Screen, row: &Row, x: i32, y: i32, width: i32, clip: Rect, now: f64) {
        if let Some(band) = &row.band {
            screen.fill(Rect::new(x - 1, y, band.width.min(width + 2), 1), &Style::bg(band.bg));
        }
        let mut column = x;
        let mut trailing = row.trailing.clone();
        if let Some(live) = &row.live {
            let end = x + width.min(46) - 7;
            match live {
                LiveRow::Header { label, since } => {
                    column = screen.put_in(column, y, "▾ ", &st::FAINT, clip);
                    column = spans(screen, column, y, &shimmer(label, now - since.unwrap_or(0.)), Some(clip));
                    if let Some(since) = since {
                        trailing = Some(sp(helpers::elapsed(now - since), st::FAINT));
                    }
                }
                LiveRow::Step { activity, since, label, doing } => {
                    let ms = now - since;
                    column = screen.put_in(column, y, "│ ", &Style::fg(palette::RULE), clip);
                    let glyph = activity_glyph(*activity, ms, self.0.icons == IconStyle::Badges);
                    column = screen.put_in(column, y, &glyph.text, &glyph.style, clip);
                    column = screen.put_in(column, y, " ", &st::DIM, clip);
                    column = spans(screen, column, y, &shimmer(label, ms), Some(clip));
                    if let Some(doing) = doing.as_ref().filter(|s| !s.is_empty()) {
                        if end - column > 4 {
                            column = screen.put_in(column, y, &truncate(&format!(" · {doing}"), end - column), &st::FAINT, clip);
                        }
                    }
                    trailing = Some(sp(helpers::elapsed(ms), st::FAINT));
                }
            }
        } else {
            column = spans(screen, column, y, &row.spans, Some(clip));
        }
        if let Some(trailing) = trailing {
            let at = x + width.min(46) - text_width(&trailing.text);
            if at > column {
                screen.put_in(at, y, &trailing.text, &trailing.style, clip);
            }
        }
    }
    fn draw_welcome(&self, screen: &mut Screen, x: i32, y: i32, width: i32, height: i32) {
        let state = self.0.state.borrow();
        let mut rows: Vec<(String, Style, bool)> = vec![];
        let mut add = |text: String, style: Style| rows.push((text, style, false));
        if state.connection == ConnectionState::Connected && state.set_name.as_ref().is_some_and(|s| !s.is_empty()) {
            let name = state.set_name.as_ref().unwrap();
            add(format!("Kumi can see {name}."), st::DIM);
            if let Some(catch) = state.catch_up.as_ref().filter(|c| &c.set == name) {
                add(String::new(), st::TEXT);
                let when = since(catch.last_seen_at as f64, now_ms_f64());
                if catch.lines.is_empty() {
                    add(format!("Nothing changed since you were last here, {when}."), st::DIM);
                } else {
                    add(format!("Since you were last here · {when}"), st::FAINT);
                    for change in &catch.lines {
                        add(format!("  • {change}"), st::TEXT);
                    }
                    if catch.more > 0 {
                        add(format!("  and {} more {}", catch.more, if catch.more == 1 { "change" } else { "changes" }), st::FAINT);
                    }
                }
            }
            add(String::new(), st::TEXT);
            add("Try".into(), st::FAINT);
            for text in ["  “What's on this track?”", "  “Set the tempo to 124”", "  “Why might my low end sound muddy?”"] {
                add(text.into(), st::TEXT);
            }
        } else {
            add("Ask anything about production.".into(), st::DIM);
            add(String::new(), st::TEXT);
            add("  “How do I make my kick punchier?”".into(), st::TEXT);
        }
        add(String::new(), st::TEXT);
        add(format!("Kumi keeps each {}'s conversations: /conversations goes back to one.", crate::daw::words().place), st::FAINT);
        if let Some(line) = library_line(state.library.as_ref()) {
            add(line, st::FAINT);
        }
        if state.willington_off {
            if let Some(words) = crate::daw::current().toggle_words() {
                add(words.off_at_start.into(), st::FAINT);
            }
        }
        if let Some(newer) = &state.newer {
            add(String::new(), st::TEXT);
            add(format!("Kumi {newer} is out · /update gets it"), st::ACCENT);
        }
        if width >= LOGO_WIDTH && height >= rows.len() as i32 + LOGO_HEIGHT + 1 {
            let mut logo = LOGO_LETTERS.iter().map(|s| (s.to_string(), st::BRIGHT, true)).collect::<Vec<_>>();
            logo.extend([(String::new(), st::TEXT, false), (LOGO_RULE.into(), st::FAINT, true), (String::new(), st::TEXT, false)]);
            logo.extend(rows);
            rows = logo;
        }
        let top = y + ((height - rows.len() as i32) as f64 / 2.).floor().max(0.) as i32;
        for (index, (text, style, center)) in rows.iter().enumerate() {
            let text = truncate(if !string::trim(text).is_empty() && !*center { string::trim(text) } else { text }, width);
            if !text.is_empty() {
                screen.put(x + ((width - text_width(&text)) as f64 / 2.).floor().max(0.) as i32, top + index as i32, &text, style);
            }
        }
    }
    pub(super) fn flashing(&self) -> Option<ChangeRecord> {
        let state = self.0.state.borrow();
        let (id, at) = state.last_change.as_ref()?;
        if perf_now() - at >= CHANGE_FLASH_MS {
            return None;
        }
        state.records.changes().iter().find(|c| &c.id == id).cloned()
    }
    pub(super) fn now_line(&self) -> NowLine {
        let turn = self.0.options.controller.status().state;
        let state = self.0.state.borrow();
        let simple = |detail: &str, style: Style| NowLine { dot: None, label: String::new(), detail: detail.into(), style, activity: None };
        if state.closing {
            return simple("Closing…", st::DIM);
        }
        if turn == TurnState::Cancelling || state.cancelling {
            return NowLine { dot: Some(st::FAINT), label: "stopping".into(), ..simple("Stopping…", st::DIM) };
        }
        let now = perf_now();
        let flash = state
            .last_change
            .as_ref()
            .filter(|(_, at)| now - at < CHANGE_FLASH_MS)
            .and_then(|(id, _)| state.records.changes().iter().find(|c| &c.id == id));
        let action = state.last_action.as_ref().filter(|a| now - a.at < CHANGE_FLASH_MS);
        if turn == TurnState::Running {
            // A steady dot: NOW's glyph already shows Kumi is at work.
            let dot = Some(st::ACCENT);
            let entry = state.current.as_ref().map(|e| e.borrow());
            let (steps, started) = if let Some(Entry::Assistant { steps, started_at, .. }) = entry.as_deref() {
                (steps.as_slice(), *started_at)
            } else {
                (&[][..], None)
            };
            let running = steps.last().filter(|s| s.state == StepState::Running);
            let label = if let Some(goal) = state.goal.as_ref().filter(|g| matches!(g["state"].as_str(), Some("running" | "starting"))) {
                format!(
                    "goal · {}gen {} · {}",
                    goal.get("best").map(|b| format!("{}% · ", num(&b["score"]))).unwrap_or_default(),
                    num(&goal["generation"]),
                    helpers::clock_of(now - goal["since"].as_f64().unwrap_or(0.))
                )
            } else if let Some(goal) = state.objective.as_ref().filter(|g| g["state"] == "running" && state.looping.is_none()) {
                format!(
                    "goal · turn {}/{} · {}",
                    num(&goal["turns"]).parse::<f64>().map(|t| t + 1.).map(|t| t.to_string()).unwrap_or_else(|_| "1".into()),
                    num(&goal["turnBudget"]),
                    helpers::clock_of(now - goal["since"].as_f64().unwrap_or(0.))
                )
            } else if let Some(looping) = &state.looping {
                format!(
                    "loop · round {} · {} kept · {} back{} · {}",
                    num(&looping["rounds"]),
                    num(&looping["kept"]),
                    num(&looping["reverted"]),
                    looping["next"].as_str().map(|next| format!(" · next: {}", next.to_lowercase())).unwrap_or_default(),
                    helpers::clock_of(now - looping["since"].as_f64().unwrap_or(0.))
                )
            } else if let Some(matching) = &state.matching {
                format!(
                    "matching · {}{}",
                    matching
                        .get("best")
                        .map(|best| format!(
                            "{}{}% · ",
                            matching
                                .get("first")
                                .filter(|first| **first != best["score"])
                                .map(|n| format!("{}→", num(n)))
                                .unwrap_or_default(),
                            num(&best["score"])
                        ))
                        .unwrap_or_default(),
                    helpers::clock_of(now - matching["since"].as_f64().unwrap_or(0.))
                )
            } else if state.turn_changes > 0 {
                format!("working · {} {}", state.turn_changes, if state.turn_changes == 1 { "change" } else { "changes" })
            } else {
                "working".into()
            };
            // A provider asked Kumi to wait: say why and for how long (esc still stops).
            if let Some((reason, until)) = state.retry.as_ref().filter(|(_, until)| *until > now) {
                let seconds = ((until - now) / 1000.).ceil().max(1.);
                let detail = match state.waiting_for {
                    _ if until.is_infinite() => format!("carrying on · {reason}"),
                    // A plan's limit: the answer carries on when it resets.
                    Some((WaitFor::Limit, at)) => {
                        let wall = now_ms_f64();
                        format!("carrying on at {} (in {}) · {reason}", reset_words(at, wall), helpers::span_words(at - wall))
                    }
                    Some((WaitFor::Offline, _)) => {
                        let word = match state.waiting_cause {
                            Some(NetCause::Tls) => "TLS failed",
                            // As the words say: a proxy that isn't answering, or one that answered no.
                            Some(NetCause::Proxy) if state.waiting_proxy_down => "proxy not answering",
                            Some(NetCause::Proxy) => "proxy refused",
                            _ => "offline",
                        };
                        format!("{word} · trying again in {seconds}s")
                    }
                    None => format!("retrying in {seconds}s · {reason}"),
                };
                return NowLine { dot, label, detail, style: st::DIM, activity: None };
            }
            if let Some(action) = action.filter(|a| flash.is_none() || a.at > state.last_change.as_ref().unwrap().1) {
                if action.memory
                    || running.is_none()
                    || running.is_some_and(|s| (crate::daw::presentation().acts)(s.tool.as_deref().unwrap_or("")))
                {
                    return NowLine { dot, label, detail: format!("{} {}", action.glyph, action.title), style: st::BRIGHT, activity: None };
                }
            }
            if let Some(flash) = flash.filter(|_| {
                running.is_none() || running.is_some_and(|s| (crate::daw::presentation().plan)(s.tool.as_deref().unwrap_or("")))
            }) {
                return NowLine {
                    dot,
                    label,
                    detail: format!("{} {}", if flash.state == ChangeState::Heard { "♪" } else { "✓" }, flash.title),
                    style: st::BRIGHT,
                    activity: None,
                };
            }
            if let Some(running) = running {
                let mut detail = running.doing.clone().unwrap_or_else(|| doing_label(running.tool.as_deref(), &running.label));
                // A long step away from Live can go on in the background while the producer carries on.
                if running.started_at.is_some_and(|at| now - at >= BACKGROUND_HINT_MS) && self.0.options.controller.can_background() {
                    // In tmux, ctrl-b is its prefix: twice sends it on.
                    let keys = if self.0.profile.tmux { "ctrl-b twice" } else { "ctrl-b" };
                    detail = format!("{detail} · {keys} keeps you working while this runs");
                }
                return NowLine {
                    dot,
                    label,
                    detail,
                    style: st::DIM,
                    activity: Some((activity_of(running.tool.as_deref()), running.started_at.unwrap_or(now))),
                };
            }
            if state.planning.is_some() {
                return NowLine {
                    dot,
                    label,
                    detail: "writing the plan".into(),
                    style: st::DIM,
                    activity: Some((Activity::Code, state.planning_since)),
                };
            }
            return NowLine {
                dot,
                label,
                detail: if state.current.is_some() { "thinking".into() } else { state.activity.clone() },
                style: st::DIM,
                activity: Some((Activity::Think, steps.last().and_then(|s| s.ended_at).or(started).unwrap_or(state.busy_since))),
            };
        }
        if let Some(flash) = flash.filter(|_| !action.is_some_and(|a| a.at > state.last_change.as_ref().unwrap().1)) {
            // A fixed phrase's change, made with no model call.
            let instant = if state.instant_changes.contains(&flash.id) { " · instant" } else { "" };
            return simple(&format!("{} {}{instant}", if flash.state == ChangeState::Heard { "♪" } else { "✓" }, flash.title), st::BRIGHT);
        }
        if let Some(action) = action {
            return simple(&format!("{} {}", action.glyph, action.title), st::BRIGHT);
        }
        if state.watching {
            return NowLine {
                dot: Some(st::ACCENT),
                label: "watching".into(),
                ..simple(&format!("Watching your changes in {}; tell Kumi when you're done", crate::daw::words().app), st::TEXT)
            };
        }
        let jobs = state.jobs.values().filter(|status| status.state == kumi_agent::core::jobs::JobState::Running).count();
        if jobs > 0 {
            return simple(&format!("Ready · {jobs} {} running in the background", if jobs == 1 { "job" } else { "jobs" }), st::FAINT);
        }
        simple("Ready", st::FAINT)
    }
    pub(super) fn focus_lines(&self) -> Vec<(String, Style)> {
        let state = self.0.state.borrow();
        if state.connection == ConnectionState::Connected && state.set_name.as_ref().is_some_and(|s| !s.is_empty()) {
            vec![(state.set_name.clone().unwrap(), st::BRIGHT), ("Your selection will show here".into(), st::FAINT)]
        } else if state.connection == ConnectionState::Connecting {
            vec![(format!("Connecting to {}…", crate::daw::words().app), st::DIM)]
        } else {
            vec![(format!("{} isn't connected", crate::daw::words().app), st::DIM)]
        }
    }
    fn draw_composer(&self, screen: &mut Screen, box_: Rect, layout: &EditorLayout, visible: i32) -> Cursor {
        screen.fill(box_, &st::RAISED);
        let x = box_.x + 2;
        let width = (box_.width - 4).max(1);
        let first = (layout.cursor_row as i32 - visible + 1).min(layout.rows.len() as i32 - visible).max(0);
        let voice = self.0.voice.as_ref().and_then(|v| v.view(perf_now()));
        let menu = self.menu();
        let busy = self.busy();
        let state = self.0.state.borrow();
        if state.editor.is_empty() {
            let placeholder = voice.as_ref().map(|v| v.placeholder.clone()).unwrap_or_else(|| {
                if busy && (state.current.is_some() || state.pending_turn) {
                    "Tell Kumi more while it works".into()
                } else if state.connection == ConnectionState::Connected {
                    format!("Ask Kumi about your {}", crate::daw::words().place)
                } else {
                    "Ask Kumi anything".into()
                }
            });
            screen.put(x, box_.y + 1, &truncate(&placeholder, width), &st::FAINT);
        } else {
            for (index, row) in layout.rows.iter().skip(first as usize).take(visible as usize).enumerate() {
                screen.put_in(x, box_.y + 1 + index as i32, row, &st::BRIGHT, box_);
            }
        }
        // A list's own words when it has them (a countdown to Kumi's pick, another way to choose).
        let own_hint = state.panel.as_ref().and_then(|p| match &*p.borrow() {
            Panel::Pick { picker, .. } => picker.borrow().options.hint.clone(),
            _ => None,
        });
        let panel_hint = state.panel.as_ref().map(|p| match &*p.borrow() {
            _ if own_hint.is_some() => own_hint.as_deref().unwrap_or(""),
            Panel::Btw { .. } => "↑↓ to scroll · c copies · esc to close",
            Panel::Job { .. } => "↑↓ to scroll · x stops it · r runs it again · c copies · esc to close",
            Panel::Pick { picker, .. } if picker.borrow().options.answers => {
                "a number, then enter answers · or type your own · esc to close"
            }
            Panel::Pick { .. } => "↑↓ to move · enter to choose · esc to close",
            Panel::Key { .. } => "enter to save · esc to cancel",
            Panel::ChatGpt { by_code: true, .. } => "c copies the code · b uses the browser · esc to cancel",
            Panel::ChatGpt { url, .. } => {
                if url.is_some() {
                    "c copies the link · d uses a code · esc to cancel"
                } else {
                    "esc to cancel"
                }
            }
        });
        let hint = panel_hint.unwrap_or_else(|| {
            if !menu.is_empty() {
                "enter to choose · esc to close"
            } else if let Some(voice) = &voice {
                &voice.hint
            } else if busy && !state.editor.is_empty() && !is_command(string::trim(&state.editor.text())) {
                if state.current.is_some() || state.pending_turn {
                    "enter sends now · tab after · esc stops"
                } else {
                    "enter sends when ready"
                }
            } else if busy {
                "esc to stop"
            } else if self.0.voice.is_some() && state.editor.is_empty() {
                "ctrl+t to talk"
            } else {
                "enter to send"
            }
        });
        let bottom = box_.y + box_.height - 1;
        let used = voice.as_ref().map(|v| spans(screen, x, bottom, &v.status, Some(box_))).unwrap_or(x);
        let at = box_.x + box_.width - 2 - text_width(hint);
        if text_width(hint) + 2 < width && at > used + 1 {
            screen.put(at, bottom, hint, &st::FAINT);
        }
        Cursor { x: x + layout.cursor_column, y: box_.y + 1 + layout.cursor_row as i32 - first }
    }
    fn draw_menu(&self, screen: &mut Screen, box_top: i32, left: i32) {
        let items = self.menu();
        if items.is_empty() {
            return;
        }
        let width = 70.min(left - 2);
        let top = box_top - items.len() as i32 - 1;
        if top < 2 {
            return;
        }
        let about = 3 + super::input::COMMANDS.iter().map(|c| c.name.len()).max().unwrap_or(0) as i32 + 2;
        let area = Rect::new(1, top - 1, width, items.len() as i32 + 1);
        screen.fill(area, &st::RAISED);
        self.cover(area);
        let selected = self.0.state.borrow().menu_index;
        for (index, item) in items.iter().enumerate() {
            let y = top + index as i32;
            let chosen = index == selected;
            if chosen {
                screen.fill(Rect::new(1, y, width, 1), &st::SELECTED);
            }
            screen.put(3, y, item.name, &if chosen { st::ACCENT } else { st::TEXT });
            if chosen {
                screen.put(about, y, &truncate(&item.about(), (width - about - 1).max(1)), &st::BRIGHT);
            }
        }
    }
}
pub(super) fn num(v: &Value) -> String {
    number::to_string(v.as_f64().unwrap_or(0.))
}
