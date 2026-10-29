//! A conversation's log: what happened in it, as it happened, appended to `<place>/conversations/<id>.jsonl` beside its
//! checkpoint. The checkpoint (`<id>.json`) is still written whole after each answer, so an earlier Kumi carries the
//! conversation on as before: it lists `.json` files only, and never sees the log.
//!
//! One JSON object a line, each with its time (ms since the epoch):
//! - `{"kind":"header","v":1,"id":…,"at":…,"kumi":…,"before":…}` first: `before` is how many requests the conversation
//!   had when its log began (an earlier Kumi's, or those of a log that filled up), and the messages it had then follow.
//! - `{"kind":"request","at":…,"text":…}`: what the producer asked, with the files they added and what they pointed at.
//! - `{"kind":"event","at":…,"event":{…}}`: what the session told the screen about the conversation. FOCUS, the
//!   transport, the link to the DAW and what a tool is doing this second are how things are now, not what happened: a
//!   conversation taken up reads them afresh. An answer's words go as one event until something else happens.
//! - `{"kind":"message","at":…,"message":{…}}`: each model message an answer settled into the conversation, as it went
//!   in (the checkpoint keeps only what the budget leaves of them).
//! - `{"kind":"saved","at":…,"savedAt":…,"turns":…}`: the checkpoint was kept as of `savedAt`.
//!
//! Readers skip kinds they don't know and lines that don't read: the last one may be cut short by a crash, and that's
//! never an error. The words in it (requests, answers, notices, errors) pass the redactor the app gives, which takes keys
//! out as the input history does; calls, results and what a provider replays stay as the checkpoint keeps them. Each line
//! goes to the file as soon as the session is free to write it, and the file is synced to the disk at each answer's end.
//! A log takes at most 32 MB: what would go past that isn't written, and the next request begins it again from what the
//! conversation had by then.

use crate::{
    core::{
        contracts::{KernelCheckpoint, KernelEvent, PinnedNode, SavedConversation, SessionEvent, TranscriptLine, TranscriptRole, TurnState},
        conversations::{is_conversation_id, write_privately},
    },
    kernel::budget::{transcript_of, OBSERVATION_MARKER},
};
use kumi_types::place::Places;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

/// The form of log this Kumi writes, and the latest it reads.
pub const LOG_VERSION: u32 = 1;
/// The most a conversation's log takes.
pub const MAX_LOG_BYTES: u64 = 32 * 1024 * 1024;

/// A conversation's log file, beside its checkpoint.
pub fn log_name(id: &str) -> String {
    format!("{id}.jsonl")
}

/// A log's first line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogHeader {
    pub v: u32,
    pub id: String,
    /// When the log began.
    pub at: i64,
    /// The Kumi that began it.
    pub kumi: String,
    /// How many requests the conversation had when the log began; its messages from then follow the header.
    #[serde(default)]
    pub before: u32,
    /// The conversation this one is a copy of (a fork, or one another Kumi window had open): that one's log as it was
    /// then follows the header instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// What the producer asked: their words, the files they added (by name) and what they pointed at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRequest {
    pub at: i64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<PinnedNode>,
}

/// A line of a log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum LogLine {
    Header(LogHeader),
    Request(LogRequest),
    Event { at: i64, event: SessionEvent },
    Message { at: i64, message: Value },
    Saved { at: i64, saved_at: i64, turns: u32 },
}

/// A log as read: its header, then every later line that reads, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadLog {
    pub header: LogHeader,
    pub lines: Vec<LogLine>,
}

/// The log in `bytes`; None when they aren't one this Kumi reads (no header first, or a later Kumi's form). What comes
/// after the last newline is a line cut short and is left out; a line that doesn't read, or is of a kind this Kumi
/// doesn't know, is skipped.
pub fn read_log(bytes: &[u8]) -> Option<ReadLog> {
    let whole = &bytes[..bytes.iter().rposition(|b| *b == b'\n')? + 1];
    let mut lines = whole.split(|b| *b == b'\n');
    let header = match serde_json::from_slice::<LogLine>(lines.next()?).ok()? {
        LogLine::Header(header) if (1..=LOG_VERSION).contains(&header.v) => header,
        _ => return None,
    };
    let lines = lines
        .filter_map(|line| match serde_json::from_slice::<LogLine>(line) {
            Ok(LogLine::Header(_)) | Err(_) => None,
            Ok(line) => Some(line),
        })
        .collect();
    Some(ReadLog { header, lines })
}

/// The log at `file`; None when there's none, or it isn't one this Kumi reads.
pub async fn read_log_file(file: &Path) -> Option<ReadLog> {
    read_log(&tokio::fs::read(file).await.ok()?)
}

/// The most requests a conversation taken up shows again.
pub const REPLAYED: usize = 30;

/// What a conversation taken up shows of its log: its last requests with everything that happened in them, and before
/// them, while there's room, the words of the requests it had when its log began (an earlier Kumi's).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Replay {
    /// How many of the conversation's requests aren't shown: those before the ones its log holds that there wasn't room
    /// for, and those the log holds before its last 30.
    pub before: u32,
    /// The words of the requests the conversation had when its log began, and their answers, as far as there's room: no
    /// more of them happened than that, as far as the log knows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier: Vec<TranscriptLine>,
    /// The log's requests and the conversation's events from the first request shown, in order.
    pub lines: Vec<LogLine>,
}

/// What of `log` a conversation taken up shows again: its requests and events from the `most`th request from the end
/// (all of them, when there are fewer). The model's messages and the saves are the kernel's, not the screen's, and an
/// earlier run's close isn't the conversation's.
pub fn replay_of(log: &ReadLog, most: usize) -> Replay {
    let told: Vec<&LogLine> = log
        .lines
        .iter()
        .filter(|line| match line {
            LogLine::Request(_) => true,
            LogLine::Event { event, .. } => !matches!(event, SessionEvent::State { state: TurnState::Closed }),
            _ => false,
        })
        .collect();
    let requests: Vec<usize> = told.iter().enumerate().filter(|(_, line)| matches!(line, LogLine::Request(_))).map(|(n, _)| n).collect();
    let left_out = requests.len().saturating_sub(most);
    let start = if left_out > 0 { requests[left_out] } else { 0 };
    // What the conversation had when its log began: the messages right after the header, before anything happened.
    let had: Vec<Value> = log
        .lines
        .iter()
        .map_while(|line| match line {
            LogLine::Message { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect();
    let words = transcript_of(&had);
    let asked: Vec<usize> = words.iter().enumerate().filter(|(_, line)| line.role == TranscriptRole::User).map(|(n, _)| n).collect();
    let shown = asked.len().min(most.saturating_sub(requests.len()));
    let earlier = if shown == 0 { vec![] } else { words[asked[asked.len() - shown]..].to_vec() };
    Replay {
        before: log.header.before.saturating_sub(shown as u32) + left_out as u32,
        earlier,
        lines: told[start..].iter().map(|line| (*line).clone()).collect(),
    }
}

/// Whether `log` is behind its conversation's checkpoint, kept with `turns` requests: something kept turns it never saw
/// end (an earlier Kumi, after a rollback), so it misses what came then. What it holds is its last save's count, and an
/// answer's end after it for each turn since (without a save, its count from when it began, and the ends since).
pub fn behind(log: &ReadLog, turns: u32) -> bool {
    let (mut known, mut ended) = (log.header.before, 0);
    for line in &log.lines {
        match line {
            LogLine::Saved { turns, .. } => (known, ended) = (*turns, 0),
            LogLine::Event { event: SessionEvent::TurnComplete { .. }, .. } => ended += 1,
            _ => {}
        }
    }
    turns > known + ended
}

fn now() -> i64 {
    kumi_common::time::now_ms()
}

/// Whether an event is part of the conversation's story. How things are now (FOCUS, the transport, the link, the
/// library and music being learned, what a tool is doing this second, the palette shown) is read afresh by a
/// conversation taken up; so is the account of the conversation a session gives when it takes one up, the log being
/// that conversation itself; what the screen only offers (a Set's question, a request to send again) isn't history; nor
/// is a side question's answer, which never enters the conversation.
fn told(event: &SessionEvent) -> bool {
    match event {
        SessionEvent::Focus { .. }
        | SessionEvent::Pointed { .. }
        | SessionEvent::Resumed { .. }
        | SessionEvent::SetAsked { .. }
        | SessionEvent::Resend { .. }
        | SessionEvent::Connection { .. }
        | SessionEvent::Observation { .. }
        | SessionEvent::Library(_)
        | SessionEvent::Music(_)
        | SessionEvent::Doing { .. }
        | SessionEvent::Transport { .. }
        | SessionEvent::Watching { .. }
        | SessionEvent::Palette { .. }
        | SessionEvent::Aside { .. } => false,
        SessionEvent::Change { .. }
        | SessionEvent::CatchUp { .. }
        | SessionEvent::State { .. }
        | SessionEvent::Notice { .. }
        | SessionEvent::Error { .. }
        | SessionEvent::TurnComplete { .. }
        | SessionEvent::Lesson { .. }
        | SessionEvent::Match(_)
        | SessionEvent::Goal(_)
        | SessionEvent::Objective(_)
        | SessionEvent::ObjectiveCleared
        | SessionEvent::Heard(_)
        | SessionEvent::Auditioned(_)
        | SessionEvent::Judged(_)
        | SessionEvent::Loop(_)
        | SessionEvent::Watched(_)
        | SessionEvent::Web(_)
        | SessionEvent::Recipe(_)
        | SessionEvent::Technique(_)
        | SessionEvent::Action(_)
        | SessionEvent::Job(_)
        | SessionEvent::CarryOn { .. }
        | SessionEvent::Kernel(_)
        | SessionEvent::Memory(_) => true,
    }
}

/// An event with its words through the redactor: a notice's or an error's, and the producer's words steering an answer.
fn redacted_event(event: &SessionEvent, redact: &dyn Fn(&str) -> String) -> SessionEvent {
    match event {
        SessionEvent::Notice { message } => SessionEvent::Notice { message: redact(message) },
        SessionEvent::Error { message, kind, provider } => {
            SessionEvent::Error { message: redact(message), kind: *kind, provider: provider.clone() }
        }
        SessionEvent::Kernel(KernelEvent::Steer { text }) => SessionEvent::Kernel(KernelEvent::Steer { text: redact(text) }),
        other => other.clone(),
    }
}

/// A model message with its words through the redactor: the producer's (not the DAW's observation Kumi adds after them)
/// and the model's. Calls, results, reasoning and what a provider replays stay as they are.
fn redacted_message(message: &Value, redact: &dyn Fn(&str) -> String) -> Value {
    let user = match message.get("role").and_then(Value::as_str) {
        Some("user") => true,
        Some("assistant") => false,
        _ => return message.clone(),
    };
    let words = |text: &str| match text.find(OBSERVATION_MARKER).filter(|_| user) {
        Some(at) => format!("{}{}", redact(&text[..at]), &text[at..]),
        None => redact(text),
    };
    let mut message = message.clone();
    match message.get_mut("content") {
        Some(Value::String(text)) => *text = words(text),
        Some(Value::Array(parts)) => {
            for part in parts.iter_mut().filter(|part| part.get("type").and_then(Value::as_str) == Some("text")) {
                if let Some(Value::String(text)) = part.get_mut("text") {
                    *text = words(text);
                }
            }
        }
        _ => {}
    }
    message
}

fn line_of(line: &LogLine) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(line).expect("a log line serializes");
    bytes.push(b'\n');
    bytes
}

/// How a conversation's log begins when it has none: with the requests and messages the conversation had by then, or as
/// a copy of the log of the conversation it copies.
struct Begin {
    /// How many requests the conversation had (as kept, for one taken up).
    before: u32,
    /// Its messages by then: its checkpoint as kept when it was taken up, or as it was at a request.
    had: Option<Rc<KernelCheckpoint>>,
    from: Option<String>,
    /// When the checkpoint of a conversation taken up was kept: a log whose last save is older, with no answer ended
    /// since, is behind it.
    kept_at: Option<i64>,
}

/// What a copy takes of the original's log: its lines up to the end of its last save as the copy began, which is what
/// the copy's checkpoint holds, from the log begun at `at` (one begun again since is another).
#[derive(Debug, Clone, Copy)]
struct Upto {
    end: u64,
    at: i64,
}

/// What the writer is asked, in order.
enum Job {
    /// The conversation going on from here.
    Open { folder: PathBuf, id: String, begin: Begin },
    /// A line for the log.
    Line(Vec<u8>),
    /// A request's line: a full log begins again with it.
    Request { line: Vec<u8>, begin: Begin },
    /// Conversation `id`'s checkpoint was kept: a log waiting for it begins (a copy's from `from`'s).
    Saved { id: String, line: Vec<u8>, from: Option<String> },
    /// The conversation moved to another place (an unsaved Set's, once the Set was saved).
    Moved { folder: PathBuf },
    /// What's written goes to the disk.
    Sync,
    /// Says when everything before it is written.
    Written(oneshot::Sender<()>),
    /// The session is closing: what's written goes to the disk, and nothing more is written.
    Close(oneshot::Sender<()>),
}

/// The log of the conversation going on, as the session sees it; the writer beside the session does the rest.
pub struct SessionLog {
    directory: PathBuf,
    places: Places,
    redact: Rc<dyn Fn(&str) -> String>,
    /// The conversation going on: its folder and id.
    going: RefCell<Option<(PathBuf, String)>>,
    /// An answer's words so far, from when they began: written as one event once something else happens, so the
    /// redactor sees a key whole and a long answer isn't a line for every few words.
    words: RefCell<Option<(i64, String)>>,
    closed: Cell<bool>,
    cap: Rc<Cell<u64>>,
    /// The log of the conversation going on, and how much of it there was when it was opened (none: there was none).
    opened: Rc<RefCell<Option<(PathBuf, u64)>>>,
    jobs: RefCell<Option<mpsc::UnboundedSender<Job>>>,
}

impl SessionLog {
    /// The logs of the conversations kept under `directory` in these DAWs' places, their words through `redact`.
    pub fn new(directory: impl Into<PathBuf>, places: Places, redact: Rc<dyn Fn(&str) -> String>) -> Rc<SessionLog> {
        Rc::new(SessionLog {
            directory: directory.into(),
            places,
            redact,
            going: RefCell::default(),
            words: RefCell::default(),
            closed: Cell::new(false),
            cap: Rc::new(Cell::new(MAX_LOG_BYTES)),
            opened: Rc::default(),
            jobs: RefCell::default(),
        })
    }

    /// `on_event` with every event logged on its way: what the session says and what the DAW does go through the one
    /// function, in the order the screen gets them.
    pub fn tee(log: &Rc<SessionLog>, on_event: Rc<dyn Fn(SessionEvent)>) -> Rc<dyn Fn(SessionEvent)> {
        let log = log.clone();
        Rc::new(move |event| {
            log.event(&event);
            on_event(event)
        })
    }

    /// Holds a log to `bytes` instead of 32 MB: for tests.
    #[doc(hidden)]
    pub fn set_cap(&self, bytes: u64) {
        self.cap.set(bytes);
    }

    fn open_now(&self) -> bool {
        !self.closed.get() && self.going.borrow().is_some()
    }

    fn send(&self, job: Job) {
        let mut jobs = self.jobs.borrow_mut();
        let jobs = jobs.get_or_insert_with(|| {
            let (send, receive) = mpsc::unbounded_channel();
            let kit = Kit { redact: self.redact.clone(), cap: self.cap.clone() };
            tokio::task::spawn_local(write(receive, Writer { kit, log: None, left: vec![], opened: self.opened.clone() }));
            send
        });
        let _ = jobs.send(job);
    }

    /// The answer's words so far, written.
    fn said(&self) {
        let Some((at, text)) = self.words.borrow_mut().take() else { return };
        let event = SessionEvent::Kernel(KernelEvent::Text { text: (self.redact)(&text) });
        self.send(Job::Line(line_of(&LogLine::Event { at, event })));
    }

    fn line(&self, line: &LogLine) {
        self.said();
        self.send(Job::Line(line_of(line)));
    }

    /// An event the screen is sent, logged when it's part of the conversation's story.
    pub fn event(&self, event: &SessionEvent) {
        if !self.open_now() || !told(event) {
            return;
        }
        if let SessionEvent::Kernel(KernelEvent::Text { text }) = event {
            self.words.borrow_mut().get_or_insert_with(|| (now(), String::new())).1.push_str(text);
            return;
        }
        self.line(&LogLine::Event { at: now(), event: redacted_event(event, self.redact.as_ref()) });
        // The answer's end goes to the disk with everything before it.
        if matches!(event, SessionEvent::TurnComplete { .. }) {
            self.send(Job::Sync);
        }
    }

    /// The conversation `id` in `place` goes on from here, `kept` as it was kept when it's taken up. Without a log yet, its
    /// log begins once it's kept: with the requests and messages it had by then, or (`from`) as a copy of that
    /// conversation's log as it is now. One whose log is behind it (an earlier Kumi carried it on without the log) begins
    /// its log again from what's kept.
    pub fn open(&self, place: &str, id: &str, kept: Option<&SavedConversation>, from: Option<String>) {
        if self.closed.get() {
            return;
        }
        let folder = self.folder(place).filter(|_| is_conversation_id(id));
        let going = folder.map(|folder| (folder, id.to_owned()));
        if *self.going.borrow() == going {
            return;
        }
        // Words still unwritten are the last conversation's.
        self.said();
        *self.going.borrow_mut() = going.clone();
        if let Some((folder, id)) = going {
            let begin = Begin {
                before: kept.and_then(|kept| kept.turns).unwrap_or(0),
                had: kept.map(|kept| Rc::new(kept.checkpoint.clone())),
                from,
                kept_at: kept.map(|kept| kept.saved_at),
            };
            self.send(Job::Open { folder, id, begin });
        }
    }

    fn folder(&self, place: &str) -> Option<PathBuf> {
        self.places.parse(place).map(|_| self.directory.join(place).join("conversations"))
    }

    /// The conversation going on moved to `place`, its log with it.
    pub fn moved(&self, place: &str) {
        let Some(folder) = self.folder(place) else { return };
        let mut going = self.going.borrow_mut();
        let Some((was, _)) = going.as_mut().filter(|(was, _)| *was != folder) else { return };
        *was = folder.clone();
        drop(going);
        self.send(Job::Moved { folder });
    }

    /// A request the producer made: a full log begins again with it, from the `before` requests and the messages `had`
    /// the conversation had by then.
    pub fn request(&self, text: &str, files: Vec<String>, pinned: Option<PinnedNode>, before: u32, had: Option<Rc<KernelCheckpoint>>) {
        if !self.open_now() {
            return;
        }
        self.said();
        let line = line_of(&LogLine::Request(LogRequest { at: now(), text: (self.redact)(text), files, pinned }));
        self.send(Job::Request { line, begin: Begin { before, had, from: None, kept_at: None } });
    }

    /// What an answer settled into the conversation, message by message.
    pub fn messages(&self, messages: &[Value]) {
        if !self.open_now() {
            return;
        }
        let at = now();
        for message in messages {
            self.line(&LogLine::Message { at, message: redacted_message(message, self.redact.as_ref()) });
        }
    }

    /// Conversation `id`'s checkpoint was kept as of `saved_at`, with `turns` requests: its log says so, and one that
    /// waited for it begins (a copy's, from the log of `from`, the conversation it copies), the conversation going on or
    /// one left before its first save landed. (An id is a conversation's in every place: one that moved is still it.)
    pub fn saved(&self, id: &str, saved_at: i64, turns: u32, from: Option<String>) {
        if self.closed.get() {
            return;
        }
        self.said();
        self.send(Job::Saved { id: id.to_owned(), line: line_of(&LogLine::Saved { at: now(), saved_at, turns }), from });
        self.send(Job::Sync);
    }

    /// Once everything given so far is in the file (written, not yet synced): what reads the log of the conversation
    /// going on waits for this first.
    pub async fn written(&self) {
        self.said();
        let (done, finished) = oneshot::channel();
        let sent = self.jobs.borrow().as_ref().is_some_and(|jobs| jobs.send(Job::Written(done)).is_ok());
        if sent {
            let _ = finished.await;
        }
    }

    /// The log of the conversation going on as it was when it was opened, before anything this run wrote in it: what the
    /// conversation shows again as it's taken up. None without one (a conversation new, or kept before there were logs).
    pub async fn as_opened(&self) -> Option<ReadLog> {
        self.written().await;
        let (file, had) = self.opened.borrow().clone()?;
        if had == 0 {
            return None;
        }
        let mut bytes = tokio::fs::read(&file).await.ok()?;
        bytes.truncate(usize::try_from(had).ok()?);
        read_log(&bytes)
    }

    /// What's been said goes to the disk, and nothing more is written: the session is closing.
    pub async fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        self.said();
        let (done, finished) = oneshot::channel();
        let sent = self.jobs.borrow().as_ref().is_some_and(|jobs| jobs.send(Job::Close(done)).is_ok());
        if sent {
            let _ = finished.await;
        }
    }
}

/// What every log is written with.
struct Kit {
    redact: Rc<dyn Fn(&str) -> String>,
    cap: Rc<Cell<u64>>,
}

/// The writer beside the session: one job at a time, in the order the session gave them.
struct Writer {
    kit: Kit,
    /// The log of the conversation going on.
    log: Option<Log>,
    /// The conversations left last, the newest last: a save of one that lands after it was left (a /new straight after
    /// an answer) still goes in its log, or begins it with the lines that waited for it.
    left: Vec<Log>,
    /// The log opened last, and how much of it there was then.
    opened: Rc<RefCell<Option<(PathBuf, u64)>>>,
}

/// A conversation's log as it's written.
struct Log {
    folder: PathBuf,
    id: String,
    begin: Begin,
    /// For a copy, what it takes of the original's log.
    upto: Option<Upto>,
    state: Written,
    /// A line was left out to keep within the cap: nothing more goes in until a request begins the log again.
    full: bool,
    /// Written to since it was last synced.
    dirty: bool,
}

enum Written {
    /// The conversation isn't kept yet, nor has a log: its lines wait for its first save, in order.
    Waiting(Vec<u8>),
    /// Lines go on its end, `size` bytes in, after a line cut short (`torn`, by a crash) that the next mustn't join.
    Appending { size: u64, torn: bool },
    /// A later Kumi's log, in a form this one doesn't write: nothing goes in it.
    Theirs,
}

/// The most conversations left that a late save still finds.
const LEFT: usize = 8;

async fn write(mut jobs: mpsc::UnboundedReceiver<Job>, mut writer: Writer) {
    let mut next = None;
    loop {
        let job = match next.take() {
            Some(job) => job,
            None => match jobs.recv().await {
                Some(job) => job,
                None => break,
            },
        };
        let kit = &writer.kit;
        match job {
            Job::Line(line) => {
                // Lines given meanwhile go with it, in one write.
                let mut lines = vec![line];
                loop {
                    match jobs.try_recv() {
                        Ok(Job::Line(line)) => lines.push(line),
                        Ok(other) => {
                            next = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                if let Some(log) = writer.log.as_mut() {
                    log.put(kit, lines).await;
                }
            }
            Job::Open { folder, id, begin } => {
                writer.leave().await;
                // Taken up again soon after it was left: the lines it waits with are its own still, and one written is
                // read again (another window may have written in it since).
                let log = match writer.left.iter().position(|log| log.id == id) {
                    Some(at) => {
                        let left = writer.left.remove(at);
                        let from = begin.from.or_else(|| left.begin.from.clone());
                        Log { folder, begin: Begin { from, ..begin }, ..left }.again_as_left()
                    }
                    None => {
                        let upto = match &begin.from {
                            Some(from) if is_conversation_id(from) => upto(&folder.join(log_name(from))).await,
                            _ => None,
                        };
                        Log { folder, id, begin, upto, state: Written::Waiting(vec![]), full: false, dirty: false }
                    }
                };
                let kit = &writer.kit;
                let log = writer.log.insert(log);
                let had = log.start(kit).await;
                *writer.opened.borrow_mut() = Some((log.file(), had.unwrap_or(0)));
            }
            Job::Request { line, begin } => {
                if let Some(log) = writer.log.as_mut() {
                    log.again(kit, begin).await;
                    log.put(kit, vec![line]).await;
                }
            }
            Job::Saved { id, line, from } => {
                let log = match writer.log.as_mut().filter(|log| log.id == id) {
                    Some(log) => log,
                    None => match writer.left.iter().position(|log| log.id == id) {
                        Some(at) => {
                            let mut log = writer.left.remove(at).again_as_left();
                            log.saved(kit, line, from).await;
                            log.sync().await;
                            writer.left.push(log);
                            continue;
                        }
                        // Not a conversation this run writes.
                        None => continue,
                    },
                };
                log.saved(kit, line, from).await;
            }
            Job::Moved { folder } => {
                if let Some(log) = writer.log.as_mut() {
                    log.moved(kit, folder).await;
                }
            }
            Job::Sync => {
                if let Some(log) = writer.log.as_mut() {
                    log.sync().await;
                }
            }
            Job::Written(done) => {
                let _ = done.send(());
            }
            Job::Close(done) => {
                // A first save may have landed since its log was last looked for (the session waits for its saves).
                writer.leave().await;
                for mut log in std::mem::take(&mut writer.left) {
                    log.start(&writer.kit).await;
                    log.sync().await;
                }
                let _ = done.send(());
                break;
            }
        }
    }
}

impl Writer {
    /// The conversation going on is left: what's written goes to the disk, and one not kept yet keeps its lines waiting
    /// for its first save, which may still land.
    async fn leave(&mut self) {
        let Some(mut log) = self.log.take() else { return };
        log.start(&self.kit).await;
        log.sync().await;
        self.left.push(log);
        if self.left.len() > LEFT {
            self.left.remove(0);
        }
    }
}

impl Log {
    fn file(&self) -> PathBuf {
        self.folder.join(log_name(&self.id))
    }

    /// A log left a while ago, written to again: one this run was writing is read again first (its size, how it ends),
    /// and one still waiting keeps its lines.
    fn again_as_left(mut self) -> Log {
        if matches!(self.state, Written::Appending { .. }) {
            self.state = Written::Waiting(vec![]);
        }
        self
    }

    /// A log that waited begins, once its conversation is kept: one there already (an earlier session's) goes on,
    /// unless it's behind what's kept; otherwise it's made. The size of the one that went on, when one did.
    async fn start(&mut self, kit: &Kit) -> Option<u64> {
        if !matches!(self.state, Written::Waiting(_)) {
            return None;
        }
        if !tokio::fs::try_exists(self.folder.join(format!("{}.json", self.id))).await.unwrap_or(false) {
            return None;
        }
        let Written::Waiting(waiting) = std::mem::replace(&mut self.state, Written::Theirs) else { return None };
        let file = self.file();
        match found(&file).await {
            Ok(Found::Ours { size, torn, at }) => {
                // Kept since without it (an earlier Kumi, after a rollback): begun again from what's kept, so it never
                // skips what happened then.
                let behind = match self.begin.kept_at {
                    Some(kept) => last_save(&file, size, at).await.is_some_and(|(saved, ended)| kept > saved && !ended),
                    None => false,
                };
                if behind {
                    self.make(kit, waiting).await;
                    return None;
                }
                self.state = Written::Appending { size, torn };
                self.put(kit, split(waiting)).await;
                Some(size)
            }
            Ok(Found::Theirs) => None,
            Ok(Found::Unreadable) => {
                self.make(kit, waiting).await;
                None
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.make(kit, waiting).await;
                None
            }
            // Not to be read now (its permissions, a scanner holding it): left as it is, the lines wait for the next save.
            Err(_) => {
                self.state = Written::Waiting(waiting);
                None
            }
        }
    }

    /// Its checkpoint was kept: one that waited for that begins (a copy's from `from`'s log as it was when the copy
    /// began, or as it is now for one that's named only now), and its line goes on.
    async fn saved(&mut self, kit: &Kit, line: Vec<u8>, from: Option<String>) {
        if matches!(self.state, Written::Waiting(_)) && self.begin.from.is_none() {
            if let Some(from) = from.filter(|from| is_conversation_id(from)) {
                self.upto = upto(&self.folder.join(log_name(&from))).await;
                self.begin.from = Some(from);
            }
        }
        self.start(kit).await;
        self.put(kit, vec![line]).await;
    }

    /// The log made new: its header, what the conversation had (or what a copy takes of the original's log), then
    /// `after`.
    async fn make(&mut self, kit: &Kit, after: Vec<u8>) {
        let cap = kit.cap.get();
        let mut header = LogHeader {
            v: LOG_VERSION,
            id: self.id.clone(),
            at: now(),
            kumi: kumi_types::version::KUMI_VERSION.into(),
            before: self.begin.before,
            from: None,
        };
        let copied = match (&self.begin.from, self.upto) {
            (Some(from), Some(upto)) => copy_of(&self.folder.join(log_name(from)), upto).await,
            _ => None,
        };
        let mut had = vec![];
        match copied {
            Some((before, lines)) => {
                header.before = before;
                header.from = self.begin.from.clone();
                had.push(lines);
            }
            None => {
                for message in self.begin.had.iter().flat_map(|kept| &kept.messages) {
                    had.push(line_of(&LogLine::Message { at: header.at, message: redacted_message(message, kit.redact.as_ref()) }));
                }
            }
        }
        let mut bytes = line_of(&LogLine::Header(header));
        let mut full = false;
        for line in had.into_iter().chain(split(after.clone())) {
            if full || (bytes.len() + line.len()) as u64 > cap {
                full = true;
                continue;
            }
            bytes.extend(line);
        }
        let text = String::from_utf8(bytes).expect("log lines are JSON");
        match write_privately(&self.folder, &log_name(&self.id), &text).await {
            Ok(()) => {
                self.state = Written::Appending { size: text.len() as u64, torn: false };
                self.full = full;
                self.dirty = true;
            }
            // Kept waiting: the next save tries again.
            Err(_) => self.state = Written::Waiting(after),
        }
    }

    /// A full log begins again with the request now made, from what the conversation had by then.
    async fn again(&mut self, kit: &Kit, begin: Begin) {
        if !self.full {
            return;
        }
        self.begin = begin;
        self.upto = None;
        self.full = false;
        let made = match &mut self.state {
            Written::Waiting(waiting) => {
                waiting.clear();
                false
            }
            Written::Appending { .. } => true,
            Written::Theirs => false,
        };
        if made {
            self.make(kit, vec![]).await;
        }
    }

    /// Lines on the log's end, as many as fit within the cap: from the first that doesn't, none until the log begins
    /// again.
    async fn put(&mut self, kit: &Kit, lines: Vec<Vec<u8>>) {
        let cap = kit.cap.get();
        if self.full {
            return;
        }
        let (size, torn) = match &mut self.state {
            Written::Theirs => return,
            Written::Waiting(waiting) => {
                for line in lines {
                    if (waiting.len() + line.len()) as u64 > cap {
                        self.full = true;
                        return;
                    }
                    waiting.extend(line);
                }
                return;
            }
            Written::Appending { size, torn } => (*size, *torn),
        };
        // A line cut short by a crash ends here, so the next one starts on its own.
        let mut bytes = if torn { vec![b'\n'] } else { vec![] };
        for line in lines {
            if size + (bytes.len() + line.len()) as u64 > cap {
                self.full = true;
                break;
            }
            bytes.extend(line);
        }
        if bytes.len() <= usize::from(torn) {
            return;
        }
        let file = self.file();
        match append(&file, &bytes).await {
            Ok(()) => {
                self.state = Written::Appending { size: size + bytes.len() as u64, torn: false };
                self.dirty = true;
            }
            // Gone from under it: made again from what the conversation had, with these lines.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.make(kit, bytes[usize::from(torn)..].to_vec()).await,
            // Whatever of it went in may end on a line cut short; its size is read again.
            Err(_) => {
                let size = tokio::fs::metadata(&file).await.map(|m| m.len()).unwrap_or(size);
                self.state = Written::Appending { size, torn: true };
                self.dirty = true;
            }
        }
    }

    async fn moved(&mut self, kit: &Kit, folder: PathBuf) {
        let from = self.file();
        self.folder = folder;
        if matches!(self.state, Written::Waiting(_)) {
            // Kept there already, it begins there.
            self.start(kit).await;
            return;
        }
        let to = self.file();
        let mut made = tokio::fs::DirBuilder::new();
        made.recursive(true);
        #[cfg(unix)]
        made.mode(0o700);
        let moved = made.create(&self.folder).await.is_ok() && tokio::fs::rename(&from, &to).await.is_ok();
        if !moved {
            // Left behind, it goes to its conversation's place with the store's next look (see its trim); lines go on a
            // new one here meanwhile.
            self.state = Written::Waiting(vec![]);
            self.start(kit).await;
        }
    }

    async fn sync(&mut self) {
        if !self.dirty {
            return;
        }
        // Opened to write (nothing's written through it): Windows syncs only a file opened so, not one opened to append.
        if let Ok(handle) = tokio::fs::OpenOptions::new().write(true).open(self.file()).await {
            let _ = handle.sync_data().await;
        }
        self.dirty = false;
    }
}

/// What a log on disk is to this Kumi: its own form (its size, whether it ends on a line cut short, and when it began),
/// a later Kumi's, or none it can read.
enum Found {
    Ours { size: u64, torn: bool, at: i64 },
    Theirs,
    Unreadable,
}

/// How a later Kumi's header begins, whatever else it says: its version is read first.
#[derive(Deserialize)]
struct Form {
    kind: String,
    #[serde(default)]
    v: u64,
}

static LATER: std::sync::LazyLock<regex::bytes::Regex> =
    std::sync::LazyLock::new(|| regex::bytes::Regex::new(r#"^\{"kind":"header","v":([0-9]{1,9})[,}]"#).unwrap());

async fn found(file: &Path) -> std::io::Result<Found> {
    let mut handle = tokio::fs::File::open(file).await?;
    let size = handle.metadata().await?.len();
    let mut head = Vec::with_capacity(4096);
    (&mut handle).take(64 * 1024).read_to_end(&mut head).await?;
    let Some(end) = head.iter().position(|b| *b == b'\n') else {
        // A first line that never ends is most likely cut short (or zeros, after a power cut): made again, unless it
        // begins as a later Kumi's header.
        let later = LATER.captures(&head).and_then(|v| std::str::from_utf8(&v[1]).ok()?.parse::<u64>().ok());
        return Ok(if later.is_some_and(|v| v > u64::from(LOG_VERSION)) { Found::Theirs } else { Found::Unreadable });
    };
    match serde_json::from_slice::<Form>(&head[..end]) {
        Ok(form) if form.kind == "header" && form.v > u64::from(LOG_VERSION) => return Ok(Found::Theirs),
        Ok(form) if form.kind == "header" && form.v >= 1 => {}
        _ => return Ok(Found::Unreadable),
    }
    let Ok(LogLine::Header(header)) = serde_json::from_slice::<LogLine>(&head[..end]) else { return Ok(Found::Unreadable) };
    handle.seek(std::io::SeekFrom::End(-1)).await?;
    let mut last = [0u8];
    handle.read_exact(&mut last).await?;
    Ok(Found::Ours { size, torn: last[0] != b'\n', at: header.at })
}

const SAVED: &[u8] = br#"{"kind":"saved","#;
const COMPLETED: &[u8] = br#""event":{"type":"turn-complete""#;

/// When a log's last save was kept (when the log began, without one), and whether an answer ended after it: one that
/// did is a turn whose save the log missed (Kumi stopped right after it), not one kept without the log. Read from its
/// last MB, or all of it when that holds no save. (Saves are told apart by when, not by their counts: a checkpoint
/// counts each of the producer's lines, a steer's too, where the log counts requests.)
async fn last_save(file: &Path, size: u64, began: i64) -> Option<(i64, bool)> {
    const TAIL: u64 = 1024 * 1024;
    let mut handle = tokio::fs::File::open(file).await.ok()?;
    let from = size.saturating_sub(TAIL);
    handle.seek(std::io::SeekFrom::Start(from)).await.ok()?;
    let mut bytes = vec![];
    handle.read_to_end(&mut bytes).await.ok()?;
    let mut found = saves_in(&bytes, from > 0);
    if found.0.is_none() && from > 0 {
        found = saves_in(&tokio::fs::read(file).await.ok()?, false);
    }
    let (saved, ended) = found;
    Some((saved.unwrap_or(began), ended))
}

/// When the last save in these lines was kept, and whether an answer ended after it (or in all of them, without one). A
/// first line cut where they begin (`cut`) isn't read.
fn saves_in(bytes: &[u8], cut: bool) -> (Option<i64>, bool) {
    let mut lines = bytes.split(|b| *b == b'\n');
    if cut {
        lines.next();
    }
    let (mut saved, mut ended) = (None, false);
    for line in lines {
        if line.starts_with(SAVED) {
            if let Ok(LogLine::Saved { saved_at, .. }) = serde_json::from_slice(line) {
                saved = Some(saved_at);
                ended = false;
            }
        } else if line.windows(COMPLETED.len()).any(|window| window == COMPLETED) {
            ended = true;
        }
    }
    (saved, ended)
}

async fn append(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut handle = tokio::fs::OpenOptions::new().append(true).open(file).await?;
    handle.write_all(bytes).await?;
    // The write is the system's once it's flushed: a crash of Kumi from here loses none of it.
    handle.flush().await
}

/// Bytes of whole lines as lines.
fn split(bytes: Vec<u8>) -> Vec<Vec<u8>> {
    bytes.split_inclusive(|b| *b == b'\n').map(<[u8]>::to_vec).collect()
}

/// What a copy beginning now would take of the original's log at `file`: up to the end of its last save. None without
/// a save in it (nothing a copy's checkpoint holds that its own messages don't), or a log this Kumi reads.
async fn upto(file: &Path) -> Option<Upto> {
    let bytes = tokio::fs::read(file).await.ok()?;
    let first = bytes.iter().position(|b| *b == b'\n')?;
    let LogLine::Header(header) = serde_json::from_slice::<LogLine>(&bytes[..first]).ok()? else { return None };
    if !(1..=LOG_VERSION).contains(&header.v) {
        return None;
    }
    let mut end = None;
    let mut at = first + 1;
    for line in bytes[first + 1..].split_inclusive(|b| *b == b'\n') {
        at += line.len();
        if line.starts_with(SAVED) && line.ends_with(b"\n") {
            end = Some(at);
        }
    }
    Some(Upto { end: end? as u64, at: header.at })
}

/// The lines of the original's log at `file` after its header, to `upto`'s end, with its count of requests from before
/// it began: what a copy's log begins with. None when it's no longer that log (begun again since), or can't be read.
async fn copy_of(file: &Path, upto: Upto) -> Option<(u32, Vec<u8>)> {
    let bytes = tokio::fs::read(file).await.ok()?;
    let first = bytes.iter().position(|b| *b == b'\n')?;
    let LogLine::Header(header) = serde_json::from_slice::<LogLine>(&bytes[..first]).ok()? else { return None };
    let end = usize::try_from(upto.end).ok()?;
    if header.at != upto.at || end > bytes.len() || end <= first {
        return None;
    }
    Some((header.before, bytes[first + 1..end].to_vec()))
}
