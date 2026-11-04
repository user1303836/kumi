// Words and numbers the UI shows, and the requests it puts in the chat. Pure functions, tested in tests/ui.
import type { HistoryItem, Named, Status } from './types.ts'

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

// The requests the UI puts in the chat (contract C5). Every change goes model → tool → approval like any other
// request, and names the ids the user is looking at, so nothing is guessed.

export function undoRequest(item: Pick<HistoryItem, 'id' | 'title' | 'label'>): string {
  const title = clean(item.title ?? item.label ?? '', 120)
  return title ? `Undo Kumi change ${item.id} (“${title}”)` : `Undo Kumi change ${item.id}`
}

export function useSoundRequest(soundId: string, name?: string): string {
  const said = clean(name ?? '', 80)
  return said ? `Use sound ${soundId} (“${said}”) on the selected track` : `Use sound ${soundId} on the selected track`
}

export function setupRequest(daw: unknown): string {
  return `Set up Kumi for ${dawName(daw)}`
}

export function connectRequest(daw: unknown): string {
  return `Connect Kumi to ${dawName(daw)}`
}

/** After an outcome Kumi couldn't confirm: a fresh look (a read) before anything else. */
export const LOOK_AGAIN_REQUEST = 'Kumi, look at my project again before changing anything else'

/** The status chip's words: `Live · Night Drive · 124 BPM`, or what's missing. */
export function chipWords(status: Status | null, tempo?: unknown): { tone: 'bad' | 'good' | 'muted' | 'warn'; text: string } {
  if (!status) return { tone: 'muted', text: 'Kumi' }
  const daw = dawName(status.daw ?? status.setup?.daw)
  const project = clean(status.project?.name ?? '', 40)
  if (status.connected) {
    const parts = [daw, project, bpm(tempo)].filter(Boolean)
    return { tone: 'good', text: parts.join(' · ') }
  }
  if (status.stale && status.owner) return { tone: 'warn', text: project ? `${daw} · ${project} · not answering` : `${daw} · not answering` }
  return { tone: 'muted', text: `Kumi · ${daw} not connected` }
}
