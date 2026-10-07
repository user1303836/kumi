//! What the model asks for when it makes a device, and the checks it has to pass. The model never
//! writes a patch: it names the device, its knobs and its code, and Kumi builds the rest.

use std::collections::HashSet;
use std::sync::LazyLock;

use kumi_common::js::json::stringify;
use kumi_common::js::string::{head, trim, utf16_len};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::amxd::DeviceType;
use super::gen::{check_gen_code, param_name, param_problem, MAX_VOICES};

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
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ChoiceControl {
    pub name: String,
    pub options: Vec<String>,
    pub default: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SwitchControl {
    pub name: String,
    pub default: bool,
}

impl Control {
    pub fn name(&self) -> &str {
        match self {
            Control::Number(control) | Control::Integer(control) => &control.name,
            Control::Choice(control) => &control.name,
            Control::Switch(control) => &control.name,
        }
    }

    /// The control's type as the model names it: number, integer, choice or switch.
    pub fn type_name(&self) -> &'static str {
        match self {
            Control::Number(_) => "number",
            Control::Integer(_) => "integer",
            Control::Choice(_) => "choice",
            Control::Switch(_) => "switch",
        }
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceSpec {
    MidiEffect(MidiSpec),
    AudioEffect(AudioEffectSpec),
    Instrument(InstrumentSpec),
}

impl DeviceSpec {
    pub fn kind(&self) -> DeviceKind {
        match self {
            DeviceSpec::MidiEffect(_) => DeviceType::MidiEffect,
            DeviceSpec::AudioEffect(_) => DeviceType::AudioEffect,
            DeviceSpec::Instrument(_) => DeviceType::Instrument,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            DeviceSpec::MidiEffect(spec) => &spec.name,
            DeviceSpec::AudioEffect(spec) => &spec.name,
            DeviceSpec::Instrument(spec) => &spec.name,
        }
    }

    pub fn about(&self) -> &str {
        match self {
            DeviceSpec::MidiEffect(spec) => &spec.about,
            DeviceSpec::AudioEffect(spec) => &spec.about,
            DeviceSpec::Instrument(spec) => &spec.about,
        }
    }

    pub fn controls(&self) -> &[Control] {
        match self {
            DeviceSpec::MidiEffect(spec) => &spec.controls,
            DeviceSpec::AudioEffect(spec) => &spec.controls,
            DeviceSpec::Instrument(spec) => &spec.controls,
        }
    }

    pub fn code(&self) -> &str {
        match self {
            DeviceSpec::MidiEffect(spec) => &spec.code,
            DeviceSpec::AudioEffect(spec) => &spec.code,
            DeviceSpec::Instrument(spec) => &spec.code,
        }
    }
}

pub type DeviceKind = DeviceType;
pub const DEVICE_KINDS: [DeviceKind; 3] = [DeviceType::MidiEffect, DeviceType::AudioEffect, DeviceType::Instrument];

/// Ceilings, not targets: a device gets the controls, options, voices and code it needs, and these
/// only keep its file one Live loads. Past eight, the face shows its controls in up to three rows.
pub const MAX_CONTROLS: usize = 128;
pub const MAX_OPTIONS: usize = 128;
pub const MAX_CODE: usize = 100_000;
pub const MAX_TESTS: usize = 32;
const MAX_TEST_EVENTS: usize = 256;
static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9 ._()&'+-]{0,31}$").unwrap());
static CONTROL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9 ._()&'+/#%-]{0,31}$").unwrap());
static DEVICE_ON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^device on$").unwrap());
static MIX_OR_OUTPUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(mix|output)$").unwrap());
static MIDI_FUNCTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)function\s+midi\s*\(").unwrap());
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
/// the function has to come back holding all of it. HTML-like comments are refused first: Live's V8 reads `<!--` and
/// `-->` as comments in a script and QuickJS doesn't, so the two could read the braces differently.
fn whole_body(code: &str) -> Option<String> {
    if code.contains("<!--") || code.contains("-->") {
        return Some("code: \"<!--\" and \"-->\" aren't allowed: Live reads them as comments, which can change what the code is.".into());
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
        Err(_) => {
            Some("code: it doesn't read as one function's body (a brace, quote or bracket that isn't closed, or one too many).".into())
        }
    }
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
        (Regex::new(r#"(?-u:\b)import\s*[({"'`A-Za-z0-9_*]|(?-u:\b)export\s+"#).unwrap(), "modules aren't available"),
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

/// A spec from the model's input, or the problems with it, each said so the model can fix it.
pub fn check_spec(input: &Map<String, Value>) -> Result<DeviceSpec, Vec<String>> {
    let mut problems: Vec<String> = Vec::new();
    let kind = match input.get("type") {
        None | Some(Value::Null) => Some(DeviceType::MidiEffect),
        Some(Value::String(text)) => DeviceType::parse(text),
        Some(_) => None,
    };
    if kind.is_none() {
        problems.push(format!("type {}: midi_effect, audio_effect or instrument.", stringify(input.get("type").unwrap_or(&Value::Null))));
    }
    let gen = matches!(kind, Some(DeviceType::AudioEffect) | Some(DeviceType::Instrument));
    let name = trimmed_string(input.get("name"));
    if !NAME.is_match(&name) {
        problems.push("name: 1–32 characters, letters, digits, spaces and . _ ( ) & ' + -, starting with a letter or digit.".to_string());
    }
    let about = trimmed_string(input.get("about"));
    if about.is_empty() || utf16_len(&about) > 400 {
        problems.push("about: what the device does, in a sentence or two (up to 400 characters).".to_string());
    }
    let controls: Vec<Value> = match input.get("controls") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    if controls.len() > MAX_CONTROLS {
        problems.push(format!("controls: at most {MAX_CONTROLS}."));
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut checked: Vec<Control> = Vec::new();
    for (index, raw) in controls.iter().take(MAX_CONTROLS).enumerate() {
        let control = object_of(raw);
        let label = format!("controls[{index}]");
        let control_name = trimmed_string(control.get("name"));
        if !CONTROL.is_match(&control_name) || DEVICE_ON.is_match(&control_name) {
            problems.push(format!("{label}.name: 1–32 letters, digits, spaces and . _ ( ) & ' + / # % -, starting with a letter."));
            continue;
        }
        if seen.contains(&control_name.to_lowercase()) {
            problems.push(format!("{label}.name: \"{control_name}\" is used twice."));
            continue;
        }
        // In GenExpr a control is its Param: "Pre-Delay" and "Pre Delay" would both be pre_delay.
        let twin = if gen { checked.iter().find(|other| param_name(other.name()) == param_name(&control_name)) } else { None };
        if let Some(twin) = twin {
            problems.push(format!(
                "{label}.name: \"{control_name}\" and \"{}\" would both be the Param {}; name them apart.",
                twin.name(),
                param_name(&control_name)
            ));
            continue;
        }
        if gen && MIX_OR_OUTPUT.is_match(&control_name) {
            let what = if kind == Some(DeviceType::AudioEffect) { "audio effect" } else { "instrument" };
            problems.push(format!("{label}.name: Kumi adds {control_name} to every {what} itself; leave it out."));
            continue;
        }
        let as_param = if gen { param_problem(&control_name) } else { None };
        if let Some(as_param) = as_param {
            problems.push(format!("{label}.name: {as_param}."));
            continue;
        }
        seen.insert(control_name.to_lowercase());
        let control_type = control.get("type").and_then(Value::as_str).unwrap_or("");
        if control_type == "number" || control_type == "integer" {
            let min = control.get("min");
            let max = control.get("max");
            let fallback = control.get("default");
            let finite = |value: Option<&Value>| value.and_then(Value::as_f64).filter(|value| value.is_finite());
            let numbers = (finite(min), finite(max), finite(fallback));
            let unit = match control.get("unit") {
                None | Some(Value::Null) => Some(Unit::None),
                Some(Value::String(text)) => Unit::parse(text),
                Some(_) => None,
            };
            let (Some(min), Some(max), Some(fallback)) = numbers else {
                problems.push(format!("{label} ({control_name}): min, max and default are numbers."));
                continue;
            };
            if min >= max {
                problems.push(format!("{label} ({control_name}): min is below max."));
                continue;
            }
            if fallback < min || fallback > max {
                problems.push(format!("{label} ({control_name}): the default, a value that sounds right on load, is between min and max."));
                continue;
            }
            if control_type == "integer" && ![min, max, fallback].iter().all(|value| value.fract() == 0.0) {
                problems.push(format!("{label} ({control_name}): an integer control has whole-number min, max and default."));
                continue;
            }
            let Some(unit) = unit else {
                let units = UNITS.iter().filter(|unit| **unit != Unit::None).map(|unit| unit.as_str()).collect::<Vec<_>>().join(", ");
                problems.push(format!("{label} ({control_name}): unit is one of {units}, or \"\" for none."));
                continue;
            };
            let numeric = NumericControl { name: control_name, min, max, default: fallback, unit };
            checked.push(if control_type == "integer" { Control::Integer(numeric) } else { Control::Number(numeric) });
        } else if control_type == "choice" {
            let options: Vec<Value> = match control.get("options") {
                Some(Value::Array(items)) => items.clone(),
                _ => Vec::new(),
            };
            let strings: Option<Vec<String>> = options
                .iter()
                .map(|option| option.as_str().filter(|text| !trim(text).is_empty() && utf16_len(text) <= 32).map(str::to_string))
                .collect();
            let Some(options) = strings.filter(|options| options.len() >= 2 && options.len() <= MAX_OPTIONS) else {
                problems.push(format!("{label} ({control_name}): 2–{MAX_OPTIONS} options of up to 32 characters."));
                continue;
            };
            let Some(default) = control.get("default").and_then(Value::as_str).filter(|text| options.iter().any(|option| option == text))
            else {
                problems.push(format!("{label} ({control_name}): the default is one of the options."));
                continue;
            };
            checked.push(Control::Choice(ChoiceControl { name: control_name, options, default: default.to_string() }));
        } else if control_type == "switch" {
            let Some(default) = control.get("default").and_then(Value::as_bool) else {
                problems.push(format!("{label} ({control_name}): a switch's default is true or false."));
                continue;
            };
            checked.push(Control::Switch(SwitchControl { name: control_name, default }));
        } else {
            problems.push(format!("{label} ({control_name}): type is number, integer, choice or switch."));
        }
    }
    let code = input.get("code").and_then(Value::as_str).unwrap_or("").to_string();
    if trim(&code).is_empty() || utf16_len(&code) > MAX_CODE {
        problems.push(format!("code: the device's {}, up to 100,000 characters.", if gen { "GenExpr" } else { "JavaScript" }));
    } else if gen {
        problems.extend(check_gen_code(&code, kind.expect("a gen device has a type"), &checked));
    } else {
        if !MIDI_FUNCTION.is_match(&code) {
            problems.push("code: define function midi(event), called for each MIDI event that arrives.".to_string());
        }
        let bare = COMMENTS.replace_all(&code, "");
        for (pattern, why) in FORBIDDEN.iter() {
            if let Some(found) = pattern.find(&bare) {
                problems.push(format!("code: \"{}\" isn't allowed: {why}.", trim(found.as_str())));
            }
        }
        problems.extend(whole_body(&code));
    }
    // Only a MIDI effect's code runs outside Live; an audio effect or instrument is heard with audition once loaded.
    let tests: Vec<Value> = match input.get("tests") {
        Some(Value::Array(items)) if !gen => items.clone(),
        _ => Vec::new(),
    };
    if tests.len() > MAX_TESTS {
        problems.push(format!("tests: at most {MAX_TESTS}."));
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
            problems.push(format!(
                "{label}: a name, input and expect, each up to {MAX_TEST_EVENTS} events of type {types} (at: milliseconds)."
            ));
            continue;
        };
        let set = match test.get("set") {
            None => None,
            Some(Value::Object(set)) => Some(set.clone()),
            Some(_) => {
                problems.push(format!("{label}: set names controls and their values."));
                continue;
            }
        };
        checked_tests.push(MidiTest { name: head(name, 80), set, input: input_events, expect: expected });
    }
    let mut voices: u32 = 8;
    if kind == Some(DeviceType::Instrument) {
        if let Some(given) = input.get("voices") {
            match given.as_f64().filter(|value| value.fract() == 0.0 && *value >= 1.0 && *value <= MAX_VOICES as f64) {
                Some(given) => voices = given as u32,
                None => problems.push(format!("voices: how many notes play at once, 1 (mono) to {MAX_VOICES}.")),
            }
        }
    }
    if !gen {
        if let Some(runs_free) = input.get("runs_free") {
            if !runs_free.is_boolean() {
                problems
                    .push("runs_free: true for a MIDI effect that keeps sending on its own (an LFO, a clock, a generator).".to_string());
            }
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    match kind {
        Some(DeviceType::AudioEffect) => Ok(DeviceSpec::AudioEffect(AudioEffectSpec { name, about, controls: checked, code })),
        Some(DeviceType::Instrument) => Ok(DeviceSpec::Instrument(InstrumentSpec { name, about, controls: checked, code, voices })),
        _ => Ok(DeviceSpec::MidiEffect(MidiSpec {
            name,
            about,
            controls: checked,
            code,
            tests: checked_tests,
            runs_free: input.get("runs_free") == Some(&Value::Bool(true)),
        })),
    }
}
