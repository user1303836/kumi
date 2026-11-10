// A small waveform in a sound's own colours: bars mirrored about the middle, the part already played bright and a
// playhead where it is now. One colour for a sound from the map; its parents' colours, left to right, for a sound Kumi
// made from them. Drawn on a canvas, which takes no CSS variable: colours come resolved (`rgb(…)`, `#rrggbb`), or the
// canvas's own CSS colour stands in.
import { T } from '@kumi/theme'
import { useEffect, useRef, useState } from 'react'

import { rgbOf } from './color.ts'
import { progressOf } from './pulse.ts'

/** How far through the sound a play is (0 to 1), a frame at a time while `playing`; null when it isn't, or is over. */
export function usePlayhead(playing: boolean, duration: number): number | null {
  const [startedAt, setStartedAt] = useState<number | null>(null)
  const [now, setNow] = useState(0)
  useEffect(() => {
    setStartedAt(playing ? performance.now() : null)
    setNow(performance.now())
  }, [playing])
  const progress = startedAt === null ? null : progressOf(now - startedAt, duration)
  const running = progress !== null
  useEffect(() => {
    if (!running) return
    const frame = requestAnimationFrame(setNow)
    return () => cancelAnimationFrame(frame)
  }, [running, now])
  return progress
}

export function Wave({ peaks, colors, width, height, progress = null }: { peaks: readonly number[] | null; colors: readonly (string | null)[]; width: number; height: number; progress?: number | null }) {
  const canvas = useRef<HTMLCanvasElement | null>(null)
  const tint = colors.join()
  useEffect(() => {
    const element = canvas.current
    const context = element?.getContext('2d')
    if (!element || !context || width <= 0) return
    const ratio = window.devicePixelRatio || 1
    element.width = Math.round(width * ratio)
    element.height = Math.round(height * ratio)
    context.setTransform(ratio, 0, 0, ratio, 0, 0)
    context.clearRect(0, 0, width, height)
    // A colour the canvas can't take (a theme variable, or none known) is the canvas's own CSS colour, the accent.
    const own = getComputedStyle(element).color
    const tints = colors.map(color => (color && rgbOf(color) ? color : own))
    if (tints.length > 1) {
      const across = context.createLinearGradient(0, 0, width, 0)
      tints.forEach((color, index) => across.addColorStop(index / (tints.length - 1), color))
      context.fillStyle = across
    } else context.fillStyle = tints[0] ?? own
    // A bar every three pixels, each the loudest of the peaks it covers.
    const bars = Math.max(1, Math.floor(width / 3))
    for (let bar = 0; bar < bars; bar++) {
      let peak = 0.06
      if (peaks && peaks.length > 0) {
        const [from, to] = [Math.floor((bar * peaks.length) / bars), Math.max(Math.floor((bar * peaks.length) / bars) + 1, Math.floor(((bar + 1) * peaks.length) / bars))]
        for (let at = from; at < to && at < peaks.length; at++) peak = Math.max(peak, peaks[at])
      }
      const tall = Math.max(1, peak * (height - 2))
      context.globalAlpha = !peaks ? 0.3 : progress !== null && bar / bars <= progress ? 1 : progress !== null ? 0.5 : 0.9
      context.fillRect(bar * 3, (height - tall) / 2, 2, tall)
    }
    context.globalAlpha = 1
  }, [peaks, tint, width, height, progress]) // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <span aria-hidden style={{ position: 'relative', display: 'block', flex: 'none', width, height }}>
      <canvas ref={canvas} style={{ display: 'block', width, height, color: T.accent }} />
      {progress !== null && progress > 0 && progress < 1 && <span style={{ position: 'absolute', top: -1, bottom: -1, left: progress * width, width: 1.5, borderRadius: 1, background: T.text }} />}
    </span>
  )
}
