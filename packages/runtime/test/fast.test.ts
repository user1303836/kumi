import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import type { JsonObject } from "../src/core/contracts.js";
import { PYTHON_BRIDGE } from "../src/integrations/ableton/bridge-version.js";
import { findScript, revertScript, setScript } from "../src/integrations/ableton/fast.js";
import { opened, signal, tool } from "./fixtures/synthetic-bridge.js";

/** A knob in the fake Live: its range, its steps, and how Live shows a value. */
interface Knob { name: string; value: number; min: number; max: number; quantized?: boolean; items?: string[]; enabled?: boolean; shows: (value: number) => string }
const db = (value: number) => `${(-36 + 48 * value).toFixed(1)} dB`;
const hz = (value: number) => { const at = 20 * 1000 ** value; return at >= 1000 ? `${(at / 1000).toFixed(2)} kHz` : `${at.toFixed(0)} Hz`; };

/**
 * The fixture's Operator ("7:device:0:0" on "7:track:0"), as Kumi's fast scripts see it: each script is
 * told apart by its first line and read for its arguments, and does here what it does in Live.
 */
function fakeLive() {
  const knobs: Knob[] = [
    { name: "Osc-A Level", value: 0.75, min: 0, max: 1, shows: db },
    { name: "Filter Freq", value: 0.5, min: 0, max: 1, shows: hz },
    { name: "Ae Release", value: 0.2, min: 0, max: 1, shows: (value) => `${(value * 10).toFixed(2)} s` },
    { name: "Filter Type", value: 0, min: 0, max: 3, quantized: true, items: ["Low", "High", "Band", "Notch"], shows: (value) => ["Low", "High", "Band", "Notch"][Math.round(value)]! },
    { name: "Spread", value: 0, min: 0, max: 1, enabled: false, shows: (value) => `${Math.round(value * 100)} %` },
  ];
  const scripts: string[] = [];
  const find = (target: JsonObject): Knob => {
    if (typeof target.ref === "string") { const index = Number(target.ref.split(":").at(-1)); return knobs[index]!; }
    const knob = knobs[target.index as number];
    if (!knob || knob.name !== target.name) throw new Error(`the device changed: its parameter ${String(target.index)} is now ${knob?.name}`);
    return knob;
  };
  const python = (args: JsonObject): JsonObject => {
    const code = String(args.code);
    const marker = /^# kumi:(fast-[a-z]+)/.exec(code)?.[1];
    if (!marker) return { ok: false, result: null, stdout: "", error: { type: "ValueError", message: "not a fast script", traceback: "" } };
    scripts.push(marker);
    const given = JSON.parse(JSON.parse(/^ARGS = json\.loads\((.*)\)$/m.exec(code)![1]!) as string) as JsonObject[];
    try {
      if (marker === "fast-find") return { ok: true, stdout: "", error: null, result: given.map((target) => {
        const index = typeof target.ref === "string" ? undefined : knobs.findIndex((knob) => knob.name.toLowerCase() === String(target.parameter).toLowerCase()) >= 0
          ? knobs.findIndex((knob) => knob.name.toLowerCase() === String(target.parameter).toLowerCase()) : knobs.findIndex((knob) => knob.name.toLowerCase().startsWith(String(target.parameter).toLowerCase()));
        if (index === -1) return { missing: knobs.map((knob) => knob.name) };
        const knob = index === undefined ? find(target) : knobs[index]!;
        return { name: knob.name, min: knob.min, max: knob.max, ...(index !== undefined ? { index } : {}),
          ...(target.map ? { items: knob.items ?? [], grid: Array.from({ length: 129 }, (_, i) => { const value = knob.min + (knob.max - knob.min) * i / 128; return [value, knob.shows(value)]; }) } : {}) };
      }) };
      if (marker === "fast-set") {
        const found = given.map((target) => { const knob = find(target); if (knob.enabled === false) throw new Error(`${knob.name} is greyed out in Live right now`); return { knob, value: target.value as number }; });
        const items = found.map(({ knob, value }) => {
          const prior = knob.value; const held = Math.min(knob.max, Math.max(knob.min, value));
          knob.value = knob.quantized ? Math.round(held) : held;
          return { name: knob.name, prior, priorDisplay: knob.shows(prior), min: knob.min, max: knob.max, value: knob.value, display: knob.shows(knob.value) };
        });
        return { ok: true, stdout: "", error: null, result: { device: "Operator", track: { ref: "7:track:0", type: "Track", name: "Fixture Bass" }, items } };
      }
      let back = 0; const moved: string[] = [];
      for (const target of [...given].reverse()) {
        const knob = find(target);
        if (Math.abs(knob.value - (target.applied as number)) > 1e-6) { moved.push(knob.name); continue; }
        knob.value = target.prior as number; back++;
      }
      return { ok: true, stdout: "", error: null, result: { back, moved, gone: [] } };
    } catch (error) { return { ok: false, result: null, stdout: "", error: { type: "ValueError", message: (error as Error).message, traceback: "" } }; }
  };
  return { knobs, scripts, python, knob: (name: string) => knobs.find((knob) => knob.name === name)! };
}

test("a device's parameters are set in one trip into Live, named and as Live shows them, and HISTORY says it as ever", async () => {
  const live = fakeLive();
  const b = await opened({ version: PYTHON_BRIDGE, parameters: true, python: live.python });
  try {
    const result = await tool(b.tools, "set_device_parameter").execute({ deviceRef: "device:1", parameter: "Osc-A Level", value: "-6 dB" }, signal());
    assert.equal(result.isError, false, result.text);
    assert.ok(Math.abs(live.knob("Osc-A Level").value - 30 / 48) < 0.005, "-6 dB, placed from Live's own text");
    // One trip to find it and read its text, one to set it; none of the bridge's preview and apply.
    assert.deepEqual(live.scripts, ["fast-find", "fast-set"]);
    assert.equal(b.requests.some((request) => /^live_device_parameter_(preview|apply)$/.test(request.name)), false);
    assert.equal(b.records.length, 1);
    // Live's text, its number and unit held together by a no-break space.
    assert.equal(b.records[0]!.title, "Operator · Osc-A Level 0.0 dB → -6.0 dB");
    assert.equal(b.records[0]!.state, "applied");
    assert.equal(b.records[0]!.track?.name, "Fixture Bass");
    // Found once: the same knob again is one trip.
    const again = await tool(b.tools, "set_device_parameter").execute({ deviceRef: "device:1", parameter: "osc-a level", value: "-12 dB" }, signal());
    assert.equal(again.isError, false, again.text);
    assert.deepEqual(live.scripts, ["fast-find", "fast-set", "fast-set"]);
    // A stepped parameter by the name of its step, and a number as it is.
    const stepped = await tool(b.tools, "set_device_parameter").execute({ deviceRef: "device:1", parameter: "Filter Type", value: "Band" }, signal());
    assert.equal(stepped.isError, false, stepped.text);
    assert.equal(live.knob("Filter Type").value, 2);
    const plain = await tool(b.tools, "set_device_parameter").execute({ deviceRef: "device:1", parameter: "Ae Release", value: 0.4 }, signal());
    assert.equal(plain.isError, false, plain.text);
    assert.equal(live.knob("Ae Release").value, 0.4);
  } finally { await b.integration.close(); }
});

test("several parameters of a device in a plan are one change in one trip, and its undo puts them back", async () => {
  const live = fakeLive();
  const b = await opened({ version: PYTHON_BRIDGE, parameters: true, fullControl: true, python: live.python });
  try {
    const plan = await tool(b.tools, "make_changes").execute({ steps: [
      { tool: "set_device_parameter", input: { deviceRef: "device:1", parameter: "Filter Freq", value: "800 Hz" } },
      { tool: "set_device_parameter", input: { deviceRef: "device:1", parameter: "Ae Release", value: "1.5 s" } },
    ] }, signal());
    assert.equal(plan.isError, false, plan.text);
    assert.deepEqual(live.scripts, ["fast-find", "fast-set"], "both found in one trip, both set in another");
    assert.ok(Math.abs(20 * 1000 ** live.knob("Filter Freq").value - 800) < 8);
    assert.ok(Math.abs(live.knob("Ae Release").value - 0.15) < 0.002);
    assert.equal(b.records.length, 1);
    assert.equal(b.records[0]!.title, "Operator · 2 parameters");
    const undone = await tool(b.tools, "undo_change").execute({ change: b.records[0]!.id }, signal());
    assert.equal(undone.isError, false, undone.text);
    assert.equal(live.knob("Filter Freq").value, 0.5);
    assert.equal(live.knob("Ae Release").value, 0.2);
    assert.equal(b.records.at(-1)!.state, "undone");
  } finally { await b.integration.close(); }
});

test("undo leaves a parameter the producer moved since, and says which", async () => {
  const live = fakeLive();
  const b = await opened({ version: PYTHON_BRIDGE, parameters: true, fullControl: true, python: live.python });
  try {
    const plan = await tool(b.tools, "make_changes").execute({ steps: [
      { tool: "set_device_parameter", input: { deviceRef: "device:1", parameter: "Filter Freq", value: 0.9 } },
      { tool: "set_device_parameter", input: { deviceRef: "device:1", parameter: "Ae Release", value: 0.6 } },
    ] }, signal());
    assert.equal(plan.isError, false, plan.text);
    // The producer turns Ae Release in Live.
    live.knob("Ae Release").value = 0.33;
    const undone = await tool(b.tools, "undo_change").execute({ change: b.records[0]!.id }, signal());
    assert.equal(undone.isError, true);
    assert.match(undone.text, /put back 1 of its parameters; Ae Release changed in Live since/);
    assert.equal(live.knob("Filter Freq").value, 0.5, "what Kumi left as it was goes back");
    assert.equal(live.knob("Ae Release").value, 0.33, "what the producer moved stays");
    assert.equal(b.records.at(-1)!.state, "kept");
  } finally { await b.integration.close(); }
});

test("a parameter the device doesn't have, or one Live greys out, changes nothing and says why", async () => {
  const live = fakeLive();
  const b = await opened({ version: PYTHON_BRIDGE, parameters: true, python: live.python });
  try {
    const missing = await tool(b.tools, "set_device_parameter").execute({ deviceRef: "device:1", parameter: "Wobble", value: 0.5 }, signal());
    assert.equal(missing.isError, true);
    assert.match(missing.text, /no parameter called "Wobble"; its parameters include Osc-A Level, Filter Freq/);
    const greyed = await tool(b.tools, "set_device_parameter").execute({ deviceRef: "device:1", parameter: "Spread", value: 0.5 }, signal());
    assert.equal(greyed.isError, true);
    assert.match(greyed.text, /Spread is greyed out/);
    const unplaced = await tool(b.tools, "set_device_parameter").execute({ deviceRef: "device:1", parameter: "Filter Freq", value: "very bright" }, signal());
    assert.equal(unplaced.isError, true);
    assert.deepEqual(b.records, []);
    assert.equal(live.knob("Filter Freq").value, 0.5);
  } finally { await b.integration.close(); }
});

test("without Python in the bridge, or with fast changes off, parameters go through the bridge's preview and apply", async () => {
  for (const options of [{ version: "1.0.67" }, { version: PYTHON_BRIDGE, fast: false }]) {
    const live = fakeLive();
    const b = await opened({ ...options, parameters: true, python: live.python });
    try {
      await tool(b.tools, "live_discover").execute({ kind: "parameter", parent: "device:1" }, signal());
      const result = await tool(b.tools, "set_device_parameter").execute({ deviceRef: "device:1", parameterRef: "parameter:1", value: 0.3 }, signal());
      assert.equal(result.isError, false, result.text);
      assert.deepEqual(live.scripts, []);
      assert.ok(b.requests.some((request) => request.name === "live_device_parameter_apply"));
    } finally { await b.integration.close(); }
  }
});

/** Python 3 on this computer, or none (the scripts' own test is then skipped). */
const python = ["python3", "python"].find((command) => { const run = spawnSync(command, ["--version"], { encoding: "utf8" }); return run.status === 0 && /Python 3/.test(`${run.stdout}${run.stderr}`); });

test("the scripts themselves, in Python, against a Live made of plain objects", { skip: python ? false : "no Python 3 here" }, () => {
  // Live's objects as the scripts use them: a track, a device on it, its parameters; references by name.
  const live = `
import json
class Track:
    def __init__(self, name): self.name, self.canonical_parent = name, None
class Device:
    def __init__(self, name, parameters, parent):
        self.name, self.parameters, self.canonical_parent = name, parameters, parent
        for p in parameters: p.canonical_parent = self
class Parameter:
    def __init__(self, name, value, lo, hi, quantized=False, items=(), enabled=True):
        self.name, self._value, self.min, self.max, self.is_quantized, self.value_items, self.is_enabled = name, value, lo, hi, quantized, list(items), enabled
    @property
    def value(self): return self._value
    @value.setter
    def value(self, v):
        if v < self.min or v > self.max: raise ValueError('out of range')
        if self.name == 'Locked': raise RuntimeError('Live refused')
        self._value = v
    def str_for_value(self, v): return self.value_items[int(v)] if self.is_quantized else '%.1f dB' % (-36 + 48 * v)
class Refs:
    def __init__(self, objects): self.objects = objects
    def get(self, ref):
        if ref not in self.objects: raise KeyError('stale or invalid reference')
        return self.objects[ref]
class Bridge:
    def __init__(self, refs): self.refs = refs
track = Track('Bass')
device = Device('Saturator', [Parameter('Drive', 0.75, 0.0, 1.0), Parameter('Type', 0, 0, 3, True, ['Analog', 'Soft', 'Medium', 'Hard']), Parameter('Locked', 0.5, 0.0, 1.0)], track)
bridge = Bridge(Refs({'7:device:0:0': device, '7:parameter:0:0:1': device.parameters[1]}))
def plain(value):
    if isinstance(value, (Track, Device, Parameter)): return {'ref': '?', 'type': type(value).__name__, 'name': value.name}
    raise TypeError(type(value).__name__)
out = []
for code in json.loads(CODES):
    env = {'bridge': bridge, 'result': None}
    try:
        exec(compile(code, '<python.run>', 'exec'), env, env)
        out.append({'ok': True, 'result': json.loads(json.dumps(env['result'], default=plain)), 'drive': device.parameters[0].value, 'type': device.parameters[1].value})
    except Exception as error:
        out.append({'ok': False, 'error': str(error), 'drive': device.parameters[0].value, 'type': device.parameters[1].value})
print(json.dumps(out))
`;
  const codes = [
    findScript([{ device: "7:device:0:0", parameter: "dri", map: true }, { ref: "7:parameter:0:0:1", map: false }, { device: "7:device:0:0", parameter: "Wobble", map: false }]),
    setScript([{ device: "7:device:0:0", index: 0, name: "Drive", value: 2 }, { ref: "7:parameter:0:0:1", value: 1.6 }]),
    revertScript([{ device: "7:device:0:0", index: 0, name: "Drive", prior: 0.75, applied: 1 }, { ref: "7:parameter:0:0:1", prior: 0, applied: 2 }]),
    // All or nothing: Live refuses the second, so the first goes back.
    setScript([{ device: "7:device:0:0", index: 0, name: "Drive", value: 0.25 }, { device: "7:device:0:0", index: 2, name: "Locked", value: 0.9 }]),
    // A device changed under its place: the name no longer matches.
    setScript([{ device: "7:device:0:0", index: 1, name: "Drive", value: 0.3 }]),
    setScript([{ ref: "8:parameter:0:0:1", value: 1 }]),
  ];
  const run = spawnSync(python!, ["-"], { input: `CODES = ${JSON.stringify(JSON.stringify(codes))}\n${live}`, encoding: "utf8" });
  assert.equal(run.status, 0, run.stderr);
  const [found, set, reverted, refused, moved, stale] = JSON.parse(run.stdout) as JsonObject[];
  const rows = found!.result as JsonObject[];
  assert.equal(rows[0]!.name, "Drive"); assert.equal(rows[0]!.index, 0); assert.equal((rows[0]!.grid as unknown[]).length, 129);
  assert.deepEqual((rows[0]!.grid as [number, string][])[64], [0.5, "-12.0 dB"]);
  assert.equal(rows[1]!.name, "Type"); assert.equal(rows[1]!.index, undefined);
  assert.deepEqual(rows[2]!.missing, ["Drive", "Type", "Locked"]);
  // Held within its range, and on a step.
  const result = set!.result as JsonObject;
  assert.equal(result.device, "Saturator"); assert.deepEqual(result.track, { ref: "?", type: "Track", name: "Bass" });
  assert.deepEqual((result.items as JsonObject[]).map((item) => [item.name, item.prior, item.value, item.priorDisplay, item.display]), [["Drive", 0.75, 1, "0.0 dB", "12.0 dB"], ["Type", 0, 2, "Analog", "Medium"]]);
  assert.equal(set!.drive, 1); assert.equal(set!.type, 2);
  assert.deepEqual(reverted!.result, { back: 2, moved: [], gone: [] }); assert.equal(reverted!.drive, 0.75); assert.equal(reverted!.type, 0);
  assert.equal(refused!.ok, false); assert.equal(refused!.drive, 0.75, "the first went back when the second was refused");
  assert.equal(moved!.ok, false); assert.match(String(moved!.error), /device changed: its parameter 1 is now Type/);
  assert.equal(stale!.ok, false); assert.match(String(stale!.error), /stale or invalid reference/);
});
