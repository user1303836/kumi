// The sound map, drawn on a canvas: each sound of the library where Kumi's layout put it, coloured by its group.
// A drag with the left button draws a box that picks the sounds in it (Shift adds another box to the picks, Command
// or Ctrl with a click picks or unpicks one); the middle button pans, as does the left with Space held or the hand
// tool on. A mouse's wheel zooms; a trackpad's two-finger scroll pans and its pinch zooms. A click picks the sound
// under the pointer. Search and the legend dim what they leave out; sounds the agent just found get a ring. What a
// search or a find picks out is also listed by name, for the keyboard. Like native Kumi's map: a sound plays as the
// pointer comes onto it (`onHover`), and the palette's sounds are marked. On the keyboard (`mapKey`), the arrows move
// to the nearest sound that way, Shift+←/→ step through every sound in turn, Shift+↑/↓ zoom and Home shows the whole
// map; each sound reached is named to a screen reader, and Tab goes on to its Play and Use….
// Both UI halves use it; what a picked sound can do comes from `actions`, what the picks can do from `pickedActions`.
import { CANVAS, T } from '@kumi/theme'
import type { KeyboardEvent, PointerEvent, ReactNode } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'

import type { DesignResult } from './design.ts'
import { clean, num } from './format.ts'
import { type Grow, type GrowEnd, soundEnd } from './grow.ts'
import { GrowLayer } from './GrowLayer.tsx'
import { arrangedModel, buildModel, fitView, floatBeside, foundLine, frame, GLYPH_SHAPES, glyphOf, indexedValues, keepInView, mapKey, mapLabel, type MapModel, mapSide, type MapViewport, moveWith, pick, pointsInBox, searchSounds, tileLayout, tileModel, toScreen, traceGlyph, type WheelMemory, type WheelMode, wheelPans, zoomAt } from './mapModel.ts'
import { glass, Icon, type LayoutChoice, LayoutMenu, Rule, ToolButton, ToolGroup } from './mapUi.tsx'
import { Player } from './Player.tsx'
import type { KumiState } from './store.ts'
import type { MapSound, SoundMap } from './types.ts'
import { FOCUSABLE, S, TextButton, useT } from './ui.tsx'

export interface MapCanvasProps {
  map: SoundMap | null
  found: KumiState['found']
  playing: string | null
  selected: string | null
  /** A click, or a found list's row: pick that sound and play it. */
  onPick: (id: string) => void
  /** The keyboard moved to a sound: picked, not played (its Play is the next tab stop). Null: a click on empty map let
   *  go of it. */
  onSelect: (id: string | null) => void
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
  /** What the picks can do, floating beside them while there are any ("Add 12 to palette", Clear). */
  pickedActions?: (ids: string[]) => ReactNode
  /** Square tiles instead of dots (the dots morph into them); `onTiles` shows the toggle and hears it. */
  tiles?: boolean
  onTiles?: (on: boolean) => void
  /** Sounds just gathered into the palette, which fly from where they are into the dock under the map. */
  flying?: { ids: string[]; at: number } | null
  /** What the wheel does: `auto` tells a mouse's wheel (zoom) from a trackpad's scroll (pan). */
  wheel?: WheelMode
  /** The audio behind an id (the audition's cache), for the player's waveform; and how to stop what plays. */
  audio?: (id: string) => Promise<string>
  onStop?: () => void
  /** How much of the map's right a side sheet covers (the Make panel beside a wide map): the fit, a find's frame and
   *  the picks' actions keep to the room left of it. */
  coverRight?: number
  /** The layout shown: '' for similarity, else one of the map's arrangements by id (one it lacks is similarity); the
   *  parent keeps it, hearing each choice in `onArrangement`. Without it the map keeps its own. */
  arrangement?: string
  onArrangement?: (id: string) => void
  /** Grow's vine, drawn over the map; while it names its ends, a click on a sound names one (and plays it) instead of
   *  picking it. `growActions` are the parent's for a grown point (Keep, Use…, Make from); `onPlayMade` plays one. */
  grow?: Grow
  growActions?: (result: DesignResult) => ReactNode
  onPlayMade?: (result: DesignResult) => void
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
/** How long the sounds take to move to a new layout. */
const ARRANGE_MS = 1000
/** The layouts this plug-in can name, in the order the menu lists them (serve's `arrangements` ids). */
const LAYOUTS = ['character', 'texture', 'keys', 'tempo', 'families', 'length', 'folders'] as const
/** The axis words each layout has (`arrange.<id>.<side>`): its x axis's ends, then its y axis's. */
const AXES: Record<string, readonly ('bottom' | 'left' | 'right' | 'top')[]> = {
  character: ['left', 'right', 'top', 'bottom'],
  texture: ['left', 'right', 'top', 'bottom'],
  tempo: ['left', 'right', 'bottom']
}

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
    strong: read(CANVAS.strong, '#ddd'),
    ground: read(CANVAS.ground, '#151515')
  }
}

// How many interleaved paths each group's points are drawn in: enough that overlapping groups mix about as the map's
// own order would, few enough that a pass stays a few hundred fills.
const ROUNDS = 8

// Each map's own id for its keys hint. Not React's useId: the hosts' React builds differ, and this needs none of it.
let maps = 0

export function MapCanvas({ map, found, playing, selected, onPick, onSelect, actions, empty, leaving, hiddenActions, marked, picked, onPicked, onHover, tools, pickedActions, tiles = false, onTiles, flying, wheel = 'auto', audio, onStop, coverRight = 0, arrangement, onArrangement, grow, growActions, onPlayMade }: MapCanvasProps) {
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
  // A drag: with the left button a box that picks what's in it (`add`: Shift, Command or Ctrl held, so it adds to the
  // picks); a pan with the middle button, with the left while Space is held or the hand tool is on, or on a map that
  // picks nothing.
  const drag = useRef<{ x: number; y: number; moved: boolean; view: MapViewport; box: boolean; add: boolean; button: number } | null>(null)
  const [box, setBox] = useState<[number, number, number, number] | null>(null)
  const [handTool, setHandTool] = useState(false)
  const [grabbing, setGrabbing] = useState(false)
  // Space held over the map pans with the left button, as in drawing apps. Desktop takes Space for its composer before
  // the map sees it, so this works in the dashboard; Desktop has the hand tool and the middle button.
  const over = useRef(false)
  const [spaceHeld, setSpaceHeld] = useState(false)
  const space = useRef(false)
  useEffect(() => {
    const hold = (held: boolean) => {
      if (space.current === held) return
      space.current = held
      setSpaceHeld(held)
    }
    const down = (event: globalThis.KeyboardEvent) => {
      if (event.code !== 'Space' || event.defaultPrevented || !over.current) return
      const target = event.target as HTMLElement | null
      if (target?.isContentEditable || target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return
      event.preventDefault()
      hold(true)
    }
    const up = (event: globalThis.KeyboardEvent) => {
      if (event.code === 'Space') hold(false)
    }
    const blur = () => hold(false)
    window.addEventListener('keydown', down, true)
    window.addEventListener('keyup', up, true)
    window.addEventListener('blur', blur)
    return () => {
      window.removeEventListener('keydown', down, true)
      window.removeEventListener('keyup', up, true)
      window.removeEventListener('blur', blur)
    }
  }, [])
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
    moved.current = true
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
  // What floats over the map's top: the map fits below its bar and legend (the cards under them, a find's list and the
  // quiet line, float over the map's corner and leave its scale alone).
  const topRef = useRef<HTMLDivElement | null>(null)
  const legendRef = useRef<HTMLDivElement | null>(null)
  const [topInset, setTopInset] = useState(0)
  const mapSize = mapSide(size.width, size.height, topInset)
  const fit = () => fitView(size.width, size.height, topInset, coverRight)
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
  // The layout: similarity (''), or one of the map's arrangements that this plug-in can name. The parent keeps the
  // choice when it listens; a choice the map lacks is similarity.
  const [ownArrangement, setOwnArrangement] = useState('')
  const arrangementId = arrangement ?? ownArrangement
  const arrangementIndex = (LAYOUTS as readonly string[]).includes(arrangementId) ? model.arrangements.findIndex(each => each.id === arrangementId) : -1
  const arranged = useMemo(() => arrangedModel(model, arrangementIndex), [model, arrangementIndex])
  const chooseArrangement = (id: string) => {
    if (arrangement === undefined) setOwnArrangement(id)
    onArrangement?.(id)
  }
  const layoutChoices = useMemo((): LayoutChoice[] => {
    const named = LAYOUTS.filter(id => model.arrangements.some(each => each.id === id))
    return named.length === 0 ? [] : [{ id: '', name: t('arrange.similar.name'), title: t('arrange.similar.title') }, ...named.map(id => ({ id, name: t(`arrange.${id}.name`), title: t(`arrange.${id}.title`) }))]
  }, [model, t])
  // A new layout moves every sound from where it was last drawn to its new place, the middle leading, as the tiles do;
  // from where it was drawn, so a change in the middle of a move turns smoothly. A new map starts where it is. The
  // move is noted as the layout changes, before the frame that first draws it, so no frame jumps ahead.
  const drawn = useRef<{ model: MapModel; places: Float32Array } | null>(null)
  const shift = useRef<{ from: Float32Array; at: number } | null>(null)
  const shiftedTo = useRef(arranged)
  if (shiftedTo.current !== arranged) {
    shiftedTo.current = arranged
    const last = drawn.current
    shift.current = !still && last && last.model === model && last.places.length === model.points.length * 2 ? { from: last.places.slice(), at: performance.now() } : null
  }
  useEffect(() => {
    if (shift.current) animate(ARRANGE_MS)
  }, [arranged]) // eslint-disable-line react-hooks/exhaustive-deps
  // The tile view's model: laid out (in the layout shown) the first time tiles show, and kept while the morph runs
  // back to dots.
  const layout = useMemo(() => (tiles || morph.from > 0 ? tileLayout(arranged) : null), [arranged, tiles, morph.from > 0]) // eslint-disable-line react-hooks/exhaustive-deps
  const tiled = useMemo(() => (layout ? tileModel(arranged, layout) : null), [arranged, layout])
  // What the pointer, the keyboard, a box and a find's frame work on: the tiles once they show, else the dots, in the
  // layout shown.
  const active = tiles && tiled ? tiled : arranged
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
  // CC-23: a near search names its seeds (the top-level `near`) from this map, the first by name and how many more;
  // seeds the map doesn't list head nothing.
  const nearName = useMemo(() => {
    const seeds = !searched && foundIds ? (found?.near ?? []).filter(id => model.sounds[id]) : []
    if (seeds.length === 0) return ''
    const first = clean(model.sounds[seeds[0]].name ?? '', 60)
    return seeds.length === 1 ? t('map.near', first) : t('map.nearMany', first, seeds.length - 1)
  }, [searched, foundIds, found, model, t])

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

  // Follow the box's size (the pane or tab is resized all the time), and the height of what floats over its top.
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
  useEffect(() => {
    const top = topRef.current
    if (!top) return
    // The overlay sits 8 px down; the legend's offset is within it. 8 px more keeps the map's margin clear of it.
    const observer = new ResizeObserver(() => {
      const legend = legendRef.current
      setTopInset(legend ? Math.ceil(8 + legend.offsetTop + legend.offsetHeight + 8) : 0)
    })
    observer.observe(top)
    return () => observer.disconnect()
  }, [hasPoints])

  // A new map, or a new size before the user moved the view: fit the whole map, below what floats over its top.
  useEffect(() => {
    if (size.width > 0 && size.height > 0) setView(current => current ?? fit())
  }, [size.width, size.height]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => setView(size.width > 0 ? fit() : null), [model.revision]) // eslint-disable-line react-hooks/exhaustive-deps
  // The top's height settles once its legend has laid out, and a side sheet opens and closes: a view nobody has moved
  // yet follows them.
  const moved = useRef(false)
  useEffect(() => {
    if (!moved.current && size.width > 0) setView(fit())
  }, [topInset, coverRight]) // eslint-disable-line react-hooks/exhaustive-deps

  // What the agent found: frame it, once the map has a size. Only once: a later resize (the palette drawer opening, a
  // side sheet) leaves the producer's own view alone.
  const framedFound = useRef<ReadonlySet<string> | null>(null)
  useEffect(() => {
    if (!foundIds || size.width === 0 || framedFound.current === foundIds) return
    framedFound.current = foundIds
    const framed = frame(active, foundIds, size.width, size.height, topInset, coverRight)
    if (framed) setView(framed)
  }, [foundIds, model, size.width, size.height]) // eslint-disable-line react-hooks/exhaustive-deps

  // The wheel (a native listener: React's wheel handler can't stop the page from scrolling). A pinch, which comes as a
  // wheel with Ctrl held, zooms; so does a mouse's wheel; a trackpad's two-finger scroll pans (`wheelPans`).
  const wheelMode = useRef(wheel)
  wheelMode.current = wheel
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const memory: WheelMemory = { pans: null, at: 0 }
    const onWheel = (event: WheelEvent) => {
      event.preventDefault()
      const base = nextView.current ?? viewRef.current
      if (!base) return
      const legacy = event as WheelEvent & { wheelDeltaX?: number; wheelDeltaY?: number }
      const pans = !event.ctrlKey && (wheelMode.current === 'auto' ? wheelPans(legacy, memory, performance.now()) : wheelMode.current === 'pan')
      // Lines and pages (Firefox's mouse wheel) count as 16 and 400 pixels.
      const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 400 : 1
      if (pans) {
        queueView({ ...base, dx: base.dx - event.deltaX * unit, dy: base.dy - event.deltaY * unit })
        return
      }
      const rect = canvas.getBoundingClientRect()
      const factor = Math.exp(-event.deltaY * unit * (event.ctrlKey ? 0.01 : 0.0025))
      queueView(zoomAt(base, factor, event.clientX - rect.left, event.clientY - rect.top))
    }
    canvas.addEventListener('wheel', onWheel, { passive: false })
    return () => canvas.removeEventListener('wheel', onWheel)
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
    // The layout's move: how far it is, and where each sound set off from (map units, as last drawn).
    const dots = arranged.points
    const moving = shift.current
    const shifting = moving && !still ? clamp01((now - moving.at) / ARRANGE_MS) : 1
    const from = moving && shifting < 1 && moving.from.length === count * 2 ? moving.from : null
    // Where each sound is drawn this frame (map units), for the next layout's move to start from.
    const places = drawn.current?.model === model && drawn.current.places.length === count * 2 ? drawn.current.places : new Float32Array(count * 2)
    drawn.current = { model, places }
    const placeOf = (index: number): [number, number, number] => {
      const dot = dots[index]
      const blend = blendOf(index)
      let x = tilePoints ? dot.x + (tilePoints[index].x - dot.x) * blend : dot.x
      let y = tilePoints ? dot.y + (tilePoints[index].y - dot.y) * blend : dot.y
      if (from) {
        const along = easeInOut(clamp01(shifting * (1 + STAGGER) - spread[index] * STAGGER))
        x = from[2 * index] + (x - from[2 * index]) * along
        y = from[2 * index + 1] + (y - from[2 * index + 1]) * along
      }
      places[2 * index] = x
      places[2 * index + 1] = y
      return [x * unit + view.dx, y * unit + view.dy, blend]
    }
    // A new map's points pop in from the middle out: each grows from nothing, just past its size, then settles.
    const introFor = (index: number): number => {
      const since = now - introAt.current - spread[index] * (INTRO_MS * 0.45)
      return since >= INTRO_MS * 0.55 ? 1 : since <= 0 ? 0 : easeOutBack(since / (INTRO_MS * 0.55))
    }
    // A dot is its group's glyph (a kind reads by shape as well as colour); a tile a rounded square; between the two
    // while the morph runs.
    const glyphs = new Map(model.groups.map(group => [group.key, glyphOf(group.key)]))
    const shape = (path: Path2D, x: number, y: number, blend: number, grow = 1, glyph = 0) => {
      if (blend <= 0) {
        const r = radius * grow
        if (r <= 0.05) return
        traceGlyph(path, glyph, x, y, r)
        return
      }
      const side = (2 * radius + (tileSide - 2 * radius) * blend) * grow
      if (side <= 0.1) return
      const corner = side / 2 - (side / 2 - side * 0.18) * blend
      if (typeof path.roundRect === 'function') path.roundRect(x - side / 2, y - side / 2, side, side, corner)
      else path.rect(x - side / 2, y - side / 2, side, side)
    }
    const glyphAt = (index: number): number => glyphs.get(points[index].group) ?? 0
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
        shape(path, x, y, blend, introFor(index), glyphAt(index))
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
      shape(path, x, y, blend, 1 - eased + 0.02, glyphAt(index))
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
    // The layout's words: its data words where they belong (keys, BPM, folder names…), then its axis words inside the
    // map's edges, kept in view. They fade in as the sounds arrive, haloed in the ground colour to read over points.
    const layoutShown = arrangementIndex >= 0 ? model.arrangements[arrangementIndex] : null
    const wordsIn = clamp01((shifting - 0.55) / 0.45)
    if (layoutShown && wordsIn > 0) {
      const family = getComputedStyle(canvas).fontFamily || 'sans-serif'
      context.textBaseline = 'middle'
      context.lineJoin = 'round'
      context.strokeStyle = colors.ground
      context.lineWidth = 3
      context.textAlign = 'center'
      context.font = `600 11px ${family}`
      context.fillStyle = colors.strong
      context.globalAlpha = 0.9 * wordsIn
      for (const label of layoutShown.labels) {
        const [x, y] = [label.x * unit + view.dx, label.y * unit + view.dy]
        if (x < left - 80 || y < top || x > right + 80 || y > bottom) continue
        context.strokeText(label.text, x, y)
        context.fillText(label.text, x, y)
      }
      context.font = `11px ${family}`
      context.fillStyle = colors.text
      context.globalAlpha = 0.85 * wordsIn
      const [x0, y0, x1, y1] = [view.dx, view.dy, view.dx + unit, view.dy + unit]
      const [minX, maxX, minY, maxY] = [8, size.width - coverRight - 8, topInset + 10, size.height - 12]
      const keepY = (y: number) => Math.min(Math.max(y, minY), maxY)
      const middleX = Math.min(Math.max((x0 + x1) / 2, minX + 60), maxX - 60)
      for (const side of AXES[layoutShown.id] ?? []) {
        const word = t(`arrange.${layoutShown.id}.${side}`)
        const [text, x, y, align]: [string, number, number, CanvasTextAlign] =
          side === 'left'
            ? [`← ${word}`, Math.max(x0 + 6, minX), keepY((y0 + y1) / 2), 'left']
            : side === 'right'
              ? [`${word} →`, Math.min(x1 - 6, maxX), keepY((y0 + y1) / 2), 'right']
              : side === 'top'
                ? [`↑ ${word}`, middleX, keepY(y0 + 10), 'center']
                : [`↓ ${word}`, middleX, keepY(y1 - 10), 'center']
        context.textAlign = align
        context.strokeText(text, x, y)
        context.fillText(text, x, y)
      }
      context.textAlign = 'start'
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
      shape(path, x, y, blend, 1.45, glyphAt(hovered))
      context.save()
      context.shadowColor = color
      context.shadowBlur = 14
      context.fillStyle = color
      context.fill(path)
      context.restore()
      // And a fine ring in the text colour, so one sound stands out of a dense cluster of its own colour.
      const ring = new Path2D()
      shape(ring, x, y, blend, 2.3, glyphAt(hovered))
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
  }, [model, arranged, arrangementIndex, indexOf, view, size, mask, foundIds, selected, playing, mapSize, painted, clock, marked, picked, tiled, layout, morph, spread, flying, still, hoverAt, t, topInset, coverRight])

  const at = (event: { clientX: number; clientY: number }): [number, number] => {
    const rect = canvasRef.current?.getBoundingClientRect()
    return rect ? [event.clientX - rect.left, event.clientY - rect.top] : [0, 0]
  }
  // How near the pointer must be to a sound to reach it: a dot's few pixels, or most of a tile.
  const reachPx = (base: number): number => (tiles && layout && view ? Math.max(base, ((mapSize * view.scale) / layout.side) * 0.62) : base)

  // A press with the left button starts a box (Shift, Command or Ctrl held: one more box, added to the picks); with the
  // middle button, with Space held or the hand tool on, a pan. A left press that doesn't move picks the sound under it:
  // Command or Ctrl picks or unpicks it, Shift adds it to the picks, and a plain click picks and plays it.
  const press = (event: PointerEvent<HTMLCanvasElement>) => {
    if (!view || (event.button !== 0 && event.button !== 1)) return
    // The middle button scrolls the page on some systems (autoscroll): not over the map.
    if (event.button === 1) event.preventDefault()
    event.currentTarget.setPointerCapture?.(event.pointerId)
    const [x, y] = at(event)
    const boxing = event.button === 0 && Boolean(onPicked) && !space.current && !handTool && !grow?.choosing
    drag.current = { x, y, moved: false, view, box: boxing, add: event.shiftKey || event.metaKey || event.ctrlKey, button: event.button }
    rest(null)
    setHover(null)
    if (boxing) setBox([x, y, x, y])
    else setGrabbing(true)
  }
  const release = (event: PointerEvent<HTMLCanvasElement>) => {
    const held = drag.current
    drag.current = null
    setBox(null)
    setGrabbing(false)
    if (!held || !view) return
    const [x, y] = at(event)
    if (held.box && held.moved) {
      const inside = pointsInBox(active, view, mapSize, [held.x, held.y, x, y], visible).map(index => model.points[index].id)
      onPicked?.(inside, held.add ? 'add' : 'replace')
      return
    }
    if (held.moved || held.button !== 0) return
    const index = pick(active, view, mapSize, x, y, reachPx(10), visible)
    const adding = event.shiftKey || event.metaKey || event.ctrlKey
    if (index < 0) {
      // A plain click on empty map lets go of everything: the picks, and the sound picked last.
      if (adding) return
      if (pickedIds.length > 0) onPicked?.([], 'replace')
      if (selected !== null) onSelect(null)
      return
    }
    const id = model.points[index].id
    if (grow?.choosing) {
      const end = endOf(id)
      if (end) grow.name(end)
      onPick(id)
    } else if (onPicked && (event.metaKey || event.ctrlKey)) onPicked([id], 'toggle')
    else if (onPicked && event.shiftKey) onPicked([id], 'add')
    else onPick(id)
  }

  // Grow's ends: a map sound as a source (with the map it's from); where an end sits in map units (a map sound wherever
  // the layout shown puts it, a made one where its bud grew); and its colour (a made one's is the accent).
  const endOf = (id: string): GrowEnd | null => soundEnd(id, model.sounds[id]?.name, model.revision)
  const placeOfEnd = (end: GrowEnd | null): readonly [number, number] | null => {
    if (!end) return null
    if (end.id === null) return end.place
    const index = indexOf.get(end.id)
    return index === undefined ? null : [active.points[index].x, active.points[index].y]
  }
  const colorOfEnd = (end: GrowEnd | null): string => {
    const index = end?.id ? indexOf.get(end.id) : undefined
    return index === undefined ? T.accent : (model.groups.find(group => group.key === model.points[index].group)?.color ?? T.accent)
  }
  const screenOf = (place: readonly [number, number] | null): [number, number] | null => (place && view ? toScreen(view, mapSize, place[0], place[1]) : null)

  const onKey = (event: KeyboardEvent<HTMLCanvasElement>) => {
    // Escape stops Grow naming its sounds, else lets go of the picks. Only while there's one of those: otherwise it
    // stays Desktop's (it stops a running turn), which stands down for a key the map already took.
    if (event.key === 'Escape' && grow?.choosing && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey) {
      event.preventDefault()
      grow.choose(false)
      return
    }
    if (event.key === 'Escape' && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey && pickedIds.length > 0 && onPicked) {
      event.preventDefault()
      onPicked([], 'replace')
      setSaid(t('dock.cleared'))
      return
    }
    const key = mapKey(event)
    if (!view || !key) return
    event.preventDefault()
    const from = selected === null ? undefined : indexOf.get(selected)
    const picked = from !== undefined && visible(from) ? from : undefined
    const middle: [number, number] = [size.width / 2, size.height / 2]
    if ('fit' in key) {
      moved.current = false
      setView(fit())
    } else if ('zoom' in key) {
      // Around the sound picked, so it stays where it is; else around the middle.
      const [x, y] = picked === undefined ? middle : toScreen(view, mapSize, active.points[picked].x, active.points[picked].y)
      moved.current = true
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

  // Where the picks sit on screen, for their actions to float beside them; null with none, or none in view.
  const picksBox = useMemo((): [number, number, number, number] | null => {
    if (!view || pickedIds.length === 0) return null
    let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity]
    for (const id of pickedIds) {
      const index = indexOf.get(id)
      if (index === undefined || mask[index] === 2) continue
      const [x, y] = toScreen(view, mapSize, active.points[index].x, active.points[index].y)
      ;[x0, y0, x1, y1] = [Math.min(x0, x), Math.min(y0, y), Math.max(x1, x), Math.max(y1, y)]
    }
    const inView = Number.isFinite(x0) && x1 >= 0 && y1 >= 0 && x0 <= size.width && y0 <= size.height
    return inView ? [x0, y0, x1, y1] : null
  }, [view, pickedIds, indexOf, mask, active, mapSize, size.width, size.height])
  const cluster = useRef<HTMLDivElement | null>(null)
  const [clusterSize, setClusterSize] = useState<[number, number]>([320, 40])
  const hasPicks = pickedIds.length > 0
  useEffect(() => {
    const element = cluster.current
    if (!element) return
    const observer = new ResizeObserver(entries => {
      const rect = entries[0]?.contentRect
      if (rect) setClusterSize([Math.ceil(rect.width) + 12, Math.ceil(rect.height) + 12])
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [hasPicks])

  // No sounds yet: what builds the map, in the middle of the pane it will fill.
  if (!model.points.length) {
    return (
      <div style={{ ...S.muted, fontSize: 13, flex: 1, height: '100%', minHeight: 240, display: 'flex', alignItems: 'center', justifyContent: 'center', padding: 24, textAlign: 'center' }}>
        <div style={{ maxWidth: 380 }}>
          {empty}
          {(map?.hidden ?? 0) > 0 && (
            <div role="status" style={{ ...S.row, flexWrap: 'wrap', justifyContent: 'center', gap: 6, marginTop: 8 }}>
              <span style={{ ...S.faint, flex: '1 1 14rem' }}>{t('map.hidden', map?.hidden ?? 0)}</span>
              {hiddenActions}
            </div>
          )}
        </div>
      </div>
    )
  }

  const chosen = selected ? model.sounds[selected] : undefined
  const chosenIndex = selected ? indexOf.get(selected) : undefined
  const chosenColor = chosenIndex === undefined ? T.accent : (model.groups.find(group => group.key === model.points[chosenIndex].group)?.color ?? T.accent)
  const hovered = hover ? model.points[hover.index] : undefined
  const hoveredSound = hovered ? model.sounds[hovered.id] : undefined
  const countWords =
    shown === model.points.length - leavingCount ? t('map.count', num(shown, 0)) : t('map.countOf', num(shown, 0), num(model.points.length - leavingCount, 0))
  const hiddenCount = (map?.hidden ?? 0) + leavingCount
  // The picks' actions float beside the picks, never over them; with the picks out of view, at the map's foot (above
  // the player, when there is one).
  const clusterAt = floatBeside(picksBox, clusterSize[0], clusterSize[1], size.width - coverRight, size.height, topInset, chosen ? 120 : 10)

  return (
    <div style={{ position: 'relative', height: '100%', minHeight: 240, overflow: 'hidden', color: T.text }}>
      {/* What floats over the map's top: the search and the tools, the legend, a search's or a find's sounds by name, and
          the quiet line about sounds that can't play. Only the pieces take the pointer; the map shows through the gaps.
          It comes before the canvas, so the keyboard meets the search and the tools first, as it always has; z-index
          keeps it above the canvas, drawn after it. */}
      <div ref={topRef} style={{ position: 'absolute', top: 8, left: 8, right: 8, zIndex: 1, display: 'flex', flexDirection: 'column', alignItems: 'flex-start', gap: 6, pointerEvents: 'none' }}>
        <div style={{ ...S.row, alignSelf: 'stretch', gap: 8, flexWrap: 'wrap', alignItems: 'flex-start' }}>
          {/* The count sits inside the search field. The revision is an id (`r` and 32 hex digits): there for anyone who
              looks, not in the line itself. */}
          <div style={{ ...glass, borderRadius: 18, position: 'relative', display: 'flex', alignItems: 'center', flex: '1 1 180px', maxWidth: 420, minWidth: 150, height: 36, pointerEvents: 'auto' }}>
            <input
              aria-label={t('map.search')}
              className={FOCUSABLE}
              onChange={event => setQuery(event.target.value)}
              placeholder={t('map.searchHint')}
              style={{ flex: 1, minWidth: 0, height: '100%', font: 'inherit', fontSize: 12.5, padding: '0 12px', paddingRight: `${countWords.length * 0.55 + 1.6}em`, borderRadius: 18, border: 'none', background: 'transparent', color: T.text, outlineOffset: -2 }}
              value={query}
            />
            <span
              style={{ ...S.faint, fontSize: 10.5, lineHeight: '18px', padding: '0 7px', borderRadius: 9, background: T.chip, position: 'absolute', right: 8, top: '50%', transform: 'translateY(-50%)', whiteSpace: 'nowrap', cursor: 'default' }}
              title={model.revision ? t('map.revision', model.revision) : undefined}
            >
              {countWords}
            </span>
          </div>
          <span style={{ flex: 1 }} />
          <ToolGroup label={t('map.tools')}>
            {onPicked && <ToolButton icon="pick" label={t('map.pickTool')} onClick={() => setHandTool(false)} pressed={!handTool} title={t('map.pickToolTitle')} />}
            {onPicked && <ToolButton icon="hand" label={t('map.hand')} onClick={() => setHandTool(true)} pressed={handTool} title={t('map.handTitle')} />}
            {grow && <ToolButton icon="grow" label={t('grow.tool')} onClick={() => grow.choose(!grow.choosing)} pressed={grow.choosing !== null} title={t('grow.toolTitle')} />}
            {onPicked && <Rule />}
            {layoutChoices.length > 0 && <LayoutMenu choices={layoutChoices} current={arrangementIndex >= 0 ? arrangementId : ''} label={t('arrange.label')} onChoose={chooseArrangement} />}
            {onTiles && <ToolButton icon="tiles" label={t('map.tiles')} onClick={() => onTiles(!tiles)} pressed={tiles} title={t('map.tilesTitle')} />}
            {tools}
            <Rule />
            <ToolButton
              icon="fit"
              label={t('map.fit')}
              onClick={() => {
                moved.current = false
                setView(fit())
              }}
              title={t('map.fitTitle')}
            />
          </ToolGroup>
        </div>
        <div aria-label={t('map.legend')} ref={legendRef} role="group" style={{ ...glass, borderRadius: 14, ...S.row, flexWrap: 'wrap', gap: 4, padding: 4, pointerEvents: 'auto', maxWidth: '100%' }}>
          {/* Serve names every class it knows, most of them empty in a small library: the legend lists those with sounds. */}
          {model.groups.filter(group => group.count > 0).map(group => {
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
                style={{ ...S.row, gap: 5, font: 'inherit', fontSize: 11, padding: '2px 8px 2px 6px', borderRadius: 10, border: 'none', background: off ? 'transparent' : `color-mix(in srgb, ${group.color} 12%, transparent)`, color: off ? T.faint : T.secondary, cursor: 'pointer', opacity: off ? 0.55 : 1, transition: 'background-color 140ms, opacity 140ms' }}
              >
                <svg aria-hidden height="9" style={{ flex: 'none', opacity: off ? 0.35 : 1 }} viewBox="-1.5 -1.5 3 3" width="9">
                  <GlyphShape color={group.color} glyph={glyphOf(group.key)} />
                </svg>
                {group.name} <span style={{ color: T.faint }}>{group.count}</span>
              </button>
            )
          })}
        </div>
        {grow?.choosing && (
          // Grow names its sounds with clicks on the map: which one this click names, and a way out.
          <div className="kumi-pop" role="status" style={{ ...glass, borderRadius: 16, ...S.row, gap: 8, padding: '4px 6px 4px 12px', pointerEvents: 'auto', maxWidth: '100%' }}>
            <Icon name="grow" size={13} />
            <span style={{ fontSize: 12, color: T.text }}>{grow.choosing === 'from' ? t('grow.from') : t('grow.to', clean(grow.from?.name ?? '', 60))}</span>
            <TextButton onClick={() => grow.choose(false)}>{t('grow.cancel')}</TextButton>
          </div>
        )}
        {matches.length > 0 && (
          <div aria-label={searched ? t('map.matches') : nearName || t('map.found')} role="group" style={{ ...glass, display: 'flex', flexDirection: 'column', gap: 1, padding: '6px 6px 6px', width: 'min(340px, 100%)', pointerEvents: 'auto' }}>
            <div style={{ ...S.row, gap: 6, padding: '0 4px 4px' }}>
              {/* CC-23: a near search's neighbours, under the seeds' names from this map (the seeds themselves aren't listed). */}
              <span style={{ ...S.muted, ...S.ellipsis, flex: 1, fontSize: 11.5 }}>{searched ? t('map.matches') : nearName || t('map.found')}</span>
              {onPicked && (
                // Native's `a`: every sound the search keeps, or every sound Kumi found (a "More like these" among them).
                <TextButton onClick={() => onPicked(matches, 'replace')} title={searched !== null ? t('dock.pickAllTitle') : t('dock.pickFoundTitle')}>
                  {t('dock.pickAll', matches.length)}
                </TextButton>
              )}
            </div>
            {matches.slice(0, MATCHES_LISTED).map(id => {
              const sound = model.sounds[id]
              const indexed = indexedValues(sound, t)
              // A typed search lists names; the agent's find also says why each matched, as Kumi bounds it (CC-23).
              const reasons = searched ? '' : foundLine(found?.matches?.[id], nearName !== '', t)
              return (
                <button
                  aria-pressed={selected === id}
                  className={FOCUSABLE}
                  key={id}
                  onClick={() => onPick(id)}
                  type="button"
                  style={{ display: 'flex', flexDirection: 'column', alignItems: 'stretch', gap: 0, font: 'inherit', fontSize: 11.5, padding: '2px 6px', borderRadius: 6, border: 'none', textAlign: 'left', cursor: 'pointer', color: T.text, background: selected === id ? T.card : 'transparent' }}
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
                  {reasons && (
                    <span style={{ ...S.ellipsis, color: T.faint, fontSize: 10.5 }} title={nearName ? t('map.closenessTitle') : undefined}>
                      {reasons}
                    </span>
                  )}
                </button>
              )
            })}
            {matches.length > MATCHES_LISTED && <span style={{ ...S.faint, padding: '0 6px' }}>{t('map.more', matches.length - MATCHES_LISTED)}</span>}
          </div>
        )}
        {hiddenCount > 0 && (
          // One quiet line for every sound that couldn't play here, never a notice per sound.
          <div role="status" style={{ ...glass, borderRadius: 14, ...S.row, flexWrap: 'wrap', gap: 6, padding: '3px 6px 3px 10px', pointerEvents: 'auto', maxWidth: '100%' }}>
            <span style={{ ...S.faint, fontSize: 11 }}>{t('map.hidden', hiddenCount)}</span>
            {hiddenActions}
          </div>
        )}
      </div>
      <div ref={boxRef} style={{ position: 'absolute', inset: 0 }}>
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
            setGrabbing(false)
          }}
          onPointerEnter={() => {
            over.current = true
          }}
          onPointerLeave={() => {
            over.current = false
            setHover(null)
            rest(null)
          }}
          // The middle button's own press would start the page's autoscroll on some systems.
          onMouseDown={event => {
            if (event.button === 1) event.preventDefault()
          }}
          onDoubleClick={() => {
            moved.current = false
            setView(fit())
          }}
          role="application"
          tabIndex={0}
          // The box clips what's outside it: the focus ring sits inside the canvas's edge.
          style={{
            position: 'absolute',
            inset: 0,
            width: '100%',
            height: '100%',
            outlineOffset: -3,
            cursor: grabbing ? 'grabbing' : spaceHeld || handTool || !onPicked ? 'grab' : box ? 'crosshair' : hover ? 'pointer' : 'default',
            touchAction: 'none'
          }}
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
            style={{ ...glass, borderRadius: 8, position: 'absolute', left: Math.min(hover.x + 14, Math.max(0, size.width - 230)), top: Math.max(0, hover.y - 38), maxWidth: 230, pointerEvents: 'none', padding: '4px 9px', fontSize: 11.5, color: T.text, ...S.ellipsis }}
          >
            {clean(hoveredSound.name, 48)} <span style={{ color: T.muted }}>· {clean(hoveredSound.class ?? '', 16)}</span>
          </div>
        )}
        {grow && (grow.from || grow.to) && (
          <GrowLayer
            actions={growActions}
            from={screenOf(placeOfEnd(grow.from))}
            fromColor={colorOfEnd(grow.from)}
            fromMap={placeOfEnd(grow.from)}
            grow={grow}
            height={size.height}
            onPlay={result => onPlayMade?.(result)}
            onStop={() => onStop?.()}
            playing={playing}
            to={screenOf(placeOfEnd(grow.to))}
            toColor={colorOfEnd(grow.to)}
            toMap={placeOfEnd(grow.to)}
            top={topInset}
            width={size.width - coverRight}
          />
        )}
      </div>
      {hasPicks && (
        // The picks' actions, beside the picks (`kumi-pop` as they appear): how many, then what they can do.
        <div
          aria-label={t('dock.picked', pickedIds.length)}
          className="kumi-pop"
          ref={cluster}
          role="group"
          style={{
            ...glass,
            position: 'absolute',
            zIndex: 1,
            left: clusterAt.left,
            top: clusterAt.top,
            maxWidth: `calc(100% - 16px)`,
            ...S.row,
            flexWrap: 'wrap',
            gap: 6,
            padding: '5px 6px 5px 10px',
            transition: still ? 'none' : 'left 220ms cubic-bezier(.2,.8,.2,1), top 220ms cubic-bezier(.2,.8,.2,1)'
          }}
        >
          <span style={{ ...S.row, gap: 5, fontSize: 12, fontWeight: 650, color: T.text }}>
            <Icon name="pick" size={12} />
            {t('dock.picked', pickedIds.length)}
          </span>
          {pickedActions?.(pickedIds)}
        </div>
      )}
      {selected && chosen && (
        // The sound picked last, in the player at the map's foot (left of a side sheet, when one is open).
        <div style={{ position: 'absolute', left: 10, right: 10 + coverRight, bottom: 10, zIndex: 1, display: 'flex', justifyContent: 'center', pointerEvents: 'none', transition: still ? 'none' : 'right 260ms cubic-bezier(.2,.8,.2,1)' }}>
          <div style={{ width: 'min(640px, 100%)' }}>
            <Player
              actions={actions?.(selected, chosen)}
              audio={audio ?? (() => Promise.reject(new Error('no audio')))}
              color={chosenColor}
              id={selected}
              key={selected}
              onClose={() => onSelect(null)}
              onPlay={() => onPick(selected)}
              onStop={() => onStop?.()}
              playing={playing === selected}
              sound={chosen}
            />
          </div>
        </div>
      )}
    </div>
  )
}

/** A group's glyph as SVG, on the unit circle (the legend's chips: the shape the map draws that group's sounds in). */
function GlyphShape({ glyph, color }: { glyph: number; color: string }) {
  const shape = GLYPH_SHAPES[glyph]
  return shape ? <polygon fill={color} points={shape.map(([x, y]) => `${x},${y}`).join(' ')} /> : <circle cx="0" cy="0" fill={color} r="1" />
}
