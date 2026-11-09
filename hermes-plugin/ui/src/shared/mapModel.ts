// The sound map as plain data the canvas draws: points with their group colours, a grid for finding the point under
// the pointer, the view (zoom and pan) and the search. No DOM here: tested in ui/tests.
import { clean, num } from './format.ts'
import { english, type Translate } from './i18n.ts'
import type { FoundMatch } from './store.ts'
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
  return { revision: map?.revision ?? null, points, groups, sounds: map?.sounds ?? {}, cells }
}

/** The square-tile view's grid: `side` × `side` tiles over the map's square, each sound on its own tile near where the
 *  layout put it. `col`/`row` are each point's tile; `cells` the point on each tile (row-major), -1 for none. */
export interface TileLayout {
  side: number
  col: Int32Array
  row: Int32Array
  cells: Int32Array
}

/** How much bigger than the sounds the tiled area is: a few empty tiles keep the clusters' outlines. */
const TILE_ROOM = 1.12

/** Snap every point to its own tile, keeping the map's shape: the grid is as fine as the area the map covers needs, and
 *  each point, from the middle out, takes the free tile nearest its own place. Each row's free tiles are found through
 *  two union-finds (nearest free to the right, to the left), so a crowded cluster spills into its surroundings at the
 *  cost of a few rows read, and a 25,000-sound map lays out in a few tens of milliseconds. */
export function tileLayout(model: MapModel): TileLayout {
  const points = model.points
  const count = points.length
  // The area the map covers, on a coarse grid: the tiles fill about that much of the square.
  const COARSE = 40
  const covered = new Uint8Array(COARSE * COARSE)
  for (const point of points) covered[Math.min(COARSE - 1, Math.floor(point.y * COARSE)) * COARSE + Math.min(COARSE - 1, Math.floor(point.x * COARSE))] = 1
  const area = Math.max(0.05, covered.reduce((sum, cell) => sum + cell, 0) / (COARSE * COARSE))
  const side = Math.max(2, Math.ceil(Math.sqrt((Math.max(1, count) * TILE_ROOM) / area)))
  const span = side + 1
  // Per row: `right[c]` leads to the first free tile at or after c (side: none); `left[c + 1]` to the last free at or
  // before c (offset by one, 0: none).
  const right = new Int32Array(side * span)
  const left = new Int32Array(side * span)
  for (let r = 0; r < side; r++) {
    for (let c = 0; c < span; c++) {
      right[r * span + c] = c
      left[r * span + c] = c
    }
  }
  const find = (links: Int32Array, base: number, at: number): number => {
    let x = at
    while (links[base + x] !== x) {
      links[base + x] = links[base + links[base + x]]
      x = links[base + x]
    }
    return x
  }
  const col = new Int32Array(count)
  const row = new Int32Array(count)
  const cells = new Int32Array(side * side).fill(-1)
  let cx = 0
  let cy = 0
  for (const point of points) {
    cx += point.x / Math.max(1, count)
    cy += point.y / Math.max(1, count)
  }
  const order = points.map((_, index) => index).sort((a, b) => (points[a].x - cx) ** 2 + (points[a].y - cy) ** 2 - ((points[b].x - cx) ** 2 + (points[b].y - cy) ** 2) || a - b)
  for (const index of order) {
    const tx = points[index].x * side - 0.5
    const ty = points[index].y * side - 0.5
    const r0 = Math.min(side - 1, Math.max(0, Math.round(ty)))
    const c0 = Math.min(side - 1, Math.max(0, Math.round(tx)))
    let best = Infinity
    let bestRow = -1
    let bestCol = -1
    const consider = (r: number, c: number) => {
      const distance = (c - tx) ** 2 + (r - ty) ** 2
      if (distance < best) {
        best = distance
        bestRow = r
        bestCol = c
      }
    }
    for (let dy = 0; dy < side; dy++) {
      const near = Math.max(0, dy - 0.5)
      if (near * near >= best) break
      for (const r of dy === 0 ? [r0] : [r0 - dy, r0 + dy]) {
        if (r < 0 || r >= side) continue
        const base = r * span
        const after = find(right, base, c0)
        if (after < side) consider(r, after)
        const before = find(left, base, c0 + 1) - 1
        if (before >= 0) consider(r, before)
      }
    }
    // The grid always has room: side² ≥ count by construction.
    col[index] = bestCol
    row[index] = bestRow
    cells[bestRow * side + bestCol] = index
    right[bestRow * span + bestCol] = bestCol + 1
    left[bestRow * span + bestCol + 1] = bestCol
  }
  return { side, col, row, cells }
}

/** The map as the tile view draws and picks it: every point at its tile's centre. */
export function tileModel(model: MapModel, layout: TileLayout): MapModel {
  const cells = new Map<number, number[]>()
  const points = model.points.map((point, index) => {
    const tiled = { ...point, x: (layout.col[index] + 0.5) / layout.side, y: (layout.row[index] + 0.5) / layout.side }
    const cell = cellOf(tiled.x, tiled.y)
    const list = cells.get(cell)
    if (list) list.push(index)
    else cells.set(cell, [index])
    return tiled
  })
  return { ...model, points, cells }
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

/** How the wheel moves the map: `zoom` (a mouse's wheel), `pan` (a trackpad's two-finger scroll), or `auto`, which
 *  tells them apart (`wheelPans`). A pinch always zooms. */
export type WheelMode = 'auto' | 'pan' | 'zoom'

/** One gesture's verdict, kept across its events: within `WHEEL_GESTURE_MS` of the last one, it's the same gesture. */
export interface WheelMemory {
  pans: boolean | null
  at: number
}

export const WHEEL_GESTURE_MS = 300
/** macOS's step for a mouse's wheel in Chromium and Safari: every notch moves a whole number of these. */
const MAC_WHEEL_STEP = 4.000244140625

/** Whether a wheel event that isn't a pinch (the browser sends a pinch as a wheel with Ctrl held) pans the map, as a
 *  trackpad's two-finger scroll does, rather than zooming, as a mouse's wheel does. A delta in lines or pages, or one in
 *  macOS's wheel steps, is a mouse's. Otherwise the legacy `wheelDelta` tells: Chromium and Safari give a precise
 *  device's (a trackpad's) as −3 × its delta, a wheel's notches otherwise (120 per 100 px on Windows, 120 per macOS
 *  step), and both scale alike with the page's zoom; with no legacy delta at all, pixel deltas are a trackpad's. A
 *  gesture keeps the verdict its first event had, so one event that looks like a notch doesn't switch a swipe to
 *  zooming. */
export function wheelPans(
  event: { deltaX: number; deltaY: number; deltaMode: number; wheelDeltaX?: number; wheelDeltaY?: number },
  memory: WheelMemory,
  now: number
): boolean {
  const going = memory.pans !== null && now - memory.at <= WHEEL_GESTURE_MS
  memory.at = now
  if (going) return memory.pans as boolean
  const vertical = Math.abs(event.deltaY) >= Math.abs(event.deltaX)
  const delta = vertical ? event.deltaY : event.deltaX
  const legacy = vertical ? event.wheelDeltaY : event.wheelDeltaX
  if (event.deltaMode !== 0 || (delta !== 0 && delta % MAC_WHEEL_STEP === 0)) memory.pans = false
  // Legacy deltas are whole numbers: within rounding of −3 × the delta is a precise device's.
  else if (typeof legacy === 'number' && legacy !== 0 && delta !== 0) memory.pans = Math.abs(legacy + 3 * delta) <= 1.5
  else memory.pans = true
  return memory.pans
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

// Each map's search text per point (its name, class, kind and folder, lowercased), built on the map's first search
// and kept with it: opening a 25,000-sound map doesn't pay for it, and every later keystroke is one pass of `includes`.
const haystacks = new WeakMap<MapModel, string[]>()

function haystackOf(model: MapModel): string[] {
  let text = haystacks.get(model)
  if (!text) {
    text = model.points.map(point => {
      const sound = model.sounds[point.id]
      return [sound?.name, sound?.class, sound?.kind, sound?.folder].filter(value => typeof value === 'string').join(' ').toLowerCase()
    })
    haystacks.set(model, text)
  }
  return text
}

/** CC-23c: a sound's indexed tempo, key and root note as one line (`124 BPM · A minor`), from the map's own metadata;
 *  '' when the index has none. */
export function indexedValues(sound: MapSound | undefined, t: Translate = english): string {
  if (!sound) return ''
  const parts: string[] = []
  if (typeof sound.bpm === 'number' && Number.isFinite(sound.bpm) && sound.bpm > 0) parts.push(`${num(sound.bpm, 1)} BPM`)
  const key = clean(sound.key, 24)
  if (key) parts.push(key)
  const note = clean(sound.note, 8)
  if (note) parts.push(t('map.note', note))
  return parts.join(' · ')
}

/** CC-23: what a find said of one hit, as one line: on a near search its closeness (relative to this map, never a
 *  percentage), then its reasons, in Kumi's order. '' when it said nothing. */
export function foundLine(match: FoundMatch | undefined, near: boolean, t: Translate = english): string {
  if (!match) return ''
  const closeness = near && typeof match.closeness === 'number' ? [t('map.closeness', String(match.closeness))] : []
  return [...closeness, ...match.why].join(' · ')
}

/** The sounds a search names: every word must appear in the name, class, kind or folder. Null for no search. */
export function searchSounds(model: MapModel, query: string): Set<string> | null {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  if (!words.length) return null
  const text = haystackOf(model)
  const found = new Set<string>()
  model.points.forEach((point, index) => {
    if (words.every(word => text[index].includes(word))) found.add(point.id)
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

/** The points inside a screen box (a drag's corners, any order), among those `visible` allows, in the map's own order:
 *  what a box selection picks. Read through the grid, so a box over a corner of a 25,000-sound map tests only the
 *  points near it. */
export function pointsInBox(
  model: MapModel,
  view: MapViewport,
  size: number,
  box: [number, number, number, number],
  visible?: (index: number) => boolean
): number[] {
  const unit = size * view.scale
  if (unit <= 0) return []
  const [ax, ay, bx, by] = box
  const x0 = (Math.min(ax, bx) - view.dx) / unit
  const x1 = (Math.max(ax, bx) - view.dx) / unit
  const y0 = (Math.min(ay, by) - view.dy) / unit
  const y1 = (Math.max(ay, by) - view.dy) / unit
  if (x1 < 0 || y1 < 0 || x0 > 1 || y0 > 1) return []
  const cell = (value: number) => Math.min(GRID - 1, Math.max(0, Math.floor(value * GRID)))
  const out: number[] = []
  for (let cy = cell(y0); cy <= cell(y1); cy++) {
    for (let cx = cell(x0); cx <= cell(x1); cx++) {
      for (const index of model.cells.get(cy * GRID + cx) ?? []) {
        const point = model.points[index]
        if (point.x >= x0 && point.x <= x1 && point.y >= y0 && point.y <= y1 && (!visible || visible(index))) out.push(index)
      }
    }
  }
  return out.sort((a, b) => a - b)
}
