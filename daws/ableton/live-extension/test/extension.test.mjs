// The built extension (dist/extension.js, as committed) against a fake Live, over its real socket.
import assert from "node:assert/strict";
import { createHash, createHmac, randomBytes } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { createConnection } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { fakeLive } from "./fake-live.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const bundle = join(here, "..", "dist", "extension.js");
const registryPath = join(here, "..", "..", "..", "protocol", "ableton-live-v1.operations.json");
const canonical = (value) => value === null || typeof value !== "object" ? JSON.stringify(Object.is(value, -0) ? 0 : value) : Array.isArray(value) ? `[${value.map(canonical).join(",")}]` : `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;

let root, storage, extension, live, secret, endpoint;

async function waitFor(check, ms = 5_000) {
  const until = Date.now() + ms;
  while (Date.now() < until) { const value = check(); if (value) return value; await new Promise((resolve) => setTimeout(resolve, 10)); }
  throw new Error("timed out");
}

/** A signed client, as the bridge host is. */
async function connect() {
  const socket = createConnection({ host: endpoint.host, port: endpoint.port });
  const frames = []; const events = []; let buffer = "";
  socket.on("data", (chunk) => {
    buffer += chunk; let index;
    while ((index = buffer.indexOf("\n")) >= 0) { const frame = JSON.parse(buffer.slice(0, index)); buffer = buffer.slice(index + 1); (frame.id === "event" ? events : frames).push(frame); }
  });
  const hello = await waitFor(() => frames.find((frame) => frame.id === "hello"));
  let sequence = 0;
  const mac = (payload) => createHmac("sha256", secret).update(canonical(payload)).digest("base64url");
  const send = async (fields, { sign = true, sequenceOverride, deadlineMs } = {}) => {
    const id = `t${++sequence}`;
    const unsigned = { version: "ableton-loopback/v1", id, ...fields, nonce: randomBytes(18).toString("base64url"), sequence: sequenceOverride ?? sequence, bridgeEpoch: hello.bridgeEpoch, connectionChallenge: hello.connectionChallenge, deadlineMs: deadlineMs ?? Date.now() + 30_000 };
    socket.write(`${JSON.stringify({ ...unsigned, mac: sign ? mac(unsigned) : "x".repeat(43) })}\n`);
    return waitFor(() => frames.find((frame) => frame.id === id));
  };
  const invoke = async (operation, args) => { const response = await send({ method: "invoke", operation, args }); if (!response.ok) throw new Error(response.error); return response.result; };
  return { socket, hello, frames, events, send, invoke, mac, close: () => socket.destroy() };
}

before(async () => {
  root = mkdtempSync(join(tmpdir(), "kumi-extension-test-"));
  storage = join(root, "storage");
  const fake = fakeLive({ storage, temp: join(root, "temp"), liveTemp: join(root, "live-temp") });
  live = fake.model;
  extension = createRequire(import.meta.url)(bundle);
  extension.activate(fake.activation);
  endpoint = await waitFor(() => existsSync(join(storage, "endpoint.json")) && JSON.parse(readFileSync(join(storage, "endpoint.json"), "utf8")));
  secret = readFileSync(join(storage, "secret"), "utf8").trim();
});

after(async () => { await extension.deactivate(); rmSync(root, { recursive: true, force: true }); });

test("the committed bundle is the one its checksum names, built from the current registry", async () => {
  const [recorded] = readFileSync(join(here, "..", "dist", "extension.js.sha256"), "utf8").split(/\s+/);
  assert.equal(createHash("sha256").update(readFileSync(bundle)).digest("hex"), recorded);
  const client = await connect();
  // A registry change needs the bundle rebuilt (node apps/live-extension/build.mjs, with the SDK in vendor/).
  assert.equal(client.hello.result.registryHash, createHash("sha256").update(canonical(JSON.parse(readFileSync(registryPath, "utf8")))).digest("hex"));
  client.close();
});

test("it writes where it listens, owner-only, and answers status with its operations", async () => {
  assert.equal(endpoint.host, "127.0.0.1"); assert.ok(endpoint.port > 0); assert.equal(endpoint.pid, process.pid);
  if (process.platform !== "win32") {
    assert.equal(statSync(join(storage, "endpoint.json")).mode & 0o777, 0o600);
    assert.equal(statSync(join(storage, "secret")).mode & 0o777, 0o600);
  }
  const client = await connect();
  const status = await client.send({ method: "status" });
  assert.equal(status.ok, true); assert.equal(status.result.adapter, "extension");
  assert.deepEqual(status.result.operations, ["status", "render.offline", "arrangement.midi-clip.create", "clip.clear-range", "device.duplicate", "drum-pad.sample-chain", "project.import", "transaction.group"]);
  // Answers are signed with the shared secret, as the Remote Script's are.
  const { mac, ...unsigned } = status;
  assert.equal(client.mac(unsigned), mac);
  client.close();
});

test("unsigned, replayed and unknown requests are refused", async () => {
  const client = await connect();
  assert.equal((await client.send({ method: "status" }, { sign: false })).error, "authentication or replay check failed");
  assert.equal((await client.send({ method: "status" })).ok, true);
  assert.equal((await client.send({ method: "status" }, { sequenceOverride: 1 })).error, "invalid request");
  assert.match((await client.send({ method: "invoke", operation: "tempo.set", args: {} })).error, /unavailable on the Extensions channel/);
  assert.match((await client.send({ method: "invoke", operation: "render.offline", args: { trackRef: "1:track:2" } })).error, /required by registry/);
  assert.match((await client.send({ method: "invoke", operation: "render.offline", args: { trackRef: "1:track:2", fromBeat: 0, toBeat: 4 } })).error, /expectedName is required/);
  client.close();
});

test("a line that isn't a request object is refused, and the host keeps serving", async () => {
  // Any local process can write to the port: `null` threw outside every catch and ended the whole host.
  const client = await connect();
  for (const line of ["null", "[]", "42", "\"text\""]) client.socket.write(`${line}\n`);
  const refused = await waitFor(() => { const found = client.frames.filter((frame) => frame.id === "invalid"); return found.length >= 4 && found; });
  assert.ok(refused.every((frame) => frame.ok === false));
  assert.equal((await client.send({ method: "status" })).ok, true);
  client.close();
});

test("until its first signed request a connection's lines stay small, and there are only so many", async () => {
  // Unsigned, a 70 KiB line closes the connection before anything parses it.
  const stranger = await connect();
  const closed = new Promise((resolve) => stranger.socket.once("close", resolve));
  stranger.socket.write(`${"x".repeat(70 * 1024)}\n`);
  await closed;
  // Signed in, a big line is read (and here answered as malformed), and the connection carries on.
  const client = await connect();
  assert.equal((await client.send({ method: "status" })).ok, true);
  client.socket.write(`${"x".repeat(200 * 1024)}\n`);
  await waitFor(() => client.frames.some((frame) => frame.id === "invalid" && frame.error === "malformed request"));
  assert.equal((await client.send({ method: "status" })).ok, true);
  client.close();
  // Connections past the limit are closed at once, without a hello.
  const sockets = [];
  let rejected = false;
  while (!rejected && sockets.length < 20) {
    const socket = createConnection({ host: endpoint.host, port: endpoint.port });
    sockets.push(socket);
    rejected = (await new Promise((resolve) => { socket.once("data", () => resolve("hello")); socket.once("close", () => resolve("closed")); })) === "closed";
  }
  assert.ok(rejected && sockets.length <= 17, `${sockets.length} connections, the last ${rejected ? "refused" : "taken"}`);
  for (const socket of sockets) socket.destroy();
  await new Promise((resolve) => setTimeout(resolve, 100));
  const again = await connect();
  assert.equal((await again.send({ method: "status" })).ok, true);
  again.close();
});

test("an Arrangement MIDI clip goes in with its notes, where the Remote Script can't write them", async () => {
  const client = await connect();
  const result = await client.invoke("arrangement.midi-clip.create", { trackRef: "7:track:0", start: 8, length: 4, name: "Kumi chord", expectedName: "Keys", notes: [{ pitch: 60, start: 0, duration: 2, velocity: 100 }, { pitch: 67, start: 2, duration: 1, mute: true, probability: 0.5 }] });
  assert.deepEqual(result, { ref: "7:arrangement_clip:0:0", trackRef: "7:track:0", name: "Kumi chord", start: 8, end: 12, notes: 2 });
  const clip = live.keys.arrangementClips[0];
  assert.deepEqual(clip.notes, [{ pitch: 60, startTime: 0, duration: 2, velocity: 100 }, { pitch: 67, startTime: 2, duration: 1, muted: true, probability: 0.5 }]);
  await assert.rejects(client.invoke("arrangement.midi-clip.create", { trackRef: "7:track:2", start: 0, length: 4, notes: [], expectedName: "Vox" }), /isn't a MIDI track/);
  await assert.rejects(client.invoke("arrangement.midi-clip.create", { trackRef: "7:track:0", start: 0, length: 4, notes: [], expectedName: "Bass" }), /"Keys" now, not "Bass"/);
  client.close();
});

test("clearing a range takes the clips inside it and cuts the ones across its edges", async () => {
  const client = await connect();
  for (const [start, length] of [[0, 4], [4, 2], [8, 4]]) await client.invoke("arrangement.midi-clip.create", { trackRef: "7:track:1", start, length, notes: [], expectedName: "Drums" });
  const result = await client.invoke("clip.clear-range", { trackRef: "7:track:1", fromBeat: 2, toBeat: 9, expectedName: "Drums" });
  assert.equal(result.clipsBefore, 3); assert.equal(result.clipsAfter, 2);
  assert.deepEqual(result.removed, [{ name: "", start: 4, end: 6, isAudio: false }]);
  assert.deepEqual(live.drums.arrangementClips.map((clip) => [clip.start, clip.end]), [[0, 2], [9, 12]]);
  client.close();
});

test("an offline render is an audio track's own clips, copied to a name of its own", async () => {
  const client = await connect();
  const first = await client.invoke("render.offline", { trackRef: "7:track:2", fromBeat: 0, toBeat: 8, expectedName: "Vox" });
  assert.equal(first.format, "wav"); assert.equal(first.channels, 2); assert.equal(first.sampleRate, 44100); assert.equal(first.bitDepth, 24);
  assert.equal(first.seconds, 4); assert.ok(first.path.startsWith(join(root, "temp", "renders")));
  const second = await client.invoke("render.offline", { trackRef: "7:track:2", fromBeat: 0, toBeat: 4, expectedName: "Vox" });
  assert.notEqual(second.path, first.path); assert.ok(existsSync(first.path)); assert.equal(second.seconds, 2);
  await assert.rejects(client.invoke("render.offline", { trackRef: "7:track:0", fromBeat: 0, toBeat: 8, expectedName: "Keys" }), /isn't an audio track/);
  await assert.rejects(client.invoke("render.offline", { trackRef: "7:track:2", fromBeat: 4, toBeat: 4, expectedName: "Vox" }), /empty/);
  // A group track (the SDK lists it as an audio track) has no clips of its own to render.
  const bus = live.track("AudioTrack", "Bus"); live.song.tracks.push(bus); live.vox.group = bus;
  await assert.rejects(client.invoke("render.offline", { trackRef: "7:track:3", fromBeat: 0, toBeat: 4, expectedName: "Bus" }), /"Bus" is a group: render its tracks/);
  live.vox.group = null; live.song.tracks.pop();
  client.close();
});

test("a device is copied straight after itself; a Drum Rack pad gets a sample without the Browser", async () => {
  const client = await connect();
  assert.deepEqual(await client.invoke("device.duplicate", { ref: "7:device:0:0", expectedName: "Operator" }), { ref: "7:device:0:1", name: "Operator", index: 1 });
  assert.deepEqual(live.keys.devices.map((device) => device.name), ["Operator", "Operator", "Reverb"]);
  const sample = join(root, "snare.wav"); writeFileSync(sample, "");
  const result = await client.invoke("drum-pad.sample-chain", { rackRef: "7:device:1:0", note: 38, samplePath: sample, expectedName: "Drum Rack" });
  assert.deepEqual(result, { chainRef: "7:chain:1:0:1", deviceRef: "7:device:1:0:1:0", note: 38, samplePath: sample });
  const chain = live.rack.chains[1]; assert.equal(chain.receivingNote, 38n); assert.equal(chain.devices[0].cls, "Simpler"); assert.equal(chain.devices[0].sample.filePath, sample);
  await assert.rejects(client.invoke("drum-pad.sample-chain", { rackRef: "7:device:0:0", note: 38, samplePath: sample, expectedName: "Operator" }), /isn't a Drum Rack/);
  // A pad that already plays something isn't layered onto.
  await assert.rejects(client.invoke("drum-pad.sample-chain", { rackRef: "7:device:1:0", note: 36, samplePath: sample, expectedName: "Drum Rack" }), /pad 36 .* already plays something/);
  client.close();
});

test("right-click 'Ask Kumi about this' tells every connected host where the producer pointed", async () => {
  assert.equal(live.menu.filter((item) => item.title === "Ask Kumi about this").length, 9);
  assert.deepEqual(live.menu.filter((item) => item.title === "Ask Kumi about this selection").map((item) => item.scope), ["ClipSlotSelection", "AudioTrack.ArrangementSelection", "MidiTrack.ArrangementSelection"]);
  const client = await connect();
  live.commands.get("kumi.point")(live.handle(live.kick));
  const pointed = await waitFor(() => client.events.at(-1));
  assert.equal(pointed.result.event.type, "pointed");
  assert.deepEqual({ ...pointed.result.event.payload, at: undefined }, { kind: "device", path: [1, 0, 0, 0], name: "Kick", trail: ["Drums", "Drum Rack", "Kick"], at: undefined });
  live.commands.get("kumi.point-selection")({ time_selection_start: 16, time_selection_end: 32, selected_lanes: [live.handle(live.keys), live.handle(live.drums)] });
  const selection = await waitFor(() => client.events.length === 2 && client.events[1]);
  assert.deepEqual(selection.result.event.payload.timeSelection, { fromBeat: 16, toBeat: 32 });
  assert.deepEqual(selection.result.event.payload.lanes.map((lane) => [lane.kind, lane.path]), [["track", [0]], ["track", [1]]]);
  assert.equal(selection.result.event.sequence, 2);
  live.commands.get("kumi.point")(live.handle(live.keys.clipSlots[1].clip));
  const clip = await waitFor(() => client.events.length === 3 && client.events[2]);
  assert.deepEqual([clip.result.event.payload.kind, clip.result.event.payload.path], ["clip", [0, 1]]);
  client.close();
});

test("a group starts its steps together inside one Live transaction, and names a step that fails", async () => {
  const client = await connect();
  const before = live.created.length; const notesBefore = live.notesSet.length; const transactionsBefore = live.transactions.length;
  const grouped = await client.invoke("transaction.group", { ops: [
    { operation: "arrangement.midi-clip.create", args: { trackRef: "7:track:0", start: 32, length: 4, notes: [{ pitch: 60, start: 0, duration: 1 }], expectedName: "Keys" } },
    { operation: "arrangement.midi-clip.create", args: { trackRef: "7:track:1", start: 32, length: 4, notes: [{ pitch: 36, start: 0, duration: 1 }], expectedName: "Drums" } },
  ] });
  assert.deepEqual(grouped.results.map((result) => result.notes), [1, 1]);
  // Both clips are made in one transaction, then both get their notes in one more: two undo steps in Live.
  assert.deepEqual(live.created.slice(before).map((entry) => entry.insideTransaction), [true, true]);
  assert.deepEqual(live.notesSet.slice(notesBefore).map((entry) => entry.insideTransaction), [true, true]);
  const outermost = live.transactions.slice(transactionsBefore).reduce((state, mark) => ({ depth: state.depth + (mark === "begin" ? 1 : -1), opened: state.opened + (mark === "begin" && state.depth === 0 ? 1 : 0) }), { depth: 0, opened: 0 });
  assert.equal(outermost.opened, 2);
  await assert.rejects(client.invoke("transaction.group", { ops: [{ operation: "arrangement.midi-clip.create", args: { trackRef: "7:track:0", start: 40, length: 4, notes: [] } }] }), /expectedName is required/);
  await assert.rejects(client.invoke("transaction.group", { ops: [
    { operation: "arrangement.midi-clip.create", args: { trackRef: "7:track:0", start: 40, length: 4, notes: [], expectedName: "Keys" } },
    { operation: "render.offline", args: { trackRef: "7:track:0", fromBeat: 0, toBeat: 4, expectedName: "Keys" } },
  ] }), /step 2 failed \(.*isn't an audio track.*\); 1 of 2 steps were made/);
  await assert.rejects(client.invoke("transaction.group", { ops: [{ operation: "tempo.set", args: {} }] }), /can't hold tempo\.set/);
  client.close();
});

test("a change whose deadline passes while it waits its turn doesn't happen late", async () => {
  const client = await connect();
  const count = live.keys.arrangementClips.length;
  live.renderDelayMs = 150;
  // A slow render first; the clip behind it asks to be done within 50 ms.
  const slow = client.send({ method: "invoke", operation: "render.offline", args: { trackRef: "7:track:2", fromBeat: 0, toBeat: 4, expectedName: "Vox" } });
  const stale = await client.send({ method: "invoke", operation: "arrangement.midi-clip.create", args: { trackRef: "7:track:0", start: 64, length: 4, notes: [], expectedName: "Keys" } }, { deadlineMs: Date.now() + 50 });
  assert.equal((await slow).ok, true); live.renderDelayMs = 0;
  assert.equal(stale.ok, false); assert.match(stale.error, /deadline passed before Live could start it/);
  assert.equal(live.keys.arrangementClips.length, count);
  client.close();
});
