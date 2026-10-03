/** A bridge in memory, shaped like the real one's responses, for the Ableton integration's tests. */
import assert from "node:assert/strict";
import type { CallToolResult, Tool } from "@modelcontextprotocol/sdk/types.js";
import type { AuditionEvent, ChangeRecord, ConnectionState, JsonObject, KernelTool, LiveFocus, LiveTransport, PinnedNode } from "../../src/core/contracts.js";
import type { McpEndpoint } from "../../src/mcp/client.js";
import { createAbletonIntegration } from "../../src/integrations/ableton/index.js";
import type { EarsLink } from "../../src/ears/link.js";
import type { Hands } from "../../src/hands/index.js";
import { lowDisk } from "../../src/core/disk.js";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Synthetic bridge responses shaped like the real ones recorded in .pi/kumi-evidence (previews
// return prior and proposed values, a transaction id and a confirmation; applies return a state).
/** A device on a fixture track: its knobs by name. */
export interface FixtureDevice { name: string; className: string; params: { name: string; value: number; min: number; max: number }[] }
type Options = {
  midiClips?: boolean; padBatches?: boolean; parameters?: boolean; /** 150 parameters, a page of 100 at a time, "Feedback" the last. */ manyParameters?: boolean; racks?: boolean; /** Rack tools only once a rack is loaded, with the bridge's catalog notice arriving late (as on real Live). */ lateRacks?: boolean; /** The bridge's version; "1" (older than any gate) by default. */ version?: string;
  /** Playing, recording and the emergency stop, as bridge 1.0.34 offers them. */ transport?: boolean;
  /** An audio clip (playing this file) in the first track's first slot, and a MIDI clip in the second track's Arrangement. */ audioClip?: string;
  /** The Set's saved file, which the bridge can back up (live_project_backup_*). */ savedSet?: string;
  /** Free space on the disk Live records to, in bytes (plenty when left out). */ freeDisk?: number;
  /** Renders: what recording each source track's Post FX makes (a file), for auditions; with it, Main, the playhead and the song's length are Live's. */ renders?: (source: string, devices: readonly FixtureDevice[]) => string | undefined;
  /** The Set's tempo (120 when left out). */ tempo?: number;
  /** Recording starts this many beats after it's asked for, once (a bridge whose steps outlast a render's lead-in). */ lateRecord?: number;
  /** Live's Start Playback with Record turned off: recording on while stopped doesn't start playing. */ noPlayOnRecord?: boolean;
  /** Tracks beyond the two fixtures, with their devices (goal candidates). */ extraTracks?: { name: string; devices: FixtureDevice[] }[];
  /** A big Set: this many more tracks, every fourth a group holding the three after it, each with four devices that discovery lists without a parent. */ bigSet?: number;
  /** Discovery ends every page after this many rows, with a cursor, whatever the limit (as the Remote Script ends a page when its time is up). */ pageSize?: number;
  /** The tools of bridge 1.0.58: explicit deletions, Live's undo steps, and Kumi's Live extension (offline render, Arrangement MIDI clips). */ fullControl?: boolean;
  /** The JSON reply to Python execution inside Live (bridge 1.0.68). */ python?: (args: JsonObject) => JsonObject;
  /** What's selected in Live, as the focus feed reports it. */ onFocus?: (focus: LiveFocus | null) => void;
  /** Live's transport, for the beat light. */ onTransport?: (transport: LiveTransport | null) => void;
  /** The session's own hooks, for a session over this bridge. */ onConnection?: (state: ConnectionState) => void; onAudition?: (event: AuditionEvent) => void;
  /** Where the audition keeps Main's level while it renders. */ restoreFile?: string;
  /** Kumi's listening devices: a fake link, and the Browser listing the device (live_browser_inspect). Off when left out. */ ears?: { open: () => Promise<EarsLink> };
  /** Kumi's hands (Live's menus): fake ones, with selecting and showing in Live. Off when left out. */ hands?: { open: () => Promise<Hands | undefined> };
  /** false: parameters through the bridge's preview and apply even where it runs Python (fast.ts otherwise). */ fast?: boolean };
export function bridge(options: Options = {}) {
  const requests: { name: string; args: JsonObject }[] = [];
  const records: ChangeRecord[] = [];
  let tempo = options.tempo ?? 120;
  let live = true; let epoch = 7;
  let tracks: { name: string; color: number; armed?: boolean; frozen?: boolean; input?: string; clips?: { start: number; filePath?: string }[]; made?: string; madeAt?: number; devices?: FixtureDevice[] }[] = [{ name: "Fixture Bass", color: 0xf7f47c }, { name: "Fixture Drums", color: 0x10ff00 },
    ...(options.extraTracks ?? []).map((track) => ({ name: track.name, color: 0x808080, devices: structuredClone(track.devices) })),
    ...Array.from({ length: options.bigSet ?? 0 }, (_, index) => ({ name: index % 4 === 0 ? `Bus ${index / 4 + 1}` : `Part ${index + 1}`, color: 0x808080,
      devices: ["Operator", "EQ Eight", "Compressor", "Reverb"].map((name) => ({ name, className: name.replace(/ /g, ""), params: [] })) }))];
  // Main's fader and the playhead, as auditions use them, moving as Live's does: "continue" plays on from where
  // playback last stopped and "start" from the start marker, wherever the playhead was moved while stopped; a
  // jump while playing is honoured, and one while recording ends the take there; stopping while stopped goes
  // back to the start; and recording on while stopped plays from the start marker (Start Playback with Record).
  const main = { volume: 0.85 }; let recordingFrom: number | undefined;
  const playhead = { at: 0, since: 0, stoppedAt: 0, marker: 0 };
  let lateRecord = options.lateRecord;
  /** Where the playhead is: while playing, on from where it started. */
  const now = () => transport.playing ? playhead.at + (Date.now() - playhead.since) * tempo / 60_000 : playhead.at;
  const play = (from: number) => { transport.playing = true; playhead.at = from; playhead.since = Date.now(); if (transport.arrangementRecord) recordingFrom = from; };
  let failStep: string | undefined;
  /** A take on each armed track (what its source's Post FX made), from where recording started. */
  const takes = () => {
    if (recordingFrom === undefined || !options.renders) return;
    const from = recordingFrom;
    for (const track of tracks) if (track.armed) { const source = tracks.find((item) => item.name === track.input); const filePath = track.input ? options.renders(track.input, source?.devices ?? []) : undefined; track.clips = [...(track.clips ?? []).filter((clip) => clip.start !== from), { start: from, ...(filePath ? { filePath } : {}) }]; }
    recordingFrom = undefined;
  };
  let undoRefusal: string | undefined;
  let applyFailure: "throw" | "uncertain" | "unreadable" | undefined;
  let gate: { sent: () => void; wait: Promise<void> } | undefined;
  /** Requests held until released, by tool name (the first of each). */
  const holds = new Map<string, { sent: () => void; wait: Promise<void> }>();
  const names = [...(options.midiClips ? ["live_midi_clip_preview", "live_midi_clip_apply", "live_clip_properties_preview", "live_clip_properties_apply", "live_follow_actions_preview", "live_follow_actions_apply"] : []), "server_status", "live_status", "live_discover", "live_snapshot", "live_undo", ...(options.renders ? ["live_transaction_release"] : []),
    "live_tempo_preview", "live_tempo_apply", "live_mixer_preview", "live_mixer_apply",
    "live_session_structure_preview", "live_session_structure_apply", "live_object_rename_preview", "live_object_rename_apply", "live_audio_capture_apply", "live_transport_apply",
    "live_track_properties_preview", "live_track_properties_apply", "live_device_preview", "live_device_apply", "live_drum_pad_preview", "live_drum_pad_apply",
    "live_browser_load_preview", "live_browser_load_apply", ...(options.ears ? ["live_browser_inspect"] : []), ...(options.parameters ? ["live_device_parameter_preview", "live_device_parameter_apply"] : []),
    ...(options.racks || options.lateRacks ? ["live_rack_preview", "live_rack_apply", "live_chain_mixer_preview", "live_chain_mixer_apply"] : []),
    ...(options.savedSet ? ["live_project_backup_preview", "live_project_backup_apply"] : []),
    ...(options.renders ? ["live_transport_preview", "live_song_state", "live_track_structure_preview", "live_track_structure_apply", ...(options.parameters ? [] : ["live_device_parameter_preview", "live_device_parameter_apply"])] : []),
    ...(options.transport ? ["live_transport_action_preview", "live_transport_action_apply", "live_recording_preview", "live_recording_apply", "live_session_emergency_stop", "live_routing_preview", "live_routing_apply"] : []),
    ...(options.fullControl ? ["live_clip_delete_preview", "live_clip_delete_apply", "live_track_delete_preview", "live_track_delete_apply", "live_undo_step_begin", "live_undo_step_end", "live_render_offline",
      "live_arrangement_midi_clip_preview", "live_arrangement_midi_clip_apply", "live_clip_clear_range_preview", "live_clip_clear_range_apply", "live_subscribe",
      "live_device_edit_preview", "live_device_edit_apply", "live_song_undo", "live_song_redo", "live_device_read"] : []),
    ...(options.python ? ["live_run_python"] : []),
    ...(options.hands ? ["live_selection_preview", "live_selection_apply", "live_view_preview", "live_view_apply"] : [])];
  // Live's transport: what's playing and recording, and whether its ordinary stop is refused (as 1.0.33's was while playing).
  const transport = { playing: false, sessionRecord: false, arrangementRecord: false, refuseStop: false, emergencyStops: 0, recordUnsure: false };
  // Like the bridge, drum pad tools appear once the Set has a Drum Rack.
  let drumRack = false;
  const catalog: Tool[] = names.map((name) => ({ name, description: `bridge ${name}`, inputSchema: name === "live_session_structure_preview"
    ? { type: "object", properties: { tracks: { type: "array", items: { type: "object", properties: { name: { type: "string" }, kind: { type: "string" }, index: { type: "integer", description: "request order" } } } }, scenes: { type: "array" } } }
    : name === "live_drum_pad_preview" && options.padBatches ? { type: "object", properties: { action: { type: "string", enum: ["set", "delete-all-chains", "load-sample", "load-samples"] } }, additionalProperties: true }
    : name === "live_clip_properties_preview" ? { type: "object", required: ["clipRef"], additionalProperties: false, properties: { clipRef: { type: "string" }, legato: { type: "boolean" }, grooveRef: { type: ["string", "null"] } } }
    : name === "live_device_parameter_preview" ? { type: "object", properties: { deviceRef: { type: "string" }, parameterRef: { type: "string" }, value: { type: "number" }, values: { type: "array" } }, additionalProperties: true }
    : { type: "object", properties: {}, additionalProperties: true } }));
  const wrap = (value: JsonObject): CallToolResult => ({ content: [{ type: "text", text: JSON.stringify(value) }], structuredContent: value });
  const refusal = (text: string, extra: JsonObject = {}): CallToolResult => ({ isError: true, content: [{ type: "text", text }], structuredContent: { message: text, ...extra } });
  const pending = new Map<string, { name: string; args: JsonObject }>();
  const catalogListeners = new Set<() => void>();
  const liveEventListeners = new Set<(event: JsonObject) => void>();
  let rackLoaded = false;
  let clipCreated = false;
  let clipAdvertised = false;
  let transactions = 0;
  /** Transactions the client gave up the undo of. */
  const released: string[] = [];
  const endpoint: McpEndpoint = {
    pid: null, serverInfo: { name: "kumi-synthetic-bridge", version: options.version ?? "1" }, stderrStatus: () => ({ bytes: 0, truncated: false }),
    async list() { return { tools: catalog.filter((tool) => (clipAdvertised || !tool.name.startsWith("live_clip_properties_")) && (drumRack || !tool.name.startsWith("live_drum_pad_")) && (!options.lateRacks || rackLoaded || !/^live_(rack|chain_mixer)_/.test(tool.name))) }; },
    async call(name, args, signal) {
      signal.throwIfAborted(); requests.push({ name, args: structuredClone(args) });
      const holding = holds.get(name);
      if (holding) { holds.delete(name); holding.sent(); await holding.wait; }
      if (failStep && (name === failStep || (name === "live_transport_action_preview" && args.action === failStep))) { failStep = undefined; return refusal("adapter request failed"); }
      if (name === "live_song_state") return wrap({ songLength: 64, signatureNumerator: 4, signatureDenominator: 4 });
      if (name === "live_status") { clipAdvertised = clipCreated; return wrap({ connected: live, adapter: "remote-script", provenance: "fake-live", epoch: live ? epoch : null }); }
      if (name === "live_snapshot") return wrap({ epoch, snapshot: { set: { ref: "7:set:song", objectIdentity: "song", name: "Fixture Set" },
        playback: { transport: { playing: transport.playing, sessionRecord: transport.sessionRecord, arrangementRecord: transport.arrangementRecord }, firedTargets: [],
          playingTargets: transport.playing ? [{ trackRef: "7:track:0", clipSlotRef: "7:clip_slot:0:0", sceneRef: "7:scene:0" }] : [] } } });
      if (name === "live_session_emergency_stop") {
        const expected = transport.sessionRecord && transport.arrangementRecord ? "both" : transport.sessionRecord ? "session" : transport.arrangementRecord ? "arrangement" : "stopped";
        if (args.confirmation !== "emergency-stop" || args.expectedRecording !== expected) return refusal("expected recording mode does not match fresh authoritative playback");
        transport.playing = false; transport.sessionRecord = false; transport.arrangementRecord = false; transport.emergencyStops++;
        return wrap({ stopped: true, stoppedTargets: args.expectedTargets ?? [], recordingStopped: expected !== "stopped" });
      }
      if (name === "live_discover" && args.kind === "session-playback") return wrap({ epoch, kind: args.kind, revision: "r1", truncated: false, items: [{ ref: "7:session_playback:0",
        transport: { playing: transport.playing, sessionRecord: transport.sessionRecord, arrangementRecord: transport.arrangementRecord }, firedTargets: [],
        playingTargets: transport.playing ? [{ trackRef: "7:track:0", clipSlotRef: "7:clip_slot:0:0", sceneRef: "7:scene:0" }] : [] }] });
      if (name === "live_discover" && args.kind === "parameter" && options.manyParameters) {
        const rows = Array.from({ length: 150 }, (_, index) => ({ ref: `7:parameter:${index}`, parentRef: "7:device:0:0", name: index === 149 ? "Feedback" : `Knob ${index}`, value: 0, min: 0, max: 1 }));
        const from = typeof args.cursor === "string" ? Number(args.cursor.slice(5)) : 0; const limit = typeof args.limit === "number" ? args.limit : 100;
        const next = from + limit < rows.length ? `page:${from + limit}` : undefined;
        return wrap({ epoch: 7, kind: args.kind, items: rows.slice(from, from + limit), revision: "r1", truncated: Boolean(next), ...(next ? { nextCursor: next } : {}) });
      }
      if (name === "live_discover") {
        const set = { ref: "7:set:song", objectIdentity: "song", name: "Fixture Set", tempo, position: now(), playing: transport.playing, ...(options.savedSet ? { filePath: options.savedSet } : {}) };
        const items = args.kind === "set" ? [set] : args.kind === "track"
          ? tracks.map((track, index) => {
            // In a big Set, every fourth track (from the third) is a group holding the three after it.
            const big = index - 2; const group = options.bigSet && big >= 0 ? (big % 4 === 0 ? undefined : `7:track:${index - (big % 4)}`) : undefined;
            return { ref: `7:track:${index}`, parentRef: set.ref, name: track.name, color: track.color, armed: track.armed === true, isFrozen: track.frozen === true, ...(options.bigSet && big >= 0 && big % 4 === 0 ? { kind: "group" } : {}), ...(group ? { groupTrackRef: group } : {}) };
          })
          : args.kind === "selection" ? [{ ref: "7:selection:0", selectedTrackRef: "7:track:0" }]
          : args.kind === "main-track" ? [{ ref: "7:main_track:0", parentRef: set.ref, name: "Main", kind: "main", mixer: { volume: main.volume } }]
          : args.kind === "device" && options.renders && typeof args.parent === "string" ? (tracks[Number(args.parent.split(":").at(-1))]?.devices ?? []).map((device, index) => ({ ref: `${args.parent as string}:d${index}`.replace(":track:", ":device:"), parentRef: args.parent, name: device.name, className: device.className }))
          : args.kind === "parameter" && options.renders && typeof args.parent === "string" && /:device:\d+:d\d+$/.test(args.parent) ? (() => {
            const [, t, d] = /:device:(\d+):d(\d+)$/.exec(args.parent as string)!;
            return (tracks[Number(t)]?.devices?.[Number(d)]?.params ?? []).map((param, index) => ({ ref: `7:parameter:${t}:${d}:${index}`, parentRef: args.parent, name: param.name, value: param.value, min: param.min, max: param.max }));
          })()
          : args.kind === "arrangement-clip" && options.renders ? tracks.flatMap((track, index) => (track.clips ?? []).map((clip, at) => ({ ref: `7:arrangement_clip:${index}:${at}`, parentRef: `7:track:${index}`, name: "Take", isAudio: true, start: clip.start, length: 16, filePath: clip.filePath ?? null })))
          : args.kind === "device" && options.bigSet && args.parent === undefined ? tracks.flatMap((track, index) => (track.devices ?? []).map((device, at) => ({ ref: `7:device:${index}:${at}`, parentRef: `7:track:${index}`, name: device.name, className: device.className })))
          : args.kind === "device" && options.racks ? [
            { ref: "7:device:0:0", parentRef: "7:track:0", name: "Instrument Rack", className: "InstrumentGroupDevice", chainList: [{ ref: "7:chain:0:0:0", name: "Keys" }, { ref: "7:chain:0:0:1", name: "Pad" }] },
            { ref: "7:device:0:0:0:0", parentRef: "7:chain:0:0:0", name: "Operator", className: "Operator" },
            { ref: "7:device:0:1", parentRef: "7:track:0", name: "Reverb", className: "Reverb" }]
          : args.kind === "device" && options.parameters ? [{ ref: "7:device:0:0", parentRef: "7:track:0", name: "Operator", className: "Operator" }]
          : args.kind === "parameter" && options.parameters ? ["Osc-A Level", "Filter Freq", "Ae Release"].map((name, index) => ({ ref: `7:parameter:${index}`, parentRef: "7:device:0:0", name, value: 0, min: 0, max: 1 }))
          : args.kind === "clip-slot" && options.audioClip ? [{ ref: "7:clip_slot:0:0", parentRef: "7:track:0", sceneIndex: 0, clipRef: "7:clip:0:0" }, { ref: "7:clip_slot:0:1", parentRef: "7:track:0", sceneIndex: 1, clipRef: null }]
          : args.kind === "session-clip" && options.audioClip ? [{ ref: "7:clip:0:0", parentRef: "7:clip_slot:0:0", name: "Bounce", isAudio: true, filePath: options.audioClip }]
          : args.kind === "arrangement-clip" && options.audioClip ? [{ ref: "7:arrangement_clip:1:0", parentRef: "7:track:1", name: "Beat", isAudio: false, filePath: null }] : [];
        // Like the bridge, a parent narrows the rows to those it holds.
        const all = args.parent === undefined ? items : (items as JsonObject[]).filter((item) => item.parentRef === args.parent);
        if (options.pageSize && args.kind !== "set" && args.kind !== "selection") {
          const from = typeof args.cursor === "string" ? Number(args.cursor.replace("early:", "")) : 0;
          const next = from + options.pageSize < all.length ? `early:${from + options.pageSize}` : undefined;
          return wrap({ epoch: 7, kind: args.kind, items: all.slice(from, from + options.pageSize), revision: "r1", truncated: Boolean(next), ...(next ? { nextCursor: next } : {}) });
        }
        return wrap({ epoch: 7, kind: args.kind, items: all, revision: "r1", truncated: false });
      }
      if (name === "live_subscribe") return wrap({ subscribed: true, types: args.types ?? [] });
      if (name === "live_song_undo" || name === "live_song_redo") return wrap({ done: true, canUndo: name === "live_song_redo", canRedo: name === "live_song_undo" });
      if (name === "live_run_python" && options.python) return wrap(options.python(args));
      if (name === "live_device_read") return wrap({ names: ["Cutoff", "Resonance", "Drive"], total: 3 });
      if (name === "live_undo_step_begin") return wrap({ open: true, stepId: `undo-step-${transactions}`, expiresAt: now() + 600_000, closedPrevious: false });
      if (name === "live_undo_step_end") return wrap({ closed: true, stepId: args.stepId ?? null, reason: "ended" });
      if (name === "live_render_offline") return wrap({ path: join(tmpdir(), "kumi-fixture-render.wav"), format: "wav", channels: 2, sampleRate: 44100, bitDepth: 24, seconds: (Number(args.toBeat) - Number(args.fromBeat)) * 60 / tempo, bytes: 1, renderMs: 12, trackRef: args.trackRef, fromBeat: args.fromBeat, toBeat: args.toBeat });
      if (name.endsWith("_preview")) {
        const id = `tx${++transactions}`;
        pending.set(id, { name, args });
        const base = { transactionId: id, epoch: 7, confirmation: name === "live_mixer_preview" ? "secret-confirmation-token-0123456789" : "apply" };
        if (name === "live_project_backup_preview") return wrap({ ...base, path: options.savedSet, allowedRoot: args.allowedRoot, impact: "creates-verified-backup" });
        if (name === "live_tempo_preview") return wrap({ ...base, priorTempo: tempo, proposedTempo: args.tempo });
        if (name === "live_transport_preview") return wrap({ ...base, prior: { position: now(), loop: { enabled: false } }, proposed: { position: args.position, loopEnabled: args.loopEnabled } });
        if (name === "live_mixer_preview") return wrap({ ...base, trackRef: args.trackRef, prior: { volume: args.trackRef === "7:main_track:0" ? main.volume : 0.85, pan: 0 }, ...(args.volume === 0.4 ? { priorDisplay: { volume: "0.0 dB", pan: "C" } } : {}), proposed: { volume: args.volume, pan: args.pan } });
        if (name === "live_object_rename_preview") return wrap({ ...base, target: { kind: args.kind, ref: args.ref, currentName: tracks[Number(String(args.ref).split(":").at(-1))]?.name }, proposedName: args.name });
        if (name === "live_track_properties_preview") return wrap({ ...base, ref: args.ref, prior: { colorIndex: 4 }, proposed: { colorIndex: args.colorIndex } });
        if (name === "live_device_preview") return wrap({ ...base, action: args.action, payload: { trackRef: args.trackRef, deviceName: args.deviceName }, sample: { path: args.filePath, size: 18 } });
        if (name === "live_drum_pad_preview" && args.action === "load-samples") return wrap({ ...base, action: args.action, deviceRef: args.deviceRef, pads: (args.pads as JsonObject[]).map((pad) => ({ padRef: `7:drum_pad:0:0:${String(pad.note)}`, note: pad.note, sample: { path: pad.filePath } })) });
        if (name === "live_drum_pad_preview") return wrap({ ...base, action: args.action, padRef: `7:drum_pad:0:0:${String(args.note)}`, note: args.note, sample: { path: args.filePath } });
        if (name === "live_browser_load_preview") return wrap({ ...base, trackRef: args.trackRef ?? "7:track:0", item: { name: String(args.itemId).split("/").at(-1) }, ...(args.chainRef ? { chainRef: args.chainRef, chainName: "Keys", rackName: "Instrument Rack" } : {}) });
        if (name === "live_rack_preview") return wrap({ ...base, action: args.action, rackRef: args.rackRef, rackName: "Instrument Rack", prior: args.action === "add-macro" ? { visibleMacroCount: 8 } : {}, impact: args.action === "insert-chain" ? "momentary-rack-action-no-undo" : "edits-rack" });
        if (name === "live_chain_mixer_preview") return wrap({ ...base, chainRef: args.chainRef, chainName: "Pad", rackName: "Instrument Rack", prior: { volume: 0.85, pan: 0 }, proposed: { volume: args.volume, pan: args.pan } });
        if (name === "live_track_delete_preview") {
          // As the bridge has it: a group goes with the tracks in it, which the track it names lists.
          const index = Number(String(args.trackRef).split(":").at(-1)); const big = index - 2;
          const inside = options.bigSet && big >= 0 && big % 4 === 0 ? tracks.slice(index + 1, index + 4).map((track) => track.name) : [];
          return wrap({ ...base, track: { ref: args.trackRef, name: tracks[index]?.name, kind: inside.length ? "group" : "regular", ...(inside.length ? { alsoDeletes: inside } : {}) }, impact: "deletes-track-no-undo" });
        }
        if (name === "live_device_parameter_preview") {
          const device = { ref: args.deviceRef, name: "Operator", trackRef: "7:track:0" };
          // Live's own checks: a finite value, within the parameter's range (these are 0 to 1).
          const given = Array.isArray(args.values) ? (args.values as JsonObject[]).map((item) => item.value) : [args.value];
          if (given.some((value) => typeof value !== "number" || !Number.isFinite(value))) return refusal("The bridge rejected the arguments: deviceRef, parameterRef, and finite value are required");
          if (given.some((value) => (value as number) < 0 || (value as number) > 1)) return refusal(JSON.stringify({ reason: "parameter value is outside authoritative bounds", remediation: "Parameter preview failed without mutation" }));
          const row = (parameterRef: unknown, value: unknown) => ({ ref: parameterRef, name: ["Osc-A Level", "Filter Freq", "Ae Release"][Number(String(parameterRef).split(":").at(-1))], currentValue: 0, proposedValue: value, min: 0, max: 1 });
          return wrap(Array.isArray(args.values) ? { ...base, device, parameters: (args.values as JsonObject[]).map((item) => row(item.parameterRef, item.value)) } : { ...base, device, parameter: row(args.parameterRef, args.value) });
        }
        const proposed = [...(Array.isArray(args.tracks) ? args.tracks as JsonObject[] : []).map((item) => ({ kind: "track", name: item.name, trackKind: item.kind, index: item.index ?? 0 }))];
        return wrap({ ...base, prior: { tracks: tracks.map((track, index) => ({ ref: `7:track:${index}`, name: track.name, index })), scenes: [] }, proposed });
      }
      if (name.endsWith("_apply")) {
        const transaction = pending.get(String(args.transactionId));
        assert(transaction, "apply names a previewed transaction");
        if (gate) { const held = gate; gate = undefined; held.sent(); await held.wait; }
        if (applyFailure === "throw") throw new Error("socket closed");
        if (applyFailure === "uncertain") return refusal("Apply is uncertain; perform fresh discovery.", { state: "uncertain" });
        if (applyFailure === "unreadable") return { content: [{ type: "text", text: "not json" }] };
        if (transaction.name === "live_tempo_preview") tempo = Number(transaction.args.tempo);
        if (transaction.name === "live_project_backup_preview") return wrap({ transactionId: args.transactionId, state: "applied", backup: String(options.savedSet).replace(/\.als$/, `.backup-${transactions}.als`), verified: true });
        if (transaction.name === "live_transport_action_preview") {
          if (transaction.args.action === "stop" && transport.refuseStop && transport.playing) return refusal("request failed: missing, expired, stale, or mismatched mutation preflight");
          // Like Live: playing with recording on records from where it starts; stopping writes the takes.
          if (transaction.args.action === "start" && !transport.playing) play(playhead.marker);
          if (transaction.args.action === "continue" && !transport.playing) play(playhead.stoppedAt);
          // And, as on real Live, stopping ends the recording: the next pass records only once it's started again.
          if (transaction.args.action === "stop" && transport.playing) { playhead.at = now(); playhead.stoppedAt = playhead.at; transport.playing = false; takes(); transport.arrangementRecord = false; }
          else if (transaction.args.action === "stop") { playhead.at = 0; playhead.stoppedAt = 0; playhead.marker = 0; }
          return wrap({ transactionId: args.transactionId, state: "applied", done: transaction.args.action });
        }
        if (transaction.name === "live_transport_preview" && typeof transaction.args.position === "number") {
          // A jump while recording ends the take where the playhead was, and Live records nothing after it.
          if (transport.playing && recordingFrom !== undefined) { takes(); transport.arrangementRecord = false; }
          playhead.at = transaction.args.position; playhead.since = Date.now();
        }
        if (transaction.name === "live_mixer_preview" && transaction.args.trackRef === "7:main_track:0" && typeof transaction.args.volume === "number") main.volume = transaction.args.volume;
        if (transaction.name === "live_routing_preview" && (typeof transaction.args.arm === "boolean" || typeof transaction.args.inputType === "string")) {
          const index = Number(String(transaction.args.trackRef).split(":").at(-1));
          if (tracks[index]) tracks[index] = { ...tracks[index]!, ...(typeof transaction.args.arm === "boolean" ? { armed: transaction.args.arm } : {}), ...(typeof transaction.args.inputType === "string" ? { input: transaction.args.inputType } : {}) };
          return wrap({ transactionId: args.transactionId, state: "applied" });
        }
        if (transaction.name === "live_recording_preview") {
          // Like the bridge: exactly the destination (and the tracks named alongside it) armed.
          const recorded = [transaction.args.destinationTrackRef, ...(Array.isArray(transaction.args.alsoTrackRefs) ? transaction.args.alsoTrackRefs : [])];
          if (transaction.args.action === "start" && tracks.some((track, index) => track.armed && !recorded.includes(`7:track:${index}`))) return refusal("adapter request failed");
          const on = transaction.args.action === "start";
          if (transaction.args.lane === "arrangement" && on) {
            transport.arrangementRecord = true;
            if (!transport.playing && !options.noPlayOnRecord) play(playhead.marker);
            // On the beat it started on: the fixture's renders don't move with the take, so its start stays put.
            else if (transport.playing) { recordingFrom = Math.floor(now() + (lateRecord ?? 0)); lateRecord = undefined; }
          }
          if (transaction.args.lane === "arrangement" && !on) takes();
          if (transaction.args.lane === "arrangement") transport.arrangementRecord = on; else transport.sessionRecord = on;
          // Like the bridge when Live doesn't confirm in time: it happened, but the answer can't say so.
          if (transport.recordUnsure) return refusal("Recording state is uncertain; perform fresh discovery.", { state: "uncertain" });
          return wrap({ transactionId: args.transactionId, state: "applied", recording: on });
        }
        if (transaction.name === "live_session_structure_preview") {
          // Like Live, each new track goes where its index says.
          const created: JsonObject[] = [];
          for (const item of transaction.args.tracks as JsonObject[]) {
            const at = typeof item.index === "number" ? Math.min(item.index, tracks.length) : tracks.length;
            tracks = [...tracks.slice(0, at), { name: String(item.name), color: 0, made: String(args.transactionId), madeAt: at }, ...tracks.slice(at)];
            created.push({ kind: "track", ref: `7:track:${at}`, name: String(item.name) });
          }
          return wrap({ transactionId: args.transactionId, state: "applied", created });
        }
        if (transaction.name === "live_mixer_preview" && transaction.args.volume === 0.4) return wrap({ transactionId: args.transactionId, state: "applied", display: { volume: "-9.3 dB", pan: "25L" } });
        if (transaction.name === "live_drum_pad_preview" && transaction.args.action === "load-samples") return wrap({ transactionId: args.transactionId, state: "applied", result: { pads: (transaction.args.pads as JsonObject[]).map((pad) => ({ ref: `7:drum_pad:0:0:${String(pad.note)}`, route: "hotswap" })) } });
        if (transaction.name === "live_drum_pad_preview") return wrap({ transactionId: args.transactionId, state: "applied", result: { ref: `7:drum_pad:0:0:${String(transaction.args.note)}`, route: "chain", samplePath: "/staged/Kick Deep.wav" } });
        if (transaction.name === "live_device_parameter_preview" && options.renders) {
          // Like Live with a knob it won't take a value for: the change isn't confirmed.
          const refuses = (Array.isArray(transaction.args.values) ? transaction.args.values as JsonObject[] : []).some((item) => {
            const match = /:parameter:(\d+):(\d+):(\d+)$/.exec(String(item.parameterRef));
            return match && tracks[Number(match[1])]?.devices?.[Number(match[2])]?.params[Number(match[3])]?.name === "Stuck Tone";
          });
          if (refuses) return refusal("request failed: parameter 1 of 1: parameter mutation was not confirmed", { state: "uncertain" });
          // Like Live: the knobs move.
          for (const item of (Array.isArray(transaction.args.values) ? transaction.args.values : [{ parameterRef: transaction.args.parameterRef, value: transaction.args.value }]) as JsonObject[]) {
            const match = /:parameter:(\d+):(\d+):(\d+)$/.exec(String(item.parameterRef));
            const param = match ? tracks[Number(match[1])]?.devices?.[Number(match[2])]?.params[Number(match[3])] : undefined;
            if (param && typeof item.value === "number") param.value = item.value;
          }
        }
        if (transaction.name === "live_device_parameter_preview") return wrap({ transactionId: args.transactionId, state: "applied", ...(Array.isArray(transaction.args.values) ? { parameters: (transaction.args.values as JsonObject[]).map((item) => ({ ref: item.parameterRef, value: item.value, revision: 2 })) } : { value: transaction.args.value }) });
        if (transaction.name === "live_browser_load_preview" && options.renders && String(transaction.args.itemId) === "audio_effects/Limiter") {
          const track = tracks[Number(String(transaction.args.trackRef).split(":").at(-1))]!;
          // As Live 12 has it: its input 0 to 1 for -24 to +24 dB.
          track.devices = [...(track.devices ?? []), { name: "Limiter", className: "Limiter", params: [{ name: "Input Gain", value: 0.5, min: 0, max: 1 }, { name: "Ceiling", value: 0.97, min: 0, max: 1 }] }];
          return wrap({ transactionId: args.transactionId, state: "applied", deviceRef: `7:device:${String(transaction.args.trackRef).split(":").at(-1)}:d${track.devices.length - 1}` });
        }
        if (transaction.name === "live_track_structure_preview" && transaction.args.action === "duplicate-track") {
          const at = Number(String(transaction.args.ref).split(":").at(-1));
          tracks = [...tracks.slice(0, at + 1), { ...structuredClone(tracks[at]!), armed: false, clips: [], made: String(args.transactionId) }, ...tracks.slice(at + 1)];
          return wrap({ transactionId: args.transactionId, state: "applied" });
        }
        if (transaction.name === "live_object_rename_preview" && options.renders) {
          const track = tracks[Number(String(transaction.args.ref).split(":").at(-1))];
          if (track) track.name = String(transaction.args.name);
          return wrap({ transactionId: args.transactionId, state: "applied" });
        }
        if (transaction.name === "live_browser_load_preview") {
          // Like the bridge, a Drum Rack in the Set brings the pad tools.
          if (String(transaction.args.itemId).endsWith("Drum Rack")) { drumRack = true; for (const listener of catalogListeners) listener(); }
          // A rack brings the rack tools, but (like real Live) the catalog notice comes later: no listener is told.
          if (/Rack$/.test(String(transaction.args.itemId))) rackLoaded = true;
          if (transaction.args.chainRef) return wrap({ transactionId: args.transactionId, state: "applied", deviceRef: "7:device:0:0:2:0",
            placement: { owner: "chain", rack: "Instrument Rack", chain: 2, index: 0, chains: [{ name: "Keys", devices: ["Operator"] }, { name: "Pad", devices: [] }, { name: "Bells", devices: ["Collision"] }] } });
          return wrap({ transactionId: args.transactionId, state: "applied", deviceRef: `7:device:${String(transaction.args.trackRef).split(":").at(-1)}:0` });
        }
        if (transaction.name === "live_rack_preview") {
          if (transaction.args.action === "insert-chain") return wrap({ transactionId: args.transactionId, state: "applied", chainRef: "7:chain:0:0:2",
            placement: { owner: "rack", rack: "Instrument Rack", chain: 2, chains: [{ name: "Keys", devices: ["Operator"] }, { name: "Pad", devices: [] }, { name: "Chain", devices: [] }] } });
          return wrap({ transactionId: args.transactionId, state: "applied", visibleMacroCount: 9 });
        }
        if (transaction.name === "live_midi_clip_preview") { clipCreated = true; return wrap({ transactionId: args.transactionId, state: "applied", clipRef: `7:clip:0:${String(transaction.args.sceneIndex)}` }); }
        if (transaction.name === "live_device_preview") return wrap({ transactionId: args.transactionId, state: "applied", result: { ref: "7:device:0:0", objectIdentity: "device-identity", samplePath: "/staged/Kick Deep.wav" } });
        if (transaction.name === "live_track_properties_preview") {
          const track = tracks[Number(String(transaction.args.ref).split(":").at(-1))]!;
          track.color = 0xe553a0;
          return wrap({ transactionId: args.transactionId, state: "applied", color: track.color });
        }
        return wrap({ transactionId: args.transactionId, state: "applied" });
      }
      if (name === "live_transaction_release") { const ids = args.transactionIds as string[]; for (const id of ids) { pending.delete(id); released.push(id); } return wrap({ released: ids.length }); }
      if (name === "live_undo") {
        if (undoRefusal) return refusal(undoRefusal);
        const transaction = pending.get(String(args.transactionId));
        if (transaction?.name === "live_tempo_preview") tempo = 120;
        // Tracks it made go, unless one has recorded since (then only with discard, as the bridge does).
        if (transaction?.name === "live_track_structure_preview" && transaction.args.action === "duplicate-track") tracks = tracks.filter((track) => track.made !== String(args.transactionId));
        if (transaction?.name === "live_session_structure_preview") {
          const made = tracks.filter((track) => track.made === String(args.transactionId));
          if (made.some((track) => track.clips?.length) && args.discard !== true) return refusal("created Session structure was modified after apply; undo refused");
          // Like the bridge: tracks another change made above these must go first.
          const highest = Math.max(...made.map((track) => tracks.indexOf(track)));
          if (tracks.some((track, index) => index > highest && track.made !== undefined && track.made !== String(args.transactionId))) return refusal("request failed: transaction-owned structure cleanup must proceed from the highest positional authority");
          // Like the bridge: the undo is tied to where it made them.
          if (made.some((track) => track.madeAt !== undefined && tracks.indexOf(track) !== track.madeAt)) return refusal("transaction-owned Session structure shifted from its exact reference", { state: "uncertain" });
          tracks = tracks.filter((track) => !made.includes(track));
        }
        return wrap({ transactionId: args.transactionId, state: "undone", idempotent: false });
      }
      return wrap({});
    },
    onCatalogChanged(listener) { catalogListeners.add(listener); return () => { catalogListeners.delete(listener); }; },
    onDisconnect() { return () => {}; },
    onLiveEvent(listener) { liveEventListeners.add(listener); return () => { liveEventListeners.delete(listener); }; },
    async close() {},
  };
  let settledDepth = 0; let deepest = 0;
  const answer = endpoint.call.bind(endpoint);
  endpoint.call = async (name: string, args: JsonObject, signal: AbortSignal) => {
    const depth = settledDepth + 1; deepest = Math.max(deepest, depth);
    try { return await answer(name, args, signal); } finally { settledDepth = Math.max(settledDepth, depth); }
  };
  const states: string[] = [];
  const pins: PinnedNode[] = [];
  const actions: { title: string; playing?: boolean; recording?: boolean }[] = [];
  const auditions: AuditionEvent[] = [];
  const integration = createAbletonIntegration({ connect: async () => endpoint, onConnection: (state) => { states.push(state); options.onConnection?.(state); }, onChange: (change) => records.push(change),
    onAction: (action) => actions.push(action), changeTimeoutMs: 2_000, reconnectIntervalMs: 10, onAudition: (event) => { auditions.push(event); options.onAudition?.(event); },
    onPointed: (pin) => pins.push(pin), ...(options.onFocus ? { onFocus: options.onFocus, focusIntervalMs: 60_000 } : {}), ...(options.onTransport ? { onTransport: options.onTransport } : {}),
    // Never the producer's own ~/.kumi: each bridge its own file.
    restoreFile: options.restoreFile ?? join(mkdtempSync(join(tmpdir(), "kumi-restore-")), "audition-restore.json"),
    lowDisk: (path, needed, what) => lowDisk(path, needed, what, async () => options.freeDisk ?? 1e12), ears: options.ears ?? false, hands: options.hands ?? false,
    ...(options.fast !== undefined ? { fast: options.fast } : {}),
    // Never the producer's own User Library: wavetables and devices go in a throwaway one.
    userLibrary: mkdtempSync(join(tmpdir(), "kumi-user-library-")) });
  return {
    integration, requests, records, states, actions, auditions, released, pins, get tempo() { return tempo; },
    /** Live sends an event (notifications/live_event). */
    liveEvent: (event: JsonObject) => { for (const listener of [...liveEventListeners]) listener(event); },
    main, get position() { return now(); }, trackNames: () => tracks.map((track) => track.name),
    /** Live starts playing from `beat` (with space, say: Kumi isn't asked). */
    startPlayback: (beat: number) => play(beat),
    /** A track's devices as they are now (knobs moved by the search included). */
    devicesOf: (name: string) => tracks.find((track) => track.name === name)?.devices,
    /** The next request of this name (or play action) is refused, as Live refuses one. */
    failNext: (what: string) => { failStep = what; },
    /** How many Live round trips `work` waited for one after another; concurrent ones count once. */
    async roundTrips<T>(work: () => Promise<T>): Promise<{ value: T; trips: number; calls: number }> {
      const before = requests.length; settledDepth = 0; deepest = 0;
      const value = await work();
      return { value, trips: deepest, calls: requests.length - before };
    },
    transport,
    /** A track armed in Live (by the producer, or left armed). */
    arm: (index: number, armed = true) => { if (tracks[index]) tracks[index] = { ...tracks[index]!, armed }; },
    armed: () => tracks.flatMap((track, index) => (track.armed ? [index] : [])),
    liveAway: () => { live = false; },
    liveBack: () => { live = true; epoch++; },
    refuseUndo: (text: string) => { undoRefusal = text; },
    /** The bridge re-negotiates its tools after content changes and says so. */
    catalogChanged: () => { for (const listener of catalogListeners) listener(); },
    addDrumRack: () => { drumRack = true; for (const listener of catalogListeners) listener(); },
    deleteLastTrack: () => { tracks = tracks.slice(0, -1); },
    /** Live makes a track itself (a bounce, a group), as a command of its own would. */
    addTrack: (name: string) => { tracks = [...tracks, { name, color: 0x808080 }]; },
    freeze: (name: string, frozen = true) => { tracks = tracks.map((track) => (track.name === name ? { ...track, frozen } : track)); },
    failApply: (how: "throw" | "uncertain" | "unreadable") => { applyFailure = how; },
    /** The next request to this tool waits, after it's sent, until released. */
    hold: (name: string) => {
      let sent!: () => void; let release!: () => void;
      const arrived = new Promise<void>((resolve) => { sent = resolve; });
      holds.set(name, { sent, wait: new Promise<void>((resolve) => { release = resolve; }) });
      return { sent: arrived, release };
    },
    holdApply: () => {
      let sent!: () => void; let release!: () => void;
      const began = new Promise<void>((resolve) => { sent = resolve; });
      gate = { sent, wait: new Promise<void>((resolve) => { release = resolve; }) };
      return { began, release };
    },
  };
}
export const signal = () => new AbortController().signal;
export function tool(tools: readonly KernelTool[], name: string) { const found = tools.find((item) => item.name === name); assert(found, `${name} is offered`); return found; }
export async function opened(options: Options = {}) {
  const b = bridge(options);
  await b.integration.start(signal());
  const observation = await b.integration.observe(signal());
  // Keep the fixture's getters live (a spread would copy their current values).
  return Object.assign(b, { tools: observation.tools, observation });
}
