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
                    let delay = Duration::from_secs_f64(self.inner.escape_delay_ms.max(0.0) / 1000.0);
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
