// The canvas over the sound map's own, for what moves (`overlay.ts`): mounted over the map's box, the same size, and
// never in the pointer's way. It hears which sound plays, asks for its waveform (decoded once; the audio is already
// here, it is playing) and hands both to the overlay; everything stops when it unmounts.
import { T } from '@kumi/theme'
import { useEffect, useRef } from 'react'

import { peaksFor, peaksKept } from '../peaks.ts'
import { Overlay } from './overlay.ts'
import { WAVE_BARS } from './pulse.ts'

/** The sound that plays, when it is on this map: its id, colour, glyph and length in seconds. */
export interface PlayingSound {
  id: string
  color: string
  glyph: number
  duration: number
}

export function AtlasOverlay({
  width,
  height,
  playing,
  place,
  audio,
  still,
  moved
}: {
  width: number
  height: number
  playing: PlayingSound | null
  /** Where the playing sound is drawn now, in the canvas's pixels (it moves with a pan, a zoom, a layout's move). */
  place: () => readonly [number, number] | null
  audio?: (id: string) => Promise<string>
  /** The system asks for less motion: the pulse is drawn once, still. */
  still: boolean
  /** Changes whenever the map under the overlay moves or is redrawn: a still pulse follows it. */
  moved: unknown
}) {
  const canvas = useRef<HTMLCanvasElement | null>(null)
  const overlay = useRef<Overlay | null>(null)
  // The parents hand `place` and `audio` over anew on every render: kept here, so a play isn't begun again for them.
  const placed = useRef(place)
  placed.current = place
  const fetched = useRef(audio)
  fetched.current = audio
  useEffect(() => {
    if (!canvas.current) return
    const made = new Overlay(canvas.current)
    overlay.current = made
    return () => {
      made.dispose()
      overlay.current = null
    }
  }, [])
  useEffect(() => {
    if (overlay.current) overlay.current.still = still
  }, [still])
  useEffect(() => overlay.current?.resize(width, height, window.devicePixelRatio || 1), [width, height])
  useEffect(() => overlay.current?.moved(), [moved])
  const [id, color, glyph, duration] = [playing?.id ?? null, playing?.color ?? '', playing?.glyph ?? 0, playing?.duration ?? 0]
  useEffect(() => {
    const drawn = overlay.current
    if (!drawn) return
    if (id === null) {
      drawn.play(null)
      return
    }
    let live = true
    drawn.play({ place: () => placed.current(), color, glyph, duration, levels: peaksKept(id, WAVE_BARS) })
    // Wanted only while this sound still plays: a sweep across the map decodes only the sound it rests on.
    if (fetched.current) void peaksFor(id, WAVE_BARS, fetched.current, () => live).then(levels => live && drawn.levels(levels))
    return () => {
      live = false
    }
  }, [id, color, glyph, duration])
  return <canvas aria-hidden ref={canvas} style={{ position: 'absolute', inset: 0, width: '100%', height: '100%', pointerEvents: 'none', color: T.text }} />
}
