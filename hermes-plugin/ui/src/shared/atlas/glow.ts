// The glow under the sound map's sounds, on its canvas: the layout's nebula (`nebula.ts`, drawn scaled from a small
// picture kept per layout) and a soft halo under each sound (one sprite per colour and size, drawn additively). Both
// are kept per canvas and made off screen; a browser without an off-screen canvas draws the map without them.
import type { MapModel, MapViewport } from '../mapModel.ts'
import { isDark, lighten, type Rgb, rgba, rgbOf } from './color.ts'
import { NEBULA_PAD, NEBULA_SIDE, nebulaField } from './nebula.ts'

type Context = CanvasRenderingContext2D
type Picture = OffscreenCanvas

const GREY: Rgb = [128, 128, 128]
/** How many halo sprites one map keeps (a colour at a size each): a zoom passes through a few dozen sizes. */
const MOST_SPRITES = 240

interface Sheet {
  canvas: Picture
  context: OffscreenCanvasRenderingContext2D
}

function picture(width: number, height: number): Sheet | null {
  if (typeof OffscreenCanvas !== 'function') return null
  const canvas = new OffscreenCanvas(width, height)
  const context = canvas.getContext('2d')
  return context ? { canvas, context } : null
}

/** A layout's nebula as a picture, for the sounds `mask` keeps (1). */
interface Nebula {
  layout: MapModel
  mask: Uint8Array
  sheet: Sheet
}

export interface Halos {
  /** A halo under the sound at (`x`, `y`) in `color`, at `strength` of 1 (a sound arriving, or turning into a tile). */
  at(color: string, x: number, y: number, strength: number): void
  done(): void
}

export class Glow {
  private current: Nebula | null = null
  private last: Nebula | null = null
  private readonly sprites = new Map<string, Picture>()
  private readonly crowds = new WeakMap<MapModel, number>()
  private readonly shapes = new Map<string, string>()
  private ground = ''
  private dark = true

  /** Whether the canvas's ground is a dark one, from the theme colour the canvas resolved (a colour it can't read
   *  counts as dark, as both hosts' first themes are). */
  onDark(context: Context, ground: string): boolean {
    if (ground === this.ground) return this.dark
    this.ground = ground
    const kept = context.fillStyle
    context.fillStyle = '#000'
    context.fillStyle = ground
    const read = typeof context.fillStyle === 'string' ? rgbOf(context.fillStyle) : null
    context.fillStyle = kept
    this.dark = read ? isDark(read) : true
    this.shapes.clear()
    return this.dark
  }

  /** The colour a sound's shape is filled in: its family's, a little brighter on a dark ground so it stays crisp over
   *  its own glow. */
  shape(color: string): string {
    let shape = this.shapes.get(color)
    if (shape === undefined) {
      const channels = this.dark ? rgbOf(color) : null
      shape = channels ? lighten(channels, 0.2) : color
      this.shapes.set(color, shape)
    }
    return shape
  }

  /** The nebula of `layout` for the sounds `mask` keeps, made the first time it's asked for and kept until either
   *  changes; with it, the one the map showed before a new layout (for the move between them to fade out). Two
   *  pictures in all, drawn over in turn. */
  nebula(layout: MapModel, mask: Uint8Array, colorOf: (group: number) => string): { now: Picture | null; before: Picture | null } {
    if (this.current?.layout !== layout || this.current.mask !== mask) {
      const same = this.current?.layout === layout
      const sheet = (same ? this.current?.sheet : this.last?.sheet) ?? picture(NEBULA_SIDE, NEBULA_SIDE)
      if (!sheet) return { now: null, before: null }
      if (!same) this.last = this.current
      const pixels = nebulaField(
        layout.points,
        index => mask[index] === 1,
        group => rgbOf(colorOf(group)) ?? GREY
      )
      sheet.context.putImageData(new ImageData(pixels as Uint8ClampedArray<ArrayBuffer>, NEBULA_SIDE, NEBULA_SIDE), 0, 0)
      this.current = { layout, mask, sheet }
    }
    return { now: this.current.sheet.canvas, before: this.last && this.last.layout !== layout ? this.last.sheet.canvas : null }
  }

  /** Draw a nebula under the map at `view`, `unit` pixels to the map's square, at `alpha`. */
  draw(context: Context, nebula: Picture, view: MapViewport, unit: number, alpha: number): void {
    if (alpha <= 0) return
    const kept = context.globalAlpha
    context.globalAlpha = alpha
    context.imageSmoothingEnabled = true
    context.drawImage(nebula, view.dx - NEBULA_PAD * unit, view.dy - NEBULA_PAD * unit, (1 + 2 * NEBULA_PAD) * unit, (1 + 2 * NEBULA_PAD) * unit)
    context.globalAlpha = kept
  }

  /** How many sounds the layout's most crowded cell holds (of its 64 × 64 grid). */
  crowd(layout: MapModel): number {
    let crowd = this.crowds.get(layout)
    if (crowd === undefined) {
      crowd = 0
      for (const cell of layout.cells.values()) crowd = Math.max(crowd, cell.length)
      this.crowds.set(layout, crowd)
    }
    return crowd
  }

  /** Halos of `radius` pixels at `alpha`, on a canvas drawn at `ratio` device pixels to one: each `at` adds one, `done`
   *  hands the canvas back as it was. Null when there's nothing to draw them with, or nothing to see. */
  halos(context: Context, radius: number, alpha: number, ratio: number): Halos | null {
    if (alpha < 0.02 || radius < 1 || typeof OffscreenCanvas !== 'function') return null
    // Sizes in half pixels: a zoom passes through few of them, and a sprite a quarter pixel off is the same halo.
    const size = Math.round(radius * 2) / 2
    const side = Math.max(2, Math.ceil(size * 2 * ratio))
    const [keptAlpha, keptBlend] = [context.globalAlpha, context.globalCompositeOperation]
    context.globalCompositeOperation = 'lighter'
    let strong = -1
    let color = ''
    let sprite: Picture | null = null
    return {
      at: (next, x, y, strength) => {
        if (strength <= 0.02) return
        if (next !== color) {
          color = next
          sprite = this.sprite(next, side)
        }
        if (!sprite) return
        // A tenth of a step in strength is as fine as an eye tells: the canvas's alpha changes that much less often.
        const stepped = Math.round(Math.min(1, strength) * 10) / 10
        if (stepped !== strong) {
          strong = stepped
          context.globalAlpha = alpha * stepped
        }
        context.drawImage(sprite, x - size, y - size, size * 2, size * 2)
      },
      done: () => {
        context.globalAlpha = keptAlpha
        context.globalCompositeOperation = keptBlend
      }
    }
  }

  /** A soft round sprite in one colour, `side` device pixels across: bright in the middle, gone at its edge. */
  private sprite(color: string, side: number): Picture | null {
    const key = `${color}|${side}`
    const kept = this.sprites.get(key)
    if (kept) return kept
    const channels = rgbOf(color)
    const made = channels ? picture(side, side) : null
    if (!made || !channels) return null
    const middle = side / 2
    const fade = made.context.createRadialGradient(middle, middle, 0, middle, middle, middle)
    fade.addColorStop(0, rgba(channels, 1))
    fade.addColorStop(0.35, rgba(channels, 0.45))
    fade.addColorStop(1, rgba(channels, 0))
    made.context.fillStyle = fade
    made.context.fillRect(0, 0, side, side)
    if (this.sprites.size >= MOST_SPRITES) this.sprites.clear()
    this.sprites.set(key, made.canvas)
    return made.canvas
  }
}

const glows = new WeakMap<HTMLCanvasElement, Glow>()

/** The glow kept for a map's canvas: its nebula and sprites live as long as the canvas does. */
export function glowOf(canvas: HTMLCanvasElement): Glow {
  let glow = glows.get(canvas)
  if (!glow) {
    glow = new Glow()
    glows.set(canvas, glow)
  }
  return glow
}
