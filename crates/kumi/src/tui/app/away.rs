//! For a producer who isn't looking at Kumi's window: Kumi's status in the terminal (OSC 7501, 9;4
//! progress) all the time, and, while the window is behind another, a desktop notification and a line
//! in the DAW's own window (Live's status bar) when Kumi finishes or needs them.

use super::super::alerts::{self, Blocked, NotifyChannel, ProgramState, StatusReport, TerminalProfile};
use super::*;
use kumi_ai::failure::reset_words;

/// What the producer chose about being told while away (settings.json).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AwaySettings {
    /// A desktop notification (or the bell) while Kumi's window is behind another: `"notifications"`.
    pub notifications: bool,
    /// A line in the DAW's window for the same (Live's status bar): `"liveStatusBar"`.
    pub live_status_bar: bool,
    /// Seconds before Kumi takes the answer it recommended when nobody's there to pick; 0 never: `"answerAfter"`.
    pub answer_after: u32,
}

impl Default for AwaySettings {
    fn default() -> Self {
        Self { notifications: true, live_status_bar: true, answer_after: 60 }
    }
}

/// Runs a program that shows a desktop notification (osascript, notify-send), with its arguments.
pub type NotifierRunner = Rc<dyn Fn(&'static str, Vec<String>)>;
/// Whether Kumi's terminal app is the one in front, asked of the system; None when it can't say.
pub type FrontApp = Rc<dyn Fn() -> LocalBoxFuture<'static, Option<bool>>>;

/// Something worth telling a producer who isn't looking.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Alert {
    /// The notification's words: "Done: the bass is darker".
    pub body: String,
    /// The DAW's line (Live's status bar): "Kumi · done · the bass is darker".
    pub live: String,
}

/// At most one alert in this long; the newest of those that came meanwhile goes when it's over.
const ALERT_GAP_MS: f64 = 10_000.;
/// While the DAW records, a held notification is tried again this often.
const RECORDING_RECHECK_MS: u64 = 15_000;
/// The status's words or progress change at most this often (a change of state goes at once).
const STATUS_GAP_MS: f64 = 1_000.;
/// An answer this long or longer is worth a notification when it's done.
pub(super) const LONG_ANSWER_MS: f64 = 15_000.;

impl TuiApp {
    /// The terminal said Kumi's window came to the front, or went behind another.
    pub(super) fn window_focus(&self, focused: bool) {
        {
            let mut state = self.0.state.borrow_mut();
            state.focus_reported = true;
            if state.window_focused == focused {
                return;
            }
            state.window_focused = focused;
            state.unfocused_since = (!focused).then(perf_now);
            if focused {
                // Back: nothing waits to be told, and the answer that ended has been seen.
                state.alert_waiting = None;
                if let Some(timer) = state.alert_timer.take() {
                    timer.abort();
                }
                if matches!(state.resting, Some((ProgramState::Done | ProgramState::Error, _))) {
                    state.resting = Some((ProgramState::Idle, String::new()));
                }
            }
        }
        if focused {
            // So have the jobs that ended: their records go, as the root's goes back to idle.
            self.clear_finished_jobs();
        }
        self.0.scheduler.request();
    }

    /// How the status rests once an answer ends: done or error with its words, or idle.
    pub(super) fn rest(&self, state: ProgramState, words: &str) {
        self.0.state.borrow_mut().resting = Some((state, alerts::one_line(words, 400)));
    }

    /// What the terminal's status says now: working with NOW's words while Kumi answers, blocked while a pick or
    /// a sign-in waits for the producer, otherwise how the last answer ended.
    pub(super) fn program_status(&self) -> StatusReport {
        let title = self.0.state.borrow().set_name.clone();
        let report = |state: ProgramState, msg: String, progress: Option<u8>| StatusReport {
            state: Some(state),
            id: None,
            progress,
            title: title.clone(),
            msg: Some(msg).filter(|msg| !msg.is_empty()),
        };
        if self.busy() {
            return report(ProgramState::Working, self.working_words(), self.progress());
        }
        if self.answers_open() {
            return report(ProgramState::Blocked(Blocked::Question), "Your answer, in Kumi".into(), None);
        }
        if self.signing_in() {
            return report(ProgramState::Blocked(Blocked::Auth), "Signing in, in Kumi".into(), None);
        }
        let state = self.0.state.borrow();
        match &state.resting {
            Some((rest, words)) => report(*rest, words.clone(), None),
            None => report(ProgramState::Idle, String::new(), None),
        }
    }

    /// NOW's words without what ticks (a run's clock, a countdown's seconds), so the status changes only when what
    /// Kumi does does.
    fn working_words(&self) -> String {
        static CLOCK: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new(r" · \d+:\d{2}(:\d{2})?$").expect("regex"));
        let now = self.now_line();
        // A think's time and its streaming headline would tick it: it says what NOW said before them.
        let thinking = now.label.starts_with(super::usage::THINKING);
        let label = if thinking {
            super::usage::working_label(self.0.state.borrow().turn_changes)
        } else {
            CLOCK.replace(&now.label, "").into_owned()
        };
        let retry = {
            let state = self.0.state.borrow();
            state.retry.as_ref().map(|(reason, _)| match state.waiting_for {
                Some((WaitFor::Limit, at)) => format!("{reason} · carrying on at {}", reset_words(at, now_ms_f64())),
                Some((WaitFor::Offline, _)) if state.waiting_cause.is_some() => reason.clone(),
                Some((WaitFor::Offline, _)) => format!("offline · {reason}"),
                None => reason.clone(),
            })
        };
        let detail = retry.unwrap_or(if thinking { "thinking".into() } else { now.detail });
        [label, detail].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" · ")
    }

    /// How far a run has got, 0–100, when Kumi knows: a /goal's turns against its budget, or a /loop's rounds against
    /// its budget's, which are the rounds it has judged and those it has left.
    fn progress(&self) -> Option<u8> {
        let state = self.0.state.borrow();
        let share = |done: f64, of: f64| (of > 0.).then(|| ((done / of) * 100.).clamp(0., 100.).round() as u8);
        if let Some(goal) = state.objective.as_ref().filter(|goal| goal.status.state == ObjectiveState::Running) {
            return share(goal.status.turns.into(), goal.status.turn_budget.into());
        }
        let looping = &state.looping.as_ref()?.status;
        share(looping.rounds.into(), f64::from(looping.rounds) + f64::from(looping.rounds_left))
    }

    /// Writes the status (after a frame) when it changed: a new state at once, new words or progress at most once a
    /// second.
    pub(super) fn show_status(&self) {
        let profile = &self.0.profile;
        if !profile.status && !profile.progress {
            return;
        }
        let report = self.program_status();
        let now = perf_now();
        let progress_before = {
            let mut state = self.0.state.borrow_mut();
            match &state.status_sent {
                Some((sent, _)) if *sent == report => return,
                Some((sent, at)) if sent.state == report.state && now - at < STATUS_GAP_MS => {
                    if state.status_timer.is_none() {
                        let weak = Rc::downgrade(&self.0);
                        let wait = STATUS_GAP_MS - (now - at);
                        state.status_timer = Some(tokio::task::spawn_local(async move {
                            tokio::time::sleep(Duration::from_secs_f64(wait.max(0.) / 1000.)).await;
                            if let Some(a) = weak.upgrade() {
                                a.state.borrow_mut().status_timer = None;
                                TuiApp(a).show_status();
                            }
                        }));
                    }
                    return;
                }
                sent => sent.as_ref().map(|(sent, _)| (sent.state, sent.progress)),
            }
        };
        let mut out = String::new();
        let mut put = |sequence: String| out.push_str(&if profile.tmux { alerts::tmux_wrap(&sequence) } else { sequence });
        if profile.status {
            if let Some(sequence) = alerts::status_sequence(&report) {
                put(sequence);
            }
        }
        if profile.progress && progress_before != Some((report.state, report.progress)) {
            put(alerts::progress_sequence(report.state.unwrap_or(ProgramState::Idle), report.progress));
        }
        self.0.tty.write(&out);
        self.0.state.borrow_mut().status_sent = Some((report, now));
    }

    /// Whether Kumi's window is known to be in front: the terminal said so last (and has said anything at all).
    pub(super) fn known_in_front(&self) -> bool {
        let state = self.0.state.borrow();
        state.focus_reported && state.window_focused
    }

    /// Whether the producer is looking elsewhere now. A terminal that has said nothing of focus may never say (tmux
    /// without `focus-events on`): then the system says whether its app is in front, asked only when there's
    /// something to tell.
    async fn looking_away(&self) -> bool {
        let (reported, focused) = {
            let state = self.0.state.borrow();
            (state.focus_reported, state.window_focused)
        };
        if reported {
            return !focused;
        }
        let ask = self.0.options.front_app.clone().or_else(|| {
            self.0.profile.app_id.clone().map(|id| {
                Rc::new(move || {
                    let id = id.clone();
                    async move { alerts::app_in_front(&id).await }.boxed_local()
                }) as FrontApp
            })
        });
        match ask {
            Some(ask) => ask().await == Some(false),
            None => false,
        }
    }

    /// Tells a producer who isn't looking: a desktop notification and a line in the DAW's window. At most one in
    /// ~10 s (the newest of those that came meanwhile goes when it's over); the notification waits while the DAW
    /// records. Nothing while Kumi's window is in front.
    pub(super) fn alert(&self, body: &str, live: &str) {
        // (nothing of a conversation shown again from its log is news)
        if self.0.state.borrow().replaying {
            return;
        }
        let settings = &self.0.options.away;
        let notifying = settings.notifications && self.0.profile.notify != NotifyChannel::None;
        if !notifying && !settings.live_status_bar {
            return;
        }
        let alert = Alert { body: alerts::one_line(body, 200), live: alerts::one_line(live, 120) };
        if self.known_in_front() {
            return;
        }
        {
            let mut state = self.0.state.borrow_mut();
            if state.closing {
                return;
            }
            let gap = self.0.options.alert_gap_ms.unwrap_or(ALERT_GAP_MS);
            let since = perf_now() - state.alert_sent_at;
            if since < gap {
                state.alert_waiting = Some(alert);
                if state.alert_timer.is_none() {
                    let wait = gap - since;
                    state.alert_timer = Some(self.alert_later(Duration::from_secs_f64(wait / 1000.)));
                }
                return;
            }
            state.alert_sent_at = perf_now();
        }
        self.send_alert(alert);
    }

    /// Sends the alert that waited, after `wait`, if the producer is still away.
    fn alert_later(&self, wait: Duration) -> JoinHandle<()> {
        let weak = Rc::downgrade(&self.0);
        tokio::task::spawn_local(async move {
            tokio::time::sleep(wait).await;
            let Some(a) = weak.upgrade() else { return };
            let waiting = {
                let mut state = a.state.borrow_mut();
                state.alert_timer = None;
                let in_front = state.focus_reported && state.window_focused;
                let waiting = state.alert_waiting.take().filter(|_| !in_front && !state.closing);
                if waiting.is_some() {
                    state.alert_sent_at = perf_now();
                }
                waiting
            };
            if let Some(alert) = waiting {
                TuiApp(a).send_alert(alert);
            }
        })
    }

    fn send_alert(&self, alert: Alert) {
        let settings = self.0.options.away.clone();
        let channel = self.0.profile.notify;
        self.task(move |app| async move {
            if !app.looking_away().await {
                return Ok(());
            }
            // The DAW's line goes beside the notification: a busy DAW doesn't hold the notification up.
            if settings.live_status_bar && !alert.live.is_empty() && app.can().daw_message {
                let (app, text) = (app.clone(), alert.live.clone());
                tokio::task::spawn_local(async move {
                    let _ = app.session().op(op::DawMessage { text }).await;
                });
            }
            if !settings.notifications || channel == NotifyChannel::None {
                return Ok(());
            }
            // No notification, and no bell, over a take being recorded.
            if app.session().op(op::DawRecording {}).await.ok().flatten() == Some(true) {
                let mut state = app.0.state.borrow_mut();
                let in_front = state.focus_reported && state.window_focused;
                if !in_front && !state.closing && state.alert_timer.is_none() {
                    state.alert_waiting = Some(Alert { live: String::new(), ..alert });
                    let recheck = app.0.options.alert_gap_ms.map_or(RECORDING_RECHECK_MS, |gap| gap as u64);
                    state.alert_timer = Some(app.alert_later(Duration::from_millis(recheck)));
                }
                return Ok(());
            }
            if app.known_in_front() {
                return Ok(());
            }
            app.notify(&alert.body);
            Ok(())
        });
    }

    /// A desktop notification by the terminal's own sequence, or by the program that shows one.
    fn notify(&self, body: &str) {
        let profile = &self.0.profile;
        if let Some(sequence) = alerts::notify_sequence(profile.notify, "Kumi", body) {
            let wrapped = if profile.tmux { alerts::tmux_wrap(&sequence) } else { sequence };
            self.0.tty.write(&wrapped);
        } else if let Some((program, args)) = alerts::notify_command(profile.notify, "Kumi", body) {
            match &self.0.options.run_notifier {
                Some(run) => run(program, args),
                None => {
                    tokio::task::spawn_local(async move {
                        let args: Vec<&str> = args.iter().map(String::as_str).collect();
                        let _ = super::super::attach::bounded(program, &args).await;
                    });
                }
            }
        }
    }

    /// An answer that ended while the producer was away: done, a pick waiting, or one that stopped.
    pub(super) fn answer_ended(&self, reason: StopReason, elapsed_ms: f64, answer: &str, offered: Option<usize>) {
        // An answer shown again from a log ended long ago: the terminal's status isn't about it.
        if self.0.state.borrow().replaying {
            return;
        }
        let summary = summary_of(answer);
        match reason {
            StopReason::Completed => {
                self.rest(ProgramState::Done, &summary);
                let in_run = self.0.state.borrow().run_told;
                if let Some(count) = offered {
                    self.alert(&format!("Needs you: pick 1–{count}"), &format!("Kumi needs you · pick 1–{count} in Kumi"));
                } else if elapsed_ms >= LONG_ANSWER_MS && !in_run {
                    let words = if summary.is_empty() { "finished".to_string() } else { summary };
                    self.alert(&format!("Done: {words}"), &format!("Kumi · done · {words}"));
                }
            }
            StopReason::MaxSteps => {
                self.rest(ProgramState::Done, "Reached its step limit for one answer");
                self.alert("Stopped: reached its step limit for one answer", "Kumi · stopped · step limit reached");
            }
            StopReason::Cancelled => self.rest(ProgramState::Idle, ""),
        }
    }

    /// A run's end (a /goal, /loop or sound match) told once, in place of the answer's own "done".
    pub(super) fn run_ended(&self, body: &str, live: &str) {
        if self.0.state.borrow().replaying {
            return;
        }
        self.0.state.borrow_mut().run_told = true;
        self.alert(body, live);
    }
}

/// An answer in a line: its first line of words, without Markdown's marks, at most 80 characters.
pub(super) fn summary_of(answer: &str) -> String {
    let line = answer
        .lines()
        .map(|line| line.trim().trim_start_matches(['#', '>', '*', '-', ' ']).replace("**", "").replace('`', ""))
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    alerts::one_line(&line, 80)
}

/// The profile Kumi uses where none was given: this process's terminal.
pub(super) fn detect_profile_here() -> TerminalProfile {
    alerts::detect_terminal_profile(&kumi_common::env::vars(), kumi_types::system::platform())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_summed_up_by_its_first_line_of_words() {
        assert_eq!(summary_of("\n## Made the bass darker\n\nDetails…"), "Made the bass darker");
        assert_eq!(summary_of("**Done.** I lowered the cutoff"), "Done. I lowered the cutoff");
        assert_eq!(summary_of(""), "");
        assert!(summary_of(&"word ".repeat(40)).chars().count() <= 80);
    }
}
