//! What a code box's JavaScript does with a message, read from its code and the files it includes or requires:
//! whether the function the message calls sends out of an outlet at once. A message that only redraws a [v8ui], or
//! starts a Task that sends later, goes no further than the box.
//!
//! A device's code can be anyone's, so what reading it takes is bounded: each file is read for its functions once,
//! however many boxes run it and however many names reach it, its functions' bodies kept as places in it; a file bigger
//! than [`MOST_SOURCE`] isn't read at all; and the questions asked of one patcher's code do at most [`MOST_ASKED`] in all,
//! each reading a function once and looking up a name once, however many call it. Past either, what the code does is
//! unknown, as it is for code that can't be read.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use kumi_common::js::string::utf16_len;
use regex::Regex;
use serde_json::Value;

use super::{Files, MaxBox};

/// How deep includes and requires are followed.
const MAX_DEPTH: usize = 8;
/// The most JavaScript in one file (or held in one box) that's read for its functions, in characters (UTF-16 units, as
/// Max and Kumi's caps count them): Kumi's own devices' code, at most the spec's `MAX_CODE` of them, is well under it.
const MOST_SOURCE: usize = 256 * 1024;
/// What the questions asked of one patcher's code may do in all: each byte of a function's body read, each look for a
/// name in a script, and each function a name finds count one. Code made so that a question reads it over and over
/// (functions nested in each other, each calling the next), or finds thousands of functions by a name, is unknown past it.
pub(crate) const MOST_ASKED: usize = 16 * 1024 * 1024;

/// The JavaScript a code box runs: the scripts of its own code and of what that includes and requires.
#[derive(Debug, Clone)]
pub struct Code {
    /// Each script, the box's own first, then what it includes and requires as they're met, with whether it runs in the
    /// box's own scope (an include does; a required module keeps its own).
    scripts: Vec<(Arc<Script>, bool)>,
    /// Whether a file it includes or requires couldn't be read, or was too big to, so what it does is only partly known.
    pub partial: bool,
    asked: Asked,
}

impl PartialEq for Code {
    fn eq(&self, other: &Code) -> bool {
        self.scripts == other.scripts && self.partial == other.partial
    }
}

/// What questions of the code may still read (shared by every box of one patcher), and what each found, by the function
/// it started from.
#[derive(Debug)]
struct Asked {
    left: Arc<AtomicUsize>,
    found: Mutex<HashMap<(usize, usize), Option<bool>>>,
}

impl Clone for Asked {
    fn clone(&self) -> Asked {
        Asked { left: Arc::clone(&self.left), found: Mutex::default() }
    }
}

/// One file's JavaScript (or what a box holds), read for its functions once, however many boxes run it.
#[derive(Debug, PartialEq, Default)]
pub struct Script {
    /// The code without comments, each string's text blanked: what each function's body is a place in.
    clean: String,
    functions: Vec<Function>,
    /// Each function by its name, in order.
    named: HashMap<String, Vec<usize>>,
    /// Where each Task is handed what it runs later (`new Task(…)`), in order: it runs on the Task's clock.
    tasks: Vec<Range<usize>>,
    /// What it includes (true) and requires (false), by name, in order.
    uses: Vec<(bool, String)>,
    /// Whether it was too big to read: what it does is unknown.
    unread: bool,
}

/// A function in a script: a message calls one by its name when it's defined at the top of the box's own scope (in the
/// box's own code or a file it includes), never one in a required module.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    /// Whether it's defined outside every block.
    outermost: bool,
    /// Its body: its braces after its parameters, or an arrow's expression to the end of its line.
    body: Range<usize>,
}

/// What a box's code is.
pub(crate) enum Own<'a> {
    /// What it holds itself: a [v8.codebox]'s own code, or code it embeds.
    Held(&'a str),
    /// The script file it names.
    File(&'a str),
}

static INCLUDE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\b(include|require)\s*\(\s*["']([^"']+)["']"#).unwrap());
static OUTLET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\boutlet\w*\s*\(").unwrap());
static CALL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([A-Za-z_$][\w$]*)\s*\(").unwrap());
static TASK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bnew\s+Task\s*\(").unwrap());
/// Where a function starts: `function name(`, `name = function(`, `name: function(`, `name = (…) => {`, or a class's
/// or an object's method `name(…) {`.
static DEFINITION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\bfunction\s*\*?\s*(?P<named>[A-Za-z_$][\w$]*)\s*\(",
        r"|(?P<assigned>[A-Za-z_$][\w$]*)\s*[:=]\s*(?:async\s+)?(?:function\b[^(]*\(|(?:\([^()]*\)|[A-Za-z_$][\w$]*)\s*=>)",
        r"|(?m:^[ \t]*(?:(?:static|async|get|set)\s+)*(?P<method>[A-Za-z_$][\w$]*)\s*\([^()]*\)\s*\{)",
    ))
    .unwrap()
});
const KEYWORDS: [&str; 9] = ["if", "for", "while", "switch", "catch", "function", "return", "with", "else"];

impl Code {
    /// What a box's code is: what it holds (a [v8.codebox]'s own, or what a [v8ui], a [jsui], a [v8] or a [js] embeds,
    /// with Max's embed flag and its text), else the file it names; None for a box that runs no code.
    pub(crate) fn own(item: &MaxBox) -> Option<Own<'_>> {
        let embedded = item
            .fields
            .get("textfile")
            .filter(|file| file.get("embed").and_then(Value::as_i64) == Some(1))
            .and_then(|file| file.get("text"))
            .and_then(Value::as_str);
        match item.class() {
            "v8.codebox" => Some(item.str("code")).filter(|code| !code.is_empty()).map(Own::Held),
            "v8ui" | "jsui" => Some(embedded.map_or(Own::File(item.str("filename")), Own::Held)),
            "js" | "v8" if item.maxclass() == "newobj" => {
                Some(embedded.map_or_else(|| Own::File(item.args().first().copied().unwrap_or("")), Own::Held))
            }
            _ => None,
        }
    }

    /// The code a box runs (a [v8.codebox]'s own, a [v8ui]'s or a [js]'s file, or what it embeds) with what it
    /// includes and requires, read through `files` as a patcher's boxes read theirs: each file once, by its place. None
    /// for a box that runs none, or whose file can't be read.
    pub fn read(item: &MaxBox, files: &dyn Files) -> Option<Code> {
        super::Reading::bounded().code(files, item).map(Arc::unwrap_or_clone)
    }

    /// The code that runs `own`, with what it includes and requires, each found by `script` (by its name, with or without
    /// `.js`, one file's the same script by any name that reaches it); questions asked of it take from what `left` says is
    /// left.
    pub(crate) fn of(own: Arc<Script>, script: &mut dyn FnMut(&str) -> Option<Arc<Script>>, left: Arc<AtomicUsize>) -> Code {
        let mut code = Code { scripts: Vec::new(), partial: false, asked: Asked { left, found: Mutex::default() } };
        code.add(own, true, script, &mut HashSet::new(), &mut HashSet::new(), 0);
        code
    }

    /// Adds `own`, and what it includes and requires: each name looked up once, and each script added once, however many
    /// names reach it.
    fn add(
        &mut self,
        own: Arc<Script>,
        top: bool,
        script: &mut dyn FnMut(&str) -> Option<Arc<Script>>,
        names: &mut HashSet<String>,
        added: &mut HashSet<*const Script>,
        depth: usize,
    ) {
        if !added.insert(Arc::as_ptr(&own)) {
            return;
        }
        self.partial |= own.unread;
        self.scripts.push((Arc::clone(&own), top));
        for (include, name) in &own.uses {
            if depth >= MAX_DEPTH || !names.insert(name.clone()) {
                continue;
            }
            match script(name) {
                // An include runs in the box's own scope; a required module keeps its own.
                Some(used) => self.add(used, top && *include, script, names, added, depth + 1),
                None => self.partial = true,
            }
        }
    }

    /// Whether the message `selector` makes the code send out of an outlet at once: the function it calls (or
    /// `anything` when there's none of its name) or any function that one calls. None when the code is only partly
    /// known, or the question reads more than is left to; Some(false) when no function answers it (Max says so, and
    /// nothing is sent).
    pub fn sends_at_once(&self, selector: &str) -> Option<bool> {
        if self.partial {
            return None;
        }
        let Some(start) = self.handler(selector).or_else(|| self.handler("anything")) else { return Some(false) };
        if let Some(found) = self.asked.found.lock().ok().and_then(|found| found.get(&start).copied()) {
            return found;
        }
        let found = self.follow(start);
        if let Ok(mut known) = self.asked.found.lock() {
            known.insert(start, found);
        }
        found
    }

    /// The first function of a name that a message calls: one defined at the top of the box's own scope.
    fn handler(&self, name: &str) -> Option<(usize, usize)> {
        self.scripts.iter().enumerate().filter(|(_, (_, top))| *top).find_map(|(at, (script, _))| {
            script.named.get(name)?.iter().copied().find(|&function| script.functions[function].outermost).map(|function| (at, function))
        })
    }

    /// Whether the function at `start` (its script, and it in that), or any it calls, reaches an outlet at once; None past
    /// what's left. Each function is read once, and each name called looked up once, however many functions it names and
    /// however many call it.
    fn follow(&self, start: (usize, usize)) -> Option<bool> {
        let mut queued = HashSet::from([start]);
        let mut looked: HashSet<&str> = HashSet::new();
        let mut stack = vec![start];
        while let Some((at, index)) = stack.pop() {
            let script = &self.scripts[at].0;
            let body = script.functions[index].body.clone();
            self.take(body.len())?;
            // What the body hands a Task runs later: it doesn't count.
            let later = script.tasks_in(&body);
            let now = |found: usize| {
                let place = body.start + found;
                let next = later.partition_point(|task| task.start <= place);
                next == 0 || later[next - 1].end <= place
            };
            let text = &script.clean[body.clone()];
            if OUTLET.find_iter(text).any(|found| now(found.start())) {
                return Some(true);
            }
            for call in CALL.captures_iter(text).filter(|call| call.get(0).is_some_and(|whole| now(whole.start()))) {
                let Some(name) = call.get(1).map(|name| name.as_str()) else { continue };
                if !looked.insert(name) {
                    continue;
                }
                // A look in each script, and each function the name finds there, count as a byte read does.
                self.take(self.scripts.len())?;
                for (other, (script, _)) in self.scripts.iter().enumerate() {
                    let Some(functions) = script.named.get(name) else { continue };
                    self.take(functions.len())?;
                    stack.extend(functions.iter().map(|&function| (other, function)).filter(|&found| queued.insert(found)));
                }
            }
        }
        Some(false)
    }

    /// Takes `cost` from what's left to the questions of this code; None when less is left.
    fn take(&self, cost: usize) -> Option<()> {
        self.asked.left.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| left.checked_sub(cost)).ok().map(drop)
    }
}

impl Script {
    /// A script read for its functions, in one pass over its code; one bigger than [`MOST_SOURCE`] isn't read.
    pub(crate) fn read(source: &str) -> Script {
        // A character is one to three bytes for each of its UTF-16 units: they're counted only when its bytes don't say.
        if source.len() > MOST_SOURCE && (source.len() > 3 * MOST_SOURCE || utf16_len(source) > MOST_SOURCE) {
            return Script { unread: true, ..Script::default() };
        }
        let clean = clean(source);
        let pairs = Pairs::of(&clean);
        let functions = functions(&clean, &pairs);
        let mut named: HashMap<String, Vec<usize>> = HashMap::new();
        for (at, function) in functions.iter().enumerate() {
            named.entry(function.name.clone()).or_default().push(at);
        }
        let tasks = TASK
            .find_iter(&clean)
            .map(|found| found.start()..pairs.close(found.end() - 1).map_or(clean.len(), |close| close + 1))
            .collect();
        let uses =
            INCLUDE.captures_iter(&clean_keeping_strings(source)).map(|found| (&found[1] == "include", found[2].to_string())).collect();
        Script { clean, functions, named, tasks, uses, unread: false }
    }

    /// The parts of a body it hands Tasks, as places that don't overlap, in order.
    fn tasks_in(&self, body: &Range<usize>) -> Vec<Range<usize>> {
        let (from, to) =
            (self.tasks.partition_point(|task| task.start < body.start), self.tasks.partition_point(|task| task.start < body.end));
        let mut later: Vec<Range<usize>> = Vec::new();
        for task in &self.tasks[from..to] {
            match later.last_mut() {
                Some(last) if task.start <= last.end => last.end = last.end.max(task.end),
                _ => later.push(task.clone()),
            }
        }
        later
    }
}

/// Where each brace and parenthesis that opens in clean code closes, found in one pass.
struct Pairs {
    closes: Vec<u32>,
}

impl Pairs {
    fn of(clean: &str) -> Pairs {
        let mut closes = vec![u32::MAX; clean.len()];
        let (mut braces, mut parens) = (Vec::new(), Vec::new());
        for (at, byte) in clean.bytes().enumerate() {
            match byte {
                b'{' => braces.push(at),
                b'(' => parens.push(at),
                b'}' => {
                    if let Some(open) = braces.pop() {
                        closes[open] = at as u32;
                    }
                }
                b')' => {
                    if let Some(open) = parens.pop() {
                        closes[open] = at as u32;
                    }
                }
                _ => {}
            }
        }
        Pairs { closes }
    }

    /// Where the brace or parenthesis opening at `open` closes.
    fn close(&self, open: usize) -> Option<usize> {
        self.closes.get(open).filter(|close| **close != u32::MAX).map(|close| *close as usize)
    }
}

static INLETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[^\w$.])inlets\s*=\s*(\d+)").unwrap());
static OUTLETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[^\w$.])outlets\s*=\s*(\d+)").unwrap());

/// The inlets and outlets a script gives its box (`inlets = 2; outlets = 3;` in its global code), each None when it
/// doesn't say (Max gives one).
pub fn declared_ports(source: &str) -> (Option<usize>, Option<usize>) {
    let clean = clean(source);
    let count = |pattern: &Regex| pattern.captures(&clean).and_then(|found| found[1].parse().ok());
    (count(&INLETS), count(&OUTLETS))
}

static AT_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":(\d+)(?::\d+)?\)?\s*$").unwrap());

/// What stops JavaScript from parsing, with its line (from 1); None when it parses. The code is only read, never run:
/// `Function(source)` makes a function of it without calling it. QuickJS reads it as Max's engine would, short of a
/// few newer forms.
pub fn syntax_problem(source: &str) -> Option<(usize, String)> {
    let checked = (|| -> rquickjs::Result<Option<(usize, String)>> {
        let runtime = rquickjs::Runtime::new()?;
        runtime.set_memory_limit(64 * 1024 * 1024);
        let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
        runtime.set_interrupt_handler(Some(Box::new(move || std::time::Instant::now() > until)));
        let context = rquickjs::Context::full(&runtime)?;
        context.with(|ctx| {
            let make: rquickjs::Function = ctx.globals().get("Function")?;
            match make.call::<_, rquickjs::Value>((source,)) {
                Ok(_) => Ok(None),
                Err(rquickjs::Error::Exception) => {
                    let thrown = ctx.catch();
                    let exception = thrown.as_exception();
                    let message = exception.and_then(|exception| exception.message()).unwrap_or_else(|| "it doesn't parse".into());
                    // The function the text is put in starts two lines before it, and ends a line after it.
                    let line = exception
                        .and_then(|exception| exception.stack())
                        .and_then(|stack| stack.lines().find_map(|line| AT_LINE.captures(line)?[1].parse::<usize>().ok()))
                        .map_or(1, |line| line.saturating_sub(2).clamp(1, source.lines().count().max(1)));
                    Ok(Some((line, message)))
                }
                Err(other) => Err(other),
            }
        })
    })();
    checked.unwrap_or(None)
}

/// Code without comments, and with each string's text blanked (its quotes kept), so braces and words in them don't
/// count.
pub(crate) fn clean(source: &str) -> String {
    strip(source, true)
}

/// Code without comments, strings kept: what includes and requires name.
fn clean_keeping_strings(source: &str) -> String {
    strip(source, false)
}

fn strip(source: &str, blank_strings: bool) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        if c == '/' && next == Some('/') {
            while at < chars.len() && chars[at] != '\n' {
                at += 1;
            }
        } else if c == '/' && next == Some('*') {
            at += 2;
            while at < chars.len() && !(chars[at] == '*' && chars.get(at + 1) == Some(&'/')) {
                at += 1;
            }
            at += 2;
            out.push(' ');
        } else if matches!(c, '"' | '\'' | '`') {
            out.push(c);
            at += 1;
            while at < chars.len() && chars[at] != c {
                if chars[at] == '\\' {
                    at += 1;
                } else if !blank_strings {
                    out.push(chars[at]);
                } else if chars[at] == '\n' {
                    out.push('\n');
                }
                at += 1;
            }
            out.push(c);
            at += 1;
        } else {
            out.push(c);
            at += 1;
        }
    }
    out
}

/// The functions defined in clean code, each with its body as a place in it, in one pass.
fn functions(clean: &str, pairs: &Pairs) -> Vec<Function> {
    let bytes = clean.as_bytes();
    let lines: Vec<usize> = bytes.iter().enumerate().filter(|(_, byte)| **byte == b'\n').map(|(at, _)| at).collect();
    // How many braces are open where each function is defined, to tell a top-level function from one inside another.
    let (mut depth, mut counted) = (0usize, 0usize);
    let mut found = Vec::new();
    for definition in DEFINITION.captures_iter(clean) {
        let (name, whole) = match (definition.name("named"), definition.name("assigned"), definition.name("method")) {
            (Some(name), _, _) | (_, Some(name), _) | (_, _, Some(name)) => (name.as_str(), definition.get(0).unwrap()),
            _ => continue,
        };
        if KEYWORDS.contains(&name) {
            continue;
        }
        for byte in &bytes[counted..whole.start()] {
            match byte {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        counted = whole.start();
        // A method's head ends at its body's brace.
        let head_end = if whole.as_str().ends_with('{') { whole.end() - 1 } else { whole.end() };
        let Some(body) = body_after(clean, head_end, pairs, &lines) else { continue };
        found.push(Function { name: name.to_string(), outermost: depth == 0, body });
    }
    found
}

/// The body of the function whose head ends at `from`: the braces after its parameters, or an arrow's expression up to
/// the end of its line (`lines` are where the code's lines end).
fn body_after(clean: &str, from: usize, pairs: &Pairs, lines: &[usize]) -> Option<Range<usize>> {
    // A head that ends inside its parameters (`function name(`) closes them first.
    let at = if clean[..from].ends_with('(') { pairs.close(from - 1).map_or(clean.len(), |close| close + 1) } else { from };
    let rest = clean[at..].trim_start();
    let start = clean.len() - rest.len();
    if !rest.starts_with('{') {
        // An arrow's expression: to the end of its line.
        return Some(start..lines.get(lines.partition_point(|line| *line < start)).copied().unwrap_or(clean.len()));
    }
    pairs.close(start).map(|close| start..close + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map, Value};
    use std::collections::HashMap;

    struct Scripts(HashMap<&'static str, &'static str>);

    impl Files for Scripts {
        fn place(&self, name: &str) -> Option<super::super::Place> {
            self.0.get(name).map(|text| super::super::Place { at: name.to_string(), bytes: text.len() })
        }

        fn patcher(&self, _name: &str) -> Option<Value> {
            None
        }

        fn text(&self, name: &str) -> Option<String> {
            self.0.get(name).map(|text| text.to_string())
        }
    }

    fn code_box(fields: Value) -> MaxBox {
        MaxBox { fields: fields.as_object().cloned().unwrap_or_else(Map::new), ..MaxBox::default() }
    }

    const BUTTON: &str = r#"
        outlets = 2;
        include('helpers.js');
        // const Old = require('gone.js');
        const ui = require('ui.js').ui;
        let held = false;
        const poll = new Task(function () { if (held) { outlet(0, 1); } }, this);
        function keep(value) { held = value; mgraphics.redraw(); }
        function watch(on) { if (on) { poll.repeat(); } else { poll.cancel(); } }
        function choose(mode) { held = mode; tell(mode); }
        var label = function (text) { ui.paint(text); };
        const nothing = (x) => { /* outlet(0, x) */ var s = "outlet(0)"; };
        function bang() { report(); }
        "#;

    #[test]
    fn a_message_sends_when_the_function_it_calls_reaches_an_outlet_and_not_through_a_task() {
        let files = Scripts(HashMap::from([
            ("button.js", BUTTON),
            ("helpers.js", "function tell(x) { outlet(1, x); }\nfunction report() { post('hi'); }"),
            ("ui.js", "exports.ui = { paint: function (t) { mgraphics.redraw(); } };"),
        ]));
        let code = Code::read(&code_box(json!({ "maxclass": "v8ui", "filename": "button.js" })), &files).expect("its code");
        assert!(!code.partial, "a commented-out require isn't read");
        assert_eq!(code.sends_at_once("keep"), Some(false), "it only redraws");
        assert_eq!(code.sends_at_once("watch"), Some(false), "the Task sends later");
        assert_eq!(code.sends_at_once("choose"), Some(true), "through a function an include defines");
        assert_eq!(code.sends_at_once("label"), Some(false));
        assert_eq!(code.sends_at_once("nothing"), Some(false), "comments and strings don't count");
        assert_eq!(code.sends_at_once("bang"), Some(false));
        assert_eq!(code.sends_at_once("missing"), Some(false), "no function answers it: Max says so, nothing is sent");
        assert_eq!(code.sends_at_once("paint"), Some(false), "a required module's functions aren't the box's");

        let anything = Code::read(
            &code_box(json!({ "maxclass": "v8.codebox", "filename": "none", "code": "function anything() { outlet(0, messagename); }" })),
            &files,
        )
        .unwrap();
        assert_eq!(anything.sends_at_once("whatever"), Some(true), "anything answers what no function does");

        let partial = Code::read(
            &code_box(json!({ "maxclass": "newobj", "text": "js lost" })),
            &Scripts(HashMap::from([("lost.js", "include('gone.js');")])),
        )
        .unwrap();
        assert!(partial.partial);
        assert_eq!(partial.sends_at_once("keep"), None, "what it includes can't be read, so it can't be known");
        assert!(Code::read(&code_box(json!({ "maxclass": "newobj", "text": "js nowhere" })), &files).is_none());

        let embedded = code_box(json!({ "maxclass": "newobj", "text": "v8 steps @embed 1", "filename": "steps.js",
            "textfile": { "text": "function bang() { outlet(0, 1); }", "filename": "steps.js", "flags": 1, "embed": 1, "autowatch": 1 } }));
        assert_eq!(Code::read(&embedded, &files).unwrap().sends_at_once("bang"), Some(true), "the code it embeds, with no file");
    }

    #[test]
    fn a_scripts_ports_and_whether_it_parses_are_read_from_its_code() {
        assert_eq!(declared_ports("inlets = 2;\noutlets = 3;\nfunction bang() { this.inlets = 9; }"), (Some(2), Some(3)));
        assert_eq!(declared_ports("// outlets = 4\nfunction bang() { outlet(0, 1); }"), (None, None), "a comment doesn't count");
        assert_eq!(syntax_problem("outlets = 1;\nfunction bang() { outlet(0, [1, 2].map((x) => x * 2)); }"), None);
        assert_eq!(syntax_problem("outlets = 1;\nfunction bang() {\n  outlet(0, 1;\n}\n"), Some((3, "Unexpected token ';'".into())));
        assert_eq!(syntax_problem("function f( {"), Some((1, "Unexpected end of input".into())), "the last line, at most");
    }

    /// #772: code anyone wrote is read in one pass, each function's body a place in it, and what a question of it reads
    /// is bounded. A box nesting thousands of functions answers at once; one bigger than what's read isn't read; and a
    /// question that would read the nesting over and over is unknown.
    #[test]
    fn nested_functions_are_read_in_one_pass_and_what_a_question_reads_is_bounded() {
        let nested = |levels: usize, inner: &str| {
            let mut code = String::from("function bang() { f0(); }\n");
            for level in 0..levels {
                code.push_str(&format!("function f{level}() {{ f{}(); ", level + 1));
            }
            code.push_str(inner);
            code.push_str(&"}".repeat(levels));
            code
        };
        let held = |code: String| code_box(json!({ "maxclass": "v8.codebox", "code": code }));
        let none = Scripts(HashMap::new());
        let sends = Code::read(&held(nested(5_000, "outlet(0, 1);")), &none).unwrap();
        assert!(!sends.partial);
        assert_eq!(sends.sends_at_once("bang"), Some(true), "the innermost function sends");
        let quiet = Code::read(&held(nested(5_000, "post(1);")), &none).unwrap();
        assert!(!quiet.partial);
        assert_eq!(quiet.sends_at_once("bang"), None, "each calls the next: too much to read to know");
        let huge = Code::read(&held(nested(100_000, "outlet(0, 1);")), &none).unwrap();
        assert!(huge.partial, "past what's read of one file");
        assert_eq!(huge.sends_at_once("bang"), None);

        // What a body hands a Task runs later; a function written in a Task's code still sends when it's called itself.
        let tasked = Code::read(
            &held("function later() { new Task(function tick() { outlet(0, 1); }).schedule(10); }\nfunction now() { tick(); }".into()),
            &none,
        )
        .unwrap();
        assert_eq!(tasked.sends_at_once("later"), Some(false));
        assert_eq!(tasked.sends_at_once("now"), Some(true));
    }

    /// #772: a name thousands of functions share is looked up once a question, and each function it finds read once, both
    /// counting against what the questions may do; a file many names reach is read for its functions once; and what's
    /// read of a file is counted in characters, as Kumi's own caps count them.
    #[test]
    fn a_name_many_functions_share_and_a_file_many_names_reach_are_taken_once() {
        let held = |code: String| code_box(json!({ "maxclass": "v8.codebox", "code": code }));
        let none = Scripts(HashMap::new());
        let shared = |many: usize| format!("function bang() {{ a(); }}\n{}", "a=()=>a()\n".repeat(many));
        let taken = |code: &Code| MOST_ASKED - code.asked.left.load(Ordering::Relaxed);

        // What a name finds counts as well as what's read: with only the functions' bodies' worth left, it can't be known.
        let script = Arc::new(Script::read(&shared(2_000)));
        let bodies: usize = script.functions.iter().map(|function| function.body.len()).sum();
        let short = Code::of(script, &mut |_: &str| None, Arc::new(AtomicUsize::new(bodies)));
        assert_eq!(short.sends_at_once("bang"), None);

        // Twenty thousand functions of one name, each calling it: each is read once, and the name looked up once.
        let many = Code::read(&held(shared(20_000)), &none).unwrap();
        assert!(!many.partial);
        assert_eq!(many.sends_at_once("bang"), Some(false));
        assert!(taken(&many) < 10 * 20_000, "{} taken", taken(&many));

        // A file a hundred names reach is one script: a question of it takes what it takes through one name.
        struct Lib;
        impl Files for Lib {
            fn place(&self, name: &str) -> Option<super::super::Place> {
                name.starts_with("lib").then(|| super::super::Place { at: "lib.js".into(), bytes: 64 })
            }

            fn patcher(&self, _name: &str) -> Option<Value> {
                None
            }

            fn text(&self, name: &str) -> Option<String> {
                name.starts_with("lib").then(|| "function tell(x) { post(x); }".to_string())
            }
        }
        let including = |names: usize| {
            let includes: String = (0..names).map(|at| format!("include('lib{at}');\n")).collect();
            let code = Code::read(&held(format!("{includes}function bang() {{ tell(1); }}")), &Lib).unwrap();
            assert_eq!(code.sends_at_once("bang"), Some(false));
            taken(&code)
        };
        assert_eq!(including(100), including(1));

        // A hundred thousand characters, the most Kumi writes of a code box, are read whatever script they're in: here
        // three bytes each.
        let wide = Code::read(&held(format!("// {}\nfunction bang() {{ outlet(0, 1); }}", "音".repeat(100_000))), &none).unwrap();
        assert!(!wide.partial);
        assert_eq!(wide.sends_at_once("bang"), Some(true));
    }
}
