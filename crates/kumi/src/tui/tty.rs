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
pub const RESTORE: &str =
    "\u{1b}[?2026l\u{1b}[0m\u{1b}[<u\u{1b}[?1006l\u{1b}[?1002l\u{1b}[?1000l\u{1b}[?1004l\u{1b}[?2004l\u{1b}[?7h\u{1b}[?25h\u{1b}[?1049l";

pub struct TtyOptions {
    pub input: Rc<dyn TtyInput>,
    pub output: Rc<dyn TtyOutput>,
    pub on_input: Rc<dyn Fn(InputEvent)>,
    pub on_resize: Rc<dyn Fn()>,
    /// Mouse reporting stops the terminal's own text selection; on by default.
    pub mouse: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub columns: i32,
    pub rows: i32,
}

struct Inner {
    options: TtyOptions,
    active: Arc<AtomicBool>,
    /// The crash path gave the terminal back: a panic, which Kumi may have caught (a worker's) and lived on.
    taken: Arc<AtomicBool>,
    was_raw: Cell<bool>,
    parser: InputParser,
    decoder: RefCell<Utf8Decoder>,
    emergency: Cell<Option<u64>>,
}

pub struct Tty {
    inner: Rc<Inner>,
}

impl Tty {
    pub fn new(options: TtyOptions) -> Tty {
        let parser = InputParser::new(Rc::clone(&options.on_input));
        Tty {
            inner: Rc::new(Inner {
                options,
                active: Arc::new(AtomicBool::new(false)),
                taken: Arc::new(AtomicBool::new(false)),
                was_raw: Cell::new(false),
                parser,
                decoder: RefCell::new(Utf8Decoder::new()),
                emergency: Cell::new(None),
            }),
        }
    }

    pub fn size(&self) -> Size {
        let output = &self.inner.options.output;
        Size { columns: output.columns().unwrap_or(80).max(1), rows: output.rows().unwrap_or(24).max(1) }
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
        let weak: Weak<Inner> = Rc::downgrade(inner);
        input.resume(Rc::new(move |chunk| {
            if let Some(inner) = weak.upgrade() {
                let text = inner.decoder.borrow_mut().write(chunk);
                inner.parser.push(&text);
            }
        }));
        output.watch_resize(Rc::clone(&inner.options.on_resize));
        // Taken again by a new start, not by `recover`: the crash path's registration goes with the old one.
        if let Some(id) = inner.emergency.take() {
            unregister(id);
        }
        inner.taken.store(false, Ordering::SeqCst);
        let (active, taken) = (Arc::clone(&inner.active), Arc::clone(&inner.taken));
        let writer = output.emergency_writer();
        let raw_mode = input.emergency_raw_mode();
        let was_raw = inner.was_raw.get();
        inner.emergency.set(Some(register(Arc::new(move || {
            if !active.swap(false, Ordering::SeqCst) {
                return;
            }
            if let Some(writer) = &writer {
                writer(RESTORE);
            }
            if let Some(raw_mode) = &raw_mode {
                raw_mode(was_raw);
            }
            taken.store(true, Ordering::SeqCst);
        }))));
        output.write(&format!("{ENTER}{}", if inner.options.mouse { MOUSE_ON } else { "" }));
        Ok(())
    }

    pub fn write(&self, data: &str) {
        if self.is_active() && !data.is_empty() {
            self.inner.options.output.write(data);
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
        inner.options.output.write(&format!("{ENTER}{}", if inner.options.mouse { MOUSE_ON } else { "" }));
        true
    }
}

impl Inner {
    fn restore(&self, sync: bool) {
        // Given back by the crash path and not taken again: what it didn't undo is still Kumi's to undo.
        if !self.active.swap(false, Ordering::SeqCst) && !self.taken.swap(false, Ordering::SeqCst) {
            return;
        }
        let input = &self.options.input;
        let output = &self.options.output;
        self.parser.dispose();
        output.unwatch_resize();
        if let Some(id) = self.emergency.take() {
            unregister(id);
        }
        // The terminal may already be gone: a failed write is nothing to report.
        if sync {
            if output.write_sync(RESTORE).is_none() {
                output.write(RESTORE);
            }
        } else {
            output.write(RESTORE);
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
        self.is_tty().then(|| crossterm::terminal::size().ok()).flatten().map(|(columns, _)| columns as i32)
    }

    fn rows(&self) -> Option<i32> {
        self.is_tty().then(|| crossterm::terminal::size().ok()).flatten().map(|(_, rows)| rows as i32)
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
        });
        assert!(!tty.recover(), "nothing to take back before a crash");
        tty.start().unwrap();
        let registered = emergency_restorers();
        // A worker's panic, caught: the crash path gives the terminal back before its message is printed.
        let caught = std::thread::spawn(|| std::panic::catch_unwind(|| panic!("a worker's bug")).is_err()).join().unwrap();
        assert!(caught);
        assert!(terminal.crashed.lock().unwrap().ends_with(RESTORE));
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
        // A second caught panic, then Kumi quits without a frame between: it's given back, and nothing stays registered.
        let _ = std::thread::spawn(|| std::panic::catch_unwind(|| panic!("another"))).join();
        assert!(!tty.is_active());
        tty.restore(false);
        assert!(!terminal.is_raw());
        assert_eq!(emergency_restorers(), registered - 1);
        assert!(!tty.recover());
    }
}
