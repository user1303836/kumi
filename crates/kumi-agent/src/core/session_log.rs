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
        contracts::{KernelCheckpoint, KernelEvent, PinnedNode, SessionEvent},
        conversations::{is_conversation_id, write_privately},
    },
    kernel::budget::OBSERVATION_MARKER,
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

fn now() -> i64 {
    kumi_common::time::now_ms()
}

/// Whether an event is part of the conversation's story. How things are now (FOCUS, the transport, the link, the
/// library and music being learned, what a tool is doing this second, the palette shown) is read afresh by a
/// conversation taken up; so is the account of the conversation a session gives when it takes one up, the log being
/// that conversation itself; and what the screen only offers (a Set's question, a request to send again) isn't history.
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
        | SessionEvent::Palette { .. } => false,
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
    before: u32,
    had: Option<Rc<KernelCheckpoint>>,
    from: Option<String>,
}

/// What the writer is asked, in order.
enum Job {
    /// The conversation going on from here.
    Open { folder: PathBuf, id: String, begin: Begin },
    /// A line for the log.
    Line(Vec<u8>),
    /// A request's line: a full log begins again with it.
    Request { line: Vec<u8>, begin: Begin },
    /// The checkpoint was kept: a log waiting for it begins (a copy's from `from`'s).
    Saved { line: Vec<u8>, from: Option<String> },
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
            tokio::task::spawn_local(write(receive, Writer { redact: self.redact.clone(), cap: self.cap.clone(), log: None }));
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

    /// The conversation `id` in `place` goes on from here. Without a log yet, its log begins once it's kept: with the
    /// `before` requests and the messages `had` it had by then, or (`from`) as a copy of that conversation's log.
    pub fn open(&self, place: &str, id: &str, before: u32, had: Option<Rc<KernelCheckpoint>>, from: Option<String>) {
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
            self.send(Job::Open { folder, id, begin: Begin { before, had, from } });
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
        self.send(Job::Request { line, begin: Begin { before, had, from: None } });
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

    /// Conversation `id`'s checkpoint was kept as of `saved_at`, with `turns` requests: a log that waited for it begins (a
    /// copy's, from the log of `from`, the conversation it copies). Kept after another conversation began, it's that
    /// one's log no longer. (An id is the conversation's in every place: one that moved since is still the one.)
    pub fn saved(&self, id: &str, saved_at: i64, turns: u32, from: Option<String>) {
        if !self.open_now() || !self.going.borrow().as_ref().is_some_and(|(_, going)| going == id) {
            return;
        }
        self.said();
        self.send(Job::Saved { line: line_of(&LogLine::Saved { at: now(), saved_at, turns }), from });
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

/// The writer beside the session: one job at a time, in the order the session gave them.
struct Writer {
    redact: Rc<dyn Fn(&str) -> String>,
    cap: Rc<Cell<u64>>,
    log: Option<Log>,
}

/// The log being written.
struct Log {
    folder: PathBuf,
    id: String,
    begin: Begin,
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

impl Log {
    fn file(&self) -> PathBuf {
        self.folder.join(log_name(&self.id))
    }
}

/// What a log on disk is to this Kumi: its own form (its size, and whether it ends on a line cut short), a later Kumi's,
/// or none it can read.
enum Found {
    Ours { size: u64, torn: bool },
    Theirs,
    Unreadable,
}

async fn found(file: &Path) -> std::io::Result<Found> {
    let mut handle = tokio::fs::File::open(file).await?;
    let size = handle.metadata().await?.len();
    let mut head = Vec::with_capacity(4096);
    (&mut handle).take(64 * 1024).read_to_end(&mut head).await?;
    // An empty file is made again; a first line longer than any header this Kumi writes is a later Kumi's.
    let Some(end) = head.iter().position(|b| *b == b'\n') else {
        return Ok(if size == 0 { Found::Unreadable } else { Found::Theirs });
    };
    // A later form's header may say other things: its version is read first.
    #[derive(Deserialize)]
    struct Form {
        kind: String,
        #[serde(default)]
        v: u64,
    }
    match serde_json::from_slice::<Form>(&head[..end]) {
        Ok(form) if form.kind == "header" && form.v > u64::from(LOG_VERSION) => return Ok(Found::Theirs),
        Ok(form) if form.kind == "header" && form.v >= 1 => {}
        _ => return Ok(Found::Unreadable),
    }
    if !matches!(serde_json::from_slice::<LogLine>(&head[..end]), Ok(LogLine::Header(_))) {
        return Ok(Found::Unreadable);
    }
    handle.seek(std::io::SeekFrom::End(-1)).await?;
    let mut last = [0u8];
    handle.read_exact(&mut last).await?;
    Ok(Found::Ours { size, torn: last[0] != b'\n' })
}

async fn append(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut handle = tokio::fs::OpenOptions::new().append(true).open(file).await?;
    handle.write_all(bytes).await?;
    // The write is the system's once it's flushed: a crash of Kumi from here loses none of it.
    handle.flush().await
}

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
                writer.put(lines).await;
            }
            Job::Open { folder, id, begin } => {
                writer.sync().await;
                writer.log = Some(Log { folder, id, begin, state: Written::Waiting(vec![]), full: false, dirty: false });
                writer.begin().await;
            }
            Job::Request { line, begin } => {
                writer.again(begin).await;
                writer.put(vec![line]).await;
            }
            Job::Saved { line, from } => {
                if let Some(log) = writer.log.as_mut().filter(|log| matches!(log.state, Written::Waiting(_))) {
                    if log.begin.from.is_none() {
                        log.begin.from = from;
                    }
                }
                writer.begin().await;
                writer.put(vec![line]).await;
            }
            Job::Moved { folder } => writer.moved(folder).await,
            Job::Sync => writer.sync().await,
            Job::Written(done) => {
                let _ = done.send(());
            }
            Job::Close(done) => {
                // Its first save may have landed since it was last looked for (the session waits for its saves first).
                writer.begin().await;
                writer.sync().await;
                writer.log = None;
                let _ = done.send(());
                break;
            }
        }
    }
}

impl Writer {
    /// A log that waited begins, once its conversation is kept: where one is there already (an earlier session's), lines
    /// go on its end; otherwise it's made.
    async fn begin(&mut self) {
        let Some(log) = self.log.as_mut().filter(|log| matches!(log.state, Written::Waiting(_))) else { return };
        if !tokio::fs::try_exists(log.folder.join(format!("{}.json", log.id))).await.unwrap_or(false) {
            return;
        }
        let Written::Waiting(waiting) = std::mem::replace(&mut log.state, Written::Theirs) else { return };
        let file = log.file();
        match found(&file).await {
            Ok(Found::Ours { size, torn }) => {
                if let Some(log) = self.log.as_mut() {
                    log.state = Written::Appending { size, torn };
                }
                self.put(split(waiting)).await;
            }
            Ok(Found::Theirs) => {}
            Ok(Found::Unreadable) | Err(_) => self.make(waiting).await,
        }
    }

    /// The log made new: its header, what the conversation had (or the log of the one it copies), then `after`.
    async fn make(&mut self, after: Vec<u8>) {
        let cap = self.cap.get();
        let Some(log) = self.log.as_ref() else { return };
        let mut header = LogHeader {
            v: LOG_VERSION,
            id: log.id.clone(),
            at: now(),
            kumi: kumi_types::version::KUMI_VERSION.into(),
            before: log.begin.before,
            from: None,
        };
        let copied = match &log.begin.from {
            Some(from) if is_conversation_id(from) => copy_of(&log.folder.join(log_name(from)), cap).await,
            _ => None,
        };
        let mut had = vec![];
        match copied {
            Some((before, lines)) => {
                header.before = before;
                header.from = log.begin.from.clone();
                had.push(lines);
            }
            None => {
                for message in log.begin.had.iter().flat_map(|kept| &kept.messages) {
                    had.push(line_of(&LogLine::Message { at: header.at, message: redacted_message(message, self.redact.as_ref()) }));
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
        let (folder, name) = (log.folder.clone(), log_name(&log.id));
        let text = String::from_utf8(bytes).expect("log lines are JSON");
        let made = write_privately(&folder, &name, &text).await;
        let Some(log) = self.log.as_mut() else { return };
        match made {
            Ok(()) => {
                log.state = Written::Appending { size: text.len() as u64, torn: false };
                log.full = full;
                log.dirty = true;
            }
            // Kept waiting: the next save tries again.
            Err(_) => log.state = Written::Waiting(after),
        }
    }

    /// A full log begins again with the request now made, from what the conversation had by then.
    async fn again(&mut self, begin: Begin) {
        let Some(log) = self.log.as_mut().filter(|log| log.full) else { return };
        log.begin = begin;
        log.full = false;
        let made = match &mut log.state {
            Written::Waiting(waiting) => {
                waiting.clear();
                false
            }
            Written::Appending { .. } => true,
            Written::Theirs => false,
        };
        if made {
            self.make(vec![]).await;
        }
    }

    /// Lines on the log's end, as many as fit within the cap: from the first that doesn't, none until the log begins
    /// again.
    async fn put(&mut self, lines: Vec<Vec<u8>>) {
        let cap = self.cap.get();
        let Some(log) = self.log.as_mut().filter(|log| !log.full) else { return };
        let (size, torn) = match &mut log.state {
            Written::Theirs => return,
            Written::Waiting(waiting) => {
                for line in lines {
                    if (waiting.len() + line.len()) as u64 > cap {
                        log.full = true;
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
                log.full = true;
                break;
            }
            bytes.extend(line);
        }
        if bytes.len() <= usize::from(torn) {
            return;
        }
        let file = log.file();
        let appended = append(&file, &bytes).await;
        let Some(log) = self.log.as_mut() else { return };
        match appended {
            Ok(()) => {
                log.state = Written::Appending { size: size + bytes.len() as u64, torn: false };
                log.dirty = true;
            }
            // Gone from under it: made again from what the conversation had, with these lines.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.make(bytes[usize::from(torn)..].to_vec()).await,
            // Whatever of it went in may end on a line cut short; its size is read again.
            Err(_) => {
                let size = tokio::fs::metadata(&file).await.map(|m| m.len()).unwrap_or(size);
                log.state = Written::Appending { size, torn: true };
                log.dirty = true;
            }
        }
    }

    async fn moved(&mut self, folder: PathBuf) {
        let Some(log) = self.log.as_mut() else { return };
        let from = log.file();
        log.folder = folder;
        if matches!(log.state, Written::Waiting(_)) {
            // Kept there already, it begins there.
            self.begin().await;
            return;
        }
        let to = log.file();
        let mut made = tokio::fs::DirBuilder::new();
        made.recursive(true);
        #[cfg(unix)]
        made.mode(0o700);
        let moved = made.create(&log.folder).await.is_ok() && tokio::fs::rename(&from, &to).await.is_ok();
        if !moved {
            // Left behind, it goes to its conversation's place with the store's next look (see its trim); lines go on a
            // new one here meanwhile.
            log.state = Written::Waiting(vec![]);
            self.begin().await;
        }
    }

    async fn sync(&mut self) {
        let Some(log) = self.log.as_mut().filter(|log| log.dirty) else { return };
        // Opened to write (nothing's written through it): Windows syncs only a file opened so, not one opened to append.
        if let Ok(handle) = tokio::fs::OpenOptions::new().write(true).open(log.file()).await {
            let _ = handle.sync_data().await;
        }
        log.dirty = false;
    }
}

/// Bytes of whole lines as lines.
fn split(bytes: Vec<u8>) -> Vec<Vec<u8>> {
    bytes.split_inclusive(|b| *b == b'\n').map(<[u8]>::to_vec).collect()
}

/// The lines of the log at `file` after its header, the whole ones, with its count of requests from before it began:
/// what a copy of its conversation's log begins with. None when it has none this Kumi reads, or one too big to copy.
async fn copy_of(file: &Path, cap: u64) -> Option<(u32, Vec<u8>)> {
    if tokio::fs::metadata(file).await.ok()?.len() > cap {
        return None;
    }
    let bytes = tokio::fs::read(file).await.ok()?;
    let read = read_log(&bytes)?;
    let start = bytes.iter().position(|b| *b == b'\n')? + 1;
    let end = bytes.iter().rposition(|b| *b == b'\n')? + 1;
    Some((read.header.before, bytes[start..end].to_vec()))
}
