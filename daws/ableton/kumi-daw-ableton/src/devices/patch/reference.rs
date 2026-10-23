//! What Kumi knows of each of Max's objects, learned on this machine from the installed Max so it keeps up as Max
//! changes: each object's inlets and outlets for the arguments it's given, fitted to the instances saved in Max's own
//! help patchers, what the object and each inlet and outlet are for (its reference page's digests), and the messages
//! that only set something in it (its attributes, and the methods its page says send nothing). Max's bundled packages
//! count, their pages wherever their docs folder keeps them (not Gen's or RNBO's, whose operators share names with
//! Max's objects), and so do the abstractions Max ships. It's learned once for each Max and kept in Kumi's folder with where each object's page is, so the page is
//! read when it's asked for ([`super::page`]); none of Max's files are carried in Kumi.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::catalog::catalog;
use super::page::{self, Element, LOOKS};

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

/// The most inlets or outlets Kumi gives a box, whatever its arguments or code say.
pub const MOST_PORTS: usize = 512;

impl Count {
    pub fn of(&self, args: &[String]) -> usize {
        self.uncapped(args).min(MOST_PORTS)
    }

    fn uncapped(&self, args: &[String]) -> usize {
        match *self {
            Count::Fixed { count } => count,
            Count::Arguments { plus, bare } => {
                if args.is_empty() {
                    bare
                } else {
                    args.len().saturating_add(plus)
                }
            }
            // A count of none ([gate 0]) is Max's default.
            Count::FirstArgument { plus, bare } => match args.first().and_then(|arg| arg.parse::<f64>().ok()) {
                Some(value) if value >= 1.0 && value.fract() == 0.0 && value <= MOST_PORTS as f64 => (value as usize).saturating_add(plus),
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
    /// How many different saved texts the counts were fitted to (0: the reference page's alone).
    pub seen: usize,
    /// The numbers of arguments its counts hold for, as Max's help shows them (at least two in three of the texts with
    /// that many agree), and those they don't: a rule none of Kumi's counts fits. [pipe]'s inlets are its arguments
    /// but never fewer than two, so [pipe 0 0 1 250] has four; [notein] alone has an outlet for the channel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unheld: Vec<usize>,
    /// The one text its counts were seen to hold for, when there's only one ([bangbang 3]): one text can't tell a count
    /// that's always the same from one its arguments set, so a fixed count holds for that text alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<Vec<String>>,
    /// The messages that only set something in it, sending nothing at once: its attributes, and the methods its page
    /// says send nothing ("Set the value with no output") or that set a thing without a word of sending.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub quiet: BTreeSet<String>,
    /// Of those, the attributes that only change how it looks or is described (hidden, bgcolor, fontsize): nothing
    /// it sends depends on them.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub looks: BTreeSet<String>,
    /// Its reference page, from the C74 folder ("docs/refpages/max-ref/pipe.maxref.xml"); empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub page: String,
}

/// The page of the attributes every box has (hidden, presentation_rect, varname…).
const BOX_PAGE: &str = "jbox";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    /// What it was learned from, so a different Max is learned anew.
    pub fingerprint: String,
    pub objects: BTreeMap<String, Object>,
    /// The C74 folder its objects' pages are in: where Max is now, so it isn't kept.
    #[serde(skip)]
    pub root: PathBuf,
}

/// An object's inlets and outlets for the arguments it's given.
#[derive(Debug, Clone, PartialEq)]
pub struct Ports {
    pub inlets: usize,
    pub outlets: usize,
    pub outlet_types: Vec<String>,
}

impl Object {
    /// Whether its page lists its inlets or outlets.
    pub fn listed(&self) -> bool {
        !self.inlet_digests.is_empty() || !self.outlet_digests.is_empty()
    }
}

impl Reference {
    /// An object by its class, or the class an alias stands for ("t" is trigger).
    pub fn object(&self, class: &str) -> Option<&Object> {
        self.objects.get(class).or_else(|| self.objects.get(catalog().canonical(class)))
    }

    /// An object's ports for a box's text ("route a b c"): None for a class the reference doesn't know, one whose
    /// ports come from what's inside it (a subpatcher, gen~, code), or one whose fixed count isn't known to hold for
    /// those arguments.
    pub fn ports(&self, text: &str) -> Option<Ports> {
        let words = atoms(text);
        let (class, rest) = words.split_first()?;
        if catalog().ports_from_contents(class) {
            return None;
        }
        let object = self.object(class)?;
        let args: Vec<String> = rest.iter().take_while(|word| !word.starts_with('@')).cloned().collect();
        // A fixed count holds as far as Max's help shows it: where it does, and for a number of arguments the help never
        // shows only when it held at every number shown, up to two or more (one more argument can add a port). Seen in
        // one text, it holds for that text; seen in none, a page's count holds for the box bare. An abstraction's inlet
        // and outlet objects are its count, whatever its arguments. A count not known to hold leaves the ports to the
        // cords, which keeps a correct cord; a wrong count would drop it.
        let n = args.len();
        let holds = |count: &Count| {
            if !matches!(count, Count::Fixed { .. }) || object.module == "abstraction" {
                return true;
            }
            // A page that lists no inlets or outlets doesn't say it has none.
            if object.seen == 0 {
                return n == 0 && object.listed();
            }
            if let Some(only) = &object.only {
                return *only == args;
            }
            object.held.contains(&n) || (object.unheld.is_empty() && object.held.last().is_some_and(|&most| n <= most || most >= 2))
        };
        if !holds(&object.inlets) || !holds(&object.outlets) {
            return None;
        }
        let (inlets, outlets) = (object.inlets.of(&args), object.outlets.of(&args));
        let filler = object.outlet_types.last().cloned().unwrap_or_default();
        let outlet_types = (0..outlets).map(|at| object.outlet_types.get(at).cloned().unwrap_or_else(|| filler.clone())).collect();
        Some(Ports { inlets, outlets, outlet_types })
    }

    /// Whether the message `selector` only sets something in an object of `class` (one of its attributes, or a method
    /// its page says sends nothing), so the object sends nothing at once. A box with a face (`ui`: a live.dial, a
    /// jsui) also takes every box's attributes (hidden, presentation_rect) as messages.
    pub fn quiet(&self, class: &str, selector: &str, ui: bool) -> bool {
        self.own_or_box(class, selector, ui).is_some()
    }

    /// Whether the message `selector` only changes how an object of `class` looks (an attribute filed under
    /// Appearance, Color, Font…), so nothing it sends depends on it.
    pub fn looks_only(&self, class: &str, selector: &str, ui: bool) -> bool {
        self.own_or_box(class, selector, ui).is_some_and(|object| object.looks.contains(selector))
    }

    /// The page that says `selector` only sets something: the object's own, else every box's for one with a face.
    fn own_or_box(&self, class: &str, selector: &str, ui: bool) -> Option<&Object> {
        let own = self.object(class).filter(|object| object.quiet.contains(selector));
        own.or_else(|| self.objects.get(BOX_PAGE).filter(|object| ui && object.quiet.contains(selector)))
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

/// The instances that say what Max does now, one for each text: of those with the same arguments, the newest Max's
/// saves, and of those the ports saved most. A help patcher saved before an object gained an outlet keeps the count it
/// had then ([midiparse] gained its eighth), and a patcher holding one text many times says it once.
fn newest(instances: Vec<Instance>) -> Vec<Instance> {
    let mut texts: BTreeMap<Vec<String>, Vec<Instance>> = BTreeMap::new();
    for instance in instances {
        texts.entry(instance.args.clone()).or_default().push(instance);
    }
    texts
        .into_values()
        .filter_map(|saves| {
            let latest = saves.iter().map(|save| save.version).max()?;
            let latest: Vec<Instance> = saves.into_iter().filter(|save| save.version == latest).collect();
            let ports = |save: &Instance| (save.inlets, save.outlets);
            let most = latest.iter().max_by_key(|save| latest.iter().filter(|other| ports(other) == ports(save)).count()).map(ports)?;
            latest.into_iter().find(|save| ports(save) == most)
        })
        .collect()
}

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

/// The messages a page says only set something (its own attributes, not an attribute's or a method's, and its methods
/// that send nothing; a method that sends, by an attribute's name, isn't quiet), and of those the attributes that only
/// change how the box looks (filed under Appearance, Color, Font…).
fn quiet_messages(page: &Element) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut quiet, mut looks) = (BTreeSet::new(), BTreeSet::new());
    for attribute in page.child("attributelist").into_iter().flat_map(|list| list.children("attribute")) {
        let Some(name) = attribute.attribute("name") else { continue };
        quiet.insert(name.to_string());
        if page::meta(attribute, "category").is_some_and(|category| LOOKS.contains(&category)) {
            looks.insert(name.to_string());
        }
    }
    // An MC object's attributes for each channel, and a Jitter object's own, each listed apart.
    for (list, item) in [("mcwrapper", "mcwrapperattr"), ("jitterattributelist", "jitterattribute")] {
        for attribute in page.children(list).flat_map(|list| list.children(item)) {
            quiet.extend(attribute.attribute("name").map(str::to_string));
        }
    }
    for method in page.child("methodlist").into_iter().flat_map(|list| list.children("method")) {
        let Some(name) = method.attribute("name") else { continue };
        if name.starts_with('(') || DATA.contains(&name) {
            continue;
        }
        let digest = method.child("digest").map(Element::text).unwrap_or_default();
        if SENDS_NOTHING.is_match(&digest) || (!SENDS.is_match(&digest) && SETS.is_match(&digest)) {
            quiet.insert(name.to_string());
        } else {
            quiet.remove(name);
        }
        // A method by an attribute's name does what its page says.
        looks.remove(name);
    }
    (quiet, looks)
}

/// An object from its reference page (a .maxref.xml): its class and what the page says of it. A page of what a group
/// of objects share (box="0": "Snapshot Messages") is no object's.
fn read_page(text: &str) -> Option<(String, Object)> {
    let page = page::read(text).filter(|page| page.tag == "c74object" && page.attribute("box") != Some("0"))?;
    // As it's typed in Max: the page escapes it ("&gt;" is [>]).
    let name = page.attribute("name").filter(|name| !name.is_empty())?.to_string();
    let (inlets, outlets) = (page::ports(&page, "inlet"), page::ports(&page, "outlet"));
    let (quiet, looks) = quiet_messages(&page);
    let outlet_types = outlets.iter().map(|port| if port.kind.starts_with("signal") { "signal" } else { "" }.to_string()).collect();
    let object = Object {
        module: page.attribute("module").unwrap_or_default().to_string(),
        digest: page.child("digest").map(Element::text).unwrap_or_default(),
        inlets: Count::Fixed { count: inlets.len() },
        outlets: Count::Fixed { count: outlets.len() },
        outlet_types,
        inlet_digests: inlets.into_iter().map(|port| port.digest).collect(),
        outlet_digests: outlets.into_iter().map(|port| port.digest).collect(),
        seen: 0,
        held: Vec::new(),
        unheld: Vec::new(),
        only: None,
        quiet,
        looks,
        page: String::new(),
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

/// What the instances say of an object's counts: the numbers of arguments they hold for and those they don't (held where
/// at least two in three of the instances with that many agree, so one odd save among many doesn't undo a count), and
/// the text they hold for when only one does.
type Evidence = (Vec<usize>, Vec<usize>, Option<Vec<String>>);

fn evidence(instances: &[Instance], inlets: Count, outlets: Count) -> Evidence {
    let mut tally: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    let mut agreeing: BTreeSet<&Vec<String>> = BTreeSet::new();
    for instance in instances {
        let agrees = inlets.of(&instance.args) == instance.inlets && outlets.of(&instance.args) == instance.outlets;
        let (agree, against) = tally.entry(instance.args.len()).or_default();
        *if agrees { agree } else { against } += 1;
        if agrees {
            agreeing.insert(&instance.args);
        }
    }
    let (held, unheld): (Vec<_>, Vec<_>) = tally.into_iter().partition(|(_, (agree, against))| *agree > 0 && *agree >= 2 * *against);
    let only = if agreeing.len() == 1 { agreeing.into_iter().next().cloned() } else { None };
    (held.into_iter().map(|(n, _)| n).collect(), unheld.into_iter().map(|(n, _)| n).collect(), only)
}

/// Every file under a folder with one of the extensions, in order.
fn files(folder: &Path, extension: &str, found: &mut Vec<PathBuf>) {
    for (entry, folder) in folders_first(folder) {
        if folder {
            files(&entry, extension, found);
        } else if entry.to_string_lossy().ends_with(extension) {
            found.push(entry);
        }
    }
}

/// What's in a folder, in order, each with whether it's a folder: a link to one isn't, so a link that leads back up is
/// never walked round.
fn folders_first(folder: &Path) -> Vec<(PathBuf, bool)> {
    let mut entries: Vec<(PathBuf, bool)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| (entry.path(), entry.file_type().is_ok_and(|kind| kind.is_dir())))
        .collect();
    entries.sort();
    entries
}

/// Packages whose reference pages describe their own operators (gen's, RNBO's), not Max's objects.
const OTHER_LANGUAGES: [&str; 2] = ["Gen", "RNBO"];

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
    // Max's own pages are in docs/refpages; a package keeps its pages anywhere in its docs folder.
    files(&c74.join("docs").join("refpages"), ".maxref.xml", &mut found.pages);
    for root in std::iter::once(c74.to_path_buf()).chain(packages) {
        let package = root != c74;
        let other = root.file_name().is_some_and(|name| OTHER_LANGUAGES.iter().any(|other| name == *other));
        if package && !other {
            files(&root.join("docs"), ".maxref.xml", &mut found.pages);
        }
        files(&root.join("help"), ".maxhelp", &mut found.help);
        files(&root.join("patchers"), ".maxpat", &mut found.abstractions);
    }
    found
}

/// How Kumi learns the reference: a new way is learned anew, as a new Max is.
const LEARNER: u32 = 10;

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
        held: Vec::new(),
        unheld: Vec::new(),
        only: None,
        quiet: BTreeSet::new(),
        looks: BTreeSet::new(),
        page: String::new(),
    }
}

/// The object a page's file is named for ("pipe" for pipe.maxref.xml).
fn page_stem(file: &Path) -> String {
    file.file_name().map(|file| file.to_string_lossy().trim_end_matches(".maxref.xml").to_string()).unwrap_or_default()
}

/// The reference learned from a Max's C74 folder: its reference pages, its help patchers' saved instances, then the
/// abstractions it ships (their inlets and outlets are their inlet and outlet objects).
pub fn learn(c74: &Path) -> Reference {
    let found = sources(c74);
    let mut objects: BTreeMap<String, Object> = BTreeMap::new();
    // A page whose file is named for another object than the one it names ([fswap]'s, named swap) is that object's,
    // when Max's help shows it and no page names it. (An operator's page is named for its sign: bitand's is [&].)
    let mut misnamed: Vec<(String, Object)> = Vec::new();
    for file in &found.pages {
        if let Some((name, mut object)) = std::fs::read_to_string(file).ok().as_deref().and_then(read_page) {
            object.page = file.strip_prefix(c74).unwrap_or(file).to_string_lossy().into_owned();
            let stem = page_stem(file);
            // Of two pages that name an object, the one whose file is named for it.
            let own = stem == name;
            match objects.get(&name) {
                Some(kept) if !own || page_stem(Path::new(&kept.page)) == name => {}
                _ => {
                    objects.insert(name.clone(), object.clone());
                }
            }
            if !own && !stem.is_empty() {
                misnamed.push((stem, object));
            }
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
        (object.held, object.unheld, object.only) = evidence(&instances, object.inlets, object.outlets);
    }
    for (stem, page) in misnamed {
        if let Some(object) = objects.get_mut(&stem).filter(|object| object.page.is_empty()) {
            let counts =
                (object.inlets, object.outlets, object.outlet_types.clone(), object.seen, object.held.clone(), object.unheld.clone());
            let only = object.only.clone();
            *object = page;
            (object.inlets, object.outlets, object.outlet_types, object.seen, object.held, object.unheld) = counts;
            object.only = only;
        }
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
    Reference { fingerprint: fingerprint(c74), objects, root: c74.to_path_buf() }
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
    // Max's folder of its own is C74 on a Mac; on Windows it's resources, beside Max.exe.
    let named = root.file_name().is_some_and(|name| name == "C74" || name.eq_ignore_ascii_case("resources"));
    if named && root.join("docs").join("refpages").is_dir() {
        return Some(root.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    folders_first(root).into_iter().filter(|(_, folder)| *folder).find_map(|(entry, _)| c74_within(&entry, depth - 1))
}

static INSTALLED: LazyLock<Option<Reference>> = LazyLock::new(|| {
    let c74 = find_c74()?;
    let print = fingerprint(&c74);
    let folder = std::env::var("KUMI_HOME")
        .ok()
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home::home_dir().unwrap_or_default().join(".kumi"))
        .join("max");
    let file = folder.join("reference.json");
    if let Some(mut kept) = std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice::<Reference>(&bytes).ok()) {
        if kept.fingerprint == print {
            kept.root = c74;
            return Some(kept);
        }
    }
    let learned = learn(&c74);
    // Written beside it, then put in its place: another Kumi reading it, or one stopped writing it, never sees half.
    if std::fs::create_dir_all(&folder).is_ok() {
        let temporary = folder.join(format!(".reference.{}.json", uuid::Uuid::new_v4()));
        if std::fs::write(&temporary, serde_json::to_vec(&learned).unwrap_or_default()).is_err()
            || std::fs::rename(&temporary, &file).is_err()
        {
            let _ = std::fs::remove_file(&temporary);
        }
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
        // Eight comparisons with two inlets, and one help patcher holding one text eleven times with one: a text says
        // its count once.
        let mut compare: Vec<Instance> =
            ["0.", "60.", "120.", "180.", "240.", "360.", "380.", "420."].iter().map(|arg| instance(&[arg], 2, 1)).collect();
        compare.extend((0..11).map(|_| instance(&["1"], 1, 1)));
        compare.extend((0..6).map(|_| instance(&[], 2, 1)));
        assert_eq!(fit(&newest(compare), |i| i.inlets, 2), Count::Fixed { count: 2 });
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
                <attributelist><attribute name="label" get="1" set="1" type="symbol" size="1" value="Carry" />
                  <attribute name="category" get="1" set="1" type="symbol" size="1" value="Color" /></attributelist>
              </attribute>
              <attribute name="compatmode" get="1" set="1" type="int" size="1">
                <attributelist><attribute name="category" get="1" set="1" type="symbol" size="1" value="Behavior" /></attributelist>
              </attribute>
              <attribute name="textcolor" get="1" set="1" type="float" size="4">
                <attributelist><attribute name="category" get="1" set="1" type="symbol" size="1" value="Color" /></attributelist>
              </attribute>
            </attributelist></c74object>"#;
        let (_, object) = read_page(page).unwrap();
        assert_eq!(
            object.quiet.iter().map(String::as_str).collect::<Vec<_>>(),
            ["compatmode", "max", "poll", "set", "textcolor"],
            "attributes (not an attribute's own), and methods that send nothing; a number always counts"
        );
        assert_eq!(object.looks.iter().map(String::as_str).collect::<Vec<_>>(), ["textcolor"], "filed under Color, and not a method");
        let mut reference = Reference::default();
        reference.objects.insert("counter".into(), object);
        reference.objects.insert(
            BOX_PAGE.into(),
            Object { quiet: BTreeSet::from(["hidden".to_string()]), looks: BTreeSet::from(["hidden".to_string()]), ..unpaged("max", 0, 0) },
        );
        assert!(reference.quiet("counter", "set", false) && !reference.quiet("counter", "jam", false));
        assert!(reference.looks_only("counter", "textcolor", false) && !reference.looks_only("counter", "set", false));
        assert!(reference.looks_only("live.dial", "hidden", true), "a box with a face takes every box's attributes");
        assert!(!reference.quiet("prepend", "hidden", false), "an object in a box doesn't");
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
                seen: 3,
                ..Object::default()
            },
        );
        assert_eq!(reference.ports("route a b c").map(|ports| (ports.inlets, ports.outlets, ports.outlet_types.len())), Some((4, 4, 4)));
        assert_eq!(reference.ports("route").map(|ports| ports.outlets), Some(2));
        assert_eq!(reference.ports("nothing 1 2"), None);
        assert_eq!(atoms(r#"sprintf "%s and %s" \"q"#), ["sprintf", "%s and %s", "\"q"]);
    }

    #[test]
    fn a_count_holds_only_for_the_arguments_the_instances_agree_on() {
        let fitted = |instances: &[Instance], page: (usize, usize)| {
            let (inlets, outlets) = (fit(instances, |i| i.inlets, page.0), fit(instances, |i| i.outlets, page.1));
            let (held, unheld, only) = evidence(instances, inlets, outlets);
            Object { inlets, outlets, seen: instances.len(), held, unheld, only, ..Object::default() }
        };
        // [pipe] in Max's help: mostly a delay time alone; a few delay several numbers, an inlet and an outlet each.
        let mut pipe: Vec<Instance> = ["100", "50", "25", "500", "4n", "1"].iter().map(|arg| instance(&[arg], 2, 1)).collect();
        pipe.extend([instance(&[], 2, 1), instance(&["0", "10000"], 2, 1), instance(&["0.", "1000"], 2, 1)]);
        pipe.extend([instance(&["0", "0", "1000"], 3, 2), instance(&["0.", "0.", "0.", "2000"], 4, 3)]);
        let pipe = fitted(&pipe, (2, 1));
        assert_eq!((pipe.inlets, pipe.outlets), (Count::Fixed { count: 2 }, Count::Fixed { count: 1 }), "no count Kumi fits says it all");
        assert_eq!((pipe.held.as_slice(), pipe.unheld.as_slice()), ([0, 1, 2].as_slice(), [3, 4].as_slice()));
        // [notein] alone, or with a port's name, has an outlet for the channel; with a channel number it hasn't. As in Max's
        // help, the two tie, and the count is the smaller.
        let notein = [
            instance(&[], 1, 3),
            instance(&["17"], 1, 2),
            instance(&["QUNEO"], 1, 3),
            instance(&["a"], 1, 3),
            instance(&["b", "6"], 1, 2),
            instance(&["a", "1"], 1, 2),
        ];
        let notein = fitted(&notein, (1, 3));
        assert_eq!(notein.outlets, Count::Fixed { count: 2 });
        // [*] has a right inlet; one odd save among many doesn't undo it.
        let mut times: Vec<Instance> = (0..9).map(|at| instance(&[&at.to_string()], 2, 1)).collect();
        times.push(instance(&["pi"], 1, 1));
        let times = fitted(&times, (2, 1));
        let mut reference = Reference::default();
        reference.objects.insert("pipe".into(), pipe);
        reference.objects.insert("notein".into(), notein);
        reference.objects.insert("*".into(), times);
        reference.objects.insert("metro".into(), fitted(&[instance(&["100"], 2, 1), instance(&["250"], 2, 1)], (2, 1)));
        // Seen in one text each, whatever the page lists for the object bare: [bangbang 3]'s three outlets may be fixed
        // or its argument's; a variable's name leaves out the count of outlets a second argument sets; a fifth argument
        // at its default leaves out the outlets it sets; and a bare box leaves out a channel, which takes an outlet away.
        let page = |inlets: usize, outlets: usize| (vec![String::new(); inlets], vec![String::new(); outlets]);
        for (class, text, ports) in [
            ("bangbang", vec!["3"], (1, 3)),
            ("variable", vec!["name"], (1, 1)),
            ("stutters~", vec!["22050", "11025", "-1", "10", "1"], (3, 1)),
            ("bendin", vec![], (1, 2)),
        ] {
            let (inlet_digests, outlet_digests) = page(ports.0, ports.1);
            let object = fitted(&[instance(&text, ports.0, ports.1)], ports);
            reference.objects.insert(class.into(), Object { inlet_digests, outlet_digests, ..object });
        }
        reference.objects.insert("zl".into(), fitted(&[instance(&["group"], 2, 2), instance(&["slice", "2", "x"], 2, 2)], (2, 2)));
        let (inlet_digests, outlet_digests) = (vec![String::new(); 2], vec![String::new(); 2]);
        let (inlets, outlets) = (Count::Fixed { count: 2 }, Count::Fixed { count: 2 });
        reference.objects.insert("cverb~".into(), Object { inlets, outlets, inlet_digests, outlet_digests, ..Object::default() });
        let fixed = |inlets: usize, outlets: usize| (Count::Fixed { count: inlets }, Count::Fixed { count: outlets });
        let (inlets, outlets) = fixed(1, 2);
        reference.objects.insert("two.ways".into(), Object { module: "abstraction".into(), inlets, outlets, ..Object::default() });
        let ports = |text: &str| reference.ports(text).map(|ports| (ports.inlets, ports.outlets));
        assert_eq!((ports("pipe 250"), ports("pipe 0 250")), (Some((2, 1)), Some((2, 1))), "where the instances agree");
        assert_eq!((ports("pipe 0 0 1000"), ports("pipe 0 0 1 250")), (None, None), "where they don't, and past them: its cords say");
        assert_eq!(ports("notein"), None, "alone, it has a channel outlet the count doesn't");
        assert_eq!(ports("notein a 1"), Some((1, 2)));
        assert_eq!(ports("* 2."), Some((2, 1)));
        assert_eq!(
            (ports("metro"), ports("metro 100 200")),
            (Some((2, 1)), None),
            "seen only with one argument: one more could add a port"
        );
        assert_eq!(ports("zl group 4 @zlmaxsize 512"), Some((2, 2)), "held up to three arguments, and none disagrees: it stays");
        assert_eq!((ports("bangbang 3"), ports("bangbang"), ports("bangbang 5")), (Some((1, 3)), None, None), "one text holds for itself");
        assert_eq!(
            (ports("variable x 3"), ports("stutters~ 44100 22050 -1 10 2"), ports("bendin 1")),
            (None, None, None),
            "and for no other, though the page agrees with it"
        );
        assert_eq!(ports("bendin"), Some((1, 2)));
        assert_eq!((ports("cverb~"), ports("cverb~ 0.5 0.2 0.9")), (Some((2, 2)), None), "a page alone holds for the box bare");
        assert_eq!(ports("two.ways 1 2 3"), Some((1, 2)), "an abstraction's ports are its inlet and outlet objects");
        let (name, _) = read_page(r#"<c74object name="&gt;=" module="max"><digest>Compare</digest></c74object>"#).unwrap();
        assert_eq!(name, ">=", "named as it's typed in Max, so its page and its help instances meet");
    }

    #[test]
    fn maxs_own_folder_is_found_as_each_system_names_it_and_a_link_round_is_never_walked() {
        let root = tempfile::tempdir().unwrap();
        for (folder, found) in [
            ("Max 9/resources", true),
            ("Live/Resources/Max/resources", true),
            ("Max.app/Contents/Resources/C74", true),
            ("Other/docs", false),
        ] {
            let c74 = root.path().join(folder);
            std::fs::create_dir_all(c74.join("docs").join("refpages")).unwrap();
            let base = root.path().join(folder.split('/').next().unwrap());
            assert_eq!(c74_within(&base, 4), found.then_some(c74), "{folder}");
        }
        #[cfg(unix)]
        {
            let help = root.path().join("Max 9/resources/help");
            std::fs::create_dir_all(&help).unwrap();
            std::os::unix::fs::symlink(&help, help.join("again")).unwrap();
            let mut found = Vec::new();
            files(&help, ".maxhelp", &mut found);
            assert!(found.is_empty(), "the link is a file, not a folder walked again");
        }
        let overflowing = Count::Arguments { plus: usize::MAX - 2, bare: 1 };
        assert_eq!(overflowing.of(&["a".into(), "b".into(), "c".into()]), MOST_PORTS, "a count from a broken help file can't overflow");
    }

    #[test]
    fn a_packages_pages_are_read_wherever_its_docs_keep_them_and_a_page_is_named_for_its_file() {
        let c74 = tempfile::tempdir().unwrap();
        let write = |folder: &[&str], file: &str, name: &str| {
            let folder = folder.iter().fold(c74.path().to_path_buf(), |path, part| path.join(part));
            std::fs::create_dir_all(&folder).unwrap();
            let page = format!(r#"<c74object name="{name}" module="max"><digest>{name} from {file}</digest></c74object>"#);
            std::fs::write(folder.join(format!("{file}.maxref.xml")), page).unwrap();
        };
        write(&["docs", "refpages", "max-ref"], "thing", "thing");
        // A page whose file is named for one object but which names another: the second's own page wins it, and the
        // first gets it, as Max's help shows the first. An operator's page, named for its sign, makes no object.
        write(&["docs", "refpages", "max-ref"], "fthing", "thing");
        write(&["docs", "refpages", "max-ref"], "bitand", "&");
        std::fs::create_dir_all(c74.path().join("help")).unwrap();
        let help =
            json!({ "patcher": { "boxes": [{ "box": { "maxclass": "newobj", "text": "fthing 1", "numinlets": 2, "numoutlets": 2 } }] } });
        std::fs::write(c74.path().join("help").join("fthing.maxhelp"), help.to_string()).unwrap();
        write(&["packages", "Elements", "docs"], "live.thing~", "live.thing~");
        write(&["packages", "dsp", "docs"], "abl.thing~", "abl.thing~");
        write(&["packages", "Gen", "docs", "refpages1"], "gen_cycle", "cycle");
        write(&["packages", "RNBO", "docs", "refpages"], "rnbo_mix", "mix");
        let reference = learn(c74.path());
        let names: Vec<&str> = reference.objects.keys().map(String::as_str).collect();
        assert_eq!(names, ["&", "abl.thing~", "fthing", "live.thing~", "thing"], "Gen's and RNBO's operators aren't Max's objects");
        assert_eq!(reference.objects["thing"].digest, "thing from thing");
        let fthing = &reference.objects["fthing"];
        assert_eq!(
            (fthing.digest.as_str(), fthing.inlets, fthing.seen),
            ("thing from fthing", Count::Fixed { count: 2 }, 1),
            "its page, its help's counts"
        );
        // A page that lists no ports says nothing of how many there are: the cords decide.
        assert_eq!(reference.ports("live.thing~"), None);
    }

    #[test]
    fn a_page_is_read_whatever_order_and_quotes_its_attributes_take() {
        // A Live API object's page (written for this test): its name last, single quotes, its ports in a list of
        // their own with descriptions, and an attribute inside a method's own list.
        let page = r#"<c74object category='Live API Objects' module='m4l' name='live.thing'>
            <digest> Watch a thing in Live </digest><inletlist/><outletlist/>
            <apiinletlist><inlet name='Left inlet' id='0'><description> Takes <m>bang</m>. Then more. </description></inlet>
              <inlet name='Right inlet' id='1'><description>Takes an id</description></inlet></apiinletlist>
            <apioutletlist><outlet name='Left outlet' id='0'><description>Sends the value.</description></outlet></apioutletlist>
            <methodlist><method name='go'><arglist/><attributelist><attribute name='introduced' get='1' set='1' type='symbol' size='1' value='9'/></attributelist>
              <digest>Output the value</digest></method></methodlist>
            <attributelist><attribute name='slot' get='1' set='1' type='int' size='1'><digest>Which slot</digest></attribute></attributelist>
            </c74object>"#;
        let (name, object) = read_page(page).unwrap();
        assert_eq!((name.as_str(), object.module.as_str(), object.digest.as_str()), ("live.thing", "m4l", "Watch a thing in Live"));
        assert_eq!((object.inlets, object.outlets), (Count::Fixed { count: 2 }, Count::Fixed { count: 1 }));
        assert_eq!(object.inlet_digests, ["Takes bang.", "Takes an id"], "a description's first sentence");
        assert_eq!(object.quiet.iter().map(String::as_str).collect::<Vec<_>>(), ["slot"], "a method's own attributes aren't the object's");
        let group = r#"<c74object box="0" name="Shared Messages" module="msp"><digest>What a group shares</digest></c74object>"#;
        assert_eq!(read_page(group), None, "a page of what a group of objects shares is no object's");
    }
}
