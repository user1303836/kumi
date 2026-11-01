//! (#399 K34) What a kind or an action offers the model is read on its way to the DAW, by that kind's own code, not just named
//! somewhere in its bridge. A kind's code is where its bridge registers it and what that reaches, found by reading the source
//! as text (as `own_changes::each_daws_kinds_are_exactly_what_its_bridge_makes` does), so a field a bridge drops or renames
//! fails here, by its kind and name.
//!
//! - REAPER's extension is Rust and Renoise's tool Lua. What the host makes or takes itself is listed with why, and read in the
//!   host's functions.
//! - A read is what comes off the request: not a key of what a change answers, nor one read off what it answered or found.
//! - Code is read in blocks by their indent: rustfmt keeps Rust's so, and the tool's style its Lua's.
//! - The simulators stand in for a few kinds and read only what their tests need, so "reads every field" doesn't apply to them.
//!   They're held to the other way round: what they read, their DAW offers.
//! - The tests' fakes answer what each test scripts, and read no kind's input.
use daw_mcp_server::renoise::{instruments, library_files, presets};
use kumi_daw_own::changes::OwnChangeSet;
use kumi_daw_reaper::changes as reaper_changes;
use kumi_daw_renoise::changes as renoise_changes;
use regex::Regex;
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap, HashSet},
    rc::Rc,
    sync::LazyLock,
};

/// How many calls from a kind's own code its reads are looked for.
const REACH: usize = 4;

const REAPER_CHANGES: &str = "daws/reaper/reaper-kumi/src/changes";
const RENOISE_TOOL: &str = "daws/renoise/com.kumi.Bridge.xrnx/payload";
const HOST: &str = "daws/own/daw-mcp-server/src";

/// The kinds the host makes itself, from the device's preset data, and the host's functions that read their fields. The tool
/// only writes what the host made, so these fields are held to the host's code, not to the Lua's. `doofer-make`'s apply is the
/// tool's, with the input as it came, so the tool's reads count for it too.
const MADE_BY_THE_HOST: &[(&str, &[(&str, &str)], bool)] = &[
    ("doofer-macro", &[("presets", "doofer_macro"), ("presets", "read_params")], false),
    ("doofer-device-add", &[("presets", "device_add"), ("presets", "read_params")], false),
    ("doofer-device-delete", &[("presets", "device_delete"), ("presets", "read_params")], false),
    ("doofer-device-move", &[("presets", "device_move"), ("presets", "read_params")], false),
    ("doofer-device-set", &[("presets", "device_set"), ("presets", "read_params")], false),
    ("splitter-set", &[("presets", "splitter_set"), ("presets", "read_params")], false),
    ("notepad-write", &[("presets", "notepad_write"), ("presets", "read_params")], false),
    ("doofer-make", &[("presets", "make"), ("presets", "make_preview")], true),
];

/// Fields the bridge never gets, by DAW, kind and field: the host's function that takes each, and why. (A library file's
/// `path`, for `FILE_KINDS`, is the same: the host checks the file and names it to the tool.)
const TAKEN_BY_THE_HOST: &[(&str, &str, &str, (&str, &str), &str)] = &[
    ("REAPER", "fx-preset", "path", ("preset_files", "with_file"), "the host checks the .vstpreset file and hands the bridge its bytes"),
    (
        "Renoise",
        "plugin-preset-data",
        "path",
        ("library_files", "vst3_file"),
        "the host checks the .vstpreset file and hands the tool its bytes",
    ),
    ("Renoise", "instrument-macro-map", "add", ("instruments", "edit"), "the host makes the mappings in the instrument's file"),
    ("Renoise", "instrument-macro-map", "add.modulation", ("instruments", "edit"), "the host makes the mappings in the instrument's file"),
    ("Renoise", "instrument-macro-map", "add.fx", ("instruments", "edit"), "the host makes the mappings in the instrument's file"),
    ("Renoise", "instrument-macro-map", "add.device", ("instruments", "edit"), "the host makes the mappings in the instrument's file"),
    ("Renoise", "instrument-macro-map", "remove", ("instruments", "edit"), "the host makes the mappings in the instrument's file"),
];

/// Each action's method: where its bridge answers it, and the host's function that forwards its fields (none: all of them).
const REAPER_ACTIONS: &[(&str, (&str, &str), Option<&str>)] = &[
    ("transport_play", ("mod", "transport_play"), Some("transport")),
    ("transport_stop", ("mod", "transport_stop"), None),
    ("cursor_set", ("mod", "cursor_set"), Some("transport")),
    ("fx_chain_save", ("mod", "fx_chain_save"), Some("fx_chain_save")),
    ("project_save", ("actions", "project_save"), None),
    ("track_template_save", ("actions", "track_template_save"), None),
];
const RENOISE_ACTIONS: &[(&str, (&str, &str), Option<&str>)] = &[
    ("transport_play", ("bridge", "handlers.transport_play"), Some("transport")),
    ("transport_stop", ("bridge", "handlers.transport_stop"), None),
    ("audition_note", ("actions", "M.audition"), Some("action")),
    ("select", ("actions", "M.select"), Some("action")),
    ("project_save", ("actions", "M.project_save"), None),
    ("instrument_save", ("changes_instruments", "M.save"), Some("action")),
    ("device_editor", ("changes_presets", "M.editor"), Some("renoise_action")),
    ("preset_save", ("changes_presets", "M.save"), Some("renoise_action")),
];

/// The simulators' files, by what they stand in for.
const SIMULATORS: &[(&str, &[&str])] = &[
    ("REAPER", &["reaper/simulator.rs", "simulator/listening.rs"]),
    ("Renoise", &["renoise/simulator.rs", "renoise/simulator/takes.rs", "renoise/simulator/dsp.rs", "simulator/listening.rs"]),
];

fn sets() -> [(&'static str, &'static OwnChangeSet); 2] {
    [("REAPER", &reaper_changes::CHANGES), ("Renoise", &renoise_changes::CHANGES)]
}

fn source(path: &str) -> String {
    std::fs::read_to_string(kumi_testkit::workspace_path(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The files of a folder whose names start with `prefix`, by their names less `extension`, without tests' files.
fn folder(path: &str, prefix: &str, extension: &str) -> Vec<(String, String)> {
    let mut files = vec![];
    for entry in std::fs::read_dir(kumi_testkit::workspace_path(path)).unwrap() {
        let file = entry.unwrap().path();
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(extension).filter(|stem| stem.starts_with(prefix) && !stem.starts_with("tests")) {
            files.push((stem.to_string(), std::fs::read_to_string(&file).unwrap()));
        }
    }
    files
}

/// A field a schema offers: where it is (`outer.inner`), the name its reader knows it by, and whether it's at the top.
struct Field {
    path: String,
    name: String,
    top: bool,
}

/// Every field a schema offers, inside an array's items or one of its shapes too, a top-level one under its `wire` name.
fn fields_of(schema: &str, wire: &[(&str, &str)]) -> Vec<Field> {
    fn walk(schema: &Value, path: &str, wire: &[(&str, &str)], out: &mut Vec<Field>) {
        let Some(schema) = schema.as_object() else { return };
        for (name, inner) in schema.get("properties").and_then(Value::as_object).into_iter().flatten() {
            let top = path.is_empty();
            let field = if top { name.clone() } else { format!("{path}.{name}") };
            let known = wire.iter().find(|(ours, _)| top && *ours == name.as_str()).map_or(name.as_str(), |(_, theirs)| *theirs);
            out.push(Field { path: field.clone(), name: known.to_string(), top });
            walk(inner, &field, wire, out);
        }
        for key in ["items", "additionalProperties"] {
            if let Some(inner) = schema.get(key) {
                walk(inner, if path.is_empty() { "[]" } else { path }, wire, out);
            }
        }
        for key in ["oneOf", "anyOf", "allOf"] {
            for inner in schema.get(key).and_then(Value::as_array).into_iter().flatten() {
                walk(inner, path, wire, out);
            }
        }
    }
    let mut out = vec![];
    walk(&serde_json::from_str(schema).unwrap(), "", wire, &mut out);
    out
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The line that opens the block holding this one: the nearest above it with less indent.
fn opening_line(lines: &[&str], at: usize) -> usize {
    (0..at).rev().find(|above| !lines[*above].trim().is_empty() && indent(lines[*above]) < indent(lines[at])).unwrap_or(at)
}

/// The line that closes the block opening at `at`: the first at its indent that starts with one of `closers` and opens nothing
/// new; or the line before one with less indent.
fn closing_line(lines: &[&str], at: usize, closers: &[&str]) -> usize {
    for later in at + 1..lines.len() {
        let line = lines[later];
        if line.trim().is_empty() {
            continue;
        }
        if indent(line) < indent(lines[at]) {
            return later - 1;
        }
        if indent(line) == indent(lines[at])
            && closers.iter().any(|closer| line.trim_start().starts_with(closer))
            && !line.trim_end().ends_with('{')
        {
            return later;
        }
    }
    lines.len() - 1
}

fn rust_block_end(lines: &[&str], at: usize) -> usize {
    let first = lines[at].trim_end();
    let closed = first.ends_with(';')
        || first.ends_with("),")
        || first.ends_with("},")
        || (first.ends_with('}') && first.matches('{').count() == first.matches('}').count());
    if closed {
        at
    } else {
        closing_line(lines, at, &["}", "]", ")"])
    }
}

fn lua_block_end(lines: &[&str], at: usize) -> usize {
    let first = lines[at].trim_end();
    let table_closed = first.ends_with('}') || first.ends_with("})") || first.ends_with("},");
    let closed = first.ends_with(" end") || first.ends_with(" end)") || first.ends_with(" end,") || table_closed;
    if closed && first.matches('{').count() <= first.matches('}').count() {
        at
    } else {
        closing_line(lines, at, &["end", "}", ")", "until"])
    }
}

/// Rust's lines, without whole-line comments or `#[cfg(test)]` items.
fn rust_lines(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim_start().starts_with("//")).collect();
    let (mut out, mut at) = (vec![], 0);
    while at < lines.len() {
        if lines[at].trim() == "#[cfg(test)]" {
            at = rust_block_end(&lines, at + 1) + 1;
            continue;
        }
        out.push(lines[at].to_string());
        at += 1;
    }
    out
}

fn char_before(text: &str, at: usize) -> Option<u8> {
    at.checked_sub(1).map(|before| text.as_bytes()[before])
}

/// What a stretch of code reads, by name: off the request (the top of a kind's input), and off anything (what's inside it).
#[derive(Default)]
struct Reads {
    of_input: HashSet<String>,
    anywhere: HashSet<String>,
}

impl Reads {
    fn add(&mut self, more: &Reads) {
        self.of_input.extend(more.of_input.iter().cloned());
        self.anywhere.extend(more.anywhere.iter().cloned());
    }
    fn holds(&self, field: &str, top: bool) -> bool {
        if top {
            self.of_input.contains(field)
        } else {
            self.anywhere.contains(field)
        }
    }
}

/// A unit of code: a function or a table, by its module and name.
type Unit = (String, String);

/// A language's code, as the walk from a kind's own code needs it.
trait Code {
    fn reads(&self, text: &str) -> Reads;
    /// The units a stretch of a module names.
    fn names(&self, module: &str, text: &str) -> Vec<Unit>;
    fn texts(&self, unit: &Unit) -> Vec<&str>;
}

/// What each unit reads and names, worked out once.
#[derive(Default)]
struct Memo(RefCell<HashMap<Unit, Rc<(Reads, Vec<Unit>)>>>);

impl Memo {
    fn of(&self, code: &dyn Code, unit: &Unit) -> Rc<(Reads, Vec<Unit>)> {
        if let Some(known) = self.0.borrow().get(unit) {
            return known.clone();
        }
        let mut reads = Reads::default();
        let mut names = vec![];
        for text in code.texts(unit) {
            reads.add(&code.reads(text));
            names.extend(code.names(&unit.0, text));
        }
        let walked = Rc::new((reads, names));
        self.0.borrow_mut().insert(unit.clone(), walked.clone());
        walked
    }
}

/// What a kind's code reads and where it looked: `entries` (a module, and a stretch of it), and the units they reach.
fn scope(code: &dyn Code, memo: &Memo, entries: &[(String, String)]) -> (Reads, BTreeSet<Unit>) {
    let mut reads = Reads::default();
    let mut frontier: Vec<Unit> = vec![];
    for (module, text) in entries {
        reads.add(&code.reads(text));
        frontier.extend(code.names(module, text));
    }
    let mut seen = BTreeSet::new();
    for _ in 0..REACH {
        let mut next = vec![];
        for unit in frontier {
            if seen.insert(unit.clone()) {
                let walked = memo.of(code, &unit);
                reads.add(&walked.0);
                next.extend(walked.1.iter().cloned());
            }
        }
        frontier = next;
    }
    (reads, seen)
}

// ----- Rust -----

static RUST_UNIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bfn ([a-z_][a-z0-9_]*)\s*(?:<[^>]*>)?\s*\(|^\s*(?:pub(?:\([a-z]+\))?\s+)?(?:static|const) ([A-Z][A-Z0-9_]*)\b").unwrap()
});
static CALL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:([a-z_][a-z0-9_]*)::)?([a-z_][a-z0-9_]*)\s*\(").unwrap());
static SELF_CALL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bself\.([a-z_][a-z0-9_]*)\s*\(").unwrap());
static PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([a-z_][a-z0-9_]*)::([a-z_][a-z0-9_]*)\b").unwrap());
static NAMED_CONSTANT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:([a-z_][a-z0-9_]*)::)?([A-Z][A-Z0-9_]+)\b").unwrap());
static STRING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([A-Za-z][A-Za-z0-9_]*)""#).unwrap());
static OFF_THE_REQUEST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:input|params|args|request)\s*(?:\.\s*get(?:_mut)?\(\s*|\[\s*)$").unwrap());
static THE_REQUEST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:input|params|args|request)\b").unwrap());
static A_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bkey\s*:\s*$").unwrap());
static NAME_AT_THE_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\s*$").unwrap());
static DISPATCH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([a-z]+(?:-[a-z]+)*)" => Some\(([^\n]*)\),"#).unwrap());
static ARM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:Kind|Which|Mode|Marks)::([A-Z][A-Za-z]*)\b(?:\([^)]*\))?\s*=>\s*(?:Ok\()?(?:([a-z_]+)::)?([a-z_]+)\(").unwrap()
});
static VARIANT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:([a-z_]+)::)?[A-Z][A-Za-z]*::([A-Z][A-Za-z]*)").unwrap());

/// Whether the string at `at` stands where a field of the request is read, `after` being what follows it: off the request
/// (`input.get("f")`), beside it in a plain call (`moment(ctx, input, "f")`), as a table's `key: "f"`, an item of a literal list
/// or a tuple row, the one argument to a local closure (`id("f")`), or a match arm. A label (`noun: "f"`), a comparison, an API's
/// name (`fx_set_config(at.fx, "f", …)`) or a read off anything else isn't one.
fn rust_off_the_request(text: &str, at: usize, after: &str) -> bool {
    let mut from = at.saturating_sub(200);
    while !text.is_char_boundary(from) {
        from -= 1;
    }
    let before = &text[from..at];
    let close = before.trim_end();
    let a_value = close.strip_suffix(':').is_some_and(|key| key.trim_end().ends_with('"'));
    if a_value || close.ends_with("==") || close.ends_with("!=") || after.starts_with("==") || after.starts_with("!=") {
        return false;
    }
    if OFF_THE_REQUEST.is_match(before)
        || A_KEY.is_match(before)
        || after.starts_with("=>")
        || after.starts_with('|')
        || close.ends_with('|')
    {
        return true;
    }
    let mut depth = 0;
    for (open, byte) in before.bytes().enumerate().rev() {
        match byte {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' if depth > 0 => depth -= 1,
            b'{' => return false,
            // (a list, not an index off some other name)
            b'[' => {
                let head = before[..open].trim_end();
                return head.ends_with(" in") || !head.ends_with(|c: char| c.is_alphanumeric() || matches!(c, '_' | ']' | ')'));
            }
            b'(' => {
                let (head, args) = (&before[..open], &before[open + 1..]);
                let name = NAME_AT_THE_END.captures(head).map(|found| found[1].to_string());
                let a_method = name
                    .as_ref()
                    .is_some_and(|name| head.trim_end().strip_suffix(name.as_str()).is_some_and(|rest| rest.trim_end().ends_with('.')));
                if a_method {
                    return false;
                }
                if args.trim().is_empty() {
                    return match name {
                        Some(name) => !["Some", "Ok", "Err", "bad", "failed", "observation"].contains(&name.as_str()),
                        // (a tuple row, not a macro's argument)
                        None => !head.trim_end().ends_with('!'),
                    };
                }
                return THE_REQUEST.is_match(args) && !args.contains('[') && !args.contains(".get(");
            }
            _ => {}
        }
    }
    false
}

/// Rust's reads, of a string that isn't a key of what's answered (`json!({"f": …})`, `map.insert("f".into(), …)`,
/// `state["f"] = …`): inside a field, any such string, as what holds it goes by any name; at the top of the input, only what
/// stands off the request.
fn rust_reads(text: &str) -> Reads {
    let mut reads = Reads::default();
    for found in STRING.captures_iter(text) {
        let whole = found.get(0).unwrap();
        let after = text[whole.end()..].trim_start();
        let a_key = after.starts_with(':') && !after.starts_with("::");
        let made = [".into()", ".to_string()", ".to_owned()"].iter().any(|made| after.starts_with(made));
        let written =
            after.strip_prefix(']').is_some_and(|rest| rest.trim_start().starts_with('=') && !rest.trim_start().starts_with("=="));
        if a_key || made || written {
            continue;
        }
        reads.anywhere.insert(found[1].to_string());
        if rust_off_the_request(text, whole.start(), after) {
            reads.of_input.insert(found[1].to_string());
        }
    }
    reads
}

struct RustCode {
    modules: Vec<(String, Vec<String>)>,
    /// Functions and constants by name, with the module of each.
    units: HashMap<String, Vec<(String, String)>>,
    /// Each module's `"kind" => Some(its path),`, by kind.
    dispatch: HashMap<String, Vec<(String, String)>>,
    /// Each module's `Which::Add => add_preview(…)`, by module and variant: the function and what it's qualified by.
    arms: HashMap<(String, String), Vec<(Option<String>, String)>>,
}

impl RustCode {
    fn new(files: Vec<(String, String)>) -> Self {
        let modules: Vec<(String, Vec<String>)> = files.into_iter().map(|(name, text)| (name, rust_lines(&text))).collect();
        let (mut units, mut dispatch, mut arms) =
            (HashMap::<String, Vec<_>>::new(), HashMap::<String, Vec<_>>::new(), HashMap::<_, Vec<_>>::new());
        for (module, lines) in &modules {
            let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
            for (at, line) in lines.iter().enumerate() {
                if let Some(found) = RUST_UNIT.captures(line) {
                    let name = found.get(1).or(found.get(2)).unwrap().as_str().to_string();
                    units.entry(name).or_default().push((module.clone(), lines[at..=rust_block_end(&lines, at)].join("\n")));
                }
            }
            let text = lines.join("\n");
            for found in DISPATCH.captures_iter(&text) {
                dispatch.entry(found[1].to_string()).or_default().push((module.clone(), found[2].to_string()));
            }
            for found in ARM.captures_iter(&text) {
                arms.entry((module.clone(), found[1].to_string()))
                    .or_default()
                    .push((found.get(2).map(|m| m.as_str().to_string()), found[3].to_string()));
            }
        }
        Self { modules, units, dispatch, arms }
    }
    /// A function in a module: a stretch of it to start a walk from.
    fn function(&self, module: &str, name: &str) -> Vec<(String, String)> {
        self.units.get(name).into_iter().flatten().filter(|(defined, _)| defined == module).cloned().collect()
    }
    /// `m::f` in m; `f` in its own module, else the only one by that name.
    fn resolve(&self, out: &mut Vec<Unit>, module: &str, qualifier: Option<&str>, name: &str) {
        let Some(defined) = self.units.get(name) else { return };
        let qualifier = if qualifier == Some("super") { Some("mod") } else { qualifier };
        let inside = |wanted: &str| defined.iter().any(|(module, _)| module == wanted);
        match qualifier.filter(|qualifier| self.modules.iter().any(|(module, _)| module == qualifier)) {
            Some(qualifier) => {
                if inside(qualifier) {
                    out.push((qualifier.to_string(), name.to_string()));
                }
            }
            None if inside(module) => out.push((module.to_string(), name.to_string())),
            None if defined.len() == 1 => out.push((defined[0].0.clone(), name.to_string())),
            None => {}
        }
    }
}

impl Code for RustCode {
    fn reads(&self, text: &str) -> Reads {
        rust_reads(text)
    }
    fn names(&self, module: &str, text: &str) -> Vec<Unit> {
        let mut out = vec![];
        for found in CALL.captures_iter(text) {
            if char_before(text, found.get(0).unwrap().start()) != Some(b'.') {
                self.resolve(&mut out, module, found.get(1).map(|m| m.as_str()), &found[2]);
            }
        }
        for found in SELF_CALL.captures_iter(text) {
            self.resolve(&mut out, module, None, &found[1]);
        }
        for found in PATH.captures_iter(text) {
            if char_before(text, found.get(0).unwrap().start()) != Some(b'.') {
                self.resolve(&mut out, module, Some(&found[1]), &found[2]);
            }
        }
        for found in NAMED_CONSTANT.captures_iter(text) {
            self.resolve(&mut out, module, found.get(1).map(|m| m.as_str()), &found[2]);
        }
        out
    }
    fn texts(&self, unit: &Unit) -> Vec<&str> {
        self.units.get(&unit.1).into_iter().flatten().filter(|(module, _)| *module == unit.0).map(|(_, text)| text.as_str()).collect()
    }
}

/// Where REAPER's extension makes a kind: a table's entry with its module's functions for that table, or what its name's
/// dispatch reaches, from the innermost variant with arms of its own.
fn reaper_entries(code: &RustCode, kind: &str) -> Vec<(String, String)> {
    let mut entries = vec![];
    let entry = format!("kind: \"{kind}\"");
    for (module, lines) in &code.modules {
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        for at in (0..lines.len()).filter(|at| lines[*at].contains(&entry)) {
            let open = opening_line(&lines, at);
            entries.push((module.clone(), lines[open..=rust_block_end(&lines, open)].join("\n")));
            for table in ["prop_preview", "prop_apply", "preview", "apply"] {
                entries.extend(code.function(module, table));
            }
        }
    }
    for (module, path) in code.dispatch.get(kind).into_iter().flatten() {
        let variants: Vec<_> = VARIANT.captures_iter(path).map(|v| (v.get(1).map(|m| m.as_str().to_string()), v[2].to_string())).collect();
        for (named, variant) in variants.iter().rev() {
            let places: BTreeSet<&str> = [Some(module.as_str()), Some("mod"), named.as_deref()].into_iter().flatten().collect();
            let found: Vec<_> = places
                .into_iter()
                .filter_map(|place| code.arms.get(&(place.to_string(), variant.clone())).map(|arms| (place, arms)))
                .flat_map(|(place, arms)| arms.iter().map(move |arm| (place, arm)))
                .collect();
            if found.is_empty() {
                continue;
            }
            for (place, (qualifier, function)) in found {
                entries.extend(code.function(qualifier.as_deref().unwrap_or(place), function));
            }
            break;
        }
    }
    entries
}

// ----- Lua -----

const ID: &str = "[A-Za-z_][A-Za-z0-9_]*";
fn lua_regex(pattern: &str) -> Regex {
    Regex::new(&pattern.replace("ID", ID)).unwrap()
}
static LUA_IMPORT: LazyLock<Regex> = LazyLock::new(|| lua_regex(r#"local (ID) = import\("([a-z_0-9]+)"\)"#));
static LUA_UNIT: LazyLock<Regex> = LazyLock::new(|| {
    lua_regex(r"^(?:local\s+)?(?:function\s+(ID(?:[.:]ID)?)\s*\(|(ID(?:\.ID)?)\s*=\s*function\s*\(|((?:M\.)?ID)\s*=\s*\{)")
});
static LUA_LOOP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^for .*\bdo\s*$").unwrap());
static LUA_FILLED: LazyLock<Regex> = LazyLock::new(|| lua_regex(r"(?m)^\s+(ID)\["));
static LUA_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| lua_regex(r"^(?:local\s+)?((?:M\.)?ID(?:\s*,\s*(?:M\.)?ID)*)\s*=\s*(.+)$"));
static LUA_NAME: LazyLock<Regex> = LazyLock::new(|| lua_regex(r"\b(ID)(?:\.(ID))?"));
static LUA_OFF_REQUEST: LazyLock<Regex> =
    LazyLock::new(|| lua_regex(r#"\b(?:input|params|source|args|request|given|asked)(?:\.(ID)|\["(ID)"\])"#));
static LUA_OFF_ANYTHING: LazyLock<Regex> = LazyLock::new(|| lua_regex(r#"\.(ID)|\["(ID)"\]"#));
static LUA_FIELD_TABLE: LazyLock<Regex> = LazyLock::new(|| lua_regex(r"\b(ID)\s*=\s*\{\s*[^}\s]"));
static LUA_REQUEST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:input|params|source|args|request|given|asked)\b").unwrap());
static LUA_NAME_IS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bname\s*=$").unwrap());
static LUA_STRING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([A-Za-z][A-Za-z0-9_]*)""#).unwrap());
static LUA_HELD: LazyLock<Regex> = LazyLock::new(|| lua_regex(r"^\s*(?:local\s+)?(ID)\s*=\s*(?:ID(?:\.ID)?\s*\(?\s*)?\{"));
static LUA_REUSED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"KINDS\["([a-z-]+)"\]"#).unwrap());

/// Whether the string at `at..end` names a field where it stands: a table's item, `name = "f"`, or an argument beside the
/// request itself (`need(input, "f")`). Not a comparison, a joined word, or a label beside what's read (`f(input.g, "g")`).
fn lua_names_a_field(text: &str, at: usize, end: usize) -> bool {
    let beside_the_request = |args: &str| LUA_REQUEST.find_iter(args).any(|word| !args[word.end()..].trim_start().starts_with(['.', '[']));
    let mut from = at.saturating_sub(2000);
    while !text.is_char_boundary(from) {
        from -= 1;
    }
    let before = text[from..at].trim_end();
    if text[end..].trim_start().starts_with("..") || ["..", "==", "~=", "<=", ">="].iter().any(|mark| before.ends_with(mark)) {
        return false;
    }
    if before.ends_with('=') {
        return LUA_NAME_IS.is_match(before);
    }
    let mut depth = 0;
    for (at, byte) in before.bytes().enumerate().rev() {
        match byte {
            b')' | b'}' => depth += 1,
            b'(' | b'{' if depth > 0 => depth -= 1,
            b'{' => return true,
            b'(' => {
                let close = text[end..].find(')').map_or(end, |n| end + n);
                return beside_the_request(&before[at + 1..]) || beside_the_request(&text[end..close]);
            }
            _ => {}
        }
    }
    false
}

/// Lua's reads: off the request, or anything inside it; a name in a list; a table of fields (`volume = { get = … }`).
fn lua_reads(text: &str) -> Reads {
    let mut reads = Reads::default();
    let not_a_call = |end: usize| !text[end..].trim_start().starts_with('(');
    for found in LUA_OFF_REQUEST.captures_iter(text) {
        if not_a_call(found.get(0).unwrap().end()) {
            reads.of_input.insert(found.get(1).or(found.get(2)).unwrap().as_str().to_string());
        }
    }
    for found in LUA_OFF_ANYTHING.captures_iter(text) {
        if not_a_call(found.get(0).unwrap().end()) {
            reads.anywhere.insert(found.get(1).or(found.get(2)).unwrap().as_str().to_string());
        }
    }
    for found in LUA_STRING.captures_iter(text) {
        let whole = found.get(0).unwrap();
        if lua_names_a_field(text, whole.start(), whole.end()) {
            reads.of_input.insert(found[1].to_string());
            reads.anywhere.insert(found[1].to_string());
        }
    }
    for found in LUA_FIELD_TABLE.captures_iter(text) {
        let start = found.get(0).unwrap().start();
        if char_before(text, start) != Some(b'.') && !text[..start].ends_with("local ") {
            reads.of_input.insert(found[1].to_string());
            reads.anywhere.insert(found[1].to_string());
        }
    }
    reads
}

#[derive(Default)]
struct LuaCode {
    modules: Vec<(String, Vec<String>)>,
    imports: HashMap<String, HashMap<String, String>>,
    /// Functions and tables at a module's top, by module and name.
    bodies: HashMap<Unit, Vec<String>>,
    /// What a module exports: its name there, and the unit that has it.
    exports: HashMap<String, HashMap<String, String>>,
    /// A local that's another module's export.
    aliases: HashMap<String, HashMap<String, (String, String)>>,
}

impl LuaCode {
    fn new(files: Vec<(String, String)>) -> Self {
        let mut code = Self::default();
        for (module, text) in files {
            let lines: Vec<String> = text.lines().filter(|line| !line.trim_start().starts_with("--")).map(str::to_string).collect();
            code.read_module(&module, &lines);
            code.modules.push((module, lines));
        }
        code
    }
    fn body(&mut self, module: &str, name: &str, text: String) {
        self.bodies.entry((module.to_string(), name.to_string())).or_default().push(text);
        if let Some(exported) = name.strip_prefix("M.") {
            self.exports.entry(module.to_string()).or_default().insert(exported.to_string(), name.to_string());
        }
    }
    fn read_module(&mut self, module: &str, lines: &[String]) {
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let imports: HashMap<String, String> =
            LUA_IMPORT.captures_iter(&lines.join("\n")).map(|found| (found[1].to_string(), found[2].to_string())).collect();
        let mut aliases: HashMap<String, (String, String)> = HashMap::new();
        let plain = |text: &str| !text.is_empty() && text.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
        for (at, line) in lines.iter().enumerate() {
            if let Some(found) = LUA_UNIT.captures(line) {
                let name = found.get(1).or(found.get(2)).or(found.get(3)).unwrap().as_str().replace(':', ".");
                self.body(module, &name, lines[at..=lua_block_end(&lines, at)].join("\n"));
            } else if LUA_LOOP.is_match(line) {
                // (a loop at the module's top that fills a table is part of that table)
                let block = lines[at..=lua_block_end(&lines, at)].join("\n");
                for filled in LUA_FILLED.captures_iter(&block) {
                    self.body(module, &filled[1], block.clone());
                }
            } else if let Some(found) = LUA_ASSIGNMENT.captures(line) {
                let left: Vec<&str> = found[1].split(',').map(str::trim).collect();
                let right: Vec<&str> = found[2].split(',').map(str::trim).collect();
                if left.len() != right.len() {
                    continue;
                }
                for (name, value) in left.into_iter().zip(right) {
                    if let Some(exported) = name.strip_prefix("M.").filter(|_| plain(value)) {
                        self.exports.entry(module.to_string()).or_default().insert(exported.to_string(), value.to_string());
                    } else if let Some((from, what)) =
                        value.split_once('.').filter(|(from, what)| !name.starts_with("M.") && plain(*from) && plain(*what))
                    {
                        let target = imports.get(from).cloned().or_else(|| aliases.get(from).map(|(module, _)| module.clone()));
                        if let Some(target) = target {
                            aliases.insert(name.to_string(), (target, what.to_string()));
                        }
                    }
                }
            }
        }
        self.imports.insert(module.to_string(), imports);
        self.aliases.insert(module.to_string(), aliases);
    }
    fn export(&self, module: &str, name: &str, hops: usize) -> Option<Unit> {
        let local = self.exports.get(module)?.get(name)?;
        if self.bodies.contains_key(&(module.to_string(), local.clone())) {
            return Some((module.to_string(), local.clone()));
        }
        let (from, what) = self.aliases.get(module)?.get(local)?;
        (hops < 4).then(|| self.export(from, what, hops + 1)).flatten()
    }
    /// Where the tool makes a kind: the table that names it (with the calls it's given to, on its first line), the lines naming
    /// the local it's kept in; or a `register("kind", …)`.
    fn entries(&self, kind: &str, known: &mut HashSet<String>) -> Vec<(String, String)> {
        let mut entries = vec![];
        if !known.insert(kind.to_string()) {
            return entries;
        }
        let (table, registered) = (format!("kind = \"{kind}\""), format!("register(\"{kind}\""));
        for (module, lines) in self.modules.iter().filter(|(module, _)| module.starts_with("changes")) {
            let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
            for (at, line) in lines.iter().enumerate() {
                if line.contains(&registered) {
                    entries.push((module.clone(), line.to_string()));
                }
                if !line.contains(&table) {
                    continue;
                }
                let open = opening_line(&lines, at);
                entries.push((module.clone(), lines[open..=lua_block_end(&lines, open)].join("\n")));
                if let Some(held) = LUA_HELD.captures(lines[open]) {
                    let named = Regex::new(&format!(r"(?:^|[^A-Za-z0-9_.]){}\b", regex::escape(&held[1]))).unwrap();
                    entries
                        .push((module.clone(), lines.iter().filter(|line| named.is_match(line)).copied().collect::<Vec<_>>().join("\n")));
                }
            }
        }
        entries
    }
}

impl Code for LuaCode {
    fn reads(&self, text: &str) -> Reads {
        lua_reads(text)
    }
    fn names(&self, module: &str, text: &str) -> Vec<Unit> {
        let mut out = vec![];
        let Some(imports) = self.imports.get(module) else { return out };
        let aliases = self.aliases.get(module);
        for found in LUA_NAME.captures_iter(text) {
            if matches!(char_before(text, found.get(0).unwrap().start()), Some(b'.' | b':')) {
                continue;
            }
            let first = &found[1];
            match found.get(2).map(|m| m.as_str()) {
                Some(second) if self.bodies.contains_key(&(module.to_string(), format!("{first}.{second}"))) => {
                    out.push((module.to_string(), format!("{first}.{second}")));
                }
                Some(second) if imports.contains_key(first) => out.extend(self.export(&imports[first], second, 0)),
                _ if self.bodies.contains_key(&(module.to_string(), first.to_string())) => {
                    out.push((module.to_string(), first.to_string()))
                }
                _ => out.extend(aliases.and_then(|aliases| aliases.get(first)).and_then(|(from, what)| self.export(from, what, 0))),
            }
        }
        out
    }
    fn texts(&self, unit: &Unit) -> Vec<&str> {
        self.bodies.get(unit).into_iter().flatten().map(String::as_str).collect()
    }
}

/// A kind's code in the tool, with the spec of another kind it reuses by name (`base.KINDS["device-param"]`).
fn renoise_scope(code: &LuaCode, memo: &Memo, kind: &str) -> Option<(Reads, BTreeSet<Unit>)> {
    let mut known = HashSet::new();
    let mut entries = code.entries(kind, &mut known);
    if entries.is_empty() {
        return None;
    }
    loop {
        let (reads, seen) = scope(code, memo, &entries);
        let reached = seen.iter().flat_map(|unit| code.texts(unit)).chain(entries.iter().map(|(_, text)| text.as_str()));
        let reused: BTreeSet<String> =
            reached.flat_map(|text| LUA_REUSED.captures_iter(text).map(|found| found[1].to_string()).collect::<Vec<_>>()).collect();
        let new: Vec<String> = reused.into_iter().filter(|other| !known.contains(other)).collect();
        if new.is_empty() {
            return Some((reads, seen));
        }
        for other in new {
            entries.extend(code.entries(&other, &mut known));
        }
    }
}

// ----- the bridges and the host, read -----

struct Bridges {
    reaper: RustCode,
    renoise: LuaCode,
    host: RustCode,
}

fn reaper_files() -> Vec<(String, String)> {
    folder(REAPER_CHANGES, "", ".rs")
}

fn renoise_files() -> Vec<(String, String)> {
    let mut files = folder(RENOISE_TOOL, "changes", ".lua");
    for file in ["actions", "bridge", "instruments"] {
        files.push((file.to_string(), source(&format!("{RENOISE_TOOL}/{file}.lua"))));
    }
    files
}

fn host_files() -> Vec<(String, String)> {
    ["renoise/presets", "renoise/instruments", "renoise/library_files", "preset_files", "changes"]
        .into_iter()
        .map(|file| (file.rsplit('/').next().unwrap().to_string(), source(&format!("{HOST}/{file}.rs"))))
        .collect()
}

impl Bridges {
    fn new(reaper: Vec<(String, String)>, renoise: Vec<(String, String)>) -> Self {
        Self { reaper: RustCode::new(reaper), renoise: LuaCode::new(renoise), host: RustCode::new(host_files()) }
    }
    /// What a kind's own code on its bridge reads, and where it looked; None when the walk can't find where the bridge makes it.
    fn kind_scope(&self, memo: &Memo, app: &str, kind: &str) -> Option<(Reads, BTreeSet<Unit>)> {
        if app != "REAPER" {
            return renoise_scope(&self.renoise, memo, kind);
        }
        let entries = reaper_entries(&self.reaper, kind);
        (!entries.is_empty()).then(|| scope(&self.reaper, memo, &entries))
    }
    fn host_reads(&self, memo: &Memo, functions: &[(&str, &str)]) -> Reads {
        let entries: Vec<(String, String)> = functions.iter().flat_map(|(module, function)| self.host.function(module, function)).collect();
        assert_eq!(entries.len(), functions.len(), "the host's {functions:?}");
        scope(&self.host, memo, &entries).0
    }
    /// What's offered and read nowhere it should be (of one kind, or all), in words a failure can say.
    fn unread_fields(&self, only: Option<&str>) -> Vec<String> {
        let (reaper_memo, renoise_memo, host_memo) = (Memo::default(), Memo::default(), Memo::default());
        let file_kinds: Vec<&str> = library_files::FILE_KINDS.iter().map(|spec| spec.kind).collect();
        let mut unread = vec![];
        for (app, set) in sets() {
            let memo = if app == "REAPER" { &reaper_memo } else { &renoise_memo };
            for kind in set.kinds.iter().filter(|kind| only.is_none_or(|only| only == kind.kind)) {
                let Some((reads, looked)) = self.kind_scope(memo, app, kind.kind) else {
                    unread.push(format!("{app}: can't find where its bridge makes {}", kind.kind));
                    continue;
                };
                let made = MADE_BY_THE_HOST.iter().find(|(made, _, _)| app == "Renoise" && *made == kind.kind);
                let by_host = made.map(|(_, functions, _)| self.host_reads(&host_memo, functions));
                for field in fields_of(kind.schema, kind.wire) {
                    let taken =
                        TAKEN_BY_THE_HOST.iter().find(|(daw, taken, name, _, _)| *daw == app && *taken == kind.kind && *name == field.path);
                    let from_a_file = app == "Renoise" && field.path == "path" && file_kinds.contains(&kind.kind);
                    let read = if let Some((_, _, _, function, _)) = taken {
                        self.host_reads(&host_memo, &[*function]).holds(&field.name, field.top)
                    } else if from_a_file {
                        self.host_reads(&host_memo, &[("library_files", "with_file")]).holds(&field.name, field.top)
                    } else if let (Some((_, _, tool_too)), Some(by_host)) = (made, &by_host) {
                        by_host.holds(&field.name, field.top) || (*tool_too && reads.holds(&field.name, field.top))
                    } else {
                        reads.holds(&field.name, field.top)
                    };
                    if !read {
                        let looked: Vec<String> = looked.iter().take(12).map(|(module, name)| format!("{module}::{name}")).collect();
                        unread.push(format!(
                            "{app}: {}'s {} is offered to the model, but its code doesn't read {} (looked in {}…): read it there, take it out of the schema, or, if the host takes it, list it in TAKEN_BY_THE_HOST",
                            kind.kind,
                            field.path,
                            field.name,
                            looked.join(", ")
                        ));
                    }
                }
            }
        }
        unread
    }
}

#[test]
fn each_kinds_own_code_reads_every_field_it_offers() {
    let unread = Bridges::new(reaper_files(), renoise_files()).unread_fields(None);
    assert!(unread.is_empty(), "{}", unread.join("\n"));
}

/// What the lists say stays true: each is a field its kind offers, and the host's list of the kinds it makes is ours.
#[test]
fn what_the_host_is_said_to_make_or_take_it_does() {
    let ours: BTreeSet<&str> = MADE_BY_THE_HOST.iter().map(|(kind, _, _)| *kind).collect();
    assert_eq!(ours, presets::KINDS.into_iter().collect::<BTreeSet<_>>(), "the kinds the host makes from a device's preset data");
    for (app, set) in sets() {
        for (_, kind, field, _, why) in TAKEN_BY_THE_HOST.iter().filter(|(daw, ..)| *daw == app) {
            let offered = set
                .kinds
                .iter()
                .find(|ours| ours.kind == *kind)
                .is_some_and(|ours| fields_of(ours.schema, ours.wire).iter().any(|offered| offered.path == *field));
            assert!(offered, "{app}'s {kind} doesn't offer {field}, which the host takes: {why}");
        }
    }
    assert!(TAKEN_BY_THE_HOST.iter().any(|(_, kind, _, _, _)| *kind == instruments::KIND), "the macro map's own name");
    for spec in library_files::FILE_KINDS.iter() {
        let kind = renoise_changes::CHANGES.kinds.iter().find(|ours| ours.kind == spec.kind).unwrap();
        assert!(fields_of(kind.schema, kind.wire).iter().any(|field| field.path == "path"), "{} takes a file by its path", spec.kind);
    }
}

/// The case this test is for: a bridge drops a kind's read, while other kinds, what it answers and its own words still name the
/// field. (`track-pan`'s `noun: "pan"` is a label, not a read.)
#[test]
fn dropping_a_kinds_read_of_a_field_fails() {
    let renamed = |mut files: Vec<(String, String)>, file: &str, read: &str, to: &str| {
        let (_, text) = files.iter_mut().find(|(name, _)| name == file).unwrap();
        assert!(text.contains(read), "{read} isn't how that field is read now: drop that here");
        *text = text.replace(read, to);
        files
    };
    for (kind, field, read, to) in
        [("track-volume", "volumeDb", ".get(\"volumeDb\")", ".get(\"db\")"), ("track-pan", "pan", ".get(\"pan\")", ".get(\"panning\")")]
    {
        let unread = Bridges::new(renamed(reaper_files(), "tracks", read, to), renoise_files()).unread_fields(Some(kind));
        assert!(unread.iter().any(|line| line.starts_with(&format!("REAPER: {kind}'s {field} is offered"))), "{unread:?}");
    }
    for (kind, field, read, to) in
        [("track-volume", "volumeDb", "input.volumeDb", "input.db"), ("track-pan", "pan", "input.pan", "input.panning")]
    {
        let unread = Bridges::new(reaper_files(), renamed(renoise_files(), "changes_tracks", read, to)).unread_fields(Some(kind));
        assert!(unread.iter().any(|line| line.starts_with(&format!("Renoise: {kind}'s {field} is offered"))), "{unread:?}");
    }
}

/// A read isn't a key of what's answered, nor one off what a change answered or found.
#[test]
fn what_a_change_answers_isnt_a_read() {
    let read = |text: &str| rust_reads(text).holds("level", true);
    for text in [
        r#"input.get("level")"#,
        r#"input["level"]"#,
        r#"params.get("level")"#,
        r#"moment(ctx, input, "level", "other")"#,
        r#"Field { key: "level", parse: flag }"#,
        r#"["other", "level"].iter()"#,
        r#"for field in ["other", "level"] {}"#,
        r#"[("level", 1, "words")]"#,
        r#"id("level")"#,
        r#"match name { "level" => 1, _ => 0 }"#,
    ] {
        assert!(read(text), "{text}");
    }
    for text in [
        r#"json!({"level": 1})"#,
        r#"json!({"kind": "level"})"#,
        r#"state["level"] = 1.into()"#,
        r#"to["level"]"#,
        r#"to.get("level")"#,
        r#"number(to, "level")"#,
        r#"map.insert("level".into(), 1)"#,
        r#"fields.remove("level")"#,
        r#"Prop { kind: "x", noun: "level" }"#,
        r#"name == "level""#,
        r#"fx_set_config(at.fx, "level", 1)"#,
        r#"Some("level")"#,
    ] {
        assert!(!read(text), "{text}");
    }
    assert!(rust_reads(r#"note.get("level")"#).holds("level", false), "inside a field, what holds it goes by any name");
    assert!(!rust_reads(r#"json!({"level": 1})"#).holds("level", false));
    let lua = |text: &str| lua_reads(text).holds("level", true);
    for text in [
        "local a = input.level",
        "local ORDER = { \"level\" }",
        "{ name = \"level\", read = at }",
        "need(input, \"level\")",
        "level = { get = at }",
    ] {
        assert!(lua(text), "{text}");
    }
    for text in [
        "local a = to.level",
        "return { level = 1 }",
        "local level = {}",
        "if kind == \"level\" then end",
        "{ kind = \"level\" }",
        "numbers_of(env, input.other, \"level\")",
        "bad(\"level\")",
    ] {
        assert!(!lua(text), "{text}");
    }
    assert!(lua_reads("local a = to.level").holds("level", false), "inside a field, the reader's name is its own");
}

/// Each action's fields reach the bridge (the host forwards only those it names) and its handler reads them. Kumi names an
/// action's refs `…Ref`; Renoise's actions read them under either name (actions.lua's `named_refs`).
#[test]
fn each_action_reads_every_field_it_offers() {
    let bridges = Bridges::new(reaper_files(), renoise_files());
    let mut unread = vec![];
    for (app, set, table) in [("REAPER", &reaper_changes::CHANGES, REAPER_ACTIONS), ("Renoise", &renoise_changes::CHANGES, RENOISE_ACTIONS)]
    {
        let code: &dyn Code = if app == "REAPER" { &bridges.reaper } else { &bridges.renoise };
        let memo = Memo::default();
        for (method, _, _) in table {
            assert!(set.actions.iter().any(|action| action.method == *method), "{app}: no action calls {method} now: take its row out");
        }
        for action in set.actions {
            let Some((_, (module, function), forwarded_by)) = table.iter().find(|(method, _, _)| *method == action.method) else {
                unread.push(format!("{app}: which function answers {}? Add it to the test's list", action.method));
                continue;
            };
            let handler = (module.to_string(), function.to_string());
            let entries: Vec<(String, String)> =
                code.texts(&handler).into_iter().map(|text| (module.to_string(), text.to_string())).collect();
            assert!(!entries.is_empty(), "{app}'s bridge has no {module}::{function}");
            let (reads, _) = scope(code, &memo, &entries);
            let forwarded = forwarded_by.map(|function| forwarded_fields(&bridges.host, function, action.method));
            for field in fields_of(action.schema, &[]) {
                if forwarded.as_ref().is_some_and(|names| field.top && !names.contains(&field.name)) {
                    unread.push(format!("{app}: the host doesn't pass {}'s {} on to the bridge", action.method, field.name));
                }
                let plain = field.name.strip_suffix("Ref").filter(|_| app == "Renoise").unwrap_or(&field.name);
                if !reads.holds(&field.name, field.top) && !reads.holds(plain, field.top) {
                    unread
                        .push(format!("{app}: {}'s {} is offered to the model, but its handler doesn't read it", action.tool, field.path));
                }
            }
        }
    }
    assert!(unread.is_empty(), "{}", unread.join("\n"));
}

/// The fields a host function forwards for a method: the method's own list, when it has one (`"select" => &[…]`).
fn forwarded_fields(host: &RustCode, function: &str, method: &str) -> HashSet<String> {
    let bodies = host.units.get(function).unwrap_or_else(|| panic!("the host has no {function}"));
    let mut names = HashSet::new();
    for (_, body) in bodies {
        let own = body.find(&format!("\"{method}\" => &[")).map(|at| {
            let start = at + method.len() + 2;
            &body[start..body[start..].find(']').map_or(body.len(), |n| start + n)]
        });
        names.extend(STRING.captures_iter(own.unwrap_or(body)).map(|found| found[1].to_string()));
    }
    names
}

/// A simulator reads only what its DAW offers: else what passes against it couldn't happen against the DAW.
#[test]
fn what_a_simulator_reads_of_a_changes_input_its_daw_offers() {
    let read = Regex::new(r#"\["input"\]\["([A-Za-z]+)"\]|\binput\["([A-Za-z]+)"\]|\binput\s*\.\s*get\("([A-Za-z]+)"\)"#).unwrap();
    for (app, set) in sets() {
        let mut offered = HashSet::new();
        for kind in set.kinds {
            offered.extend(fields_of(kind.schema, kind.wire).into_iter().map(|field| field.name));
        }
        for action in set.actions {
            offered.extend(fields_of(action.schema, &[]).into_iter().map(|field| field.name));
        }
        let (_, files) = SIMULATORS.iter().find(|(daw, _)| *daw == app).unwrap();
        for file in *files {
            let text = rust_lines(&source(&format!("{HOST}/{file}"))).join("\n");
            for found in read.captures_iter(&text) {
                let name = found.iter().skip(1).flatten().next().unwrap().as_str();
                assert!(offered.contains(name), "{app}'s simulator ({file}) reads {name}, which no kind or action offers");
            }
        }
    }
}
