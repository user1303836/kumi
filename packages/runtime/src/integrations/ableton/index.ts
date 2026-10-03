import { randomUUID } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import { existsSync, readFileSync, statSync } from "node:fs";
import { mkdir, readdir, rm } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, extname, isAbsolute, join } from "node:path";
import { lowDisk, MB } from "../../core/disk.js";
import { connect as connectSocket } from "node:net";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import type { AuditionEvent, AuditionRequest, AuditionResult, AuditionTake, HeardTake, HearRequest, GoalRig, GoalSlotInfo, CatchUp, ChainNode, ChangeRecord, ConnectionState, DeviceNode, DeviceTree, DisconnectCause, Integration, PinnedNode, JsonObject, KernelTool, LiveFocus, LiveTransport, Observation, StreamingCall } from "../../core/contracts.js";
import { MIX_CANDIDATE } from "../../core/contracts.js";
import { KumiError } from "../../core/errors.js";
import { connectMcp, type McpEndpoint } from "../../mcp/client.js";
import { AllowedTools, MODEL_TOOLS } from "../../mcp/allowed-tools.js";
import { discoveryArgs, discoveryPayload, FIELDS, INSTRUCTIONS, object, ObservationError, PARENTS, payload, queryKey, setIdentity, statusPayload } from "./context.js";
import { foldTracks } from "./fold.js";
import { defaultSampleFolders, findSamples, folderPath, SAMPLE_EXTENSIONS, userLibrary, type Sample } from "./samples.js";
import { deviceTool } from "../../devices/tool.js";
import { CHANGES, EMERGENCY_STOP, hexColor, HOST_TOOLS, newRecord, nextChangeId, type ChangeContext, type SampleSelector, REFERENCE_FIELDS, UNDO_DESCRIPTION, UNDO_TOOL, undoNote, type ChangeKind, type KnownTrack } from "./changes.js";
import { arrange, ARRANGE_DESCRIPTION, ARRANGE_SCHEMA, ARRANGE_TOOL, type ArrangeHost } from "./arrange.js";
import { ACTIONS, type ActionKind } from "./actions.js";
import { bars, setMeter } from "./more-changes.js";
import { ARRANGEMENT_BRIDGE, atLeast, EARS_BRIDGE, FULL_CONTROL_BRIDGE, GOAL_BRIDGE, PYTHON_BRIDGE, RENDER_BRIDGE, SCALE_BRIDGE } from "./bridge-version.js";
import { AUDITION_DESCRIPTION, AUDITION_SCHEMA, AUDITION_TOOL, auditionRequest, RENDER_DESCRIPTION, RENDER_SCHEMA, RENDER_TOOL, renderSpan, restoreStore, silentRender } from "./audition.js";
import { audioPath, closeness, hear, type Analysis } from "../../audio/index.js";
import { summary as heardSummary } from "../../audio/tools.js";
import type { Knob } from "../../core/evolve.js";
import { startFocusFeed, type FocusFeed } from "./focus.js";
import { stepScanner } from "./plan-stream.js";
import { catchUpFrom, describeDiff, describeWatch, projectIdOf, since, type Baseline, type ProjectStore } from "./project.js";
import { KUMI } from "../../command.js";
import { EARS_ITEM, installEars } from "../../ears/device.js";
import { openEarsLink, type EarsLink, type Tap } from "../../ears/link.js";
import { frameAt, readCapture, runs, writeCaptureWav } from "../../ears/capture.js";
import { HandsError, openHands, type Hands, type HandsReply, type MenuItem } from "../../hands/index.js";
import { COMMANDS, findItem, LIVE_COMMAND_DESCRIPTION, LIVE_COMMAND_SCHEMA, LIVE_COMMAND_TOOL, shortcut } from "./live-command.js";
import { DISPLAY_MAP_SCRIPT, valueForDisplay, type DisplayMap } from "./display.js";
import { findScript, revertScript, setScript, type FastFound, type FastRevert, type FastSet, type FastTarget } from "./fast.js";
import { PLUGIN_DESCRIPTION, PLUGIN_SCHEMA, PLUGIN_TOOL } from "./plugin-tool.js";
import { adapterFor, folderFor, pluginGuide } from "../../plugins/registry.js";
import { buildWavetable, writeWavetable, type Keyframe } from "../../audio/wavetable.js";

/** Bridge tools Kumi uses to catch up on a Set; never offered to the model. */
const PROJECT_TOOLS = ["live_project_info", "live_project_snapshot_export", "live_project_snapshot_diff"];
/** A verified copy of the Set as last saved, kept before big plans. */
const BACKUP_TOOLS = ["live_project_backup_preview", "live_project_backup_apply"];
const PYTHON_TOOLS = ["live_run_python"];
/** Every bridge tool Kumi may call: the model's reads, and those behind Kumi's own tools. */
export const BRIDGE_TOOLS: readonly string[] = [...new Set([...MODEL_TOOLS, ...HOST_TOOLS, ...PROJECT_TOOLS, ...BACKUP_TOOLS, ...PYTHON_TOOLS])];
/** Keys whose values are Live references: ref, parent, trackRef, parentRef, selectedTrackRef… */
const REF_KEY = /^(?:ref|parent)$|Refs?$/;
/** Live's references: an epoch, a kind and a path ("1232800184424618:track:4"). */
const LIVE_REF = /^\d+:[a-z][a-z_]{0,31}:/;
const MAKE_CHANGES = "make_changes";
/** A plan step that waits: while a recording runs, say. */
const WAIT = "wait";
const WATCH_TOOL = "watch_me";
const WATCH_DESCRIPTION = "Learn a routine the producer does by hand in Live: action start just before they do it (Kumi notes the Set as it is; nothing in Live changes), then action stop when they say they're done. Stop gives what changed: tracks added (with routing, arming, monitoring), devices loaded (with the knobs they turned from Live's defaults; switches and modes aren't compared), clips recorded or made, and settings changed, each with its track. Save it at once with save_recipe as the steps that would redo it, with $blanks for what differs each time (the source track, say), leaving out anything clearly unrelated: a device as load_device (as: \"sat\") and each knob turned as set_device_parameter with deviceRef \"@sat\" and the knob's name as parameter. Then say in a few words what the recipe does. The producer can ask you to change it.";
/** How many devices added while watching Kumi reads the settings of. */
const WATCH_DEVICES = 12;
const MAKE_CHANGES_DESCRIPTION = "Make changes in one call, in order: each step is one of your change tools (or play, record, fire_scene and the like) with its input, and \"@name\" in an input stands for what an earlier step marked as: \"name\" made (a new track, a loaded device). A wait step ({\"beats\": 8} or {\"seconds\": 4}) lets a recording run, as in bouncing a sound to audio: route and arm a new audio track, record, play, wait, stop. It stops at the first step that fails and says what was done. With final: true and every step done, Kumi tells the producer what changed and the answer ends there, with no reply from you: use it when the changes complete the request, even a single change.";
const FIND_SAMPLES = "find_sounds";
const FIND_SAMPLES_DESCRIPTION = "Find sounds on this computer by words in their file and folder names (\"kick\", \"808\", \"vinyl\"), or pick some at random. Searches the folders the producer names, as full paths or ~/…, and otherwise where Live keeps samples: the User Library, Live's Core Library and Factory Packs. Returns each sample's name, path and length in seconds (for WAV and AIFF).";
const FIND_SAMPLES_SCHEMA: JsonObject = { type: "object", additionalProperties: false, properties: {
  folders: { type: "array", maxItems: 8, items: { type: "string", minLength: 1, maxLength: 1024 }, description: "Folders to search, such as ~/Samples; Live's User Library when empty" },
  words: { type: "array", maxItems: 8, items: { type: "string", minLength: 1, maxLength: 64 }, description: "Every word must appear in the file's name or its folders" },
  random: { type: "boolean", description: "Pick at random among the matches instead of the best ones" },
  limit: { type: "integer", minimum: 1, maximum: 50, description: "How many to return (20 when unset)" },
} };

const MAX_CHANGES_PER_TURN = 5_000;

/** Any audio file on this computer by its path (absolute, or from ~), as a sample for a change. */
function audioFileAt(path: string): Sample | undefined {
  const full = path.startsWith("~/") || path.startsWith("~\\") ? `${homedir()}${path.slice(1)}` : path;
  if (!isAbsolute(full) || !SAMPLE_EXTENSIONS.has(extname(full).toLowerCase())) return undefined;
  try {
    const stat = statSync(full);
    return stat.isFile() ? { name: basename(full).replace(/\.[^.]+$/, ""), path: full, folder: dirname(full), bytes: stat.size } : undefined;
  } catch { return undefined; }
}

/** What a tool says when Live went away (or the Set changed) during the answer. */
const NO_CURRENT_LIVE = "Kumi has no current view of Live: it disconnected, or the Set changed. Kumi reconnects on its own when Live is back; tell the producer, and don't describe earlier readings as current.";
const MAX_CHANGE_RECORDS = 20_000;
/** Seconds of a render heard before the part starts. */
const LEAD_IN = 0.1;
/**
 * The stretch of a render that's heard: a sound from just before its start (its attack whole); a section from
 * its first beat, as the reference is, so the two line up in time (with the lead-in, every hit came 100 ms late).
 */
const heardSpan = (start: number, seconds: number, focus: "sound" | "section" | undefined) =>
  focus === "section" ? { start: start + LEAD_IN, seconds } : { start, seconds: seconds + LEAD_IN };

function noAccess(key: string, now: Date, project?: Observation["project"]): Observation {
  return { key, label: "Inference-only — No Live access", instructions: INSTRUCTIONS, tools: [], revision: "no-live", ...(project ? { project } : {}),
    context: JSON.stringify({ observedAt: now.toISOString(), mode: "inference-only", access: "No Live access; do not describe remembered Set data as current. Kumi reconnects on its own when Live is back, and the conversation carries on." }) };
}
export function createInferenceOnlyIntegration(onConnection: (state: ConnectionState) => void): Integration {
  let closed = false;
  return {
    async start(signal) { signal.throwIfAborted(); if (closed) throw new Error("Integration is closed"); onConnection("disconnected"); },
    async observe(signal) { signal.throwIfAborted(); if (closed) throw new Error("Integration is closed"); return noAccess("inference-only", new Date()); },
    async close() { closed = true; },
  };
}
interface Options {
  /** `cause` says why it's disconnected: Live went away, or the bridge's own connection dropped. */
  onConnection: (state: ConnectionState, cause?: DisconnectCause) => void;
  bridgeConfig?: string;
  connect?: (signal: AbortSignal) => Promise<McpEndpoint>;
  now?: () => Date;
  generation?: string;
  onDispatch?: (name: string) => void;
  /** Basic focus: what the producer is looking at, reported when it changes. */
  onFocus?: (focus: LiveFocus | null) => void;
  /** The producer pointed at something in Live (its right-click "Ask Kumi about this"): pin it for their next message. */
  onPointed?: (pin: PinnedNode) => void;
  /** Live's transport, read when it starts or stops and now and then while it plays (null when Live's gone). */
  onTransport?: (transport: LiveTransport | null) => void;
  focusIntervalMs?: number;
  /** A change Kumi made or undid in Live, for HISTORY. */
  onChange?: (change: ChangeRecord) => void;
  /** Something Kumi did in Live that isn't a change to the Set (playing, launching, recording, showing), for NOW. */
  onAction?: (action: { title: string; playing?: boolean; recording?: boolean }) => void;
  /** For tests: the free-disk check before recording. */
  lowDisk?: typeof lowDisk;
  /** Kumi started (true) or stopped (false) watching the producer work (watch_me), for NOW. */
  onWatch?: (on: boolean) => void;
  /** Bound on one apply or undo once sent; it runs to the end even if the turn is cancelled. */
  changeTimeoutMs?: number;
  /** Where Kumi keeps each saved Set's last-seen state; without it Kumi doesn't catch up. */
  projectStore?: ProjectStore;
  /** What changed in a saved Set while Kumi wasn't running. */
  onCatchUp?: (catchUp: CatchUp) => void;
  /** How often to look for Live while it's away. */
  reconnectIntervalMs?: number;
  /** Live's User Library, where the devices Kumi makes go (make_device); Live's default place when left out. */
  userLibrary?: string;
  /** Where Main's level is kept while an audition renders, to put it back after a crash (none: not kept). */
  restoreFile?: string;
  /** Each audition, for the conversation's round lines. */
  onAudition?: (event: AuditionEvent) => void;
  /**
   * Kumi's listening devices (Kumi Ears): on by default, false to always record instead (scratch tracks).
   * Tests give their own link (and skip writing the device into a User Library).
   */
  ears?: false | { open: () => Promise<EarsLink> };
  /** Kumi's hands (Live's own menus and keys): on where the OS has them, false for none; tests give their own. */
  hands?: false | { open: () => Promise<Hands | undefined> };
  /**
   * A device's parameters set in one trip into Live, through Kumi's own Python (fast.ts), where the
   * bridge runs Python: on by default; false (or KUMI_FAST=0) for the bridge's preview and apply.
   */
  fast?: boolean;
}

/** `restore` is the name or colour a rename or recolour replaced in Kumi's picture of the track, put back if it's undone. */
interface Applied {
  record: ChangeRecord; transactionId: string; undoKey?: string; restore?: { ref: string; field: "name" | "color"; value?: string };
  /** Kept from the start: only Live's own undo can take it back, so Kumi's isn't tried. */
  permanent?: true;
  /** Several changes as one line in HISTORY (an arrangement): their ids, undone together, latest first. */
  members?: string[];
  /** The line this change is part of: it isn't one of its own. */
  within?: string;
  /** A fast change (fast.ts): undone by putting its parameters back, not through the bridge. */
  revert?: FastRevert[];
}

const resultText = (result: CallToolResult) => result.content.map((item) => (item.type === "text" ? item.text : "")).join("\n");
const uncertain = (result: CallToolResult) => (result.structuredContent as JsonObject | undefined)?.state === "uncertain" || /uncertain/i.test(resultText(result));

export function createAbletonIntegration(options: Options): Integration {
  const generation = options.generation ?? randomUUID();
  const now = options.now ?? (() => new Date());
  const lifetime = new AbortController();
  const refs = new Map<string, string>();
  // The model's names for Live's references: "parameter:12" for
  // "1232800184424618:parameter:1232800184424618:device:4:0:12". References were half of a read and
  // most of a plan, and the model reads and writes them token by token; Kumi maps them back.
  const shortRefs = new Map<string, string>(); const longRefs = new Map<string, string>(); const refCounts = new Map<string, number>();
  const cursors = new Map<string, string>();
  const unlisten: (() => void)[] = [];
  let endpoint: McpEndpoint | undefined;
  let tools: AllowedTools | undefined;
  let closed = false;
  let started = false;
  let available = false;
  let lost = false;
  let observationGeneration = 0;
  let currentEpoch: number | undefined;
  let currentSet: string | undefined;
  /** The Set's tempo, for waits measured in beats. */
  let currentTempo: number | undefined;
  /** Beats in a bar, from the time signature (4 in 4/4, 3 in 6/8). */
  let beatsPerBar = 4;
  /** While an audition runs: the ids of its own steps, kept out of HISTORY and undone after. */
  let quiet: string[] | undefined;
  /** This answer's auditions: how many, and the best score so far, for "Round 2 · 58% → 71%". */
  let rounds = { count: 0, best: undefined as number | undefined };
  /** References heard this session, by file and span: a matching run hears the same one each round. */
  const referenceCache = new Map<string, Analysis>();
  // Without a file, Main is still put back after every render; only a crash's leftover isn't.
  const restore = options.restoreFile ? restoreStore(options.restoreFile) : { save() {}, load: () => undefined, clear() {} };
  let toldQuietly = false;
  let closing: Promise<void> | undefined;
  let focusFeed: FocusFeed | undefined;
  /** Tracks from this turn's discovery (names and colours), for HISTORY's chips. */
  const known = new Map<string, KnownTrack>();
  /** Kumi's changes while this bridge connection lives; its transactions are what undo uses. */
  const changes = new Map<string, Applied>();
  /** Samples find_sounds returned, by path, with the folder searched (any other audio file loads by its path too). */
  const samples = new Map<string, Sample>();
  let changesThisTurn = 0;
  /** Samples Kumi picked itself in this answer, so random picks don't repeat. */
  const picked = new Set<string>();
  /** The Set as it was when the producer asked Kumi to watch them work, until they say they're done. */
  let watching: { set: string | undefined; pages: JsonObject[]; devices: Set<string>; at: number } | undefined;
  const changeTimeoutMs = options.changeTimeoutMs ?? 30_000;
  /** The saved Set Kumi is keeping track of (unsaved Sets have no file, so nothing to remember). */
  let project: { identity: string; path?: string; name: string } | undefined;
  let catchUpContext: JsonObject | undefined;
  let saving: Promise<void> = Promise.resolve();
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  let lastSaved = 0;
  let watcher: ReturnType<typeof setInterval> | undefined;
  let looking = false;
  let lastEpoch: number | undefined;
  let lostEpoch: number | undefined;
  let lastFreshBridge = 0;
  let closingStarted = false;
  /** Live came back after going away; the next observation may continue the same conversation. */
  let reconnected = false;
  /** The Set the conversation is about: its key, and its file when saved. */
  let previous: { key: string; name: string; identity: string; path?: string; project?: { id: string; name: string } } | undefined;

  const invalidate = () => { refs.clear(); cursors.clear(); known.clear(); currentEpoch = undefined; observationGeneration++; };
  /** Look for Live every couple of seconds until it's back. */
  const keepLooking = () => {
    clearInterval(watcher);
    watcher = setInterval(() => { void lookForLive(); }, options.reconnectIntervalMs ?? 2_000);
    watcher.unref?.();
  };
  /** The bridge's own connection dropped (its process ended, say): look for Live, and start a fresh bridge when it answers. */
  const loseAccess = () => {
    if (closed || closingStarted || (lost && !available)) return;
    available = false; focusFeed?.stop(); clearTimeout(transportTimer); reportTransport(null);
    if (!lost) { lost = true; lostEpoch = lastEpoch; invalidate(); options.onConnection("disconnected", "bridge"); }
    keepLooking();
  };
  /** Live is gone but the bridge is still here: wait for Live and carry on when it's back. */
  const loseLive = () => {
    if (closed || closingStarted || lost) return;
    lost = true; lostEpoch = lastEpoch; invalidate(); options.onConnection("disconnected", "live");
    // Live may come back with Max for Live: its listening device is tried again.
    earsRefused = false;
    clearTimeout(transportTimer); reportTransport(null);
    keepLooking();
  };
  async function openEndpoint(signal: AbortSignal): Promise<McpEndpoint> {
    if (options.connect) return options.connect(signal);
    if (!options.bridgeConfig) throw new ObservationError("Bridge configuration is required; choose explicit inference-only mode otherwise");
    return connectMcp({ signal, bridgeConfig: options.bridgeConfig, allowTools: [...BRIDGE_TOOLS],
      ...(options.onDispatch ? { onDispatch: options.onDispatch } : {}) });
  }
  /** Whether Live's Remote Script answers on the bridge's port (a plain connect, closed at once). */
  function remoteScriptListening(): Promise<boolean> {
    let target: { host: string; port: number } | undefined;
    try {
      const config = options.bridgeConfig ? JSON.parse(readFileSync(options.bridgeConfig, "utf8")) as { bridge?: { host?: unknown; port?: unknown } } : undefined;
      const host = config?.bridge?.host; const port = config?.bridge?.port;
      if (typeof host === "string" && (host === "127.0.0.1" || host === "localhost" || host === "::1") && Number.isInteger(port)) target = { host, port: port as number };
    } catch { target = undefined; }
    if (!target) return Promise.resolve(false);
    return new Promise((resolve) => {
      const socket = connectSocket({ host: target!.host, port: target!.port });
      const done = (listening: boolean) => { socket.destroy(); resolve(listening); };
      socket.setTimeout(500, () => done(false));
      socket.once("connect", () => done(true));
      socket.once("error", () => done(false));
    });
  }
  /** Use this bridge connection from now on: its tools, its disconnect signal and the focus feed. */
  function attach(connected: McpEndpoint) {
    endpoint = connected;
    tools = new AllowedTools(connected, new Set([...HOST_TOOLS, ...PROJECT_TOOLS, ...BACKUP_TOOLS, ...PYTHON_TOOLS]));
    // A changed catalog is read again on next use (AllowedTools listens for it); only losing the bridge ends access.
    unlisten.push(connected.onDisconnect(loseAccess));
    focusFeed?.stop();
    if (options.onFocus) {
      // A fixed internal read, not a model tool call: it bypasses the model's allowlist,
      // which is emptied while the catalog refreshes.
      focusFeed = startFocusFeed({ read: (signal) => connected.call("live_discover", { kind: "selection", limit: 1 }, signal), onFocus: options.onFocus,
        // Failing reads are the first sign that Live went away; check, and wait for it if so.
        onFailure: () => { if (!lost) void readStatus(AbortSignal.timeout(1_500)).then((status) => { if (!status.connected) loseLive(); }).catch(() => {}); },
        ...(options.focusIntervalMs ? { intervalMs: options.focusIntervalMs } : {}) });
    }
    // Live's events: the selection is read the moment it changes, and what the producer points at in Live
    // becomes a pin. (Subscribing waits for the catalog: see subscribe.)
    subscribed = false;
    if (connected.onLiveEvent) unlisten.push(connected.onLiveEvent(liveEvent));
  }
  let subscribed = false;
  /** Live says when its transport starts and stops (else it's read every few seconds). */
  let transportEvents = false;
  /** Once per connection, where the bridge has Live's events: then the focus poll is only a heartbeat. */
  async function subscribe(signal: AbortSignal) {
    if (subscribed || !tools?.has("live_subscribe")) { void readTransport(); return; }
    subscribed = true;
    try {
      // The transport too, for the beat light; a Live that can't tell it is subscribed without.
      let result = options.onTransport ? await tools.call("live_subscribe", { types: ["selection", "structure", "transport"] }, signal, { host: true }) : undefined;
      transportEvents = result !== undefined && !result.isError;
      if (!transportEvents) result = await tools.call("live_subscribe", { types: ["selection", "structure"] }, signal, { host: true });
      if (!result!.isError) focusFeed?.slow();
    } catch { subscribed = false; transportEvents = false; }
    void readTransport();
  }
  let transportTimer: ReturnType<typeof setTimeout> | undefined;
  let transportReading = false; let transportAgain = false;
  /** A bar's beats, from the time signature: read with the first read, then now and then. */
  let barBeats: number | undefined; let transportReads = 0;
  let lastTransport = "";
  const reportTransport = (transport: LiveTransport | null) => {
    const key = transport ? `${transport.playing}:${transport.tempo}:${transport.beatsPerBar}:${transport.playing ? transport.beat : ""}` : "null";
    if (key === lastTransport) return;
    lastTransport = key;
    try { options.onTransport?.(transport); } catch { /* a listener failure must not affect Live */ }
  };
  /**
   * Read the transport: whether Live plays, its tempo, and where the playhead is, timed to the middle of
   * the read so the beat can be followed between reads. Again in a few seconds while it plays (the tempo
   * may change), and every few seconds anyway where Live can't say when it starts.
   */
  async function readTransport(): Promise<void> {
    if (!options.onTransport || closed || !available || lost) return;
    if (transportReading) { transportAgain = true; return; }
    transportReading = true; clearTimeout(transportTimer);
    // A read that fails, or can't happen yet (the bridge's tools being read again), keeps to the last one's pace.
    let playing = lastTransport.startsWith("true"); let soon = false;
    try {
      if (!tools?.has("live_discover")) { soon = true; return; }
      const signal = AbortSignal.any([lifetime.signal, AbortSignal.timeout(3_000)]);
      if (transportReads++ % 8 === 0 && tools.has("live_song_state")) {
        const song = await tools.call("live_song_state", {}, signal, { host: true }).catch(() => undefined);
        const state = song && !song.isError ? payload(song) : {};
        const numerator = Number(state.signatureNumerator); const denominator = Number(state.signatureDenominator);
        if (numerator > 0 && denominator > 0) barBeats = numerator * 4 / denominator;
      }
      const sent = performance.now();
      const read = await tools.call("live_discover", { kind: "set", fields: ["tempo", "position", "playing"], limit: 1 }, signal, { host: true });
      const at = (sent + performance.now()) / 2;
      const row = read.isError ? undefined : (payload(read).items as JsonObject[] | undefined)?.[0];
      if (row && !closed) {
        playing = row.playing === true;
        reportTransport({ playing, at, ...(typeof row.tempo === "number" ? { tempo: row.tempo } : {}), ...(typeof row.position === "number" ? { beat: row.position } : {}),
          ...(barBeats ? { beatsPerBar: barBeats } : {}) });
      }
    } catch { /* the next read tries again */ }
    finally {
      transportReading = false;
      if (transportAgain) { transportAgain = false; void readTransport(); }
      else if (!closed && available && !lost && (soon || playing || !transportEvents)) {
        transportTimer = setTimeout(() => { void readTransport(); }, soon ? 500 : playing ? 4_000 : 2_500);
        transportTimer.unref?.();
      }
    }
  }
  function liveEvent(event: JsonObject) {
    if (event.type === "selection" || event.type === "structure") focusFeed?.poke();
    if (event.type === "transport") void readTransport();
    if (event.type === "pointed" && options.onPointed) {
      const pin = pointedPin(event);
      if (pin) { try { options.onPointed(pin); } catch { /* a listener failure must not affect Live */ } }
    }
  }
  /** A pin from what the producer pointed at in Live: its reference (the bridge's), what it is, its name and where. */
  function pointedPin(event: JsonObject): PinnedNode | undefined {
    const data = event.payload && typeof event.payload === "object" && !Array.isArray(event.payload) ? event.payload as JsonObject : {};
    const kind = String(data.kind ?? "");
    const trail = Array.isArray(data.trail) ? data.trail.filter((part): part is string => typeof part === "string") : [];
    const path = Array.isArray(data.path) ? data.path : [];
    if (/selection$/.test(kind)) {
      // A stretch of the Arrangement (or slots of the Session): its first lane, and the time it spans.
      const lanes = (Array.isArray(data.lanes) ? data.lanes : Array.isArray(data.slots) ? data.slots : []).filter((lane): lane is JsonObject => Boolean(lane) && typeof lane === "object");
      const lane = lanes[0]; const time = data.timeSelection && typeof data.timeSelection === "object" ? data.timeSelection as JsonObject : {};
      if (typeof lane?.ref !== "string") return undefined;
      const from = typeof time.fromBeat === "number" ? time.fromBeat : undefined; const to = typeof time.toBeat === "number" ? time.toBeat : undefined;
      const laneName = typeof lane.name === "string" ? lane.name : "";
      return { trackRef: lane.kind === "track" ? lane.ref : "", ref: lane.ref, node: from !== undefined && to !== undefined ? "selection" : "clip-slot", name: laneName, trail: [], siblings: lanes.slice(1).map((other) => String(other.name ?? "")),
        live: true, ...(laneName ? { track: laneName } : {}), ...(from !== undefined && to !== undefined ? { time: { fromBeat: from, toBeat: to } } : {}) };
    }
    const ref = typeof data.ref === "string" ? data.ref : typeof event.ref === "string" ? event.ref : undefined;
    if (!ref) return undefined;
    const epoch = ref.split(":")[0];
    const node: PinnedNode["node"] = kind === "track" ? "track" : kind === "scene" ? "scene" : kind === "clip_slot" ? "clip-slot" : /clip$/.test(kind) ? "clip" : "device";
    const name = typeof data.name === "string" && data.name ? data.name : node === "scene" && typeof path[0] === "number" ? `Scene ${path[0] + 1}` : "";
    // Live's trail ends with the object itself; a pin's is what holds it (as FOCUS's are).
    if (trail.length && (trail.at(-1) === data.name || trail.at(-1) === "")) trail.pop();
    return { trackRef: node === "track" ? ref : node === "scene" || typeof path[0] !== "number" ? "" : `${epoch}:track:${path[0]}`, ref, node, name, trail, siblings: [], live: true,
      ...(node !== "track" && node !== "scene" && trail[0] ? { track: trail[0] } : {}) };
  }
  async function lookForLive() {
    if (closed || !lost || looking) return;
    looking = true;
    try {
      let answering: boolean;
      if (available) {
        const status = await readStatus(AbortSignal.any([lifetime.signal, AbortSignal.timeout(1_500)]));
        if (status.connected) { back(status.epoch !== lostEpoch, false); return; }
        // The bridge won't carry on across a Live restart (its old transactions can't be reconciled),
        // so a restarted Live needs a fresh bridge: start one as soon as Live's Remote Script answers
        // on its port, and otherwise only now and then.
        answering = status.reason === "remote-bridge-or-live-epoch-changed" || await remoteScriptListening();
      } else answering = await remoteScriptListening(); // No bridge at all: a fresh one is the only way back.
      if (!answering && Date.now() - lastFreshBridge < 30_000) return;
      lastFreshBridge = Date.now();
      const signal = AbortSignal.any([lifetime.signal, AbortSignal.timeout(20_000)]);
      const fresh = await openEndpoint(signal);
      let epoch: unknown; let ready = false;
      // A fixed status read on the new bridge, before anything else uses it.
      try { const status = statusPayload(await fresh.call("live_status", {}, signal)); ready = status.connected === true; epoch = status.epoch; } catch { ready = false; }
      if (!ready || closed || !lost) { await fresh.close().catch(() => {}); return; }
      // Swap: stop listening to the old bridge before closing it, so closing isn't mistaken for losing it.
      const old = tools;
      for (const remove of unlisten.splice(0)) remove();
      attach(fresh);
      // The fresh bridge is the connection now, even if the old one dropped meanwhile.
      available = true;
      void old?.close().catch(() => {});
      back(epoch !== lostEpoch, true);
    } catch { /* still away */ } finally { looking = false; }
  }
  /** Live answers again: through the same bridge, or a fresh one (whose undo can't reach the old one's changes). */
  function back(restarted: boolean, freshBridge: boolean) {
    clearInterval(watcher); watcher = undefined;
    // A restarted Live has a new epoch; the bridge's transactions from before can't be undone.
    if (restarted) retireChanges("Live restarted since, so Kumi can't undo this; it's in the Set only if the Set was saved.");
    else if (freshBridge) retireChanges("Kumi's link to Live restarted since, so Kumi can't undo this; Live's own undo still can.");
    lost = false; reconnected = true;
    options.onConnection("connected");
  }
  /** Changes whose undo can no longer work stay in HISTORY as kept, with why. */
  function retireChanges(note: string) {
    for (const entry of changes.values()) {
      if (entry.record.state !== "applied" && entry.record.state !== "unsure") continue;
      entry.record = { ...entry.record, state: "expired", note };
      emitChange(entry.record);
    }
  }
  function changed() {
    invalidate(); options.onConnection("error");
    throw new ObservationError("Live epoch or Set identity changed; result discarded. Refresh before continuing.");
  }
  function assertLease(lease: number, signal: AbortSignal) {
    signal.throwIfAborted(); lifetime.signal.throwIfAborted();
    if (closed || lease !== observationGeneration) throw new ObservationError("Observation changed; late result discarded");
  }
  function assertEpoch(actual: unknown, expected: number) { if (actual !== expected) changed(); }
  /**
   * Changes in the Set can change what the bridge offers (a first clip brings clip tools). The
   * list is read again when needed; the Set, its references and the conversation stay current.
   */
  async function ensureCatalog(signal: AbortSignal) {
    if (!tools || tools.isValid) return;
    try { await tools.refresh(signal); }
    catch { signal.throwIfAborted(); await tools.refresh(signal); }
  }
  async function readStatus(signal: AbortSignal) {
    await ensureCatalog(signal);
    if (!tools?.has("live_status")) throw new ObservationError("Live status capability is unavailable");
    return statusPayload(await tools.call("live_status", {}, signal, { host: true }));
  }
  async function guardEpoch(signal: AbortSignal, expected: number, lease: number) {
    const status = await readStatus(signal);
    assertLease(lease, signal);
    if (!status.connected) { loseLive(); throw new ObservationError("No Live access; current observations were discarded"); }
    assertEpoch(status.epoch, expected);
    return status;
  }
  function registerRows(kind: string, rows: JsonObject[], args: JsonObject, nextCursor?: string) {
    for (const row of rows) {
      if (args.parent !== undefined && row.parentRef !== args.parent) throw new ObservationError("Discovery returned a different parent; result discarded");
      if (typeof row.ref === "string" && row.ref.length > 0 && row.ref.length <= 256) {
        refs.set(row.ref, kind);
        if (kind === "clip-slot" && typeof row.clipRef === "string" && row.clipRef.length <= 256) refs.set(row.clipRef, "session-clip");
        if (kind === "device" && Array.isArray(row.chainList)) for (const chain of row.chainList) {
          if (chain && typeof chain === "object" && typeof (chain as JsonObject).ref === "string" && String((chain as JsonObject).ref).length <= 256) refs.set(String((chain as JsonObject).ref), "chain");
        }
        const color = hexColor(row.color);
        if (/track$/.test(kind) && typeof row.name === "string") known.set(row.ref, { name: row.name.slice(0, 256), ...(color ? { color } : {}) });
      }
    }
    if (refs.size > 1_000_000) { invalidate(); throw new ObservationError("Too many current references; refresh and narrow the request"); }
    if (nextCursor) {
      if (nextCursor === args.cursor) throw new ObservationError("Discovery cursor repeated; narrow the request");
      cursors.set(nextCursor, queryKey(args));
      if (cursors.size > 100_000) { invalidate(); throw new ObservationError("Too many page cursors; refresh and narrow the request"); }
    }
  }
  function validateParentAndCursor(args: JsonObject) {
    const kind = String(args.kind);
    const parentKinds = PARENTS[kind];
    if (parentKinds || args.parent !== undefined) {
      const parentKind = typeof args.parent === "string" ? refs.get(args.parent) : undefined;
      const takes = parentKinds ?? ["set"];
      if (!parentKind) throw new ObservationError(args.parent === undefined ? `${kind} needs a parent: a ${takes.join(" or ")} from this turn` : "A fresh authoritative parent is required; discover the parent in this turn, not from history");
      // A parent that's current but of the wrong kind: say which kind this takes, and how to get there.
      if (!takes.includes(parentKind)) throw new ObservationError(`${kind} takes a ${takes.join(" or ")} as its parent, not a ${parentKind}${kind === "session-clip" && parentKind === "track" ? ": discover the track's clip-slots, each gives its clipRef" : ""}`);
    }
    if (args.cursor !== undefined && (typeof args.cursor !== "string" || cursors.get(args.cursor) !== queryKey(args))) {
      throw new ObservationError("Cursor is stale or belongs to another query; rediscover without it");
    }
  }
  /** A track row's mixer, as a producer reads it: values and Live's text, without the bridge's internal references. */
  function shortRef(ref: string): string {
    if (!LIVE_REF.test(ref)) return ref;
    const known = shortRefs.get(ref); if (known) return known;
    // Counts go on after a clear, so a name never comes back meaning something else.
    if (shortRefs.size >= 10_000_000) { shortRefs.clear(); longRefs.clear(); }
    const kind = /^\d+:([a-z][a-z_]{0,31}):/.exec(ref)![1]!;
    const count = (refCounts.get(kind) ?? 0) + 1; refCounts.set(kind, count);
    const name = `${kind}:${count}`;
    shortRefs.set(ref, name); longRefs.set(name, ref);
    return name;
  }
  /** A copy with Live's references, under keys that hold them, as the model's short names. */
  function shorten(value: unknown, key = "", depth = 0): unknown {
    if (depth > 32) return value;
    if (typeof value === "string") return REF_KEY.test(key) ? shortRef(value) : value;
    if (Array.isArray(value)) return value.map((item) => shorten(item, key, depth + 1));
    if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([child, item]) => [child, shorten(item, child, depth + 1)]));
    return value;
  }
  /** The model's input with its short names turned back into Live's references. */
  function lengthen(value: unknown, key = "", depth = 0): unknown {
    if (depth > 32) return value;
    if (typeof value === "string") return REF_KEY.test(key) ? longRefs.get(value) ?? value : value;
    if (Array.isArray(value)) return value.map((item) => lengthen(item, key, depth + 1));
    if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([child, item]) => [child, lengthen(item, child, depth + 1)]));
    return value;
  }
  /** Mixer rows as the model needs them: the values and Live's text for them. */
  function slimMixers(content: JsonObject): JsonObject {
    if (!Array.isArray(content.items) || !content.items.some((item) => item && typeof item === "object" && "mixer" in (item as JsonObject))) return content;
    const keep = ["volume", "pan", "mute", "solo", "cueVolume", "sends", "volumeDisplay", "panDisplay", "cueVolumeDisplay", "sendDisplays"];
    const items = content.items.map((item) => {
      const row = item as JsonObject;
      if (!row.mixer || typeof row.mixer !== "object") return row;
      return { ...row, mixer: Object.fromEntries(Object.entries(row.mixer as JsonObject).filter(([key]) => keep.includes(key))) };
    });
    return { ...content, items };
  }
  function encode(result: CallToolResult, epoch: number, slim = false): { text: string; isError: boolean } {
    if (result.isError) return { text: JSON.stringify(result), isError: true };
    // Live's answer as plain JSON, not a string inside the bridge's envelope: escaped quotes cost the model too.
    let live: unknown;
    try { live = payload(result); } catch { live = result; }
    const text = JSON.stringify({ live: shorten(slim && live && typeof live === "object" && !Array.isArray(live) ? slimMixers(live as JsonObject) : live), observation: { observedAt: now().toISOString(), connectionGeneration: generation, epoch,
      coverage: "Bounded read; preserve truncated/nextCursor markers. Traversal completeness is not established." } });
    if (Buffer.byteLength(text) > 64 * 1024) return { text: "Result too large; narrow fields/parent/page.", isError: true };
    return { text, isError: false };
  }
  async function invoke(name: string, input: JsonObject, originalSignal: AbortSignal) {
    const signal = AbortSignal.any([originalSignal, lifetime.signal]);
    let reading = false;
    const lease = observationGeneration;
    try {
      signal.throwIfAborted();
      if (!available || lost || currentEpoch === undefined || !tools) throw new ObservationError(NO_CURRENT_LIVE);
      const epoch = currentEpoch;
      await ensureCatalog(signal); assertLease(lease, signal);
      if (!tools.has(name)) throw new ObservationError("That read isn't available for the open Set right now");
      const named = lengthen(input) as JsonObject;
      const args = name === "live_discover" ? discoveryArgs(named) : named;
      if (name === "live_discover") validateParentAndCursor(args);
      else requireFreshReferences(args);
      reading = true;
      // A discovery answer carries its epoch, checked below, so only the check after it is needed.
      if (name !== "live_discover") await guardEpoch(signal, epoch, lease);
      let result = await tools.call(name, args, signal, { host: true });
      assertLease(lease, signal);
      // The model gets bounded reads: too big means narrowing the request, whatever the answer holds.
      if (Buffer.byteLength(JSON.stringify(result)) > 64 * 1024) { refs.clear(); cursors.clear(); return { text: "Result too large; narrow fields/parent/page instead of requesting a whole Set dump.", isError: true }; }
      if (!result.isError && name === "live_discover") {
        assertEpoch(payload(result).epoch, epoch);
        const first = discoveryPayload(result, String(args.kind), epoch);
        if (args.kind === "set" && (first.items.length !== 1 || setIdentity(first.items[0]!) !== currentSet)) changed();
        registerRows(String(args.kind), first.items, args, first.nextCursor);
        // Kumi reads on for the model while the rows stay small: a big device's parameters come in
        // one answer, not a model reply per page.
        let items = first.items; let next = first.nextCursor; let pages = 1;
        while (next && pages < 3 && Buffer.byteLength(JSON.stringify(items)) < 32 * 1024) {
          const pageArgs = { ...args, cursor: next };
          const more = await tools.call(name, pageArgs, signal, { host: true }); assertLease(lease, signal);
          if (more.isError || Buffer.byteLength(JSON.stringify(more)) > 64 * 1024) break;
          // Anything odd about a later page ends reading on; the model gets what came, with its cursor.
          try {
            assertEpoch(payload(more).epoch, epoch);
            const page = discoveryPayload(more, String(args.kind), epoch);
            registerRows(String(args.kind), page.items, pageArgs, page.nextCursor);
            items = [...items, ...page.items]; next = page.nextCursor; pages++;
          } catch { break; }
        }
        if (pages > 1) {
          const { nextCursor: _cursor, ...rest } = payload(result);
          const merged: JsonObject = { ...rest, items, truncated: Boolean(next), ...(next ? { nextCursor: next } : {}) };
          result = { content: [{ type: "text", text: JSON.stringify(merged) }], structuredContent: merged };
        }
      }
      await guardEpoch(signal, epoch, lease);
      if (!result.isError) {
        if (name === "live_snapshot") {
          const data = payload(result); assertEpoch(data.epoch, epoch);
          if (setIdentity(object(object(data.snapshot).set)) !== currentSet) changed();
          // Snapshot refs intentionally do not satisfy fresh-discovery parent leases.
        } else if (name === "live_status") {
          const data = statusPayload(result);
          if (!data.connected) loseLive();
          assertEpoch(data.epoch, epoch);
        }
      }
      const encoded = encode(result, epoch, name === "live_discover");
      if (encoded.isError) { refs.clear(); cursors.clear(); }
      return encoded;
    } catch (error) {
      // A failed upstream read cannot authorize retries with cached refs/cursors.
      if (reading && lease === observationGeneration) { refs.clear(); cursors.clear(); }
      return { text: error instanceof ObservationError ? error.message : "Live read failed; refresh current observations and narrow the request before retrying.", isError: true };
    }
  }
  function requireFreshReferences(args: JsonObject, depth = 0) {
    for (const field of REFERENCE_FIELDS) {
      const value = args[field];
      if (value !== undefined && (typeof value !== "string" || !refs.has(value))) throw new ObservationError(`${field} must come from discovery in this turn; discover it again${field === "parameterRef" ? " (or name the parameter instead, with parameter \"Filter Freq\", and the device's deviceRef from this turn's observation)" : ""}`);
    }
    // And the ones in a list, such as each parameter of several changed at once.
    if (depth < 2) for (const value of Object.values(args)) if (Array.isArray(value)) for (const item of value) if (item && typeof item === "object" && !Array.isArray(item)) requireFreshReferences(item as JsonObject, depth + 1);
  }
  function emitChange(record: ChangeRecord) {
    // An audition's own steps aren't the producer's changes: HISTORY gets one line for the whole of it.
    if (quiet) return;
    try { options.onChange?.(structuredClone(record)); } catch { /* a listener failure must not affect Live */ }
  }
  function remember(record: ChangeRecord, transactionId: string, restore?: Applied["restore"]) {
    changes.set(record.id, { record, transactionId, ...(restore ? { restore } : {}), ...(record.state === "kept" ? { permanent: true as const } : {}) });
    if (quiet) { quiet.push(record.id); return; }
    if (changes.size > MAX_CHANGE_RECORDS) changes.delete(changes.keys().next().value!);
    emitChange(record);
    scheduleSave(20_000);
  }
  async function exportPages(signal: AbortSignal): Promise<JsonObject[]> {
    const pages: JsonObject[] = [];
    let cursor: string | undefined;
    do {
      const page = payload(await tools!.call("live_project_snapshot_export", { profile: "local", limit: 200, ...(cursor ? { cursor } : {}) }, signal, { host: true }));
      pages.push(page);
      const next = object(page.page).nextCursor;
      cursor = typeof next === "string" && next ? next : undefined;
    } while (cursor && pages.length < 64);
    if (cursor) throw new ObservationError("The Set is too large to remember yet");
    return pages;
  }
  const artifactOf = (pages: readonly JsonObject[]) => { const id = object(pages[0]?.artifact ?? {}).id; return typeof id === "string" ? id : ""; };
  /** Save the Set's current state as what Kumi last saw; one save at a time. */
  function saveNow(bound = 30_000): Promise<void> {
    const work = saving.then(async () => {
      const known = project;
      if (!known?.path || !options.projectStore || !available || lost || closed) return;
      const signal = AbortSignal.any([lifetime.signal, AbortSignal.timeout(bound)]);
      await ensureCatalog(signal);
      if (!tools!.has("live_project_snapshot_export")) return;
      const pages = await exportPages(signal);
      if (project !== known) return;
      await options.projectStore.save({ version: 1, path: known.path, name: known.name, savedAt: now().getTime(), artifactId: artifactOf(pages), pages });
      lastSaved = Date.now();
    }).catch(() => { /* remembering is best effort; Live and the conversation are unaffected */ });
    saving = work;
    return work;
  }
  function scheduleSave(delayMs: number) {
    if (!options.projectStore || closed) return;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => { void saveNow(); }, delayMs);
    saveTimer.unref?.();
  }
  /** Compare the Set with what Kumi last saw, say what changed, then remember it as it is now. */
  /** The open Set's file, which identifies a saved Set between sessions; none for an unsaved Set. */
  async function projectPath(signal: AbortSignal): Promise<string | undefined> {
    try {
      await ensureCatalog(signal);
      if (!tools!.has("live_project_info")) return undefined;
      const info = payload(await tools!.call("live_project_info", {}, AbortSignal.any([signal, AbortSignal.timeout(5_000)]), { host: true }));
      return typeof info.path === "string" && info.path && info.exists !== false ? info.path : undefined;
    } catch { return undefined; }
  }
  function catchUp(identity: string, name: string, afterReconnect = false): void {
    catchUpContext = undefined;
    const store = options.projectStore;
    const path = project?.identity === identity ? project.path : undefined;
    if (!store || !path) return;
    saving = saving.then(async () => {
      const signal = AbortSignal.any([lifetime.signal, AbortSignal.timeout(60_000)]);
      await ensureCatalog(signal);
      if (!PROJECT_TOOLS.every((tool) => tools!.has(tool))) return;
      if (project?.identity !== identity) return;
      const pages = await exportPages(signal);
      const baseline: Baseline | undefined = await store.load(path);
      if (baseline && project?.identity === identity) {
        let described: { lines: string[]; more: number } | undefined = { lines: [], more: 0 };
        if (baseline.artifactId !== artifactOf(pages)) {
          try {
            const diff = payload(await tools!.call("live_project_snapshot_diff", { beforePages: baseline.pages, afterPages: pages, limit: 200 }, signal, { host: true }));
            described = describeDiff(diff, baseline.pages, pages);
            // Differences Kumi can't put into words (recomputed hashes, a moved return track) aren't worth a
            // catch-up, and "nothing changed" wouldn't be true either: say nothing.
            if (!described.lines.length) described = undefined;
          } catch { described = { lines: ["The Set changed, but it's too big for Kumi to compare yet"], more: 0 }; }
        }
        if (described) {
          const summary = { ...catchUpFrom(name, baseline, described), ...(afterReconnect ? { afterReconnect: true } : {}) };
          catchUpContext = { lastSeen: since(baseline.savedAt, now().getTime()), changes: summary.lines, ...(summary.more ? { more: summary.more } : {}) };
          try { options.onCatchUp?.(summary); } catch { /* a listener failure must not affect Live */ }
        }
      }
      await store.save({ version: 1, path, name, savedAt: now().getTime(), artifactId: artifactOf(pages), pages });
      lastSaved = Date.now();
    }).catch(() => { /* catching up is best effort */ });
  }
  const knownTrack = (ref: unknown) => (typeof ref === "string" ? known.get(ref) : undefined);
  /** The track a Live reference is on (a track, or what's on one: slots, clips, devices, chains, their parameters), by its position. */
  const trackIndexOf = (ref: string) => { const match = /:(?:track|clip_slot|clip|arrangement_clip|device|chain|drum_pad|routing_choice|take_lane|mixer):(\d+)/.exec(ref); return match ? Number(match[1]) : undefined; };
  /**
   * A reference whose position now holds something else (or nothing): its lease and its short
   * name go, so whatever is there next gets a new name and an old name can't reach it.
   */
  const unname = (ref: string) => { const short = shortRefs.get(ref); if (short !== undefined) { shortRefs.delete(ref); longRefs.delete(short); } };
  const retire = (ref: string) => { refs.delete(ref); known.delete(ref); unname(ref); };
  /** Changes that shift devices along a chain: their track's device, parameter and chain references move. */
  const DEVICE_SHIFTS = new Set(["move_device", "move_device_to", "delete_device"]);
  /** The scene a reference is in: a scene, or a Session slot or clip. */
  const sceneIndexOf = (ref: string) => { const match = /:scene:(\d+)|:(?:clip_slot|clip):\d+:(\d+)/.exec(ref); return match ? Number(match[1] ?? match[2]) : undefined; };
  /** Whether the connected bridge is new enough for this tool (see `since`). */
  const supported = (kind: { since?: string }) => !kind.since || atLeast(endpoint?.serverInfo?.version, kind.since);
  /** Rows per discovery page: a whole collection from a bridge without size caps, 100 from an older one. */
  /** A bridge with no caps on a Set's size, which pages by work (SCALE_BRIDGE): Kumi's own reads ask it for everything. */
  const scaled = () => atLeast(endpoint?.serverInfo?.version, SCALE_BRIDGE);
  const pageLimit = () => scaled() ? 100_000 : 100;
  /** The traversal budget of Kumi's own reads of a whole collection: everything, where the bridge allows it. */
  const wholeBudget = () => scaled() ? 10_000_000 : 1000;
  const tooOld = (kind: { since?: string }) => `That needs the Ableton bridge ${kind.since} or later; this one is ${endpoint?.serverInfo?.version ?? "older"}. Tell the producer to update it (kumi doctor says how).`;
  /** New tracks go after the last one unless the model gave a position (the bridge's default is request order). */
  async function appendAtEnd(kind: ChangeKind, input: JsonObject, signal: AbortSignal): Promise<JsonObject> {
    const lacks = (items: unknown) => Array.isArray(items) && items.some((item) => item && typeof item === "object" && (item as JsonObject).index === undefined);
    if (kind.family !== "structure" || (!lacks(input.tracks) && !lacks(input.scenes))) return input;
    const probe = await tools!.call(kind.preview, input, signal, { host: true });
    if (probe.isError) return input;
    const prior = object(payload(probe).prior);
    const place = (items: unknown, count: number) => Array.isArray(items)
      ? items.map((item, index) => (item && typeof item === "object" && (item as JsonObject).index === undefined ? { ...(item as JsonObject), index: count + index } : item)) : items;
    const tracks = Array.isArray(prior.tracks) ? prior.tracks.length : 0; const scenes = Array.isArray(prior.scenes) ? prior.scenes.length : 0;
    return { ...input, ...(input.tracks !== undefined ? { tracks: place(input.tracks, tracks) } : {}), ...(input.scenes !== undefined ? { scenes: place(input.scenes, scenes) } : {}) };
  }
  /** Every parameter of a device, page by page: an instrument such as Operator has more than one page's worth. */
  async function deviceParameters(deviceRef: unknown, fields: string[], signal: AbortSignal): Promise<JsonObject[]> {
    const rows: JsonObject[] = []; let cursor: string | undefined;
    for (let page = 0; page < 10_000; page++) {
      const read = payload(await tools!.call("live_discover", { kind: "parameter", parent: deviceRef, fields, limit: pageLimit(), ...(cursor ? { cursor } : {}) }, signal, { host: true }));
      rows.push(...(Array.isArray(read.items) ? read.items : []).map((item) => object(item)));
      cursor = typeof read.nextCursor === "string" && read.nextCursor !== cursor ? read.nextCursor : undefined;
      if (!cursor) break;
    }
    return rows;
  }
  /** Preview and apply one change as a single step, then record it for HISTORY. */
  /** Each parameter's map of Live's own text across its range, read once (by its reference: refs last while Live does). */
  const displayMaps = new Map<string, DisplayMap>();
  /** The value that makes a parameter show `text`, from Live's str_for_value across its range (read through Python in Live); or why not. */
  async function valueForText(parameterRef: string, text: string, signal: AbortSignal): Promise<number | string> {
    const long = String(lengthen(parameterRef, "parameterRef"));
    let map = displayMaps.get(long);
    if (!map) {
      if (!supported({ since: PYTHON_BRIDGE }) || !tools?.has("live_run_python")) return `Give ${JSON.stringify(text)} as a number in the parameter's range: this bridge can't read the parameter's units (update it with ${KUMI} bridge).`;
      const read = await tools.call("live_run_python", { code: DISPLAY_MAP_SCRIPT, mode: "exec", ref: long, timeoutMs: 5_000 }, AbortSignal.any([signal, lifetime.signal]), { host: true });
      const done = read.isError ? undefined : payload(read);
      const result = done?.ok === true ? object(done.result ?? {}) : undefined;
      if (!result || typeof result.min !== "number" || typeof result.max !== "number" || !Array.isArray(result.grid)) return `Kumi couldn't read how this parameter shows its values; give a number in its range.`;
      map = { min: result.min, max: result.max, items: Array.isArray(result.items) ? result.items.filter((item): item is string => typeof item === "string") : [],
        grid: result.grid.filter((pair): pair is [number, string] => Array.isArray(pair) && typeof pair[0] === "number" && typeof pair[1] === "string") };
      displayMaps.set(long, map);
      if (displayMaps.size > 4096) displayMaps.delete(displayMaps.keys().next().value!);
    }
    return valueForDisplay(map, text);
  }

  /** Whether parameters go the fast way (fast.ts): this bridge runs Python, and it isn't turned off. */
  const fastOn = () => options.fast !== false && process.env.KUMI_FAST !== "0" && supported({ since: PYTHON_BRIDGE }) && Boolean(tools?.has("live_run_python"));
  /**
   * A parameter named on a device, found once while Kumi's references hold: where it is on the device
   * and its range. Forgotten when they're retired, or devices move; the scripts check the name anyway.
   */
  const fastFound = new Map<string, FastFound & { index: number }>();
  let fastGeneration = -1;
  /** Kumi's own Python in Live, for a fast change: its result, or why not (`sent`: it reached Live, so it may have happened). */
  async function runFast(code: string, signal: AbortSignal): Promise<{ result: unknown } | { error: string; sent: boolean }> {
    let called: CallToolResult;
    try { called = await tools!.call("live_run_python", { code, mode: "exec", timeoutMs: 10_000 }, signal, { host: true }); }
    catch { return { error: "Live didn't answer", sent: true }; }
    if (called.isError) return { error: resultText(called).slice(0, 600), sent: uncertain(called) };
    let body: JsonObject;
    try { body = payload(called); } catch { return { error: "Kumi couldn't read Live's answer", sent: true }; }
    if (body.ok !== true) return { error: String(object(body.error ?? {}).message ?? "Live refused it").slice(0, 600), sent: false };
    return { result: body.result };
  }

  /**
   * A device's parameters set in one trip into Live (fast.ts), with one trip more first when one is
   * named or its value is given as Live shows it. HISTORY gets the change as ever, and its undo puts
   * back only what's still where Kumi left it.
   */
  async function fastParameters(kind: ChangeKind, input: JsonObject, signal: AbortSignal): Promise<{ text: string; isError: boolean }> {
    const deviceRef = typeof input.deviceRef === "string" ? input.deviceRef : undefined;
    if (!deviceRef) return { text: "Name the device (deviceRef) whose parameter this is.", isError: true };
    if (fastGeneration !== observationGeneration) { fastFound.clear(); fastGeneration = observationGeneration; }
    const several = Array.isArray(input.values);
    const asked = (several ? input.values as unknown[] : [input]).map((item) => object(item));
    if (!asked.length) return { text: "Give at least one parameter and its value.", isError: true };
    const numeric = (value: unknown) => (typeof value === "number" ? value : typeof value === "string" && value.trim() !== "" && Number.isFinite(Number(value)) ? Number(value) : undefined);
    const steps = asked.map((item) => ({ ref: typeof item.parameterRef === "string" ? item.parameterRef : undefined, name: typeof item.parameter === "string" ? item.parameter : undefined,
      number: numeric(item.value), text: typeof item.value === "string" && numeric(item.value) === undefined ? item.value : undefined }));
    for (const step of steps) {
      if (!step.ref && !step.name) return { text: "Each parameter needs its parameterRef from discovery, or its name as parameter (\"Drive\").", isError: true };
      if (step.number === undefined && step.text === undefined) return { text: `Give ${step.name ?? "the parameter"} a value: a number in its range, or what Live shows ("2 dB").`, isError: true };
    }
    const key = (step: (typeof steps)[number]) => step.ref ?? `${deviceRef}\u0000${step.name!.trim().toLowerCase()}`;
    const mapKey = (step: (typeof steps)[number], index?: number) => step.ref ?? `${deviceRef}\u0000#${index}`;
    // The first trip, for what Kumi doesn't know yet: a named parameter's place, Live's text across a range.
    const unknown = steps.filter((step) => (step.name && !step.ref && !fastFound.has(key(step))) || (step.text !== undefined && !displayMaps.has(mapKey(step, fastFound.get(key(step))?.index))));
    if (unknown.length) {
      const found = await runFast(findScript(unknown.map((step) => (step.ref ? { ref: step.ref, map: step.text !== undefined } : { device: deviceRef, parameter: step.name!, map: step.text !== undefined }))), signal);
      if ("error" in found) return { text: `Kumi couldn't find those parameters on the device: ${found.error}`, isError: true };
      const rows = Array.isArray(found.result) ? found.result.map((row) => object(row)) : [];
      for (const [index, step] of unknown.entries()) {
        const row = rows[index] ?? {};
        if (Array.isArray(row.missing)) return { text: `The device has no parameter called ${JSON.stringify(step.name!.slice(0, 64))}; its parameters include ${row.missing.slice(0, 12).join(", ")}.`, isError: true };
        if (typeof row.error === "string" || typeof row.name !== "string" || typeof row.min !== "number" || typeof row.max !== "number") return { text: `Kumi couldn't read ${step.name ?? "that parameter"} in Live: ${String(row.error ?? "no answer")}. Discover the device again.`, isError: true };
        if (typeof row.index === "number") fastFound.set(key(step), { index: row.index, name: row.name, min: row.min, max: row.max });
        if (Array.isArray(row.grid)) displayMaps.set(mapKey(step, typeof row.index === "number" ? row.index : undefined), { min: row.min, max: row.max,
          items: Array.isArray(row.items) ? row.items.filter((item): item is string => typeof item === "string") : [],
          grid: row.grid.filter((pair): pair is [number, string] => Array.isArray(pair) && typeof pair[0] === "number" && typeof pair[1] === "string") });
      }
    }
    // Each value as the parameter takes it, and where the parameter is.
    const targets: (FastTarget & { value: number })[] = [];
    for (const step of steps) {
      const place = step.ref ? undefined : fastFound.get(key(step))!;
      const target: FastTarget = step.ref ? { ref: step.ref } : { device: deviceRef, index: place!.index, name: place!.name };
      let value = step.number;
      if (value === undefined) {
        const placed = valueForDisplay(displayMaps.get(mapKey(step, place?.index))!, step.text!);
        if (typeof placed === "string") return { text: placed, isError: true };
        value = placed;
      }
      targets.push({ ...target, value });
    }
    signal.throwIfAborted();
    changesThisTurn++;
    const set = await runFast(setScript(targets), AbortSignal.any([lifetime.signal, AbortSignal.timeout(changeTimeoutMs)]));
    if ("error" in set) {
      if (!set.sent) return { text: `Live didn't change them: ${set.error}`, isError: true };
      remember(newRecord(kind, { title: `${steps.length === 1 ? steps[0]!.name ?? "A parameter" : `${steps.length} parameters`} (unconfirmed)` }, "unsure", now().getTime()), "");
      return { text: "Live didn't confirm this change, so it may or may not have happened. Tell the producer to check Live; discover again before more changes.", isError: true };
    }
    const result = object(set.result);
    const rows = (Array.isArray(result.items) ? result.items : []).map((row) => object(row) as unknown as FastSet);
    const device = { ref: deviceRef, name: typeof result.device === "string" ? result.device : undefined, trackRef: object(result.track).ref };
    // The shapes the bridge's preview and apply have, so HISTORY says it the same way.
    const shownRows = rows.map((row, index) => { const target = targets[index]!; return { ref: "ref" in target ? target.ref : `${deviceRef}#${target.index}`, name: row.name, currentValue: row.prior, proposedValue: row.value, min: row.min, max: row.max, displayValue: row.priorDisplay }; });
    const summary = several || rows.length > 1
      ? kind.summarize({ device, parameters: shownRows }, input, knownTrack, { parameters: shownRows.map((row, index) => ({ ref: row.ref, displayValue: rows[index]!.display })) })
      : kind.summarize({ device, parameter: shownRows[0] }, input, knownTrack, { displayValue: rows[0]?.display });
    const record = newRecord(kind, summary, "applied", now().getTime());
    remember(record, "");
    const entry = changes.get(record.id);
    if (entry) entry.revert = targets.map((target, index) => ({ ...(("ref" in target) ? { ref: target.ref } : { device: target.device, index: target.index, name: target.name }), prior: rows[index]!.prior, applied: rows[index]!.value }));
    const reply = { changed: record.title, change: record.id, state: record.state, ...(summary.lines?.length ? { lines: summary.lines } : {}),
      live: { parameters: rows.map((row) => ({ name: row.name, value: row.value, displayValue: row.display })) } };
    return { text: JSON.stringify(reply), isError: false };
  }
  /** A fast change undone: its parameters put back, each only if it's still where Kumi left it. */
  async function fastRevert(revert: FastRevert[], signal: AbortSignal): Promise<{ back: number; moved: string[]; gone: string[] } | string> {
    const done = await runFast(revertScript(revert), AbortSignal.any([signal, lifetime.signal, AbortSignal.timeout(changeTimeoutMs)]));
    if ("error" in done) return done.error;
    const result = object(done.result);
    const names = (value: unknown) => (Array.isArray(value) ? value.filter((item): item is string => typeof item === "string").slice(0, 16) : []);
    return { back: typeof result.back === "number" ? result.back : 0, moved: names(result.moved), gone: names(result.gone) };
  }
  function changeContext(signal: AbortSignal): ChangeContext {
    return {
      sample: (path) => samples.get(path) ?? audioFileAt(path),
      async parameters(deviceRef) {
        return (await deviceParameters(deviceRef, ["ref", "name"], signal))
          .filter((row): row is JsonObject & { ref: string; name: string } => typeof row.ref === "string" && typeof row.name === "string").map((row) => ({ ref: row.ref, name: row.name }));
      },
      async ranges(deviceRef) {
        return (await deviceParameters(deviceRef, ["ref", "name", "min", "max", "value", "displayValue"], signal))
          .filter((row): row is JsonObject & { ref: string; name: string } => typeof row.ref === "string" && typeof row.name === "string")
          .map((row) => ({ ref: row.ref, name: row.name, ...(typeof row.min === "number" ? { min: row.min } : {}), ...(typeof row.max === "number" ? { max: row.max } : {}),
            ...(typeof row.value === "number" ? { value: row.value } : {}), ...(typeof row.displayValue === "string" ? { display: row.displayValue } : {}) }));
      },
      valueFor: (parameterRef, text) => valueForText(parameterRef, text, signal),
      async pick(selector: SampleSelector) {
        const named = (selector.folders ?? []).map((folder) => folderPath(folder)).filter((folder): folder is string => Boolean(folder));
        const found = await findSamples({ folders: named.length ? named : defaultSampleFolders(), words: selector.words ?? [], limit: 50, random: selector.random === true || !(selector.words ?? []).length, signal });
        const choice = found.samples.find((sample) => !picked.has(sample.path));
        if (!choice) return undefined;
        picked.add(choice.path); samples.delete(choice.path); samples.set(choice.path, choice);
        return choice;
      },
    };
  }

  /** A step with `each: { note: [36, 37, 38] }` runs once per value, with that input field set to it;
   * with several lists of one length (parameterRef and value), the i-th run takes the i-th of each. */
  function expandStep(raw: unknown, index: number): unknown[] | string {
    const item = raw && typeof raw === "object" && !Array.isArray(raw) ? raw as JsonObject : {};
    const each = item.each && typeof item.each === "object" && !Array.isArray(item.each) ? Object.entries(item.each as JsonObject) : [];
    if (!each.length) return [raw];
    const runs = Array.isArray(each[0]![1]) ? (each[0]![1] as unknown[]).length : -1;
    if (runs < 0 || each.some(([, values]) => !Array.isArray(values) || values.length !== runs)) return `Step ${index + 1}: each gives input fields lists of one length, one value per run.`;
    const base = item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input as JsonObject : {};
    return Array.from({ length: Math.min(runs, MAX_CHANGES_PER_TURN + 1) }, (_, run) => ({ tool: item.tool, input: { ...base, ...Object.fromEntries(each.map(([field, values]) => [field, (values as unknown[])[run]])) } }));
  }

  /**
   * Several changes in one call: each step runs exactly as its own tool would (the same checks,
   * HISTORY entry and undo), in order, and stops at the first that fails. "@name" in a step's
   * input stands for what an earlier step marked `as: "name"` made. One call instead of a model
   * round trip per change, which is most of the time a multi-step request takes. A plan that isn't
   * valid throughout changes nothing.
   */
  async function makeChanges(input: JsonObject, signal: AbortSignal): Promise<{ text: string; isError: boolean; reply?: string }> {
    const steps: unknown[] = [];
    for (const [index, raw] of (Array.isArray(input.steps) ? input.steps : []).entries()) {
      const expanded = expandStep(raw, index);
      if (typeof expanded === "string") return { text: expanded, isError: true };
      steps.push(...expanded);
      if (steps.length > MAX_CHANGES_PER_TURN) break;
    }
    if (!steps.length || steps.length > MAX_CHANGES_PER_TURN) return { text: `Give 1 to ${MAX_CHANGES_PER_TURN} steps in all.`, isError: true };
    const plan = runPlan(signal);
    plan.add(steps); plan.close();
    return plan.result(input.final === true);
  }

  /**
   * make_changes while the model is still writing it: each step starts as soon as it's whole, so
   * the changes happen alongside the writing rather than after it. (A step found invalid stops the
   * plan there; the steps before it have happened.)
   */
  function streamChanges(signal: AbortSignal, onStart: () => void): StreamingCall {
    const plan = runPlan(signal, onStart);
    const received: unknown[] = [];
    const take = (raw: unknown) => {
      const index = received.length; received.push(raw);
      if (plan.failed) return;
      const expanded = expandStep(raw, index);
      if (typeof expanded === "string") plan.fail(expanded);
      else if (plan.count + expanded.length > MAX_CHANGES_PER_TURN) plan.fail(`Give 1 to ${MAX_CHANGES_PER_TURN} steps in all.`);
      else plan.add(expanded);
    };
    const scan = stepScanner(take);
    return {
      get started() { return plan.started; },
      push: scan,
      async finish(input) {
        // Nothing arrived early (the provider sent the input whole): the plan is checked whole, as ever.
        if (!received.length) { void plan.abandon(); return input ? makeChanges(input, signal) : { text: "Tool arguments must be a JSON object.", isError: true }; }
        if (!input) plan.fail("The rest of the plan wasn't valid JSON, so it stopped there.");
        else {
          const all = Array.isArray(input.steps) ? input.steps : [];
          if (received.some((raw, index) => JSON.stringify(raw) !== JSON.stringify(all[index]))) plan.fail("The plan's steps changed as they were written, so it stopped there.");
          else for (const raw of all.slice(received.length)) take(raw);
        }
        plan.close();
        return plan.result(input?.final === true);
      },
      abandon: () => plan.abandon(),
    };
  }

  /**
   * A track's devices, level by level: the track's own, then each rack's chains read together
   * (one of Live's display ticks per level), to a bound. A Drum Rack's pads are listed, not opened.
   * The model's references aren't touched: this is for FOCUS and for checking a pin.
   */
  async function readDeviceTree(trackRef: string, signal: AbortSignal): Promise<DeviceTree | undefined> {
    if (!available || lost || !tools?.has("live_discover") || !/^\d+:track:\d+$/.test(trackRef)) return undefined;
    const fields = ["parentRef", "name", "className", "canHaveChains", "canHaveDrumPads", "chainList", "deviceType"];
    const read = async (parent: string): Promise<JsonObject[]> => {
      const rows: JsonObject[] = []; let cursor: string | undefined;
      for (let page = 0; page < 10_000; page++) {
        const result = await tools!.call("live_discover", { kind: "device", parent, fields, limit: pageLimit(), ...(cursor ? { cursor } : {}) }, signal, { host: true });
        if (result.isError) throw new ObservationError("Live didn't list the devices");
        const body = payload(result);
        rows.push(...(Array.isArray(body.items) ? body.items as JsonObject[] : []));
        cursor = typeof body.nextCursor === "string" ? body.nextCursor : undefined;
        if (!cursor) break;
      }
      return rows;
    };
    const node = (row: JsonObject): DeviceNode => ({
      ref: String(row.ref), name: typeof row.name === "string" ? row.name.slice(0, 256) : "Device",
      ...(typeof row.className === "string" ? { className: row.className.slice(0, 128) } : {}),
      ...(typeof row.canHaveChains === "boolean" ? { canHaveChains: row.canHaveChains } : {}),
      ...(typeof row.canHaveDrumPads === "boolean" ? { canHaveDrumPads: row.canHaveDrumPads } : {}),
      ...(row.deviceType === "instrument" || row.deviceType === "audio_effect" || row.deviceType === "midi_effect" ? { deviceType: row.deviceType } : {}),
      ...(Array.isArray(row.chainList) ? { chains: (row.chainList as JsonObject[]).filter((chain) => chain && typeof chain.ref === "string").slice(0, 128)
        .map((chain) => ({ ref: String(chain.ref), name: typeof chain.name === "string" ? chain.name.slice(0, 256) : "Chain" })) } : {}),
    });
    try {
      const devices = (await read(trackRef)).map(node);
      let level: ChainNode[] = devices.filter((device) => device.canHaveDrumPads !== true).flatMap((device) => device.chains ?? []);
      let count = devices.length;
      for (let depth = 0; depth < 32 && level.length && count < 100_000; depth++) {
        const read_ = await Promise.all(level.map((chain) => read(chain.ref)));
        const next: ChainNode[] = [];
        level.forEach((chain, index) => {
          chain.devices = read_[index]!.map(node); count += chain.devices.length;
          for (const device of chain.devices) if (device.canHaveDrumPads !== true) next.push(...(device.chains ?? []));
        });
        level = next;
      }
      return { trackRef, devices };
    } catch { signal.throwIfAborted(); return undefined; }
  }

  /**
   * What the producer pointed at in Kumi, checked against the Set now: still there (by its reference,
   * or by name where it was), it's given to the model with a reference for this turn; gone, it says so.
   */
  async function checkPin(pin: PinnedNode, signal: AbortSignal): Promise<JsonObject> {
    if (pin.live) {
      // Pointed at in Live: its reference holds while Live's references do (the same epoch).
      if (!pin.ref.startsWith(`${currentEpoch}:`)) return { gone: `The producer pointed at “${pin.name}” in Live, but Live's references changed since: say so, and ask what they mean.` };
      const refKind = /^\d+:([a-z_]+):/.exec(pin.ref)?.[1] ?? "";
      const discovered = ({ track: "track", device: "device", chain: "chain", scene: "scene", clip_slot: "clip-slot", clip: "session-clip", arrangement_clip: "arrangement-clip" } as Record<string, string>)[refKind];
      if (discovered) refs.set(pin.ref, discovered);
      const words = pin.time ? "this part of the Arrangement" : `this ${pin.node === "clip-slot" ? "slot" : pin.node}`;
      return { ref: shortRef(pin.ref), kind: pin.node, name: pin.name, ...(pin.trail.length ? { in: pin.trail.join(" › ") } : {}), ...(pin.track && pin.node !== "track" ? { track: pin.track } : {}),
        ...(pin.time ? { fromBeat: pin.time.fromBeat, toBeat: pin.time.toBeat, spans: `${bars(pin.time.fromBeat)} to ${bars(pin.time.toBeat)}` } : {}),
        note: `The producer pointed at this in Live (right-click): "this", "${words}" or "here" in their message means it.` };
    }
    const tree = await readDeviceTree(pin.trackRef, signal).catch(() => undefined);
    const found: { ref: string; trail: string[] }[] = [];
    const walk = (devices: readonly DeviceNode[], trail: string[]) => {
      for (const device of devices) {
        if (pin.node === "device" && device.name === pin.name) found.push({ ref: device.ref, trail });
        for (const chain of device.chains ?? []) {
          if (pin.node === "chain" && chain.name === pin.name) found.push({ ref: chain.ref, trail: [...trail, device.name] });
          walk(chain.devices ?? [], [...trail, device.name, chain.name]);
        }
      }
    };
    if (tree) walk(tree.devices, []);
    const same = found.find((item) => item.ref === pin.ref) ?? found.find((item) => item.trail.join("\u0000") === pin.trail.join("\u0000"));
    if (!same) return { gone: `The producer pointed at “${pin.name}”${pin.track ? ` on ${pin.track}` : ""} in Kumi, and it isn't there any more: say so, and ask what they mean.` };
    refs.set(same.ref, pin.node);
    return { ref: shortRef(same.ref), kind: pin.node, name: pin.name, ...(same.trail.length ? { in: same.trail.join(" › ") } : {}), ...(pin.track ? { track: pin.track } : {}),
      ...(pin.siblings.length ? { nextTo: pin.siblings.slice(0, 12) } : {}), note: "The producer pointed at this in Kumi: \"this\", \"this device\" or \"this group\" in their message means it." };
  }

  /** Saved versions of the Set copied this session (file, size, time): one copy each. */
  const copied = new Set<string>();
  /**
   * Before a plan of three steps or more, or one that deletes, a copy of the Set as last saved, next
   * to it (the bridge's verified backup), once for each saved version. Unsaved work isn't in the file,
   * so it isn't in the copy; Kumi's own changes have their undo in HISTORY. Where it is, or nothing
   * when there's no saved file or the bridge can't; the plan goes ahead either way.
   */
  async function keepCopy(signal: AbortSignal): Promise<string | undefined> {
    const path = project?.path;
    if (!path || !BACKUP_TOOLS.every((name) => tools?.has(name))) return undefined;
    let version: string;
    try { const stats = statSync(path); version = `${path}:${stats.size}:${stats.mtimeMs}`; } catch { return undefined; }
    if (copied.has(version)) return undefined;
    try {
      const previewed = await tools!.call("live_project_backup_preview", { confirmation: "backup", allowedRoot: dirname(path) }, signal, { host: true });
      if (previewed.isError) return undefined;
      const transactionId = payload(previewed).transactionId;
      if (typeof transactionId !== "string") return undefined;
      const applied = await tools!.call("live_project_backup_apply", { transactionId, confirmation: "apply", idempotencyKey: randomUUID() }, signal, { host: true });
      const backup = applied.isError ? undefined : payload(applied).backup;
      if (typeof backup !== "string") return undefined;
      copied.add(version);
      return backup;
    } catch { signal.throwIfAborted(); return undefined; }
  }

  /**
   * A plan's steps run in order as they arrive (all at once, or one by one as the model writes
   * them). Steps in a row on one device wait for the next to arrive, so they still become one
   * change in one Live request. `close` says no more are coming.
   */
  function runPlan(signal: AbortSignal, onStart?: () => void) {
    const steps: unknown[] = [];
    let closed = false; let abandoned = false; let started = false;
    let failure: { at: number; error: string } | undefined;
    let wake: (() => void) | undefined;
    const nudge = () => { const resolve = wake; wake = undefined; resolve?.(); };
    const until = async (ready: () => boolean) => { while (!ready()) await new Promise<void>((resolve) => { wake = resolve; }); };
    // A cancelled turn wakes the plan too, so it stops (and stops what it started) rather than wait for steps that won't come.
    const known = (index: number) => steps.length > index || closed || failure !== undefined || abandoned || signal.aborted;
    signal.addEventListener("abort", nudge, { once: true });
    const made = new Map<string, string>();
    const done: JsonObject[] = [];
    /** Where the copy of the Set this plan kept first is, if it kept one. */
    let copy: string | undefined; let copyChecked = false;
    // Steps in a row on one device become one change, in one Live request, when the bridge can:
    // samples onto a rack's pads, and parameters of a device.
    const schemaOf = (kind: ChangeKind) => object(object(tools?.tool(kind.preview)?.inputSchema ?? {}).properties ?? {});
    const batches = [
      { tool: "load_sample_to_pad", kind: CHANGES.find((kind) => kind.tool === "load_samples_to_pads")!, most: 16, what: "pads",
        offered: (kind: ChangeKind) => { const action = object(schemaOf(kind).action ?? {}); return Array.isArray(action.enum) && action.enum.includes("load-samples"); },
        input: (group: JsonObject[]) => ({ deviceRef: group[0]!.deviceRef ?? null, pads: group.map((step) => ({ note: step.note ?? null, sample: step.sample ?? null, ...(step.instrument === "Drum Sampler" ? { instrument: "Drum Sampler" } : {}) })) }) },
      { tool: "set_device_parameter", kind: CHANGES.find((kind) => kind.tool === "set_device_parameters")!, most: 64, what: "parameters",
        offered: (kind: ChangeKind) => "values" in schemaOf(kind),
        input: (group: JsonObject[]) => ({ deviceRef: group[0]!.deviceRef ?? null, values: group.map((step) => ({ ...(typeof step.parameter === "string" && step.parameterRef === undefined ? { parameter: step.parameter } : { parameterRef: step.parameterRef ?? null }), value: step.value ?? null })) }) },
    ];
    const batchStep = (value: unknown, tool: string, device?: unknown) => {
      const item = value && typeof value === "object" && !Array.isArray(value) ? value as JsonObject : {};
      const stepInput = item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input as JsonObject : {};
      return item.tool === tool && item.as === undefined && typeof stepInput.deviceRef === "string" && (device === undefined || stepInput.deviceRef === device);
    };
    const resolve = (value: unknown, step: number): unknown => {
      if (typeof value === "string" && /^@[a-z][a-z0-9_]{0,31}$/i.test(value)) {
        const found = made.get(value.slice(1));
        if (!found) throw new ObservationError(`step ${step} refers to ${value}, which no earlier step made`);
        return found;
      }
      if (Array.isArray(value)) return value.map((item) => resolve(item, step));
      if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, resolve(item, step)]));
      return value;
    };
    type Stop = { step: number; tool: unknown; error: string };
    // What this plan's own steps set going: stopped again if the plan doesn't finish.
    const running: { playing?: boolean; recording?: string | undefined } = {};
    // The whole plan is one Cmd-Z in Live (a step opened before its first change, closed after its last or when
    // it stops), where the bridge has Live's undo steps. Kumi's own undo stays per change, in HISTORY.
    let undoStep: string | undefined; let opening: Promise<void> | undefined;
    // Opening and closing aren't the turn's to cancel: an Esc mid-opening must still leave the step closed.
    const lasting = () => AbortSignal.any([lifetime.signal, AbortSignal.timeout(10_000)]);
    const openUndoStep = async () => {
      if (undoStep !== undefined || !tools?.has("live_undo_step_begin") || !tools.has("live_undo_step_end")) return;
      undoStep = "";
      opening = (async () => {
        try { const opened = payload(await tools.call("live_undo_step_begin", { label: "Kumi", timeoutMs: 600_000 }, lasting(), { host: true })); if (typeof opened.stepId === "string") undoStep = opened.stepId; }
        catch { /* Live's undo then has a step for each change, as before */ }
      })();
      await opening;
    };
    const closeUndoStep = async () => {
      await opening?.catch(() => undefined);
      const id = undoStep; undoStep = undefined;
      if (!id) return;
      try { await tools!.call("live_undo_step_end", { stepId: id }, lasting(), { host: true }); }
      catch { /* the bridge closes it when the connection goes, and Live's side when its time is up */ }
    };
    const steps_ = async (): Promise<Stop | undefined> => {
      for (let index = 0; ;) {
        await until(() => known(index));
        if (abandoned) return undefined;
        signal.throwIfAborted();
        if (index >= steps.length) return failure?.at === index ? { step: index + 1, tool: null, error: failure.error } : undefined;
        const step = index + 1;
        const raw = steps[index];
        const item = raw && typeof raw === "object" && !Array.isArray(raw) ? raw as JsonObject : {};
        const stop = (error: string): Stop => ({ step, tool: item.tool ?? null, error: error.slice(0, 600) });
        // Earlier steps of this plan just confirmed Live is the same, so later ones skip that check.
        const confirmed = done.length > 0;
        const device = (item.input as JsonObject | undefined)?.deviceRef;
        let run = 1;
        // A step that could join the next one in a single change waits to see it (or the plan's end).
        if (batches.some((candidate) => batchStep(raw, candidate.tool))) { await until(() => known(index + 1)); if (abandoned) return undefined; signal.throwIfAborted(); }
        const batch = batches.find((candidate) => batchStep(raw, candidate.tool) && batchStep(steps[index + 1], candidate.tool, device));
        if (batch) {
          // A rack loaded just now changed the bridge's tools; read them again before asking.
          try { await ensureCatalog(signal); } catch { signal.throwIfAborted(); }
          if (batch.offered(batch.kind)) {
            while (run < batch.most) {
              await until(() => known(index + run));
              if (abandoned) return undefined;
              signal.throwIfAborted();
              if (index + run < steps.length && batchStep(steps[index + run], batch.tool, device)) run++; else break;
            }
          }
        }
        if (!started) { started = true; onStart?.(); }
        if (!copyChecked && (index >= 2 || /^delete_/.test(String(item.tool)))) { copyChecked = true; copy = await keepCopy(signal); }
        await openUndoStep();
        if (batch && run > 1) {
          let inputs: JsonObject[];
          try { inputs = steps.slice(index, index + run).map((other, offset) => resolve((other as JsonObject).input, step + offset) as JsonObject); }
          catch (error) { return stop(error instanceof Error ? error.message : "a reference didn't resolve"); }
          const outcome = await change(batch.kind, batch.input(inputs), signal, confirmed);
          let reply: JsonObject = {};
          try { reply = JSON.parse(outcome.text) as JsonObject; } catch { reply = {}; }
          if (outcome.isError) return stop(`${batch.what} ${step}–${step + run - 1}, as one change: ${outcome.text}`);
          const lines = Array.isArray(reply.lines) ? reply.lines : [];
          for (let offset = 0; offset < run; offset++) done.push({ step: step + offset, changed: typeof lines[offset] === "string" ? lines[offset] : reply.changed ?? null, change: reply.change ?? null });
          index += run;
          continue;
        }
        const kind = CHANGES.find((candidate) => candidate.tool === item.tool && !candidate.internal);
        const action = kind ? undefined : ACTIONS.find((candidate) => candidate.tool === item.tool);
        if (!kind && !action && item.tool !== WAIT) return stop(`${String(item.tool).slice(0, 64)} isn't one of Kumi's change tools`);
        const needs = kind ?? action;
        if (needs && !supported(needs)) return stop(tooOld(needs));
        let stepInput: JsonObject;
        try { stepInput = resolve(item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : {}, step) as JsonObject; }
        catch (error) { return stop(error instanceof Error ? error.message : "a reference didn't resolve"); }
        if (item.tool === WAIT) {
          // Recording and listening take as long as they take: wait by the clock, or by the beat at the Set's tempo.
          const seconds = typeof stepInput.seconds === "number" ? stepInput.seconds : typeof stepInput.beats === "number" && currentTempo ? stepInput.beats * 60 / currentTempo : NaN;
          if (!(seconds > 0 && seconds <= 1_800)) return stop("wait takes seconds (up to 1800) or beats");
          await delay(seconds * 1000, undefined, { signal });
          done.push({ step, changed: `waited ${Math.round(seconds * 10) / 10} s`, change: null });
          index++;
          continue;
        }
        if (action) {
          const outcome = await act(action, stepInput, signal);
          // A start Live didn't confirm may still have happened: the cleanup stops it either way.
          if (!outcome.isError || outcome.maybe) {
            if (outcome.done?.playing !== undefined) running.playing = outcome.done.playing || running.playing === true && outcome.maybe === true;
            if (outcome.done?.recording !== undefined) running.recording = outcome.done.recording ? String(stepInput.lane ?? "arrangement") : outcome.maybe ? running.recording : undefined;
          }
          if (outcome.isError) return stop(outcome.text);
          done.push({ step, changed: outcome.done?.title ?? null, change: null });
          index++;
          continue;
        }
        const outcome = await change(kind!, stepInput, signal, confirmed);
        let reply: JsonObject = {};
        try { reply = JSON.parse(outcome.text) as JsonObject; } catch { reply = {}; }
        if (outcome.isError) return stop(typeof reply.changed === "string" ? `${reply.changed}: ${outcome.text}` : outcome.text);
        if (typeof item.as === "string" && typeof reply.ref === "string") made.set(item.as, reply.ref);
        done.push({ step, changed: reply.changed ?? null, change: reply.change ?? null, ...(typeof reply.ref === "string" ? { ref: reply.ref } : {}), ...(Array.isArray(reply.lines) ? { lines: reply.lines } : {}) });
        index++;
      }
    };
    /** A plan that recorded or played and then stopped short (a failed step, a cancelled turn) doesn't leave Live running. */
    const quiet = async (): Promise<string | undefined> => {
      if (!running.recording && !running.playing) return undefined;
      const settle = AbortSignal.any([lifetime.signal, AbortSignal.timeout(changeTimeoutMs)]);
      const what = running.recording ? "the recording and playback" : "playback";
      if (!await stopEverything(settle)) {
        // Said in NOW as well as to the model: after a cancel there's no model to tell the producer.
        try { options.onAction?.({ title: `Live may still be ${running.recording ? "recording" : "playing"}: press space in Live, or /stop` }); } catch { /* a listener failure must not affect Live */ }
        return `The plan didn't finish and Live may still be ${running.recording ? "recording" : "playing"}; tell the producer.`;
      }
      try { options.onAction?.({ title: running.recording ? "Recording stopped" : "Stopped", playing: false, recording: false }); } catch { /* a listener failure must not affect Live */ }
      return `Kumi stopped ${what}, since the plan didn't finish.`;
    };
    const settled = (async (): Promise<Stop | undefined> => {
      let outcome: Stop | undefined; let finished = false;
      try {
        outcome = await steps_();
        finished = !outcome && !abandoned;
        return outcome;
      } finally {
        signal.removeEventListener("abort", nudge);
        await closeUndoStep();
        if (!finished) {
          const note = await quiet().catch(() => undefined);
          if (note && outcome) outcome.error = `${outcome.error} ${note}`.slice(0, 800);
        }
      }
    })();
    // A cancelled plan rejects here; whoever asks for the result (or abandons it) still sees that.
    void settled.catch(() => {});
    return {
      get started() { return started; },
      get failed() { return failure !== undefined; },
      get count() { return steps.length; },
      add(more: unknown[]) { if (!closed && !failure) { steps.push(...more); nudge(); } },
      /** The plan can't go on past the steps it has: `error` says why, at the next step. */
      fail(error: string) { if (!failure) { failure = { at: steps.length, error }; nudge(); } },
      close() { closed = true; nudge(); },
      abandon() { abandoned = true; nudge(); return settled.then(() => {}, () => {}); },
      async result(final: boolean): Promise<{ text: string; isError: boolean; reply?: string }> {
        const stopped = await settled;
        const kept = copy ? { copy, copyNote: "Before this, Kumi kept a copy of the Set as last saved, next to it. Tell the producer in a few words, with the file's name." } : {};
        await until(() => closed || abandoned);
        if (stopped) {
          // A plan refused before anything happened says only why.
          if (!done.length && failure?.at === 0) return { text: failure.error, isError: true };
          return { text: JSON.stringify({ done, stopped, ...(steps.length > stopped.step ? { skipped: steps.length - stopped.step } : {}), ...kept }), isError: true };
        }
        if (!done.length) return { text: `Give 1 to ${MAX_CHANGES_PER_TURN} steps in all.`, isError: true };
        const text = JSON.stringify({ done, ...kept });
        if (!final) return { text, isError: false };
        // The plan finished the request: Kumi says what changed, sparing the producer a model reply.
        // A line for each thing changed: a change of several parameters gives one for each.
        const lines = done.flatMap((item) => Array.isArray(item.lines) ? item.lines : [item.changed]).filter((line): line is string => typeof line === "string" && line.length > 0);
        const copied = copy ? `\n\nFirst, Kumi kept a copy of your Set as last saved, next to it: ${basename(copy)}` : "";
        return { text, isError: false, reply: `${lines.length === 1 ? `Done: ${lines[0]}.` : `Done:\n${lines.map((line) => `- ${line}`).join("\n")}`}${copied}` };
      },
    };
  }

  /** `settled`: an earlier change in the same plan just confirmed Live's epoch, so it isn't read again. */
  async function change(kind: ChangeKind, named: JsonObject, originalSignal: AbortSignal, settled = false): Promise<{ text: string; isError: boolean }> {
    const signal = AbortSignal.any([originalSignal, lifetime.signal]);
    const input = lengthen(named) as JsonObject;
    const lease = observationGeneration;
    try {
      signal.throwIfAborted();
      if (!available || lost || currentEpoch === undefined || !tools) throw new ObservationError(NO_CURRENT_LIVE);
      await ensureCatalog(signal); assertLease(lease, signal);
      // The bridge offers some tools only once the Set has what they work on (edit_rack once there's a
      // rack): a plan that just loaded one may be ahead of the bridge's catalog-changed notice, so the
      // catalog is read again before saying the change isn't available.
      if (!tools.has(kind.preview) || !tools.has(kind.apply)) {
        // tools/list uses the host's cached capabilities. Refresh Live status first so a newly
        // created clip or loaded device can advertise the operations it now supports.
        await guardEpoch(signal, currentEpoch, lease);
        await tools.refresh(signal); assertLease(lease, signal);
      }
      if (!tools.has(kind.preview) || !tools.has(kind.apply)) throw new ObservationError(kind.unavailable ?? "That change isn't available for the open Set right now");
      if (!supported(kind)) throw new ObservationError(tooOld(kind));
      if (changesThisTurn >= MAX_CHANGES_PER_TURN) throw new ObservationError(`That's ${MAX_CHANGES_PER_TURN} changes in one answer; carry on in the next one`);
      requireFreshReferences(input);
      if (kind.family === "parameter" && fastOn()) return await fastParameters(kind, input, signal);
      const prepared = kind.prepare ? await kind.prepare(input, changeContext(signal)) : input;
      if (typeof prepared === "string") return { text: prepared, isError: true };
      assertLease(lease, signal);
      const epoch = currentEpoch;
      if (!settled) await guardEpoch(signal, epoch, lease);
      const args = await appendAtEnd(kind, prepared, signal); assertLease(lease, signal);
      const previewed = await tools.call(kind.preview, args, signal, { host: true }); assertLease(lease, signal);
      if (previewed.isError) {
        const more = kind.explain ? await kind.explain(JSON.stringify(previewed), args, changeContext(signal)).catch(() => undefined) : undefined;
        return { text: `${JSON.stringify(previewed)}${more ? ` ${more}` : ""}`, isError: true };
      }
      const preview = payload(previewed);
      if (preview.epoch !== undefined && preview.epoch !== epoch) changed();
      const { transactionId, confirmation } = preview;
      if (typeof transactionId !== "string" || !transactionId || transactionId.length > 256 || typeof confirmation !== "string" || !confirmation || confirmation.length > 512) {
        throw new ObservationError("The bridge's preview was malformed; nothing was changed");
      }
      const summary = kind.summarize(preview, args, knownTrack);
      signal.throwIfAborted();
      changesThisTurn++;
      // From here the change may happen. It runs to the end (bounded) even if the turn is cancelled,
      // so every change that reaches Live is recorded, with its undo.
      const settle = AbortSignal.any([lifetime.signal, AbortSignal.timeout(changeTimeoutMs)]);
      let applied: CallToolResult;
      try {
        applied = await tools.call(kind.apply, { transactionId, confirmation, idempotencyKey: randomUUID() }, settle, { host: true });
      } catch {
        remember(newRecord(kind, summary, "unsure", now().getTime()), transactionId);
        return { text: "Live didn't confirm this change, so it may or may not have happened. Tell the producer to check Live; discover again before more changes.", isError: true };
      }
      if (applied.isError) {
        if (!uncertain(applied)) return { text: JSON.stringify(applied), isError: true };
        remember(newRecord(kind, summary, "unsure", now().getTime()), transactionId);
        return { text: `Live couldn't confirm this change: ${JSON.stringify(applied)}`, isError: true };
      }
      let result: JsonObject;
      try { result = payload(applied); } catch {
        remember(newRecord(kind, summary, "unsure", now().getTime()), transactionId);
        return { text: "Kumi couldn't read Live's answer to this change, so it can't confirm whether it happened. Tell the producer to check Live; discover again before more changes.", isError: true };
      }
      const settledSummary = kind.summarize(preview, args, knownTrack, result);
      // A change Live can't take back stays in HISTORY as kept, with why, instead of an undo that would fail.
      const permanent = result.state === "applied" ? kind.permanent?.(args) : undefined;
      const record = { ...newRecord(kind, settledSummary, result.state === "applied" ? permanent ? "kept" : "applied" : "unsure", now().getTime()), ...(permanent ? { note: permanent } : {}) };
      const field = kind.family === "rename" ? "name" : kind.family === "color" ? "color" : undefined;
      const replaced = field && typeof args.ref === "string" ? known.get(args.ref) : undefined;
      remember(record, transactionId, field && replaced ? { ref: args.ref as string, field, ...(replaced[field] !== undefined ? { value: replaced[field] } : {}) } : undefined);
      // A later wait in beats counts at the new tempo.
      if (kind.tool === "set_tempo" && record.state === "applied" && typeof args.tempo === "number") currentTempo = args.tempo;
      // A renamed track keeps its new name in later HISTORY entries.
      if (kind.family === "rename" && summary.track && typeof args.ref === "string" && known.has(args.ref)) known.set(args.ref, { ...known.get(args.ref)!, name: summary.track.name });
      // Likewise its new colour.
      if (kind.family === "color" && record.colors && typeof args.ref === "string" && known.has(args.ref)) known.set(args.ref, { ...known.get(args.ref)!, color: record.colors.to });
      if (kind.restructures) {
        cursors.clear();
        // New tracks and scenes at given places move only what comes after them (return tracks follow the
        // regular ones); references before them stay good. Other restructures can move anything.
        const created = (Array.isArray(result.created) ? result.created : []).map((item) => (item && typeof item === "object" ? item as JsonObject : {}));
        const at = (pattern: RegExp) => created.map((item) => (typeof item.ref === "string" ? pattern.exec(item.ref) : null)).filter((match): match is RegExpExecArray => match !== null).map((match) => Number(match[1]));
        const tracksAt = at(/:track:(\d+)$/); const scenesAt = at(/:scene:(\d+)$/);
        if (kind.tool === "add_tracks_and_scenes" && created.length && tracksAt.length + scenesAt.length === created.length) {
          const fromTrack = tracksAt.length ? Math.min(...tracksAt) : Infinity; const fromScene = scenesAt.length ? Math.min(...scenesAt) : Infinity;
          for (const ref of new Set([...refs.keys(), ...shortRefs.keys()])) {
            const track = trackIndexOf(ref); const scene = sceneIndexOf(ref);
            if ((track !== undefined && track >= fromTrack) || (scene !== undefined && scene >= fromScene)) retire(ref);
          }
        } else { refs.clear(); known.clear(); shortRefs.clear(); longRefs.clear(); }
        for (const item of created) if (typeof item.ref === "string") retire(item.ref);
        for (const item of created) {
          const created = item && typeof item === "object" ? item as JsonObject : {};
          if (typeof created.ref === "string" && created.ref.length <= 256 && (created.kind === "track" || created.kind === "scene")) {
            refs.set(created.ref, created.kind);
            if (created.kind === "track" && typeof created.name === "string") known.set(created.ref, { name: created.name.slice(0, 256) });
          }
        }
      }
      // A device moved or deleted shifts the ones after it: on the tracks involved, earlier device,
      // parameter and chain references (and their short names) are retired.
      const shifted = DEVICE_SHIFTS.has(kind.tool);
      if (shifted || kind.restructures) fastFound.clear();
      if (shifted) {
        const tracks = new Set([args.deviceRef, args.ref, args.targetTrackRef, args.targetChainRef].filter((value): value is string => typeof value === "string")
          .map((ref) => trackIndexOf(ref)).filter((index): index is number => index !== undefined));
        for (const ref of new Set([...refs.keys(), ...shortRefs.keys()])) {
          if (/:(?:device|parameter|chain|drum_pad):/.test(ref) && !/:mixer:/.test(ref) && tracks.has(trackIndexOf(ref) ?? -1)) retire(ref);
        }
      }
      // What the change made (a new track, a loaded device) is usable at once, without discovering it.
      const produced = kind.produces?.(result);
      // Something new at a position gets a name of its own, not the one its old occupant had.
      if (produced && produced.ref.length <= 256 && !kind.restructures) unname(produced.ref);
      if (produced && produced.ref.length <= 256) refs.set(produced.ref, produced.kind);
      const { lines } = settledSummary;
      const reply = { changed: record.title, change: record.id, state: record.state, ...(produced ? { ref: shortRef(produced.ref) } : {}), ...(lines?.length ? { lines } : {}),
        ...(kind.restructures ? { note: kind.tool === "add_tracks_and_scenes" ? "Tracks and scenes after the new ones moved (return tracks among them): discover those again; earlier references still work, and the new ones in live.created are current."
          : "Track and scene positions moved; discover again before using earlier references (the new ones in live.created are current)." } : {}),
        ...(shifted ? { note: "Devices on the tracks involved moved along their chains: discover them (and their parameters) again before using earlier references." } : {}) };
      const full = JSON.stringify({ ...reply, live: shorten(result) });
      return { text: Buffer.byteLength(full) <= 16 * 1024 ? full : JSON.stringify(reply), isError: record.state !== "applied" && !permanent };
    } catch (error) {
      return { text: error instanceof ObservationError ? error.message : "The change failed before anything happened in Live; discover again, then retry.", isError: true };
    }
  }
  /**
   * Something that isn't a change to the Set (playing, launching, recording, selecting, showing):
   * the same preview and apply as a change, with the same checks, but no HISTORY entry or undo.
   */
  async function act(kind: ActionKind, named: JsonObject, originalSignal: AbortSignal, cleanup = false): Promise<{ text: string; isError: boolean; maybe?: boolean; done?: ReturnType<ActionKind["summarize"]> }> {
    // Live records onto one armed track only: another one armed (a leftover, or one the producer
    // armed) refuses the recording. It's disarmed first, each a change with its undo, and said.
    let disarmed: string[] = [];
    if (kind.tool === "record" && named.action === "start" && typeof named.destinationTrackRef === "string" && !cleanup) {
      const cleared = await disarmOthers([named.destinationTrackRef, ...(Array.isArray(named.alsoTrackRefs) ? named.alsoTrackRefs.filter((ref): ref is string => typeof ref === "string") : [])], originalSignal);
      if (typeof cleared === "string") return { text: cleared, isError: true };
      disarmed = cleared;
    }
    const recorded = await actOnce(kind, named, originalSignal, cleanup);
    const first = `after disarming ${disarmed.join(", ")}`;
    const outcome = disarmed.length && !recorded.isError ? { ...recorded, text: JSON.stringify({ ...JSON.parse(recorded.text) as JsonObject, disarmedFirst: disarmed }),
      ...(recorded.done ? { done: { ...recorded.done, title: `${recorded.done.title}, ${first}` } } : {}) } : recorded;
    // Stopping must work whatever Live is doing: when the ordinary stop is refused, stop everything.
    const stopping = (kind.tool === "play" && named.action === "stop") || (kind.tool === "record" && named.action === "stop");
    if (!outcome.isError || !stopping || originalSignal.aborted) return outcome;
    if (!await stopEverything(AbortSignal.any([lifetime.signal, AbortSignal.timeout(changeTimeoutMs)]))) return outcome;
    const done = kind.tool === "record" ? { title: "Recording stopped", recording: false, playing: false } : { title: "Stopped", playing: false };
    try { options.onAction?.(done); } catch { /* a listener failure must not affect Live */ }
    return { text: JSON.stringify({ done: done.title, note: "Live's ordinary stop was refused, so Kumi stopped clips, the transport and recording together." }), isError: false, done };
  }
  /** Disarms every armed track but those recorded (`keep`), each a change; their names, or why one couldn't be. */
  async function disarmOthers(keep: readonly string[], signal: AbortSignal): Promise<string[] | string> {
    const kept = new Set(keep.flatMap((ref) => [ref, lengthen(ref) as string]));
    const read = await invoke("live_discover", { kind: "track", fields: ["ref", "name", "armed"], limit: pageLimit() }, signal);
    // Unread, the recording's own check decides.
    if (read.isError) return [];
    let items: { ref?: unknown; name?: unknown; armed?: unknown }[] = [];
    try { items = ((JSON.parse(read.text) as { live?: { items?: typeof items } }).live?.items ?? []); } catch { return []; }
    const routing = CHANGES.find((candidate) => candidate.tool === "set_routing");
    const disarmed: string[] = [];
    for (const track of items.filter((item) => item.armed === true && typeof item.ref === "string" && !kept.has(item.ref))) {
      const name = typeof track.name === "string" ? track.name.slice(0, 80) : "a track";
      const outcome = routing ? await change(routing, { trackRef: track.ref as string, arm: false }, signal) : { text: "Live doesn't offer disarming here", isError: true };
      if (outcome.isError) return `${name} is armed too, and Live records onto one armed track only; Kumi couldn't disarm it: ${outcome.text.slice(0, 200)}`;
      disarmed.push(name);
    }
    return disarmed;
  }
  async function actOnce(kind: ActionKind, named: JsonObject, originalSignal: AbortSignal, cleanup: boolean): Promise<{ text: string; isError: boolean; maybe?: boolean; done?: ReturnType<ActionKind["summarize"]> }> {
    const signal = AbortSignal.any([originalSignal, lifetime.signal]);
    const input = lengthen(named) as JsonObject;
    const lease = observationGeneration;
    try {
      signal.throwIfAborted();
      if (!available || lost || currentEpoch === undefined || !tools) throw new ObservationError(NO_CURRENT_LIVE);
      await ensureCatalog(signal); if (!cleanup) assertLease(lease, signal);
      if (!tools.has(kind.preview) || !tools.has(kind.apply)) throw new ObservationError("Live doesn't offer that for the open Set right now");
      if (!supported(kind) && !cleanup) throw new ObservationError(tooOld(kind));
      const newer = kind.newer?.[String(input.action)];
      if (newer && !cleanup && !supported({ since: newer })) throw new ObservationError(tooOld({ since: newer }));
      if (!cleanup) requireFreshReferences(input);
      const prepared = kind.prepare ? kind.prepare(input) : input;
      if (typeof prepared === "string") return { text: prepared, isError: true };
      // Live records into the Set's folder (an unsaved Set's into its own, on the system disk).
      if (kind.tool === "record" && input.action === "start" && !cleanup) {
        const full = await (options.lowDisk ?? lowDisk)(project?.path ? dirname(project.path) : homedir(), 100 * MB, "Live records to");
        if (full) return { text: `${full} Nothing was recorded.`, isError: true };
      }
      const previewed = await tools.call(kind.preview, prepared, signal, { host: true }); if (!cleanup) assertLease(lease, signal);
      if (previewed.isError) return { text: JSON.stringify(previewed), isError: true };
      const preview = payload(previewed);
      const { transactionId, confirmation } = preview;
      if (typeof transactionId !== "string" || !transactionId || typeof confirmation !== "string" || !confirmation) throw new ObservationError("The bridge's preview was malformed; nothing happened");
      // Once sent, Live may have done it even when it doesn't say so: said as such, and kept for the cleanup.
      const unsure = { text: "Live didn't confirm this, so it may have happened: check Live (/stop stops it) before trying again.", isError: true, maybe: true, done: kind.summarize(preview, prepared, knownTrack) };
      let applied: CallToolResult;
      try { applied = await tools.call(kind.apply, { transactionId, confirmation, idempotencyKey: randomUUID() }, AbortSignal.any([lifetime.signal, AbortSignal.timeout(changeTimeoutMs)]), { host: true }); }
      catch { return unsure; }
      if (applied.isError) return uncertain(applied) ? unsure : { text: JSON.stringify(applied), isError: true };
      const done = kind.summarize(preview, prepared, knownTrack);
      if (!quiet) try { options.onAction?.(done); } catch { /* a listener failure must not affect Live */ }
      return { text: JSON.stringify({ done: done.title, live: shorten(payload(applied)) }), isError: false, done };
    } catch (error) {
      return { text: error instanceof ObservationError ? error.message : "That didn't happen in Live; discover again, then retry.", isError: true };
    }
  }

  /** Every device in the Set (to the bound discovery allows), with what identifies it and where it is. */
  async function allDevices(signal: AbortSignal): Promise<JsonObject[]> {
    const rows: JsonObject[] = [];
    let cursor: string | undefined;
    for (let page = 0; page < 10_000; page++) {
      const read = payload(await tools!.call("live_discover", { kind: "device", fields: ["ref", "parentRef", "objectIdentity", "name", "className"], limit: pageLimit(), ...(cursor ? { cursor } : {}) }, signal, { host: true }));
      rows.push(...(Array.isArray(read.items) ? read.items : []).map((item) => object(item)));
      cursor = typeof read.nextCursor === "string" && read.nextCursor ? read.nextCursor : undefined;
      if (!cursor) break;
    }
    return rows;
  }
  /** watch_me: note the Set now, or say what the producer changed since. */
  const watchingNow = (on: boolean) => { try { options.onWatch?.(on); } catch { /* a listener failure must not affect Live */ } };
  async function watch(input: JsonObject, originalSignal: AbortSignal): Promise<{ text: string; isError: boolean }> {
    const signal = AbortSignal.any([originalSignal, lifetime.signal, AbortSignal.timeout(60_000)]);
    try {
      if (!available || lost || !tools) throw new ObservationError("Live isn't connected, so Kumi can't watch it.");
      await ensureCatalog(signal);
      if (!PROJECT_TOOLS.slice(1).every((name) => tools!.has(name))) throw new ObservationError("This bridge can't compare the Set's states, so Kumi can't learn routines by watching.");
      if (input.action === "start") {
        const [exported, devices] = await Promise.all([exportPages(signal), allDevices(signal)]);
        watching = { set: currentSet, pages: exported, devices: new Set(devices.map((device) => String(device.objectIdentity))), at: now().getTime() };
        watchingNow(true);
        return { text: JSON.stringify({ watching: true, note: "Tell the producer to go ahead in Live and to say when they're done." }), isError: false };
      }
      if (!watching) return { text: "Kumi isn't watching yet: start first, before the producer does it.", isError: true };
      if (watching.set !== currentSet) { watching = undefined; watchingNow(false); return { text: "A different Set is open now, so there's nothing to compare; start again.", isError: true }; }
      const before = watching;
      const [exported, devices, tracks] = await Promise.all([exportPages(signal), allDevices(signal),
        pages({ kind: "track", fields: ["ref", "name", "mediaKind"], limit: pageLimit(), budget: wholeBudget() }, signal).then((read) => payload(read).items)]);
      const diff = payload(await tools.call("live_project_snapshot_diff", { beforePages: before.pages, afterPages: exported, limit: 200 }, signal, { host: true }));
      const { changes, more } = describeWatch(diff, before.pages, exported);
      const trackRows = (Array.isArray(tracks) ? tracks : []).map((item) => object(item));
      const trackName = new Map(trackRows.map((track) => [String(track.ref), String(track.name ?? "")]));
      // An added track is audio or MIDI, which the Set's snapshot doesn't say.
      for (const change of changes) {
        if (change.added !== "track") continue;
        const media = trackRows.find((track) => track.name === change.name)?.mediaKind;
        if (typeof media === "string") change.media = media;
      }
      // What the producer set on each device they added: the knobs away from Live's defaults.
      const added = devices.filter((device) => !before.devices.has(String(device.objectIdentity))).slice(0, WATCH_DEVICES);
      const settings = await Promise.all(added.map(async (device) => {
        const moved = (await deviceParameters(device.ref, ["name", "value", "defaultValue", "displayValue"], signal))
          .filter((parameter) => typeof parameter.defaultValue === "number" && typeof parameter.value === "number" && Math.abs(parameter.value - parameter.defaultValue) > 1e-6)
          .map((parameter) => ({ name: parameter.name ?? null, value: parameter.value, ...(typeof parameter.displayValue === "string" ? { shows: parameter.displayValue } : {}) }));
        const owner = trackName.get(String(device.parentRef));
        return { device: device.name ?? device.className ?? null, className: device.className ?? null, ...(owner ? { on: owner } : { inside: "a rack" }), knobs: moved.slice(0, 24) };
      }));
      watching = undefined; watchingNow(false);
      const seconds = Math.round((now().getTime() - before.at) / 1000);
      return { text: JSON.stringify({ watched: `${seconds} s`, changes, ...(more ? { more } : {}), ...(settings.length ? { devicesAdded: settings } : {}),
        ...(changes.length ? {} : { note: "Nothing in the Set changed while Kumi watched." }) }), isError: false };
    } catch (error) {
      signal.throwIfAborted();
      return { text: error instanceof ObservationError ? error.message : "Kumi couldn't compare the Set just now; try again.", isError: true };
    }
  }
  /**
   * Stop clips, the transport and recording at once, through the bridge's emergency stop, which
   * works whatever Live is doing. True when Live is stopped afterwards (or already was).
   */
  async function stopEverything(signal: AbortSignal): Promise<boolean> {
    try {
      if (!available || lost || !tools) return false;
      await ensureCatalog(signal);
      if (!tools.has(EMERGENCY_STOP) || !tools.has("live_discover")) return false;
      // The playback row alone (not the whole Set, which a big one makes slow or too large); read
      // again once if what's playing changed between the read and the stop.
      for (let attempt = 0; attempt < 2; attempt++) {
        const read = payload(await tools.call("live_discover", { kind: "session-playback", limit: 1 }, signal, { host: true }));
        const playback = object((Array.isArray(read.items) ? read.items : [])[0] ?? {});
        const transport = object(playback.transport ?? {});
        const targets = [...(Array.isArray(playback.firedTargets) ? playback.firedTargets : []), ...(Array.isArray(playback.playingTargets) ? playback.playingTargets : [])].map((target) => object(target));
        const expectedTargets = [...new Set(targets.map((target) => `${String(target.trackRef)}|${String(target.clipSlotRef)}|${String(target.sceneRef)}`))].sort();
        const session = transport.sessionRecord === true; const arrangement = transport.arrangementRecord === true;
        const expectedRecording = session && arrangement ? "both" : session ? "session" : arrangement ? "arrangement" : "stopped";
        if (transport.playing !== true && !expectedTargets.length && expectedRecording === "stopped") return true;
        const result = await tools.call(EMERGENCY_STOP, { confirmation: "emergency-stop", expectedTargets, expectedRecording, idempotencyKey: randomUUID() }, signal, { host: true });
        if (!result.isError) return true;
      }
      return false;
    } catch { return false; }
  }

  /** Undo one change through the bridge's guarded undo. Refusals keep the change and say why. */
  async function undoChange(target: string, signal: AbortSignal, discard = false): Promise<{ record?: ChangeRecord; text: string; isError: boolean }> {
    const entry = target === "last" ? [...changes.values()].reverse().find((item) => item.record.state === "applied" && !item.within) : changes.get(target);
    if (!entry) return { text: target === "last" ? "There's no change of Kumi's left to undo." : `There's no change ${target.slice(0, 32)} in this session.`, isError: true };
    if (entry.record.state === "undone") return { record: entry.record, text: JSON.stringify({ undone: entry.record.title, change: entry.record.id, already: true }), isError: false };
    if (entry.record.state === "expired") return { record: entry.record, text: entry.record.note ?? "Kumi can't undo this anymore.", isError: true };
    // A change only Live's own undo can take back isn't tried. One kept by a refused undo is: what
    // changed since may have gone back (a track inserted above, taken out again).
    if (entry.permanent) return { record: entry.record, text: entry.record.note ?? "Kumi can't take this back; Live's own undo (Cmd-Z in Live) can.", isError: true };
    try { await ensureCatalog(signal); } catch { /* reported just below */ }
    const update = (next: Partial<ChangeRecord>) => {
      const { note: _note, ...rest } = entry.record;
      entry.record = { ...rest, ...next };
      emitChange(entry.record);
      return entry.record;
    };
    if (entry.revert) {
      if (!available || lost || !tools?.has("live_run_python")) return { text: "Kumi can't reach Live right now, so it can't undo.", isError: true };
      signal.throwIfAborted();
      const back = await fastRevert(entry.revert, signal);
      if (typeof back === "string") return { record: update({ state: "unsure", note: "Live didn't confirm the undo; try again." }), text: `Live didn't confirm the undo: ${back}`, isError: true };
      scheduleSave(20_000);
      const left = [...back.moved, ...back.gone];
      if (!left.length) return { record: update({ state: "undone" }), text: JSON.stringify({ undone: entry.record.title, change: entry.record.id }), isError: false };
      // Only what's still where Kumi left it goes back: what was moved since stays as it is. Nothing put
      // back can be tried again (it may be moved back); part put back can't.
      if (back.back) delete entry.revert;
      const them = left.length === 1 ? "it" : "them";
      const note = back.back ? `Kumi put back ${back.back} of its parameters; ${left.join(", ")} changed in Live since, so Kumi left ${them}.`
        : `${left.join(", ")} changed in Live since Kumi set ${them}, so Kumi left ${them} as ${left.length === 1 ? "it is" : "they are"}.`;
      return { record: update({ state: "kept", note }), text: note, isError: true };
    }
    if (!available || lost || !tools?.has("live_undo")) return { text: "Kumi can't reach Live right now, so it can't undo.", isError: true };
    signal.throwIfAborted();
    // One key per change, so a retry after an unconfirmed undo reconciles instead of undoing twice.
    entry.undoKey ??= randomUUID();
    if (entry.members) {
      // Several changes as one: each taken back by its own undo, latest first, and HISTORY keeps the one line.
      const members = entry.members;
      await quietly([], async () => { for (const id of [...members].reverse()) if (changes.get(id)?.record.state !== "undone") await undoChange(id, signal).catch(() => undefined); });
      const left = members.filter((id) => changes.get(id)?.record.state !== "undone").length;
      if (!left) return { record: update({ state: "undone" }), text: JSON.stringify({ undone: entry.record.title, change: entry.record.id }), isError: false };
      const note = `Kumi took back ${members.length - left} of its ${members.length} changes; the rest changed in Live since, so Kumi left them.`;
      return { record: update({ state: "kept", note }), text: note, isError: true };
    }
    // A fast change Live never confirmed has nothing to undo it with.
    if (!entry.transactionId) return { record: entry.record, text: "Kumi can't take this back; Live's own undo (Cmd-Z in Live) can.", isError: true };
    let result: CallToolResult;
    try {
      result = await tools.call("live_undo", { transactionId: entry.transactionId, confirmation: "undo", idempotencyKey: entry.undoKey, ...(discard ? { discard: true } : {}) }, AbortSignal.any([lifetime.signal, AbortSignal.timeout(changeTimeoutMs)]), { host: true });
    } catch {
      return { record: update({ state: "unsure", note: "Live didn't answer the undo; try again." }), text: "Live didn't answer the undo; it can be retried.", isError: true };
    }
    if (result.isError) {
      const message = resultText(result).slice(0, 2048);
      // A refusal is final even when the bridge calls its own state uncertain: trying again won't help.
      const refused = /modified after apply|changed before deletion|undo refused/i.test(message);
      if (uncertain(result) && !refused) return { record: update({ state: "unsure", note: "Live didn't confirm the undo; try again." }), text: message, isError: true };
      return { record: update({ state: "kept", note: undoNote(message) }), text: message, isError: true };
    }
    const body = payload(result);
    if (body.state !== "undone") return { record: update({ state: "unsure", note: "Live didn't confirm the undo; try again." }), text: JSON.stringify(body), isError: true };
    scheduleSave(20_000);
    // Only the field the change touched goes back: later changes to the track's other field stay.
    const restore = entry.restore; const current = restore ? known.get(restore.ref) : undefined;
    if (restore && current) {
      if (restore.field === "name") known.set(restore.ref, { ...current, name: restore.value ?? current.name });
      else { const { color: _color, ...rest } = current; known.set(restore.ref, restore.value ? { ...rest, color: restore.value } : rest); }
    }
    return { record: update({ state: "undone" }), text: JSON.stringify({ undone: entry.record.title, change: entry.record.id }), isError: false };
  }
  /** A discovery for the audition's own use, through the model's path (so its references are current); its rows. */
  /**
   * Kumi's own read of a whole collection, as one answer: every page, however Live's side splits it (the
   * Remote Script ends a page early to keep Live's UI responsive), up to the limit asked. An error on any
   * page is the read's; a page from another epoch or kind ends it as Live having changed.
   */
  async function pages(args: JsonObject, signal: AbortSignal): Promise<CallToolResult> {
    const first = await tools!.call("live_discover", args, signal, { host: true });
    if (first.isError) return first;
    let head: JsonObject;
    try { head = payload(first); } catch { return first; }
    if (typeof head.nextCursor !== "string" || !head.nextCursor) return first;
    const items: unknown[] = Array.isArray(head.items) ? [...head.items] : [];
    const limit = typeof args.limit === "number" ? args.limit : Number.POSITIVE_INFINITY;
    let next: string | undefined = head.nextCursor;
    const seen = new Set<string>([next]);
    for (let count = 1; next && items.length < limit && count < 100_000; count++) {
      const more = await tools!.call("live_discover", { ...args, cursor: next }, signal, { host: true });
      if (more.isError) return more;
      const page = payload(more);
      if (page.epoch !== head.epoch || page.kind !== head.kind) return { isError: true, content: [{ type: "text", text: "Live changed while Kumi read it; read it again" }] };
      if (Array.isArray(page.items)) for (const item of page.items) items.push(item);
      next = typeof page.nextCursor === "string" && page.nextCursor ? page.nextCursor : undefined;
      // A page that leads back to one already read would never end.
      if (next && seen.has(next)) return { isError: true, content: [{ type: "text", text: "Live's pages of that read didn't end; read it again" }] };
      if (next) seen.add(next);
    }
    const { nextCursor: _read, ...rest } = head;
    const merged: JsonObject = { ...rest, items: items as JsonObject[], truncated: Boolean(next), ...(next ? { nextCursor: next } : {}) };
    return { content: [{ type: "text", text: JSON.stringify(merged) }], structuredContent: merged };
  }
  /** Kumi's own read of every row of a kind (under `extra.parent`, say), registered for this turn's changes. */
  async function rows(kind: string, extra: JsonObject, signal: AbortSignal): Promise<JsonObject[]> {
    if (!available || lost || currentEpoch === undefined || !tools) throw new ObservationError(NO_CURRENT_LIVE);
    const epoch = currentEpoch;
    const args = discoveryArgs({ kind, limit: pageLimit(), budget: wholeBudget(), ...(lengthen(extra) as JsonObject) });
    const read = await pages(args, AbortSignal.any([signal, lifetime.signal]));
    if (read.isError) throw new ObservationError(read.content.map((part) => (part.type === "text" ? part.text : "")).join("") || "Live read failed");
    const page = discoveryPayload(read, kind, epoch);
    registerRows(kind, page.items, args, page.nextCursor);
    // As the model sees them: Live's references by their short names, mixers slimmed.
    const shown = shorten(slimMixers({ items: page.items })) as { items: unknown[] };
    return shown.items.map((item) => object(item));
  }
  /** One of the audition's steps, as its own tool would run it (quietly, while an audition runs); its reply. */
  async function step(tool: string, input: JsonObject, signal: AbortSignal): Promise<JsonObject> {
    const kind = CHANGES.find((candidate) => candidate.tool === tool);
    const action = kind ? undefined : ACTIONS.find((candidate) => candidate.tool === tool);
    const outcome = kind ? await change(kind, input, signal, true) : await act(action!, input, signal);
    if (outcome.isError) throw new ObservationError(`${tool}: ${outcome.text.slice(0, 400)}`);
    try { return JSON.parse(outcome.text) as JsonObject; } catch { return {}; }
  }
  /** Main's fader now, read fresh. */
  async function mainVolume(signal: AbortSignal): Promise<{ ref: string; volume?: number }> {
    const main = (await rows("main-track", { fields: ["name", "mixer"] }, signal))[0];
    if (!main || typeof main.ref !== "string") throw new ObservationError("Live didn't say where Main is.");
    const volume = object(main.mixer ?? {}).volume;
    return { ref: main.ref, ...(typeof volume === "number" ? { volume } : {}) };
  }
  /** Main back where it was, set directly and read back (its undo can be refused once Live moves it). True when it is. */
  async function putMainBack(volume: number, signal: AbortSignal): Promise<boolean> {
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        const main = await mainVolume(signal);
        if (main.volume !== undefined && Math.abs(main.volume - volume) < 1e-4) return true;
        await step("set_mixer", { trackRef: main.ref, volume }, signal);
        const after = await mainVolume(signal);
        if (after.volume !== undefined && Math.abs(after.volume - volume) < 1e-4) return true;
      } catch { /* once more, then say so */ }
    }
    return false;
  }
  /** Where a fader position is in dB, as Live shows it (0.85 is 0 dB). */
  const faderDb = (volume: number) => (volume <= 0 ? "-inf dB" : `${(20 * Math.log10(volume / 0.85) * (volume > 0.85 ? 0.3 : 1)).toFixed(1)} dB`);

  /** The current "Kumi · Goal best" copy's own steps, across a goal's rigs (a pause and a resume): undone when a better one replaces it. */
  let bestSteps: string[] = [];
  /** Whether a render is running (an audition, or a goal's pass): one at a time. */
  let rendering = false;
  /** Run Kumi's own steps quietly (no HISTORY, no NOW), their ids into `into`, and the answer's change count as it was. */
  async function quietly<T>(into: string[] | undefined, work: () => Promise<T>): Promise<T> {
    const outer = quiet; quiet = []; const counted = changesThisTurn;
    try { return await work(); }
    finally {
      if (into) into.push(...quiet);
      else {
        // Steps Kumi will never undo give up their undo in the bridge too, so they don't hold its room.
        const released = quiet.flatMap((id) => { const entry = changes.get(id); changes.delete(id); return entry && entry.record.state === "applied" ? [entry.transactionId] : []; });
        release(released);
      }
      quiet = outer; changesThisTurn = counted;
    }
  }
  /** Give up the bridge's undo of these transactions (bridge 1.0.50), in the background; best effort. */
  function release(given: readonly string[]): void {
    // A fast change has no bridge transaction to release.
    const transactionIds = given.filter(Boolean);
    if (!transactionIds.length || !tools?.has("live_transaction_release")) return;
    for (let at = 0; at < transactionIds.length; at += 64) {
      void tools.call("live_transaction_release", { transactionIds: transactionIds.slice(at, at + 64) }, AbortSignal.any([lifetime.signal, AbortSignal.timeout(10_000)]), { host: true }).catch(() => undefined);
    }
  }

  /**
   * A render rig: a scratch track per source, recording its Post FX, kept for as many passes as a
   * search needs. Its scaffolding (the tracks, clips copied into the Arrangement) is undone when it
   * closes; the transport is put back as it was read.
   */
  interface Rig {
    tag: string;
    sources: { track: string; name: string; scratch: string; label: string; clip?: string; /** The scene its clip is in, once copied: it names the clip in a later turn. */ scene?: number; /** The whole mix, recorded through Resampling. */ mix?: boolean }[];
    /** The candidates play Session clips, copied to a free stretch of the Arrangement at `from`. */
    clips: boolean;
    /** A goal's rig holds Main down, the transport primed and recording on between passes (see renderPass). */
    hold?: { main?: { ref: string; prior: number }; primed?: string; recording?: boolean; rearm?: string[] };
    /** The stretch rendered now, when it's not the whole part (a snippet, screening a generation). */
    window?: { from: number; beats: number };
    from: number; beats: number;
    /** The rig's own changes, undone at close. */
    steps: string[];
    transport?: { position?: number; loop?: boolean };
    notes: string[];
    /** Kumi's listening devices, one at the end of each source's chain (Main's for the mix), when they're used instead of scratch tracks. */
    ears?: { link: EarsLink; taps: Map<string, Tap> };
  }
  /** Where the playhead is now; undefined when Live doesn't say. */
  async function playheadNow(signal: AbortSignal): Promise<number | undefined> {
    const at = (await rows("set", { fields: ["position"] }, signal))[0]?.position;
    return typeof at === "number" ? at : undefined;
  }
  /** Where the transport is, to put it back after renders. */
  async function transportNow(signal: AbortSignal): Promise<NonNullable<Rig["transport"]>> {
    const set = (await rows("set", { fields: ["position", "loop"] }, signal).catch(() => [] as JsonObject[]))[0];
    const loop = set?.loop && typeof set.loop === "object" ? (set.loop as JsonObject).enabled : undefined;
    return { ...(typeof set?.position === "number" ? { position: set.position } : {}), ...(typeof loop === "boolean" ? { loop } : {}) };
  }
  /** A rig for these candidates: their names checked, Session clips copied into the Arrangement, a scratch track each. */
  async function openRig(candidates: readonly AuditionRequest["candidates"][number][], fromBeat: number | undefined, beats: number | undefined, signal: AbortSignal): Promise<Rig> {
    const rig: Rig = { tag: randomUUID().slice(0, 4), sources: [], from: fromBeat ?? 0, beats: beats ?? 8, steps: [], notes: [], clips: candidates.some((candidate) => candidate.clip) };
    rig.transport = await transportNow(signal);
    // Kumi's listening devices hear each source where it is; without them (no Max for Live, say) it records.
    const link = await earsReady(signal);
    const tracks = await rows("track", { fields: ["name"] }, signal);
    /** A source shares its name with another track: recording (routed by name) can't render it. */
    let shared = false;
    for (const [index, candidate] of candidates.entries()) {
      // The whole mix: what Main plays, which Resampling records before Main's fader (so Main can stay silent).
      if (candidate.mix) {
        if (!rig.sources.some((source) => source.mix)) rig.sources.push({ track: MIX_CANDIDATE, name: MIX_CANDIDATE, scratch: `Kumi · render ${rig.sources.length + 1} ${rig.tag}`, label: candidate.label ?? "The whole mix", mix: true });
        continue;
      }
      // By its reference from this turn, or (a goal resumed after a restart) by its name.
      const found = tracks.find((track) => track.ref === candidate.track) ?? tracks.find((track) => track.name === candidate.track);
      if (!found || typeof found.name !== "string") throw new ObservationError(`${candidate.track} isn't a track in this turn's discovery; discover again.`);
      // Recording routes by name: two tracks of one name can't be told apart (a listening device needs no name).
      if (tracks.filter((track) => track.name === found.name).length > 1) {
        if (!link) throw new ObservationError(`Two tracks are named “${found.name}”; rename one so Kumi can render it.`);
        shared = true;
      }
      // Named twice (by reference and by name): rendered once.
      if (rig.sources.some((source) => source.name === found.name)) continue;
      rig.sources.push({ track: String(found.ref), name: found.name, scratch: `Kumi · render ${rig.sources.length + 1} ${rig.tag}`, label: candidate.label ?? `Candidate ${index + 1}`, ...(candidate.clip ? { clip: candidate.clip } : {}) });
    }
    // Failing partway, the rig undoes what it made before saying so: nobody else holds its steps yet.
    try {
    await quietly(rig.steps, async () => {
      // Session clips play from a free stretch of the Arrangement, after everything in it. When any
      // candidate plays one, they all do (each its own, or its first): the stretch is empty otherwise.
      if (rig.clips) {
        const song = tools!.has("live_song_state") ? payload(await tools!.call("live_song_state", {}, signal, { host: true })) : {};
        const end = typeof song.songLength === "number" ? song.songLength : 0;
        rig.from = (Math.ceil(end / beatsPerBar) + 2) * beatsPerBar;
        let longest = 0;
        for (const source of rig.sources) longest = Math.max(longest, await copyClip(rig, source.track, source.clip, signal, source));
        rig.beats = beats ?? Math.min(32, longest || 8);
      }
      if (link) {
        rig.ears = { link, taps: new Map() };
        try { await placeTaps(rig, rig.sources, signal); }
        catch (error) {
          signal.throwIfAborted();
          // The listening devices couldn't be placed: their steps go, and the sources are recorded instead (from
          // now on in this Live, so no render waits on a device that won't start).
          await removeTaps(rig);
          // Only a device that never started says this Live can't run it; a refused load is about that track.
          if (error instanceof EarsSilent) {
            earsRefused = true;
            rig.notes.push("Kumi's listening device didn't start in this Live (it needs Max for Live), so Kumi records to listen instead.");
          }
          if (shared) throw error;
          delete rig.ears;
          await addScratch(rig, rig.sources, signal);
        }
      } else await addScratch(rig, rig.sources, signal);
    });
    } catch (error) { await closeRig(rig); throw error; }
    return rig;
  }
  /**
   * A candidate's Session clip (the one named, or "first": its first) copied to the rig's stretch of the
   * Arrangement; its length in beats. A track without one plays nothing there, and is said so.
   */
  async function copyClip(rig: Rig, track: string, clipRef: string | undefined, signal: AbortSignal, source?: Rig["sources"][number]): Promise<number> {
    const slots = await rows("clip-slot", { parent: track, fields: ["clipRef"] }, signal);
    // A clip by its reference from this turn, by its scene ("scene:2", which lasts from turn to turn), or the track's first.
    const scene = /^scene:(\d+)$/.exec(clipRef ?? "")?.[1];
    const slot = scene !== undefined ? slots[Number(scene)] : clipRef && clipRef !== "first" ? slots.find((row) => row.clipRef === clipRef) : slots.find((row) => typeof row.clipRef === "string");
    if (!slot || typeof slot.clipRef !== "string") {
      if (clipRef && clipRef !== "first") throw new ObservationError(`${clipRef} isn't a Session clip on that track; discover its clip slots again.`);
      rig.notes.push("A candidate has no Session clip to play, so it renders silent.");
      return 0;
    }
    if (source) source.scene = slots.indexOf(slot);
    const clip = (await rows("session-clip", { parent: slot.ref, fields: ["length"] }, signal))[0];
    await step("duplicate_clip", { clipRef: slot.clipRef, arrangementPosition: rig.from }, signal);
    return typeof clip?.length === "number" ? clip.length : 0;
  }
  /** Scratch tracks for these sources, routed from their Post FX and armed. */
  async function addScratch(rig: Rig, sources: Rig["sources"], signal: AbortSignal): Promise<void> {
    await step("add_tracks_and_scenes", { tracks: sources.map((source) => ({ name: source.scratch, kind: "audio" })), scenes: [] }, signal);
    const now_ = await rows("track", { fields: ["name"] }, signal);
    for (const source of sources) {
      const found = now_.find((track) => track.name === source.scratch);
      if (typeof found?.ref !== "string") throw new ObservationError("Kumi's scratch track didn't appear.");
      await step("set_routing", source.mix ? { trackRef: found.ref, inputType: "Resampling", arm: true, monitoring: "off" } : { trackRef: found.ref, inputType: source.name, inputSubRouting: "Post FX", arm: true, monitoring: "off" }, signal);
    }
  }
  /** A new source for an open rig (a candidate the model built mid-search). */
  async function addToRig(rig: Rig, candidate: { track: string; name: string; label: string; clip?: string }, signal: AbortSignal): Promise<void> {
    const source = { ...candidate, scratch: `Kumi · render ${rig.sources.length + 1} ${rig.tag}` };
    await quietly(rig.steps, async () => {
      if (rig.clips) await copyClip(rig, candidate.track, candidate.clip ?? "first", signal, source);
      if (rig.ears) await placeTaps(rig, [source], signal); else await addScratch(rig, [source], signal);
    });
    rig.sources.push(source);
  }
    /** A listening device that never said hello: this Live can't run it (no Max for Live, say). */
  class EarsSilent extends ObservationError {}
/** Kumi's end of its listening devices, opened once; undefined when they can't be used (then Kumi records instead). */
  let earsSetup: Promise<EarsLink | undefined> | undefined;
  /** The device didn't start in this Live (no Max for Live, say): Kumi records instead until Live restarts. */
  let earsRefused = false;
  /** Where the devices write what they heard, and the WAVs made from it (gone when Kumi closes). */
  const earsFolder = join(tmpdir(), "kumi-ears", generation.slice(0, 8));
  async function earsReady(signal: AbortSignal): Promise<EarsLink | undefined> {
    if (options.ears === false || process.env.KUMI_EARS === "0" || earsRefused) return undefined;
    if (!tools?.has("live_browser_load_preview") || !tools.has("live_browser_inspect") || !supported({ since: EARS_BRIDGE })) return undefined;
    const given = options.ears ? options.ears.open : undefined;
    earsSetup ??= (async () => {
      const link = given ? await given() : await openEarsLink();
      await mkdir(earsFolder, { recursive: true, mode: 0o700 });
      if (given) return link;
      // The device in the User Library's Kumi folder (rewritten only when this Kumi's differs), and listed by Live's Browser.
      const installed = await installEars(options.userLibrary ?? userLibrary());
      const deadline = Date.now() + (installed.written ? 20_000 : 4_000);
      while (Date.now() < deadline) {
        const seen = await tools!.call("live_browser_inspect", { itemId: EARS_ITEM }, AbortSignal.any([lifetime.signal, AbortSignal.timeout(5_000)])).then((read) => !read.isError, () => false);
        if (seen) return link;
        await delay(400, undefined, { signal: lifetime.signal });
      }
      await link.close();
      return undefined;
    })().catch(() => undefined);
    const link = await earsSetup;
    // Not ready this time (the Browser hadn't listed it yet, say): asked again next time.
    if (!link) earsSetup = undefined;
    signal.throwIfAborted();
    return link;
  }
  /** Where a track is in Live's own terms ("live_set tracks 3", "live_set return_tracks 0", "live_set master_track"), from its reference. */
  async function lomTrackPath(trackRef: string, signal: AbortSignal): Promise<string> {
    const long = String(lengthen(trackRef, "trackRef"));
    if (/:main_track:/.test(long)) return "live_set master_track";
    const ret = /:return_track:(\d+)$/.exec(long);
    if (ret) return `live_set return_tracks ${Number(ret[1])}`;
    const index = Number(/:track:(\d+)$/.exec(long)?.[1]);
    if (!Number.isInteger(index)) throw new ObservationError("Kumi couldn't tell where that track is; discover it again.");
    // A reference's number counts the regular tracks, then the returns, then Main.
    const regular = (await rows("track", { fields: ["name"] }, signal)).length;
    if (index < regular) return `live_set tracks ${index}`;
    const returns = (await rows("return-track", { fields: ["name"] }, signal)).length;
    return index < regular + returns ? `live_set return_tracks ${index - regular}` : "live_set master_track";
  }
  /** A listening device at the end of each source's chain (Main's for the mix), quietly: the rig's own steps, undone when it closes. */
  async function placeTaps(rig: Rig, sources: Rig["sources"], signal: AbortSignal): Promise<void> {
    const ears = rig.ears!;
    for (const source of sources) {
      const trackRef = source.mix ? (await mainVolume(signal)).ref : source.track;
      ears.taps.set(source.name, await placeTap(ears.link, trackRef, signal));
    }
  }
  /** Load the device onto a track and wait for it to say hello from there (the one that loaded just now). */
  async function placeTap(link: EarsLink, trackRef: string, signal: AbortSignal): Promise<Tap> {
    const where = await lomTrackPath(trackRef, signal);
    const before = new Set(link.taps().map((tap) => tap.id));
    const loading = Date.now();
    await step("load_device", { itemId: EARS_ITEM, trackRef }, signal);
    // Live can give the new device a removed one's id: a device that says when it loaded is matched by that.
    const fresh = (tap: Tap) => (tap.loadedAt !== undefined ? tap.loadedAt >= loading - 1_000 : !before.has(tap.id));
    const tap = await link.waitFor((candidate) => fresh(candidate) && candidate.path.startsWith(`${where} devices `), 6_000, signal);
    if (!tap) throw new EarsSilent("Kumi's listening device didn't start on that track (Max for Live is needed: Live Suite, or Standard with Max for Live).");
    return tap;
  }
  /** The rig's listening devices taken away now (their loads undone), before it records instead. */
  async function removeTaps(rig: Rig): Promise<void> {
    const cleanup = AbortSignal.any([lifetime.signal, AbortSignal.timeout(changeTimeoutMs * 3)]);
    const loads = rig.steps.filter((id) => changes.get(id)?.record.family === "device");
    for (const id of [...loads].reverse()) {
      await undoChange(id, cleanup).catch(() => undefined);
      changes.delete(id);
      rig.steps.splice(rig.steps.indexOf(id), 1);
    }
    rig.ears?.taps.clear();
  }
  /** A path as Max reads it: forward slashes everywhere. */
  const maxPath = (file: string) => file.replace(/\\/g, "/");
  /** The WAVs of earlier passes go once there are many: a goal hears hundreds. */
  async function pruneEars(): Promise<void> {
    try {
      const names = (await readdir(earsFolder)).filter((name) => name.endsWith(".wav"));
      if (names.length <= 96) return;
      const dated = await Promise.all(names.map(async (name) => ({ name, at: statSync(join(earsFolder, name)).mtimeMs })));
      for (const { name } of dated.sort((a, b) => a.at - b.at).slice(0, names.length - 64)) await rm(join(earsFolder, name), { force: true });
    } catch { /* tidying is best effort */ }
  }
  /**
   * One silent pass through the listening devices: Main to -inf (written down first, for a crash), every
   * device recording, the part played, Main back exactly. Nothing is recorded into the Set and no track is
   * armed or added. Each source's file (the part cut out on Live's beat, a moment before it) and where the
   * part starts in it.
   */
  async function earsPass(rig: Rig, signal: AbortSignal): Promise<Map<string, { file: string; start: number }>> {
    const tempo = currentTempo!;
    const link = rig.ears!.link;
    const files = new Map<string, { file: string; start: number }>();
    const held = rig.hold;
    let prior = held?.main?.prior;
    let mainRef = held?.main?.ref;
    if (prior === undefined) {
      const main = await mainVolume(signal);
      if (main.volume === undefined) throw new ObservationError("Live didn't say Main's level, so Kumi won't touch it.");
      prior = main.volume; mainRef = main.ref;
      const pending = restore.load();
      if (prior === 0 && pending && (pending.path ? pending.path === project?.path : pending.set === currentSet)) prior = pending.volume;
    }
    const window = rig.window ?? { from: rig.from, beats: rig.beats };
    const taps = [...rig.ears!.taps.entries()];
    let started = false;
    let failed = true;
    try {
      await quietly(undefined, async () => {
        if (!held?.main) {
          restore.save({ set: currentSet ?? "", ...(project?.path ? { path: project.path } : {}), volume: prior!, at: now().getTime() });
          await step("set_mixer", { trackRef: mainRef!, volume: 0 }, signal);
          if (held) held.main = { ref: mainRef!, prior: prior! };
        }
        const beatMs = 60 / tempo * 1000;
        for (const longer of [false, true]) {
          const span = renderSpan(window.from, window.beats, beatsPerBar, tempo, longer, 0);
          const primeKey = `${span.position}`;
          const priming = !held || held.primed !== primeKey;
          if (priming && supported({ since: ARRANGEMENT_BRIDGE })) await step("play", { action: "back-to-arrangement" }, signal);
          if (priming && rig.transport?.loop !== false) await step("set_transport", { loopEnabled: false }, signal);
          // Room for the whole pass, Live's wait before it jumps (its launch quantization), and the steps around it.
          const seconds = (span.wait + 4 * beatsPerBar) * beatMs / 1000 + 6;
          started = true;
          // The devices record from before Live moves; Live plays, then jumps to the count-in when its launch
          // quantization says (on a beat or a bar). The position each device records says where every sample
          // was, so the part is found wherever the jump landed.
          await Promise.all(taps.map(([, tap]) => link.arm(tap, seconds, signal)));
          await step("play", { action: "continue" }, signal);
          const probe = taps[0]![1];
          // Live may start right at the count-in (stopped there): then there's nothing to jump.
          let first: { beats: number; running: boolean } | undefined;
          for (let check = 0; check < 8 && !first?.running; check++) {
            first = await link.transport(probe, signal);
            if (!first?.running) await delay(15, undefined, { signal });
          }
          const there = first?.running === true && first.beats >= span.position - 0.01 && first.beats <= span.position + 0.25;
          if (!there) await step("set_transport", { position: span.position }, signal);
          // Until a device hears Live in the count-in (after the jump) and then past the part and its tail.
          const end = window.from + window.beats + beatsPerBar / 2;
          const countIn = (beats: number) => beats >= span.position - 0.01 && beats < Math.max(window.from, span.position + 0.5);
          const deadline = Date.now() + seconds * 1000;
          let jumped = there;
          while (Date.now() < deadline) {
            const now_ = await link.transport(probe, signal);
            if (now_?.running) {
              if (countIn(now_.beats)) jumped = true;
              if (jumped && now_.beats >= end) break;
            }
            const ahead = jumped && now_ ? (end - now_.beats) * beatMs : 0;
            await delay(Math.max(20, Math.min(500, ahead * 0.8)), undefined, { signal });
          }
          if (held) held.primed = primeKey;
          await step("play", { action: "stop" }, signal);
          started = false;
          // Each device writes what it heard; the part is cut out of the stretch the transport played it in.
          let late = false;
          await Promise.all(taps.map(async ([name, tap]) => {
            const raw = join(earsFolder, `${randomUUID()}.raw`);
            try {
              const written = await link.write(tap, maxPath(raw), signal);
              const capture = await readCapture(raw, written.channels, written.sampleRate);
              // Placed by the position each device recorded (an older device's capture, by where it was armed).
              const stretches = runs(capture, { first: written.beats, afterJump: span.position });
              // The last stretch that played the whole part.
              const part = stretches.filter((run) => frameAt(run, window.from) !== undefined && frameAt(run, window.from + window.beats - 1e-3) !== undefined).at(-1);
              if (!part) { if (stretches.length) late = true; return; }
              const at = frameAt(part, window.from)!;
              const lead = Math.round(LEAD_IN * capture.sampleRate);
              const end = Math.min(part.to, at + Math.ceil((window.beats + beatsPerBar / 2) * part.samplesPerBeat));
              const wav = join(earsFolder, `${randomUUID()}.wav`);
              await writeCaptureWav(wav, capture, at - lead, end);
              files.set(name, { file: wav, start: Math.min(at, lead) / capture.sampleRate });
            } catch (error) {
              rig.notes.push(error instanceof Error ? error.message.slice(0, 200) : "A listening device didn't write what it heard.");
            } finally { await rm(raw, { force: true }).catch(() => undefined); }
          }));
          if (!late) break;
          if (longer) rig.notes.push("Live jumped past the part's start before Kumi could hear it; listen again.");
        }
      });
      failed = false;
    } finally {
      const cleanup = AbortSignal.any([lifetime.signal, AbortSignal.timeout(changeTimeoutMs * 3)]);
      if (started) await stopEverything(cleanup);
      // Every device stops recording, whatever happened.
      for (const [, tap] of taps) link.stop(tap);
      if (!held || failed) {
        if (!held) {
          const back = await quietly(undefined, () => putMainBack(prior!, cleanup));
          if (back) restore.clear();
          else rig.notes.push(`Main may still be silent: set it back to ${faderDb(prior!)} in Live.`);
        }
      }
      void pruneEars();
    }
    return files;
  }
  /**
   * One silent pass: Main to -inf (written down first, for a crash), every scratch track armed and
   * recording, the part played, Main back exactly. Each source's file and where the part starts in it.
   */
  async function renderPass(rig: Rig, signal: AbortSignal): Promise<Map<string, { file: string; start: number }>> {
    if (rig.ears) return earsPass(rig, signal);
    const tempo = currentTempo!;
    // Where a pass's time goes, for probing throughput (KUMI_TIMING=1 prints it).
    let lapAt = Date.now(); const laps: string[] = [];
    const lap = (what: string) => { laps.push(`${what} ${Date.now() - lapAt}`); lapAt = Date.now(); };
    const files = new Map<string, { file: string; start: number }>();
    // A held rig (a goal's) keeps Main down, the transport primed and recording on between passes: a pass is
    // then play, wait, stop. Every step through the bridge costs a couple of seconds.
    const held = rig.hold;
    let prior = held?.main?.prior;
    let mainRef = held?.main?.ref;
    if (prior === undefined) {
      const main = await mainVolume(signal);
      if (main.volume === undefined) throw new ObservationError("Live didn't say Main's level, so Kumi won't touch it.");
      prior = main.volume; mainRef = main.ref;
      // Main at -inf with a level still waiting to be put back (an earlier render couldn't): that level is
      // the producer's, not -inf.
      const pending = restore.load();
      if (prior === 0 && pending && (pending.path ? pending.path === project?.path : pending.set === currentSet)) prior = pending.volume;
    }
    lap("main read");
    let started = false;
    const window = rig.window ?? { from: rig.from, beats: rig.beats };
    /** The producer's own armed tracks: disarmed for the pass (Live records exactly the armed ones), armed again after. */
    const rearm: string[] = held?.rearm ?? [];
    let failed = true;
    try {
      await quietly(undefined, async () => {
        if (!held?.main) {
          restore.save({ set: currentSet ?? "", ...(project?.path ? { path: project.path } : {}), volume: prior!, at: now().getTime(), scratch: rig.sources.map((source) => source.scratch) });
          await step("set_mixer", { trackRef: mainRef!, volume: 0 }, signal);
          if (held) held.main = { ref: mainRef!, prior: prior! };
        }
        lap("main down");
        // Scratch tracks something else disarmed (another recording) are armed again.
        const tracks = await rows("track", { fields: ["name", "armed"] }, signal);
        lap("tracks read");
        const refs = rig.sources.map((source) => { const found = tracks.find((track) => track.name === source.scratch); if (typeof found?.ref !== "string") throw new ObservationError(`Kumi's render track “${source.scratch}” is gone.`); return found; });
        for (const track of refs) if (track.armed !== true) await step("set_routing", { trackRef: track.ref, arm: true }, signal);
        for (const track of tracks) {
          if (track.armed !== true || typeof track.ref !== "string" || refs.includes(track)) continue;
          await step("set_routing", { trackRef: track.ref, arm: false }, signal); rearm.push(track.ref);
        }
        const record = { action: "start", lane: "arrangement", destinationTrackRef: refs[0]!.ref as string, ...(refs.length > 1 ? { alsoTrackRefs: refs.slice(1).map((track) => track.ref as string) } : {}) };
        const beatMs = 60 / tempo * 1000;
        // A pass whose takes started after the part (its steps outlasted the lead-in) goes again once, with twice the lead-in.
        for (const longer of [false, true]) {
          const span = renderSpan(window.from, window.beats, beatsPerBar, tempo, longer);
          const primeKey = `${span.position}`;
          const priming = !held || held.primed !== primeKey;
          if (priming && supported({ since: ARRANGEMENT_BRIDGE })) { await step("play", { action: "back-to-arrangement" }, signal); lap("back to arrangement"); }
          if (held?.recording) { await step("record", { action: "stop", lane: "arrangement" }, signal); held.recording = false; }
          started = true;
          // On real Live, "continue" plays from where playback last stopped and "start" from the start marker,
          // wherever the playhead was moved while stopped (moved to beat 40, it played on from 251); a jump
          // while playing is honoured; and a jump while recording ends the take there, recording nothing after.
          if (span.position > 0) {
            // So a pass plays, jumps to its lead-in, then records: the take starts there, before the part.
            await step("play", { action: "continue" }, signal);
            lap("play");
            await step("set_transport", { position: span.position, ...(priming ? { loopEnabled: false } : {}) }, signal);
            const jumpedAt = Date.now();
            lap("jump");
            await step("record", record, signal);
            if (held) held.recording = true;
            lap("record start");
            await delay(Math.max(0, span.wait * beatMs - (Date.now() - jumpedAt)), undefined, { signal });
          } else {
            // Too near the Set's start for a lead-in: stopped twice, Live is at its start, and recording on plays
            // from there (Live's Start Playback with Record; with that turned off, start does).
            await step("play", { action: "stop" }, signal);
            await step("play", { action: "stop" }, signal);
            if (priming && rig.transport?.loop !== false) await step("set_transport", { loopEnabled: false }, signal);
            lap("to the start");
            await step("record", record, signal);
            if (held) held.recording = true;
            lap("record start");
            const now_ = (await rows("set", { fields: ["position", "playing"] }, signal))[0];
            if (now_?.playing !== true) { await step("play", { action: "start" }, signal); lap("play"); }
            // Until the playhead is past the part: a count-in holds it at the start for a bar or more first.
            let at = now_?.playing === true && typeof now_.position === "number" ? now_.position : 0;
            for (let check = 0; check < 4 && at < span.wait - 0.25; check++) {
              await delay((span.wait - at) * beatMs, undefined, { signal });
              const read = (await rows("set", { fields: ["position"] }, signal))[0]?.position;
              if (typeof read !== "number") break;
              at = read;
            }
          }
          if (held) held.primed = primeKey;
          lap("wait");
          await step("play", { action: "stop" }, signal);
          lap("stop");
          if (!held) { await step("record", { action: "stop", lane: "arrangement" }, signal); lap("record stop"); }
          // Stopping ends Live's recording too, held or not: the next pass starts it again (without that, a
          // held rig's later passes recorded nothing, and each was scored on the first pass's take).
          else held.recording = false;
          started = false;
          // Each source's take covering the part (the newest: a pass records over the last), read together.
          let late = false;
          await Promise.all(rig.sources.map(async (source, index) => {
            const clips = (await rows("arrangement-clip", { parent: refs[index]!.ref, fields: ["start", "length", "isAudio"] }, signal)).filter((row) => row.isAudio === true && typeof row.ref === "string");
            const clip = clips.map((row, order) => ({ row, order })).filter(({ row }) => typeof row.start === "number" && row.start <= window.from)
              .sort((a, b) => (b.row.start as number) - (a.row.start as number) || b.order - a.order)[0]?.row ?? clips.at(-1);
            if (!clip) return;
            if (typeof clip.start === "number" && clip.start > window.from + 1e-3) late = true;
            const file = await clipFile(clip.ref as string, signal).catch(() => undefined);
            // A tenth of a second before the part: a window that opens right on the attack hears it as a flurry of onsets.
            if (file) files.set(source.name, { file, start: Math.max(0, (window.from - (typeof clip.start === "number" ? clip.start : span.position)) * 60 / tempo - LEAD_IN) });
          }));
          lap("clips and files");
          if (!late) break;
          if (longer) rig.notes.push("Live started recording after the part had begun, so a take may miss its start; render it again.");
          else lap("late: again with a longer lead-in");
        }
      });
      failed = false;
    } finally {
      const cleanup = AbortSignal.any([lifetime.signal, AbortSignal.timeout(changeTimeoutMs * 3)]);
      if (started) { await stopEverything(cleanup); if (held) held.recording = false; }
      if (!held || failed) {
        // A one-off pass (an audition) puts everything back now; a held rig when it closes.
        for (const ref of rearm.splice(0)) await quietly(undefined, () => step("set_routing", { trackRef: ref, arm: true }, cleanup)).catch(() => rig.notes.push("A track Kumi disarmed to render may still be disarmed; arm it again in Live."));
        if (!held) {
          const back = await quietly(undefined, () => putMainBack(prior!, cleanup));
          lap("main back");
          if (back) restore.clear();
          else rig.notes.push(`Main may still be silent: set it back to ${faderDb(prior!)} in Live.`);
        }
      } else held.rearm = rearm;
      if (process.env.KUMI_TIMING) console.error(`[render pass · ${rig.sources.length} sources${held ? " · held" : ""}] ${laps.join(" · ")} ms`);
    }
    return files;
  }
  /** The rig's scaffolding undone, newest first (its tracks with discard: they recorded since), the transport put back. */
  async function closeRig(rig: Rig): Promise<void> {
    const cleanup = AbortSignal.any([lifetime.signal, AbortSignal.timeout(changeTimeoutMs * 3)]);
    const scratch = rig.sources.map((source) => source.scratch);
    // A held rig lets go first: recording off, playback stopped, the producer's armed tracks armed, Main back.
    const held = rig.hold;
    if (held) {
      if (held.recording) { await quietly(undefined, () => step("record", { action: "stop", lane: "arrangement" }, cleanup)).catch(() => undefined); held.recording = false; }
      await stopEverything(cleanup);
      for (const ref of (held.rearm ?? []).splice(0)) await quietly(undefined, () => step("set_routing", { trackRef: ref, arm: true }, cleanup)).catch(() => rig.notes.push("A track Kumi disarmed to render may still be disarmed; arm it again in Live."));
      if (held.main) {
        const back = await quietly(undefined, () => putMainBack(held.main!.prior, cleanup));
        if (back) restore.clear();
        else rig.notes.push(`Main may still be silent: set it back to ${faderDb(held.main.prior)} in Live.`);
        delete held.main;
      }
    }
    await quietly(undefined, async () => {
      for (const id of [...rig.steps].reverse()) {
        const entry = changes.get(id);
        // What was set on a scratch track goes with it (but the scratch track itself is undone).
        if (!entry || entry.record.state !== "applied" || (entry.record.family !== "structure" && entry.record.track && scratch.includes(entry.record.track.name))) continue;
        const undone = await undoChange(id, cleanup, entry.record.family === "structure").catch(() => undefined);
        if (!undone || undone.isError) {
          // Scratch tracks that stay mustn't stay armed (the next recording would take them too).
          if (entry.record.family === "structure") {
            const tracks = await rows("track", { fields: ["name"] }, cleanup).catch(() => [] as JsonObject[]);
            for (const name of scratch) { const ref = tracks.find((track) => track.name === name)?.ref; if (typeof ref === "string") await step("set_routing", { trackRef: ref, arm: false }, cleanup).catch(() => undefined); }
          }
          rig.notes.push(`Couldn't take back “${entry.record.title}” (${(undone?.text ?? "no answer").slice(0, 200)}); ${entry.record.family === "structure" ? "delete Kumi's render track by hand" : "check it in Live"}.`);
        }
      }
      for (const id of rig.steps) changes.delete(id);
      if (rig.transport?.position !== undefined || rig.transport?.loop !== undefined) {
        await step("set_transport", { ...(rig.transport.position !== undefined ? { position: rig.transport.position } : {}), ...(rig.transport.loop !== undefined ? { loopEnabled: rig.transport.loop } : {}) }, cleanup).catch(() => undefined);
      }
    });
  }
  /** A track's devices and their knobs, read fresh (references only last a turn): its chain in words, and the knobs. */
  async function readKnobs(trackName: string, signal: AbortSignal): Promise<{ ref: string; chain: string; devices: JsonObject[]; knobs: Knob[] }> {
    const track = (await rows("track", { fields: ["name"] }, signal)).find((row) => row.name === trackName);
    if (typeof track?.ref !== "string") throw new ObservationError(`The track “${trackName}” is gone.`);
    const devices = await rows("device", { parent: track.ref, fields: ["name", "className"] }, signal);
    const knobs: Knob[] = [];
    for (const [index, device] of devices.entries()) {
      const parameters = await rows("parameter", { parent: device.ref, fields: ["name", "value", "min", "max", "quantization"] }, signal);
      for (const parameter of parameters) {
        if (typeof parameter.ref !== "string" || typeof parameter.name !== "string" || typeof parameter.value !== "number" || typeof parameter.min !== "number" || typeof parameter.max !== "number") continue;
        knobs.push({ ref: parameter.ref, device: `${index}:${String(device.name ?? device.className ?? "Device")}`, name: parameter.name, min: parameter.min, max: parameter.max, value: parameter.value,
          ...(typeof parameter.quantization === "number" && parameter.quantization > 0 ? { step: parameter.quantization } : {}) });
      }
    }
    return { ref: track.ref, chain: devices.map((device) => String(device.name ?? device.className ?? "Device")).join(" → ") || "empty", devices, knobs };
  }
  /**
   * A goal's rig: the candidates' scratch tracks kept open for every generation, a safety limiter at
   * the end of each candidate's chain, and the reference heard once.
   */
  async function openGoal(given: AuditionRequest, originalSignal: AbortSignal): Promise<GoalRig | string> {
    // A goal turns one track's own knobs (and closes its chain with a safety limiter): not Main's.
    if (given.candidates.some((candidate) => candidate.mix || candidate.track === MIX_CANDIDATE)) return "A goal searches a track's own devices. For the whole mix, audition it against the reference (candidates [{\"mix\": true}]) and change EQ, compression and levels between rounds.";
    if (!available || lost || !tools || currentEpoch === undefined) return NO_CURRENT_LIVE;
    if (!supported({ since: GOAL_BRIDGE })) return tooOld({ since: GOAL_BRIDGE });
    if (!given.reference) return "A goal needs a reference to reach.";
    if (!currentTempo) return "Kumi doesn't know the Set's tempo yet; try again.";
    const signal = AbortSignal.any([originalSignal, lifetime.signal]);
    const reference = await heardReference(given.reference, given, signal);
    const request = given;
    const rig = await openRig(request.candidates, request.fromBeat, request.beats, signal);
    // Held between passes: Main stays down while the goal searches (said once), the transport primed, recording on.
    rig.hold = {};
    try { options.onAction?.({ title: "Live stays quiet while the goal searches; it comes back when the goal stops or pauses" }); } catch { /* a listener failure must not affect Live */ }
    const slots: GoalSlotInfo[] = [];
    /** Each slot's values as last set, so a generation only sends what changes. */
    const current = new Map<string, Map<string, number>>();
    const key = (knob: Pick<Knob, "device" | "name">) => `${knob.device}|${knob.name}`;
    /** Each slot's knobs with their references, read again only when the references have expired (a new observation). */
    const known = new Map<string, { lease: number; read: Awaited<ReturnType<typeof readKnobs>> }>();
    const knobsNow = async (name: string, signal: AbortSignal) => {
      const cached = known.get(name);
      if (cached && cached.lease === observationGeneration) return cached.read;
      const read = await readKnobs(name, signal);
      known.set(name, { lease: observationGeneration, read });
      return read;
    };
    const adopt = async (source: { name: string; label: string }, signal: AbortSignal): Promise<GoalSlotInfo> => {
      let read = await readKnobs(source.name, signal);
      // Every candidate's chain ends in a limiter: a runaway patch can't reach a dangerous level, even played by the producer.
      if (!read.devices.at(-1) || read.devices.at(-1)!.className !== "Limiter") {
        await quietly(undefined, () => step("load_device", { itemId: "audio_effects/Limiter", trackRef: read.ref }, signal));
        read = await readKnobs(source.name, signal);
        // Its input well down: a hot synth into a limiter at 0 dB is limited all the time, and that's heard
        // (a sine chord came back with its upper mids up 20 dB). Down here it only catches a runaway.
        const limiter = read.devices.at(-1);
        const gain = read.knobs.find((knob) => knob.device === `${read.devices.length - 1}:${String(limiter?.name ?? "Limiter")}` && /^(gain|input( gain)?)$/i.test(knob.name));
        // In dB, or (Live 12's "Input Gain") 0 to 1 for -24 to +24 dB, where -12 dB is 0.25 (read back on real Live).
        const down = gain ? (gain.min < 0 ? Math.max(gain.min, -12) : gain.min === 0 && gain.max === 1 ? 0.25 : undefined) : undefined;
        if (gain && down !== undefined && typeof limiter?.ref === "string") {
          await quietly(undefined, () => step("set_device_parameters", { deviceRef: limiter.ref, values: [{ parameterRef: gain.ref, value: down }] }, signal)).catch(() => undefined);
          read = await readKnobs(source.name, signal);
        }
      }
      current.set(source.name, new Map(read.knobs.map((knob) => [key(knob), knob.value])));
      const slot = { name: source.name, label: source.label, chain: read.chain, knobs: read.knobs };
      slots.push(slot);
      return slot;
    };
    try { for (const source of rig.sources) await adopt(source, signal); }
    catch (error) { await closeRig(rig); throw error; }
    const focus = request.focus;
    // A long part is screened on its most characteristic few seconds: the reference's loudest, most changing
    // stretch, found in its loudness over time, and the same stretch of the part.
    const tempo0 = currentTempo!;
    let screen: { from: number; beats: number; reference: Analysis } | undefined;
    if (rig.beats * 60 / tempo0 >= 6) {
      const beats = Math.max(2, Math.round(3 * tempo0 / 60));
      // The loudness slices cover what was heard (reference.seconds is the whole file's length).
      const lufs = reference.overTime.lufs; const slice = Number.parseFloat(reference.overTime.every) || reference.seconds / Math.max(1, lufs.length);
      const across = Math.max(1, Math.round(beats * 60 / tempo0 / slice));
      let bestAt = 0; let bestScore = -Infinity;
      for (let at = 0; at + across <= lufs.length; at++) {
        const part = lufs.slice(at, at + across).map((value) => value ?? -70);
        const mean = part.reduce((sum, value) => sum + value, 0) / part.length;
        const change = part.slice(1).reduce((sum, value, index) => sum + Math.abs(value - part[index]!), 0);
        if (mean + change > bestScore) { bestScore = mean + change; bestAt = at; }
      }
      const offset = Math.max(0, Math.min(rig.beats - beats, Math.round(bestAt * slice * tempo0 / 60)));
      const snippet = await heardReference(given.reference, { ...request, referenceFrom: (request.referenceFrom ?? 0) + offset * 60 / tempo0, referenceSeconds: beats * 60 / tempo0 }, signal);
      screen = { from: rig.from + offset, beats, reference: snippet };
    }
    /** Analyses by candidate settings (and window): a render already heard isn't heard again. */
    const heardBefore = new Map<string, { score: number; gaps: string[]; structural?: { gap: string; move: string } }>();
    /** A slot's knobs set to these values, by name (references move when tracks do). */
    const setSlot = async (slot: string, knobs: readonly Knob[], values: readonly number[], signal: AbortSignal) => {
      const last = current.get(slot)!; const fresh = await readKnobs(slot, signal);
      const byKey = new Map(fresh.knobs.map((knob) => [key(knob), knob]));
      const byDevice = new Map<string, { parameterRef: string; value: number }[]>();
      knobs.forEach((knob, index) => { const now_ = byKey.get(key(knob)); const device = fresh.devices[Number(knob.device.split(":")[0])]?.ref; if (now_ && typeof device === "string") byDevice.set(device, [...(byDevice.get(device) ?? []), { parameterRef: now_.ref, value: values[index]! }]); });
      // Live takes 64 of a device's values at a time.
      await quietly(undefined, async () => { for (const [deviceRef, set] of byDevice) for (let at = 0; at < set.length; at += 64) await step("set_device_parameters", { deviceRef, values: set.slice(at, at + 64) }, signal); });
      knobs.forEach((knob, index) => last.set(key(knob), values[index]!));
    };
    /** A track's closing limiter (the search's safety) with its input back at 0 dB. */
    const limiterAtUnity = async (track: string, signal: AbortSignal) => {
      const read = await readKnobs(track, signal);
      const limiter = read.devices.at(-1);
      const input = limiter?.className === "Limiter" ? read.knobs.find((knob) => knob.device === `${read.devices.length - 1}:${String(limiter.name ?? "Limiter")}` && /^(gain|input( gain)?)$/i.test(knob.name)) : undefined;
      const unity = input ? (input.min < 0 ? 0 : input.min === 0 && input.max === 1 ? 0.5 : undefined) : undefined;
      if (input && unity !== undefined && typeof limiter?.ref === "string") await quietly(undefined, () => step("set_device_parameters", { deviceRef: limiter.ref, values: [{ parameterRef: input.ref, value: unity }] }, signal));
    };

    return {
      slots,
      screens: screen !== undefined,
      async add(candidate, given) {
        const signal = AbortSignal.any([given, lifetime.signal]);
        try {
          const track = (await rows("track", { fields: ["name"] }, signal)).find((row) => row.ref === candidate.track);
          if (typeof track?.name !== "string") return `${candidate.track} isn't a track in this turn's discovery.`;
          if (slots.some((slot) => slot.name === track.name)) return `“${track.name}” is already in the search.`;
          const label = candidate.label ?? track.name;
          await addToRig(rig, { track: String(track.ref), name: track.name, label, ...(candidate.clip ? { clip: candidate.clip } : {}) }, signal);
          return await adopt({ name: track.name, label }, signal);
        } catch (error) { signal.throwIfAborted(); return error instanceof Error ? error.message : "It couldn't join the search."; }
      },
      async generation(trials, given, options = {}) {
        const signal = AbortSignal.any([given, lifetime.signal]);
        const screened = options.screen === true && screen !== undefined;
        const cacheKey = (trial: (typeof trials)[number]) => `${trial.slot}|${screened ? "screen" : "full"}|${trial.values.map((value) => value.toFixed(4)).join(",")}`;
        if (rendering) throw new ObservationError("Another render is running.");
        rendering = true;
        let files: Map<string, { file: string; start: number }>;
        const frozen = new Map<string, Set<string>>();
        let setFrom = Date.now(); let renderFrom = setFrom;
        try {
          setFrom = Date.now();
          // Each trial's values onto its slot, only what moved, references read fresh.
          const plans: { trial: (typeof trials)[number]; last: Map<string, number>; moved: { knob: Knob; value: number }[]; byKey: Map<string, Knob>; byDevice: Map<string, { parameterRef: string; value: number }[]> }[] = [];
          for (const trial of trials) {
            const last = current.get(trial.slot)!;
            const moved = trial.knobs.map((knob, index) => ({ knob, value: trial.values[index]! })).filter(({ knob, value }) => Math.abs((last.get(key(knob)) ?? NaN) - value) > 1e-6 || !last.has(key(knob)));
            if (!moved.length) continue;
            const fresh = await knobsNow(trial.slot, signal);
            const byKey = new Map(fresh.knobs.map((knob) => [key(knob), knob]));
            const byDevice = new Map<string, { parameterRef: string; value: number }[]>();
            for (const { knob, value } of moved) {
              const now_ = byKey.get(key(knob));
              if (!now_) continue;
              const device = fresh.devices[Number(knob.device.split(":")[0])]?.ref;
              if (typeof device !== "string") continue;
              byDevice.set(device, [...(byDevice.get(device) ?? []), { parameterRef: now_.ref, value }]);
            }
            plans.push({ trial, last, moved, byKey, byDevice });
          }
          // One change per device: on real Live a batch that met a knob Live won't take left the host uncertain.
          const settled = new Set<(typeof plans)[number]>();
          for (const plan of plans) {
            const refused = new Set<string>();
            if (!settled.has(plan)) {
              // A value Live won't take (a knob it keeps to steps it doesn't say, one off in this mode) is tried alone,
              // and a knob that still won't move leaves the search rather than stopping it.
              await quietly(undefined, async () => {
                for (const [deviceRef, values] of plan.byDevice) {
                  try { await step("set_device_parameters", { deviceRef, values }, signal); continue; } catch { signal.throwIfAborted(); }
                  for (const value of values) {
                    try { await step("set_device_parameters", { deviceRef, values: [value] }, signal); }
                    catch { signal.throwIfAborted(); const knob = plan.moved.find(({ knob }) => plan.byKey.get(key(knob))?.ref === value.parameterRef)?.knob; if (knob) refused.add(key(knob)); }
                  }
                }
              });
            }
            for (const { knob, value } of plan.moved) if (!refused.has(key(knob))) plan.last.set(key(knob), value);
            if (refused.size) frozen.set(plan.trial.slot, refused);
          }
          renderFrom = Date.now();
          // Every trial heard before at these settings: no pass at all.
          if (trials.length && trials.every((trial) => !trial.fresh && heardBefore.has(cacheKey(trial)))) files = new Map();
          else {
            if (screened) rig.window = { from: screen!.from, beats: screen!.beats }; else delete rig.window;
            files = await renderPass(rig, signal);
          }
        } finally { rendering = false; }
        const heardFrom = Date.now();
        const scores = new Map<string, number>(); const gaps = new Map<string, string[]>(); const silent: string[] = [];
        const structural = new Map<string, { gap: string; move: string }>();
        const tempo = currentTempo!;
        let cached = 0;
        const against = screened ? screen!.reference : reference;
        const beatsHeard = screened ? screen!.beats : rig.beats;
        await Promise.all(trials.map(async (trial) => {
          const name = trial.slot;
          const known = !trial.fresh ? heardBefore.get(cacheKey(trial)) : undefined;
          if (known) { cached++; scores.set(name, known.score); gaps.set(name, known.gaps); if (known.structural) structural.set(name, known.structural); return; }
          const rendered = files.get(name);
          if (!rendered) { silent.push(name); return; }
          const heard = await hear(rendered.file, { ...heardSpan(rendered.start, beatsHeard * 60 / tempo, focus), ...(focus ? { focus: focus === "section" ? "mix" : "sound" } : {}), signal });
          if (silentRender(heard)) { silent.push(name); return; }
          const close = closeness(heard, against, focus);
          scores.set(name, close.score); gaps.set(name, close.gaps);
          if (close.structural) structural.set(name, { gap: close.structural.gap, move: close.structural.move });
          heardBefore.set(cacheKey(trial), { score: close.score, gaps: close.gaps, ...(close.structural ? { structural: { gap: close.structural.gap, move: close.structural.move } } : {}) });
          if (heardBefore.size > 2_000) heardBefore.delete(heardBefore.keys().next().value!);
        }));
        if (process.env.KUMI_TIMING) console.error(`[generation · ${trials.length} trials] set ${renderFrom - setFrom} ms · render ${heardFrom - renderFrom} ms · hear ${Date.now() - heardFrom} ms`);
        return { scores, gaps, silent, frozen, structural, screened, cached };
      },
      async settle(slot, knobs, values, given) {
        const signal = AbortSignal.any([given, lifetime.signal]);
        try {
          await setSlot(slot, knobs, values, signal);
          // It stays where it is, at the level it was made at: the search's limiter input back at 0 dB.
          await limiterAtUnity(slot, signal);
          return undefined;
        } catch (error) { signal.throwIfAborted(); return error instanceof Error ? error.message : "Its settings couldn't be put back."; }
      },
      async keepBest(slot, knobs, values, given) {
        const signal = AbortSignal.any([given, lifetime.signal]);
        try {
          // The slot back at its best values, then copied to a track of its own: that one's the producer's to keep.
          await setSlot(slot, knobs, values, signal);
          // Copying a track moves every track after it: the knobs' references are read again.
          known.clear();
          // The last copy goes first: there's one best.
          const previous = bestSteps; bestSteps = [];
          await quietly(undefined, async () => { for (const id of [...previous].reverse()) await undoChange(id, signal, changes.get(id)?.record.family === "structure").catch(() => undefined); });
          for (const id of previous) changes.delete(id);
          const name = `Kumi · Goal best`;
          await quietly(bestSteps, async () => {
            const before = await rows("track", { fields: ["name"] }, signal);
            const at = before.findIndex((row) => row.name === slot);
            await step("change_structure", { action: "duplicate-track", ref: before[at]!.ref }, signal);
            const after = await rows("track", { fields: ["name"] }, signal);
            const copy = after[at + 1];
            if (typeof copy?.ref !== "string") throw new ObservationError("The copy didn't appear.");
            await step("rename", { kind: "track", ref: copy.ref, name: after.some((row) => row.name === name) ? `${name} ${randomUUID().slice(0, 3)}` : name }, signal);
            // The candidate may be muted (kept for A/B): its copy plays.
            await step("set_mixer", { trackRef: copy.ref, mute: false }, signal).catch(() => undefined);
            // And at the level it was made at: the search's limiter took its input 12 dB down; the copy's goes back to 0 dB.
            await limiterAtUnity(String((await rows("track", { fields: ["name"] }, signal)).find((row) => row.ref === copy.ref)?.name ?? name), signal).catch(() => undefined);
          });
          return name;
        } catch (error) { signal.throwIfAborted(); return error instanceof Error ? error.message : "The best couldn't be kept on its own track."; }
      },
      async tidy(top, given) {
        const signal = AbortSignal.any([given, lifetime.signal]);
        const said: string[] = [];
        const tracks = await rows("track", { fields: ["name"] }, signal).catch(() => [] as JsonObject[]);
        // Newest first: removing a later track doesn't move an earlier one.
        const order = [...slots].sort((a, b) => tracks.findIndex((row) => row.name === b.name) - tracks.findIndex((row) => row.name === a.name));
        for (const slot of order) {
          const ref = (await rows("track", { fields: ["name"] }, signal).catch(() => tracks)).find((row) => row.name === slot.name)?.ref;
          if (typeof ref !== "string") continue;
          if (top.includes(slot.name)) { await step("set_mixer", { trackRef: ref, mute: true }, signal).catch(() => undefined); continue; }
          // The rest go, by undoing the change that added each (Kumi's, this session), recorded onto or not.
          const made = [...changes.values()].reverse().find((entry) => entry.record.family === "structure" && entry.record.state === "applied" && entry.record.title.includes(`“${slot.name}”`));
          const undone = made ? await undoChange(made.record.id, signal, true).catch(() => undefined) : undefined;
          if (!undone || undone.isError) { await step("set_mixer", { trackRef: ref, mute: true }, signal).catch(() => undefined); said.push(`“${slot.name}” stays, muted.`); }
        }
        return said;
      },
      async close() { await closeRig(rig); return rig.notes; },
    };
  }
  /**
   * Render each candidate's Post FX onto a scratch track, with Main silenced, in one pass; hear each
   * against the reference; then undo every step (the scratch tracks go, though they recorded) and
   * put Main back exactly, whatever happened: an error, a cancelled turn, a dropped connection.
   */
  async function audition(request: AuditionRequest, originalSignal: AbortSignal): Promise<AuditionResult | string> {
    const began = Date.now();
    if (!available || lost || !tools || currentEpoch === undefined) return NO_CURRENT_LIVE;
    if (!supported({ since: RENDER_BRIDGE })) return tooOld({ since: RENDER_BRIDGE });
    if (rendering) return "An audition is already running; wait for it.";
    const tempo = currentTempo;
    if (!tempo) return "Kumi doesn't know the Set's tempo yet; try again.";
    const signal = AbortSignal.any([originalSignal, lifetime.signal]);
    const focus = request.focus;
    const notes: string[] = [];
    rounds.count++;
    const round = rounds.count;
    const tell = (title: string, playing?: boolean) => { try { options.onAction?.({ title, ...(playing !== undefined ? { playing } : {}) }); } catch { /* a listener failure must not affect Live */ } };
    const files: { take: AuditionTake; file: string; start: number }[] = [];
    const takes: AuditionTake[] = request.candidates.map((candidate, index) => ({ label: candidate.label ?? `Candidate ${index + 1}`, track: candidate.track }));
    let beats = request.beats ?? 8;
    rendering = true;
    let rig: Rig | undefined;
    try {
      const seconds = beats * 60 / tempo;
      tell(!toldQuietly ? `Listening to my version quietly (about ${Math.round(seconds + 6)} s a round)` : `Round ${round}: listening quietly`, true);
      toldQuietly = true;
      rig = await openRig(request.candidates, request.fromBeat, request.beats, signal);
      beats = rig.beats;
      const rendered = await renderPass(rig, signal);
      for (const [index, source] of rig.sources.entries()) {
        const found = rendered.get(source.name);
        // Where it is, in names that last beyond this turn: its track's, and its clip's scene.
        const take = takes.find((item) => item.track === source.track || item.track === source.name) ?? takes[index]!;
        take.where = { track: source.name, ...(source.scene !== undefined ? { clip: `scene:${source.scene}` } : {}) };
        if (found) files.push({ take, ...found }); else take.silent = true;
      }
    } catch (error) {
      if (originalSignal.aborted) notes.push("Stopped before it finished.");
      else notes.push(error instanceof Error ? error.message.slice(0, 400) : "The render failed.");
    } finally {
      if (rig) { await closeRig(rig); notes.push(...rig.notes); }
      rendering = false;
    }
    // Listening happens with Live back as it was.
    try {
      const reference = request.reference ? await heardReference(request.reference, request, signal) : undefined;
      for (const { take, file, start } of files) {
        const heard = await hear(file, { ...heardSpan(start, beats * 60 / tempo, focus), ...(focus ? { focus: focus === "section" ? "mix" : "sound" } : {}), signal });
        take.heard = { lufs: heard.loudness.integratedLufs, summary: heardSummary(heard) };
        take.render = { file, start };
        if (silentRender(heard)) { take.silent = true; continue; }
        if (reference) take.closeness = closeness(heard, reference, focus);
      }
      const scored = takes.filter((take) => take.closeness).sort((a, b) => b.closeness!.score - a.closeness!.score);
      const best = scored[0];
      if (takes.every((take) => take.silent) && files.length) notes.push("The render was silent: is the source playing in the Arrangement at that spot (its clips there, the track not muted)?");
      const previous = rounds.best;
      if (best && (previous === undefined || best.closeness!.score > previous)) rounds.best = best.closeness!.score;
      const result: AuditionResult = { takes, ...(best ? { best: best.label } : {}), ...(reference ? { reference: { file: reference.file, summary: heardSummary(reference) } } : {}), seconds: Math.round((Date.now() - began) / 100) / 10, notes };
      // HISTORY: one quiet line for the whole of it.
      // The score goes beside it in HISTORY, where an undo would be.
      const title = best ? `Auditioned ${takes.length === 1 ? best.label : `${takes.length} candidates · best ${best.label}`}`
        : takes.every((take) => take.silent) && files.length ? "Auditioned: the render was silent" : `Auditioned ${takes.length === 1 ? takes[0]!.label : `${takes.length} candidates`}`;
      emitChange({ id: `a${randomUUID().slice(0, 8)}`, family: "clip", title, state: "heard", ...(best ? { score: best.closeness!.score } : {}), at: now().getTime() });
      try {
        options.onAudition?.({ type: "auditioned", round, ...(best ? { best: { label: best.label, score: best.closeness!.score } } : {}), ...(previous !== undefined ? { previous } : {}),
          takes: [...scored.map((take) => ({ label: take.label, score: take.closeness!.score, ...(take.where ? { where: take.where } : {}) })), ...takes.filter((take) => !take.closeness).map((take) => ({ label: take.label, ...(take.silent ? { silent: true } : {}), ...(take.where ? { where: take.where } : {}) }))],
          gaps: best?.closeness!.gaps.slice(0, 3) ?? [], request, ...(reference ? { reference: heardSummary(reference) } : {}),
          ...(best?.closeness!.structural ? { structural: { gap: best.closeness.structural.gap, move: best.closeness.structural.move } } : {}) });
      } catch { /* a listener failure must not affect Live */ }
      tell(best ? `Auditioned · ${best.closeness!.score}%` : "Auditioned", false);
      return result;
    } catch (error) {
      tell("Auditioned", false);
      signal.throwIfAborted();
      return `${notes.length ? `${notes.join(" ")} ` : ""}Kumi couldn't listen to the render: ${error instanceof Error ? error.message.slice(0, 300) : "it failed"}`;
    }
  }
  /**
   * Hear tracks (or the mix) in the Set, the way the producer would: while Live plays and no stretch is
   * named, through Kumi's listening devices as it plays (nothing else in Live is touched); otherwise a
   * quiet pass over the stretch (the loop, or from the playhead), as an audition renders, with nothing
   * to compare. Each one's file and where its part starts.
   */
  async function hearInSet(request: HearRequest, originalSignal: AbortSignal): Promise<HeardTake[] | string> {
    if (!available || lost || !tools || currentEpoch === undefined) return NO_CURRENT_LIVE;
    const tempo = currentTempo;
    if (!tempo) return "Kumi doesn't know the Set's tempo yet; try again.";
    const signal = AbortSignal.any([originalSignal, lifetime.signal]);
    const set = (await rows("set", { fields: ["playing", "position", "loop"] }, signal).catch(() => [] as JsonObject[]))[0];
    const tell = (title: string, playing?: boolean) => { try { options.onAction?.({ title, ...(playing !== undefined ? { playing } : {}) }); } catch { /* a listener failure must not affect Live */ } };
    if (set?.playing === true && request.fromBeat === undefined) {
      const link = await earsReady(signal);
      if (link) return hearAsItPlays(link, request, tell, signal);
    }
    if (!supported({ since: RENDER_BRIDGE })) return tooOld({ since: RENDER_BRIDGE });
    if (rendering) return "Kumi is already listening to something; wait for it.";
    // Stopped: the loop when it's on, else from the playhead, a few bars.
    const loop = set?.loop && typeof set.loop === "object" ? set.loop as JsonObject : undefined;
    const looped = loop?.enabled === true && typeof loop.length === "number" && loop.length > 0;
    const fromBeat = request.fromBeat ?? (looped ? (typeof loop!.start === "number" ? loop!.start : 0) : typeof set?.position === "number" ? set.position : 0);
    const beats = Math.min(64, request.beats ?? (looped ? loop!.length as number : 4 * beatsPerBar));
    const candidates = request.mix ? [{ track: MIX_CANDIDATE, mix: true, label: "The whole mix" }] : request.tracks.map((track) => ({ track }));
    tell(`Listening quietly from ${bars(fromBeat)}`, true);
    rendering = true;
    let rig: Rig | undefined;
    const takes: HeardTake[] = [];
    try {
      rig = await openRig(candidates, fromBeat, beats, signal);
      const rendered = await renderPass(rig, signal);
      for (const source of rig.sources) {
        const found = rendered.get(source.name);
        if (found) takes.push({ label: source.mix ? "The whole mix" : source.name, file: found.file, start: found.start, seconds: beats * 60 / tempo, live: false });
      }
    } catch (error) {
      signal.throwIfAborted();
      return error instanceof Error ? error.message.slice(0, 400) : "Kumi couldn't hear that.";
    } finally {
      if (rig) await closeRig(rig);
      rendering = false;
      tell("Listened", false);
    }
    if (!takes.length) return `Nothing came through${rig?.notes.length ? `: ${rig.notes.join(" ")}` : ""}. Is something playing there in the Arrangement (its clips at ${bars(fromBeat)}, the track not muted)?`;
    return takes;
  }
  /** Hear tracks (or the mix) for a few seconds as Live plays them: a listening device each, taken away after. */
  async function hearAsItPlays(link: EarsLink, request: HearRequest, tell: (title: string, playing?: boolean) => void, signal: AbortSignal): Promise<HeardTake[] | string> {
    const seconds = Math.min(60, Math.max(2, request.seconds ?? 8));
    const steps: string[] = [];
    const placed: { label: string; tap: Tap }[] = [];
    try {
      await quietly(steps, async () => {
        if (request.mix) { placed.push({ label: "The whole mix", tap: await placeTap(link, (await mainVolume(signal)).ref, signal) }); return; }
        const tracks = [...await rows("track", { fields: ["name"] }, signal), ...await rows("return-track", { fields: ["name"] }, signal)];
        for (const named of request.tracks) {
          const found = tracks.find((track) => track.ref === named) ?? tracks.find((track) => track.name === named);
          if (!found || typeof found.ref !== "string") throw new ObservationError(`${named} isn't a track in this turn's discovery; discover it again.`);
          placed.push({ label: typeof found.name === "string" ? found.name : named, tap: await placeTap(link, found.ref, signal) });
        }
      });
      tell(`Listening to ${placed.length === 1 ? placed[0]!.label : `${placed.length} tracks`} as it plays (${Math.round(seconds)} s)`);
      await Promise.all(placed.map(({ tap }) => link.arm(tap, seconds + 2, signal)));
      await delay(seconds * 1000, undefined, { signal });
      const takes: HeardTake[] = [];
      await Promise.all(placed.map(async ({ label, tap }) => {
        const raw = join(earsFolder, `${randomUUID()}.raw`);
        try {
          const written = await link.write(tap, maxPath(raw), signal);
          const capture = await readCapture(raw, written.channels, written.sampleRate);
          const wav = join(earsFolder, `${randomUUID()}.wav`);
          await writeCaptureWav(wav, capture, 0, capture.left.length);
          takes.push({ label, file: wav, start: 0, seconds: capture.left.length / capture.sampleRate, live: true });
        } finally { await rm(raw, { force: true }).catch(() => undefined); }
      }));
      return takes.sort((a, b) => placed.findIndex((item) => item.label === a.label) - placed.findIndex((item) => item.label === b.label));
    } catch (error) {
      signal.throwIfAborted();
      return error instanceof Error ? error.message.slice(0, 400) : "Kumi couldn't hear that.";
    } finally {
      for (const { tap } of placed) link.stop(tap);
      const cleanup = AbortSignal.any([lifetime.signal, AbortSignal.timeout(changeTimeoutMs * 3)]);
      await quietly(undefined, async () => { for (const id of [...steps].reverse()) { await undoChange(id, cleanup).catch(() => undefined); changes.delete(id); } });
      tell("Listened");
      void pruneEars();
    }
  }
  /** Kumi's hands, started once (a Mac builds its helper the first time); undefined where there are none. */
  let handsSetup: Promise<Hands | undefined> | undefined;
  async function handsReady(): Promise<Hands | undefined> {
    if (options.hands === false) return undefined;
    const tell = (title: string) => { try { options.onAction?.({ title }); } catch { /* a listener failure must not affect Live */ } };
    handsSetup ??= (options.hands ? options.hands.open() : openHands({ onBuild: tell })).catch(() => undefined);
    const hands = await handsSetup;
    if (!hands) handsSetup = undefined;
    return hands;
  }
  /** Live's commands whose menu item toggles (one item, retitled with the selection). */
  const toggles = new Set(["freeze_track", "unfreeze_track"]);
  /** Live's menus as last read (read again when an item isn't found: they change with the selection and the view). */
  let menuItems: MenuItem[] | undefined;
  /** The Set's tracks by name, in order: what a command changed is told from them. */
  async function trackNames(signal: AbortSignal): Promise<string[]> {
    const rows_ = [...await rows("track", { fields: ["name"] }, signal), ...await rows("return-track", { fields: ["name"] }, signal)];
    return rows_.map((row) => (typeof row.name === "string" ? row.name : ""));
  }
  /** A track the model names, by its reference from this turn or by its name; its reference. */
  async function trackRefOf(named: string, signal: AbortSignal): Promise<string> {
    const tracks = [...await rows("track", { fields: ["name"] }, signal), ...await rows("return-track", { fields: ["name"] }, signal)];
    const found = tracks.find((track) => track.ref === named) ?? tracks.find((track) => track.name === named);
    if (!found || typeof found.ref !== "string") throw new ObservationError(`${named} isn't a track in this turn's discovery; discover it again.`);
    return found.ref;
  }
  /**
   * live_command: select what the command works on, press it in Live's menus (or the keys given), and
   * say what changed. Changes Live makes this way are undone with Live's own undo, so HISTORY keeps them
   * with that said.
   */
  async function liveCommand(input: JsonObject, originalSignal: AbortSignal): Promise<{ text: string; isError: boolean }> {
    const signal = AbortSignal.any([originalSignal, lifetime.signal]);
    if (!available || lost || !tools || currentEpoch === undefined) return { text: NO_CURRENT_LIVE, isError: true };
    const hands = await handsReady();
    if (!hands) return { text: process.platform === "darwin" ? "Kumi can't use Live's menus on this Mac yet: its helper is built with Xcode's command line tools (run xcode-select --install), or comes with Kumi's next update." : "Kumi can't use Live's menus on this computer.", isError: true };
    const tell = (title: string) => { try { options.onAction?.({ title }); } catch { /* a listener failure must not affect Live */ } };
    try {
      if (!await hands.trusted()) {
        // macOS shows its own request once; the producer turns Kumi's terminal on, then asks again.
        await hands.trusted(true).catch(() => false);
        return { text: "Kumi needs Accessibility access to use Live's menus. macOS just asked for it: in System Settings › Privacy & Security › Accessibility, turn on the app Kumi runs in (your terminal), then ask again. Tell the producer exactly that.", isError: true };
      }
      // Live's dialog first, when that's what's asked.
      if (typeof input.answer === "string") {
        const reply = await hands.answer(input.answer, signal);
        if (!reply.ok) {
          const open = await hands.dialog(signal).catch(() => ({ open: false as const }));
          return { text: open.open ? `Live's dialog has no "${input.answer}" button; its buttons: ${(open as { buttons?: string[] }).buttons?.join(", ") || "none Kumi can see"}.` : "Live has no dialog open.", isError: true };
        }
        tell(`Pressed ${input.answer} in Live's dialog`);
        await delay(200, undefined, { signal });
        const next = await hands.dialog(signal).catch(() => ({ open: false as const }));
        return { text: JSON.stringify({ pressed: input.answer, ...(next.open ? { dialog: next } : {}) }), isError: false };
      }
      const command = typeof input.command === "string" ? COMMANDS[input.command] : undefined;
      if (typeof input.command === "string" && !command) return { text: `Kumi doesn't know the command ${input.command}; name the menu item instead (menu).`, isError: true };
      const menu = Array.isArray(input.menu) ? input.menu.filter((part): part is string => typeof part === "string") : undefined;
      const keys = Array.isArray(input.keys) ? input.keys.filter((part): part is string => typeof part === "string") : undefined;
      if (!command && !menu?.length && !keys?.length) return { text: "Give a command, a menu item (menu) or keys.", isError: true };
      // What it works on, selected in Live first, the way the producer would before pressing the command.
      const named = { track: typeof input.track === "string" ? input.track : undefined, tracks: Array.isArray(input.tracks) ? input.tracks.filter((item): item is string => typeof item === "string") : [],
        clip: typeof input.clip === "string" ? input.clip : undefined };
      const target = command?.target ?? "none";
      if ((target === "track" || target === "track-or-clip") && !named.track && !named.clip && !named.tracks.length) return { text: `${input.command} works on a track: give track.`, isError: true };
      if (target === "tracks" && named.tracks.length < 2 && !named.track) return { text: "Give the tracks to group, side by side, first to last (tracks).", isError: true };
      if (target === "clip" && !named.clip) return { text: `${input.command} works on a clip: give clip (its clipRef from this turn, or "selected" for the one selected in Live).`, isError: true };
      const before = await trackNames(signal);
      const savedBefore = project?.path && existsSync(project.path) ? statSync(project.path).mtimeMs : undefined;
      let what = "";
      /** The tracks selected for it, by name (to see a freeze through). */
      let chosen: string[] = [];
      if (named.clip === "selected") {
        // The clip the producer selected in Live: pressed as it is.
        what = " the selected clip";
      } else if (named.clip && (target === "clip" || target === "track-or-clip" || target === "none")) {
        const long = String(lengthen(named.clip, "clipRef"));
        const session = /^(\d+):clip:(\d+):(\d+)$/.exec(long);
        // Live's scripting can't select a clip in the Arrangement, nor can its accessibility.
        if (!session && target === "clip") return { text: "Kumi can't select a clip in the Arrangement for Live's own commands yet: ask the producer to click it, then use clip: \"selected\" (or work on a Session clip).", isError: true };
        // Its slot selected with the Session in front: Live's own selection for the Create menu's commands.
        if (session) await act(ACTIONS.find((kind) => kind.tool === "show")!, { action: "focus-view", view: "Session" }, signal);
        // The clip's slot, so Live's selection is on it (Session view), and the clip in the Clip view.
        const slot = session ? `${session[1]}:clip_slot:${session[2]}:${session[3]}` : undefined;
        if (slot && !refs.has(slot)) refs.set(slot, "clip-slot");
        const selected = slot ? await act(ACTIONS.find((kind) => kind.tool === "select")!, { slotRef: slot, detailClipRef: named.clip }, signal)
          : await act(ACTIONS.find((kind) => kind.tool === "select")!, { detailClipRef: named.clip }, signal);
        if (selected.isError) return { text: `Kumi couldn't select that clip in Live: ${selected.text.slice(0, 300)}`, isError: true };
        what = " the clip";
      } else if (named.track || named.tracks.length) {
        const list = named.tracks.length ? named.tracks : [named.track!];
        // Each by its name in Live's track headers, and which of that name it is (several tracks can share one).
        const all = [...await rows("track", { fields: ["name", "kind", "isFrozen", "isVisible"] }, signal), ...await rows("return-track", { fields: ["name"] }, signal)];
        const targets: { name: string; nth: number }[] = [];
        for (const one of list) {
          const ref = await trackRefOf(one, signal);
          const index = all.findIndex((track) => track.ref === ref);
          const row = all[index];
          const name = typeof row?.name === "string" ? row.name : one;
          // Live's freeze and group commands toggle: what's already so is said, not pressed (it would undo it).
          const already = input.command === "freeze_track" && row?.isFrozen === true ? "is frozen already"
            : (input.command === "unfreeze_track" || input.command === "flatten_track") && row?.isFrozen === false ? `isn't frozen${input.command === "flatten_track" ? " (Flatten works on a frozen track: freeze it first)" : ""}`
            : input.command === "ungroup_tracks" && row && row.kind !== "group" ? "isn't a group" : undefined;
          if (already) return { text: `${name} ${already}; nothing pressed.`, isError: true };
          if (row?.isVisible === false) return { text: `${name} is inside a folded group, so Live's track headers don't show it: unfold the group, then ask again.`, isError: true };
          targets.push({ name, nth: all.slice(0, Math.max(0, index)).filter((track) => track.name === name).length });
        }
        const selected = await hands.tracks(targets, signal).catch((error: unknown) => ({ ok: false, error: error instanceof Error ? error.message : "failed" }) as HandsReply);
        if (!selected.ok && selected.error === "no-track") {
          const missing = Array.isArray(selected.missing) ? (selected.missing as string[]).join(", ") : list.join(", ");
          return { text: `Live's track headers don't show ${missing}: is it inside a folded group? Unfold the group, then ask again.`, isError: true };
        }
        if (!selected.ok) {
          // An older helper, or no track headers to be found: one track can still be selected through Live's scripting.
          if (list.length > 1) return { text: `Kumi couldn't select several tracks in Live here (${String(selected.error)}); select them in Live, then ask again.`, isError: true };
          const viaScript = await act(ACTIONS.find((kind) => kind.tool === "select")!, { trackRef: await trackRefOf(list[0]!, signal) }, signal);
          if (viaScript.isError) return { text: `Kumi couldn't select ${list[0]} in Live: ${viaScript.text.slice(0, 300)}`, isError: true };
        } else tell(`Selected ${targets.map((target) => target.name).join(", ")}`);
        chosen = targets.map((target) => target.name);
        what = list.length > 1 ? ` ${list.length} tracks` : ` ${targets[0]!.name}`;
      }
      // Press it: the command's menu item (wherever Live keeps it), the item named, or the keys.
      let pressed: string;
      let key: string | undefined;
      if (command || menu?.length) {
        const look = (items: readonly MenuItem[]) => (command ? findItem(items, command.titles) : items.find((item) => item.path.length === menu!.length && item.path.every((part, index) => part.toLowerCase() === menu![index]!.toLowerCase()))
          ?? findItem(items, [menu!.at(-1)!]));
        menuItems ??= await hands.menus(signal);
        let item = look(menuItems);
        if (!item) { menuItems = await hands.menus(signal); item = look(menuItems); }
        if (!item) return { text: `Live's menus don't have ${command ? `“${command.titles[0]}”` : `“${menu!.join(" › ")}”`} here: it may need a newer Live, Live Suite, or something selected first.`, isError: true };
        const reply = await hands.menu(item.path, { signal, ...(command ? { titles: command.titles } : {}) });
        if (!reply.ok) {
          return { text: reply.error === "disabled" ? `Live has “${item.path.join(" › ")}” greyed out right now: it needs the right thing selected (and some commands need the Arrangement or Session view in front).`
            : `Live didn't take “${item.path.join(" › ")}” (${String(reply.error)}).`, isError: true };
        }
        // What Live's menu said as it was pressed (its titles follow the selection: "Group Tracks"); a toggle's
        // title can lag the selection, so a freeze is said as what it did.
        const said = toggles.has(String(input.command)) ? command!.titles[0]! : typeof reply.title === "string" && reply.title ? reply.title : item.path.at(-1)!;
        pressed = [...item.path.slice(0, -1), said].join(" › "); key = shortcut(item);
      } else {
        const reply = await hands.keys(keys!, { signal });
        if (!reply.ok) return { text: `Live didn't take those keys (${String(reply.error)}).`, isError: true };
        pressed = keys!.join(", ");
      }
      // A freeze renders first (Live shows its progress): seen through to the end in Live's own track state, so
      // what's said is what happened, and the next command finds Live's menus caught up.
      const toggle = input.command === "freeze_track" ? true : input.command === "unfreeze_track" || input.command === "flatten_track" ? false : undefined;
      if (toggle !== undefined && chosen.length) {
        tell(`${toggle ? "Freezing" : input.command === "flatten_track" ? "Flattening" : "Unfreezing"}${what}`);
        for (let wait = 0; wait < 600; wait++) {
          const rows_ = await rows("track", { fields: ["name", "isFrozen"] }, signal);
          if (chosen.every((name) => rows_.some((row) => row.name === name && row.isFrozen === toggle))) break;
          // A dialog with buttons asks something (the progress has none): it's said, not waited out.
          const open = await hands.dialog(signal).catch(() => ({ open: false as const, buttons: [] as string[] }));
          if (open.open && (open.buttons?.length ?? 0) > 0) break;
          await delay(100, undefined, { signal });
        }
      }
      // A bounce or a conversion renders first, Live's progress window up meanwhile: seen through to its end.
      if (command && /Bounce|Convert|Separate|Slice|Consolidate|Flatten|Paste Bounced/.test(command.titles[0]!)) {
        const working = async () => { const open = await hands.dialog(signal).catch(() => ({ open: false as const, buttons: [] as string[] })); return open.open && (open.buttons?.length ?? 0) === 0; };
        await delay(150, undefined, { signal });
        for (let wait = 0; wait < 1_200 && await working(); wait++) await delay(100, undefined, { signal });
      }
      tell(command ? `${command.done}${what}` : `Pressed ${pressed} in Live`);
      // Live does it on its own time: a bounce or a freeze renders first.
      const dialog = await (async () => {
        for (let check = 0; check < (command?.dialog ? 6 : 2); check++) {
          await delay(command?.dialog ? 250 : 150, undefined, { signal });
          const open = await hands.dialog(signal).catch(() => ({ open: false as const }));
          // Live's progress (no buttons) isn't a question.
          if (open.open && (open.buttons?.length ?? 0) > 0) return open;
        }
        return undefined;
      })();
      // Live's structure may have changed under every reference: read it all again.
      refs.clear(); known.clear(); cursors.clear(); shortRefs.clear(); longRefs.clear(); observationGeneration++;
      let after = await trackNames(signal);
      // Tracks it adds or renames show a moment after (in place, a bounce may keep every name: then this ends soon).
      const same = (a: readonly string[], b: readonly string[]) => a.length === b.length && a.every((name, index) => name === b[index]);
      for (let wait = 0; wait < 8 && !dialog && command && command.target !== "none" && same(after, before) && /Bounce|Convert|Separate|Slice|Group/.test(command.titles[0]!); wait++) {
        await delay(250, undefined, { signal }); after = await trackNames(signal);
      }
      const added = after.filter((name) => !before.includes(name) || after.filter((other) => other === name).length > before.filter((other) => other === name).length);
      const removed = before.filter((name) => !after.includes(name));
      const saved = savedBefore !== undefined && project?.path && existsSync(project.path) && statSync(project.path).mtimeMs > savedBefore;
      if (command || added.length || removed.length) {
        emitChange({ id: `l${randomUUID().slice(0, 8)}`, family: "structure", title: command ? `${command.done}${what}` : `Pressed ${pressed} in Live`, state: "kept",
          note: "Done with Live's own command: Live's undo (Cmd-Z) takes it back.", at: now().getTime() });
      }
      return { text: JSON.stringify({ pressed, ...(key ? { liveShortcut: key } : {}), ...(added.length ? { newTracks: added } : {}), ...(removed.length ? { goneTracks: removed } : {}),
        ...(saved ? { saved: true } : {}), ...(dialog ? { dialog, next: "Answer it with answer (the button's title), or tell the producer what it asks." } : {}),
        note: "References from before are gone: discover again before using any." }), isError: false };
    } catch (error) {
      signal.throwIfAborted();
      return { text: error instanceof HandsError || error instanceof ObservationError ? error.message : `Kumi couldn't use Live's menus: ${error instanceof Error ? error.message.slice(0, 200) : "it failed"}`, isError: true };
    }
  }
  /** The plugin tool: a plug-in's guide (Kumi's knowledge, set against its real parameters), or a wavetable for it. */
  async function pluginTool(input: JsonObject, originalSignal: AbortSignal): Promise<{ text: string; isError: boolean }> {
    const signal = AbortSignal.any([originalSignal, lifetime.signal]);
    if (!available || lost || !tools || currentEpoch === undefined) return { text: NO_CURRENT_LIVE, isError: true };
    if (typeof input.device !== "string") return { text: "Give the plug-in device (its deviceRef from this turn).", isError: true };
    const long = String(lengthen(input.device, "deviceRef"));
    try { requireFreshReferences({ deviceRef: long }); } catch (error) { return { text: error instanceof Error ? error.message : "deviceRef must come from discovery in this turn", isError: true }; }
    try {
      // Its name, as Live shows it, from its track's devices.
      const track = /^(\d+):device:(\d+):/.exec(long);
      let name = "";
      if (track) {
        const devices = await tools.call("live_discover", { kind: "device", parent: `${track[1]}:track:${track[2]}`, fields: ["name", "className"], limit: pageLimit(), budget: wholeBudget() }, signal, { host: true });
        const row = devices.isError ? undefined : ((payload(devices).items as JsonObject[] | undefined) ?? []).map((item) => object(item)).find((item) => item.ref === long);
        name = typeof row?.name === "string" ? row.name : "";
      }
      const adapter = adapterFor(name);
      if (input.action === "wavetable") {
        const spec = object(input.wavetable ?? {});
        const label = typeof spec.name === "string" && spec.name.trim() ? spec.name.trim().replace(/[\\/:*?"<>|]+/g, " ").slice(0, 48) : "Kumi Wavetable";
        const fromAudio = spec.from_audio && typeof spec.from_audio === "object" ? object(spec.from_audio) : undefined;
        const frames = await buildWavetable({
          ...(Array.isArray(spec.keyframes) ? { keyframes: spec.keyframes as Keyframe[] } : {}),
          ...(typeof spec.count === "number" ? { count: spec.count } : {}),
          ...(fromAudio && typeof fromAudio.file === "string" ? { fromAudio: { file: audioPath(fromAudio.file), ...(typeof fromAudio.start === "number" ? { start: fromAudio.start } : {}), ...(typeof fromAudio.seconds === "number" ? { seconds: fromAudio.seconds } : {}) } } : {}) });
        const folder = folderFor(adapter?.folders?.wavetables) ?? join(options.userLibrary ?? userLibrary(), "Kumi", "Wavetables");
        await mkdir(folder, { recursive: true });
        let file = join(folder, `${label}.wav`);
        for (let index = 2; existsSync(file) && index < 1000; index++) file = join(folder, `${label} ${index}.wav`);
        await writeWavetable(file, frames);
        try { options.onAction?.({ title: `Made the wavetable ${basename(file, ".wav")} (${frames.length} frames)` }); } catch { /* a listener failure must not affect Live */ }
        return { text: JSON.stringify({ made: basename(file, ".wav"), file, frames: frames.length,
          next: adapter ? `It's in ${adapter.name}'s wavetable folder: in its window (set_device_details isEditorOpen opens it), the oscillator's wavetable menu lists it. Picking it there is a click for the producer; say where.`
            : "Load it in the synth's oscillator from that file (most read 2048-sample frames)." }), isError: false };
      }
      // Every name the plug-in has, and the ones Live lets Kumi turn.
      const read = await tools.call("live_device_read", { deviceRef: long, what: "parameter-names" }, signal, { host: true });
      if (read.isError) return { text: /only a plug-in/.test(resultText(read)) ? "That isn't a plug-in: its parameters are all in discovery (kind parameter)." : `Live didn't list that plug-in's parameters: ${resultText(read).slice(0, 200)}`, isError: true };
      const names = ((payload(read).names as unknown[] | undefined) ?? []).filter((item): item is string => typeof item === "string");
      const exposed = (await deviceParameters(long, ["ref", "name", "displayValue"], signal))
        .filter((row) => typeof row.name === "string" && row.name !== "Device On").map((row) => ({ name: String(row.name), ref: String(row.ref), ...(typeof row.displayValue === "string" ? { display: row.displayValue } : {}) }));
      const guide = pluginGuide(name || "this plug-in", adapter, names, exposed);
      const text = JSON.stringify(guide);
      return { text: Buffer.byteLength(text) <= 48 * 1024 ? text : JSON.stringify({ ...guide, parameters: { ...(guide.parameters as JsonObject), groups: "too many to list" } }), isError: false };
    } catch (error) {
      signal.throwIfAborted();
      return { text: error instanceof ObservationError ? error.message : `Kumi couldn't read that plug-in: ${error instanceof Error ? error.message.slice(0, 200) : "it failed"}`, isError: true };
    }
  }
  /**
   * An audition cut off by a crash left Main silent: with that Set open again, Main goes back to where
   * it was, and the producer is told (with any scratch tracks to delete). What was said, or nothing.
   */
  async function restoreAfterCrash(identity: string, path: string | undefined, signal: AbortSignal): Promise<string | undefined> {
    const pending = restore.load();
    if (!pending || rendering) return undefined;
    // The same Set: its file, or (unsaved) its identity in this Live.
    if (pending.path ? pending.path !== path : pending.set !== identity) return undefined;
    // Quietly: putting Main back isn't one of Kumi's changes for HISTORY.
    const back = await quietly(undefined, () => putMainBack(pending.volume, signal).catch(() => false));
    if (!back) return undefined;
    restore.clear();
    const said = `Kumi's last render was cut off, so it put Main back to ${faderDb(pending.volume)}.${pending.scratch?.length ? ` Delete its render tracks if they're still there: ${pending.scratch.join(", ")}.` : ""}`;
    try { options.onAction?.({ title: said }); } catch { /* a listener failure must not affect Live */ }
    return said;
  }
  /** The reference, heard once per file and span this session. */
  async function heardReference(named: string, request: AuditionRequest, signal: AbortSignal): Promise<Analysis> {
    const file = (await clipFile(named, signal).catch(() => undefined)) ?? audioPath(named);
    const key = JSON.stringify([file, request.referenceFrom ?? 0, request.referenceSeconds ?? null, request.focus ?? null]);
    const known = referenceCache.get(key);
    if (known) return known;
    const heard = await hear(file, { ...(request.referenceFrom !== undefined ? { start: request.referenceFrom } : {}), ...(request.referenceSeconds !== undefined ? { seconds: request.referenceSeconds } : {}),
      ...(request.focus ? { focus: request.focus === "section" ? "mix" : "sound" } : {}), signal });
    referenceCache.set(key, heard);
    if (referenceCache.size > 32) referenceCache.delete(referenceCache.keys().next().value!);
    return heard;
  }
  /** An audio clip in the Set, named by its clipRef from discovery, as the file it plays; undefined for anything that isn't a clip. */
async function clipFile(named: string, originalSignal: AbortSignal): Promise<string | undefined> {
    const given = named.trim(); const ref = longRefs.get(given) ?? given;
    const arrangement = /^(\d+):arrangement_clip:(\d+):\d+$/.exec(ref); const session = /^(\d+):clip:(\d+):(\d+)$/.exec(ref);
    if (!arrangement && !session) {
      if (/^(arrangement_clip|clip):[\w:]+$/.test(given)) throw new ObservationError("That clip isn't one from this turn's discovery; discover it again.");
      return undefined;
    }
    if (!available || lost || !tools) throw new ObservationError("Live isn't connected, so Kumi can't find that clip's file.");
    const signal = AbortSignal.any([originalSignal, lifetime.signal, AbortSignal.timeout(15_000)]);
    // A Session clip's parent is its slot; an Arrangement clip's, its track.
    const query = arrangement ? { kind: "arrangement-clip", parent: `${arrangement[1]}:track:${arrangement[2]}` } : { kind: "session-clip", parent: `${session![1]}:clip_slot:${session![2]}:${session![3]}` };
    const read = await pages({ ...query, fields: ["ref", "name", "isAudio", "filePath"], limit: pageLimit(), budget: wholeBudget() }, signal);
    const items = read.isError ? [] : payload(read).items;
    const clip = (Array.isArray(items) ? items : []).map((item) => object(item)).find((item) => item.ref === ref);
    if (!clip) throw new ObservationError("That clip isn't in the Set any more; discover it again.");
    if (clip.isAudio === false) throw new ObservationError("That's a MIDI clip, which has no sound of its own: record its track to audio first (resampling), then listen to the recording.");
    if (typeof clip.filePath !== "string" || !clip.filePath) throw new ObservationError("Live didn't say which file that clip plays.");
    return clip.filePath;
  }
  /**
   * One HISTORY line for changes made quietly (an arrangement): its undo takes them back, latest first. Those
   * `apart` aren't undone with it (the playhead's move); they, and working steps already taken back, go. Its id.
   */
  function grouped(title: string, ids: readonly string[], apart: readonly string[] = []): string | undefined {
    const members = ids.filter((id) => !apart.includes(id) && ["applied", "unsure", "kept"].includes(changes.get(id)?.record.state ?? ""));
    const gone = ids.filter((id) => !members.includes(id));
    release(gone.flatMap((id) => (changes.get(id)?.record.state === "applied" ? [changes.get(id)!.transactionId] : [])));
    for (const id of gone) changes.delete(id);
    if (!members.length) return undefined;
    const record: ChangeRecord = { id: nextChangeId(), family: "clip", title: title.slice(0, 160), state: "applied", at: now().getTime() };
    for (const id of members) changes.get(id)!.within = record.id;
    changes.set(record.id, { record, transactionId: "", members });
    emitChange(record); scheduleSave(20_000);
    return record.id;
  }
  /** What arranging needs of Live: Kumi's reads and changes, quietly, with one line in HISTORY and one undo step in Live. */
  function arrangeHost(): ArrangeHost {
    // The first change checks that Live is still the Set Kumi read; the rest follow on from it.
    let confirmed = false;
    // Opening and closing Live's undo step aren't the answer's to cancel, as in a plan.
    const lasting = () => AbortSignal.any([lifetime.signal, AbortSignal.timeout(10_000)]);
    return {
      tempo: () => currentTempo, beatsPerBar: () => beatsPerBar,
      read: (kind, extra, signal) => rows(kind, extra, signal),
      offers: (tool) => { const kind = CHANGES.find((candidate) => candidate.tool === tool); return Boolean(kind && supported(kind) && tools?.has(kind.preview) && tools.has(kind.apply)); },
      async change(tool, input, signal) {
        const outcome = await change(CHANGES.find((candidate) => candidate.tool === tool)!, input, signal, confirmed);
        let reply: JsonObject = {};
        try { reply = JSON.parse(outcome.text) as JsonObject; } catch { /* a refusal in words */ }
        if (outcome.isError || typeof reply.change !== "string") throw new ObservationError(`${typeof reply.changed === "string" ? `${reply.changed}: ` : ""}${outcome.text.slice(0, 400)}`);
        confirmed = true;
        return { id: reply.change, ...(typeof reply.ref === "string" ? { ref: reply.ref } : {}) };
      },
      undo: async (id, signal) => !(await undoChange(id, signal)).isError,
      async quietly(work) { const ids: string[] = []; const value = await quietly(ids, work); return { value, ids }; },
      record: (title, ids, apart) => grouped(title, ids, apart),
      async undoStep() {
        if (!tools?.has("live_undo_step_begin") || !tools.has("live_undo_step_end")) return { opened: false, close: async () => {} };
        let stepId: string | undefined;
        try { const opened = payload(await tools.call("live_undo_step_begin", { label: "Kumi", timeoutMs: 600_000 }, lasting(), { host: true })); if (typeof opened.stepId === "string") stepId = opened.stepId; }
        catch { /* Live's undo then has a step for each change */ }
        return { opened: stepId !== undefined, close: async () => { if (stepId) await tools!.call("live_undo_step_end", { stepId }, lasting(), { host: true }).catch(() => undefined); } };
      },
      keepCopy: (signal) => keepCopy(signal),
      tell: (title) => { try { options.onAction?.({ title }); } catch { /* a listener failure must not affect Live */ } },
    };
  }
  function definitions(): KernelTool[] {
    const reads: KernelTool[] = tools!.list().map((tool) => ({ name: tool.name, description: tool.description ?? "Read current Live state", inputSchema: tool.inputSchema,
      execute: (input, signal) => invoke(tool.name, input, signal) }));
    // A change whose target an earlier step can create is offered by any bridge that makes changes (has undo).
    const edits: KernelTool[] = CHANGES.filter((kind) => !kind.internal && supported(kind) && ((kind.always && kind.inputSchema && tools!.has("live_undo")) || (tools!.has(kind.preview) && tools!.has(kind.apply)))).map((kind) => ({
      name: kind.tool, description: kind.description,
      inputSchema: kind.fallbackSchema ? tools!.tool(kind.preview)?.inputSchema as JsonObject ?? kind.inputSchema! : kind.inputSchema ?? (kind.schema ? kind.schema(tools!.tool(kind.preview)!.inputSchema as JsonObject) : tools!.tool(kind.preview)!.inputSchema as JsonObject),
      execute: (input, signal) => change(kind, input, signal) }));
    const actions: KernelTool[] = ACTIONS.filter((kind) => supported(kind) && tools!.has(kind.preview) && tools!.has(kind.apply)).map((kind) => ({
      name: kind.tool, description: kind.description, inputSchema: kind.inputSchema ?? tools!.tool(kind.preview)!.inputSchema as JsonObject,
      execute: async (input, signal) => { const outcome = await act(kind, input, signal); return { text: outcome.text, isError: outcome.isError }; } }));
    const undo: KernelTool[] = tools!.has("live_undo") ? [{ name: UNDO_TOOL, description: UNDO_DESCRIPTION,
      inputSchema: { type: "object", properties: { change: { type: "string", minLength: 1, maxLength: 32, description: "A change id such as c3, or \"last\"" } }, required: ["change"], additionalProperties: false },
      execute: async (input, signal) => { const outcome = await undoChange(typeof input.change === "string" ? input.change : "last", signal); return { text: outcome.text, isError: outcome.isError }; } }] : [];
    const sampleSearch: KernelTool = { name: FIND_SAMPLES, description: FIND_SAMPLES_DESCRIPTION, inputSchema: FIND_SAMPLES_SCHEMA,
      execute: async (input, signal) => {
        const named = Array.isArray(input.folders) ? input.folders.filter((folder): folder is string => typeof folder === "string") : [];
        const folders = named.map((folder) => folderPath(folder));
        if (folders.some((folder) => !folder)) return { text: "Name folders by their full path, such as ~/Samples or /Users/me/Music/Drums.", isError: true };
        const words = Array.isArray(input.words) ? input.words.filter((word): word is string => typeof word === "string") : [];
        const limit = typeof input.limit === "number" && Number.isInteger(input.limit) ? Math.min(50, Math.max(1, input.limit)) : 20;
        const found = await findSamples({ folders: folders.length ? folders as string[] : defaultSampleFolders(), words, limit, random: input.random === true, signal });
        for (const sample of found.samples) { samples.delete(sample.path); samples.set(sample.path, sample); }
        while (samples.size > 5_000) samples.delete(samples.keys().next().value!);
        return { text: JSON.stringify({ samples: found.samples.map((sample) => ({ name: sample.name, path: sample.path, ...(sample.seconds !== undefined ? { seconds: sample.seconds } : {}), kb: Math.round(sample.bytes / 1024) })),
          matched: found.matched, looked: found.scanned, ...(found.partial ? { partial: true } : {}), ...(found.missing.length ? { missing: found.missing } : {}),
          ...(folders.length ? {} : { searched: "the User Library, Live's Core Library and Factory Packs" }) }), isError: false };
      } };
    const batch: KernelTool[] = tools!.has("live_undo") && edits.length ? [{ name: MAKE_CHANGES, description: MAKE_CHANGES_DESCRIPTION,
      inputSchema: { type: "object", additionalProperties: false, required: ["steps"], properties: { steps: { type: "array", minItems: 1, maxItems: MAX_CHANGES_PER_TURN, items: {
        type: "object", additionalProperties: false, required: ["tool", "input"], properties: {
          tool: { type: "string", enum: [...edits.map((item) => item.name), ...actions.map((item) => item.name), WAIT] }, input: { type: "object", description: "What that tool takes; \"@name\" for what an earlier step made; wait takes {\"beats\": 8} or {\"seconds\": 4}" },
          as: { type: "string", pattern: "^[a-zA-Z][a-zA-Z0-9_]{0,31}$", description: "Name what this step makes (a new track, a loaded device) for later steps" },
          each: { type: "object", description: "Repeat this step: each field's list gives that input field its value run by run, e.g. {\"note\": [36, 37, 38, 39]}, or several lists of one length, e.g. {\"parameterRef\": [\"parameter:3\", \"parameter:9\"], \"value\": [0.5, 1]}", additionalProperties: { type: "array", maxItems: MAX_CHANGES_PER_TURN } } } } },
        final: { type: "boolean", description: "These changes complete the request: Kumi says what changed and you aren't called again. Leave it out to see the results and carry on." } } },
      execute: (input, signal) => makeChanges(input, signal), stream: (signal, onStart) => streamChanges(signal, onStart) }] : [];
    // Devices Kumi makes reach Live through its Browser: offered when the bridge can find and load one there.
    const devices: KernelTool[] = tools!.has("live_browser_inspect") && tools!.has("live_browser_load_preview") ? [deviceTool({ userLibrary: options.userLibrary ?? userLibrary(),
      // Asked of the bridge directly: a device not listed yet mustn't cost the model its references.
      browserSees: async (itemId, signal) => { try { return !(await tools!.call("live_browser_inspect", { itemId }, signal)).isError; } catch { return false; } } })] : [];
    const watcher: KernelTool[] = PROJECT_TOOLS.slice(1).every((name) => tools!.has(name)) ? [{ name: WATCH_TOOL, description: WATCH_DESCRIPTION,
      inputSchema: { type: "object", additionalProperties: false, required: ["action"], properties: { action: { type: "string", enum: ["start", "stop"] } } },
      execute: (input, signal) => watch(input, signal) }] : [];
    const auditions: KernelTool[] = supported({ since: RENDER_BRIDGE }) && tools!.has("live_undo") && tools!.has("live_recording_preview") ? [{ name: AUDITION_TOOL, description: AUDITION_DESCRIPTION, inputSchema: AUDITION_SCHEMA,
      execute: async (input, signal) => {
        const request = auditionRequest(input);
        if (typeof request === "string") return { text: request, isError: true };
        const result = await audition(request, signal);
        if (typeof result === "string") return { text: result, isError: true };
        const round = rounds.count;
        return { text: JSON.stringify({ round, ...(result.best ? { best: result.best } : {}),
          takes: result.takes.map((take) => ({ label: take.label, track: shortRef(take.track), ...(take.silent ? { silent: true } : {}), ...(take.heard ? { heard: take.heard.summary } : {}),
            ...(take.closeness ? { score: take.closeness.score, gaps: take.closeness.gaps, features: Object.fromEntries(take.closeness.features.map((feature) => [feature.name, feature.similarity])),
              ...(take.closeness.structural ? { knobsCantCloseThis: `${take.closeness.structural.gap}: ${take.closeness.structural.move}` } : {}) } : {}) })),
          ...(result.reference ? { reference: result.reference.summary } : {}), seconds: result.seconds, ...(result.notes.length ? { notes: result.notes } : {}) }), isError: result.takes.every((take) => take.silent || !take.heard) };
      } }] : [];
    // Kumi's Live extension renders an audio track's own clips offline: a file at once, Live untouched.
    const renders: KernelTool[] = supported({ since: FULL_CONTROL_BRIDGE }) && tools!.has("live_render_offline") ? [{ name: RENDER_TOOL, description: RENDER_DESCRIPTION, inputSchema: RENDER_SCHEMA,
      execute: async (input, signal) => {
        const from = typeof input.from_beat === "number" && Number.isFinite(input.from_beat) && input.from_beat >= 0 ? input.from_beat : undefined;
        const beats = typeof input.beats === "number" && Number.isFinite(input.beats) && input.beats > 0 ? input.beats : undefined;
        if (typeof input.track !== "string" || from === undefined || beats === undefined) return { text: "Give the track (an audio track's reference from this turn), from_beat and beats.", isError: true };
        const trackRef = lengthen(input.track, "trackRef") as string;
        try { requireFreshReferences({ trackRef }); } catch (error) { return { text: error instanceof Error ? error.message : "track must come from discovery in this turn", isError: true }; }
        const name = knownTrack(trackRef)?.name;
        const result = await tools!.call("live_render_offline", { trackRef, fromBeat: from, toBeat: from + beats, ...(name ? { expectedName: name } : {}) }, AbortSignal.any([signal, lifetime.signal]), { host: true });
        if (result.isError) return { text: resultText(result), isError: true };
        const rendered = payload(result);
        return { text: JSON.stringify({ file: rendered.path, seconds: rendered.seconds, channels: rendered.channels, sampleRate: rendered.sampleRate,
          note: "The track's own clips, before its devices. Hear it with listen (file), against a reference with compare_to." }), isError: false };
      } }] : [];
    // Live's own undo, for what the producer did in Live; Kumi's changes undo exactly through undo_change.
    const liveUndo: KernelTool[] = supported({ since: FULL_CONTROL_BRIDGE }) && tools!.has("live_song_undo") && tools!.has("live_song_redo") ? [{ name: "undo_in_live",
      description: "Live's own undo (or redo, with redo: true), once, exactly like Cmd-Z in Live: for something the producer did in Live themselves, or a change of Kumi's that undo_change can't take back (a deletion, a crop). For Kumi's other changes use undo_change, which undoes exactly that change; this undoes whatever Live did last.",
      inputSchema: { type: "object", additionalProperties: false, properties: { redo: { type: "boolean", description: "Live's redo instead" } } },
      execute: async (input, signal) => {
        const redo = input.redo === true;
        const result = await tools!.call(redo ? "live_song_redo" : "live_song_undo", { confirmation: redo ? "redo-in-live" : "undo-in-live", idempotencyKey: randomUUID() }, AbortSignal.any([signal, lifetime.signal]), { host: true });
        if (result.isError) return { text: resultText(result), isError: true };
        const done = payload(result);
        try { options.onAction?.({ title: done.done === true ? (redo ? "Redid in Live" : "Undid in Live") : redo ? "Nothing to redo in Live" : "Nothing to undo in Live" }); } catch { /* a listener failure must not affect Live */ }
        return { text: JSON.stringify(done), isError: false };
      } }] : [];
    const python: KernelTool[] = supported({ since: PYTHON_BRIDGE }) && tools!.has("live_run_python") ? [{ name: "run_python",
      description: "Run Python inside Live for anything your typed tools don't cover; use those first. Explore Live's API with dir(). mode eval returns an expression; exec (default) returns whatever you assign to result. Names: Live, song, app, obj (optional ref), bridge. Returns JSON {ok, result, stdout, error}, with Live objects as usable {ref, type, name} and errors as type, message and traceback. Each run opens one step in Live's undo or joins an already open step; undo_in_live takes it back. No HISTORY entry or undo_change. Set reads and old references are discarded after every run, including failures. timeoutMs defaults to 5000 (1–30000), checked by Python tracing; native calls finish before the deadline can be checked.",
      inputSchema: { type: "object", additionalProperties: false, properties: {
        code: { type: "string", minLength: 1, maxLength: 65536 }, mode: { type: "string", enum: ["eval", "exec"] },
        ref: { type: "string", minLength: 1, maxLength: 256 }, timeoutMs: { type: "integer", minimum: 1, maximum: 30000 },
      }, required: ["code"] },
      execute: async (input, signal) => {
        const args = lengthen(input) as JsonObject;
        try { requireFreshReferences(args); } catch (error) { return { text: error instanceof Error ? error.message : "Use a current Live reference", isError: true }; }
        let result: CallToolResult;
        try {
          result = await tools!.call("live_run_python", args, AbortSignal.any([signal, lifetime.signal]), { host: true });
        } finally {
          // A script can move or change anything, even before failing. Retire reads and short names.
          refs.clear(); known.clear(); cursors.clear(); shortRefs.clear(); longRefs.clear(); observationGeneration++;
        }
        if (result.isError) return { text: resultText(result), isError: true };
        const done = payload(result);
        const register = (value: unknown): void => {
          if (Array.isArray(value)) { for (const item of value) register(item); }
          else if (value && typeof value === "object") {
            const row = value as JsonObject;
            if (typeof row.ref === "string" && LIVE_REF.test(row.ref) && row.ref.length <= 256 && typeof row.type === "string") {
              const kind = /^\d+:([a-z_]+):/.exec(row.ref)![1]!;
              refs.set(row.ref, kind === "clip" ? "session-clip" : kind.replace(/_/g, "-"));
              if (kind === "track" && typeof row.name === "string") known.set(row.ref, { name: row.name.slice(0, 256) });
            }
            for (const child of Object.values(row)) register(child);
          }
        };
        register(done.result);
        return { text: JSON.stringify(shorten(done)), isError: done.ok !== true };
      } }] : [];
    // An arrangement from the producer's clips, as one change: offered where clips can be copied into the Arrangement.
    const arranging: KernelTool[] = tools!.has("live_undo") && tools!.has("live_clip_duplicate_preview") && tools!.has("live_clip_duplicate_apply") ? [{ name: ARRANGE_TOOL, description: ARRANGE_DESCRIPTION, inputSchema: ARRANGE_SCHEMA,
      execute: (input, signal) => arrange(input, arrangeHost(), AbortSignal.any([signal, lifetime.signal])) }] : [];
    // Live's own menus and keys, where Kumi has hands (macOS, Windows) and the bridge can select things first.
    const commands: KernelTool[] = options.hands !== false && (options.hands || process.platform === "darwin" || process.platform === "win32") && tools!.has("live_selection_preview")
      ? [{ name: LIVE_COMMAND_TOOL, description: LIVE_COMMAND_DESCRIPTION, inputSchema: LIVE_COMMAND_SCHEMA, execute: (input, signal) => liveCommand(input, signal) }] : [];
    // Plug-ins: Kumi's knowledge of them, set against their real parameters, and wavetables for their oscillators.
    const plugins: KernelTool[] = tools!.has("live_device_read") ? [{ name: PLUGIN_TOOL, description: PLUGIN_DESCRIPTION, inputSchema: PLUGIN_SCHEMA, execute: (input, signal) => pluginTool(input, signal) }] : [];
    return [...reads, sampleSearch, ...devices, ...edits, ...actions, ...batch, ...arranging, ...undo, ...watcher, ...auditions, ...renders, ...liveUndo, ...python, ...commands, ...plugins];
  }
  return {
    async start(signal) {
      if (closed || started) throw new ObservationError("Integration cannot be started again");
      started = true; options.onConnection("connecting");
      const combined = AbortSignal.any([signal, lifetime.signal]);
      try {
        if (!options.connect && !options.bridgeConfig) throw new ObservationError("Bridge configuration is required; choose explicit inference-only mode otherwise");
        const fresh = await openEndpoint(combined);
        if (combined.aborted || closed) { await fresh.close(); combined.throwIfAborted(); throw new ObservationError("Connection closed"); }
        attach(fresh);
        available = true;
      } catch {
        options.onConnection("error");
        // The usual cause after updating Kumi: Live's Remote Script is from an older bridge than Kumi's.
        throw new KumiError("live", `Kumi couldn't start its bridge to Live. After updating Kumi, Live's part needs updating too: quit Live, then run ${KUMI} bridge. Otherwise: ${KUMI} doctor`);
      }
    },
    stopLive: (signal) => stopEverything(AbortSignal.any([signal, lifetime.signal])),
    /** A track's devices, racks' chains and what's in them, for FOCUS. */
    deviceTree: (trackRef, signal) => readDeviceTree(trackRef, AbortSignal.any([signal, lifetime.signal])),
    /** A track's Session slots around a scene (seven), and their clips' names read together, for FOCUS. */
    async sessionStrip(trackRef, scene, originalSignal) {
      const signal = AbortSignal.any([originalSignal, lifetime.signal]);
      if (!available || lost || !tools?.has("live_discover") || !/^\d+:track:\d+$/.test(trackRef)) return undefined;
      try {
        const read = await pages({ kind: "clip-slot", parent: trackRef, fields: ["sceneIndex", "clipRef", "playingStatus"], limit: pageLimit(), budget: wholeBudget() }, signal);
        if (read.isError) return undefined;
        const rows = (payload(read).items as JsonObject[] | undefined) ?? [];
        const start = Math.max(0, Math.min(rows.length - 7, scene - 3));
        const window = rows.slice(start, start + 7);
        const clips = await Promise.all(window.map(async (row) => {
          if (typeof row.clipRef !== "string" || typeof row.ref !== "string") return undefined;
          const found = await tools!.call("live_discover", { kind: "session-clip", parent: row.ref, fields: ["name", "isAudio"], limit: 1 }, signal, { host: true }).catch(() => undefined);
          const clip = found && !found.isError ? (payload(found).items as JsonObject[] | undefined)?.[0] : undefined;
          return clip ? { name: typeof clip.name === "string" ? clip.name.slice(0, 256) : "", audio: clip.isAudio === true } : { name: "", audio: false };
        }));
        return { trackRef, scene, slots: window.map((row, index) => ({ index: typeof row.sceneIndex === "number" ? row.sceneIndex : start + index,
          ...(clips[index] ? { clip: clips[index]! } : {}), ...(row.playingStatus === 1 ? { playing: true } : {}), ...(row.playingStatus === 2 ? { queued: true } : {}) })) };
      } catch { signal.throwIfAborted(); return undefined; }
    },
    /** The Arrangement at a glance: its length, the playhead, the loop and the locators, read together. */
    async arrangementStrip(originalSignal) {
      const signal = AbortSignal.any([originalSignal, lifetime.signal]);
      if (!available || lost || !tools?.has("live_discover")) return undefined;
      try {
        const [set, locators, song] = await Promise.all([
          tools.call("live_discover", { kind: "set", fields: ["position", "loop", "playing"], limit: 1 }, signal, { host: true }),
          pages({ kind: "locator", fields: ["name", "position"], limit: pageLimit(), budget: wholeBudget() }, signal),
          tools.has("live_song_state") ? tools.call("live_song_state", {}, signal, { host: true }) : Promise.resolve(undefined)]);
        const row = set.isError ? undefined : (payload(set).items as JsonObject[] | undefined)?.[0];
        if (!row || typeof row.position !== "number") return undefined;
        const loop = row.loop && typeof row.loop === "object" ? row.loop as JsonObject : undefined;
        const length = song && !song.isError ? payload(song).songLength : undefined;
        const marks = locators.isError ? [] : ((payload(locators).items as JsonObject[] | undefined) ?? []).flatMap((item) => (typeof item.position === "number" ? [{ name: typeof item.name === "string" ? item.name.slice(0, 128) : "", position: item.position }] : []));
        return { length: typeof length === "number" && length > 0 ? length : Math.max(row.position, ...marks.map((mark) => mark.position), 16), position: row.position, playing: row.playing === true,
          // The bridge leaves out a loop start of 0.
          ...(loop && typeof loop.length === "number" ? { loop: { start: typeof loop.start === "number" ? loop.start : 0, length: loop.length, enabled: loop.enabled === true } } : {}), locators: marks };
      } catch { signal.throwIfAborted(); return undefined; }
    },
    /** A Session slot's MIDI clip, its notes and which are selected, for FOCUS; the model's references aren't touched. */
    async clipView(slotRef, originalSignal) {
      const signal = AbortSignal.any([originalSignal, lifetime.signal]);
      if (!available || lost || !tools?.has("live_discover") || !/^\d+:clip_slot:\d+:\d+$/.test(slotRef)) return undefined;
      const clipRef = slotRef.replace(":clip_slot:", ":clip:");
      try {
        const clips = await tools.call("live_discover", { kind: "session-clip", parent: slotRef, fields: ["name", "length", "isAudio"], limit: 1 }, signal, { host: true });
        const clip = clips.isError ? undefined : (payload(clips).items as JsonObject[] | undefined)?.[0];
        if (!clip || clip.isAudio === true || typeof clip.length !== "number" || !(clip.length > 0)) return undefined;
        const notes: JsonObject[] = []; let cursor: string | undefined;
        for (let page = 0; page < 10_000 && notes.length < 1_000_000; page++) {
          const read = await tools.call("live_discover", { kind: "note", parent: clipRef, limit: pageLimit(), ...(cursor ? { cursor } : {}) }, signal, { host: true });
          if (read.isError) break;
          const body = payload(read);
          notes.push(...(Array.isArray(body.items) ? body.items as JsonObject[] : []));
          cursor = typeof body.nextCursor === "string" ? body.nextCursor : undefined;
          if (!cursor) break;
        }
        // Which are selected in Live's editor (by id), when the bridge can say.
        let selected = new Set<number>();
        if (tools.has("live_note_read")) {
          const read = await tools.call("live_note_read", { clipRef, selected: true }, signal, { host: true }).catch(() => undefined);
          const ids = read && !read.isError ? (payload(read).notes as JsonObject[] | undefined)?.map((note) => note.id) : undefined;
          selected = new Set((ids ?? []).filter((id): id is number => typeof id === "number"));
        }
        const number = (value: unknown) => (typeof value === "number" && Number.isFinite(value) ? value : undefined);
        return { slotRef, name: typeof clip.name === "string" ? clip.name.slice(0, 256) : "", length: clip.length,
          notes: notes.slice(0, 512).flatMap((note) => {
            const pitch = number(note.pitch); const start = number(note.start); const duration = number(note.duration);
            if (pitch === undefined || start === undefined || duration === undefined) return [];
            return [{ pitch, start, duration, velocity: number(note.velocity) ?? 100, ...(typeof note.id === "number" && selected.has(note.id) ? { selected: true } : {}) }];
          }) };
      } catch { signal.throwIfAborted(); return undefined; }
    },
    audioFile: (named, signal) => clipFile(named, signal),
    audition: (request, signal) => audition(request, signal),
    hear: (request, signal) => hearInSet(request, signal),
    goal: (request, signal) => openGoal(request, signal),
    async observe(originalSignal, hints) {
      const signal = AbortSignal.any([originalSignal, lifetime.signal]);
      signal.throwIfAborted();
      if (!started || closed) throw new ObservationError("Integration is not open");
      invalidate(); const lease = observationGeneration; changesThisTurn = 0; picked.clear();
      // A match run's next round is the same answer: its rounds count on.
      if (!hints?.continuing) rounds = { count: 0, best: undefined };
      // While Live is away the conversation stays with its Set (and keeps being saved there).
      const away = () => noAccess(previous?.key ?? `${generation}:no-live`, now(), previous?.path && previous.project ? previous.project : undefined);
      if (!available || lost) return away();
      try {
        await tools!.refresh(signal); assertLease(lease, signal);
        await ensureCatalog(signal); assertLease(lease, signal);
        if (!tools!.has("live_status")) throw new ObservationError("Live status capability is unavailable");
        void subscribe(signal);
        // Live answers the requests waiting at each display tick (about every 100 ms) together, one
        // after another on its own thread. Sent together, the reads below arrive in one tick instead
        // of one tick each, and the Set can't change between them; each carries the epoch, checked
        // against the status's.
        const setArgs = discoveryArgs({ kind: "set", fields: [...FIELDS.set!, "filePath"] });
        const trackArgs = discoveryArgs({ kind: "track", fields: ["name", "kind", "mediaKind", "groupTrackRef"], limit: pageLimit(), budget: wholeBudget() });
        const deviceArgs = discoveryArgs({ kind: "device", fields: ["parentRef", "name", "className", "chainList"], limit: pageLimit(), budget: wholeBudget() });
        const settle = <T>(work: Promise<T>): Promise<{ value: T } | { error: unknown }> => work.then((value) => ({ value }), (error: unknown) => ({ error }));
        const song = tools!.has("live_song_state") ? settle(tools!.call("live_song_state", {}, signal, { host: true }).then(payload)) : Promise.resolve(undefined);
        const discover = tools!.has("live_discover");
        // The Set's tracks and devices are read whole, page after page; the Set and the selection are one row.
        const read = (args: JsonObject) => settle(!discover ? Promise.reject(new ObservationError("Required Set discovery capability is unavailable"))
          : args.kind === "track" || args.kind === "device" ? pages(args, signal) : tools!.call("live_discover", args, signal, { host: true }));
        // And what's selected in Live, so "this track" means something.
        const selectionArgs = discoveryArgs({ kind: "selection", limit: 1 });
        const [statusRead, setRead, tracksRead, devicesRead, selectionRead] = await Promise.all([
          settle(tools!.call("live_status", {}, signal, { host: true }).then(statusPayload)), read(setArgs), read(trackArgs), read(deviceArgs), read(selectionArgs)]);
        const songRead = await song;
        assertLease(lease, signal);
        if ("error" in statusRead) throw statusRead.error;
        const status = statusRead.value;
        if (!status.connected) { loseLive(); return away(); }
        if (!discover) throw new ObservationError("Required Set discovery capability is unavailable");
        if ("error" in setRead) throw setRead.error;
        const epoch = status.epoch as number;
        assertEpoch(payload(setRead.value).epoch, epoch);
        const page = discoveryPayload(setRead.value, "set", epoch);
        if (page.items.length !== 1) throw new ObservationError("Current Set discovery did not return one authoritative Set");
        const row = page.items[0]!;
        const identity = setIdentity(row);
        currentEpoch = epoch; currentSet = identity; lastEpoch = epoch;
        currentTempo = typeof row.tempo === "number" ? row.tempo : undefined;
        // The time signature, for bars in titles and the model's arithmetic (a read that fails leaves 4/4).
        const songState = songRead && "value" in songRead ? songRead.value : undefined;
        const numerator = typeof songState?.signatureNumerator === "number" ? songState.signatureNumerator : 4;
        const denominator = typeof songState?.signatureDenominator === "number" ? songState.signatureDenominator : 4;
        setMeter(numerator, denominator); beatsPerBar = numerator * 4 / denominator;
        registerRows("set", page.items, setArgs, page.nextCursor);
        // The Set's tracks, with references usable in this turn: most requests then need no discovery first.
        let trackList: JsonObject[] | undefined; let moreTracks = false; let moreDevices = false;
        try {
          if ("error" in tracksRead) throw tracksRead.error;
          if (!tracksRead.value.isError) {
            const trackPage = discoveryPayload(tracksRead.value, "track", epoch);
            registerRows("track", trackPage.items, trackArgs, trackPage.nextCursor);
            trackList = trackPage.items.map((item) => ({ ref: typeof item.ref === "string" ? shortRef(item.ref) : null, name: typeof item.name === "string" ? item.name.slice(0, 120) : null, type: item.kind === "group" ? "group" : item.mediaKind ?? item.kind ?? null,
              ...(typeof item.groupTrackRef === "string" ? { group: shortRef(item.groupTrackRef) } : {}) }));
            moreTracks = Boolean(trackPage.nextCursor) || trackPage.truncated === true;
            // And the devices on them, so a request about a track's sound goes straight to its parameters.
            try {
              if ("error" in devicesRead) throw devicesRead.error;
              if (!devicesRead.value.isError) {
                const devicePage = discoveryPayload(devicesRead.value, "device", epoch);
                registerRows("device", devicePage.items, deviceArgs);
                // Devices by what holds them: a track, or a rack's chain. A rack lists its chains, empty ones too,
                // each with its devices, so a request about a layer or a parallel chain goes straight to it.
                const onTrack = new Map<unknown, JsonObject[]>(); const chainRows: JsonObject[] = [];
                for (const device of devicePage.items) {
                  if (typeof device.ref !== "string") continue;
                  const name = typeof device.name === "string" ? device.name.slice(0, 120) : null;
                  const type = typeof device.className === "string" && device.className !== name ? device.className.slice(0, 64) : undefined;
                  const chains = Array.isArray(device.chainList) ? device.chainList.filter((chain): chain is JsonObject => Boolean(chain) && typeof chain === "object" && typeof (chain as JsonObject).ref === "string").slice(0, 32) : [];
                  chainRows.push(...chains);
                  const row: JsonObject = { ref: shortRef(device.ref), name, ...(type ? { type } : {}), ...(chains.length ? { chains: chains.map((chain) => ({ ref: shortRef(chain.ref as string), name: typeof chain.name === "string" ? chain.name.slice(0, 120) : null, devices: [] as JsonObject[] })) } : {}) };
                  onTrack.set(device.parentRef, [...(onTrack.get(device.parentRef) ?? []), row]);
                }
                registerRows("chain", chainRows, {});
                // Nest each chain's devices under it; what's left keyed by a track is the track's own.
                const byChain = new Map<string, JsonObject[]>();
                for (const rows of onTrack.values()) for (const row of rows) for (const chain of (row.chains as JsonObject[] | undefined) ?? []) byChain.set(chain.ref as string, chain.devices as JsonObject[]);
                for (const [parent, rows] of onTrack) { const chain = typeof parent === "string" ? byChain.get(shortRef(parent)) : undefined; if (chain) { chain.push(...rows); onTrack.delete(parent); } }
                trackList = trackList.map((entry, index) => { const devices = onTrack.get(trackPage.items[index]!.ref); return devices ? { ...entry, devices } : entry; });
                moreDevices = Boolean(devicePage.nextCursor) || devicePage.truncated === true;
              }
            } catch (error) { if (lease !== observationGeneration) throw error; }
          }
        } catch (error) { if (lease !== observationGeneration) throw error; trackList = undefined; }
        // What's selected in Live: its track (by the reference the model uses, and its name) and scene.
        let selected: JsonObject | undefined;
        try {
          if ("value" in selectionRead && !selectionRead.value.isError) {
            const row = discoveryPayload(selectionRead.value, "selection", epoch).items[0];
            const trackRef = typeof row?.selectedTrackRef === "string" ? row.selectedTrackRef : undefined;
            const track = trackRef ? known.get(trackRef) : undefined;
            if (trackRef && track) selected = { track: { ref: shortRef(trackRef), name: track.name }, note: "\"This track\" or \"here\" in the producer's words means this one, unless they pointed at something in Kumi." };
          }
        } catch { selected = undefined; }
        options.onConnection("connected");
        const restored = await restoreAfterCrash(identity, typeof row.filePath === "string" ? row.filePath : undefined, signal); assertLease(lease, signal);
        const name = typeof row.name === "string" && row.name.trim() ? row.name.slice(0, 256) : "(unnamed/unsaved)";
        // Its file says which saved Set this is (for its conversation and catching up). It's read for a
        // newly seen Set, and again when the name changes: Save As, or an unsaved Set's first save.
        const newSet = project?.identity !== identity;
        let path = newSet ? undefined : project!.path;
        if (newSet || project!.name !== name) {
          // The Set's row says where its file is (an unsaved Set has none); an older bridge is asked.
          if (Object.hasOwn(row, "filePath")) path = typeof row.filePath === "string" && row.filePath && existsSync(row.filePath) ? row.filePath : undefined;
          else { path = await projectPath(signal); assertLease(lease, signal); }
        }
        // The conversation stays with the Set: the same Set keeps its key. When Live comes back (restarted,
        // say, into a blank Set) it carries on too, unless a different saved Set is open. References
        // from before are gone either way; the model discovers again every turn.
        const otherFile = previous?.path !== undefined && path !== undefined && previous.path !== path;
        const continues = previous !== undefined && (previous.identity === identity || (reconnected && !otherFile));
        const key = continues ? previous!.key : JSON.stringify([generation, epoch, identity]);
        const afterReconnect = reconnected; reconnected = false;
        // What the producer pointed at in Kumi, if it's still there, for "this" in their message.
        const pinned = hints?.pinned ? await checkPin(hints.pinned, signal) : undefined; assertLease(lease, signal);
        project = { identity, name, ...(path ? { path } : {}) };
        previous = { key, name, identity, ...(path ? { path, project: { id: projectIdOf(path), name } } : {}) };
        if (newSet) catchUp(identity, name, afterReconnect);
        else if (Date.now() - lastSaved > 5 * 60_000) scheduleSave(1_000);
        await ensureCatalog(signal); assertLease(lease, signal);
        const provenance = typeof status.provenance === "string" ? status.provenance : "unknown";
        const source = provenance === "real-live" && status.adapter === "remote-script" ? "Remote Script · real-live" : `unverified/synthetic fixture · ${provenance}`;
        // A big Set is folded to about the same size as a small one: the tracks in focus (selected in
        // Live, pinned in Kumi, changed lately) keep their devices, every other track is one line.
        const focusRefs = new Set<string>([...(selected ? [String((selected.track as JsonObject).ref)] : []), ...(hints?.pinned ? [shortRef(hints.pinned.trackRef)] : [])]);
        // Changes name their track by name, which duplicates share: each of the last four names Kumi
        // changed brings in one track, the first with it, so the focus never grows with the Set.
        const recent = [...new Set([...changes.values()].reverse().flatMap(({ record }) => (record.track?.name ? [record.track.name] : [])))].slice(0, 4);
        for (const name of recent) { const track = trackList?.find((row) => row.name === name); if (track) focusRefs.add(String(track.ref)); }
        const shown = trackList ? foldTracks(trackList, (track) => focusRefs.has(String(track.ref))) : undefined;
        return {
          key,
          revision: String(tools!.generation),
          label: `Current open Set: ${name} — ${source}`,
          instructions: INSTRUCTIONS, tools: definitions(),
          ...(project?.identity === identity && project.path ? { project: { id: projectIdOf(project.path), name } } : {}),
          ...(trackList && !moreTracks ? { tracks: trackList.flatMap((track) => (typeof track.name === "string" ? [track.name] : [])) } : {}),
          ...(project?.identity === identity && project.path ? (() => { try { return { savedAt: statSync(project.path).mtimeMs }; } catch { return {}; } })() : {}),
          context: JSON.stringify({ observedAt: now().toISOString(), connectionGeneration: generation, epoch,
            adapter: status.adapter, provenance, liveVersion: status.environment && typeof status.environment === "object" ? object(status.environment).liveVersion ?? null : null,
            set: { ref: typeof row.ref === "string" ? shortRef(row.ref) : row.ref, name, tempo: row.tempo ?? null, timeSignature: `${numerator}/${denominator}`, playing: row.playing ?? null, position: row.position ?? null, loop: row.loop ?? null,
              ...(songState ? { recording: { session: songState.sessionRecord === true, arrangement: row.recording ?? null }, swing: songState.swingAmount ?? null } : {}) },
            ...(shown ? { tracks: shown.tracks, ...(shown.folded ? { folded: shown.folded } : {}), ...(moreTracks || shown.moreTracks ? { moreTracks: shown.moreTracks ?? "More tracks than listed; discover the rest" } : {}), ...(moreDevices ? { moreDevices: "Not every device is listed; discover a track's devices" } : {}) } : {}),
            ...(catchUpContext && project?.identity === identity ? { sinceLastTime: catchUpContext } : {}),
            ...(pinned ? { pinned } : {}),
            ...(selected ? { selectedInLive: selected } : {}),
            ...(restored ? { restoredAfterCrash: restored } : {}),
            // Live's references changed with the connection: ones from earlier answers would fail (or, renumbered, point elsewhere).
            ...(afterReconnect ? { reconnected: "Kumi reconnected to Live since your last answer, so every reference from earlier answers (track:…, device:…, clip:… and the like) is gone. Use the ones listed here, or discover again." } : {}),
            // What Kumi changed lately and where each change stands, HISTORY undos and stopped answers included.
            ...(changes.size ? { kumiChanges: [...changes.values()].filter((entry) => !entry.within).slice(-12).map(({ record }) => ({ change: record.id, what: record.title, state: record.state, ...(record.note ? { note: record.note } : {}) })) } : {}),
            truncated: page.truncated, ...(page.nextCursor ? { nextCursor: page.nextCursor } : {}),
            coverage: "Current open Set only. Bounded discovery; details and track counts require fresh paged reads. Names/paths are not durable identity.",
          }),
        };
      } catch (error) {
        if (lease !== observationGeneration) throw new ObservationError("Observation changed; late refresh discarded");
        refs.clear(); cursors.clear(); currentEpoch = undefined;
        // The cause stays with it, for logs and tests; the message is what the producer and model see.
        throw new ObservationError(error instanceof ObservationError ? error.message : "Live observation refresh failed; old observations are not current", { cause: error });
      }
    },
    async undo(id, signal) {
      if (!started || closed) throw new KumiError("request", "Kumi isn't connected to Live, so it can't undo.");
      const outcome = await undoChange(id ?? "last", signal);
      if (!outcome.record) throw new KumiError("request", outcome.text);
      return outcome.record;
    },
    close() {
      if (closing) return closing;
      closingStarted = true;
      clearTimeout(saveTimer); clearInterval(watcher);
      // Remember the Set as Kumi leaves it, so next time's catch-up starts here (bounded).
      const remembered = project?.path && options.projectStore && available && !lost
        ? Promise.race([saveNow(2_000), new Promise<void>((resolve) => { setTimeout(resolve, 2_500).unref?.(); })]) : Promise.resolve();
      closing = remembered.then(async () => {
        closed = true; available = false; lifetime.abort(); invalidate(); focusFeed?.stop(); clearTimeout(transportTimer);
        for (const remove of unlisten) remove();
        // The listening devices' socket, and what they wrote this session.
        const link = await earsSetup?.catch(() => undefined);
        await link?.close().catch(() => undefined);
        await rm(earsFolder, { recursive: true, force: true }).catch(() => undefined);
        return tools ? tools.close() : endpoint ? endpoint.close() : Promise.resolve();
      });
      return closing;
    },
  };
}
