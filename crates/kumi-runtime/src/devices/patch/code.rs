//! What a code box's JavaScript does with a message, read from its code and the files it includes or requires:
//! whether the function the message calls sends out of an outlet at once. A message that only redraws a [v8ui], or
//! starts a Task that sends later, goes no further than the box.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use super::{Files, MaxBox};

/// How deep includes and requires are followed.
const MAX_DEPTH: usize = 8;

/// The JavaScript a code box runs, as its functions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Code {
    pub functions: Vec<Function>,
    /// Whether a file it includes or requires couldn't be read, so what it does is only partly known.
    pub partial: bool,
}

/// A function in the code: a message calls one by its name when it's defined at the top (in the box's own file or
/// one it includes), never one in a required module.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub top: bool,
    /// Its body without comments, strings or the functions it hands a Task (they run later).
    pub body: String,
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
    /// The code a box runs (a [v8.codebox]'s own, a [v8ui]'s or a [js]'s file) with what it includes and requires,
    /// read through `files`; None for a box that runs none, or whose file can't be read.
    pub fn read(item: &MaxBox, files: &dyn Files) -> Option<Code> {
        let own = match item.class() {
            "v8.codebox" => Some(item.str("code").to_string()).filter(|code| !code.is_empty()),
            "v8ui" | "jsui" => script(files, item.str("filename")),
            "js" | "v8" if item.maxclass() == "newobj" => script(files, item.args().first().copied().unwrap_or("")),
            _ => None,
        }?;
        let mut code = Code::default();
        let mut read = HashSet::new();
        code.add(&own, true, files, &mut read, 0);
        Some(code)
    }

    fn add(&mut self, source: &str, top: bool, files: &dyn Files, read: &mut HashSet<String>, depth: usize) {
        let clean = clean(source);
        self.functions.extend(functions(&clean, top));
        for found in INCLUDE.captures_iter(&clean_keeping_strings(source)) {
            let name = found[2].to_string();
            if depth >= MAX_DEPTH || !read.insert(name.clone()) {
                continue;
            }
            match script(files, &name) {
                // An include runs in the box's own scope; a required module keeps its own.
                Some(text) => self.add(&text, top && &found[1] == "include", files, read, depth + 1),
                None => self.partial = true,
            }
        }
    }

    /// Whether the message `selector` makes the code send out of an outlet at once: the function it calls (or
    /// `anything` when there's none of its name) or any function that one calls. None when the code is only partly
    /// known; Some(false) when no function answers it (Max says so, and nothing is sent).
    pub fn sends_at_once(&self, selector: &str) -> Option<bool> {
        if self.partial {
            return None;
        }
        let handler = |name: &str| self.functions.iter().position(|function| function.top && function.name == name);
        let Some(start) = handler(selector).or_else(|| handler("anything")) else { return Some(false) };
        let mut visited = HashSet::new();
        let mut stack = vec![start];
        while let Some(at) = stack.pop() {
            if !visited.insert(at) {
                continue;
            }
            let body = &self.functions[at].body;
            if OUTLET.is_match(body) {
                return Some(true);
            }
            for call in CALL.captures_iter(body) {
                let name = &call[1];
                stack.extend(self.functions.iter().enumerate().filter(|(_, function)| function.name == name).map(|(index, _)| index));
            }
        }
        Some(false)
    }
}

/// A script file's text, by its name with or without `.js`.
fn script(files: &dyn Files, name: &str) -> Option<String> {
    if name.is_empty() || name == "none" {
        return None;
    }
    files.text(name).or_else(|| if name.contains('.') { None } else { files.text(&format!("{name}.js")) })
}

/// Code without comments, and with each string's text blanked (its quotes kept), so braces and words in them don't
/// count.
fn clean(source: &str) -> String {
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

/// The functions defined in clean code, each with its body; `top` says whether the code runs in the box's own scope.
fn functions(clean: &str, top: bool) -> Vec<Function> {
    // How many braces are open before each byte, to tell a top-level function from one inside another.
    let mut depth_at = Vec::with_capacity(clean.len() + 1);
    let mut depth = 0usize;
    for byte in clean.bytes() {
        depth_at.push(depth);
        match byte {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth_at.push(depth);
    let mut found = Vec::new();
    for definition in DEFINITION.captures_iter(clean) {
        let (name, whole) = match (definition.name("named"), definition.name("assigned"), definition.name("method")) {
            (Some(name), _, _) | (_, Some(name), _) | (_, _, Some(name)) => (name.as_str(), definition.get(0).unwrap()),
            _ => continue,
        };
        if KEYWORDS.contains(&name) {
            continue;
        }
        // A method's head ends at its body's brace.
        let head_end = if whole.as_str().ends_with('{') { whole.end() - 1 } else { whole.end() };
        let Some(body) = body_after(clean, head_end) else { continue };
        found.push(Function { name: name.to_string(), top: top && depth_at[whole.start()] == 0, body: without_tasks(body) });
    }
    found
}

/// The body of the function whose head ends at `from`: the braces after its parameters, or an arrow's expression up to
/// the end of its line.
fn body_after(clean: &str, from: usize) -> Option<&str> {
    let bytes = clean.as_bytes();
    // A head that ends inside its parameters (`function name(`) closes them first.
    let mut at = from;
    let mut open = usize::from(clean[..from].ends_with('('));
    while open > 0 && at < bytes.len() {
        match bytes[at] {
            b'(' => open += 1,
            b')' => open -= 1,
            _ => {}
        }
        at += 1;
    }
    let rest = clean[at..].trim_start();
    let start = clean.len() - rest.len();
    if !rest.starts_with('{') {
        // An arrow's expression: to the end of its line.
        let end = rest.find('\n').map_or(clean.len(), |line| start + line);
        return Some(&clean[start..end]);
    }
    let mut depth = 0usize;
    for (offset, byte) in rest.bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&clean[start..start + offset + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// A body with what it hands each Task blanked: a Task runs it later, on its own clock.
fn without_tasks(body: &str) -> String {
    let mut body = body.to_string();
    while let Some(found) = TASK.find(&body) {
        let mut depth = 1usize;
        let mut end = body.len();
        for (offset, byte) in body[found.end()..].bytes().enumerate() {
            match byte {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = found.end() + offset + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        body.replace_range(found.start()..end, &" ".repeat(end - found.start()));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map, Value};
    use std::collections::HashMap;

    struct Scripts(HashMap<&'static str, &'static str>);

    impl Files for Scripts {
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
    }
}
