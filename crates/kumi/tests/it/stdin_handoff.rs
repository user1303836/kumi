use std::cell::{Cell, RefCell};
use std::io::{BufRead, Read, Write};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use kumi::input::TerminalInput;
use kumi::tui::tty::Stdin;

const ROLE: &str = "KUMI_STDIN_HANDOFF_FIXTURE";
const WRITTEN: &str = "KUMI_STDIN_HANDOFF_WRITTEN";

fn marker(text: &str) {
    println!("[handoff] {text}");
    std::io::stdout().flush().unwrap();
}

fn receive(input: &Stdin, length: usize, pause_inside: bool) -> tokio::sync::oneshot::Receiver<Vec<u8>> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let send = RefCell::new(Some(send));
    let bytes = RefCell::new(Vec::new());
    let source = input.clone();
    input.resume(Rc::new(move |chunk| {
        bytes.borrow_mut().extend_from_slice(chunk);
        if bytes.borrow().len() >= length {
            if pause_inside {
                source.pause();
            }
            if let Some(send) = send.borrow_mut().take() {
                let _ = send.send(std::mem::take(&mut *bytes.borrow_mut()));
            }
        }
    }));
    receive
}

#[test]
fn stdin_fixture_process() {
    let Ok(role) = std::env::var(ROLE) else { return };
    if role == "reopened" {
        marker("child-ready");
        // Input can arrive before the reopened application has finished starting.
        std::thread::sleep(Duration::from_millis(100));
        let mut input = [0; 6];
        std::io::stdin().read_exact(&mut input).unwrap();
        assert_eq!(&input, b"child\n");
        marker("child-received");
        return;
    }
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(tokio::task::LocalSet::new().run_until(async {
        let input = Stdin::new();
        let terminal = input.is_tty();
        #[cfg(unix)]
        let before = terminal.then(|| {
            let mut mode: libc::termios = unsafe { std::mem::zeroed() };
            assert_eq!(unsafe { libc::tcgetattr(0, &mut mode) }, 0);
            mode
        });
        if terminal {
            input.set_raw_mode(true).unwrap();
        }
        let initial = if role == "tail" { vec![b'p'; 4096] } else { vec![b'a'] };
        if role == "buffered" {
            #[cfg(unix)]
            {
                marker("parent-ready");
                let written = std::path::PathBuf::from(std::env::var_os(WRITTEN).unwrap());
                let limit = std::time::Instant::now() + Duration::from_secs(5);
                while !written.exists() {
                    assert!(std::time::Instant::now() < limit, "supervisor did not write input");
                    std::thread::sleep(Duration::from_millis(1));
                }
                let _waiting = receive(&input, 1, false);
                // Hold the LocalSet until the native reader has consumed the byte.
                // pause must retain that queued byte without taking the child's input.
                loop {
                    let mut available = 0;
                    assert_eq!(unsafe { libc::ioctl(0, libc::FIONREAD, &mut available) }, 0);
                    if available == 0 {
                        break;
                    }
                    assert!(std::time::Instant::now() < limit, "native reader did not read input");
                    std::thread::yield_now();
                }
                input.pause();
            }
        } else {
            let received = receive(&input, initial.len(), !terminal);
            marker("parent-ready");
            assert_eq!(received.await.unwrap(), initial);
            if terminal {
                // Exercise a mode change before pause too: readiness from raw mode
                // must not admit a blocking read after canonical mode is restored.
                input.set_raw_mode(false).unwrap();
                #[cfg(unix)]
                {
                    let mut after: libc::termios = unsafe { std::mem::zeroed() };
                    assert_eq!(unsafe { libc::tcgetattr(0, &mut after) }, 0);
                    let before = before.as_ref().unwrap();
                    // Darwin sets PENDIN when ICANON is restored, even for a direct
                    // crossterm toggle with no reader. It is pending-input state,
                    // cleared by the next poll/read, rather than a configured mode.
                    let configured = |flags| flags & !libc::PENDIN;
                    assert_eq!(
                        (after.c_iflag, after.c_oflag, after.c_cflag, configured(after.c_lflag), after.c_cc),
                        (before.c_iflag, before.c_oflag, before.c_cflag, configured(before.c_lflag), before.c_cc)
                    );
                }
            }
            input.pause();
        }
        let input = if role == "drop" {
            drop(input);
            None
        } else {
            Some(input)
        };
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "stdin_fixture_process", "--nocapture"])
            .env(ROLE, "reopened")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        if completed.is_err() {
            child.kill().await.unwrap();
            let stolen = if let Some(input) = &input {
                Some(tokio::time::timeout(Duration::from_secs(1), receive(input, 6, true)).await)
            } else {
                None
            };
            panic!("reopened child starved; paused parent retained: {stolen:?}");
        }
        assert!(completed.unwrap().unwrap().success());
        if let Some(input) = input {
            if role == "buffered" {
                assert_eq!(receive(&input, 1, true).await.unwrap(), b"a");
            }
            for round in 0..8 {
                let expected = format!("round-{round}\n").into_bytes();
                let received = receive(&input, expected.len(), true);
                marker(&format!("round-{round}"));
                assert_eq!(received.await.unwrap(), expected);
                input.pause();
            }
            let ended = Rc::new(Cell::new(0));
            let (send, end) = tokio::sync::oneshot::channel();
            let send = RefCell::new(Some(send));
            input.on_end(Rc::new({
                let ended = ended.clone();
                let input = input.clone();
                move || {
                    ended.set(ended.get() + 1);
                    input.pause();
                    if let Some(send) = send.borrow_mut().take() {
                        let _ = send.send(());
                    }
                }
            }));
            input.resume(Rc::new(|bytes| panic!("unexpected bytes before EOF: {bytes:?}")));
            marker("eof-ready");
            end.await.unwrap();
            input.on_end(Rc::new({
                let ended = ended.clone();
                move || ended.set(ended.get() + 1)
            }));
            input.resume(Rc::new(|_| panic!("data after EOF")));
            for _ in 0..4 {
                tokio::task::yield_now().await;
            }
            assert_eq!(ended.get(), 1, "resume repeated EOF");
            input.pause();
            drop(input);
        }
        marker("complete");
    }));
}

struct Process(Option<Child>);
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn command(role: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "stdin_fixture_process", "--nocapture"]).env(ROLE, role).stdout(Stdio::piped()).stderr(Stdio::piped());
    command
}

fn drive(child: Child, input: impl Write, role: &str, terminal: bool, written: Option<&std::path::Path>) {
    let mut process = Process(Some(child));
    let mut input = Some(input);
    let (send, receive) = std::sync::mpsc::channel();
    let stdout = process.0.as_mut().unwrap().stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut output = Vec::new();
    let mut complete = false;
    while let Ok(line) = receive.recv_timeout(Duration::from_secs(10)) {
        if line.contains("[handoff] parent-ready") {
            let bytes = if role == "tail" { [vec![b'p'; 4096], b"child\n".to_vec()].concat() } else { vec![b'a'] };
            input.as_mut().unwrap().write_all(&bytes).unwrap();
            if let Some(path) = written {
                std::fs::write(path, b"written").unwrap();
            }
        }
        if line.contains("[handoff] child-ready") && role != "tail" {
            input.as_mut().unwrap().write_all(b"child\n").unwrap();
        }
        if let Some(round) = line.strip_prefix("[handoff] round-") {
            input.as_mut().unwrap().write_all(format!("round-{round}\n").as_bytes()).unwrap();
        }
        if line.contains("[handoff] eof-ready") {
            if terminal {
                input.as_mut().unwrap().write_all(&[4]).unwrap();
            } else {
                input.take();
            }
        }
        if line.contains("[handoff] complete") {
            complete = true;
        }
        output.push(line);
    }
    if !complete {
        let _ = process.0.as_mut().unwrap().kill();
    }
    drop(input);
    let result = process.0.take().unwrap().wait_with_output().unwrap();
    reader.join().unwrap();
    assert!(
        complete && result.status.success(),
        "{role} handoff failed: {:?}\n{}\n{}",
        result.status,
        output.join("\n"),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn pipe(role: &str) {
    let dir = tempfile::tempdir().unwrap();
    let written = dir.path().join("written");
    let mut child = command(role).env(WRITTEN, &written).stdin(Stdio::piped()).spawn().unwrap();
    let input = child.stdin.take().unwrap();
    drive(child, input, role, false, Some(&written));
}

#[test]
fn paused_pipe_releases_input_and_callbacks_can_pause_repeatedly_through_eof() {
    pipe("pipe");
}

#[test]
fn no_private_read_ahead_hides_the_inherited_pipe_tail() {
    pipe("tail");
}

#[test]
fn dropping_stdin_stops_its_reader_before_a_child_inherits_input() {
    pipe("drop");
}

#[cfg(unix)]
#[test]
fn bytes_read_before_pause_survive_until_resume() {
    pipe("buffered");
}

#[cfg(unix)]
#[test]
fn paused_terminal_leaves_input_for_reopened_child_and_restores_raw_mode() {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::process::CommandExt;
    let (master, slave) = unsafe {
        let (mut master, mut slave) = (-1, -1);
        assert_eq!(libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()), 0);
        assert_eq!(libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC), 0);
        assert_eq!(libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC), 0);
        (std::fs::File::from_raw_fd(master), std::fs::File::from_raw_fd(slave))
    };
    let mut command = command("terminal");
    command.stdin(slave);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    // macOS drains terminal echo before a session leader exits.
    let drain_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop = drain_done.clone();
    let mut drain = master.try_clone().unwrap();
    let drainer = std::thread::spawn(move || {
        let mut buffer = [0u8; 1024];
        while !stop.load(std::sync::atomic::Ordering::SeqCst) {
            let mut fd = libc::pollfd { fd: drain.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            if unsafe { libc::poll(&mut fd, 1, 20) } > 0 && drain.read(&mut buffer).is_err() {
                break;
            }
        }
    });
    drive(command.spawn().unwrap(), master, "terminal", true, None);
    drain_done.store(true, std::sync::atomic::Ordering::SeqCst);
    drainer.join().unwrap();
}
