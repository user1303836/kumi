//! Telling the producer what Kumi is doing while they look elsewhere: the terminal's program status
//! (OSC 7501, and OSC 9;4 progress where the terminal draws that) and a desktop notification. Which
//! sequences go out depends on the terminal, chosen once from the environment
//! ([`detect_terminal_profile`]): Kumi never asks the terminal, since a reply would reach the input as
//! keys (`ESC ]` is Alt-]).

use std::collections::HashMap;

use base64::Engine;

/// How this terminal shows Kumi's status and alerts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalProfile {
    /// OSC 7501 program status. A terminal ignores an OSC it doesn't know, so these go out everywhere
    /// but the Linux console, a dumb terminal and the old Windows console.
    pub status: bool,
    /// OSC 9;4 progress, for the terminals known to draw it: Windows Terminal, ConEmu and Ghostty.
    pub progress: bool,
    /// How a desktop notification reaches the producer.
    pub notify: NotifyChannel,
    /// Inside tmux: each sequence is wrapped to pass through it (tmux needs `allow-passthrough on`).
    pub tmux: bool,
    /// The terminal app's own id on macOS (`__CFBundleIdentifier`: "com.apple.Terminal"): while the terminal
    /// says nothing of focus, the system tells whether that app is in front.
    pub app_id: Option<String>,
    /// Kitty's images through Unicode placeholders (changes' pictures): kitty and Ghostty, outside a multiplexer.
    pub images: bool,
    /// iTerm2, inside tmux too (its `LC_TERMINAL` comes over SSH): its own selection is Option-drag, not Shift-drag.
    pub iterm: bool,
    /// Blanks can be erased in the colours set (EL, ECH): everywhere but GNU screen, which erases in its own unless
    /// told `bce on`.
    pub erase: bool,
    /// Every cell is written in each frame that changes, unless the `fullRepaint` setting says otherwise: Windows
    /// Terminal.
    pub repaint: bool,
    /// The terminal opens a link's words itself (OSC 8), so an answer's addresses needn't follow them: iTerm2, Ghostty,
    /// WezTerm, kitty, Windows Terminal and VS Code's terminal, outside a multiplexer.
    pub hyperlinks: bool,
}

impl TerminalProfile {
    /// Nothing at all: no status, no notification. Tests start from this.
    pub fn quiet() -> Self {
        Self {
            status: false,
            progress: false,
            notify: NotifyChannel::None,
            tmux: false,
            app_id: None,
            images: false,
            iterm: false,
            erase: true,
            repaint: false,
            hyperlinks: false,
        }
    }
}

/// How a desktop notification is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyChannel {
    None,
    /// `OSC 777 ; notify ; title ; body`: Ghostty, WezTerm, foot.
    Osc777,
    /// `OSC 9 ; text`: iTerm2.
    Osc9,
    /// `OSC 99`, kitty's own.
    Osc99,
    /// macOS's notification through osascript: Terminal.app, and terminals with none of their own (or inside tmux).
    AppleScript,
    /// `notify-send`, on Linux where the terminal has none of its own.
    NotifySend,
    /// The bell, which Windows Terminal answers by flashing the taskbar.
    Bell,
}

impl NotifyChannel {
    /// Shown by a program Kumi runs, not written to the terminal.
    pub fn is_program(self) -> bool {
        matches!(self, NotifyChannel::AppleScript | NotifyChannel::NotifySend)
    }
}

/// The terminal's status and notification sequences, from its environment. `platform` is Node's
/// (`darwin`, `linux`, `win32`).
pub fn detect_terminal_profile(env: &HashMap<String, String>, platform: &str) -> TerminalProfile {
    let get = |name: &str| env.get(name).map(String::as_str).unwrap_or("");
    let term = get("TERM");
    let program = get("TERM_PROGRAM");
    let tmux = !get("TMUX").is_empty();
    let windows_terminal = !get("WT_SESSION").is_empty();
    let conemu = !get("ConEmuPID").is_empty() || get("ConEmuANSI") == "ON";
    let old_console = platform == "win32" && !windows_terminal && !conemu && program.is_empty();
    if matches!(term, "linux" | "dumb") || old_console {
        return TerminalProfile::quiet();
    }
    let ghostty = program == "ghostty";
    let kitty = term == "xterm-kitty" || !get("KITTY_WINDOW_ID").is_empty();
    let iterm = program == "iTerm.app" || get("LC_TERMINAL") == "iTerm2";
    let wezterm = program == "WezTerm";
    let foot = term == "foot" || term.starts_with("foot-");
    // Inside tmux the terminal outside is unknown (tmux names itself in TERM_PROGRAM): the system's own
    // notification reaches the producer wherever that is.
    let notify = if tmux {
        match platform {
            "darwin" => NotifyChannel::AppleScript,
            "linux" if on_path("notify-send", env) => NotifyChannel::NotifySend,
            _ => NotifyChannel::None,
        }
    } else if ghostty || wezterm || foot {
        NotifyChannel::Osc777
    } else if iterm {
        NotifyChannel::Osc9
    } else if kitty {
        NotifyChannel::Osc99
    } else if platform == "darwin" {
        NotifyChannel::AppleScript
    } else if platform == "linux" && on_path("notify-send", env) {
        NotifyChannel::NotifySend
    } else if windows_terminal {
        NotifyChannel::Bell
    } else {
        NotifyChannel::None
    };
    let app_id = Some(get("__CFBundleIdentifier")).filter(|id| platform == "darwin" && !id.is_empty()).map(str::to_string);
    // Images only where the terminal is the one named: a terminal started from kitty or Ghostty inherits their names
    // (kitty's window id where nothing else names the terminal, Ghostty's name where TERM is its own or a generic
    // xterm's), and inside tmux, screen or Zellij an image wouldn't reach the terminal.
    let own_kitty = term == "xterm-kitty" || (!get("KITTY_WINDOW_ID").is_empty() && program.is_empty() && term.starts_with("xterm"));
    let own_ghostty = ghostty && (term.is_empty() || term == "xterm-ghostty" || term.starts_with("xterm-256") || term == "xterm");
    let multiplexed = tmux || !get("ZELLIJ").is_empty() || term.starts_with("screen") || term.starts_with("tmux");
    let images = !multiplexed && (own_kitty || own_ghostty);
    // GNU screen names its session in STY; tmux, inside it or around it, erases in the colours set itself.
    let screen = !get("STY").is_empty() && !tmux;
    // Named as images are, kitty and Ghostty by their own names only.
    let hyperlinks = !multiplexed && (iterm || own_ghostty || wezterm || own_kitty || windows_terminal || program == "vscode");
    TerminalProfile {
        status: true,
        progress: !tmux && (windows_terminal || conemu || ghostty),
        notify,
        tmux,
        app_id,
        images,
        iterm,
        erase: !screen,
        repaint: windows_terminal,
        hyperlinks,
    }
}

fn on_path(name: &str, env: &HashMap<String, String>) -> bool {
    env.get("PATH").is_some_and(|path| std::env::split_paths(path).any(|folder| folder.join(name).is_file()))
}

/// A program's state in OSC 7501's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramState {
    Idle,
    Working,
    Blocked(Blocked),
    Done,
    Error,
}

/// What a blocked program waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// A choice the producer makes.
    Question,
    /// Signing in.
    Auth,
}

impl ProgramState {
    fn name(self) -> &'static str {
        match self {
            ProgramState::Idle => "idle",
            ProgramState::Working => "working",
            ProgramState::Blocked(_) => "blocked",
            ProgramState::Done => "done",
            ProgramState::Error => "error",
        }
    }
}

/// One OSC 7501 report: the root record (Kumi's answer), or a child record such as a background job's (`id`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatusReport {
    pub state: Option<ProgramState>,
    pub id: Option<String>,
    /// 0–100; working or blocked only.
    pub progress: Option<u8>,
    pub title: Option<String>,
    pub msg: Option<String>,
}

const APP: &str = "kumi";
/// The spec's limits (rev 0.4): the whole sequence, and `msg` and `title` before encoding.
const MAX_SEQUENCE: usize = 4096;
const MAX_MSG: usize = 2048;
const MAX_TITLE: usize = 192;
const ST: &str = "\u{1b}\\";

/// The report as an OSC 7501 sequence; None when it has no state or its id breaks the spec's grammar.
/// The message is what's cut when the whole would pass 4096 bytes.
pub fn status_sequence(report: &StatusReport) -> Option<String> {
    let state = report.state?;
    if report.id.as_deref().is_some_and(|id| !valid_id(id)) {
        return None;
    }
    let mut pairs = vec![format!("state={}", state.name())];
    if let Some(id) = &report.id {
        pairs.push(format!("id={id}"));
    }
    match state {
        ProgramState::Blocked(Blocked::Question) => pairs.push("kind=question".into()),
        ProgramState::Blocked(Blocked::Auth) => pairs.push("kind=auth".into()),
        _ => {}
    }
    if let Some(progress) = report.progress.filter(|_| matches!(state, ProgramState::Working | ProgramState::Blocked(_))) {
        pairs.push(format!("progress={}", progress.min(100)));
    }
    pairs.push(format!("app={APP}"));
    if let Some(title) = report.title.as_deref().map(|title| one_line(title, MAX_TITLE)).filter(|title| !title.is_empty()) {
        pairs.push(format!("title={}", encode(&title)));
    }
    let lead = format!("\u{1b}]7501;{}", pairs.join(":"));
    // What's left besides the lead, ":msg=" and the terminator, in base64's 4 bytes for each 3.
    let room = MAX_SEQUENCE.saturating_sub(lead.len() + 5 + ST.len()) / 4 * 3;
    let msg = report.msg.as_deref().map(|msg| fit(&one_line(msg, MAX_MSG), room)).unwrap_or_default();
    let msg = if msg.is_empty() { String::new() } else { format!(":msg={}", encode(&msg)) };
    Some(format!("{lead}{msg}{ST}"))
}

/// Removes Kumi's status records: the root and every child (no id), or one record and its children.
pub fn clear_sequence(id: Option<&str>) -> String {
    match id.filter(|id| valid_id(id)) {
        Some(id) => format!("\u{1b}]7501;state=clear:id={id}{ST}"),
        None => format!("\u{1b}]7501;state=clear{ST}"),
    }
}

/// OSC 9;4 progress for the same state: a bar while working (indeterminate without a number), paused
/// while blocked, red on an error, gone when done or idle.
pub fn progress_sequence(state: ProgramState, progress: Option<u8>) -> String {
    let (kind, value) = match state {
        ProgramState::Working => progress.map_or((3, 0), |p| (1, p.min(100))),
        ProgramState::Blocked(_) => (4, progress.unwrap_or(0).min(100)),
        ProgramState::Error => (2, 100),
        ProgramState::Done | ProgramState::Idle => (0, 0),
    };
    format!("\u{1b}]9;4;{kind};{value}{ST}")
}

/// Takes OSC 9;4 progress away.
pub const PROGRESS_CLEAR: &str = "\u{1b}]9;4;0;0\u{1b}\\";

/// A desktop notification written to the terminal; None for a channel that's a program (or none).
pub fn notify_sequence(channel: NotifyChannel, title: &str, body: &str) -> Option<String> {
    let title = one_line(title, 120);
    let body = one_line(body, 400);
    match channel {
        // The title ends at the next ';': the body may hold them.
        NotifyChannel::Osc777 => Some(format!("\u{1b}]777;notify;{};{body}{ST}", title.replace(';', ","))),
        NotifyChannel::Osc9 => Some(format!("\u{1b}]9;{}{ST}", joined(&title, &body))),
        NotifyChannel::Osc99 => {
            Some(format!("\u{1b}]99;i=kumi:d=0:e=1;{}{ST}\u{1b}]99;i=kumi:p=body:e=1;{}{ST}", encode(&title), encode(&body)))
        }
        NotifyChannel::Bell => Some("\u{7}".into()),
        NotifyChannel::AppleScript | NotifyChannel::NotifySend | NotifyChannel::None => None,
    }
}

/// The program that shows a notification for `channel`, with its arguments; None for one written to the terminal.
pub fn notify_command(channel: NotifyChannel, title: &str, body: &str) -> Option<(&'static str, Vec<String>)> {
    let title = one_line(title, 120);
    let body = one_line(body, 400);
    match channel {
        NotifyChannel::AppleScript => {
            let quoted = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
            Some(("osascript", vec!["-e".into(), format!("display notification {} with title {}", quoted(&body), quoted(&title))]))
        }
        NotifyChannel::NotifySend => Some(("notify-send", vec!["--app-name=Kumi".into(), "--".into(), title, body])),
        _ => None,
    }
}

/// Whether the app in front is the one with this id (macOS's `lsappinfo`, which needs no permission); None where
/// that can't be told.
pub async fn app_in_front(app_id: &str) -> Option<bool> {
    let front = super::attach::bounded("lsappinfo", &["front"]).await.ok()?;
    let asn = String::from_utf8_lossy(&front.stdout).trim().to_string();
    if asn.is_empty() {
        return None;
    }
    let info = super::attach::bounded("lsappinfo", &["info", "-only", "bundleID", &asn]).await.ok()?;
    let text = String::from_utf8_lossy(&info.stdout).into_owned();
    // `    bundleID="com.apple.Terminal"`, or `bundleID=[ NULL ]` for a process without one.
    let id = text.split_once("bundleID=\"")?.1.split('"').next()?.to_string();
    Some(id.eq_ignore_ascii_case(app_id))
}

/// A sequence wrapped to pass through tmux, which drops the OSCs it doesn't know (each Escape inside is doubled).
pub fn tmux_wrap(sequence: &str) -> String {
    if sequence.is_empty() || !sequence.contains('\u{1b}') {
        return sequence.to_string();
    }
    format!("\u{1b}Ptmux;{}{ST}", sequence.replace('\u{1b}', "\u{1b}\u{1b}"))
}

fn joined(title: &str, body: &str) -> String {
    match (title.is_empty(), body.is_empty()) {
        (false, false) => format!("{title}: {body}"),
        (true, _) => body.to_string(),
        (false, true) => title.to_string(),
    }
}

fn encode(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
}

/// The text on one line with no control characters (OSC 7501 discards a report whose text has any), at
/// most `max` bytes: an ellipsis ends text that was cut.
pub fn one_line(text: &str, max: usize) -> String {
    let spaced: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let words = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.len() <= max {
        return words;
    }
    format!("{}…", fit(&words, max.saturating_sub('…'.len_utf8())).trim_end())
}

/// At most `max` bytes of `text`, cut at a character's edge.
fn fit(text: &str, max: usize) -> String {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// OSC 7501's id grammar: 1–8 segments of 1–32 of `A-Za-z0-9_.+-`, joined by '/', 128 bytes at most.
fn valid_id(id: &str) -> bool {
    let segments: Vec<&str> = id.split('/').collect();
    id.len() <= 128
        && segments.len() <= 8
        && segments
            .iter()
            .all(|segment| (1..=32).contains(&segment.len()) && segment.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
    }

    fn decode(text: &str) -> String {
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(text).unwrap()).unwrap()
    }

    #[test]
    fn each_terminal_gets_its_own_notification_and_progress() {
        let profile = |pairs: &[(&str, &str)], platform: &str| detect_terminal_profile(&env(pairs), platform);
        let ghostty =
            profile(&[("TERM_PROGRAM", "ghostty"), ("TERM", "xterm-ghostty"), ("__CFBundleIdentifier", "com.mitchellh.ghostty")], "darwin");
        assert_eq!(
            ghostty,
            TerminalProfile {
                status: true,
                progress: true,
                notify: NotifyChannel::Osc777,
                tmux: false,
                app_id: Some("com.mitchellh.ghostty".into()),
                images: true,
                iterm: false,
                erase: true,
                repaint: false,
                hyperlinks: true,
            }
        );
        assert_eq!(profile(&[("TERM_PROGRAM", "WezTerm")], "linux").notify, NotifyChannel::Osc777);
        assert_eq!(profile(&[("TERM", "foot")], "linux").notify, NotifyChannel::Osc777);
        assert_eq!(profile(&[("TERM_PROGRAM", "iTerm.app")], "darwin").notify, NotifyChannel::Osc9);
        assert_eq!(profile(&[("LC_TERMINAL", "iTerm2")], "darwin").notify, NotifyChannel::Osc9);
        // iTerm2 still, inside tmux over SSH, where its notifications take another way.
        let tmux = profile(&[("LC_TERMINAL", "iTerm2"), ("TMUX", "/tmp/tmux-1000/default,1,0")], "linux");
        assert!(tmux.iterm && tmux.notify != NotifyChannel::Osc9);
        assert!(!profile(&[("TERM_PROGRAM", "ghostty")], "darwin").iterm);
        assert_eq!(profile(&[("TERM", "xterm-kitty")], "darwin").notify, NotifyChannel::Osc99);
        assert_eq!(profile(&[("TERM_PROGRAM", "Apple_Terminal")], "darwin").notify, NotifyChannel::AppleScript);
        assert_eq!(profile(&[("TERM_PROGRAM", "vscode")], "darwin").notify, NotifyChannel::AppleScript);
        // Without notify-send on PATH, a Linux terminal of no known kind has nothing.
        assert_eq!(profile(&[("TERM", "xterm-256color"), ("PATH", "/nowhere")], "linux").notify, NotifyChannel::None);
        let windows = profile(&[("WT_SESSION", "1"), ("__CFBundleIdentifier", "ignored")], "win32");
        assert_eq!(
            windows,
            TerminalProfile {
                status: true,
                progress: true,
                notify: NotifyChannel::Bell,
                tmux: false,
                app_id: None,
                images: false,
                iterm: false,
                erase: true,
                repaint: true,
                hyperlinks: true,
            }
        );
        assert!(!profile(&[("TERM_PROGRAM", "iTerm.app")], "darwin").progress);
    }

    #[test]
    fn images_go_only_to_the_terminal_that_names_itself_outside_a_multiplexer() {
        let images = |pairs: &[(&str, &str)]| detect_terminal_profile(&env(pairs), "darwin").images;
        assert!(images(&[("TERM", "xterm-kitty")]));
        assert!(images(&[("KITTY_WINDOW_ID", "3"), ("TERM", "xterm-256color")]));
        assert!(images(&[("TERM_PROGRAM", "ghostty"), ("TERM", "xterm-ghostty")]));
        assert!(images(&[("TERM_PROGRAM", "ghostty")]));
        assert!(!images(&[("TERM_PROGRAM", "iTerm.app")]));
        // Started from kitty or Ghostty, another terminal inherits their names.
        assert!(!images(&[("KITTY_WINDOW_ID", "1"), ("TERM_PROGRAM", "vscode"), ("TERM", "xterm-256color")]));
        assert!(!images(&[("TERM_PROGRAM", "ghostty"), ("TERM", "alacritty")]));
        // An image wouldn't pass a multiplexer.
        assert!(!images(&[("TERM_PROGRAM", "ghostty"), ("TERM", "xterm-ghostty"), ("ZELLIJ", "0")]));
        assert!(!images(&[("TERM", "xterm-kitty"), ("TMUX", "/tmp/tmux-501/default,1,0")]));
        assert!(!images(&[("TERM", "screen-256color"), ("KITTY_WINDOW_ID", "1")]));
    }

    #[test]
    fn the_terminals_known_to_open_links_get_an_answers_link_words_alone() {
        let opens = |pairs: &[(&str, &str)], platform: &str| detect_terminal_profile(&env(pairs), platform).hyperlinks;
        assert!(opens(&[("TERM_PROGRAM", "iTerm.app")], "darwin"));
        assert!(opens(&[("TERM_PROGRAM", "ghostty"), ("TERM", "xterm-ghostty")], "darwin"));
        assert!(opens(&[("TERM_PROGRAM", "WezTerm")], "linux"));
        assert!(opens(&[("TERM", "xterm-kitty")], "linux"));
        assert!(opens(&[("WT_SESSION", "1")], "win32"));
        assert!(opens(&[("TERM_PROGRAM", "vscode")], "darwin"));
        assert!(!opens(&[("TERM_PROGRAM", "Apple_Terminal")], "darwin"));
        assert!(!opens(&[("TERM", "xterm-256color")], "linux"));
        // A multiplexer between them, or a terminal that only inherited kitty's name: the address stays.
        assert!(!opens(&[("TERM_PROGRAM", "tmux"), ("TMUX", "/tmp/tmux-501/default,1,0"), ("LC_TERMINAL", "iTerm2")], "darwin"));
        assert!(!opens(&[("KITTY_WINDOW_ID", "1"), ("TERM_PROGRAM", "Apple_Terminal"), ("TERM", "xterm-256color")], "darwin"));
    }

    #[test]
    fn gnu_screen_alone_gets_blanks_written_rather_than_erased() {
        let erase = |pairs: &[(&str, &str)]| detect_terminal_profile(&env(pairs), "linux").erase;
        assert!(!erase(&[("STY", "4242.pts-0.studio"), ("TERM", "screen-256color")]));
        assert!(erase(&[("STY", "4242.pts-0.studio"), ("TMUX", "/tmp/tmux-1000/default,1,0"), ("TERM", "tmux-256color")]));
        assert!(erase(&[("TERM", "xterm-256color")]));
    }

    #[test]
    fn consoles_that_would_print_the_sequences_get_none() {
        assert_eq!(detect_terminal_profile(&env(&[("TERM", "linux")]), "linux"), TerminalProfile::quiet());
        assert_eq!(detect_terminal_profile(&env(&[("TERM", "dumb")]), "darwin"), TerminalProfile::quiet());
        assert_eq!(detect_terminal_profile(&env(&[]), "win32"), TerminalProfile::quiet());
    }

    #[test]
    fn inside_tmux_the_system_notifies_and_sequences_pass_through() {
        let profile = detect_terminal_profile(&env(&[("TMUX", "/tmp/tmux-501/default,1,0"), ("TERM_PROGRAM", "tmux")]), "darwin");
        assert_eq!(
            profile,
            TerminalProfile {
                status: true,
                progress: false,
                notify: NotifyChannel::AppleScript,
                tmux: true,
                app_id: None,
                images: false,
                iterm: false,
                erase: true,
                repaint: false,
                hyperlinks: false,
            }
        );
        assert_eq!(
            tmux_wrap("\u{1b}]7501;state=idle:app=kumi\u{1b}\\"),
            "\u{1b}Ptmux;\u{1b}\u{1b}]7501;state=idle:app=kumi\u{1b}\u{1b}\\\u{1b}\\"
        );
        assert_eq!(tmux_wrap("\u{7}"), "\u{7}");
    }

    #[test]
    fn a_status_report_names_its_state_app_and_words() {
        let report = StatusReport {
            state: Some(ProgramState::Working),
            progress: Some(40),
            title: Some("Night Drive".into()),
            msg: Some("rendering bars 1–8".into()),
            ..Default::default()
        };
        let sequence = status_sequence(&report).unwrap();
        let body = sequence.strip_prefix("\u{1b}]7501;").unwrap().strip_suffix("\u{1b}\\").unwrap();
        let pairs: Vec<(&str, &str)> = body.split(':').map(|pair| pair.split_once('=').unwrap()).collect();
        assert_eq!(pairs[..3], [("state", "working"), ("progress", "40"), ("app", "kumi")]);
        assert_eq!(decode(pairs[3].1), "Night Drive");
        assert_eq!(decode(pairs[4].1), "rendering bars 1–8");
        // Every value stays in the spec's alphabet: nothing to escape.
        assert!(body.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-:".contains(&b)));
    }

    #[test]
    fn blocked_says_why_and_progress_only_goes_with_work() {
        let blocked = StatusReport { state: Some(ProgramState::Blocked(Blocked::Question)), ..Default::default() };
        assert_eq!(status_sequence(&blocked).unwrap(), "\u{1b}]7501;state=blocked:kind=question:app=kumi\u{1b}\\");
        let auth = StatusReport { state: Some(ProgramState::Blocked(Blocked::Auth)), ..Default::default() };
        assert!(status_sequence(&auth).unwrap().contains("kind=auth"));
        let done = StatusReport { state: Some(ProgramState::Done), progress: Some(100), ..Default::default() };
        assert_eq!(status_sequence(&done).unwrap(), "\u{1b}]7501;state=done:app=kumi\u{1b}\\");
        let child = StatusReport { state: Some(ProgramState::Working), id: Some("job-3".into()), ..Default::default() };
        assert_eq!(status_sequence(&child).unwrap(), "\u{1b}]7501;state=working:id=job-3:app=kumi\u{1b}\\");
        assert_eq!(status_sequence(&StatusReport::default()), None);
    }

    #[test]
    fn ids_outside_the_grammar_are_never_sent() {
        for id in ["", "job 3", "a/", &"x".repeat(33), "a/b/c/d/e/f/g/h/i", "jöb"] {
            let report = StatusReport { state: Some(ProgramState::Done), id: Some(id.into()), ..Default::default() };
            assert_eq!(status_sequence(&report), None, "{id:?}");
        }
        assert_eq!(clear_sequence(None), "\u{1b}]7501;state=clear\u{1b}\\");
        assert_eq!(clear_sequence(Some("job-2")), "\u{1b}]7501;state=clear:id=job-2\u{1b}\\");
    }

    #[test]
    fn words_lose_control_characters_and_fit_the_limits() {
        assert_eq!(one_line("one\ntwo\tthree\u{7}", 100), "one two three");
        assert_eq!(one_line("ab cd ef", 6), "ab…");
        let long = "é".repeat(5000);
        let report = StatusReport { state: Some(ProgramState::Done), title: Some(long.clone()), msg: Some(long), ..Default::default() };
        let sequence = status_sequence(&report).unwrap();
        assert!(sequence.len() <= MAX_SEQUENCE, "{}", sequence.len());
        let body = sequence.strip_prefix("\u{1b}]7501;").unwrap().strip_suffix("\u{1b}\\").unwrap();
        let get = |key: &str| body.split(':').find_map(|pair| pair.strip_prefix(&format!("{key}="))).map(decode).unwrap();
        assert!(get("title").len() <= MAX_TITLE);
        assert!(get("msg").len() <= MAX_MSG);
        assert!(!get("msg").chars().any(char::is_control));
    }

    #[test]
    fn progress_follows_the_state() {
        assert_eq!(progress_sequence(ProgramState::Working, Some(55)), "\u{1b}]9;4;1;55\u{1b}\\");
        assert_eq!(progress_sequence(ProgramState::Working, None), "\u{1b}]9;4;3;0\u{1b}\\");
        assert_eq!(progress_sequence(ProgramState::Blocked(Blocked::Question), None), "\u{1b}]9;4;4;0\u{1b}\\");
        assert_eq!(progress_sequence(ProgramState::Error, None), "\u{1b}]9;4;2;100\u{1b}\\");
        assert_eq!(progress_sequence(ProgramState::Done, Some(100)), PROGRESS_CLEAR);
    }

    #[test]
    fn notifications_take_each_channels_form() {
        assert_eq!(
            notify_sequence(NotifyChannel::Osc777, "Kumi; done", "bass is darker; kept 2").unwrap(),
            "\u{1b}]777;notify;Kumi, done;bass is darker; kept 2\u{1b}\\"
        );
        assert_eq!(notify_sequence(NotifyChannel::Osc9, "Kumi", "done\nnow").unwrap(), "\u{1b}]9;Kumi: done now\u{1b}\\");
        let kitty = notify_sequence(NotifyChannel::Osc99, "Kumi", "needs you").unwrap();
        assert_eq!(
            kitty,
            format!("\u{1b}]99;i=kumi:d=0:e=1;{}\u{1b}\\\u{1b}]99;i=kumi:p=body:e=1;{}\u{1b}\\", encode("Kumi"), encode("needs you"))
        );
        assert_eq!(notify_sequence(NotifyChannel::Bell, "Kumi", "x").unwrap(), "\u{7}");
        assert_eq!(notify_sequence(NotifyChannel::AppleScript, "Kumi", "x"), None);
        let (program, args) = notify_command(NotifyChannel::AppleScript, "Kumi", "say \"hi\" \\ there").unwrap();
        assert_eq!(program, "osascript");
        assert_eq!(args, ["-e", "display notification \"say \\\"hi\\\" \\\\ there\" with title \"Kumi\""]);
        let (program, args) = notify_command(NotifyChannel::NotifySend, "Kumi", "--done").unwrap();
        assert_eq!(
            (program, args.as_slice()),
            ("notify-send", ["--app-name=Kumi".to_string(), "--".into(), "Kumi".into(), "--done".into()].as_slice())
        );
        assert_eq!(notify_command(NotifyChannel::Osc777, "Kumi", "x"), None);
    }
}
