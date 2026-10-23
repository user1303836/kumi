// Right-click "Ask Kumi about this": the producer points at an object (or a selection) in Live, and
// Kumi's next message about "this" means it. Each click becomes a `pointed` event carrying the object's
// positional path, as the Remote Script would name it, plus names for showing it.
import { DataModelObject, type ArrangementSelection, type ClipSlotSelection, type ContextMenuScope, type ExtensionContext, type Handle } from "@ableton-extensions/sdk";
import { locate, type Located } from "./refs.js";

type Context = ExtensionContext<"1.0.0">;

export const OBJECT_SCOPES: ContextMenuScope<"1.0.0">[] = ["AudioClip", "MidiClip", "AudioTrack", "MidiTrack", "ClipSlot", "Scene", "Simpler", "Sample", "DrumRack"];
export const SELECTION_SCOPES: ContextMenuScope<"1.0.0">[] = ["ClipSlotSelection", "AudioTrack.ArrangementSelection", "MidiTrack.ArrangementSelection"];
export const POINT = "kumi.point";
export const POINT_SELECTION = "kumi.point-selection";

function where(located: Located): Record<string, unknown> {
  return { kind: located.kind, path: located.path, name: located.name, trail: located.trail };
}

/** A Sample belongs to its Simpler: point at the Simpler, saying it was the sample. */
function locateObject(context: Context, object: DataModelObject<"1.0.0">): Located | undefined {
  const found = locate(context, object);
  if (found) return found;
  const parent = object.parent;
  const owner = parent ? locate(context, parent) : undefined;
  return owner ? { ...owner, kind: owner.kind === "device" ? "sample" : owner.kind } : undefined;
}

export function pointedAt(context: Context, argument: unknown): Record<string, unknown> | undefined {
  const selection = argument as Partial<ArrangementSelection & ClipSlotSelection> | undefined;
  if (selection && Array.isArray(selection.selected_lanes)) {
    const lanes = selection.selected_lanes.map((handle) => locateObject(context, context.getObjectFromHandle(handle, DataModelObject))).filter((item): item is Located => !!item);
    const from = Number(selection.time_selection_start); const to = Number(selection.time_selection_end);
    return { kind: "arrangement_selection", lanes: lanes.map(where), timeSelection: { fromBeat: from, toBeat: to }, name: lanes.map((lane) => lane.name).join(", "), trail: lanes.map((lane) => lane.name) };
  }
  if (selection && Array.isArray(selection.selected_clip_slots)) {
    const slots = selection.selected_clip_slots.map((handle) => locateObject(context, context.getObjectFromHandle(handle, DataModelObject))).filter((item): item is Located => !!item);
    return { kind: "clip_slot_selection", slots: slots.map(where), name: `${slots.length} clip slots`, trail: [...new Set(slots.map((slot) => slot.trail[0] ?? ""))] };
  }
  const object = context.getObjectFromHandle(argument as Handle, DataModelObject);
  const located = locateObject(context, object);
  return located ? where(located) : undefined;
}

/** Registers the menu items; each click calls `onPointed` with where the producer pointed. */
export async function registerPointing(context: Context, onPointed: (payload: Record<string, unknown>) => void, log: (line: string) => void = () => undefined): Promise<void> {
  const handle = (argument: unknown) => {
    try {
      const payload = pointedAt(context, argument);
      if (payload) onPointed({ ...payload, at: Date.now() });
      else log("pointed at something Kumi couldn't find in the Set");
    } catch (error) { log(`pointing failed: ${error instanceof Error ? error.message : String(error)}`); }
  };
  context.commands.registerCommand(POINT, handle);
  context.commands.registerCommand(POINT_SELECTION, handle);
  for (const scope of OBJECT_SCOPES) await context.ui.registerContextMenuAction(scope, "Ask Kumi about this", POINT);
  for (const scope of SELECTION_SCOPES) await context.ui.registerContextMenuAction(scope, "Ask Kumi about this selection", POINT_SELECTION);
}
