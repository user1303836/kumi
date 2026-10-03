import assert from "node:assert/strict";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { parseDisplay, valueForDisplay, type DisplayMap } from "../src/integrations/ableton/display.js";
import { framesFromAudio, framesFromKeyframes, FRAME, periodOf, shapeHarmonics, synthesize, writeWavetable } from "../src/audio/wavetable.js";
import { openAudio } from "../src/audio/decode.js";
import { opened, signal, tool } from "./fixtures/synthetic-bridge.js";
import { saw, wav } from "./fixtures/synthetic-audio.js";

test("a device's text is read as numbers in its own units", () => {
  assert.deepEqual(parseDisplay("812 Hz"), { value: 812, unit: "hz" });
  assert.deepEqual(parseDisplay("1.20 kHz"), { value: 1200, unit: "hz" });
  assert.deepEqual(parseDisplay("-6.0 dB"), { value: -6, unit: "db" });
  assert.deepEqual(parseDisplay("−inf dB"), { value: -Infinity, unit: "db" });
  assert.deepEqual(parseDisplay("120 ms"), { value: 0.12, unit: "s" });
  assert.deepEqual(parseDisplay("35 %"), { value: 35, unit: "%" });
  assert.deepEqual(parseDisplay("4.0:1"), { value: 4, unit: "ratio" });
  assert.equal(parseDisplay("Saw"), undefined);
});

// A cutoff that runs 20 Hz to 20 kHz on a log scale over 0–1, as Live shows it; a menu of shapes.
const cutoff: DisplayMap = { min: 0, max: 1, grid: Array.from({ length: 129 }, (_, i) => { const v = i / 128; const hz = 20 * 1000 ** v; return [v, hz >= 1000 ? `${(hz / 1000).toFixed(2)} kHz` : `${hz.toFixed(0)} Hz`] as [number, string]; }) };
const gain: DisplayMap = { min: 0, max: 1, grid: Array.from({ length: 129 }, (_, i) => [i / 128, `${(-36 + 48 * (i / 128)).toFixed(1)} dB`] as [number, string]) };
const shape: DisplayMap = { min: 0, max: 3, items: ["Sine", "Saw", "Square", "Noise"], grid: [[0, "Sine"], [1, "Saw"], [2, "Square"], [3, "Noise"]] };

test("a value is placed from what the device shows: by ratio for frequencies and times, evenly for decibels, by name for steps", () => {
  const at800 = valueForDisplay(cutoff, "800 Hz") as number;
  assert.ok(Math.abs(20 * 1000 ** at800 - 800) < 8, `800 Hz lands at ${20 * 1000 ** at800}`);
  assert.ok(Math.abs((valueForDisplay(cutoff, "2.5 kHz") as number) - Math.log(2500 / 20) / Math.log(1000)) < 0.005);
  assert.ok(Math.abs((valueForDisplay(gain, "-6 dB") as number) - 30 / 48) < 0.005);
  assert.equal(valueForDisplay(shape, "Square"), 2);
  assert.equal(valueForDisplay(shape, "sq"), 2, "a name's start will do");
  assert.equal(valueForDisplay(gain, 0.25), 0.25, "a number passes as it is");
  assert.equal(valueForDisplay(gain, "0.5"), 0.5);
  assert.equal(valueForDisplay(cutoff, "40 kHz"), 1, "past the end, the nearest end");
  assert.match(String(valueForDisplay(cutoff, "loud")), /isn't a value Kumi can place/);
  assert.match(String(valueForDisplay(gain, "300 Hz")), /doesn't show values in hz/);
});

test("set_device_parameter takes what the device shows, read once from Live's own text", async () => {
  // Through the bridge's preview and apply; fast.test.ts has the fast way.
  const b = await opened({ version: "1.0.70", parameters: true, fast: false, python: (args) => ({ ok: true, result: { min: 0, max: 1, items: [], grid: cutoff.grid }, stdout: "", ref: args.ref }) });
  try {
    await tool(b.tools, "live_discover").execute({ kind: "parameter", parent: "device:1" }, signal());
    const knob = { ref: "parameter:1" };
    const result = await tool(b.tools, "set_device_parameter").execute({ deviceRef: "device:1", parameterRef: knob.ref, value: "800 Hz" }, signal());
    assert.equal(result.isError, false, result.text);
    const sent = b.requests.filter((request) => request.name === "live_device_parameter_preview").at(-1)!.args;
    assert.ok(Math.abs(20 * 1000 ** (sent.value as number) - 800) < 8, `the bridge was sent ${String(sent.value)}`);
    assert.equal(b.requests.filter((request) => request.name === "live_run_python").length, 1);
    // The map is read once.
    await tool(b.tools, "set_device_parameter").execute({ deviceRef: "device:1", parameterRef: knob.ref, value: "1.2 kHz" }, signal());
    assert.equal(b.requests.filter((request) => request.name === "live_run_python").length, 1);
    const refused = await tool(b.tools, "set_device_parameter").execute({ deviceRef: "device:1", parameterRef: knob.ref, value: "very bright" }, signal());
    assert.equal(refused.isError, true);
  } finally { await b.integration.close(); }
});

test("wavetables: shapes as harmonics, keyframes morphing by harmonics, cycles cut from a sound, and Serum's frame marker", async () => {
  assert.deepEqual(shapeHarmonics("square", 0.5, 5).map((value) => Math.round(value * 1000) / 1000), [1, 0, 0.333, 0, 0.2]);
  assert.equal(shapeHarmonics("triangle", 0.5, 3)[2]! < 0, true, "a triangle's harmonics alternate");
  const sine = synthesize([1]);
  assert.equal(sine.length, FRAME);
  assert.ok(Math.abs(sine[FRAME / 4]! - 1) < 1e-6);
  const frames = framesFromKeyframes([{ shape: "sine" }, { shape: "saw" }], 16);
  assert.equal(frames.length, 16);
  assert.ok(Math.max(...frames.flatMap((frame) => [...frame].map(Math.abs))) <= 0.99 + 1e-6, "normalized together");
  const folder = mkdtempSync(join(tmpdir(), "kumi-wavetable-"));
  const file = join(folder, "Kumi Sweep.wav");
  await writeWavetable(file, frames);
  const bytes = readFileSync(file);
  assert.ok(bytes.includes(Buffer.from("clm ")), "Serum's chunk is there");
  assert.ok(bytes.includes(Buffer.from(`<!>${FRAME}`)), "with the frame size");
  const source = await openAudio(file);
  try { assert.equal(source.frames, 16 * FRAME); assert.equal(source.channels, 1); } finally { await source.close(); }
  // Cycles of a 110 Hz saw at 48 kHz are 436.4 samples long.
  const note = wav("wavetable-saw.wav", saw(2, 110));
  assert.ok(Math.abs(periodOf(saw(1, 110), 48000)! - 48000 / 110) < 1, "its period is heard");
  const cut = await framesFromAudio(note, 8);
  assert.equal(cut.length, 8);
  assert.ok(cut.every((frame) => frame.length === FRAME));
});
