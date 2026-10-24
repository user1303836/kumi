//! Background jobs in the app: the JOBS tab, the header's count, the card when one ends, its overlay, /jobs and ctrl-b.
use super::super::alerts::{self, ProgramState, StatusReport};
use super::super::{
    activity::{activity_glyph, activity_of},
    markdown::render_markdown,
    transcript::doing_label,
    width::truncate,
    wrap::Span,
};
use super::drawing::{sp, st};
use super::panels::{inert, item, noted};
use super::*;
use crate::tui::picker::{NoteTone, PickerOptions};
use base64::Engine;
use kumi_agent::core::jobs::{JobDetail, JobState, JobStatus, JobStepState};

/// The JOBS tab: on the strip from the first job.
pub(super) struct JobsTab {
    pub app: Weak<Inner>,
}
impl Tab for JobsTab {
    fn id(&self) -> &str {
        "jobs"
    }
    fn title(&self) -> &str {
        "JOBS"
    }
    fn badge(&self) -> Option<i64> {
        self.app.upgrade().map(|a| TuiApp(a).running_jobs() as i64).filter(|n| *n > 0)
    }
    fn rows(&self, width: i32) -> Rc<[TabRow]> {
        self.app.upgrade().map(|a| TuiApp(a).job_rows(width)).unwrap_or_default()
    }
    fn empty(&self) -> Option<&str> {
        Some("No jobs yet")
    }
    fn shown(&self) -> bool {
        self.app.upgrade().is_some_and(|a| !a.state.borrow().jobs.is_empty())
    }
}

/// A job's time so far, or all of it.
fn job_ms(status: &JobStatus) -> f64 {
    (status.ended_at.unwrap_or_else(|| now_ms_f64() as i64) - status.started_at).max(0) as f64
}

/// "12.4k tokens".
fn tokens_words(tokens: u64) -> String {
    if tokens >= 1000 {
        format!("{}k tokens", number::to_fixed(tokens as f64 / 1000., 1))
    } else {
        format!("{tokens} tokens")
    }
}

/// "5 steps, 3 sources".
fn steps_words(status: &JobStatus) -> String {
    let mut words = vec![format!("{} {}", status.steps, if status.steps == 1 { "step" } else { "steps" })];
    if status.sources > 0 {
        words.push(format!("{} {}", status.sources, if status.sources == 1 { "source" } else { "sources" }));
    }
    words.join(", ")
}

/// What a job is doing now, in words: what its tool said, or what its tool does.
fn doing_words(status: &JobStatus) -> String {
    status.doing.clone().unwrap_or_else(|| match status.tool.as_deref() {
        Some(tool) => doing_label(Some(tool), "working"),
        None => "starting".into(),
    })
}

impl TuiApp {
    pub(super) fn running_jobs(&self) -> usize {
        self.0.state.borrow().jobs.values().filter(|status| status.state == JobState::Running).count()
    }

    /// A job's status came: its row, and a card when it ends.
    pub(super) fn job_event(&self, status: &JobStatus) {
        let ended = {
            let mut state = self.0.state.borrow_mut();
            let before = state.jobs.insert(status.id, status.clone());
            status.state.ended() && before.is_none_or(|before| !before.state.ended())
        };
        self.job_status(status);
        if ended {
            self.job_ended(status);
            if status.when_done == kumi_agent::core::jobs::WhenDone::Continue && status.state != JobState::Interrupted {
                self.0.state.borrow_mut().carry.push(status.id);
            }
        }
        // Rows and the header's count tick while jobs run.
        let busy = self.busy();
        self.0.scheduler.set_animating(busy || self.running_jobs() > 0 || self.0.voice.as_ref().is_some_and(VoiceInput::active));
    }

    /// A job's record in the terminal's status (OSC 7501 `id=job-N`, beside Kumi's own): working with what it's doing,
    /// done or error with its line; one stopped or interrupted is cleared. Written only when it changes.
    fn job_status(&self, status: &JobStatus) {
        let profile = &self.0.profile;
        if !profile.status {
            return;
        }
        let id = format!("job-{}", status.id);
        // One that ended while the producer looks on is seen in its card: its record goes, as once they're back.
        let seen = status.state.ended() && self.known_in_front();
        let (state, msg) = match status.state {
            JobState::Running => (Some(ProgramState::Working), doing_words(status)),
            JobState::Done if !seen => (Some(ProgramState::Done), status.line.clone().unwrap_or_default()),
            JobState::Failed if !seen => (Some(ProgramState::Error), status.line.clone().unwrap_or_default()),
            _ => (None, String::new()),
        };
        let sequence = match state {
            Some(state) => {
                let report = StatusReport {
                    state: Some(state),
                    id: Some(id),
                    progress: None,
                    title: Some(self.clean_text(&status.title)),
                    msg: Some(self.clean_text(&msg)).filter(|msg| !msg.is_empty()),
                };
                if self.0.state.borrow().job_status_sent.get(&status.id) == Some(&report) {
                    return;
                }
                let sequence = alerts::status_sequence(&report);
                self.0.state.borrow_mut().job_status_sent.insert(status.id, report);
                sequence
            }
            None => {
                if self.0.state.borrow_mut().job_status_sent.remove(&status.id).is_none() {
                    return;
                }
                Some(alerts::clear_sequence(Some(&id)))
            }
        };
        if let Some(sequence) = sequence {
            self.0.tty.write(&if profile.tmux { alerts::tmux_wrap(&sequence) } else { sequence });
        }
    }

    /// The producer is back: the records of jobs that ended (done or failed) go, so a long session doesn't pile them up.
    pub(super) fn clear_finished_jobs(&self) {
        let profile = &self.0.profile;
        let finished: Vec<u32> = {
            let mut state = self.0.state.borrow_mut();
            let finished: Vec<u32> = state
                .job_status_sent
                .iter()
                .filter(|(_, report)| matches!(report.state, Some(ProgramState::Done | ProgramState::Error)))
                .map(|(id, _)| *id)
                .collect();
            for id in &finished {
                state.job_status_sent.remove(id);
            }
            finished
        };
        let out: String = finished
            .iter()
            .map(|id| {
                let sequence = alerts::clear_sequence(Some(&format!("job-{id}")));
                if profile.tmux {
                    alerts::tmux_wrap(&sequence)
                } else {
                    sequence
                }
            })
            .collect();
        if !out.is_empty() {
            self.0.tty.write(&out);
        }
    }

    /// A job ended: its card goes into the conversation, above the answer under way if there is one, and the input box is
    /// left as it is. A producer who's away hears of one that told its result (a job the request needed is told by the
    /// answer that carries on with it).
    pub(super) fn job_ended(&self, status: &JobStatus) {
        let title = self.clean_text(&status.title);
        match status.state {
            JobState::Done if status.when_done == kumi_agent::core::jobs::WhenDone::Tell => {
                self.alert(&format!("Job {} done: {title}", status.id), &format!("Kumi · job done · {title}"))
            }
            JobState::Failed => self.alert(&format!("Job {} failed: {title}", status.id), &format!("Kumi · job failed · {title}")),
            _ => {}
        }
        let tokens = status.tokens().map(|t| format!(" · {}", tokens_words(t))).unwrap_or_default();
        let (done, head, line) = match status.state {
            JobState::Done => (
                true,
                format!("Job {} · {} · {}{tokens}", status.id, self.clean_text(&status.title), steps_words(status)),
                status.line.clone(),
            ),
            JobState::Interrupted => return,
            JobState::Stopped => (false, format!("Job {} · {} · stopped", status.id, self.clean_text(&status.title)), None),
            _ => (false, format!("Job {} · {} · failed", status.id, self.clean_text(&status.title)), status.line.clone()),
        };
        let line = line.map(|line| self.clean_text(&line));
        let running = self.0.state.borrow().current.clone();
        let mut state = self.0.state.borrow_mut();
        let entry = Entry::Job { done, head, line };
        match running {
            Some(current) => {
                state.transcript.insert_before(entry, Some(&current));
            }
            None => {
                state.transcript.add(entry);
            }
        }
        drop(state);
        self.0.scheduler.request();
    }

    /// A job whose request carries on starts its answer only when the producer isn't in the middle of something: no
    /// answer under way, no pick or panel open, nothing typed or waiting to send, and not in the map.
    pub(super) fn maybe_carry_on(&self) {
        let id = {
            let state = self.0.state.borrow();
            let quiet = !state.closing
                && !state.pending_turn
                && !state.cancelling
                && state.panel.is_none()
                && state.editor.is_empty()
                && state.held.is_empty()
                && state.setup.is_none()
                && state.map.is_none();
            match state.carry.first() {
                Some(id) if quiet && !self.busy() => *id,
                _ => return,
            }
        };
        self.0.state.borrow_mut().carry.retain(|waiting| *waiting != id);
        self.task(move |app| async move {
            if !app.0.options.controller.carry_on(id) && app.0.options.controller.jobs().iter().any(|s| s.id == id) {
                // Kumi was answering after all: it waits for the next quiet moment.
                app.0.state.borrow_mut().carry.insert(0, id);
            }
            Ok(())
        });
    }

    /// Kumi carries on by itself with a job's result: the answer that follows is Kumi's, not the producer's.
    pub(super) fn carry_on_event(&self, job: u32, title: &str) {
        let line = format!("Carrying on with job {job}'s result: {}", self.clean_text(title));
        let mut state = self.0.state.borrow_mut();
        state.transcript.add(Entry::Notice { text: line, tone: NoticeTone::Info });
        state.pending_turn = true;
        state.activity = "carrying on".into();
        drop(state);
        self.0.scheduler.request();
    }

    fn clean_text(&self, text: &str) -> String {
        sanitize_text(text, &self.0.state.borrow().secrets).replace('\n', " ")
    }

    /// The JOBS tab's rows: running jobs first, newest first, then the ones that ended.
    pub(super) fn job_rows(&self, width: i32) -> Rc<[TabRow]> {
        let jobs: Vec<JobStatus> = {
            let state = self.0.state.borrow();
            let mut jobs: Vec<JobStatus> = state.jobs.values().cloned().collect();
            jobs.sort_by_key(|status| (status.state.ended(), std::cmp::Reverse(status.id)));
            jobs
        };
        let now = perf_now();
        let plain = self.0.icons == IconStyle::Badges;
        let mut rows = vec![];
        for status in jobs {
            let id = status.id;
            let weak = Rc::downgrade(&self.0);
            let open: Rc<dyn Fn()> = Rc::new(move || {
                if let Some(a) = weak.upgrade() {
                    TuiApp(a).open_job(id);
                }
            });
            let stop: Rc<dyn Fn()> = {
                let weak = Rc::downgrade(&self.0);
                Rc::new(move || {
                    if let Some(a) = weak.upgrade() {
                        TuiApp(a).stop_job(id);
                    }
                })
            };
            let title = truncate(&self.clean_text(&status.title), (width - 8).max(4));
            let item = Some(format!("job-{id}"));
            match status.state {
                JobState::Running => {
                    let glyph = activity_glyph(activity_of(status.tool.as_deref()), now, plain);
                    rows.push(TabRow {
                        spans: vec![glyph, sp(" ", st::DIM), sp(title, st::TEXT)],
                        right: Some(sp("stop", st::ACCENT)),
                        action: Some(stop.clone()),
                        open: Some(open),
                        keys: vec![("x", stop)],
                        item: item.clone(),
                    });
                    let mut detail = format!("{} · {}", doing_words(&status), helpers::clock_of(job_ms(&status)));
                    if let Some(tokens) = status.tokens() {
                        detail.push_str(&format!(" · {}", tokens_words(tokens)));
                    }
                    rows.push(TabRow {
                        spans: vec![sp("  ", st::DIM), sp(self.clean_text(&detail), st::FAINT)],
                        item,
                        ..Default::default()
                    });
                }
                state => {
                    let (glyph, style) = match state {
                        JobState::Done => ("✓ ", st::ACCENT),
                        JobState::Stopped | JobState::Interrupted => ("■ ", st::FAINT),
                        _ => ("✗ ", st::WARN),
                    };
                    let mut right = helpers::clock_of(job_ms(&status));
                    if let Some(tokens) = status.tokens() {
                        right.push_str(&format!(" · {}", tokens_words(tokens)));
                    }
                    rows.push(TabRow {
                        spans: vec![sp(glyph, style), sp(title, st::DIM)],
                        right: Some(sp(right, st::FAINT)),
                        open: Some(open),
                        item: item.clone(),
                        ..Default::default()
                    });
                    let line = match state {
                        JobState::Stopped => "stopped".to_string(),
                        _ => status.line.clone().unwrap_or_default(),
                    };
                    if !line.is_empty() {
                        rows.push(TabRow {
                            spans: vec![sp("  ", st::DIM), sp(self.clean_text(&line), st::FAINT)],
                            item,
                            ..Default::default()
                        });
                    }
                }
            }
        }
        rows.into()
    }

    pub(super) fn open_job(&self, id: u32) {
        self.0.state.borrow_mut().panel = Some(Rc::new(RefCell::new(Panel::Job { id, scroll: 0 })));
        self.0.scheduler.request();
    }

    pub(super) fn stop_job(&self, id: u32) {
        if self.0.options.controller.stop_job(id) {
            self.notice(&format!("Stopping job {id}."), NoticeTone::Info);
        }
        self.0.scheduler.request();
    }

    fn run_job_again(&self, id: u32) {
        match self.0.options.controller.run_job_again(id) {
            Ok(again) => {
                self.0.state.borrow_mut().jobs.remove(&id);
                self.notice(&format!("Running it again as job {again}."), NoticeTone::Info);
            }
            Err(why) => self.notice(&why, NoticeTone::Warn),
        }
        self.0.scheduler.request();
    }

    /// /jobs: each job; Enter opens one, where x stops it and r runs it again.
    pub(super) fn open_jobs(&self) {
        let jobs = self.0.options.controller.jobs();
        let items = if jobs.is_empty() {
            vec![inert(&format!(
                "None yet: Kumi starts jobs for slow work that doesn't change {}, such as research or a tutorial",
                crate::daw::words().app
            ))]
        } else {
            let mut jobs = jobs;
            jobs.sort_by_key(|status| (status.state.ended(), std::cmp::Reverse(status.id)));
            jobs.iter()
                .map(|status| {
                    let what = match status.state {
                        JobState::Running => doing_words(status),
                        _ => status.line.clone().unwrap_or_default(),
                    };
                    let (note, tone) = match status.state {
                        JobState::Running => (format!("running {}", helpers::clock_of(job_ms(status))), NoteTone::Accent),
                        JobState::Done => ("done".into(), NoteTone::Faint),
                        JobState::Failed => ("failed".into(), NoteTone::Warn),
                        JobState::Stopped => ("stopped".into(), NoteTone::Faint),
                        JobState::Interrupted => ("interrupted".into(), NoteTone::Warn),
                    };
                    noted(
                        item(
                            format!("Job {} · {}", status.id, self.clean_text(&status.title)),
                            status.id.to_string(),
                            self.clean_text(&what),
                        ),
                        note,
                        tone,
                    )
                })
                .collect()
        };
        self.pick(
            Picker::with_options("Background jobs · enter opens one: x stops it there, r runs it again", items, PickerOptions::default()),
            move |app, item| async move {
                app.close_panel();
                if let Some(id) = item.value.as_deref().and_then(|v| v.parse::<u32>().ok()) {
                    app.open_job(id);
                }
                Ok(())
            },
        );
    }

    /// Ctrl-b: what the answer is doing away from Live goes on in the background, and the answer carries on.
    pub(super) fn background_now(&self) {
        match self.0.options.controller.background() {
            Ok(id) => self.notice(
                &format!("Moved to the background as job {id}: the answer carries on, and its report comes when it's done."),
                NoticeTone::Info,
            ),
            Err(why) => self.notice(&why, NoticeTone::Info),
        }
        self.0.scheduler.request();
    }

    /// The overlay's lines for a job: its head, what it was asked, its steps, then its report.
    pub(super) fn job_lines(&self, detail: &JobDetail, width: i32) -> Vec<(Vec<Span>, Option<Span>)> {
        let status = &detail.status;
        let mut lines: Vec<(Vec<Span>, Option<Span>)> = vec![];
        let state = match status.state {
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Stopped => "stopped",
            JobState::Interrupted => "interrupted",
        };
        let mut right = format!("{state} · {}", helpers::clock_of(job_ms(status)));
        if let Some(tokens) = status.tokens() {
            right.push_str(&format!(" · {}", tokens_words(tokens)));
        }
        lines.push((vec![sp(format!("Job {} · {}", status.id, self.clean_text(&status.title)), st::TITLE)], Some(sp(right, st::FAINT))));
        for row in super::super::wrap::wrap(&[sp(format!("Asked: {}", self.clean_text(&detail.asked)), st::DIM)], width.max(8)) {
            lines.push((row, None));
        }
        let now = perf_now();
        let plain = self.0.icons == IconStyle::Badges;
        for step in &detail.steps {
            let glyph = match step.state {
                JobStepState::Running => activity_glyph(activity_of(step.tool.as_deref()), now, plain),
                JobStepState::Done => sp("✓", st::ACCENT),
                JobStepState::Failed => sp("✗", st::WARN),
            };
            let words = if step.text.is_empty() {
                match step.state {
                    JobStepState::Running => doing_label(step.tool.as_deref(), "working"),
                    _ => step.tool.as_deref().map(super::super::transcript::step_label).unwrap_or_default(),
                }
            } else {
                step.text.clone()
            };
            lines.push((vec![glyph, sp(" ", st::DIM), sp(truncate(&self.clean_text(&words), (width - 2).max(4)), st::DIM)], None));
        }
        if let Some(report) = detail.report.as_ref().filter(|r| !r.is_empty()) {
            lines.push((vec![], None));
            for row in render_markdown(&sanitize_text(report, &self.0.state.borrow().secrets), width.max(8), &Rc::new(st::TEXT)) {
                lines.push((row.spans, None));
            }
        } else if status.state.ended() {
            if let Some(line) = status.line.as_ref() {
                lines.push((vec![], None));
                lines.push((vec![sp(self.clean_text(line), if status.state == JobState::Done { st::TEXT } else { st::WARN })], None));
            }
        }
        lines
    }

    /// Keys on a job's overlay: scroll, x stops it, r runs it again, c copies its report, Esc closes.
    pub(super) fn job_panel_input(&self, panel: &PanelRef, event: InputEvent) {
        let Panel::Job { id, .. } = &*panel.borrow() else { return };
        let id = *id;
        match event {
            InputEvent::Key { name, mods, .. } => {
                if matches!(name.as_str(), "escape" | "enter") || (mods.ctrl && name == "c") {
                    self.close_panel();
                    return;
                }
                if let Panel::Job { scroll, .. } = &mut *panel.borrow_mut() {
                    match name.as_str() {
                        "up" | "pageup" => *scroll = (*scroll - if name == "up" { 1 } else { 5 }).max(0),
                        "down" | "pagedown" => *scroll += if name == "down" { 1 } else { 5 },
                        _ => {}
                    }
                }
            }
            InputEvent::Text { text } => match text.to_lowercase().as_str() {
                " " => self.close_panel(),
                "x" => self.stop_job(id),
                "r" if self.0.options.controller.jobs().iter().any(|s| s.id == id && s.state.ended()) => {
                    self.close_panel();
                    self.run_job_again(id);
                }
                "c" => {
                    let report = self.0.options.controller.job(id).and_then(|detail| detail.report).unwrap_or_default();
                    if !report.is_empty() {
                        self.0.tty.write(&format!("\x1b]52;c;{}\x07", base64::engine::general_purpose::STANDARD.encode(report)));
                        self.notice(&format!("Copied job {id}'s report."), NoticeTone::Info);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        self.0.scheduler.request();
    }
}
