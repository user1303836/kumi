use super::super::{
    choices::choices,
    icons::IconKind,
    transcript::{self, step_label, Step, WebLine},
};
use super::*;
use crate::text::web_words;
use kumi_agent::core::session_log::{LogLine, Replay};
use kumi_ai::failure::reset_words;
use kumi_common::time::since;
use kumi_types::errors::FailureKind;

impl TuiApp {
    pub(super) fn event(&self, event: SessionEvent) {
        if self.0.state.borrow().closing {
            return;
        }
        // Anything from the model, or the turn's end, means the wait is over; once it's handled, the model at work takes
        // in what the producer asked to say now. Every kernel event is placed here, so a new one has to be.
        let (ends_wait, at_work) = match &event {
            SessionEvent::Kernel(kernel) => match kernel {
                KernelEvent::Text { .. }
                | KernelEvent::Reasoning { .. }
                | KernelEvent::ToolInput { .. }
                | KernelEvent::ToolStart { .. } => (true, true),
                KernelEvent::ToolEnd { .. } => (false, true),
                KernelEvent::Steer { .. } | KernelEvent::Retry { .. } | KernelEvent::ModelSwitch { .. } | KernelEvent::Usage(_) => {
                    (false, false)
                }
            },
            SessionEvent::TurnComplete { .. } | SessionEvent::State { .. } => (true, false),
            _ => (false, false),
        };
        if ends_wait {
            let mut state = self.0.state.borrow_mut();
            state.retry = None;
            state.waiting_for = None;
            state.waiting_cause = None;
            state.waiting_proxy_down = false;
        }
        // A turn's state or a job's may be when a job's request carries on, once it's handled.
        let carry_on = matches!(event, SessionEvent::State { .. } | SessionEvent::Job(_));
        match event {
            SessionEvent::Kernel(KernelEvent::Retry { reason, wait_ms, wait, cause, proxy_down }) => {
                let reason = self.clean_line(&reason, 80);
                // No wait: Kumi carries on at once, and the model's next words end it.
                let until = if wait_ms > 0 { perf_now() + wait_ms as f64 } else { f64::INFINITY };
                let at = now_ms_f64() + wait_ms as f64;
                {
                    let mut state = self.0.state.borrow_mut();
                    // The call tried again thinks from the start: its headline comes from what it says.
                    state.reasoning.clear();
                    state.retry = Some((reason.clone(), until));
                    state.waiting_for = wait.map(|wait| (wait, at));
                    state.waiting_cause = cause;
                    state.waiting_proxy_down = proxy_down;
                }
                // A plan's limit pauses the answer until it resets: told once, as a wait, not a stop.
                if wait == Some(WaitFor::Limit) {
                    let clock = reset_words(at, now_ms_f64());
                    self.alert(
                        &format!("Waiting: {reason} · carrying on at {clock}"),
                        &format!("Kumi · {reason} · carrying on at {clock}"),
                    );
                }
            }
            SessionEvent::Kernel(KernelEvent::ModelSwitch { model, reason, back }) => {
                let name = model.split_once('/').map_or(model.as_str(), |(_, name)| name);
                let reason = self.clean_line(&reason, 120);
                // The header asks the session which model answers; the conversation says when that changes.
                if back {
                    self.notice(&format!("{reason}: back to {name}."), NoticeTone::Info);
                } else {
                    self.notice(&format!("{reason}; carrying on with {name} (/model to change)."), NoticeTone::Info);
                }
            }
            SessionEvent::State { state: turn } => {
                {
                    let mut state = self.0.state.borrow_mut();
                    match turn {
                        TurnState::Running => {
                            if state.pending_turn {
                                state.pending_turn = false;
                                state.last_stop = None;
                                state.suppress = false;
                                state.failed = false;
                                state.bytes = 0;
                                state.stream.discard();
                                state.current = Some(state.transcript.add(assistant(Some(perf_now()), AnswerStatus::Running)));
                                state.view.follow();
                                state.planning = None;
                                state.turn_changes = 0;
                                state.reasoning.clear();
                                // (read from the provider's last answer: no state of the app's)
                                state.plan_before = self.answering_plan();
                            }
                        }
                        TurnState::Cancelling => {
                            state.suppress = true;
                            state.stream.discard();
                        }
                        TurnState::Idle if state.pending_turn => state.pending_turn = false,
                        TurnState::Idle | TurnState::Closed => {
                            if let Some(entry) = state.current.take() {
                                if let Entry::Assistant { status, .. } = &mut *entry.borrow_mut() {
                                    *status = if state.failed { AnswerStatus::Failed } else { AnswerStatus::Stopped };
                                }
                                end_steps(&entry, !state.replaying);
                                state.transcript.touch(&entry);
                            }
                        }
                    }
                }
                if turn == TurnState::Idle {
                    self.after_busy();
                }
                {
                    let mut state = self.0.state.borrow_mut();
                    if matches!(turn, TurnState::Running | TurnState::Cancelling) && state.busy_since == 0. {
                        state.busy_since = perf_now();
                        // At work again: the status says so, not how the last answer ended.
                        state.resting = None;
                        state.run_told = false;
                        state.run_active = false;
                    } else if turn == TurnState::Idle {
                        state.busy_since = 0.;
                    }
                }
                if !self.replaying() {
                    self.0.scheduler.set_animating(
                        matches!(turn, TurnState::Running | TurnState::Cancelling) || self.0.voice.as_ref().is_some_and(VoiceInput::active),
                    );
                }
            }
            SessionEvent::Connection { state: connection } => {
                self.0.state.borrow_mut().connection = connection;
                if connection == ConnectionState::Connected {
                    self.read_tree(true);
                    self.read_clip(true);
                    self.read_strip(true);
                    self.read_own(true);
                } else {
                    let mut state = self.0.state.borrow_mut();
                    state.tree = None;
                    state.tree_revision += 1;
                    state.tree_key = None;
                    state.tree_cursor = None;
                    state.clip = None;
                    state.clip_key = None;
                    state.strip = Strip::default();
                    // (a DAW's own body is read anew once it's back)
                    state.own.key = None;
                    state.own.body = None;
                }
            }
            SessionEvent::Observation { label, name } => self.0.state.borrow_mut().set_name = observed_name(&label, name.as_deref()),
            SessionEvent::Library(LibraryEvent { status: library }) => {
                let tell = {
                    let mut state = self.0.state.borrow_mut();
                    let tell = !state.told_library
                        && matches!(library.state, LibraryState::Learning | LibraryState::Paused)
                        && library.learned_at.unwrap_or(0) == 0;
                    if tell {
                        state.told_library = true;
                    }
                    state.library = Some(library);
                    tell && !state.transcript.is_empty()
                };
                if tell {
                    self.notice("Learning your library in the background…", NoticeTone::Info);
                }
            }
            SessionEvent::Music(MusicEvent { status: music }) => {
                let tell = {
                    let mut state = self.0.state.borrow_mut();
                    // The first time Kumi learns their music: nothing heard yet.
                    let tell = !state.told_music
                        && music.state == kumi_listening::music::learning::MusicState::Learning
                        && music.heard() == 0
                        && music.learned_at.is_none();
                    if tell {
                        state.told_music = true;
                    }
                    state.music = Some(music);
                    tell && !state.transcript.is_empty()
                };
                if tell {
                    self.notice("Learning your music in the background…", NoticeTone::Info);
                }
            }
            SessionEvent::Focus { focus } => {
                {
                    let mut state = self.0.state.borrow_mut();
                    // Read in neutral terms: the DAW's presentation fills them from its own wire form.
                    let focus = focus.map(|focus| filled(crate::daw::view(), &focus));
                    state.touched = touched_next(state.focus.as_ref(), focus.as_ref(), state.touched);
                    state.focus = focus;
                }
                self.read_tree(false);
                self.read_clip(false);
                self.read_strip(false);
                self.read_own(false);
                self.follow_lines();
            }
            SessionEvent::Pointed { pin } => {
                let kind = match pin.node {
                    PinKind::Track => IconKind::MidiTrack,
                    PinKind::Scene => IconKind::Scene,
                    PinKind::Clip | PinKind::Selection | PinKind::ClipSlot => IconKind::MidiClip,
                    PinKind::Chain => IconKind::Chain,
                    PinKind::Device => IconKind::Device,
                    PinKind::Instrument => IconKind::Instrument,
                };
                let row = pin.r#ref.clone();
                self.0.state.borrow_mut().pinned = Some(Pin { pin, kind, row });
            }
            SessionEvent::Resumed { saved_at, lines, changes, chosen, unreadable, replay } => {
                // Another conversation in the model's context: the meter waits for its next call (or its log's last).
                self.0.state.borrow_mut().last_call = None;
                let when = since(saved_at as f64, now_ms_f64());
                // What's new, shown as Kumi started, doesn't count: the conversation goes above it.
                let at_start = {
                    let state = self.0.state.borrow();
                    state.transcript.is_empty() || state.news.is_some()
                };
                if chosen == Some(true) {
                    self.0.state.borrow_mut().transcript.add(Entry::Divider { text: format!("Back to your conversation from {when}") });
                } else if unreadable == Some(true) {
                    self.notice(&format!("Your conversation from {when}, which this model can't continue:"), NoticeTone::Info);
                } else {
                    self.notice(&format!("Continuing your conversation from {when}. /new starts fresh."), NoticeTone::Info);
                }
                // With its log, its last requests come back as they happened (the words of those from before its log
                // began first); without one, its words.
                match replay {
                    Some(replay) => self.shown_again(replay),
                    None => self.resumed_words(lines),
                }
                let mut state = self.0.state.borrow_mut();
                let mut earlier: Vec<ChangeRecord> = changes
                    .unwrap_or_default()
                    .into_iter()
                    .map(kept)
                    .filter(|c| !state.records.changes().iter().any(|known| known.id == c.id))
                    .collect();
                if !earlier.is_empty() {
                    let changes = state.records.changes_mut();
                    earlier.append(changes);
                    if earlier.len() > 500 {
                        earlier.drain(..earlier.len() - 500);
                    }
                    *changes = earlier;
                }
                // What's new goes below a conversation carried on at the start, where it's seen.
                if let Some(news) = state.news.take() {
                    let entry = news.borrow().clone();
                    state.transcript.remove(&news);
                    state.transcript.add(entry);
                }
                // Carried on at the start, the conversation takes the welcome screen's place, and its word on Willington.
                let tell = at_start && state.willington_off;
                drop(state);
                if tell {
                    if let Some(words) = crate::daw::current().toggle_words() {
                        self.notice(words.off_at_start, NoticeTone::Info);
                    }
                }
            }
            SessionEvent::SetAsked { ask } => {
                self.0.state.borrow_mut().selection = None;
                self.open_set_ask(ask);
            }
            SessionEvent::Resend { text } => {
                let mut state = self.0.state.borrow_mut();
                // A held message's send puts it back itself when it fails.
                if state.editor.is_empty() && state.sending_held.as_deref() != Some(text.as_str()) {
                    // The last message sent comes back as the box held it, its long pastes chips again.
                    let words = state.last_words.clone().filter(|_| state.last_sent.as_deref() == Some(text.as_str()));
                    match words {
                        Some(words) => state.editor.put_back(&[words]),
                        None => state.editor.set(&text),
                    }
                    state.recall = None;
                }
            }
            SessionEvent::CatchUp { catch_up: catch } => {
                let tell = !self.0.state.borrow().transcript.is_empty() && !(catch.after_reconnect == Some(true) && catch.lines.is_empty());
                if tell {
                    self.notice(&catch_up_text(&catch, now_ms_f64()), NoticeTone::Info);
                }
                self.0.state.borrow_mut().catch_up = Some(catch);
            }
            SessionEvent::Change { change } => {
                let change = kept(change);
                let (new, refresh) = {
                    let mut state = self.0.state.borrow_mut();
                    if let Some(index) = state.records.changes().iter().position(|c| c.id == change.id) {
                        // A picture that came after the change (its curve) is drawn again.
                        if state.records.changes()[index].picture != change.picture {
                            state.images.forget(&change.id);
                        }
                        state.records.changes_mut()[index] = change;
                        (false, false)
                    } else {
                        let refresh = change.track.as_ref().is_some_and(|t| {
                            !t.name.is_empty() && state.focus.as_ref().and_then(|f| f.track.as_ref()).is_some_and(|f| f.name == t.name)
                        });
                        if !state.replaying {
                            state.last_change = Some((change.id.clone(), perf_now()));
                        }
                        if state.instant_running {
                            let id = change.id.clone();
                            state.instant_changes.push(id);
                            if state.instant_changes.len() > 50 {
                                state.instant_changes.remove(0);
                            }
                        }
                        let changes = state.records.changes_mut();
                        changes.push(change);
                        if changes.len() > 500 {
                            changes.remove(0);
                        }
                        if state.current.is_some() {
                            state.turn_changes += 1;
                        }
                        (true, refresh)
                    }
                };
                if refresh && !self.replaying() {
                    self.read_tree(true);
                }
                if new {
                    self.flash();
                }
            }
            SessionEvent::Notice { message } => self.notice(&message, NoticeTone::Info),
            SessionEvent::Job(status) => self.job_event(&status),
            SessionEvent::CarryOn { job, title } => self.carry_on_event(job, &title),
            SessionEvent::Palette { palette } => self.palette_event(palette),
            // An aside's words as they're written, drawn at once.
            SessionEvent::Aside { id, text } => {
                let aside = self.0.state.borrow().asides.iter().find(|aside| aside.borrow().id == id).cloned();
                if let Some(aside) = aside {
                    let mut aside = aside.borrow_mut();
                    let shown = aside.stream.push(&text);
                    aside.answer.push_str(&shown);
                }
                self.0.scheduler.request();
            }
            // What a /slots fetch or check under way says.
            SessionEvent::SlotsProgress { line, .. } => self.notice(&line, NoticeTone::Info),
            // How a ChatGPT sign-in under way goes: its panel shows the code, or the browser opens the address.
            SessionEvent::SignIn { id, url, code } => {
                let heard = self.0.state.borrow().sign_ins.get(&id).cloned();
                if let Some(heard) = heard {
                    heard(url, code);
                }
            }
            SessionEvent::Error { message: said, kind, provider } => {
                {
                    let mut state = self.0.state.borrow_mut();
                    state.failed = true;
                    state.stream.discard();
                }
                let message = self.clean_line(&said, 300);
                self.rest(ProgramState::Error, &message);
                if kind == Some(FailureKind::Auth) {
                    self.alert(&format!("Needs you: {message}"), "Kumi needs you · sign in, in Kumi");
                } else {
                    self.alert(&format!("Stopped: {message}"), &format!("Kumi · stopped · {message}"));
                }
                self.notice(&said, NoticeTone::Warn);
                // Shown again from a log, it was the producer's to deal with then: nothing is offered for it now.
                if let Some(kind) = kind.filter(|_| !self.replaying()) {
                    self.offer_fix(kind, provider.as_deref());
                }
            }
            SessionEvent::Kernel(KernelEvent::Reasoning { text }) => self.reasoned(&text),
            SessionEvent::Kernel(KernelEvent::Usage(usage)) => self.called(usage),
            SessionEvent::Kernel(KernelEvent::Text { text }) => {
                if !text.is_empty() {
                    self.thought_ends();
                }
                let mut state = self.0.state.borrow_mut();
                if state.suppress || state.current.is_none() {
                    return;
                }
                state.bytes += text.len();
                if state.bytes > 4 * 1024 * 1024 {
                    // Shown again from a log, it was stopped then: what's shown of it stops here, as it did, and
                    // nothing live is cancelled.
                    let replaying = state.replaying;
                    if replaying {
                        state.suppress = true;
                        state.stream.discard();
                    }
                    drop(state);
                    self.notice("That answer got too long to show, so Kumi stopped it.", NoticeTone::Warn);
                    if !replaying {
                        self.cancel();
                    }
                    return;
                }
                let text = state.stream.push(&text);
                if let Some(entry) = state.current.clone() {
                    if let Entry::Assistant { text: words, .. } = &mut *entry.borrow_mut() {
                        words.push_str(&text);
                    }
                    state.transcript.touch(&entry);
                }
            }
            SessionEvent::Memory(MemoryEvent::Remembered { scope, note, replaced, pending }) => {
                let set = self.0.state.borrow().set_name.clone().unwrap_or_else(|| format!("this {}", crate::daw::words().place));
                let pending = pending == Some(true);
                let about = match scope {
                    MemoryScope::Producer => "about you".into(),
                    MemoryScope::Set if pending => format!("about {set}, kept once it's saved"),
                    MemoryScope::Set => format!("about {set}"),
                };
                self.memory_line(
                    MemoryKind::Note,
                    &format!("{} {about}: {}", if replaced.is_some() { "Updated a note" } else { "Noted" }, note.text),
                );
                let key = format!("note:{}:{}{}", scope_key(scope), if pending { "pending:" } else { "" }, note.id);
                self.keep(key, MemoryKind::Note, &note.text, Forget::Note(note.id));
            }
            SessionEvent::Memory(MemoryEvent::Forgot { scope, note }) => {
                self.memory_line(MemoryKind::Note, &format!("Forgot: {}", note.text));
                self.forgotten(&format!("note:{}:{}", scope_key(scope), note.id));
                self.forgotten(&format!("note:{}:pending:{}", scope_key(scope), note.id));
            }
            SessionEvent::Lesson { action, id, line } => {
                let key = format!("lesson:{id}");
                match action {
                    LessonAction::Forgot => {
                        self.memory_line(MemoryKind::Lesson, &format!("Forgot a lesson: {line}"));
                        self.forgotten(&key);
                    }
                    LessonAction::Learned | LessonAction::Updated => {
                        self.memory_line(
                            MemoryKind::Lesson,
                            &format!(
                                "{}: {line}",
                                if action == LessonAction::Updated { "Updated what I learned" } else { "Learned from this match" }
                            ),
                        );
                        self.keep(key, MemoryKind::Lesson, &line, Forget::Lesson(id));
                    }
                }
            }
            SessionEvent::Technique(TechniqueEvent { action, technique }) => {
                let name = technique.name.as_str();
                let line = match action {
                    TechniqueAction::Offered => {
                        // (shown again, it's no longer asked)
                        if self.replaying() || self.offer_technique(name) {
                            format!("Keep this as a technique? {name}")
                        } else {
                            format!("Keep this as a technique? {name} · say “keep the technique”")
                        }
                    }
                    TechniqueAction::Kept => format!("Kept a technique: {name}"),
                    TechniqueAction::Updated => format!("Updated a technique: {name}"),
                    TechniqueAction::Used => format!("Using your technique: {name}"),
                    TechniqueAction::Forgot => format!("Forgot the technique: {name}"),
                };
                self.memory_line(MemoryKind::Technique, &line);
                let key = format!("technique:{}", technique.id);
                match action {
                    TechniqueAction::Kept | TechniqueAction::Updated => {
                        self.keep(key, MemoryKind::Technique, name, Forget::Technique(technique.id.clone()));
                    }
                    TechniqueAction::Forgot => self.forgotten(&key),
                    TechniqueAction::Offered | TechniqueAction::Used => {}
                }
            }
            SessionEvent::Recipe(RecipeEvent { action, name, steps }) => {
                let steps = format!("{} {}", number::to_string(steps as f64), if steps == 1 { "step" } else { "steps" });
                self.memory_line(
                    MemoryKind::Recipe,
                    &match action {
                        RecipeAction::Saved => format!("Saved a recipe: {name} ({steps})"),
                        RecipeAction::Updated => format!("Updated a recipe: {name} ({steps})"),
                        RecipeAction::Running => format!("Running your recipe: {name} ({steps})"),
                        RecipeAction::Forgotten => format!("Forgot the recipe: {name}"),
                    },
                );
                let key = format!("recipe:{}", name.to_lowercase());
                match action {
                    RecipeAction::Saved | RecipeAction::Updated => {
                        self.keep(key, MemoryKind::Recipe, &name, Forget::Recipe(name.clone()));
                    }
                    RecipeAction::Forgotten => self.forgotten(&key),
                    RecipeAction::Running => {}
                }
            }
            SessionEvent::Action(ActionEvent { title, playing, recording }) => {
                let title = self.clean(&title, 120);
                let glyph = if recording == Some(true) {
                    "●"
                } else if playing == Some(true) {
                    "▶"
                } else if playing == Some(false) || recording == Some(false) {
                    "■"
                } else {
                    "›"
                };
                if !self.replaying() {
                    self.0.state.borrow_mut().last_action = Some(LastAction { title, at: perf_now(), glyph: glyph.into(), memory: false });
                    self.flash();
                }
            }
            SessionEvent::Transport { transport } => {
                let started = {
                    let mut state = self.0.state.borrow_mut();
                    let was = state.transport.as_ref().is_some_and(|transport| transport.playing);
                    let now = transport.as_ref().is_some_and(|transport| transport.playing);
                    state.transport = transport;
                    now && !was && matches!(state.own.body, Some(FocusBody::Tracker(_)))
                };
                self.next_beat();
                // (a pattern that starts playing is read again for its playing line; then it's followed)
                if started {
                    self.read_own(true);
                }
                self.follow_lines();
            }
            SessionEvent::Watching { on } => self.0.state.borrow_mut().watching = on,
            SessionEvent::Heard(heard) => self.insert_before(Entry::Heard {
                file: self.clean(&heard.file, 120),
                summary: self.clean(&heard.summary, 200),
                bands: heard.bands,
                compared: heard.compared.map(|c| transcript::Comparison {
                    reference: self.clean(&c.reference, 120),
                    summary: self.clean(&c.summary, 200),
                    differences: c.differences,
                }),
            }),
            SessionEvent::Goal(status) => {
                if matches!(status.state, GoalPhase::Running | GoalPhase::Starting) {
                    self.0.state.borrow_mut().run_active = true;
                }
                let was = self.0.state.borrow().goal.as_ref().map(|goal| goal.status.state);
                if matches!(was, Some(GoalPhase::Running | GoalPhase::Starting))
                    && matches!(status.state, GoalPhase::Done | GoalPhase::Paused)
                {
                    let best = status.best.as_ref().map(|best| format!(" · best {}%", number::to_string(best.score))).unwrap_or_default();
                    let ended = if status.state == GoalPhase::Done { "done" } else { "paused" };
                    self.run_ended(&format!("Sound match {ended}{best}"), &format!("Kumi · sound match {ended}{best}"));
                }
                let show = self.0.state.borrow().goal.is_none() || status.state == GoalPhase::Starting;
                if show && !self.replaying() {
                    self.0.tabs.show("goal");
                }
                let mut state = self.0.state.borrow_mut();
                state.goal = Some(Timed { since: perf_now() - status.elapsed_ms as f64, status });
                // A sound-match search took over: the GOAL tab shows it, not the last /goal.
                state.objective = None;
            }
            SessionEvent::Match(status) => {
                let running = status.state == MatchState::Running;
                if running {
                    self.0.state.borrow_mut().run_active = true;
                }
                self.0.state.borrow_mut().matching =
                    running.then(|| Timed { since: perf_now() - status.elapsed_ms as f64, status: status.clone() });
                if let (MatchState::Done, Some(best)) = (status.state, &status.best) {
                    let score = number::to_string(best.score);
                    self.run_ended(&format!("Matching done · best {score}%"), &format!("Kumi · matching done · {score}%"));
                    let first = status
                        .first
                        .filter(|first| *first != best.score)
                        .map(|first| format!("{}% → ", number::to_string(first)))
                        .unwrap_or_default();
                    let why = match status.stop.unwrap_or(MatchStop::Plateau) {
                        MatchStop::Reached => "close enough",
                        MatchStop::Budget => "its budget spent",
                        MatchStop::NoAudition => "nothing to compare",
                        MatchStop::Plateau => "no more gain",
                    };
                    self.notice(
                        &format!("Matching: {first}{score}% ({}) · {} · {why}", best.label, helpers::clock_of(status.elapsed_ms as f64)),
                        NoticeTone::Info,
                    );
                }
            }
            SessionEvent::Objective(status) => {
                if status.state == ObjectiveState::Running {
                    self.0.state.borrow_mut().run_active = true;
                }
                let was = self.0.state.borrow().objective.as_ref().map(|goal| goal.status.state);
                if was == Some(ObjectiveState::Running) && status.state != ObjectiveState::Running {
                    let goal = self.clean_line(&status.objective, 80);
                    let reason = self.clean_line(status.reason.as_deref().unwrap_or(""), 120);
                    let (body, live) = match (status.state, status.verdict) {
                        (ObjectiveState::Done, Some(Verdict::Complete)) => {
                            (format!("Goal met: {goal}"), format!("Kumi · goal met · {goal}"))
                        }
                        (_, Some(Verdict::Blocked)) => (format!("Goal blocked: {reason}"), "Kumi needs you · goal blocked".to_string()),
                        (_, Some(Verdict::Stuck)) => (format!("Goal stuck: {reason}"), "Kumi · goal stuck".to_string()),
                        (_, Some(Verdict::Budget)) => {
                            (format!("Goal stopped: its budget is spent ({goal})"), "Kumi · goal out of budget".to_string())
                        }
                        (_, None | Some(Verdict::Complete | Verdict::Continue)) => {
                            (format!("Goal paused: {}", if reason.is_empty() { &goal } else { &reason }), "Kumi · goal paused".to_string())
                        }
                    };
                    self.run_ended(&body, &live);
                }
                self.0.state.borrow_mut().objective = Some(Timed { since: perf_now() - status.elapsed_ms as f64, status });
                if !self.replaying() {
                    self.0.tabs.show("goal");
                }
            }
            // No goal here (none set, or another Set is open): the GOAL tab stops showing the last one.
            SessionEvent::ObjectiveCleared => self.0.state.borrow_mut().objective = None,
            // Requests taken back: what's shown is the conversation as it now stands, without them.
            SessionEvent::TakenBack { replay, .. } | SessionEvent::Forked { replay, .. } => self.shown_anew(replay),
            SessionEvent::Loop(status) => {
                let running = status.state == LoopState::Running;
                if running {
                    self.0.state.borrow_mut().run_active = true;
                }
                self.0.state.borrow_mut().looping =
                    running.then(|| Timed { since: perf_now() - status.elapsed_ms as f64, status: status.clone() });
                if status.state == LoopState::Done && status.rounds > 0 {
                    let (rounds, kept) = (status.rounds, status.kept);
                    self.run_ended(&format!("Loop done: {rounds} rounds, {kept} kept"), &format!("Kumi · loop done · {kept} kept"));
                    let why = match status.stop {
                        Some(LoopStop::Met) => "every item within tolerance",
                        Some(LoopStop::Stalled) => "changes stopped helping",
                        Some(LoopStop::Budget) => "its budget spent",
                        Some(LoopStop::Ended) => "ended",
                        Some(LoopStop::Unjudged) | None => "nothing more judged",
                    };
                    self.notice(
                        &format!(
                            "Loop: {} rounds · {} kept · {} taken back · {} listens · {} · {why}",
                            status.rounds,
                            status.kept,
                            status.reverted,
                            status.listens,
                            helpers::clock_of(status.elapsed_ms as f64)
                        ),
                        NoticeTone::Info,
                    );
                }
            }
            SessionEvent::Judged(round) => {
                let lines = round.lines().iter().map(|line| self.clean_line(line, 400)).collect();
                self.insert_before(Entry::Judged { lines, kept: round.kept, met: round.met });
            }
            SessionEvent::Auditioned(audition) => {
                let gaps = audition
                    .gaps
                    .iter()
                    .take(3)
                    .map(|gap| {
                        let mut text = gap.as_str();
                        if let Some((prefix, _)) = text.split_once(" (").filter(|_| text.ends_with(')')) {
                            text = prefix;
                        }
                        text = text.strip_suffix(" against the reference").unwrap_or(text);
                        self.clean_line(text, 48)
                    })
                    .collect::<Vec<_>>();
                self.insert_before(Entry::Auditioned {
                    round: audition.round.into(),
                    best: audition.best.map(|best| transcript::Best { label: self.clean_line(&best.label, 60), score: best.score }),
                    previous: audition.previous,
                    takes: audition
                        .takes
                        .into_iter()
                        .take(8)
                        .map(|take| transcript::Take { label: self.clean_line(&take.label, 40), score: take.score, silent: take.silent })
                        .collect(),
                    gaps,
                });
            }
            SessionEvent::Watched(watched) => self.insert_before(Entry::Watched {
                title: self.clean_line(&watched.title, 160),
                channel: watched.channel.filter(|channel| !channel.is_empty()).map(|channel| self.clean_line(&channel, 80)),
                duration: watched.duration.filter(|duration| *duration != 0.),
                from: watched.from,
                to: watched.to,
                chapters: watched.chapters.iter().take(24).map(|chapter| self.clean_line(chapter, 60)).collect(),
                words: match watched.words {
                    WordsSource::Captions => "its captions",
                    WordsSource::Automatic => "its automatic captions",
                    WordsSource::Transcribed => "its speech, transcribed by Kumi",
                    WordsSource::None => "no words",
                }
                .into(),
                frames: watched
                    .frames
                    .into_iter()
                    .take(16)
                    .filter(|frame| {
                        let (w, h) = (frame.thumb.width as usize, frame.thumb.height as usize);
                        w > 0 && w <= 64 && h > 0 && h <= 64 && frame.thumb.rgb.len() == w * h * 3
                    })
                    .map(|frame| transcript::Frame {
                        at: frame.at,
                        zoom: frame.zoom,
                        thumb: transcript::Picture {
                            width: frame.thumb.width as usize,
                            height: frame.thumb.height as usize,
                            rgb: frame.thumb.rgb,
                        },
                    })
                    .collect(),
                sound: watched.sound.map(|sound| transcript::Sound { from: sound.from, to: sound.to }),
                notes: watched.notes.iter().map(|note| self.clean_line(note, 300)).collect(),
                pictures: matches!(self.0.depth, ColorDepth::Truecolor | ColorDepth::Colors256),
            }),
            SessionEvent::Web(web) => {
                let words = web_words(&web, &|text, max| {
                    head(
                        &sanitize_text(text, &self.0.state.borrow().secrets)
                            .split(|c: char| string::trim(&c.to_string()).is_empty())
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(" "),
                        max,
                    )
                });
                let line = WebLine { lead: words.lead, title: words.title, detail: words.detail };
                let mut state = self.0.state.borrow_mut();
                let at = state.current.as_ref().and_then(|current| state.transcript.entries.iter().position(|e| Rc::ptr_eq(e, current)));
                let above = match at {
                    Some(n) if n > 0 => state.transcript.entries.get(n - 1).cloned(),
                    None => state.transcript.entries.last().cloned(),
                    _ => None,
                };
                let mut added = false;
                if let Some(above) = above {
                    if let Entry::Web { lines } = &mut *above.borrow_mut() {
                        if lines.len() < 24 {
                            lines.push(line.clone());
                            added = true;
                        }
                    }
                    if added {
                        state.transcript.touch(&above);
                    }
                }
                if !added {
                    let current = state.current.clone();
                    state.transcript.insert_before(Entry::Web { lines: vec![line] }, current.as_ref());
                }
            }
            SessionEvent::Kernel(KernelEvent::Steer { text }) => {
                // Shown as the box showed it, a long paste as its chip.
                let shown = {
                    let mut state = self.0.state.borrow_mut();
                    match state.held.iter().position(|h| h.when == "now" && h.taken && h.text == text) {
                        Some(at) => state.held.remove(at).shown,
                        None => text.clone(),
                    }
                };
                let words = string::trim(&self.clean(&shown, usize::MAX)).to_string();
                let mut state = self.0.state.borrow_mut();
                if let Some(before) = state.current.take() {
                    let tail = if !state.suppress { state.stream.finish() } else { String::new() };
                    let empty = if let Entry::Assistant { text, status, steps, .. } = &mut *before.borrow_mut() {
                        text.push_str(&tail);
                        *status = AnswerStatus::Done;
                        text.is_empty() && steps.is_empty()
                    } else {
                        false
                    };
                    state.transcript.touch(&before);
                    if empty {
                        state.transcript.remove(&before);
                    }
                    state.transcript.add(Entry::User { text: words });
                    state.current = Some(state.transcript.add(assistant(Some(perf_now()), AnswerStatus::Running)));
                } else {
                    state.transcript.add(Entry::User { text: words });
                }
            }
            SessionEvent::Doing { text } => {
                let text = self.clean_line(&text, 80);
                let state = self.0.state.borrow();
                if let Some(entry) = &state.current {
                    let mut changed = false;
                    if let Entry::Assistant { steps, .. } = &mut *entry.borrow_mut() {
                        if let Some(step) = steps.iter_mut().rev().find(|s| s.state == StepState::Running) {
                            step.doing = Some(text);
                            changed = true;
                        }
                    }
                    if changed {
                        state.transcript.touch(entry);
                    }
                }
            }
            SessionEvent::Kernel(KernelEvent::ToolInput { id, name }) => {
                self.thought_ends();
                let mut state = self.0.state.borrow_mut();
                if state.current.is_some() && !state.suppress && (crate::daw::presentation().plan)(&name) {
                    state.planning = Some(id);
                    state.planning_since = perf_now();
                }
            }
            SessionEvent::Kernel(KernelEvent::ToolStart { id, name }) => {
                self.thought_ends();
                let mut state = self.0.state.borrow_mut();
                if state.planning.as_deref() == Some(id.as_str()) {
                    state.planning = None;
                }
                if !state.suppress && !QUIET_TOOLS.contains(&name.as_str()) {
                    if let Some(entry) = &state.current {
                        if let Entry::Assistant { steps, .. } = &mut *entry.borrow_mut() {
                            steps.push(Step {
                                id,
                                label: step_label(&name).into(),
                                tool: Some(name),
                                state: StepState::Running,
                                started_at: Some(perf_now()),
                                ..Default::default()
                            });
                        }
                        state.transcript.touch(entry);
                    }
                }
            }
            SessionEvent::Kernel(KernelEvent::ToolEnd { id, is_error, elapsed_ms, .. }) => {
                let state = self.0.state.borrow();
                if let Some(entry) = &state.current {
                    let mut changed = false;
                    if let Entry::Assistant { steps, .. } = &mut *entry.borrow_mut() {
                        if let Some(step) = steps.iter_mut().find(|s| s.id == id) {
                            step.state = if is_error { StepState::Error } else { StepState::Done };
                            step.ms = Some(elapsed_ms as f64);
                            step.ended_at = Some(perf_now());
                            step.doing = None;
                            changed = true;
                        }
                    }
                    if changed {
                        state.transcript.touch(entry);
                    }
                }
            }
            SessionEvent::TurnComplete { result, elapsed_ms } => {
                let reason = result.stop_reason;
                let elapsed = elapsed_ms as f64;
                let mut offered = None;
                let mut answer = String::new();
                self.thought_ends();
                let changes = self.0.state.borrow().turn_changes;
                let footer = self.answer_footer(elapsed, changes, result.usage.as_ref());
                let model = self.answering_model();
                {
                    let mut state = self.0.state.borrow_mut();
                    // (an answer shown again from a log was counted by the session that answered it)
                    if let Some(usage) = result.usage.as_ref().filter(|_| !state.replaying) {
                        state.used.input += usage.input_tokens;
                        state.used.output += usage.output_tokens;
                        state.used.cached += usage.cache_read_tokens;
                        state.used.answers += 1;
                    }
                    state.last_stop = Some(reason);
                    let Some(entry) = state.current.take() else {
                        drop(state);
                        self.answer_ended(reason, elapsed, "", None);
                        self.0.scheduler.request();
                        return;
                    };
                    let cancelled = reason == StopReason::Cancelled;
                    let tail = if !cancelled && !state.suppress {
                        state.stream.finish()
                    } else {
                        state.stream.discard();
                        String::new()
                    };
                    // The model that gave it, for the details (ctrl+o).
                    if let Some(model) = model.filter(|_| !state.replaying) {
                        state.answer_models.insert(entry.id, model);
                    }
                    if let Entry::Assistant { text, steps, status, elapsed_ms, footer: line, .. } = &mut *entry.borrow_mut() {
                        text.push_str(&tail);
                        *status = if cancelled { AnswerStatus::Stopped } else { AnswerStatus::Done };
                        *elapsed_ms = Some(elapsed);
                        // Under what the answer said or did; one with neither (a run told in notices) has no line.
                        *line = (!text.is_empty() || !steps.is_empty()).then_some(footer);
                        // An answer that ends asking the producer to pick: one key answers, unless they've already
                        // typed or queued something.
                        if reason == StopReason::Completed && state.held.is_empty() && state.editor.is_empty() && state.overlays.is_empty()
                        {
                            offered = choices(text);
                        }
                        answer = text.clone();
                    }
                    end_steps(&entry, !state.replaying);
                    state.transcript.touch(&entry);
                }
                self.answer_ended(reason, elapsed, &answer, offered.as_ref().map(|(_, choices)| choices.len()));
                if let Some(offered) = offered.filter(|_| !self.replaying()) {
                    self.0.state.borrow_mut().selection = None;
                    self.open_answers(offered);
                }
                if reason == StopReason::MaxSteps {
                    self.notice("Kumi reached its step limit for one answer. Ask it to carry on.", NoticeTone::Info);
                }
            }
        }
        // Shown again from a log, nothing of the moment comes with it: what the producer holds isn't sent, no job's
        // request carries on, and it's drawn once it's all there.
        if self.replaying() {
            return;
        }
        if at_work && self.0.state.borrow().held.iter().any(|h| h.when == "now" && !h.taken && !h.asked) {
            self.steer_held();
        }
        if carry_on {
            self.maybe_carry_on();
        }
        // What the session sends (words as they stream, state, the DAW's events) is drawn at the animation's pace;
        // only the producer's own input is drawn within the 16 ms floor.
        self.0.scheduler.request_animation();
    }
    fn replaying(&self) -> bool {
        self.0.state.borrow().replaying
    }
    /// The conversation as it now stands, in place of what was shown: requests taken back, or a copy carried on in from
    /// before one (None: nothing of it to show).
    fn shown_anew(&self, replay: Option<Replay>) {
        {
            let mut state = self.0.state.borrow_mut();
            state.transcript.clear();
            state.current = None;
            state.selection = None;
            state.view.follow();
        }
        if let Some(replay) = replay {
            self.shown_again(replay);
        }
        self.0.scheduler.request();
    }
    /// A conversation shown again from its log: how many earlier messages aren't shown, the words of those from before
    /// its log began, then its last requests as they happened.
    fn shown_again(&self, replay: Replay) {
        match replay.before {
            0 => {}
            1 => self.notice("1 earlier message isn't shown.", NoticeTone::Info),
            n => self.notice(&format!("{n} earlier messages aren't shown."), NoticeTone::Info),
        }
        self.resumed_words(replay.earlier);
        self.replayed(replay.lines);
    }
    /// A kept conversation's words, as a conversation without a log comes back: what the producer asked, and each
    /// answer with the steps its tools took.
    fn resumed_words(&self, lines: Vec<TranscriptLine>) {
        let mut answer: Option<EntryRef> = None;
        for line in lines {
            let text = self.clean(&line.text, usize::MAX);
            if line.role == TranscriptRole::User {
                answer = None;
                if !text.is_empty() {
                    self.0.state.borrow_mut().transcript.add(Entry::User { text });
                }
                continue;
            }
            let answer = answer.get_or_insert_with(|| self.0.state.borrow_mut().transcript.add(assistant(None, AnswerStatus::Done)));
            if let Entry::Assistant { text: words, steps, .. } = &mut *answer.borrow_mut() {
                if !text.is_empty() {
                    if !words.is_empty() {
                        words.push_str("\n\n");
                    }
                    words.push_str(&text);
                }
                for tool in line.tools.iter().flatten() {
                    if !QUIET_TOOLS.contains(&tool.as_str()) {
                        steps.push(Step {
                            id: format!("resumed:{}", steps.len()),
                            tool: Some(tool.into()),
                            label: step_label(tool).into(),
                            ..Default::default()
                        });
                    }
                }
            }
            self.0.state.borrow_mut().transcript.touch(answer);
        }
    }
    /// A conversation taken up, shown again from its log as it happened: each request as the producer sent it, then each
    /// event as it came, through the same handling, with nothing of the moment (`replaying`). An answer the log ends in
    /// the middle of (Kumi stopped with it) ends there, stopped.
    fn replayed(&self, lines: Vec<LogLine>) {
        // What the pane shows of how things are now (runs, jobs, the last catch-up) is the live session's: what the log
        // says of them then goes once it's shown.
        let now = {
            let mut state = self.0.state.borrow_mut();
            state.replaying = true;
            (
                state.goal.clone(),
                state.matching.clone(),
                state.objective.clone(),
                state.looping.clone(),
                state.run_active,
                state.jobs.clone(),
                state.catch_up.clone(),
            )
        };
        for line in lines {
            match line {
                LogLine::Request(request) => {
                    let text = string::trim(&self.clean(&request.text, usize::MAX)).to_string();
                    let mut state = self.0.state.borrow_mut();
                    // One the log leaves open before it (a run that stopped without ending it) ended there.
                    stopped_there(&mut state);
                    if !text.is_empty() {
                        state.transcript.add(Entry::User { text });
                    }
                    state.pending_turn = true;
                }
                LogLine::Event { event, .. } => self.event(event),
                LogLine::Header(_) | LogLine::Message { .. } | LogLine::Saved { .. } | LogLine::TakenBack { .. } => {}
            }
        }
        let mut state = self.0.state.borrow_mut();
        stopped_there(&mut state);
        state.pending_turn = false;
        state.busy_since = 0.;
        state.suppress = false;
        state.failed = false;
        state.last_stop = None;
        state.stream.discard();
        (state.goal, state.matching, state.objective, state.looping, state.run_active, state.jobs, state.catch_up) = now;
        state.replaying = false;
        drop(state);
        self.0.scheduler.request();
    }
    fn clean_line(&self, text: &str, max: usize) -> String {
        head(&sanitize_text(text, &self.0.state.borrow().secrets).replace('\n', " "), max)
    }
    fn insert_before(&self, entry: Entry) {
        let mut state = self.0.state.borrow_mut();
        let current = state.current.clone();
        state.transcript.insert_before(entry, current.as_ref());
    }
    fn flash(&self) {
        if self.replaying() {
            return;
        }
        let weak = Rc::downgrade(&self.0);
        // A redraw when the flash ends, kept so quitting aborts it: a JavaScript timer that didn't hold
        // the process open.
        let timer = tokio::task::spawn_local(async move {
            tokio::time::sleep(Duration::from_millis((CHANGE_FLASH_MS + 20.) as u64)).await;
            if let Some(a) = weak.upgrade() {
                if !a.state.borrow().closing {
                    a.scheduler.request();
                }
            }
        });
        let mut state = self.0.state.borrow_mut();
        state.flash_timers.retain(|timer| !timer.is_finished());
        state.flash_timers.push(timer);
    }
    pub(super) fn memory_line(&self, what: MemoryKind, text: &str) {
        let text = self.clean_line(text, 300);
        let mut state = self.0.state.borrow_mut();
        state.transcript.add(Entry::Memory { what, text: text.clone() });
        if !state.replaying {
            state.last_action = Some(LastAction { title: head(&text, 120), at: perf_now(), glyph: what.glyph().into(), memory: true });
        }
        drop(state);
        self.flash();
        self.0.scheduler.request();
    }
    fn keep(&self, key: String, what: MemoryKind, title: &str, forget: Forget) {
        let title = self.clean_line(title, 200);
        let mut state = self.0.state.borrow_mut();
        let kept = state.records.kept_mut();
        if let Some(index) = kept.iter().position(|e| e.borrow().key == key) {
            kept.remove(index);
        }
        kept.push(Rc::new(RefCell::new(Kept { key, what, title, forgotten: false, forget })));
        if kept.len() > 50 {
            kept.remove(0);
        }
    }
    /// Forgets something Kumi kept, through the session: whether it was still there to forget.
    pub(super) async fn forget(&self, forget: Forget) -> Result<bool, RuntimeError> {
        match forget {
            Forget::Note(id) => Ok(self.session().op(op::Forget { id }).await?.is_some()),
            Forget::Lesson(id) => self.session().op(op::ForgetLesson { id }).await,
            Forget::Technique(id) => self.session().op(op::ForgetTechnique { id }).await,
            Forget::Recipe(name) => self.session().op(op::ForgetRecipe { name }).await,
        }
    }
    fn forgotten(&self, key: &str) {
        let mut state = self.0.state.borrow_mut();
        let entries: Vec<_> = state.records.kept().iter().filter(|entry| entry.borrow().key == key).cloned().collect();
        for entry in entries {
            state.records.set_forgotten(&entry);
        }
    }
    pub(super) fn hold(&self, raw: String, shown: String, words: Composed, when: &'static str) {
        self.0.state.borrow_mut().held.push(Held { text: raw, shown, words, when, taken: false, asked: false });
        if when == "now" {
            self.steer_held();
        }
        self.0.scheduler.request();
    }
    fn steer_held(&self) {
        if !self.can().steer {
            return;
        }
        let held = self.0.state.borrow().held.clone();
        for (index, item) in held.into_iter().enumerate() {
            if item.when == "now" && !item.taken && !item.asked {
                // Asked until the answer says: a session on a thread of its own answers after the events meanwhile, and
                // one that didn't take it is asked again at the next.
                if let Some(current) = self.0.state.borrow_mut().held.get_mut(index) {
                    current.asked = true;
                }
                self.when_answered(op::Steer { text: item.text.clone() }, move |app, taken| {
                    if let Some(current) = app.0.state.borrow_mut().held.get_mut(index).filter(|h| h.text == item.text) {
                        current.taken = taken.unwrap_or(false);
                        current.asked = false;
                    }
                });
            }
        }
    }
    fn after_busy(&self) {
        if self.replaying() {
            return;
        }
        let next = {
            let mut state = self.0.state.borrow_mut();
            let back = state.last_stop == Some(StopReason::Cancelled) || state.failed || state.cancelling;
            state.last_stop = None;
            state.failed = false;
            if state.held.is_empty() || state.closing {
                return;
            }
            if back {
                let held: Vec<Composed> = state.held.drain(..).map(|h| h.words).collect();
                state.editor.put_back(&held);
                state.recall = None;
                return;
            }
            state.held.remove(0)
        };
        let words = string::trim(&self.clean(&next.shown, usize::MAX)).to_string();
        let shown = {
            let mut state = self.0.state.borrow_mut();
            state.last_sent = Some(next.text.clone());
            state.last_words = Some(next.words.clone());
            state.view.follow();
            state.transcript.add(Entry::User { text: words })
        };
        self.task(move |app| async move {
            app.0.state.borrow_mut().sending_held = Some(next.text.clone());
            let sent = app.send_naming(&next.text, next.words.references()).await;
            app.0.state.borrow_mut().sending_held = None;
            if sent || app.0.state.borrow().closing {
                return Ok(());
            }
            let mut state = app.0.state.borrow_mut();
            state.transcript.remove(&shown);
            let mut back = vec![next.words];
            back.extend(state.held.drain(..).map(|h| h.words));
            state.editor.put_back(&back);
            drop(state);
            app.0.scheduler.request();
            Ok(())
        });
    }
}
/// A change as its saved record keeps it (a notes picture's numbers to 3 decimals), so one shown as it's made and one a
/// carried-on conversation brings back from disk read the same: a note restated with fewer decimals stays the note it was.
fn kept(change: ChangeRecord) -> ChangeRecord {
    ChangeRecord { picture: change.picture.map(ChangePicture::as_kept), ..change }
}
/// A memory's scope as the keys of what Kumi kept name it ("note:producer:p3").
fn scope_key(scope: MemoryScope) -> &'static str {
    match scope {
        MemoryScope::Producer => "producer",
        MemoryScope::Set => "set",
    }
}
/// An answer a log ends in the middle of (Kumi stopped with it), ended there: its steps cut short have no time, since
/// the log doesn't say when they'd have ended.
fn stopped_there(state: &mut State) {
    if let Some(entry) = state.current.take() {
        if let Entry::Assistant { status, .. } = &mut *entry.borrow_mut() {
            *status = AnswerStatus::Stopped;
        }
        end_steps(&entry, false);
        state.transcript.touch(&entry);
    }
}
