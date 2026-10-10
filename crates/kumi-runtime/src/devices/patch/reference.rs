//! What Kumi knows of each of Max's objects, learned on this machine from the installed Max so it keeps up as Max
//! changes: each object's inlets and outlets for the arguments it's given, fitted to the instances saved in Max's own
//! help patchers, what the object and each inlet and outlet are for (its reference page's digests), and the messages
//! that only set something in it (its attributes, and the methods its page says send nothing). Max's bundled packages
//! count (not Gen's or RNBO's pages, whose operators share names with Max's objects), and so do the abstractions Max
//! ships. It's learned once for each Max and kept in Kumi's folder; none of Max's files are carried in Kumi.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::catalog::catalog;

/// How many of an object's inlets or outlets there are, for the arguments it's given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Count {
    /// Always the same.
    Fixed { count: usize },
    /// One for each argument and `plus` more; `bare` without arguments (pack, unpack, trigger, route).
    Arguments { plus: usize, bare: usize },
    /// The first argument's value and `plus` more; `bare` without one (gate's outlets, switch's inlets).
    FirstArgument { plus: usize, bare: usize },
    /// One for each format in the arguments (sprintf's %d, %s); `bare` without any.
    Formats { bare: usize },
    /// As many as the highest numbered variable in the arguments (expr's $i2, $f3); `bare` without any.
    Variables { bare: usize },
}

impl Default for Count {
    fn default() -> Count {
        Count::Fixed { count: 0 }
    }
}

impl Count {
    pub fn of(&self, args: &[String]) -> usize {
        match *self {
            Count::Fixed { count } => count,
            Count::Arguments { plus, bare } => {
                if args.is_empty() {
                    bare
                } else {
                    args.len() + plus
                }
            }
            // A count of none ([gate 0]) is Max's default.
            Count::FirstArgument { plus, bare } => match args.first().and_then(|arg| arg.parse::<f64>().ok()) {
                Some(value) if value >= 1.0 && value.fract() == 0.0 && value <= 512.0 => value as usize + plus,
                _ => bare,
            },
            Count::Formats { bare } => Some(formats(args)).filter(|count| *count > 0).unwrap_or(bare),
            Count::Variables { bare } => Some(variables(args)).filter(|count| *count > 0).unwrap_or(bare),
        }
    }
}

/// How many formats the arguments hold (%d, %s…; %% is a percent sign).
fn formats(args: &[String]) -> usize {
    let mut count = 0;
    for arg in args {
        let mut chars = arg.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%' {
                if chars.peek() == Some(&'%') {
                    chars.next();
                } else {
                    count += 1;
                }
            }
        }
    }
    count
}

static VARIABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$[ifsx](\d+)").unwrap());

/// The highest numbered variable the arguments use ($i1, $f2…), 0 without any.
fn variables(args: &[String]) -> usize {
    args.iter()
        .flat_map(|arg| VARIABLE.captures_iter(arg).filter_map(|found| found[1].parse::<usize>().ok()).collect::<Vec<_>>())
        .max()
        .unwrap_or(0)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Object {
    /// max, msp, jit or m4l ("" for one only the help patchers have).
    pub module: String,
    pub digest: String,
    pub inlets: Count,
    pub outlets: Count,
    /// What each outlet sends, as Max lists it for the instance seen most ("signal", "bang", "" for anything).
    pub outlet_types: Vec<String>,
    pub inlet_digests: Vec<String>,
    pub outlet_digests: Vec<String>,
    /// How many saved instances the counts were fitted to (0: the reference page's alone).
    pub seen: usize,
    /// The messages that only set something in it, sending nothing at once: its attributes, and the methods its page
    /// says send nothing ("Set the value with no output") or that set a thing without a word of sending.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub quiet: BTreeSet<String>,
}

/// The page of the attributes every box has (hidden, presentation_rect, varname…).
const BOX_PAGE: &str = "jbox";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    /// What it was learned from, so a different Max is learned anew.
    pub fingerprint: String,
    pub objects: BTreeMap<String, Object>,
}

/// An object's inlets and outlets for the arguments it's given.
#[derive(Debug, Clone, PartialEq)]
pub struct Ports {
    pub inlets: usize,
    pub outlets: usize,
    pub outlet_types: Vec<String>,
}

impl Reference {
    /// An object by its class, or the class an alias stands for ("t" is trigger).
    pub fn object(&self, class: &str) -> Option<&Object> {
        self.objects.get(class).or_else(|| self.objects.get(catalog().canonical(class)))
    }

    /// An object's ports for a box's text ("route a b c"): None for a class the reference doesn't know, or one whose
    /// ports come from what's inside it (a subpatcher, gen~, code).
    pub fn ports(&self, text: &str) -> Option<Ports> {
        let words = atoms(text);
        let (class, rest) = words.split_first()?;
        if catalog().ports_from_contents(class) {
            return None;
        }
        let object = self.object(class)?;
        let args: Vec<String> = rest.iter().take_while(|word| !word.starts_with('@')).cloned().collect();
        let (inlets, outlets) = (object.inlets.of(&args), object.outlets.of(&args));
        let filler = object.outlet_types.last().cloned().unwrap_or_default();
        let outlet_types = (0..outlets).map(|at| object.outlet_types.get(at).cloned().unwrap_or_else(|| filler.clone())).collect();
        Some(Ports { inlets, outlets, outlet_types })
    }

    /// Whether the message `selector` only sets something in an object of `class` (one of its attributes or every
    /// box's, or a method its page says sends nothing), so the object sends nothing at once.
    pub fn quiet(&self, class: &str, selector: &str) -> bool {
        let lists = |object: Option<&Object>| object.is_some_and(|object| object.quiet.contains(selector));
        lists(self.object(class)) || lists(self.objects.get(BOX_PAGE))
    }
}

/// A box's text as Max reads it into atoms: words, a quoted symbol one atom.
pub fn atoms(text: &str) -> Vec<String> {
    let mut atoms = Vec::new();
    let mut atom = String::new();
    let (mut quoted, mut escaped, mut started) = (false, false, false);
    for c in text.chars() {
        if escaped {
            atom.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
            started = true;
        } else if c == '"' {
            quoted = !quoted;
            started = true;
        } else if c.is_whitespace() && !quoted {
            if started {
                atoms.push(std::mem::take(&mut atom));
                started = false;
            }
        } else {
            atom.push(c);
            started = true;
        }
    }
    if started {
        atoms.push(atom);
    }
    atoms
}

/// An instance saved in a help patcher: its arguments, inlets, outlets and what its outlets send, and the version of
/// Max that saved it.
#[derive(Debug, Clone)]
struct Instance {
    args: Vec<String>,
    inlets: usize,
    outlets: usize,
    outlet_types: Vec<String>,
    version: [u64; 3],
}

/// The version of Max that saved a patcher (its appversion), [0, 0, 0] when it doesn't say.
fn saved_by(patcher: &Value) -> [u64; 3] {
    let part = |key: &str| patcher["appversion"][key].as_u64().unwrap_or(0);
    [part("major"), part("minor"), part("revision")]
}

/// The instances that say what Max does now: of those with the same arguments, the ones the newest Max saved. A help
/// patcher saved before an object gained an outlet keeps the count it had then ([midiparse] gained its eighth).
fn newest(instances: Vec<Instance>) -> Vec<Instance> {
    let mut latest: HashMap<Vec<String>, [u64; 3]> = HashMap::new();
    for instance in &instances {
        let version = latest.entry(instance.args.clone()).or_default();
        *version = (*version).max(instance.version);
    }
    instances.into_iter().filter(|instance| latest[&instance.args] == instance.version).collect()
}

static OBJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<c74object\s+name="([^"]+)"([^>]*)>"#).unwrap());
static MODULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"module="([^"]*)""#).unwrap());
static DIGEST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<digest>(.*?)</digest>").unwrap());
static INLETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<inletlist>(.*?)</inletlist>").unwrap());
static OUTLETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<outletlist>(.*?)</outletlist>").unwrap());
static PORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<(?:inlet|outlet)\s+id="(\d+)"(?:\s+type="([^"]*)")?[^>]*>\s*<digest>(.*?)</digest>"#).unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static METHOD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<method\s+name="([^"]+)"[^>]*>(.*?)</method>"#).unwrap());
/// An attribute's opening tag (self-closing for one without parts) or its closing tag: an attribute's own attributes
/// (its label, its category) are written inside it.
static ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<attribute\s+name="([^"]+)"[^>]*?(/?)>|</attribute>"#).unwrap());
/// How a page says a method sends nothing: "with no output", "without triggering output", "do not output".
static SENDS_NOTHING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(without|no|not)\b[^.]*\boutput").unwrap());
/// How a page says a method sends: "Output the value", "Set the value and cause output", "Set both values, output…".
static SENDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(output|send|report|trigger|bang|dump)\b|\b(cause|causes|causing|trigger|triggers|and|or|then)\s+(output|send|sends|report|reports)\b|,\s*(output|send|sends|report|reports)\b").unwrap()
});
/// How a page says a method sets something: "Set the maximum value", "Replace the stored message", "Turn off polling".
static SETS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(set|sets|replace|replaces|change|changes|store|stores|clear|clears|define|defines|enable|enables|disable|disables|turn|reset|resets|initialize|initializes|add|adds|insert|inserts|append|appends|prepend|prepends|remove|removes|delete|deletes)\b").unwrap()
});
/// The messages that carry what an object works on: whatever their page says, they're its reason to send.
const DATA: [&str; 6] = ["bang", "int", "float", "list", "anything", "symbol"];

/// The messages a page says only set something: its attributes (not an attribute's own), and its methods that send
/// nothing; a method that sends, by an attribute's name, isn't quiet.
fn quiet_messages(text: &str) -> BTreeSet<String> {
    let mut quiet = BTreeSet::new();
    let mut depth = 0usize;
    for tag in ATTRIBUTE.captures_iter(text) {
        match tag.get(1) {
            Some(name) => {
                if depth == 0 {
                    quiet.insert(name.as_str().to_string());
                }
                if &tag[2] != "/" {
                    depth += 1;
                }
            }
            None => depth = depth.saturating_sub(1),
        }
    }
    for method in METHOD.captures_iter(text) {
        let name = &method[1];
        if name.starts_with('(') || DATA.contains(&name) {
            continue;
        }
        let digest = DIGEST.captures(&method[2]).map(|found| plain(&found[1])).unwrap_or_default();
        if SENDS_NOTHING.is_match(&digest) || (!SENDS.is_match(&digest) && SETS.is_match(&digest)) {
            quiet.insert(name.to_string());
        } else {
            quiet.remove(name);
        }
    }
    quiet
}

/// A reference page's text as plain words: tags out, entities read, spaces collapsed.
fn plain(text: &str) -> String {
    let text = TAG.replace_all(text, "");
    let text = text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An object from its reference page (a .maxref.xml): its class and what the page says of it.
fn read_page(text: &str) -> Option<(String, Object)> {
    let head = OBJECT.captures(text)?;
    let name = head[1].to_string();
    let module = MODULE.captures(&head[2]).map(|found| found[1].to_string()).unwrap_or_default();
    let digest = DIGEST.captures(text).map(|found| plain(&found[1])).unwrap_or_default();
    let ports = |list: &Regex| -> Vec<(String, String)> {
        list.captures(text)
            .map(|found| {
                PORT.captures_iter(&found[1])
                    .map(|port| {
                        let kind = port.get(2).map_or("", |kind| kind.as_str());
                        // A page's placeholder (INLET_TYPE) says nothing about what the port takes.
                        let kind = if kind.contains("_TYPE") { "" } else { kind };
                        (kind.to_string(), plain(&port[3]))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let (inlets, outlets) = (ports(&INLETS), ports(&OUTLETS));
    let outlet_types =
        outlets.iter().map(|(kind, _)| if kind.starts_with("signal") { "signal".to_string() } else { String::new() }).collect();
    let object = Object {
        module,
        digest,
        inlets: Count::Fixed { count: inlets.len() },
        outlets: Count::Fixed { count: outlets.len() },
        outlet_types,
        inlet_digests: inlets.into_iter().map(|(_, digest)| digest).collect(),
        outlet_digests: outlets.into_iter().map(|(_, digest)| digest).collect(),
        seen: 0,
        quiet: quiet_messages(text),
    };
    Some((name, object))
}

/// The instances of Max's objects saved in a help patcher (by Max `version`) and the patchers inside it (not gen's or
/// RNBO's, whose operators share names with Max's objects).
fn instances(patcher: &Value, version: [u64; 3], found: &mut HashMap<String, Vec<Instance>>) {
    let space = patcher.get("classnamespace").and_then(Value::as_str).unwrap_or("box");
    if space != "box" {
        return;
    }
    for entry in patcher.get("boxes").and_then(Value::as_array).into_iter().flatten() {
        let item = &entry["box"];
        if let (Some("newobj"), Some(text)) = (item["maxclass"].as_str(), item["text"].as_str()) {
            let words = atoms(text);
            if let (Some((class, rest)), Some(inlets), Some(outlets)) =
                (words.split_first(), item["numinlets"].as_u64(), item["numoutlets"].as_u64())
            {
                let args = rest.iter().take_while(|word| !word.starts_with('@')).cloned().collect();
                let outlet_types =
                    item["outlettype"].as_array().into_iter().flatten().map(|kind| kind.as_str().unwrap_or("").to_string()).collect();
                found.entry(class.clone()).or_default().push(Instance {
                    args,
                    inlets: inlets as usize,
                    outlets: outlets as usize,
                    outlet_types,
                    version,
                });
            }
        }
        if item["patcher"].is_object() {
            instances(&item["patcher"], version, found);
        }
    }
}

/// The most common value (the smallest of those tied).
fn mode(values: impl Iterator<Item = usize>) -> Option<usize> {
    let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
    for value in values {
        *counts.entry(value).or_default() += 1;
    }
    counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(value, _)| value)
}

/// The count that explains the most instances: the same for every instance, one for each argument, or the first
/// argument's value (in that order when they explain as many). `page` is the reference page's count, for an object
/// never seen without arguments.
fn fit(instances: &[Instance], value: impl Fn(&Instance) -> usize, page: usize) -> Count {
    let fixed = Count::Fixed { count: mode(instances.iter().map(&value)).unwrap_or(page) };
    let bare = mode(instances.iter().filter(|instance| instance.args.is_empty()).map(&value)).unwrap_or(page);
    let mut candidates = vec![fixed];
    let with_args = instances.iter().filter(|instance| !instance.args.is_empty());
    if let Some(plus) = mode(with_args.clone().filter_map(|instance| value(instance).checked_sub(instance.args.len()))) {
        candidates.push(Count::Arguments { plus, bare });
    }
    let first = |instance: &Instance| {
        instance.args.first().and_then(|arg| arg.parse::<f64>().ok()).filter(|value| value.fract() == 0.0 && *value >= 0.0)
    };
    if let Some(plus) = mode(with_args.filter_map(|instance| value(instance).checked_sub(first(instance)? as usize))) {
        candidates.push(Count::FirstArgument { plus, bare });
    }
    // Text that sets the count by what it says (sprintf's formats, expr's variables): only where some instance has it.
    for (feature, make) in [(formats as fn(&[String]) -> usize, Count::Formats { bare: 0 }), (variables, Count::Variables { bare: 0 })] {
        if instances.iter().any(|instance| feature(&instance.args) > 0) {
            let bare = mode(instances.iter().filter(|instance| feature(&instance.args) == 0).map(&value)).unwrap_or(page);
            candidates.push(match make {
                Count::Formats { .. } => Count::Formats { bare },
                _ => Count::Variables { bare },
            });
        }
    }
    let explains = |count: &Count| instances.iter().filter(|instance| count.of(&instance.args) == value(instance)).count();
    let mut best = candidates[0];
    for candidate in candidates.into_iter().skip(1) {
        if explains(&candidate) > explains(&best) {
            best = candidate;
        }
    }
    best
}

/// Every file under a folder with one of the extensions, in order.
fn files(folder: &Path, extension: &str, found: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(folder).into_iter().flatten().flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for entry in entries {
        if entry.is_dir() {
            files(&entry, extension, found);
        } else if entry.to_string_lossy().ends_with(extension) {
            found.push(entry);
        }
    }
}

/// Packages whose reference pages describe their own operators (gen's, RNBO's), not Max's objects.
const OTHER_LANGUAGES: [&str; 3] = ["Gen", "RNBO", "ableton-dsp"];

/// What the reference is learned from in a Max's C74 folder: its reference pages, help patchers and abstractions, and
/// those of its bundled packages.
struct Sources {
    pages: Vec<PathBuf>,
    help: Vec<PathBuf>,
    abstractions: Vec<PathBuf>,
}

fn sources(c74: &Path) -> Sources {
    let mut found = Sources { pages: Vec::new(), help: Vec::new(), abstractions: Vec::new() };
    let mut packages: Vec<PathBuf> =
        std::fs::read_dir(c74.join("packages")).into_iter().flatten().flatten().map(|entry| entry.path()).collect();
    packages.sort();
    for root in std::iter::once(c74.to_path_buf()).chain(packages) {
        let other = root.file_name().is_some_and(|name| OTHER_LANGUAGES.iter().any(|other| name == *other));
        if !other {
            files(&root.join("docs").join("refpages"), ".maxref.xml", &mut found.pages);
        }
        files(&root.join("help"), ".maxhelp", &mut found.help);
        files(&root.join("patchers"), ".maxpat", &mut found.abstractions);
    }
    found
}

/// How Kumi learns the reference: a new way is learned anew, as a new Max is.
const LEARNER: u32 = 2;

/// What the reference is learned from in a Max's C74 folder, and how, as one line: a new Max reads differently.
pub fn fingerprint(c74: &Path) -> String {
    let found = sources(c74);
    let all = found.pages.iter().chain(&found.help).chain(&found.abstractions);
    let bytes: u64 = all.filter_map(|file| std::fs::metadata(file).ok()).map(|meta| meta.len()).sum();
    format!(
        "learner {LEARNER}: {} pages, {} help patchers, {} abstractions, {bytes} bytes",
        found.pages.len(),
        found.help.len(),
        found.abstractions.len()
    )
}

/// An object only a help patcher or an abstraction tells of: no reference page.
fn unpaged(module: &str, inlets: usize, outlets: usize) -> Object {
    Object {
        module: module.to_string(),
        digest: String::new(),
        inlets: Count::Fixed { count: inlets },
        outlets: Count::Fixed { count: outlets },
        outlet_types: vec![String::new(); outlets],
        inlet_digests: Vec::new(),
        outlet_digests: Vec::new(),
        seen: 0,
        quiet: BTreeSet::new(),
    }
}

/// The reference learned from a Max's C74 folder: its reference pages, its help patchers' saved instances, then the
/// abstractions it ships (their inlets and outlets are their inlet and outlet objects).
pub fn learn(c74: &Path) -> Reference {
    let found = sources(c74);
    let mut objects: BTreeMap<String, Object> = BTreeMap::new();
    for page in &found.pages {
        if let Some((name, object)) = std::fs::read_to_string(page).ok().as_deref().and_then(read_page) {
            objects.entry(name).or_insert(object);
        }
    }
    let mut seen: HashMap<String, Vec<Instance>> = HashMap::new();
    for file in &found.help {
        if let Some(document) = std::fs::read(file).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()) {
            instances(&document["patcher"], saved_by(&document["patcher"]), &mut seen);
        }
    }
    // An alias (t) starts from what's learned of the class it stands for (trigger), so it comes after it.
    let mut seen: Vec<(String, Vec<Instance>)> = seen.into_iter().collect();
    seen.sort_by_key(|(class, _)| (catalog().canonical(class) != class, class.clone()));
    for (class, instances) in seen {
        let instances = newest(instances);
        let start = objects.get(catalog().canonical(&class)).cloned().unwrap_or_else(|| unpaged("", 0, 0));
        let object = objects.entry(class).or_insert(start);
        // What's known for no arguments (the page's count, or an alias's class's), for an object never seen without.
        let page = |count: Count| count.of(&[]);
        object.inlets = fit(&instances, |instance| instance.inlets, page(object.inlets));
        object.outlets = fit(&instances, |instance| instance.outlets, page(object.outlets));
        // What the outlets send: as the fewest-argument instance seen most lists them.
        let fewest = instances.iter().map(|instance| instance.args.len()).min().unwrap_or(0);
        let mut lists: BTreeMap<Vec<String>, usize> = BTreeMap::new();
        for instance in instances.iter().filter(|instance| instance.args.len() == fewest) {
            *lists.entry(instance.outlet_types.clone()).or_default() += 1;
        }
        if let Some((types, _)) = lists.into_iter().max_by_key(|(_, count)| *count) {
            object.outlet_types = types;
        }
        object.seen = instances.len();
    }
    for file in &found.abstractions {
        let Some(name) = file.file_stem().map(|stem| stem.to_string_lossy().into_owned()) else { continue };
        if objects.contains_key(&name) {
            continue;
        }
        let Some(document) = std::fs::read(file).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()) else { continue };
        let boxes = document["patcher"]["boxes"].as_array().cloned().unwrap_or_default();
        let count = |maxclass: &str| boxes.iter().filter(|entry| entry["box"]["maxclass"] == maxclass).count();
        objects.insert(name, unpaged("abstraction", count("inlet"), count("outlet")));
    }
    Reference { fingerprint: fingerprint(c74), objects }
}

/// The installed Max's C74 folder: Live's own Max (the newest Live first), or a Max installed by itself.
pub fn find_c74() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut named = |folder: PathBuf, prefix: &str, then: &[&str]| {
        let mut names: Vec<String> = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names.into_iter().rev().filter(|name| name.starts_with(prefix)) {
            roots.push(then.iter().fold(folder.join(&name), |path, part| path.join(part)));
        }
    };
    if cfg!(target_os = "macos") {
        named(PathBuf::from("/Applications"), "Ableton Live", &["Contents", "App-Resources", "Max"]);
        named(PathBuf::from("/Applications"), "Max", &[]);
    } else if cfg!(windows) {
        let program_data = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
        named(PathBuf::from(program_data).join("Ableton"), "Live", &["Resources", "Max"]);
        let program_files = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
        named(PathBuf::from(&program_files).join("Ableton"), "Live", &["Resources", "Max"]);
        named(PathBuf::from(program_files).join("Cycling '74"), "Max", &[]);
    }
    roots.into_iter().find_map(|root| c74_within(&root, 4))
}

/// A folder named C74 holding Max's reference pages, within `depth` folders of `root`.
fn c74_within(root: &Path, depth: usize) -> Option<PathBuf> {
    if root.file_name().is_some_and(|name| name == "C74") && root.join("docs").join("refpages").is_dir() {
        return Some(root.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    let mut entries: Vec<PathBuf> =
        std::fs::read_dir(root).ok()?.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
    entries.sort();
    entries.into_iter().find_map(|entry| c74_within(&entry, depth - 1))
}

static INSTALLED: LazyLock<Option<Reference>> = LazyLock::new(|| {
    let c74 = find_c74()?;
    let print = fingerprint(&c74);
    let folder =
        std::env::var("KUMI_HOME").map(PathBuf::from).unwrap_or_else(|_| home::home_dir().unwrap_or_default().join(".kumi")).join("max");
    let file = folder.join("reference.json");
    if let Some(kept) = std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice::<Reference>(&bytes).ok()) {
        if kept.fingerprint == print {
            return Some(kept);
        }
    }
    let learned = learn(&c74);
    if std::fs::create_dir_all(&folder).is_ok() {
        let _ = std::fs::write(&file, serde_json::to_vec(&learned).unwrap_or_default());
    }
    Some(learned)
});

/// The installed Max's reference, learned on first use (a second or two) and kept until Max changes; None without Max.
pub fn installed() -> Option<&'static Reference> {
    INSTALLED.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn instance(args: &[&str], inlets: usize, outlets: usize) -> Instance {
        Instance { args: args.iter().map(|arg| arg.to_string()).collect(), inlets, outlets, outlet_types: vec![], version: [9, 0, 0] }
    }

    #[test]
    fn counts_are_fitted_to_the_instances_saved_in_maxs_help() {
        let pack =
            [instance(&[], 2, 1), instance(&["0", "0"], 2, 1), instance(&["0", "0", "0"], 3, 1), instance(&["0", "0", "0", "0"], 4, 1)];
        assert_eq!(fit(&pack, |i| i.inlets, 2), Count::Arguments { plus: 0, bare: 2 });
        assert_eq!(fit(&pack, |i| i.outlets, 1), Count::Fixed { count: 1 });
        let gate =
            [instance(&[], 2, 1), instance(&["2"], 2, 2), instance(&["4"], 2, 4), instance(&["8", "1"], 2, 8), instance(&["0"], 2, 1)];
        assert_eq!(
            fit(&gate, |i| i.outlets, 1),
            Count::FirstArgument { plus: 0, bare: 1 },
            "gate's outlets are its first argument's value"
        );
        assert_eq!(Count::FirstArgument { plus: 0, bare: 1 }.of(&["0".to_string()]), 1, "[gate 0] has Max's default");
        let midiparse = newest(vec![
            instance(&[], 1, 7),
            Instance { version: [9, 1, 0], ..instance(&[], 1, 8) },
            Instance { version: [7, 0, 3], ..instance(&["x"], 1, 7) },
        ]);
        assert_eq!(
            midiparse.iter().map(|i| i.outlets).collect::<Vec<_>>(),
            [8, 7],
            "the newest save of the same text says what Max does now; a text saved once stays"
        );
        let route = [instance(&["a"], 2, 2), instance(&["a", "b"], 3, 3), instance(&["a", "b", "c"], 4, 4)];
        assert_eq!(fit(&route, |i| i.outlets, 2), Count::Arguments { plus: 1, bare: 2 }, "the page's count when never seen bare");
        let plus = [instance(&[], 2, 1), instance(&["1"], 2, 1), instance(&["0.5"], 2, 1)];
        assert_eq!(
            fit(&plus, |i| i.inlets, 2),
            Count::Fixed { count: 2 },
            "the same for every instance: fixed, though one-per-argument fits too"
        );
        let sprintf = [
            instance(&["%s/%s"], 2, 1),
            instance(&["symout", "%d", "%%", "%s"], 2, 1),
            instance(&["%ld"], 1, 1),
            instance(&["set", "%s"], 1, 1),
        ];
        assert_eq!(fit(&sprintf, |i| i.inlets, 1), Count::Formats { bare: 1 }, "sprintf: an inlet for each format");
        let expr = [
            instance(&["$i1", "+", "$i2"], 2, 1),
            instance(&["$f1*2"], 1, 1),
            instance(&["$f3", "-", "$f1"], 3, 1),
            instance(&["1"], 1, 1),
        ];
        assert_eq!(fit(&expr, |i| i.inlets, 1), Count::Variables { bare: 1 }, "expr: as many inlets as its highest variable");
    }

    #[test]
    fn a_reference_page_gives_the_digests_and_a_plain_instances_ports() {
        let page = r#"<?xml version="1.0"?><c74object name="cycle~" module="msp" category="MSP Synthesis">
            <digest> Sinusoidal <o>oscillator</o> </digest><description>Use it.</description>
            <inletlist><inlet id="0" type="signal/float"><digest>Frequency</digest></inlet><inlet id="1" type="signal/float"><digest>Phase (0-1)</digest></inlet></inletlist>
            <outletlist><outlet id="0" type="signal"><digest>Output</digest></outlet></outletlist></c74object>"#;
        let (name, object) = read_page(page).unwrap();
        assert_eq!((name.as_str(), object.module.as_str(), object.digest.as_str()), ("cycle~", "msp", "Sinusoidal oscillator"));
        assert_eq!(object.inlet_digests, ["Frequency", "Phase (0-1)"]);
        assert_eq!((object.inlets, object.outlets), (Count::Fixed { count: 2 }, Count::Fixed { count: 1 }));
        assert_eq!(object.outlet_types, ["signal"]);
    }

    #[test]
    fn a_page_says_which_messages_only_set_something() {
        let page = r#"<c74object name="counter" module="max">
            <methodlist>
              <method name="int"><digest>Set the counter value with no output</digest></method>
              <method name="set"><digest>Set the counter value with no output</digest></method>
              <method name="jam"><digest>Set the counter value and cause output</digest></method>
              <method name="max"><digest>Set the maximum value</digest></method>
              <method name="min"><digest>Set the minimum value, cause output</digest></method>
              <method name="next"><digest>Output next count value</digest></method>
              <method name="carryflag"><digest>Send carry output now</digest></method>
              <method name="poll"><digest>Set output on mouse movement</digest></method>
              <method name="(mouse)"><digest>Click it</digest></method>
            </methodlist>
            <attributelist>
              <attribute name="carryflag" get="1" set="1" type="int" size="1"><digest>Carry flag</digest>
                <attributelist><attribute name="label" get="1" set="1" type="symbol" size="1" value="Carry" /></attributelist>
              </attribute>
              <attribute name="compatmode" get="1" set="1" type="int" size="1" />
            </attributelist></c74object>"#;
        let (_, object) = read_page(page).unwrap();
        assert_eq!(
            object.quiet.iter().map(String::as_str).collect::<Vec<_>>(),
            ["compatmode", "max", "poll", "set"],
            "attributes (not an attribute's own), and methods that send nothing; a number always counts"
        );
        let mut reference = Reference::default();
        reference.objects.insert("counter".into(), object);
        reference.objects.insert(BOX_PAGE.into(), Object { quiet: BTreeSet::from(["hidden".to_string()]), ..unpaged("max", 0, 0) });
        assert!(reference.quiet("counter", "set") && reference.quiet("counter", "hidden") && !reference.quiet("counter", "jam"));
        assert!(reference.quiet("prepend", "hidden") && !reference.quiet("prepend", "set"), "every box's attributes, nothing else");
    }

    #[test]
    fn a_boxs_ports_follow_its_arguments_and_gen_patchers_are_left_out() {
        let mut found = HashMap::new();
        instances(
            &json!({ "boxes": [
                { "box": { "maxclass": "newobj", "text": "route a \"b c\" @prefix x", "numinlets": 3, "numoutlets": 3, "outlettype": ["", "", ""] } },
                { "box": { "maxclass": "newobj", "text": "gen~", "numinlets": 1, "numoutlets": 1, "patcher": { "classnamespace": "dsp.gen",
                    "boxes": [{ "box": { "maxclass": "newobj", "text": "+ 1", "numinlets": 1, "numoutlets": 1 } }] } } }
            ] }),
            [9, 0, 0],
            &mut found,
        );
        assert_eq!(found["route"][0].args, ["a", "b c"], "a quoted symbol is one argument, attributes aren't arguments");
        assert!(!found.contains_key("+"), "gen's operators aren't Max's objects");
        let mut reference = Reference::default();
        reference.objects.insert(
            "route".into(),
            Object {
                module: "max".into(),
                digest: "Select outlet based on input matching".into(),
                inlets: Count::Arguments { plus: 1, bare: 2 },
                outlets: Count::Arguments { plus: 1, bare: 2 },
                outlet_types: vec![String::new()],
                inlet_digests: vec![],
                outlet_digests: vec![],
                seen: 3,
                quiet: BTreeSet::new(),
            },
        );
        assert_eq!(reference.ports("route a b c").map(|ports| (ports.inlets, ports.outlets, ports.outlet_types.len())), Some((4, 4, 4)));
        assert_eq!(reference.ports("route").map(|ports| ports.outlets), Some(2));
        assert_eq!(reference.ports("nothing 1 2"), None);
        assert_eq!(atoms(r#"sprintf "%s and %s" \"q"#), ["sprintf", "%s and %s", "\"q"]);
    }
}
