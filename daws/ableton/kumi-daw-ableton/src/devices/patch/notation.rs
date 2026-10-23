//! Kumi's patch notation: how the model writes a Max patch with Max's own objects, read into boxes and cords. Each
//! object's inlets and outlets come from what Kumi learned of the installed Max; the device around the patch, its
//! layout and its checks are Kumi's.
//!
//! ```text
//! clock = [metro 125]                      # a box: an object as it's typed in Max
//! step = [counter 0 15]
//! clock -> step -> [sel 0] -> accent       # cords, outlet 0 to inlet 0; a box written in a chain is a new one
//! step.2 -> [print carry]                  # .n: the outlet on the left of ->, the inlet on the right, from 0
//! "Rate" -> [prepend interval] -> clock    # a control, by its name
//! accent = (bang)                          # a message box
//! fx = [gen~] { out1 = in1 * 0.5; }        # gen~ holds GenExpr
//! steps = [v8 steps] { … }                 # v8 holds JavaScript, embedded in the device
//! scope = [v8ui] { … }                     # v8ui holds JavaScript that draws, on the device's face
//! voice = [p voice] { … }                  # a subpatcher holds a patch; its [inlet]s and [outlet]s number from the
//!                                          # left, as they're written
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{json, Value};

use super::code::{declared_ports, syntax_problem};
use super::reference::{atoms, Reference, MOST_PORTS};
use crate::devices::findings::Findings;
use crate::devices::gen::{gen_patcher, inputs_read, outputs_written};

/// A patch as the notation writes it: its boxes, in order, and its cords.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Notation {
    pub boxes: Vec<Item>,
    pub cords: Vec<Link>,
}

/// A box the notation writes.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Its name, when it's given one (`name = [...]`).
    pub name: Option<String>,
    pub what: What,
    /// The line it's written on, from 1.
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum What {
    /// A Max object, by its text: "metro 125".
    Object(String),
    /// A message box's text: "set $1".
    Message(String),
    /// A gen~ (or a gen) and its GenExpr.
    Gen { text: String, code: String },
    /// A v8 (or a v8ui, which draws) and its JavaScript, embedded in the device.
    Js { text: String, code: String },
    /// A subpatcher (`[p name]`) and the patch in it.
    Patcher { text: String, inner: Notation },
}

impl What {
    /// Its object's class: "metro" for [metro 125], "message" for a message box.
    pub fn class(&self) -> String {
        match self {
            What::Object(text) | What::Gen { text, .. } | What::Js { text, .. } | What::Patcher { text, .. } => {
                atoms(text).into_iter().next().unwrap_or_default()
            }
            What::Message(_) => "message".into(),
        }
    }
}

/// One end of a cord: a box of the patch, or something outside it (a control, the device's in or out).
#[derive(Debug, Clone, PartialEq)]
pub enum End {
    Box(usize),
    Control(String),
    Outside(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub from: End,
    pub outlet: usize,
    pub to: End,
    pub inlet: usize,
    pub line: usize,
}

/// The names that are the device's own: what it receives and what it sends, the modulation its Map page maps, and the
/// step its step grid lights.
pub const OUTSIDE: [&str; 4] = ["in", "out", "mod", "playhead"];

/// What sends sound out of a device by itself. Where Kumi's output stage makes a device's sound safe, that stage is the
/// only way out.
pub(crate) const SPEAKERS: [&str; 6] = ["plugout~", "mc.plugout~", "dac~", "mc.dac~", "ezdac~", "mc.ezdac~"];

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Name(String),
    Equals,
    Arrow,
    Port(usize),
    Object(String),
    Message(String),
    Control(String),
    /// A block's text, and the line it starts on.
    Block(String, usize),
    End,
}

fn describe(token: Option<&Token>) -> String {
    match token {
        None | Some(Token::End) => "the end of the line".into(),
        Some(Token::Name(name)) => format!("the name {name}"),
        Some(Token::Equals) => "=".into(),
        Some(Token::Arrow) => "->".into(),
        Some(Token::Port(port)) => format!(".{port}"),
        Some(Token::Object(text)) => format!("[{text}]"),
        Some(Token::Message(text)) => format!("({text})"),
        Some(Token::Control(name)) => format!("\"{name}\""),
        Some(Token::Block(..)) => "a { … } block".into(),
    }
}

/// Where a run of text closes, from `at`: the first `close` outside quotes and nested `nests`, on the same line. A
/// backslash escapes the character after it, but never the line's end.
fn closing(chars: &[char], mut at: usize, close: char, nests: Option<char>) -> Option<usize> {
    let (mut quoted, mut depth) = (false, 0usize);
    while at < chars.len() && chars[at] != '\n' {
        let c = chars[at];
        if c == '\\' && chars.get(at + 1).is_some_and(|next| *next != '\n') {
            at += 2;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && Some(c) == nests {
            depth += 1;
        } else if !quoted && c == close {
            if depth == 0 {
                return Some(at);
            }
            depth -= 1;
        }
        at += 1;
    }
    None
}

/// What a block holds, which says how it's read for where it ends.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Language {
    /// A patch: # and // comments, and each box's text and control's name on its line.
    Notation,
    /// GenExpr: comments, and a buffer's name in quotes.
    GenExpr,
    /// JavaScript: comments, strings, template literals (and the code in their ${…}) and regular expressions.
    JavaScript,
}

/// What a block after a box of `class` holds.
fn language(class: &str) -> Language {
    match class {
        "gen~" | "gen" => Language::GenExpr,
        "p" | "patcher" => Language::Notation,
        _ => Language::JavaScript,
    }
}

/// How deep blocks are read inside one another: deeper is read as not closed.
const NESTING: usize = 64;

/// Where a block's closing brace is, from just inside it, and how many lines it passes, read as `reading` reads it: a
/// brace in a comment, a string or a box's text is only text, and a block inside a patch is read as what it holds.
fn block_end(chars: &[char], at: usize, reading: Language) -> Option<(usize, usize)> {
    block_end_within(chars, at, reading, 0)
}

fn block_end_within(chars: &[char], mut at: usize, reading: Language, nesting: usize) -> Option<(usize, usize)> {
    if nesting > NESTING {
        return None;
    }
    let (mut depth, mut lines) = (0usize, 0usize);
    // A patch's last box on the line, for the block after it; JavaScript's last two characters and last word, which say
    // whether a / starts a regular expression or divides.
    let (mut class, mut last, mut prior, mut word) = (String::new(), None::<char>, None::<char>, String::new());
    let line_end = |at: usize| (at..chars.len()).find(|&end| chars[end] == '\n').unwrap_or(chars.len());
    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        if c == '\n' {
            lines += 1;
            class.clear();
            at += 1;
            continue;
        }
        if (c == '/' && next == Some('/')) || (reading == Language::Notation && c == '#') {
            at = line_end(at);
            continue;
        }
        if c == '/' && next == Some('*') {
            let stop = (at + 2..chars.len().saturating_sub(1))
                .find(|&end| chars[end] == '*' && chars[end + 1] == '/')
                .map_or(chars.len(), |end| end + 2);
            lines += chars[at..stop].iter().filter(|c| **c == '\n').count();
            at = stop;
            continue;
        }
        match (reading, c) {
            (Language::Notation | Language::GenExpr, '"') => {
                at = quoted_end(chars, at, '"', false).0;
                continue;
            }
            (Language::Notation, '(') => {
                at = closing(chars, at + 1, ')', Some('(')).map_or_else(|| line_end(at), |end| end + 1);
                continue;
            }
            (Language::Notation, '[') => {
                match closing(chars, at + 1, ']', None) {
                    Some(end) => {
                        class = atoms(&chars[at + 1..end].iter().collect::<String>()).into_iter().next().unwrap_or_default();
                        at = end + 1;
                    }
                    None => at = line_end(at),
                }
                continue;
            }
            (Language::Notation, '{') => {
                let (end, inner) = block_end_within(chars, at + 1, language(&class), nesting + 1)?;
                lines += inner;
                at = end + 1;
                continue;
            }
            (Language::Notation, '}') => return Some((at, lines)),
            (Language::JavaScript, '"' | '\'') => {
                let (end, inner) = quoted_end(chars, at, c, true);
                lines += inner;
                (at, last) = (end, Some(c));
                continue;
            }
            (Language::JavaScript, '`') => {
                let (end, inner) = template_end(chars, at + 1, nesting)?;
                lines += inner;
                (at, last) = (end + 1, Some(c));
                continue;
            }
            (Language::JavaScript, '/') if starts_regex(last, prior, &word) => {
                if let Some(end) = regex_end(chars, at + 1) {
                    (at, last) = (end + 1, Some(c));
                    word.clear();
                    continue;
                }
            }
            (Language::GenExpr | Language::JavaScript, '{') => depth += 1,
            (Language::GenExpr | Language::JavaScript, '}') if depth == 0 => return Some((at, lines)),
            (Language::GenExpr | Language::JavaScript, '}') => depth -= 1,
            _ => {}
        }
        if reading == Language::JavaScript && !c.is_whitespace() {
            let in_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
            if !in_word(c) || !last.is_some_and(in_word) {
                word.clear();
            }
            if in_word(c) {
                word.push(c);
            }
            (prior, last) = (last, Some(c));
        }
        at += 1;
    }
    None
}

/// Past a string from its opening quote at `at`: just past its closing quote, or at the end of its line when it isn't
/// closed there, and the lines it passes. In JavaScript a backslash at a line's end carries the string on (`carries`).
fn quoted_end(chars: &[char], at: usize, quote: char, carries: bool) -> (usize, usize) {
    let (mut at, mut lines) = (at + 1, 0);
    while at < chars.len() && chars[at] != '\n' {
        if chars[at] == '\\' && at + 1 < chars.len() && (chars[at + 1] != '\n' || carries) {
            lines += usize::from(chars[at + 1] == '\n');
            at += 2;
            continue;
        }
        if chars[at] == quote {
            return (at + 1, lines);
        }
        at += 1;
    }
    (at, lines)
}

/// Where a template literal's closing backtick is, from just inside it, and the lines it passes: its ${…} is code.
fn template_end(chars: &[char], mut at: usize, nesting: usize) -> Option<(usize, usize)> {
    let mut lines = 0;
    while at < chars.len() {
        match chars[at] {
            '\\' => {
                lines += usize::from(chars.get(at + 1) == Some(&'\n'));
                at += 2;
                continue;
            }
            '\n' => lines += 1,
            '`' => return Some((at, lines)),
            '$' if chars.get(at + 1) == Some(&'{') => {
                let (end, inner) = block_end_within(chars, at + 2, Language::JavaScript, nesting + 1)?;
                lines += inner;
                at = end + 1;
                continue;
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// Whether a / after `last` (and `prior` before it, ending `word`) starts a regular expression rather than dividing: a
/// postfix ++ or -- divides.
fn starts_regex(last: Option<char>, prior: Option<char>, word: &str) -> bool {
    match last {
        None => true,
        Some(c @ ('+' | '-')) if prior == Some(c) => false,
        Some(c) if c.is_alphanumeric() || c == '_' || c == '$' => {
            matches!(
                word,
                "return" | "typeof" | "case" | "do" | "else" | "in" | "of" | "new" | "delete" | "void" | "throw" | "yield" | "await"
            )
        }
        Some(c) => !matches!(c, ')' | ']' | '}' | '"' | '\'' | '`'),
    }
}

/// Where a regular expression's closing / is, from just after its opening one, on the same line ([/] is in a class).
fn regex_end(chars: &[char], mut at: usize) -> Option<usize> {
    let mut class = false;
    while at < chars.len() && chars[at] != '\n' {
        match chars[at] {
            '\\' if chars.get(at + 1).is_some_and(|next| *next != '\n') => at += 1,
            '[' => class = true,
            ']' => class = false,
            '/' if !class => return Some(at),
            _ => {}
        }
        at += 1;
    }
    None
}

/// A line the tokens can't be read from is left out: what was read of its statement goes (a block that opened on a line
/// before it too), and reading goes on from the next line. The line its statement starts on.
fn leave_out(out: &mut Vec<(Token, usize)>, chars: &[char], at: &mut usize, line: usize) -> usize {
    let mut first = line;
    while out.last().is_some_and(|(token, _)| *token != Token::End) {
        if let Some((_, from)) = out.pop() {
            first = first.min(from);
        }
    }
    while *at < chars.len() && chars[*at] != '\n' {
        *at += 1;
    }
    first
}

/// What's said of a statement left out: the line where it went wrong, and the lines it covered when it's more than one.
fn left_out(at: usize, first: usize, last: usize, reason: &str, hint: &str) -> String {
    if first == last {
        format!("line {at}: {reason}, so Kumi left the line out{hint}.")
    } else {
        format!("line {at}: {reason}, so Kumi left out lines {first}–{last}, the statement it's part of{hint}.")
    }
}

/// The notation's tokens, each with its line, and the lines it can't read (each left out).
fn tokens(text: &str, first_line: usize) -> (Vec<(Token, usize)>, Vec<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut unread = Vec::new();
    let mut line = first_line;
    let mut at = 0;
    let text_of = |from: usize, to: usize| chars[from..to].iter().collect::<String>();
    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        match c {
            '\n' => {
                out.push((Token::End, line));
                line += 1;
                at += 1;
            }
            // A comment, or a code fence around the whole patch.
            '#' | '`' => {
                while at < chars.len() && chars[at] != '\n' {
                    at += 1;
                }
            }
            '/' if next == Some('/') => {
                while at < chars.len() && chars[at] != '\n' {
                    at += 1;
                }
            }
            c if c.is_whitespace() => at += 1,
            '[' => match closing(&chars, at + 1, ']', None) {
                Some(end) => {
                    out.push((Token::Object(text_of(at + 1, end).trim().to_string()), line));
                    at = end + 1;
                }
                None => {
                    let first = leave_out(&mut out, &chars, &mut at, line);
                    unread.push(left_out(line, first, line, "a [ isn't closed on its line", ""));
                }
            },
            '(' => match closing(&chars, at + 1, ')', Some('(')) {
                Some(end) => {
                    out.push((Token::Message(text_of(at + 1, end).trim().to_string()), line));
                    at = end + 1;
                }
                None => {
                    let first = leave_out(&mut out, &chars, &mut at, line);
                    unread.push(left_out(line, first, line, "a ( isn't closed on its line", ""));
                }
            },
            '"' => {
                // A control's name has no quotes in it: it ends at the next one.
                let end = chars[at + 1..]
                    .iter()
                    .position(|c| *c == '"' || *c == '\n')
                    .map(|offset| at + 1 + offset)
                    .filter(|end| chars[*end] == '"');
                match end {
                    Some(end) => {
                        out.push((Token::Control(text_of(at + 1, end)), line));
                        at = end + 1;
                    }
                    None => {
                        let first = leave_out(&mut out, &chars, &mut at, line);
                        unread.push(left_out(line, first, line, "a \" isn't closed on its line", ""));
                    }
                }
            }
            '{' => {
                // A block holds what the box before it holds, and it's read that way for where it ends.
                let class = match out.last() {
                    Some((Token::Object(text), _)) => atoms(text).into_iter().next().unwrap_or_default(),
                    _ => String::new(),
                };
                match block_end(&chars, at + 1, language(&class)) {
                    Some((end, lines)) => {
                        out.push((Token::Block(text_of(at + 1, end), line), line));
                        line += lines;
                        at = end + 1;
                    }
                    None => {
                        // Not closed, it holds the rest of the patch: said, so it's closed where it ends.
                        let rest = text_of(at + 1, chars.len());
                        let to = line + rest.trim_end().matches('\n').count();
                        unread.push(format!(
                            "line {line}: a {{ isn't closed, so Kumi took the rest of the patch (to line {to}) as its block: close it with }} where it ends."
                        ));
                        let lines = rest.matches('\n').count();
                        out.push((Token::Block(rest, line), line));
                        line += lines;
                        at = chars.len();
                    }
                }
            }
            '-' if next == Some('>') => {
                out.push((Token::Arrow, line));
                at += 2;
            }
            '→' => {
                out.push((Token::Arrow, line));
                at += 1;
            }
            '=' => {
                out.push((Token::Equals, line));
                at += 1;
            }
            '.' if next.is_some_and(|next| next.is_ascii_digit()) => {
                let digits = text_of(at + 1, chars.len()).chars().take_while(char::is_ascii_digit).collect::<String>();
                at += 1 + digits.len();
                match digits.parse() {
                    Ok(port) => out.push((Token::Port(port), line)),
                    Err(_) => {
                        let first = leave_out(&mut out, &chars, &mut at, line);
                        unread.push(left_out(line, first, line, &format!(".{digits} isn't a port"), ""));
                    }
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let name: String = chars[at..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == '_').collect();
                at += name.len();
                out.push((Token::Name(name), line));
            }
            other => {
                let first = leave_out(&mut out, &chars, &mut at, line);
                let reason = format!("\"{other}\" isn't part of the notation (a box is [object] or (message), a cord is ->)");
                unread.push(left_out(line, first, line, &reason, ""));
            }
        }
    }
    out.push((Token::End, line));
    (out, unread)
}

/// The objects that hold what a block gives them: GenExpr for gen~ and gen, JavaScript for v8 and v8ui, a patch for a
/// subpatcher.
fn holds(class: &str) -> Option<&'static str> {
    match class {
        "gen~" | "gen" => Some("GenExpr"),
        "v8" | "v8ui" => Some("JavaScript"),
        "p" | "patcher" => Some("a patch"),
        _ => None,
    }
}

/// A cord's end before every name is known.
#[derive(Debug, Clone)]
enum Ref {
    Box(usize),
    Control(String),
    Name(String),
}

/// A patch written in the notation, read the nearest way it can be, and what was found reading it: each line that
/// can't be read is left out, and said with its number.
pub fn parse(text: &str) -> (Notation, Findings) {
    parse_from(text, 1, true, 0)
}

/// Reads a patch's lines: `first_line` is its first line in the whole notation; `top` is the device's own patch,
/// where `in` and `out` are the device's.
struct Reader<'a> {
    tokens: &'a [(Token, usize)],
    at: usize,
    notation: Notation,
    names: HashMap<String, (usize, usize)>,
    hops: Vec<(Ref, usize, Ref, usize, usize)>,
    found: Findings,
    /// How many subpatchers this patch is inside.
    depth: usize,
}

/// Why a statement is left out: what went wrong, the line it went wrong on, and what to write instead (said after it).
struct Failure {
    reason: String,
    at: usize,
    hint: &'static str,
}

/// How many subpatchers deep a patch is read: one deeper is made empty.
const DEPTH: usize = 16;

/// The most boxes one patcher is made with: laying a patcher out and checking it takes longer the more boxes it has, and
/// a well-made device has a few dozen in each.
pub const MOST_BOXES: usize = 2_000;

impl Reader<'_> {
    fn token(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(token, _)| token)
    }

    /// A statement left out for `reason` at the token here.
    fn failure(&self, reason: String, hint: &'static str) -> Failure {
        Failure { reason, at: self.tokens.get(self.at).map_or(0, |(_, line)| *line), hint }
    }

    /// The box whose object or message starts here, with the block after it, made the nearest way it can be.
    fn boxed(&mut self, line: usize) -> Result<What, Failure> {
        let what = match self.token().cloned() {
            Some(Token::Object(text)) => {
                let class = atoms(&text).into_iter().next().unwrap_or_default();
                if class.is_empty() {
                    return Err(self.failure("[] is empty".into(), ": write the object in it, as it's typed in Max"));
                }
                self.at += 1;
                let block = match self.token() {
                    Some(Token::Block(block, from)) => Some((block.clone(), *from)),
                    _ => None,
                };
                if block.is_some() {
                    self.at += 1;
                }
                match (block, holds(&class)) {
                    (Some((code, _)), Some("GenExpr")) => What::Gen { text, code },
                    (Some((code, _)), Some("JavaScript")) => What::Js { text, code },
                    (Some(_), Some(_)) if self.depth >= DEPTH => {
                        self.found.wrong.push(format!(
                            "line {line}: [{text}] is inside {DEPTH} subpatchers, so Kumi made it empty: put what's in it in one less deep."
                        ));
                        What::Patcher { text, inner: Notation::default() }
                    }
                    (Some((inner, from)), Some(_)) => {
                        let (inner, found) = parse_from(&inner, from, false, self.depth + 1);
                        self.found.append(found);
                        What::Patcher { text, inner }
                    }
                    (Some((code, _)), None) if matches!(class.as_str(), "js" | "jsui" | "v8.codebox") => {
                        // The way it's written now: Max's modern engine, its code embedded in the device.
                        let made = if class == "jsui" { "v8ui".to_string() } else { format!("v8{}", &text[class.len()..]) };
                        self.found.changed.push(format!(
                            "line {line}: [{text}] {{ … }} was made as [{made}] {{ … }}: Max's modern engine, its code embedded in the device."
                        ));
                        What::Js { text: made, code }
                    }
                    (Some(_), None) => {
                        self.found.wrong.push(format!(
                            "line {line}: only [gen~], [gen], [v8], [v8ui] and [p] hold a {{ … }} block, so [{class}] was made without what it was given: put that in one of them."
                        ));
                        What::Object(text)
                    }
                    (None, Some(held)) => {
                        self.found.wrong.push(format!(
                            "line {line}: [{text}] holds {held}, and it was given none, so it's empty: write it in a {{ … }} block after the box."
                        ));
                        match held {
                            "GenExpr" => What::Gen { text, code: String::new() },
                            "JavaScript" => What::Js { text, code: String::new() },
                            _ => What::Patcher { text, inner: Notation::default() },
                        }
                    }
                    (None, None) if class.starts_with("kumi.") => match crate::devices::kit::expand(&text, line) {
                        // One of Kumi's building blocks: a subpatcher of Max's own objects, made the way it's tested.
                        Ok((head, patch)) => {
                            let (inner, found) = parse_from(&patch, line, false, self.depth + 1);
                            self.found.append(found);
                            What::Patcher { text: head, inner }
                        }
                        Err(why) => {
                            self.found.wrong.push(format!("line {line}: {why}, so Max will show [{text}] as a broken box."));
                            What::Object(text)
                        }
                    },
                    (None, None) => What::Object(text),
                }
            }
            Some(Token::Message(text)) => {
                self.at += 1;
                if matches!(self.token(), Some(Token::Block(..))) {
                    self.at += 1;
                    self.found
                        .changed
                        .push(format!("line {line}: a message box holds no {{ … }} block, so ({text}) was made without one."));
                }
                What::Message(text)
            }
            other => return Err(self.failure(format!("expected a box, not {}", describe(other.as_ref())), "")),
        };
        Ok(what)
    }

    /// A box for the patch, by name or not. A name that can't be its own (the device's in or out, or one already
    /// taken) is left off the box.
    fn add(&mut self, name: Option<String>, what: What, line: usize) -> Ref {
        let mut name = name;
        if let Some(given) = name.clone() {
            if OUTSIDE.contains(&given.as_str()) {
                self.found.wrong.push(format!(
                    "line {line}: {given} is the device's own (what it receives and sends), so the box was made without that name: name it something else."
                ));
                name = None;
            } else if let Some((_, first)) = self.names.get(&given) {
                self.found.wrong.push(format!(
                    "line {line}: {given} is named twice (first on line {first}), so this box was made without the name, and cords by it reach the first."
                ));
                name = None;
            } else {
                self.names.insert(given, (self.notation.boxes.len(), line));
            }
        }
        self.notation.boxes.push(Item { name, what, line });
        Ref::Box(self.notation.boxes.len() - 1)
    }

    /// One element of a chain: a box (named, by name, or written in place) or a control, and its port.
    fn element(&mut self, line: usize) -> Result<(Ref, Option<usize>), Failure> {
        let found = match self.token().cloned() {
            Some(Token::Name(name)) => {
                self.at += 1;
                if self.token() == Some(&Token::Equals) {
                    self.at += 1;
                    if !matches!(self.token(), Some(Token::Object(_) | Token::Message(_))) {
                        let after = describe(self.token());
                        return Err(self.failure(format!("{name} = needs a box after it, [object] or (message), not {after}"), ""));
                    }
                    let what = self.boxed(line)?;
                    self.add(Some(name), what, line)
                } else {
                    Ref::Name(name)
                }
            }
            Some(Token::Object(_) | Token::Message(_)) => {
                let what = self.boxed(line)?;
                self.add(None, what, line)
            }
            Some(Token::Control(name)) => {
                self.at += 1;
                Ref::Control(name)
            }
            other => return Err(self.failure(format!("expected a box, a name or a control, not {}", describe(other.as_ref())), "")),
        };
        let port = match self.token() {
            Some(Token::Port(port)) => {
                let port = *port;
                self.at += 1;
                Some(port)
            }
            _ => None,
        };
        Ok((found, port))
    }

    /// One statement: a box, or a chain of cords, on a line or carried over several by a block. One that can't be read is
    /// left out whole, its boxes and what was said reading it too.
    fn statement(&mut self) {
        let line = self.tokens[self.at].1;
        let first_box = self.notation.boxes.len();
        let said = (self.found.wrong.len(), self.found.changed.len(), self.found.notes.len());
        // A line that names a box (name = [object]) is the box, whether or not the name could be kept.
        let names = matches!(self.tokens.get(self.at), Some((Token::Name(_), _)))
            && matches!(self.tokens.get(self.at + 1), Some((Token::Equals, _)));
        let mut chain: Vec<(Ref, Option<usize>)> = Vec::new();
        let mut failure = loop {
            match self.element(line) {
                Ok(element) => chain.push(element),
                Err(failure) => break Some(failure),
            }
            match self.token() {
                Some(Token::Arrow) => self.at += 1,
                Some(Token::End) | None => break None,
                other => {
                    let other = describe(other);
                    break Some(self.failure(format!("expected -> or the end of the line after a box, not {other}"), ""));
                }
            }
        };
        // On to the next line, past whatever's left of the statement: the line it ends on.
        while self.at < self.tokens.len() && self.tokens[self.at].0 != Token::End {
            self.at += 1;
        }
        let last = self.tokens.get(self.at).map_or(line, |(_, end)| *end);
        let end = chain.len().saturating_sub(1);
        if failure.is_none() && chain.iter().enumerate().any(|(at, (_, port))| port.is_some() && at != 0 && at != end) {
            failure = Some(Failure {
                reason: "a box in the middle of a chain is entered at inlet 0 and left by outlet 0".into(),
                at: line,
                hint: ": write a cord to or from another of its ports on a line of its own",
            });
        }
        if failure.is_none() && chain.len() == 1 && !matches!(chain[0].0, Ref::Box(_)) {
            failure = Some(Failure {
                reason: "a name or a control alone makes nothing".into(),
                at: line,
                hint: " (a line is a box, name = [object], or cords, a -> b)",
            });
        }
        if let Some(Failure { reason, at, hint }) = failure {
            self.found.wrong.truncate(said.0);
            self.found.changed.truncate(said.1);
            self.found.notes.truncate(said.2);
            self.notation.boxes.truncate(first_box);
            self.names.retain(|_, (at, _)| *at < first_box);
            self.found.wrong.push(left_out(at, line, last, &reason, hint));
            return;
        }
        if chain.len() == 1 {
            if !names {
                self.found.notes.push(format!(
                    "line {line}: the box on this line has no name and no cords, so nothing reaches it (name = [object] names one; a -> b is a cord)."
                ));
            }
            return;
        }
        for hop in 0..end {
            let outlet = if hop == 0 { chain[0].1.unwrap_or(0) } else { 0 };
            let inlet = if hop + 1 == end { chain[end].1.unwrap_or(0) } else { 0 };
            self.hops.push((chain[hop].0.clone(), outlet, chain[hop + 1].0.clone(), inlet, line));
        }
    }
}

fn parse_from(text: &str, first_line: usize, top: bool, depth: usize) -> (Notation, Findings) {
    let (tokens, unread) = tokens(text, first_line);
    let mut reader = Reader {
        tokens: &tokens,
        at: 0,
        notation: Notation::default(),
        names: HashMap::new(),
        hops: Vec::new(),
        found: Findings::default(),
        depth,
    };
    reader.found.wrong.extend(unread);
    while reader.at < tokens.len() {
        if tokens[reader.at].0 == Token::End {
            reader.at += 1;
        } else {
            reader.statement();
        }
    }
    let Reader { mut notation, names, hops, mut found, .. } = reader;
    let end = |found_ref: Ref, line: usize, found: &mut Findings| -> Option<End> {
        match found_ref {
            Ref::Box(at) => Some(End::Box(at)),
            Ref::Control(name) => Some(End::Control(name)),
            Ref::Name(name) => match names.get(&name) {
                Some((at, _)) => Some(End::Box(*at)),
                None if OUTSIDE.contains(&name.as_str()) && top => Some(End::Outside(name)),
                None if OUTSIDE.contains(&name.as_str()) => {
                    found.wrong.push(format!("line {line}: {name} is the device's own, so the cord was left out: inside a [p], take what comes in from an [inlet] and send it on through an [outlet]."));
                    None
                }
                None => {
                    found.wrong.push(format!("line {line}: no box is named {name} (name = [object] makes one), so the cord was left out."));
                    None
                }
            },
        }
    };
    for (from, outlet, to, inlet, line) in hops {
        // A port past the most a box gets is left out with its cord, before anything is made for it.
        if let Some((which, port)) = [("outlet", outlet), ("inlet", inlet)].into_iter().find(|(_, port)| *port >= MOST_PORTS) {
            found.wrong.push(format!(
                "line {line}: a cord at {which} {port} is past the {MOST_PORTS} inlets and outlets Kumi gives a box, so Kumi left it out."
            ));
            continue;
        }
        if let (Some(from), Some(to)) = (end(from, line, &mut found), end(to, line, &mut found)) {
            notation.cords.push(Link { from, outlet, to, inlet, line });
        }
    }
    if notation.boxes.len() > MOST_BOXES {
        let from = notation.boxes[MOST_BOXES].line;
        notation.boxes.truncate(MOST_BOXES);
        notation.cords.retain(|link| [&link.from, &link.to].iter().all(|end| !matches!(end, End::Box(at) if *at >= MOST_BOXES)));
        found.wrong.push(format!(
            "line {from}: a patcher holds {MOST_BOXES} boxes at most, so Kumi left out the boxes from this line on, and their cords: put some of them in a [p]."
        ));
    }
    found.by_line();
    (notation, found)
}

/// What a patch's names and controls reach outside it, by box id: the device's in and out, its controls.
#[derive(Debug, Default)]
pub struct Outside<'a> {
    pub boxes: HashMap<String, String>,
    pub controls: BTreeMap<String, String>,
    /// What Kumi learned of the installed Max, for each object's inlets and outlets.
    pub reference: Option<&'a Reference>,
    /// Whether the device's sound goes out through Kumi's output stage (an audio effect's, an instrument's): a patch's
    /// own plugout~ or dac~ is made as out, or inside a [p], where there's no out, left out.
    pub output_stage: bool,
}

/// The version of Max Kumi writes patchers for.
fn appversion() -> Value {
    json!({ "major": 9, "minor": 1, "revision": 5, "architecture": "x64", "modernui": 1 })
}

/// A patch's boxes and cords as Max writes them (`{ "box": … }`, `{ "patchline": … }`), each cord with the line it's
/// written on, and what was found making them.
#[derive(Debug, Default)]
pub struct Expanded {
    pub boxes: Vec<Value>,
    pub lines: Vec<(Value, usize)>,
    pub found: Findings,
}

/// A patch's boxes and cords as Max writes them. What can't be made the way it's written is made the nearest way it can
/// be, and said: a control that isn't one or a cord to a port a box hasn't got is left out, as Max would leave it.
pub fn expand(notation: &Notation, outside: &Outside) -> Expanded {
    expand_within(notation, outside, true)
}

/// A cord left out because a box hasn't the port it uses, said with its line; code that gives its own ports says how
/// to give it more.
fn missing_port(line: usize, label: &str, which: &str, port: usize, has: usize, code: bool) -> String {
    let side = if which == "outlet" { "leaves" } else { "enters" };
    let more = if code { format!(": set {which}s = {}; in its code to give it more", port + 1) } else { String::new() };
    format!(
        "line {line}: a cord {side} {which} {port} of {label}, which has {}, so Kumi left it out, as Max would{more}.",
        ports_had(which, has)
    )
}

/// A box's inlets or outlets as they're numbered, from 0: "only outlet 0", "outlets 0 to 3", "no outlets".
pub(crate) fn ports_had(which: &str, has: usize) -> String {
    match has {
        0 => format!("no {which}s"),
        1 => format!("only {which} 0"),
        _ => format!("{which}s 0 to {}", has - 1),
    }
}

fn expand_within(notation: &Notation, outside: &Outside, top: bool) -> Expanded {
    let mut found = Findings::default();
    let id = |at: usize| format!("obj-{}", at + 1);
    // How many of each box's ports the cords use: an object Max's reference doesn't count has that many.
    let (mut inlets_used, mut outlets_used) = (vec![0usize; notation.boxes.len()], vec![0usize; notation.boxes.len()]);
    for link in &notation.cords {
        if let End::Box(at) = link.from {
            outlets_used[at] = outlets_used[at].max(link.outlet + 1);
        }
        if let End::Box(at) = link.to {
            inlets_used[at] = inlets_used[at].max(link.inlet + 1);
        }
    }
    let (mut inlets, mut outlets) = (0usize, 0usize);
    let mut boxes: Vec<Value> = Vec::new();
    // Each box's inlets and outlets, whether its code gives them, and how it's written; None for one left out.
    let mut made: Vec<Option<(usize, usize, bool, String)>> = vec![None; notation.boxes.len()];
    // The patch's own ways out made as out.
    let mut speakers: HashSet<usize> = HashSet::new();
    for (at, item) in notation.boxes.iter().enumerate() {
        let class = item.what.class();
        let label = match &item.what {
            What::Message(text) => format!("({text})"),
            What::Object(text) | What::Gen { text, .. } | What::Js { text, .. } | What::Patcher { text, .. } => format!("[{text}]"),
        };
        if outside.output_stage && matches!(item.what, What::Object(_)) && SPEAKERS.contains(&class.as_str()) {
            if top {
                found.changed.push(format!(
                    "line {}: {label} was made as out: Kumi's output stage, with its safety net, is the device's only way out.",
                    item.line
                ));
                speakers.insert(at);
            } else {
                found.wrong.push(format!(
                    "line {}: {label} in a [p] would send sound past Kumi's output stage, the device's only way out, so Kumi left it out: send the sound out of the [p] through an [outlet], then to out.",
                    item.line
                ));
            }
            continue;
        }
        let mut fields = match &item.what {
            What::Object(_) if matches!(class.as_str(), "inlet" | "outlet") && top => {
                found.wrong.push(format!(
                    "line {}: [{class}] belongs in a [p] {{ … }}, so Kumi left it and its cords out: the device's own patch takes what it receives from in and sends it to out.",
                    item.line
                ));
                continue;
            }
            What::Object(_) if matches!(class.as_str(), "inlet" | "outlet") => {
                // Max numbers inlet and outlet objects by where they sit: they're placed in the order they're written.
                let inlet = class == "inlet";
                let order = if inlet {
                    inlets += 1;
                    inlets
                } else {
                    outlets += 1;
                    outlets
                };
                json!({ "maxclass": class, "index": order, "comment": "", "numinlets": usize::from(!inlet), "numoutlets": usize::from(inlet),
                    "outlettype": if inlet { vec![""] } else { vec![] }, "patching_rect": [40.0 + (order - 1) as f64 * 80.0, 20.0, 30.0, 30.0] })
            }
            What::Object(text) => {
                // A building block that couldn't be made was said as the patch was read.
                let kit = class.starts_with("kumi.");
                if let Some(reference) = outside.reference.filter(|reference| !kit && reference.object(&class).is_none()) {
                    let near = super::page::nearest(reference, &class);
                    let near = if near.is_empty() {
                        String::new()
                    } else {
                        format!(" Nearest: {}.", near.iter().map(|name| format!("[{name}]")).collect::<Vec<_>>().join(", "))
                    };
                    found.wrong.push(format!(
                        "line {}: [{class}] isn't an object in this Max, so Max will show {label} as a broken box: check its name (make_device with objects can look one up).{near}",
                        item.line
                    ));
                }
                let (ins, outs, types) = match outside.reference.and_then(|reference| reference.ports(text)) {
                    Some(ports) => (ports.inlets, ports.outlets, ports.outlet_types),
                    // One the reference doesn't count (Max isn't here, or its ports come from what's in it): the ports the
                    // cords use.
                    None => (inlets_used[at].max(1), outlets_used[at], vec![String::new(); outlets_used[at]]),
                };
                json!({ "maxclass": "newobj", "text": text, "numinlets": ins, "numoutlets": outs, "outlettype": types })
            }
            What::Message(text) => json!({ "maxclass": "message", "text": text, "numinlets": 2, "numoutlets": 1, "outlettype": [""] }),
            What::Gen { text, code } => {
                let (ins, outs) = (inputs_read(code), outputs_written(code).max(1));
                let kind = if class == "gen~" { "signal" } else { "" };
                json!({ "maxclass": "newobj", "text": text, "numinlets": ins.max(1), "numoutlets": outs, "outlettype": vec![kind; outs],
                    "patcher": gen_patcher(code, ins, outs) })
            }
            What::Js { text, code } => {
                if class == "v8ui" && !top {
                    found.changed.push(format!(
                        "line {}: a [v8ui] in a [p] draws there, when the [p] is opened, not on the device's face: to show it on the face, put it in the device's own patch.",
                        item.line
                    ));
                }
                if let Some((line, message)) = syntax_problem(code) {
                    found.wrong.push(format!(
                        "line {}: [{text}]'s JavaScript doesn't parse: {message}. Kumi made the box with the code as written; Max will say so in its console.",
                        item.line + line - 1
                    ));
                }
                let (ins, outs) = declared_ports(code);
                let (ins, outs) = (ins.unwrap_or(1), outs.unwrap_or(1));
                if ins.max(outs) > MOST_PORTS {
                    found.wrong.push(format!(
                        "line {}: [{text}]'s code sets more than {MOST_PORTS} inlets or outlets, so Kumi made it {MOST_PORTS}.",
                        item.line
                    ));
                }
                let (ins, outs) = (ins.min(MOST_PORTS), outs.min(MOST_PORTS));
                // As Max saves embedded code: the box opens it, and no file is needed.
                let embedded =
                    |file: &str, flags: u8| json!({ "text": code, "filename": file, "flags": flags, "embed": 1, "autowatch": 1 });
                if class == "v8ui" {
                    json!({ "maxclass": "v8ui", "filename": "none", "numinlets": ins, "numoutlets": outs, "outlettype": vec![""; outs],
                        "parameter_enable": 0, "textfile": embedded("none", 0), "patching_rect": [40.0, 40.0 + at as f64 * 30.0, 120.0, 60.0] })
                } else {
                    let name = atoms(text).into_iter().nth(1).filter(|word| !word.starts_with('@'));
                    let file = name.map_or("none".to_string(), |name| if name.ends_with(".js") { name } else { format!("{name}.js") });
                    let text = if text.contains("@embed") { text.clone() } else { format!("{text} @embed 1") };
                    json!({ "maxclass": "newobj", "text": text, "filename": file, "numinlets": ins, "numoutlets": outs,
                        "outlettype": vec![""; outs], "saved_object_attributes": { "parameter_enable": 0 }, "textfile": embedded(&file, 1) })
                }
            }
            What::Patcher { text, inner } => {
                let within = Outside {
                    boxes: HashMap::new(),
                    controls: BTreeMap::new(),
                    reference: outside.reference,
                    output_stage: outside.output_stage,
                };
                let expanded = expand_within(inner, &within, false);
                found.append(expanded.found);
                let count = |which: &str| inner.boxes.iter().filter(|item| item.what.class() == which).count();
                let (ins, outs) = (count("inlet"), count("outlet"));
                let lines: Vec<Value> = expanded.lines.into_iter().map(|(line, _)| line).collect();
                json!({ "maxclass": "newobj", "text": text, "numinlets": ins, "numoutlets": outs, "outlettype": vec![""; outs],
                    "patcher": { "fileversion": 1, "appversion": appversion(), "classnamespace": "box", "rect": [100.0, 100.0, 640.0, 480.0],
                        "gridsize": [8.0, 8.0], "boxes": expanded.boxes, "lines": lines } })
            }
        };
        let count = |key: &str| fields[key].as_u64().unwrap_or(0) as usize;
        made[at] = Some((count("numinlets"), count("numoutlets"), matches!(item.what, What::Js { .. }), label));
        fields["id"] = json!(id(at));
        // Its name is its scripting name too, unless a control on the face has it.
        if let Some(name) = item.name.as_ref().filter(|name| !outside.controls.contains_key(*name)) {
            fields["varname"] = json!(name);
        }
        if fields.get("patching_rect").is_none() {
            fields["patching_rect"] = json!([40.0, 40.0 + at as f64 * 30.0, 60.0, 22.0]);
        }
        boxes.push(json!({ "box": fields }));
    }
    let controls = || outside.controls.keys().map(|name| format!("\"{name}\"")).collect::<Vec<_>>().join(", ");
    let mut lines: Vec<(Value, usize)> = Vec::new();
    for link in &notation.cords {
        // A cord into a box made as out goes to out, on the side its inlet is (an MC box's one inlet, the left).
        let remade;
        let link = match &link.to {
            End::Box(at) if speakers.contains(at) => {
                let What::Object(text) = &notation.boxes[*at].what else { continue };
                let inlet = if text.starts_with("mc.") { 0 } else { link.inlet };
                if inlet >= 2 {
                    found.wrong.push(format!(
                        "line {}: a cord enters inlet {} of [{text}], made as out, which has only out.0 and out.1, so Kumi left it out.",
                        link.line, link.inlet
                    ));
                    continue;
                }
                remade = Link { to: End::Outside("out".into()), inlet, ..link.clone() };
                &remade
            }
            _ => link,
        };
        // A cord to a box that was left out goes with it (that box's line says why).
        let gone = |end: &End| matches!(end, End::Box(at) if made[*at].is_none());
        if gone(&link.from) || gone(&link.to) {
            continue;
        }
        // Each end that's a box of the patch has the port the cord uses, or the cord is left out (Kumi's own boxes, the
        // device's in and out and its controls, are seen to where the patch meets them).
        if let End::Box(from) = &link.from {
            if let Some((_, outs, code, label)) = made[*from].as_ref().filter(|(_, outs, _, _)| link.outlet >= *outs) {
                found.wrong.push(missing_port(link.line, label, "outlet", link.outlet, *outs, *code));
                continue;
            }
        }
        if let End::Box(to) = &link.to {
            if let Some((ins, _, code, label)) = made[*to].as_ref().filter(|(ins, _, _, _)| link.inlet >= *ins) {
                found.wrong.push(missing_port(link.line, label, "inlet", link.inlet, *ins, *code));
                continue;
            }
        }
        let mut end = |end: &End| -> Option<String> {
            match end {
                End::Box(at) => Some(id(*at)),
                End::Control(name) => match outside.controls.get(name) {
                    Some(found_id) => Some(found_id.clone()),
                    None if !top => {
                        found.wrong.push(format!(
                            "line {}: \"{name}\" is a control on the device's face, so the cord was left out: bring its value into the [p] through an [inlet].",
                            link.line
                        ));
                        None
                    }
                    None if outside.controls.is_empty() => {
                        found.wrong.push(format!(
                            "line {}: \"{name}\" isn't a control (the device has none: controls makes them), so the cord was left out.",
                            link.line
                        ));
                        None
                    }
                    None => {
                        found.wrong.push(format!(
                            "line {}: \"{name}\" isn't a control (the device's are {}), so the cord was left out.",
                            link.line,
                            controls()
                        ));
                        None
                    }
                },
                End::Outside(name) => match outside.boxes.get(name) {
                    Some(found_id) => Some(found_id.clone()),
                    None => {
                        found.wrong.push(format!("line {}: this device has no {name}, so the cord was left out.", link.line));
                        None
                    }
                },
            }
        };
        if let (Some(from), Some(to)) = (end(&link.from), end(&link.to)) {
            lines.push((json!({ "patchline": { "source": [from, link.outlet], "destination": [to, link.inlet] } }), link.line));
        }
    }
    Expanded { boxes, lines, found }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::patch::reference::{Count, Object};

    /// An object Max's help shows bare and with up to two arguments, its counts the same each time.
    fn object(inlets: usize, outlets: usize) -> Object {
        let (inlets, outlets) = (Count::Fixed { count: inlets }, Count::Fixed { count: outlets });
        Object { inlets, outlets, seen: 3, held: vec![0, 1, 2], ..Object::default() }
    }

    #[test]
    fn boxes_cords_ports_and_chains_are_read_with_their_lines() {
        let notation = parse(concat!(
            "# a clock\n",
            "clock = [metro 125]\n",
            "step = [counter 0 15]   // its count\n",
            "clock -> step -> [sel 0] -> accent\n",
            "step.2 -> [print carry]\n",
            "\"Rate\" -> [prepend interval] -> clock\n",
            "accent = (bang)\n",
            "accent -> out\n",
        ));
        let (notation, found) = notation;
        assert!(found.is_empty(), "{found:?}");
        let texts: Vec<String> = notation
            .boxes
            .iter()
            .map(|item| match &item.what {
                What::Object(text) => format!("[{text}]"),
                What::Message(text) => format!("({text})"),
                _ => String::new(),
            })
            .collect();
        assert_eq!(texts, ["[metro 125]", "[counter 0 15]", "[sel 0]", "[print carry]", "[prepend interval]", "(bang)"]);
        assert_eq!(notation.boxes[5].name.as_deref(), Some("accent"), "named where it's defined, after its first use");
        let cords: Vec<(End, usize, End, usize, usize)> =
            notation.cords.iter().map(|link| (link.from.clone(), link.outlet, link.to.clone(), link.inlet, link.line)).collect();
        assert_eq!(
            cords,
            [
                (End::Box(0), 0, End::Box(1), 0, 4),
                (End::Box(1), 0, End::Box(2), 0, 4),
                (End::Box(2), 0, End::Box(5), 0, 4),
                (End::Box(1), 2, End::Box(3), 0, 5),
                (End::Control("Rate".into()), 0, End::Box(4), 0, 6),
                (End::Box(4), 0, End::Box(0), 0, 6),
                (End::Box(5), 0, End::Outside("out".into()), 0, 8),
            ]
        );
    }

    #[test]
    fn a_gen_holds_its_code_and_a_subpatcher_its_patch() {
        let notation = parse(concat!(
            "fx = [gen~] {\n",
            "  // braces in the code: { }\n",
            "  f(x) { return x * 0.5; }\n",
            "  out1 = f(in1); out2 = in2;\n",
            "}\n",
            "in.1 -> fx.1\n",
            "voice = [p voice] {\n",
            "  [inlet] -> [* 2] -> [outlet]\n",
            "  nope -> [outlet]\n",
            "}\n",
        ));
        let (notation, found) = notation;
        assert_eq!(
            found.wrong,
            ["line 9: no box is named nope (name = [object] makes one), so the cord was left out."],
            "a line inside a block counts in the whole"
        );
        assert!(
            matches!(&notation.boxes[1].what, What::Patcher { inner, .. } if inner.boxes.len() == 4 && inner.cords.len() == 2),
            "the rest is made"
        );
        let (notation, found) =
            parse("fx = [gen~] { out1 = in1; }\nvoice = [p voice] {\n  [inlet] -> [outlet]\n}\nin -> fx -> voice -> out\n");
        assert!(found.is_empty());
        assert!(matches!(&notation.boxes[0].what, What::Gen { code, .. } if code.trim() == "out1 = in1;"));
        assert!(matches!(&notation.boxes[1].what, What::Patcher { inner, .. } if inner.boxes.len() == 2 && inner.cords.len() == 1));
    }

    #[test]
    fn what_cant_be_read_is_left_out_and_said_with_its_line_and_the_rest_is_made() {
        let (notation, found) = parse("a = [metro 100\nb = [counter]\n");
        assert_eq!(found.wrong, ["line 1: a [ isn't closed on its line, so Kumi left the line out."]);
        assert_eq!(
            notation.boxes.iter().map(|item| item.name.clone()).collect::<Vec<_>>(),
            [Some("b".to_string())],
            "the next line is read"
        );
        let (notation, found) = parse(concat!(
            "a = [metro 100]\n",
            "a = [counter]\n",
            "a -> b.1 -> c\n",
            "[loadbang]\n",
            "fx = [gen~]\n",
            "out = [dac~]\n",
            "x = [print] { y }\n",
            "z = [sel 0] -> q ?\n",
        ));
        assert_eq!(
            found.wrong,
            [
                "line 2: a is named twice (first on line 1), so this box was made without the name, and cords by it reach the first.",
                "line 3: a box in the middle of a chain is entered at inlet 0 and left by outlet 0, so Kumi left the line out: write a cord to or from another of its ports on a line of its own.",
                "line 5: [gen~] holds GenExpr, and it was given none, so it's empty: write it in a { … } block after the box.",
                "line 6: out is the device's own (what it receives and sends), so the box was made without that name: name it something else.",
                "line 7: only [gen~], [gen], [v8], [v8ui] and [p] hold a { … } block, so [print] was made without what it was given: put that in one of them.",
                "line 8: \"?\" isn't part of the notation (a box is [object] or (message), a cord is ->), so Kumi left the line out.",
            ]
        );
        assert!(found.changed.is_empty(), "{found:?}");
        assert_eq!(
            found.notes,
            ["line 4: the box on this line has no name and no cords, so nothing reaches it (name = [object] names one; a -> b is a cord)."]
        );
        let texts: Vec<String> = notation.boxes.iter().map(|item| item.what.class()).collect();
        assert_eq!(texts, ["metro", "counter", "loadbang", "gen~", "dac~", "print"], "every box that could be made is");
        let (notation, found) = parse("old = [js] { function bang() {} }\nsteps = [v8]\n");
        assert_eq!(found.changed, ["line 1: [js] { … } was made as [v8] { … }: Max's modern engine, its code embedded in the device."]);
        assert_eq!(
            found.wrong,
            ["line 2: [v8] holds JavaScript, and it was given none, so it's empty: write it in a { … } block after the box."]
        );
        assert!(matches!(&notation.boxes[0].what, What::Js { text, code } if text == "v8" && code.contains("bang")));
    }

    /// The line each box of a patch is on, by its name.
    fn lines_of(notation: &Notation) -> Vec<(String, usize)> {
        notation.boxes.iter().map(|item| (item.name.clone().unwrap_or_default(), item.line)).collect()
    }

    #[test]
    fn a_block_is_read_as_what_it_holds_for_where_it_ends_and_the_lines_after_it_count_on() {
        // JavaScript: a # is a private member's, a quote in a template literal or a regular expression is only text,
        // and a ${…} is code with braces of its own.
        let (notation, found) = parse(concat!(
            "a = [v8] {\n",
            "  class Step {\n",
            "    #n = 0;\n",
            "    next() { if (this.#n > 3) { this.#n = 0; } return this.#n++; }\n",
            "  }\n",
            "  const s = `don't ${ {a: 1}.a } {`;\n",
            "  const r = /don't[/}]/g;\n",
            "  function f(n) { if (n) { post(`it's`); } return n / 2; }\n",
            "  function g(n) { if (n-- / 2 > 0) { n = n / 2; } return n++ / 2; }\n",
            "  const open = 'not closed\n",
            "}\n",
            "b = [metro 100]\n",
        ));
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(lines_of(&notation), [("a".to_string(), 1), ("b".to_string(), 12)]);
        assert!(
            matches!(&notation.boxes[0].what, What::Js { code, .. } if code.contains("return this.#n++;") && code.contains("'not closed"))
        );
        // GenExpr: only comments; a patch: # and // comments, and a box's text or a control's name on its line, with a
        // block inside it read as what that holds.
        let (notation, found) = parse(concat!(
            "fx = [gen~] {\n",
            "  // it's \"fine\" {\n",
            "  out1 = in1; out2 = in2;\n",
            "}\n",
            "v = [p v] {\n",
            "  [inlet] -> [print don't] -> [outlet]\n",
            "  (it's) -> [outlet]\n",
            "  # a } in a comment\n",
            "  s = [v8] { const t = \"}\"; }\n",
            "  [inlet] -> s\n",
            "}\n",
            "b = [metro 100]\n",
        ));
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(lines_of(&notation), [("fx".to_string(), 1), ("v".to_string(), 5), ("b".to_string(), 12)]);
        assert!(matches!(&notation.boxes[1].what, What::Patcher { inner, .. } if inner.boxes.len() == 7), "{:?}", notation.boxes[1]);
        // A backslash at a line's end escapes nothing: the line after it is its own.
        let (notation, found) = parse("a = [print C:\\\nb = [metro 100]\n");
        assert_eq!(found.wrong, ["line 1: a [ isn't closed on its line, so Kumi left the line out."]);
        assert_eq!(lines_of(&notation), [("b".to_string(), 2)]);
    }

    #[test]
    fn a_statement_left_out_names_every_line_it_covers_and_what_was_said_reading_it_goes_with_it() {
        // A { that isn't closed holds the rest of the patch: none of its code is made into boxes.
        let (notation, found) = parse("a = [v8] {\nnotes = [60, 64, 67]\nfunction bang() {}\n");
        assert_eq!(
            found.wrong,
            ["line 1: a { isn't closed, so Kumi took the rest of the patch (to line 3) as its block: close it with } where it ends."]
        );
        assert!(matches!(&notation.boxes[..], [Item { what: What::Js { code, .. }, .. }] if code.contains("notes = [60, 64, 67]")));
        // A statement a block carries over several lines is left out whole, and said with its lines.
        let (notation, found) = parse("a = [v8] {\n  function bang() {}\n} ?\nb = [metro 100]\n");
        assert_eq!(
            found.wrong,
            ["line 3: \"?\" isn't part of the notation (a box is [object] or (message), a cord is ->), so Kumi left out lines 1–3, the statement it's part of."]
        );
        assert_eq!(lines_of(&notation), [("b".to_string(), 4)]);
        let (_, found) = parse("a = [v8] {\n  function bang() {}\n} =\n");
        assert_eq!(
            found.wrong,
            ["line 3: expected -> or the end of the line after a box, not =, so Kumi left out lines 1–3, the statement it's part of."]
        );
        // What a statement said before it was left out goes with it.
        let (notation, found) = parse("fx = [gen~] -> a.1 -> out\n");
        assert_eq!(
            found.wrong,
            ["line 1: a box in the middle of a chain is entered at inlet 0 and left by outlet 0, so Kumi left the line out: write a cord to or from another of its ports on a line of its own."]
        );
        assert!(notation.boxes.is_empty());
        // A port past the most a box gets is left out with its cord, before anything is made for it.
        let (notation, found) = parse("x = [foo]\nx.4000000000 -> out\n");
        assert_eq!(
            found.wrong,
            ["line 2: a cord at outlet 4000000000 is past the 512 inlets and outlets Kumi gives a box, so Kumi left it out."]
        );
        assert!(notation.cords.is_empty());
        let wide = parse("w = [v8] { outlets = 4000000000; }\nw -> out\n").0;
        let made = expand(&wide, &Outside { boxes: HashMap::from([("out".to_string(), "obj-out".to_string())]), ..Outside::default() });
        assert_eq!(made.found.wrong, ["line 1: [v8]'s code sets more than 512 inlets or outlets, so Kumi made it 512."]);
        assert_eq!(made.boxes[0]["box"]["numoutlets"], json!(512));
        let many = parse("g = [gen~] { out4000000000 = 0; out1 = 0; }\n").0;
        assert_eq!(expand(&many, &Outside::default()).boxes[0]["box"]["numoutlets"], json!(1), "gen~'s outN goes to 99 at most");
        // A patcher holds so many boxes at most: those after are left out, with their cords, and said once.
        let many = format!(
            "{}\n{}\n",
            (0..MOST_BOXES + 5).map(|at| format!("b{at} = [t b]")).collect::<Vec<_>>().join("\n"),
            "b0 -> b2004\nb0 -> b1"
        );
        let (notation, found) = parse(&many);
        assert_eq!(notation.boxes.len(), MOST_BOXES);
        assert_eq!(
            notation.cords.iter().map(|link| link.to.clone()).collect::<Vec<_>>(),
            [End::Box(1)],
            "the cord to a box left out goes with it"
        );
        assert_eq!(
            found.wrong,
            ["line 2001: a patcher holds 2000 boxes at most, so Kumi left out the boxes from this line on, and their cords: put some of them in a [p]."]
        );
        // Subpatchers nest 16 deep at most.
        let deep = format!("{}{}", "[p x] {\n".repeat(20), "}\n".repeat(20));
        assert!(parse(&deep).1.wrong.iter().any(|said| said.contains("is inside 16 subpatchers, so Kumi made it empty")));
    }

    #[test]
    fn javascript_is_embedded_in_its_box_with_the_ports_its_code_gives() {
        let notation = parse(concat!(
            "steps = [v8 steps] {\n",
            "  inlets = 2;\n",
            "  outlets = 2;\n",
            "  function bang() { outlet(0, [1, 0, 1].map((x) => x * 127)); }\n",
            "}\n",
            "view = [v8ui] {\n",
            "  mgraphics.init();\n",
            "  function paint() { mgraphics.rectangle(0, 0, 10, 10); mgraphics.fill(); }\n",
            "}\n",
            "steps.1 -> view\n",
            "steps -> out\n",
        ));
        let (notation, found) = notation;
        assert!(found.is_empty());
        let outside = Outside { boxes: HashMap::from([("out".to_string(), "obj-out".to_string())]), ..Outside::default() };
        let Expanded { boxes, found, .. } = expand(&notation, &outside);
        assert!(found.is_empty(), "{found:?}");
        let steps = &boxes[0]["box"];
        assert_eq!(
            (steps["text"].clone(), steps["numinlets"].clone(), steps["numoutlets"].clone()),
            (json!("v8 steps @embed 1"), json!(2), json!(2))
        );
        assert_eq!((steps["textfile"]["embed"].clone(), steps["textfile"]["filename"].clone()), (json!(1), json!("steps.js")));
        assert!(steps["textfile"]["text"].as_str().unwrap().contains("function bang()"), "its code, in the device: no file");
        let view = &boxes[1]["box"];
        assert_eq!((view["maxclass"].clone(), view["numinlets"].clone(), view["numoutlets"].clone()), (json!("v8ui"), json!(1), json!(1)));

        let wrong = parse(concat!(
            "a = [v8] {\n",
            "  function bang() {\n",
            "    outlet(0, 1;\n",
            "  }\n",
            "}\n",
            "b = [v8] { function bang() { outlet(1, 1); } }\n",
            "b.1 -> out\n",
            "p1 = [p inner] {\n",
            "  [inlet] -> [v8ui] { function paint() {} }\n",
            "}\n",
        ))
        .0;
        let made = expand(&wrong, &outside);
        assert_eq!(
            made.found.wrong,
            [
                "line 3: [v8]'s JavaScript doesn't parse: Unexpected token ';'. Kumi made the box with the code as written; Max will say so in its console.",
                "line 7: a cord leaves outlet 1 of [v8], which has only outlet 0, so Kumi left it out, as Max would: set outlets = 2; in its code to give it more.",
            ]
        );
        assert_eq!(
            made.found.changed,
            ["line 9: a [v8ui] in a [p] draws there, when the [p] is opened, not on the device's face: to show it on the face, put it in the device's own patch."]
        );
        assert!(made.boxes[0]["box"]["textfile"]["text"].as_str().unwrap().contains("outlet(0, 1;"), "the code as written");
        assert!(made.lines.is_empty(), "the cord to an outlet [v8] hasn't got is left out");
    }

    #[test]
    fn a_patch_expands_to_maxs_boxes_with_the_ports_max_gives_them() {
        let mut reference = Reference::default();
        reference.objects.insert("metro".into(), object(2, 1));
        reference.objects.insert("counter".into(), object(5, 4));
        reference.objects.insert("print".into(), object(1, 0));
        let notation = parse(concat!(
            "clock = [metro 125]\n",
            "\"Rate\" -> clock.1\n",
            "clock -> [counter 0 7] -> out\n",
            "clock -> [mystery] -> out\n",
            "voice = [p voice] {\n",
            "  [inlet] -> [outlet]\n",
            "  [inlet] -> [outlet]\n",
            "}\n",
            "clock -> voice.1\n",
        ))
        .0;
        let outside = Outside {
            boxes: HashMap::from([("out".to_string(), "obj-out".to_string())]),
            controls: BTreeMap::from([("Rate".to_string(), "obj-control-1".to_string())]),
            reference: Some(&reference),
            output_stage: false,
        };
        let Expanded { boxes, lines, found } = expand(&notation, &outside);
        assert_eq!(
            found.wrong,
            ["line 4: [mystery] isn't an object in this Max, so Max will show [mystery] as a broken box: check its name (make_device with objects can look one up)."]
        );
        let field = |at: usize, key: &str| boxes[at]["box"][key].clone();
        assert_eq!((field(0, "numinlets"), field(0, "numoutlets"), field(0, "varname")), (json!(2), json!(1), json!("clock")));
        assert_eq!((field(1, "numinlets"), field(1, "numoutlets")), (json!(5), json!(4)), "from Max's reference");
        assert_eq!((field(2, "numinlets"), field(2, "numoutlets")), (json!(1), json!(1)), "one Max doesn't count: as the cords use it");
        assert_eq!((field(3, "numinlets"), field(3, "numoutlets")), (json!(2), json!(2)), "a subpatcher's are its inlets and outlets");
        let inner = &boxes[3]["box"]["patcher"]["boxes"];
        let ports: Vec<(Value, Value)> =
            (0..4).map(|at| (inner[at]["box"]["maxclass"].clone(), inner[at]["box"]["index"].clone())).collect();
        assert_eq!(
            ports,
            [(json!("inlet"), json!(1)), (json!("outlet"), json!(1)), (json!("inlet"), json!(2)), (json!("outlet"), json!(2))]
        );
        assert!(inner[0]["box"]["patching_rect"][0].as_f64() < inner[2]["box"]["patching_rect"][0].as_f64(), "in the order written");
        let cords: Vec<Value> =
            lines.iter().map(|(line, _)| json!([line["patchline"]["source"], line["patchline"]["destination"]])).collect();
        assert!(cords.contains(&json!([["obj-control-1", 0], ["obj-1", 1]])) && cords.contains(&json!([["obj-2", 0], ["obj-out", 0]])));
        assert!(cords.contains(&json!([["obj-1", 0], ["obj-4", 1]])));

        let wrong = parse("\"Rote\" -> [print]\nin -> [print]\n").0;
        assert_eq!(
            expand(&wrong, &outside).found.wrong,
            [
                "line 1: \"Rote\" isn't a control (the device's are \"Rate\"), so the cord was left out.",
                "line 2: this device has no in, so the cord was left out."
            ]
        );
        let inside = parse("v = [p v] {\n  \"Rate\" -> [outlet]\n}\n").0;
        assert_eq!(
            expand(&inside, &outside).found.wrong,
            ["line 2: \"Rate\" is a control on the device's face, so the cord was left out: bring its value into the [p] through an [inlet]."]
        );
        // An object Max doesn't have is made with the ports its cords use, and its cords are kept, as Max keeps them on a
        // broken box.
        let unknown = expand(&parse("m = [mystery]\nin -> m.1\nm.2 -> out\n").0, &outside);
        assert_eq!((unknown.boxes[0]["box"]["numinlets"].clone(), unknown.boxes[0]["box"]["numoutlets"].clone()), (json!(2), json!(3)));
        assert_eq!(unknown.lines.len(), 1, "the cord from in is left out (this outside has no in); the one to out is kept");
        let loose = expand(&parse("[inlet] -> out\n").0, &outside);
        assert_eq!(
            loose.found.wrong,
            ["line 1: [inlet] belongs in a [p] { … }, so Kumi left it and its cords out: the device's own patch takes what it receives from in and sends it to out."]
        );
        assert!(loose.boxes.is_empty() && loose.lines.is_empty());
    }
}
