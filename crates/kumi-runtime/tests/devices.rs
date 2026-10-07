use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;
use std::time::Instant;

use kumi_common::js::json::stringify;
use kumi_runtime::devices::amxd::{decode_amxd, encode_amxd, DecodedAmxd, DeviceType};
use kumi_runtime::devices::gen::{
    after_functions, audio_effect_patcher, effect_code, inputs_read, instrument_patcher, param_name, voice_code, GenSpec,
};
use kumi_runtime::devices::harness::{check_midi_device, check_midi_device_isolated, Checked, IsolatedOptions};
use kumi_runtime::devices::midi::{midi_device_code, midi_device_patcher};
use kumi_runtime::devices::spec::{check_spec, DeviceSpec, MidiSpec};
use regex::Regex;
use rquickjs::context::EvalOptions;
use rquickjs::prelude::{Coerced, Rest};
use rquickjs::{Context, Function, Runtime, Value as JsValue};
use serde_json::{json, Map, Value};

/// A device a producer might ask for: each chord's lowest note, everything else untouched.
fn lowest() -> Map<String, Value> {
    json!({
        "type": "midi_effect", "name": "Lowest Note", "about": "Keeps the lowest note of each chord (notes within the window); everything else passes through untouched.",
        "controls": [{ "name": "Window", "type": "number", "min": 1, "max": 50, "default": 15, "unit": "ms" }],
        "code": "let pending = [];
let timer = null;
const sounding = new Map();
const key = (event) => event.channel + \":\" + event.pitch;
function flush() {
  timer = null;
  if (!pending.length) return;
  const lowest = pending.reduce((a, b) => (b.pitch < a.pitch ? b : a));
  pending = [];
  send({ type: \"noteon\", pitch: lowest.pitch, velocity: lowest.velocity, channel: lowest.channel });
  sounding.set(key(lowest), lowest);
}
function midi(event) {
  if (event.type === \"noteon\") { pending.push(event); if (!timer) timer = after(params.Window, flush); return; }
  if (event.type === \"noteoff\") {
    if (pending.some((note) => key(note) === key(event))) { cancel(timer); flush(); }
    const note = sounding.get(key(event));
    if (note) { send({ type: \"noteoff\", pitch: note.pitch, channel: note.channel }); sounding.delete(key(event)); }
    return;
  }
  pass(event);
}
function reset() { pending = []; timer = null; sounding.clear(); }",
        "tests": [
            { "name": "a chord keeps its lowest note", "input": [{ "type": "noteon", "pitch": 64, "velocity": 90, "at": 0 }, { "type": "noteon", "pitch": 60, "velocity": 100, "at": 5 }, { "type": "noteon", "pitch": 67, "at": 10 },
                { "type": "noteoff", "pitch": 60, "at": 500 }, { "type": "noteoff", "pitch": 64, "at": 500 }, { "type": "noteoff", "pitch": 67, "at": 500 }],
                "expect": [{ "type": "noteon", "pitch": 60, "velocity": 100, "at": 15 }, { "type": "noteoff", "pitch": 60, "at": 500 }] },
            { "name": "notes apart both play", "input": [{ "type": "noteon", "pitch": 60, "at": 0 }, { "type": "noteon", "pitch": 64, "at": 20 }, { "type": "noteoff", "pitch": 60, "at": 300 }, { "type": "noteoff", "pitch": 64, "at": 320 }],
                "expect": [{ "type": "noteon", "pitch": 60, "at": 15 }, { "type": "noteon", "pitch": 64, "at": 35 }, { "type": "noteoff", "pitch": 60, "at": 300 }, { "type": "noteoff", "pitch": 64, "at": 320 }] },
            { "name": "the rest passes", "input": [{ "type": "cc", "controller": 1, "value": 64, "at": 0 }, { "type": "pitchbend", "value": 9000, "at": 5 }], "expect": [{ "type": "cc", "controller": 1, "value": 64 }, { "type": "pitchbend", "value": 9000 }] },
            { "name": "a wider window", "set": { "Window": 30 }, "input": [{ "type": "noteon", "pitch": 62, "at": 0 }, { "type": "noteon", "pitch": 55, "at": 25 }, { "type": "noteoff", "pitch": 62, "at": 200 }, { "type": "noteoff", "pitch": 55, "at": 200 }],
                "expect": [{ "type": "noteon", "pitch": 55, "at": 30 }, { "type": "noteoff", "pitch": 55, "at": 200 }] },
        ],
    })
    .as_object()
    .unwrap()
    .clone()
}

fn with(base: Map<String, Value>, overrides: Value) -> Map<String, Value> {
    let mut input = base;
    for (key, value) in overrides.as_object().unwrap() {
        input.insert(key.clone(), value.clone());
    }
    input
}

fn spec(overrides: Value) -> MidiSpec {
    let checked = check_spec(&with(lowest(), overrides));
    match checked {
        Ok(DeviceSpec::MidiEffect(spec)) => spec,
        other => panic!("{other:?}"),
    }
}

fn matches(text: &str, pattern: &str) -> bool {
    Regex::new(pattern).unwrap().is_match(text)
}

fn problems_of(checked: Result<DeviceSpec, Vec<String>>) -> Vec<String> {
    match checked {
        Err(problems) => problems,
        Ok(spec) => panic!("accepted: {spec:?}"),
    }
}

fn boxes_of(patcher: &Value) -> Vec<&Map<String, Value>> {
    patcher["patcher"]["boxes"].as_array().unwrap().iter().map(|item| item["box"].as_object().unwrap()).collect()
}

/// The check's own program, for the tests that run a device in a process of its own.
fn use_built_harness() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| std::env::set_var("KUMI_HARNESS_BIN", env!("CARGO_BIN_EXE_kumi-harness")));
}

#[test]
fn a_device_file_is_lives_container_ampf_the_types_letters_meta_and_the_patcher_as_json() {
    let bytes = encode_amxd(DeviceType::MidiEffect, &json!({ "patcher": { "title": "x" } }));
    assert_eq!(&bytes[0..12], b"ampf\x04\x00\x00\x00mmmm");
    assert_eq!(&bytes[12..16], b"meta");
    assert_eq!(bytes.last(), Some(&0), "the patcher ends in a NUL");
    assert_eq!(decode_amxd(&bytes), Some(DecodedAmxd { kind: DeviceType::MidiEffect, patcher: json!({ "patcher": { "title": "x" } }) }));
    assert_eq!(decode_amxd(b"not a device"), None);
    assert_eq!(&encode_amxd(DeviceType::AudioEffect, &json!({}))[8..12], b"aaaa");
}

#[test]
fn a_midi_effects_patch_midiin_the_code_midiout_and_each_control_a_live_parameter_feeding_it() {
    let patcher = midi_device_patcher(&spec(
        json!({ "controls": [{ "name": "Window", "type": "number", "min": 1, "max": 50, "default": 15, "unit": "ms" }, { "name": "Mode", "type": "choice", "options": ["Lowest", "Highest"], "default": "Lowest" }, { "name": "Bypass Drums", "type": "switch", "default": false }] }),
    ));
    let boxes = boxes_of(&patcher);
    let texts: Vec<&str> = boxes.iter().filter(|item| item["maxclass"] == "newobj").map(|item| item["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["midiin", "midiout", "prepend c1", "prepend c2", "prepend c3"]);
    let code = boxes.iter().find(|item| item["maxclass"] == "v8.codebox").unwrap();
    assert!(matches(code["code"].as_str().unwrap(), r"function midi\(event\)"));
    assert!(matches(code["code"].as_str().unwrap(), r#"const CONTROLS = \[\{"id":"c1","name":"Window"\}"#));
    let dial = boxes.iter().find(|item| item["maxclass"] == "live.dial").unwrap();
    assert_eq!(
        stringify(&dial["saved_attribute_attributes"]["valueof"]),
        r#"{"parameter_longname":"Window","parameter_shortname":"Window","parameter_initial_enable":1,"parameter_type":0,"parameter_mmin":1,"parameter_mmax":50,"parameter_initial":[15],"parameter_unitstyle":2,"parameter_exponent":3}"#,
        "a time over decades turns on a curve"
    );
    assert_eq!(boxes.iter().find(|item| item["maxclass"] == "live.menu").unwrap()["varname"], "Mode");
    assert_eq!(boxes.iter().find(|item| item["maxclass"] == "live.toggle").unwrap()["varname"], "Bypass Drums");
    assert_eq!(patcher["patcher"]["openinpresentation"], 1);
    assert_eq!(patcher["patcher"]["project"]["amxdtype"], 0x6d6d6d6d);
    assert_eq!(patcher["patcher"]["description"], lowest()["about"]);
    // Options that read like the frame's placeholders are options: each placeholder is filled once, from the frame.
    let tricky = spec(
        json!({ "controls": [{ "name": "Mode", "type": "choice", "options": ["__CODE__", "__DEFAULTS__"], "default": "__CODE__" }], "code": "function midi(event) { pass(event); }", "tests": [] }),
    );
    let frame = midi_device_code(&tricky.controls, &tricky.code);
    assert!(frame.contains(r#"const CONTROLS = [{"id":"c1","name":"Mode","options":["__CODE__","__DEFAULTS__"]}];"#), "{frame}");
    assert!(frame.contains(r#"const params = {"Mode":"__CODE__"};"#), "{frame}");
    assert_eq!(check_midi_device(&tricky).problems, Vec::<String>::new());
}

#[test]
fn the_devices_code_cant_reach_files_the_network_or_max_and_live_the_frame_hides_them_and_the_check_refuses_them() {
    // Hidden at run time: Max's objects are undefined inside the device's own code, and the frame itself (in Live as
    // here) refuses every way of making code from a string and of reading the frame's functions off a stack.
    let code = midi_device_code(
        &[],
        "function midi(event) {
          const refused = (attempt) => { try { attempt(); return false; } catch (error) { return true; } };
          const blocked = [
            () => (0, eval)('1'),
            () => Reflect.construct(Function, ['return 1']),
            () => (function* () {}).constructor('yield 1'),
            () => (async function () {}).constructor(''),
            () => post['constr' + 'uctor']('return 1'),
            () => { Error.prepareStackTrace = () => 1; },
          ].every(refused);
          const hidden = [typeof File, typeof Dict, typeof LiveAPI, typeof outlet, typeof max, typeof box, typeof include].every((kind) => kind === 'undefined');
          send({ type: 'cc', controller: 1, value: blocked && hidden ? 1 : 0 });
        }",
    );
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    let sent: Vec<f64> = context.with(|ctx| {
        let sent: Rc<RefCell<Vec<f64>>> = Rc::new(RefCell::new(Vec::new()));
        let globals = ctx.globals();
        globals
            .set(
                "outlet",
                Function::new(ctx.clone(), {
                    let sent = sent.clone();
                    move |_index: JsValue, byte: Coerced<f64>| sent.borrow_mut().push(byte.0)
                })
                .unwrap(),
            )
            .unwrap();
        globals.set("post", Function::new(ctx.clone(), |_args: Rest<JsValue>| {}).unwrap()).unwrap();
        ctx.eval::<(), _>("class Task {}; class File {}; class Dict {}; class LiveAPI {}; var max = {}; var box = { patcher: {} }; function include() {} var inlet = 0;")
            .unwrap();
        let mut options = EvalOptions::default();
        options.strict = false;
        ctx.eval_with_options::<(), _>(code, options).unwrap();
        for byte in [0xB0, 7, 100] {
            globals.get::<_, Function>("msg_int").unwrap().call::<_, ()>((byte,)).unwrap();
        }
        let sent = sent.borrow().clone();
        sent
    });
    assert_eq!(sent, [0xB0 as f64, 1.0, 1.0]);
    // Refused up front, saying why.
    let refused = problems_of(check_spec(&with(
        lowest(),
        json!({ "code": "function midi(e) { const f = new File('/tmp/x'); XMLHttpRequest; outlet(0, 1); new Task(() => {}); eval('1'); }" }),
    )));
    assert_eq!(refused.len(), 3, "one line for each kind of reach");
    assert!(matches(&refused.join(" "), "files, the network"));
    assert!(matches(&refused.join(" "), "can't make code"));
    // Ordinary names are fine: Math.max, a helper called parse, a class.
    assert!(check_spec(&with(lowest(), json!({ "code": "class Voice { constructor(p) { this.p = p; } }\nconst parse = (x) => Math.max(0, x);\nfunction midi(event) { pass(event); }" }))).is_ok());
}

#[test]
fn a_spec_says_whats_wrong_with_it_each_so_it_can_be_fixed() {
    assert!(matches(
        &problems_of(check_spec(&with(lowest(), json!({ "type": "reverb" })))).join(" "),
        "midi_effect, audio_effect or instrument"
    ));
    let checked = check_spec(
        json!({ "type": "midi_effect", "name": "", "about": "", "controls": [
            { "name": "Window", "type": "number", "min": 5, "max": 1, "default": 3 }, { "name": "Window", "type": "switch", "default": true }, { "name": "Mode", "type": "choice", "options": ["A"], "default": "A" },
            { "name": "Level", "type": "integer", "min": 0, "max": 10, "default": 2.5 }, { "name": "Rate", "type": "number", "min": 0, "max": 1, "default": 0.5, "unit": "furlongs" }], "code": "send(1)", "tests": [{ "name": "x", "input": "no" }] })
        .as_object()
        .unwrap(),
    );
    let text = problems_of(checked).join("\n");
    for expected in [
        r"(?m)^name:",
        r"(?m)^about:",
        "min is below max",
        "used twice",
        "2–128 options",
        "whole-number",
        "unit is one of",
        "define function midi",
        r"(?m)^tests\[0\]",
    ] {
        assert!(matches(&text, expected), "{expected}: {text}");
    }
}

#[test]
fn kumi_runs_the_devices_tests_and_its_own_checks_a_working_device_passes_a_broken_one_is_told_what_went_wrong() {
    assert_eq!(check_midi_device(&spec(json!({}))), Checked { passed: 4, of: 4, problems: vec![] });
    let wrong_test = check_midi_device(&spec(
        json!({ "tests": [{ "name": "wrong", "input": [{ "type": "noteon", "pitch": 60, "at": 0 }, { "type": "noteoff", "pitch": 60, "at": 100 }], "expect": [{ "type": "noteon", "pitch": 61 }] }] }),
    ));
    assert!(
        matches(
            &wrong_test.problems[0],
            r#"^test "wrong": expected noteon pitch 61; got noteon pitch 60 velocity 100 at 15 ms, noteoff pitch 60 at 100 ms\.$"#
        ),
        "{:?}",
        wrong_test.problems
    );
    let hanging =
        check_midi_device(&spec(json!({ "code": "function midi(event) { if (event.type !== 'noteoff') send(event); }", "tests": [] })));
    assert!(matches(&hanging.problems.join(" "), r"leaves notes hanging \(pitch 60, 64, 67, 72\)"), "{:?}", hanging.problems);
    let throwing = check_midi_device(&spec(
        json!({ "code": "function midi(event) { if (event.type === 'cc') missing(); pass(event); }", "tests": [] }),
    ));
    assert!(matches(&throwing.problems.join(" "), "it threw: missing is not defined"), "{:?}", throwing.problems);
    let running = check_midi_device(&spec(
        json!({ "code": "function tick() { after(100, tick); }\ntick();\nfunction midi(event) { pass(event); }", "tests": [] }),
    ));
    assert!(matches(&running.problems.join(" "), "timers keep running"), "{:?}", running.problems);
    // A note-on of velocity 0 in a test goes to the device as one, which the frame hands on as a note-off.
    let zero = check_midi_device(&spec(
        json!({ "tests": [{ "name": "velocity 0 releases", "input": [{ "type": "noteon", "pitch": 60, "velocity": 100, "at": 0 }, { "type": "noteon", "pitch": 60, "velocity": 0, "at": 90 }],
        "expect": [{ "type": "noteon", "pitch": 60, "at": 15 }, { "type": "noteoff", "pitch": 60, "at": 90 }] }] }),
    ));
    assert_eq!(zero.problems, Vec::<String>::new());
    let broken = check_midi_device(&spec(json!({ "code": "function midi(event) { pass(event) ", "tests": [] })));
    assert!(matches(&broken.problems.join(" "), "the code doesn't run"), "{:?}", broken.problems);
    // The time a device reads is today's, as in Live: code that acts on the date is checked as it will run there.
    let dated = check_midi_device(&spec(json!({ "code": "function midi(event) { if (Date.now() > 1e12) pass(event); }", "tests": [
        { "name": "passes", "input": [{ "type": "noteon", "pitch": 60, "at": 0 }, { "type": "noteoff", "pitch": 60, "at": 100 }], "expect": [{ "type": "noteon", "pitch": 60 }, { "type": "noteoff", "pitch": 60 }] }] })));
    assert_eq!(dated.problems, Vec::<String>::new());
}

// ported with devices/tool.rs: "make_device reads its guide on demand, makes a device where Live's Browser sees it, and waits for the Browser"

#[tokio::test]
async fn a_devices_code_is_checked_in_a_process_of_its_own_it_cant_reach_kumi_cant_make_code_from_strings_and_cant_hang_kumi() {
    use_built_harness();
    // The same verdicts as in-process.
    assert_eq!(
        check_midi_device_isolated(&spec(json!({})), IsolatedOptions::default()).await,
        Checked { passed: 4, of: 4, problems: vec![] }
    );
    // An escape through a host function's constructor can't compile anything.
    let escape = check_midi_device_isolated(
        &spec(json!({ "code": "function midi(event) { const F = post['constr' + 'uctor']; F('return process')().exit(3); pass(event); }", "tests": [] })),
        IsolatedOptions::default(),
    )
    .await;
    assert!(matches(&escape.problems.join(" "), "it threw: .*[Cc]ode generation from strings disallowed"), "{:?}", escape.problems);
    // A loop that never ends is stopped at the deadline, and said.
    let started = Instant::now();
    let endless = check_midi_device_isolated(
        &spec(json!({ "code": "function midi(event) { while (true) {} }", "tests": [] })),
        IsolatedOptions { timeout_ms: Some(1_500) },
    )
    .await;
    assert!(matches(&endless.problems.join(" "), "didn't finish within 2 s; something loops forever"), "{:?}", endless.problems);
    assert!(started.elapsed().as_millis() < 5_000);
}

/// An audio effect a producer might ask for: a saturator with a tone control, its own function first.
fn grit() -> Map<String, Value> {
    json!({
        "type": "audio_effect", "name": "Grit", "about": "Warm saturation with a tone control.",
        "controls": [{ "name": "Drive", "type": "number", "min": 0, "max": 24, "default": 6, "unit": "dB" }, { "name": "Tone", "type": "number", "min": 0, "max": 1, "default": 0.5 }, { "name": "Hard", "type": "switch", "default": false }],
        "code": "// Soft or hard clipping.
shaper(x, hard_clip) {
  return hard_clip > 0.5 ? clamp(x, -1, 1) : tanh(x);
}
History lp_l(0), lp_r(0);
g = dbtoa(drive);
lp_l = mix(lp_l, shaper(in1 * g, hard), 0.05 + tone * 0.9);
lp_r = mix(lp_r, shaper(in2 * g, hard), 0.05 + tone * 0.9);
out1 = lp_l / g;
out2 = lp_r / g;",
    })
    .as_object()
    .unwrap()
    .clone()
}

/// A plucked voice for an instrument.
fn pluck() -> Map<String, Value> {
    json!({
        "type": "instrument", "name": "Pluck", "about": "A plucked saw.", "voices": 6,
        "controls": [{ "name": "Decay", "type": "number", "min": 0.05, "max": 4, "default": 0.6, "unit": "s" }],
        "code": "History env(0);
env = change(strike) != 0 ? velocity / 127 : env * exp(-1 / (decay * samplerate));
osc = phasor(mtof(note + bend)) * 2 - 1;
out1 = osc * env * 0.2;
out2 = out1;",
    })
    .as_object()
    .unwrap()
    .clone()
}

fn gen_spec(input: &Map<String, Value>) -> GenSpec {
    match check_spec(input) {
        Ok(DeviceSpec::AudioEffect(spec)) => GenSpec::from(&spec),
        Ok(DeviceSpec::Instrument(spec)) => GenSpec::from(&spec),
        other => panic!("{other:?}"),
    }
}

fn inner<'a>(patcher: &'a Value, id: &str) -> &'a Map<String, Value> {
    boxes_of(patcher).into_iter().find(|item| item["id"] == id).unwrap_or_else(|| panic!("{id}"))
}

fn code_of(patcher: &Value, id: &str) -> String {
    inner(patcher, id)["patcher"]["boxes"].as_array().unwrap().iter().find(|item| item["box"]["maxclass"] == "codebox").unwrap()["box"]
        ["code"]
        .as_str()
        .unwrap()
        .to_string()
}

fn wires(patcher: &Value) -> Vec<String> {
    patcher["patcher"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| {
            let line = &line["patchline"];
            format!(
                "{}:{}>{}:{}",
                line["source"][0].as_str().unwrap(),
                line["source"][1],
                line["destination"][0].as_str().unwrap(),
                line["destination"][1]
            )
        })
        .collect()
}

#[test]
fn genexprs_order_holds_the_models_functions_first_then_kumis_params_for_the_controls_then_the_rest_inputs_as_the_code_reads_them() {
    assert_eq!(param_name("Decay Time"), "decay_time");
    assert_eq!(param_name(" Hi-Cut 2 "), "hi_cut_2");
    let code = effect_code(&gen_spec(&grit()));
    let shaper = code.find("shaper(x, hard_clip)");
    let params = code.find("Param drive(6, min=0, max=24);");
    let history = code.find("History lp_l");
    assert!(shaper.is_some() && params > shaper && history > params, "{code}");
    assert!(matches(&code, r"Param tone\(0\.5, min=0, max=1\);\nParam hard\(0, min=0, max=1\);"));
    assert_eq!(after_functions("out1 = in1; out2 = in2;"), 0, "no functions: Params go first");
    assert_eq!(after_functions("foo(1);\nout1 = in1;"), 0, "a call isn't a definition");
    assert_eq!(inputs_read(grit()["code"].as_str().unwrap()), 2);
    assert_eq!(inputs_read("out1 = in1; out2 = in1; // in2 in a comment"), 1);
    assert_eq!(inputs_read(pluck()["code"].as_str().unwrap()), 0);
    let voice = voice_code(&gen_spec(&pluck()));
    assert!(
        matches(
            &voice,
            r"Param note\(60, min=0, max=127\);[\s\S]*Param strike\(0\);[\s\S]*Param decay\(0\.6, min=0\.05, max=4\);\n\nHistory env\(0\);"
        ),
        "{voice}"
    );
}

#[test]
fn an_audio_effect_plugin_into_the_models_gen_kumis_fixed_output_stage_with_mix_and_output_plugout_each_control_a_live_parameter() {
    let checked = check_spec(&grit());
    assert!(matches!(checked, Ok(DeviceSpec::AudioEffect(_))), "{checked:?}");
    let patcher = audio_effect_patcher(&gen_spec(&grit()));
    let effect = inner(&patcher, "obj-effect");
    assert_eq!(effect["text"], "gen~");
    assert_eq!(effect["patcher"]["classnamespace"], "dsp.gen");
    assert_eq!(effect["numinlets"], 2);
    assert!(code_of(&patcher, "obj-effect").contains("\r\n"), "Max's line endings");
    let stage = code_of(&patcher, "obj-output");
    for line in ["Param kumi_mix(100", "Param kumi_output(0", "clamp(dcblock(fixnan(fixdenorm(in1))), -2, 2)"] {
        assert!(stage.contains(line), "{line}");
    }
    // The dry signal passes untouched: a hot track isn't clipped by a Kumi effect at Mix 0.
    assert!(matches(&stage, r"out1 = mix\(in3, clamp\(dcblock\(fixnan\(fixdenorm\(in1\)\)\), -2, 2\), wet\) \* gain;"));
    let lines = wires(&patcher);
    for expected in [
        "obj-plugin:0>obj-effect:0",
        "obj-plugin:1>obj-effect:1",
        "obj-effect:0>obj-output:0",
        "obj-plugin:0>obj-output:2",
        "obj-plugin:1>obj-output:3",
        "obj-output:1>obj-plugout:1",
    ] {
        assert!(lines.contains(&expected.to_string()), "{expected}");
    }
    let faces: Vec<&str> = boxes_of(&patcher)
        .into_iter()
        .filter(|item| item.get("parameter_enable") == Some(&json!(1)))
        .map(|item| item["saved_attribute_attributes"]["valueof"]["parameter_longname"].as_str().unwrap())
        .collect();
    assert_eq!(faces, ["Drive", "Tone", "Hard", "Mix", "Output"]);
    assert!(lines.contains(&"obj-control-1:0>obj-prepend-1:0".to_string()));
    assert_eq!(inner(&patcher, "obj-prepend-1")["text"], "prepend drive");
    assert_eq!(inner(&patcher, "obj-prepend-4")["text"], "prepend kumi_mix");
    let mono = audio_effect_patcher(&GenSpec { controls: vec![], code: "out1 = tanh(in1); out2 = out1;".into(), ..gen_spec(&grit()) });
    assert_eq!(inner(&mono, "obj-effect")["numinlets"], 1);
    assert!(!wires(&mono).contains(&"obj-plugin:1>obj-effect:1".to_string()), "a code that reads only in1 gets only in1");
    assert_eq!(decode_amxd(&encode_amxd(DeviceType::AudioEffect, &patcher)).map(|decoded| decoded.kind), Some(DeviceType::AudioEffect));
}

#[test]
fn an_instrument_notes_shared_out_by_poly_to_one_gen_per_voice_bend_exactly_0_at_rest_a_strike_per_note_every_voice_into_kumis_output_stage(
) {
    let checked = check_spec(&pluck());
    assert!(matches!(&checked, Ok(DeviceSpec::Instrument(spec)) if spec.voices == 6), "{checked:?}");
    let patcher = instrument_patcher(&gen_spec(&pluck()));
    assert_eq!(inner(&patcher, "obj-poly")["text"], "poly 6 1");
    assert_eq!(inner(&patcher, "obj-route")["text"], "route 1 2 3 4 5 6");
    assert_eq!(inner(&patcher, "obj-bendcentre")["text"], "- 64");
    assert_eq!(inner(&patcher, "obj-bendscale")["text"], "/ 32.");
    let voices = boxes_of(&patcher).into_iter().filter(|item| item["id"].as_str().unwrap().starts_with("obj-voice-")).count();
    assert_eq!(voices, 6);
    assert_eq!(inner(&patcher, "obj-voice-1")["numinlets"], 1, "no audio input, one inlet for its Params");
    let lines = wires(&patcher);
    for voice in 1..=6 {
        for expected in [
            format!("obj-route:{}>obj-order-{voice}:0", voice - 1),
            format!("obj-order-{voice}:1>obj-unpack-{voice}:0"),
            format!("obj-order-{voice}:0>obj-heard-{voice}:0"),
            format!("obj-note-{voice}:0>obj-voice-{voice}:0"),
            format!("obj-strike-{voice}:0>obj-voice-{voice}:0"),
            format!("obj-voice-{voice}:0>obj-output:0"),
            format!("obj-voice-{voice}:1>obj-output:1"),
            format!("obj-prepend-1:0>obj-voice-{voice}:0"),
        ] {
            assert!(lines.contains(&expected), "{expected}");
        }
    }
    assert!(
        lines.contains(&"obj-heard-1:1>obj-played-1:0".to_string()) && lines.contains(&"obj-played-1:1>obj-bang-1:0".to_string()),
        "a strike counts only notes played (velocity above 0)"
    );
    // On real Live, a bare counter (first count 0) left strike at 0, so no voice played its first note.
    assert_eq!(inner(&patcher, "obj-count-1")["text"], "counter 1 1000000", "a voice's first note changes strike too");
    assert!(matches(&code_of(&patcher, "obj-output"), "Param kumi_output"));
    assert!(!matches(&code_of(&patcher, "obj-output"), "kumi_mix"), "an instrument has no dry signal to mix");
    assert_eq!(decode_amxd(&encode_amxd(DeviceType::Instrument, &patcher)).map(|decoded| decoded.kind), Some(DeviceType::Instrument));
}

#[test]
fn kumis_checks_for_an_audio_effect_or_an_instrument_say_whats_wrong_each_so_it_can_be_fixed() {
    let problems = |input: Map<String, Value>| match check_spec(&input) {
        Err(problems) => problems.join("\n"),
        Ok(_) => String::new(),
    };
    assert!(matches(
        &problems(with(grit(), json!({ "controls": [{ "name": "Mix", "type": "number", "min": 0, "max": 1, "default": 1 }] }))),
        "Kumi adds Mix"
    ));
    assert!(matches(
        &problems(with(grit(), json!({ "controls": [{ "name": "Delay", "type": "number", "min": 0, "max": 1, "default": 1 }] }))),
        "Param delay, a name gen~ or Kumi already uses; call it something else"
    ));
    let knobs: Vec<Value> = (0..129)
        .map(|index| json!({ "name": format!("Knob {}", index + 1), "type": "number", "min": 0, "max": 1, "default": 0 }))
        .collect();
    assert!(matches(&problems(with(grit(), json!({ "controls": knobs }))), r"controls: at most 128\."));
    assert!(matches(
        &problems(with(
            grit(),
            json!({ "controls": [{ "name": "Pre-Delay", "type": "number", "min": 0, "max": 1, "default": 0 }, { "name": "Pre Delay", "type": "number", "min": 0, "max": 1, "default": 0 }] })
        )),
        r#""Pre Delay" and "Pre-Delay" would both be the Param pre_delay"#
    ));
    assert!(matches(&problems(with(grit(), json!({ "code": "out1 = in1;" }))), r"assign out1 \(left\) and out2 \(right\)"));
    assert!(matches(&problems(with(grit(), json!({ "code": "out1 = in3; out2 = in1;" }))), "two inputs"));
    assert!(matches(&problems(with(grit(), json!({ "code": "out1 = cycle(440); out2 = out1;" }))), "reads its input"));
    assert!(matches(&problems(with(grit(), json!({ "code": "out1 = in1 * (velocity / 127); out2 = in2;" }))), "gets no notes"));
    assert!(matches(&problems(with(grit(), json!({ "code": "out1 = tanh(in1; out2 = in2;" }))), "don't pair up"));
    assert!(matches(&problems(with(pluck(), json!({ "code": "out1 = in1; out2 = in2;" }))), "no audio input"));
    assert!(matches(&problems(with(pluck(), json!({ "code": "out1 = cycle(440); out2 = out1;" }))), "plays the note it's given"));
    assert!(matches(&problems(with(pluck(), json!({ "voices": 33 }))), r"1 \(mono\) to 32"));
    assert!(matches(
        &problems(with(pluck(), json!({ "code": "Param note(60);\nout1 = cycle(mtof(note)); out2 = out1;" }))),
        "Kumi's; use them without declaring"
    ));
    // A control's Param declared by the model too (as gen~ code usually is) would be declared twice.
    assert!(matches(
        &problems(with(grit(), json!({ "code": "Param drive(1, min=1, max=20);\nout1 = tanh(in1 * drive); out2 = tanh(in2 * drive);" }))),
        "drive is the Drive control's Param, which Kumi declares"
    ));
    assert_eq!(
        problems(with(grit(), json!({ "tests": [{ "name": "ignored", "input": [], "expect": [] }] }))),
        "",
        "an audio effect isn't tested with MIDI; its tests are left out"
    );
}

#[test]
fn a_device_gets_what_it_needs_many_controls_in_rows_on_the_face_names_with_punctuation_long_menus_more_voices() {
    let knobs: Vec<Value> = (0..20)
        .map(|index| json!({ "name": format!("Size/Decay-{}", index + 1), "type": "number", "min": 0, "max": 1, "default": 0.5 }))
        .collect();
    let checked = check_spec(&with(grit(), json!({ "controls": knobs })));
    assert!(matches!(&checked, Ok(DeviceSpec::AudioEffect(spec)) if spec.controls.len() == 20), "{checked:?}");
    let patcher = audio_effect_patcher(&gen_spec(&with(
        grit(),
        json!({ "controls": (0..20).map(|index| json!({ "name": format!("Size/Decay-{}", index + 1), "type": "number", "min": 0, "max": 1, "default": 0.5 })).collect::<Vec<_>>() }),
    )));
    let faces: Vec<Vec<f64>> = boxes_of(&patcher)
        .into_iter()
        .filter(|item| item.get("parameter_enable") == Some(&json!(1)))
        .map(|item| item["presentation_rect"].as_array().unwrap().iter().map(|value| value.as_f64().unwrap()).collect())
        .collect();
    assert_eq!(faces.len(), 22, "20 of the model's, Mix and Output");
    // 22 controls: three rows of 8, all inside Live's 169-pixel device view, and the face as wide as its rows.
    let mut rows: Vec<f64> = Vec::new();
    for rect in &faces {
        if !rows.contains(&rect[1]) {
            rows.push(rect[1]);
        }
    }
    assert_eq!(rows, [8.0, 60.0, 112.0]);
    assert!(faces.iter().all(|rect| rect[1] + rect[3] <= 169.0));
    assert_eq!(patcher["patcher"]["devicewidth"], (16 + 8 * 52) as f64);
    // Up to eight stay in one row, as before.
    let midi = midi_device_patcher(&spec(json!({})));
    assert_eq!(midi["patcher"]["devicewidth"], 120.0);
    let options: Vec<String> = (0..40).map(|index| format!("Mode {}", index + 1)).collect();
    let menu = check_spec(&with(
        lowest(),
        json!({ "controls": [{ "name": "Scale", "type": "choice", "options": options, "default": "Mode 1" }] }),
    ));
    assert!(menu.is_ok(), "{menu:?}");
    let wide = check_spec(&with(pluck(), json!({ "voices": 16 })));
    assert!(matches!(&wide, Ok(DeviceSpec::Instrument(spec)) if spec.voices == 16));
    assert_eq!(inner(&instrument_patcher(&gen_spec(&with(pluck(), json!({ "voices": 16 })))), "obj-poly")["text"], "poly 16 1");
}

#[tokio::test]
async fn a_midi_effect_that_runs_free_an_lfo_a_clock_passes_with_runs_free_without_it_kumi_asks_for_it_or_for_its_timers_to_stop() {
    use_built_harness();
    let lfo = with(
        lowest(),
        json!({ "name": "CC LFO", "controls": [{ "name": "Rate", "type": "number", "min": 10, "max": 1000, "default": 50, "unit": "ms" }], "tests": [],
        "code": "let phase = 0;\nfunction tick() { phase = (phase + 1) % 32; send({ type: 'cc', controller: 74, value: Math.abs(16 - phase) * 8 }); after(params.Rate, tick); }\ntick();\nfunction midi(event) { pass(event); }\nfunction reset() { tick(); }" }),
    );
    let held = check_midi_device(&spec(Value::Object(lfo.clone())));
    assert!(matches(&held.problems.join(" "), r"timers keep running[\s\S]*runs_free: true"), "{:?}", held.problems);
    let free = check_spec(&with(lfo.clone(), json!({ "runs_free": true })));
    let Ok(DeviceSpec::MidiEffect(free)) = free else { panic!("{free:?}") };
    assert!(free.runs_free);
    assert_eq!(check_midi_device_isolated(&free, IsolatedOptions::default()).await, Checked { passed: 0, of: 0, problems: vec![] });
    // Running free isn't sending without end.
    let flood = check_midi_device(&MidiSpec {
        code: "function tick() { for (let i = 0; i < 50; i++) send({ type: 'cc', controller: 1, value: i }); after(1, tick); }\ntick();\nfunction midi(event) { pass(event); }".into(),
        ..free.clone()
    });
    assert!(matches(&flood.problems.join(" "), "something sends without end"), "{:?}", flood.problems);
    assert!(matches(&problems_of(check_spec(&with(lfo, json!({ "runs_free": "yes" })))).join(" "), "runs_free: true for a MIDI effect"));
}

// ported with devices/tool.rs: "make_device makes an audio effect and an instrument, with Kumi's own knobs, and says to hear them"

#[tokio::test(flavor = "current_thread")]
async fn make_device_reads_guide_makes_a_device_and_waits_for_live_browser() {
    use futures::FutureExt;
    use kumi_common::abort::Signal;
    use kumi_runtime::devices::tool::{device_tool, DeviceToolOptions};
    use std::cell::Cell;
    use_built_harness();
    let folder = tempfile::tempdir().unwrap();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let calls = Rc::new(Cell::new(0));
    let tool = device_tool(DeviceToolOptions {
        user_library: folder.path().to_string_lossy().into(),
        wait_ms: Some(5_000),
        browser_sees: Rc::new({
            let seen = seen.clone();
            move |item, _| {
                seen.borrow_mut().push(item);
                calls.set(calls.get() + 1);
                let listed = calls.get() > 1;
                async move { Ok(listed) }.boxed_local()
            }
        }),
    });
    let guide = tool.execute(json!({"guide":true}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
    assert!(guide.text.starts_with("Making a MIDI effect"));
    assert!(guide.text.contains("Every note-on the device sends gets a note-off"));
    let made = tool.execute(lowest(), Signal::new()).await.unwrap();
    assert!(!made.is_error, "{}", made.text);
    let result: Value = serde_json::from_str(&made.text).unwrap();
    assert_eq!(result["itemId"], "user_library/Kumi/Lowest Note");
    assert_eq!(result["file"], folder.path().join("Kumi").join("Lowest Note.amxd").to_string_lossy().as_ref());
    assert_eq!(result["controls"], json!(["Window (1–50 ms; 15)"]));
    assert!(result["checks"].as_str().unwrap().contains("4 of 4"));
    assert!(result.get("note").is_none());
    assert_eq!(*seen.borrow(), ["user_library/Kumi/Lowest Note", "user_library/Kumi/Lowest Note"]);
    let decoded = decode_amxd(&std::fs::read(folder.path().join("Kumi/Lowest Note.amxd")).unwrap()).unwrap();
    assert_eq!(decoded.kind, DeviceType::MidiEffect);
    assert_eq!(decoded.patcher["patcher"]["title"], "Lowest Note");
    let again: Value = serde_json::from_str(&tool.execute(lowest(), Signal::new()).await.unwrap().text).unwrap();
    assert_eq!(again["itemId"], "user_library/Kumi/Lowest Note 2");
    let refused = tool
        .execute(
            with(
                lowest(),
                json!({"name":"Broken","code":"function midi(event) { if (event.type !== 'noteoff') send(event); }","tests":[]}),
            ),
            Signal::new(),
        )
        .await
        .unwrap();
    assert!(refused.is_error);
    assert!(refused.text.contains("leaves notes hanging"));
    assert!(!folder.path().join("Kumi/Broken.amxd").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn make_device_makes_audio_effect_and_instrument_with_kumis_knobs() {
    use futures::FutureExt;
    use kumi_common::abort::Signal;
    use kumi_runtime::devices::tool::{device_tool, DeviceToolOptions};
    let folder = tempfile::tempdir().unwrap();
    let tool = device_tool(DeviceToolOptions {
        user_library: folder.path().to_string_lossy().into(),
        wait_ms: Some(1_000),
        browser_sees: Rc::new(|_, _| async { Ok(true) }.boxed_local()),
    });
    for (kind, start, phrase) in [
        ("audio_effect", "Making an audio effect", "Max compiles the code when Live loads"),
        ("instrument", "Making an instrument", "change(strike) != 0"),
    ] {
        let guide = tool.execute(json!({"guide":true,"type":kind}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
        assert!(guide.text.starts_with(start));
        assert!(guide.text.contains(phrase));
    }
    let effect: Value = serde_json::from_str(&tool.execute(grit(), Signal::new()).await.unwrap().text).unwrap();
    assert_eq!(effect["type"], "audio effect");
    assert_eq!(effect["itemId"], "user_library/Kumi/Grit");
    assert_eq!(
        effect["controls"],
        json!(["Drive (0–24 dB; 6)", "Tone (0–1; 0.5)", "Hard (on/off; off)", "Mix (0–100 %; 100)", "Output (-36–12 dB; 0)"])
    );
    assert!(effect["next"].as_str().unwrap().contains("audition"));
    assert_eq!(decode_amxd(&std::fs::read(folder.path().join("Kumi/Grit.amxd")).unwrap()).unwrap().kind, DeviceType::AudioEffect);
    let instrument: Value = serde_json::from_str(&tool.execute(pluck(), Signal::new()).await.unwrap().text).unwrap();
    assert_eq!(instrument["type"], "instrument");
    assert_eq!(instrument["voices"], 6);
    assert_eq!(decode_amxd(&std::fs::read(folder.path().join("Kumi/Pluck.amxd")).unwrap()).unwrap().kind, DeviceType::Instrument);
}
