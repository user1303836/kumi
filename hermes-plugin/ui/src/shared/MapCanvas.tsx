// The sound map, drawn on a canvas: each sound of the library where Kumi's layout put it, coloured by its group.
// Wheel zooms, a drag pans, a click picks the sound under the pointer. Search and the legend dim what they leave out;
// sounds the agent just found get a ring. What a search or a find picks out is also listed by name, for the keyboard.
// Like native Kumi's map: a sound plays as the pointer rests on it (`onHover`), a box drawn with Shift held (or with
// Select on) picks the sounds in it, Command or Ctrl with a click picks one more, and the palette's sounds are marked.
// On the keyboard (`mapKey`), the arrows move to the nearest sound that way, Shift+←/→ step through every sound in turn,
// Shift+↑/↓ zoom and Home shows the whole map; each sound reached is named to a screen reader, and Tab goes on to its
// Play and Use….
// Both UI halves use it; what a picked sound can do comes from `actions`, what the picks can do from `pickedActions`.
import { CANVAS, T } from '@kumi/theme'
import type { KeyboardEvent, PointerEvent, ReactNode } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'

import { clean, num, seconds } from './format.ts'
import { buildModel, fitView, foundLine, frame, indexedValues, keepInView, mapKey, mapLabel, type MapViewport, moveWith, pick, pointsInBox, searchSounds, tileLayout, tileModel, toScreen, zoomAt } from './mapModel.ts'
import type { KumiState } from './store.ts'
import type { MapSound, SoundMap } from './types.ts'
import { Chip, FOCUSABLE, S, TextButton, useT } from './ui.tsx'

export interface MapCanvasProps {
  map: SoundMap | null
  found: KumiState['found']
  playing: string | null
  selected: string | null
  /** A click, or a found list's row: pick that sound and play it. */
  onPick: (id: string) => void
  /** The keyboard moved to a sound: picked, not played (its Play is the next tab stop). */
  onSelect: (id: string) => void
  actions?: (id: string, sound: MapSound) => ReactNode
  empty: ReactNode
  /** Sounds that couldn't play here (the store's `leaving`): they fade off the map at once, and can't be picked. */
  leaving?: ReadonlySet<string>
  /** What the quiet line about hidden sounds offers ("Show them", a rescan), when there are any. */
  hiddenActions?: ReactNode
  /** The palette's sounds on this map: marked on it. */
  marked?: ReadonlySet<string>
  /** The sounds picked to work with (a box, Command or Ctrl with a click): marked brighter. */
  picked?: ReadonlySet<string>
  /** A box drawn, a Command or Ctrl click, or "Pick all": `replace` the picks, `add` to them, or `toggle` one. Without
   *  it the map picks nothing (no Select, no box). */
  onPicked?: (ids: string[], how: 'add' | 'replace' | 'toggle') => void
  /** The pointer came to rest on a sound, or on none (null; it also left the map): hover-to-play listens here. */
  onHover?: (id: string | null) => void
  /** The parent's own toggles, at the end of the toolbar (hover to play). */
  tools?: ReactNode
  /** What the picks can do, in a bar while there are any ("Add 12 to palette", Clear). */
  pickedActions?: (ids: string[]) => ReactNode
  /** Square tiles instead of dots (the dots morph into them); `onTiles` shows the toggle and hears it. */
  tiles?: boolean
  onTiles?: (on: boolean) => void
  /** Sounds just gathered into the palette, which fly from where they are into the dock under the map. */
  flying?: { ids: string[]; at: number } | null
}

/** How many searched or found sounds are listed by name above the map; the rest stay rings on it. */
const MATCHES_LISTED = 6
/** How long a sound that couldn't play takes to fade off the map. */
const FADE_MS = 450
// The map's motion, all of it short and none of it while the map sits still: a new map's points pop in from the
// middle out, dots morph into tiles and back, gathered sounds fly into the dock, and a playing sound ripples.
const INTRO_MS = 900
const MORPH_MS = 760
const FLY_MS = 620
const FLY_STAGGER = 14
const FLY_MOST = 48
const RIPPLE_MS = 1100
/** How far apart, in a morph's span, the middle's points and the edge's move: the middle leads. */
const STAGGER = 0.55

const clamp01 = (value: number): number => Math.min(1, Math.max(0, value))
const easeInOut = (t: number): number => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2)
const easeOutBack = (t: number): number => 1 + 2.2 * (t - 1) ** 3 + 1.2 * (t - 1) ** 2

interface Morph {
  from: number
  to: number
  at: number
}

/** How far the dots-to-tiles morph is at `now`: 0 dots, 1 tiles. */
function morphAt(morph: Morph, now: number): number {
  return morph.from + (morph.to - morph.from) * easeInOut(clamp01((now - morph.at) / MORPH_MS))
}

interface Size {
  width: number
  height: number
}

function resolve(canvas: HTMLCanvasElement): Record<keyof typeof CANVAS, string> {
  const style = getComputedStyle(canvas)
  const read = (name: string, fallback: string): string => style.getPropertyValue(name).trim() || fallback
  return {
    text: read(CANVAS.text, '#999'),
    faint: read(CANVAS.faint, '#666'),
    border: read(CANVAS.border, '#444'),
    accent: read(CANVAS.accent, '#4a90e2'),
    strong: read(CANVAS.strong, '#ddd')
  }
}

// How many interleaved paths each group's points are drawn in: enough that overlapping groups mix about as the map's
// own order would, few enough that a pass stays a few hundred fills.
const ROUNDS = 8

// Each map's own id for its keys hint. Not React's useId: the hosts' React builds differ, and this needs none of it.
let maps = 0

export function MapCanvas({ map, found, playing, selected, onPick, onSelect, actions, empty, leaving, hiddenActions, marked, picked, onPicked, onHover, tools, pickedActions, tiles = false, onTiles, flying }: MapCanvasProps) {
  const t = useT()
  const model = useMemo(() => buildModel(map), [map])
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const boxRef = useRef<HTMLDivElement | null>(null)
  const [size, setSize] = useState<Size>({ width: 0, height: 0 })
  const [view, setView] = useState<MapViewport | null>(null)
  // What the search box holds, and what the map searches for: the latter a beat behind while the producer types, so a
  // 25,000-sound map redraws once per pause, not on every key.
  const [query, setQuery] = useState('')
  const [searchFor, setSearchFor] = useState('')
  const [hidden, setHidden] = useState<Set<number>>(() => new Set())
  const [hover, setHover] = useState<{ index: number; x: number; y: number } | null>(null)
  // A drag: a pan, or with Shift held (or Select on) a box that picks what's in it.
  const drag = useRef<{ x: number; y: number; moved: boolean; view: MapViewport; box: boolean; add: boolean } | null>(null)
  const [box, setBox] = useState<[number, number, number, number] | null>(null)
  const [selecting, setSelecting] = useState(false)
  // The sound the pointer last rested on, so `onHover` hears only changes.
  const rested = useRef<string | null>(null)
  const rest = (id: string | null) => {
    if (rested.current === id) return
    rested.current = id
    onHover?.(id)
  }
  // A drag or a wheel moves the view at most once per frame: the latest view waits here for the next animation frame.
  const viewRef = useRef<MapViewport | null>(null)
  viewRef.current = view
  const nextView = useRef<MapViewport | null>(null)
  const frameRef = useRef(0)
  const queueView = (next: MapViewport) => {
    nextView.current = next
    if (frameRef.current) return
    frameRef.current = requestAnimationFrame(() => {
      frameRef.current = 0
      const latest = nextView.current
      nextView.current = null
      if (latest) setView(latest)
    })
  }
  useEffect(() => () => cancelAnimationFrame(frameRef.current), [])
  // The canvas's backing store, resized only when its size or the screen's pixel ratio changes: setting a canvas's
  // width reallocates and clears it.
  const backing = useRef('')
  // What a screen reader is told when the keyboard moves to a sound.
  const [said, setSaid] = useState('')
  const [hintId] = useState(() => `kumi-map-keys-${(maps += 1)}`)
  const mapSize = Math.max(1, Math.min(size.width, size.height) - 32)
  // The canvas exists only once the map has points: the effects that attach to it wait for that.
  const hasPoints = model.points.length > 0

  useEffect(() => {
    if (query === searchFor) return
    const timer = setTimeout(() => setSearchFor(query), query ? 150 : 0)
    return () => clearTimeout(timer)
  }, [query, searchFor])
  const searched = useMemo(() => searchSounds(model, searchFor), [model, searchFor])
  const indexOf = useMemo(() => new Map(model.points.map((point, index) => [point.id, index])), [model])
  const foundIds = useMemo(
    () => (found && (found.revision === null || found.revision === model.revision) ? new Set(found.ids) : null),
    [found, model.revision]
  )
  // Which points the legend and the search keep (1), dim (0), or leave off the map (2: they couldn't play here), once
  // per change of any of them, so a frame's draw and a hover read one byte per point instead of a set lookup by id.
  const mask = useMemo(() => {
    const out = new Uint8Array(model.points.length)
    model.points.forEach((point, index) => {
      out[index] = leaving?.has(point.id) ? 2 : (hidden.size > 0 && hidden.has(point.group)) || (searched !== null && !searched.has(point.id)) ? 0 : 1
    })
    return out
  }, [model, hidden, searched, leaving])
  const visible = useMemo(() => (index: number): boolean => mask[index] === 1, [mask])
  const shown = useMemo(() => mask.reduce((count, keep) => count + (keep === 1 ? 1 : 0), 0), [mask])
  const leavingCount = useMemo(() => mask.reduce((count, keep) => count + (keep === 2 ? 1 : 0), 0), [mask])
  // Motion: when each leaving sound started fading, when the map arrived (its points pop in from the middle out), the
  // dots-to-tiles morph, and the sounds flying into the palette. One clock moves once a frame while any of them runs,
  // or while a sound plays (its ripple), and not otherwise: a map that sits still draws nothing. A system asking for
  // less motion gets none of it.
  const still = useMemo(() => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches, [])
  const leftAt = useRef(new Map<string, number>())
  const introAt = useRef(-Infinity)
  const until = useRef(0)
  const [clock, setClock] = useState(0)
  const animate = (ms: number) => {
    until.current = Math.max(until.current, performance.now() + ms)
    setClock(performance.now())
  }
  useEffect(() => {
    const now = performance.now()
    for (const id of leaving ?? []) if (!leftAt.current.has(id)) leftAt.current.set(id, now)
    for (const id of [...leftAt.current.keys()]) if (!leaving?.has(id)) leftAt.current.delete(id)
    if (leaving?.size) animate(FADE_MS)
  }, [leaving]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (still || !model.revision) return
    introAt.current = performance.now()
    animate(INTRO_MS)
  }, [model.revision]) // eslint-disable-line react-hooks/exhaustive-deps
  const [morph, setMorph] = useState<Morph>(() => ({ from: tiles ? 1 : 0, to: tiles ? 1 : 0, at: -Infinity }))
  useEffect(() => {
    const now = performance.now()
    setMorph(current => (current.to === (tiles ? 1 : 0) ? current : { from: still ? (tiles ? 1 : 0) : morphAt(current, now), to: tiles ? 1 : 0, at: now }))
    if (!still) animate(MORPH_MS)
  }, [tiles]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (flying && !still) animate(FLY_MS + FLY_STAGGER * FLY_MOST)
  }, [flying]) // eslint-disable-line react-hooks/exhaustive-deps
  const playingHere = playing !== null && indexOf.has(playing) && !still
  useEffect(() => {
    if (performance.now() >= until.current && !playingHere) return
    const frame = requestAnimationFrame(setClock)
    return () => cancelAnimationFrame(frame)
  }, [clock, playingHere])
  // The tile view's model: laid out the first time tiles show, and kept while the morph runs back to dots.
  const layout = useMemo(() => (tiles || morph.from > 0 ? tileLayout(model) : null), [model, tiles, morph.from > 0]) // eslint-disable-line react-hooks/exhaustive-deps
  const tiled = useMemo(() => (layout ? tileModel(model, layout) : null), [model, layout])
  // What the pointer, the keyboard, a box and a find's frame work on: the tiles once they show, else the dots.
  const active = tiles && tiled ? tiled : model
  // Each point's place in the motion: 0 at the map's middle, 1 at its edge.
  const spread = useMemo(() => {
    const points = model.points
    const out = new Float32Array(points.length)
    if (points.length === 0) return out
    const cx = points.reduce((sum, point) => sum + point.x, 0) / points.length
    const cy = points.reduce((sum, point) => sum + point.y, 0) / points.length
    let far = 0
    points.forEach((point, index) => {
      out[index] = Math.hypot(point.x - cx, point.y - cy)
      far = Math.max(far, out[index])
    })
    if (far > 0) for (let index = 0; index < out.length; index++) out[index] /= far
    return out
  }, [model])
  // What a search, or else the agent's find, picked out, also as named rows: a keyboard or a screen reader can pick a
  // sound the canvas only draws.
  const matches = useMemo(() => {
    const ids = searched ?? foundIds
    return ids ? [...ids].filter(id => indexOf.has(id) && visible(indexOf.get(id) as number)) : []
  }, [searched, foundIds, indexOf, visible])
  // The picks in the map's own order, as a gather sends them.
  const pickedIds = useMemo(() => (picked && picked.size > 0 ? model.points.filter(point => picked.has(point.id)).map(point => point.id) : []), [model, picked])
  // CC-23: a near search names its seed (the top-level `near`) from this map; one the map doesn't list heads nothing.
  const nearName = !searched && foundIds && found?.near && model.sounds[found.near] ? clean(model.sounds[found.near].name ?? '', 60) : ''

  // The canvas paints the theme's colours itself, and neither host tells a plugin its theme changed (Desktop's
  // useTheme is newer than some Desktops; the dashboard has none): it looks at them once a second, and a light or dark
  // switch redraws it.
  const [painted, setPainted] = useState('')
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const timer = setInterval(() => {
      const now = Object.values(resolve(canvas)).join()
      setPainted(current => (current === now ? current : now))
    }, 1000)
    return () => clearInterval(timer)
  }, [hasPoints])

  // Follow the box's size (the pane or tab is resized all the time).
  useEffect(() => {
    const box = boxRef.current
    if (!box) return
    const observer = new ResizeObserver(entries => {
      const rect = entries[0]?.contentRect
      if (rect) setSize({ width: Math.floor(rect.width), height: Math.floor(rect.height) })
    })
    observer.observe(box)
    return () => observer.disconnect()
  }, [hasPoints])

  // A new map, or a new size before the user moved the view: fit the whole map.
  useEffect(() => {
    if (size.width > 0 && size.height > 0) setView(current => current ?? fitView(size.width, size.height))
  }, [size.width, size.height])
  useEffect(() => setView(size.width > 0 ? fitView(size.width, size.height) : null), [model.revision]) // eslint-disable-line react-hooks/exhaustive-deps

  // What the agent found: frame it.
  useEffect(() => {
    if (!foundIds || size.width === 0) return
    const framed = frame(model, foundIds, size.width, size.height)
    if (framed) setView(framed)
  }, [foundIds, model, size.width, size.height])

  // Wheel zoom (a native listener: React's wheel handler can't stop the page from scrolling).
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const wheel = (event: WheelEvent) => {
      event.preventDefault()
      const rect = canvas.getBoundingClientRect()
      const factor = Math.exp(-event.deltaY * (event.ctrlKey ? 0.01 : 0.0025))
      const base = nextView.current ?? viewRef.current
      if (base) queueView(zoomAt(base, factor, event.clientX - rect.left, event.clientY - rect.top))
    }
    canvas.addEventListener('wheel', wheel, { passive: false })
    return () => canvas.removeEventListener('wheel', wheel)
  }, [hasPoints])

  // The sound under the pointer, for its glow: a move over the same sound draws nothing new.
  const hoverAt = hover ? hover.index : null
  // Draw whenever anything shown changes, and once a frame while something moves. Points are batched into one path
  // per group colour, so a 25,000-sound map is a few hundred fills, not 25,000. Each colour's points are split into
  // `ROUNDS` interleaved paths, drawn round by round: where groups overlap, their colours still mix as the map's own
  // order left them, instead of the last colour covering the rest. A point is a dot, a square tile, or between the
  // two while the morph runs.
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || !view || size.width === 0) return
    const ratio = window.devicePixelRatio || 1
    const wanted = `${Math.round(size.width * ratio)}x${Math.round(size.height * ratio)}`
    if (backing.current !== wanted) {
      backing.current = wanted
      canvas.width = Math.round(size.width * ratio)
      canvas.height = Math.round(size.height * ratio)
    }
    const context = canvas.getContext('2d')
    if (!context) return
    const colors = resolve(canvas)
    context.setTransform(ratio, 0, 0, ratio, 0, 0)
    context.clearRect(0, 0, size.width, size.height)
    const now = performance.now()
    // A small map (a fresh folder of a few sounds) draws bigger points than a library of thousands.
    const points = model.points
    const count = points.length
    const base = count <= 40 ? 5 : count <= 300 ? 3.2 : 2.2
    const radius = Math.max(1.5, Math.min(base + 3, base + Math.log2(view.scale) * 0.8))
    const groupColor = new Map(model.groups.map(group => [group.key, group.color]))
    const unit = mapSize * view.scale
    const [left, top, right, bottom] = [-8, -8, size.width + 8, size.height + 8]
    // The morph (0 dots, 1 tiles), each point's own share of it (the middle leads), and a tile's side in pixels.
    const morphed = still ? morph.to : morphAt(morph, now)
    const tilePoints = tiled && morphed > 0 ? tiled.points : null
    const tileSide = layout ? (unit / layout.side) * 0.86 : 0
    const blendOf = (index: number): number => (tilePoints ? clamp01(morphed * (1 + STAGGER) - spread[index] * STAGGER) : 0)
    const placeOf = (index: number): [number, number, number] => {
      const point = points[index]
      const blend = blendOf(index)
      const x = tilePoints ? point.x + (tilePoints[index].x - point.x) * blend : point.x
      const y = tilePoints ? point.y + (tilePoints[index].y - point.y) * blend : point.y
      return [x * unit + view.dx, y * unit + view.dy, blend]
    }
    // A new map's points pop in from the middle out: each grows from nothing, just past its size, then settles.
    const introFor = (index: number): number => {
      const since = now - introAt.current - spread[index] * (INTRO_MS * 0.45)
      return since >= INTRO_MS * 0.55 ? 1 : since <= 0 ? 0 : easeOutBack(since / (INTRO_MS * 0.55))
    }
    const shape = (path: Path2D, x: number, y: number, blend: number, grow = 1) => {
      if (blend <= 0) {
        const r = radius * grow
        if (r <= 0.05) return
        path.moveTo(x + r, y)
        path.arc(x, y, r, 0, Math.PI * 2)
        return
      }
      const side = (2 * radius + (tileSide - 2 * radius) * blend) * grow
      if (side <= 0.1) return
      const corner = side / 2 - (side / 2 - side * 0.18) * blend
      if (typeof path.roundRect === 'function') path.roundRect(x - side / 2, y - side / 2, side, side, corner)
      else path.rect(x - side / 2, y - side / 2, side, side)
    }
    // How far beyond a point its rings sit: past a tile's corner once tiles show.
    const reach = (blend: number): number => radius + (Math.max(radius, tileSide * 0.62) - radius) * blend
    // The labels need at most 41 on-screen points: past 40 there are none.
    const labelled: [number, number, string][] = []
    let onScreen = 0
    // Dimmed points first, then the ones the search and legend keep, so those sit on top.
    for (const pass of [0, 1]) {
      const rounds = Array.from({ length: ROUNDS }, () => new Map<string, Path2D>())
      for (let index = 0; index < count; index++) {
        if (mask[index] !== pass) continue
        const [x, y, blend] = placeOf(index)
        if (x < left || y < top || x > right || y > bottom) continue
        const color = groupColor.get(points[index].group) ?? colors.faint
        const paths = rounds[index % ROUNDS]
        let path = paths.get(color)
        if (!path) {
          path = new Path2D()
          paths.set(color, path)
        }
        shape(path, x, y, blend, introFor(index))
        if (pass === 1 && ++onScreen <= 41) labelled.push([x, y, model.sounds[points[index].id]?.name ?? ''])
      }
      context.globalAlpha = pass ? 0.92 : 0.16
      for (const paths of rounds) {
        for (const [color, path] of paths) {
          context.fillStyle = color
          context.fill(path)
        }
      }
    }
    // A sound that couldn't play shrinks and fades off where it was, drawn on its own: there are only ever a few.
    for (const [id, at] of leftAt.current) {
      const progress = (now - at) / FADE_MS
      const index = indexOf.get(id)
      if (progress >= 1 || index === undefined) continue
      const eased = 1 - (1 - progress) ** 2
      const [x, y, blend] = placeOf(index)
      const path = new Path2D()
      shape(path, x, y, blend, 1 - eased + 0.02)
      context.globalAlpha = 0.92 * (1 - eased)
      context.fillStyle = groupColor.get(points[index].group) ?? colors.faint
      context.fill(path)
    }
    // Few enough on screen (a small map, a narrow search, or zoomed in) to read: each one's name beside it, the sound
    // file's own name drawn as canvas text, never markup. A name that would overlap one already drawn is left out, so
    // a tight cluster stays legible (its sounds are named on hover and in the list above).
    if (onScreen <= 40) {
      context.globalAlpha = 0.85
      context.fillStyle = colors.text
      context.font = `11px ${getComputedStyle(canvas).fontFamily || 'sans-serif'}`
      context.textBaseline = 'middle'
      const placed: [number, number, number, number][] = []
      const gap = reach(morphed) + 4
      for (const [x, y, name] of labelled) {
        const text = mapLabel(name)
        if (!text) continue
        const box: [number, number, number, number] = [x + gap, y - 7, context.measureText(text).width, 14]
        if (placed.some(([left, top, width, height]) => box[0] < left + width && left < box[0] + box[2] && box[1] < top + height && top < box[1] + box[3])) continue
        placed.push(box)
        context.fillText(text, box[0], y)
      }
    }
    context.globalAlpha = 1
    // The palette's sounds, and the picks, as rings: one path each, however many there are.
    const rings = (ids: ReadonlySet<string> | undefined, extra: number): Path2D | null => {
      if (!ids || ids.size === 0) return null
      const path = new Path2D()
      for (const id of ids) {
        const index = indexOf.get(id)
        if (index === undefined || mask[index] === 2) continue
        const [x, y, blend] = placeOf(index)
        if (x < left || y < top || x > right || y > bottom) continue
        const r = reach(blend) + extra
        path.moveTo(x + r, y)
        path.arc(x, y, r, 0, Math.PI * 2)
      }
      return path
    }
    const inPalette = rings(marked, 2.2)
    if (inPalette) {
      // A thin ring, a little see-through: a palette of a whole cluster stays a cluster of its colour.
      context.globalAlpha = 0.8
      context.strokeStyle = colors.accent
      context.lineWidth = 1
      context.stroke(inPalette)
      context.globalAlpha = 1
    }
    const inPicks = rings(picked, 1.4)
    if (inPicks) {
      context.strokeStyle = colors.strong
      context.lineWidth = 1.6
      context.stroke(inPicks)
    }
    const ring = (id: string, color: string, extra: number, width: number) => {
      const index = indexOf.get(id)
      if (index === undefined) return
      const [x, y, blend] = placeOf(index)
      context.strokeStyle = color
      context.lineWidth = width
      context.beginPath()
      context.arc(x, y, reach(blend) + extra, 0, Math.PI * 2)
      context.stroke()
    }
    foundIds?.forEach(id => ring(id, colors.accent, 3, 1.5))
    // The sound under the pointer glows in its colour, a little bigger.
    const hovered = hoverAt
    if (hovered !== null && hovered < count && mask[hovered] === 1) {
      const [x, y, blend] = placeOf(hovered)
      const color = groupColor.get(points[hovered].group) ?? colors.faint
      const path = new Path2D()
      shape(path, x, y, blend, 1.45)
      context.save()
      context.shadowColor = color
      context.shadowBlur = 14
      context.fillStyle = color
      context.fill(path)
      context.restore()
      // And a fine ring in the text colour, so one sound stands out of a dense cluster of its own colour.
      const ring = new Path2D()
      shape(ring, x, y, blend, 2.3)
      context.strokeStyle = colors.strong
      context.lineWidth = 1.25
      context.stroke(ring)
    }
    // A playing sound ripples: two rings spread from it and fade, half a beat apart.
    if (playing) {
      ring(playing, colors.accent, 6, 2)
      const index = indexOf.get(playing)
      if (index !== undefined && !still) {
        const [x, y, blend] = placeOf(index)
        for (const offset of [0, 0.5]) {
          const phase = ((now / RIPPLE_MS + offset) % 1 + 1) % 1
          context.globalAlpha = 0.65 * (1 - phase) ** 1.5
          context.strokeStyle = groupColor.get(points[index].group) ?? colors.accent
          context.lineWidth = 2 - phase
          context.beginPath()
          context.arc(x, y, reach(blend) + 4 + phase * 26, 0, Math.PI * 2)
          context.stroke()
        }
        context.globalAlpha = 1
      }
    }
    if (selected) ring(selected, colors.strong, 4, 2)
    // Sounds just gathered fly down into the palette under the map, shrinking, one after another.
    if (flying && !still) {
      const target: [number, number] = [size.width / 2, size.height + 24]
      flying.ids.slice(0, FLY_MOST).forEach((id, order) => {
        const index = indexOf.get(id)
        if (index === undefined) return
        const progress = clamp01((now - flying.at - order * FLY_STAGGER) / FLY_MS)
        if (progress <= 0 || progress >= 1) return
        const eased = progress * progress * (3 - 2 * progress)
        const [x0, y0] = placeOf(index)
        // A slight arc: up a little first, then down into the dock.
        const x = x0 + (target[0] - x0) * eased
        const y = y0 + (target[1] - y0) * eased - Math.sin(progress * Math.PI) * 28
        context.globalAlpha = 0.95 - 0.5 * progress
        context.fillStyle = groupColor.get(points[index].group) ?? colors.accent
        context.beginPath()
        context.arc(x, y, Math.max(1.4, (radius + 2.5) * (1 - 0.55 * progress)), 0, Math.PI * 2)
        context.fill()
        // Outlined in the text colour, so they read as leaving even over a dense cluster of their own colour.
        context.strokeStyle = colors.strong
        context.lineWidth = 0.9
        context.stroke()
      })
      context.globalAlpha = 1
    }
  }, [model, indexOf, view, size, mask, foundIds, selected, playing, mapSize, painted, clock, marked, picked, tiled, layout, morph, spread, flying, still, hoverAt])

  const at = (event: { clientX: number; clientY: number }): [number, number] => {
    const rect = canvasRef.current?.getBoundingClientRect()
    return rect ? [event.clientX - rect.left, event.clientY - rect.top] : [0, 0]
  }
  // How near the pointer must be to a sound to reach it: a dot's few pixels, or most of a tile.
  const reachPx = (base: number): number => (tiles && layout && view ? Math.max(base, ((mapSize * view.scale) / layout.side) * 0.62) : base)

  // A press starts a pan, or with Shift held (or Select on) a box; a press with Command or Ctrl that doesn't move picks
  // or unpicks the sound under it.
  const press = (event: PointerEvent<HTMLCanvasElement>) => {
    if (!view) return
    event.currentTarget.setPointerCapture?.(event.pointerId)
    const [x, y] = at(event)
    const boxing = Boolean(onPicked) && (event.shiftKey || selecting)
    drag.current = { x, y, moved: false, view, box: boxing, add: event.metaKey || event.ctrlKey }
    rest(null)
    setHover(null)
    if (boxing) setBox([x, y, x, y])
  }
  const release = (event: PointerEvent<HTMLCanvasElement>) => {
    const held = drag.current
    drag.current = null
    setBox(null)
    if (!held || !view) return
    const [x, y] = at(event)
    if (held.box && held.moved) {
      const inside = pointsInBox(active, view, mapSize, [held.x, held.y, x, y], visible).map(index => model.points[index].id)
      onPicked?.(inside, held.add ? 'add' : 'replace')
      return
    }
    if (held.moved) return
    const index = pick(active, view, mapSize, x, y, reachPx(10), visible)
    if (index < 0) return
    if (onPicked && (event.metaKey || event.ctrlKey)) onPicked([model.points[index].id], 'toggle')
    else onPick(model.points[index].id)
  }

  const onKey = (event: KeyboardEvent<HTMLCanvasElement>) => {
    const key = mapKey(event)
    if (!view || !key) return
    event.preventDefault()
    const from = selected === null ? undefined : indexOf.get(selected)
    const picked = from !== undefined && visible(from) ? from : undefined
    const middle: [number, number] = [size.width / 2, size.height / 2]
    if ('fit' in key) {
      setView(fitView(size.width, size.height))
    } else if ('zoom' in key) {
      // Around the sound picked, so it stays where it is; else around the middle.
      const [x, y] = picked === undefined ? middle : toScreen(view, mapSize, active.points[picked].x, active.points[picked].y)
      setView(zoomAt(view, key.zoom, x, y))
    } else {
      const next = moveWith(active, view, mapSize, picked, key, middle, visible)
      if (next < 0) return
      const point = active.points[next]
      const [x, y] = toScreen(view, mapSize, point.x, point.y)
      setView(keepInView(view, x, y, size.width, size.height))
      onSelect(point.id)
      const sound = model.sounds[point.id]
      setSaid([clean(sound?.name ?? point.id, 80), clean(sound?.class, 16), clean(sound?.folder, 60)].filter(Boolean).join(', '))
    }
  }

  if (!model.points.length) {
    return (
      <div style={{ ...S.muted, padding: 16 }}>
        {empty}
        {(map?.hidden ?? 0) > 0 && (
          <div role="status" style={{ ...S.row, flexWrap: 'wrap', gap: 6, marginTop: 8 }}>
            <span style={{ ...S.faint, flex: '1 1 14rem' }}>{t('map.hidden', map?.hidden ?? 0)}</span>
            {hiddenActions}
          </div>
        )}
      </div>
    )
  }

  const chosen = selected ? model.sounds[selected] : undefined
  const hovered = hover ? model.points[hover.index] : undefined
  const hoveredSound = hovered ? model.sounds[hovered.id] : undefined
  const countWords =
    shown === model.points.length - leavingCount ? t('map.count', num(shown, 0)) : t('map.countOf', num(shown, 0), num(model.points.length - leavingCount, 0))

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 240, color: T.text }}>
      <div style={{ ...S.row, flexWrap: 'wrap', padding: '8px 10px', gap: 6, borderBottom: `1px solid ${T.border}` }}>
        {/* The count sits inside the search field, so the field and the toggles fit one line beside the chat. The
            revision is an id (`r` and 32 hex digits): there for anyone who looks, not in the line itself. */}
        <div style={{ position: 'relative', display: 'flex', flex: '1 1 160px', minWidth: 130 }}>
          <input
            aria-label={t('map.search')}
            className={FOCUSABLE}
            onChange={event => setQuery(event.target.value)}
            placeholder={t('map.searchHint')}
            style={{ flex: 1, minWidth: 0, font: 'inherit', fontSize: 12, padding: '3px 8px', paddingRight: `${countWords.length * 0.55 + 1.2}em`, borderRadius: 3, border: `1px solid ${T.border}`, background: 'transparent', color: T.text }}
            value={query}
          />
          <span
            style={{ ...S.faint, fontSize: 10.5, lineHeight: '16px', padding: '0 6px', borderRadius: 8, background: T.chip, position: 'absolute', right: 4, top: '50%', transform: 'translateY(-50%)', whiteSpace: 'nowrap', cursor: 'default' }}
            title={model.revision ? t('map.revision', model.revision) : undefined}
          >
            {countWords}
          </span>
        </div>
        {onPicked && searched !== null && matches.length > 0 && (
          // Native's `a`: every sound the search keeps.
          <TextButton onClick={() => onPicked(matches, 'replace')} title={t('dock.pickAllTitle')}>
            {t('dock.pickAll', matches.length)}
          </TextButton>
        )}
        {onPicked && (
          <TextButton onClick={() => setSelecting(!selecting)} pressed={selecting} title={t('dock.selectTitle')}>
            {t('dock.select')}
          </TextButton>
        )}
        {onTiles && (
          <TextButton onClick={() => onTiles(!tiles)} pressed={tiles} title={t('map.tilesTitle')}>
            {t('map.tiles')}
          </TextButton>
        )}
        {tools}
      </div>
      <div style={{ ...S.row, flexWrap: 'wrap', gap: 4, padding: '6px 10px' }}>
        {model.groups.map(group => {
          const off = hidden.has(group.key)
          return (
            <button
              aria-pressed={!off}
              className={FOCUSABLE}
              key={group.key}
              onClick={() =>
                setHidden(current => {
                  const next = new Set(current)
                  if (next.has(group.key)) next.delete(group.key)
                  else next.add(group.key)
                  return next
                })
              }
              title={off ? t('map.show', group.name) : t('map.hide', group.name)}
              type="button"
              style={{ ...S.row, gap: 4, font: 'inherit', fontSize: 11, padding: '1px 6px', borderRadius: 10, border: `1px solid ${T.border}`, background: 'transparent', color: off ? T.faint : T.secondary, cursor: 'pointer', opacity: off ? 0.6 : 1 }}
            >
              <span style={{ width: 8, height: 8, borderRadius: '50%', background: group.color, opacity: off ? 0.3 : 1 }} />
              {group.name} <span style={{ color: T.faint }}>{group.count}</span>
            </button>
          )
        })}
      </div>
      {matches.length > 0 && (
        <div aria-label={searched ? t('map.matches') : nearName ? t('map.near', nearName) : t('map.found')} role="group" style={{ display: 'flex', flexDirection: 'column', gap: 1, padding: '0 10px 6px' }}>
          {/* CC-23: a near search's neighbours, under the seed's name from this map (the seed itself isn't listed). */}
          {nearName && <span style={{ ...S.muted, padding: '0 6px' }}>{t('map.near', nearName)}</span>}
          {matches.slice(0, MATCHES_LISTED).map(id => {
            const sound = model.sounds[id]
            const indexed = indexedValues(sound, t)
            // A typed search lists names; the agent's find also says why each matched, as Kumi bounds it (CC-23).
            const said = searched ? '' : foundLine(found?.matches?.[id], nearName !== '', t)
            return (
              <button
                aria-pressed={selected === id}
                className={FOCUSABLE}
                key={id}
                onClick={() => onPick(id)}
                type="button"
                style={{ display: 'flex', flexDirection: 'column', alignItems: 'stretch', gap: 0, font: 'inherit', fontSize: 11.5, padding: '1px 6px', borderRadius: 3, border: 'none', textAlign: 'left', cursor: 'pointer', color: T.text, background: selected === id ? T.card : 'transparent' }}
              >
                <span style={{ ...S.row, gap: 6 }}>
                  <span style={{ ...S.ellipsis, flex: 1 }}>{clean(sound?.name ?? id, 80)}</span>
                  {indexed && (
                    <span style={{ color: T.faint, whiteSpace: 'nowrap' }} title={t('map.indexedTitle')}>
                      {t('map.indexed', indexed)}
                    </span>
                  )}
                  {sound?.class && <span style={{ color: T.faint }}>{clean(sound.class, 16)}</span>}
                </span>
                {said && (
                  <span style={{ ...S.ellipsis, color: T.faint, fontSize: 10.5 }} title={nearName ? t('map.closenessTitle') : undefined}>
                    {said}
                  </span>
                )}
              </button>
            )
          })}
          {matches.length > MATCHES_LISTED && <span style={{ ...S.faint, padding: '0 6px' }}>{t('map.more', matches.length - MATCHES_LISTED)}</span>}
        </div>
      )}
      <div ref={boxRef} style={{ position: 'relative', flex: 1, minHeight: 160, overflow: 'hidden' }}>
        <canvas
          ref={canvasRef}
          aria-describedby={hintId}
          aria-label={t('map.canvas')}
          aria-roledescription={t('map.roledescription')}
          className={FOCUSABLE}
          onKeyDown={onKey}
          onPointerDown={press}
          onPointerMove={event => {
            const [x, y] = at(event)
            const held = drag.current
            if (held) {
              if (Math.abs(x - held.x) + Math.abs(y - held.y) > 3) held.moved = true
              if (held.box) setBox([held.x, held.y, x, y])
              else if (held.moved) queueView({ ...held.view, dx: held.view.dx + x - held.x, dy: held.view.dy + y - held.y })
              return
            }
            if (!view) return
            const index = pick(active, view, mapSize, x, y, reachPx(8), visible)
            setHover(index >= 0 ? { index, x, y } : null)
            rest(index >= 0 ? model.points[index].id : null)
          }}
          onPointerUp={release}
          onPointerCancel={() => {
            drag.current = null
            setBox(null)
          }}
          onPointerLeave={() => {
            setHover(null)
            rest(null)
          }}
          onDoubleClick={() => setView(fitView(size.width, size.height))}
          role="application"
          tabIndex={0}
          // The box clips what's outside it: the focus ring sits inside the canvas's edge.
          style={{ position: 'absolute', inset: 0, width: '100%', height: '100%', outlineOffset: -3, cursor: selecting || box ? 'crosshair' : hover ? 'pointer' : drag.current?.moved ? 'grabbing' : 'grab', touchAction: 'none' }}
        />
        {box && Math.abs(box[2] - box[0]) + Math.abs(box[3] - box[1]) > 3 && (
          // The box being drawn, its edge marching (`kumi-ants`, in FocusStyle).
          <svg
            aria-hidden
            height={Math.abs(box[3] - box[1])}
            style={{ position: 'absolute', left: Math.min(box[0], box[2]), top: Math.min(box[1], box[3]), overflow: 'visible', pointerEvents: 'none' }}
            width={Math.abs(box[2] - box[0])}
          >
            <rect
              className="kumi-ants"
              height={Math.max(0, Math.abs(box[3] - box[1]) - 1)}
              rx={2}
              style={{ fill: `color-mix(in srgb, ${T.accent} 12%, transparent)`, stroke: T.accent, strokeWidth: 1, strokeDasharray: '5 4' }}
              width={Math.max(0, Math.abs(box[2] - box[0]) - 1)}
              x={0.5}
              y={0.5}
            />
          </svg>
        )}
        <span id={hintId} style={S.hidden}>
          {t('map.keys')}
        </span>
        <span aria-live="polite" style={S.hidden}>
          {said}
        </span>
        {hovered && hoveredSound && hover && (
          <div
            style={{ position: 'absolute', left: Math.min(hover.x + 12, Math.max(0, size.width - 220)), top: Math.max(0, hover.y - 34), maxWidth: 220, pointerEvents: 'none', padding: '3px 7px', borderRadius: 4, background: T.raised, border: `1px solid ${T.border}`, boxShadow: '0 6px 16px -8px rgba(0,0,0,.6)', fontSize: 11.5, color: T.text, ...S.ellipsis }}
          >
            {clean(hoveredSound.name, 48)} <span style={{ color: T.muted }}>· {clean(hoveredSound.class ?? '', 16)}</span>
          </div>
        )}
      </div>
      {pickedIds.length > 0 && (
        <div role="group" aria-label={t('dock.picked', pickedIds.length)} style={{ ...S.row, flexWrap: 'wrap', gap: 6, padding: '6px 10px', borderTop: `1px solid ${T.border}` }}>
          <span style={{ ...S.text, fontWeight: 600, flex: '1 1 8rem' }}>{t('dock.picked', pickedIds.length)}</span>
          {pickedActions?.(pickedIds)}
        </div>
      )}
      {(map?.hidden ?? 0) + leavingCount > 0 && (
        // One quiet line for every sound that couldn't play here, never a notice per sound.
        <div role="status" style={{ ...S.row, flexWrap: 'wrap', gap: 6, padding: '4px 10px', borderTop: `1px solid ${T.border}` }}>
          <span style={{ ...S.faint, flex: '1 1 14rem' }}>{t('map.hidden', (map?.hidden ?? 0) + leavingCount)}</span>
          {hiddenActions}
        </div>
      )}
      {selected && chosen && (
        // Beside the chat the map can be narrow: the name and the actions wrap rather than push actions out of view.
        <div style={{ ...S.row, justifyContent: 'space-between', flexWrap: 'wrap', gap: 8, padding: '8px 10px', borderTop: `1px solid ${T.border}` }}>
          <div style={{ display: 'flex', flexDirection: 'column', minWidth: 0 }}>
            <span style={{ ...S.text, ...S.ellipsis, fontWeight: 600 }}>{clean(chosen.name, 80)}</span>
            <span style={{ ...S.row, gap: 4, marginTop: 2 }}>
              {chosen.class && <Chip>{clean(chosen.class, 16)}</Chip>}
              {chosen.kind && <Chip>{clean(chosen.kind, 16)}</Chip>}
              {typeof chosen.duration === 'number' && <Chip>{seconds(chosen.duration)}</Chip>}
              {chosen.folder && <span style={{ ...S.faint, ...S.ellipsis }}>{clean(chosen.folder, 60)}</span>}
            </span>
          </div>
          <div style={{ ...S.row, gap: 6, flexWrap: 'wrap', justifyContent: 'flex-end', marginLeft: 'auto' }}>{actions?.(selected, chosen)}</div>
        </div>
      )}
    </div>
  )
}
