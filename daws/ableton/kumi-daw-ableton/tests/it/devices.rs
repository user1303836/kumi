use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;
use std::time::Instant;

use kumi_common::js::json::stringify;
use kumi_daw_ableton::devices::amxd::{decode_amxd, encode_amxd, DecodedAmxd, DeviceType};
use kumi_daw_ableton::devices::findings::Findings;
use kumi_daw_ableton::devices::gen::{
    after_functions, audio_effect_patcher, effect_code, inputs_read, instrument_patcher, param_name, voice_code, GenSpec,
};
use kumi_daw_ableton::devices::harness::{check_midi_device, check_midi_device_isolated, Checked, IsolatedOptions};
use kumi_daw_ableton::devices::midi::{midi_device_code, midi_device_patcher};
use kumi_daw_ableton::devices::spec::{check_spec, kept_unrun, DeviceSpec, MidiSpec};
use kumi_daw_ableton::devices::tool::{device_tool, kind_folder, DeviceToolOptions, Made};
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

/// A MIDI effect made from the overrides, which leave nothing to say.
fn spec(overrides: Value) -> MidiSpec {
    match check_spec(&with(lowest(), overrides)) {
        (DeviceSpec::MidiEffect(spec), found) if found.is_empty() => spec,
        other => panic!("{other:?}"),
    }
}

fn matches(text: &str, pattern: &str) -> bool {
    Regex::new(pattern).unwrap().is_match(text)
}

/// What's said beside a device made from a spec: what's wrong, then what Kumi changed, then what's worth a look.
fn findings_of(checked: (DeviceSpec, Findings)) -> Vec<String> {
    checked.1.all().cloned().collect()
}

fn boxes_of(patcher: &Value) -> Vec<&Map<String, Value>> {
    patcher["patcher"]["boxes"].as_array().unwrap().iter().map(|item| item["box"].as_object().unwrap()).collect()
}

/// The check's own program, for the tests that run a device in a process of its own.
fn use_built_harness() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| std::env::set_var("KUMI_HARNESS_BIN", env!("CARGO_BIN_EXE_kumi-harness")));
}

/// Kumi's folder for the tests that make devices with the tool: what's learned of this machine's Max is kept there, one
/// folder under the build's own for every run, not in the developer's own.
fn use_own_kumi_folder() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| std::env::set_var("KUMI_HOME", std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("devices-kumi-home")));
}

/// The tool, making devices in `library` for a conversation of its own.
fn tool_in(library: &std::path::Path, made: Rc<RefCell<Made>>) -> Rc<dyn kumi_types::contracts::KernelTool> {
    use futures::FutureExt;
    use_own_kumi_folder();
    device_tool(DeviceToolOptions {
        user_library: library.to_string_lossy().into(),
        wait_ms: Some(0),
        browser_sees: Rc::new(|_, _| async { Ok(true) }.boxed_local()),
        made,
    })
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
    // A switch is a button with its name, lit while it's on.
    let button = boxes.iter().find(|item| item["maxclass"] == "live.text").unwrap();
    assert_eq!((&button["varname"], &button["text"], &button["mode"]), (&json!("Bypass Drums"), &json!("Bypass Drums"), &json!(1)));
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
    // Said up front, beside the device, which is made with the code as written: the frame hides what it reaches for.
    let reaching = "function midi(e) { const f = new File('/tmp/x'); XMLHttpRequest; outlet(0, 1); new Task(() => {}); eval('1'); }";
    let (made, found) = check_spec(&with(lowest(), json!({ "code": reaching })));
    assert!(matches!(&made, DeviceSpec::MidiEffect(spec) if spec.code == reaching));
    assert_eq!(found.wrong.len(), 3, "one line for each kind of reach");
    assert!(matches(&found.wrong.join(" "), "files, the network"));
    assert!(matches(&found.wrong.join(" "), "can't make code"));
    // Ordinary names are fine: Math.max, a helper called parse, a class.
    assert!(findings_of(check_spec(&with(lowest(), json!({ "code": "class Voice { constructor(p) { this.p = p; } }\nconst parse = (x) => Math.max(0, x);\nfunction midi(event) { pass(event); }" })))).is_empty());
    // The code is one function's body, as the frame needs. Code that closes the device's function early, or leaves a
    // brace open, whatever names it uses, and HTML-like comments, which Live reads as comments, could run outside what
    // the frame hides: the device is made with the code kept in it unrun, and it passes MIDI on as it comes.
    let unrun = |code: &str| {
        let (made, found) = check_spec(&with(lowest(), json!({ "code": code })));
        let DeviceSpec::MidiEffect(made) = made else { panic!("{made:?}") };
        let (head, kept) = made.code.split_once('\n').unwrap();
        assert!(head.starts_with("// Kumi kept this code without running it: "), "{}", made.code);
        assert_eq!(kept.lines().map(|line| line.strip_prefix("// ").unwrap()).collect::<Vec<_>>().join("\n"), code, "each line a comment");
        assert!(made.tests.is_empty(), "the code isn't run, nor its tests");
        assert_eq!(
            check_midi_device(&made).problems,
            Vec::<String>::new(),
            "the frame reads it, and the device passes MIDI on as it comes"
        );
        found.wrong.join(" ")
    };
    assert!(matches(
        &unrun("function midi(event) { pass(event); }\n})();\nconst outside = 1;\n(function () {"),
        "closes the device's function early"
    ));
    assert!(matches(&unrun("function midi(event) { pass(event);"), r"^code: a brace, bracket or quote in it isn't closed"));
    assert!(matches(&unrun("function midi(event) { pass(event); }\n<!-- a note -->"), "Live reads \"<!--\" anywhere"));
    assert!(matches(
        &unrun("function midi(event) { pass(event); }\n  /* x */ --> a note"),
        "and \"-->\" at the start of a line, as a comment"
    ));
    // A "-->" anywhere else is code Live and the check read alike.
    for code in [
        "// a note --> a chord\nfunction midi(event) { pass(event); }",
        "function midi(event) { let x = 2; while (x-->0) {} post(\"-->\"); pass(event); }",
    ] {
        assert!(findings_of(check_spec(&with(lowest(), json!({ "code": code })))).is_empty(), "{code}");
    }
    let twice = unrun("function midi(event) { pass(event); }\nconst a = 1; const a = 2;");
    assert!(matches(&twice, "^code: line 2: [a-z]"), "QuickJS's own word and line: {twice}");
    // Kept unrun, a line break JavaScript has ends no comment early, and a control character is written as its escape.
    let kept = kept_unrun("a\r\nb\rc\u{2028}d\u{2029}e \"q\" \\ \u{0}\u{1b}", "why");
    assert_eq!(kept, "// Kumi kept this code without running it: why. Fix it and make the device again.\n// a\n// b\n// c\n// d\n// e \"q\" \\ \\u{0}\\u{1b}");
    // A brace in a string or a comment is only text.
    assert!(findings_of(check_spec(&with(
        lowest(),
        json!({ "code": "const close = \"})();\"; // }\nfunction midi(event) { pass(event); }" })
    )))
    .is_empty());
    // A module runs outside every name the frame hides: code with one is kept unrun too. A "//" in a string doesn't
    // hide the rest of its line from the check: an import there is found all the same.
    assert!(matches(
        &unrun("import(\"data:text/javascript,0\");\nfunction midi(event) { pass(event); }"),
        "\"import\\(\" doesn't work in the device: modules aren't available"
    ));
    assert!(matches(
        &unrun("const url = \"https://x.y/\"; import(\"data:text/javascript,0\");\nfunction midi(event) { pass(event); }"),
        "doesn't work in the device, even in a comment or a string: modules aren't available"
    ));
    // Only import(…) is a module a function's body can hold: a word that starts with import, or a comment that says
    // export, is code that runs; a static import isn't one function's body, and QuickJS says why.
    for code in [
        "const importance = (event) => event.velocity / 127;\nfunction midi(event) { pass(event); }",
        "// important: keep notes in order\nfunction midi(event) { pass(event); }",
        "// export the note to outlet 0\nconst importFrom = (a) => a;\nfunction midi(event) { pass(event); }",
    ] {
        assert!(findings_of(check_spec(&with(lowest(), json!({ "code": code })))).is_empty(), "{code}");
    }
    assert!(matches(&unrun("import x from \"y\";\nfunction midi(event) { pass(event); }"), "^code: line 1: "));
    // A line break in what's quoted can't break the kept code: the frame still reads it and passes MIDI through.
    assert!(matches(
        &unrun("async function f() { await import\n(\"x\"); }\nfunction midi(event) { pass(event); }"),
        "\"import \\(\" doesn't work"
    ));
    // The frame takes whatever midi is a function.
    for code in ["const midi = (event) => pass(event);", "let midi = function (event) { pass(event); };"] {
        assert!(findings_of(check_spec(&with(lowest(), json!({ "code": code })))).is_empty(), "{code}");
    }
}

#[test]
fn a_spec_is_made_the_nearest_way_it_can_be_and_says_what_was_changed() {
    let (made, found) = check_spec(&with(lowest(), json!({ "type": "reverb" })));
    assert!(matches!(made, DeviceSpec::MidiEffect(_)));
    assert_eq!(found.changed, ["type \"reverb\": made as midi_effect (midi_effect, audio_effect or instrument)."]);
    let checked = check_spec(
        json!({ "type": "midi_effect", "name": "", "about": "", "controls": [
            { "name": "Window", "type": "number", "min": 5, "max": 1, "default": 3 }, { "name": "Window", "type": "switch", "default": true }, { "name": "Mode", "type": "choice", "options": ["A"], "default": "A" },
            { "name": "Level", "type": "integer", "min": 0, "max": 10, "default": 2.5 }, { "name": "Rate", "type": "number", "min": 0, "max": 1, "default": 0.5, "unit": "furlongs" }], "code": "send(1)", "tests": [{ "name": "x", "input": "no" }] })
        .as_object()
        .unwrap(),
    );
    let (made, found) = checked;
    let DeviceSpec::MidiEffect(made) = made else { panic!("{made:?}") };
    assert_eq!(made.name, "Device");
    let controls: Vec<Value> = made.controls.iter().map(|control| json!([control.name(), control.default_value()])).collect();
    assert_eq!(
        controls,
        [json!(["Window", 3.0]), json!(["Window 2", true]), json!(["Mode", "A"]), json!(["Level", 3.0]), json!(["Rate", 0.5])]
    );
    let text = found.all().cloned().collect::<Vec<_>>().join("\n");
    for expected in [
        r#"(?m)^name "": made as "Device""#,
        r"(?m)^about: none given",
        r"Window\): min was above max; made 1 to 5, starting at 3\.",
        r#"controls\[1\]\.name "Window": made as "Window 2""#,
        r"Level\): an integer control's min, max and default are whole numbers; made 0 to 10, starting at 3\.",
        r#"unit "furlongs" isn't one of ms, s, Hz"#,
        r"define function midi\(event\)",
        r"(?m)^tests\[0\] wasn't run",
    ] {
        assert!(matches(&text, expected), "{expected}: {text}");
    }
    assert!(!matches(&text, "Mode"), "one option is a menu Live can show: {text}");
}

#[test]
fn what_kumi_once_refused_is_made_the_nearest_way_it_can_be_and_said() {
    // A long description is cut at a word, within 400 characters.
    let long = "A delay that repeats every note it hears ".repeat(12);
    let (made, found) = check_spec(&with(lowest(), json!({ "about": long })));
    assert!(made.about().chars().count() <= 400 && made.about().ends_with('…'), "{}", made.about());
    assert!(matches(&found.changed.join(" "), r"^about: \d+ characters, cut to the first \d+ \(up to 400\)\."), "{found:?}");
    // Controls: a default past its range, a menu without options or with a default it hasn't got, a switch whose default
    // isn't true or false, and a type Kumi doesn't know.
    let (made, found) = check_spec(&with(
        lowest(),
        json!({ "controls": [
            { "name": "Gain", "type": "number", "min": 0, "max": 10, "default": 99 },
            { "name": "Shape", "type": "choice", "options": [], "default": "Sine" },
            { "name": "Scale", "type": "choice", "options": ["Major", "", "Minor"], "default": 1 },
            { "name": "Hold", "type": "switch", "default": "on" },
            { "name": "Feel", "type": "knob", "min": 0, "max": 1, "default": 0.5 },
        ] }),
    ));
    let controls: Vec<Value> =
        made.controls().iter().map(|control| json!([control.name(), control.type_name(), control.default_value()])).collect();
    assert_eq!(
        controls,
        [
            json!(["Gain", "number", 10.0]),
            json!(["Shape", "choice", "Shape"]),
            json!(["Scale", "choice", "Major"]),
            json!(["Hold", "switch", true]),
            json!(["Feel", "number", 0.5])
        ]
    );
    let said = found.changed.join("\n");
    for expected in [
        r"Gain\): the default was outside min and max; made 0 to 10, starting at 10\.",
        r#"Shape\): it had no options, its default "Sine" isn't one of its options; made with "Shape""#,
        r#"Scale\): option "" was left out, its default 1 isn't one of its options; made with "Major", "Minor", starting at "Major"\."#,
        r#"Hold\): a switch's default is true or false, not "on"; made on\."#,
        r#"Feel\): type "knob": made as a number"#,
    ] {
        assert!(matches(&said, expected), "{expected}: {said}");
    }
    // Code and a patch together: the patch is made. No code at all: the device passes what it gets on, or is silent.
    let (made, found) = check_spec(&with(grit(), json!({ "patch": "in.0 -> out.0\nin.1 -> out.1\n" })));
    assert!(matches!(&made, DeviceSpec::Patch(_)) && found.changed[0].starts_with("code: the device was made from its patch"), "{found:?}");
    for (base, made_code, says) in [
        (lowest(), "function midi(event) { pass(event); }", "passes MIDI on as it comes"),
        (grit(), "out1 = in1;\nout2 = in2;", "passes its input on as it comes"),
        (pluck(), "out1 = 0;\nout2 = 0;", "makes no sound"),
    ] {
        let (made, found) = check_spec(&with(base, json!({ "code": "", "tests": [] })));
        assert_eq!(made.code(), made_code);
        assert!(found.wrong[0].contains(says), "{found:?}");
    }
    // A blank patch, as a model leaves a field it doesn't use, is none: the code is made.
    for blank in [json!(""), json!(" "), json!([]), json!(false)] {
        let (made, found) = check_spec(&with(grit(), json!({ "patch": blank })));
        assert!(matches!(made, DeviceSpec::AudioEffect(_)) && found.is_empty(), "{blank}: {found:?}");
    }
    let (made, found) = check_spec(&with(grit(), json!({ "code": null, "patch": " " })));
    assert!(matches!(made, DeviceSpec::AudioEffect(_)) && found.wrong[0].contains("passes its input on as it comes"), "{found:?}");
    let (made, _) = check_spec(&with(grit(), json!({ "code": null, "patch": ["in.0 -> out.0", "in.1 -> out.1"] })));
    assert!(matches!(&made, DeviceSpec::Patch(patch) if patch.patch == "in.0 -> out.0\nin.1 -> out.1"), "a patch's lines as a list");
    // A patch past 100,000 characters is cut at a line's end, and that's said.
    let long = "in.0 -> [*~ 1.] -> out.0\n".repeat(5_000);
    let (made, found) = check_spec(&with(grit(), json!({ "patch": long })));
    let DeviceSpec::Patch(cut) = made else { panic!() };
    assert!(cut.patch.chars().count() <= 100_000 && cut.patch.ends_with("out.0"), "cut at a line's end");
    assert_eq!(
        found.wrong,
        ["patch: it's 125000 characters, past the 100000 Kumi reads, so it was cut after line 4000, and what came after was left out."]
    );
    // A menu of numbers: a number default is the option it's the text of, else the one it counts to from 0.
    let menu = |default: Value| {
        let checked = check_spec(&with(
            lowest(),
            json!({ "controls": [{ "name": "Steps", "type": "choice", "options": [1, 2, 4, 8, 16], "default": default }] }),
        ));
        (checked.0.controls()[0].default_value(), checked.1.changed.join(" "))
    };
    let (four, said) = menu(json!(4));
    assert_eq!(four, json!("4"));
    assert!(matches(&said, r#"its options were numbers, made as text: the code gets an option's text \("4"\)"#), "{said}");
    assert_eq!((menu(json!(8)).0, menu(json!(0)).0, menu(json!(5)).0), (json!("8"), json!("1"), json!("1")));
    assert!(menu(json!(5)).1.contains("its default 5 isn't one of its options"));
    // Controls given by name, a unit that isn't text, and min and max the same past 2^53.
    let (made, found) = check_spec(&with(
        lowest(),
        json!({ "controls": { "Window": { "type": "number", "min": 1, "max": 50, "default": 15, "unit": 5 }, "Big": { "type": "number", "min": 1e17, "max": 1e17 } } }),
    ));
    assert_eq!(made.controls().iter().map(|control| control.name()).collect::<Vec<_>>(), ["Window", "Big"]);
    let said = found.changed.join("\n");
    assert!(said.contains("controls: given by name") && said.contains("unit 5 isn't text, so it shows none"), "{said}");
    let kumi_daw_ableton::devices::spec::Control::Number(big) = &made.controls()[1] else { panic!() };
    assert!(big.max > big.min, "{big:?}");
    assert!(matches(
        &findings_of(check_spec(&with(lowest(), json!({ "controls": "Rate" })))).join(" "),
        "^controls \"Rate\": controls are a list"
    ));
    // A description without a space near 400 is cut where it reaches it.
    let (made, _) = check_spec(&with(lowest(), json!({ "about": format!("A {}", "x".repeat(500)) })));
    assert!(made.about().chars().count() == 400 && made.about().starts_with("A xxx") && made.about().ends_with('…'), "{}", made.about());
    // More tests than are run: the first 32.
    let test = json!({ "name": "t", "input": [], "expect": [] });
    let (DeviceSpec::MidiEffect(made), found) = check_spec(&with(lowest(), json!({ "tests": vec![test; 33] }))) else { panic!() };
    assert_eq!(made.tests.len(), 32);
    assert_eq!(found.changed, ["tests: the first 32 were run, and 1 more weren't."]);
}

#[test]
fn kumi_runs_the_devices_tests_and_its_own_checks_a_working_device_passes_a_broken_one_is_told_what_went_wrong() {
    assert_eq!(check_midi_device(&spec(json!({}))), Checked { passed: 4, of: 4, problems: vec![], runaway: false });
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
    // Code that doesn't read isn't run: it isn't one function's body, so the device is made with it kept unrun.
    let broken = findings_of(check_spec(&with(lowest(), json!({ "code": "function midi(event) { pass(event) ", "tests": [] }))));
    assert!(matches(&broken.join(" "), "isn't closed, or one is closed twice, so it doesn't read as one function's body"), "{broken:?}");
    // In Kumi's own process too, where nothing else would end it.
    let flooded = check_midi_device(&spec(
        json!({ "code": "function midi(event) { for (;;) send({ type: 'cc', controller: 1, value: 1 }); }", "tests": [] }),
    ));
    assert!(matches(&flooded.problems.join(" "), "and was stopped there; something sends without end"), "{:?}", flooded.problems);
    // The time a device reads is today's, as in Live: code that acts on the date is checked as it will run there.
    let dated = check_midi_device(&spec(json!({ "code": "function midi(event) { if (Date.now() > 1e12) pass(event); }", "tests": [
        { "name": "passes", "input": [{ "type": "noteon", "pitch": 60, "at": 0 }, { "type": "noteoff", "pitch": 60, "at": 100 }], "expect": [{ "type": "noteon", "pitch": 60 }, { "type": "noteoff", "pitch": 60 }] }] })));
    assert_eq!(dated.problems, Vec::<String>::new());
}

// ported with devices/tool.rs: "make_device reads its guide on demand, makes a device where Live's Browser sees it, and waits for the Browser"

#[tokio::test]
async fn a_devices_code_is_checked_in_a_process_of_its_own_it_cant_reach_kumi_cant_make_code_from_strings_and_cant_hang_kumi() {
    use_built_harness();
    // Each check in a process of its own, all at once: two of them wait out their deadline.
    let isolated = |code: Value, options: IsolatedOptions| async move { check_midi_device_isolated(&spec(code), options).await };
    // The loops that send or say without end are stopped by what a run keeps, not at the deadline: theirs is twice the
    // usual, as a Windows runner took 9 s and more to get there beside the other checks.
    let capped = IsolatedOptions { timeout_ms: Some(20_000) };
    let (same, escape, (endless, endless_took), sending, saying, looping, throwing) = tokio::join!(
        isolated(json!({}), IsolatedOptions::default()),
        isolated(
            json!({ "code": "function midi(event) { const F = post['constr' + 'uctor']; F('return process')().exit(3); pass(event); }", "tests": [] }),
            IsolatedOptions::default(),
        ),
        async {
            let started = Instant::now();
            let endless = isolated(
                json!({ "code": "function midi(event) { while (true) {} }", "tests": [] }),
                IsolatedOptions { timeout_ms: Some(1_500) },
            )
            .await;
            (endless, started.elapsed())
        },
        isolated(
            json!({ "code": "function midi(event) { for (;;) send({ type: 'cc', controller: 1, value: 1 }); }", "tests": [] }),
            capped.clone(),
        ),
        isolated(json!({ "code": "function midi(event) { for (;;) post('Kumi device: again\\n'); pass(event); }", "tests": [] }), capped),
        isolated(
            json!({ "code": "let i = 0; while (true) { i++; }\nfunction midi(event) { pass(event); }", "tests": [] }),
            IsolatedOptions::default()
        ),
        isolated(
            json!({ "code": "function midi(event) { if (event.type === 'cc') missing(); if (event.type !== 'noteoff') pass(event); }", "tests": [] }),
            IsolatedOptions::default(),
        ),
    );
    // The same verdicts as in-process.
    assert_eq!(same, Checked { passed: 4, of: 4, problems: vec![], runaway: false });
    // An escape through a host function's constructor can't compile anything.
    assert!(matches(&escape.problems.join(" "), "it threw: .*[Cc]ode generation from strings disallowed"), "{:?}", escape.problems);
    // A loop that never ends is stopped at the deadline, and said.
    assert!(matches(&endless.problems.join(" "), "didn't finish within 2 s; something loops forever"), "{:?}", endless.problems);
    assert!(endless.runaway, "code that never stops is said to run away, for the device to keep it unrun");
    assert!(endless_took.as_millis() < 5_000);
    // A loop that sends or says without end is stopped once a run has kept enough, and said so: before, what it sent
    // and said grew the check's process toward a gigabyte until the deadline.
    assert!(
        matches(&sending.problems.join(" "), r"it sent more than \d+ events for 11 in, and was stopped there; something sends without end"),
        "{:?}",
        sending.problems
    );
    assert!(sending.runaway);
    assert!(matches(&saying.problems.join(" "), "Kumi's check: it threw: again"), "{:?}", saying.problems);
    assert!(!matches(&saying.problems.join(" "), "didn't finish"), "{:?}", saying.problems);
    assert!(saying.runaway, "it says without end");
    // A top level that never stops runs away too; code that throws, fails its tests or leaves notes hanging doesn't.
    assert!(looping.runaway && matches(&looping.problems.join(" "), "Script execution timed out after 2000ms"), "{looping:?}");
    assert!(!throwing.runaway && throwing.problems.len() == 2, "{throwing:?}");
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
    match check_spec(input).0 {
        DeviceSpec::AudioEffect(spec) => GenSpec::from(&spec),
        DeviceSpec::Instrument(spec) => GenSpec::from(&spec),
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
    assert!(matches!(&checked, (DeviceSpec::AudioEffect(_), found) if found.is_empty()), "{checked:?}");
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
    assert!(matches!(&checked, (DeviceSpec::Instrument(spec), found) if spec.voices == 6 && found.is_empty()), "{checked:?}");
    let patcher = instrument_patcher(&gen_spec(&pluck()));
    assert_eq!(inner(&patcher, "obj-poly")["text"], "poly 6 1");
    assert_eq!(inner(&patcher, "obj-route")["text"], "route 1 2 3 4 5 6");
    assert_eq!(inner(&patcher, "obj-bendcentre")["text"], "- 64");
    assert_eq!(inner(&patcher, "obj-bendscale")["text"], "/ 32.");
    let voices = boxes_of(&patcher).into_iter().filter(|item| item["id"].as_str().unwrap().starts_with("obj-voice-")).count();
    assert_eq!(voices, 6);
    // Each voice is a [p voice n]: its note in on the left, the Params, bend and mod wheel on the right, its sound out.
    assert_eq!(inner(&patcher, "obj-voice-2")["text"], "p voice 2");
    let voice = json!({ "patcher": inner(&patcher, "obj-voice-1")["patcher"] });
    assert_eq!(inner(&voice, "obj-voice")["text"], "gen~");
    assert_eq!(inner(&voice, "obj-voice")["numinlets"], 1, "no audio input, one inlet for its Params");
    assert_eq!((inner(&voice, "obj-notes")["maxclass"].as_str(), inner(&voice, "obj-params")["index"].as_u64()), (Some("inlet"), Some(2)));
    let lines = wires(&patcher);
    for voice in 1..=6 {
        for expected in [
            format!("obj-route:{}>obj-voice-{voice}:0", voice - 1),
            format!("obj-bend:0>obj-voice-{voice}:1"),
            format!("obj-mod:0>obj-voice-{voice}:1"),
            format!("obj-prepend-1:0>obj-voice-{voice}:1"),
            format!("obj-voice-{voice}:0>obj-output:0"),
            format!("obj-voice-{voice}:1>obj-output:1"),
        ] {
            assert!(lines.contains(&expected), "{expected}");
        }
    }
    let inside = wires(&voice);
    for expected in [
        "obj-notes:0>obj-order:0",
        "obj-order:1>obj-unpack:0",
        "obj-order:0>obj-heard:0",
        "obj-note:0>obj-voice:0",
        "obj-strike:0>obj-voice:0",
        "obj-params:0>obj-voice:0",
        "obj-voice:0>obj-left:0",
        "obj-voice:1>obj-right:0",
    ] {
        assert!(inside.contains(&expected.to_string()), "{expected}");
    }
    assert!(
        inside.contains(&"obj-heard:1>obj-played:0".to_string()) && inside.contains(&"obj-played:1>obj-bang:0".to_string()),
        "a strike counts only notes played (velocity above 0)"
    );
    // On real Live, a bare counter (first count 0) left strike at 0, so no voice played its first note.
    assert_eq!(inner(&voice, "obj-count")["text"], "counter 1 1000000", "a voice's first note changes strike too");
    assert!(matches(&code_of(&patcher, "obj-output"), "Param kumi_output"));
    assert!(!matches(&code_of(&patcher, "obj-output"), "kumi_mix"), "an instrument has no dry signal to mix");
    assert_eq!(decode_amxd(&encode_amxd(DeviceType::Instrument, &patcher)).map(|decoded| decoded.kind), Some(DeviceType::Instrument));
}

#[test]
fn kumis_checks_for_an_audio_effect_or_an_instrument_say_whats_wrong_beside_the_device_each_so_it_can_be_fixed() {
    let problems = |input: Map<String, Value>| findings_of(check_spec(&input)).join("\n");
    assert!(matches(
        &problems(with(grit(), json!({ "controls": [{ "name": "Mix", "type": "number", "min": 0, "max": 1, "default": 1 }] }))),
        "Kumi adds Mix"
    ));
    assert!(matches(
        &problems(with(grit(), json!({ "controls": [{ "name": "Delay", "type": "number", "min": 0, "max": 1, "default": 1 }] }))),
        r#"controls\[0\]\.name "Delay": made as "Delay Amount", whose Param in the code is delay_amount: delay is gen~'s own\."#
    ));
    // A control whose Param Kumi changed, or a control nothing reads, changes nothing: said, with what the code reads.
    let delay = json!({ "controls": [{ "name": "Delay", "type": "number", "min": 0, "max": 1, "default": 1 }], "code": "out1 = in1 * delay; out2 = in2 * delay;" });
    assert!(matches(
        &problems(with(grit(), delay)),
        r#"code: it reads delay, but the "Delay Amount" control's Param is delay_amount \(delay is gen~'s own\), so the control changes nothing: read delay_amount where it should act\."#
    ));
    let rates = json!({ "controls": [{ "name": "Rate", "type": "number", "min": 0, "max": 1, "default": 1 }, { "name": "Rate Amount", "type": "number", "min": 0, "max": 1, "default": 1 }], "code": "out1 = in1 * rate_amount; out2 = in2;" });
    let (made, found) = check_spec(&with(grit(), rates));
    assert_eq!(
        made.controls().iter().map(|control| control.name()).collect::<Vec<_>>(),
        ["Rate Amount 2", "Rate Amount"],
        "a rename keeps clear of a later control's name"
    );
    assert!(matches(&found.wrong.join("\n"), r#"nothing in it reads rate_amount_2, the "Rate Amount 2" control's Param"#), "{found:?}");
    let gain = check_spec(&with(
        grit(),
        json!({ "controls": [{ "name": "Gain", "type": "number", "min": 0, "max": 1, "default": 1 }], "code": "out1 = in1 * gain; out2 = in2 * gain;" }),
    ));
    assert!(gain.1.is_empty(), "Kumi's output stage is a gen~ of its own: gain is free: {:?}", gain.1);
    assert!(matches(
        &problems(with(
            grit(),
            json!({ "controls": [{ "name": "Output", "type": "number", "min": 0, "max": 1, "default": 1 }], "code": "out1 = in1 * output; out2 = in2 * output;" })
        )),
        r"code: it reads output, the Output control's Param, but that control was left out"
    ));
    let knobs: Vec<Value> = (0..129)
        .map(|index| json!({ "name": format!("Knob {}", index + 1), "type": "number", "min": 0, "max": 1, "default": 0 }))
        .collect();
    assert!(matches(
        &problems(with(grit(), json!({ "controls": knobs }))),
        r"controls: the first 128 were made, and 1 more were left out\."
    ));
    assert!(matches(
        &problems(with(
            grit(),
            json!({ "controls": [{ "name": "Pre-Delay", "type": "number", "min": 0, "max": 1, "default": 0 }, { "name": "Pre Delay", "type": "number", "min": 0, "max": 1, "default": 0 }] })
        )),
        r#"controls\[1\]\.name "Pre Delay": made as "Pre Delay 2""#
    ));
    assert!(matches(&problems(with(grit(), json!({ "code": "out1 = in1;" }))), r"assign out1 \(left\) and out2 \(right\)"));
    assert!(matches(&problems(with(grit(), json!({ "code": "out1 = in3; out2 = in1;" }))), "two inputs"));
    assert!(matches(&problems(with(grit(), json!({ "code": "out1 = cycle(440); out2 = out1;" }))), "reads its input"));
    assert!(matches(&problems(with(grit(), json!({ "code": "out1 = in1 * (velocity / 127); out2 = in2;" }))), "gets no notes"));
    assert!(matches(&problems(with(grit(), json!({ "code": "out1 = tanh(in1; out2 = in2;" }))), "don't pair up"));
    assert!(matches(&problems(with(pluck(), json!({ "code": "out1 = in1; out2 = in2;" }))), "no audio input"));
    assert!(matches(&problems(with(pluck(), json!({ "code": "out1 = cycle(440); out2 = out1;" }))), "plays the note it's given"));
    assert!(matches(&problems(with(pluck(), json!({ "voices": 33 }))), r"voices 33: made 32 \(1, mono, to 32\)"));
    // What the code declares itself, Kumi doesn't declare again (gen~ refuses a name declared twice): it uses the code's.
    let declared = with(pluck(), json!({ "code": "Param note(60);\nout1 = cycle(mtof(note)); out2 = out1;" }));
    assert!(matches(
        &problems(declared.clone()),
        "it declares note, which Kumi gives each voice, so Kumi used the code's declaration of it"
    ));
    assert_eq!(voice_code(&gen_spec(&declared)).matches("Param note").count(), 1);
    let drive = with(grit(), json!({ "code": "Param drive(1, min=1, max=20);\nout1 = tanh(in1 * drive * tone * hard); out2 = out1;" }));
    assert!(matches(&problems(drive.clone()), "it declares drive, the \"Drive\" control's Param, so Kumi didn't declare it again"));
    assert_eq!(effect_code(&gen_spec(&drive)).matches("Param drive").count(), 1);
    // Only a Param takes a control's value, or a voice's: a History by its name is said, every name a declaration gives.
    let history = problems(with(grit(), json!({ "code": "History lp(0), drive(0);\nout1 = in1 * drive * tone * hard; out2 = out1;" })));
    assert!(
        matches(
            &history,
            "it declares drive as a History, and drive is the \"Drive\" control's Param: only a Param takes the control's value"
        ),
        "{history}"
    );
    let velocity =
        problems(with(pluck(), json!({ "code": "History velocity(0);\nout1 = cycle(mtof(note)) * velocity * decay; out2 = out1;" })));
    assert!(matches(&velocity, "it declares velocity as a History, and Kumi gives each voice its velocity as a Param"), "{velocity}");
    assert_eq!(
        problems(with(grit(), json!({ "tests": [{ "name": "ignored", "input": [], "expect": [] }] }))),
        "tests: they run a MIDI effect's code, and an audio effect or instrument is heard with audition once it's loaded, so they weren't run.",
        "an audio effect isn't tested with MIDI: its tests are left out, and said"
    );
}

#[test]
fn a_device_gets_what_it_needs_many_controls_in_rows_on_the_face_names_with_punctuation_long_menus_more_voices() {
    let knobs: Vec<Value> = (0..20)
        .map(|index| json!({ "name": format!("Size/Decay-{}", index + 1), "type": "number", "min": 0, "max": 1, "default": 0.5 }))
        .collect();
    let reads = (0..20).map(|index| format!("size_decay_{}", index + 1)).collect::<Vec<_>>().join(" + ");
    let checked = check_spec(&with(grit(), json!({ "controls": knobs, "code": format!("out1 = in1 * ({reads}); out2 = in2;") })));
    assert!(matches!(&checked, (DeviceSpec::AudioEffect(spec), found) if spec.controls.len() == 20 && found.is_empty()), "{checked:?}");
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
    // 22 dials: columns of two, all inside Live's 169-pixel device view, none over another, Kumi's own Mix and Output
    // last, and the face as wide as its columns.
    assert!(faces.iter().all(|rect| rect[1] >= 0.0 && rect[1] + rect[3] <= 169.0));
    for (at, a) in faces.iter().enumerate() {
        for b in &faces[at + 1..] {
            let apart = a[0] + a[2] <= b[0] || b[0] + b[2] <= a[0] || a[1] + a[3] <= b[1] || b[1] + b[3] <= a[1];
            assert!(apart, "{a:?} over {b:?}");
        }
    }
    let width = patcher["patcher"]["devicewidth"].as_f64().unwrap();
    assert!(faces.iter().all(|rect| rect[0] + rect[2] <= width));
    assert!(faces[20][0] > faces[19][0] && faces[21][0] >= faces[20][0], "Mix and Output last");
    // A device with no controls is Live's narrowest.
    let midi = midi_device_patcher(&spec(json!({})));
    assert_eq!(midi["patcher"]["devicewidth"], 120.0);
    let options: Vec<String> = (0..40).map(|index| format!("Mode {}", index + 1)).collect();
    let menu = check_spec(&with(
        lowest(),
        json!({ "controls": [{ "name": "Scale", "type": "choice", "options": options, "default": "Mode 1" }] }),
    ));
    assert!(menu.1.is_empty(), "{menu:?}");
    let wide = check_spec(&with(pluck(), json!({ "voices": 16 })));
    assert!(matches!(&wide, (DeviceSpec::Instrument(spec), _) if spec.voices == 16));
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
    let (DeviceSpec::MidiEffect(free), _) = free else { panic!("{free:?}") };
    assert!(free.runs_free);
    assert_eq!(
        check_midi_device_isolated(&free, IsolatedOptions::default()).await,
        Checked { passed: 0, of: 0, problems: vec![], runaway: false }
    );
    // Running free isn't sending without end.
    let flood = check_midi_device(&MidiSpec {
        code: "function tick() { for (let i = 0; i < 50; i++) send({ type: 'cc', controller: 1, value: i }); after(1, tick); }\ntick();\nfunction midi(event) { pass(event); }".into(),
        ..free.clone()
    });
    assert!(matches(&flood.problems.join(" "), "something sends without end"), "{:?}", flood.problems);
    let (DeviceSpec::MidiEffect(asked), found) = check_spec(&with(lfo, json!({ "runs_free": "yes" }))) else { panic!() };
    assert!(
        !asked.runs_free && matches(&found.changed.join(" "), r#"runs_free "yes": taken as false \(true for a MIDI effect"#),
        "{found:?}"
    );
}

// ported with devices/tool.rs: "make_device makes an audio effect and an instrument, with Kumi's own knobs, and says to hear them"

#[tokio::test(flavor = "current_thread")]
async fn make_device_reads_guide_makes_a_device_and_waits_for_live_browser() {
    use futures::FutureExt;
    use kumi_common::abort::Signal;
    use std::cell::Cell;
    use_built_harness();
    use_own_kumi_folder();
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
        made: Default::default(),
    });
    let guide = tool.execute(json!({"guide":true}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
    assert!(guide.text.starts_with("Making a MIDI effect"));
    assert!(guide.text.contains("Every note-on the device sends gets a note-off"));
    let made = tool.execute(lowest(), Signal::new()).await.unwrap();
    assert!(!made.is_error, "{}", made.text);
    let result: Value = serde_json::from_str(&made.text).unwrap();
    assert_eq!(result["itemId"], "user_library/Kumi/MIDI Effects/Lowest Note");
    assert_eq!(result["file"], folder.path().join("Kumi").join("MIDI Effects").join("Lowest Note.amxd").to_string_lossy().as_ref());
    assert_eq!(result["controls"], json!(["Window (1–50 ms; 15)"]));
    assert!(result["checks"].as_str().unwrap().contains("4 of 4"));
    assert!(result.get("browser").is_none());
    assert_eq!(*seen.borrow(), ["user_library/Kumi/MIDI Effects/Lowest Note", "user_library/Kumi/MIDI Effects/Lowest Note"]);
    let decoded = decode_amxd(&std::fs::read(folder.path().join("Kumi/MIDI Effects/Lowest Note.amxd")).unwrap()).unwrap();
    assert_eq!(decoded.kind, DeviceType::MidiEffect);
    assert_eq!(decoded.patcher["patcher"]["title"], "Lowest Note");
    let again: Value = serde_json::from_str(&tool.execute(lowest(), Signal::new()).await.unwrap().text).unwrap();
    assert_eq!(again["itemId"], "user_library/Kumi/MIDI Effects/Lowest Note", "the same name replaces the device this conversation made");
    assert_eq!(again["replaced"], "the MIDI effect this conversation made as Lowest Note", "{again}");
    assert!(again.get("wrong").is_none(), "{again}");
    let broken = tool
        .execute(
            with(
                lowest(),
                json!({"name":"Broken","code":"function midi(event) { if (event.type !== 'noteoff') send(event); }","tests":[]}),
            ),
            Signal::new(),
        )
        .await
        .unwrap();
    assert!(!broken.is_error, "{}", broken.text);
    let broken: Value = serde_json::from_str(&broken.text).unwrap();
    assert!(broken["wrong"][0].as_str().unwrap().contains("leaves notes hanging"), "{broken}");
    assert_eq!(
        broken["checks"], "It has no tests; what Kumi found is under wrong",
        "never \"nothing wrong\" beside a list that says otherwise"
    );
    assert!(
        broken["next"].as_str().unwrap().contains(
            "what's under wrong needs fixing: fix it and call make_device again in this conversation with the name \"Broken\", which replaces this device"
        ),
        "{broken}"
    );
    assert!(folder.path().join("Kumi/MIDI Effects/Broken.amxd").exists(), "made, with what's wrong beside it");
}

#[tokio::test(flavor = "current_thread")]
async fn make_device_keeps_code_that_never_stops_in_the_device_without_running_it() {
    use kumi_common::abort::Signal;
    use_built_harness();
    let folder = tempfile::tempdir().unwrap();
    let tool = tool_in(folder.path(), Default::default());
    // In Max, whose JavaScript has no deadline, code that never stops would hold Max and Live up: the device is made, its
    // code kept unrun (it passes MIDI on as it comes), and what Kumi's check found is said.
    for (name, code, says) in [
        ("Loops", "let i = 0; while (true) { i++; }\nfunction midi(event) { pass(event); }", "Script execution timed out"),
        ("Floods", "function midi(event) { for (;;) send({ type: 'cc', controller: 1, value: 1 }); }", "something sends without end"),
    ] {
        let made = tool.execute(with(lowest(), json!({ "name": name, "code": code, "tests": [] })), Signal::new()).await.unwrap();
        let made: Value = serde_json::from_str(&made.text).unwrap();
        let wrong = made["wrong"].as_array().unwrap().iter().map(|said| said.as_str().unwrap()).collect::<Vec<_>>().join("\n");
        assert!(wrong.contains(says) && wrong.contains("Kumi kept the code in the device without running it"), "{made}");
        assert!(made["checks"].as_str().unwrap().starts_with("Kumi's check found the code never stops"), "{made}");
        let file = std::fs::read(folder.path().join("Kumi").join("MIDI Effects").join(format!("{name}.amxd"))).unwrap();
        let written = String::from_utf8_lossy(&file);
        // As the file holds it: every line of the code is a comment.
        let commented = written.match_indices("function midi").all(|(at, _)| written[..at].ends_with("// "));
        assert!(written.contains("Kumi kept this code without running it") && commented, "kept as comments");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn make_device_answers_what_maxs_objects_take_and_send_and_makes_a_device_given_beside_them() {
    use kumi_common::abort::Signal;
    use kumi_daw_ableton::devices::patch::page::NO_MAX;
    let folder = tempfile::tempdir().unwrap();
    let library = folder.path().join("library");
    let tool = tool_in(&library, Default::default());
    let ask = |input: Value| input.as_object().unwrap().clone();
    let answer = tool.execute(ask(json!({"objects": ["metro 16n", "no.such.object"]})), Signal::new()).await.unwrap();
    assert!(!answer.is_error, "{}", answer.text);
    // With Max on this machine, a page for each object; without it, Kumi says so.
    let paged = answer.text.starts_with("[metro] ") && answer.text.contains("\n\nNo [no.such.object] in this Max.");
    assert!(answer.text == NO_MAX || paged, "{}", answer.text);
    let both = tool.execute(ask(json!({"objects": ["metro"], "guide": true, "type": "midi_effect"})), Signal::new()).await.unwrap();
    assert!(both.text.contains("\n\nMaking a MIDI effect") && both.text.contains("make_device with objects"));
    let none = tool.execute(ask(json!({"objects": []})), Signal::new()).await.unwrap();
    assert!(!none.is_error && none.text.starts_with("objects: give one or more of Max's objects"), "{}", none.text);
    let named = tool.execute(ask(json!({"objects": ["metro"], "name": "Arp"})), Signal::new()).await.unwrap();
    assert!(named.text.ends_with("To make the device as well, give its code or its patch beside these."), "{}", named.text);
    assert!(!library.exists(), "nothing to make, so nothing is made");
    // Beside a device, lookups and the guide come with it; none ([]) is nothing.
    let device = |extra: Value| with(with(lowest(), json!({ "tests": [] })), extra);
    let made: Value =
        serde_json::from_str(&tool.execute(device(json!({"objects": ["metro"], "guide": true})), Signal::new()).await.unwrap().text)
            .unwrap();
    assert_eq!(made["made"], "Lowest Note");
    assert!(made["objects"].as_str().is_some_and(|pages| pages == NO_MAX || pages.starts_with("[metro] ")), "{made}");
    assert!(made["guide"].as_str().unwrap().starts_with("Making a MIDI effect"), "{made}");
    let made: Value =
        serde_json::from_str(&tool.execute(device(json!({"objects": [], "patch": ""})), Signal::new()).await.unwrap().text).unwrap();
    assert!(made.get("objects").is_none() && made.get("guide").is_none() && made["type"] == "MIDI effect", "{made}");
}

#[tokio::test(flavor = "current_thread")]
async fn make_device_replaces_only_a_device_this_conversation_made_and_keeps_what_it_replaces() {
    use kumi_common::abort::Signal;
    let folder = tempfile::tempdir().unwrap();
    let kept = folder.path().join("kept");
    let made = Rc::new(RefCell::new(Made::keeping(kept.clone())));
    made.borrow_mut().conversation("first");
    let tool = tool_in(folder.path(), made.clone());
    let device = |name: &str| with(lowest(), json!({ "name": name, "tests": [] }));
    let run = |input: Map<String, Value>| {
        let tool = tool.clone();
        async move { serde_json::from_str::<Value>(&tool.execute(input, Signal::new()).await.unwrap().text).unwrap() }
    };
    // A device in the folder that this conversation didn't make may be in a saved Set: it keeps its file.
    std::fs::create_dir_all(folder.path().join("Kumi/MIDI Effects")).unwrap();
    std::fs::write(folder.path().join("Kumi/MIDI Effects/Echo.amxd"), "someone's").unwrap();
    let echo = run(device("Echo")).await;
    assert_eq!(echo["made"], "Echo 2");
    assert!(echo.get("replaced").is_none());
    assert!(
        echo["changed"][0] == "name: made as \"Echo 2\": Echo is another device in Live's User Library, and a saved Set may use it.",
        "{echo}"
    );
    assert_eq!(std::fs::read_to_string(folder.path().join("Kumi/MIDI Effects/Echo.amxd")).unwrap(), "someone's");
    // Made again by the name asked for, or the name it was made as, it's replaced, and the old one is kept.
    for name in ["Echo", "Echo 2"] {
        let again = run(device(name)).await;
        assert_eq!(again["made"], "Echo 2", "{again}");
        assert!(
            again["replaced"].as_str().unwrap().starts_with("the MIDI effect this conversation made as Echo 2; the old one is kept at "),
            "{again}"
        );
    }
    assert_eq!(std::fs::read_dir(&kept).unwrap().count(), 2);
    // A device of another kind by that name isn't replaced (a saved Set expects the kind it has): it's made in its own
    // kind's folder, beside it.
    let audio = run(with(grit(), json!({ "name": "Echo 2" }))).await;
    assert_eq!((audio["made"].as_str(), audio["itemId"].as_str()), (Some("Echo 2"), Some("user_library/Kumi/Audio Effects/Echo 2")));
    assert!(audio.get("replaced").is_none() && audio.get("changed").is_none(), "{audio}");
    assert!(folder.path().join("Kumi/MIDI Effects/Echo 2.amxd").exists());
    // Made again by that name, each replaces the device of its own kind: the audio effect didn't take the MIDI effect's
    // place in what this conversation made.
    let midi = run(device("Echo 2")).await;
    assert_eq!((midi["made"].as_str(), midi["itemId"].as_str()), (Some("Echo 2"), Some("user_library/Kumi/MIDI Effects/Echo 2")));
    assert!(midi["replaced"].as_str().unwrap().starts_with("the MIDI effect this conversation made as Echo 2"), "{midi}");
    let audio = run(with(grit(), json!({ "name": "Echo 2" }))).await;
    assert!(audio["replaced"].as_str().unwrap().starts_with("the audio effect this conversation made as Echo 2"), "{audio}");
    // Kumi Ears is kept beside the audio effects, and Kumi writes it again whenever it differs: an audio effect never
    // takes its name, even before it's there. A MIDI effect, in a folder of its own, may.
    let ears = run(with(grit(), json!({ "name": "Kumi Ears" }))).await;
    assert_eq!(ears["made"], "Kumi Ears 2", "{ears}");
    assert!(
        ears["changed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|said| said == "name: made as \"Kumi Ears 2\": Kumi Ears is the name of Kumi's own listening device."),
        "{ears}"
    );
    assert_eq!(run(device("Kumi Ears")).await["itemId"], "user_library/Kumi/MIDI Effects/Kumi Ears");
    // Names Kumi fits to the same file name are two devices, not one replacing the other.
    let (chord, keys) = (run(device("和音")).await, run(device("🎹")).await);
    assert_eq!((chord["made"].as_str(), keys["made"].as_str()), (Some("Device"), Some("Device 2")), "{keys}");
    let (first, second) = (run(device("")).await, run(device("")).await);
    assert_eq!((first["made"].as_str(), second["made"].as_str()), (Some("Device 3"), Some("Device 4")), "no name is no device's");
    // Edited since (saved in Max), or made in another conversation, it's someone's own.
    std::fs::write(folder.path().join("Kumi/MIDI Effects/Device.amxd"), "edited in Max").unwrap();
    assert_eq!(run(device("和音")).await["made"], "Device 5");
    made.borrow_mut().conversation("second");
    assert_eq!(run(device("Echo")).await["made"], "Echo 3");
    // A folder Kumi can't write leaves no device, and says what to do.
    let file = folder.path().join("a file");
    std::fs::write(&file, "").unwrap();
    let unwritable = tool_in(&file, Default::default()).execute(device("Echo"), Signal::new()).await.unwrap();
    assert!(
        unwritable.is_error && unwritable.text.contains("Make the folder writable, then call make_device again."),
        "{}",
        unwritable.text
    );
}

#[test]
fn each_kind_of_device_is_made_in_a_folder_the_bridge_reads_as_that_kind() {
    // The bridge knows what a User Library item loads as only by a folder on its path named for its kind (as Live's own
    // preset folders are), and loads an item that might be an instrument onto no track that has one: an effect made in
    // a folder of no kind never went after an instrument. Read from the bridge's own table, so the two can't drift.
    let script = kumi_testkit::workspace_path("daws/ableton/remote-script/ableton_mcp_remote_script.py");
    let script = std::fs::read_to_string(script).unwrap();
    let table = script.lines().find(|line| line.trim_start().starts_with("_PRESET_FOLDER_TYPES = ")).expect("the bridge's folder table");
    let folders: Vec<(Regex, u64)> = Regex::new(r#"re\.compile\(r"([^"]+)", re\.I\), (\d)\)"#)
        .unwrap()
        .captures_iter(table)
        .map(|found| (Regex::new(&format!("(?i)^(?:{})$", &found[1])).unwrap(), found[2].parse().unwrap()))
        .collect();
    assert_eq!(folders.len(), 3, "{table}");
    // Live's device types: 1 an instrument, 2 an audio effect, 4 a MIDI effect.
    for (kind, live) in [(DeviceType::Instrument, 1), (DeviceType::AudioEffect, 2), (DeviceType::MidiEffect, 4)] {
        let read = folders.iter().find(|(pattern, _)| pattern.is_match(kind_folder(kind))).map(|(_, live)| *live);
        assert_eq!(read, Some(live), "{}", kind_folder(kind));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn make_device_makes_audio_effect_and_instrument_with_kumis_knobs() {
    use futures::FutureExt;
    use kumi_common::abort::Signal;
    use_own_kumi_folder();
    let folder = tempfile::tempdir().unwrap();
    let tool = device_tool(DeviceToolOptions {
        user_library: folder.path().to_string_lossy().into(),
        wait_ms: Some(1_000),
        browser_sees: Rc::new(|_, _| async { Ok(true) }.boxed_local()),
        made: Default::default(),
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
    assert_eq!(effect["itemId"], "user_library/Kumi/Audio Effects/Grit");
    assert_eq!(
        effect["controls"],
        json!(["Drive (0–24 dB; 6)", "Tone (0–1; 0.5)", "Hard (on/off; off)", "Mix (0–100 %; 100)", "Output (-36–12 dB; 0)"])
    );
    assert!(effect["next"].as_str().unwrap().contains("audition"));
    assert_eq!(
        decode_amxd(&std::fs::read(folder.path().join("Kumi/Audio Effects/Grit.amxd")).unwrap()).unwrap().kind,
        DeviceType::AudioEffect
    );
    let instrument: Value = serde_json::from_str(&tool.execute(pluck(), Signal::new()).await.unwrap().text).unwrap();
    assert_eq!(instrument["type"], "instrument");
    assert_eq!(instrument["voices"], 6);
    assert_eq!(instrument["itemId"], "user_library/Kumi/Instruments/Pluck");
    assert_eq!(
        decode_amxd(&std::fs::read(folder.path().join("Kumi/Instruments/Pluck.amxd")).unwrap()).unwrap().kind,
        DeviceType::Instrument
    );
}

#[tokio::test(flavor = "current_thread")]
async fn make_device_makes_a_device_from_a_patch_of_max_objects_and_says_whats_wrong_with_one() {
    use futures::FutureExt;
    use kumi_common::abort::Signal;
    use_own_kumi_folder();
    let folder = tempfile::tempdir().unwrap();
    let tool = device_tool(DeviceToolOptions {
        user_library: folder.path().to_string_lossy().into(),
        wait_ms: Some(1_000),
        browser_sees: Rc::new(|_, _| async { Ok(true) }.boxed_local()),
        made: Default::default(),
    });
    let guide = tool.execute(json!({"guide":true,"type":"audio_effect"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
    assert!(guide.text.contains("Patching with Max's objects") && guide.text.contains("in.0 and in.1 are the input's left and right"));
    let gain = json!({
        "type": "audio_effect", "name": "Gain", "about": "Turns the sound up or down.",
        "controls": [{ "name": "Gain", "type": "number", "min": -24, "max": 24, "default": 0, "unit": "dB" }],
        "patch": "left = [*~ 1.]\nright = [*~ 1.]\nin.0 -> left -> out.0\nin.1 -> right -> out.1\n\"Gain\" -> [dbtoa] -> both\nboth = [t f f]\nboth.0 -> left.1\nboth.1 -> right.1\n",
    });
    let made: Value = serde_json::from_str(&tool.execute(gain.as_object().unwrap().clone(), Signal::new()).await.unwrap().text).unwrap();
    assert_eq!(made["type"], "audio effect");
    assert_eq!(made["controls"], json!(["Gain (-24–24 dB; 0)", "Mix (0–100 %; 100)", "Output (-36–12 dB; 0)"]));
    assert!(made["checks"].as_str().unwrap().contains("laid it out top to bottom"), "{made}");
    assert_eq!(
        decode_amxd(&std::fs::read(folder.path().join("Kumi/Audio Effects/Gain.amxd")).unwrap()).unwrap().kind,
        DeviceType::AudioEffect
    );
    let mut unwired = gain.clone();
    unwired["patch"] = json!("in.0 -> [*~ 1.] -> out.0\nin.1 -> [*~ 1.] -> out.1\n");
    unwired["tests"] = json!([{ "name": "x", "input": [], "expect": [] }]);
    let made = tool.execute(unwired.as_object().unwrap().clone(), Signal::new()).await.unwrap();
    assert!(!made.is_error, "{}", made.text);
    let made: Value = serde_json::from_str(&made.text).unwrap();
    assert!(made["changed"][0].as_str().unwrap().contains("tests: they run a MIDI effect's code"), "{made}");
    assert!(made["wrong"][0].as_str().unwrap().contains("\"Gain\" isn't wired"), "{made}");
    assert_eq!(made["itemId"], "user_library/Kumi/Audio Effects/Gain", "made again under its name, in place of the first");
}

#[test]
fn kumis_own_devices_keep_its_standard_for_max_patchers() {
    use kumi_daw_ableton::devices::patch::check::check;
    use kumi_daw_ableton::devices::patch::standard::Level;
    use kumi_daw_ableton::devices::patch::{NoFiles, Patcher};
    let midi = midi_device_patcher(&spec(
        json!({ "controls": [{ "name": "Window", "type": "number", "min": 1, "max": 50, "default": 15, "unit": "ms" }, { "name": "Mode", "type": "choice", "options": ["Lowest", "Highest"], "default": "Lowest" }, { "name": "Bypass Drums", "type": "switch", "default": false }] }),
    ));
    let devices = [
        ("MIDI effect", midi),
        ("audio effect", audio_effect_patcher(&gen_spec(&grit()))),
        ("instrument", instrument_patcher(&gen_spec(&pluck()))),
    ];
    for (kind, patcher) in devices {
        let report = check(&Patcher::read(&patcher, &NoFiles));
        let broken: Vec<String> = report.at_least(Level::Warn).map(ToString::to_string).collect();
        assert!(broken.is_empty(), "the {kind} breaks {} rules:\n{}", broken.len(), broken.join("\n"));
    }
}
