// Sound design as the map's Make panel uses it: a new sound crossed, fused or stacked from two to sixteen sounds of the
// map or the palette, by Kumi's engine (`POST /design`, the contract's "engine's interface"). Plain data here, no DOM:
// tested in ui/tests.
import { plainId, quotable } from './format.ts'

export type DesignOp = 'cross' | 'fuse' | 'stack'
export type DesignMode = 'attack' | 'blend' | 'loop' | 'modulate' | 'resonate' | 'smear' | 'texture'

/** A sound the engine takes, by id only: a map sound with its revision, or a file Kumi made (an earlier result). */
export interface DesignSource {
  sound_id?: string
  map_revision?: string
  artifact_id?: string
  name?: string
}

export interface DesignOptions {
  seconds?: number
  bpm?: number
  bars?: number
  attack_from?: number
  split?: number[]
  depth?: number
  stretch?: number
}

export interface DesignRequest {
  op: DesignOp
  mode?: DesignMode
  sources: DesignSource[]
  seed?: number
  options?: DesignOptions
}

/** The engine's answer: the new sound's id (`/audio` plays it, the palette keeps it), the seed that made it (send it
 *  back to make it again, change it to reroll), and what it did, in one line. */
export interface DesignResult {
  artifact_id: string
  op: DesignOp
  mode?: DesignMode
  seed: number
  sources: DesignSource[]
  duration: number
  sample_rate: number
  channels: number
  peaks: number[]
  recipe: string
  at?: number
}

/** One card of the Make panel: an op and its mode, and how many sounds it takes. */
export interface DesignCard {
  key: string
  op: DesignOp
  mode?: DesignMode
  /** The options the card shows; the engine draws any left out from the seed. */
  options: (keyof DesignOptions)[]
}

export const MOST_SOURCES = 16
export const MOST_STACKED = 8

/** The cards, in the panel's order: cross's four, fuse's three, then stack. */
export const CARDS: DesignCard[] = [
  { key: 'blend', op: 'cross', mode: 'blend', options: ['seconds'] },
  { key: 'attack', op: 'cross', mode: 'attack', options: ['attack_from'] },
  { key: 'resonate', op: 'cross', mode: 'resonate', options: ['depth'] },
  { key: 'modulate', op: 'cross', mode: 'modulate', options: ['depth'] },
  { key: 'texture', op: 'fuse', mode: 'texture', options: ['seconds', 'depth'] },
  { key: 'loop', op: 'fuse', mode: 'loop', options: ['bpm', 'bars'] },
  { key: 'smear', op: 'fuse', mode: 'smear', options: ['stretch', 'depth'] },
  { key: 'stack', op: 'stack', options: [] }
]

/** The ranges each option takes (the engine's own), and the step a slider moves by. */
export const RANGES: Record<Exclude<keyof DesignOptions, 'split' | 'attack_from'>, { min: number; max: number; step: number }> = {
  seconds: { min: 0.5, max: 30, step: 0.5 },
  bpm: { min: 40, max: 240, step: 1 },
  bars: { min: 1, max: 4, step: 1 },
  depth: { min: 0, max: 1, step: 0.05 },
  stretch: { min: 4, max: 16, step: 1 }
}

/** The most sounds a card takes: the panel sends the first ones, as Kumi's TUI does. */
export function mostFor(card: Pick<DesignCard, 'op'>): number {
  return card.op === 'stack' ? MOST_STACKED : MOST_SOURCES
}

/** What the panel says of a card's sounds: too few to make anything (an i18n key and the least), the first ones only
 *  when there are more than it takes (the key, how many go in, and of how many), or nothing. */
export function cardNote(card: Pick<DesignCard, 'op'>, count: number): { key: 'make.needTwo'; least: number } | { key: 'make.firstOnly'; most: number; of: number } | null {
  if (count < 2) return { key: 'make.needTwo', least: 2 }
  const most = mostFor(card)
  return count > most ? { key: 'make.firstOnly', most, of: count } : null
}

/** The request a card makes from these sounds and options: only the card's own options go (the rest come from the
 *  seed), each inside the engine's range; no seed means a fresh one (a reroll), which the answer names. */
export function requestFor(card: DesignCard, sources: DesignSource[], options: DesignOptions = {}, seed?: number): DesignRequest {
  const kept: DesignOptions = {}
  for (const key of card.options) {
    const value = options[key]
    if (key === 'attack_from') {
      if (typeof value === 'number' && Number.isInteger(value) && value >= 0 && value < sources.length) kept.attack_from = value
    } else if (key !== 'split' && typeof value === 'number' && Number.isFinite(value)) {
      const range = RANGES[key]
      kept[key] = Math.min(range.max, Math.max(range.min, value))
    }
  }
  return {
    op: card.op,
    ...(card.mode ? { mode: card.mode } : {}),
    sources: sources.slice(0, mostFor(card)),
    ...(seed === undefined ? {} : { seed }),
    ...(Object.keys(kept).length ? { options: kept } : {})
  }
}

/** The same take again with another seed: what Reroll sends. The engine picks the seed and names it. */
export function reroll(request: DesignRequest): DesignRequest {
  const { seed: _seed, ...rest } = request
  return rest
}

/** Which take a result belongs to: its op, mode and sounds (rerolls of one take share it). */
export function takeKey(result: Pick<DesignResult, 'op' | 'mode' | 'sources'>): string {
  return JSON.stringify([result.op, result.mode ?? null, result.sources.map(source => source.sound_id ?? source.artifact_id ?? '')])
}

/** A request from an earlier answer (`GET /designs`), to reroll it: its options weren't kept, so the engine draws them. */
export function requestOf(result: DesignResult): DesignRequest {
  return { op: result.op, ...(result.mode ? { mode: result.mode } : {}), sources: result.sources.map(source => ({ ...source })) }
}

/** A result's name, as the palette names it once kept: its recipe's lead (up to the first `:` or `;`, "Blend of 3")
 *  and its seed, so two variations of one take tell apart ("Blend of 3, seed 482113"). */
export function resultName(result: Pick<DesignResult, 'recipe' | 'mode' | 'op' | 'seed'>): string {
  const lead = quotable(result.recipe.split(/[:;]/, 1)[0] ?? '', 60) || quotable(result.mode ?? result.op, 60).replace(/^\w/, letter => letter.toUpperCase())
  return Number.isInteger(result.seed) ? `${lead}, seed ${result.seed}` : lead
}

/** Left unsent in the chat, like Use…: a result by its artifact id, so the model can hand `{artifact_id}` to a file step
 *  on a pad or a track. Null for an id the runtime doesn't write. */
export function useRequest(result: Pick<DesignResult, 'artifact_id' | 'recipe' | 'mode' | 'op' | 'seed'>): string | null {
  const id = plainId(result.artifact_id)
  if (!id) return null
  const said = resultName(result)
  return said ? `Put “${said}” (artifact ${id}) on a pad or track in my project` : `Put artifact ${id} on a pad or track in my project`
}

/** The sounds the panel makes from: picks in map order, or the palette's playable items, as the engine names them. */
export function sourcesOf(items: { sound_id?: string | null; artifact_id?: string | null; name?: string | null }[], revision: string | null): DesignSource[] {
  const out: DesignSource[] = []
  for (const item of items) {
    const name = quotable(item.name ?? '', 120) || undefined
    if (item.sound_id && revision) out.push({ sound_id: item.sound_id, map_revision: revision, ...(name ? { name } : {}) })
    else if (item.artifact_id) out.push({ artifact_id: item.artifact_id, ...(name ? { name } : {}) })
  }
  return out
}

/** An answer as the panel can draw it, or null: an id, a seed, a recipe, a duration and 256 peaks, each in range. */
export function resultOf(value: unknown): DesignResult | null {
  if (!value || typeof value !== 'object') return null
  const raw = value as Record<string, unknown>
  const id = typeof raw.artifact_id === 'string' ? plainId(raw.artifact_id) : null
  const op = raw.op === 'cross' || raw.op === 'fuse' || raw.op === 'stack' ? raw.op : null
  if (!id || !op || typeof raw.seed !== 'number' || !Number.isInteger(raw.seed)) return null
  const peaks = Array.isArray(raw.peaks) ? raw.peaks.map(peak => (typeof peak === 'number' && Number.isFinite(peak) ? Math.min(1, Math.max(0, peak)) : 0)) : []
  const sources = Array.isArray(raw.sources) ? (raw.sources.filter(source => source && typeof source === 'object') as DesignSource[]) : []
  const modes = CARDS.map(card => card.mode)
  return {
    artifact_id: id,
    op,
    ...(typeof raw.mode === 'string' && modes.includes(raw.mode as DesignMode) ? { mode: raw.mode as DesignMode } : {}),
    seed: raw.seed,
    sources,
    duration: typeof raw.duration === 'number' && Number.isFinite(raw.duration) ? raw.duration : 0,
    sample_rate: typeof raw.sample_rate === 'number' ? raw.sample_rate : 48_000,
    channels: typeof raw.channels === 'number' ? raw.channels : 2,
    peaks,
    recipe: typeof raw.recipe === 'string' ? raw.recipe : '',
    ...(typeof raw.at === 'number' ? { at: raw.at } : {})
  }
}
