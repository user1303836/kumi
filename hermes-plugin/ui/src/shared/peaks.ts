// Waveforms drawn from the audio itself, for the palette's cards and the map's player: decoded once per sound and
// size, one decode at a time (a palette of hundreds mustn't decode them all at once), and kept for the session.
import { peaksOf } from './palette.ts'

/** The largest audio (in data-URL characters) a waveform decodes. */
const DECODE_MOST = 12 * 1024 * 1024

const kept = new Map<string, number[] | null>()
const loading = new Map<string, Promise<number[] | null>>()
let queue: Promise<unknown> = Promise.resolve()

/** The waveform of `id` in `bars` bars, 0 to 1, from the cache or decoded now; null for audio that can't be decoded
 *  here (or is too big to). */
export function peaksFor(id: string, bars: number, audio: (id: string) => Promise<string>): Promise<number[] | null> {
  const key = `${bars}:${id}`
  const done = kept.get(key)
  if (done !== undefined) return Promise.resolve(done)
  const running = loading.get(key)
  if (running) return running
  const run = async (): Promise<number[] | null> => {
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
    return peaksOf(mixed, bars)
  }
  const promise = queue
    .then(run, run)
    .catch(() => null)
    .then(peaks => {
      kept.set(key, peaks)
      loading.delete(key)
      return peaks
    })
  queue = promise
  loading.set(key, promise)
  return promise
}

/** A waveform already decoded, without decoding: what a card can draw at once. */
export function peaksKept(id: string, bars: number): number[] | null {
  return kept.get(`${bars}:${id}`) ?? null
}
