//! Owns the terminal while Kumi is on screen: raw input, the alternate screen, bracketed
//! paste, mouse and focus reporting, no autowrap. Restoring is idempotent and also runs on
//! process exit and before a crash is printed, so a producer is never left with a broken
//! terminal they would need to `reset`.
//!
//! The process's own terminal is [`Stdin`] and [`Stdout`]; tests give a `Tty` fakes of the
//! same traits.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::{IsTerminal, Write};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};

use tokio::task::JoinHandle;

pub use crate::input::TerminalInput as TtyInput;
use crate::input::{ByteListener, ErrorListener, RawModeRestorer, Utf8Decoder};

use super::keys::{InputEvent, InputParser};

mod stdin_reader;

#[cfg(any(windows, test))]
mod windows_input;

/// Writes to the terminal from a crash or exit path, from any thread.
pub type EmergencyWriter = Arc<dyn Fn(&str) + Send + Sync>;

/// A terminal's output: Node's `Writable & { isTTY?, columns?, rows?, fd? }`.
pub trait TtyOutput {
    fn is_tty(&self) -> bool;
    fn columns(&self) -> Option<i32>;
    fn rows(&self) -> Option<i32>;
    /// Both at once: the terminal is asked once per frame, not twice.
    fn size(&self) -> Option<(i32, i32)> {
        Some((self.columns()?, self.rows()?))
    }
    fn write(&self, data: &str);
    /// Node's `writeSync` on the output's fd, for exit and crash paths; `None` where there's no fd.
    fn write_sync(&self, data: &str) -> Option<std::io::Result<()>> {
        let _ = data;
        None
    }
    /// Node's `on("resize", listener)`: the terminal changed size.
    fn watch_resize(&self, listener: Rc<dyn Fn()>) {
        let _ = listener;
    }
    /// Node's `removeListener("resize")`.
    fn unwatch_resize(&self) {}
    /// Node's `once("error", listener)`: the terminal went away.
    fn on_error(&self, listener: ErrorListener) {
        let _ = listener;
    }
    /// Writes to the terminal from a crash or exit path, from any thread; `None` where there's no fd.
    fn emergency_writer(&self) -> Option<EmergencyWriter> {
        None
    }
}

// \u001b[>3u asks terminals that support it (kitty, Ghostty, WezTerm, iTerm2) to report keys
// unambiguously, so Shift+Enter is distinct from Enter and Escape needs no timeout, and to say when
// a key repeats and when it's let go (ctrl+t held down talks until it's let go). Others ignore it.
const ENTER: &str = "\u{1b}[?1049h\u{1b}[?7l\u{1b}[?25l\u{1b}[?2004h\u{1b}[?1004h\u{1b}[>3u";
const MOUSE_ON: &str = "\u{1b}[?1000h\u{1b}[?1002h\u{1b}[?1006h";
// Any-event mouse reporting (1003): the mouse's moves with no button held, as well as its clicks and drags. On only
// while something listens for them (the map, which plays the point the mouse comes to): otherwise they'd stream in
// whenever the mouse crossed the window.
const MOUSE_MOVES_ON: &str = "\u{1b}[?1003h";
const MOUSE_MOVES_OFF: &str = "\u{1b}[?1003l";
// With the mouse left to the terminal, its wheel isn't Kumi's either: on a full screen, many terminals turn it into ↑
// and ↓ (alternate scroll, 1007), which would bring back what the producer sent before. Off while Kumi runs, and on
// again as it leaves, as those terminals have it.
const WHEEL_KEYS_OFF: &str = "\u{1b}[?1007l";
const WHEEL_KEYS_ON: &str = "\u{1b}[?1007h";
/// What gives the terminal back as Kumi found it: every mode it set off, and the wheel's keys on again where it turned
/// them off (the mouse left to the terminal).
fn leaving(mouse: bool) -> String {
    format!("{RESTORE}{}", if mouse { "" } else { WHEEL_KEYS_ON })
}
pub const RESTORE: &str =
    "\u{1b}[?2026l\u{1b}[0m\u{1b}[<u\u{1b}[?1003l\u{1b}[?1006l\u{1b}[?1002l\u{1b}[?1000l\u{1b}[?1004l\u{1b}[?2004l\u{1b}[?7h\u{1b}[?25h\u{1b}[?1049l";

pub struct TtyOptions {
    pub input: Rc<dyn TtyInput>,
    pub output: Rc<dyn TtyOutput>,
    pub on_input: Rc<dyn Fn(InputEvent)>,
    pub on_resize: Rc<dyn Fn()>,
    /// Mouse reporting stops the terminal's own text selection; on by default. Off, the wheel doesn't type ↑ and ↓
    /// either.
    pub mouse: bool,
    /// Written before [`RESTORE`] whenever the terminal is given back, the crash path's too: what clears the
    /// status Kumi showed in the terminal (its OSC 7501 record and 9;4 progress). Empty for none.
    pub farewell: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub columns: i32,
    pub rows: i32,
}

struct Inner {
    options: TtyOptions,
    /// The farewell, then what gives the terminal back ([`leaving`]): written on quitting, and by the crash path.
    restore: String,
    active: Arc<AtomicBool>,
    /// The crash path gave the terminal back: a panic, which Kumi may have caught (a worker's) and lived on.
    taken: Arc<AtomicBool>,
    was_raw: Cell<bool>,
    /// The mouse's moves are asked for (`report_moves`): reported while the terminal is Kumi's, and again when it's
    /// taken back.
    moves: Cell<bool>,
    /// Lent to a program Kumi runs in it ([`Tty::hand_over`]), and not yet given back for good.
    lent: Cell<bool>,
    /// Given back for good while lent (Kumi closing): what waits for a key in the lent terminal stops waiting. Only a
    /// wait under way is woken (a wait starts and polls it without yielding, so none misses it), and nothing is kept for
    /// later: the terminal is never lent again, so a later wait returns at once.
    closed: tokio::sync::Notify,
    parser: InputParser,
    decoder: RefCell<Utf8Decoder>,
    emergency: Cell<Option<u64>>,
}

/// How long the rest of a key's bytes may trail its first ([`Tty::wait_for_key`]).
const KEY_REST_MS: u64 = 40;

pub struct Tty {
    inner: Rc<Inner>,
}

impl Tty {
    pub fn new(options: TtyOptions) -> Tty {
        let parser = InputParser::new(Rc::clone(&options.on_input));
        Tty {
            inner: Rc::new(Inner {
                restore: format!("{}{}", options.farewell, leaving(options.mouse)),
                options,
                active: Arc::new(AtomicBool::new(false)),
                taken: Arc::new(AtomicBool::new(false)),
                was_raw: Cell::new(false),
                moves: Cell::new(false),
                lent: Cell::new(false),
                closed: tokio::sync::Notify::new(),
                parser,
                decoder: RefCell::new(Utf8Decoder::new()),
                emergency: Cell::new(None),
            }),
        }
    }

    pub fn size(&self) -> Size {
        let (columns, rows) = self.inner.options.output.size().unwrap_or((80, 24));
        Size { columns: columns.max(1), rows: rows.max(1) }
    }

    pub fn is_active(&self) -> bool {
        self.inner.active.load(Ordering::SeqCst)
    }

    pub fn start(&self) -> std::io::Result<()> {
        let inner = &self.inner;
        if inner.active.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let input = &inner.options.input;
        let output = &inner.options.output;
        inner.was_raw.set(input.is_raw());
        input.set_raw_mode(true)?;
        self.listen();
        output.watch_resize(Rc::clone(&inner.options.on_resize));
        // Taken again by a new start, not by `recover`: the crash path's registration goes with the old one.
        if let Some(id) = inner.emergency.take() {
            unregister(id);
        }
        inner.taken.store(false, Ordering::SeqCst);
        let (active, taken) = (Arc::clone(&inner.active), Arc::clone(&inner.taken));
        let writer = output.emergency_writer();
        let restore = inner.restore.clone();
        let raw_mode = input.emergency_raw_mode();
        let was_raw = inner.was_raw.get();
        inner.emergency.set(Some(register(Arc::new(move || {
            if !active.swap(false, Ordering::SeqCst) {
                return;
            }
            if let Some(writer) = &writer {
                writer(&restore);
            }
            if let Some(raw_mode) = &raw_mode {
                raw_mode(was_raw);
            }
            taken.store(true, Ordering::SeqCst);
        }))));
        output.write(&inner.entering());
        Ok(())
    }

    fn listen(&self) {
        let weak: Weak<Inner> = Rc::downgrade(&self.inner);
        self.inner.options.input.resume(Rc::new(move |chunk| {
            if let Some(inner) = weak.upgrade() {
                let text = inner.decoder.borrow_mut().write(chunk);
                inner.parser.push(&text);
            }
        }));
    }

    pub fn write(&self, data: &str) {
        if self.is_active() && !data.is_empty() {
            self.inner.options.output.write(data);
        }
    }

    /// Reads `bytes` as if they had just come in: what was read before the screen was Kumi's (while the terminal was
    /// asked what it is), so keys typed meanwhile aren't lost and the answers among them are read as answers.
    pub fn read_again(&self, bytes: &[u8]) {
        if bytes.is_empty() || !self.is_active() {
            return;
        }
        let text = self.inner.decoder.borrow_mut().write(bytes);
        self.inner.parser.push(&text);
    }

    /// Lends the terminal to a program Kumi runs in it (the producer's editor) until `run` is done. The screen and every
    /// mode Kumi set go back as they were, raw input too, and Kumi's reader stops reading, so the program gets every
    /// key. Nothing Kumi draws is written meanwhile; once it's taken back, the caller draws the whole screen again. A
    /// terminal given back for good meanwhile (Kumi closing) isn't taken again.
    pub async fn hand_over<T>(&self, run: impl std::future::Future<Output = T>) -> T {
        let inner = &self.inner;
        if inner.lent.get() || !inner.active.swap(false, Ordering::SeqCst) {
            return run.await;
        }
        inner.lent.set(true);
        inner.parser.dispose();
        inner.options.output.write(&leaving(inner.options.mouse));
        inner.options.input.pause();
        let _ = inner.options.input.set_raw_mode(inner.was_raw.get());
        let result = run.await;
        if inner.lent.replace(false) {
            let _ = inner.options.input.set_raw_mode(true);
            // Keys typed meanwhile (beside a window of its own) were the program's, as is a reply to what it asked the
            // terminal: one ending in BEL would read as ctrl+g.
            inner.options.input.discard_pending();
            self.listen();
            inner.active.store(true, Ordering::SeqCst);
            inner.options.output.write(&inner.entering());
        }
        result
    }

    /// While the terminal is lent ([`Tty::hand_over`]), waits for a key as a program run in it would, and drops it whole:
    /// the rest of an arrow's or a function key's bytes, a moment behind the first, are dropped with it. What was typed
    /// before and is still in the terminal's own buffer is dropped first; bytes Kumi's reader had already taken before
    /// the terminal was lent (rare) end the wait at once. Kumi closing meanwhile ends the wait; not lent, it returns at
    /// once.
    pub async fn wait_for_key(&self) {
        let inner = &self.inner;
        if !inner.lent.get() {
            return;
        }
        let input = &inner.options.input;
        input.discard_pending();
        let _ = input.set_raw_mode(true);
        let pressed = Rc::new(tokio::sync::Notify::new());
        let key = Rc::clone(&pressed);
        input.resume(Rc::new(move |_: &[u8]| key.notify_one()));
        tokio::select! {
            () = pressed.notified() => tokio::time::sleep(std::time::Duration::from_millis(KEY_REST_MS)).await,
            () = inner.closed.notified() => {}
        }
        if inner.lent.get() {
            input.pause();
            input.discard_pending();
            let _ = input.set_raw_mode(inner.was_raw.get());
        }
    }

    /// Whether the terminal is lent to a program Kumi runs ([`Tty::hand_over`]): its keys, ctrl+c too, are that
    /// program's.
    pub fn lent(&self) -> bool {
        self.inner.lent.get()
    }

    /// Whether the terminal reports the mouse's moves (no button held) as well as its clicks: asked while the map is
    /// open, which plays the point the mouse comes to, and taken back as it closes, so they don't stream in meanwhile.
    /// Nothing where mouse reporting is off.
    pub fn report_moves(&self, on: bool) {
        let inner = &self.inner;
        if !inner.options.mouse || inner.moves.replace(on) == on {
            return;
        }
        if self.is_active() {
            inner.options.output.write(if on { MOUSE_MOVES_ON } else { MOUSE_MOVES_OFF });
        }
    }

    /// Put the terminal back as it was. `sync` writes immediately, for exit and crash paths.
    pub fn restore(&self, sync: bool) {
        self.inner.restore(sync);
    }

    /// Takes the terminal again after a panic Kumi caught (a worker's): the crash path gave it back before the
    /// panic's message was printed, in case the panic ended Kumi, and Kumi lived on. True when it did, so the caller
    /// draws the whole screen again.
    pub fn recover(&self) -> bool {
        let inner = &self.inner;
        if !inner.taken.swap(false, Ordering::SeqCst) {
            return false;
        }
        let _ = inner.options.input.set_raw_mode(true);
        inner.active.store(true, Ordering::SeqCst);
        inner.options.output.write(&inner.entering());
        true
    }
}

impl Inner {
    /// What takes the terminal: the screen and the keys, mouse reporting where it's on (else the wheel's keys off), and
    /// the mouse's moves while they're asked for.
    fn entering(&self) -> String {
        let mouse = if self.options.mouse { MOUSE_ON } else { WHEEL_KEYS_OFF };
        let moves = if self.options.mouse && self.moves.get() { MOUSE_MOVES_ON } else { "" };
        format!("{ENTER}{mouse}{moves}")
    }

    fn restore(&self, sync: bool) {
        // Lent to a program: its screen and modes are given back already, and aren't taken again when it's done.
        let lent = self.lent.replace(false);
        if lent {
            self.closed.notify_waiters();
        }
        // Given back by the crash path and not taken again: what it didn't undo is still Kumi's to undo.
        let active = self.active.swap(false, Ordering::SeqCst);
        let taken = self.taken.swap(false, Ordering::SeqCst);
        if !active && !taken && !lent {
            return;
        }
        let input = &self.options.input;
        let output = &self.options.output;
        self.parser.dispose();
        output.unwatch_resize();
        if let Some(id) = self.emergency.take() {
            unregister(id);
        }
        // The terminal may already be gone: a failed write is nothing to report. Once the crash path wrote it, it isn't
        // written again: its `?1049l` would put the cursor back where Kumi found it, over the panic's message (the
        // shell's prompt, or a farewell, would land on it).
        if active && !(sync && output.write_sync(&self.restore).is_some()) {
            output.write(&self.restore);
        }
        // What clears Kumi's status in the terminal still goes, without taking the screen from the program.
        if lent && !self.options.farewell.is_empty() && !(sync && output.write_sync(&self.options.farewell).is_some()) {
            output.write(&self.options.farewell);
        }
        input.pause();
        let _ = input.set_raw_mode(self.was_raw.get());
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.restore(true);
    }
}

// ---- restoring from a crash or exit: Node's process "exit" and "uncaughtExceptionMonitor" listeners

type Restorer = Arc<dyn Fn() + Send + Sync>;

static RESTORERS: Mutex<Vec<(u64, Restorer)>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static HOOKS: Once = Once::new();

fn restorers() -> std::sync::MutexGuard<'static, Vec<(u64, Restorer)>> {
    RESTORERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn register(restorer: Restorer) -> u64 {
    install_hooks();
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    restorers().push((id, restorer));
    id
}

fn unregister(id: u64) {
    restorers().retain(|(other, _)| *other != id);
}

/// How many terminals wait to be put back on exit or a crash; for tests.
pub fn emergency_restorers() -> usize {
    restorers().len()
}

fn run_restorers() {
    let pending: Vec<Restorer> = restorers().iter().map(|(_, restorer)| Arc::clone(restorer)).collect();
    for restorer in pending {
        restorer();
    }
}

#[cfg(unix)]
extern "C" fn at_exit() {
    run_restorers();
}

/// Node's process "exit" listener: restore when the process exits, however it does.
#[cfg(unix)]
fn install_at_exit() {
    // SAFETY: at_exit is a plain function that touches only a process-wide mutex.
    unsafe {
        libc::atexit(at_exit);
    }
}

#[cfg(not(unix))]
fn install_at_exit() {}

fn install_hooks() {
    HOOKS.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // A panic on the session's own thread ends the screen in order (the terminal given back, then what it
            // said), from the screen's thread: it isn't this thread's to restore.
            if kumi_agent::core::threaded::session_panicked(info) {
                return;
            }
            run_restorers();
            previous(info);
        }));
        install_at_exit();
    });
}

// ---- the process's own terminal

enum Message {
    Data(Vec<u8>),
    End,
    Error(std::io::Error),
}

struct StdinInner {
    #[cfg(windows)]
    raw_mode: Arc<Mutex<windows_input::RawMode>>,
    listener: RefCell<Option<ByteListener>>,
    ends: RefCell<Vec<Rc<dyn Fn()>>>,
    errors: RefCell<Vec<ErrorListener>>,
    paused: Cell<bool>,
    resumed: Arc<tokio::sync::Notify>,
    pending: RefCell<VecDeque<Message>>,
    pump: RefCell<Option<JoinHandle<()>>>,
    reader: RefCell<Option<stdin_reader::Reader>>,
}

/// The process's standard input (Node's `process.stdin`): a thread reads it and bytes reach the
/// listener on the runtime thread, so a read never blocks Kumi. Pausing releases kernel input
/// ownership; bytes already read wait for the next `resume`.
#[derive(Clone)]
pub struct Stdin {
    inner: Rc<StdinInner>,
}

impl Default for Stdin {
    fn default() -> Self {
        Self::new()
    }
}

impl Stdin {
    pub fn new() -> Stdin {
        Stdin {
            inner: Rc::new(StdinInner {
                #[cfg(windows)]
                raw_mode: Arc::new(Mutex::new(windows_input::RawMode::default())),
                listener: RefCell::new(None),
                ends: RefCell::new(Vec::new()),
                errors: RefCell::new(Vec::new()),
                paused: Cell::new(true),
                resumed: Arc::new(tokio::sync::Notify::new()),
                pending: RefCell::new(VecDeque::new()),
                pump: RefCell::new(None),
                reader: RefCell::new(None),
            }),
        }
    }

    fn deliver(inner: &StdinInner, message: Message) {
        match message {
            Message::Data(bytes) => {
                let listener = inner.listener.borrow().clone();
                if let Some(listener) = listener {
                    listener(&bytes);
                }
            }
            Message::End => {
                let ends = std::mem::take(&mut *inner.ends.borrow_mut());
                for listener in ends {
                    listener();
                }
            }
            Message::Error(error) => {
                let errors = inner.errors.borrow().clone();
                for listener in errors {
                    listener(&error);
                }
            }
        }
    }

    fn start_pump(&self) {
        if self.inner.pump.borrow().is_some() {
            return;
        }
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Message>();
        match stdin_reader::Reader::new(sender.clone()) {
            Ok(reader) => *self.inner.reader.borrow_mut() = Some(reader),
            Err(error) => {
                // A missing/disconnected standard handle is a stream error, just
                // like a failed read. Deliver it once through the ordinary pump.
                let _ = sender.send(Message::Error(error));
            }
        }
        drop(sender);
        let weak = Rc::downgrade(&self.inner);
        let resumed = self.inner.resumed.clone();
        let pump = tokio::task::spawn_local(async move {
            let mut closed = false;
            loop {
                {
                    let Some(inner) = weak.upgrade() else { break };
                    while !inner.paused.get() {
                        let next = inner.pending.borrow_mut().pop_front();
                        let Some(message) = next else { break };
                        Stdin::deliver(&inner, message);
                        if let Some(reader) = inner.reader.borrow().as_ref() {
                            reader.acknowledge();
                        }
                    }
                    if closed && inner.pending.borrow().is_empty() {
                        break;
                    }
                }
                tokio::select! {
                    _ = resumed.notified() => {},
                    message = receiver.recv(), if !closed => {
                        match message {
                            Some(message) => {
                                let Some(inner) = weak.upgrade() else { break };
                                inner.pending.borrow_mut().push_back(message);
                            }
                            None => closed = true,
                        }
                    }
                }
            }
        });
        *self.inner.pump.borrow_mut() = Some(pump);
    }
}

impl TtyInput for Stdin {
    fn is_tty(&self) -> bool {
        std::io::stdin().is_terminal()
    }

    fn is_raw(&self) -> bool {
        #[cfg(windows)]
        {
            // Node's stream starts with isRaw=false, even if its inherited console
            // already has raw flags. The owner preserves that exact inherited mode.
            self.inner.raw_mode.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).is_raw()
        }
        #[cfg(not(windows))]
        {
            crossterm::terminal::is_raw_mode_enabled().unwrap_or(false)
        }
    }

    fn set_raw_mode(&self, enabled: bool) -> std::io::Result<()> {
        let change = || {
            #[cfg(windows)]
            {
                self.inner.raw_mode.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).set(enabled)
            }
            #[cfg(not(windows))]
            {
                if enabled {
                    crossterm::terminal::enable_raw_mode()
                } else {
                    crossterm::terminal::disable_raw_mode()
                }
            }
        };
        match self.inner.reader.borrow().as_ref() {
            Some(reader) => reader.with_mode(change),
            None => change(),
        }
    }

    fn resume(&self, listener: ByteListener) {
        *self.inner.listener.borrow_mut() = Some(listener);
        self.inner.paused.set(false);
        self.start_pump();
        if let Some(reader) = self.inner.reader.borrow().as_ref() {
            reader.resume();
        }
        self.inner.resumed.notify_one();
    }

    fn pause(&self) {
        self.inner.paused.set(true);
        *self.inner.listener.borrow_mut() = None;
        if let Some(reader) = self.inner.reader.borrow().as_ref() {
            reader.pause();
        }
    }

    fn on_end(&self, listener: Rc<dyn Fn()>) {
        self.inner.ends.borrow_mut().push(listener);
    }

    fn on_error(&self, listener: ErrorListener) {
        self.inner.errors.borrow_mut().push(listener);
    }

    fn discard_pending(&self) {
        #[cfg(unix)]
        // SAFETY: tcflush only reads the descriptor; stdin is open for the process's life.
        unsafe {
            libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Console::{FlushConsoleInputBuffer, GetStdHandle, STD_INPUT_HANDLE};
            // SAFETY: GetStdHandle returns the process's own input handle, or one FlushConsoleInputBuffer refuses.
            unsafe {
                FlushConsoleInputBuffer(GetStdHandle(STD_INPUT_HANDLE));
            }
        }
    }

    fn emergency_raw_mode(&self) -> Option<RawModeRestorer> {
        #[cfg(windows)]
        let restore: RawModeRestorer = {
            let raw_mode = self.inner.raw_mode.clone();
            Arc::new(move |was_raw| {
                let _ = raw_mode.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).restore(was_raw);
            })
        };
        #[cfg(not(windows))]
        let restore: RawModeRestorer = Arc::new(|enabled| {
            let _ = if enabled { crossterm::terminal::enable_raw_mode() } else { crossterm::terminal::disable_raw_mode() };
        });
        Some(match self.inner.reader.borrow().as_ref() {
            Some(reader) => reader.guard_restorer(restore),
            None => restore,
        })
    }
}

impl Drop for StdinInner {
    fn drop(&mut self) {
        if let Some(pump) = self.pump.get_mut().take() {
            pump.abort();
        }
        self.reader.get_mut().take();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stream {
    Out,
    Err,
}

struct StdoutInner {
    stream: Stream,
    errors: RefCell<Vec<ErrorListener>>,
    failed: Cell<bool>,
    resize: RefCell<Option<JoinHandle<()>>>,
}

/// The process's standard output or error stream (Node's `process.stdout` / `process.stderr`).
#[derive(Clone)]
pub struct Stdout {
    inner: Rc<StdoutInner>,
}

/// Escape sequences on a Windows console are text unless its virtual-terminal processing is on, which
/// Node's libuv turned on for every terminal stream. This turns it on for standard output and error
/// while it lives, and puts the consoles back as they were when dropped. Elsewhere it does nothing.
pub struct VtOutput {
    #[cfg(windows)]
    restore: Vec<(windows_sys::Win32::Foundation::HANDLE, u32)>,
}

impl VtOutput {
    pub fn enable() -> VtOutput {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
            let handles = [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].map(|which| unsafe { GetStdHandle(which) });
            VtOutput { restore: Self::enable_on(&handles) }
        }
        #[cfg(not(windows))]
        VtOutput {}
    }

    /// Each console among `handles` without VT processing gets it; returns what to put back.
    #[cfg(windows)]
    fn enable_on(handles: &[windows_sys::Win32::Foundation::HANDLE]) -> Vec<(windows_sys::Win32::Foundation::HANDLE, u32)> {
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            System::Console::{GetConsoleMode, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING},
        };
        let mut restore = Vec::new();
        for &handle in handles {
            let mut mode = 0;
            // Not a console (a pipe or a file): nothing to turn on.
            if handle.is_null() || handle == INVALID_HANDLE_VALUE || unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
                continue;
            }
            if mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING == 0
                && unsafe { SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) } != 0
            {
                restore.push((handle, mode));
            }
        }
        restore
    }
}

impl Drop for VtOutput {
    fn drop(&mut self) {
        #[cfg(windows)]
        for &(handle, mode) in self.restore.iter().rev() {
            unsafe { windows_sys::Win32::System::Console::SetConsoleMode(handle, mode) };
        }
    }
}

impl Default for Stdout {
    fn default() -> Self {
        Self::new()
    }
}

impl Stdout {
    /// Standard output.
    pub fn new() -> Stdout {
        Stdout::of(Stream::Out)
    }

    /// Standard error.
    pub fn stderr() -> Stdout {
        Stdout::of(Stream::Err)
    }

    fn of(stream: Stream) -> Stdout {
        Stdout {
            inner: Rc::new(StdoutInner { stream, errors: RefCell::new(Vec::new()), failed: Cell::new(false), resize: RefCell::new(None) }),
        }
    }

    fn write_now(stream: Stream, data: &str) -> std::io::Result<()> {
        match stream {
            Stream::Out => {
                let mut out = std::io::stdout().lock();
                out.write_all(data.as_bytes())?;
                out.flush()
            }
            Stream::Err => {
                let mut err = std::io::stderr().lock();
                err.write_all(data.as_bytes())?;
                err.flush()
            }
        }
    }
}

impl TtyOutput for Stdout {
    fn is_tty(&self) -> bool {
        match self.inner.stream {
            Stream::Out => std::io::stdout().is_terminal(),
            Stream::Err => std::io::stderr().is_terminal(),
        }
    }

    fn columns(&self) -> Option<i32> {
        self.size().map(|(columns, _)| columns)
    }

    fn rows(&self) -> Option<i32> {
        self.size().map(|(_, rows)| rows)
    }

    fn size(&self) -> Option<(i32, i32)> {
        self.is_tty().then(|| crossterm::terminal::size().ok()).flatten().map(|(columns, rows)| (columns as i32, rows as i32))
    }

    fn write(&self, data: &str) {
        if let Err(error) = Stdout::write_now(self.inner.stream, data) {
            // Node's stream errors once (EPIPE, say) and is destroyed; later writes are dropped.
            if !self.inner.failed.replace(true) {
                let listeners = std::mem::take(&mut *self.inner.errors.borrow_mut());
                for listener in listeners {
                    listener(&error);
                }
            }
        }
    }

    fn write_sync(&self, data: &str) -> Option<std::io::Result<()>> {
        Some(Stdout::write_now(self.inner.stream, data))
    }

    fn watch_resize(&self, listener: Rc<dyn Fn()>) {
        self.unwatch_resize();
        #[cfg(unix)]
        let task = tokio::task::spawn_local(async move {
            let Ok(mut changes) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()) else { return };
            while changes.recv().await.is_some() {
                listener();
            }
        });
        #[cfg(not(unix))]
        let task = tokio::task::spawn_local(async move {
            // Windows has no SIGWINCH: the size is checked a few times a second.
            let mut last = crossterm::terminal::size().ok();
            let mut ticks = tokio::time::interval(std::time::Duration::from_millis(200));
            loop {
                ticks.tick().await;
                let size = crossterm::terminal::size().ok();
                if size != last {
                    last = size;
                    listener();
                }
            }
        });
        *self.inner.resize.borrow_mut() = Some(task);
    }

    fn unwatch_resize(&self) {
        if let Some(task) = self.inner.resize.borrow_mut().take() {
            task.abort();
        }
    }

    fn on_error(&self, listener: ErrorListener) {
        self.inner.errors.borrow_mut().push(listener);
    }

    fn emergency_writer(&self) -> Option<EmergencyWriter> {
        let stream = self.inner.stream;
        Some(Arc::new(move |data| {
            let _ = Stdout::write_now(stream, data);
        }))
    }
}

#[cfg(all(test, windows))]
mod vt_tests {
    use super::VtOutput;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{GetConsoleMode, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING};

    #[test]
    fn output_that_is_not_a_console_is_left_alone() {
        let file = tempfile::tempfile().unwrap();
        assert!(VtOutput::enable_on(&[file.as_raw_handle() as _]).is_empty());
    }

    #[test]
    fn a_console_gets_vt_processing_and_its_mode_back() {
        // The console this test runs in, if any (CI runs without one).
        let Ok(console) = std::fs::OpenOptions::new().read(true).write(true).open("CONOUT$") else { return };
        let handle = console.as_raw_handle() as _;
        let mut before = 0;
        if unsafe { GetConsoleMode(handle, &mut before) } == 0 {
            return;
        }
        let off = before & !ENABLE_VIRTUAL_TERMINAL_PROCESSING;
        assert_ne!(unsafe { SetConsoleMode(handle, off) }, 0);
        let enabled = VtOutput { restore: VtOutput::enable_on(&[handle]) };
        let mut on = 0;
        unsafe { GetConsoleMode(handle, &mut on) };
        assert_ne!(on & ENABLE_VIRTUAL_TERMINAL_PROCESSING, 0);
        drop(enabled);
        let mut after = 0;
        unsafe { GetConsoleMode(handle, &mut after) };
        assert_eq!(after, off);
        unsafe { SetConsoleMode(handle, before) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic on the session's own thread leaves the terminal alone: the screen's thread gives it back, in order, when
    /// it hears the session ended. A panic anywhere else gives it back at once, as it always has.
    #[test]
    fn a_panic_on_the_sessions_thread_leaves_the_terminal_to_the_screens_thread() {
        use futures::FutureExt;
        let restored = Arc::new(AtomicBool::new(false));
        let id = register({
            let restored = restored.clone();
            Arc::new(move || restored.store(true, Ordering::SeqCst))
        });
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let started = tokio::task::LocalSet::new().block_on(&runtime, async {
            let build: kumi_agent::core::threaded::Build = Box::new(|_| async { panic!("no room for the store") }.boxed_local());
            kumi_agent::core::threaded::Threaded::spawn(build).await
        });
        let why = started.err().expect("the session didn't start").message();
        assert!(why.starts_with("no room for the store"), "{why}");
        assert!(!restored.load(Ordering::SeqCst), "the session's thread gave the terminal back");
        // Any other thread's panic gives it back, as before.
        let _ = std::thread::spawn(|| panic!("a worker's own panic")).join();
        assert!(restored.load(Ordering::SeqCst));
        unregister(id);
    }

    /// A terminal whose crash path is seen: raw mode and the crash writes are shared with the hook's thread.
    struct Crash {
        raw: Arc<AtomicBool>,
        crashed: Arc<Mutex<String>>,
        written: RefCell<String>,
    }
    impl TtyInput for Crash {
        fn is_tty(&self) -> bool {
            true
        }
        fn is_raw(&self) -> bool {
            self.raw.load(Ordering::SeqCst)
        }
        fn set_raw_mode(&self, enabled: bool) -> std::io::Result<()> {
            self.raw.store(enabled, Ordering::SeqCst);
            Ok(())
        }
        fn resume(&self, _: ByteListener) {}
        fn pause(&self) {}
        fn emergency_raw_mode(&self) -> Option<RawModeRestorer> {
            let raw = Arc::clone(&self.raw);
            Some(Arc::new(move |enabled| raw.store(enabled, Ordering::SeqCst)))
        }
    }
    impl TtyOutput for Crash {
        fn is_tty(&self) -> bool {
            true
        }
        fn columns(&self) -> Option<i32> {
            Some(80)
        }
        fn rows(&self) -> Option<i32> {
            Some(24)
        }
        fn write(&self, data: &str) {
            self.written.borrow_mut().push_str(data);
        }
        fn emergency_writer(&self) -> Option<EmergencyWriter> {
            let crashed = Arc::clone(&self.crashed);
            Some(Arc::new(move |data| crashed.lock().unwrap().push_str(data)))
        }
    }

    #[test]
    fn with_the_mouse_off_the_terminal_keeps_it_and_its_wheel_types_no_keys() {
        let terminal = Rc::new(Crash { raw: Arc::new(AtomicBool::new(false)), crashed: Arc::default(), written: RefCell::default() });
        let tty = Tty::new(TtyOptions {
            input: terminal.clone(),
            output: terminal.clone(),
            on_input: Rc::new(|_| {}),
            on_resize: Rc::new(|| {}),
            mouse: false,
            farewell: String::new(),
        });
        // Taken without mouse reporting and with the wheel's ↑ and ↓ off; given back, the crash path's way too, with
        // them on again.
        assert_eq!(tty.inner.entering(), format!("{ENTER}{WHEEL_KEYS_OFF}"));
        assert_eq!(tty.inner.restore, format!("{RESTORE}{WHEEL_KEYS_ON}"));
        // Nor are the mouse's moves asked for, for the map.
        tty.report_moves(true);
        assert_eq!(tty.inner.entering(), format!("{ENTER}{WHEEL_KEYS_OFF}"));
    }

    // No other test in this binary owns a terminal: the panic here runs every registered restorer in the process.
    #[test]
    fn a_panic_kumi_catches_gives_the_terminal_back_then_kumi_takes_it_again() {
        let terminal = Rc::new(Crash { raw: Arc::new(AtomicBool::new(false)), crashed: Arc::default(), written: RefCell::default() });
        let tty = Tty::new(TtyOptions {
            input: terminal.clone(),
            output: terminal.clone(),
            on_input: Rc::new(|_| {}),
            on_resize: Rc::new(|| {}),
            mouse: true,
            farewell: "\u{1b}]7501;state=clear\u{1b}\\".into(),
        });
        assert!(!tty.recover(), "nothing to take back before a crash");
        tty.start().unwrap();
        let registered = emergency_restorers();
        // A worker's panic, caught: the crash path gives the terminal back before its message is printed.
        let caught = std::thread::spawn(|| std::panic::catch_unwind(|| panic!("a worker's bug")).is_err()).join().unwrap();
        assert!(caught);
        // Kumi's status in the terminal goes with it.
        assert!(terminal.crashed.lock().unwrap().ends_with(&format!("\u{1b}]7501;state=clear\u{1b}\\{RESTORE}")));
        assert!(!terminal.is_raw());
        assert!(!tty.is_active());
        // Kumi lived on: its next frame takes the terminal again, once, and draws all of it.
        terminal.written.borrow_mut().clear();
        assert!(tty.recover());
        assert!(terminal.is_raw() && tty.is_active());
        assert_eq!(*terminal.written.borrow(), format!("{ENTER}{MOUSE_ON}"));
        assert!(!tty.recover());
        tty.write("frame");
        assert!(terminal.written.borrow().ends_with("frame"));
        // The map asks for the mouse's moves: reported from then on (asked once, however often), still when Kumi takes
        // the terminal back after another caught panic, and no more once the map is done with them.
        terminal.written.borrow_mut().clear();
        tty.report_moves(true);
        tty.report_moves(true);
        assert_eq!(*terminal.written.borrow(), MOUSE_MOVES_ON);
        let _ = std::thread::spawn(|| std::panic::catch_unwind(|| panic!("a third"))).join();
        assert!(terminal.crashed.lock().unwrap().ends_with(RESTORE) && RESTORE.contains(MOUSE_MOVES_OFF), "given back without them");
        terminal.written.borrow_mut().clear();
        assert!(tty.recover());
        assert_eq!(*terminal.written.borrow(), format!("{ENTER}{MOUSE_ON}{MOUSE_MOVES_ON}"));
        terminal.written.borrow_mut().clear();
        tty.report_moves(false);
        assert_eq!(*terminal.written.borrow(), MOUSE_MOVES_OFF);
        // A second caught panic, then Kumi quits without a frame between: it's given back once (by the crash path, not
        // again on quitting, which would put the cursor back over the message), and nothing stays registered.
        terminal.written.borrow_mut().clear();
        terminal.crashed.lock().unwrap().clear();
        let _ = std::thread::spawn(|| std::panic::catch_unwind(|| panic!("another"))).join();
        assert!(!tty.is_active());
        assert!(terminal.crashed.lock().unwrap().ends_with(RESTORE));
        tty.restore(false);
        assert!(!terminal.written.borrow().contains(RESTORE), "{:?}", terminal.written.borrow());
        assert!(!terminal.is_raw());
        assert_eq!(emergency_restorers(), registered - 1);
        assert!(!tty.recover());
    }
}
