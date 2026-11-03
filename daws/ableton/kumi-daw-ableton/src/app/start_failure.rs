//! Why Live couldn't start AbletonMcpBridge, from Live's own log. A Control Surface whose script fails as Live starts
//! it shows nothing in Live, so setup and the session would go on asking for it to be chosen, which it is. Live's
//! Log.txt has the script's traceback (its port held by another program, Kumi's bridge files failing the script's
//! checks): that is said instead, with what fixes it.
use super::{
    bridge_setup::{is_live_running_on, live_pids_on},
    doctor::{bridge_ports, is_live, port_owner, PortOwner},
    report::logs_in,
};
use kumi_daw::process::Run;
use kumi_types::system::Env;
use regex::Regex;
use std::{
    sync::{LazyLock, Mutex},
    time::{Duration, Instant, SystemTime},
};

/// Where a start of Live begins in its log: Live adds to one log for each version, start after start.
const STARTED: &str = "info: Started: Live";
/// Live's own line once a Control Surface's script raised as it was loaded or made.
const NOT_LOADED: &str = "Exception: Script could not be loaded.";
/// A start of the script, from _Framework, before any of the bridge's own code runs.
const INITIALIZING: &str = "(AbletonMcpBridge) Initializing...";
/// The Remote Script's own line for a start that failed (since the bridge that says it in Live's status bar too), the
/// exception after it: one that starts later than Live made it (on Windows, once its files' ACLs are checked off Live's
/// thread) fails without a traceback Live reports.
const SAID_FAILED: &str = "RemoteScriptMessage: (AbletonMcpBridge) AbletonMcpBridge couldn't start: ";

/// A failed start of AbletonMcpBridge in Live, as Live's log has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failed {
    /// The exception's last line, put back together from the pieces Live logs it in
    /// ("OSError: [Errno 48] Address already in use").
    pub(crate) exception: String,
    /// How many of Live's Control Surface slots have AbletonMcpBridge, by the last list of them.
    pub(crate) slots: usize,
}

/// A line of Live's log after its time and level ("2026-11-02T23:24:41.316608: error: "); None for a line of its own: a
/// row of a list, or the end of the line a piece of a traceback ended.
fn entry(line: &str) -> Option<&str> {
    static ENTRY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d\d-\d\dT[\d:.]+: [a-z]+: ").unwrap());
    ENTRY.find(line).map(|at| &line[at.end()..])
}

/// The last start of AbletonMcpBridge in `log`'s last start of Live, when it failed: the last traceback through the
/// bridge's own start (its `create_instance`, or its files as they're loaded) that Live followed with "Script could not
/// be loaded", with no start of the bridge after it, and the bridge still in a Control Surface slot (Live lists them
/// again when they change in its Settings).
pub(crate) fn failed_start(log: &str) -> Option<Failed> {
    // A frame of the bridge's start, by its folder ("…/Remote Scripts/AbletonMcpBridge/__init__.py"; `\` on Windows).
    static OURS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"File "[^"]*[/\\]AbletonMcpBridge[/\\][^"]*", line \d+, in (create_instance|<module>)"#).unwrap());
    static SLOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^\s*MidiRemoteScript \d+ \[Control Surface="([^"]*)""#).unwrap());
    let run = log.rfind(STARTED).map_or(log, |at| &log[at..]);
    let mut failed = None;
    // The traceback being logged: Live logs each piece Python writes as an entry of its own, and an empty line after a
    // piece that ended a line.
    let mut traceback: Option<String> = None;
    // Live's list of Control Surface slots while it's read, how many have the bridge in it, and in the last list.
    let (mut listing, mut listed, mut slots) = (false, 0, None);
    for line in run.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some(text) = entry(line) else {
            if listing {
                if let Some(slot) = SLOT.captures(line) {
                    listed += usize::from(&slot[1] == "AbletonMcpBridge");
                    continue;
                }
            }
            if let Some(traceback) = traceback.as_mut().filter(|_| line.is_empty()) {
                traceback.push('\n');
            }
            continue;
        };
        if std::mem::take(&mut listing) {
            slots = Some(listed);
        }
        if let Some(piece) = text.strip_prefix("RemoteScriptError:") {
            let piece = piece.strip_prefix(' ').unwrap_or(piece);
            if piece.starts_with("Traceback (most recent call last):") {
                traceback = Some(String::new());
            }
            if let Some(traceback) = traceback.as_mut() {
                traceback.push_str(piece);
            }
        } else if text.starts_with("AMidiIO: Midi Remote Scripts:") {
            (listing, listed) = (true, 0);
        } else if text.ends_with(INITIALIZING) {
            (failed, traceback) = (None, None);
        } else if let Some(exception) = text.strip_prefix(SAID_FAILED).filter(|exception| !exception.trim().is_empty()) {
            failed = Some(exception.trim().to_string());
        } else if text == NOT_LOADED {
            if let Some(traceback) = traceback.take().filter(|traceback| OURS.is_match(traceback)) {
                failed = traceback.lines().map(str::trim).filter(|line| !line.is_empty()).last().map(str::to_string);
            }
        }
    }
    if listing {
        slots = Some(listed);
    }
    let slots = slots.unwrap_or(1);
    failed.filter(|_| slots > 0).map(|exception| Failed { exception, slots })
}

/// `log` from the line of its last start of Live on, read from its end back: a log that has kept many starts isn't read
/// whole.
fn current_run(log: &str) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 256 * 1024;
    let mut file = std::fs::File::open(log)?;
    let mut end = file.metadata()?.len();
    let mut text: Vec<u8> = vec![];
    // Where the last start's words are in `text`, once found.
    let mut marker: Option<usize> = None;
    while end > 0 {
        let start = end.saturating_sub(CHUNK);
        let mut chunk = vec![0; (end - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut chunk)?;
        let grown = chunk.len();
        chunk.extend_from_slice(&text);
        text = chunk;
        match marker.as_mut() {
            Some(at) => *at += grown,
            // The new part, and words that run on into the part read before.
            None => {
                marker = text[..(grown + STARTED.len() - 1).min(text.len())].windows(STARTED.len()).rposition(|w| w == STARTED.as_bytes())
            }
        }
        // From the start of its line: its time tells this start of Live from the next.
        if let Some(line) = marker.and_then(|at| text[..at].iter().rposition(|&b| b == b'\n')) {
            text.drain(..=line);
            break;
        }
        end = start;
    }
    Ok(String::from_utf8_lossy(&text).into_owned())
}

/// A failed start, and the start of Live it was in (its line in the log).
type Found = (String, Failed);
/// The newest log as it was last read: its file, size and time, and its failed start.
type Read = (String, u64, SystemTime, Option<Found>);
static LAST_READ: Mutex<Option<Read>> = Mutex::new(None);

/// The last failed start of the bridge in the newest of Live's logs (`root`: Live's preferences folder, a folder for
/// each version), which the running Live writes, with the start of Live it was in. The log is read again only once it
/// has changed.
pub(crate) async fn last_failed(root: &str) -> Option<Found> {
    let log = logs_in(root).await.into_iter().next()?;
    let meta = tokio::fs::metadata(&log).await.ok()?;
    let (size, at) = (meta.len(), meta.modified().ok()?);
    let read = |held: &Option<Read>| held.as_ref().filter(|r| r.0 == log && r.1 == size && r.2 == at).map(|r| r.3.clone());
    if let Some(found) = read(&LAST_READ.lock().unwrap_or_else(|e| e.into_inner())) {
        return found;
    }
    let file = log.clone();
    let run = tokio::task::spawn_blocking(move || current_run(&file)).await.ok()?.ok()?;
    let found = failed_start(&run).map(|failed| (run.lines().next().unwrap_or_default().to_string(), failed));
    *LAST_READ.lock().unwrap_or_else(|e| e.into_inner()) = Some((log, size, at, found.clone()));
    found
}

/// A failed start as Kumi has seen it: the start of Live it was in, when Kumi first saw it, and its words once said.
struct Seen {
    found: Found,
    at: Instant,
    words: Option<String>,
}
static SEEN: Mutex<Option<Seen>> = Mutex::new(None);
/// A failed start found while Live was closed: the system isn't asked again about the same one (the session's watch
/// looks every 2 s, for an hour, while it isn't connected). A Live that opens logs a start of its own at once, which is
/// another failed start, asked about anew, or none.
static CLOSED: Mutex<Option<Found>> = Mutex::new(None);
/// How long a failed start is seen before it's said. A Live that has just opened may not have logged its start yet, and
/// its log then still ends with the start before (whose bridge may have failed): by the next look, a second or two
/// later, it has.
const SETTLED: Duration = Duration::from_secs(1);

/// Why the open Live couldn't start the bridge the last time it tried, and its fix: None when it started, Live wasn't
/// open when the failure was first seen (a Live killed leaves no end to its start in the log, so only an open Live's
/// start is the one running now), its log doesn't say, or the failure was first seen less than `settled` ago. Once said,
/// the same words come back for the same failure until Live logs another start, without asking the system again: a Live
/// quit since isn't looked for, and the words say to restart it.
pub(crate) async fn start_failure(env: &Env, root: &str, run: Run, platform: &str) -> Option<String> {
    start_failure_settled(env, root, run, platform, SETTLED).await
}
/// The same, from Live's own preferences folder, said at once: for a look made once (the doctor's).
pub(crate) async fn start_failure_now(env: &Env, platform: &str) -> Option<String> {
    let root = super::folders::live_preferences_dir(env);
    start_failure_settled(env, &root, kumi_daw::process::default_run(), platform, Duration::ZERO).await
}
/// What the Remote Script keeps beside itself while its last start in a running Live failed (`start-failure.json`, from
/// the bridge that says its failure in Live's status bar on): that Live's process and the exception, taken while that
/// process is a running Live's. Where Kumi has Live's log, the log decides (it also tells a Control Surface set to None
/// since); this is for a Live whose log Kumi can't read.
async fn recorded_failure(env: &Env, run: Run, platform: &str) -> Option<Found> {
    let file = kumi_library::sources::join(&super::folders::remote_scripts_dir(env), "AbletonMcpBridge/start-failure.json");
    let record: serde_json::Value = serde_json::from_slice(&tokio::fs::read(&file).await.ok()?).ok()?;
    let pid = record["pid"].as_u64().filter(|_| record["version"] == 1)?;
    let exception = record["exception"].as_str().filter(|exception| !exception.trim().is_empty())?;
    if !live_pids_on(run, platform, env).await.contains(&u32::try_from(pid).ok()?) {
        return None;
    }
    Some((format!("process {pid} at {}", record["at"]), Failed { exception: exception.to_string(), slots: 1 }))
}
async fn start_failure_settled(env: &Env, root: &str, run: Run, platform: &str, settled: Duration) -> Option<String> {
    let found =
        if logs_in(root).await.is_empty() { recorded_failure(env, run.clone(), platform).await? } else { last_failed(root).await? };
    let seen = SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|seen| seen.found == found)
        .map(|seen| (seen.at, seen.words.clone()));
    let since = match seen {
        Some((_, Some(words))) => return Some(words),
        Some((since, None)) => since,
        None => {
            if CLOSED.lock().unwrap_or_else(|e| e.into_inner()).as_ref() == Some(&found) {
                return None;
            }
            if !is_live_running_on(run, platform, env).await {
                *CLOSED.lock().unwrap_or_else(|e| e.into_inner()) = Some(found);
                return None;
            }
            let since = Instant::now();
            *SEEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Seen { found: found.clone(), at: since, words: None });
            since
        }
    };
    if since.elapsed() < settled {
        return None;
    }
    // Who holds the bridge's ports, asked only when one was in use.
    let failed = &found.1;
    let mut held = vec![];
    if cause(&failed.exception) == Cause::PortHeld {
        for port in bridge_ports(env) {
            held.push((port, port_owner(port, platform).await));
        }
    }
    let words = words(failed, &held, platform == "win32");
    if let Some(seen) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).as_mut().filter(|seen| seen.found == found) {
        seen.words = Some(words.clone());
    }
    Some(words)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cause {
    /// Another program had its port.
    PortHeld,
    /// The system didn't let it have its port (Windows' 10013: a port Windows keeps, or one a program holds alone).
    PortRefused,
    /// One of the bridge's own checks of its configuration, its reference to it and its secret: the message says which.
    Files,
    /// Its files didn't load whole.
    Damaged,
    Other,
}

fn cause(exception: &str) -> Cause {
    let (kind, message) = exception.split_once(": ").unwrap_or((exception, ""));
    let kind = kind.rsplit('.').next().unwrap_or(kind);
    let has = |words: &[&str]| words.iter().any(|w| exception.contains(w));
    // (macOS's EADDRINUSE is 48, Linux's 98, Windows' 10048; 10013 is Windows' WSAEACCES)
    if has(&["Address already in use", "[WinError 10048]", "[Errno 10048]", "[Errno 98]", "[Errno 48]"]) {
        Cause::PortHeld
    } else if has(&["[WinError 10013]", "[Errno 10013]"]) {
        Cause::PortRefused
    } else if kind == "ValueError"
        && ["bridge configuration", "bridge secret", "unsupported bridge", "realtime port", "explicit loopback host"]
            .iter()
            .any(|start| message.starts_with(start))
    {
        Cause::Files
    } else if ["ImportError", "ModuleNotFoundError", "SyntaxError", "IndentationError", "TabError"].contains(&kind) {
        Cause::Damaged
    } else {
        Cause::Other
    }
}

/// `text` as quoted: its first 200 characters.
fn quoted(text: &str) -> String {
    match text.char_indices().nth(200) {
        Some((at, _)) => format!("“{}…”", &text[..at]),
        None => format!("“{text}”"),
    }
}

/// What Kumi says of a failed start, from the exception Live logged, who holds the bridge's ports now (`held`, looked
/// up for a port in use), and whether this is Windows. Each says only what the exception says, then the fix.
pub(crate) fn words(failed: &Failed, held: &[(u16, PortOwner)], windows: bool) -> String {
    const LEAD: &str = "Live has AbletonMcpBridge as a Control Surface but couldn't start it";
    const AGAIN: &str = "Kumi connects by itself once Live answers.";
    const REPORT: &str = "If Live says this again, kumi report collects what's needed for a bug report.";
    let exception = quoted(&failed.exception);
    match cause(&failed.exception) {
        Cause::PortHeld => {
            let taken = held.iter().find_map(|(port, owner)| owner.as_ref().map(|name| (*port, name.clone())));
            match taken {
                Some((port, Some(name))) if is_live(&name) && failed.slots > 1 => format!(
                    "Live has AbletonMcpBridge in {} Control Surface slots, and only one can have port {port}. In Live's Settings › Link, Tempo & MIDI, set the others to None, then restart Live. {AGAIN}",
                    failed.slots
                ),
                Some((port, Some(name))) if is_live(&name) => {
                    format!("{LEAD}: Live itself still had port {port}, which the bridge needs. Restart Live. {AGAIN}")
                }
                Some((port, Some(name))) => {
                    format!("{LEAD}: {name} holds port {port}, which the bridge needs. Quit {name}, then restart Live. {AGAIN}")
                }
                Some((port, None)) => format!(
                    "{LEAD}: another program holds port {port}, which the bridge needs. Quit it, then restart Live ({}). {AGAIN}",
                    if windows {
                        format!("netstat -ano | findstr :{port} shows its PID; Task Manager's Details tab names it")
                    } else {
                        format!("lsof -nP -iTCP:{port} -sTCP:LISTEN names it")
                    }
                ),
                None => format!("{LEAD}: its port was in use as Live started it ({exception}). Restart Live. {AGAIN}"),
            }
        }
        Cause::PortRefused => format!("{LEAD}: Windows didn't let the bridge have its port ({exception}). Restart Live. {REPORT}"),
        Cause::Files => {
            let message = failed.exception.split_once(": ").map_or(failed.exception.as_str(), |(_, message)| message);
            // On Windows the bridge checks its files' owner and permissions with PowerShell, and a check that can't
            // run fails as one that found them unsafe: putting the files back can't help then.
            let powershell = windows
                && ["reference is missing or unsafe", "must be an existing regular file"].iter().any(|said| message.contains(said));
            format!(
                "{LEAD}: the bridge's check of its files said {}. Quit Live and run kumi bridge to put them back, then open Live. {}",
                quoted(message),
                if powershell {
                    "If Live says this again, Windows may be keeping the bridge from running PowerShell, which it checks the files with: kumi report collects what's needed for a bug report."
                } else {
                    AGAIN
                }
            )
        }
        Cause::Damaged => format!(
            "{LEAD}: Kumi's bridge files in Live look incomplete ({exception}). Quit Live and run kumi bridge to put them back, then open Live. {AGAIN}"
        ),
        Cause::Other => format!("{LEAD}: {exception}. Quit Live and run kumi bridge to put Kumi's bridge back, then open Live. {REPORT}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    use std::{cell::RefCell, rc::Rc};

    /// Live's log from real starts of Live 12.4.25b1 on macOS whose bridge failed (2026-11-02, in the lab): its port
    /// held by another program, and its reference file readable by others. Cut to the lines about the bridge, with
    /// a producer's home folder for the lab's.
    const PORT_HELD: &str = include_str!("start_failure/port-held.txt");
    const UNSAFE: &str = include_str!("start_failure/unsafe-reference.txt");
    const PORT_IN_USE: &str = "OSError: [Errno 48] Address already in use";

    fn failed(exception: &str, slots: usize) -> Option<Failed> {
        Some(Failed { exception: exception.into(), slots })
    }

    #[test]
    fn a_failed_start_is_put_back_together_from_the_pieces_live_logs() {
        assert_eq!(failed_start(PORT_HELD), failed(PORT_IN_USE, 1));
        assert_eq!(failed_start(UNSAFE), failed("ValueError: bridge configuration reference is missing or unsafe", 1));
    }

    #[test]
    fn the_remote_scripts_own_line_is_a_failed_start_with_no_traceback_too() {
        let said = "2026-11-03T07:00:00.000001: info: Started: Live 12.4.25b1\n\
            2026-11-03T07:00:03.000001: info: RemoteScriptMessage: (AbletonMcpBridge) Initializing...\n\
            2026-11-03T07:00:04.000001: info: RemoteScriptMessage: (AbletonMcpBridge) AbletonMcpBridge couldn't start: OSError: [Errno 48] Address already in use\n";
        assert_eq!(failed_start(said), failed(PORT_IN_USE, 1));
        // A start after it puts it by, as with a traceback.
        let again = "2026-11-03T07:01:00.000001: info: RemoteScriptMessage: (AbletonMcpBridge) Initializing...\n";
        assert_eq!(failed_start(&format!("{said}{again}")), None);
    }

    #[test]
    fn only_the_start_of_live_running_now_counts() {
        // An earlier start's failure, then a start whose bridge started.
        let started = "2026-11-02T23:30:00.000001: info: Started: Live 12.4.25b1 Build: 2026-10-28_e04f372900\n\
            2026-11-02T23:30:03.000001: info: RemoteScriptMessage: (AbletonMcpBridge) Initializing...\n\
            2026-11-02T23:30:05.000001: info: RemoteScriptMessage: (AbletonMcpBridge) Bridge timer: 34 calls a second\n";
        assert_eq!(failed_start(&format!("{PORT_HELD}{started}")), None);
        // A start that hasn't got as far as the bridge yet.
        assert_eq!(failed_start(&format!("{PORT_HELD}{}\n", started.lines().next().unwrap())), None);
    }

    #[test]
    fn a_start_after_the_failure_or_a_slot_set_to_none_since_puts_it_by() {
        let again = "2026-11-02T23:26:00.000001: info: RemoteScriptMessage: (AbletonMcpBridge) Initializing...\n";
        assert_eq!(failed_start(&format!("{PORT_HELD}{again}")), None, "chosen again in Settings: a new start");
        let slots = |first: &str, second: &str| {
            format!(
                "2026-11-02T23:26:00.000001: info: AMidiIO: Midi Remote Scripts: \n  MidiRemoteScript 1 [Control Surface=\"{first}\" Input=\"None\" Output=\"None\"]\n  MidiRemoteScript 2 [Control Surface=\"{second}\" Input=\"None\" Output=\"None\"]\n2026-11-02T23:26:00.000002: info: AMidiIO: Takeover Mode: None\n"
            )
        };
        assert_eq!(failed_start(&format!("{PORT_HELD}{}", slots("None", "None"))), None, "no longer chosen: asking for it is right");
        assert_eq!(failed_start(&format!("{PORT_HELD}{}", slots("AbletonMcpBridge", "AbletonMcpBridge"))), failed(PORT_IN_USE, 2));
        // The list last in the log, with nothing after it.
        assert_eq!(failed_start(&format!("{PORT_HELD}{}", slots("None", "None").lines().take(3).collect::<Vec<_>>().join("\n"))), None);
    }

    #[test]
    fn other_scripts_and_the_bridges_own_errors_after_a_good_start_arent_a_failed_start() {
        // Another script's traceback, then Live's line for it.
        let other = PORT_HELD.replace("AbletonMcpBridge/", "SomeController/");
        assert_eq!(failed_start(&other), None);
        // The bridge started; a thread of it raised later, and another script then failed to load.
        let good = "2026-11-02T23:30:00.000001: info: Started: Live 12.4.25b1\n\
            2026-11-02T23:30:03.000001: info: RemoteScriptMessage: (AbletonMcpBridge) Initializing...\n\
            2026-11-02T23:31:00.000001: error: RemoteScriptError: Traceback (most recent call last):\n\n\
            2026-11-02T23:31:00.000002: error: RemoteScriptError:   File \"/Users/producer/Music/Ableton/User Library/Remote Scripts/AbletonMcpBridge/ableton_mcp_remote_script.py\", line 13500, in _client\n\n\
            2026-11-02T23:31:00.000003: error: RemoteScriptError: ConnectionResetError\n\
            2026-11-02T23:31:00.000004: error: RemoteScriptError: : \n\
            2026-11-02T23:31:00.000005: error: RemoteScriptError: [Errno 54] Connection reset by peer\n\
            2026-11-02T23:31:00.000006: error: RemoteScriptError: \n\n\
            2026-11-02T23:32:00.000001: info: Exception: Script could not be loaded.\n";
        assert_eq!(failed_start(good), None);
    }

    #[test]
    fn a_windows_log_with_its_line_ends_mark_and_backslashes_reads_the_same() {
        // (built from the macOS one: no Windows here to take one from)
        let windows = PORT_HELD
            .replace(
                "/Users/producer/Music/Ableton/User Library/Remote Scripts/AbletonMcpBridge/",
                "C:\\Users\\producer\\Documents\\Ableton\\User Library\\Remote Scripts\\AbletonMcpBridge\\",
            )
            .replace(
                "[Errno 48] Address already in use",
                "[WinError 10048] Only one usage of each socket address (protocol/network address/port) is normally permitted",
            )
            .replace("(Mac)", "(Win)")
            .replace('\n', "\r\n");
        let found = failed_start(&format!("\u{feff}{windows}")).unwrap();
        assert_eq!(
            found.exception,
            "OSError: [WinError 10048] Only one usage of each socket address (protocol/network address/port) is normally permitted"
        );
        assert_eq!(cause(&found.exception), Cause::PortHeld);
    }

    fn start_line(log: &str) -> &str {
        let at = log.find(STARTED).unwrap();
        let line = log[..at].rfind('\n').map_or(0, |end| end + 1);
        log[line..].lines().next().unwrap()
    }

    #[test]
    fn a_long_log_is_read_from_its_last_start_on() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("Log.txt");
        let read = || current_run(log.to_str().unwrap()).unwrap();
        // Earlier starts, longer than a part read, before the one running now.
        let earlier = "2026-11-01T10:00:00.000001: info: Started: Live 12.4.25b1\nfiller\n".repeat(40_000);
        std::fs::write(&log, format!("{earlier}{PORT_HELD}")).unwrap();
        let run = read();
        assert_eq!(run.lines().next().unwrap(), start_line(PORT_HELD), "from its line, with its time");
        assert!(PORT_HELD.ends_with(&run) && run.len() < PORT_HELD.len(), "only the start running now");
        // Where the last part read begins, from the start's words: 5 bytes into them (the words split between two
        // parts), and 10 before them (its time in the part before).
        let line = "2026-11-02T23:24:38.242633: info: Started: Live 12.4.25b1\n";
        let words_on = (line.len() - line.find(STARTED).unwrap()) as i64;
        for from in [5i64, -10] {
            let after = (256 * 1024 + from - words_on) as usize;
            std::fs::write(&log, format!("an earlier line\n{line}{}", "y".repeat(after))).unwrap();
            assert!(read().starts_with(line), "from {from}");
        }
        // The first line of the log, and a log without a start: all of it.
        std::fs::write(&log, format!("{line}more\n")).unwrap();
        assert_eq!(read(), format!("{line}more\n"));
        std::fs::write(&log, "a line\n").unwrap();
        assert_eq!(read(), "a line\n");
    }

    /// Live's preferences folder with two versions' logs, the newer one `newest`.
    fn logs(newest: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let (older, newer) = (root.path().join("Live 12.4.15b7"), root.path().join("Live 12.4.25b1"));
        for dir in [&older, &newer] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(older.join("Log.txt"), UNSAFE).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(newer.join("Log.txt"), newest).unwrap();
        (root, newer.join("Log.txt"))
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_newest_log_is_read_again_only_once_it_changes() {
        let (root, newest) = logs(PORT_HELD);
        let root = root.path().to_str().unwrap();
        let found = last_failed(root).await.unwrap();
        assert_eq!((found.0.as_str(), Some(found.1)), (start_line(PORT_HELD), failed(PORT_IN_USE, 1)), "the newest");
        // Live starts the bridge again, and it starts.
        let started = "2026-11-02T23:26:00.000001: info: RemoteScriptMessage: (AbletonMcpBridge) Initializing...\n";
        std::fs::write(&newest, format!("{PORT_HELD}{started}")).unwrap();
        assert_eq!(last_failed(root).await, None, "read again");
    }

    /// A stand-in for the system: whether Live runs, and each program asked.
    fn system(running: bool) -> (Run, Rc<RefCell<Vec<String>>>) {
        let asked = Rc::new(RefCell::new(vec![]));
        let held = asked.clone();
        let run: Run = Rc::new(move |program, _args, _cwd| {
            held.borrow_mut().push(program);
            async move { kumi_daw::process::Ran { code: if running { 0 } else { 1 }, ..Default::default() } }.boxed_local()
        });
        (run, asked)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_failure_is_said_once_it_has_settled_and_only_while_live_runs() {
        let env = Env::new();
        // The reference's failure, which needs no look at the ports.
        let (root, newest) = logs(UNSAFE);
        let root = root.path().to_str().unwrap();
        let (closed, asked) = system(false);
        assert_eq!(
            start_failure_settled(&env, root, closed.clone(), "darwin", Duration::ZERO).await,
            None,
            "Live isn't open: an earlier start's"
        );
        assert_eq!(start_failure_settled(&env, root, closed, "darwin", Duration::ZERO).await, None);
        assert_eq!(asked.borrow().as_slice(), ["pgrep"], "the same one isn't asked about again at once");
        // Live opens, and its bridge fails again.
        std::fs::write(&newest, format!("{UNSAFE}{}", UNSAFE.replace("2026-11-02T23:24:", "2026-11-02T23:44:"))).unwrap();
        let (open, asked) = system(true);
        let settled = Duration::from_millis(150);
        assert_eq!(start_failure_settled(&env, root, open.clone(), "darwin", settled).await, None, "first seen: not yet");
        assert_eq!(start_failure_settled(&env, root, open.clone(), "darwin", settled).await, None, "still settling");
        tokio::time::sleep(settled).await;
        let said = start_failure_settled(&env, root, open.clone(), "darwin", settled).await.unwrap();
        assert!(said.contains("the bridge's check of its files said “bridge configuration reference is missing or unsafe”"), "{said}");
        assert_eq!(start_failure_settled(&env, root, open, "darwin", settled).await, Some(said), "the same words");
        assert_eq!(asked.borrow().as_slice(), ["pgrep"], "the system asked once");
    }

    /// A stand-in for the system that says these Live processes run (`pgrep`'s output).
    fn running_lives(pids: &str) -> Run {
        let pids = pids.to_string();
        Rc::new(move |_program, _args, _cwd| {
            let stdout = pids.clone();
            async move { kumi_daw::process::Ran { code: if stdout.is_empty() { 1 } else { 0 }, stdout, ..Default::default() } }
                .boxed_local()
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn with_no_log_of_lives_the_remote_scripts_own_record_says_it_while_its_live_runs() {
        let scripts = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(scripts.path().join("AbletonMcpBridge")).unwrap();
        let env = Env::from([("KUMI_REMOTE_SCRIPTS_DIR".to_string(), scripts.path().display().to_string())]);
        let record = |pid: u32| {
            serde_json::json!({"version": 1, "pid": pid, "at": 1.5, "exception": "ValueError: bridge secret is invalid"}).to_string()
        };
        std::fs::write(scripts.path().join("AbletonMcpBridge/start-failure.json"), record(4242)).unwrap();
        let nothing = tempfile::tempdir().unwrap();
        let no_log = nothing.path().to_str().unwrap();
        let said = start_failure_settled(&env, no_log, running_lives("4242\n"), "darwin", Duration::ZERO).await.unwrap();
        assert!(said.contains("the bridge's check of its files said “bridge secret is invalid”"), "{said}");
        // Kept by a Live that has quit since: not said.
        std::fs::write(scripts.path().join("AbletonMcpBridge/start-failure.json"), record(4243)).unwrap();
        assert_eq!(start_failure_settled(&env, no_log, running_lives("4242\n"), "darwin", Duration::ZERO).await, None);
        // With Live's log to read, the log decides: here, a start after the failure.
        std::fs::write(scripts.path().join("AbletonMcpBridge/start-failure.json"), record(4242)).unwrap();
        let started = "2026-11-02T23:26:00.000001: info: RemoteScriptMessage: (AbletonMcpBridge) Initializing...\n";
        let (root, _newest) = logs(&format!("{PORT_HELD}{started}"));
        assert_eq!(
            start_failure_settled(&env, root.path().to_str().unwrap(), running_lives("4242\n"), "darwin", Duration::ZERO).await,
            None
        );
    }

    #[test]
    fn each_cause_says_what_the_exception_says_and_its_fix() {
        let port = |owner: PortOwner, slots| words(&Failed { exception: PORT_IN_USE.into(), slots }, &[(9765, owner), (9766, None)], false);
        assert_eq!(
            port(Some(Some("Loopback".into())), 1),
            "Live has AbletonMcpBridge as a Control Surface but couldn't start it: Loopback holds port 9765, which the bridge needs. Quit Loopback, then restart Live. Kumi connects by itself once Live answers."
        );
        assert_eq!(
            port(Some(None), 1),
            "Live has AbletonMcpBridge as a Control Surface but couldn't start it: another program holds port 9765, which the bridge needs. Quit it, then restart Live (lsof -nP -iTCP:9765 -sTCP:LISTEN names it). Kumi connects by itself once Live answers."
        );
        // Live itself: the bridge in two slots, or an earlier start of it in the same Live.
        assert_eq!(
            port(Some(Some("Live".into())), 2),
            "Live has AbletonMcpBridge in 2 Control Surface slots, and only one can have port 9765. In Live's Settings › Link, Tempo & MIDI, set the others to None, then restart Live. Kumi connects by itself once Live answers."
        );
        assert_eq!(
            port(Some(Some("Ableton Live 12 Suite".into())), 1),
            "Live has AbletonMcpBridge as a Control Surface but couldn't start it: Live itself still had port 9765, which the bridge needs. Restart Live. Kumi connects by itself once Live answers."
        );
        // Free again by now.
        assert_eq!(
            port(None, 1),
            "Live has AbletonMcpBridge as a Control Surface but couldn't start it: its port was in use as Live started it (“OSError: [Errno 48] Address already in use”). Restart Live. Kumi connects by itself once Live answers."
        );
        let windows_port =
            words(&Failed { exception: "OSError: [WinError 10048] Only one usage".into(), slots: 1 }, &[(9765, Some(None))], true);
        assert!(
            windows_port.contains("(netstat -ano | findstr :9765 shows its PID; Task Manager's Details tab names it)"),
            "{windows_port}"
        );
        let files = |message: &str, windows| words(&Failed { exception: format!("ValueError: {message}"), slots: 1 }, &[], windows);
        assert_eq!(
            files("bridge configuration reference is missing or unsafe", false),
            "Live has AbletonMcpBridge as a Control Surface but couldn't start it: the bridge's check of its files said “bridge configuration reference is missing or unsafe”. Quit Live and run kumi bridge to put them back, then open Live. Kumi connects by itself once Live answers."
        );
        // On Windows that check also fails when PowerShell couldn't run it, which putting the files back can't fix.
        assert!(files("bridge configuration reference is missing or unsafe", true).ends_with(
            "If Live says this again, Windows may be keeping the bridge from running PowerShell, which it checks the files with: kumi report collects what's needed for a bug report."
        ));
        assert!(files("bridge secret is invalid", true).ends_with("Kumi connects by itself once Live answers."), "no owner check in it");
        assert_eq!(
            words(&Failed { exception: "OSError: [WinError 10013] An attempt was made to access a socket in a way forbidden by its access permissions".into(), slots: 1 }, &[], true),
            "Live has AbletonMcpBridge as a Control Surface but couldn't start it: Windows didn't let the bridge have its port (“OSError: [WinError 10013] An attempt was made to access a socket in a way forbidden by its access permissions”). Restart Live. If Live says this again, kumi report collects what's needed for a bug report."
        );
        assert_eq!(
            words(&Failed { exception: "ModuleNotFoundError: No module named 'AbletonMcpBridge.ableton_mcp_remote_script'".into(), slots: 1 }, &[], false),
            "Live has AbletonMcpBridge as a Control Surface but couldn't start it: Kumi's bridge files in Live look incomplete (“ModuleNotFoundError: No module named 'AbletonMcpBridge.ableton_mcp_remote_script'”). Quit Live and run kumi bridge to put them back, then open Live. Kumi connects by itself once Live answers."
        );
        assert_eq!(
            words(&Failed { exception: "RuntimeError: something else".into(), slots: 1 }, &[], false),
            "Live has AbletonMcpBridge as a Control Surface but couldn't start it: “RuntimeError: something else”. Quit Live and run kumi bridge to put Kumi's bridge back, then open Live. If Live says this again, kumi report collects what's needed for a bug report."
        );
        let long = words(&Failed { exception: format!("RuntimeError: {}", "x".repeat(400)), slots: 1 }, &[], false);
        assert!(long.contains(&format!("“RuntimeError: {}…”", "x".repeat(186))), "{long}");
    }
}
