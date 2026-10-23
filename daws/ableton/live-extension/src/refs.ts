// Positional references, as the Remote Script makes them: "{epoch}:{kind}:{path}". Tracks count the
// Set's tracks, then its returns, then Main (`track:3`); a device's path runs from its track through
// racks' chains (`device:3:0:1:2` is track 3, device 0, its chain 1, device 2). The epoch belongs to
// the host, which checks it; here a reference is resolved by walking the model afresh every time,
// because the SDK's handles change when anything moves.
import { Chain, Clip, ClipSlot, DataModelObject, Device, RackDevice, Scene, TakeLane, Track, type ExtensionContext } from "@ableton-extensions/sdk";

type Context = ExtensionContext<"1.0.0">;
type AnyTrack = Track<"1.0.0">;
type AnyDevice = Device<"1.0.0">;
type AnyChain = Chain<"1.0.0">;
type AnyClip = Clip<"1.0.0">;

export interface ParsedRef { epoch: string; kind: string; path: number[] }

export function parseRef(reference: string): ParsedRef {
  const parts = reference.split(":");
  if (parts.length < 3) throw new Error(`not a Live reference: ${reference}`);
  const [epoch, kind, ...rest] = parts;
  const path = rest.map((part) => { const index = Number(part); if (!Number.isSafeInteger(index) || index < 0) throw new Error(`a reference this channel can't follow: ${reference}`); return index; });
  return { epoch: epoch!, kind: kind!, path };
}

export function makeRef(epoch: string, kind: string, path: readonly number[]): string { return `${epoch}:${kind}:${path.join(":")}`; }

/** Every track in the Remote Script's order: the Set's tracks, its returns, then Main. */
export function allTracks(context: Context): AnyTrack[] {
  const song = context.application.song;
  return [...song.tracks, ...song.returnTracks, song.mainTrack];
}

function at<T>(items: readonly T[], index: number | undefined, what: string): T {
  if (index === undefined || index >= items.length) throw new Error(`${what} ${index ?? "?"} isn't in the Set any more`);
  return items[index]!;
}

export function trackAt(context: Context, index: number | undefined): AnyTrack { return at(allTracks(context), index, "track"); }

/** A device and what holds it (its track, or a rack's chain) from a device path [t, d, (c, d)*]. */
export function deviceAt(context: Context, path: readonly number[]): { device: AnyDevice; owner: AnyTrack | AnyChain; index: number; track: AnyTrack } {
  if (path.length < 2 || path.length % 2 !== 0) throw new Error(`not a device path: ${path.join(":")}`);
  const track = trackAt(context, path[0]);
  let owner: AnyTrack | AnyChain = track;
  let device = at(track.devices, path[1], "device");
  let index = path[1]!;
  for (let step = 2; step < path.length; step += 2) {
    if (!(device instanceof RackDevice)) throw new Error(`${device.name} has no chains`);
    owner = at(device.chains, path[step], "chain");
    index = path[step + 1]!;
    device = at(owner.devices, index, "device");
  }
  return { device, owner, index, track };
}

/** A rack's chain from a chain path [t, d, (c, d)*, c]. */
export function chainAt(context: Context, path: readonly number[]): { chain: AnyChain; rack: RackDevice<"1.0.0">; index: number } {
  if (path.length < 3 || path.length % 2 !== 1) throw new Error(`not a chain path: ${path.join(":")}`);
  const { device } = deviceAt(context, path.slice(0, -1));
  if (!(device instanceof RackDevice)) throw new Error(`${device.name} has no chains`);
  const index = path[path.length - 1]!;
  return { chain: at(device.chains, index, "chain"), rack: device, index };
}

export function clipSlotAt(context: Context, path: readonly number[]): ClipSlot<"1.0.0"> {
  return at(trackAt(context, path[0]).clipSlots, path[1], "clip slot");
}

export function sceneAt(context: Context, index: number | undefined): Scene<"1.0.0"> { return at(context.application.song.scenes, index, "scene"); }

export function takeLaneAt(context: Context, path: readonly number[]): TakeLane<"1.0.0"> { return at(trackAt(context, path[0]).takeLanes, path[1], "take lane"); }

/** Any object a reference names, for the kinds this channel follows. */
export function resolve(context: Context, reference: string): DataModelObject<"1.0.0"> {
  const { kind, path } = parseRef(reference);
  switch (kind) {
    case "track": return trackAt(context, path[0]);
    case "device": return deviceAt(context, path).device;
    case "chain": return chainAt(context, path).chain;
    case "clip_slot": return clipSlotAt(context, path);
    case "clip": { const clip = clipSlotAt(context, path).clip; if (!clip) throw new Error("that clip slot is empty now"); return clip; }
    case "arrangement_clip": return at(trackAt(context, path[0]).arrangementClips, path[1], "Arrangement clip");
    case "take_lane": return takeLaneAt(context, path);
    case "take_lane_clip": return at(takeLaneAt(context, path).clips, path[2], "take lane clip");
    case "scene": return sceneAt(context, path[0]);
    case "locator": return at(context.application.song.cuePoints, path[0], "locator");
    default: throw new Error(`this channel doesn't follow ${kind} references`);
  }
}

/** Throws unless the object still has the name the host read, so a shifted position can't be acted on. */
export function checkName(object: { name: string }, expected: string | undefined, what: string): void {
  if (expected !== undefined && object.name !== expected) throw new Error(`the ${what} at that position is "${object.name}" now, not "${expected}"`);
}

export interface Located { kind: string; path: number[]; name: string; trail: string[] }

const same = (a: DataModelObject<"1.0.0">, b: DataModelObject<"1.0.0">) => a.handle.id === b.handle.id;

function locateDevice(devices: readonly AnyDevice[], target: DataModelObject<"1.0.0">, path: number[], trail: string[]): Located | undefined {
  for (const [index, device] of devices.entries()) {
    if (same(device, target)) return { kind: "device", path: [...path, index], name: device.name, trail: [...trail, device.name] };
    if (device instanceof RackDevice) {
      for (const [chainIndex, chain] of device.chains.entries()) {
        const found = locateDevice(chain.devices, target, [...path, index, chainIndex], [...trail, device.name]);
        if (found) return found;
      }
    }
  }
  return undefined;
}

/** Where an object is, as the Remote Script would name it, by walking the Set; undefined when it isn't found. */
export function locate(context: Context, target: DataModelObject<"1.0.0">): Located | undefined {
  const song = context.application.song;
  for (const [index, scene] of song.scenes.entries()) if (same(scene, target)) return { kind: "scene", path: [index], name: scene.name, trail: [scene.name] };
  for (const [t, track] of allTracks(context).entries()) {
    if (same(track, target)) return { kind: "track", path: [t], name: track.name, trail: [track.name] };
    for (const [s, slot] of track.clipSlots.entries()) {
      if (same(slot, target)) return { kind: "clip_slot", path: [t, s], name: slot.clip?.name ?? "", trail: [track.name] };
      const clip = slot.clip;
      if (clip && same(clip, target)) return { kind: "clip", path: [t, s], name: clip.name, trail: [track.name, clip.name] };
    }
    for (const [c, clip] of track.arrangementClips.entries()) if (same(clip, target)) return { kind: "arrangement_clip", path: [t, c], name: clip.name, trail: [track.name, clip.name] };
    for (const [l, lane] of track.takeLanes.entries()) {
      if (same(lane, target)) return { kind: "take_lane", path: [t, l], name: lane.name, trail: [track.name, lane.name] };
      for (const [c, clip] of lane.clips.entries()) if (same(clip, target)) return { kind: "take_lane_clip", path: [t, l, c], name: clip.name, trail: [track.name, lane.name, clip.name] };
    }
    const device = locateDevice(track.devices, target, [t], [track.name]);
    if (device) return device;
  }
  return undefined;
}

export type { AnyClip, AnyDevice, AnyTrack, AnyChain };
