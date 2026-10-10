//! Kumi's checks of a Max for Live patcher against its standard. Each broken rule is one line naming the rule, where it
//! broke and the fix, and every one is found in one pass, so whoever made the patcher (the model, or Kumi's own
//! builder) fixes them all at once. What a rule looks at is counted too, for the survey's measurements.
//!
//! - **patch**: every object is one Max knows (when Kumi has learned the installed Max), and every cord leaves and
//!   enters a port its box has.
//! - **layout**: no cord runs over a box, no two boxes overlap, cords run down the patcher except to close a loop.
//! - **order**: when one outlet's cords meet again at a box, one only to be kept (a cold inlet, or a message that only
//!   sets something) and one to make it send, which arrives first depends on where boxes sit; a trigger makes it
//!   explicit. A branch that sets the inlet itself before it sends is settled.
//! - **names**: a send, a buffer~ or a dict named without `---` is shared by every copy of the device.
//! - **params**: Live parameters are named for what they do, once each, kept in Push's banks and explained in Live's
//!   Info View.
//! - **gen**: a gen~ Param is declared once.
//! - **face**: no opaque panel hides what's on the device's face.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;

use serde_json::Value;

use super::catalog::{catalog, Catalog};
use super::geometry::{inlet_point, outlet_point, Rect};
use super::reference::{atoms, Reference};
use super::standard::{standard, Level, Standard};
use super::{MaxBox, Patcher};
use crate::devices::face::hidden_by_panels;

pub const UNKNOWN_OBJECT: &str = "patch.unknown-object";
pub const NO_SUCH_PORT: &str = "patch.no-such-port";
pub const CORD_OVER_BOX: &str = "layout.cord-over-box";
pub const BOXES_OVERLAP: &str = "layout.boxes-overlap";
pub const CORD_UPWARD: &str = "layout.cord-upward";
pub const FAN_OUT_REJOINS: &str = "order.fan-out-rejoins";
pub const SHARED_BY_COPIES: &str = "names.shared-by-copies";
pub const DEFAULT_NAME: &str = "params.default-name";
pub const SAME_NAME: &str = "params.same-name";
pub const BANK_REPEAT: &str = "params.bank-repeat";
pub const BANK_UNKNOWN: &str = "params.bank-unknown";
pub const BANK_MISSING: &str = "params.bank-missing";
pub const NO_INFO: &str = "params.no-info";
pub const PARAM_TWICE: &str = "gen.param-twice";
pub const HIDDEN_BY_PANEL: &str = "face.hidden-by-panel";

/// Every rule the checker knows.
pub const RULES: [&str; 15] = [
    UNKNOWN_OBJECT,
    NO_SUCH_PORT,
    CORD_OVER_BOX,
    BOXES_OVERLAP,
    CORD_UPWARD,
    FAN_OUT_REJOINS,
    SHARED_BY_COPIES,
    DEFAULT_NAME,
    SAME_NAME,
    BANK_REPEAT,
    BANK_UNKNOWN,
    BANK_MISSING,
    NO_INFO,
    PARAM_TWICE,
    HIDDEN_BY_PANEL,
];

/// How far a chain of messages is followed from a fan-out.
const MAX_HOPS: usize = 24;
/// How far two boxes can run into each other and only touch, in pixels.
const TOUCH: f64 = 1.5;
/// Boxes Max draws behind others on purpose: a cord or a box over one is fine.
const BACKDROPS: [&str; 3] = ["panel", "fpic", "live.line"];
/// Max's Live objects that are parameters unless turned off.
const LIVE_PARAMETERS: [&str; 9] =
    ["live.button", "live.dial", "live.gain~", "live.menu", "live.numbox", "live.slider", "live.tab", "live.text", "live.toggle"];

/// A broken rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub rule: &'static str,
    pub level: Level,
    /// The subpatchers it's in, outermost first; "" at the patcher's top.
    pub at: String,
    pub says: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.at.is_empty() {
            write!(f, "{}: {}", self.rule, self.says)
        } else {
            write!(f, "{} (in {}): {}", self.rule, self.at, self.says)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Every broken rule, whatever its level.
    pub findings: Vec<Finding>,
    /// How many things each rule looked at (cords, parameters, fan-outs), whether or not any broke it.
    pub looked_at: BTreeMap<&'static str, usize>,
}

impl Report {
    /// The findings at `level` or above.
    pub fn at_least(&self, level: Level) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(move |finding| finding.level >= level && finding.level != Level::Off)
    }

    /// How many times each rule broke.
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for finding in &self.findings {
            *counts.entry(finding.rule).or_default() += 1;
        }
        counts
    }
}

/// A device's patcher checked against Kumi's standard, the patchers inside it too. The rules that need to know Max's
/// objects (which exist, how many ports each has for its arguments) are left out: see [`check_with`].
pub fn check(patcher: &Patcher) -> Report {
    check_with(patcher, standard(), None)
}

/// A device's patcher checked against `standard`, and against what `reference` knows of Max's objects (the installed
/// Max's, from [`super::reference::installed`]).
pub fn check_with(patcher: &Patcher, standard: &Standard, reference: Option<&Reference>) -> Report {
    let mut checker = Checker { standard, catalog: catalog(), reference, report: Report::default(), files_seen: HashSet::new() };
    checker.walk(patcher, "");
    checker.params(patcher);
    checker.face(patcher);
    checker.report
}

struct Checker<'a> {
    standard: &'a Standard,
    catalog: &'a Catalog,
    reference: Option<&'a Reference>,
    report: Report,
    /// Files already checked: a dial used three times is one patcher, checked once.
    files_seen: HashSet<String>,
}

/// The place inside `at` of a box's own patcher.
fn inside(at: &str, item: &MaxBox) -> String {
    if at.is_empty() {
        item.label()
    } else {
        format!("{at} › {}", item.label())
    }
}

impl Checker<'_> {
    fn found(&mut self, rule: &'static str, at: &str, says: String) {
        self.report.findings.push(Finding { rule, level: self.standard.level(rule), at: at.to_string(), says });
    }

    fn looked(&mut self, rule: &'static str, count: usize) {
        *self.report.looked_at.entry(rule).or_default() += count;
    }

    fn walk(&mut self, patcher: &Patcher, at: &str) {
        self.ports(patcher, at);
        self.layout(patcher, at);
        if patcher.is_gen() {
            self.gen_code(patcher, at);
        } else {
            self.objects(patcher, at);
            self.order(patcher, at);
            self.names(patcher, at);
        }
        for item in &patcher.boxes {
            let inner = match (&item.patcher, &item.file) {
                (Some(inner), _) => inner,
                (None, Some((name, inner))) if self.files_seen.insert(name.clone()) => inner,
                _ => continue,
            };
            self.walk(inner, &inside(at, item));
        }
    }

    /// How many inlets (or outlets) a box has: as saved with it, else as the reference counts them for its text.
    fn port_count(&self, item: &MaxBox, inlets: bool) -> Option<usize> {
        let saved = item.fields.get(if inlets { "numinlets" } else { "numoutlets" }).and_then(Value::as_u64);
        saved.map(|count| count as usize).or_else(|| {
            let ports = self.reference?.ports(item.text()).filter(|_| item.maxclass() == "newobj")?;
            Some(if inlets { ports.inlets } else { ports.outlets })
        })
    }

    fn ports(&mut self, patcher: &Patcher, at: &str) {
        let by_id: HashMap<&str, &MaxBox> = patcher.boxes.iter().map(|item| (item.id(), item)).collect();
        let label = labels(patcher);
        self.looked(NO_SUCH_PORT, patcher.cords.len());
        for cord in &patcher.cords {
            let (Some(from), Some(to)) = (by_id.get(cord.from.as_str()), by_id.get(cord.to.as_str())) else {
                let missing = if by_id.contains_key(cord.from.as_str()) { &cord.to } else { &cord.from };
                self.found(
                    NO_SUCH_PORT,
                    at,
                    format!("a cord joins {missing}, which isn't in the patcher: take the cord out, or add the box."),
                );
                continue;
            };
            if let Some(outlets) = self.port_count(from, false).filter(|outlets| cord.outlet >= *outlets) {
                self.found(
                    NO_SUCH_PORT,
                    at,
                    format!(
                        "a cord leaves outlet {} of {}, which has {outlets} (counted from 0): use one of its outlets.",
                        cord.outlet,
                        label[from.id()]
                    ),
                );
            }
            if let Some(inlets) = self.port_count(to, true).filter(|inlets| cord.inlet >= *inlets) {
                self.found(
                    NO_SUCH_PORT,
                    at,
                    format!(
                        "a cord enters inlet {} of {}, which has {inlets} (counted from 0): use one of its inlets.",
                        cord.inlet,
                        label[to.id()]
                    ),
                );
            }
        }
    }

    fn objects(&mut self, patcher: &Patcher, at: &str) {
        let Some(reference) = self.reference else { return };
        let label = labels(patcher);
        for item in patcher.boxes.iter().filter(|item| item.maxclass() == "newobj" && !item.class().is_empty()) {
            self.looked(UNKNOWN_OBJECT, 1);
            // An abstraction the device holds is an object of its own.
            if item.file.is_some() || reference.object(item.class()).is_some() {
                continue;
            }
            self.found(
                UNKNOWN_OBJECT,
                at,
                format!(
                    "{} isn't an object this machine's Max knows: check its name (a typo, an object from a package that isn't installed, or an abstraction the device doesn't hold).",
                    label[item.id()]
                ),
            );
        }
    }

    fn layout(&mut self, patcher: &Patcher, at: &str) {
        let placed: Vec<(&MaxBox, Rect)> = patcher
            .boxes
            .iter()
            .filter(|item| !BACKDROPS.contains(&item.maxclass()))
            .filter_map(|item| Some((item, item.rect()?)))
            .collect();
        let label = labels(patcher);
        self.looked(BOXES_OVERLAP, placed.len());
        for (index, (item, rect)) in placed.iter().enumerate() {
            // Boxes a pixel or so into each other only touch: Max's rounding, or a comment's empty margin.
            let shrunk = Rect::new(rect.x + TOUCH, rect.y + TOUCH, (rect.w - 2.0 * TOUCH).max(0.0), (rect.h - 2.0 * TOUCH).max(0.0));
            for (other, other_rect) in &placed[index + 1..] {
                if shrunk.overlaps(other_rect) {
                    self.found(
                        BOXES_OVERLAP,
                        at,
                        format!("{} and {} overlap: move one so both can be read.", label[item.id()], label[other.id()]),
                    );
                }
            }
        }
        let by_id: HashMap<&str, &MaxBox> = patcher.boxes.iter().map(|item| (item.id(), item)).collect();
        let next = downstream(patcher);
        self.looked(CORD_OVER_BOX, patcher.cords.len());
        self.looked(CORD_UPWARD, patcher.cords.len());
        for cord in &patcher.cords {
            let (Some(from), Some(to)) = (by_id.get(cord.from.as_str()), by_id.get(cord.to.as_str())) else { continue };
            let (Some(start), Some(end)) = (from.rect(), to.rect()) else { continue };
            let start = outlet_point(&start, from.outlets().max(cord.outlet + 1), cord.outlet);
            let end = inlet_point(&end, to.inlets().max(cord.inlet + 1), cord.inlet);
            let points: Vec<[f64; 2]> = std::iter::once(start).chain(cord.midpoints.iter().copied()).chain(std::iter::once(end)).collect();
            let under: Vec<&MaxBox> = placed
                .iter()
                .filter(|(item, _)| item.id() != cord.from && item.id() != cord.to)
                .filter(|(_, rect)| points.windows(2).any(|pair| rect.crossed_by(pair[0], pair[1], 1.0)))
                .map(|(item, _)| *item)
                .collect();
            if let Some(first) = under.first() {
                let more = if under.len() > 1 { format!(" and {} more", under.len() - 1) } else { String::new() };
                self.found(
                    CORD_OVER_BOX,
                    at,
                    format!(
                        "the cord from {} to {} runs over {}{more}: move the box out of its way, or route the cord around it.",
                        label[from.id()],
                        label[to.id()],
                        label[first.id()]
                    ),
                );
            }
            // A cord that closes a loop has to run back up; any other runs down from its outlet to its inlet.
            if end[1] < start[1] - 0.5 && !reaches(&next, &cord.to, &cord.from) {
                self.found(
                    CORD_UPWARD,
                    at,
                    format!(
                        "the cord from {} runs up to {}: put {} below it, so the patcher reads top-down.",
                        label[from.id()],
                        label[to.id()],
                        label[to.id()]
                    ),
                );
            }
        }
    }

    fn order(&mut self, patcher: &Patcher, at: &str) {
        let by_id: HashMap<&str, &MaxBox> = patcher.boxes.iter().map(|item| (item.id(), item)).collect();
        let label = labels(patcher);
        let next = downstream(patcher);
        let mut outlets: BTreeMap<(usize, usize), Vec<(&str, usize)>> = BTreeMap::new();
        for cord in &patcher.cords {
            let (Some(index), Some(from)) = (patcher.index_of(&cord.from), by_id.get(cord.from.as_str())) else { continue };
            if !from.sends_signal(cord.outlet) {
                outlets.entry((index, cord.outlet)).or_default().push((cord.to.as_str(), cord.inlet));
            }
        }
        let fan_outs: Vec<_> = outlets.into_iter().filter(|(_, branches)| branches.len() > 1).collect();
        self.looked(FAN_OUT_REJOINS, fan_outs.len());
        for ((index, outlet), branches) in fan_outs {
            let source = &patcher.boxes[index];
            let message = messages_out(source);
            let reached: Vec<HashMap<&str, Arrival>> =
                branches.iter().map(|&(to, inlet)| self.follow(&by_id, &next, to, inlet, message.clone())).collect();
            let mut named: HashSet<&str> = HashSet::new();
            for (i, branch) in reached.iter().enumerate() {
                for (&id, arrival) in branch {
                    for (&inlet, how) in &arrival.kept {
                        // Another branch that makes the box send uses whichever value came last, unless it sets this
                        // inlet itself first. Two of the fan-out's own cords into one box go right to left, wherever
                        // it sits.
                        let unsettled = reached.iter().enumerate().any(|(j, other)| {
                            j != i
                                && other.get(id).is_some_and(|other| other.hot && !other.kept.contains_key(&inlet))
                                && !(branches[i].0 == id && branches[j].0 == id)
                        });
                        if !unsettled || !named.insert(id) {
                            continue;
                        }
                        let says = match how {
                            None => format!(
                                "outlet {outlet} of {} reaches {} twice, at cold inlet {inlet} and at its hot inlet: which arrives first depends on where boxes sit. Send it through a [t] whose right outlet feeds the cold inlet.",
                                label[source.id()],
                                label[id]
                            ),
                            Some(selector) => format!(
                                "outlet {outlet} of {} reaches {} twice, once with `{selector}` (which only sets it) and once to make it send: which arrives first depends on where boxes sit. Send it through a [t] whose right outlet sends the `{selector}`.",
                                label[source.id()],
                                label[id]
                            ),
                        };
                        self.found(FAN_OUT_REJOINS, at, says);
                    }
                }
            }
        }
    }

    /// Whether the message `selector` only sets something in a box, sending nothing at once: its code says no function
    /// the message calls sends, or Max's reference says it's an attribute or a method that sends nothing.
    fn quiet(&self, item: &MaxBox, selector: &str) -> bool {
        if let Some(sends) = item.code.as_ref().and_then(|code| code.sends_at_once(selector)) {
            return !sends;
        }
        self.reference.is_some_and(|reference| reference.quiet(item.class(), selector))
    }

    /// Where a message into `inlet` of box `id` goes in the same chain: each box it reaches, and whether to make it
    /// send or only to be kept. `message` is what arrives, by its first words, when a box's text says it. A box that
    /// keeps it (a cold inlet, a message that only sets something), ends it (an audio object) or sends it later goes
    /// no further.
    fn follow<'p>(
        &self,
        by_id: &HashMap<&str, &'p MaxBox>,
        next: &HashMap<&'p str, Vec<(usize, &'p str, usize)>>,
        id: &'p str,
        inlet: usize,
        message: Option<Vec<String>>,
    ) -> HashMap<&'p str, Arrival> {
        let mut reached: HashMap<&str, Arrival> = HashMap::new();
        let mut queue = VecDeque::from([(id, inlet, 0usize, message)]);
        let mut seen = HashSet::new();
        while let Some((id, inlet, hops, message)) = queue.pop_front() {
            if !seen.insert((id, inlet, message.clone())) {
                continue;
            }
            let Some(item) = by_id.get(id) else { continue };
            let class = item.class();
            let arrival = reached.entry(id).or_default();
            if !self.catalog.hot(class, inlet) {
                if !self.catalog.all_hot(class) {
                    arrival.kept.entry(inlet).or_insert(None);
                }
                continue;
            }
            if let Some(selectors) = message.as_ref().filter(|selectors| selectors.iter().all(|selector| self.quiet(item, selector))) {
                arrival.kept.entry(inlet).or_insert_with(|| selectors.first().cloned());
                continue;
            }
            arrival.hot = true;
            let ends =
                self.catalog.ends_messages(class) || self.catalog.defers(class) || matches!(class, "outlet" | "send" | "s" | "forward");
            if ends || hops >= MAX_HOPS {
                continue;
            }
            let out = messages_out(item);
            if out.as_ref().is_some_and(Vec::is_empty) {
                continue;
            }
            for &(outlet, to, to_inlet) in next.get(id).into_iter().flatten() {
                if !item.sends_signal(outlet) {
                    queue.push_back((to, to_inlet, hops + 1, out.clone()));
                }
            }
        }
        reached
    }

    fn names(&mut self, patcher: &Patcher, at: &str) {
        let label = labels(patcher);
        for item in &patcher.boxes {
            if item.maxclass() != "newobj" || !self.catalog.names_something(item.class()) {
                continue;
            }
            let Some(name) = item.args().first().copied() else { continue };
            if name.parse::<f64>().is_ok() {
                continue;
            }
            self.looked(SHARED_BY_COPIES, 1);
            // ---name is each copy's own; #0 and #1… are an abstraction's own or its arguments; $1 is filled in later.
            if name.starts_with("---") || name.contains('#') || name.starts_with('$') {
                continue;
            }
            self.found(
                SHARED_BY_COPIES,
                at,
                format!(
                    "{} names \"{name}\", which every copy of the device (and any other device using the name) shares: call it ---{name} so each copy has its own.",
                    label[item.id()]
                ),
            );
        }
    }

    fn gen_code(&mut self, patcher: &Patcher, at: &str) {
        for item in patcher.boxes.iter().filter(|item| item.maxclass() == "codebox") {
            let declared = params_declared(item.str("code"));
            self.looked(PARAM_TWICE, declared.len());
            let mut seen: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for (name, default) in &declared {
                seen.entry(name.as_str()).or_default().push(default.as_str());
            }
            for (name, defaults) in seen.into_iter().filter(|(_, defaults)| defaults.len() > 1) {
                let differ = defaults.iter().any(|default| *default != defaults[0]);
                self.found(
                    PARAM_TWICE,
                    at,
                    format!(
                        "the codebox declares Param {name} {} times{}: declare it once.",
                        defaults.len(),
                        if differ { format!(", with defaults {}", defaults.join(" and ")) } else { String::new() }
                    ),
                );
            }
        }
    }

    fn params(&mut self, patcher: &Patcher) {
        let mut found: Vec<Parameter> = Vec::new();
        parameters(patcher, patcher, "", "", &mut found);
        let automatable: Vec<&Parameter> = found.iter().filter(|parameter| parameter.automatable).collect();
        self.looked(DEFAULT_NAME, automatable.len());
        self.looked(SAME_NAME, automatable.len());
        self.looked(NO_INFO, automatable.len());
        let mut by_name: BTreeMap<&str, Vec<&Parameter>> = BTreeMap::new();
        for parameter in &automatable {
            by_name.entry(parameter.name.as_str()).or_default().push(parameter);
            if default_name(&parameter.name, &parameter.maxclass) {
                self.found(
                    DEFAULT_NAME,
                    &parameter.at,
                    format!(
                        "{} is a Live parameter named \"{}\", Max's default: name it for what it does (its parameter_longname).",
                        parameter.label, parameter.name
                    ),
                );
            }
            if !parameter.info {
                self.found(
                    NO_INFO,
                    &parameter.at,
                    format!("{} has no info text: say in a sentence what it does (its annotation), for Live's Info View.", parameter.label),
                );
            }
        }
        for (name, twins) in by_name.iter().filter(|(_, twins)| twins.len() > 1) {
            self.found(
                SAME_NAME,
                "",
                format!(
                    "{} Live parameters are named \"{name}\" ({}): Max renames all but one when the device loads, and automation follows the wrong one. Name each for what it does.",
                    twins.len(),
                    twins.iter().map(|parameter| parameter.label.as_str()).collect::<Vec<_>>().join(", ")
                ),
            );
        }
        let banks = banks(patcher);
        let slots: usize = banks.iter().map(|bank| bank.slots.iter().filter(|slot| *slot != "-").count()).sum();
        self.looked(BANK_REPEAT, slots);
        self.looked(BANK_UNKNOWN, slots);
        let mut banked: HashSet<&str> = HashSet::new();
        for bank in &banks {
            let mut in_bank: HashSet<&str> = HashSet::new();
            for slot in bank.slots.iter().filter(|slot| *slot != "-") {
                banked.insert(slot);
                if !in_bank.insert(slot) {
                    self.found(
                        BANK_REPEAT,
                        "",
                        format!("Push bank \"{}\" lists \"{slot}\" twice: one of its slots was meant for another parameter.", bank.name),
                    );
                }
                if !by_name.contains_key(slot.as_str()) && !found.iter().any(|parameter| parameter.name == *slot) {
                    self.found(
                        BANK_UNKNOWN,
                        "",
                        format!(
                            "Push bank \"{}\" lists \"{slot}\", which no parameter is named: name a parameter in its place.",
                            bank.name
                        ),
                    );
                }
            }
        }
        if !banks.is_empty() {
            let mut while_running = HashSet::new();
            banked_while_running(patcher, &mut while_running);
            self.looked(BANK_MISSING, automatable.len());
            let missing = |parameter: &&&Parameter| !banked.contains(parameter.name.as_str()) && !while_running.contains(&parameter.name);
            for parameter in automatable.iter().filter(missing) {
                self.found(
                    BANK_MISSING,
                    &parameter.at,
                    format!(
                        "\"{}\" is in none of the device's Push banks, so Push can't reach it: give it a slot, or set its Parameter Visibility to Stored Only or Hidden if it isn't one to play.",
                        parameter.name
                    ),
                );
            }
        }
    }

    fn face(&mut self, patcher: &Patcher) {
        let panels = patcher.boxes.iter().filter(|item| item.maxclass() == "panel" && item.shown()).count();
        self.looked(HIDDEN_BY_PANEL, panels);
        for line in hidden_by_panels(&patcher.to_document()) {
            self.found(
                HIDDEN_BY_PANEL,
                "",
                format!("{line}. Max draws a box above the boxes after it in its layer: list the panel after them, or put it in the background layer."),
            );
        }
    }
}

/// How a chain of messages reached a box.
#[derive(Debug, Default, Clone)]
struct Arrival {
    /// Whether it reached an inlet that makes the box send, with a message that does.
    hot: bool,
    /// The inlets where the box only kept what came: a cold inlet (None), or a message that only sets something
    /// (`set`, an attribute, a function that doesn't send).
    kept: BTreeMap<usize, Option<String>>,
}

/// The messages a box sends, where its text says them: each of a message box's messages that leave by its outlet
/// (what follows a semicolon goes to receivers), a [prepend]'s word; by each one's first word. None when a message
/// comes from what reached the box, or starts with a number or a $ argument.
fn messages_out(item: &MaxBox) -> Option<Vec<String>> {
    let word = |message: &str| atoms(message).into_iter().next().filter(|word| !word.starts_with('$') && word.parse::<f64>().is_err());
    match item.maxclass() {
        "message" => {
            let out = item.text().split(';').next().unwrap_or("");
            out.split(',').map(str::trim).filter(|message| !message.is_empty()).map(word).collect()
        }
        "newobj" if item.class() == "prepend" => item.args().first().and_then(|first| word(first)).map(|word| vec![word]),
        _ => None,
    }
}

/// How notes name a patcher's boxes: each one's label, with its scripting name (or else its id) when another box in
/// the patcher has the same label.
fn labels(patcher: &Patcher) -> HashMap<&str, String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for item in &patcher.boxes {
        *counts.entry(item.label()).or_default() += 1;
    }
    patcher
        .boxes
        .iter()
        .map(|item| {
            let label = item.label();
            let label = if counts[&label] > 1 {
                format!("{label} ({})", Some(item.str("varname")).filter(|name| !name.is_empty()).unwrap_or(item.id()))
            } else {
                label
            };
            (item.id(), label)
        })
        .collect()
}

/// The parameters a device puts in its Push banks while it runs: the names in the `edit` and `new` messages its
/// message boxes send [live.banks] (`edit 0 main 2 "Rate"`: bank 0, called main, gets Rate in its third slot).
fn banked_while_running(patcher: &Patcher, names: &mut HashSet<String>) {
    let banks: HashSet<&str> = patcher.boxes.iter().filter(|item| item.class() == "live.banks").map(MaxBox::id).collect();
    for cord in patcher.cords.iter().filter(|cord| banks.contains(cord.to.as_str())) {
        let Some(item) = patcher.find(&cord.from).filter(|item| item.maxclass() == "message") else { continue };
        for message in item.text().split(';').next().unwrap_or("").split(',') {
            let words = atoms(message);
            if matches!(words.first().map(String::as_str), Some("edit" | "new")) {
                // After the bank's index and name come pairs: a slot, and the parameter in it (- for none).
                names.extend(words.into_iter().skip(4).step_by(2).filter(|name| name != "-"));
            }
        }
    }
    for item in &patcher.boxes {
        if let Some(inner) = item.inner() {
            banked_while_running(inner, names);
        }
    }
}

/// Each box's cords: the outlet, the box and inlet it goes to.
fn downstream(patcher: &Patcher) -> HashMap<&str, Vec<(usize, &str, usize)>> {
    let mut next: HashMap<&str, Vec<(usize, &str, usize)>> = HashMap::new();
    for cord in &patcher.cords {
        next.entry(cord.from.as_str()).or_default().push((cord.outlet, cord.to.as_str(), cord.inlet));
    }
    next
}

/// Whether box `to` can be reached from box `from` along cords.
fn reaches(next: &HashMap<&str, Vec<(usize, &str, usize)>>, from: &str, to: &str) -> bool {
    let mut queue = VecDeque::from([from]);
    let mut seen = HashSet::from([from]);
    while let Some(id) = queue.pop_front() {
        if id == to {
            return true;
        }
        for &(_, next_id, _) in next.get(id).into_iter().flatten() {
            if seen.insert(next_id) {
                queue.push_back(next_id);
            }
        }
    }
    false
}

/// A Live parameter in the device.
#[derive(Debug, Clone)]
struct Parameter {
    /// The subpatchers it's in.
    at: String,
    label: String,
    maxclass: String,
    name: String,
    /// Shown to Live: automated and mapped (not stored only, not hidden).
    automatable: bool,
    /// Whether it says what it does in Live's Info View.
    info: bool,
}

/// The device's Live parameters, through its subpatchers and the files they load. A name the device's own list gives
/// (Max saves one with the device, after any abstraction's overrides) is the parameter's name.
fn parameters(device: &Patcher, patcher: &Patcher, path: &str, at: &str, found: &mut Vec<Parameter>) {
    for item in &patcher.boxes {
        let id_path = if path.is_empty() { item.id().to_string() } else { format!("{path}::{}", item.id()) };
        let enabled = match item.fields.get("parameter_enable").and_then(Value::as_f64) {
            Some(enable) => enable == 1.0,
            None => LIVE_PARAMETERS.contains(&item.maxclass()),
        };
        if enabled {
            let valueof = item.valueof();
            let text = |key: &str| valueof.and_then(|valueof| valueof.get(key)).and_then(Value::as_str).filter(|text| !text.is_empty());
            // The device's own list holds each parameter's name as Max last saved it. An override renames a parameter
            // inside an abstraction or a bpatcher; one on a top-level box is left over, and Max doesn't apply it.
            let list = device.fields.get("parameters");
            let listed = list.and_then(|list| list.get(&id_path)).and_then(|entry| entry.get(0)).and_then(Value::as_str);
            let overridden = list
                .and_then(|list| list.get("parameter_overrides"))
                .and_then(|overrides| overrides.get(&id_path))
                .and_then(|entry| entry.get("parameter_longname"))
                .and_then(Value::as_str)
                .filter(|_| id_path.contains("::"));
            let name = listed
                .or(overridden)
                .or(text("parameter_longname"))
                .or(Some(item.str("varname")).filter(|name| !name.is_empty()))
                .unwrap_or(item.maxclass())
                .to_string();
            let invisible = valueof.and_then(|valueof| valueof.get("parameter_invisible")).and_then(Value::as_f64).unwrap_or(0.0);
            let info = !item.str("annotation").trim().is_empty();
            found.push(Parameter {
                at: at.to_string(),
                label: item.label(),
                maxclass: item.maxclass().to_string(),
                name,
                automatable: invisible == 0.0,
                info,
            });
        }
        if let Some(inner) = item.inner() {
            parameters(device, inner, &id_path, &inside(at, item), found);
        }
    }
}

/// Whether a parameter's name is the one Max gives a new object: its class, or its class numbered ("live.dial[3]").
fn default_name(name: &str, maxclass: &str) -> bool {
    let bare = match name.rsplit_once('[') {
        Some((stem, number)) if number.ends_with(']') && number[..number.len() - 1].chars().all(|c| c.is_ascii_digit()) => stem,
        _ => name,
    };
    bare == maxclass || (bare.starts_with("live.") && LIVE_PARAMETERS.contains(&bare))
}

/// A Push bank: its name and its eight slots ("-" for an empty one).
struct Bank {
    name: String,
    slots: Vec<String>,
}

/// The device's Push banks, in order.
fn banks(patcher: &Patcher) -> Vec<Bank> {
    let Some(banks) = patcher.fields.get("parameters").and_then(|list| list.get("parameterbanks")).and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut banks: Vec<(u64, Bank)> = banks
        .values()
        .map(|bank| {
            let slots = bank["parameters"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            (bank["index"].as_u64().unwrap_or(0), Bank { name: bank["name"].as_str().unwrap_or("").to_string(), slots })
        })
        .collect();
    banks.sort_by_key(|(index, _)| *index);
    banks.into_iter().map(|(_, bank)| bank).collect()
}

/// The Params a gen codebox declares, each with its parenthesised default ("" without one), in order.
fn params_declared(code: &str) -> Vec<(String, String)> {
    let bare = without_comments(code);
    let mut declared = Vec::new();
    let mut rest = bare.as_str();
    while let Some(at) = find_word(rest, "Param") {
        rest = &rest[at + "Param".len()..];
        let end = rest.find(';').unwrap_or(rest.len());
        let statement = &rest[..end];
        rest = &rest[end..];
        let mut depth = 0i32;
        let mut part = String::new();
        let mut parts = Vec::new();
        for c in statement.chars() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(std::mem::take(&mut part));
                    continue;
                }
                _ => {}
            }
            part.push(c);
        }
        parts.push(part);
        for part in parts {
            let part = part.trim();
            let name: String = part.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let default = part[name.len()..].trim().to_string();
            declared.push((name, default));
        }
    }
    declared
}

/// Where `word` first stands as a whole word in `text`.
fn find_word(text: &str, word: &str) -> Option<usize> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(found) = text[from..].find(word) {
        let at = from + found;
        let before = text[..at].chars().next_back().is_some_and(ident);
        let after = text[at + word.len()..].chars().next().is_some_and(ident);
        if !before && !after {
            return Some(at);
        }
        from = at + word.len();
    }
    None
}

/// The code with its comments taken out.
fn without_comments(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' && chars.peek() == Some(&'/') {
            for c in chars.by_ref() {
                if c == '\n' {
                    out.push('\n');
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut last = ' ';
            for c in chars.by_ref() {
                if last == '*' && c == '/' {
                    break;
                }
                last = c;
            }
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::patch::reference::Object;
    use crate::devices::patch::NoFiles;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn read(value: Value) -> Patcher {
        Patcher::read(&value, &NoFiles)
    }

    fn newobj(id: &str, text: &str, rect: [f64; 4], ins: usize, outs: usize) -> Value {
        json!({ "box": { "id": id, "maxclass": "newobj", "text": text, "patching_rect": rect, "numinlets": ins, "numoutlets": outs, "outlettype": vec![""; outs] } })
    }

    fn cord(from: &str, outlet: usize, to: &str, inlet: usize) -> Value {
        json!({ "patchline": { "source": [from, outlet], "destination": [to, inlet] } })
    }

    fn rules(report: &Report) -> Vec<&str> {
        report.findings.iter().map(|finding| finding.rule).collect()
    }

    #[test]
    fn a_cord_over_a_box_overlapping_boxes_and_a_cord_running_up_are_each_found() {
        let patcher = read(json!({ "boxes": [
            newobj("obj-1", "metro 100", [40., 40., 70., 22.], 2, 1),
            newobj("obj-2", "counter", [40., 120., 60., 22.], 5, 4),
            newobj("obj-3", "print", [20., 80., 100., 22.], 1, 0),
            newobj("obj-4", "print", [90., 85., 50., 22.], 1, 0),
            newobj("obj-5", "+ 1", [200., 100., 40., 22.], 2, 1)
        ], "lines": [cord("obj-1", 0, "obj-2", 0), cord("obj-2", 0, "obj-5", 0)] }));
        let report = check(&patcher);
        assert_eq!(rules(&report), [BOXES_OVERLAP, CORD_OVER_BOX, CORD_UPWARD], "{report:#?}");
        assert_eq!(
            report.findings[1].to_string(),
            "layout.cord-over-box: the cord from [metro 100] to [counter] runs over [print] (obj-3): move the box out of its way, or route the cord around it.",
            "two boxes alike are told apart"
        );
        assert_eq!(report.looked_at[CORD_OVER_BOX], 2);
        // Routed around [print] with a segmented cord, the first is fine; a cord that closes a loop may run up.
        let mut routed = patcher.clone();
        routed.cords[0].midpoints = vec![[43.5, 70.], [10., 70.], [10., 110.], [43.5, 110.]];
        routed.boxes.truncate(3);
        routed.cords.truncate(1);
        let mut back = crate::devices::patch::Cord::new("obj-2", 0, "obj-1", 0);
        back.midpoints = vec![[43.5, 150.], [150., 150.], [150., 30.], [43.5, 30.]];
        routed.cords.push(back);
        assert_eq!(rules(&check(&routed)), Vec::<&str>::new(), "{:#?}", check(&routed));
    }

    #[test]
    fn a_fan_out_that_meets_again_at_hot_and_cold_inlets_is_found_and_a_trigger_settles_it() {
        // [number] → [+ ] left and right: the sum uses the old right-hand value or the new one, by position.
        let fan = read(json!({ "boxes": [
            newobj("obj-1", "random 10", [40., 20., 70., 22.], 2, 1),
            newobj("obj-2", "* 2", [0., 80., 40., 22.], 2, 1),
            newobj("obj-3", "+", [0., 140., 80., 22.], 2, 1)
        ], "lines": [cord("obj-1", 0, "obj-2", 0), cord("obj-1", 0, "obj-3", 1), cord("obj-2", 0, "obj-3", 0)] }));
        let report = check(&fan);
        assert_eq!(rules(&report), [FAN_OUT_REJOINS], "{report:#?}");
        assert!(
            report.findings[0].says.starts_with("outlet 0 of [random 10] reaches [+] twice, at cold inlet 1"),
            "{}",
            report.findings[0]
        );
        let ordered = read(json!({ "boxes": [
            newobj("obj-1", "random 10", [40., 20., 70., 22.], 2, 1),
            newobj("obj-4", "t i i", [40., 50., 40., 22.], 1, 2),
            newobj("obj-2", "* 2", [0., 80., 40., 22.], 2, 1),
            newobj("obj-3", "+", [0., 140., 80., 22.], 2, 1)
        ], "lines": [cord("obj-1", 0, "obj-4", 0), cord("obj-4", 0, "obj-2", 0), cord("obj-4", 1, "obj-3", 1), cord("obj-2", 0, "obj-3", 0)] }));
        assert_eq!(rules(&check(&ordered)), Vec::<&str>::new());
        // Branches to separate places, or into objects that decide for themselves, are fine as they are.
        let apart = read(json!({ "boxes": [
            newobj("obj-1", "random 10", [40., 20., 70., 22.], 2, 1),
            newobj("obj-2", "prepend drive", [40., 80., 80., 22.], 1, 1),
            newobj("obj-3", "pak 0 0", [160., 80., 60., 22.], 2, 1)
        ], "lines": [cord("obj-1", 0, "obj-2", 0), cord("obj-1", 0, "obj-3", 0), cord("obj-1", 0, "obj-3", 1)] }));
        assert_eq!(rules(&check(&apart)), Vec::<&str>::new());
    }

    fn message(id: &str, text: &str, rect: [f64; 4]) -> Value {
        json!({ "box": { "id": id, "maxclass": "message", "text": text, "patching_rect": rect, "numinlets": 2, "numoutlets": 1, "outlettype": [""] } })
    }

    fn order_says(report: &Report) -> Vec<&str> {
        report.findings.iter().filter(|finding| finding.rule == FAN_OUT_REJOINS).map(|finding| finding.says.as_str()).collect()
    }

    #[test]
    fn a_branch_that_sets_the_inlet_itself_or_a_message_that_only_sets_something_settles_a_fan_out() {
        // Both branches pass one [t l l], which sets [+]'s right inlet before its left: each sets it itself.
        let shared = read(json!({ "boxes": [
            newobj("obj-1", "random 10", [40., 20., 70., 22.], 2, 1),
            newobj("obj-2", "* 1", [0., 60., 40., 22.], 2, 1),
            newobj("obj-3", "* 2", [80., 60., 40., 22.], 2, 1),
            newobj("obj-4", "t l l", [40., 100., 40., 22.], 1, 2),
            newobj("obj-5", "+", [40., 140., 40., 22.], 2, 1)
        ], "lines": [cord("obj-1", 0, "obj-2", 0), cord("obj-1", 0, "obj-3", 0), cord("obj-2", 0, "obj-4", 0), cord("obj-3", 0, "obj-4", 0),
            cord("obj-4", 1, "obj-5", 1), cord("obj-4", 0, "obj-5", 0)] }));
        assert_eq!(order_says(&check(&shared)), Vec::<&str>::new());
        // Two of a fan-out's own cords into one box go right to left, wherever the box sits.
        let direct = read(json!({ "boxes": [
            newobj("obj-1", "random 10", [40., 20., 70., 22.], 2, 1),
            newobj("obj-5", "+", [40., 140., 40., 22.], 2, 1)
        ], "lines": [cord("obj-1", 0, "obj-5", 0), cord("obj-1", 0, "obj-5", 1)] }));
        assert_eq!(order_says(&check(&direct)), Vec::<&str>::new());

        // Hiding a toggle doesn't make it send; setting it only keeps the value, and a bang beside it isn't settled.
        let mut reference = Reference::default();
        reference.objects.insert("toggle".into(), Object { quiet: BTreeSet::from(["set".to_string()]), ..Object::default() });
        reference.objects.insert("jbox".into(), Object { quiet: BTreeSet::from(["hidden".to_string()]), ..Object::default() });
        let toggle = |id: &str, rect: [f64; 4]| json!({ "box": { "id": id, "maxclass": "toggle", "patching_rect": rect, "numinlets": 1, "numoutlets": 1, "outlettype": ["int"] } });
        let hidden = read(json!({ "boxes": [
            newobj("obj-1", "random 10", [40., 20., 70., 22.], 2, 1),
            message("obj-2", "hidden $1", [0., 60., 60., 22.]),
            toggle("obj-3", [0., 100., 24., 24.]),
            newobj("obj-5", "+", [0., 160., 80., 22.], 2, 1)
        ], "lines": [cord("obj-1", 0, "obj-2", 0), cord("obj-2", 0, "obj-3", 0), cord("obj-3", 0, "obj-5", 0), cord("obj-1", 0, "obj-5", 1)] }));
        assert_eq!(order_says(&check(&hidden)).len(), 1, "without Max's reference, the toggle may send");
        assert_eq!(order_says(&check_with(&hidden, standard(), Some(&reference))), Vec::<&str>::new());
        let set_and_bang = read(json!({ "boxes": [
            newobj("obj-1", "random 10", [40., 20., 70., 22.], 2, 1),
            message("obj-2", "set $1", [0., 60., 50., 22.]),
            message("obj-4", "bang", [80., 60., 40., 22.]),
            toggle("obj-3", [40., 120., 24., 24.])
        ], "lines": [cord("obj-1", 0, "obj-2", 0), cord("obj-1", 0, "obj-4", 0), cord("obj-2", 0, "obj-3", 0), cord("obj-4", 0, "obj-3", 0)] }));
        assert_eq!(
            order_says(&check_with(&set_and_bang, standard(), Some(&reference))),
            ["outlet 0 of [random 10] reaches toggle twice, once with `set` (which only sets it) and once to make it send: which arrives first depends on where boxes sit. Send it through a [t] whose right outlet sends the `set`."]
        );

        // A code box's function that doesn't reach an outlet only sets something; one that does, sends.
        let code = |selector: &str| {
            read(json!({ "boxes": [
                newobj("obj-1", "random 10", [40., 20., 70., 22.], 2, 1),
                message("obj-2", &format!("{selector} $1"), [0., 60., 80., 22.]),
                { "box": { "id": "obj-3", "maxclass": "v8.codebox", "filename": "none", "patching_rect": [0., 100., 120., 40.], "numinlets": 1, "numoutlets": 1,
                    "code": "var kept = 0;\nfunction keep(v) { kept = v; }\nfunction play(v) { outlet(0, kept + v); }" } },
                newobj("obj-5", "+", [0., 180., 80., 22.], 2, 1)
            ], "lines": [cord("obj-1", 0, "obj-2", 0), cord("obj-2", 0, "obj-3", 0), cord("obj-3", 0, "obj-5", 0), cord("obj-1", 0, "obj-5", 1)] }))
        };
        assert_eq!(order_says(&check(&code("keep"))), Vec::<&str>::new());
        assert_eq!(order_says(&check(&code("play"))).len(), 1);
    }

    #[test]
    fn names_every_copy_shares_parameters_banks_and_gen_params_are_each_found() {
        let device = read(json!({ "boxes": [
            newobj("obj-1", "s level", [40., 20., 60., 22.], 1, 0),
            newobj("obj-2", "r ---level", [140., 20., 70., 22.], 0, 1),
            newobj("obj-3", "buffer~ #0-loops", [240., 20., 110., 22.], 1, 2),
            { "box": { "id": "obj-4", "maxclass": "live.dial", "patching_rect": [40., 60., 44., 48.], "parameter_enable": 1,
                "saved_attribute_attributes": { "valueof": { "parameter_longname": "live.dial[1]" } } } },
            { "box": { "id": "obj-5", "maxclass": "live.dial", "patching_rect": [100., 60., 44., 48.], "annotation": "How hard it drives.",
                "saved_attribute_attributes": { "valueof": { "parameter_longname": "Drive" } } } },
            { "box": { "id": "obj-6", "maxclass": "live.text", "patching_rect": [160., 60., 44., 20.],
                "saved_attribute_attributes": { "valueof": { "parameter_longname": "live.text", "parameter_invisible": 2 } } } },
            { "box": { "id": "obj-7", "maxclass": "newobj", "text": "gen~", "patching_rect": [240., 60., 40., 22.], "numinlets": 1, "numoutlets": 1,
                "patcher": { "classnamespace": "dsp.gen", "boxes": [{ "box": { "id": "obj-1", "maxclass": "codebox",
                    "code": "// Param ghost(3);\nParam drive(1), tone(0.5);\nParam drive(10);\nout1 = in1 * drive;" } }], "lines": [] } } }
        ], "lines": [], "parameters": { "parameterbanks": { "0": { "index": 0, "name": "Main", "parameters": ["Drive", "Drive", "Tone", "-"] } } } }));
        let report = check(&device);
        let said: Vec<String> = report.findings.iter().map(Finding::to_string).collect();
        assert_eq!(
            said,
            [
                "names.shared-by-copies: [s level] names \"level\", which every copy of the device (and any other device using the name) shares: call it ---level so each copy has its own.",
                "gen.param-twice (in [gen~]): the codebox declares Param drive 2 times, with defaults (1) and (10): declare it once.",
                "params.default-name: live.dial \"live.dial[1]\" is a Live parameter named \"live.dial[1]\", Max's default: name it for what it does (its parameter_longname).",
                "params.no-info: live.dial \"live.dial[1]\" has no info text: say in a sentence what it does (its annotation), for Live's Info View.",
                "params.bank-repeat: Push bank \"Main\" lists \"Drive\" twice: one of its slots was meant for another parameter.",
                "params.bank-unknown: Push bank \"Main\" lists \"Tone\", which no parameter is named: name a parameter in its place.",
                "params.bank-missing: \"live.dial[1]\" is in none of the device's Push banks, so Push can't reach it: give it a slot, or set its Parameter Visibility to Stored Only or Hidden if it isn't one to play.",
            ],
            "the hidden live.text isn't a parameter Live shows"
        );
        assert_eq!(report.findings[0].level, Level::Warn);
        assert_eq!(report.findings[3].level, Level::Advice);
        assert_eq!(report.looked_at[SHARED_BY_COPIES], 3);
        assert_eq!(report.looked_at[PARAM_TWICE], 3);
    }

    #[test]
    fn a_parameter_the_device_banks_while_it_runs_is_in_a_bank() {
        let device = |cords: Vec<Value>| {
            read(json!({ "boxes": [
                { "box": { "id": "obj-1", "maxclass": "live.dial", "patching_rect": [40., 20., 44., 48.], "annotation": "a",
                    "saved_attribute_attributes": { "valueof": { "parameter_longname": "Drive" } } } },
                { "box": { "id": "obj-2", "maxclass": "live.dial", "patching_rect": [100., 20., 44., 48.], "annotation": "b",
                    "saved_attribute_attributes": { "valueof": { "parameter_longname": "Rate Synced" } } } },
                message("obj-3", "edit 0 Main 1 \"Rate Synced\", edit 0 Main 2 -", [40., 100., 200., 22.]),
                newobj("obj-4", "live.banks", [40., 140., 70., 22.], 1, 1)
            ], "lines": cords, "parameters": { "parameterbanks": { "0": { "index": 0, "name": "Main", "parameters": ["Drive", "-", "-", "-"] } } } }))
        };
        let banked = |patcher: &Patcher| check(patcher).findings.iter().filter(|finding| finding.rule == BANK_MISSING).count();
        assert_eq!(banked(&device(vec![cord("obj-3", 0, "obj-4", 0)])), 0, "[live.banks] puts it in bank 0 while the device runs");
        assert_eq!(banked(&device(vec![])), 1);
    }

    #[test]
    fn two_parameters_with_one_name_are_found_once() {
        let device = read(json!({ "boxes": [
            { "box": { "id": "obj-1", "maxclass": "live.dial", "patching_rect": [40., 60., 44., 48.], "annotation": "a",
                "saved_attribute_attributes": { "valueof": { "parameter_longname": "Time" } } } },
            { "box": { "id": "obj-2", "maxclass": "live.numbox", "patching_rect": [100., 60., 44., 15.], "annotation": "b",
                "saved_attribute_attributes": { "valueof": { "parameter_longname": "Time" } } } }
        ], "lines": [] }));
        let said: Vec<String> = check(&device).findings.iter().map(Finding::to_string).collect();
        assert_eq!(said, ["params.same-name: 2 Live parameters are named \"Time\" (live.dial \"Time\", live.numbox \"Time\"): Max renames all but one when the device loads, and automation follows the wrong one. Name each for what it does."]);
    }

    #[test]
    fn a_parameters_name_is_the_one_max_saved_and_an_override_counts_inside_an_abstraction() {
        let dial = json!({ "box": { "id": "obj-9", "maxclass": "live.dial", "patching_rect": [0., 0., 44., 48.], "annotation": "a",
            "saved_attribute_attributes": { "valueof": { "parameter_longname": "Amount" } } } });
        let device = read(json!({ "boxes": [
            { "box": { "id": "obj-1", "maxclass": "live.numbox", "patching_rect": [40., 60., 44., 15.], "annotation": "a",
                "saved_attribute_attributes": { "valueof": { "parameter_longname": "Volume" } } } },
            { "box": { "id": "obj-2", "maxclass": "bpatcher", "patching_rect": [100., 60., 60., 60.], "embed": 1,
                "patcher": { "boxes": [dial.clone()], "lines": [] } } },
            { "box": { "id": "obj-3", "maxclass": "bpatcher", "patching_rect": [200., 60., 60., 60.], "embed": 1,
                "patcher": { "boxes": [dial], "lines": [] } } }
        ], "lines": [], "parameters": {
            "obj-1": ["Volume", "Volume", 0],
            "parameter_overrides": { "obj-1": { "parameter_longname": "Old Name" }, "obj-2::obj-9": { "parameter_longname": "Left" },
                "obj-3::obj-9": { "parameter_longname": "Right" } },
            "parameterbanks": { "0": { "index": 0, "name": "Main", "parameters": ["Volume", "Left", "Right", "-"] } }
        } }));
        assert_eq!(
            check(&device).findings,
            Vec::<Finding>::new(),
            "the dial in each bpatcher is renamed; the top-level override is left over"
        );
    }

    #[test]
    fn an_object_max_doesnt_know_and_a_cord_to_a_port_a_box_hasnt_got_are_found() {
        use crate::devices::patch::reference::Count;
        let mut reference = Reference::default();
        for (class, inlets, outlets) in [
            ("metro", Count::Fixed { count: 2 }, Count::Fixed { count: 1 }),
            ("route", Count::Arguments { plus: 1, bare: 2 }, Count::Arguments { plus: 1, bare: 2 }),
        ] {
            reference.objects.insert(class.into(), Object { module: "max".into(), inlets, outlets, seen: 1, ..Object::default() });
        }
        let patcher = read(json!({ "boxes": [
            newobj("obj-1", "metro 100", [40., 20., 70., 22.], 2, 1),
            { "box": { "id": "obj-2", "maxclass": "newobj", "text": "route a b", "patching_rect": [40., 80., 70., 22.] } },
            newobj("obj-3", "metor 100", [160., 20., 70., 22.], 2, 1)
        ], "lines": [cord("obj-1", 1, "obj-2", 0), cord("obj-1", 0, "obj-2", 3), cord("obj-1", 0, "obj-9", 0)] }));
        let said: Vec<String> = check_with(&patcher, standard(), Some(&reference)).findings.iter().map(Finding::to_string).collect();
        assert_eq!(
            said,
            [
                "patch.no-such-port: a cord leaves outlet 1 of [metro 100], which has 1 (counted from 0): use one of its outlets.",
                "patch.no-such-port: a cord enters inlet 3 of [route a b], which has 3 (counted from 0): use one of its inlets.",
                "patch.no-such-port: a cord joins obj-9, which isn't in the patcher: take the cord out, or add the box.",
                "patch.unknown-object: [metor 100] isn't an object this machine's Max knows: check its name (a typo, an object from a package that isn't installed, or an abstraction the device doesn't hold).",
            ],
            "route a b's ports come from the reference: it wasn't saved with any"
        );
        assert!(
            check(&patcher).findings.iter().all(|finding| finding.rule != UNKNOWN_OBJECT),
            "without a reference, no object is called unknown"
        );
    }

    #[test]
    fn params_are_read_from_declarations_with_comments_and_commas() {
        assert_eq!(
            params_declared("/* Param x(1); */ Param a(1, min=0, max=2), b;\n// Param c(3);\nHistory Paramx(0); Param d(2);"),
            [("a".to_string(), "(1, min=0, max=2)".to_string()), ("b".to_string(), String::new()), ("d".to_string(), "(2)".to_string())]
        );
        assert!(default_name("live.text[4]", "live.text") && default_name("jsui[2]", "jsui") && !default_name("Drive[2]", "live.dial"));
    }
}
