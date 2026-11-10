// The nebula under the sound map, as plain numbers (no DOM here: tested in ui/tests): how dense each family's sounds
// are across a layout, as a small square picture in map space. The canvas draws it scaled under the sounds, so a pan or
// a zoom moves it for nothing and it is rebuilt only when the layout, or what the legend and the search keep, changes.
import type { Rgb } from './color.ts'

/** The picture's side in cells, and how far past the map's unit square it reaches on every side (a glow at the map's
 *  edge needs room to fade). */
export const NEBULA_SIDE = 128
export const NEBULA_PAD = 0.12
/** How far each sound's light spreads, in cells: a core (two box blurs of `SPREAD` make a tent about a fiftieth of the
 *  map wide) and a wide skirt (two more of `SKIRT`: about twice that), `CORE` of it in the core. A cluster glows where
 *  its sounds are and fades well past its last one. */
const SPREAD = 2
const SKIRT = 5
const CORE = 0.55
/** What one sound alone adds at its own cell: two box blurs each way leave a `SPREAD` tent's peak there. */
const ALONE = CORE / (2 * SPREAD + 1) ** 2
/** How many sounds on one spot glow as brightly as the map's densest place may: a library of a dozen sounds gets a
 *  faint light under each, never a spotlight. */
const CROWD = 8
/** The glow at the map's densest place, of 1: short of full, so the sounds there still read over it. */
const BRIGHTEST = 0.85

/** One box blur of radius `radius` along rows (`step` 1) or columns (`step` `side`), from `from` into `into`. */
function blur(from: Float32Array, into: Float32Array, side: number, radius: number, step: number): void {
  const across = step === 1 ? side : 1
  const scale = 1 / (2 * radius + 1)
  for (let line = 0; line < side; line++) {
    const start = line * across
    let sum = 0
    for (let k = 0; k <= radius && k < side; k++) sum += from[start + k * step]
    for (let k = 0; k < side; k++) {
      into[start + k * step] = sum * scale
      const enters = k + radius + 1
      const leaves = k - radius
      if (enters < side) sum += from[start + enters * step]
      if (leaves >= 0) sum -= from[start + leaves * step]
    }
  }
}

/** The nebula of a layout: RGBA bytes (not premultiplied), `side` × `side`, over the map's square and `NEBULA_PAD`
 *  around it. Each sound `keep` allows adds its family's colour around its place; a cell's colour is the mix of the
 *  families there and its alpha how dense they are, eased so the densest place glows at `BRIGHTEST` and never whiter:
 *  a crowded core keeps its family's colour. */
export function nebulaField(
  points: readonly { x: number; y: number; group: number }[],
  keep: (index: number) => boolean,
  colorOf: (group: number) => Rgb,
  side = NEBULA_SIDE
): Uint8ClampedArray {
  const cells = side * side
  const sums = [new Float32Array(cells), new Float32Array(cells), new Float32Array(cells), new Float32Array(cells)]
  const [red, green, blue, weight] = sums
  const span = side / (1 + 2 * NEBULA_PAD)
  const colors = new Map<number, Rgb>()
  for (let index = 0; index < points.length; index++) {
    if (!keep(index)) continue
    const point = points[index]
    let color = colors.get(point.group)
    if (!color) {
      color = colorOf(point.group)
      colors.set(point.group, color)
    }
    // Shared between the four cells around it, so a sound between two cells glows between them.
    const fx = (point.x + NEBULA_PAD) * span - 0.5
    const fy = (point.y + NEBULA_PAD) * span - 0.5
    const x0 = Math.floor(fx)
    const y0 = Math.floor(fy)
    if (x0 < 0 || y0 < 0 || x0 + 1 >= side || y0 + 1 >= side) continue
    const right = fx - x0
    const down = fy - y0
    for (let corner = 0; corner < 4; corner++) {
      const share = (corner & 1 ? right : 1 - right) * (corner & 2 ? down : 1 - down)
      const cell = (y0 + (corner >> 1)) * side + x0 + (corner & 1)
      red[cell] += color[0] * share
      green[cell] += color[1] * share
      blue[cell] += color[2] * share
      weight[cell] += share
    }
  }
  const spare = new Float32Array(cells)
  const core = new Float32Array(cells)
  for (const sum of sums) {
    for (let pass = 0; pass < 2; pass++) {
      blur(sum, spare, side, SPREAD, 1)
      blur(spare, sum, side, SPREAD, side)
    }
    core.set(sum)
    for (let pass = 0; pass < 2; pass++) {
      blur(sum, spare, side, SKIRT, 1)
      blur(spare, sum, side, SKIRT, side)
    }
    for (let cell = 0; cell < cells; cell++) sum[cell] = CORE * core[cell] + (1 - CORE) * sum[cell]
  }
  let peak = 0
  for (let cell = 0; cell < cells; cell++) if (weight[cell] > peak) peak = weight[cell]
  const pixels = new Uint8ClampedArray(cells * 4)
  if (peak <= 0) return pixels
  const gain = -Math.log(1 - BRIGHTEST) / Math.max(peak, ALONE * CROWD)
  for (let cell = 0; cell < cells; cell++) {
    const here = weight[cell]
    if (here <= 1e-9) continue
    pixels[4 * cell] = red[cell] / here
    pixels[4 * cell + 1] = green[cell] / here
    pixels[4 * cell + 2] = blue[cell] / here
    pixels[4 * cell + 3] = 255 * (1 - Math.exp(-here * gain))
  }
  return pixels
}

/** How strongly the nebula shows at a zoom: whole when the map is all in view, calmer as it zooms in (the sounds' own
 *  shapes and names take over), and not worth drawing at all past a few times. */
export function nebulaAlpha(zoom: number): number {
  const alpha = 1 / Math.max(1, zoom) ** 1.35
  return alpha < 0.03 ? 0 : alpha
}

/** A halo's radius under a sound drawn `radius` pixels big, and how strong it is at a zoom, on a map whose densest
 *  64th-by-64th cell holds `crowd` sounds and whose square is `unit` pixels across: strong enough to lift a lone sound,
 *  weak enough that a crowded core adds up to a glow in its colour, never to white. */
export function haloOf(radius: number, zoom: number, crowd: number, unit: number): { radius: number; alpha: number } {
  const reach = radius * 3.3
  // A halo's light, summed over its sprite, is about 0.77 of its radius squared; a core's sounds per pixel times that
  // is how many halos overlap there.
  const overlap = ((crowd * 64 * 64) / (unit * unit)) * 0.77 * reach * reach
  return { radius: reach, alpha: Math.min(0.34 / Math.sqrt(Math.max(1, zoom)), overlap > 0 ? Math.max(0.16, 0.9 / overlap) : 1) }
}
