// Kumi's palette as the map and its dock use it: the sounds the producer gathered to work with, kept by the backend
// (`/palette`) and shared with the agent's `kumi_palette`, with native Kumi's semantics. Plain data here, no DOM:
// tested in ui/tests.
import { plainId, quotable } from './format.ts'

export interface PaletteItem {
  /** The item's own id in the palette: what remove and move name. */
  key: string
  source: 'artifact' | 'sound'
  /** The sound's id on the current map; null when its file isn't on it (shown, not playable here). */
  sound_id: string | null
  /** A sound-design result or another file Kumi made, played by this id. */
  artifact_id: string | null
  name: string
  class: string | null
  /** The map group the sound is in, for its colour; null off the map. */
  group: number | null
}

export interface PaletteView {
  id: string
  name: string
  /** 0: strict, the palette's own sounds; 1–50: loose, each sound brings its nearest that many. */
  reach: number
  mode: string
  /** How many sounds the palette keeps out (the agent's `exclude`); they aren't in `items`. */
  excluded: number
  items: PaletteItem[]
}

export interface PaletteListed {
  id: string
  name: string
  items: number
  current: boolean
}

/** `/palette`'s answer: the current palette (none until something is gathered or a palette is made) and every
 *  palette's name. */
export interface PaletteReply {
  seq: number
  map_revision: string | null
  current: PaletteView | null
  palettes: PaletteListed[]
}

/** What the dock and the map ask of `POST /palette`. Edits change no project, so they run at once. */
export type PaletteAction =
  | { action: 'gather'; sound_ids?: string[]; artifact_ids?: string[] }
  | { action: 'remove'; keys: string[] }
  | { action: 'move'; key: string; to: number }
  | { action: 'new' | 'rename'; name: string }
  | { action: 'forget' | 'use'; id: string }
  | { action: 'reach'; reach: number }

/** The most sounds one gather takes (native's MOST_GATHERED): a palette is what to work with, not the library. */
export const MOST_GATHERED = 10_000
/** Native's `/palette loose` with no number, and its farthest reach. */
export const LOOSE = 5
export const FARTHEST = 50
/** The longest palette name the backend takes. */
export const NAME_MOST = 80

/** The palette's sounds on the map in hand, for the map to mark: only ids of that map's revision count. */
export function marksOf(palette: PaletteReply | null, revision: string | null | undefined): Set<string> {
  const out = new Set<string>()
  if (!palette?.current || !revision || palette.map_revision !== revision) return out
  for (const item of palette.current.items) if (item.sound_id) out.add(item.sound_id)
  return out
}

/** What a chip plays: its sound on this map, or the file Kumi made; null when neither can play here. */
export function playableId(item: PaletteItem): string | null {
  return item.sound_id ?? item.artifact_id
}

/** The items in the order a move of `key` to `to` leaves them, as the backend keeps it: shown at once, before the
 *  backend's answer. */
export function moved(items: readonly PaletteItem[], key: string, to: number): PaletteItem[] {
  const from = items.findIndex(item => item.key === key)
  if (from < 0) return [...items]
  const out = items.filter(item => item.key !== key)
  out.splice(Math.max(0, Math.min(out.length, to)), 0, items[from])
  return out
}

/** A palette name as the backend takes it: one plain line, 1–80 characters; null for nothing left. */
export function paletteName(text: string): string | null {
  const name = text.replace(/[\p{Cc}\p{Cf}\u2028\u2029]+/gu, ' ').replace(/\s+/g, ' ').trim().slice(0, NAME_MOST).trim()
  return name || null
}

/** Left unsent in the chat, like Use…: the model reads the palette with `kumi_palette` (`kit`) and loads its sounds as
 *  the DAW at hand takes a kit. */
export function kitRequest(name: string): string {
  const said = quotable(name)
  return said ? `Load the sounds in my palette “${said}” as a kit` : 'Load the sounds in my palette as a kit'
}

/** The sound ids a gather sends: plain ids only, each once, at most `MOST_GATHERED` (the first ones, in map order). */
export function gatherIds(ids: Iterable<string>): { ids: string[]; dropped: number } {
  const seen = new Set<string>()
  for (const id of ids) if (plainId(id)) seen.add(id)
  const all = [...seen]
  return { ids: all.slice(0, MOST_GATHERED), dropped: Math.max(0, all.length - MOST_GATHERED) }
}

/** A waveform thumbnail's bars: the loudest sample in each of `bins` equal stretches of `samples`, 0 to 1, scaled so
 *  the loudest bar is full height (a quiet sound still shows its shape). */
export function peaksOf(samples: Float32Array, bins: number): number[] {
  const out = new Array<number>(bins).fill(0)
  if (samples.length === 0 || bins <= 0) return out
  let loudest = 0
  for (let bin = 0; bin < bins; bin++) {
    const start = Math.floor((bin * samples.length) / bins)
    const end = Math.max(start + 1, Math.floor(((bin + 1) * samples.length) / bins))
    let peak = 0
    for (let at = start; at < end && at < samples.length; at++) {
      const value = Math.abs(samples[at])
      if (value > peak) peak = value
    }
    out[bin] = peak
    if (peak > loudest) loudest = peak
  }
  return loudest > 0 ? out.map(peak => peak / loudest) : out
}
