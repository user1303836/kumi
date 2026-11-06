//! Bounded, offline execution of a declared set of Max message objects. This
//! reads the normalized patcher that is written, including its cord order and
//! nested patchers. Unsupported objects/messages stop the test, never pass it.
//! No Max, Live, JavaScript, files or network are executed by this interpreter.

use std::{cell::Cell, collections::HashMap, rc::Rc};

use serde_json::{Map, Value};

use super::reference::atoms;
use crate::devices::amxd::DeviceType;
use crate::devices::harness::{bytes_of, describe, events_of, hanging, matches, Checked, PROBE};
use crate::devices::spec::{Control, MidiEvent, PatchSpec};

const MOST_NODES: usize = 2_000;
const MOST_CORDS: usize = 16_384;
const MOST_STEPS: usize = 100_000;
const MOST_PENDING: usize = 4_096;
const MOST_BYTES: usize = 16_384;
const MOST_ATOMS: usize = 128;
const MOST_MESSAGE_BYTES: usize = 4_096;
const EXACT_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Debug)]
enum Problem {
    Unsupported(String),
    Limit(String),
}

impl From<String> for Problem {
    fn from(why: String) -> Self {
        Self::Unsupported(why)
    }
}

impl From<&str> for Problem {
    fn from(why: &str) -> Self {
        Self::Unsupported(why.into())
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(why) | Self::Limit(why) => formatter.write_str(why),
        }
    }
}

impl Problem {
    fn at(self, label: &str) -> Self {
        match self {
            Self::Unsupported(why) => Self::Unsupported(format!("{label}: {why}")),
            Self::Limit(why) => Self::Limit(format!("{label}: {why}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Atom {
    Int(i64),
    Float(f64),
    Symbol(String),
}

impl Atom {
    fn parse(text: &str) -> Self {
        text.parse::<i64>()
            .map(Self::Int)
            .unwrap_or_else(|_| text.parse::<f64>().ok().map_or_else(|| Self::Symbol(text.into()), Self::Float))
    }

    fn number(&self) -> Result<f64, String> {
        match self {
            Self::Int(number) if number.unsigned_abs() <= EXACT_INTEGER as u64 => Ok(*number as f64),
            Self::Float(number) if number.is_finite() && number.abs() <= EXACT_INTEGER as f64 => Ok(*number),
            _ => Err("a numeric inlet received a symbol, non-finite number or value outside the exact supported range".into()),
        }
    }

    fn cast(&self, value: &Atom) -> Result<Atom, String> {
        match self {
            Self::Int(_) => Ok(Self::Int(value.number()?.trunc() as i64)),
            Self::Float(_) => Ok(Self::Float(value.number()?)),
            Self::Symbol(_) => match value {
                Self::Symbol(_) => Ok(value.clone()),
                _ => Err("numeric-to-symbol coercion is not simulated".into()),
            },
        }
    }
}

/// An empty vector is bang. Otherwise these are Max's message atoms, with a
/// leading symbol retaining its selector. Types matter to pack and trigger.
type Message = Vec<Atom>;

fn message_bytes(message: &[Atom]) -> usize {
    message
        .iter()
        .map(|atom| match atom {
            Atom::Symbol(word) => word.len() + 16,
            _ => 16,
        })
        .sum()
}

fn message(text: &str) -> Message {
    let result: Message = atoms(text).iter().map(|word| Atom::parse(word)).collect();
    if result == [Atom::Symbol("bang".into())] {
        Vec::new()
    } else {
        result
    }
}

fn first_number(message: &[Atom]) -> Result<f64, String> {
    message.first().ok_or_else(|| "bang has no numeric value here".to_string())?.number()
}

fn normalize(mut message: Message) -> Result<Message, String> {
    let Some(Atom::Symbol(selector)) = message.first() else { return Ok(message) };
    match selector.as_str() {
        "bang" if message.len() == 1 => Ok(vec![]),
        "list" => Ok(message.into_iter().skip(1).collect()),
        "int" | "float" if message.len() == 2 => {
            let integer = selector == "int";
            Ok(vec![numeric(message[1].number()?, integer)?])
        }
        "symbol" if message.len() == 2 && matches!(message[1], Atom::Symbol(_)) => Ok(vec![message.remove(1)]),
        "bang" | "int" | "float" | "symbol" => Err("this typed message's arguments are not simulated".into()),
        _ => Ok(message),
    }
}

fn numeric(value: f64, integer: bool) -> Result<Atom, String> {
    if !value.is_finite() || value.abs() > EXACT_INTEGER as f64 {
        return Err("an operation produced a value outside the finite exact supported range".into());
    }
    Ok(if integer { Atom::Int(value.trunc() as i64) } else { Atom::Float(value) })
}

#[derive(Clone)]
enum Kind {
    In,
    Out,
    Control(String),
    Passive,
    Print,
    Inlet,
    Outlet { parent: usize, outlet: usize },
    Patcher(Vec<usize>),
    Parse { status: u8, data: Vec<u8>, hires: bool },
    Format { channel: i64, hires: bool },
    Number(Atom),
    Arithmetic { operation: String, left: Atom, right: Atom },
    Pack { stored: Vec<Atom>, all: bool },
    Unpack(Vec<Atom>),
    Trigger(Vec<String>),
    Route { keys: Vec<Atom>, select: bool },
    Gate { count: usize, open: usize },
    Prepend(Message),
    Message(Message),
    Delay { delay: f64, generation: u64 },
    Pipe { stored: Vec<Atom>, delay: f64, generation: u64 },
}

struct Node {
    label: String,
    kind: Kind,
    ports: (usize, usize),
}

#[derive(Clone)]
enum Work {
    Deliver(usize, usize, Message),
    Emit(usize, usize, Message),
    Schedule { node: usize, delay: f64, generation: u64, output: Vec<(usize, Message)> },
}

struct Timer {
    due: f64,
    order: u64,
    node: usize,
    generation: u64,
    output: Vec<(usize, Message)>,
}

struct Graph {
    nodes: Vec<Node>,
    cords: HashMap<(usize, usize), Vec<(usize, usize)>>,
    pending: Vec<Timer>,
    sent: Vec<(f64, f64)>,
    now: f64,
    order: u64,
    steps: Rc<Cell<usize>>,
    cord_count: usize,
}

fn defaults(arguments: &[String]) -> Vec<Atom> {
    let arguments = if arguments.is_empty() { vec!["0".to_string(), "0".to_string()] } else { arguments.to_vec() };
    arguments
        .iter()
        .map(|argument| match argument.as_str() {
            "i" => Atom::Int(0),
            "f" => Atom::Float(0.0),
            "s" => Atom::Symbol(String::new()),
            _ => Atom::parse(argument),
        })
        .collect()
}

fn kind(item: &Value, controls: &[Control]) -> Result<Kind, Problem> {
    let class = item["maxclass"].as_str().unwrap_or("");
    if matches!(class, "comment" | "live.comment" | "panel" | "live.line") {
        return Ok(Kind::Passive);
    }
    if item["id"].as_str().is_some_and(|id| id.starts_with("obj-control-")) {
        let name = item["varname"].as_str().unwrap_or("");
        if controls.iter().any(|control| control.name() == name) {
            return Ok(Kind::Control(name.into()));
        }
    }
    if class == "message" {
        let text = item["text"].as_str().unwrap_or("");
        if text.chars().any(|ch| matches!(ch, ';' | ',' | '\\' | '"')) {
            return Err("message separators, remote sends, quoting and escapes are not simulated".into());
        }
        let contents = message(text);
        for atom in &contents {
            if !matches!(atom, Atom::Symbol(_)) {
                atom.number()?;
            }
        }
        if contents.len() > MOST_ATOMS || message_bytes(&contents) > MOST_MESSAGE_BYTES {
            return Err(Problem::Limit("message-box contents exceed the simulator's size limit".into()));
        }
        return Ok(Kind::Message(contents));
    }
    if class == "inlet" {
        return Ok(Kind::Inlet);
    }
    let text = item["text"].as_str().unwrap_or(class);
    if text.chars().any(|ch| matches!(ch, '"' | '\\')) {
        return Err("quoted or escaped object arguments are not simulated".into());
    }
    let words = atoms(text);
    let Some(class) = words.first().map(String::as_str) else { return Err("a box has no class".into()) };
    let arguments = &words[1..];
    for argument in arguments {
        let atom = Atom::parse(argument);
        if !matches!(atom, Atom::Symbol(_)) {
            atom.number()?;
        }
    }
    if matches!(class, "midiparse" | "midiformat") {
        let mut args = arguments.to_vec();
        let hires = if args.ends_with(&["@hires".to_string(), "2".to_string()]) {
            args.truncate(args.len() - 2);
            true
        } else {
            false
        };
        if args.iter().any(|word| word.starts_with('@')) || args.len() > usize::from(class == "midiformat") {
            return Err("only default MIDI format or @hires 2 is simulated".into());
        }
        return if class == "midiparse" {
            Ok(Kind::Parse { status: 0, data: vec![], hires })
        } else {
            let channel = args.first().map_or(Ok(1.0), |value| Atom::parse(value).number())?;
            if channel < 1.0 {
                return Err("non-positive MIDI channels are not simulated".into());
            }
            Ok(Kind::Format { channel: channel.trunc() as i64, hires })
        };
    }
    if arguments.len() > MOST_ATOMS {
        return Err(Problem::Limit("the object exceeds the 128-argument limit".into()));
    }
    if arguments.iter().any(|word| word.starts_with('@') || word.len() > 256) {
        return Err("attributes or more than 128 arguments are not simulated for this object".into());
    }
    let one = || -> Result<Atom, String> {
        if arguments.len() > 1 {
            return Err("this object's extra arguments are not simulated".into());
        }
        Ok(arguments.first().map_or(Atom::Int(0), |word| Atom::parse(word)))
    };
    match class {
        "midiin" if arguments.is_empty() => Ok(Kind::In),
        "midiout" if arguments.is_empty() => Ok(Kind::Out),
        "print" => Ok(Kind::Print),
        "i" | "int" => Ok(Kind::Number(Atom::Int(one()?.number()?.trunc() as i64))),
        "f" | "float" => Ok(Kind::Number(Atom::Float(one()?.number()?))),
        "+" | "-" | "*" | "/" | "%" => {
            let right = one()?;
            right.number()?;
            let left = if matches!(right, Atom::Float(_)) { Atom::Float(0.0) } else { Atom::Int(0) };
            Ok(Kind::Arithmetic { operation: class.into(), left, right })
        }
        "pack" | "pak" => Ok(Kind::Pack { stored: defaults(arguments), all: class == "pak" }),
        "unpack" => Ok(Kind::Unpack(defaults(arguments))),
        "t" | "trigger" => {
            let types = if arguments.is_empty() { vec!["i".into(), "i".into()] } else { arguments.to_vec() };
            if types
                .iter()
                .any(|kind| !matches!(kind.as_str(), "b" | "bang" | "i" | "int" | "f" | "float" | "l" | "list" | "a" | "anything"))
            {
                return Err("only bang, int, float, list and anything trigger outlets are simulated".into());
            }
            Ok(Kind::Trigger(types))
        }
        "route" | "sel" | "select" => {
            if class == "route" && arguments.iter().any(|word| matches!(word.as_str(), "int" | "float" | "list" | "symbol")) {
                return Err("route's message-type matching is not simulated".into());
            }
            Ok(Kind::Route {
                keys: if arguments.is_empty() { vec![Atom::Int(0)] } else { arguments.iter().map(|word| Atom::parse(word)).collect() },
                select: class != "route",
            })
        }
        "gate" => {
            if arguments.len() > 2 {
                return Err("gate's extra arguments are not simulated".into());
            }
            let count = arguments.first().map_or(Ok(1.0), |word| Atom::parse(word).number())?;
            if count.fract() != 0.0 || !(1.0..=128.0).contains(&count) {
                return Err("gate needs 1 to 128 outlets".into());
            }
            let open = arguments.get(1).map_or(Ok(0.0), |word| Atom::parse(word).number())?;
            Ok(Kind::Gate { count: count as usize, open: open.max(0.0).min(count) as usize })
        }
        "prepend" => Ok(Kind::Prepend(arguments.iter().map(|word| Atom::parse(word)).collect())),
        "del" | "delay" => Ok(Kind::Delay { delay: one()?.number()?.max(0.0), generation: 0 }),
        "pipe" => {
            let delay = arguments.last().map_or(Ok(0.0), |word| Atom::parse(word).number())?.max(0.0);
            let values = if arguments.len() <= 1 { vec![Atom::Int(0)] } else { defaults(&arguments[..arguments.len() - 1]) };
            Ok(Kind::Pipe { stored: values, delay, generation: 0 })
        }
        _ => Err(format!("[{class}] is not supported by the message simulator").into()),
    }
}

fn ports(kind: &Kind, item: &Value) -> (usize, usize) {
    match kind {
        Kind::In | Kind::Inlet | Kind::Control(_) => (0, 1),
        Kind::Out | Kind::Print | Kind::Outlet { .. } => (1, 0),
        Kind::Passive => (0, 0),
        Kind::Patcher(_) => (item["numinlets"].as_u64().unwrap_or(0) as usize, item["numoutlets"].as_u64().unwrap_or(0) as usize),
        Kind::Parse { .. } => (1, 8),
        Kind::Format { .. } => (7, 2),
        Kind::Number(_) | Kind::Arithmetic { .. } | Kind::Message(_) | Kind::Delay { .. } => (2, 1),
        Kind::Pack { stored, .. } => (stored.len(), 1),
        Kind::Unpack(stored) => (1, stored.len()),
        Kind::Trigger(types) => (1, types.len()),
        Kind::Route { keys, .. } => (1, keys.len() + 1),
        Kind::Gate { count, .. } => (2, *count),
        Kind::Prepend(_) => (1, 1),
        Kind::Pipe { stored, .. } => (stored.len() + 1, stored.len()),
    }
}

/// Count before compiling any object, so an unsupported first box cannot hide
/// a fatal resource violation later in the document.
fn budget(patcher: &Value, depth: usize, counts: &mut (usize, usize)) -> Result<(), Problem> {
    if depth > 16 {
        return Err(Problem::Limit("nested patchers exceed 16 levels".into()));
    }
    let boxes = patcher["boxes"].as_array().ok_or("the patcher has no boxes")?;
    let lines = patcher["lines"].as_array().ok_or("the patcher has no cords")?;
    counts.0 = counts.0.saturating_add(boxes.len());
    counts.1 = counts.1.saturating_add(lines.len());
    if counts.0 > MOST_NODES {
        return Err(Problem::Limit("the message graph exceeds 2,000 boxes".into()));
    }
    if counts.1 > MOST_CORDS {
        return Err(Problem::Limit("the message graph exceeds 16,384 cords".into()));
    }
    for entry in boxes {
        let item = &entry["box"];
        if item["patcher"].is_object() {
            budget(&item["patcher"], depth + 1, counts)?;
        }
        if item["data"]["patcher"].is_object() {
            budget(&item["data"]["patcher"], depth + 1, counts)?;
        }
    }
    Ok(())
}

impl Graph {
    fn read(document: &Value, controls: &[Control], steps: Rc<Cell<usize>>) -> Result<Self, Problem> {
        budget(&document["patcher"], 0, &mut (0, 0))?;
        let mut graph =
            Self { nodes: vec![], cords: HashMap::new(), pending: vec![], sent: vec![], now: 0.0, order: 0, steps, cord_count: 0 };
        graph.add_patcher(&document["patcher"], controls, "", None, 0)?;
        Ok(graph)
    }

    fn add_patcher(
        &mut self,
        patcher: &Value,
        controls: &[Control],
        path: &str,
        parent: Option<usize>,
        depth: usize,
    ) -> Result<Vec<usize>, Problem> {
        if depth > 16 {
            return Err(Problem::Limit("nested patchers exceed 16 levels".into()));
        }
        let boxes = patcher["boxes"].as_array().ok_or("the patcher has no boxes")?;
        let lines = patcher["lines"].as_array().ok_or("the patcher has no cords")?;
        let mut ids = HashMap::new();
        let mut inlets = Vec::new();
        let mut positions = HashMap::new();
        for entry in boxes {
            if self.nodes.len() >= MOST_NODES {
                return Err(Problem::Limit("the message graph exceeds 2,000 boxes".into()));
            }
            let item = &entry["box"];
            let id = item["id"].as_str().ok_or("a box has no id")?;
            let index = self.nodes.len();
            if ids.insert(id, index).is_some() {
                return Err("two boxes have the same id".into());
            }
            positions.insert(index, item["patching_rect"][0].as_f64().unwrap_or(0.0));
            let label = format!("{path}{id} [{}]", item["text"].as_str().unwrap_or(item["maxclass"].as_str().unwrap_or("")));
            let node_kind = if item["patcher"].is_object() {
                let class = atoms(item["text"].as_str().unwrap_or("")).first().cloned().unwrap_or_default();
                if !matches!(class.as_str(), "p" | "patcher") {
                    return Err(format!("{label}: only message [p] subpatchers are simulated").into());
                }
                Kind::Patcher(vec![])
            } else if item["maxclass"] == "outlet" {
                Kind::Outlet {
                    parent: parent.ok_or("a top-level outlet has no parent")?,
                    outlet: item["index"].as_u64().unwrap_or(1).saturating_sub(1) as usize,
                }
            } else {
                kind(item, controls).map_err(|why| why.at(&label))?
            };
            if matches!(node_kind, Kind::Inlet) {
                inlets.push((item["index"].as_u64().unwrap_or(1), index));
            }
            let ports = ports(&node_kind, item);
            self.nodes.push(Node { label, kind: node_kind, ports });
            if item["patcher"].is_object() {
                let inputs = self.add_patcher(&item["patcher"], controls, &format!("{path}{id}/"), Some(index), depth + 1)?;
                self.nodes[index].kind = Kind::Patcher(inputs);
            }
        }
        let mut ordered: HashMap<(usize, usize), Vec<(Option<u64>, f64, usize, usize)>> = HashMap::new();
        for entry in lines {
            if self.cord_count >= MOST_CORDS {
                return Err(Problem::Limit("the message graph exceeds 16,384 cords".into()));
            }
            self.cord_count += 1;
            let line = &entry["patchline"];
            let source = *ids.get(line["source"][0].as_str().ok_or("a cord has no source")?).ok_or("a cord's source is missing")?;
            let target = *ids.get(line["destination"][0].as_str().ok_or("a cord has no target")?).ok_or("a cord's target is missing")?;
            let outlet = line["source"][1].as_u64().ok_or("a cord has no outlet")? as usize;
            let inlet = line["destination"][1].as_u64().ok_or("a cord has no inlet")? as usize;
            if outlet > 128 || inlet > 128 {
                return Err("a cord exceeds 128 message ports".into());
            }
            if outlet >= self.nodes[source].ports.1 || inlet >= self.nodes[target].ports.0 {
                return Err("a cord reaches a port this object's simulated form does not support".into());
            }
            if matches!(self.nodes[source].kind, Kind::Parse { .. }) && outlet == 7 {
                return Err("midievent output is not simulated".into());
            }
            if matches!(self.nodes[source].kind, Kind::Format { .. }) && outlet == 1 {
                return Err("midievent output is not simulated".into());
            }
            ordered.entry((source, outlet)).or_default().push((line["order"].as_u64(), positions[&target], target, inlet));
        }
        for (source, mut edges) in ordered {
            let explicit = edges.iter().filter(|edge| edge.0.is_some()).count();
            if explicit != 0 && explicit != edges.len() {
                return Err("a fan-out mixes ordered and unordered cords".into());
            }
            if explicit == 0 {
                edges.sort_by(|a, b| b.1.total_cmp(&a.1));
                if edges.windows(2).any(|pair| pair[0].1 == pair[1].1) {
                    return Err("a fan-out with tied box positions needs explicit cord order or trigger".into());
                }
            } else {
                edges.sort_by_key(|edge| edge.0);
                if edges.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                    return Err("a fan-out repeats a cord order".into());
                }
            }
            self.cords.insert(source, edges.into_iter().map(|(_, _, target, inlet)| (target, inlet)).collect());
        }
        inlets.sort_by_key(|&(order, _)| order);
        Ok(inlets.into_iter().map(|(_, index)| index).collect())
    }

    fn work(&mut self, actions: Vec<Work>) -> Result<(), Problem> {
        let mut stack: Vec<Work> = actions.into_iter().rev().collect();
        while let Some(action) = stack.pop() {
            self.steps.set(self.steps.get().saturating_add(1));
            if self.steps.get() > MOST_STEPS || stack.len() > MOST_PENDING {
                return Err(Problem::Limit("the simulator stopped at its 100,000-message work limit".into()));
            }
            match action {
                Work::Emit(node, outlet, message) => {
                    if let Some(edges) = self.cords.get(&(node, outlet)) {
                        if stack.len() + edges.len() > MOST_PENDING {
                            return Err(Problem::Limit("the simulator stopped at its 4,096-immediate-message limit".into()));
                        }
                        stack.extend(edges.iter().rev().map(|&(target, inlet)| Work::Deliver(target, inlet, message.clone())));
                    }
                }
                Work::Deliver(node, inlet, message) => {
                    if message.len() > MOST_ATOMS || message_bytes(&message) > MOST_MESSAGE_BYTES {
                        return Err(Problem::Limit("a message exceeds the 128-atom or 4,096-byte limit".into()));
                    }
                    let actions = self.dispatch(node, inlet, message).map_err(|why| why.at(&self.nodes[node].label))?;
                    if stack.len() + actions.len() > MOST_PENDING {
                        return Err(Problem::Limit("the simulator stopped at its 4,096-immediate-message limit".into()));
                    }
                    stack.extend(actions.into_iter().rev());
                }
                Work::Schedule { node, delay, generation, output } => {
                    if !delay.is_finite() || delay < 0.0 {
                        return Err("a timer has an invalid delay".into());
                    }
                    if self.pending.len() >= MOST_PENDING {
                        return Err(Problem::Limit("the simulator stopped at its 4,096-timer limit".into()));
                    }
                    if output.iter().map(|(_, message)| message_bytes(message)).sum::<usize>() > MOST_MESSAGE_BYTES {
                        return Err(Problem::Limit("a timer exceeds the 4,096-byte message limit".into()));
                    }
                    self.order += 1;
                    let due = self.now + delay;
                    if !due.is_finite() {
                        return Err(Problem::Limit("a timer exceeds the finite virtual clock".into()));
                    }
                    self.pending.push(Timer { due, order: self.order, node, generation, output });
                }
            }
        }
        Ok(())
    }

    fn advance(&mut self, until: f64) -> Result<(), Problem> {
        while let Some((index, _)) = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, timer)| timer.due <= until)
            .min_by(|(_, a), (_, b)| a.due.total_cmp(&b.due).then(a.order.cmp(&b.order)))
        {
            let timer = self.pending.remove(index);
            let generation = match &self.nodes[timer.node].kind {
                Kind::Delay { generation, .. } | Kind::Pipe { generation, .. } => *generation,
                _ => return Err("a timer lost its owner".into()),
            };
            if generation != timer.generation {
                continue;
            }
            self.now = timer.due;
            self.work(timer.output.into_iter().map(|(outlet, message)| Work::Emit(timer.node, outlet, message)).collect())?;
        }
        self.now = until;
        Ok(())
    }

    fn dispatch(&mut self, node: usize, inlet: usize, input: Message) -> Result<Vec<Work>, Problem> {
        let input = normalize(input)?;
        let emit = |outlet, message| Work::Emit(node, outlet, message);
        let mut output = Vec::new();
        let first = input.first().cloned();
        let command = match input.first() {
            Some(Atom::Symbol(command)) => Some(command.as_str()),
            _ => None,
        };
        match &mut self.nodes[node].kind {
            Kind::Passive | Kind::Print => {}
            Kind::In | Kind::Inlet | Kind::Control(_) => output.push(emit(0, input)),
            Kind::Outlet { parent, outlet } => output.push(Work::Emit(*parent, *outlet, input)),
            Kind::Patcher(inlets) => output.push(Work::Deliver(*inlets.get(inlet).ok_or("the subpatcher has no such inlet")?, 0, input)),
            Kind::Out => {
                let byte = first_number(&input)?;
                if input.len() != 1 || byte.fract() != 0.0 || !(0.0..240.0).contains(&byte) {
                    return Err("only complete channel MIDI bytes (0..239) are simulated at out".into());
                }
                if self.sent.len() >= MOST_BYTES {
                    return Err(Problem::Limit("the simulator stopped at its 16,384-output-byte limit".into()));
                }
                self.sent.push((self.now, byte));
            }
            Kind::Number(stored) => {
                if inlet > 1 {
                    return Err("the number has no such inlet".into());
                }
                if command == Some("set") && input.len() == 2 {
                    *stored = stored.cast(&input[1])?;
                } else {
                    if let Some(first) = first {
                        *stored = stored.cast(&first)?;
                    }
                    if inlet == 0 {
                        output.push(emit(0, vec![stored.clone()]));
                    }
                }
            }
            Kind::Arithmetic { operation, left, right } => {
                if inlet > 1 || input.len() > 2 {
                    return Err("this arithmetic message is not simulated".into());
                }
                if inlet == 1 {
                    *right = right.cast(first.as_ref().ok_or("bang in a cold arithmetic inlet is not simulated")?)?;
                } else {
                    if let Some(first) = first {
                        *left = left.cast(&first)?;
                    }
                    if let Some(second) = input.get(1) {
                        *right = right.cast(second)?;
                    }
                    let (a, b) = (left.number()?, right.number()?);
                    let value = match operation.as_str() {
                        "+" => a + b,
                        "-" => a - b,
                        "*" => a * b,
                        "/" if b != 0.0 => a / b,
                        "%" if b != 0.0 => a % b,
                        _ => return Err("division/remainder by zero is not simulated".into()),
                    };
                    output.push(emit(0, vec![numeric(value, matches!(left, Atom::Int(_)))?]));
                }
            }
            Kind::Pack { stored, all } => {
                if inlet >= stored.len() {
                    return Err("pack has no such inlet".into());
                }
                for (slot, value) in stored.iter_mut().skip(inlet).zip(&input) {
                    *slot = slot.cast(value)?;
                }
                if inlet == 0 || *all {
                    output.push(emit(0, stored.clone()));
                }
            }
            Kind::Unpack(stored) => {
                if inlet != 0 {
                    return Err("unpack has no such inlet".into());
                }
                for (slot, value) in stored.iter_mut().zip(&input) {
                    *slot = slot.cast(value)?;
                }
                let length = if input.is_empty() { stored.len() } else { input.len().min(stored.len()) };
                output.extend((0..length).rev().map(|index| emit(index, vec![stored[index].clone()])));
            }
            Kind::Trigger(types) => {
                if inlet != 0 {
                    return Err("trigger has no such inlet".into());
                }
                for (outlet, kind) in types.iter().enumerate().rev() {
                    let value = match kind.as_str() {
                        "b" | "bang" => vec![],
                        "a" | "anything" => input.clone(),
                        "l" | "list" => {
                            if input.is_empty() {
                                vec![Atom::Int(0)]
                            } else {
                                input.clone()
                            }
                        }
                        "i" | "int" => vec![Atom::Int(if input.len() == 1 {
                            first.as_ref().and_then(|atom| atom.number().ok()).unwrap_or(0.0).trunc() as i64
                        } else {
                            0
                        })],
                        "f" | "float" => vec![Atom::Float(if input.len() == 1 {
                            first.as_ref().and_then(|atom| atom.number().ok()).unwrap_or(0.0)
                        } else {
                            0.0
                        })],
                        _ => return Err("unknown trigger conversion".into()),
                    };
                    output.push(emit(outlet, value));
                }
            }
            Kind::Route { keys, select } => {
                if inlet != 0 {
                    return Err("changing route/select keys is not simulated".into());
                }
                let head = first.unwrap_or(Atom::Symbol("bang".into()));
                if keys.iter().any(|key| matches!((key, &head), (Atom::Int(_), Atom::Float(_)) | (Atom::Float(_), Atom::Int(_)))) {
                    return Err("mixed integer/float route matching is not simulated".into());
                }
                if let Some(index) = keys.iter().position(|key| *key == head) {
                    output.push(emit(index, if *select { vec![] } else { input.into_iter().skip(1).collect() }));
                } else {
                    output.push(emit(keys.len(), input));
                }
            }
            Kind::Gate { count, open } => {
                if inlet == 0 {
                    if input.len() != 1 {
                        return Err("this gate control message is not simulated".into());
                    }
                    *open = first_number(&input)?.max(0.0).min(*count as f64) as usize;
                } else if inlet == 1 {
                    if *open > 0 {
                        output.push(emit(*open - 1, input));
                    }
                } else {
                    return Err("gate has no such inlet".into());
                }
            }
            Kind::Prepend(prefix) => {
                if inlet != 0 {
                    return Err("changing prepend's prefix is not simulated".into());
                }
                let mut message = prefix.clone();
                message.extend(input);
                output.push(emit(0, message));
            }
            Kind::Message(text) => {
                if inlet != 0 {
                    return Err("changing a message box is not simulated".into());
                }
                if matches!(command, Some("set" | "append" | "prepend" | "add2" | "clear" | "replace")) {
                    return Err("message-box editing commands are not simulated".into());
                }
                let mut expanded = Vec::new();
                for atom in text.iter() {
                    let value = match atom {
                        Atom::Symbol(word) if word.starts_with('$') => {
                            let index = word[1..]
                                .parse::<usize>()
                                .ok()
                                .filter(|index| *index > 0)
                                .ok_or("only whole $1..$128 message substitutions are simulated")?;
                            input.get(index - 1).cloned().ok_or("a message substitution has no input atom")?
                        }
                        _ => atom.clone(),
                    };
                    expanded.push(value);
                }
                output.push(emit(0, expanded));
            }
            Kind::Delay { delay, generation } => {
                if command == Some("stop") {
                    if inlet != 0 || input.len() != 1 {
                        return Err("this delay stop message is not simulated".into());
                    }
                    *generation += 1;
                } else if inlet == 1 {
                    if input.len() != 1 {
                        return Err("transport-relative delay times are not simulated".into());
                    }
                    *delay = first_number(&input)?.max(0.0);
                } else if inlet == 0 {
                    if input.len() > 1 {
                        return Err("transport-relative delay times are not simulated".into());
                    }
                    if !input.is_empty() {
                        *delay = first_number(&input)?.max(0.0);
                    }
                    *generation += 1;
                    output.push(Work::Schedule { node, delay: *delay, generation: *generation, output: vec![(0, vec![])] });
                } else {
                    return Err("delay has no such inlet".into());
                }
            }
            Kind::Pipe { stored, delay, generation } => {
                if matches!(command, Some("clear" | "stop")) {
                    if inlet != 0 || input.len() != 1 {
                        return Err("this pipe cancellation message is not simulated".into());
                    }
                    *generation += 1;
                } else if inlet == stored.len() {
                    *delay = first_number(&input)?.max(0.0);
                } else if inlet < stored.len() {
                    for (slot, value) in stored.iter_mut().skip(inlet).zip(&input) {
                        *slot = slot.cast(value)?;
                    }
                    if inlet == 0 {
                        output.push(Work::Schedule {
                            node,
                            delay: *delay,
                            generation: *generation,
                            output: stored.iter().enumerate().rev().map(|(index, atom)| (index, vec![atom.clone()])).collect(),
                        });
                    }
                } else {
                    return Err("pipe has no such inlet".into());
                }
            }
            Kind::Parse { status, data, hires } => {
                if input.is_empty() {
                    *status = 0;
                    data.clear();
                } else {
                    let byte = first_number(&input)?;
                    if input.len() != 1 || byte.fract() != 0.0 || !(0.0..240.0).contains(&byte) {
                        return Err("only channel MIDI bytes are simulated".into());
                    }
                    let byte = byte as u8;
                    if byte >= 128 {
                        *status = byte;
                        data.clear();
                    } else if *status >= 128 {
                        data.push(byte);
                        let family = *status & 0xf0;
                        if data.len() == if matches!(family, 0xc0 | 0xd0) { 1 } else { 2 } {
                            output.push(emit(6, vec![Atom::Int((*status as i64 & 15) + 1)]));
                            let pair = || vec![Atom::Int(data[0] as i64), Atom::Int(data[1] as i64)];
                            let (outlet, message) = match family {
                                0x80 => (0, vec![Atom::Int(data[0] as i64), Atom::Int(0)]),
                                0x90 => (0, pair()),
                                0xa0 => (1, pair()),
                                0xb0 => (2, pair()),
                                0xc0 => (3, vec![Atom::Int(data[0] as i64)]),
                                0xd0 => (4, vec![Atom::Int(data[0] as i64)]),
                                0xe0 => (
                                    5,
                                    vec![Atom::Int(if *hires {
                                        (data[0] as i64 | ((data[1] as i64) << 7)) - 8192
                                    } else {
                                        data[1] as i64
                                    })],
                                ),
                                _ => return Err("unsupported MIDI status".into()),
                            };
                            output.push(emit(outlet, message));
                            data.clear();
                        }
                    }
                }
            }
            Kind::Format { channel, hires } => {
                if inlet == 6 {
                    let value = first_number(&input)?;
                    if value < 1.0 {
                        return Err("non-positive MIDI channels are not simulated".into());
                    }
                    *channel = value.trunc() as i64;
                } else {
                    let data = input.iter().map(Atom::number).collect::<Result<Vec<_>, _>>()?;
                    let expected = if inlet <= 2 { 2 } else { 1 };
                    if inlet > 5 || data.len() != expected {
                        return Err("midiformat needs a complete supported MIDI message".into());
                    }
                    if data.iter().any(|number| number.fract() != 0.0) {
                        return Err("fractional MIDI data is not simulated".into());
                    }
                    if inlet != 5 && data.iter().any(|number| !(0.0..=127.0).contains(number)) {
                        return Err("out-of-range MIDI data is not simulated".into());
                    }
                    if inlet == 5 && ((!*hires && !(0.0..=127.0).contains(&data[0])) || (*hires && !(-8192.0..=8191.0).contains(&data[0])))
                    {
                        return Err("out-of-range pitch bend is not simulated".into());
                    }
                    let channel = (*channel - 1).rem_euclid(16) as u8;
                    let b7 = |number: f64| number.clamp(0.0, 127.0) as u8;
                    let (status, bytes) = match inlet {
                        0 => (0x90, vec![b7(data[0]), b7(data[1])]),
                        // Max's documented poly-pressure input is pressure, key.
                        1 => (0xa0, vec![b7(data[1]), b7(data[0])]),
                        2 => (0xb0, vec![b7(data[0]), b7(data[1])]),
                        3 => (0xc0, vec![b7(data[0])]),
                        4 => (0xd0, vec![b7(data[0])]),
                        5 => {
                            let value = if *hires { (data[0] + 8192.0).clamp(0.0, 16383.0) as u16 } else { (b7(data[0]) as u16) << 7 };
                            (0xe0, vec![(value & 127) as u8, (value >> 7) as u8])
                        }
                        _ => return Err("unsupported MIDI inlet".into()),
                    };
                    output.push(emit(0, vec![Atom::Int((status | channel) as i64)]));
                    output.extend(bytes.into_iter().map(|byte| emit(0, vec![Atom::Int(byte as i64)])));
                }
            }
        }
        Ok(output)
    }

    fn run(
        mut self,
        controls: &[Control],
        set: Option<&Map<String, Value>>,
        input: &[MidiEvent],
    ) -> Result<(Vec<MidiEvent>, usize), Problem> {
        if set.is_some_and(|set| set.keys().any(|name| !controls.iter().any(|control| control.name() == name))) {
            return Err("set names a control the device does not have".into());
        }
        for (index, name) in self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| match &node.kind {
                Kind::Control(name) => Some((index, name.clone())),
                _ => None,
            })
            .collect::<Vec<_>>()
        {
            let control = controls.iter().find(|control| control.name() == name).ok_or("a control has no spec")?;
            let given = set.and_then(|set| set.get(&name));
            let value = match control {
                Control::Number(numeric_control) | Control::Integer(numeric_control) => {
                    let value = given
                        .map_or(Some(numeric_control.default), Value::as_f64)
                        .filter(|value| value.is_finite())
                        .ok_or("a test control value is not finite numeric data")?
                        .clamp(numeric_control.min, numeric_control.max);
                    let integer = matches!(control, Control::Integer(_));
                    if integer && value.fract() != 0.0 {
                        return Err("fractional integer-control test values are not simulated".into());
                    }
                    numeric(value, integer)?
                }
                Control::Choice(control) => {
                    let label =
                        given.map_or(Some(control.default.as_str()), Value::as_str).ok_or("a choice test value must name an option")?;
                    Atom::Int(
                        control.options.iter().position(|option| option == label).ok_or("a test choice is not one of its options")? as i64
                    )
                }
                Control::Switch(control) => Atom::Int(i64::from(
                    given.map_or(Some(control.default), Value::as_bool).ok_or("a switch test value must be true or false")?,
                )),
            };
            self.work(vec![Work::Emit(index, 0, vec![value])])?;
        }
        let sources =
            self.nodes.iter().enumerate().filter_map(|(index, node)| matches!(node.kind, Kind::In).then_some(index)).collect::<Vec<_>>();
        let mut input = input.to_vec();
        input.sort_by(|a, b| a.at.unwrap_or(0.0).total_cmp(&b.at.unwrap_or(0.0)));
        for event in input {
            let at = event.at.unwrap_or(0.0);
            if !at.is_finite() || at < 0.0 {
                return Err("a test event has an invalid time".into());
            }
            self.advance(at)?;
            for byte in bytes_of(&event) {
                self.work(sources.iter().map(|&source| Work::Emit(source, 0, vec![Atom::Int(byte as i64)])).collect())?;
            }
        }
        self.advance(self.now + 2_000.0)?;
        let mut remaining = 0;
        let mut status = 0u8;
        for &(_, byte) in &self.sent {
            let byte = byte as u8;
            if byte >= 128 {
                if remaining != 0 {
                    return Err("out sends an incomplete MIDI message".into());
                }
                status = byte;
                remaining = if matches!(status & 0xf0, 0xc0 | 0xd0) { 1 } else { 2 };
            } else {
                if status == 0 {
                    return Err("out sends MIDI data without a status byte".into());
                }
                if remaining == 0 {
                    remaining = if matches!(status & 0xf0, 0xc0 | 0xd0) { 1 } else { 2 };
                }
                remaining -= 1;
            }
        }
        if remaining != 0 {
            return Err("out sends an incomplete MIDI message".into());
        }
        let pending = self
            .pending
            .iter()
            .filter(|timer| match self.nodes[timer.node].kind {
                Kind::Delay { generation, .. } | Kind::Pipe { generation, .. } => generation == timer.generation,
                _ => false,
            })
            .count();
        Ok((events_of(&self.sent), pending))
    }
}

/// Run supplied MIDI expectations and the same finite-note probe as code MIDI
/// effects. The whole graph is compiled before execution; unsupported behavior
/// reports every supplied test as unrun, with its expectations still retained.
pub fn check(spec: &PatchSpec, document: &Value) -> Checked {
    let mut checked = Checked { of: spec.tests.len(), ..Default::default() };
    let steps = Rc::new(Cell::new(0));
    let compile = || {
        if spec.kind != DeviceType::MidiEffect {
            return Err("only MIDI-effect message patches are simulated; audio and instruments need audition".into());
        }
        Graph::read(document, &spec.controls, Rc::clone(&steps))
    };
    if let Err(why) = compile() {
        checked.runaway = matches!(why, Problem::Limit(_));
        checked.problems.push(format!("Kumi's message check wasn't run: {why}."));
        checked.problems.extend(spec.tests.iter().map(|test| format!("test \"{}\" wasn't run: {why}.", test.name)));
        return checked;
    }
    match compile().and_then(|graph| graph.run(&spec.controls, None, &PROBE)) {
        Ok((events, pending)) => {
            let loose = hanging(&events);
            if !loose.is_empty() {
                checked.problems.push(format!("Kumi's message check: notes are left hanging (pitch {}).", loose.join(", ")));
            }
            if pending > 0 {
                checked.problems.push("Kumi's message check: timers remain after the finite-note probe.".into());
            }
        }
        Err(why) => {
            checked.runaway |= matches!(why, Problem::Limit(_));
            checked.problems.push(format!("Kumi's message check stopped: {why}."));
            if checked.runaway {
                checked.problems.extend(
                    spec.tests.iter().map(|test| format!("test \"{}\" wasn't run: the request reached a resource limit.", test.name)),
                );
                return checked;
            }
        }
    }
    for (index, test) in spec.tests.iter().enumerate() {
        match compile().and_then(|graph| graph.run(&spec.controls, test.set.as_ref(), &test.input)) {
            Ok((events, 0))
                if events.len() == test.expect.len()
                    && test.expect.iter().zip(&events).all(|(expected, actual)| matches(expected, actual)) =>
            {
                checked.passed += 1
            }
            Ok((_, pending)) if pending > 0 => checked.problems.push(format!(
                "test \"{}\" wasn't completed: {pending} timers remain beyond the finite check horizon; its expectations were not counted as passing.", test.name)),
            Ok((events, _)) => checked.problems.push(format!(
                "test \"{}\": expected {}; got {}.",
                test.name,
                test.expect.iter().map(describe).collect::<Vec<_>>().join(", "),
                events.iter().map(describe).collect::<Vec<_>>().join(", ")
            )),
            Err(why) => {
                checked.runaway |= matches!(why, Problem::Limit(_));
                checked.problems.push(format!("test \"{}\" stopped without passing: {why}.", test.name));
                if checked.runaway {
                    checked.problems.extend(spec.tests[index + 1..].iter().map(|test| format!("test \"{}\" wasn't run: the request reached a resource limit.", test.name)));
                    break;
                }
            }
        }
    }
    checked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{
        patched::patched_device,
        spec::{check_spec, DeviceSpec},
    };
    use serde_json::json;

    fn checked(patch: &str, tests: Value, controls: Value) -> (PatchSpec, Checked) {
        let (DeviceSpec::Patch(spec), _) = check_spec(json!({ "type": "midi_effect", "name": "Message test", "about": "Offline messages.", "patch": patch, "controls": controls, "tests": tests }).as_object().unwrap()) else { panic!("a patch") };
        let (document, _) = patched_device(&spec, None);
        let result = check(&spec, &document);
        (spec, result)
    }

    fn notes(pitch: u8, shift: u8, at: u32) -> Value {
        json!({ "name": "a note and its release", "input": [{ "type": "noteon", "pitch": pitch, "at": 0 }, { "type": "noteoff", "pitch": pitch, "at": 90 }], "expect": [{ "type": "noteon", "pitch": shift, "velocity": 100, "at": at }, { "type": "noteoff", "pitch": shift, "at": 90 + at }] })
    }

    const TRANSPOSE: &str = "in -> parse = [midiparse]\nparse -> pair = [unpack 0 0]\npair.1 -> together.1\npair -> shift = [+ 0] -> together = [pack 0 0] -> format = [midiformat] -> out\nparse.6 -> format.6\n\"Transpose\" -> shift.1\n";

    #[test]
    fn the_written_patch_runs_notes_releases_and_control_values_with_cold_inlets_ready_first() {
        let controls = json!([{ "name": "Transpose", "type": "number", "min": -24, "max": 24, "default": 12 }]);
        let first = notes(60, 72, 0);
        let mut second = notes(60, 48, 0);
        second["set"] = json!({ "Transpose": -12 });
        let (_, result) = checked(TRANSPOSE, json!([first, second]), controls);
        assert_eq!(result, Checked { passed: 2, of: 2, problems: vec![], runaway: false });
        let integer = json!([{ "name": "Transpose", "type": "integer", "min": -24, "max": 24, "default": 12 }]);
        let mut integer_test = notes(60, 48, 0);
        integer_test["set"] = json!({ "Transpose": -12 });
        let (_, result) = checked(TRANSPOSE, json!([notes(60, 72, 0), integer_test]), integer.clone());
        assert_eq!(result, Checked { passed: 2, of: 2, problems: vec![], runaway: false });
        let mut fractional = notes(60, 72, 0);
        fractional["set"] = json!({ "Transpose": 12.5 });
        let (_, result) = checked(TRANSPOSE, json!([fractional]), integer);
        assert_eq!((result.passed, result.of), (0, 1));
        assert!(result.problems.iter().any(|problem| problem.contains("fractional integer-control")), "{result:?}");
    }

    #[test]
    fn a_delayed_nested_message_patch_uses_virtual_time_and_keeps_the_midi_channel() {
        let patch = "pass = [p pass] {\n[ inlet ] -> [t i] -> [outlet]\n}\nin -> pass -> [pipe 10] -> out\n";
        let mut test = notes(60, 60, 10);
        for event in test["input"].as_array_mut().unwrap() {
            event["channel"] = json!(3);
        }
        for event in test["expect"].as_array_mut().unwrap() {
            event["channel"] = json!(3);
        }
        let (_, result) = checked(patch, json!([test]), json!([]));
        assert_eq!(result, Checked { passed: 1, of: 1, problems: vec![], runaway: false });
    }

    #[test]
    fn channel_messages_and_signed_high_resolution_bend_round_trip_through_the_actual_objects() {
        let patch = "in -> parse = [midiparse @hires 2]\nparse -> format = [midiformat @hires 2] -> out\nparse.2 -> format.2\nparse.3 -> format.3\nparse.4 -> format.4\nparse.5 -> format.5\nparse.6 -> format.6\n";
        let events = json!([
            { "type": "cc", "channel": 3, "controller": 7, "value": 90, "at": 0 },
            { "type": "program", "channel": 3, "value": 4, "at": 10 },
            { "type": "aftertouch", "channel": 3, "value": 45, "at": 20 },
            { "type": "pitchbend", "channel": 3, "value": 9000, "at": 30 },
            { "type": "noteon", "channel": 3, "pitch": 60, "velocity": 0, "at": 40 }
        ]);
        let mut expected = events.clone();
        expected[4]["type"] = json!("noteoff");
        let (_, result) = checked(patch, json!([{ "name": "channel data", "input": events, "expect": expected }]), json!([]));
        assert_eq!(result, Checked { passed: 1, of: 1, problems: vec![], runaway: false });
    }

    #[test]
    fn unsupported_boxes_keep_expectations_and_cannot_pass_an_empty_expectation() {
        let (spec, result) = checked(
            "in -> [v8 test] { function msg_int(v) { outlet(0, v); } } -> out",
            json!([{ "name": "must run", "input": [], "expect": [] }]),
            json!([]),
        );
        assert_eq!(spec.tests.len(), 1);
        assert_eq!((result.passed, result.of), (0, 1));
        assert!(result.problems.iter().any(|problem| problem.contains("must run") && problem.contains("wasn't run")));
    }

    #[test]
    fn a_wrong_release_is_a_failed_test_and_the_probe_finds_the_hanging_notes() {
        let patch = "in -> [midiparse] -> [unpack 0 0] -> [pack 0 100] -> [midiformat] -> out";
        let (_, result) = checked(patch, json!([notes(60, 60, 0)]), json!([]));
        assert_eq!((result.passed, result.of), (0, 1));
        assert!(result.problems.iter().any(|problem| problem.contains("hanging")), "{result:?}");
        assert!(result.problems.iter().any(|problem| problem.starts_with("test \"")), "{result:?}");
    }

    #[test]
    fn feedback_stops_at_a_work_limit_instead_of_hanging_the_builder() {
        let (_, result) = checked(
            "in -> loop = [t i]\nloop -> loop\n",
            json!([{ "name": "bounded", "input": [{ "type": "noteon", "pitch": 60 }], "expect": [] }]),
            json!([]),
        );
        assert!(result.runaway);
        assert_eq!((result.passed, result.of), (0, 1));
        assert!(result.problems.iter().any(|problem| problem.contains("100,000-message")), "{result:?}");
    }

    #[test]
    fn a_pending_timer_cannot_make_an_empty_expectation_pass() {
        let (_, result) = checked(
            "in -> [pipe 5000] -> out",
            json!([{ "name": "not finished", "input": [{ "type": "noteon", "pitch": 60 }], "expect": [] }]),
            json!([]),
        );
        assert_eq!((result.passed, result.of), (0, 1));
        assert!(
            result.problems.iter().any(|problem| problem.contains("not finished") && problem.contains("wasn't completed")),
            "{result:?}"
        );
    }

    #[test]
    fn an_unsupported_first_box_cannot_hide_a_fatal_graph_budget() {
        let (spec, _) = checked("in -> out", json!([]), json!([]));
        let mut boxes = vec![json!({ "box": { "id": "unsupported", "maxclass": "newobj", "text": "v8" } })];
        boxes.extend((0..MOST_NODES).map(|index| json!({ "box": { "id": format!("label-{index}"), "maxclass": "comment" } })));
        let result = check(&spec, &json!({ "patcher": { "boxes": boxes, "lines": [] } }));
        assert!(result.runaway, "{result:?}");
        assert!(result.problems[0].contains("2,000 boxes"), "{result:?}");
    }

    #[test]
    fn an_unverified_integer_range_is_explicitly_unrun() {
        let (_, result) =
            checked("in -> [+ 9007199254740993] -> out", json!([{ "name": "no saturation", "input": [], "expect": [] }]), json!([]));
        assert_eq!((result.passed, result.of), (0, 1));
        assert!(result.problems.iter().any(|problem| problem.contains("exact supported range")), "{result:?}");
    }

    #[test]
    fn a_trigger_without_arguments_preserves_integer_bytes_instead_of_making_bangs() {
        let (_, result) = checked("in -> [t] -> out", json!([notes(60, 60, 0)]), json!([]));
        assert_eq!(result, Checked { passed: 1, of: 1, problems: vec![], runaway: false });
    }
}
