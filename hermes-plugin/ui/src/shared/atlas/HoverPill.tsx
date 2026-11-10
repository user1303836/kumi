// The label beside the sound under the pointer: its family's glyph and its name, its kind, length and folder, and,
// once its audio is here (it plays on hover, or has played), its small waveform in its family's colour with a playhead
// while it plays. It never fetches audio itself: a hover with hover-to-play off shows the words alone.
import { T } from '@kumi/theme'
import { useEffect, useRef, useState } from 'react'

import { clean, seconds } from '../format.ts'
import { glass } from '../mapUi.tsx'
import { peaksFor, peaksKept } from '../peaks.ts'
import type { MapSound } from '../types.ts'
import { S } from '../ui.tsx'
import { Glyph } from './Glyph.tsx'
import { WAVE_BARS } from './pulse.ts'
import { usePlayhead, Wave } from './Wave.tsx'

/** The label's size at its widest, and how far from its sound it sits. */
const WIDEST = 300
const TALLEST = 44
const AWAY = 24

export function HoverPill({
  id,
  sound,
  color,
  glyph,
  playing,
  audio,
  at,
  within
}: {
  id: string
  sound: MapSound
  color: string
  glyph: number
  playing: boolean
  audio?: (id: string) => Promise<string>
  /** Where the sound is, and the room the map has (its width and height): the label sits to the sound's right, level
   *  with it, or to its left when the right has no room. */
  at: readonly [number, number]
  within: readonly [number, number]
}) {
  const [peaks, setPeaks] = useState<number[] | null>(() => peaksKept(id, WAVE_BARS))
  // The parent hands `audio` over anew on every render: kept here, so the waveform is asked for once per play.
  const fetched = useRef(audio)
  fetched.current = audio
  useEffect(() => {
    setPeaks(peaksKept(id, WAVE_BARS))
    if (!playing || !fetched.current) return
    let live = true
    void peaksFor(id, WAVE_BARS, fetched.current, () => live).then(found => live && found && setPeaks(found))
    return () => {
      live = false
    }
  }, [id, playing])
  const duration = typeof sound.duration === 'number' ? sound.duration : 0
  const progress = usePlayhead(playing, duration)
  const under = [seconds(sound.duration), clean(sound.folder ?? '', 40)].filter(Boolean).join(' · ')
  const side = at[0] + AWAY + WIDEST > within[0] ? { right: Math.max(0, within[0] - at[0] + AWAY) } : { left: at[0] + AWAY }
  const top = Math.min(Math.max(0, at[1] - TALLEST / 2), Math.max(0, within[1] - TALLEST))
  return (
    <div style={{ ...glass, borderRadius: 10, position: 'absolute', ...side, top, maxWidth: WIDEST, pointerEvents: 'none', padding: '5px 10px', ...S.row, gap: 10 }}>
      <div style={{ minWidth: 0, flex: '0 1 auto' }}>
        <div style={{ ...S.row, gap: 6, fontSize: 11.5, color: T.text }}>
          <Glyph color={color} glyph={glyph} />
          <span style={{ ...S.ellipsis, fontWeight: 600 }}>
            {clean(sound.name, 48)} <span style={{ color: T.muted, fontWeight: 400 }}>· {clean(sound.class ?? '', 16)}</span>
          </span>
        </div>
        {under && <div style={{ ...S.ellipsis, fontSize: 10.5, color: T.faint, paddingLeft: 15, fontVariantNumeric: 'tabular-nums' }}>{under}</div>}
      </div>
      {peaks && <Wave colors={[color]} height={24} peaks={peaks} progress={progress} width={96} />}
    </div>
  )
}
