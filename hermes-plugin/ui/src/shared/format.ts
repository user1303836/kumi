// Words and numbers the UI shows, and the requests it puts in the chat. Pure functions, tested in ui/tests.
import type { Focus, HistoryItem, Job, Named, Status } from './types.ts'

const DAWS: Record<string, string> = { ableton: 'Live', live: 'Live', reaper: 'REAPER', renoise: 'Renoise', soundcraft: 'SoundCraft' }

/** The DAW's name as producers say it: `ableton` is Live. */
export function dawName(daw: unknown): string {
  return (typeof daw === 'string' && DAWS[daw.toLowerCase()]) || 'your DAW'
}

/** A name from FOCUS or HISTORY, which may be a string or an object with a `name`. */
export function nameOf(value: Named): string | undefined {
  if (typeof value === 'string') return value.trim() || undefined
  if (value && typeof value === 'object' && typeof value.name === 'string') return value.name.trim() || undefined
  return undefined
}

/** What's in focus as `track › device`, else `track › item` (a clip, a media item, a pattern), each part cut for
 *  display; empty when nothing is. Live names its clip in `clip`; REAPER and Renoise their item in `item`. */
export function focusPath(focus: Focus | null | undefined): string[] {
  const parts = focus?.focus
  const inner = nameOf(parts?.device) ?? nameOf(parts?.item?.name) ?? nameOf(parts?.clip)
  return [nameOf(parts?.track), inner].filter((part): part is string => Boolean(part)).map(part => clean(part, 40))
}

/** The FOCUS chips beyond the path: the view the producer works in, and the section selected (a scene, a marker). */
export function focusPlace(focus: Focus | null | undefined): string[] {
  const parts = focus?.focus
  const view = nameOf(parts?.mainView?.label) ?? nameOf(parts?.view)
  const section = nameOf(parts?.section?.name) ?? nameOf(parts?.scene)
  return [view && clean(view, 20), section && clean(section, 24)].filter((part): part is string => Boolean(part))
}

/** The parameter last touched, `Filter Freq 1.20 kHz`, or nothing. */
export function focusParameter(focus: Focus | null | undefined): string | undefined {
  const parameter = focus?.focus?.parameter
  const name = nameOf(parameter?.name)
  if (!name) return undefined
  const value = nameOf(parameter?.value ?? undefined)
  return clean(value ? `${name} ${value}` : name, 60)
}

/** What changed while Kumi wasn't looking: every catch-up's lines, and how many more weren't listed. */
export function catchUpLines(focus: Focus | null | undefined, most = 4): { lines: string[]; more: number } {
  const entries = Array.isArray(focus?.catch_up) ? focus.catch_up : []
  const all = entries.flatMap(entry => (entry && Array.isArray(entry.lines) ? entry.lines.filter((line): line is string => typeof line === 'string') : []))
  const unlisted = entries.reduce((sum, entry) => sum + (entry && typeof entry.more === 'number' && entry.more > 0 ? Math.floor(entry.more) : 0), 0)
  return { lines: all.slice(0, most).map(line => clean(line, 120)), more: Math.max(0, all.length - most) + unlisted }
}

/** A track's colour, when FOCUS or HISTORY gives one as `#rrggbb`. */
export function colorOf(value: Named): string | undefined {
  const color = value && typeof value === 'object' ? value.color : undefined
  return typeof color === 'string' && /^#[0-9a-f]{6}$/i.test(color) ? color : undefined
}

/** Text from the backend, safe to put in a sentence: one line, no control characters, quotes turned typographic,
 *  at most `max` characters. */
export function clean(text: unknown, max = 80): string {
  if (typeof text !== 'string') return ''
  // eslint-disable-next-line no-control-regex
  const one = text.replace(/[\u0000-\u001f\u007f]+/g, ' ').replace(/\s+/g, ' ').replace(/"/g, '”').trim()
  return one.length > max ? `${one.slice(0, max - 1)}…` : one
}

const MINUS = '\u2212'

/** A number with a true minus sign, `digits` decimals at most, and thousands separators. */
export function num(value: number, digits = 1): string {
  const fixed = Math.abs(value).toLocaleString('en-US', { maximumFractionDigits: digits, minimumFractionDigits: 0 })
  return value < 0 && fixed !== '0' ? `${MINUS}${fixed}` : fixed
}

/** A difference with its sign always shown: `+1.2`, `−0.4`, `0`. */
export function signed(value: number, digits = 1): string {
  const text = num(value, digits)
  return value > 0 && text !== '0' ? `+${text}` : text
}

export function hz(value: number): string {
  return value >= 10_000 ? `${num(value / 1000, 1)} kHz` : `${num(value, 0)} Hz`
}

export function bpm(tempo: unknown): string | undefined {
  return typeof tempo === 'number' && tempo > 0 ? `${num(tempo, 1)} BPM` : undefined
}

/** How long ago `at` (ms since 1970) was: `now`, `4 min ago`, `2 h ago`, `3 d ago`. */
export function ago(at: unknown, now = Date.now()): string {
  if (typeof at !== 'number') return ''
  const seconds = Math.max(0, (now - at) / 1000)
  if (seconds < 45) return 'now'
  if (seconds < 3600) return `${Math.round(seconds / 60)} min ago`
  if (seconds < 86_400) return `${Math.round(seconds / 3600)} h ago`
  return `${Math.round(seconds / 86_400)} d ago`
}

export function seconds(value: unknown): string {
  if (typeof value !== 'number' || !(value >= 0)) return ''
  return value < 10 ? `${num(value, 2)} s` : value < 60 ? `${num(value, 1)} s` : `${Math.floor(value / 60)}:${String(Math.round(value % 60)).padStart(2, '0')}`
}

// A job's kind as a producer says it: running, and once it ended.
const JOB_KINDS: Record<string, { running: string; noun: string }> = { scan: { running: 'Scanning', noun: 'Scan' } }

/** A background job in words: `Scanning` · `Drums · 412 of 1,200`, how much of it is done (0–1, when it's known),
 *  and a tone for its state. */
export function jobWords(job: Job): { title: string; detail: string; fraction: number | null; tone: 'bad' | 'good' | 'muted' | 'warn' } {
  const kind = typeof job.kind === 'string' && job.kind ? job.kind : ''
  const named = JOB_KINDS[kind] ?? { running: `${clean(kind || 'Job', 24)} running`, noun: clean(kind || 'Job', 24) }
  const progress = job.progress ?? {}
  const done = typeof progress.done === 'number' && Number.isFinite(progress.done) ? progress.done : null
  const total = typeof progress.total === 'number' && Number.isFinite(progress.total) && progress.total > 0 ? progress.total : null
  const message = typeof progress.message === 'string' ? clean(progress.message, 60) : ''
  if (job.state === 'running' || job.state === undefined) {
    const count = done === null ? '' : total === null ? num(done, 0) : `${num(done, 0)} of ${num(total, 0)}`
    return { title: named.running, detail: [message, count].filter(Boolean).join(' · '), fraction: done !== null && total !== null ? Math.min(1, done / total) : null, tone: 'muted' }
  }
  if (job.state === 'done') {
    const result = job.result && typeof job.result === 'object' ? (job.result as Record<string, unknown>) : {}
    const sounds = typeof result.sounds === 'number' ? `${num(result.sounds, 0)} sound${result.sounds === 1 ? '' : 's'}` : ''
    return { title: `${named.noun} done`, detail: sounds, fraction: 1, tone: 'good' }
  }
  if (job.state === 'failed') return { title: `${named.noun} failed`, detail: clean(job.error?.reason ?? '', 120), fraction: null, tone: 'bad' }
  if (job.state === 'cancelled') return { title: `${named.noun} stopped`, detail: '', fraction: null, tone: 'muted' }
  if (job.state === 'interrupted') return { title: `${named.noun} interrupted`, detail: 'Kumi stopped before it finished.', fraction: null, tone: 'warn' }
  return { title: named.noun, detail: clean(job.state ?? '', 24), fraction: null, tone: 'muted' }
}

// The requests the UI puts in the chat (contract C5). Every change goes model → tool → approval like any other
// request, and names the ids the user is looking at, so nothing is guessed. A request that's sent at once carries
// nothing but our own words and an id: no text from the DAW or a sample pack reaches the model as the user's.

/** Ids as the runtime writes them; anything else is never put in a request. */
const ID = /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,63}$/

export function plainId(value: unknown): string | null {
  return typeof value === 'string' && ID.test(value) ? value : null
}

/** Whether the UI offers to undo `item` now: Kumi connected and sure of the project (CC-2 R6), the row undoable. */
export function canUndo(status: Status | null | undefined, item: HistoryItem | null | undefined): boolean {
  return Boolean(item && status?.connected && !status.stale && !status.unsure && item.undo_available === true && plainId(item.id))
}

/** Sent as is: the id alone. Only for a row `canUndo` allows. */
export function undoRequest(item: Pick<HistoryItem, 'id'>): string {
  return `Undo Kumi change ${plainId(item.id) ?? ''}`.trim()
}

/** Left unsent in the input, so the producer reads the sound's name before sending; null for an id the runtime
 *  doesn't write. It names the map revision the sound is from, so the model can hand CC-11's
 *  `{sound_id, map_revision}` source to a file step without searching again. */
export function soundRequest(soundId: string, name?: string, revision?: string | null): string | null {
  const id = plainId(soundId)
  if (!id) return null
  const from = revision ? plainId(revision) : null
  const said = clean(name ?? '', 80)
  return `Use sound ${id}${from ? ` from map ${from}` : ''}${said ? ` (“${said}”)` : ''} on the selected track`
}

export function setupRequest(daw: unknown): string {
  return `Set up Kumi for ${dawName(daw)}`
}

export function connectRequest(daw: unknown): string {
  return `Connect Kumi to ${dawName(daw)}`
}

/** After an outcome Kumi couldn't confirm: a fresh look (a read) before anything else. */
export const LOOK_AGAIN_REQUEST = 'Kumi, look at my project again before changing anything else'

/** What an unsure or partial outcome's `cleanup` says Kumi did about it (CC-2), as a sentence or nothing. */
export function cleanupWords(cleanup: unknown): string {
  if (cleanup === 'restored') return 'Kumi put back what it changed meanwhile.'
  if (cleanup === 'unsure') return 'Kumi isn’t sure it put everything back.'
  return ''
}

/** The status chip's words: `Live · Night Drive · 124 BPM`, or what's missing. */
export function chipWords(status: Status | null, tempo?: unknown): { tone: 'bad' | 'good' | 'muted' | 'warn'; text: string } {
  if (!status) return { tone: 'muted', text: 'Kumi' }
  const daw = dawName(status.daw)
  const project = clean(status.project?.name ?? '', 40)
  if (status.connected) {
    const parts = [daw, project, bpm(tempo)].filter(Boolean)
    return { tone: 'good', text: parts.join(' · ') }
  }
  // Its last word was that it stopped (its chat ended, or it sat idle): nothing's wrong, it just isn't running.
  if (status.stale && status.running === false) return { tone: 'muted', text: project ? `${daw} · ${project} · Kumi stopped` : `${daw} · Kumi stopped` }
  if (status.stale && status.owner) return { tone: 'warn', text: project ? `${daw} · ${project} · not answering` : `${daw} · not answering` }
  return { tone: 'muted', text: `Kumi · ${daw} not connected` }
}
