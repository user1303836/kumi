// A sound or notes Kumi had made through Mirelo (`kumi_generate`, then `kumi_job`), in CC-13's shapes: what was asked,
// the file it made, what it cost against what was reserved, and the producer's own cap. Credits are Mirelo API credits,
// never money; `null` is unknown, never zero. Nothing here spends, places or retries anything.
import { clean, jobWords, num, placeRequest, plainId, planNotesRequest, seconds as secondsWords } from './format.ts'
import { english, type Translate } from './i18n.ts'
import type { Job } from './types.ts'

/** The plugin's own budget after reconciliation (C7). Shown only from the result in hand, never kept from an older one. */
export interface Ledger {
  cap: number
  spent: number
  reserved: number
  left: number
  frozen: boolean
}

export interface Generation {
  /** The job's state: `running`, `done`, `failed`, `cancelled` or `interrupted`. */
  state: string
  /** `sfx` or `audio_to_midi` (CC-13), or one of CC-15's `video_to_sfx`, `extend` and `inpaint`. */
  kind: string
  /** What was asked for (a sound effect's words); empty for notes from audio. */
  prompt: string
  /** The primary output: its artifact, its basename and its type (`audio`, `midi`, `notes`, `musicxml`). */
  artifactId: string | null
  file: string
  outputType: string | null
  /** The billed span. For an `sfx` it's the sound's length; for CC-15's kinds it's the window, never the file's length. */
  seconds: number | null
  /** CC-15's window, when the kind has one and its numbers hold (W4-b); null shows none rather than a guess. */
  window: Window | null
  noteCount: number | null
  estimate: number | null
  reserved: number | null
  actual: number | null
  ledger: Ledger | null
  error: { code: string; reason: string } | null
  /** `not_submitted`, `unknown` or `submitted` (CC-13): only a job that ended `not_submitted` was never billed. */
  submission: string
  /** What the job is on now, in its own words (a real phase, never a path). */
  message: string
}

/** What a CC-15 generation covered, in the source's seconds: the video's first `seconds`, the span `extend` appended,
 *  or the region `inpaint` replaced. */
export type Window =
  | { kind: 'video_to_sfx'; seconds: number; source: number }
  | { kind: 'extend'; append: number; source: number }
  | { kind: 'inpaint'; start: number; end: number; source: number }

/** CC-15's kinds: each edits or scores a source, and its `seconds` is a window's, not the returned file's length. */
const WINDOWED = new Set(['video_to_sfx', 'extend', 'inpaint'])

/** A time as CC-15 gives it (W4-b): a finite, non-negative number of seconds in whole milliseconds. */
function timing(value: unknown): number | null {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) return null
  return Math.abs(value * 1000 - Math.round(value * 1000)) < 1e-6 ? value : null
}

const ms = (value: number): number => Math.round(value * 1000)

/** The window a CC-15 result names, checked against its own billed `seconds`: any number missing, out of order or
 *  disagreeing gives none. */
function windowOf(kind: string, result: Record<string, unknown>): Window | null {
  const source = timing(result.source_seconds)
  const billed = timing(result.seconds)
  if (source === null || source === 0 || billed === null || billed === 0) return null
  if (kind === 'video_to_sfx') return billed <= source ? { kind, seconds: billed, source } : null
  if (kind === 'extend') {
    const append = timing(result.append_seconds)
    return append !== null && ms(append) === ms(billed) ? { kind, append, source } : null
  }
  if (kind === 'inpaint') {
    const start = timing(result.region_start_seconds)
    const end = timing(result.region_end_seconds)
    if (start === null || end === null || !(start < end) || end > source || ms(end) - ms(start) !== ms(billed)) return null
    return { kind, start, end, source }
  }
  return null
}

const STATES = new Set(['running', 'done', 'failed', 'cancelled', 'interrupted'])

function objectOf(value: unknown): Record<string, unknown> | null {
  let found = value
  if (typeof found === 'string') {
    try {
      found = JSON.parse(found)
    } catch {
      return null
    }
  }
  return found && typeof found === 'object' && !Array.isArray(found) ? (found as Record<string, unknown>) : null
}

/** A whole number of credits, or null: unknown, fractional or negative amounts are never shown as a number. */
const credits = (value: unknown): number | null => (typeof value === 'number' && Number.isInteger(value) && value >= 0 ? value : null)
const positive = (value: unknown): number | null => (typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null)

function ledgerOf(value: unknown): Ledger | null {
  const ledger = objectOf(value)
  if (!ledger) return null
  const [cap, spent, reserved, left] = [ledger.cap, ledger.spent, ledger.reserved, ledger.left].map(credits)
  if (cap === null || spent === null || reserved === null || left === null) return null
  return { cap, spent, reserved, left, frozen: ledger.frozen === true }
}

/** A generation from a tool's result or a `generate.updated` card: `kumi_job`'s envelope for a `generate` job, or
 *  `kumi_generate`'s create answer. Null for anything else: a quote (no job yet), a scan, a refusal before a job. */
export function parseGeneration(value: unknown): Generation | null {
  const outer = objectOf(value)
  if (!outer || plainId(outer.job_id) === null) return null
  // `kumi_job`'s envelope carries the job's own kind; `kumi_generate`'s create answer is the generation itself, running,
  // with the quote it consumed. A scan's envelope is neither.
  const envelope = outer.kind === 'generate'
  const result = envelope ? objectOf(outer.result) ?? {} : outer
  const state = typeof outer.state === 'string' && STATES.has(outer.state) ? outer.state : null
  if (!state || (!envelope && (state !== 'running' || plainId(outer.quote_id) === null))) return null
  const spent = objectOf(result.credits)
  const estimate = objectOf(result.estimate)
  const error = objectOf(outer.error)
  const progress = objectOf(outer.progress)
  const notes = result.note_count
  const kind = typeof result.kind === 'string' ? clean(result.kind, 24) : ''
  return {
    state,
    kind,
    prompt: typeof result.prompt === 'string' ? clean(result.prompt, 200) : '',
    artifactId: plainId(result.artifact_id),
    file: typeof result.file === 'string' ? clean(result.file.split(/[\\/]/).pop() ?? '', 80) : '',
    outputType: typeof result.output_type === 'string' ? clean(result.output_type, 16) : null,
    seconds: positive(result.seconds),
    window: WINDOWED.has(kind) ? windowOf(kind, result) : null,
    noteCount: typeof notes === 'number' && Number.isInteger(notes) && notes >= 0 ? notes : null,
    estimate: credits(spent?.estimate ?? estimate?.credits),
    reserved: credits(spent?.reserved ?? estimate?.max_credits),
    actual: credits(spent?.actual),
    // A raw job snapshot has no ledger: then none is shown, rather than an older one.
    ledger: ledgerOf(result.ledger ?? outer.ledger),
    error: error ? { code: typeof error.code === 'string' ? clean(error.code, 40) : '', reason: typeof error.reason === 'string' ? clean(error.reason, 200) : '' } : null,
    submission: typeof result.submission === 'string' ? clean(result.submission, 24) : '',
    message: typeof progress?.message === 'string' ? clean(progress.message, 60) : ''
  }
}

/** Whether an ended job was never sent to Mirelo (CC-13's `not_submitted`): the one case nothing was charged. A running
 *  job's `not_submitted` isn't that: it may still submit. The result's own `submission` decides when it's there; the
 *  error's code only stands in when it isn't (rgzm), so `unknown` beside a `not_submitted` code is still unknown. */
function neverSent(made: Generation): boolean {
  if (made.state === 'running') return false
  return made.submission ? made.submission === 'not_submitted' : made.error?.code === 'not_submitted'
}

/** What it cost: `12 credits (est. 15)` once the charge is known; while it runs, what's reserved; after it ended
 *  without a known charge, that the charge isn't known, with what stays reserved. Never a guessed or zero charge. */
export function costWords(made: Generation, t: Translate = english): string {
  const amount = (n: number) => t('generate.credits', n, num(n, 0))
  if (made.actual !== null) {
    return made.estimate === null || made.estimate === made.actual ? amount(made.actual) : t('generate.cost', amount(made.actual), num(made.estimate, 0))
  }
  if (neverSent(made)) return t('generate.notSubmitted')
  const held = made.reserved ?? made.estimate
  if (held === null) return ''
  return made.state === 'running' ? t('generate.reserved', amount(held)) : t('generate.unknownCharge', amount(held))
}

/** What's left under the producer's cap, from this result's ledger only. */
export function capWords(made: Generation, t: Translate = english): string {
  return made.ledger ? t('generate.left', num(made.ledger.left, 0), num(made.ledger.cap, 0)) : ''
}

const NOTES = new Set(['midi', 'notes', 'musicxml'])

/** What a generation's primary output is to the UI: `audio` (waveform, Place…), `notes` (MIDI, notes or MusicXML:
 *  Plan notes…, never an audio step), or null for a type CC-13 doesn't name, which gets neither (CC-13). Before its
 *  output is known, a sound effect and CC-15's kinds are audio. */
export function outputKind(made: Generation): 'audio' | 'notes' | null {
  if (made.outputType === null) return made.kind === 'sfx' || WINDOWED.has(made.kind) ? 'audio' : made.kind === 'audio_to_midi' ? 'notes' : null
  if (made.outputType === 'audio') return 'audio'
  return NOTES.has(made.outputType) ? 'notes' : null
}

/** What the card and the Jobs row call it: the words it was made from, else what it is. */
export function generationTitle(made: Generation, t: Translate = english): string {
  if (made.prompt) return made.prompt
  if (WINDOWED.has(made.kind)) return t(`generate.kinds.${made.kind}`)
  return outputKind(made) === 'notes' ? t('generate.notesTitle') : t('generate.soundTitle')
}

// "2–4 s", or "1:10–1:14" past a minute: both ends in the same form.
const clock = (value: number): string => `${Math.floor(value / 60)}:${String(Math.round(value % 60)).padStart(2, '0')}`
const span = (from: number, to: number): string => (to < 60 ? `${num(from, 2)}–${secondsWords(to)}` : `${clock(from)}–${clock(to)}`)

/** How long a generation is, as its card shows it: CC-15's window in words (never its `seconds` as the file's length),
 *  an `sfx`'s length, or nothing. */
export function lengthWords(made: Generation, t: Translate = english): string {
  const covered = made.window
  if (covered?.kind === 'video_to_sfx') return t('generate.window.video_to_sfx', secondsWords(covered.seconds))
  if (covered?.kind === 'extend') return t('generate.window.extend', secondsWords(covered.append), secondsWords(covered.source))
  if (covered?.kind === 'inpaint') return t('generate.window.inpaint', span(covered.start, covered.end), secondsWords(covered.source))
  return WINDOWED.has(made.kind) || made.seconds === null ? '' : secondsWords(made.seconds)
}

/** A generation job as a Jobs row: the shared title and progress, with its file, CC-15's window, its cost and, when
 *  the job failed or the ledger is frozen, why. The raw `/job` snapshot has no ledger, so the frozen state comes from
 *  the latest `generate.updated` card kept on the job (`card`); a charge above the job's own reservation is amber
 *  either way. */
export function generationJobWords(job: Job & { card?: unknown }, t: Translate = english): ReturnType<typeof jobWords> {
  const words = jobWords(job, t)
  const made = parseGeneration({ ...job, kind: 'generate' })
  if (!made) return words
  const frozen = (made.ledger ?? parseGeneration(job.card)?.ledger)?.frozen === true
  const over = made.actual !== null && made.reserved !== null && made.actual > made.reserved
  const reason = made.state === 'failed' || made.state === 'interrupted' ? made.error?.reason ?? '' : ''
  const covered = made.window ? lengthWords(made, t) : ''
  const detail = [made.state === 'running' ? made.message : made.file, covered, reason, costWords(made, t), frozen ? t('generate.frozen') : '']
  return { ...words, detail: detail.filter(Boolean).join(' · '), tone: (frozen || over) && words.tone === 'good' ? 'warn' : words.tone }
}

/** The one thing a finished generation offers, as a request left unsent in the chat: audio goes through CC-11's file
 *  step (Place…); notes get a plan from the project's own timing first, never an audio step (CC-13). The request
 *  stays English (C5): it names the producer's own words, never a translated title. */
export function generationRequest(made: Generation, t: Translate = english): { label: string; title: string; request: string } | null {
  if (made.state !== 'done' || !made.artifactId) return null
  const kind = outputKind(made)
  if (kind === 'notes') {
    const request = planNotesRequest(made.artifactId, made.noteCount)
    return request ? { label: t('generate.planNotes'), title: t('generate.planNotesTitle'), request } : null
  }
  if (kind !== 'audio') return null
  const request = placeRequest(made.artifactId, made.prompt)
  return request ? { label: t('generate.place'), title: t('generate.placeTitle'), request } : null
}
