// What the Extensions channel does that the Remote Script can't, or can't as well: render offline,
// write an Arrangement MIDI clip with its notes, clear a range, copy a device, give a Drum Rack pad a
// sample without the Browser, and copy a file into the project. Each is one registry operation.
import { copyFileSync, existsSync, mkdirSync, readdirSync, statSync, unlinkSync } from "node:fs";
import { extname, join } from "node:path";
import { AudioClip, AudioTrack, DrumChain, DrumRack, MidiTrack, TakeLane, Track, type ExtensionContext, type NoteDescription } from "@ableton-extensions/sdk";
import { audioInfo } from "./audio-info.js";
import { allTracks, checkName, deviceAt, makeRef, parseRef, takeLaneAt, trackAt } from "./refs.js";
import { token } from "./wire.js";

type Context = ExtensionContext<"1.0.0">;
type Args = Record<string, unknown>;
export interface Environment { rendersDir: string }
export type Operation = (context: Context, args: Args, environment: Environment) => Promise<Record<string, unknown>>;

const str = (value: unknown): string | undefined => typeof value === "string" ? value : undefined;
/** The name the host read at this position: required, so a position that now holds something else is refused. */
const expected = (args: Args): string => { const name = str(args.expectedName); if (name === undefined) throw new Error("expectedName is required on the Extensions channel"); return name; };
const num = (value: unknown): number => { if (typeof value !== "number" || !Number.isFinite(value)) throw new Error("a number is missing"); return value; };

// Renders older than this are removed as new ones are made; Kumi has read them long before.
const RENDER_KEEP_MS = 6 * 60 * 60 * 1000;

function pruneRenders(dir: string): void {
  const now = Date.now();
  for (const name of existsSync(dir) ? readdirSync(dir) : []) {
    const path = join(dir, name);
    try { if (now - statSync(path).mtimeMs > RENDER_KEEP_MS) unlinkSync(path); } catch { /* another render may be reading it */ }
  }
}

/** An audio track's own clips between two beats, rendered offline before its devices (Live's renderPreFxAudio). */
const renderOffline: Operation = async (context, args, environment) => {
  const { path } = parseRef(String(args.trackRef));
  const track = trackAt(context, path[0]);
  checkName(track, expected(args), "track");
  if (!(track instanceof AudioTrack)) throw new Error(`"${track.name}" isn't an audio track: offline renders are of an audio track's own clips, before its devices`);
  // The SDK lists a group track among the tracks, as an audio track, but it has no clips of its own and Live refuses to render it.
  if (allTracks(context).some((other) => other.groupTrack?.handle.id === track.handle.id)) throw new Error(`"${track.name}" is a group: render its tracks`);
  const from = num(args.fromBeat); const to = num(args.toBeat);
  if (!(to > from)) throw new Error("the range to render is empty");
  const started = performance.now();
  const rendered = await context.resources.renderPreFxAudio(track, from, to);
  const renderMs = performance.now() - started;
  // Live names a render after its clip, to the second, so the next render of the same clip would
  // replace it: move it to a name of its own at once.
  mkdirSync(environment.rendersDir, { recursive: true, mode: 0o700 });
  pruneRenders(environment.rendersDir);
  const target = join(environment.rendersDir, `${Date.now()}-${token(6)}${extname(rendered).toLowerCase() || ".wav"}`);
  copyFileSync(rendered, target);
  try { unlinkSync(rendered); } catch { /* Live's own temp folder; it clears it */ }
  const info = audioInfo(target);
  return { path: target, format: info.format, channels: info.channels, sampleRate: info.sampleRate, bitDepth: info.bitDepth, seconds: info.seconds, bytes: info.bytes, renderMs };
};

function noteDescriptions(value: unknown): NoteDescription[] {
  if (!Array.isArray(value)) throw new Error("notes must be a list");
  return value.map((raw) => {
    const note = raw as Record<string, unknown>;
    const description: NoteDescription = { pitch: num(note.pitch), startTime: num(note.start), duration: num(note.duration) };
    if (typeof note.velocity === "number") description.velocity = note.velocity;
    if (typeof note.mute === "boolean") description.muted = note.mute;
    if (typeof note.probability === "number") description.probability = note.probability;
    if (typeof note.velocityDeviation === "number") description.velocityDeviation = note.velocityDeviation;
    if (typeof note.releaseVelocity === "number") description.releaseVelocity = note.releaseVelocity;
    return description;
  });
}

interface MadeClip { finish: () => void; result: () => Record<string, unknown> }

/**
 * A MIDI clip in the Arrangement (or a take lane) with its notes: what the Remote Script can't write
 * there. Two parts, because the SDK makes a clip asynchronously and its notes need the clip: making it,
 * then (`finish`, synchronous) its notes, name and looping. A group makes all its clips in one undo step
 * and gives them their notes in a second.
 */
async function makeArrangementMidiClip(context: Context, args: Args): Promise<MadeClip> {
  const reference = String(args.trackRef); const { epoch, path } = parseRef(reference);
  const track = trackAt(context, path[0]);
  checkName(track, expected(args), "track");
  if (!(track instanceof MidiTrack)) throw new Error(`"${track.name}" isn't a MIDI track`);
  const notes = noteDescriptions(args.notes);
  let lane: MidiTrack<"1.0.0"> | TakeLane<"1.0.0"> = track; let laneIndex: number | undefined;
  if (typeof args.takeLaneRef === "string") {
    const lanePath = parseRef(args.takeLaneRef).path;
    if (lanePath[0] !== path[0]) throw new Error("that take lane belongs to another track");
    lane = takeLaneAt(context, lanePath); laneIndex = lanePath[1];
  }
  const clip = await lane.createMidiClip(num(args.start), num(args.length));
  return {
    finish: () => {
      clip.notes = notes;
      if (typeof args.name === "string") clip.name = args.name;
      if (typeof args.looping === "boolean") clip.looping = args.looping;
    },
    result: () => {
      const clips = lane instanceof TakeLane ? lane.clips : track.arrangementClips;
      const index = clips.findIndex((candidate) => candidate.handle.id === clip.handle.id);
      if (index < 0) throw new Error("Live made the clip, but it isn't among the lane's clips");
      const ref = laneIndex === undefined ? makeRef(epoch, "arrangement_clip", [path[0]!, index]) : makeRef(epoch, "take_lane_clip", [path[0]!, laneIndex, index]);
      return { ref, trackRef: reference, name: clip.name, start: clip.startTime, end: clip.endTime, notes: clip.notes.length };
    },
  };
}

const arrangementMidiClip: Operation = async (context, args) => {
  const made = await makeArrangementMidiClip(context, args);
  context.withinTransaction(made.finish);
  return made.result();
};

/** Every clip in a beat range on a track goes, and a clip crossing either edge is cut at it. */
const clearRange: Operation = async (context, args) => {
  const reference = String(args.trackRef); const { path } = parseRef(reference);
  const track = trackAt(context, path[0]);
  checkName(track, expected(args), "track");
  if (args.takeLaneRef !== undefined) throw new Error("a range is cleared on the track's own lane");
  const from = num(args.fromBeat); const to = num(args.toBeat);
  if (!(to > from)) throw new Error("the range to clear is empty");
  const before = track.arrangementClips.map((clip) => ({ name: clip.name, start: clip.startTime, end: clip.endTime, isAudio: clip instanceof AudioClip }));
  await track.clearClipsInRange(from, to);
  const clipsAfter = track.arrangementClips.length;
  return { trackRef: reference, clipsBefore: before.length, clipsAfter, removed: before.filter((clip) => clip.start >= from && clip.end <= to) };
};

/** A copy of a device, straight after it in its chain. */
const duplicateDevice: Operation = async (context, args) => {
  const reference = String(args.ref); const { epoch, path } = parseRef(reference);
  const { device, owner, index } = deviceAt(context, path);
  checkName(device, expected(args), "device");
  const copy = await owner.duplicateDevice(device);
  const copyIndex = owner.devices.findIndex((candidate) => candidate.handle.id === copy.handle.id);
  if (copyIndex < 0) throw new Error("Live made the copy, but not in this device's chain");
  return { ref: makeRef(epoch, "device", [...path.slice(0, -1), copyIndex]), name: copy.name, index: copyIndex };
};

/** A sample on a Drum Rack pad without the Browser: a new chain on the pad's note, a Simpler in it, the sample in that. */
const padSampleChain: Operation = async (context, args) => {
  const reference = String(args.rackRef); const { epoch, path } = parseRef(reference);
  const { device: rack } = deviceAt(context, path);
  checkName(rack, expected(args), "Drum Rack");
  if (!(rack instanceof DrumRack)) throw new Error(`"${rack.name}" isn't a Drum Rack`);
  const samplePath = String(args.samplePath);
  if (!existsSync(samplePath)) throw new Error("the sample isn't there any more");
  const note = num(args.note);
  if (rack.chains.some((existing) => existing.receivingNote === note)) throw new Error(`pad ${note} of "${rack.name}" already plays something; clear it first`);
  const chain = await rack.insertChain(rack.chains.length);
  if (!(chain instanceof DrumChain)) throw new Error("Live didn't add a pad chain to the Drum Rack");
  chain.receivingNote = note;
  const simpler = await chain.insertDevice("Simpler", 0);
  if (!("replaceSample" in simpler)) throw new Error("Live didn't put a Simpler in the new chain");
  await (simpler as unknown as { replaceSample(path: string): Promise<unknown> }).replaceSample(samplePath);
  const chainIndex = rack.chains.findIndex((candidate) => candidate.handle.id === chain.handle.id);
  if (chainIndex < 0) throw new Error("Live added the chain, but it isn't in the rack's chains");
  return { chainRef: makeRef(epoch, "chain", [...path, chainIndex]), deviceRef: makeRef(epoch, "device", [...path, chainIndex, 0]), note, samplePath };
};

/** A copy of a file in the project folder, which Live then manages (Collect All and Save does the same). */
const projectImport: Operation = async (context, args) => ({ path: await context.resources.importIntoProject(String(args.filePath)) });

export const OPERATIONS: Readonly<Record<string, Operation>> = {
  "render.offline": renderOffline,
  "arrangement.midi-clip.create": arrangementMidiClip,
  "clip.clear-range": clearRange,
  "device.duplicate": duplicateDevice,
  "drum-pad.sample-chain": padSampleChain,
  "project.import": projectImport,
};

/**
 * Several of this channel's changes, grouped in Live's undo: the steps start together inside one SDK
 * transaction (the SDK groups only what starts inside its synchronous callback), so every step's first
 * change is one undo step; the Arrangement clips' notes, names and looping, which need the clips made
 * first, then go in one more. Steps must not depend on one another. Each step's arguments are checked
 * against the registry first, and a failed step is named, with how many others were made.
 */
export function transactionGroup(validate: (operation: string, args: Args) => void): Operation {
  return async (context, args, environment) => {
    const steps = Array.isArray(args.ops) ? args.ops as Array<{ operation: string; args: Args }> : [];
    for (const [index, step] of steps.entries()) {
      if (!OPERATIONS[step.operation]) throw new Error(`step ${index + 1}: a group can't hold ${step.operation}`);
      validate(step.operation, step.args);
    }
    const start = (step: { operation: string; args: Args }): Promise<MadeClip | Record<string, unknown>> => step.operation === "arrangement.midi-clip.create" ? makeArrangementMidiClip(context, step.args) : OPERATIONS[step.operation]!(context, step.args, environment);
    const settled = await context.withinTransaction(() => Promise.allSettled(steps.map(start)));
    const made = settled.flatMap((outcome) => outcome.status === "fulfilled" && typeof (outcome.value as Partial<MadeClip>).finish === "function" ? [outcome.value as MadeClip] : []);
    if (made.length) context.withinTransaction(() => { for (const clip of made) clip.finish(); });
    const failed = settled.findIndex((outcome) => outcome.status === "rejected");
    if (failed >= 0) {
      const reason = (settled[failed] as PromiseRejectedResult).reason;
      const done = settled.filter((outcome) => outcome.status === "fulfilled").length;
      throw new Error(`step ${failed + 1} failed (${reason instanceof Error ? reason.message : String(reason ?? "Live refused it without a reason")}); ${done} of ${steps.length} steps were made`);
    }
    return { results: settled.map((outcome) => { const value = (outcome as PromiseFulfilledResult<MadeClip | Record<string, unknown>>).value; return typeof (value as Partial<MadeClip>).finish === "function" ? (value as MadeClip).result() : value as Record<string, unknown>; }) };
  };
}

/** True for an object the SDK knows as a track (tracks, returns and Main). */
export const isTrack = (value: unknown): boolean => value instanceof Track;
export { allTracks };
