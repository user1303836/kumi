// Words and numbers the UI shows, and the requests it puts in the chat. Pure functions, tested in ui/tests. The UI's
// words come through a `Translate` (English by default); the requests stay English, as contract C5 has them.
import { english, type Translate } from './i18n.ts'
import type { Focus, HistoryItem, Job, Named, Status } from './types.ts'

const DAWS: Record<string, string> = { ableton: 'Live', live: 'Live', reaper: 'REAPER', renoise: 'Renoise', soundcraft: 'SoundCraft' }

/** The DAW's name as producers say it (`ableton` is Live), or null for one Kumi doesn't name. */
export function knownDaw(daw: unknown): string | null {
  return (typeof daw === 'string' && DAWS[daw.toLowerCase()]) || null
}

/** The DAW's name, or "your DAW" in `t`'s words. */
export function dawName(daw: unknown, t: Translate = english): string {
  return knownDaw(daw) ?? t('common.yourDaw')
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
 *  at most `max` characters. Characters that hide, reorder or redraw text (bidi controls, zero-width characters, the
 *  BOM, tag characters) are dropped, so a name can't pass for other words. */
export function clean(text: unknown, max = 80): string {
  if (typeof text !== 'string') return ''
  const one = text
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u001f\u007f-\u009f\u2028\u2029]+/g, ' ')
    .replace(/[\u061c\u200b-\u200f\u202a-\u202e\u2060-\u2064\u2066-\u206f\ufeff\u{e0000}-\u{e007f}]/gu, '')
    .replace(/\s+/g, ' ')
    .replace(/"/g, '”')
    .trim()
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
export function ago(at: unknown, now = Date.now(), t: Translate = english): string {
  if (typeof at !== 'number') return ''
  const seconds = Math.max(0, (now - at) / 1000)
  if (seconds < 45) return t('time.now')
  if (seconds < 3600) return t('time.minutes', Math.round(seconds / 60))
  if (seconds < 86_400) return t('time.hours', Math.round(seconds / 3600))
  return t('time.days', Math.round(seconds / 86_400))
}

export function seconds(value: unknown): string {
  if (typeof value !== 'number' || !(value >= 0)) return ''
  return value < 10 ? `${num(value, 2)} s` : value < 60 ? `${num(value, 1)} s` : `${Math.floor(value / 60)}:${String(Math.round(value % 60)).padStart(2, '0')}`
}

// A job's kind as a producer says it, running and once it ended (catalog keys).
const JOB_KINDS: Record<string, { running: string; noun: string }> = {
  scan: { running: 'job.scanning', noun: 'job.scan' },
  generate: { running: 'job.generating', noun: 'job.generation' }
}

/** A count a job's result gives as a number or as a list of what it counted; null when it gives neither. */
function count(value: unknown): number | null {
  if (Array.isArray(value)) return value.length
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null
}

/** A background job in words: `Scanning` · `Drums · 412 of 1,200`, how much of it is done (0–1, when it's known),
 *  and a tone for its state. */
export function jobWords(job: Job, t: Translate = english): { title: string; detail: string; fraction: number | null; tone: 'bad' | 'good' | 'muted' | 'warn' } {
  const kind = typeof job.kind === 'string' && job.kind ? job.kind : ''
  const known = JOB_KINDS[kind]
  const noun = known ? t(known.noun) : clean(kind, 24) || t('job.job')
  const running = known ? t(known.running) : t('job.running', noun)
  const progress = job.progress ?? {}
  const done = typeof progress.done === 'number' && Number.isFinite(progress.done) ? progress.done : null
  const total = typeof progress.total === 'number' && Number.isFinite(progress.total) && progress.total > 0 ? progress.total : null
  const message = typeof progress.message === 'string' ? clean(progress.message, 60) : ''
  if (job.state === 'running' || job.state === undefined) {
    const count = done === null ? '' : total === null ? num(done, 0) : t('job.of', num(done, 0), num(total, 0))
    return { title: running, detail: [message, count].filter(Boolean).join(' · '), fraction: done !== null && total !== null ? Math.min(1, done / total) : null, tone: 'muted' }
  }
  if (job.state === 'done') {
    const result = job.result && typeof job.result === 'object' ? (job.result as Record<string, unknown>) : {}
    // A scan's result (W3): `sounds` indexed, `failed` files it couldn't read, as counts.
    const sounds = count(result.sounds)
    const failed = count(result.failed)
    const detail = [sounds === null ? '' : t('job.sounds', sounds, num(sounds, 0)), failed ? t('job.unreadable', num(failed, 0)) : '']
    return { title: t('job.done', noun), detail: detail.filter(Boolean).join(' · '), fraction: 1, tone: failed ? 'warn' : 'good' }
  }
  if (job.state === 'failed') return { title: t('job.failed', noun), detail: clean(job.error?.reason ?? '', 120), fraction: null, tone: 'bad' }
  if (job.state === 'cancelled') return { title: t('job.stopped', noun), detail: '', fraction: null, tone: 'muted' }
  if (job.state === 'interrupted') return { title: t('job.interrupted', noun), detail: t('job.interruptedWhy'), fraction: null, tone: 'warn' }
  return { title: noun, detail: clean(job.state ?? '', 24), fraction: null, tone: 'muted' }
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

/** Sent as is: the id alone. Only for a row `canUndo` allows, or a change's newest row when `canUndoChange` does: from
 *  the Nov 6 runtime on, Kumi's skill takes the request as the whole change (CC-3 A2's `whole_change`). */
export function undoRequest(item: Pick<HistoryItem, 'id'>): string {
  return `Undo Kumi change ${plainId(item.id) ?? ''}`.trim()
}

/** HISTORY's changes, newest first, each with its rows newest first (CC-3 A2). Rows a `change` call made share its
 *  `change_id`, even with a newer change listed between them. A row without one is a change of its own: recorded
 *  before A2, or not by a `change`. */
export function changesOf(items: readonly HistoryItem[]): HistoryItem[][] {
  const byChange = new Map<string, HistoryItem[]>()
  for (const item of items) {
    const key = typeof item.change_id === 'string' && item.change_id ? `change:${item.change_id}` : `row:${item.id}`
    const rows = byChange.get(key)
    if (rows) rows.push(item)
    else byChange.set(key, [item])
  }
  return [...byChange.values()]
}

/** The row a change's undo starts from: its newest not already undone. A whole-change undo goes newest first and stops
 *  at the first row it can't undo (A2), so rows undone before it are passed over. Its Undo sits on this row, and the
 *  request names it (A2 takes any row of the change). */
export function undoRowOf(rows: readonly HistoryItem[]): HistoryItem | undefined {
  return rows.find(row => row.state !== 'undone')
}

/** Whether the UI offers to undo a change now: its next row to undo can be undone. One Kumi can't undo blocks the rest. */
export function canUndoChange(status: Status | null | undefined, rows: readonly HistoryItem[]): boolean {
  return canUndo(status, undoRowOf(rows))
}

// Double quote marks, and the marks that pass for them, which a name could use to close the quotes a drafted request
// puts around it and go on in the producer's voice (W5's LOW 15): straight, curly, low-9, reversed, primes,
// fullwidth, angle and CJK quotes, halfwidth and vertical corner brackets included. A name keeps its apostrophes:
// they can't close “…”.
const QUOTE_MARKS = /["“”„‟″‶〃＂«»「」『』〝〞〟｢｣﹁﹂﹃﹄]/g

/** A name as a drafted request quotes it: `clean()`, then with no quote mark of its own, so everything from the DAW
 *  or a sample pack stays inside the request's quotes. */
export function quotable(name: unknown, max = 80): string {
  return clean(typeof name === 'string' ? name.replace(QUOTE_MARKS, ' ') : name, max).replace(QUOTE_MARKS, ' ').replace(/\s+/g, ' ').trim()
}

/** Left unsent in the input, like `soundRequest`: generated audio, by its artifact id, so the model can hand CC-11's
 *  `{artifact_id}` source to the DAW's file step. The producer says where before sending. Audio only (CC-13). The
 *  words it was made from are quoted like a sound's name: they can't close the quotes either. */
export function placeRequest(artifactId: string, name?: string): string | null {
  const id = plainId(artifactId)
  if (!id) return null
  const said = quotable(name)
  return said ? `Place “${said}” (artifact ${id}) in my project` : `Place artifact ${id} in my project`
}

/** Left unsent: generated notes never go through an audio file step (CC-13). The request asks for a plan from the
 *  project's own timing, shown before any separately approved notes or clip change; nothing is inferred or inserted. */
export function planNotesRequest(artifactId: string, count?: number | null): string | null {
  const id = plainId(artifactId)
  if (!id) return null
  const notes = typeof count === 'number' && Number.isInteger(count) && count >= 0 ? `the ${count} notes` : 'the notes'
  return `Plan putting ${notes} from artifact ${id} (timing in seconds) in my project: read the project's tempo and where they'd go first, and show me before changing anything`
}

/** Left unsent in the input, so the producer reads the sound's name before sending; null for an id the runtime
 *  doesn't write. The name leads, as the producer would say it; the ids follow in parentheses, with the map revision
 *  the sound is from, so the model can hand CC-11's `{sound_id, map_revision}` source to a file step without searching
 *  again. */
export function soundRequest(soundId: string, name?: string, revision?: string | null): string | null {
  const id = plainId(soundId)
  if (!id) return null
  const from = revision ? plainId(revision) : null
  const said = quotable(name)
  const which = `sound ${id}${from ? ` from map ${from}` : ''}`
  return said ? `Use “${said}” (${which}) on the selected track` : `Use ${which} on the selected track`
}

/** The most sounds one "More like these" names: Kumi's `near` takes up to sixteen. */
export const MOST_NEAR = 16
/** How many of them the request names, before "and N more". */
const NEAR_NAMED = 3

/** Left unsent, like Use…: the map's "More like this" (one sound) and "More like these" (the picks) ask the model for
 *  the sounds nearest them (a `near` search), naming the first `MOST_NEAR` by id with the map revision they're from,
 *  and the first few by name; null when none is an id the runtime writes. */
export function nearRequest(sounds: readonly { id: string; name?: string }[], revision?: string | null): string | null {
  const kept = sounds.flatMap(sound => {
    const id = plainId(sound.id)
    return id ? [{ id, said: quotable(sound.name) }] : []
  }).slice(0, MOST_NEAR)
  if (kept.length === 0) return null
  const from = revision ? plainId(revision) : null
  const which = `${kept.length === 1 ? 'sound' : 'sounds'} ${kept.map(sound => sound.id).join(', ')}${from ? ` from map ${from}` : ''}`
  const names = kept.flatMap(sound => (sound.said ? [`“${sound.said}”`] : [])).slice(0, NEAR_NAMED)
  if (names.length === 0) return `Find more sounds like ${which}`
  const rest = kept.length - names.length
  const listed = rest > 0 ? `${names.join(', ')} and ${rest} more` : names.length > 1 ? `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}` : names[0]
  return `Find more sounds like ${listed} (${which})`
}

const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B']

/** A MIDI note as Live names it on a Drum Rack pad: 36 is C1 (Live's middle C, 60, is C3). */
export function liveNote(note: number): string {
  return `${NOTE_NAMES[note % 12]}${Math.floor(note / 12) - 2}`
}

/** Left unsent, like Use…: put the sound on the drum pad that plays `note` on the selected track, an empty pad
 *  included (flow C). Live's pad is named as Live shows it, with its note; another DAW's by its note alone, since each
 *  names octaves its own way. Null for an id the runtime doesn't write or a note that isn't MIDI's. */
export function padRequest(soundId: string, note: number, daw: unknown, name?: string, revision?: string | null): string | null {
  const id = plainId(soundId)
  if (!id || !Number.isInteger(note) || note < 0 || note > 127) return null
  const from = revision ? plainId(revision) : null
  const said = quotable(name)
  const which = `sound ${id}${from ? ` from map ${from}` : ''}`
  const pad = daw === 'ableton' ? `the Drum Rack pad ${liveNote(note)} (note ${note})` : `the drum pad for note ${note}`
  return `Put ${said ? `“${said}” (${which})` : which} on ${pad} on the selected track`
}

export function setupRequest(daw: unknown): string {
  return `Set up Kumi for ${dawName(daw)}`
}

export function connectRequest(daw: unknown): string {
  return `Connect Kumi to ${dawName(daw)}`
}

/** After an outcome Kumi couldn't confirm: a fresh look (a read) before anything else. */
export const LOOK_AGAIN_REQUEST = 'Kumi, look at my project again before changing anything else'

/** From the map's line about hidden sounds: a scan finds what moved, and drops what's gone, for good. */
export const RESCAN_REQUEST = 'Kumi, rescan my sample folders'

/** What an unsure or partial outcome's `cleanup` says Kumi did about it (CC-2), as a sentence or nothing. */
export function cleanupWords(cleanup: unknown, t: Translate = english): string {
  if (cleanup === 'restored') return t('history.restored')
  if (cleanup === 'unsure') return t('history.restoreUnsure')
  return ''
}

/** The status chip's words: `Live · Night Drive · 124 BPM`, or what's missing. */
export function chipWords(status: Status | null, tempo?: unknown, t: Translate = english): { tone: 'bad' | 'good' | 'muted' | 'warn'; text: string } {
  if (!status) return { tone: 'muted', text: t('chip.kumi') }
  const daw = dawName(status.daw, t)
  const project = clean(status.project?.name ?? '', 40)
  if (status.connected) {
    const parts = [daw, project, bpm(tempo)].filter(Boolean)
    return { tone: 'good', text: parts.join(' · ') }
  }
  // Its last word was that it stopped (its chat ended, or it sat idle): nothing's wrong, it just isn't running.
  if (status.stale && status.running === false) return { tone: 'muted', text: [daw, project, t('chip.stopped')].filter(Boolean).join(' · ') }
  if (status.stale && status.owner) return { tone: 'warn', text: [daw, project, t('chip.notAnswering')].filter(Boolean).join(' · ') }
  return { tone: 'muted', text: t('chip.notConnected', daw) }
}
