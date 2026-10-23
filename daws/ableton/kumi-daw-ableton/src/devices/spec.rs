//! What the model asks for when it makes a device, and how Kumi makes it whatever it's given. The model names the device
//! and its knobs, then gives its code (JavaScript or GenExpr, which Kumi builds a device around) or a patch of Max's own
//! objects in Kumi's notation, which Kumi frames, lays out and checks. What doesn't fit is made the nearest way it can
//! be, and said beside the device.

use std::collections::HashMap;
use std::sync::LazyLock;

use kumi_common::js::json::stringify;
use kumi_common::js::number::to_string as number_to_string;
use kumi_common::js::string::{head, trim, utf16_len};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::amxd::DeviceType;
use super::findings::Findings;
use super::gen::{check_gen_code, names, param_name, param_problem, MAX_VOICES};
use super::patch::code::{clean, syntax_problem};

/// A knob, a menu or a switch on the device: an ordinary Live parameter, automatable and mappable.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    Number(NumericControl),
    Integer(NumericControl),
    Choice(ChoiceControl),
    Switch(SwitchControl),
}

/// A number or integer control: its range, its default and its unit.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct NumericControl {
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    #[serde(default)]
    pub unit: Unit,
    #[serde(default, skip_serializing_if = "Place::is_default")]
    pub place: Place,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ChoiceControl {
    pub name: String,
    pub options: Vec<String>,
    pub default: String,
    #[serde(default, skip_serializing_if = "Place::is_default")]
    pub place: Place,
}

/// A switch: on or off. A trigger is a switch shown as a button that's on only while it's pressed.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SwitchControl {
    pub name: String,
    pub default: bool,
    #[serde(default, skip_serializing_if = "Place::is_default")]
    pub place: Place,
}

/// Where a control sits on the face and how it shows: its group (the controls that work together, under a heading of
/// the group's name) and its look. Kumi lays the face out from these.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Place {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(default, skip_serializing_if = "Show::is_auto")]
    pub show: Show,
}

impl Place {
    pub fn is_default(&self) -> bool {
        self == &Place::default()
    }
}

/// How a control shows on the face; Auto is its type's own look (a dial, a menu, a button with its name).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Show {
    #[default]
    Auto,
    /// A number: a knob with its name and value.
    Dial,
    /// A number: a small box of its value, under its name.
    Number,
    /// A number: an upright fader.
    Slider,
    /// A choice: a menu.
    Menu,
    /// A choice: its options as buttons side by side.
    Tabs,
    /// A switch: a button with its name, lit while on.
    Button,
    /// A trigger: a button with its name, on only while it's pressed.
    Trigger,
}

impl Show {
    pub fn is_auto(&self) -> bool {
        *self == Show::Auto
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Show::Auto => "",
            Show::Dial => "dial",
            Show::Number => "number",
            Show::Slider => "slider",
            Show::Menu => "menu",
            Show::Tabs => "tabs",
            Show::Button => "button",
            Show::Trigger => "trigger",
        }
    }

    /// The looks a control of `kind` can take (its type as the model names it).
    pub fn for_type(kind: &str) -> &'static [Show] {
        match kind {
            "number" | "integer" => &[Show::Dial, Show::Number, Show::Slider],
            "choice" => &[Show::Menu, Show::Tabs],
            _ => &[Show::Button],
        }
    }
}

impl Control {
    pub fn name(&self) -> &str {
        match self {
            Control::Number(control) | Control::Integer(control) => &control.name,
            Control::Choice(control) => &control.name,
            Control::Switch(control) => &control.name,
        }
    }

    /// The control's type as the model names it: number, integer, choice, switch or trigger.
    pub fn type_name(&self) -> &'static str {
        match self {
            Control::Number(_) => "number",
            Control::Integer(_) => "integer",
            Control::Choice(_) => "choice",
            Control::Switch(control) if control.place.show == Show::Trigger => "trigger",
            Control::Switch(_) => "switch",
        }
    }

    /// Where it sits on the face and how it shows.
    pub fn place(&self) -> &Place {
        match self {
            Control::Number(control) | Control::Integer(control) => &control.place,
            Control::Choice(control) => &control.place,
            Control::Switch(control) => &control.place,
        }
    }

    /// Whether it's a trigger: a button on only while it's pressed.
    pub fn trigger(&self) -> bool {
        matches!(self, Control::Switch(control) if control.place.show == Show::Trigger)
    }

    /// The control's default as JSON: a number, an option or true/false.
    pub fn default_value(&self) -> Value {
        match self {
            Control::Number(control) | Control::Integer(control) => Value::from(control.default),
            Control::Choice(control) => Value::from(control.default.clone()),
            Control::Switch(control) => Value::from(control.default),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Unit {
    #[default]
    #[serde(rename = "")]
    None,
    #[serde(rename = "ms")]
    Ms,
    #[serde(rename = "s")]
    S,
    #[serde(rename = "Hz")]
    Hz,
    #[serde(rename = "dB")]
    Db,
    #[serde(rename = "%")]
    Percent,
    #[serde(rename = "st")]
    St,
    #[serde(rename = "note")]
    Note,
    #[serde(rename = "pan")]
    Pan,
    #[serde(rename = "beats")]
    Beats,
    #[serde(rename = "bpm")]
    Bpm,
    #[serde(rename = "x")]
    X,
}

pub const UNITS: [Unit; 12] =
    [Unit::None, Unit::Ms, Unit::S, Unit::Hz, Unit::Db, Unit::Percent, Unit::St, Unit::Note, Unit::Pan, Unit::Beats, Unit::Bpm, Unit::X];

impl Unit {
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::None => "",
            Unit::Ms => "ms",
            Unit::S => "s",
            Unit::Hz => "Hz",
            Unit::Db => "dB",
            Unit::Percent => "%",
            Unit::St => "st",
            Unit::Note => "note",
            Unit::Pan => "pan",
            Unit::Beats => "beats",
            Unit::Bpm => "bpm",
            Unit::X => "x",
        }
    }

    pub fn parse(text: &str) -> Option<Unit> {
        UNITS.into_iter().find(|unit| unit.as_str() == text)
    }
}

impl std::fmt::Display for Unit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum MidiEventType {
    #[default]
    NoteOn,
    NoteOff,
    Cc,
    PitchBend,
    Aftertouch,
    Polytouch,
    Program,
}

pub const EVENT_TYPES: [MidiEventType; 7] = [
    MidiEventType::NoteOn,
    MidiEventType::NoteOff,
    MidiEventType::Cc,
    MidiEventType::PitchBend,
    MidiEventType::Aftertouch,
    MidiEventType::Polytouch,
    MidiEventType::Program,
];

impl MidiEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            MidiEventType::NoteOn => "noteon",
            MidiEventType::NoteOff => "noteoff",
            MidiEventType::Cc => "cc",
            MidiEventType::PitchBend => "pitchbend",
            MidiEventType::Aftertouch => "aftertouch",
            MidiEventType::Polytouch => "polytouch",
            MidiEventType::Program => "program",
        }
    }

    pub fn parse(text: &str) -> Option<MidiEventType> {
        EVENT_TYPES.into_iter().find(|kind| kind.as_str() == text)
    }
}

impl std::fmt::Display for MidiEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A MIDI event as the device's code sees it (times in milliseconds).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct MidiEvent {
    #[serde(rename = "type")]
    pub kind: MidiEventType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub velocity: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
}

/// Events in and what should come out, to check the device's code before it's made.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MidiTest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<Map<String, Value>>,
    pub input: Vec<MidiEvent>,
    pub expect: Vec<MidiEvent>,
}

/// A MIDI effect: JavaScript that decides what happens to each MIDI event, with tests Kumi runs. One
/// that runs free (an LFO, a clock, a generator) keeps sending once every note is released.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MidiSpec {
    pub name: String,
    pub about: String,
    pub controls: Vec<Control>,
    pub code: String,
    pub tests: Vec<MidiTest>,
    #[serde(rename = "runsFree", default, skip_serializing_if = "std::ops::Not::not")]
    pub runs_free: bool,
}

/// An audio effect: GenExpr that turns in1/in2 into out1/out2, sample by sample.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AudioEffectSpec {
    pub name: String,
    pub about: String,
    pub controls: Vec<Control>,
    pub code: String,
}

/// An instrument: GenExpr for one voice, played by notes, and how many voices play at once.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct InstrumentSpec {
    pub name: String,
    pub about: String,
    pub controls: Vec<Control>,
    pub code: String,
    pub voices: u32,
}

/// A device of any kind that the model patches with Max's own objects, in Kumi's notation.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PatchSpec {
    pub kind: DeviceType,
    pub name: String,
    pub about: String,
    pub controls: Vec<Control>,
    pub patch: String,
    /// The Map page's slots and its sources' names, for a patch that sends modulation to mod.
    #[serde(default, skip_serializing_if = "MapSpec::is_default")]
    pub map: MapSpec,
}

/// A device's Map page: how many slots, and what each source the patch sends to mod (mod.0, mod.1…) is called.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MapSpec {
    pub slots: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
}

impl Default for MapSpec {
    fn default() -> MapSpec {
        MapSpec { slots: super::mapping::SLOTS, sources: Vec::new() }
    }
}

impl MapSpec {
    pub fn is_default(&self) -> bool {
        self == &MapSpec::default()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceSpec {
    MidiEffect(MidiSpec),
    AudioEffect(AudioEffectSpec),
    Instrument(InstrumentSpec),
    Patch(PatchSpec),
}

impl DeviceSpec {
    pub fn kind(&self) -> DeviceKind {
        match self {
            DeviceSpec::MidiEffect(_) => DeviceType::MidiEffect,
            DeviceSpec::AudioEffect(_) => DeviceType::AudioEffect,
            DeviceSpec::Instrument(_) => DeviceType::Instrument,
            DeviceSpec::Patch(spec) => spec.kind,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            DeviceSpec::MidiEffect(spec) => &spec.name,
            DeviceSpec::AudioEffect(spec) => &spec.name,
            DeviceSpec::Instrument(spec) => &spec.name,
            DeviceSpec::Patch(spec) => &spec.name,
        }
    }

    pub fn about(&self) -> &str {
        match self {
            DeviceSpec::MidiEffect(spec) => &spec.about,
            DeviceSpec::AudioEffect(spec) => &spec.about,
            DeviceSpec::Instrument(spec) => &spec.about,
            DeviceSpec::Patch(spec) => &spec.about,
        }
    }

    pub fn controls(&self) -> &[Control] {
        match self {
            DeviceSpec::MidiEffect(spec) => &spec.controls,
            DeviceSpec::AudioEffect(spec) => &spec.controls,
            DeviceSpec::Instrument(spec) => &spec.controls,
            DeviceSpec::Patch(spec) => &spec.controls,
        }
    }

    /// The device's code, or its patch.
    pub fn code(&self) -> &str {
        match self {
            DeviceSpec::MidiEffect(spec) => &spec.code,
            DeviceSpec::AudioEffect(spec) => &spec.code,
            DeviceSpec::Instrument(spec) => &spec.code,
            DeviceSpec::Patch(spec) => &spec.patch,
        }
    }
}

pub type DeviceKind = DeviceType;
pub const DEVICE_KINDS: [DeviceKind; 3] = [DeviceType::MidiEffect, DeviceType::AudioEffect, DeviceType::Instrument];

/// Ceilings, not targets: a device gets the controls, options, voices and code it needs, and these
/// only keep its file one Live loads. Past eight, the face shows its controls in up to three rows.
pub const MAX_CONTROLS: usize = 128;
pub const MAX_OPTIONS: usize = 128;
/// The most JavaScript Kumi reads through to see it's one function's body, in characters.
pub const MAX_CODE: usize = 100_000;
/// The most of a patch Kumi reads, in characters, as its schema says: laying a patch out and checking it takes longer the
/// more it holds.
pub const MAX_PATCH: usize = 100_000;
pub const MAX_TESTS: usize = 32;
const MAX_TEST_EVENTS: usize = 256;
static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9 ._()&'+-]{0,31}$").unwrap());
static CONTROL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9 ._()&'+/#%-]{0,31}$").unwrap());
static DEVICE_ON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^device on$").unwrap());
static MIX_OR_OUTPUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(mix|output)$").unwrap());
/// A midi the frame finds: it takes whatever `midi` is a function (`function midi(event)`, `const midi = (event) => …`).
static MIDI_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?-u:\b)(function\s*\*?\s*midi\s*\(|(const|let|var)\s+midi\s*=)").unwrap());
/// What the frame can't hide: a module, which runs outside every name it hides. In a function's body only `import(…)`
/// reads; a static import or an export doesn't, so it's caught as code that isn't one function's body.
static MODULES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)import\s*\(").unwrap());
/// Where Live reads `-->` as a comment: at a line's start, after only spaces and block comments.
static HTML_CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^(\s|/\*.*?\*/)*-->|\*/(\s|/\*.*?\*/)*-->").unwrap());
static COMMENTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"//[^\n]*|/\*[\s\S]*?\*/").unwrap());

/// The source text of a function, by the intrinsic captured before anything else runs, in a context where no code is
/// made from strings: what [`whole_body`] reads the device's function back with.
const SOURCE_OF: &str = r#"(() => {
  const toString = Function.prototype.toString;
  const apply = Reflect.apply;
  const refuse = function () { throw new EvalError("Code generation from strings disallowed for this context"); };
  for (const sample of [function () {}, function* () {}, async function () {}, async function* () {}]) {
    Object.defineProperty(Object.getPrototypeOf(sample), "constructor", { value: refuse, writable: false, configurable: false });
  }
  globalThis.eval = refuse;
  globalThis.Function = refuse;
  return (f) => apply(toString, f, []);
})()"#;

/// Whether the code is one whole function body, as the frame needs: the frame puts it inside a function of its own,
/// within the scope that hides Max and Live, so code that closed that function early (opening another for the frame's
/// tail) would run outside every name the frame hides. Read alone inside a strict function, in a context of its own,
/// the function has to come back holding all of it. HTML-like comments are found first: Live's V8 reads `<!--`
/// anywhere, and `-->` at a line's start, as comments in a script and QuickJS doesn't, so the two could read the braces
/// differently.
fn whole_body(code: &str) -> Option<String> {
    let length = utf16_len(code);
    if length > MAX_CODE {
        return Some(format!("code: it's {length} characters, past the {MAX_CODE} Kumi reads through to see it's one function's body."));
    }
    if code.contains("<!--") || HTML_CLOSE.is_match(code) {
        return Some("code: Live reads \"<!--\" anywhere, and \"-->\" at the start of a line, as a comment, which can change what the code is: write them another way (\"x-- > 0\").".into());
    }
    let wrapped = format!("function(){{\"use strict\";\n{code}\n}}");
    let read = (|| -> rquickjs::Result<Option<String>> {
        let runtime = rquickjs::Runtime::new()?;
        runtime.set_memory_limit(64 * 1024 * 1024);
        let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
        runtime.set_interrupt_handler(Some(Box::new(move || std::time::Instant::now() > until)));
        let context = rquickjs::Context::full(&runtime)?;
        context.with(|ctx| {
            let source: rquickjs::Function = ctx.eval(SOURCE_OF)?;
            let mut options = rquickjs::context::EvalOptions::default();
            options.strict = false;
            let value: rquickjs::Value = ctx.eval_with_options(format!("({wrapped})"), options)?;
            if !value.is_function() {
                return Ok(None);
            }
            Ok(Some(source.call::<_, String>((value,))?))
        })
    })();
    match read {
        Ok(Some(text)) if text == wrapped => None,
        Ok(_) => Some("code: it has to be one function's body: a } in it closes the device's function early.".into()),
        Err(_) if !paired(code) => Some(
            "code: a brace, bracket or quote in it isn't closed, or one is closed twice, so it doesn't read as one function's body.".into(),
        ),
        Err(_) => Some(match syntax_problem(code) {
            Some((line, message)) if !message.contains("interrupted") && !message.contains("out of memory") => {
                format!("code: line {line}: {message}.")
            }
            Some(_) => "code: Kumi couldn't read it through in time to see it's one function's body.".into(),
            None => "code: it doesn't read as one function's body in strict mode, where the device's code runs.".into(),
        }),
    }
}

/// Whether the code's braces, brackets and parentheses pair up, its strings and comments aside.
fn paired(code: &str) -> bool {
    let mut open: Vec<char> = Vec::new();
    for c in clean(code).chars() {
        let opens = match c {
            ')' => '(',
            ']' => '[',
            '}' => '{',
            '(' | '[' | '{' => {
                open.push(c);
                continue;
            }
            _ => continue,
        };
        if open.pop() != Some(opens) {
            return false;
        }
    }
    open.is_empty()
}

/// What the device's code may not reach: files, the network, the rest of Max and Live, and the
/// frame's own plumbing (it sends through send and pass, and times through after, so no note is
/// left hanging). The frame also hides these names from the code; the check says why up front.
static FORBIDDEN: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (
            Regex::new(r"(?-u:\b)(File|Folder|XMLHttpRequest|fetch|SQLite|Dict|Buffer|Global|LiveAPI|messnamed|globalThis|require|patcher)(?-u:\b)|(?-u:\b)max\s*\.\s*[a-z]").unwrap(),
            "files, the network and the rest of Max and Live are out of reach",
        ),
        (
            Regex::new(r#"(?-u:\b)eval\s*\(|(?-u:\b)Function\s*\(|\.\s*constructor(?-u:\b)|\[\s*["'`]constructor|__proto__"#).unwrap(),
            "code can't make code",
        ),
        (
            Regex::new(r"(?-u:\b)outlet\s*\(|(?-u:\b)new\s+Task(?-u:\b)").unwrap(),
            "send with send(event) or pass(event), and time with after(ms, fn): the frame keeps count of held notes",
        ),
    ]
});

/// A JSON value as a string, trimmed as JavaScript trims; "" when it isn't a string.
fn trimmed_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => trim(text).to_string(),
        _ => String::new(),
    }
}

/// `raw && typeof raw === "object" ? raw : {}`: an object's fields (an array has none that matter).
fn object_of(raw: &Value) -> Map<String, Value> {
    match raw {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    }
}

/// An event from the model's input: its type one of EVENT_TYPES, its time 0–60 s, its numbers numbers.
fn event_of(raw: &Value) -> Option<MidiEvent> {
    let event = raw.as_object()?;
    let kind = MidiEventType::parse(event.get("type")?.as_str()?)?;
    let number = |key: &str| -> Option<Option<f64>> {
        match event.get(key) {
            None => Some(None),
            Some(Value::Number(number)) => Some(number.as_f64()),
            // TS: a pitch, velocity, controller, value or channel that isn't a number was kept and coerced later; here the test is refused.
            Some(_) => None,
        }
    };
    let at = number("at")?;
    if let Some(at) = at {
        if !(0.0..=60_000.0).contains(&at) {
            return None;
        }
    }
    Some(MidiEvent {
        kind,
        channel: number("channel")?,
        pitch: number("pitch")?,
        velocity: number("velocity")?,
        controller: number("controller")?,
        value: number("value")?,
        at,
    })
}

/// A name of up to 32 characters that `allowed` takes, starting with one `first` takes: what's left of `text` that
/// fits, or `fallback`.
fn fitted(text: &str, allowed: impl Fn(char) -> bool, first: impl Fn(char) -> bool, fallback: &str) -> String {
    let kept: String = text.chars().map(|c| if allowed(c) { c } else { ' ' }).collect();
    let kept = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let kept: String = kept.trim_start_matches(|c: char| !first(c)).chars().take(32).collect();
    let kept = kept.trim_end();
    if kept.is_empty() {
        fallback.to_string()
    } else {
        kept.to_string()
    }
}

/// `name`, numbered past the names `taken` already has ("Rate 2"), in up to 32 characters.
fn unique(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut candidate = name.to_string();
    let mut number = 1;
    while taken(&candidate) {
        number += 1;
        let suffix = format!(" {number}");
        let stem: String = name.chars().take(32 - suffix.chars().count()).collect();
        candidate = format!("{}{suffix}", stem.trim_end());
    }
    candidate
}

/// The unit a control's unit is written as ("Seconds" is s, "semitones" st); None for one Kumi doesn't show.
fn unit_like(text: &str) -> Option<Unit> {
    let unit = match text.trim().to_lowercase().as_str() {
        "" | "none" => Unit::None,
        "ms" | "msec" | "millisecond" | "milliseconds" => Unit::Ms,
        "s" | "sec" | "secs" | "second" | "seconds" => Unit::S,
        "hz" | "hertz" => Unit::Hz,
        "db" | "decibel" | "decibels" => Unit::Db,
        "%" | "percent" | "pct" => Unit::Percent,
        "st" | "semi" | "semis" | "semitone" | "semitones" => Unit::St,
        "note" | "notes" => Unit::Note,
        "pan" => Unit::Pan,
        "beat" | "beats" => Unit::Beats,
        "bpm" => Unit::Bpm,
        "x" | "times" | "ratio" => Unit::X,
        _ => return None,
    };
    Some(unit)
}

/// A device's type from what was given: one that names audio is an audio effect, one that names an instrument or a
/// synth an instrument, anything else a MIDI effect.
fn kind_like(given: &Value) -> DeviceType {
    let text = given.as_str().unwrap_or("").to_lowercase();
    if text.contains("audio") {
        DeviceType::AudioEffect
    } else if text.contains("instrument") || text.contains("synth") {
        DeviceType::Instrument
    } else {
        DeviceType::MidiEffect
    }
}

/// A MIDI effect's code kept in the device where it doesn't run, with why: each line a comment (split at every line
/// break JavaScript has, so none runs on), so the frame finds no midi() in it, the device passes MIDI on as it comes,
/// and the producer can read the code in the box. A control character is written as its escape.
pub fn kept_unrun(code: &str, why: &str) -> String {
    let commented = |text: &str| -> Vec<String> {
        text.split("\r\n")
            .flat_map(|line| line.split(['\n', '\r', '\u{2028}', '\u{2029}']))
            .map(|line| {
                let line: String = line
                    .chars()
                    .map(|c| if c.is_control() && c != '\t' { format!("\\u{{{:x}}}", c as u32) } else { c.to_string() })
                    .collect();
                format!("// {line}")
            })
            .collect()
    };
    let mut lines = commented(&format!("Kumi kept this code without running it: {why}. Fix it and make the device again."));
    lines.extend(commented(code));
    lines.join("\n")
}

/// A Map page's slots and sources' names, from what was given, made the nearest way they can be.
fn map_of(given: Option<&Value>, patched: bool, found: &mut Findings) -> MapSpec {
    let mut map = MapSpec::default();
    let Some(given) = given.filter(|given| !given.is_null()) else { return map };
    if !patched {
        found.changed.push("map: a Map page is for a patch that sends its modulation to mod, so this was left out.".into());
        return map;
    }
    let Some(fields) = given.as_object() else {
        found.changed.push("map: it's an object ({\"slots\": 8, \"sources\": [\"LFO 1\"]}), so the page has 8 slots.".into());
        return map;
    };
    if let Some(slots) = fields.get("slots") {
        match slots.as_f64().filter(|slots| slots.is_finite()) {
            Some(count) if count.fract() == 0.0 && (1.0..=super::mapping::MOST_SLOTS as f64).contains(&count) => map.slots = count as usize,
            _ => {
                map.slots = slots
                    .as_f64()
                    .filter(|slots| slots.is_finite())
                    .map_or(super::mapping::SLOTS, |slots| slots.round().clamp(1.0, super::mapping::MOST_SLOTS as f64) as usize);
                found.changed.push(format!("map.slots {}: made {} (1 to {}).", stringify(slots), map.slots, super::mapping::MOST_SLOTS));
            }
        }
    }
    if let Some(Value::Array(sources)) = fields.get("sources") {
        map.sources = sources
            .iter()
            .take(super::mapping::MOST_SOURCES)
            .map(|source| head(trim(source.as_str().unwrap_or("")), 16))
            .map(|name| if name.is_empty() { "Mod".to_string() } else { name })
            .collect();
        if sources.len() > super::mapping::MOST_SOURCES {
            found.changed.push(format!("map.sources: the first {} were kept.", super::mapping::MOST_SOURCES));
        }
    }
    map
}

/// The most characters of a group's name: a heading over a few controls.
const MOST_GROUP: usize = 24;

/// Where a control sits and how it shows, from its group and show, made the nearest way they can be.
fn place_of(control: &Map<String, Value>, control_type: &str, label: &str, found: &mut Findings) -> Place {
    let group = trimmed_string(control.get("group"));
    let group = if utf16_len(&group) > MOST_GROUP {
        let cut = head(&group, MOST_GROUP).trim_end().to_string();
        found.changed.push(format!("{label}: group \"{group}\" was cut to \"{cut}\" ({MOST_GROUP} characters, a heading)."));
        cut
    } else {
        group
    };
    let looks = Show::for_type(control_type);
    let show = match (control_type, control.get("show").and_then(Value::as_str).map(trim)) {
        ("trigger", _) => Show::Trigger,
        (_, None | Some("")) => Show::Auto,
        (_, Some(given)) => match looks.iter().find(|look| look.as_str() == given) {
            Some(look) => *look,
            None => {
                let can = looks.iter().map(|look| look.as_str()).collect::<Vec<_>>().join(", ");
                found.changed.push(format!("{label}: show \"{given}\" isn't a look a {control_type} has ({can}), so it shows its own."));
                Show::Auto
            }
        },
    };
    Place { group, show }
}

/// A device from the model's input, always: what doesn't fit is made the nearest way it can be, and every such
/// change and every problem with the code is said beside it, so the model can fix it on its next call.
pub fn check_spec(input: &Map<String, Value>) -> (DeviceSpec, Findings) {
    let mut found = Findings::default();
    let kind = match input.get("type") {
        None | Some(Value::Null) => DeviceType::MidiEffect,
        Some(given) => match given.as_str().and_then(DeviceType::parse) {
            Some(kind) => kind,
            None => {
                let kind = kind_like(given);
                found.changed.push(format!(
                    "type {}: made as {} (midi_effect, audio_effect or instrument).",
                    stringify(given),
                    kind.as_str()
                ));
                kind
            }
        },
    };
    // A device given as a patch of Max's objects has no code of its own: its controls are wired in the patch. A patch is
    // the notation's lines, as text or a list of them; a blank one (a field a model leaves empty) is none.
    let patch = match input.get("patch") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(lines)) => lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
        Some(other @ (Value::Object(_) | Value::Number(_) | Value::Bool(true))) => {
            found.changed.push(format!(
                "patch {}: a patch is lines of Kumi's notation, as text, so this one was left out.",
                head(&stringify(other), 40)
            ));
            String::new()
        }
        _ => String::new(),
    };
    // Past MAX_PATCH it's cut at a line's end, and what came after is said to be left out.
    let patch = if utf16_len(&patch) > MAX_PATCH {
        let cut = head(&patch, MAX_PATCH);
        let kept = cut.rfind('\n').map_or(cut.as_str(), |at| &cut[..at]);
        let line = kept.matches('\n').count() + 1;
        found.wrong.push(format!(
            "patch: it's {} characters, past the {MAX_PATCH} Kumi reads, so it was cut after line {line}, and what came after was left out.",
            utf16_len(&patch)
        ));
        kept.to_string()
    } else {
        patch
    };
    let patched = !trim(&patch).is_empty();
    let sound = matches!(kind, DeviceType::AudioEffect | DeviceType::Instrument);
    let gen = sound && !patched;
    let given_name = trimmed_string(input.get("name"));
    let name = if NAME.is_match(&given_name) {
        given_name
    } else {
        let allowed = |c: char| c.is_ascii_alphanumeric() || " ._()&'+-".contains(c);
        let name = fitted(&given_name, allowed, |c| c.is_ascii_alphanumeric(), "Device");
        found.changed.push(format!(
            "name {}: made as \"{name}\" (1–32 letters, digits, spaces and . _ ( ) & ' + -, starting with a letter or digit).",
            stringify(&Value::String(given_name.clone()))
        ));
        name
    };
    let about = trimmed_string(input.get("about"));
    let about = if about.is_empty() {
        found.changed.push("about: none given, so the device's description is its name.".to_string());
        name.clone()
    } else if utf16_len(&about) > 400 {
        // Cut at a word near the end, or else where it reaches 400.
        let cut = head(&about, 399);
        let kept = match cut.rfind(' ') {
            Some(at) if utf16_len(&cut[..at]) >= 300 => &cut[..at],
            _ => cut.as_str(),
        };
        let cut = format!("{}…", kept.trim_end_matches([',', ';', ':']));
        found.changed.push(format!("about: {} characters, cut to the first {} (up to 400).", utf16_len(&about), utf16_len(&cut)));
        cut
    } else {
        about
    };
    let controls: Vec<Value> = match input.get("controls") {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::Object(named)) => {
            found.changed.push(
                "controls: given by name, so they were read as a list in that order, each named by its key where it has no name.".into(),
            );
            named
                .iter()
                .map(|(key, control)| {
                    let mut control = control.clone();
                    if let Value::Object(fields) = &mut control {
                        fields.entry("name").or_insert_with(|| Value::String(key.clone()));
                    }
                    control
                })
                .collect()
        }
        None | Some(Value::Null | Value::Bool(false)) => Vec::new(),
        Some(Value::String(text)) if trim(text).is_empty() => Vec::new(),
        Some(other) => {
            found.changed.push(format!(
                "controls {}: controls are a list, each with its name and type, so none was made from this.",
                head(&stringify(other), 40)
            ));
            Vec::new()
        }
    };
    if controls.len() > MAX_CONTROLS {
        found
            .changed
            .push(format!("controls: the first {MAX_CONTROLS} were made, and {} more were left out.", controls.len() - MAX_CONTROLS));
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || " ._()&'+/#%-".contains(c);
    // Each control's name as asked for, fitted: a name Kumi changes keeps clear of the names the controls after it ask for.
    let asked: Vec<String> = controls
        .iter()
        .take(MAX_CONTROLS)
        .enumerate()
        .map(|(index, raw)| {
            let given = trimmed_string(object_of(raw).get("name"));
            if CONTROL.is_match(&given) {
                given
            } else {
                fitted(&given, allowed, |c| c.is_ascii_alphabetic(), &format!("Control {}", index + 1))
            }
        })
        .collect();
    let mut checked: Vec<Control> = Vec::new();
    // For a control whose Param Kumi changed: the Param it was asked as, and why that one couldn't be its.
    let mut renamed: HashMap<String, (String, String)> = HashMap::new();
    // The Mix and Output controls left out of a device Kumi gives its own.
    let mut left_out: Vec<String> = Vec::new();
    for (index, raw) in controls.iter().take(MAX_CONTROLS).enumerate() {
        let control = object_of(raw);
        let label = format!("controls[{index}]");
        let given = trimmed_string(control.get("name"));
        let wanted = asked[index].clone();
        if sound && MIX_OR_OUTPUT.is_match(&wanted) {
            let what = if kind == DeviceType::AudioEffect { "audio effect" } else { "instrument" };
            found.changed.push(format!("{label} ({wanted}): Kumi adds {wanted} to every {what} itself, so this one was left out."));
            left_out.push(wanted);
            continue;
        }
        let mut why: Vec<String> = Vec::new();
        if wanted != given {
            why.push("1–32 letters, digits, spaces and . _ ( ) & ' + / # % -, starting with a letter".into());
        }
        // Whose a name is already (Live's, or another control's, by its name or its Param), if anyone's; `later` counts
        // the names the controls after this one ask for.
        let clash = |candidate: &str, later: bool| -> Option<String> {
            if DEVICE_ON.is_match(candidate) {
                return Some("Device On is Live's own switch".into());
            }
            let same = |other: &str| other.eq_ignore_ascii_case(candidate) || (gen && param_name(other) == param_name(candidate));
            let earlier = checked.iter().map(Control::name).find(|other| same(other));
            let after = if later { asked[index + 1..].iter().map(String::as_str).find(|other| same(other)) } else { None };
            earlier.or(after).map(|other| {
                if gen {
                    format!("{} is the \"{other}\" control's Param", param_name(candidate))
                } else {
                    format!("\"{other}\" is another control's name")
                }
            })
        };
        // In GenExpr a control is its Param: a name gen~ or Kumi uses is renamed, clear of every other control's.
        let control_name = if gen && param_problem(&wanted).is_some() {
            let id = param_name(&wanted);
            why.push(if id.starts_with("kumi_") {
                format!("{id} is kept for Kumi's own Params")
            } else if ["note", "velocity", "strike", "bend", "mod_wheel"].contains(&id.as_str()) {
                format!("{id} is kept for an instrument's voices")
            } else {
                format!("{id} is gen~'s own")
            });
            let amount = fitted(&format!("{wanted} Amount"), allowed, |c| c.is_ascii_alphabetic(), "");
            let amount = if !amount.is_empty() && param_problem(&amount).is_none() { amount } else { format!("Control {}", index + 1) };
            unique(&amount, |candidate| clash(candidate, true).is_some())
        } else {
            if let Some(whose) = clash(&wanted, false) {
                why.push(whose);
            }
            unique(&wanted, |candidate| clash(candidate, false).is_some())
        };
        if control_name != given {
            let param = if gen { format!(", whose Param in the code is {}", param_name(&control_name)) } else { String::new() };
            found.changed.push(format!(
                "{label}.name {}: made as \"{control_name}\"{param}: {}.",
                stringify(&Value::String(given.clone())),
                why.join("; ")
            ));
            if gen && param_name(&control_name) != param_name(&wanted) {
                renamed.insert(control_name.clone(), (param_name(&wanted), why.last().cloned().unwrap_or_default()));
            }
        }
        let label = format!("{label} ({control_name})");
        let given_type = control.get("type").and_then(Value::as_str).unwrap_or("");
        let control_type = match given_type {
            "number" | "integer" | "choice" | "switch" | "trigger" => given_type,
            _ => {
                let made = if control.get("options").is_some_and(Value::is_array) {
                    "choice"
                } else if control.get("default").is_some_and(Value::is_boolean) {
                    "switch"
                } else {
                    "number"
                };
                found.changed.push(format!(
                    "{label}: type {}: made as a {made} (number, integer, choice, switch or trigger).",
                    stringify(&Value::String(given_type.into()))
                ));
                made
            }
        };
        let place = place_of(&control, control_type, &label, &mut found);
        if control_type == "number" || control_type == "integer" {
            let finite = |key: &str| control.get(key).and_then(Value::as_f64).filter(|value| value.is_finite());
            let (given_min, given_max, given_default) = (finite("min"), finite("max"), finite("default"));
            let mut said: Vec<String> = ["min", "max", "default"]
                .iter()
                .zip([given_min, given_max, given_default])
                .filter(|(_, value)| value.is_none())
                .map(|(key, _)| format!("{key} wasn't a number"))
                .collect();
            let mut min = given_min.unwrap_or(0.0);
            let mut max = given_max.unwrap_or(if min < 1.0 { 1.0 } else { min + 1.0 });
            if min > max {
                std::mem::swap(&mut min, &mut max);
                said.push("min was above max".into());
            }
            let mut fallback = given_default.unwrap_or(min);
            if control_type == "integer" && ![min, max, fallback].iter().all(|value| value.fract() == 0.0) {
                (min, max, fallback) = (min.floor(), max.ceil(), fallback.round());
                said.push("an integer control's min, max and default are whole numbers".into());
            }
            if min == max {
                // One more, or past 2^53, where one more is the same number, the next number there is.
                max = if min + 1.0 > min { min + 1.0 } else { min.next_up() };
                if !max.is_finite() {
                    (min, max) = (min.next_down(), min);
                }
                said.push("min and max were the same".into());
            }
            if fallback < min || fallback > max {
                fallback = fallback.clamp(min, max);
                said.push("the default was outside min and max".into());
            }
            let unit = match control.get("unit") {
                None | Some(Value::Null) => Unit::None,
                Some(given) => match given.as_str().map(|text| (text, unit_like(text))) {
                    Some((text, Some(unit))) => {
                        if unit.as_str() != text {
                            said.push(format!("unit {} is \"{unit}\"", stringify(given)));
                        }
                        unit
                    }
                    Some((_, None)) => {
                        let units =
                            UNITS.iter().filter(|unit| **unit != Unit::None).map(|unit| unit.as_str()).collect::<Vec<_>>().join(", ");
                        said.push(format!("unit {} isn't one of {units}, so it shows none", stringify(given)));
                        Unit::None
                    }
                    None => {
                        said.push(format!("unit {} isn't text, so it shows none", stringify(given)));
                        Unit::None
                    }
                },
            };
            if !said.is_empty() {
                found.changed.push(format!(
                    "{label}: {}; made {} to {}, starting at {}.",
                    said.join(", "),
                    number_to_string(min),
                    number_to_string(max),
                    number_to_string(fallback)
                ));
            }
            let numeric = NumericControl { name: control_name, min, max, default: fallback, unit, place };
            checked.push(if control_type == "integer" { Control::Integer(numeric) } else { Control::Number(numeric) });
        } else if control_type == "choice" {
            let given_options: Vec<Value> = match control.get("options") {
                Some(Value::Array(items)) => items.clone(),
                _ => Vec::new(),
            };
            let mut said: Vec<String> = Vec::new();
            // Each option as it's made, in the order given (None for one left out).
            let made: Vec<Option<String>> = given_options
                .iter()
                .map(|option| {
                    let text = match option {
                        Value::String(text) => trim(text).to_string(),
                        Value::Number(number) => number.to_string(),
                        _ => String::new(),
                    };
                    if text.is_empty() {
                        said.push(format!("option {} was left out", stringify(option)));
                        None
                    } else if utf16_len(&text) > 32 {
                        said.push(format!("option \"{text}\" was cut to 32 characters"));
                        Some(head(&text, 32))
                    } else {
                        Some(text)
                    }
                })
                .collect();
            let mut options: Vec<String> = made.iter().flatten().cloned().collect();
            if options.len() > MAX_OPTIONS {
                said.push(format!("only the first {MAX_OPTIONS} options were kept"));
                options.truncate(MAX_OPTIONS);
            }
            if kind == DeviceType::MidiEffect && !patched && given_options.iter().any(Value::is_number) {
                said.push(
                    "its options were numbers, made as text: the code gets an option's text (\"4\"), which Number(…) makes a number".into(),
                );
            }
            if options.is_empty() {
                said.push("it had no options".into());
                options.push(control_name.clone());
            }
            // An option's text; a number is the option it's the text of, else the one it counts to from 0, as given.
            let given_default = control.get("default");
            let default = match given_default {
                Some(Value::String(text)) if options.iter().any(|option| option == trim(text)) => Some(trim(text).to_string()),
                Some(Value::Number(number)) if options.contains(&number.to_string()) => Some(number.to_string()),
                Some(Value::Number(number)) => {
                    number.as_u64().and_then(|at| made.get(at as usize).cloned().flatten()).filter(|option| options.contains(option))
                }
                _ => None,
            };
            let default = default.unwrap_or_else(|| {
                said.push(format!("its default {} isn't one of its options", given_default.map_or("(none)".to_string(), stringify)));
                options[0].clone()
            });
            if !said.is_empty() {
                found.changed.push(format!(
                    "{label}: {}; made with {}, starting at \"{default}\".",
                    said.join(", "),
                    options.iter().map(|option| format!("\"{option}\"")).collect::<Vec<_>>().join(", ")
                ));
            }
            checked.push(Control::Choice(ChoiceControl { name: control_name, options, default, place }));
        } else if control_type == "trigger" {
            checked.push(Control::Switch(SwitchControl { name: control_name, default: false, place }));
        } else {
            let default = match control.get("default") {
                Some(Value::Bool(on)) => *on,
                given => {
                    let on = match given {
                        Some(Value::Number(number)) => number.as_f64().is_some_and(|value| value != 0.0),
                        Some(Value::String(text)) => matches!(text.trim().to_lowercase().as_str(), "on" | "true" | "yes" | "1"),
                        _ => false,
                    };
                    found.changed.push(format!(
                        "{label}: a switch's default is true or false, not {}; made {}.",
                        given.map_or("(none)".to_string(), stringify),
                        if on { "on" } else { "off" }
                    ));
                    on
                }
            };
            checked.push(Control::Switch(SwitchControl { name: control_name, default, place }));
        }
    }
    let mut code = input.get("code").and_then(Value::as_str).unwrap_or("").to_string();
    let mut runs = true;
    if patched {
        if !trim(&code).is_empty() {
            found.changed.push(
                "code: the device was made from its patch, so the code beside it was left out (a gen~ in the patch holds its own code)."
                    .to_string(),
            );
            code.clear();
        }
    } else if trim(&code).is_empty() {
        let (does, give) = match kind {
            DeviceType::MidiEffect => ("passes MIDI on as it comes", "JavaScript"),
            DeviceType::AudioEffect => ("passes its input on as it comes", "GenExpr"),
            DeviceType::Instrument => ("makes no sound", "GenExpr for one voice"),
        };
        found.wrong.push(format!("code: none given, so the device {does}: give it its {give}."));
        code = match kind {
            DeviceType::MidiEffect => "function midi(event) { pass(event); }".to_string(),
            DeviceType::AudioEffect => "out1 = in1;\nout2 = in2;".to_string(),
            DeviceType::Instrument => "out1 = 0;\nout2 = 0;".to_string(),
        };
    } else if gen {
        found.append(check_gen_code(&code, kind, &checked, &renamed));
        let read = names(&code).read;
        for left in &left_out {
            let id = param_name(left);
            if read.contains(&id) {
                found.wrong.push(format!(
                    "code: it reads {id}, the {left} control's Param, but that control was left out (Kumi's own {left} comes after the code): call the control something else, or take {id} out of the code."
                ));
            }
        }
    } else {
        if !MIDI_FUNCTION.is_match(&code) {
            found.wrong.push("code: it defines no midi(event), so the device passes MIDI on as it comes: define function midi(event), called for each MIDI event that arrives.".to_string());
        }
        // Matched without comments, and as written too: the comment stripper doesn't know strings, so a "//" or "/*"
        // inside one could hide the rest of a line (an `import(…)`, which the frame can't shadow) from the first.
        let bare = COMMENTS.replace_all(&code, "");
        for (pattern, why) in FORBIDDEN.iter() {
            if let Some(found_text) = pattern.find(&bare) {
                found.wrong.push(format!("code: \"{}\" doesn't work in the device: {why}.", trim(found_text.as_str())));
            } else if let Some(found_text) = pattern.find(&code) {
                found.wrong.push(format!(
                    "code: \"{}\" doesn't work in the device, even in a comment or a string: {why}.",
                    trim(found_text.as_str())
                ));
            }
        }
        // A module runs outside every name the frame hides, and code that isn't one whole function body could too: either
        // is kept in the device without running. A module is found as written as well (see above).
        let module = MODULES
            .find(&bare)
            .map(|module| (module.as_str().to_string(), ""))
            .or_else(|| MODULES.find(&code).map(|module| (module.as_str().to_string(), ", even in a comment or a string")));
        let unrun = match module {
            Some((text, written)) => Some(format!(
                "code: \"{}\" doesn't work in the device{written}: modules aren't available, and one would run outside every name the frame hides.",
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            )),
            None => whole_body(&code),
        };
        if let Some(problem) = unrun {
            found
                .wrong
                .push(format!("{problem} Kumi kept the code in the device without running it, so the device passes MIDI on as it comes."));
            code = kept_unrun(&code, problem.trim_start_matches("code: ").trim_end_matches('.'));
            runs = false;
        }
    }
    // Only a MIDI effect's code runs outside Live; an audio effect or instrument is heard with audition once loaded.
    let tests: Vec<Value> = match input.get("tests") {
        Some(Value::Array(items)) if patched && !items.is_empty() => {
            found.changed.push("tests: they run a MIDI effect's code, and a patch is heard in Live, so they weren't run.".to_string());
            Vec::new()
        }
        Some(Value::Array(items)) if gen && !items.is_empty() => {
            found.changed.push(
                "tests: they run a MIDI effect's code, and an audio effect or instrument is heard with audition once it's loaded, so they weren't run."
                    .to_string(),
            );
            Vec::new()
        }
        Some(Value::Array(items)) if !gen && !runs && !items.is_empty() => {
            found.changed.push("tests: the code doesn't run, so they weren't run.".to_string());
            Vec::new()
        }
        Some(Value::Array(items)) if !gen => items.clone(),
        _ => Vec::new(),
    };
    if tests.len() > MAX_TESTS {
        found.changed.push(format!("tests: the first {MAX_TESTS} were run, and {} more weren't.", tests.len() - MAX_TESTS));
    }
    let mut checked_tests: Vec<MidiTest> = Vec::new();
    for (index, raw) in tests.iter().take(MAX_TESTS).enumerate() {
        let test = object_of(raw);
        let label = format!("tests[{index}]");
        let events = |value: Option<&Value>| -> Option<Vec<MidiEvent>> {
            match value {
                Some(Value::Array(items)) if items.len() <= MAX_TEST_EVENTS => items.iter().map(event_of).collect(),
                _ => None,
            }
        };
        let input_events = events(test.get("input"));
        let expected = events(test.get("expect"));
        let name = test.get("name").and_then(Value::as_str).map(trim).filter(|name| !name.is_empty());
        let (Some(name), Some(input_events), Some(expected)) = (name, input_events, expected) else {
            let types = EVENT_TYPES.iter().map(|kind| kind.as_str()).collect::<Vec<_>>().join(", ");
            found.changed.push(format!(
                "{label} wasn't run: a test has a name, input and expect, each up to {MAX_TEST_EVENTS} events of type {types} (at: milliseconds)."
            ));
            continue;
        };
        let set = match test.get("set") {
            None => None,
            Some(Value::Object(set)) => Some(set.clone()),
            Some(_) => {
                found.changed.push(format!("{label} wasn't run: set names controls and their values."));
                continue;
            }
        };
        checked_tests.push(MidiTest { name: head(name, 80), set, input: input_events, expect: expected });
    }
    let mut voices: u32 = 8;
    if kind == DeviceType::Instrument && !patched {
        if let Some(given) = input.get("voices") {
            match given.as_f64().filter(|value| value.is_finite()) {
                Some(value) if value.fract() == 0.0 && (1.0..=MAX_VOICES as f64).contains(&value) => voices = value as u32,
                Some(value) => {
                    voices = value.round().clamp(1.0, MAX_VOICES as f64) as u32;
                    found.changed.push(format!("voices {}: made {voices} (1, mono, to {MAX_VOICES}).", stringify(given)));
                }
                None => found.changed.push(format!("voices {}: made 8 (1, mono, to {MAX_VOICES}).", stringify(given))),
            }
        }
    }
    let mut runs_free = false;
    if !gen && !patched {
        match input.get("runs_free") {
            None | Some(Value::Null) => {}
            Some(Value::Bool(given)) => runs_free = *given,
            Some(given) => found.changed.push(format!(
                "runs_free {}: taken as false (true for a MIDI effect that keeps sending on its own: an LFO, a clock, a generator).",
                stringify(given)
            )),
        }
    }
    let map = map_of(input.get("map"), patched, &mut found);
    let spec = if patched {
        DeviceSpec::Patch(PatchSpec { kind, name, about, controls: checked, patch, map })
    } else {
        match kind {
            DeviceType::AudioEffect => DeviceSpec::AudioEffect(AudioEffectSpec { name, about, controls: checked, code }),
            DeviceType::Instrument => DeviceSpec::Instrument(InstrumentSpec { name, about, controls: checked, code, voices }),
            DeviceType::MidiEffect => {
                DeviceSpec::MidiEffect(MidiSpec { name, about, controls: checked, code, tests: checked_tests, runs_free })
            }
        }
    };
    (spec, found)
}
