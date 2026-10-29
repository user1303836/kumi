//! Decodes what a terminal sends in raw mode: typed text, keys with modifiers (xterm and
//! CSI u encodings), bracketed pastes, SGR mouse reports and focus changes. Sequences can
//! arrive split across reads; a lone Escape is only a key once nothing follows it. Terminals
//! using the kitty keyboard protocol also say when a key repeats as it's held, and when it's
//! let go.
//!
//! The Escape timer is a Tokio task, so a parser lives inside a `LocalSet`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tokio::task::JoinHandle;

use kumi_common::js;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers { ctrl: false, alt: false, shift: false };
    pub const CTRL: Modifiers = Modifiers { ctrl: true, alt: false, shift: false };
    pub const ALT: Modifiers = Modifiers { ctrl: false, alt: true, shift: false };
    pub const SHIFT: Modifiers = Modifiers { ctrl: false, alt: false, shift: true };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseAction {
    Press,
    Release,
    Drag,
    Move,
    Wheel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WheelDirection {
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputEvent {
    Key {
        name: String,
        mods: Modifiers,
        /// Held down and repeating, where the terminal says so.
        repeat: bool,
    },
    /// A key let go: only terminals using the kitty protocol report it.
    Release {
        name: String,
        mods: Modifiers,
    },
    Text {
        text: String,
    },
    Paste {
        text: String,
    },
    Mouse {
        action: MouseAction,
        button: MouseButton,
        direction: Option<WheelDirection>,
        x: i32,
        y: i32,
        mods: Modifiers,
    },
    Focus {
        focused: bool,
    },
    /// The terminal's answer to something Kumi asked it as it started (`tui::probe`): read, never typed, even when it
    /// comes late.
    Reply(Reply),
}

/// A terminal's answer to one of the questions Kumi asks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// XTVERSION's: its name and version (`kitty(0.35.2)`, `ghostty 1.1.3`, `tmux 3.4`).
    Version(String),
    /// DECRPM's: whether a private mode is known and set (`CSI ? 2026 ; 2 $ y`).
    Mode { mode: u32, value: u32 },
    /// The kitty keyboard protocol's flags: the terminal speaks it.
    KittyKeys(u32),
    /// OSC 10's (the text's colour) or 11's (the background's), as 8-bit red, green and blue when Kumi can read it.
    Color { which: u32, rgb: Option<[u8; 3]> },
    /// DA1's, asked last: the others have come, or won't.
    Attributes(String),
    /// The terminal's colours turned dark or light (its report once mode 2031 is on: `CSI ? 997 ; 1 n` dark, `2` light).
    Scheme { dark: bool },
}

/// What an OSC answer says: a colour Kumi asked for (`11;rgb:1e1e/1e1e/1e1e`), or nothing Kumi reads.
fn osc_reply(content: &str) -> Option<Reply> {
    let (which, value) = content.split_once(';')?;
    let which = which.parse::<u32>().ok().filter(|which| matches!(which, 10 | 11))?;
    Some(Reply::Color { which, rgb: rgb(value) })
}

/// `rgb:R/G/B` with one to four hex digits each, as 8-bit channels.
fn rgb(value: &str) -> Option<[u8; 3]> {
    let channel = |hex: &str| {
        let full = (1u32 << (4 * hex.len().clamp(1, 4))) - 1;
        u32::from_str_radix(hex, 16).ok().filter(|_| (1..=4).contains(&hex.len())).map(|value| ((value * 255 + full / 2) / full) as u8)
    };
    let mut channels = value.strip_prefix("rgb:")?.split('/').map(channel);
    let rgb = [channels.next()??, channels.next()??, channels.next()??];
    channels.next().is_none().then_some(rgb)
}

impl InputEvent {
    /// A key pressed with these modifiers.
    pub fn key(name: &str, mods: Modifiers) -> InputEvent {
        InputEvent::Key { name: name.to_string(), mods, repeat: false }
    }

    pub fn text(text: &str) -> InputEvent {
        InputEvent::Text { text: text.to_string() }
    }
}

const PASTE_START: &str = "\u{1b}[200~";
const PASTE_END: &str = "\u{1b}[201~";

fn arrow(final_byte: char) -> Option<&'static str> {
    Some(match final_byte {
        'A' => "up",
        'B' => "down",
        'C' => "right",
        'D' => "left",
        'H' => "home",
        'F' => "end",
        _ => return None,
    })
}

fn ss3(final_byte: char) -> Option<&'static str> {
    arrow(final_byte).or(match final_byte {
        'P' => Some("f1"),
        'Q' => Some("f2"),
        'R' => Some("f3"),
        'S' => Some("f4"),
        _ => None,
    })
}

fn tilde(number: f64) -> Option<&'static str> {
    if number.fract() != 0.0 {
        return None;
    }
    Some(match number as i64 {
        1 => "home",
        2 => "insert",
        3 => "delete",
        4 => "end",
        5 => "pageup",
        6 => "pagedown",
        7 => "home",
        8 => "end",
        11 => "f1",
        12 => "f2",
        13 => "f3",
        14 => "f4",
        15 => "f5",
        17 => "f6",
        18 => "f7",
        19 => "f8",
        20 => "f9",
        21 => "f10",
        23 => "f11",
        24 => "f12",
        _ => return None,
    })
}

/// `Number(text)`: NaN as `None`.
fn number(text: &str) -> Option<f64> {
    js::number::parse(text)
}

/// xterm's modifier parameter: 1 + shift(1) + alt(2) + ctrl(4) + meta(8), meta read as alt.
fn modifiers(parameter: Option<&str>) -> Modifiers {
    let value = parameter.map_or(Some(1.0), number).filter(|value| *value != 0.0).unwrap_or(1.0);
    let bits = (value - 1.0).max(0.0) as i64;
    Modifiers { shift: bits & 1 != 0, alt: bits & 2 != 0 || bits & 8 != 0, ctrl: bits & 4 != 0 }
}

/// The modifiers, and after them the kitty protocol's event ("5:3"): 1 pressed (or unsaid), 2 repeating, 3 let go.
fn key_state(parameter: Option<&str>) -> (Modifiers, i64) {
    let parameter = parameter.unwrap_or("1");
    let mut parts = parameter.split(':');
    let mods = parts.next();
    let event = parts.next().and_then(number).filter(|value| *value != 0.0).map_or(1, |value| value as i64);
    (modifiers(mods), event)
}

fn lines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn is_control(byte: u8) -> bool {
    byte < 0x20 || byte == 0x7f
}

struct State {
    buffer: String,
    pasted: Option<String>,
    timer: Option<JoinHandle<()>>,
}

struct Inner {
    emit: Rc<dyn Fn(InputEvent)>,
    escape_delay_ms: f64,
    state: RefCell<State>,
}

#[derive(Clone)]
pub struct InputParser {
    inner: Rc<Inner>,
}

impl InputParser {
    /// Decodes into `emit`, waiting 25 ms after a lone Escape before calling it a key.
    pub fn new(emit: Rc<dyn Fn(InputEvent)>) -> InputParser {
        InputParser::with_escape_delay(emit, 25.0)
    }

    pub fn with_escape_delay(emit: Rc<dyn Fn(InputEvent)>, escape_delay_ms: f64) -> InputParser {
        InputParser {
            inner: Rc::new(Inner {
                emit,
                escape_delay_ms,
                state: RefCell::new(State { buffer: String::new(), pasted: None, timer: None }),
            }),
        }
    }

    pub fn push(&self, chunk: &str) {
        let mut events = Vec::new();
        {
            let mut state = self.inner.state.borrow_mut();
            cancel_timer(&mut state);
            // Terminals without bracketed paste deliver a multi-line paste in one read; Enter
            // inside it must not send the message.
            if state.pasted.is_none() && state.buffer.is_empty() && !chunk.contains('\u{1b}') && has_line_break_inside(chunk) {
                drop(state);
                (self.inner.emit)(InputEvent::Paste { text: lines(chunk) });
                return;
            }
            state.buffer.push_str(chunk);
            self.parse(&mut state, false, &mut events);
        }
        for event in events {
            (self.inner.emit)(event);
        }
    }

    /// Whether typed bytes wait to be read as a key: a lone Escape, until its delay passes (or what follows it comes).
    pub fn waiting(&self) -> bool {
        !self.inner.state.borrow().buffer.is_empty()
    }

    /// Stop the pending Escape timer; call when input ends.
    pub fn dispose(&self) {
        cancel_timer(&mut self.inner.state.borrow_mut());
    }

    fn timed_out(&self) {
        let mut events = Vec::new();
        {
            let mut state = self.inner.state.borrow_mut();
            state.timer = None;
            self.parse(&mut state, true, &mut events);
        }
        for event in events {
            (self.inner.emit)(event);
        }
    }

    fn parse(&self, state: &mut State, timed_out: bool, events: &mut Vec<InputEvent>) {
        while !state.buffer.is_empty() {
            if let Some(pasted) = state.pasted.as_mut() {
                match state.buffer.find(PASTE_END) {
                    None => {
                        let mut keep = 0;
                        for length in (1..=(PASTE_END.len() - 1).min(state.buffer.len())).rev() {
                            if state.buffer.ends_with(&PASTE_END[..length]) {
                                keep = length;
                                break;
                            }
                        }
                        let split = state.buffer.len() - keep;
                        pasted.push_str(&state.buffer[..split]);
                        state.buffer.drain(..split);
                        return;
                    }
                    Some(end) => {
                        pasted.push_str(&state.buffer[..end]);
                        state.buffer.drain(..end + PASTE_END.len());
                        let text = lines(&state.pasted.take().unwrap_or_default());
                        events.push(InputEvent::Paste { text });
                        continue;
                    }
                }
            }
            let first = state.buffer.as_bytes()[0];
            if first == 0x1b {
                let used = escape(state, timed_out, events);
                if used == 0 {
                    let parser = self.clone();
                    // A terminal's answer under way can come in reads further apart than the Escape delay (over SSH): it's
                    // waited for longer, so its head isn't dropped and its tail typed.
                    let wait = if answer_under_way(&state.buffer) { REPLY_WAIT_MS } else { 0.0 };
                    let delay = Duration::from_secs_f64(self.inner.escape_delay_ms.max(wait).max(0.0) / 1000.0);
                    // Start the timeout when input arrives, as setTimeout does,
                    // even if the spawned task is first polled much later.
                    let timeout = tokio::time::sleep(delay);
                    state.timer = Some(tokio::task::spawn_local(async move {
                        timeout.await;
                        parser.timed_out();
                    }));
                    return;
                }
                state.buffer.drain(..used);
                continue;
            }
            if is_control(first) {
                control(first as char, Modifiers::NONE, events);
                state.buffer.drain(..1);
                continue;
            }
            let end = state.buffer.bytes().position(is_control).unwrap_or(state.buffer.len());
            events.push(InputEvent::Text { text: state.buffer[..end].to_string() });
            state.buffer.drain(..end);
        }
    }
}

/// `/[\r\n][^\r\n]/`: a line break with something after it.
fn has_line_break_inside(chunk: &str) -> bool {
    let mut after_break = false;
    for character in chunk.chars() {
        let is_break = character == '\r' || character == '\n';
        if after_break && !is_break {
            return true;
        }
        after_break = is_break;
    }
    false
}

fn cancel_timer(state: &mut State) {
    if let Some(timer) = state.timer.take() {
        timer.abort();
    }
}

fn key(events: &mut Vec<InputEvent>, name: &str, mods: Modifiers, event: i64) {
    if event == 3 {
        events.push(InputEvent::Release { name: name.to_string(), mods });
    } else {
        events.push(InputEvent::Key { name: name.to_string(), mods, repeat: event == 2 });
    }
}

/// Characters consumed from an escape at the start of the buffer, or 0 to wait for more.
/// How long a terminal's answer under way is waited for, at most, between its reads.
const REPLY_WAIT_MS: f64 = 500.0;

/// What starts only a terminal's answer, never a key: an OSC with its number (`ESC ] 11 ;`), a DCS report
/// (`ESC P >|`, `ESC P 1 +`), a CSI with `?` (`ESC [ ?`), kitty's graphics reply (`ESC _ G i=`).
fn answer_under_way(buffer: &str) -> bool {
    let numbered = |text: &str| {
        let digits = text.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0 && text[digits..].starts_with(';')
    };
    buffer.strip_prefix("\u{1b}]").is_some_and(numbered)
        || buffer.strip_prefix("\u{1b}P").is_some_and(|text| text.starts_with('>') || text.starts_with(|c: char| c.is_ascii_digit()))
        || buffer.starts_with("\u{1b}[?")
        || buffer.strip_prefix("\u{1b}_G").is_some_and(|text| ["i=", "I=", "p=", ";"].iter().any(|start| text.starts_with(start)))
}

/// Where a string sequence's content ends and the sequence does: at ST (`ESC \`) or BEL, whichever comes first.
fn terminated(buffer: &str) -> Option<(usize, usize)> {
    let st = buffer[2..].find("\u{1b}\\").map(|at| (2 + at, 2 + at + 2));
    let bel = buffer[2..].find('\u{7}').map(|at| (2 + at, 2 + at + 1));
    [st, bel].into_iter().flatten().min()
}

fn escape(state: &mut State, timed_out: bool, events: &mut Vec<InputEvent>) -> usize {
    let buffer = state.buffer.as_str();
    if buffer.len() == 1 {
        if !timed_out {
            return 0;
        }
        key(events, "escape", Modifiers::NONE, 1);
        return 1;
    }
    let second = buffer.as_bytes()[1];
    if second == b'[' {
        return csi(state, timed_out, events);
    }
    // A terminal's reply: Kitty's graphics (APC `_G`), a DCS report. Kumi asks for none (its images ask for no
    // reply), but one is never typed into the box.
    let third = buffer.as_bytes().get(2).copied();
    // Kitty's replies go on `i=`, `I=`, `p=` or `;` after the G: Alt-_ and a typed G don't.
    let after = buffer.get(3..).unwrap_or("");
    let kitty = second == b'_' && third == Some(b'G') && ["i=", "I=", "p=", ";"].iter().any(|start| after.starts_with(start));
    if second == b'_' && third == Some(b'G') && !kitty && !timed_out && ["", "i", "I", "p"].contains(&after) {
        return 0;
    }
    if kitty || (second == b'P' && third.is_some_and(|b| b.is_ascii_digit() || b == b'>')) {
        return match terminated(buffer) {
            Some((content, end)) => {
                // XTVERSION's answer: `DCS > | name ST`.
                if let Some(version) = buffer[2..content].strip_prefix(">|").filter(|_| second == b'P') {
                    events.push(InputEvent::Reply(Reply::Version(version.to_string())));
                }
                end
            }
            None if !timed_out && buffer.len() < 64 * 1024 => 0,
            None => buffer.len(),
        };
    }
    // An OSC answer (a colour: `OSC 11 ; rgb:… BEL`) is read, never typed, and its BEL is never ctrl+g. Alt-] on its own
    // waits the Escape delay, as Escape does.
    if second == b']' {
        let rest = &buffer[2..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == rest.len() && digits < 4 && !timed_out {
            return 0;
        }
        if digits > 0 && rest.as_bytes().get(digits) == Some(&b';') {
            return match terminated(buffer) {
                Some((content, end)) => {
                    if let Some(reply) = osc_reply(&buffer[2..content]) {
                        events.push(InputEvent::Reply(reply));
                    }
                    end
                }
                None if !timed_out && buffer.len() < 64 * 1024 => 0,
                None => buffer.len(),
            };
        }
    }
    if second == b'O' {
        if buffer.len() == 2 && !timed_out {
            return 0;
        }
        let name = buffer[2..].chars().next().and_then(ss3);
        if let Some(name) = name {
            key(events, name, Modifiers::NONE, 1);
            return 3;
        }
        key(events, "o", Modifiers { alt: true, shift: true, ctrl: false }, 1);
        return 2;
    }
    if second == 0x1b {
        key(events, "escape", Modifiers::NONE, 1);
        return 1;
    }
    if is_control(second) {
        control(second as char, Modifiers::ALT, events);
        return 2;
    }
    let character = buffer[1..].chars().next().expect("a character after Escape");
    let lower = character.to_lowercase().to_string();
    let shift = lower != character.to_string();
    key(events, &lower, Modifiers { alt: true, shift, ctrl: false }, 1);
    1 + character.len_utf8()
}

fn csi(state: &mut State, timed_out: bool, events: &mut Vec<InputEvent>) -> usize {
    let buffer = state.buffer.as_str();
    let bytes = buffer.as_bytes();
    let mut index = 2;
    while index < bytes.len() {
        if (0x40..=0x7e).contains(&bytes[index]) {
            break;
        }
        index += 1;
    }
    if index >= bytes.len() {
        if !timed_out && bytes.len() < 64 {
            return 0;
        }
        return bytes.len(); // truncated or garbled; drop it rather than type it
    }
    let params = &buffer[2..index];
    let final_byte = bytes[index] as char;
    let length = index + 1;
    if buffer.starts_with(PASTE_START) {
        state.pasted = Some(String::new());
        return length;
    }
    if params == "201" && final_byte == '~' {
        return length;
    }
    // Answers to what Kumi asked: no key has `?` among its parameters.
    if let Some(answer) = params.strip_prefix('?') {
        let reply = match final_byte {
            'c' => Some(Reply::Attributes(answer.to_string())),
            'u' => answer.parse().ok().map(Reply::KittyKeys),
            'n' => match answer {
                "997;1" => Some(Reply::Scheme { dark: true }),
                "997;2" => Some(Reply::Scheme { dark: false }),
                _ => None,
            },
            'y' => answer
                .strip_suffix('$')
                .and_then(|answer| answer.split_once(';'))
                .and_then(|(mode, value)| Some(Reply::Mode { mode: mode.parse().ok()?, value: value.parse().ok()? })),
            _ => None,
        };
        events.extend(reply.map(InputEvent::Reply));
        return length;
    }
    if let Some(report) = params.strip_prefix('<') {
        if final_byte == 'M' || final_byte == 'm' {
            mouse(report, final_byte == 'M', events);
            return length;
        }
    }
    if params.is_empty() && (final_byte == 'I' || final_byte == 'O') {
        events.push(InputEvent::Focus { focused: final_byte == 'I' });
        return length;
    }
    if final_byte == 'Z' {
        key(events, "tab", Modifiers::SHIFT, 1);
        return length;
    }
    let parts: Vec<&str> = params.split(';').collect();
    if let Some(name) = arrow(final_byte).or_else(|| if "PQRS".contains(final_byte) { ss3(final_byte) } else { None }) {
        let (mods, event) = key_state(parts.get(1).copied());
        key(events, name, mods, event);
        return length;
    }
    if final_byte == '~' {
        let value = number(parts[0]);
        if value == Some(27.0) && parts.len() >= 3 {
            code_key(number(parts[2]), modifiers(parts.get(1).copied()), 1, events);
        } else if let Some(name) = value.and_then(tilde) {
            let (mods, event) = key_state(parts.get(1).copied());
            key(events, name, mods, event);
        }
        return length;
    }
    if final_byte == 'u' {
        let (mods, event) = key_state(parts.get(1).copied());
        code_key(number(parts[0].split(':').next().unwrap_or("")), mods, event, events);
        return length;
    }
    length // an unknown sequence is ignored, never typed
}

fn code_key(code: Option<f64>, mods: Modifiers, event: i64, events: &mut Vec<InputEvent>) {
    let named = match code {
        Some(9.0) => Some("tab"),
        Some(13.0) => Some("enter"),
        Some(27.0) => Some("escape"),
        Some(127.0) | Some(8.0) => Some("backspace"),
        _ => None,
    };
    if let Some(name) = named {
        key(events, name, mods, event);
        return;
    }
    let Some(code) = code.filter(|code| code.fract() == 0.0 && *code >= 32.0) else { return };
    // TS: String.fromCodePoint throws past U+10FFFF; such a code is ignored here.
    let Some(character) = u32::try_from(code as i64).ok().and_then(char::from_u32) else { return };
    // Text is typed as it's pressed (and again as it repeats); letting go of a letter means nothing.
    if !mods.ctrl && !mods.alt {
        if event != 3 {
            let text = if mods.shift { character.to_uppercase().to_string() } else { character.to_string() };
            events.push(InputEvent::Text { text });
        }
    } else {
        let name = if code == 32.0 { "space".to_string() } else { character.to_lowercase().to_string() };
        key(events, &name, mods, event);
    }
}

fn control(character: char, mods: Modifiers, events: &mut Vec<InputEvent>) {
    match character {
        '\r' => return key(events, "enter", mods, 1),
        '\n' => return key(events, "j", Modifiers { ctrl: true, ..mods }, 1),
        '\t' => return key(events, "tab", mods, 1),
        '\u{7f}' | '\u{8}' => return key(events, "backspace", mods, 1),
        '\u{0}' => return key(events, "space", Modifiers { ctrl: true, ..mods }, 1),
        _ => {}
    }
    let code = character as u32;
    if (1..=26).contains(&code) {
        let letter = char::from_u32(code + 96).expect("a lowercase letter");
        key(events, &letter.to_string(), Modifiers { ctrl: true, ..mods }, 1);
        return;
    }
    let symbol = match code {
        0x1c => "\\",
        0x1d => "]",
        0x1e => "^",
        0x1f => "_",
        _ => return,
    };
    key(events, symbol, Modifiers { ctrl: true, ..mods }, 1);
}

fn mouse(params: &str, pressed: bool, events: &mut Vec<InputEvent>) {
    let mut fields = params.split(';').map(number);
    // `[bits = 0, column = 1, row = 1]`: a missing field has its default; one that isn't a number is NaN (0 in bit tests).
    let bits = fields.next().map_or(0, |value| value.map_or(0, |value| value as i64));
    let column = fields.next().map_or(1.0, |value| value.unwrap_or(f64::NAN));
    let row = fields.next().map_or(1.0, |value| value.unwrap_or(f64::NAN));
    let mods = Modifiers { shift: bits & 4 != 0, alt: bits & 8 != 0, ctrl: bits & 16 != 0 };
    let x = (column - 1.0) as i32;
    let y = (row - 1.0) as i32;
    if bits & 64 != 0 {
        if (bits & 3) < 2 {
            let direction = if bits & 1 != 0 { WheelDirection::Down } else { WheelDirection::Up };
            events.push(InputEvent::Mouse {
                action: MouseAction::Wheel,
                button: MouseButton::None,
                direction: Some(direction),
                x,
                y,
                mods,
            });
        }
        return;
    }
    let button = [MouseButton::Left, MouseButton::Middle, MouseButton::Right, MouseButton::None][(bits & 3) as usize];
    let action = if bits & 32 != 0 {
        if button == MouseButton::None {
            MouseAction::Move
        } else {
            MouseAction::Drag
        }
    } else if pressed {
        MouseAction::Press
    } else {
        MouseAction::Release
    };
    events.push(InputEvent::Mouse { action, button, direction: None, x, y, mods });
}
