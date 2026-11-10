// The waveform of the sound that plays, for the map's pulse and the hover label: asked for only once the sound has
// played a moment, so only for a sound the pointer rests on. A sweep with hover-to-play on crosses many sounds, and
// each waveform is that sound's whole audio fetched and decoded: megabytes each, on a real library.
import { peaksFor, peaksKept } from '../peaks.ts'
import { WAVE_BARS } from './pulse.ts'

/** How long a sound must have been playing before its waveform is asked for. */
export const REST_MS = 150

/** Ask for the waveform of `id`, which has just begun to play: `got` hears it once it's decoded, unless what this
 *  returns was called first (the sound stopped, or another plays). One already decoded is answered by `peaksKept`:
 *  nothing is asked for here. */
export function restWave(id: string, audio: (id: string) => Promise<string>, got: (levels: number[] | null) => void): () => void {
  if (peaksKept(id, WAVE_BARS)) return () => {}
  let live = true
  const timer = setTimeout(() => void peaksFor(id, WAVE_BARS, audio, () => live).then(levels => live && got(levels)), REST_MS)
  return () => {
    live = false
    clearTimeout(timer)
  }
}
