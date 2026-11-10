// The label beside the sound under the pointer: its family's glyph and its name, its kind, length and folder, and,
// once its audio is here (it has played, or the pointer has rested on it as it plays), its small waveform in its
// family's colour with a playhead while it plays. It never fetches audio for a sound that isn't playing: a hover with
// hover-to-play off shows the words alone.
import { T } from '@kumi/theme'
import { useEffect, useRef, useState } from 'react'

import { clean, seconds } from '../format.ts'
import { glass } from '../mapUi.tsx'
import { peaksKept } from '../peaks.ts'
import type { MapSound } from '../types.ts'
import { S } from '../ui.tsx'
import { Glyph } from './Glyph.tsx'
import { labelPlace } from './labelPlace.ts'
import { WAVE_BARS } from './pulse.ts'
import { restWave } from './restWave.ts'
import { usePlayhead, Wave } from './Wave.tsx'

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
  /** Where the sound is, and the room the map has clear (its width left of a side sheet, and its height): the label
   *  sits to the sound's right, level with it, or to its left when the right has no room (`labelPlace`). */
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
    return restWave(id, fetched.current, found => found && setPeaks(found))
  }, [id, playing])
  const duration = typeof sound.duration === 'number' ? sound.duration : 0
  const progress = usePlayhead(playing, duration)
  const under = [seconds(sound.duration), clean(sound.folder ?? '', 40)].filter(Boolean).join(' · ')
  // Its right edge is measured from the map's left even when it sits left of the sound: the map's box runs on under a
  // side sheet, so the box's own right edge is not where the room ends.
  const place = labelPlace(at, within)
  const side = place.side === 'right' ? { left: place.x } : { right: `calc(100% - ${place.x}px)` }
  return (
    <div style={{ ...glass, borderRadius: 10, position: 'absolute', ...side, top: place.top, maxWidth: Math.max(140, place.most), pointerEvents: 'none', padding: '5px 10px', ...S.row, gap: 10 }}>
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
