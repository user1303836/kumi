// What the listen card shows for a `kumi_listen` result: the measured numbers, side by side with the reference when
// there is one, and the differences Kumi computed. Only measurements: the card never says anything was heard.
import { clean, hz, num, signed } from './format.ts'
import { english, type Translate } from './i18n.ts'
import type { Analysis, ListenReference, ListenResult, SampleWindow } from './types.ts'

export interface MetricRow {
  label: string
  mine: string
  reference?: string
  delta?: string
  /** -1 when the difference reads as darker, quieter or lower; 1 for the opposite; 0 when there's none to show. */
  direction: -1 | 0 | 1
}

export interface ListenView {
  title: string
  referenceTitle?: string
  mineId?: string
  referenceId?: string
  rows: MetricRow[]
  bands: { name: string; value: number }[]
  /** `difference` when the bands are current minus reference (level-normalized), `level` when they're one file's. */
  bandsAre: 'difference' | 'level'
  /** With a reference: both files' band levels, each a share of its own file's energy, so their shapes overlay
   *  level-matched. Empty when either file lacks them or their bands don't line up. */
  overlay: { name: string; hz: string; mine: number; reference: number }[]
  headlines: string[]
  verdict?: string
  interpretation?: { label: string; text: string }
  interpretationError?: string
  /** How a track or the mix was captured (CC-2): never a change, nothing to undo. */
  capture?: string
  /** The runtime's one-line note about the capture, as plain untrusted text. */
  captureNote?: string
}

/** How far a comparison is from its reference: current minus reference, where measured. A mastering pass's rounds
 *  show these narrowing. */
export interface Gaps {
  loudness?: number
  tilt?: number
  width?: number
}

// A track listen that didn't complete (CC-2): no measurements, no file, nothing the card could draw.
const NOT_MEASURED: Record<string, true> = { preview: true, refused: true, cancelled: true, unsure: true }

/** A tool result, as the transcript hands it over: the object, or the JSON string the handler returned. Only a result
 *  with measurements is the card's; anything else stays Desktop's to show. */
export function parseListen(result: unknown): ListenResult | null {
  let value = result
  if (typeof value === 'string') {
    try {
      value = JSON.parse(value)
    } catch {
      return null
    }
  }
  if (!value || typeof value !== 'object' || !('metrics' in value) || !value.metrics || typeof value.metrics !== 'object') return null
  const state = 'state' in value ? value.state : undefined
  if (typeof state === 'string' && NOT_MEASURED[state]) return null
  return value as ListenResult
}

// How a track was captured (catalog keys).
const CAPTURED_BY: Record<string, string> = { ears: 'listen.ears', offline: 'listen.offline' }

/** `Beats 17–25 · 7.9 s, through Kumi Ears · put back as it was` for a track capture, or nothing. */
export function captureWords(capture: ListenResult['capture'], t: Translate = english): string | undefined {
  if (!capture || typeof capture !== 'object') return undefined
  const parts: string[] = []
  const from = capture.from_beat
  const beats = capture.beats
  if (finite(from) && finite(beats)) parts.push(t('listen.beats', num(from, 2), num(from + beats, 2)))
  if (finite(capture.seconds)) parts.push(t('listen.secondsMeasured', num(capture.seconds, 1)))
  const method = typeof capture.method === 'string' ? capture.method : ''
  const by = method ? (CAPTURED_BY[method] ? t(CAPTURED_BY[method]) : clean(method, 20)) : ''
  const said = [parts.join(' · '), by && t('listen.through', by)].filter(Boolean).join(', ')
  if (!said) return undefined
  return capture.cleanup === 'restored' ? `${said} · ${t('listen.restored')}` : said
}

const finite = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value)

function fileName(analysis: Analysis | undefined, fallback: string): string {
  const file = typeof analysis?.file === 'string' ? analysis.file.split(/[\\/]/).pop() : undefined
  return clean(file || fallback, 60)
}

function row(label: string, mine: unknown, reference: unknown, show: (value: number) => string, delta?: unknown, deltaShow?: (value: number) => string): MetricRow | null {
  if (!finite(mine) && !finite(reference)) return null
  const out: MetricRow = { label, mine: finite(mine) ? show(mine) : '—', direction: 0 }
  if (reference !== undefined) {
    out.reference = finite(reference) ? show(reference) : '—'
    const difference = finite(delta) ? delta : finite(mine) && finite(reference) ? mine - reference : undefined
    if (difference !== undefined) {
      out.delta = (deltaShow ?? show)(difference)
      out.direction = difference < 0 ? -1 : difference > 0 ? 1 : 0
    }
  }
  return out
}

/** `referenceName`: the name the reference shows under when this window knows a better one than the result gives
 *  (`referenceName()`: a kept copy's, which Kumi measures under its own `<id>.wav`). */
export function listenView(result: ListenResult, t: Translate = english, referenceName?: string): ListenView {
  const mine = result.metrics ?? {}
  const compare = result.compare
  const reference = compare?.metrics
  const differences = compare?.differences
  // With a reference, a value it lacks shows as a dash; without one, there's no reference column at all.
  const theirs = (value: unknown): unknown => (compare ? (value ?? null) : undefined)
  const rows = [
    row(t('listen.loudness'), mine.loudness?.integratedLufs, theirs(reference?.loudness?.integratedLufs), v => `${num(v)} LUFS`, differences?.loudness?.differenceLu, v => `${signed(v)} LU`),
    row(t('listen.truePeak'), mine.loudness?.truePeakDbtp, theirs(reference?.loudness?.truePeakDbtp), v => `${num(v)} dBTP`, undefined, v => `${signed(v)} dB`),
    row(t('listen.range'), mine.loudness?.rangeLu, theirs(reference?.loudness?.rangeLu), v => `${num(v)} LU`, undefined, v => `${signed(v)} LU`),
    row(t('listen.crest'), mine.dynamics?.crestDb, theirs(reference?.dynamics?.crestDb), v => `${num(v)} dB`, undefined, v => `${signed(v)} dB`),
    row(t('listen.peakToLoudness'), mine.dynamics?.peakToLoudnessDb, theirs(reference?.dynamics?.peakToLoudnessDb), v => `${num(v)} dB`, undefined, v => `${signed(v)} dB`),
    row(t('listen.tilt'), mine.balance?.tiltDbPerOctave, theirs(reference?.balance?.tiltDbPerOctave), v => t('listen.perOctave', num(v)), undefined, v => t('listen.perOctave', signed(v))),
    row(t('listen.brightness'), mine.balance?.centroidHz, theirs(reference?.balance?.centroidHz), hz, undefined, v => `${signed(v, 0)} Hz`),
    row(t('listen.width'), mine.stereo?.width, theirs(reference?.stereo?.width), v => num(v, 2), undefined, v => signed(v, 2)),
    lowEnd(t, mine.stereo?.lowEndMono, compare ? (reference?.stereo?.lowEndMono ?? null) : undefined),
    row(t('listen.tempo'), mine.tempo?.bpm, theirs(reference?.tempo?.bpm), v => `${num(v)} BPM`, undefined, v => `${signed(v)} BPM`)
  ].filter((value): value is MetricRow => value !== null)
  // A key is a passage's: one sound (a snare, a one-shot) has none worth naming, whatever the estimator returned.
  const key = mine.sound ? undefined : mine.key?.name
  if (typeof key === 'string' && key) {
    const otherKey = reference?.key?.name
    rows.push({ label: t('listen.key'), mine: clean(key, 20), ...(compare ? { reference: typeof otherKey === 'string' && otherKey ? clean(otherKey, 20) : '—' } : {}), direction: 0 })
  }
  const differenceBands = (Array.isArray(differences?.balance) ? differences.balance : []).flatMap(band =>
    band && finite(band.difference) ? [{ name: clean(band.band ?? band.hz ?? '', 16), value: band.difference }] : []
  )
  const levelBands = (Array.isArray(mine.balance?.bands) ? mine.balance.bands : []).flatMap(band =>
    band && finite(band.db) ? [{ name: clean(band.name ?? band.hz ?? '', 16), value: band.db }] : []
  )
  const headlines = Array.isArray(differences?.headlines) ? differences.headlines.filter(line => typeof line === 'string').map(line => clean(line, 140)) : []
  // The runtime's verdict is its summary line, then the headlines (serve's `retain`): the card lists those, so with
  // them the verdict is the summary alone, rather than every difference said twice.
  const said = typeof result.verdict_text === 'string' ? (headlines.length ? result.verdict_text.split('\n')[0] : result.verdict_text) : ''
  const view: ListenView = {
    title: fileName(mine, t('listen.measuredAudio')),
    mineId: typeof result.artifact_id === 'string' ? result.artifact_id : undefined,
    rows,
    bands: differenceBands.length ? differenceBands : levelBands,
    bandsAre: differenceBands.length ? 'difference' : 'level',
    overlay: compare ? overlayOf(mine, reference) : [],
    headlines,
    verdict: said.trim() ? clean(said, 400) : undefined,
    capture: captureWords(result.capture, t),
    // CC-2 A3: one line of at most 200 characters, shown as it is and never read for a decision.
    captureNote: typeof result.capture?.note === 'string' && result.capture.note.trim() ? clean(result.capture.note, 200) : undefined
  }
  if (compare) {
    view.referenceTitle = compare.name ? displayReferenceName(compare) : referenceName ? clean(referenceName, 60) : fileName(reference, t('listen.reference'))
    view.referenceId = typeof compare.artifact_id === 'string' ? compare.artifact_id : undefined
  }
  const interpretation = result.interpretation
  if (interpretation && typeof interpretation.text === 'string' && interpretation.text.trim()) {
    // The text opens with the runtime's own trusted line saying what the model was given: the heading doesn't repeat it.
    const model = typeof interpretation.model === 'string' && interpretation.model ? ` · ${clean(interpretation.model, 40)}` : ''
    view.interpretation = { label: `${t('listen.interpretation')}${model}`, text: interpretation.text.trim().slice(0, 4000) }
  } else if (typeof result.interpretation_error === 'string' && result.interpretation_error) {
    view.interpretationError = clean(result.interpretation_error, 200)
  }
  return view
}

/** Both files' band levels, band by band, when every band lines up by name. */
function overlayOf(mine: Analysis, reference: Analysis | undefined): ListenView['overlay'] {
  const ours = Array.isArray(mine.balance?.bands) ? mine.balance.bands : []
  const theirs = Array.isArray(reference?.balance?.bands) ? reference.balance.bands : []
  if (!ours.length || ours.length !== theirs.length) return []
  const overlay: ListenView['overlay'] = []
  for (const [index, band] of ours.entries()) {
    const other = theirs[index]
    if (!band || !other || band.name !== other.name || !finite(band.db) || !finite(other.db)) return []
    overlay.push({ name: clean(band.name ?? '', 16), hz: clean(band.hz ?? '', 16), mine: band.db, reference: other.db })
  }
  return overlay
}

/** `mono` or `not mono` below about 120 Hz, beside the reference's: no difference to compute. */
function lowEnd(t: Translate, mine: unknown, reference: unknown): MetricRow | null {
  const words = (value: unknown) => (value === true ? t('listen.mono') : value === false ? t('listen.notMono') : '—')
  if (typeof mine !== 'boolean' && typeof reference !== 'boolean') return null
  return { label: t('listen.lowEnd'), mine: words(mine), ...(reference === undefined ? {} : { reference: words(reference) }), direction: 0 }
}

/** Native names are untrusted text. Never expose a producer path as a display name. */
export function displayReferenceName(reference: ListenReference): string {
  return clean((reference.origin?.name ?? reference.name ?? '').split(/[\\/]/).pop() || 'Reference', 160)
}

/** Exact measured seconds, not the original file's Analysis.seconds or a source capture's beats. */
export function windowWords(window: SampleWindow | null | undefined): string | undefined {
  if (!window || !finite(window.sample_rate) || window.sample_rate <= 0 || !finite(window.frame_count) || !finite(window.start_frame)) return undefined
  const start = window.start_frame / window.sample_rate
  const duration = window.frame_count / window.sample_rate
  const requested = window.requested ? ` · requested ${num(window.requested.from, 6)} + ${num(window.requested.seconds, 6)} s` : ''
  return `Reference file seconds ${num(start, 6)}–${num(start + duration, 6)} · ${num(duration, 6)} s measured (${window.frame_count} frames at ${window.sample_rate} Hz)${requested}`
}

// Sort object keys as well as the equal-weight member set; native object insertion order is not identity.
function stable(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stable).join(',')}]`
  if (value && typeof value === 'object') return `{${Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([key, item]) => `${JSON.stringify(key)}:${stable(item)}`).join(',')}}`
  return JSON.stringify(value) ?? 'null'
}

function memberKey(reference: ListenReference): string {
  const window = reference.window
  const origin = reference.origin
  // Only a full offset-zero replay represents the persisted excerpt. A new sub-window must not inherit its key.
  const replay = origin?.window && window && window.start_frame === 0 &&
    window.frame_count === origin.window.frame_count && window.sample_rate === origin.window.sample_rate &&
    window.channels === origin.window.channels
  const selected = replay ? origin : reference
  return stable({
    identity: selected.identity ?? { artifact_id: reference.artifact_id },
    window: selected.window ? {
      start_frame: selected.window.start_frame, frame_count: selected.window.frame_count,
      sample_rate: selected.window.sample_rate, channels: selected.window.channels
    } : null
  })
}

function hasReferenceFacts(result: ListenResult): boolean {
  return Array.isArray(result.references) || !!(result.compare?.identity || result.compare?.window || result.compare?.origin)
}
/** What makes two comparisons ones against the same reference: its file's name and what Kumi measured of it, which
 *  are the same each time the same file is measured (its retained copy gets a new id each time). Null without one. */
export function referenceKey(result: ListenResult): string | null {
  if (Array.isArray(result.references)) return `profile:${stable(result.references.map(memberKey).sort())}`
  if (hasReferenceFacts(result) && result.compare) return `excerpt:${memberKey(result.compare)}`
  const reference = result.compare?.metrics
  if (!reference) return null
  const lufs = reference.loudness?.integratedLufs
  return [reference.file ?? '', finite(lufs) ? lufs.toFixed(1) : '', finite(reference.seconds) ? reference.seconds.toFixed(1) : ''].join('|')
}

/** One comparison of a mastering pass's rounds: its own audio's id, its gaps, the copy of the reference Kumi kept for
 *  it (`compare.artifact_id`), and the name that reference shows under. */
export interface Round {
  id: string
  gaps: Gaps
  kept: string | null
  name: string
}

// From round 2 on, the listening skill compares against the reference's kept copy (`compare_to: {"artifact_id"}`),
// which Kumi measures under the copy's own name (`<id>.wav`) and returns under the same id: the earlier round that kept
// that copy, with the key its rounds go under.
function keptBy(result: ListenResult, rounds: Record<string, Round[]>): [string, Round] | null {
  const kept = result.compare?.artifact_id
  if (typeof kept !== 'string' || !kept) return null
  for (const [key, each] of Object.entries(rounds)) {
    const round = each.find(other => other.kept === kept)
    if (round) return [key, round]
  }
  return null
}

/** The key a comparison's rounds go under: that of the round which kept its reference's copy, when this window saw
 *  it, else `referenceKey`. Null without a reference. */
export function roundKey(result: ListenResult, rounds: Record<string, Round[]>): string | null {
  return hasReferenceFacts(result) ? referenceKey(result) : keptBy(result, rounds)?.[0] ?? referenceKey(result)
}

/** The name a comparison's reference shows under when it was compared by its kept copy: the name the round that kept
 *  the copy showed. Undefined when this window saw no such round. */
export function referenceName(result: ListenResult, rounds: Record<string, Round[]>): string | undefined {
  return keptBy(result, rounds)?.[1].name || undefined
}

/** A comparison as a round, `rounds` being those this window saw before it. */
export function roundOf(result: ListenResult, id: string, rounds: Record<string, Round[]>): Round {
  const kept = result.compare?.artifact_id
  return { id, gaps: gapsOf(result), kept: typeof kept === 'string' && kept ? kept : null, name: result.compare?.name ? displayReferenceName(result.compare) : referenceName(result, rounds) ?? fileName(result.compare?.metrics, '') }
}

/** A comparison's gaps to its reference. */
export function gapsOf(result: ListenResult): Gaps {
  const mine = result.metrics ?? {}
  const reference = result.compare?.metrics
  if (!reference) return {}
  const gap = (a: unknown, b: unknown) => (finite(a) && finite(b) ? Math.round((a - b) * 100) / 100 : undefined)
  const loudness = finite(result.compare?.differences?.loudness?.differenceLu)
    ? result.compare.differences.loudness.differenceLu
    : gap(mine.loudness?.integratedLufs, reference.loudness?.integratedLufs)
  const out: Gaps = {}
  if (loudness !== undefined) out.loudness = loudness
  const tilt = gap(mine.balance?.tiltDbPerOctave, reference.balance?.tiltDbPerOctave)
  if (tilt !== undefined) out.tilt = tilt
  const width = gap(mine.stereo?.width, reference.stereo?.width)
  if (width !== undefined) out.width = width
  return out
}

/** `Round 3 · loudness −4.1 → −1.2 → −0.4 LU · tilt +0.9 → +0.3 → +0.1 dB/oct`: comparison `number` against a
 *  reference and the gaps of up to the last three, oldest first and ending with this one's. Nothing for the first. */
export function roundWords(number: number, rounds: Gaps[], t: Translate = english): string | undefined {
  if (number < 2 || rounds.length < 2) return undefined
  const trend = (pick: (gaps: Gaps) => number | undefined, digits: number) => {
    const values = rounds.map(pick)
    return values.every(finite) ? values.map(value => signed(value as number, digits)).join(' → ') : undefined
  }
  const parts = [
    [trend(gaps => gaps.loudness, 1), (text: string) => t('listen.loudnessGap', text)],
    [trend(gaps => gaps.tilt, 1), (text: string) => t('listen.tiltGap', text)],
    [trend(gaps => gaps.width, 2), (text: string) => t('listen.widthGap', text)]
  ] as const
  const said = parts.flatMap(([text, words]) => (text ? [words(text)] : []))
  return [t('listen.round', number), ...said].join(' · ')
}
