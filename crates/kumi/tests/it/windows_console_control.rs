#![cfg(windows)]

//! Direct Win32 controls for the synthetic records used by windows_stdin.
//! These observations deliberately do not use Kumi's input reader or assume
//! that WriteConsoleInputW preserves records when VT input is enabled.

use std::fs::{File, OpenOptions};
use std::os::windows::{io::AsRawHandle, process::CommandExt};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Console::*;

const PROBE: &str = "KUMI_WINDOWS_DIRECT_CONSOLE_CONTROL";
const CASE: &str = "direct_console_reports_synthetic_input_with_and_without_vt";
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

fn describe(records: &[INPUT_RECORD]) -> Vec<String> {
    records
        .iter()
        .map(|record| {
            if record.EventType == KEY_EVENT as u16 {
                // SAFETY: EventType identifies the initialized union member.
                let event = unsafe { record.Event.KeyEvent };
                format!(
                    "key(down={},repeat={},vk={:04x},scan={:04x},char={:04x},control={:08x})",
                    event.bKeyDown,
                    event.wRepeatCount,
                    event.wVirtualKeyCode,
                    event.wVirtualScanCode,
                    unsafe { event.uChar.UnicodeChar },
                    event.dwControlKeyState
                )
            } else {
                format!("event(type={:04x})", record.EventType)
            }
        })
        .collect()
}

fn write_records(console: &File, records: &[INPUT_RECORD]) {
    let mut written = 0;
    // SAFETY: the file and initialized records remain alive for the call.
    assert_ne!(
        unsafe { WriteConsoleInputW(console.as_raw_handle(), records.as_ptr(), records.len() as u32, &mut written) },
        0,
        "{}",
        std::io::Error::last_os_error()
    );
    assert_eq!(written, records.len() as u32);
}

fn raw_records() -> Vec<INPUT_RECORD> {
    let mut focus = INPUT_RECORD { EventType: FOCUS_EVENT as u16, ..INPUT_RECORD::default() };
    focus.Event.FocusEvent = FOCUS_EVENT_RECORD { bSetFocus: 1 };
    let mut release = key(b'x' as u16, 1);
    unsafe { release.Event.KeyEvent.bKeyDown = 0 };
    let mut alt_release = key(0xe9, 1);
    unsafe {
        alt_release.Event.KeyEvent.bKeyDown = 0;
        alt_release.Event.KeyEvent.wVirtualKeyCode = 0x12;
    }
    vec![
        focus,
        release,
        key(b'a' as u16, 3),
        key(0xd83d, 1),
        key(0xde00, 1),
        key(0x1b, 1),
        key(b'[' as u16, 1),
        key(b'D' as u16, 1),
        key(0, 1),
        alt_release,
    ]
}

fn run_control(scenario: &str, vt: bool) {
    let mut processes = [0; 2];
    // SAFETY: processes is writable. Never modify a shared developer console.
    assert_eq!(unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) }, 1);
    assert_eq!(processes[0], std::process::id());
    let console = OpenOptions::new().read(true).write(true).open("CONIN$").unwrap();
    let cooked = scenario.starts_with("cooked");
    let mode = if cooked { ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT } else { 0 }
        | if vt { ENABLE_VIRTUAL_TERMINAL_INPUT } else { 0 };
    // SAFETY: this subprocess exclusively owns the console and valid input handle.
    assert_ne!(unsafe { FlushConsoleInputBuffer(console.as_raw_handle()) }, 0);
    assert_ne!(unsafe { SetConsoleMode(console.as_raw_handle(), mode) }, 0);
    let mut records = if cooked { "ab\u{8}C".encode_utf16().map(|unit| key(unit, 1)).collect::<Vec<_>>() } else { raw_records() };
    if cooked {
        let mut enter = key(13, 1);
        if scenario.contains("tagged") {
            unsafe { enter.Event.KeyEvent.wVirtualScanCode = 0xfffe };
        }
        records.push(enter);
    } else if scenario.ends_with("text") {
        // A printable sentinel bounds the direct raw text read without assuming
        // whether the host emits NUL, Alt-keyup, or repeated synthetic keys.
        records.extend("|END|".encode_utf16().map(|unit| key(unit, 1)));
    }
    println!("[windows-console-control] scenario={scenario} vt={vt} mode={mode:#x} injected={:?}", describe(&records));
    write_records(&console, &records);
    if scenario.ends_with("records") {
        let mut count = 0;
        assert_ne!(unsafe { GetNumberOfConsoleInputEvents(console.as_raw_handle(), &mut count) }, 0);
        let mut records = vec![INPUT_RECORD::default(); count as usize];
        let mut read = 0;
        if count != 0 {
            assert_ne!(unsafe { ReadConsoleInputW(console.as_raw_handle(), records.as_mut_ptr(), count, &mut read) }, 0);
        }
        println!(
            "[windows-console-control] scenario={scenario} vt={vt} queued={count} read={read} records={:?}",
            describe(&records[..read as usize])
        );
    } else {
        let mut units = Vec::new();
        loop {
            let mut buffer = [0u16; 256];
            let mut read = 0;
            assert_ne!(
                unsafe {
                    ReadConsoleW(console.as_raw_handle(), buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read, std::ptr::null())
                },
                0,
                "{}",
                std::io::Error::last_os_error()
            );
            units.extend_from_slice(&buffer[..read as usize]);
            println!(
                "[windows-console-control] scenario={scenario} vt={vt} read={read} units={units:04x?} text={:?}",
                String::from_utf16_lossy(&units)
            );
            assert_ne!(read, 0, "direct control unexpectedly reached EOF");
            assert!(units.len() < 4096, "direct control exceeded its bounded payload");
            if cooked || units.ends_with(&"|END|".encode_utf16().collect::<Vec<_>>()) {
                break;
            }
        }
    }
}

#[test]
fn direct_console_reports_synthetic_input_with_and_without_vt() {
    if let Ok(control) = std::env::var(PROBE) {
        let (scenario, vt) = control.split_once(':').unwrap();
        run_control(scenario, vt == "true");
        return;
    }
    for scenario in
        ["raw-records", "raw-text", "cooked-natural-records", "cooked-natural-text", "cooked-tagged-records", "cooked-tagged-text"]
    {
        for vt in [false, true] {
            let child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", CASE, "--nocapture"])
                .env(PROBE, format!("{scenario}:{vt}"))
                .creation_flags(CREATE_NEW_CONSOLE)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut process = Process(Some(child));
            let deadline = Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = process.0.as_mut().unwrap().try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    let child = process.0.as_mut().unwrap();
                    child.kill().unwrap();
                    break child.wait().unwrap();
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            let output = process.0.take().unwrap().wait_with_output().unwrap();
            // Forward successful observations as well as failures to CI logs.
            print!("{}", String::from_utf8_lossy(&output.stdout));
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
            assert!(status.success(), "direct console control failed: scenario={scenario} vt={vt} {status}");
        }
    }
}
