// The sound map as plain data the canvas draws: points with their group colours, a grid for finding the point under
// the pointer, the view (zoom and pan) and the search. No DOM here: tested in ui/tests.
import { clean } from './format.ts'
import type { MapSound, SoundMap } from './types.ts'

export interface MapPointView {
  id: string
  x: number
  y: number
  group: number
}

export interface MapGroupView {
  key: number
  name: string
  color: string
  count: number
}

export interface MapModel {
  revision: string | null
  points: MapPointView[]
  groups: MapGroupView[]
  sounds: Record<string, MapSound>
  /** Point indexes by grid cell (`GRID` × `GRID` over [0, 1]²). */
  cells: Map<number, number[]>
  /** Each point's search text, lowercased once per map: its name, class, kind and folder (`searchSounds`). */
  haystack: string[]
}

/** Where the map sits in its canvas: map x in [0, 1] → `x * size * scale + dx` pixels. */
export interface MapViewport {
  scale: number
  dx: number
  dy: number
}

const GRID = 64
const FALLBACK = ['#e85d4a', '#f5a623', '#50c8b4', '#7ed321', '#4a90e2', '#9b59b6', '#ec70c8', '#e5c440']

export function groupColor(color: unknown, key: number): string {
  if (Array.isArray(color) && color.length === 3 && color.every(c => Number.isInteger(c) && c >= 0 && c <= 255)) {
    return `rgb(${color[0]}, ${color[1]}, ${color[2]})`
  }
  if (typeof color === 'string' && /^#[0-9a-f]{6}$/i.test(color)) return color
  return FALLBACK[Math.abs(key) % FALLBACK.length]
}

const cellOf = (x: number, y: number): number => Math.min(GRID - 1, Math.floor(y * GRID)) * GRID + Math.min(GRID - 1, Math.floor(x * GRID))

export function buildModel(map: SoundMap | null): MapModel {
  const points: MapPointView[] = []
  const cells = new Map<number, number[]>()
  for (const raw of Array.isArray(map?.points) ? map.points : []) {
    if (!Array.isArray(raw) || raw.length < 4) continue
    const [id, x, y, group] = raw
    if (typeof id !== 'string' || !Number.isFinite(x) || !Number.isFinite(y)) continue
    const point = { id, x: Math.min(1, Math.max(0, x)), y: Math.min(1, Math.max(0, y)), group: Number.isInteger(group) ? group : -1 }
    const cell = cellOf(point.x, point.y)
    const list = cells.get(cell)
    if (list) list.push(points.length)
    else cells.set(cell, [points.length])
    points.push(point)
  }
  const groups = Object.entries(map?.groups ?? {})
    .map(([key, group]) => ({
      key: Number(key),
      name: clean(group?.name ?? `group ${key}`, 32),
      color: groupColor(group?.color, Number(key)),
      count: typeof group?.count === 'number' ? group.count : points.filter(point => point.group === Number(key)).length
    }))
    .filter(group => Number.isInteger(group.key))
    .sort((a, b) => a.key - b.key)
  const sounds = map?.sounds ?? {}
  const haystack = points.map(point => {
    const sound = sounds[point.id]
    return [sound?.name, sound?.class, sound?.kind, sound?.folder].filter(value => typeof value === 'string').join(' ').toLowerCase()
  })
  return { revision: map?.revision ?? null, points, groups, sounds, cells, haystack }
}

/** The view that shows the whole map in a `width` × `height` canvas, with a margin. */
export function fitView(width: number, height: number, margin = 16): MapViewport {
  const size = Math.max(1, Math.min(width, height) - 2 * margin)
  return { scale: 1, dx: (width - size) / 2, dy: (height - size) / 2 }
}

export function toScreen(view: MapViewport, size: number, x: number, y: number): [number, number] {
  return [x * size * view.scale + view.dx, y * size * view.scale + view.dy]
}

/** Zoom by `factor` around the screen point (`px`, `py`), within 1× to 40×. */
export function zoomAt(view: MapViewport, factor: number, px: number, py: number): MapViewport {
  const scale = Math.min(40, Math.max(1, view.scale * factor))
  const applied = scale / view.scale
  return { scale, dx: px - (px - view.dx) * applied, dy: py - (py - view.dy) * applied }
}

/** The point nearest the screen point (`px`, `py`) within `radius` pixels, among those `visible` allows; -1 if none. */
export function pick(model: MapModel, view: MapViewport, size: number, px: number, py: number, radius = 8, visible?: (index: number) => boolean): number {
  const unit = size * view.scale
  if (unit <= 0) return -1
  const mx = (px - view.dx) / unit
  const my = (py - view.dy) / unit
  const reach = radius / unit
  const from = { x: Math.max(0, Math.floor((mx - reach) * GRID)), y: Math.max(0, Math.floor((my - reach) * GRID)) }
  const to = { x: Math.min(GRID - 1, Math.floor((mx + reach) * GRID)), y: Math.min(GRID - 1, Math.floor((my + reach) * GRID)) }
  let best = -1
  let bestDistance = reach * reach
  for (let cy = from.y; cy <= to.y; cy++) {
    for (let cx = from.x; cx <= to.x; cx++) {
      for (const index of model.cells.get(cy * GRID + cx) ?? []) {
        if (visible && !visible(index)) continue
        const point = model.points[index]
        const distance = (point.x - mx) ** 2 + (point.y - my) ** 2
        if (distance <= bestDistance) {
          bestDistance = distance
          best = index
        }
      }
    }
  }
  return best
}

/** What a key does on the map. Only keys no host types with or claims first: Desktop's keybind dispatcher listens on
 *  the window in the capture phase, so it sees a key before the map does. It sends printable keys (Space, +, −, 0…) to
 *  the composer's draft, takes Enter to focus the composer and Escape to cancel the turn, and has PageUp and PageDown
 *  scroll the chat. So: the arrows move to the nearest sound that way, Shift+←/→ step through every sound in the map's
 *  own order (the arrows alone can't reach a sound on the same spot as another), Shift+↑/↓ zoom, and Home shows the
 *  whole map. Null for any other key, and for any key held with Alt, Control or Command. */
export type MapKey = { move: [number, number] } | { order: 1 | -1 } | { zoom: number } | { fit: true }

export function mapKey(event: { key: string; shiftKey: boolean; altKey: boolean; ctrlKey: boolean; metaKey: boolean }): MapKey | null {
  if (event.altKey || event.ctrlKey || event.metaKey) return null
  const direction = ({ ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] } as Record<string, [number, number]>)[event.key]
  if (direction) {
    if (!event.shiftKey) return { move: direction }
    return direction[0] ? { order: direction[0] as 1 | -1 } : { zoom: direction[1] < 0 ? 1.5 : 1 / 1.5 }
  }
  return event.key === 'Home' && !event.shiftKey ? { fit: true } : null
}

/** The sound a move or an order step picks, from point `from` (an index, or undefined before any): see `step` and
 *  `stepOrder`. -1 when there's none. */
export function moveWith(
  model: MapModel,
  view: MapViewport,
  size: number,
  from: number | undefined,
  key: { move: [number, number] } | { order: 1 | -1 },
  middle: [number, number],
  visible?: (index: number) => boolean
): number {
  return 'move' in key ? step(model, view, size, from, key.move, middle, visible) : stepOrder(model, from, key.order, visible)
}

/** The next (`delta` 1) or previous (-1) visible point in the map's own order, wrapping around; with no point yet, the
 *  first or the last. Every visible sound is one of these steps away, sounds on the same spot included. -1 when none is
 *  visible. */
export function stepOrder(model: MapModel, from: number | undefined, delta: 1 | -1, visible?: (index: number) => boolean): number {
  const count = model.points.length
  let index = from ?? (delta > 0 ? -1 : count)
  for (let tried = 0; tried < count; tried++) {
    index = (((index + delta) % count) + count) % count
    if (index !== from && (!visible || visible(index))) return index
  }
  return -1
}

/** The point an arrow key moves to: from point `from` (an index), the nearest visible one in `direction` (screen
 *  `[dx, dy]`, e.g. `[1, 0]` for right). One within 45° of that way wins; failing any, the nearest anywhere on that side.
 *  Nearer in line beats nearer off to the side. -1 when there's none that way; a point on the very same spot is never
 *  one (`stepOrder` reaches it). With no point yet (`from` undefined), the visible point nearest the middle of the
 *  screen `[cx, cy]`. */
export function step(
  model: MapModel,
  view: MapViewport,
  size: number,
  from: number | undefined,
  direction: [number, number],
  middle: [number, number],
  visible?: (index: number) => boolean
): number {
  const origin = from === undefined ? middle : toScreen(view, size, model.points[from].x, model.points[from].y)
  // [within 45°, anywhere on that side]
  const best = [-1, -1]
  const bestScore = [Infinity, Infinity]
  model.points.forEach((point, index) => {
    if (index === from || (visible && !visible(index))) return
    const [x, y] = toScreen(view, size, point.x, point.y)
    const ox = x - origin[0]
    const oy = y - origin[1]
    let score: number
    let side = 0
    if (from === undefined) {
      score = ox * ox + oy * oy
    } else {
      const along = ox * direction[0] + oy * direction[1]
      if (along <= 0.5) return
      const across = Math.abs(ox * direction[1] - oy * direction[0])
      score = along + 2 * across
      side = across <= along ? 0 : 1
    }
    if (score < bestScore[side]) {
      best[side] = index
      bestScore[side] = score
    }
  })
  return best[0] >= 0 ? best[0] : best[1]
}

/** The view moved just enough that screen point (`px`, `py`) sits inside a `width`×`height` box with `margin` around
 *  it; the same view when it already does. */
export function keepInView(view: MapViewport, px: number, py: number, width: number, height: number, margin = 24): MapViewport {
  const shiftX = px < margin ? margin - px : px > width - margin ? width - margin - px : 0
  const shiftY = py < margin ? margin - py : py > height - margin ? height - margin - py : 0
  return shiftX || shiftY ? { ...view, dx: view.dx + shiftX, dy: view.dy + shiftY } : view
}

/** A sound's name as the canvas draws it beside its point: `clean()`ed and cut at 28 characters. `fillText` applies
 *  the bidi algorithm, so a raw name holding a bidi control could reorder its own label. */
export function mapLabel(name: unknown): string {
  return clean(name, 28)
}

/** The sounds a search names: every word must appear in the name, class, kind or folder. Null for no search. The
 *  text is the model's `haystack`, built once per map, so a keystroke over 25,000 sounds is one pass of `includes`. */
export function searchSounds(model: MapModel, query: string): Set<string> | null {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  if (!words.length) return null
  const found = new Set<string>()
  model.points.forEach((point, index) => {
    const text = model.haystack[index]
    if (words.every(word => text.includes(word))) found.add(point.id)
  })
  return found
}

/** The view that centres the given points, zoomed in as far as they allow (for "show what the agent found"). */
export function frame(model: MapModel, ids: Set<string>, width: number, height: number, margin = 48): MapViewport | null {
  const chosen = model.points.filter(point => ids.has(point.id))
  if (!chosen.length) return null
  const xs = chosen.map(point => point.x)
  const ys = chosen.map(point => point.y)
  const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)]
  // The same map size `fitView` gives, zoomed so the found points fill the canvas less a margin.
  const size = Math.max(1, Math.min(width, height) - 32)
  const span = Math.max(x1 - x0, y1 - y0, 0.04)
  const scale = Math.min(40, Math.max(1, (Math.min(width, height) - 2 * margin) / (span * size)))
  const cx = (x0 + x1) / 2
  const cy = (y0 + y1) / 2
  return { scale, dx: width / 2 - cx * size * scale, dy: height / 2 - cy * size * scale }
}
