//! A MIDI effect: midiin → a v8.codebox holding Kumi's frame and the device's own code → midiout,
//! with each control a live.dial, live.menu or live.toggle (an ordinary Live parameter) feeding the
//! code. The frame parses MIDI into events, sends events back as bytes, keeps count of the notes the
//! device holds (so all-notes-off silences them), and runs timers; the device's code only decides.

use std::sync::LazyLock;

use kumi_common::js::json::stringify;
use regex::Regex;
use serde_json::{json, Map, Value};

use super::amxd::{device_patcher, face_layout, DevicePatcherOptions, DeviceType, Line, PatchBox};
use super::gen::{extend, face_control};
use super::spec::{Control, MidiSpec, NumericControl, Unit};

/// The frame, with __CONTROLS__ and __DEFAULTS__ for the device's controls and __CODE__ for its code.
const FRAME: &str = r##"// Made by Kumi. The frame (fixed) runs the device's own code, below it.
inlets = 2;
outlets = 1;
const CONTROLS = __CONTROLS__;
const params = __DEFAULTS__;
const held = new Set();
const timers = new Set();
let status = 0;
let data = [];
function now() { return Date.now(); }
function write(bytes) { for (const byte of bytes) outlet(0, byte); }
function safely(fn, args) { try { return fn.apply(undefined, args); } catch (error) { post("Kumi device: " + ((error && error.message) || error) + "\n"); } }
function whole(value, low, high) { value = Math.round(Number(value)); return Number.isFinite(value) ? Math.min(high, Math.max(low, value)) : low; }
function send(event) {
  if (!event || typeof event !== "object") return;
  const channel = whole(event.channel === undefined ? 1 : event.channel, 1, 16) - 1;
  switch (event.type) {
    case "noteon": {
      const pitch = whole(event.pitch, 0, 127); const velocity = whole(event.velocity === undefined ? 100 : event.velocity, 0, 127);
      if (velocity === 0) { send({ type: "noteoff", pitch: pitch, channel: channel + 1 }); return; }
      held.add(channel * 128 + pitch); write([0x90 | channel, pitch, velocity]); return;
    }
    case "noteoff": { const pitch = whole(event.pitch, 0, 127); held.delete(channel * 128 + pitch); write([0x80 | channel, pitch, whole(event.velocity || 0, 0, 127)]); return; }
    case "cc": write([0xB0 | channel, whole(event.controller, 0, 127), whole(event.value, 0, 127)]); return;
    case "pitchbend": { const value = whole(event.value === undefined ? 8192 : event.value, 0, 16383); write([0xE0 | channel, value & 127, value >> 7]); return; }
    case "aftertouch": write([0xD0 | channel, whole(event.value, 0, 127)]); return;
    case "polytouch": write([0xA0 | channel, whole(event.pitch, 0, 127), whole(event.value, 0, 127)]); return;
    case "program": write([0xC0 | channel, whole(event.value, 0, 127)]); return;
  }
}
const pass = send;
function after(ms, fn) {
  const task = new Task(function () { timers.delete(task); safely(fn, []); });
  timers.add(task); task.schedule(Math.max(0, Number(ms) || 0)); return task;
}
function cancel(task) { if (task && timers.has(task)) { task.cancel(); timers.delete(task); } }
const device = (function () {
  "use strict";
  // Out of the device's reach: files, the network, and the rest of Max and Live.
  const File = undefined, Folder = undefined, XMLHttpRequest = undefined, fetch = undefined, SQLite = undefined, Dict = undefined, Buffer = undefined, Global = undefined,
    LiveAPI = undefined, messnamed = undefined, max = undefined, patcher = undefined, globalThis = undefined, outlet = undefined, Task = undefined, require = undefined;
  return (function () {
__CODE__
    return { midi: typeof midi === "function" ? midi : undefined, changed: typeof changed === "function" ? changed : undefined, reset: typeof reset === "function" ? reset : undefined };
  })();
})();
function panic() {
  for (const task of timers) task.cancel();
  timers.clear();
  if (device.reset) safely(device.reset, []);
  for (const key of held) write([0x80 | (key >> 7), key & 127, 0]);
  held.clear();
}
function receive(event) {
  // All notes off and all sound off: the device forgets, what it holds goes off, and the message passes on.
  if (event.type === "cc" && (event.controller === 123 || event.controller === 120)) { panic(); send(event); return; }
  if (device.midi) safely(device.midi, [event]); else send(event);
}
function parse(byte) {
  if (byte >= 0xF8) { write([byte]); return; }
  if (byte >= 0xF0) { status = byte === 0xF0 ? 0xF0 : 0; data = []; write([byte]); return; }
  if (byte >= 0x80) { status = byte; data = []; return; }
  if (status === 0xF0 || !status) { write([byte]); return; }
  data.push(byte);
  const kind = status & 0xF0; const channel = (status & 0x0F) + 1;
  if (data.length < (kind === 0xC0 || kind === 0xD0 ? 1 : 2)) return;
  const a = data[0]; const b = data[1]; data = [];
  const time = now();
  if (kind === 0x90 && b > 0) receive({ type: "noteon", channel: channel, pitch: a, velocity: b, time: time });
  else if (kind === 0x80 || kind === 0x90) receive({ type: "noteoff", channel: channel, pitch: a, velocity: kind === 0x80 ? b : 0, time: time });
  else if (kind === 0xB0) receive({ type: "cc", channel: channel, controller: a, value: b, time: time });
  else if (kind === 0xE0) receive({ type: "pitchbend", channel: channel, value: a | (b << 7), time: time });
  else if (kind === 0xD0) receive({ type: "aftertouch", channel: channel, value: a, time: time });
  else if (kind === 0xA0) receive({ type: "polytouch", channel: channel, pitch: a, value: b, time: time });
  else if (kind === 0xC0) receive({ type: "program", channel: channel, value: a, time: time });
}
function msg_int(value) { if (inlet === 0) parse(value); }
function msg_float(value) { if (inlet === 0) parse(Math.round(value)); }
function anything() {
  if (inlet !== 1) return;
  const knob = CONTROLS.find(function (item) { return item.id === messagename; });
  if (!knob) return;
  let value = arguments[0];
  if (knob.options) value = knob.options[whole(value, 0, knob.options.length - 1)];
  else if (knob.toggle) value = Number(value) > 0;
  params[knob.name] = value;
  if (device.changed) safely(device.changed, [knob.name, value]);
}
"##;

static LINE_BREAKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r?\n").unwrap());

/// The code the device's v8.codebox runs: the frame around the model's code.
pub fn midi_device_code(controls: &[Control], code: &str) -> String {
    let listed: Vec<Value> = controls
        .iter()
        .enumerate()
        .map(|(index, control)| {
            let mut item = json!({ "id": format!("c{}", index + 1), "name": control.name() });
            if let Control::Choice(choice) = control {
                extend(&mut item, json!({ "options": choice.options }));
            }
            if let Control::Switch(_) = control {
                extend(&mut item, json!({ "toggle": true }));
            }
            item
        })
        .collect();
    let mut defaults = Map::new();
    for control in controls {
        defaults.insert(control.name().to_string(), control.default_value());
    }
    // Replaced one after the other, and the code last, so nothing in it can be taken for a placeholder.
    let indented = LINE_BREAKS.split(code).map(|line| format!("    {line}")).collect::<Vec<_>>().join("\n");
    FRAME
        .replacen("__CONTROLS__", &stringify(&Value::Array(listed)), 1)
        .replacen("__DEFAULTS__", &stringify(&Value::Object(defaults)), 1)
        .replacen("__CODE__", &indented, 1)
}

/// Live's display style for a unit, from [`unit_style`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitStyle {
    pub style: u32,
    pub units: Option<String>,
}

/// Live's display style for a unit (live.dial's parameter_unitstyle), and the text for a custom one.
/// Live's time style reads the value as milliseconds, so seconds get their own label.
pub fn unit_style(control: &NumericControl, integer: bool) -> UnitStyle {
    match control.unit {
        Unit::Ms => UnitStyle { style: 2, units: None },
        Unit::S => UnitStyle { style: 9, units: Some("s".into()) },
        Unit::Hz => UnitStyle { style: 3, units: None },
        Unit::Db => UnitStyle { style: 4, units: None },
        Unit::Percent => UnitStyle { style: 5, units: None },
        Unit::Pan => UnitStyle { style: 6, units: None },
        Unit::St => UnitStyle { style: 7, units: None },
        Unit::Note => UnitStyle { style: 8, units: None },
        Unit::Beats | Unit::Bpm | Unit::X => UnitStyle { style: 9, units: Some(control.unit.as_str().into()) },
        Unit::None => UnitStyle { style: if integer { 0 } else { 1 }, units: None },
    }
}

/// The patcher of a MIDI effect for `spec`: its face shows the controls in a row, or in rows past eight.
pub fn midi_device_patcher(spec: &MidiSpec) -> Value {
    let mut boxes: Vec<PatchBox> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    fn text(boxes: &mut Vec<PatchBox>, id: &str, content: &str, rect: [f64; 4], extra: Value) {
        let mut item =
            json!({ "id": id, "maxclass": "newobj", "text": content, "fontname": "Arial Bold", "fontsize": 10.0, "patching_rect": rect });
        extend(&mut item, extra);
        boxes.push(PatchBox::new(item));
    }
    text(&mut boxes, "obj-midiin", "midiin", [40.0, 30.0, 40.0, 20.0], json!({ "numinlets": 1, "numoutlets": 1, "outlettype": ["int"] }));
    text(&mut boxes, "obj-midiout", "midiout", [40.0, 420.0, 47.0, 20.0], json!({ "numinlets": 1, "numoutlets": 0 }));
    boxes.push(PatchBox::new(json!({ "id": "obj-code", "maxclass": "v8.codebox", "filename": "none", "code": midi_device_code(&spec.controls, &spec.code), "fontface": 0, "fontname": "Menlo", "fontsize": 11.0,
        "numinlets": 2, "numoutlets": 1, "outlettype": [""], "patching_rect": [40.0, 120.0, 520.0, 280.0], "saved_object_attributes": { "parameter_enable": 0 } })));
    lines.push(Line::new("obj-midiin", 0, "obj-code", 0));
    lines.push(Line::new("obj-code", 0, "obj-midiout", 0));
    let face = face_layout(spec.controls.len());
    for (index, control) in spec.controls.iter().enumerate() {
        let id = format!("obj-control-{}", index + 1);
        let (x, y) = face.at(index);
        let (item, valueof) = face_control(control, x, y);
        let mut item_box = json!({ "id": id, "varname": control.name(), "parameter_enable": 1, "presentation": 1, "patching_rect": [600.0 + index as f64 * 60.0, 30.0, 44.0, 48.0] });
        extend(&mut item_box, item);
        extend(&mut item_box, json!({ "saved_attribute_attributes": { "valueof": valueof } }));
        boxes.push(PatchBox::new(item_box));
        let prepend = format!("obj-prepend-{}", index + 1);
        text(
            &mut boxes,
            &prepend,
            &format!("prepend c{}", index + 1),
            [600.0 + index as f64 * 60.0, 90.0, 60.0, 20.0],
            json!({ "numinlets": 1, "numoutlets": 1, "outlettype": [""] }),
        );
        lines.push(Line::new(&id, 0, &prepend, 0));
        lines.push(Line::new(&prepend, 0, "obj-code", 1));
    }
    device_patcher(
        DeviceType::MidiEffect,
        DevicePatcherOptions {
            title: spec.name.clone(),
            description: spec.about.clone(),
            width: (16 + face.columns * 52).max(120) as f64,
            boxes,
            lines,
        },
    )
}
