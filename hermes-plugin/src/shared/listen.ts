// What the listen card shows for a `kumi_listen` result: the measured numbers, side by side with the reference when
// there is one, and the differences Kumi computed. Only measurements: the card never says anything was heard.
import { clean, hz, num, signed } from './format.ts'
import type { Analysis, ListenResult } from './types.ts'

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
  headlines: string[]
  verdict?: string
  interpretation?: { label: string; text: string }
  interpretationError?: string
}

/** A tool result, as the transcript hands it over: the object, or the JSON string the handler returned. */
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
  return value as ListenResult
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

export function listenView(result: ListenResult): ListenView {
  const mine = result.metrics ?? {}
  const compare = result.compare
  const reference = compare?.metrics
  const differences = compare?.differences
  // With a reference, a value it lacks shows as a dash; without one, there's no reference column at all.
  const theirs = (value: unknown): unknown => (compare ? (value ?? null) : undefined)
  const rows = [
    row('Loudness', mine.loudness?.integratedLufs, theirs(reference?.loudness?.integratedLufs), v => `${num(v)} LUFS`, differences?.loudness?.differenceLu, v => `${signed(v)} LU`),
    row('True peak', mine.loudness?.truePeakDbtp, theirs(reference?.loudness?.truePeakDbtp), v => `${num(v)} dBTP`, undefined, v => `${signed(v)} dB`),
    row('Brightness (centroid)', mine.balance?.centroidHz, theirs(reference?.balance?.centroidHz), hz, undefined, v => `${signed(v, 0)} Hz`),
    row('Loudness range', mine.loudness?.rangeLu, theirs(reference?.loudness?.rangeLu), v => `${num(v)} LU`, undefined, v => `${signed(v)} LU`),
    row('Crest', mine.dynamics?.crestDb, theirs(reference?.dynamics?.crestDb), v => `${num(v)} dB`, undefined, v => `${signed(v)} dB`),
    row('Tempo', mine.tempo?.bpm, theirs(reference?.tempo?.bpm), v => `${num(v)} BPM`, undefined, v => `${signed(v)} BPM`)
  ].filter((value): value is MetricRow => value !== null)
  const key = mine.key?.name
  if (typeof key === 'string' && key) {
    const otherKey = reference?.key?.name
    rows.push({ label: 'Key', mine: clean(key, 20), ...(compare ? { reference: typeof otherKey === 'string' && otherKey ? clean(otherKey, 20) : '—' } : {}), direction: 0 })
  }
  const differenceBands = (Array.isArray(differences?.balance) ? differences.balance : []).flatMap(band =>
    band && finite(band.difference) ? [{ name: clean(band.band ?? band.hz ?? '', 16), value: band.difference }] : []
  )
  const levelBands = (Array.isArray(mine.balance?.bands) ? mine.balance.bands : []).flatMap(band =>
    band && finite(band.db) ? [{ name: clean(band.name ?? band.hz ?? '', 16), value: band.db }] : []
  )
  const view: ListenView = {
    title: fileName(mine, 'Measured audio'),
    mineId: typeof result.artifact_id === 'string' ? result.artifact_id : undefined,
    rows,
    bands: differenceBands.length ? differenceBands : levelBands,
    bandsAre: differenceBands.length ? 'difference' : 'level',
    headlines: Array.isArray(differences?.headlines) ? differences.headlines.filter(line => typeof line === 'string').map(line => clean(line, 140)) : [],
    verdict: typeof result.verdict_text === 'string' && result.verdict_text ? clean(result.verdict_text, 400) : undefined
  }
  if (compare) {
    view.referenceTitle = fileName(reference, 'Reference')
    view.referenceId = typeof compare.artifact_id === 'string' ? compare.artifact_id : undefined
  }
  const interpretation = result.interpretation
  if (interpretation && typeof interpretation.text === 'string' && interpretation.text.trim()) {
    const inputs = Array.isArray(interpretation.inputs) ? interpretation.inputs : []
    const what = inputs.some(input => typeof input === 'string' && input.includes('spectrogram')) ? 'the measurements and spectrogram image' : 'the measurements'
    const model = typeof interpretation.model === 'string' && interpretation.model ? ` (${clean(interpretation.model, 40)})` : ''
    view.interpretation = { label: `Model interpretation of ${what}${model}`, text: interpretation.text.trim().slice(0, 4000) }
  } else if (typeof result.interpretation_error === 'string' && result.interpretation_error) {
    view.interpretationError = clean(result.interpretation_error, 200)
  }
  return view
}
