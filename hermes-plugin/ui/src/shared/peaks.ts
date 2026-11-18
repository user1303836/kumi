// Waveforms drawn from the audio itself, for the palette's chips and cards, the map's player and its hover: each sound
// decoded once (one decode at a time: a palette of hundreds mustn't decode them all at once) at a resolution every size
// drawn divides, each size pooled from that, and the most recently drawn sounds kept (`KEPT_SOUNDS`).
import { peaksOf } from './palette.ts'

/** The largest audio (in data-URL characters) a waveform decodes. */
const DECODE_MOST = 12 * 1024 * 1024
/** The bars a sound is decoded into: 28 (a palette chip), 64 (a palette card) and 160 (the player and the map's hover)
 *  all divide 2,240, so each of those sizes pools to exactly what decoding at that size gave. */
export const FINE_BARS = 2240
/** How many sounds' waveforms are kept, the least recently drawn forgotten first: about 11 MB at most. */
const KEPT_SOUNDS = 1024

/** A sound's waveform: its fine bars (null: audio that can't be decoded here, or is too big to) and the sizes drawn. */
interface Kept {
  fine: number[] | null
  sizes: Map<number, number[]>
}
const kept = new Map<string, Kept>()
const loading = new Map<string, Promise<unknown>>()
// Who asked for each sound still queued: it decodes if any of them still wants it.
const askers = new Map<string, (() => boolean)[]>()
let queue: Promise<unknown> = Promise.resolve()

/** What a job returns when, at the head of the queue, nobody wants it any more: not kept, so a later ask decodes. */
const SKIPPED = Symbol('skipped')

/** `fine`'s bars pooled into `bars`: each the loudest of the fine bars it covers, scaled so the loudest is full height.
 *  Where `bars` divides `fine.length`, it's what `peaksOf` gives the same samples at `bars`. */
export function poolPeaks(fine: readonly number[], bars: number): number[] {
  const out = new Array<number>(Math.max(0, bars)).fill(0)
  if (fine.length === 0 || bars <= 0) return out
  let loudest = 0
  for (let bar = 0; bar < bars; bar++) {
    const start = Math.floor((bar * fine.length) / bars)
    const end = Math.max(start + 1, Math.floor(((bar + 1) * fine.length) / bars))
    let peak = 0
    for (let at = start; at < end && at < fine.length; at++) if (fine[at] > peak) peak = fine[at]
    out[bar] = peak
    if (peak > loudest) loudest = peak
  }
  return loudest > 0 ? out.map(peak => peak / loudest) : out
}

/** A kept sound's waveform at `bars` (null: it can't be drawn), drawn once per size; undefined while it isn't kept.
 *  Asking makes it the most recently drawn. */
function sized(id: string, bars: number): number[] | null | undefined {
  const entry = kept.get(id)
  if (!entry) return undefined
  kept.delete(id)
  kept.set(id, entry)
  if (!entry.fine) return null
  let peaks = entry.sizes.get(bars)
  if (!peaks) {
    peaks = poolPeaks(entry.fine, bars)
    entry.sizes.set(bars, peaks)
  }
  return peaks
}

function keep(id: string, fine: number[] | null) {
  kept.set(id, { fine, sizes: new Map() })
  if (kept.size > KEPT_SOUNDS) {
    const oldest = kept.keys().next().value
    if (oldest !== undefined) kept.delete(oldest)
  }
}

/** The waveform of `id` in `bars` bars, 0 to 1, from what's kept or decoded now; null for audio that can't be decoded
 *  here (or is too big to). `wanted` is asked as the job reaches the head of the queue: the map's player passes false
 *  once the keyboard has moved on, so stepping through sounds fetches and decodes only the one that stays. */
export function peaksFor(id: string, bars: number, audio: (id: string) => Promise<string>, wanted: () => boolean = () => true): Promise<number[] | null> {
  const done = sized(id, bars)
  if (done !== undefined) return Promise.resolve(done)
  const running = loading.get(id)
  if (running) {
    askers.get(id)?.push(wanted)
    return running.then(() => sized(id, bars) ?? null)
  }
  askers.set(id, [wanted])
  const run = async (): Promise<number[] | null | typeof SKIPPED> => {
    if (!(askers.get(id) ?? []).some(ask => ask())) return SKIPPED
    const url = await audio(id)
    const comma = url.indexOf(',')
    if (comma < 0 || url.length > DECODE_MOST || typeof OfflineAudioContext !== 'function') return null
    const binary = atob(url.slice(comma + 1))
    const bytes = new Uint8Array(binary.length)
    for (let at = 0; at < binary.length; at++) bytes[at] = binary.charCodeAt(at)
    const buffer = await new OfflineAudioContext(1, 1, 44_100).decodeAudioData(bytes.buffer)
    const mixed = new Float32Array(buffer.length)
    for (let channel = 0; channel < buffer.numberOfChannels; channel++) {
      const data = buffer.getChannelData(channel)
      for (let at = 0; at < data.length; at++) mixed[at] += data[at]
    }
    return peaksOf(mixed, FINE_BARS)
  }
  const promise = queue
    .then(run, run)
    .catch(() => null)
    .then(fine => {
      loading.delete(id)
      askers.delete(id)
      if (fine !== SKIPPED) keep(id, fine)
    })
  queue = promise
  loading.set(id, promise)
  return promise.then(() => sized(id, bars) ?? null)
}

/** A waveform already decoded, without decoding: what a card can draw at once. */
export function peaksKept(id: string, bars: number): number[] | null {
  return sized(id, bars) ?? null
}
