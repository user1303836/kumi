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
//! voice = [p voice] { … }                  # a subpatcher holds a patch; its [inlet]s and [outlet]s number from the
//!                                          # left, as they're written
//! ```

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use super::reference::{atoms, Reference};
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
    /// A subpatcher (`[p name]`) and the patch in it.
    Patcher { text: String, inner: Notation },
}

impl What {
    /// Its object's class: "metro" for [metro 125], "message" for a message box.
    pub fn class(&self) -> String {
        match self {
            What::Object(text) | What::Gen { text, .. } | What::Patcher { text, .. } => atoms(text).into_iter().next().unwrap_or_default(),
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

/// The names that are the device's own: what it receives and what it sends.
pub const OUTSIDE: [&str; 2] = ["in", "out"];

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

/// Where a run of text closes, from `at`: the first `close` outside quotes and nested `nests`, on the same line.
fn closing(chars: &[char], mut at: usize, close: char, nests: Option<char>) -> Option<usize> {
    let (mut quoted, mut depth) = (false, 0usize);
    while at < chars.len() && chars[at] != '\n' {
        let c = chars[at];
        if c == '\\' {
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

/// Where a block's closing brace is, from just inside it, and how many lines it passes; comments and quotes skipped.
fn block_end(chars: &[char], mut at: usize) -> Option<(usize, usize)> {
    let (mut depth, mut lines) = (0usize, 0usize);
    let to_line_end = |at: &mut usize| {
        while *at + 1 < chars.len() && chars[*at + 1] != '\n' {
            *at += 1;
        }
    };
    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        match c {
            '\n' => lines += 1,
            '"' | '\'' => {
                at += 1;
                while at < chars.len() && chars[at] != c && chars[at] != '\n' {
                    at += if chars[at] == '\\' { 2 } else { 1 };
                }
            }
            '#' => to_line_end(&mut at),
            '/' if next == Some('/') => to_line_end(&mut at),
            '/' if next == Some('*') => {
                at += 2;
                while at + 1 < chars.len() && !(chars[at] == '*' && chars[at + 1] == '/') {
                    lines += usize::from(chars[at] == '\n');
                    at += 1;
                }
                at += 1;
            }
            '{' => depth += 1,
            '}' if depth == 0 => return Some((at, lines)),
            '}' => depth -= 1,
            _ => {}
        }
        at += 1;
    }
    None
}

/// The notation's tokens, each with its line.
fn tokens(text: &str, first_line: usize) -> Result<Vec<(Token, usize)>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
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
            '[' => {
                let end = closing(&chars, at + 1, ']', None).ok_or(format!("line {line}: a [ isn't closed on its line."))?;
                out.push((Token::Object(text_of(at + 1, end).trim().to_string()), line));
                at = end + 1;
            }
            '(' => {
                let end = closing(&chars, at + 1, ')', Some('(')).ok_or(format!("line {line}: a ( isn't closed on its line."))?;
                out.push((Token::Message(text_of(at + 1, end).trim().to_string()), line));
                at = end + 1;
            }
            '"' => {
                // A control's name has no quotes in it: it ends at the next one.
                let end = chars[at + 1..]
                    .iter()
                    .position(|c| *c == '"' || *c == '\n')
                    .map(|offset| at + 1 + offset)
                    .filter(|end| chars[*end] == '"')
                    .ok_or(format!("line {line}: a \" isn't closed on its line."))?;
                out.push((Token::Control(text_of(at + 1, end)), line));
                at = end + 1;
            }
            '{' => {
                let (end, lines) = block_end(&chars, at + 1).ok_or(format!("line {line}: a {{ isn't closed."))?;
                out.push((Token::Block(text_of(at + 1, end), line), line));
                line += lines;
                at = end + 1;
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
                out.push((Token::Port(digits.parse().map_err(|_| format!("line {line}: .{digits} isn't a port."))?), line));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let name: String = chars[at..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == '_').collect();
                at += name.len();
                out.push((Token::Name(name), line));
            }
            other => {
                return Err(format!("line {line}: \"{other}\" isn't part of the notation (a box is [object] or (message), a cord is ->)."))
            }
        }
    }
    out.push((Token::End, line));
    Ok(out)
}

/// The objects that hold what a block gives them: GenExpr for gen~ and gen, a patch for a subpatcher.
fn holds(class: &str) -> Option<&'static str> {
    match class {
        "gen~" | "gen" => Some("GenExpr"),
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

/// A patch written in the notation, or every problem with it, each with its line.
pub fn parse(text: &str) -> Result<Notation, Vec<String>> {
    parse_from(text, 1, true)
}

/// Reads a patch's lines: `first_line` is its first line in the whole notation; `top` is the device's own patch,
/// where `in` and `out` are the device's.
struct Reader<'a> {
    tokens: &'a [(Token, usize)],
    at: usize,
    notation: Notation,
    names: HashMap<String, (usize, usize)>,
    hops: Vec<(Ref, usize, Ref, usize, usize)>,
    problems: Vec<String>,
}

impl Reader<'_> {
    fn token(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(token, _)| token)
    }

    /// The box whose object or message starts here, with the block after it.
    fn boxed(&mut self, line: usize) -> Option<What> {
        let what = match self.token()?.clone() {
            Token::Object(text) => {
                let class = atoms(&text).into_iter().next().unwrap_or_default();
                if class.is_empty() {
                    self.problems.push(format!("line {line}: [] is empty: write the object in it, as it's typed in Max."));
                    return None;
                }
                self.at += 1;
                let block = match self.token() {
                    Some(Token::Block(block, from)) => Some((block.clone(), *from)),
                    _ => None,
                };
                match (block, holds(&class)) {
                    (Some((code, _)), Some("GenExpr")) => {
                        self.at += 1;
                        What::Gen { text, code }
                    }
                    (Some((inner, from)), Some(_)) => {
                        self.at += 1;
                        match parse_from(&inner, from, false) {
                            Ok(inner) => What::Patcher { text, inner },
                            Err(problems) => {
                                self.problems.extend(problems);
                                return None;
                            }
                        }
                    }
                    (Some(_), None) => {
                        self.problems.push(format!("line {line}: only [gen~], [gen] and [p] hold a {{ … }} block, not [{class}]."));
                        return None;
                    }
                    (None, Some(held)) => {
                        self.problems.push(format!("line {line}: [{text}] holds {held}: write it in a {{ … }} block after the box."));
                        return None;
                    }
                    (None, None) => What::Object(text),
                }
            }
            Token::Message(text) => {
                self.at += 1;
                if matches!(self.token(), Some(Token::Block(..))) {
                    self.problems.push(format!("line {line}: a message box doesn't hold a {{ … }} block."));
                    return None;
                }
                What::Message(text)
            }
            _ => return None,
        };
        Some(what)
    }

    /// A box for the patch, by name or not.
    fn add(&mut self, name: Option<String>, what: What, line: usize) -> Option<Ref> {
        if let Some(name) = &name {
            if OUTSIDE.contains(&name.as_str()) {
                self.problems
                    .push(format!("line {line}: {name} is the device's own (what it receives and sends); name the box something else."));
                return None;
            }
            if let Some((_, first)) = self.names.get(name) {
                self.problems.push(format!("line {line}: {name} is named twice (first on line {first})."));
                return None;
            }
            self.names.insert(name.clone(), (self.notation.boxes.len(), line));
        }
        self.notation.boxes.push(Item { name, what, line });
        Some(Ref::Box(self.notation.boxes.len() - 1))
    }

    /// One element of a chain: a box (named, by name, or written in place) or a control, and its port.
    fn element(&mut self, line: usize) -> Option<(Ref, Option<usize>)> {
        let found = match self.token().cloned() {
            Some(Token::Name(name)) => {
                self.at += 1;
                if self.token() == Some(&Token::Equals) {
                    self.at += 1;
                    if !matches!(self.token(), Some(Token::Object(_) | Token::Message(_))) {
                        let after = describe(self.token());
                        self.problems.push(format!("line {line}: {name} = needs a box after it, [object] or (message), not {after}."));
                        return None;
                    }
                    let what = self.boxed(line)?;
                    self.add(Some(name), what, line)?
                } else {
                    Ref::Name(name)
                }
            }
            Some(Token::Object(_) | Token::Message(_)) => {
                let what = self.boxed(line)?;
                self.add(None, what, line)?
            }
            Some(Token::Control(name)) => {
                self.at += 1;
                Ref::Control(name)
            }
            other => {
                self.problems.push(format!("line {line}: expected a box, a name or a control, not {}.", describe(other.as_ref())));
                return None;
            }
        };
        let port = match self.token() {
            Some(Token::Port(port)) => {
                let port = *port;
                self.at += 1;
                Some(port)
            }
            _ => None,
        };
        Some((found, port))
    }

    /// One line: a box, or a chain of cords.
    fn statement(&mut self) {
        let line = self.tokens[self.at].1;
        let mut chain: Vec<(Ref, Option<usize>)> = Vec::new();
        let read = loop {
            let Some(element) = self.element(line) else { break false };
            chain.push(element);
            match self.token() {
                Some(Token::Arrow) => self.at += 1,
                Some(Token::End) | None => break true,
                other => {
                    let other = describe(other);
                    self.problems.push(format!("line {line}: expected -> or the end of the line after a box, not {other}."));
                    break false;
                }
            }
        };
        // On to the next line, past whatever's left of this one.
        while self.at < self.tokens.len() && self.tokens[self.at].0 != Token::End {
            self.at += 1;
        }
        if !read {
            return;
        }
        let defined_alone = chain.len() == 1 && matches!(chain[0].0, Ref::Box(at) if self.notation.boxes[at].name.is_some());
        if chain.len() == 1 && !defined_alone {
            self.problems.push(format!("line {line}: a line is a box (name = [object]) or cords between boxes (a -> b)."));
            return;
        }
        let last = chain.len() - 1;
        if chain.iter().enumerate().any(|(at, (_, port))| port.is_some() && at != 0 && at != last) {
            self.problems.push(format!(
                "line {line}: a box in the middle of a chain is entered at inlet 0 and left by outlet 0; write a cord to or from another of its ports on a line of its own."
            ));
            return;
        }
        for hop in 0..last {
            let outlet = if hop == 0 { chain[0].1.unwrap_or(0) } else { 0 };
            let inlet = if hop + 1 == last { chain[last].1.unwrap_or(0) } else { 0 };
            self.hops.push((chain[hop].0.clone(), outlet, chain[hop + 1].0.clone(), inlet, line));
        }
    }
}

fn parse_from(text: &str, first_line: usize, top: bool) -> Result<Notation, Vec<String>> {
    let tokens = tokens(text, first_line).map_err(|problem| vec![problem])?;
    let mut reader =
        Reader { tokens: &tokens, at: 0, notation: Notation::default(), names: HashMap::new(), hops: Vec::new(), problems: Vec::new() };
    while reader.at < tokens.len() {
        if tokens[reader.at].0 == Token::End {
            reader.at += 1;
        } else {
            reader.statement();
        }
    }
    let Reader { mut notation, names, hops, mut problems, .. } = reader;
    let end = |found: Ref, line: usize, problems: &mut Vec<String>| -> Option<End> {
        match found {
            Ref::Box(at) => Some(End::Box(at)),
            Ref::Control(name) => Some(End::Control(name)),
            Ref::Name(name) => match names.get(&name) {
                Some((at, _)) => Some(End::Box(*at)),
                None if OUTSIDE.contains(&name.as_str()) && top => Some(End::Outside(name)),
                None if OUTSIDE.contains(&name.as_str()) => {
                    problems.push(format!("line {line}: {name} is the device's own; inside a [p], take what comes in from an [inlet] and send it on through an [outlet]."));
                    None
                }
                None => {
                    problems.push(format!("line {line}: no box is named {name} (name = [object] makes one)."));
                    None
                }
            },
        }
    };
    for (from, outlet, to, inlet, line) in hops {
        if let (Some(from), Some(to)) = (end(from, line, &mut problems), end(to, line, &mut problems)) {
            notation.cords.push(Link { from, outlet, to, inlet, line });
        }
    }
    if problems.is_empty() {
        Ok(notation)
    } else {
        Err(problems)
    }
}

/// What a patch's names and controls reach outside it, by box id: the device's in and out, its controls.
#[derive(Debug, Default)]
pub struct Outside<'a> {
    pub boxes: HashMap<String, String>,
    pub controls: BTreeMap<String, String>,
    /// What Kumi learned of the installed Max, for each object's inlets and outlets.
    pub reference: Option<&'a Reference>,
}

/// The version of Max Kumi writes patchers for.
fn appversion() -> Value {
    json!({ "major": 9, "minor": 1, "revision": 5, "architecture": "x64", "modernui": 1 })
}

/// A patch's boxes and cords as Max writes them (`{ "box": … }`, `{ "patchline": … }`), or what doesn't hold: a
/// control that isn't one, the device's own in or out where it has none.
pub fn expand(notation: &Notation, outside: &Outside) -> Result<(Vec<Value>, Vec<Value>), Vec<String>> {
    expand_within(notation, outside, true)
}

fn expand_within(notation: &Notation, outside: &Outside, top: bool) -> Result<(Vec<Value>, Vec<Value>), Vec<String>> {
    let mut problems: Vec<String> = Vec::new();
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
    for (at, item) in notation.boxes.iter().enumerate() {
        let class = item.what.class();
        let mut fields = match &item.what {
            What::Object(_) if matches!(class.as_str(), "inlet" | "outlet") && top => {
                problems.push(format!(
                    "line {}: [{class}] belongs in a [p] {{ … }}; the device's own patch takes what it receives from in and sends it to out.",
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
            What::Patcher { text, inner } => {
                let within = Outside { boxes: HashMap::new(), controls: BTreeMap::new(), reference: outside.reference };
                let (inner_boxes, inner_lines) = match expand_within(inner, &within, false) {
                    Ok(expanded) => expanded,
                    Err(inner_problems) => {
                        problems.extend(inner_problems);
                        continue;
                    }
                };
                let count = |which: &str| inner.boxes.iter().filter(|item| item.what.class() == which).count();
                let (ins, outs) = (count("inlet"), count("outlet"));
                json!({ "maxclass": "newobj", "text": text, "numinlets": ins, "numoutlets": outs, "outlettype": vec![""; outs],
                    "patcher": { "fileversion": 1, "appversion": appversion(), "classnamespace": "box", "rect": [100.0, 100.0, 640.0, 480.0],
                        "gridsize": [8.0, 8.0], "boxes": inner_boxes, "lines": inner_lines } })
            }
        };
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
    let mut lines: Vec<Value> = Vec::new();
    for link in &notation.cords {
        let mut end = |end: &End| -> Option<String> {
            match end {
                End::Box(at) => Some(id(*at)),
                End::Control(name) => match outside.controls.get(name) {
                    Some(found) => Some(found.clone()),
                    None if !top => {
                        problems.push(format!(
                            "line {}: \"{name}\" is a control on the device's face: bring its value into the [p] through an [inlet].",
                            link.line
                        ));
                        None
                    }
                    None if outside.controls.is_empty() => {
                        problems
                            .push(format!("line {}: \"{name}\" isn't a control: the device has none (controls makes them).", link.line));
                        None
                    }
                    None => {
                        problems.push(format!("line {}: \"{name}\" isn't a control; the device's are {}.", link.line, controls()));
                        None
                    }
                },
                End::Outside(name) => match outside.boxes.get(name) {
                    Some(found) => Some(found.clone()),
                    None => {
                        problems.push(format!("line {}: this device has no {name}.", link.line));
                        None
                    }
                },
            }
        };
        if let (Some(from), Some(to)) = (end(&link.from), end(&link.to)) {
            lines.push(json!({ "patchline": { "source": [from, link.outlet], "destination": [to, link.inlet] } }));
        }
    }
    if problems.is_empty() {
        Ok((boxes, lines))
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::patch::reference::{Count, Object};

    fn object(inlets: usize, outlets: usize) -> Object {
        Object { inlets: Count::Fixed { count: inlets }, outlets: Count::Fixed { count: outlets }, seen: 1, ..Object::default() }
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
        ))
        .unwrap();
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
        ))
        .unwrap_err();
        assert_eq!(notation, ["line 9: no box is named nope (name = [object] makes one)."], "a line inside a block counts in the whole");
        let notation =
            parse("fx = [gen~] { out1 = in1; }\nvoice = [p voice] {\n  [inlet] -> [outlet]\n}\nin -> fx -> voice -> out\n").unwrap();
        assert!(matches!(&notation.boxes[0].what, What::Gen { code, .. } if code.trim() == "out1 = in1;"));
        assert!(matches!(&notation.boxes[1].what, What::Patcher { inner, .. } if inner.boxes.len() == 2 && inner.cords.len() == 1));
    }

    #[test]
    fn what_cant_be_read_is_said_with_its_line() {
        let problems = parse("a = [metro 100\n").unwrap_err();
        assert_eq!(problems, ["line 1: a [ isn't closed on its line."]);
        let problems = parse(concat!(
            "a = [metro 100]\n",
            "a = [counter]\n",
            "a -> b.1 -> c\n",
            "[loadbang]\n",
            "fx = [gen~]\n",
            "out = [dac~]\n",
            "x = [print] { y }\n",
        ))
        .unwrap_err();
        assert_eq!(
            problems,
            [
                "line 2: a is named twice (first on line 1).",
                "line 3: a box in the middle of a chain is entered at inlet 0 and left by outlet 0; write a cord to or from another of its ports on a line of its own.",
                "line 4: a line is a box (name = [object]) or cords between boxes (a -> b).",
                "line 5: [gen~] holds GenExpr: write it in a { … } block after the box.",
                "line 6: out is the device's own (what it receives and sends); name the box something else.",
                "line 7: only [gen~], [gen] and [p] hold a { … } block, not [print].",
            ]
        );
    }

    #[test]
    fn a_patch_expands_to_maxs_boxes_with_the_ports_max_gives_them() {
        let mut reference = Reference::default();
        reference.objects.insert("metro".into(), object(2, 1));
        reference.objects.insert("counter".into(), object(5, 4));
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
        .unwrap();
        let outside = Outside {
            boxes: HashMap::from([("out".to_string(), "obj-out".to_string())]),
            controls: BTreeMap::from([("Rate".to_string(), "obj-control-1".to_string())]),
            reference: Some(&reference),
        };
        let (boxes, lines) = expand(&notation, &outside).unwrap();
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
        let cords: Vec<Value> = lines.iter().map(|line| json!([line["patchline"]["source"], line["patchline"]["destination"]])).collect();
        assert!(cords.contains(&json!([["obj-control-1", 0], ["obj-1", 1]])) && cords.contains(&json!([["obj-2", 0], ["obj-out", 0]])));
        assert!(cords.contains(&json!([["obj-1", 0], ["obj-4", 1]])));

        let wrong = parse("\"Rote\" -> [print]\nin -> [print]\n").unwrap();
        assert_eq!(
            expand(&wrong, &outside).unwrap_err(),
            ["line 1: \"Rote\" isn't a control; the device's are \"Rate\".", "line 2: this device has no in."]
        );
        let inside = parse("v = [p v] {\n  \"Rate\" -> [outlet]\n}\n").unwrap();
        assert_eq!(
            expand(&inside, &outside).unwrap_err(),
            ["line 2: \"Rate\" is a control on the device's face: bring its value into the [p] through an [inlet]."]
        );
        let loose = parse("[inlet] -> out\n").unwrap();
        assert_eq!(
            expand(&loose, &outside).unwrap_err()[0],
            "line 1: [inlet] belongs in a [p] { … }; the device's own patch takes what it receives from in and sends it to out."
        );
    }
}
