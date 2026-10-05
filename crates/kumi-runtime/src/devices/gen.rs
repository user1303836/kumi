//! Audio effects and instruments: the model writes GenExpr (the language of Max's gen~) and Kumi
//! builds the device around it.
//!
//! - An audio effect: plugin~ → gen~ (the model's code) → Kumi's output stage → plugout~.
//! - An instrument: notein → poly (voice allocation) → one gen~ per voice (the model's voice) →
//!   Kumi's output stage → plugout~.
//!
//! Kumi's output stage is fixed and always there: the model's own output has NaN, denormals and DC
//! taken out and is held under +6 dBFS (a runaway feedback patch is caught); on an effect a Mix knob
//! blends it with the dry signal, which passes untouched (a hot track isn't clipped by Kumi's device);
//! an Output knob sets the level. Each of the model's controls is a Param in the code, named
//! like the control ("Decay Time" is decay_time), and an ordinary Live parameter on the device's face.
//!
//! GenExpr's order is fixed: function definitions, then declarations (Param, History…), then
//! statements. Kumi's Params go after the model's functions, ahead of the rest of its code.

use std::collections::HashSet;
use std::sync::LazyLock;

use kumi_common::js::number::{round, to_string as num};
use kumi_common::js::string::{head, trim_end};
use regex::Regex;
use serde_json::{json, Value};

use super::amxd::{device_patcher, face_layout, DevicePatcherOptions, DeviceType, FaceLayout, Line, PatchBox};
use super::midi::unit_style;
use super::spec::{AudioEffectSpec, Control, InstrumentSpec, NumericControl, Unit};

/// gen~'s operators, constants and keywords (as Max 9's gen~ names them), which a control's Param
/// mustn't be (a Param named mix would hide the mix operator), and the names Kumi's own Params use.
static RESERVED: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    concat!(
        "abs absdiff accum acos acosh add and asin asinh atan atan2 atanh atodb bool break buffer cartopol ceil change channels clamp clip ",
        "constant continue cos cosh counter cycle data dbtoa dcblock degrees degtorad delay delta dim div e elapsed else eq eqp exp exp2 expr f fastcos fastexp ",
        "fastpow fastsin fasttan fftfullspect ffthop fftinfo fftoffset fftsize fixdenorm fixnan float floor fold for fract ftom gate gen gt gte gtep gtp halfpi ",
        "history hypot i if in int interp invpi isdenorm isnan latch ln ln10 ln2 log log10 log10e log2 log2e lookup lt lte ltep ltp max maximum min minimum mix ",
        "mod mstosamps mtof mul mulequals nearest neg neq neqp noise not or out param pass peek phasewrap phasor phi pi plusequals poke poltocar pow r radians ",
        "radtodeg rate rdiv read receive return rmod round rsub s sah sample samplerate sampstoms scale selector send setparam sign sin sinh slide smoothstep ",
        "splat sqrt sqrt1_2 sqrt2 step sub switch t60 t60time tan tanh train triangle trunc twopi vectorsize voice voices wave while wrap write xor ",
        "note velocity strike bend mod_wheel gain wet"
    )
    .split(' ')
    .collect()
});

static NOT_PARAM_CHARS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());
static EDGE_UNDERSCORES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^_+|_+$").unwrap());
static PARAM_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_]*$").unwrap());
static COMMENTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"//[^\n]*|/\*[\s\S]*?\*/").unwrap());
static FUNCTION_HEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap());
static OPEN_BRACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\{").unwrap());
static INPUT_READ: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)in([1-9])(?-u:\b)").unwrap());
static OUT1: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)out1\s*=").unwrap());
static OUT2: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)out2\s*=").unwrap());
static OUT_MORE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)out[3-9](?-u:\b)").unwrap());
static KUMI_PARAMS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?-u:\b)Param\s+(note|velocity|strike|bend|mod_wheel|kumi_[A-Za-z0-9_]*)(?-u:\b)").unwrap());
static NOTE_WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)(note|velocity|strike|mod_wheel)(?-u:\b)").unwrap());
static NOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)note(?-u:\b)").unwrap());
static LINE_ENDINGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r?\n").unwrap());
static LEADING_BLANK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\n").unwrap());

/// A control's name as its Param in the code: "Decay Time" → decay_time.
pub fn param_name(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let joined = NOT_PARAM_CHARS.replace_all(&lower, "_");
    EDGE_UNDERSCORES.replace_all(&joined, "").into_owned()
}

/// Why a control's name can't be a Param, if it can't.
pub fn param_problem(name: &str) -> Option<String> {
    let id = param_name(name);
    if !PARAM_ID.is_match(&id) {
        return Some(format!("\"{name}\" needs to start with a letter to be a Param in the code"));
    }
    if RESERVED.contains(id.as_str()) || id.starts_with("kumi_") {
        return Some(format!(
            "\"{name}\" would be the Param {id}, a name gen~ or Kumi already uses; call it something else (such as \"{name} Amount\")"
        ));
    }
    None
}

/// The code with every comment turned to spaces, so positions in it are positions in the code.
fn without_comments(code: &str) -> String {
    COMMENTS
        .replace_all(code, |comment: &regex::Captures| {
            comment[0].chars().map(|c| if c == '\n' { "\n".to_string() } else { " ".repeat(c.len_utf8()) }).collect::<String>()
        })
        .into_owned()
}

/// Where the model's leading function definitions end (GenExpr puts them first): an identifier, its
/// parameters in parentheses, then a braced body; anything else starts the declarations and statements.
/// The position is a byte index into `code`.
pub fn after_functions(code: &str) -> usize {
    let bare = without_comments(code);
    let keywords: HashSet<&str> =
        ["if", "else", "for", "while", "return", "break", "continue", "Param", "History", "Data", "Buffer", "Delay"].into();
    let bytes = bare.as_bytes();
    let mut at = 0;
    loop {
        let Some(head) = FUNCTION_HEAD.captures(&bare[at..]) else { return at };
        if keywords.contains(&head[1]) {
            return at;
        }
        let mut index = at + head[0].len();
        let mut depth = 1;
        while index < bytes.len() && depth > 0 {
            if bytes[index] == b'(' {
                depth += 1;
            } else if bytes[index] == b')' {
                depth -= 1;
            }
            index += 1;
        }
        let brace = OPEN_BRACE.find(&bare[index..]);
        let Some(brace) = brace.filter(|_| depth == 0) else { return at }; // a call, not a definition
        index += brace.len();
        depth = 1;
        while index < bytes.len() && depth > 0 {
            if bytes[index] == b'{' {
                depth += 1;
            } else if bytes[index] == b'}' {
                depth -= 1;
            }
            index += 1;
        }
        if depth > 0 {
            return at;
        }
        at = index;
    }
}

/// The highest inN the code reads (0 when none): how many inlets its codebox has.
pub fn inputs_read(code: &str) -> usize {
    let bare = without_comments(code);
    INPUT_READ.captures_iter(&bare).map(|found| found[1].parse::<usize>().unwrap_or(0)).max().unwrap_or(0)
}

/// What Kumi checks in the model's GenExpr before building (Max compiles it when Live loads the device).
pub fn check_gen_code(code: &str, kind: DeviceType, controls: &[Control]) -> Vec<String> {
    let mut problems: Vec<String> = Vec::new();
    let bare = without_comments(code);
    if !OUT1.is_match(&bare) || !OUT2.is_match(&bare) {
        problems.push("code: assign out1 (left) and out2 (right), every sample.".to_string());
    }
    if OUT_MORE.is_match(&bare) {
        problems.push("code: the device has two outputs, out1 (left) and out2 (right).".to_string());
    }
    let mut braces: i64 = 0;
    let mut parens: i64 = 0;
    let mut broken = false;
    for char in bare.chars() {
        match char {
            '{' => braces += 1,
            '}' => braces -= 1,
            '(' => parens += 1,
            ')' => parens -= 1,
            _ => {}
        }
        if braces < 0 || parens < 0 {
            broken = true;
            break;
        }
    }
    if broken || braces != 0 || parens != 0 {
        problems.push("code: its braces or parentheses don't pair up.".to_string());
    }
    if KUMI_PARAMS.is_match(&bare) {
        problems.push("code: note, velocity, strike, bend and mod_wheel are Kumi's; use them without declaring them.".to_string());
    }
    // Kumi declares each control's Param (its default and range come from the control); declared again, gen~ refuses the code.
    for control in controls {
        let id = param_name(control.name());
        let declared =
            Regex::new(&format!(r"(?-u:\b)(?:Param|History|Delay|Data|Buffer)\s+{}(?-u:\b)", regex::escape(&id))).expect("a Param's name");
        if declared.is_match(&bare) {
            problems.push(format!(
                "code: {id} is the {} control's Param, which Kumi declares from the control; use {id} without declaring it.",
                control.name()
            ));
        }
    }
    let inputs = inputs_read(code);
    if kind == DeviceType::AudioEffect && inputs == 0 {
        problems.push("code: an audio effect reads its input, in1 (left) and in2 (right).".to_string());
    }
    if kind == DeviceType::AudioEffect && NOTE_WORDS.is_match(&bare) {
        problems.push("code: an audio effect gets no notes; note, velocity, strike and mod_wheel are an instrument's.".to_string());
    }
    if kind == DeviceType::Instrument && inputs > 0 {
        problems.push("code: an instrument has no audio input; make sound from note, velocity, bend and mod_wheel.".to_string());
    }
    if kind == DeviceType::Instrument && !NOTE.is_match(&bare) {
        problems.push(
            "code: a voice plays the note it's given: use note (the MIDI note number, e.g. mtof(note + bend) for its frequency)."
                .to_string(),
        );
    }
    if inputs > 2 {
        problems.push("code: the device has two inputs, in1 (left) and in2 (right).".to_string());
    }
    problems
}

/// The Param declarations for the model's controls.
fn param_lines(controls: &[Control]) -> Vec<String> {
    controls
        .iter()
        .map(|control| {
            let id = param_name(control.name());
            match control {
                Control::Switch(control) => format!("Param {id}({}, min=0, max=1);", if control.default { 1 } else { 0 }),
                Control::Choice(control) => format!(
                    "Param {id}({}, min=0, max={});",
                    option_index(control.options.as_slice(), &control.default),
                    control.options.len() as i64 - 1
                ),
                Control::Number(control) | Control::Integer(control) => {
                    format!("Param {id}({}, min={}, max={});", num(control.default), num(control.min), num(control.max))
                }
            }
        })
        .collect()
}

/// `options.indexOf(option)`: -1 when it isn't one.
fn option_index(options: &[String], option: &str) -> i64 {
    options.iter().position(|item| item == option).map(|index| index as i64).unwrap_or(-1)
}

/// The model's code with Kumi's declarations in their place: after its functions, before the rest.
pub fn with_params(code: &str, lines: &[String], heading: &str) -> String {
    let split = after_functions(code);
    let functions = trim_end(&code[..split]);
    let rest = LEADING_BLANK.replace(&code[split..], "");
    let mut parts: Vec<String> = vec![format!("// {heading}")];
    if !functions.is_empty() {
        parts.push(functions.to_string());
        parts.push(String::new());
    }
    parts.push("// Kumi's Params (from the device's controls):".to_string());
    parts.extend(lines.iter().cloned());
    parts.push(String::new());
    parts.push(rest.into_owned());
    parts.join("\n")
}

const OUTPUT_STAGE_EFFECT: &str = concat!(
    "// Kumi's output stage (fixed): the effect's output made safe (NaN, denormals and DC out, held under +6 dBFS),\n",
    "// mixed with the dry signal, which passes untouched, then Output.\n",
    "Param kumi_mix(100, min=0, max=100);\n",
    "Param kumi_output(0, min=-36, max=12);\n",
    "gain = dbtoa(kumi_output);\n",
    "wet = kumi_mix * 0.01;\n",
    "out1 = mix(in3, clamp(dcblock(fixnan(fixdenorm(in1))), -2, 2), wet) * gain;\n",
    "out2 = mix(in4, clamp(dcblock(fixnan(fixdenorm(in2))), -2, 2), wet) * gain;"
);

const OUTPUT_STAGE_INSTRUMENT: &str = concat!(
    "// Kumi's output stage (fixed): the voices made safe (NaN, denormals and DC out, held under +6 dBFS), then Output.\n",
    "Param kumi_output(0, min=-36, max=12);\n",
    "gain = dbtoa(kumi_output);\n",
    "out1 = clamp(dcblock(fixnan(fixdenorm(in1))), -2, 2) * gain;\n",
    "out2 = clamp(dcblock(fixnan(fixdenorm(in2))), -2, 2) * gain;"
);

/// Kumi's per-voice Params, set for each note: the note, its velocity (0 once released), strike (a new
/// count on every note played, so change(strike) != 0 starts a note even when a busy voice is taken for
/// one at the same velocity), the bend in semitones and the mod wheel.
const VOICE_PARAMS: [&str; 5] = [
    "Param note(60, min=0, max=127);",
    "Param velocity(0, min=0, max=127);",
    "Param strike(0);",
    "Param bend(0, min=-2, max=2);",
    "Param mod_wheel(0, min=0, max=1);",
];

/// Max saves code with Windows line endings; it reads either, and Kumi writes it the way Max does.
fn max_lines(code: &str) -> String {
    LINE_ENDINGS.replace_all(code, "\r\n").into_owned()
}

/// A gen~ box holding one codebox, wired to as many inlets as the code reads and two outlets.
fn gen_box(id: &str, code: &str, rect: [f64; 4]) -> PatchBox {
    let ins = inputs_read(code);
    let mut inner: Vec<PatchBox> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    inner.push(PatchBox::new(json!({ "id": "obj-code", "maxclass": "codebox", "code": max_lines(code), "fontface": 0, "fontname": "<Monospaced>", "fontsize": 12.0, "numinlets": ins, "numoutlets": 2,
        "outlettype": ["", ""], "patching_rect": [40.0, 80.0, 600.0, 400.0] })));
    for index in 1..=ins {
        inner.push(PatchBox::new(json!({ "id": format!("obj-in-{index}"), "maxclass": "newobj", "text": format!("in {index}"), "numinlets": 0, "numoutlets": 1, "outlettype": [""], "patching_rect": [40.0 + (index - 1) as f64 * 90.0, 20.0, 30.0, 22.0] })));
        lines.push(Line::new(&format!("obj-in-{index}"), 0, "obj-code", index as u32 - 1));
    }
    for index in [1usize, 2] {
        inner.push(PatchBox::new(json!({ "id": format!("obj-out-{index}"), "maxclass": "newobj", "text": format!("out {index}"), "numinlets": 1, "numoutlets": 0, "patching_rect": [40.0 + (index - 1) as f64 * 90.0, 510.0, 37.0, 22.0] })));
        lines.push(Line::new("obj-code", index as u32 - 1, &format!("obj-out-{index}"), 0));
    }
    // gen~ has an inlet for each in, and always at least one, which also takes Param messages.
    PatchBox::new(
        json!({ "id": id, "maxclass": "newobj", "text": "gen~", "numinlets": ins.max(1), "numoutlets": 2, "outlettype": ["signal", "signal"], "patching_rect": rect,
        "patcher": { "fileversion": 1, "appversion": { "major": 9, "minor": 1, "revision": 5, "architecture": "x64", "modernui": 1 }, "classnamespace": "dsp.gen",
            "rect": [100.0, 100.0, 720.0, 600.0], "gridsize": [15.0, 15.0], "boxes": inner, "lines": lines } }),
    )
}

/// Builds the face's `total` controls: each sends "<param> <value>" to its targets.
struct Face {
    boxes: Vec<PatchBox>,
    lines: Vec<Line>,
    count: usize,
    layout: FaceLayout,
}

impl Face {
    fn new(total: usize) -> Face {
        Face { boxes: Vec::new(), lines: Vec::new(), count: 0, layout: face_layout(total) }
    }

    fn add(&mut self, control: &Control, param: &str, targets: &[&str]) {
        let index = self.count;
        self.count += 1;
        let id = format!("obj-control-{}", index + 1);
        let (x, y) = self.layout.at(index);
        let (item, valueof) = face_control(control, x, y);
        // A menu's first outlet is the chosen option's index; a dial's and a toggle's is the value.
        let mut item_box = json!({ "id": id, "varname": control.name(), "parameter_enable": 1, "presentation": 1, "patching_rect": [760.0 + index as f64 * 90.0, 30.0, 44.0, 48.0] });
        extend(&mut item_box, item);
        extend(&mut item_box, json!({ "saved_attribute_attributes": { "valueof": valueof } }));
        self.boxes.push(PatchBox::new(item_box));
        let prepend = format!("obj-prepend-{}", index + 1);
        self.boxes.push(PatchBox::new(json!({ "id": prepend, "maxclass": "newobj", "text": format!("prepend {param}"), "numinlets": 1, "numoutlets": 1, "outlettype": [""], "patching_rect": [760.0 + index as f64 * 90.0, 100.0, 80.0, 22.0] })));
        self.lines.push(Line::new(&id, 0, &prepend, 0));
        for target in targets {
            self.lines.push(Line::new(&prepend, 0, target, 0));
        }
    }

    fn width(&self) -> f64 {
        (16 + self.layout.columns * 52).max(140) as f64
    }
}

/// `Object.assign(target, source)` for two JSON objects.
pub(crate) fn extend(target: &mut Value, source: Value) {
    if let (Value::Object(target), Value::Object(source)) = (target, source) {
        for (key, value) in source {
            target.insert(key, value);
        }
    }
}

/// A control's box on the face (its class, in/outlets and place) and its `valueof`: how Live sees the parameter.
pub(crate) fn face_control(control: &Control, x: f64, y: f64) -> (Value, Value) {
    let mut valueof =
        json!({ "parameter_longname": control.name(), "parameter_shortname": head(control.name(), 12), "parameter_initial_enable": 1 });
    let item = match control {
        Control::Choice(control) => {
            extend(
                &mut valueof,
                json!({ "parameter_type": 2, "parameter_enum": control.options, "parameter_mmax": control.options.len() as i64 - 1, "parameter_initial": [option_index(&control.options, &control.default)] }),
            );
            json!({ "maxclass": "live.menu", "numinlets": 1, "numoutlets": 3, "outlettype": ["", "", "float"], "presentation_rect": [x, y + 24.0, 48.0, 15.0] })
        }
        Control::Switch(control) => {
            extend(
                &mut valueof,
                json!({ "parameter_type": 2, "parameter_enum": ["off", "on"], "parameter_mmax": 1, "parameter_initial": [if control.default { 1 } else { 0 }] }),
            );
            json!({ "maxclass": "live.toggle", "numinlets": 1, "numoutlets": 1, "outlettype": [""], "presentation_rect": [x + 12.0, y + 24.0, 20.0, 20.0] })
        }
        Control::Number(numeric) | Control::Integer(numeric) => {
            let integer = matches!(control, Control::Integer(_));
            let unit = unit_style(numeric, integer);
            extend(
                &mut valueof,
                json!({ "parameter_type": if integer { 1 } else { 0 }, "parameter_mmin": numeric.min, "parameter_mmax": numeric.max, "parameter_initial": [numeric.default], "parameter_unitstyle": unit.style }),
            );
            if let Some(units) = unit.units {
                extend(&mut valueof, json!({ "parameter_units": units }));
            }
            // Frequencies and times spread over decades turn more evenly on a curve.
            if exponential(numeric) {
                extend(&mut valueof, json!({ "parameter_exponent": 3.0 }));
            }
            json!({ "maxclass": "live.dial", "numinlets": 1, "numoutlets": 2, "outlettype": ["", "float"], "presentation_rect": [x, y + 8.0, 44.0, 48.0] })
        }
    };
    (item, valueof)
}

/// Whether a dial turns on a curve: a frequency or a time spread over decades.
fn exponential(control: &NumericControl) -> bool {
    (control.unit == Unit::Hz || control.unit == Unit::Ms) && control.min > 0.0 && control.max / control.min >= 20.0
}

/// Kumi's own knobs: Mix (effects) and Output.
pub static MIX: LazyLock<Control> =
    LazyLock::new(|| Control::Number(NumericControl { name: "Mix".into(), min: 0.0, max: 100.0, default: 100.0, unit: Unit::Percent }));
pub static OUTPUT: LazyLock<Control> =
    LazyLock::new(|| Control::Number(NumericControl { name: "Output".into(), min: -36.0, max: 12.0, default: 0.0, unit: Unit::Db }));

/// What an audio effect's or an instrument's patcher is built from (an instrument's `voices`).
#[derive(Debug, Clone, PartialEq)]
pub struct GenSpec {
    pub name: String,
    pub about: String,
    pub controls: Vec<Control>,
    pub code: String,
    pub voices: Option<f64>,
}

impl From<&AudioEffectSpec> for GenSpec {
    fn from(spec: &AudioEffectSpec) -> GenSpec {
        GenSpec {
            name: spec.name.clone(),
            about: spec.about.clone(),
            controls: spec.controls.clone(),
            code: spec.code.clone(),
            voices: None,
        }
    }
}

impl From<&InstrumentSpec> for GenSpec {
    fn from(spec: &InstrumentSpec) -> GenSpec {
        GenSpec {
            name: spec.name.clone(),
            about: spec.about.clone(),
            controls: spec.controls.clone(),
            code: spec.code.clone(),
            voices: Some(spec.voices as f64),
        }
    }
}

/// An instrument's most voices, each a gen~ of its own.
pub const MAX_VOICES: u32 = 32;

/// The effect's code as its gen~ holds it.
pub fn effect_code(spec: &GenSpec) -> String {
    with_params(&spec.code, &param_lines(&spec.controls), &format!("Made by Kumi: {}", spec.name))
}

/// One voice's code as each voice's gen~ holds it.
pub fn voice_code(spec: &GenSpec) -> String {
    let lines: Vec<String> = VOICE_PARAMS.iter().map(|line| line.to_string()).chain(param_lines(&spec.controls)).collect();
    with_params(&spec.code, &lines, &format!("Made by Kumi: {} (one voice)", spec.name))
}

/// An audio effect's patcher.
pub fn audio_effect_patcher(spec: &GenSpec) -> Value {
    let mut boxes: Vec<PatchBox> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    let mut face = Face::new(spec.controls.len() + 2);
    boxes.push(PatchBox::new(json!({ "id": "obj-plugin", "maxclass": "newobj", "text": "plugin~ 1 2", "numinlets": 1, "numoutlets": 2, "outlettype": ["signal", "signal"], "patching_rect": [40.0, 30.0, 80.0, 22.0] })));
    let effect = gen_box("obj-effect", &effect_code(spec), [40.0, 120.0, 300.0, 22.0]);
    let ins = inputs_read(&spec.code);
    boxes.push(effect);
    boxes.push(gen_box("obj-output", OUTPUT_STAGE_EFFECT, [40.0, 220.0, 300.0, 22.0]));
    boxes.push(PatchBox::new(json!({ "id": "obj-plugout", "maxclass": "newobj", "text": "plugout~ 1 2", "numinlets": 2, "numoutlets": 0, "patching_rect": [40.0, 320.0, 80.0, 22.0] })));
    for channel in [0u32, 1] {
        if (channel as usize) < ins {
            lines.push(Line::new("obj-plugin", channel, "obj-effect", channel));
        }
        lines.push(Line::new("obj-effect", channel, "obj-output", channel));
        lines.push(Line::new("obj-plugin", channel, "obj-output", channel + 2));
        lines.push(Line::new("obj-output", channel, "obj-plugout", channel));
    }
    for control in &spec.controls {
        face.add(control, &param_name(control.name()), &["obj-effect"]);
    }
    face.add(&MIX, "kumi_mix", &["obj-output"]);
    face.add(&OUTPUT, "kumi_output", &["obj-output"]);
    let width = face.width();
    boxes.extend(face.boxes);
    lines.extend(face.lines);
    device_patcher(
        DeviceType::AudioEffect,
        DevicePatcherOptions { title: spec.name.clone(), description: spec.about.clone(), width, boxes, lines },
    )
}

/// An instrument's patcher: `voices` copies of the model's voice, the notes shared out by poly.
pub fn instrument_patcher(spec: &GenSpec) -> Value {
    let voices = round(spec.voices.unwrap_or(8.0)).clamp(1.0, MAX_VOICES as f64) as usize;
    let mut boxes: Vec<PatchBox> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    let mut face = Face::new(spec.controls.len() + 1);
    fn obj(boxes: &mut Vec<PatchBox>, id: &str, text: &str, ins: usize, outs: usize, rect: [f64; 4], outlettype: Option<&[&str]>) {
        let outlettype: Vec<&str> = outlettype.map(|types| types.to_vec()).unwrap_or_else(|| vec![""; outs]);
        boxes.push(PatchBox::new(json!({ "id": id, "maxclass": "newobj", "text": text, "numinlets": ins, "numoutlets": outs, "outlettype": outlettype, "patching_rect": rect })));
    }
    fn wire(lines: &mut Vec<Line>, source: &str, outlet: u32, destination: &str, inlet: u32) {
        lines.push(Line::new(source, outlet, destination, inlet));
    }
    // The track's notes; poly shares them out to voices, taking the oldest when all are busy, and sends voice, pitch, velocity.
    obj(&mut boxes, "obj-notein", "notein", 1, 3, [40.0, 20.0, 60.0, 22.0], Some(&["int", "int", "int"]));
    obj(&mut boxes, "obj-poly", &format!("poly {voices} 1"), 2, 3, [40.0, 60.0, 80.0, 22.0], Some(&["int", "int", "int"]));
    obj(&mut boxes, "obj-pack", "pack 0 0 0", 3, 1, [40.0, 100.0, 80.0, 22.0], None);
    let route = (1..=voices).map(|index| index.to_string()).collect::<Vec<_>>().join(" ");
    obj(&mut boxes, "obj-route", &format!("route {route}"), 1, voices + 1, [40.0, 140.0, 40.0 + voices as f64 * 30.0, 22.0], None);
    wire(&mut lines, "obj-notein", 0, "obj-poly", 0);
    wire(&mut lines, "obj-notein", 1, "obj-poly", 1);
    for outlet in [0u32, 1, 2] {
        wire(&mut lines, "obj-poly", outlet, "obj-pack", outlet);
    }
    wire(&mut lines, "obj-pack", 0, "obj-route", 0);
    // Bend (7-bit, 64 at rest) as ±2 semitones, exactly 0 at rest; the mod wheel as 0–1.
    obj(&mut boxes, "obj-bendin", "bendin", 1, 2, [420.0, 20.0, 50.0, 22.0], Some(&["int", "int"]));
    obj(&mut boxes, "obj-bendcentre", "- 64", 2, 1, [420.0, 50.0, 40.0, 22.0], Some(&["int"]));
    obj(&mut boxes, "obj-bendscale", "/ 32.", 2, 1, [420.0, 80.0, 40.0, 22.0], Some(&["float"]));
    obj(&mut boxes, "obj-bend", "prepend bend", 1, 1, [420.0, 110.0, 90.0, 22.0], None);
    obj(&mut boxes, "obj-ctlin", "ctlin 1", 1, 2, [560.0, 20.0, 50.0, 22.0], Some(&["int", "int"]));
    obj(&mut boxes, "obj-modscale", "/ 127.", 2, 1, [560.0, 50.0, 45.0, 22.0], Some(&["float"]));
    obj(&mut boxes, "obj-mod", "prepend mod_wheel", 1, 1, [560.0, 80.0, 110.0, 22.0], None);
    wire(&mut lines, "obj-bendin", 0, "obj-bendcentre", 0);
    wire(&mut lines, "obj-bendcentre", 0, "obj-bendscale", 0);
    wire(&mut lines, "obj-bendscale", 0, "obj-bend", 0);
    wire(&mut lines, "obj-ctlin", 0, "obj-modscale", 0);
    wire(&mut lines, "obj-modscale", 0, "obj-mod", 0);
    let code = voice_code(spec);
    let mut voice_ids: Vec<String> = Vec::new();
    boxes.push(gen_box("obj-output", OUTPUT_STAGE_INSTRUMENT, [40.0, 500.0, 300.0, 22.0]));
    obj(&mut boxes, "obj-plugout", "plugout~ 1 2", 2, 0, [40.0, 560.0, 80.0, 22.0], Some(&[]));
    for voice in 0..voices {
        let n = voice + 1;
        let id = format!("obj-voice-{n}");
        let x = 40.0 + voice as f64 * 120.0;
        voice_ids.push(id.clone());
        // The voice's note and velocity first, then its strike: trigger sends right to left.
        obj(&mut boxes, &format!("obj-order-{n}"), "t l l", 1, 2, [x, 180.0, 40.0, 22.0], None);
        obj(&mut boxes, &format!("obj-unpack-{n}"), "unpack 0 0", 1, 2, [x, 210.0, 70.0, 22.0], Some(&["int", "int"]));
        obj(&mut boxes, &format!("obj-note-{n}"), "prepend note", 1, 1, [x, 240.0, 90.0, 22.0], None);
        obj(&mut boxes, &format!("obj-velocity-{n}"), "prepend velocity", 1, 1, [x, 270.0, 100.0, 22.0], None);
        // Every note played (a velocity above 0) counts one more strike, from 1: a bare counter's first count
        // is 0, which left strike at 0 and every voice's first note unplayed.
        obj(&mut boxes, &format!("obj-heard-{n}"), "unpack 0 0", 1, 2, [x + 60.0, 280.0, 70.0, 22.0], Some(&["int", "int"]));
        obj(&mut boxes, &format!("obj-played-{n}"), "sel 0", 2, 2, [x + 60.0, 305.0, 40.0, 22.0], Some(&["bang", ""]));
        obj(&mut boxes, &format!("obj-bang-{n}"), "t b", 1, 1, [x + 60.0, 330.0, 30.0, 22.0], Some(&["bang"]));
        obj(
            &mut boxes,
            &format!("obj-count-{n}"),
            "counter 1 1000000",
            5,
            4,
            [x + 60.0, 355.0, 110.0, 22.0],
            Some(&["int", "", "", "int"]),
        );
        obj(&mut boxes, &format!("obj-strike-{n}"), "prepend strike", 1, 1, [x + 60.0, 380.0, 90.0, 22.0], None);
        boxes.push(gen_box(&id, &code, [x, 420.0, 100.0, 22.0]));
        wire(&mut lines, "obj-route", voice as u32, &format!("obj-order-{n}"), 0);
        wire(&mut lines, &format!("obj-order-{n}"), 1, &format!("obj-unpack-{n}"), 0);
        wire(&mut lines, &format!("obj-unpack-{n}"), 0, &format!("obj-note-{n}"), 0);
        wire(&mut lines, &format!("obj-unpack-{n}"), 1, &format!("obj-velocity-{n}"), 0);
        wire(&mut lines, &format!("obj-order-{n}"), 0, &format!("obj-heard-{n}"), 0);
        wire(&mut lines, &format!("obj-heard-{n}"), 1, &format!("obj-played-{n}"), 0);
        wire(&mut lines, &format!("obj-played-{n}"), 1, &format!("obj-bang-{n}"), 0);
        wire(&mut lines, &format!("obj-bang-{n}"), 0, &format!("obj-count-{n}"), 0);
        wire(&mut lines, &format!("obj-count-{n}"), 0, &format!("obj-strike-{n}"), 0);
        wire(&mut lines, &format!("obj-note-{n}"), 0, &id, 0);
        wire(&mut lines, &format!("obj-velocity-{n}"), 0, &id, 0);
        wire(&mut lines, &format!("obj-strike-{n}"), 0, &id, 0);
        wire(&mut lines, "obj-bend", 0, &id, 0);
        wire(&mut lines, "obj-mod", 0, &id, 0);
        // Signals into one inlet add up: every voice into the output stage.
        wire(&mut lines, &id, 0, "obj-output", 0);
        wire(&mut lines, &id, 1, "obj-output", 1);
    }
    wire(&mut lines, "obj-output", 0, "obj-plugout", 0);
    wire(&mut lines, "obj-output", 1, "obj-plugout", 1);
    let targets: Vec<&str> = voice_ids.iter().map(String::as_str).collect();
    for control in &spec.controls {
        face.add(control, &param_name(control.name()), &targets);
    }
    face.add(&OUTPUT, "kumi_output", &["obj-output"]);
    let width = face.width();
    boxes.extend(face.boxes);
    lines.extend(face.lines);
    device_patcher(
        DeviceType::Instrument,
        DevicePatcherOptions { title: spec.name.clone(), description: spec.about.clone(), width, boxes, lines },
    )
}
