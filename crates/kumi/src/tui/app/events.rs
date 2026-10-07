use super::super::{
    choices::choices,
    icons::IconKind,
    transcript::{step_label, Step, WebLine},
};
use super::*;
use crate::text::web_words;
use kumi_runtime::integrations::ableton::project::since;
use serde_json::json;

impl TuiApp {
    pub(super) fn event(&self, event: SessionEvent) {
        if self.0.state.borrow().closing {
            return;
        }
        let value = serde_json::to_value(&event).expect("session event");
        let kind = value["type"].as_str().unwrap_or("");
        let get = |key: &str| value[key].as_str().unwrap_or("");
        let n = |key: &str| value[key].as_f64().unwrap_or(0.);
        // Anything from the model, or the turn's end, means the wait is over.
        if matches!(kind, "text" | "tool-input" | "tool-start" | "turn-complete" | "state") {
            self.0.state.borrow_mut().retry = None;
        }
        match kind {
            "retry" => {
                let reason = self.clean_line(get("reason"), 80);
                // No wait: Kumi carries on at once, and the model's next words end it.
                let until = if n("waitMs") > 0. { perf_now() + n("waitMs") } else { f64::INFINITY };
                self.0.state.borrow_mut().retry = Some((reason, until));
            }
            "state" => {
                {
                    let mut state = self.0.state.borrow_mut();
                    match get("state") {
                        "running" => {
                            if state.pending_turn {
                                state.pending_turn = false;
                                state.last_stop = None;
                                state.suppress = false;
                                state.failed = false;
                                state.bytes = 0;
                                state.stream.discard();
                                state.current = Some(state.transcript.add(assistant(Some(perf_now()), AnswerStatus::Running)));
                                state.scroll = 0;
                                state.planning = None;
                                state.turn_changes = 0;
                            }
                        }
                        "cancelling" => {
                            state.suppress = true;
                            state.stream.discard();
                        }
                        "idle" if state.pending_turn => state.pending_turn = false,
                        _ => {
                            if let Some(entry) = state.current.take() {
                                if let Entry::Assistant { status, .. } = &mut *entry.borrow_mut() {
                                    *status = if state.failed { AnswerStatus::Failed } else { AnswerStatus::Stopped };
                                }
                                end_steps(&entry);
                                state.transcript.touch(&entry);
                            }
                        }
                    }
                }
                if get("state") == "idle" {
                    self.after_busy();
                }
                {
                    let mut state = self.0.state.borrow_mut();
                    if matches!(get("state"), "running" | "cancelling") && state.busy_since == 0. {
                        state.busy_since = perf_now();
                    } else if get("state") == "idle" {
                        state.busy_since = 0.;
                    }
                }
                self.0.scheduler.set_animating(
                    matches!(get("state"), "running" | "cancelling") || self.0.voice.as_ref().is_some_and(VoiceInput::active),
                );
            }
            "connection" => {
                self.0.state.borrow_mut().connection = serde_json::from_value(value["state"].clone()).unwrap();
                if get("state") == "connected" {
                    self.read_tree(true);
                    self.read_clip(true);
                    self.read_strip(true);
                } else {
                    let mut state = self.0.state.borrow_mut();
                    state.tree = None;
                    state.tree_revision += 1;
                    state.tree_key = None;
                    state.tree_cursor = None;
                    state.clip = None;
                    state.clip_key = None;
                    state.strip = Strip::default();
                }
            }
            "observation" => self.0.state.borrow_mut().set_name = set_name_from(get("label")),
            "library" => {
                let tell = {
                    let mut state = self.0.state.borrow_mut();
                    let library: LibraryStatus = serde_json::from_value(value["status"].clone()).unwrap();
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
            "focus" => {
                {
                    let mut state = self.0.state.borrow_mut();
                    let focus: Option<LiveFocus> = serde_json::from_value(value["focus"].clone()).unwrap();
                    state.touched = touched_next(state.focus.as_ref(), focus.as_ref(), state.touched);
                    state.focus = focus;
                }
                self.read_tree(false);
                self.read_clip(false);
                self.read_strip(false);
            }
            "pointed" => {
                let pin: PinnedNode = serde_json::from_value(value["pin"].clone()).unwrap();
                let kind = match value["pin"]["node"].as_str().unwrap_or("") {
                    "track" => IconKind::MidiTrack,
                    "scene" => IconKind::Scene,
                    "clip" | "selection" | "clip-slot" => IconKind::MidiClip,
                    "chain" => IconKind::Chain,
                    _ => IconKind::Device,
                };
                self.0.state.borrow_mut().pinned = Some(Pin { pin, kind });
            }
            "resumed" => {
                let when = since(n("savedAt"), now_ms_f64());
                // What's new, shown as Kumi started, doesn't count: the conversation goes above it.
                let at_start = {
                    let state = self.0.state.borrow();
                    state.transcript.is_empty() || state.news.is_some()
                };
                if value["chosen"] == true {
                    self.0.state.borrow_mut().transcript.add(Entry::Divider { text: format!("Back to your conversation from {when}") });
                } else if value["unreadable"] == true {
                    self.notice(&format!("Your conversation from {when}, which this model can't continue:"), NoticeTone::Info);
                } else {
                    self.notice(&format!("Continuing your conversation from {when}. /new starts fresh."), NoticeTone::Info);
                }
                let mut answer: Option<EntryRef> = None;
                for line in array(&value["lines"]) {
                    let text = self.clean(s(&line["text"]), usize::MAX);
                    if line["role"] == "user" {
                        answer = None;
                        if !text.is_empty() {
                            self.0.state.borrow_mut().transcript.add(Entry::User { text });
                        }
                        continue;
                    }
                    let answer =
                        answer.get_or_insert_with(|| self.0.state.borrow_mut().transcript.add(assistant(None, AnswerStatus::Done)));
                    if let Entry::Assistant { text: words, steps, .. } = &mut *answer.borrow_mut() {
                        if !text.is_empty() {
                            if !words.is_empty() {
                                words.push_str("\n\n");
                            }
                            words.push_str(&text);
                        }
                        for tool in array(&line["tools"]).iter().map(s) {
                            if !QUIET_TOOLS.contains(&tool) {
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
                let mut state = self.0.state.borrow_mut();
                let mut earlier: Vec<ChangeRecord> = array(&value["changes"])
                    .iter()
                    .map(|c| serde_json::from_value(c.clone()).unwrap())
                    .filter(|c: &ChangeRecord| !state.records.changes().iter().any(|known| known.id == c.id))
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
                    self.notice(crate::willington::OFF_AT_START, NoticeTone::Info);
                }
            }
            "resend" => {
                let mut state = self.0.state.borrow_mut();
                if state.editor.is_empty() {
                    state.editor.set(get("text"));
                    state.recall = None;
                }
            }
            "catch-up" => {
                let catch: CatchUp = serde_json::from_value(value["catchUp"].clone()).unwrap();
                let tell = !self.0.state.borrow().transcript.is_empty() && !(catch.after_reconnect == Some(true) && catch.lines.is_empty());
                if tell {
                    self.notice(&catch_up_text(&catch, now_ms_f64()), NoticeTone::Info);
                }
                self.0.state.borrow_mut().catch_up = Some(catch);
            }
            "change" => {
                let change: ChangeRecord = serde_json::from_value(value["change"].clone()).unwrap();
                let (new, refresh) = {
                    let mut state = self.0.state.borrow_mut();
                    if let Some(index) = state.records.changes().iter().position(|c| c.id == change.id) {
                        state.records.changes_mut()[index] = change;
                        (false, false)
                    } else {
                        let refresh = change.track.as_ref().is_some_and(|t| {
                            !t.name.is_empty() && state.focus.as_ref().and_then(|f| f.track.as_ref()).is_some_and(|f| f.name == t.name)
                        });
                        state.last_change = Some((change.id.clone(), perf_now()));
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
                if refresh {
                    self.read_tree(true);
                }
                if new {
                    self.flash();
                }
            }
            "notice" => self.notice(get("message"), NoticeTone::Info),
            "error" => {
                {
                    let mut state = self.0.state.borrow_mut();
                    state.failed = true;
                    state.stream.discard();
                }
                self.notice(get("message"), NoticeTone::Warn);
                if !get("kind").is_empty() {
                    self.offer_fix(get("kind"), value["provider"].as_str());
                }
            }
            "text" => {
                let mut state = self.0.state.borrow_mut();
                if state.suppress || state.current.is_none() {
                    return;
                }
                state.bytes += get("text").len();
                if state.bytes > 4 * 1024 * 1024 {
                    drop(state);
                    self.notice("That answer got too long to show, so Kumi stopped it.", NoticeTone::Warn);
                    self.cancel();
                    return;
                }
                let text = state.stream.push(get("text"));
                if let Some(entry) = state.current.clone() {
                    if let Entry::Assistant { text: words, .. } = &mut *entry.borrow_mut() {
                        words.push_str(&text);
                    }
                    state.transcript.touch(&entry);
                }
            }
            "remembered" => {
                let set = self.0.state.borrow().set_name.clone().unwrap_or_else(|| "this Set".into());
                let about = if get("scope") == "producer" {
                    "about you".into()
                } else if value["pending"] == true {
                    format!("about {set}, kept once it's saved")
                } else {
                    format!("about {set}")
                };
                self.memory_line(
                    MemoryKind::Note,
                    &format!(
                        "{} {about}: {}",
                        if value.get("replaced").is_some() { "Updated a note" } else { "Noted" },
                        s(&value["note"]["text"])
                    ),
                );
                let id = s(&value["note"]["id"]).to_string();
                let key = format!("note:{}:{}{id}", get("scope"), if value["pending"] == true { "pending:" } else { "" });
                let controller = self.0.options.controller.clone();
                self.keep(
                    key,
                    MemoryKind::Note,
                    s(&value["note"]["text"]),
                    Rc::new(move || {
                        let controller = controller.clone();
                        let id = id.clone();
                        async move { Ok(controller.forget(&id).await?.is_some()) }.boxed_local()
                    }),
                );
            }
            "forgot" => {
                self.memory_line(MemoryKind::Note, &format!("Forgot: {}", s(&value["note"]["text"])));
                self.forgotten(&format!("note:{}:{}", get("scope"), s(&value["note"]["id"])));
                self.forgotten(&format!("note:{}:pending:{}", get("scope"), s(&value["note"]["id"])));
            }
            "lesson" => {
                let key = format!("lesson:{}", get("id"));
                if get("action") == "forgot" {
                    self.memory_line(MemoryKind::Lesson, &format!("Forgot a lesson: {}", get("line")));
                    self.forgotten(&key);
                } else {
                    self.memory_line(
                        MemoryKind::Lesson,
                        &format!(
                            "{}: {}",
                            if get("action") == "updated" { "Updated what I learned" } else { "Learned from this match" },
                            get("line")
                        ),
                    );
                    let id = get("id").to_string();
                    let controller = self.0.options.controller.clone();
                    self.keep(
                        key,
                        MemoryKind::Lesson,
                        get("line"),
                        Rc::new(move || {
                            let controller = controller.clone();
                            let id = id.clone();
                            async move { controller.forget_lesson(&id).await }.boxed_local()
                        }),
                    );
                }
            }
            "technique" if get("action") == "offered" => {
                let name = s(&value["technique"]["name"]);
                let line = if self.offer_technique(name) {
                    format!("Keep this as a technique? {name}")
                } else {
                    format!("Keep this as a technique? {name} · say “keep the technique”")
                };
                self.memory_line(MemoryKind::Technique, &line);
            }
            "technique" => {
                let technique = &value["technique"];
                let name = s(&technique["name"]);
                self.memory_line(
                    MemoryKind::Technique,
                    &format!(
                        "{}: {name}",
                        match get("action") {
                            "kept" => "Kept a technique",
                            "updated" => "Updated a technique",
                            "used" => "Using your technique",
                            _ => "Forgot the technique",
                        }
                    ),
                );
                let key = format!("technique:{}", s(&technique["id"]));
                if matches!(get("action"), "kept" | "updated") {
                    let id = s(&technique["id"]).to_string();
                    let controller = self.0.options.controller.clone();
                    self.keep(
                        key,
                        MemoryKind::Technique,
                        name,
                        Rc::new(move || {
                            let controller = controller.clone();
                            let id = id.clone();
                            async move { controller.forget_technique(&id).await }.boxed_local()
                        }),
                    );
                } else if get("action") == "forgot" {
                    self.forgotten(&key);
                }
            }
            "recipe" => {
                let steps = format!("{} {}", number::to_string(n("steps")), if n("steps") == 1. { "step" } else { "steps" });
                let name = get("name");
                self.memory_line(
                    MemoryKind::Recipe,
                    &match get("action") {
                        "saved" => format!("Saved a recipe: {name} ({steps})"),
                        "updated" => format!("Updated a recipe: {name} ({steps})"),
                        "running" => format!("Running your recipe: {name} ({steps})"),
                        _ => format!("Forgot the recipe: {name}"),
                    },
                );
                let key = format!("recipe:{}", name.to_lowercase());
                if matches!(get("action"), "saved" | "updated") {
                    let name = name.to_string();
                    let name_copy = name.clone();
                    let controller = self.0.options.controller.clone();
                    self.keep(
                        key,
                        MemoryKind::Recipe,
                        &name_copy,
                        Rc::new(move || {
                            let controller = controller.clone();
                            let name = name.clone();
                            async move { controller.forget_recipe(&name).await }.boxed_local()
                        }),
                    );
                } else if get("action") == "forgotten" {
                    self.forgotten(&key);
                }
            }
            "action" => {
                let title = self.clean(get("title"), 120);
                let glyph = if value["recording"] == true {
                    "●"
                } else if value["playing"] == true {
                    "▶"
                } else if value["playing"] == false || value["recording"] == false {
                    "■"
                } else {
                    "›"
                };
                self.0.state.borrow_mut().last_action = Some(LastAction { title, at: perf_now(), glyph: glyph.into(), memory: false });
                self.flash();
            }
            "transport" => {
                self.0.state.borrow_mut().transport = serde_json::from_value(value["transport"].clone()).unwrap();
                self.next_beat();
            }
            "watching" => self.0.state.borrow_mut().watching = value["on"] == true,
            "heard" => {
                let mut entry = value.clone();
                entry["kind"] = json!("heard");
                entry["file"] = json!(self.clean(get("file"), 120));
                entry["summary"] = json!(self.clean(get("summary"), 200));
                if let Some(c) = entry.get_mut("compared") {
                    c["reference"] = json!(self.clean(s(&c["reference"]), 120));
                    c["summary"] = json!(self.clean(s(&c["summary"]), 200));
                }
                self.insert_before(serde_json::from_value(entry).unwrap());
            }
            "goal" => {
                let show = self.0.state.borrow().goal.is_none() || get("state") == "starting";
                if show {
                    self.0.tabs.show("goal");
                }
                let mut status = value.clone();
                status["since"] = json!(perf_now() - n("elapsedMs"));
                self.0.state.borrow_mut().goal = Some(status);
            }
            "match" => {
                self.0.state.borrow_mut().matching = if get("state") == "running" {
                    let mut status = value.clone();
                    status["since"] = json!(perf_now() - n("elapsedMs"));
                    Some(status)
                } else {
                    None
                };
                if get("state") == "done" && value.get("best").is_some() {
                    let score = value["best"]["score"].as_f64().unwrap_or(0.);
                    let first = value
                        .get("first")
                        .and_then(Value::as_f64)
                        .filter(|n| *n != score)
                        .map(|n| format!("{}% → ", number::to_string(n)))
                        .unwrap_or_default();
                    let why = match value["stop"].as_str().unwrap_or("plateau") {
                        "reached" => "close enough",
                        "budget" => "its budget spent",
                        "no-audition" => "nothing to compare",
                        _ => "no more gain",
                    };
                    self.notice(
                        &format!(
                            "Matching: {first}{}% ({}) · {} · {why}",
                            number::to_string(score),
                            s(&value["best"]["label"]),
                            helpers::clock_of(n("elapsedMs"))
                        ),
                        NoticeTone::Info,
                    );
                }
            }
            "objective" => {
                let mut status = value.clone();
                status["since"] = json!(perf_now() - n("elapsedMs"));
                self.0.state.borrow_mut().objective = Some(status);
                self.0.tabs.show("goal");
            }
            "loop" => {
                self.0.state.borrow_mut().looping = if get("state") == "running" {
                    let mut status = value.clone();
                    status["since"] = json!(perf_now() - n("elapsedMs"));
                    Some(status)
                } else {
                    None
                };
                if get("state") == "done" && n("rounds") > 0. {
                    let why = match value["stop"].as_str().unwrap_or("") {
                        "met" => "every item within tolerance",
                        "stalled" => "changes stopped helping",
                        "budget" => "its budget spent",
                        "ended" => "ended",
                        _ => "nothing more judged",
                    };
                    self.notice(
                        &format!(
                            "Loop: {} rounds · {} kept · {} taken back · {} listens · {} · {why}",
                            n("rounds"),
                            n("kept"),
                            n("reverted"),
                            n("listens"),
                            helpers::clock_of(n("elapsedMs"))
                        ),
                        NoticeTone::Info,
                    );
                }
            }
            "judged" => {
                if let SessionEvent::Judged(round) = &event {
                    let lines = round.lines().iter().map(|line| self.clean_line(line, 400)).collect();
                    self.insert_before(Entry::Judged { lines, kept: round.kept, met: round.met });
                }
            }
            "auditioned" => {
                let mut entry = value.clone();
                entry["kind"] = json!("auditioned");
                let gaps = array(&value["gaps"])
                    .iter()
                    .take(3)
                    .map(|v| {
                        let mut text = s(v);
                        if let Some((prefix, _)) = text.split_once(" (").filter(|_| text.ends_with(')')) {
                            text = prefix;
                        }
                        text = text.strip_suffix(" against the reference").unwrap_or(text);
                        self.clean_line(text, 48)
                    })
                    .collect::<Vec<_>>();
                entry["gaps"] = json!(gaps);
                if let Some(best) = entry.get_mut("best") {
                    best["label"] = json!(self.clean_line(s(&best["label"]), 60));
                }
                entry["takes"] = Value::Array(
                    array(&value["takes"])
                        .iter()
                        .take(8)
                        .map(|take| {
                            let mut take = take.clone();
                            take["label"] = json!(self.clean_line(s(&take["label"]), 40));
                            take
                        })
                        .collect(),
                );
                self.insert_before(serde_json::from_value(entry).unwrap());
            }
            "watched" => {
                let mut entry = value.clone();
                entry["kind"] = json!("watched");
                entry["title"] = json!(self.clean_line(get("title"), 160));
                if !get("channel").is_empty() {
                    entry["channel"] = json!(self.clean_line(get("channel"), 80));
                } else {
                    entry.as_object_mut().unwrap().remove("channel");
                }
                if n("duration") == 0. {
                    entry.as_object_mut().unwrap().remove("duration");
                }
                entry["chapters"] =
                    json!(array(&value["chapters"]).iter().take(24).map(|s| self.clean_line(super::events::s(s), 60)).collect::<Vec<_>>());
                entry["words"] = json!(match get("words") {
                    "captions" => "its captions",
                    "automatic" => "its automatic captions",
                    "transcribed" => "its speech, transcribed by Kumi",
                    _ => "no words",
                });
                entry["frames"] = Value::Array(
                    array(&value["frames"])
                        .iter()
                        .take(16)
                        .filter(|f| {
                            let t = &f["thumb"];
                            let w = t["width"].as_u64().unwrap_or(0);
                            let h = t["height"].as_u64().unwrap_or(0);
                            w > 0 && w <= 64 && h > 0 && h <= 64 && array(&t["rgb"]).len() as u64 == w * h * 3
                        })
                        .cloned()
                        .collect(),
                );
                entry["notes"] = json!(array(&value["notes"]).iter().map(|n| self.clean_line(s(n), 300)).collect::<Vec<_>>());
                entry["pictures"] = json!(matches!(self.0.depth, ColorDepth::Truecolor | ColorDepth::Colors256));
                self.insert_before(serde_json::from_value(entry).unwrap());
            }
            "web" => {
                if let SessionEvent::Web(event) = event {
                    let words = web_words(&event, &|text, max| {
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
                    let at =
                        state.current.as_ref().and_then(|current| state.transcript.entries.iter().position(|e| Rc::ptr_eq(e, current)));
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
            }
            "steer" => {
                let words = string::trim(&self.clean(get("text"), usize::MAX)).to_string();
                let mut state = self.0.state.borrow_mut();
                if let Some(at) = state.held.iter().position(|h| h.when == "now" && h.taken && h.text == get("text")) {
                    state.held.remove(at);
                }
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
            "doing" => {
                let text = self.clean_line(get("text"), 80);
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
            "tool-input" => {
                let mut state = self.0.state.borrow_mut();
                if state.current.is_some() && !state.suppress && get("name") == "make_changes" {
                    state.planning = Some(get("id").into());
                    state.planning_since = perf_now();
                }
            }
            "tool-start" => {
                let mut state = self.0.state.borrow_mut();
                if state.planning.as_deref() == Some(get("id")) {
                    state.planning = None;
                }
                if !state.suppress && !QUIET_TOOLS.contains(&get("name")) {
                    if let Some(entry) = &state.current {
                        if let Entry::Assistant { steps, .. } = &mut *entry.borrow_mut() {
                            steps.push(Step {
                                id: get("id").into(),
                                tool: Some(get("name").into()),
                                label: step_label(get("name")).into(),
                                state: StepState::Running,
                                started_at: Some(perf_now()),
                                ..Default::default()
                            });
                        }
                        state.transcript.touch(entry);
                    }
                }
            }
            "tool-end" => {
                let state = self.0.state.borrow();
                if let Some(entry) = &state.current {
                    let mut changed = false;
                    if let Entry::Assistant { steps, .. } = &mut *entry.borrow_mut() {
                        if let Some(step) = steps.iter_mut().find(|s| s.id == get("id")) {
                            step.state = if value["isError"] == true { StepState::Error } else { StepState::Done };
                            step.ms = Some(n("elapsedMs"));
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
            "turn-complete" => {
                let reason = s(&value["result"]["stopReason"]);
                let mut offered = None;
                {
                    let mut state = self.0.state.borrow_mut();
                    if let Some(usage) = value["result"].get("usage") {
                        state.used.input += usage["inputTokens"].as_f64().unwrap_or(0.);
                        state.used.output += usage["outputTokens"].as_f64().unwrap_or(0.);
                        state.used.cached += usage["cacheReadTokens"].as_f64().unwrap_or(0.);
                        state.used.answers += 1;
                    }
                    state.last_stop = Some(reason.into());
                    let Some(entry) = state.current.take() else {
                        self.0.scheduler.request();
                        return;
                    };
                    let cancelled = reason == "cancelled";
                    let tail = if !cancelled && !state.suppress {
                        state.stream.finish()
                    } else {
                        state.stream.discard();
                        String::new()
                    };
                    if let Entry::Assistant { text, status, elapsed_ms, .. } = &mut *entry.borrow_mut() {
                        text.push_str(&tail);
                        *status = if cancelled { AnswerStatus::Stopped } else { AnswerStatus::Done };
                        *elapsed_ms = Some(n("elapsedMs"));
                        // An answer that ends asking the producer to pick: one key answers, unless they've already
                        // typed or queued something.
                        if reason == "completed" && state.held.is_empty() && state.editor.is_empty() && state.panel.is_none() {
                            offered = choices(text);
                        }
                    }
                    end_steps(&entry);
                    state.transcript.touch(&entry);
                }
                if let Some(offered) = offered {
                    self.open_answers(offered);
                }
                if reason == "max-steps" {
                    self.notice("Kumi reached its step limit for one answer. Ask it to carry on.", NoticeTone::Info);
                }
            }
            _ => {}
        }
        if matches!(kind, "text" | "tool-start" | "tool-end" | "tool-input")
            && self.0.state.borrow().held.iter().any(|h| h.when == "now" && !h.taken)
        {
            self.steer_held();
        }
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
        state.last_action = Some(LastAction { title: head(&text, 120), at: perf_now(), glyph: what.glyph().into(), memory: true });
        drop(state);
        self.flash();
        self.0.scheduler.request();
    }
    fn keep(
        &self,
        key: String,
        what: MemoryKind,
        title: &str,
        forget: Rc<dyn Fn() -> LocalBoxFuture<'static, Result<bool, RuntimeError>>>,
    ) {
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
    fn forgotten(&self, key: &str) {
        let mut state = self.0.state.borrow_mut();
        let entries: Vec<_> = state.records.kept().iter().filter(|entry| entry.borrow().key == key).cloned().collect();
        for entry in entries {
            state.records.set_forgotten(&entry);
        }
    }
    pub(super) fn hold(&self, raw: String, when: &'static str) {
        self.0.state.borrow_mut().held.push(Held { text: raw, when, taken: false });
        if when == "now" {
            self.steer_held();
        }
        self.0.scheduler.request();
    }
    fn steer_held(&self) {
        if !self.0.options.controller.has_steer() {
            return;
        }
        let held = self.0.state.borrow().held.clone();
        for (index, item) in held.iter().enumerate() {
            if item.when == "now" && !item.taken {
                let taken = self.0.options.controller.steer(&item.text);
                if let Some(current) = self.0.state.borrow_mut().held.get_mut(index).filter(|h| h.text == item.text) {
                    current.taken = taken;
                }
            }
        }
    }
    fn after_busy(&self) {
        let next = {
            let mut state = self.0.state.borrow_mut();
            let back = state.last_stop.as_deref() == Some("cancelled") || state.failed || state.cancelling;
            state.last_stop = None;
            state.failed = false;
            if state.held.is_empty() || state.closing {
                return;
            }
            if back {
                let mut words = state.held.drain(..).map(|h| h.text).collect::<Vec<_>>();
                if !state.editor.is_empty() {
                    words.push(state.editor.text().into());
                }
                state.editor.set(&words.join("\n"));
                state.recall = None;
                return;
            }
            state.held.remove(0)
        };
        let words = string::trim(&self.clean(&next.text, usize::MAX)).to_string();
        let shown = {
            let mut state = self.0.state.borrow_mut();
            state.last_sent = Some(next.text.clone());
            state.scroll = 0;
            state.transcript.add(Entry::User { text: words })
        };
        self.task(move |app| async move {
            if app.send(&next.text).await || app.0.state.borrow().closing {
                return Ok(());
            }
            let mut state = app.0.state.borrow_mut();
            state.transcript.remove(&shown);
            let mut words = vec![next.text];
            words.extend(state.held.drain(..).map(|h| h.text));
            if !state.editor.is_empty() {
                words.push(state.editor.text().into());
            }
            state.editor.set(&words.join("\n"));
            drop(state);
            app.0.scheduler.request();
            Ok(())
        });
    }
}
fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
