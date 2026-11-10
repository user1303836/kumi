// The canvas over the sound map's own, for what moves (`overlay.ts`): mounted over the map's box, the same size, and
// never in the pointer's way. It hears which sound plays, asks for its waveform once the sound has played a moment
// (`restWave`: only the sound the pointer rests on is decoded) and hands both to the overlay; with hover-to-play on,
// it also listens to the pointer over the map's canvas for its trail. Everything stops when it unmounts.
import { T } from '@kumi/theme'
import { type RefObject, useEffect, useRef } from 'react'

import { peaksKept } from '../peaks.ts'
import { Overlay } from './overlay.ts'
import { WAVE_BARS } from './pulse.ts'
import { restWave } from './restWave.ts'

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
  kin,
  place,
  audio,
  still,
  moved,
  target,
  trail
}: {
  width: number
  height: number
  playing: PlayingSound | null
  /** The sounds to ring: the parents of the made sound the pointer or the keyboard is on (their ids and colours). */
  kin: readonly { id: string; color: string }[]
  /** Where a sound is drawn now, in the canvas's pixels (it moves with a pan, a zoom, a layout's move). */
  place: (id: string) => readonly [number, number] | null
  audio?: (id: string) => Promise<string>
  /** The system asks for less motion: the pulse is drawn once, still. */
  still: boolean
  /** Changes whenever the map under the overlay moves or is redrawn: a still pulse follows it. */
  moved: unknown
  /** The map's own canvas, and whether the pointer leaves a trail over it (hover-to-play is on). */
  target: RefObject<HTMLCanvasElement | null>
  trail: boolean
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
  useEffect(() => overlay.current?.kin(kin.map(sound => ({ place: () => placed.current(sound.id), color: sound.color }))), [kin])
  const [id, color, glyph, duration] = [playing?.id ?? null, playing?.color ?? '', playing?.glyph ?? 0, playing?.duration ?? 0]
  useEffect(() => {
    const element = target.current
    if (!trail || still || !element) return
    // A press (a box, a pan) is no sweep: the trail stops there, and the next move starts a new line.
    const move = (event: PointerEvent) => {
      if (event.buttons !== 0) return overlay.current?.lift()
      const box = element.getBoundingClientRect()
      overlay.current?.trail(event.clientX - box.left, event.clientY - box.top)
    }
    const lift = () => overlay.current?.lift()
    element.addEventListener('pointermove', move)
    element.addEventListener('pointerleave', lift)
    return () => {
      element.removeEventListener('pointermove', move)
      element.removeEventListener('pointerleave', lift)
    }
  }, [trail, still, target])
  useEffect(() => {
    const drawn = overlay.current
    if (!drawn) return
    if (id === null) {
      drawn.play(null)
      return
    }
    drawn.play({ place: () => placed.current(id), color, glyph, duration, levels: peaksKept(id, WAVE_BARS) })
    if (fetched.current) return restWave(id, fetched.current, levels => drawn.levels(levels))
  }, [id, color, glyph, duration])
  return <canvas aria-hidden ref={canvas} style={{ position: 'absolute', inset: 0, width: '100%', height: '100%', pointerEvents: 'none', color: T.text }} />
}
