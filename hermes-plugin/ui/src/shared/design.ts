// Sound design as the map's Make panel uses it: a new sound crossed, fused, looped or stacked from two or more sounds of
// the map or the palette (no upper limit: the engine reads each one shorter when there are many), or grown part of the
// way from one sound toward another, by Kumi's engine (`POST /design`, the contract's "engine's interface"). Every
// option is optional: one left unset is drawn from the seed, so Reroll varies it. Plain data here, no DOM: tested in
// ui/tests.
import { artifactReference, plainId, quotable } from './format.ts'

export type DesignOp = 'cross' | 'fuse' | 'grow' | 'stack'
export type DesignMode = 'attack' | 'blend' | 'loop' | 'modulate' | 'resonate' | 'smear' | 'texture'

/** A sound the engine takes, by id only: a map sound with its revision, or a file Kumi made (an earlier result). */
export interface DesignSource {
  sound_id?: string
  map_revision?: string
  artifact_id?: string
  name?: string
}

/** A slider's span: the engine's range, the step it moves by, and the value that changes nothing (`neutral`), where
 *  there's one. */
export interface Span {
  min: number
  max: number
  step: number
  neutral?: number
}

const UNIT: Span = { min: 0, max: 1, step: 0.05 }
const BIPOLAR: Span = { min: -1, max: 1, step: 0.05, neutral: 0 }

// Resonate (A4): who rings through whom, what rings, and how long.
export const PAIRINGS = ['halves', 'in-order', 'interleave', 'bright-dark', 'dark-bright', 'hits-tones', 'tones-hits', 'one-all', 'all-one', 'chain'] as const
export const RINGS = ['partials', 'body', 'both'] as const
export type Pairing = (typeof PAIRINGS)[number]
export type Ring = (typeof RINGS)[number]

export interface ResonateOptions {
  pairing?: Pairing
  ring?: Ring
  decay?: number
}

// Modulate (A5): which sound is played and shaped, which stages may shape it, how many, and how often a short one
// repeats along it.
export const CARRIERS = ['random', 'first', 'last', 'brightest', 'darkest', 'longest', 'shortest', 'tonal', 'noisy'] as const
export const MOD_KINDS = ['phase', 'ring', 'am', 'fm', 'filter', 'gain', 'vocode'] as const
export type Carrier = (typeof CARRIERS)[number]
export type ModKind = (typeof MOD_KINDS)[number]

export interface ModulateOptions {
  carrier?: Carrier
  kinds?: ModKind[]
  stages?: number
  retrigger?: number
}

export const MODULATE_SPANS = { stages: { min: 1, max: 8, step: 1 }, retrigger: UNIT } satisfies Record<string, Span>
export const RESONATE_SPANS = { decay: UNIT } satisfies Record<string, Span>

// Loop (A6): a beat or a melody out of the picks, in time. Rhythm applies to both styles; the voices and the beat's
// feel to a beat; key, scale, tuning and the line to a melody.
export const LOOP_STYLES = ['beat', 'melody'] as const
export const GRIDS = [2, 3, 4, 6, 8] as const
export const METER_UNITS = [2, 4, 8, 16] as const
export const KEYS = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'] as const
export const SCALES = [
  'major',
  'minor',
  'dorian',
  'phrygian',
  'lydian',
  'mixolydian',
  'locrian',
  'harmonic-minor',
  'melodic-minor',
  'pentatonic',
  'minor-pentatonic',
  'blues',
  'whole-tone',
  'chromatic',
  'hirajoshi',
  'in-sen',
  'phrygian-dominant',
  'hungarian-minor',
  'pelog',
  'slendro'
] as const
export const TUNINGS = ['12-tet', '19-tet', '22-tet', '24-tet', '31-tet', 'just', 'pythagorean', 'bohlen-pierce', 'harmonic'] as const
export const CONTOURS = ['up', 'down', 'up-down', 'walk', 'leaps', 'random'] as const
export type LoopStyle = (typeof LOOP_STYLES)[number]
export type Grid = (typeof GRIDS)[number]
export type MusicKey = (typeof KEYS)[number]
export type Scale = (typeof SCALES)[number]
export type Tuning = (typeof TUNINGS)[number]
export type Contour = (typeof CONTOURS)[number]

/** The loop's sliders: the engine's ranges. */
export const LOOP_SPANS = {
  density: UNIT,
  syncopation: UNIT,
  lean: BIPOLAR,
  fills: UNIT,
  ratchet: UNIT,
  poly: UNIT,
  variation: UNIT,
  kick: UNIT,
  snare: UNIT,
  hats: UNIT,
  perc: UNIT,
  open: UNIT,
  ghosts: UNIT,
  swing: { min: 0, max: 0.75, step: 0.05 },
  humanize: UNIT,
  accent: UNIT,
  micro: UNIT,
  chords: UNIT,
  drone: UNIT,
  octaves: { min: 1, max: 4, step: 1 },
  register: { min: -2, max: 2, step: 1, neutral: 0 },
  glide: UNIT,
  gate: { min: 0.05, max: 1, step: 0.05 }
} satisfies Record<string, Span>
export type LoopNumber = keyof typeof LOOP_SPANS

export type LoopOptions = {
  style?: LoopStyle
  /** `[beats, unit]`: `[7, 8]` is 7/8. */
  meter?: [number, number]
  grid?: Grid
  key?: MusicKey
  scale?: Scale
  tuning?: Tuning
  contour?: Contour
} & Partial<Record<LoopNumber, number>>

/** The loop's controls, grouped as the panel shows them, with the style each belongs to (none: both). `meter`, `grid`,
 *  `key`, `scale`, `tuning` and `contour` are choices; the rest sliders. */
export type LoopControl = LoopNumber | 'contour' | 'grid' | 'key' | 'meter' | 'scale' | 'tuning'
export const LOOP_GROUPS: { key: 'feel' | 'harmony' | 'line' | 'rhythm' | 'voices'; style?: LoopStyle; controls: LoopControl[] }[] = [
  { key: 'rhythm', controls: ['meter', 'grid', 'density', 'syncopation', 'lean', 'fills', 'ratchet', 'poly', 'variation'] },
  { key: 'voices', style: 'beat', controls: ['kick', 'snare', 'hats', 'perc', 'open', 'ghosts'] },
  { key: 'feel', controls: ['swing', 'humanize', 'accent'] },
  { key: 'harmony', style: 'melody', controls: ['key', 'scale', 'tuning', 'micro', 'chords', 'drone'] },
  { key: 'line', style: 'melody', controls: ['contour', 'octaves', 'register', 'glide', 'gate'] }
]
/** The rhythm's controls that only a beat takes (a melody leaves them out). */
const BEAT_ONLY: readonly LoopControl[] = ['syncopation', 'lean']

/** Which style a loop control belongs to: a group's, or a beat's for the rhythm's beat-only ones; none for both. */
export function loopStyleOf(control: LoopControl): LoopStyle | undefined {
  if (BEAT_ONLY.includes(control)) return 'beat'
  return LOOP_GROUPS.find(group => group.controls.includes(control))?.style
}

// Steer (A1): on every op and mode. The result's own controls, then each source's, which every source draws within
// from the seed ("each layer rolls its own").
export const STEER_SOUND = {
  bright: BIPOLAR,
  body: BIPOLAR,
  filter: BIPOLAR,
  resonance: UNIT,
  noise: BIPOLAR,
  punch: BIPOLAR,
  length: BIPOLAR,
  drive: UNIT,
  crush: UNIT,
  width: BIPOLAR,
  space: UNIT
} satisfies Record<string, Span>
export const STEER_LAYER = {
  pitch: { min: -24, max: 24, step: 1, neutral: 0 },
  reverse: UNIT,
  start: { min: 0, max: 0.9, step: 0.05, neutral: 0 },
  delay: { min: 0, max: 500, step: 5, neutral: 0 },
  pan: UNIT,
  level: UNIT
} satisfies Record<string, Span>
/** Each source's controls that take a `[low, high]` range; the rest take one number. */
export const STEER_RANGES = ['pitch', 'start', 'delay'] as const
export type SteerSound = keyof typeof STEER_SOUND
export type SteerLayer = keyof typeof STEER_LAYER
export type SteerRange = (typeof STEER_RANGES)[number]
export type Steer = Partial<Record<SteerSound | Exclude<SteerLayer, SteerRange>, number>> & Partial<Record<SteerRange, [number, number]>>

export interface DesignOptions {
  seconds?: number
  bpm?: number
  bars?: number
  attack_from?: number
  split?: number[]
  depth?: number
  stretch?: number
  /** Grow: how far from the first sound toward the second, 0 to 1. */
  at?: number
  resonate?: ResonateOptions
  modulate?: ModulateOptions
  loop?: LoopOptions
  steer?: Steer
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

/** The top-level numbers a card shows as sliders, and `attack_from`, Attack's choice of a sound. */
export type CardOption = 'attack_from' | 'bars' | 'bpm' | 'depth' | 'seconds' | 'stretch'

/** The panel's tabs: cross's modes, fuse's, the loop (fuse's loop mode, on a tab of its own) and stack. */
export const TABS = ['cross', 'fuse', 'loop', 'stack'] as const
export type DesignTab = (typeof TABS)[number]

/** One card of the Make panel: its tab, the op and mode it sends, and the top-level options it shows. */
export interface DesignCard {
  key: string
  tab: DesignTab
  op: DesignOp
  mode?: DesignMode
  /** The options the card shows; the engine draws any left out from the seed. */
  options: CardOption[]
}

/** The cards, in the panel's order. */
export const CARDS: DesignCard[] = [
  { key: 'blend', tab: 'cross', op: 'cross', mode: 'blend', options: ['seconds'] },
  { key: 'attack', tab: 'cross', op: 'cross', mode: 'attack', options: ['attack_from'] },
  { key: 'resonate', tab: 'cross', op: 'cross', mode: 'resonate', options: ['depth'] },
  { key: 'modulate', tab: 'cross', op: 'cross', mode: 'modulate', options: ['depth'] },
  { key: 'texture', tab: 'fuse', op: 'fuse', mode: 'texture', options: ['seconds', 'depth'] },
  { key: 'smear', tab: 'fuse', op: 'fuse', mode: 'smear', options: ['stretch', 'depth'] },
  { key: 'loop', tab: 'loop', op: 'fuse', mode: 'loop', options: ['bpm', 'bars'] },
  { key: 'stack', tab: 'stack', op: 'stack', options: [] }
]

/** The ranges the top-level options take (the engine's own), and the step a slider moves by. */
export const RANGES: Record<Exclude<CardOption, 'attack_from'> | 'at', Span> = {
  seconds: { min: 0.5, max: 30, step: 0.5 },
  bpm: { min: 40, max: 240, step: 1 },
  bars: { min: 1, max: 8, step: 1 },
  depth: UNIT,
  stretch: { min: 4, max: 16, step: 1 },
  at: { min: 0, max: 1, step: 0.01 }
}

/** The engine's tempo when the panel knows none (A6): a loop is filled from the DAW's tempo otherwise. */
export const DEFAULT_BPM = 120

/** What the panel says of a card's sounds: too few to make anything (an i18n key and the least), or nothing. Every
 *  sound picked goes in, however many. */
export function cardNote(count: number): { key: 'make.needTwo'; least: number } | null {
  return count < 2 ? { key: 'make.needTwo', least: 2 } : null
}

const clampTo = (span: Span, value: number): number => Math.min(span.max, Math.max(span.min, value))
const isNumber = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value)

/** A slider's value as the engine takes it: inside its span, or nothing for a value that isn't a number. */
export function spanned(span: Span, value: unknown): number | undefined {
  return isNumber(value) ? clampTo(span, value) : undefined
}

/** A `[low, high]` range as the engine takes it: each end inside the span, low no higher than high. */
export function ranged(span: Span, value: unknown): [number, number] | undefined {
  if (!Array.isArray(value) || value.length !== 2 || !isNumber(value[0]) || !isNumber(value[1])) return undefined
  const [low, high] = [clampTo(span, value[0]), clampTo(span, value[1])]
  return low <= high ? [low, high] : [high, low]
}

const pick = <T extends string>(choices: readonly T[], value: unknown): T | undefined => (choices.includes(value as T) ? (value as T) : undefined)
const compact = <T extends object>(value: T): T | undefined => (Object.keys(value).length > 0 ? value : undefined)

/** Resonate's options as sent: only those set, each a choice it takes or inside its span. */
export function resonateOf(options: ResonateOptions | undefined): ResonateOptions | undefined {
  if (!options) return undefined
  const out: ResonateOptions = {}
  const pairing = pick(PAIRINGS, options.pairing)
  const ring = pick(RINGS, options.ring)
  const decay = spanned(RESONATE_SPANS.decay, options.decay)
  if (pairing) out.pairing = pairing
  if (ring) out.ring = ring
  if (decay !== undefined) out.decay = decay
  return compact(out)
}

/** Modulate's options as sent: the kinds as a list of known ones in the engine's order (none chosen is all, at the
 *  engine's odds: left out), the stages a whole number. */
export function modulateOf(options: ModulateOptions | undefined): ModulateOptions | undefined {
  if (!options) return undefined
  const out: ModulateOptions = {}
  const carrier = pick(CARRIERS, options.carrier)
  const kinds = Array.isArray(options.kinds) ? MOD_KINDS.filter(kind => options.kinds?.includes(kind)) : []
  const stages = spanned(MODULATE_SPANS.stages, options.stages)
  const retrigger = spanned(MODULATE_SPANS.retrigger, options.retrigger)
  if (carrier) out.carrier = carrier
  if (kinds.length > 0) out.kinds = kinds
  if (stages !== undefined) out.stages = Math.round(stages)
  if (retrigger !== undefined) out.retrigger = retrigger
  return compact(out)
}

/** The loop's options as sent: its style, always, and only the controls set that belong to that style (a melody's key
 *  goes with a melody, a beat's voices with a beat), each a choice it takes or inside its span. */
export function loopOf(options: LoopOptions | undefined): LoopOptions {
  const style: LoopStyle = options?.style === 'melody' ? 'melody' : 'beat'
  const out: LoopOptions = { style }
  if (!options) return out
  const belongs = (control: LoopControl): boolean => {
    const owner = loopStyleOf(control)
    return owner === undefined || owner === style
  }
  const meter = options.meter
  if (Array.isArray(meter) && Number.isInteger(meter[0]) && meter[0] >= 1 && meter[0] <= 16 && METER_UNITS.includes(meter[1] as (typeof METER_UNITS)[number])) out.meter = [meter[0], meter[1]]
  const grid = GRIDS.find(value => value === options.grid)
  if (grid !== undefined) out.grid = grid
  for (const [control, choices] of [
    ['key', KEYS],
    ['scale', SCALES],
    ['tuning', TUNINGS],
    ['contour', CONTOURS]
  ] as const) {
    const value = pick<string>(choices, options[control])
    if (value && belongs(control)) Object.assign(out, { [control]: value })
  }
  for (const control of Object.keys(LOOP_SPANS) as LoopNumber[]) {
    const value = spanned(LOOP_SPANS[control], options[control])
    if (value !== undefined && belongs(control)) out[control] = LOOP_SPANS[control].step === 1 ? Math.round(value) : value
  }
  return out
}

/** The steer as sent: only what moves the sound (a value at its neutral, or a range of nothing but the neutral, changes
 *  nothing and is left out), each inside its span; nothing at all when nothing is steered. */
export function steerOf(steer: Steer | undefined): Steer | undefined {
  if (!steer) return undefined
  const out: Steer = {}
  for (const [key, span] of Object.entries({ ...STEER_SOUND, ...STEER_LAYER }) as [SteerSound | SteerLayer, Span][]) {
    if ((STEER_RANGES as readonly string[]).includes(key)) {
      const range = ranged(span, steer[key as SteerRange])
      if (range && !(range[0] === (span.neutral ?? span.min) && range[1] === (span.neutral ?? span.min))) out[key as SteerRange] = range
    } else {
      const value = spanned(span, steer[key as Exclude<SteerLayer, SteerRange> | SteerSound])
      if (value !== undefined && value !== (span.neutral ?? span.min)) out[key as Exclude<SteerLayer, SteerRange> | SteerSound] = value
    }
  }
  return compact(out)
}

/** How many of the steer's controls move the sound: the count the panel shows beside "Steer". */
export function steerCount(steer: Steer | undefined): number {
  return Object.keys(steerOf(steer) ?? {}).length
}

/** A surprise: three to five of the steer's controls, each moved a good way from neutral, the rest left alone. `random`
 *  returns numbers in [0, 1), as Math.random does (a test passes its own). */
export function rollSteer(random: () => number = Math.random): Steer {
  const keys = [...Object.keys(STEER_SOUND), ...Object.keys(STEER_LAYER)] as (SteerSound | SteerLayer)[]
  const count = 3 + Math.floor(random() * 3)
  const chosen: (SteerSound | SteerLayer)[] = []
  while (chosen.length < count) {
    const key = keys[Math.floor(random() * keys.length)]
    if (!chosen.includes(key)) chosen.push(key)
  }
  const out: Steer = {}
  for (const key of chosen) {
    const span: Span = key in STEER_SOUND ? STEER_SOUND[key as SteerSound] : STEER_LAYER[key as SteerLayer]
    const snap = (value: number) => Math.round(clampTo(span, value) / span.step) * span.step
    if ((STEER_RANGES as readonly string[]).includes(key)) {
      // A range around the neutral, a moderate way out (pitch 4 to 12 semitones each side), never past the span.
      const middle = span.neutral ?? span.min
      const reach = (span.max - span.min) * (0.08 + random() * 0.17)
      const low = snap(middle === span.min ? span.min + reach * random() * 0.3 : middle - reach * random())
      const high = snap(middle === span.min ? low + reach : middle + reach * random())
      out[key as SteerRange] = [Math.min(low, high), Math.max(low, high)]
    } else if (span.neutral === 0 && span.min < 0) {
      // A bipolar control, either way, at least a fifth of the way out.
      const value = (0.2 + random() * 0.6) * (random() < 0.5 ? -1 : 1)
      out[key as SteerSound] = snap(value)
    } else {
      out[key as SteerSound] = snap(span.min + (span.max - span.min) * (0.2 + random() * 0.5))
    }
  }
  return out
}

/** The request a card makes from these sounds and options: only the card's own options go (the rest come from the
 *  seed), each inside the engine's range, and the steer on any card; no seed means a fresh one (a reroll), which the
 *  answer names. */
export function requestFor(card: DesignCard, sources: DesignSource[], options: DesignOptions = {}, seed?: number): DesignRequest {
  const kept: DesignOptions = {}
  for (const key of card.options) {
    const value = options[key]
    if (key === 'attack_from') {
      if (typeof value === 'number' && Number.isInteger(value) && value >= 0 && value < sources.length) kept.attack_from = value
    } else {
      const span = spanned(RANGES[key], value)
      if (span !== undefined) kept[key] = RANGES[key].step === 1 ? Math.round(span) : span
    }
  }
  const resonate = card.key === 'resonate' ? resonateOf(options.resonate) : undefined
  const modulate = card.key === 'modulate' ? modulateOf(options.modulate) : undefined
  if (resonate) kept.resonate = resonate
  if (modulate) kept.modulate = modulate
  if (card.key === 'loop') kept.loop = loopOf(options.loop)
  const steer = steerOf(options.steer)
  if (steer) kept.steer = steer
  return {
    op: card.op,
    ...(card.mode ? { mode: card.mode } : {}),
    sources,
    ...(seed === undefined ? {} : { seed }),
    ...(Object.keys(kept).length ? { options: kept } : {})
  }
}

/** The request for one point along a path from `from` toward `to` (A2): exactly two sounds, no mode, the point `at`
 *  (0 sounds like `from`, 1 like `to`), the path's seed (one per path: a new seed is a new path), and the steer. */
export function growRequest(from: DesignSource, to: DesignSource, at: number, seed?: number, steer?: Steer): DesignRequest {
  const kept: DesignOptions = { at: spanned(RANGES.at, at) ?? 0.5 }
  const steered = steerOf(steer)
  if (steered) kept.steer = steered
  return { op: 'grow', sources: [from, to], ...(seed === undefined ? {} : { seed }), options: kept }
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

/** A result as a source for another make: by its id, under the name the palette would give it. */
export function sourceOfResult(result: Pick<DesignResult, 'artifact_id' | 'recipe' | 'mode' | 'op' | 'seed'>): DesignSource {
  return { artifact_id: result.artifact_id, name: resultName(result) }
}

/** The stages a modulate result's recipe names ("ring by Kick 02 → low-pass filter follows Snare 4"), in order: one
 *  each, so the panel shows how the sounds were modulated. Empty for a recipe that names none. */
export function stagesOf(recipe: string): string[] {
  const body = recipe.includes(':') ? recipe.slice(recipe.indexOf(':') + 1) : recipe
  const stages = body
    .split(/\s*(?:→|->)\s*/)
    .map(stage => stage.replace(/[;·].*$/, '').trim())
    .filter(Boolean)
  return stages.length > 1 ? stages : []
}

/** What Use… puts in the input, unsent: the result's reference and nothing else (its name as the palette gives it, then
 *  its artifact id), so the producer says what to do with it; the model can hand `{artifact_id}` to a file step. Null
 *  for an id the runtime doesn't write. */
export function useRequest(result: Pick<DesignResult, 'artifact_id' | 'recipe' | 'mode' | 'op' | 'seed'>): string | null {
  return artifactReference(result.artifact_id, resultName(result))
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
  const op = raw.op === 'cross' || raw.op === 'fuse' || raw.op === 'stack' || raw.op === 'grow' ? raw.op : null
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
