// What moves over the sound map, on a canvas of its own above the map's: the pulse on the sound that is playing (rings
// and a glow that reach as far as it is loud right now), and, with hover-to-play on, the pointer's trail in the colours
// of what it played. It draws only while there is something to draw: its animation frame runs while a sound plays or
// a trail fades, and never while the map sits still, so the map's own canvas isn't redrawn for it. A system asking for
// less motion gets the pulse drawn once, still, and no trail.
import { traceGlyph } from '../mapModel.ts'
import { lighten, rgba, rgbOf } from './color.ts'
import { follow, levelAt, progressOf } from './pulse.ts'
import { type Step, stepTo, strengthOf } from './trail.ts'

/** The sound that plays, as the overlay draws it: where it is now (the map moves under a pan), its colour and glyph,
 *  how long it is, and its waveform once decoded (null until then: a plain two-ring pulse). */
export interface Pulse {
  place: () => readonly [number, number] | null
  color: string
  glyph: number
  duration: number
  levels: readonly number[] | null
}

/** A sound ringed on the map: where it is now, and its colour. */
export interface Ringed {
  place: () => readonly [number, number] | null
  color: string
}

/** How long the pulse takes to fade once its sound stops, and the plain pulse's period. */
const FADE_MS = 180
const RIPPLE_MS = 1100

export class Overlay {
  private readonly context: CanvasRenderingContext2D | null
  private width = 0
  private height = 0
  private ratio = 1
  private frame = 0
  private pulse: (Pulse & { since: number; shown: number; at: number }) | null = null
  /** The pulse that just stopped, fading where it was, and when it stopped. */
  private fading: { pulse: Pulse; shown: number; since: number } | null = null
  /** The core's colour: the canvas's own CSS colour (the theme's text colour), read when a sound starts. */
  private core = '#fff'
  /** The pointer's trail: its steps, the colour of the sound that last played (null before any has), and whether the
   *  pointer left or pressed since its last step (the next one starts a new line). */
  private path: Step[] = []
  private tone: string | null = null
  private lifted = true
  /** The sounds ringed on the map: the parents of the made sound under the pointer. */
  private ringed: Ringed[] = []
  still = false

  constructor(private readonly canvas: HTMLCanvasElement) {
    this.context = canvas.getContext('2d')
  }

  resize(width: number, height: number, ratio: number): void {
    if (width === this.width && height === this.height && ratio === this.ratio) return
    ;[this.width, this.height, this.ratio] = [width, height, ratio]
    this.canvas.width = Math.round(width * ratio)
    this.canvas.height = Math.round(height * ratio)
    this.draw(performance.now())
  }

  /** A sound starts playing (its pulse begins), or the sound stops (null: its pulse fades). */
  play(pulse: Pulse | null): void {
    const now = performance.now()
    if (this.pulse && !pulse && !this.still) this.fading = { pulse: this.pulse, shown: this.pulse.shown, since: now }
    this.pulse = pulse ? { ...pulse, since: now, shown: 0, at: now } : null
    if (pulse) this.core = getComputedStyle(this.canvas).color || this.core
    if (pulse) this.fading = null
    // The trail's colour: the playing sound's, a little lighter, so the trail reads across its own family's cluster.
    const channels = pulse ? rgbOf(pulse.color) : null
    if (pulse) this.tone = channels ? lighten(channels, 0.3) : pulse.color
    this.wake()
  }

  /** The playing sound's waveform arrived. */
  levels(levels: readonly number[] | null): void {
    if (this.pulse) this.pulse.levels = levels
  }

  /** The pointer moved to (`x`, `y`) over the map with hover-to-play on: its trail grows, in the colour of the sound
   *  that last played (nothing has played yet: no trail). */
  trail(x: number, y: number): void {
    if (this.still || !this.tone) return
    this.path = stepTo(this.path, x, y, performance.now(), this.tone, this.lifted)
    this.lifted = false
    this.wake()
  }

  /** The pointer left the map, or pressed on it: what it draws next starts a new line. */
  lift(): void {
    this.lifted = true
  }

  /** Ring these sounds (the sounds a made sound came from, while the pointer or the keyboard is on it), or none. */
  kin(ringed: Ringed[]): void {
    this.ringed = ringed
    if (this.frame === 0) this.draw(performance.now())
  }

  /** The map moved under what stands still here (a still pulse, the ringed parents): draw them where they are now. */
  moved(): void {
    if (this.frame === 0 && (this.pulse || this.fading || this.ringed.length > 0)) this.draw(performance.now())
  }

  dispose(): void {
    cancelAnimationFrame(this.frame)
    this.frame = 0
    this.pulse = null
    this.fading = null
    this.path = []
  }

  private wake(): void {
    if (this.still) {
      this.draw(performance.now())
      return
    }
    if (this.frame === 0) this.frame = requestAnimationFrame(this.tick)
  }

  private tick = (now: number): void => {
    this.frame = 0
    this.draw(now)
    if (this.pulse || this.fading || this.path.length > 0) this.frame = requestAnimationFrame(this.tick)
  }

  private draw(now: number): void {
    const context = this.context
    if (!context || this.width === 0) return
    context.setTransform(this.ratio, 0, 0, this.ratio, 0, 0)
    context.clearRect(0, 0, this.width, this.height)
    this.path = this.path.filter(step => strengthOf(step, now) > 0)
    if (this.path.length > 1) {
      // Each step a short stroke from the one before, thinner and fainter as it ages: a comet's tail, widest and
      // brightest at the pointer.
      context.save()
      context.globalCompositeOperation = 'lighter'
      // Butted, not round: round ends would overlap at every joint, and added light makes each overlap a bead.
      context.lineCap = 'butt'
      for (let index = 1; index < this.path.length; index++) {
        const [from, to] = [this.path[index - 1], this.path[index]]
        if (!to.joined) continue
        const strength = strengthOf(to, now)
        context.strokeStyle = to.color
        context.globalAlpha = 0.15 + 0.75 * strength ** 1.3
        context.lineWidth = 1 + 5 * strength ** 1.2
        context.beginPath()
        context.moveTo(from.x, from.y)
        context.lineTo(to.x, to.y)
        context.stroke()
      }
      context.restore()
    }
    // A made sound's parents: each ringed in its own colour, with a little light behind it. The light adds up, so
    // many parents share it (a crowd of them zoomed out is a glow, not a white blot); the rings don't add.
    if (this.ringed.length > 0) {
      context.save()
      context.lineWidth = 1.75
      const light = Math.min(0.6, 3 / this.ringed.length)
      for (const ringed of this.ringed) {
        const place = ringed.place()
        if (!place || place[0] < -20 || place[1] < -20 || place[0] > this.width + 20 || place[1] > this.height + 20) continue
        const channels = rgbOf(ringed.color)
        if (channels) {
          const glow = context.createRadialGradient(place[0], place[1], 0, place[0], place[1], 16)
          glow.addColorStop(0, rgba(channels, 0.7))
          glow.addColorStop(1, rgba(channels, 0))
          context.globalCompositeOperation = 'lighter'
          context.globalAlpha = light
          context.fillStyle = glow
          context.fillRect(place[0] - 16, place[1] - 16, 32, 32)
        }
        context.globalCompositeOperation = 'source-over'
        context.globalAlpha = 0.95
        context.strokeStyle = channels ? lighten(channels, 0.3) : ringed.color
        context.beginPath()
        context.arc(place[0], place[1], 7.5, 0, Math.PI * 2)
        context.stroke()
      }
      context.restore()
    }
    if (this.fading) {
      const left = 1 - (now - this.fading.since) / FADE_MS
      if (left <= 0) this.fading = null
      else this.ring(context, this.fading.pulse, this.fading.shown, left, null)
    }
    const pulse = this.pulse
    if (!pulse) return
    const progress = progressOf(now - pulse.since, pulse.duration)
    if (pulse.levels && progress !== null && !this.still) {
      pulse.shown = follow(pulse.shown, levelAt(pulse.levels, progress), now - pulse.at)
      pulse.at = now
      this.ring(context, pulse, pulse.shown, 1, null)
    } else {
      // No waveform yet (or none to be had), or past the length the map knows: the plain pulse, two rings spreading
      // from the sound; still, for a system asking for less motion.
      this.ring(context, pulse, 0.35, 1, this.still ? -1 : (now - pulse.since) / RIPPLE_MS)
    }
  }

  /** One pulse: a glow and three rings reaching as far as `level`, and the sound's own shape bright at the middle.
   *  With `ripple` (a phase), two rings spread from it instead (the plain pulse); a negative one holds them still. */
  private ring(context: CanvasRenderingContext2D, pulse: Pulse, level: number, strength: number, ripple: number | null): void {
    const place = pulse.place()
    if (!place) return
    const [x, y] = place
    if (x < -60 || y < -60 || x > this.width + 60 || y > this.height + 60) return
    const channels = rgbOf(pulse.color)
    context.save()
    context.globalCompositeOperation = 'lighter'
    if (channels) {
      const reach = 26 + 30 * level
      const glow = context.createRadialGradient(x, y, 0, x, y, reach)
      glow.addColorStop(0, rgba(channels, 1))
      glow.addColorStop(0.35, rgba(channels, 0.45))
      glow.addColorStop(1, rgba(channels, 0))
      context.globalAlpha = (0.5 + 0.4 * level) * strength
      context.fillStyle = glow
      context.fillRect(x - reach, y - reach, reach * 2, reach * 2)
    }
    // Rings a little lighter than the family's colour: a pulse in the middle of its own crowded cluster still reads.
    context.strokeStyle = channels ? lighten(channels, 0.3) : pulse.color
    if (ripple === null) {
      context.lineWidth = 1.75
      for (let ring = 0; ring < 3; ring++) {
        context.globalAlpha = (0.9 - ring * 0.25) * strength
        context.beginPath()
        context.arc(x, y, 9 + ring * (7 + 9 * level), 0, Math.PI * 2)
        context.stroke()
      }
    } else {
      for (const offset of ripple < 0 ? [0.25] : [0, 0.5]) {
        const phase = ripple < 0 ? offset : (((ripple + offset) % 1) + 1) % 1
        context.globalAlpha = 0.7 * (1 - phase) ** 1.5 * strength
        context.lineWidth = 2 - phase
        context.beginPath()
        context.arc(x, y, 8 + phase * 26, 0, Math.PI * 2)
        context.stroke()
      }
    }
    context.restore()
    // The sound's own shape, bright: what plays is still the shape it was.
    const core = new Path2D()
    traceGlyph(core, pulse.glyph, x, y, 4)
    context.globalAlpha = strength
    context.fillStyle = this.core
    context.fill(core)
    context.globalAlpha = 1
  }
}
