// A small Live for testing the built extension without Live or the SDK's sources: it implements the
// Extension Host's low-level module (the handle-based dataModel, resources, ui, commands) the SDK's
// classes call, over an in-memory Set.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const CLASSES = {
  Application: ["Application"], Song: ["Song"], AudioTrack: ["AudioTrack", "Track"], MidiTrack: ["MidiTrack", "Track"],
  AudioClip: ["AudioClip", "Clip"], MidiClip: ["MidiClip", "Clip"], ClipSlot: ["ClipSlot"], TakeLane: ["TakeLane"],
  Scene: ["Scene"], CuePoint: ["CuePoint"], Device: ["Device"], RackDevice: ["RackDevice", "Device"],
  DrumRackDevice: ["DrumRackDevice", "RackDevice", "Device"], Simpler: ["Simpler", "Device"], Sample: ["Sample"],
  Chain: ["Chain"], DrumChain: ["DrumChain", "Chain"], MixerDevice: ["MixerDevice"],
};

export function wav(path, { channels = 2, sampleRate = 44100, bits = 24, seconds = 1 } = {}) {
  const frames = Math.round(sampleRate * seconds); const data = frames * channels * (bits / 8);
  const header = Buffer.alloc(44);
  header.write("RIFF", 0); header.writeUInt32LE(36 + data, 4); header.write("WAVE", 8); header.write("fmt ", 12);
  header.writeUInt32LE(16, 16); header.writeUInt16LE(1, 20); header.writeUInt16LE(channels, 22); header.writeUInt32LE(sampleRate, 24);
  header.writeUInt32LE(sampleRate * channels * (bits / 8), 28); header.writeUInt16LE(channels * (bits / 8), 32); header.writeUInt16LE(bits, 34);
  header.write("data", 36); header.writeUInt32LE(data, 40);
  writeFileSync(path, Buffer.concat([header, Buffer.alloc(data)]));
}

export function fakeLive({ storage, temp, liveTemp, tempo = 120 }) {
  let next = 1n; const objects = new Map(); const menu = []; const commands = new Map(); const transactions = []; let depth = 0; const created = []; const notesSet = [];
  const make = (cls, fields = {}, parent = null) => { const id = next++; const object = { id, cls, parent, ...fields }; objects.set(id, object); return object; };
  const handle = (object) => ({ id: object.id });
  const get = (h) => { const object = objects.get(h.id); if (!object) throw new Error("stale handle"); return object; };
  const app = make("Application"); const song = make("Song", { tempo, tracks: [], returns: [], scenes: [], cues: [] }, app); app.song = song;
  const track = (cls, name, { slots = 2 } = {}) => { const t = make(cls, { name, clipSlots: [], arrangementClips: [], takeLanes: [], devices: [] }, song); for (let i = 0; i < slots; i++) t.clipSlots.push(make("ClipSlot", { clip: null }, t)); return t; };
  const device = (cls, name, owner, extra = {}) => make(cls, { name, chains: [], sample: null, ...extra }, owner);
  const clip = (cls, owner, start, duration, name = "") => make(cls, { name, start, end: start + duration, looping: false, notes: [], filePath: cls === "AudioClip" ? "/clip.wav" : undefined }, owner);
  const model = { app, song, objects, make, track, device, clip, menu, commands, transactions, handle, created, notesSet };
  const ok = (resolve, value) => resolve(value);
  const dataModel = {
    getObjectIsOfClass: (h, className) => (CLASSES[get(h).cls] ?? [get(h).cls]).includes(className),
    getObjectCanonicalParent: (h) => { const parent = get(h).parent; return parent ? handle(parent) : null; },
    getRoot: () => handle(app), rootGetSong: () => handle(song),
    songGetTempo: () => song.tempo, songSetTempo: (_h, value) => { song.tempo = value; },
    songGetTracks: () => song.tracks.map(handle), songGetReturnTracks: () => song.returns.map(handle), songGetMainTrack: () => handle(song.main),
    songGetScenes: () => song.scenes.map(handle), songGetCuePoints: () => song.cues.map(handle),
    trackGetName: (h) => get(h).name, trackSetName: (h, value) => { get(h).name = value; },
    trackGetClipSlots: (h) => get(h).clipSlots.map(handle), trackGetTakeLanes: (h) => get(h).takeLanes.map(handle),
    trackGetArrangementClips: (h) => get(h).arrangementClips.map(handle), trackGetDevices: (h) => get(h).devices.map(handle),
    trackGetGroupTrack: (h) => { const group = get(h).group; return group ? handle(group) : null; },
    trackCreateMidiClip: (h, start, duration, resolve) => { const t = get(h); const c = clip("MidiClip", t, start, duration); created.push({ start, insideTransaction: depth > 0 }); t.arrangementClips.push(c); t.arrangementClips.sort((a, b) => a.start - b.start); ok(resolve, handle(c)); },
    trackClearClipsInRange: (h, from, to, resolve) => {
      const t = get(h); const kept = [];
      for (const c of t.arrangementClips) {
        if (c.start >= from && c.end <= to) { objects.delete(c.id); continue; }
        if (c.start < to && c.end > from) { if (c.start < from) c.end = Math.min(c.end, from); else c.start = to; }
        kept.push(c);
      }
      t.arrangementClips = kept; resolve();
    },
    trackDuplicateDevice: (h, d, resolve) => { const t = get(h); const original = get(d); const index = t.devices.indexOf(original); const copy = device(original.cls, original.name, t); t.devices.splice(index + 1, 0, copy); ok(resolve, handle(copy)); },
    trackInsertDevice: (h, name, index, resolve) => { const t = get(h); const d = device(name === "Simpler" ? "Simpler" : "Device", name, t); t.devices.splice(Number(index), 0, d); ok(resolve, handle(d)); },
    takelaneGetClips: (h) => get(h).clips.map(handle), takelaneGetName: (h) => get(h).name, takelaneSetName: (h, value) => { get(h).name = value; },
    takelaneCreateMidiClip: (h, start, duration, resolve) => { const lane = get(h); const c = clip("MidiClip", lane, start, duration); lane.clips.push(c); ok(resolve, handle(c)); },
    clipGetName: (h) => get(h).name, clipSetName: (h, value) => { get(h).name = value; },
    clipGetStartTime: (h) => get(h).start, clipGetEndTime: (h) => get(h).end, clipGetStartMarker: () => 0, clipGetEndMarker: (h) => get(h).end - get(h).start,
    clipGetLooping: (h) => get(h).looping, clipSetLooping: (h, value) => { get(h).looping = value; },
    clipGetLoopStart: () => 0, clipGetLoopEnd: (h) => get(h).end - get(h).start, clipGetColor: () => 0n, clipSetColor: () => undefined,
    clipGetMuted: () => false, clipSetMuted: () => undefined,
    midiclipGetNotes: (h) => get(h).notes.map((note) => ({ ...note })), midiclipSetNotes: (h, notes) => { get(h).notes = notes.map((note) => ({ ...note })); notesSet.push({ start: get(h).start, insideTransaction: depth > 0 }); },
    audioclipGetFilePath: (h) => get(h).filePath,
    clipslotGetClip: (h) => { const c = get(h).clip; return c ? handle(c) : null; },
    deviceGetName: (h) => get(h).name, deviceGetParameters: () => [],
    chainGetDevices: (h) => get(h).devices.map(handle), chainGetMixerDevice: (h) => handle(get(h).mixer ?? (get(h).mixer = make("MixerDevice", {}, get(h)))),
    chainInsertDevice: (h, name, index, resolve) => { const chain = get(h); const d = device(name === "Simpler" ? "Simpler" : "Device", name, chain); chain.devices.splice(Number(index), 0, d); ok(resolve, handle(d)); },
    chainDuplicateDevice: (h, d, resolve) => { const chain = get(h); const original = get(d); const copy = device(original.cls, original.name, chain); chain.devices.splice(chain.devices.indexOf(original) + 1, 0, copy); ok(resolve, handle(copy)); },
    rackdeviceGetChains: (h) => get(h).chains.map(handle),
    rackdeviceInsertChain: (h, index, resolve) => { const rack = get(h); const chain = make(rack.cls === "DrumRackDevice" ? "DrumChain" : "Chain", { devices: [], receivingNote: 36n }, rack); rack.chains.splice(Number(index), 0, chain); ok(resolve, handle(chain)); },
    drumchainGetReceivingNote: (h) => get(h).receivingNote, drumchainSetReceivingNote: (h, value) => { get(h).receivingNote = BigInt(value); },
    simplerGetSample: (h) => { const s = get(h).sample; return s ? handle(s) : null; },
    simplerReplaceSample: (h, path, resolve) => { const simpler = get(h); simpler.sample = make("Sample", { filePath: path }, simpler); ok(resolve, handle(simpler.sample)); },
    sampleGetFilePath: (h) => get(h).filePath,
    sceneGetName: (h) => get(h).name, sceneSetName: (h, value) => { get(h).name = value; },
    cuePointGetName: (h) => get(h).name, cuePointGetTime: (h) => get(h).time,
    withinTransaction: (fn) => { transactions.push("begin"); depth += 1; try { return fn(); } finally { depth -= 1; transactions.push("end"); } },
  };
  const rendered = [];
  const resources = {
    renderPreFxAudio: (h, { startTime, endTime }, resolve, reject) => setTimeout(() => {
      const t = get(h); if (t.cls !== "AudioTrack") { reject(); return; }
      mkdirSync(liveTemp, { recursive: true });
      // Live names a render after the clip, to the second.
      const path = join(liveTemp, `${t.arrangementClips[0]?.name || "render"} [2026-09-30 120000].wav`);
      wav(path, { seconds: ((endTime - startTime) * 60) / song.tempo }); rendered.push(path); resolve(path);
    }, model.renderDelayMs ?? 0),
    importIntoProject: (path, resolve) => resolve(join(storage, "Project", "Samples", "Imported", path.split("/").pop())),
  };
  const ui = { registerContextMenuAction: (scope, title, command, done) => { menu.push({ scope, title, command }); done((finished) => finished()); } };
  const commandModule = { registerCommand: (id, callback) => commands.set(id, callback), executeCommand: (id, ...args) => commands.get(id)?.(...args) };
  const activation = { hostApiVersion: "1.0.0", initializeExtensionHost: () => ({ commands: commandModule, dataModel, environment: { storageDirectory: storage, tempDirectory: temp, language: "EN" }, resources, ui }) };
  // A small Set: two MIDI tracks, an audio track (with a clip), a return, Main, two scenes.
  const keys = track("MidiTrack", "Keys"); const drums = track("MidiTrack", "Drums"); const vox = track("AudioTrack", "Vox");
  vox.arrangementClips.push(clip("AudioClip", vox, 0, 8, "Vox take"));
  const rack = device("DrumRackDevice", "Drum Rack", drums); drums.devices.push(rack);
  const kickChain = make("DrumChain", { devices: [], receivingNote: 36n }, rack); rack.chains.push(kickChain);
  const kick = device("Simpler", "Kick", kickChain); kickChain.devices.push(kick);
  keys.devices.push(device("Device", "Operator", keys), device("Device", "Reverb", keys));
  keys.clipSlots[1].clip = clip("MidiClip", keys.clipSlots[1], 0, 4, "Chords");
  song.tracks.push(keys, drums, vox);
  song.returns.push(track("AudioTrack", "A-Reverb", { slots: 0 }));
  song.main = track("AudioTrack", "Main", { slots: 0 });
  song.scenes.push(make("Scene", { name: "Intro" }, song), make("Scene", { name: "Drop" }, song));
  Object.assign(model, { keys, drums, vox, rack, kick, rendered });
  return { activation, model };
}
