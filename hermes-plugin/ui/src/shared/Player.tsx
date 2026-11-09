// The map's player: the sound picked last, floating at the map's foot. Its name, kind, length and folder; its waveform,
// decoded from its own audio, in its group's colour, with a line running across while it plays (a click on the
// waveform plays it too); and what can be done with it (the parent's actions: Add to palette, More like this, Use on
// pad…, Use…). It slides up when a sound is picked, and × lets it go.
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'
import { useEffect, useRef, useState } from 'react'

import { clean, seconds } from './format.ts'
import { glass, Icon } from './mapUi.tsx'
import { peaksFor, peaksKept } from './peaks.ts'
import type { MapSound } from './types.ts'
import { Chip, FOCUSABLE, S, useT } from './ui.tsx'

const BARS = 160

function Waveform({ id, color, playing, duration, audio, onClick }: { id: string; color: string; playing: boolean; duration: number; audio: (id: string) => Promise<string>; onClick: () => void }) {
  const canvas = useRef<HTMLCanvasElement | null>(null)
  const [peaks, setPeaks] = useState<number[] | null>(() => peaksKept(id, BARS))
  const [width, setWidth] = useState(0)
  const [startedAt, setStartedAt] = useState<number | null>(null)
  const [now, setNow] = useState(0)
  useEffect(() => {
    let live = true
    setPeaks(peaksKept(id, BARS))
    void peaksFor(id, BARS, audio).then(found => live && setPeaks(found))
    return () => {
      live = false
    }
  }, [id, audio])
  useEffect(() => {
    const element = canvas.current?.parentElement
    if (!element) return
    const observer = new ResizeObserver(entries => setWidth(Math.floor(entries[0]?.contentRect.width ?? 0)))
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  useEffect(() => setStartedAt(playing ? performance.now() : null), [playing, id])
  useEffect(() => {
    if (startedAt === null) return
    const frame = requestAnimationFrame(setNow)
    return () => cancelAnimationFrame(frame)
  }, [startedAt, now])
  useEffect(() => {
    const element = canvas.current
    const context = element?.getContext('2d')
    if (!element || !context || width === 0) return
    const ratio = window.devicePixelRatio || 1
    const height = 34
    element.width = width * ratio
    element.height = height * ratio
    context.setTransform(ratio, 0, 0, ratio, 0, 0)
    context.clearRect(0, 0, width, height)
    // A canvas takes no CSS variables: it wears the colour as its own CSS colour, and draws with the computed value.
    context.fillStyle = getComputedStyle(element).color
    const bars = peaks ?? new Array<number>(BARS).fill(0.06)
    const step = width / bars.length
    context.globalAlpha = peaks ? 0.95 : 0.3
    bars.forEach((peak, bar) => {
      const tall = Math.max(1, peak * (height - 2))
      context.fillRect(bar * step, (height - tall) / 2, Math.max(1, step - 0.5), tall)
    })
  }, [peaks, width, color])
  const progress = startedAt !== null && duration > 0 ? Math.min(1, (now - startedAt) / 1000 / duration) : null
  return (
    <div aria-hidden onClick={onClick} style={{ position: 'relative', flex: '1 1 160px', minWidth: 120, height: 34, cursor: 'pointer' }}>
      <canvas ref={canvas} style={{ width: '100%', height: 34, display: 'block', color }} />
      {progress !== null && progress > 0 && progress < 1 && (
        <span style={{ position: 'absolute', top: -2, bottom: -2, left: `${progress * 100}%`, width: 2, borderRadius: 1, background: T.text, boxShadow: `0 0 8px ${color}` }} />
      )}
    </div>
  )
}

export function Player({
  id,
  sound,
  color,
  playing,
  onPlay,
  onStop,
  onClose,
  actions,
  audio
}: {
  id: string
  sound: MapSound
  color: string
  playing: boolean
  onPlay: () => void
  onStop: () => void
  onClose: () => void
  actions?: ReactNode
  audio: (id: string) => Promise<string>
}) {
  const t = useT()
  const [shown, setShown] = useState(false)
  useEffect(() => {
    const frame = requestAnimationFrame(() => setShown(true))
    return () => cancelAnimationFrame(frame)
  }, [])
  const name = clean(sound.name ?? id, 80)
  return (
    <section
      aria-label={t('map.player', name)}
      style={{
        ...glass,
        position: 'relative',
        display: 'flex',
        flexDirection: 'column',
        gap: 6,
        padding: '8px 34px 8px 8px',
        transform: shown ? 'none' : 'translateY(16px)',
        opacity: shown ? 1 : 0,
        transition: 'transform 240ms cubic-bezier(.2,.8,.2,1), opacity 180ms',
        pointerEvents: 'auto'
      }}
    >
      <div style={{ ...S.row, gap: 10, flexWrap: 'wrap' }}>
        <button
          aria-label={playing ? t('map.stop') : t('map.play')}
          className={FOCUSABLE}
          onClick={playing ? onStop : onPlay}
          title={t('map.playTitle')}
          type="button"
          style={{
            flex: 'none',
            width: 34,
            height: 34,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            padding: 0,
            borderRadius: '50%',
            border: `1px solid ${color}`,
            background: playing ? `color-mix(in srgb, ${color} 34%, transparent)` : `color-mix(in srgb, ${color} 14%, transparent)`,
            color,
            cursor: 'pointer',
            boxShadow: playing ? `0 0 14px -2px ${color}` : 'none',
            transition: 'background-color 140ms, box-shadow 200ms'
          }}
        >
          <Icon name={playing ? 'stop' : 'play'} size={13} />
        </button>
        <div style={{ display: 'flex', flexDirection: 'column', minWidth: 0, flex: '0 1 220px' }}>
          <span style={{ ...S.text, ...S.ellipsis, fontWeight: 650 }}>{name}</span>
          <span style={{ ...S.row, gap: 4, marginTop: 2 }}>
            {sound.class && <Chip>{clean(sound.class, 16)}</Chip>}
            {sound.kind && <Chip>{clean(sound.kind, 16)}</Chip>}
            {typeof sound.duration === 'number' && <Chip>{seconds(sound.duration)}</Chip>}
          </span>
        </div>
        <Waveform audio={audio} color={color} duration={typeof sound.duration === 'number' ? sound.duration : 2} id={id} onClick={playing ? onStop : onPlay} playing={playing} />
      </div>
      <button
        aria-label={t('map.letGo')}
        className={FOCUSABLE}
        onClick={onClose}
        title={t('map.letGo')}
        type="button"
        style={{ position: 'absolute', top: 6, right: 6, width: 24, height: 24, display: 'flex', alignItems: 'center', justifyContent: 'center', padding: 0, borderRadius: 12, border: 'none', background: 'transparent', color: T.faint, cursor: 'pointer' }}
      >
        <Icon name="close" size={12} />
      </button>
      <div style={{ ...S.row, gap: 6, flexWrap: 'wrap', marginRight: -26 }}>
        {sound.folder && <span style={{ ...S.faint, ...S.ellipsis, flex: '1 1 8rem', fontSize: 11 }}>{clean(sound.folder, 80)}</span>}
        <span style={{ ...S.row, gap: 6, flexWrap: 'wrap', justifyContent: 'flex-end', marginLeft: 'auto' }}>{actions}</span>
      </div>
    </section>
  )
}
