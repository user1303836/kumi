#![cfg(windows)]

use kumi::input::TerminalInput;
use kumi::tui::keys::{InputEvent, InputParser, Modifiers};
use kumi::tui::tty::Stdin;
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::os::windows::{io::AsRawHandle, process::CommandExt};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Console::{
    FlushConsoleInputBuffer, GetConsoleMode, GetConsoleProcessList, SetConsoleMode, SetStdHandle, WriteConsoleInputW, ENABLE_ECHO_INPUT,
    ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, ENHANCED_KEY, INPUT_RECORD, KEY_EVENT, KEY_EVENT_RECORD,
    KEY_EVENT_RECORD_0, STD_INPUT_HANDLE,
};

const PROBE: &str = "KUMI_TEST_WINDOWS_INPUT_CONSOLE";
const CASE: &str = "raw_byte_input_enables_vt_and_restores_the_actual_windows_console";
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

fn mode(console: &File) -> u32 {
    let mut mode = 0;
    // SAFETY: console owns a live console-input handle; mode is writable.
    assert_ne!(unsafe { GetConsoleMode(console.as_raw_handle(), &mut mode) }, 0, "{}", std::io::Error::last_os_error());
    mode
}

fn set_mode(console: &File, mode: u32) {
    // SAFETY: console owns a live console-input handle.
    assert_ne!(unsafe { SetConsoleMode(console.as_raw_handle(), mode) }, 0, "{}", std::io::Error::last_os_error());
}

struct Restore<'a>(&'a File, u32);
impl Drop for Restore<'_> {
    fn drop(&mut self) {
        // SAFETY: the borrowed file outlives this guard, including unwinding.
        unsafe { SetConsoleMode(self.0.as_raw_handle(), self.1) };
    }
}

#[test]
fn raw_byte_input_enables_vt_and_restores_the_actual_windows_console() {
    if std::env::var_os(PROBE).is_none() {
        // Use a new console, never the runner's or developer's console. The child
        // opens CONIN$ just as production does; harness output remains in pipes.
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CASE, "--nocapture"])
            .env(PROBE, "1")
            .creation_flags(CREATE_NEW_CONSOLE)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "console probe {}\nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let mut processes = [0; 2];
    // SAFETY: processes is writable for its declared length. Refuse to alter a
    // shared console even if someone manually sets the subprocess marker.
    let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) };
    assert_eq!(count, 1, "the probe must own its console");
    assert_eq!(processes[0], std::process::id());
    let console = OpenOptions::new().read(true).write(true).open("CONIN$").unwrap();
    let original = mode(&console);
    let _restore = Restore(&console, original);
    let cooked = ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT;
    let without_vt = original & !ENABLE_VIRTUAL_TERMINAL_INPUT;
    for initial in [
        (without_vt | ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT) & !ENABLE_ECHO_INPUT,
        without_vt & !cooked,
        original | ENABLE_VIRTUAL_TERMINAL_INPUT,
    ] {
        set_mode(&console, initial);
        let input = Stdin::new();
        assert!(!input.is_raw(), "a new stream does not own inherited raw mode");
        input.set_raw_mode(true).unwrap();
        let raw = (initial & !cooked) | ENABLE_VIRTUAL_TERMINAL_INPUT;
        assert_eq!(mode(&console), raw, "byte input requires VT conversion");
        assert!(input.is_raw());
        let nested_was_raw = input.is_raw();
        input.set_raw_mode(true).unwrap();
        input.set_raw_mode(nested_was_raw).unwrap();
        assert_eq!(mode(&console), raw);
        input.set_raw_mode(false).unwrap();
        input.set_raw_mode(false).unwrap();
        assert_eq!(mode(&console), initial, "restore all original bits, including VT");
        assert!(!input.is_raw());

        for order in [[false, true], [true, false]] {
            input.set_raw_mode(true).unwrap();
            let outer = input.emergency_raw_mode().unwrap();
            input.set_raw_mode(true).unwrap();
            let inner = input.emergency_raw_mode().unwrap();
            for was_raw in order {
                let restore = if was_raw { inner.clone() } else { outer.clone() };
                std::thread::spawn(move || restore(was_raw)).join().unwrap();
            }
            assert_eq!(mode(&console), initial, "nested emergency cleanup must not re-enable raw mode");
            assert!(!input.is_raw());
        }
        input.set_raw_mode(true).unwrap();
        drop(input);
        assert_eq!(mode(&console), initial, "dropping the raw owner restores the snapshot");
    }
}

#[test]
fn injected_console_navigation_reaches_the_input_parser_from_initially_disabled_vt() {
    const CASE: &str = "injected_console_navigation_reaches_the_input_parser_from_initially_disabled_vt";
    const PROBE: &str = "KUMI_TEST_WINDOWS_NAVIGATION_CONSOLE";
    if std::env::var_os(PROBE).is_none() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CASE, "--nocapture"])
            .env(PROBE, "1")
            .creation_flags(CREATE_NEW_CONSOLE)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                break child.wait().unwrap();
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let output = child.wait_with_output().unwrap();
        assert!(
            status.success(),
            "navigation console probe {status}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let mut processes = [0; 2];
    // SAFETY: refuse to alter a console that is shared with another process.
    assert_eq!(unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) }, 1);
    assert_eq!(processes[0], std::process::id());
    let console = OpenOptions::new().read(true).write(true).open("CONIN$").unwrap();
    let original = mode(&console);
    let _restore = Restore(&console, original);
    assert_ne!(unsafe { SetStdHandle(STD_INPUT_HANDLE, console.as_raw_handle()) }, 0);
    assert_ne!(unsafe { FlushConsoleInputBuffer(console.as_raw_handle()) }, 0);
    let initial = (original | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT) & !ENABLE_VIRTUAL_TERMINAL_INPUT;
    set_mode(&console, initial);

    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(tokio::task::LocalSet::new().run_until(async {
        let input = Stdin::new();
        input.on_error(Rc::new(|error| panic!("navigation reader failed: {error}")));
        input.set_raw_mode(true).unwrap();
        assert_ne!(mode(&console) & ENABLE_VIRTUAL_TERMINAL_INPUT, 0);
        let events = Rc::new(RefCell::new(Vec::new()));
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let (send, receive) = tokio::sync::oneshot::channel();
        let send = RefCell::new(Some(send));
        let parser = InputParser::new(Rc::new({
            let events = events.clone();
            move |event| {
                events.borrow_mut().push(event);
                if events.borrow().len() >= 4 {
                    if let Some(send) = send.borrow_mut().take() {
                        let _ = send.send(());
                    }
                }
            }
        }));
        input.resume(Rc::new({
            let bytes = bytes.clone();
            let parser = parser.clone();
            move |chunk| {
                bytes.borrow_mut().extend_from_slice(chunk);
                parser.push(std::str::from_utf8(chunk).expect("navigation sequences are ASCII"));
            }
        }));
        // These are injected Win32 key events, not pre-encoded escape bytes.
        // The actual console host must convert them before Stdin and InputParser.
        let records = [(0x26, 0x48), (0x28, 0x50), (0x25, 0x4b), (0x27, 0x4d)].map(|(virtual_key, scan)| {
            let mut record = INPUT_RECORD { EventType: KEY_EVENT as u16, ..INPUT_RECORD::default() };
            record.Event.KeyEvent = KEY_EVENT_RECORD {
                bKeyDown: 1,
                wRepeatCount: 1,
                wVirtualKeyCode: virtual_key,
                wVirtualScanCode: scan,
                uChar: KEY_EVENT_RECORD_0 { UnicodeChar: 0 },
                dwControlKeyState: ENHANCED_KEY,
            };
            record
        });
        let mut written = 0;
        assert_ne!(unsafe { WriteConsoleInputW(console.as_raw_handle(), records.as_ptr(), records.len() as u32, &mut written) }, 0);
        assert_eq!(written, records.len() as u32);
        let result = tokio::time::timeout(Duration::from_secs(5), receive).await;
        input.pause();
        input.set_raw_mode(false).unwrap();
        parser.dispose();
        assert_eq!(mode(&console), initial, "navigation restores the exact initial console mode");
        assert!(matches!(result, Ok(Ok(()))), "navigation stalled: {result:?}; bytes={:?}; events={:?}", bytes.borrow(), events.borrow());
        assert_eq!(bytes.borrow().as_slice(), b"\x1b[A\x1b[B\x1b[D\x1b[C");
        assert_eq!(*events.borrow(), ["up", "down", "left", "right"].map(|name| InputEvent::key(name, Modifiers::NONE)));
    }));
}
