#![cfg(windows)]

use kumi::input::TerminalInput;
use kumi::tui::tty::Stdin;
use std::cell::{Cell, RefCell};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, Read, Write};
use std::os::windows::{io::AsRawHandle, process::CommandExt};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Console::*;

const PROBE: &str = "KUMI_WINDOWS_STDIN_CONSOLE";
const CHILD: &str = "KUMI_WINDOWS_STDIN_CHILD";
const CREATE_NEW_CONSOLE: u32 = 0x10;

struct Process(Option<Child>);
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn wait(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "console process did not finish before its deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn isolated(case: &str, run: impl FnOnce(File)) {
    if std::env::var(PROBE).as_deref() != Ok(case) {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", case, "--nocapture"])
            .env(PROBE, case)
            .creation_flags(CREATE_NEW_CONSOLE)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut process = Process(Some(child));
        let status = wait(process.0.as_mut().unwrap(), Duration::from_secs(60));
        let output = process.0.take().unwrap().wait_with_output().unwrap();
        for line in String::from_utf8_lossy(&output.stderr).lines().filter(|line| line.contains("[slow wait]")) {
            eprintln!("{case}: {line}");
        }
        assert!(
            status.success(),
            "{case}: {status}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let mut processes = [0u32; 2];
    // SAFETY: the array is writable; refuse to touch any shared developer console.
    assert_eq!(unsafe { GetConsoleProcessList(processes.as_mut_ptr(), 2) }, 1);
    assert_eq!(processes[0], std::process::id());
    let console = OpenOptions::new().read(true).write(true).open("CONIN$").unwrap();
    // Harness output stays redirected. Production must read this actual stdin
    // handle, rather than relying on console stdout or silently opening CONIN$.
    assert_ne!(unsafe { SetStdHandle(STD_INPUT_HANDLE, console.as_raw_handle()) }, 0);
    assert_ne!(unsafe { FlushConsoleInputBuffer(console.as_raw_handle()) }, 0);
    assert_ne!(unsafe { SetConsoleMode(console.as_raw_handle(), ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT) }, 0);
    run(console);
}

fn key(unit: u16, repeats: u16) -> INPUT_RECORD {
    let mut event = INPUT_RECORD { EventType: KEY_EVENT as u16, ..INPUT_RECORD::default() };
    event.Event.KeyEvent = KEY_EVENT_RECORD {
        bKeyDown: 1,
        wRepeatCount: repeats,
        wVirtualKeyCode: if matches!(unit, 8 | 13) { unit } else { 0 },
        wVirtualScanCode: if unit == 13 { 0x1c } else { 0 },
        uChar: KEY_EVENT_RECORD_0 { UnicodeChar: unit },
        dwControlKeyState: 0,
    };
    event
}

fn write_records(console: &File, records: &[INPUT_RECORD]) {
    let mut written = 0;
    // SAFETY: records is initialized and count matches its length.
    assert_ne!(
        unsafe { WriteConsoleInputW(console.as_raw_handle(), records.as_ptr(), records.len() as u32, &mut written) },
        0,
        "{}",
        std::io::Error::last_os_error()
    );
    assert_eq!(written, records.len() as u32);
}

fn write_text(console: &File, text: &str) {
    write_records(console, &text.encode_utf16().map(|unit| key(unit, 1)).collect::<Vec<_>>());
}

fn mode(console: &File) -> u32 {
    let mut mode = 0;
    assert_ne!(unsafe { GetConsoleMode(console.as_raw_handle(), &mut mode) }, 0);
    mode
}

/// The console's input queue, as it is.
fn queued(console: &File) -> Vec<INPUT_RECORD> {
    let mut count = 0;
    assert_ne!(unsafe { GetNumberOfConsoleInputEvents(console.as_raw_handle(), &mut count) }, 0);
    let mut records = vec![INPUT_RECORD::default(); count as usize];
    let mut read = 0;
    if count > 0 {
        // SAFETY: records has room for count initialized records.
        assert_ne!(unsafe { PeekConsoleInputW(console.as_raw_handle(), records.as_mut_ptr(), count, &mut read) }, 0);
    }
    records.truncate(read as usize);
    records
}

/// A queued record, briefly, for a failure message.
fn describe(records: &[INPUT_RECORD]) -> String {
    let each = records.iter().map(|record| match u32::from(record.EventType) {
        KEY_EVENT => {
            // SAFETY: KEY_EVENT selects this union member.
            let key = unsafe { record.Event.KeyEvent };
            let unit = unsafe { key.uChar.UnicodeChar };
            format!(
                "key {:?}x{}{} vk{} scan{:#x}",
                char::from_u32(u32::from(unit)).unwrap_or('?'),
                key.wRepeatCount,
                if key.bKeyDown == 0 { " up" } else { "" },
                key.wVirtualKeyCode,
                key.wVirtualScanCode
            )
        }
        FOCUS_EVENT => "focus".into(),
        MENU_EVENT => "menu".into(),
        WINDOW_BUFFER_SIZE_EVENT => "size".into(),
        MOUSE_EVENT => "mouse".into(),
        other => format!("event {other}"),
    });
    each.collect::<Vec<_>>().join(", ")
}

fn queue_empty(console: &File) {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(10);
    loop {
        let records = queued(console);
        if records.is_empty() {
            if started.elapsed() > Duration::from_secs(1) {
                eprintln!("[slow wait] the reader took the console records in {:?}", started.elapsed());
            }
            return;
        }
        if Instant::now() >= deadline {
            // Evidence for #204: does one more input event make a waiting read take what's queued?
            let mut focus = INPUT_RECORD { EventType: FOCUS_EVENT as u16, ..INPUT_RECORD::default() };
            focus.Event.FocusEvent = FOCUS_EVENT_RECORD { bSetFocus: 1 };
            write_records(console, &[focus]);
            std::thread::sleep(Duration::from_secs(2));
            panic!(
                "reader did not consume the prepared console records ({} remain): [{}]; 2 s after one more (focus) event: [{}]",
                records.len(),
                describe(&records),
                describe(&queued(console))
            );
        }
        // Sleep, not yield: on Windows a yield hands the processor only to a thread ready on the same one,
        // so seven tests spinning at once could starve the reader they wait for.
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct InputProbe {
    receive: tokio::sync::oneshot::Receiver<Vec<u8>>,
    bytes: Rc<RefCell<Vec<u8>>>,
    count: usize,
}

fn receive(input: &Stdin, count: usize, pause: bool) -> InputProbe {
    let (send, receive) = tokio::sync::oneshot::channel();
    let send = RefCell::new(Some(send));
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let captured = bytes.clone();
    let input_copy = input.clone();
    input.resume(Rc::new(move |chunk| {
        bytes.borrow_mut().extend_from_slice(chunk);
        if bytes.borrow().len() >= count {
            if pause {
                input_copy.pause();
            }
            if let Some(send) = send.borrow_mut().take() {
                let _ = send.send(bytes.borrow().clone());
            }
        }
    }));
    InputProbe { receive, bytes: captured, count }
}

async fn received(probe: InputProbe, stage: &str) -> Vec<u8> {
    match tokio::time::timeout(Duration::from_secs(5), probe.receive).await {
        Ok(Ok(bytes)) => bytes,
        result => panic!(
            "{stage}: parent input stalled or closed ({result:?}); expected {} bytes, received {} bytes: {:?}; UTF-8 {:?}",
            probe.count,
            probe.bytes.borrow().len(),
            probe.bytes.borrow(),
            String::from_utf8_lossy(&probe.bytes.borrow()),
        ),
    }
}

fn child_reads(console: &File, text: &str) {
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "console_child_reads_exact_line", "--nocapture"])
        .env(CHILD, format!("{text}\r\n"))
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut process = Process(Some(child));
    let child = process.0.as_mut().unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert_ne!(output.read_line(&mut line).unwrap(), 0, "child exited before reading: {line}");
        if line.contains("[console-child-ready]") {
            break;
        }
        line.clear();
    }
    write_text(console, &format!("{text}\r"));
    let status = wait(child, Duration::from_secs(5));
    let mut rest = String::new();
    output.read_to_string(&mut rest).unwrap();
    let mut errors = String::new();
    child.stderr.take().unwrap().read_to_string(&mut errors).unwrap();
    assert!(status.success(), "child read failed: {status}\n{rest}\n{errors}");
    process.0.take();
    queue_empty(console);
}

#[test]
fn console_child_reads_exact_line() {
    let Ok(expected) = std::env::var(CHILD) else { return };
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let mut mode = 0;
    assert_ne!(unsafe { GetConsoleMode(input, &mut mode) }, 0, "child must inherit the owned console input");
    assert_ne!(mode & ENABLE_LINE_INPUT, 0);
    println!("[console-child-ready]");
    std::io::stdout().flush().unwrap();
    let mut buffer = [0u16; 4096];
    let mut read = 0;
    assert_ne!(unsafe { ReadConsoleW(input, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read, std::ptr::null()) }, 0);
    assert_eq!(String::from_utf16_lossy(&buffer[..read as usize]), expected, "child received stale or synthetic input");
}

#[test]
fn raw_console_preserves_vt_unicode_repeats_and_queued_parent_input() {
    isolated("raw_console_preserves_vt_unicode_repeats_and_queued_parent_input", |console| {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(tokio::task::LocalSet::new().run_until(
            async {
                let input = Stdin::new();
                input.on_error(Rc::new(|error| panic!("raw console reader failed: {error}")));
                input.set_raw_mode(true).unwrap();
                let ended = Rc::new(Cell::new(false));
                input.on_end(Rc::new({
                    let ended = ended.clone();
                    move || {
                        eprintln!("raw console reader emitted EOF");
                        ended.set(true)
                    }
                }));
                let first = receive(&input, "aaa😀\x1b[D\0é".len(), true);
                let mut focus = INPUT_RECORD { EventType: FOCUS_EVENT as u16, ..INPUT_RECORD::default() };
                focus.Event.FocusEvent = FOCUS_EVENT_RECORD { bSetFocus: 1 };
                let mut release = key(b'x' as u16, 1);
                release.Event.KeyEvent.bKeyDown = 0;
                write_records(&console, &[focus, release]);
                queue_empty(&console);
                tokio::task::yield_now().await;
                assert!(!ended.get(), "non-text records must not emit EOF");
                // The host expands VT input into text events. WriteConsoleInputW
                // itself collapses a VK0 repeat count and consumes synthetic Alt
                // key-up, as the direct Win32 control proves. Feed realistic text
                // here; decoder unit tests retain those record-level policies.
                write_records(&console, &[key(b'a' as u16, 1), key(b'a' as u16, 1), key(b'a' as u16, 1), key(0xd83d, 1)]);
                // The next batch completes the UTF-16 pair. The intervening message
                // acknowledgement must retain the pending high surrogate.
                tokio::task::yield_now().await;
                write_records(&console, &[key(0xde00, 1), key(27, 1), key(91, 1), key(68, 1), key(0, 1), key(0xe9, 1)]);
                assert_eq!(received(first, "initial raw VT/Unicode/repeat payload").await, "aaa😀\x1b[D\0é".as_bytes());
                input.set_raw_mode(false).unwrap();
                child_reads(&console, "raw-child");

                input.set_raw_mode(true).unwrap();
                // Queue the full boundary while parked. Alternating characters
                // prevent the console from coalescing repeated ASCII records.
                let prefix: String = (0..127).map(|index| if index % 2 == 0 { 'x' } else { 'y' }).collect();
                let expected = format!("{prefix}😀");
                let mut records: Vec<_> = prefix.encode_utf16().map(|unit| key(unit, 1)).collect();
                records.extend([key(0xd83d, 1), key(0xde00, 1)]);
                write_records(&console, &records);
                let mut queued = 0;
                assert_ne!(unsafe { GetNumberOfConsoleInputEvents(console.as_raw_handle(), &mut queued) }, 0);
                assert_eq!(queued, 129, "the high surrogate must be record128 and its low surrogate record129");
                // Pause in the first callback, even if it contains only the ASCII
                // prefix. Waiting for the full expected length would hide a pair
                // split across callbacks and leave its second half for the child.
                let boundary = receive(&input, 1, true);
                assert_eq!(received(boundary, "raw surrogate batch128 boundary").await, expected.as_bytes());
                input.set_raw_mode(false).unwrap();
                child_reads(&console, "surrogate-boundary-child");

                for round in 0..3 {
                    input.set_raw_mode(true).unwrap();
                    let _old_listener = receive(&input, 1, false);
                    write_text(&console, "p");
                    // No LocalSet poll until after pause: the native reader has
                    // queued genuine parent input, which must survive the handoff.
                    queue_empty(&console);
                    input.pause();
                    input.set_raw_mode(false).unwrap();
                    child_reads(&console, &format!("buffered-child-{round}"));
                    assert_eq!(received(receive(&input, 1, true), &format!("raw buffered parent round {round}")).await, b"p");
                }
                assert!(!ended.get());
            },
        ));
    });
}

#[test]
fn cooked_console_pause_preserves_os_editing_and_partial_text() {
    isolated("cooked_console_pause_preserves_os_editing_and_partial_text", |console| {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(tokio::task::LocalSet::new().run_until(
            async {
                let input = Stdin::new();
                input.on_error(Rc::new(|error| panic!("cooked console reader failed: {error}")));
                for round in 0..3 {
                    // An inherited VT flag is legal with cooked input and is restored
                    // exactly by the mode owner; cancellation must work in both modes.
                    let original = ENABLE_LINE_INPUT
                        | ENABLE_ECHO_INPUT
                        | ENABLE_PROCESSED_INPUT
                        | if round == 1 { ENABLE_VIRTUAL_TERMINAL_INPUT } else { 0 };
                    assert_ne!(unsafe { SetConsoleMode(console.as_raw_handle(), original) }, 0);
                    let _waiting = receive(&input, 2, false);
                    assert_eq!(
                        mode(&console),
                        original & !ENABLE_VIRTUAL_TERMINAL_INPUT,
                        "resume must establish cooked editing synchronously"
                    );
                    let _repeated_resume = receive(&input, 2, false);
                    assert_eq!(mode(&console), original & !ENABLE_VIRTUAL_TERMINAL_INPUT);
                    write_text(&console, "ab\u{8}C");
                    queue_empty(&console);
                    input.pause();
                    assert_eq!(mode(&console), original, "pause restores the exact inherited flags");
                    child_reads(&console, &format!("cooked-child-{round}"));
                    assert_eq!(
                        received(receive(&input, 2, true), &format!("cooked edited partial round {round}, mode {original}")).await,
                        b"aC"
                    );
                    assert_eq!(mode(&console), original, "callback pause restores flags after repeated resume");
                }
                let original = ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT;
                assert_ne!(unsafe { SetConsoleMode(console.as_raw_handle(), original) }, 0);
                let _waiting = receive(&input, 11, false);
                write_text(&console, "mode-parent");
                queue_empty(&console);
                input.set_raw_mode(true).unwrap();
                assert_eq!(mode(&console), ENABLE_VIRTUAL_TERMINAL_INPUT);
                input.pause();
                input.set_raw_mode(false).unwrap();
                assert_eq!(mode(&console), original, "RawMode must snapshot the inherited flags, not temporary cooked flags");
                child_reads(&console, "mode-change-child");
                assert_eq!(received(receive(&input, 11, true), "cooked partial across raw-mode change").await, b"mode-parent");
                assert_eq!(mode(&console), original);
                // Dropping a pending cooked read must release console ownership too.
                input.resume(Rc::new(|_| panic!("unexpected parent input")));
                assert_eq!(mode(&console), original & !ENABLE_VIRTUAL_TERMINAL_INPUT);
                write_text(&console, "drop-parent");
                queue_empty(&console);
                drop(input);
                assert_eq!(mode(&console), original, "Drop restores inherited cooked VT mode");
                child_reads(&console, "drop-child");
            },
        ));
    });
}

#[test]
fn cooked_console_eof_is_once_and_leaves_no_cancellation_input() {
    isolated("cooked_console_eof_is_once_and_leaves_no_cancellation_input", |console| {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(tokio::task::LocalSet::new().run_until(
            async {
                let input = Stdin::new();
                let ended = Rc::new(Cell::new(0));
                let (send, receive) = tokio::sync::oneshot::channel();
                let send = RefCell::new(Some(send));
                input.on_end(Rc::new({
                    let ended = ended.clone();
                    move || {
                        ended.set(ended.get() + 1);
                        if let Some(send) = send.borrow_mut().take() {
                            let _ = send.send(());
                        }
                    }
                }));
                input.resume(Rc::new(|_| panic!("Ctrl+Z at the start of a cooked line must be EOF")));
                write_text(&console, "\u{1a}\r");
                queue_empty(&console);
                input.pause();
                child_reads(&console, "eof-child");
                input.resume(Rc::new(|_| panic!("data after EOF")));
                tokio::time::timeout(Duration::from_secs(5), receive).await.unwrap().unwrap();
                input.pause();
                input.resume(Rc::new(|_| panic!("data after repeated EOF resume")));
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
                assert_eq!(ended.get(), 1);
                input.pause();
            },
        ));
    });
}

#[test]
fn natural_newline_racing_pause_never_reaches_the_child() {
    isolated("natural_newline_racing_pause_never_reaches_the_child", |console| {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(tokio::task::LocalSet::new().run_until(
            async {
                let input = Stdin::new();
                for round in 0..32 {
                    let original = ENABLE_LINE_INPUT
                        | ENABLE_ECHO_INPUT
                        | ENABLE_PROCESSED_INPUT
                        | if round >= 16 { ENABLE_VIRTUAL_TERMINAL_INPUT } else { 0 };
                    assert_ne!(unsafe { SetConsoleMode(console.as_raw_handle(), original) }, 0);
                    let _waiting = receive(&input, 8, false);
                    assert_eq!(mode(&console), original & !ENABLE_VIRTUAL_TERMINAL_INPUT);
                    write_text(&console, "parent");
                    queue_empty(&console);
                    // Enqueue the real Return first, then race its completion
                    // publication. Any synthetic Return belongs solely to the parent.
                    write_text(&console, "\r");
                    input.pause();
                    assert_eq!(mode(&console), original);
                    child_reads(&console, &format!("race-child-{round}"));
                    assert_eq!(
                        received(receive(&input, 8, true), &format!("natural newline race round {round}, mode {original}")).await,
                        b"parent\r\n"
                    );
                }
            },
        ));
    });
}

#[test]
fn cooked_pause_finishes_buffered_line_tails_before_child_handoff() {
    isolated("cooked_pause_finishes_buffered_line_tails_before_child_handoff", |console| {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(tokio::task::LocalSet::new().run_until(
            async {
                let input = Stdin::new();
                let mut cases = [4095, 4096, 4097, 8191].map(|length| "x".repeat(length)).to_vec();
                // The high surrogate is WCHAR4096; its low surrogate begins the next buffer.
                cases.push(format!("{}😀", "x".repeat(4095)));
                for (case, text) in cases.into_iter().enumerate() {
                    for newline in [false, true] {
                        let expected = if newline { format!("{text}\r\n") } else { text.clone() };
                        let _waiting = receive(&input, expected.len(), false);
                        write_text(&console, &text);
                        queue_empty(&console);
                        if newline {
                            write_text(&console, "\r");
                        }
                        input.pause();
                        child_reads(&console, &format!("tail-child-{case}-{newline}"));
                        assert_eq!(
                            received(receive(&input, expected.len(), true), &format!("long cooked case {case}, newline {newline}")).await,
                            expected.as_bytes()
                        );
                    }
                }
            },
        ));
    });
}

#[test]
fn unavailable_windows_stdin_reports_one_error_without_panicking() {
    isolated("unavailable_windows_stdin_reports_one_error_without_panicking", |console| {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(tokio::task::LocalSet::new().run_until(
            async {
                assert_ne!(unsafe { SetStdHandle(STD_INPUT_HANDLE, std::ptr::null_mut()) }, 0);
                let input = Stdin::new();
                let errors = Rc::new(Cell::new(0));
                let (send, receive) = tokio::sync::oneshot::channel();
                let send = RefCell::new(Some(send));
                input.on_error(Rc::new({
                    let errors = errors.clone();
                    move |_| {
                        errors.set(errors.get() + 1);
                        if let Some(send) = send.borrow_mut().take() {
                            let _ = send.send(());
                        }
                    }
                }));
                input.resume(Rc::new(|_| panic!("unavailable stdin emitted data")));
                tokio::time::timeout(Duration::from_secs(5), receive).await.unwrap().unwrap();
                input.pause();
                input.resume(Rc::new(|_| panic!("unavailable stdin emitted data after resume")));
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
                assert_eq!(errors.get(), 1);
                drop(input);
                assert_ne!(unsafe { SetStdHandle(STD_INPUT_HANDLE, console.as_raw_handle()) }, 0);
            },
        ));
    });
}
