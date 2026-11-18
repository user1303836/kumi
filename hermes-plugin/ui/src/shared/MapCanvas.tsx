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
import { Button, Tip } from '@kumi/controls'
import { CANVAS, T, useThemeKey } from '@kumi/theme'
import type { KeyboardEvent, PointerEvent, ReactNode } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'

import { AtlasOverlay } from './atlas/AtlasOverlay.tsx'
import { nearestTo, STARS } from './atlas/constellation.ts'
import { Corner, ToolName } from './atlas/Corner.tsx'
import { FIRST_STEPS, firstLeft, type FirstStep } from './atlas/first.ts'
import { FirstOpen } from './atlas/FirstOpen.tsx'
import { gesturesOf, hintOf, type MapState } from './atlas/gestures.ts'
import { glowOf, HALOS_ON_SCREEN } from './atlas/glow.ts'
import { HoverPill } from './atlas/HoverPill.tsx'
import { drawNames, drawWords, type Word } from './atlas/labels.ts'
import { nameRank, NAMES_ON_SCREEN, regionAlpha, regionsOf } from './atlas/names.ts'
import { haloOf, nebulaAlpha } from './atlas/nebula.ts'
import { PickBox } from './atlas/PickBox.tsx'
import type { DesignResult } from './design.ts'
import { clean, num } from './format.ts'
import { type Grow, type GrowEnd, soundEnd, vinePoint } from './grow.ts'
import { GrowLayer } from './GrowLayer.tsx'
import { useLessMotion } from './hooks.ts'
import { arrangedModel, buildModel, fitView, floatBeside, frame, GLYPH_SHAPES, glyphOf, indexedValues, keepInView, keepPlace, mapKey, mapLabel, type MapModel, mapSide, type MapViewport, moveWith, pick, placedModel, pointsInBox, searchSounds, tileLayout, tileModel, toScreen, traceGlyph, type WheelMemory, type WheelMode, wheelPans, zoomAt } from './mapModel.ts'
import { glass, Icon, type LayoutChoice, LayoutMenu, Rule, ToolButton, ToolGroup } from './mapUi.tsx'
import { Player } from './Player.tsx'
import type { KumiState } from './store.ts'
import type { MapSound, SoundMap } from './types.ts'
import { FOCUSABLE, S, TYPE, useEdgeFade, useT } from './ui.tsx'

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
  /** Something of the parent's lies over the map (Make over a narrow map): the find's card, which would peek out from
   *  under it, isn't drawn until it's gone. */
  veiled?: boolean
  /** The layout shown: '' for similarity, else one of the map's arrangements by id (one it lacks is similarity); the
   *  parent keeps it, hearing each choice in `onArrangement`. Without it the map keeps its own. */
  arrangement?: string
  onArrangement?: (id: string) => void
  /** Grow's vine, drawn over the map; while it names its ends, a click on a sound names one (and plays it) instead of
   *  picking it. `growActions` are the parent's for a grown point (Keep, Use…, Make from); `onPlayMade` plays one. */
  grow?: Grow
  growActions?: (result: DesignResult) => ReactNode
  onPlayMade?: (result: DesignResult) => void
  /** Hover-to-play is on: the pointer leaves a trail over the map, in the colours of what it played. */
  trail?: boolean
  /** Sounds to ring in their own colours: the parents of the made sound the pointer or the keyboard is on. */
  kin?: ReadonlySet<string> | null
  /** The first-open steps (`atlas/first.ts`): which the host has kept as done, and what to call as more are (hearing
   *  a sound and drawing a box are seen here; the host sees Make… pressed). Absent: the map shows none. */
  first?: { kept: string; onDone: (steps: readonly FirstStep[]) => void }
}

/** How many searched or found sounds are listed by name above the map; the rest stay rings on it. */
const MATCHES_LISTED = 6
/** How wide the map's free room must be for the corner's first line to hold the zoom too (the count, the layout's
 *  button and the zoom at 11 px); narrower, the zoom is left out rather than wrapped onto a line of its own. */
const CORNER_ZOOM_ROOM = 480
/** What the map keeps clear for its corner at its foot (its two lines and their margin): Fit, and a find's frame, put
 *  the map above it. Fixed, not measured: the map's size is the scale every view is drawn at, and must not jump when
 *  the corner's hint line wraps. */
const CORNER_ROOM = 76
/** The least room the map keeps between its top column and what docks at its foot (the player, then the corner):
 *  with less, the floating pieces give way. The corner folds to its first line, and steps aside altogether while a
 *  sound's card is open (the card already says what the corner would); a find's list folds to its head. */
const ROOM_LEAST = 200
/** The least height a find's list needs to show any of its rows (its head and a few): with less it folds to its head,
 *  and "Show" opens it anyway. */
const LIST_LEAST = 150
/** The free room the map keeps for its sounds while a find's list is open: a quarter of its height, and at least
 *  this. With less, the corner's first-open steps give way first (it folds to its first line), then the list folds to
 *  its head; a find's frame then lands in the room left. */
const FREE_LEAST = 160
/** What a find's list takes as laid out, for the map to decide before drawing it (deciding from its measured height
 *  would fold and unfold it in turn): its head with the shared facts, each row, and its "Show n more". */
const LIST_HEAD = 70
const LIST_ROW = 31
const LIST_MORE = 34
/** The corner's height with its first-open steps over its two lines, and folded to its first line. The folded height
 *  stands in for the measured one: in the render that folds the corner, the measure is still the unfolded corner's, and
 *  what's decided from it (the list's fold, a find's frame) would be decided for a corner that is gone. */
const CORNER_WITH_STEPS = 180
const CORNER_FOLDED = 40
/** How long a sound that couldn't play takes to fade off the map. */
const FADE_MS = 450
// The map's motion, all of it short and none of it while the map sits still: a new map's points pop in from the
// middle out, dots morph into tiles and back, and gathered sounds fly into the dock. (A playing sound's pulse is the
// overlay's, on a canvas of its own: `atlas/overlay.ts`.)
const INTRO_MS = 900
const MORPH_MS = 760
const FLY_MS = 620
const FLY_STAGGER = 14
const FLY_MOST = 48
/** How far apart, in a morph's span, the middle's points and the edge's move: the middle leads. */
const STAGGER = 0.55
/** How long the sounds take to move to a new layout. */
const ARRANGE_MS = 1000
/** Sounds just picked get their rings in a wave across them: POP_STEPS steps, each ring POP_EACH ms settling, POP_MS in
 *  all. */
const POP_MS = 180
const POP_EACH = 80
const POP_STEPS = 8
/** A layout's move draws each sound a tail as long as its way over this many milliseconds, whatever the frame rate. */
const STREAK_MS = 36
/** The layouts this plug-in can name, in the order the menu lists them (serve's `arrangements` ids). */
export const LAYOUTS = ['character', 'texture', 'keys', 'tempo', 'families', 'length', 'folders'] as const
/** The axis words each layout has (`arrange.<id>.<side>`): its x axis's ends, then its y axis's. */
export const AXES: Record<string, readonly ('bottom' | 'left' | 'right' | 'top')[]> = {
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

export function MapCanvas({ map, found, playing, selected, onPick, onSelect, actions, empty, leaving, hiddenActions, marked, picked, onPicked, onHover, tools, pickedActions, tiles = false, onTiles, flying, wheel = 'auto', audio, onStop, coverRight = 0, veiled = false, arrangement, onArrangement, grow, growActions, onPlayMade, trail = false, kin, first }: MapCanvasProps) {
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
  // The list of a search's or a find's sounds: all of them shown (not the first few), or put away, for the list it was
  // asked of; a new list starts folded and shown again.
  const [allOf, setAllOf] = useState<unknown>(null)
  const [closedOf, setClosedOf] = useState<unknown>(null)
  const [hidden, setHidden] = useState<Set<number>>(() => new Set())
  const [hover, setHover] = useState<{ index: number; x: number; y: number } | null>(null)
  // A drag: with the left button a box that picks what's in it (`add`: Shift, Command or Ctrl held, so it adds to the
  // picks); a pan with the middle button, with the left while Space is held or the hand tool is on, or on a map that
  // picks nothing.
  const drag = useRef<{ x: number; y: number; moved: boolean; view: MapViewport; box: boolean; add: boolean; button: number } | null>(null)
  const [box, setBox] = useState<[number, number, number, number] | null>(null)
  const [handTool, setHandTool] = useState(false)
  // What a scroll last did here (it moved the map: a trackpad; it zoomed: a mouse's wheel), for the hint line to say.
  const [scrollPans, setScrollPans] = useState<boolean | null>(null)
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
      // The map's own keyboard stop takes Space as a pick (a checkbox's key), never as a hold to pan.
      if (target === keysRef.current || target?.isContentEditable || target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return
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
  // The corner's box, the player's and Grow's card's: with what floats over the map's top and the picks' actions (where
  // they are going: `clusterBox`), the pieces no word on the map is written under. The corner's height also lifts
  // what docks at the map's foot (the picks' actions, Grow's card) clear of it.
  const cornerRef = useRef<HTMLDivElement | null>(null)
  const playerRef = useRef<HTMLDivElement | null>(null)
  const growCardRef = useRef<HTMLDivElement | null>(null)
  const clusterBox = useRef<[number, number, number, number] | null>(null)
  const [cornerHeight, setCornerHeight] = useState(0)
  // The sounds' names as the map last wrote them (each its box, and its sound's index): a click on one reaches it.
  const named = useRef<{ box: readonly [number, number, number, number]; index: number }[]>([])
  // The map's one keyboard stop (`keys` in the render): it stands for the sound the arrows reached, with a standard
  // role, and takes every key the map does; the canvas only takes the pointer.
  const keysRef = useRef<HTMLDivElement | null>(null)
  // What the overlay hands back to be called after each paint: what stands still on it follows the map.
  const overlayFollow = useRef<(() => void) | null>(null)
  const [topInset, setTopInset] = useState(0)
  // Where everything floating over the map's top ends (a find's card included): what floats beside the picks keeps below it.
  const [columnBottom, setColumnBottom] = useState(0)
  const mapSize = mapSide(size.width, size.height, topInset, CORNER_ROOM)
  // Fit shows the whole map in the room that is free (`fitView`): below the whole top column (a find's card in it),
  // above all that docks at the foot as measured (the player, the corner, the picks' actions), left of a side sheet.
  // Kept by each render, read when the map is fitted.
  const fitRoom = useRef({ top: 0, bottom: 0 })
  const fit = () => fitView(size.width, size.height, topInset, coverRight, CORNER_ROOM, fitRoom.current)
  // How far up from the map's foot what docks there reaches (the player and the corner, as last laid out): a find's
  // frame keeps above it, and no word is written under the corner. Kept by each render, read by the effects.
  const footing = useRef({ corner: 10, foot: 10 })
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
  // and not otherwise: a map that sits still draws nothing, and a sound playing redraws only the overlay. A system
  // asking for less motion gets none of it, from the moment it asks.
  const still = useLessMotion()
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
  // Sounds just picked: their rings arrive in a wave across them (top left first), not all at once.
  const popped = useRef<{ at: number; step: Map<string, number> } | null>(null)
  const pickedBefore = useRef<ReadonlySet<string> | undefined>(undefined)
  useEffect(() => {
    const before = pickedBefore.current
    pickedBefore.current = picked
    const places = drawn.current?.places
    const fresh = still || !picked || !places ? [] : [...picked].filter(id => !before?.has(id))
    if (fresh.length === 0 || !places) return
    // Across the map as it's drawn, top left first.
    const along = (id: string) => {
      const index = indexOf.get(id)
      return index === undefined ? 0 : places[2 * index] + places[2 * index + 1]
    }
    const ordered = fresh.map(id => [id, along(id)] as const).sort((a, b) => a[1] - b[1])
    popped.current = { at: performance.now(), step: new Map(ordered.map(([id], rank) => [id, Math.floor((rank / ordered.length) * POP_STEPS)])) }
    animate(POP_MS)
  }, [picked]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (performance.now() >= until.current) return
    const frame = requestAnimationFrame(setClock)
    return () => cancelAnimationFrame(frame)
  }, [clock])
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
  // When the map was last painted: a moving sound's tail is as long as its way over a set time (STREAK_MS).
  const lastPaint = useRef(0)
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
  // While the sounds are on their way (a layout's move, the dots' morph into tiles and back), the pointer, a box and the
  // keys meet them where they are drawn (`placedModel`, from the last frame's places), not where they are going: a
  // click mid-move picks the sound under it. Built at most once a frame, and only while something moves.
  const placed = useRef<{ paint: number; of: MapModel; model: MapModel } | null>(null)
  const live = (): MapModel => {
    const now = performance.now()
    const last = drawn.current
    const moving = !still && ((shift.current !== null && now - shift.current.at < ARRANGE_MS) || now - morph.at < MORPH_MS)
    if (!moving || !last || last.model !== model || last.places.length !== model.points.length * 2) return active
    if (placed.current?.paint !== lastPaint.current || placed.current.of !== model) placed.current = { paint: lastPaint.current, of: model, model: placedModel(model, last.places) }
    return placed.current.model
  }
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
  // The map's state as the pointer and the keys meet it (`atlas/gestures.ts`): the handlers below ask what a drag or
  // Escape does in it, and the corner's hint line is written from the same table. `held`: whether Space is held (the
  // handlers read it as it is now; the hint, as last drawn).
  const mapState = (held: boolean): MapState => ({ picks: Boolean(onPicked), hand: held || handTool, naming: Boolean(grow?.choosing), picked: pickedIds.length, hover: trail, scrollPans })
  // A made sound's parents on this map, each with its colour: the overlay rings them while the pointer is on the sound.
  const kinRings = useMemo(
    () =>
      [...(kin ?? [])].flatMap(id => {
        const index = indexOf.get(id)
        return index === undefined ? [] : [{ id, color: model.groups.find(group => group.key === model.points[index].group)?.color ?? T.accent }]
      }),
    [kin, indexOf, model]
  )
  // CC-23: a near search names its seeds (the top-level `near`) from this map, the first by name and how many more;
  // seeds the map doesn't list head nothing.
  const nearName = useMemo(() => {
    const seeds = !searched && foundIds ? (found?.near ?? []).filter(id => model.sounds[id]) : []
    if (seeds.length === 0) return ''
    const first = clean(model.sounds[seeds[0]].name ?? '', 60)
    return seeds.length === 1 ? t('map.near', first) : t('map.nearMany', first, seeds.length - 1)
  }, [searched, foundIds, found, model, t])

  // The canvas paints the theme's colours itself, read from the page (`resolve`); a change of them redraws it
  // (`painted` in the draw's inputs). Desktop's theme hook (v0.23 and later) says when the theme changes, but it ticks
  // before Desktop writes the new colours on the page: its provider repaints in an effect of its own, which runs after
  // this one (Hermes Desktop's src/themes/context.tsx and src/hooks/use-theme-epoch.ts say so). Read at the tick, a
  // light theme came out in the dark one's colours (dark halos round the map's words). So the colours are read on the
  // frame after it, once written. Where nothing says (older Desktops, the dashboard), they are looked at once a second.
  const themeKey = useThemeKey()
  const [painted, setPainted] = useState('')
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const look = () => {
      const now = Object.values(resolve(canvas)).join()
      setPainted(current => (current === now ? current : now))
    }
    if (themeKey === null) {
      const timer = setInterval(look, 1000)
      return () => clearInterval(timer)
    }
    const frame = requestAnimationFrame(look)
    return () => cancelAnimationFrame(frame)
  }, [hasPoints, themeKey])

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
      setColumnBottom(Math.ceil(8 + top.offsetHeight + 8))
    })
    observer.observe(top)
    return () => observer.disconnect()
  }, [hasPoints])

  // Whether the producer moved the view since it was last fitted, or a find framed it: a view nobody moved follows the
  // map's size, its top and a side sheet; a moved one stays where it was put until the map is fitted again.
  const moved = useRef(false)
  // The map's side changes as its top settles, its height changes (the palette drawer) or a side sheet opens: a view
  // somebody moved, or a find framed, keeps every sound where it is on screen (`keepPlace`) rather than sliding a
  // zoomed-in view's sounds out of sight. Done in the render that sees the new side, so no frame draws the slide.
  const sideWas = useRef(mapSize)
  if (sideWas.current !== mapSize) {
    const was = sideWas.current
    sideWas.current = mapSize
    if (moved.current && view) setView(keepPlace(view, was, mapSize))
  }
  // A new map, or a new size of a view nobody has moved: fit the whole map, below what floats over its top.
  useEffect(() => {
    if (size.width > 0 && size.height > 0) setView(current => (current && moved.current ? current : fit()))
  }, [size.width, size.height]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    moved.current = false
    setView(size.width > 0 ? fit() : null)
  }, [model.revision]) // eslint-disable-line react-hooks/exhaustive-deps
  // The top's height settles once its legend has laid out, and a side sheet opens and closes: a view nobody has moved
  // yet follows them.
  useEffect(() => {
    if (!moved.current && size.width > 0) setView(fit())
  }, [topInset, coverRight]) // eslint-disable-line react-hooks/exhaustive-deps

  // What the agent found: frame it, once the map has a size and its top has laid out, and once only. The frame counts
  // as a move, so a later resize (the palette drawer, a side sheet) keeps it until the producer fits the map again. It
  // keeps clear of the card that lists the finds (below it, or beside it, whichever leaves more room): the card used
  // to sit over the very sounds it named. A search's matches are framed the same way only when asked (the card's
  // "Zoom to them"): the view never moves under the producer's hands as they type.
  const framedFound = useRef<ReadonlySet<string> | null>(null)
  const matchesRef = useRef<HTMLDivElement | null>(null)
  const matchesList = useRef<HTMLDivElement | null>(null)
  useEdgeFade(matchesList)
  const frameSounds = (ids: Set<string>): void => {
    // The card sits in the top column (8 px in from the map's corner), which is its offset parent.
    const card = matchesRef.current
    let clear = {}
    if (card && card.offsetHeight > 0) {
      const below = { top: 8 + card.offsetTop + card.offsetHeight + 8 }
      const beside = { left: 8 + card.offsetLeft + card.offsetWidth + 8 }
      const { foot } = footing.current
      const areaBelow = (size.width - coverRight) * (size.height - foot - below.top)
      const areaBeside = (size.width - coverRight - beside.left) * (size.height - foot - topInset)
      clear = areaBelow >= areaBeside ? below : beside
    }
    // Above what docks at the foot too (the player and the corner): the frame is the room the map actually shows.
    const framed = frame(active, ids, size.width, size.height, topInset, coverRight, 48, { ...clear, bottom: footing.current.foot }, CORNER_ROOM)
    if (!framed) return
    moved.current = true
    setView(framed)
  }
  useEffect(() => {
    if (!foundIds || size.width === 0 || topInset === 0 || framedFound.current === foundIds) return
    framedFound.current = foundIds
    frameSounds(foundIds)
  }, [foundIds, model, size.width, size.height, topInset]) // eslint-disable-line react-hooks/exhaustive-deps

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
      if (!event.ctrlKey) setScrollPans(pans)
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

  // The sound under the pointer, for its glow: a move over the same sound draws nothing new. And its constellation:
  // the sounds nearest it in the layout shown (by similarity, its "more like this"), among those the search and the
  // legend keep. Tiles are a grid, where the nearest say nothing: none there.
  const hoverAt = hover ? hover.index : null
  const constellation = useMemo(() => (hoverAt === null || tiles ? [] : nearestTo(arranged.points, hoverAt, STARS, index => mask[index] === 1)), [arranged, hoverAt, mask, tiles])
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
    // Where each was drawn a frame ago, while a layout's move runs: each sound trails a short line from there, so the
    // move reads as a swarm in flight, not a slide. `tailed`: how much of the last frame's way a tail is.
    const before = from ? places.slice() : null
    const tailed = STREAK_MS / Math.max(8, now - lastPaint.current)
    lastPaint.current = now
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
    // `closed` is for the one path that is stroked: the filled ones leave each shape open (`traceGlyph` says why).
    const shape = (path: Path2D, x: number, y: number, blend: number, grow = 1, glyph = 0, closed = false) => {
      if (blend <= 0) {
        const r = radius * grow
        if (r <= 0.05) return
        traceGlyph(path, glyph, x, y, r, closed)
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
    // The sounds on screen, counted, and the first of them with their names and colours: past `NAMES_ON_SCREEN` there
    // are too many for any to carry its name.
    const labelled: [number, number, number, string][] = []
    let onScreen = 0
    // The glow under the sounds. First the layout's nebula: calmer as the map zooms in, absent under tiles, arriving
    // with a new map's sounds, and around a layout's move the old layout's fading out, then the new one's coming in
    // as the sounds arrive. Then, in the pass below, a halo under each sound the search and the legend keep, while
    // no more than `HALOS_ON_SCREEN` of them are on screen.
    const glow = glowOf(canvas)
    const dark = glow.onDark(context, colors.ground)
    const calm = nebulaAlpha(view.scale) * (1 - morphed) * clamp01((now - introAt.current) / INTRO_MS) * (dark ? 0.85 : 0.4)
    if (calm > 0) {
      const nebula = glow.nebula(arranged, mask, group => groupColor.get(group) ?? colors.faint)
      if (from && shifting < 0.3) {
        if (nebula.before) glow.draw(context, nebula.before, view, unit, calm * (1 - shifting / 0.3))
      } else if (nebula.now) glow.draw(context, nebula.now, view, unit, calm * (from ? clamp01((shifting - 0.7) / 0.3) : 1))
    }
    const halo = haloOf(radius, view.scale, glow.crowd(arranged), unit)
    // Dimmed points first, then the ones the search and legend keep, so those sit on top.
    for (const pass of [0, 1]) {
      const rounds = Array.from({ length: ROUNDS }, () => new Map<string, Path2D>())
      const halos = pass === 1 && morphed < 1 ? glow.halos(context, halo.radius, halo.alpha, ratio) : null
      // The halos wait for the count of sounds on screen: one more than `HALOS_ON_SCREEN` kept means none is drawn.
      const lit: [string, number, number, number][] = []
      const tails = pass === 1 && before ? new Map<string, Path2D>() : null
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
        const grown = introFor(index)
        if (halos && lit.length <= HALOS_ON_SCREEN) lit.push([color, x, y, grown * (1 - blend)])
        shape(path, x, y, blend, grown, glyphAt(index))
        if (tails && before) {
          const [dx, dy] = [x - (before[2 * index] * unit + view.dx), y - (before[2 * index + 1] * unit + view.dy)]
          if (dx * dx + dy * dy > 0.5) {
            let tail = tails.get(color)
            if (!tail) {
              tail = new Path2D()
              tails.set(color, tail)
            }
            tail.moveTo(x - dx * tailed, y - dy * tailed)
            tail.lineTo(x, y)
          }
        }
        if (pass === 1 && ++onScreen <= NAMES_ON_SCREEN) labelled.push([x, y, index, color])
      }
      if (halos) {
        if (lit.length <= HALOS_ON_SCREEN) for (const [color, x, y, strength] of lit) halos.at(color, x, y, strength)
        halos.done()
      }
      if (tails) {
        context.globalAlpha = 0.5
        context.lineWidth = Math.max(1, radius * 0.7)
        context.lineCap = 'round'
        for (const [color, tail] of tails) {
          context.strokeStyle = glow.shape(color)
          context.stroke(tail)
        }
      }
      context.globalAlpha = pass ? 0.92 : 0.16
      for (const paths of rounds) {
        for (const [color, path] of paths) {
          context.fillStyle = glow.shape(color)
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
    // The map's words, in the atlas's hand (`atlas/labels.ts`). A layout that writes its own data words (keys, BPM,
    // folder names; the families' names, each with its colour and count) has those, where they belong. Any other has
    // each family's name over its sounds, with its count: whole while the map is in view, gone by twice that, and
    // not for a family the legend hides. They fade in as the sounds arrive, haloed in the ground colour.
    const lettering = { family: getComputedStyle(canvas).fontFamily || 'sans-serif', ground: colors.ground, muted: colors.text }
    const layoutShown = arrangementIndex >= 0 ? model.arrangements[arrangementIndex] : null
    const wordsIn = clamp01((shifting - 0.55) / 0.45)
    let words: Word[] = []
    if (layoutShown && layoutShown.labels.length > 0) {
      const families = new Map(model.groups.map(group => [group.name, group]))
      for (const label of layoutShown.labels) {
        const [x, y] = [label.x * unit + view.dx, label.y * unit + view.dy]
        if (x < left - 80 || y < top || x > right + 80 || y > bottom) continue
        // A family's own label only where the layout is the families': elsewhere a folder named like a family is a
        // folder.
        const family = layoutShown.id === 'families' ? families.get(label.text) : undefined
        words.push(family ? { text: family.name.toLocaleUpperCase(), count: num(family.count, 0), color: glow.shape(family.color), x, y } : { text: label.text, color: colors.strong, x, y })
      }
    } else if (regionAlpha(view.scale) > 0) {
      // Over the tiles once the dots are more tiles than dots; between the two the names stand down.
      const families = new Map(model.groups.map(group => [group.key, group]))
      words = regionsOf(tilePoints && morphed > 0.5 ? tilePoints : dots)
        .flatMap(region => {
          const family = families.get(region.group)
          const [x, y] = [region.x * unit + view.dx, region.y * unit + view.dy - 20]
          if (!family || hidden.has(family.key) || x < left || y < top || x > right || y > bottom) return []
          return [{ text: family.name.toLocaleUpperCase(), count: num(family.count, 0), color: glow.shape(family.color), x, y, size: family.count }]
        })
        .sort((a, b) => b.size - a.size)
    }
    // What floats over the map, as boxes in the canvas's pixels: the search, the tools, the legend and the cards
    // under it; the player and Grow's card (where they are laid out); the corner and the picks' actions (where they
    // are going: both slide); Grow's handle and its tag; a side sheet. No word or name is written under any of them.
    // Read only when there are words or names to write.
    const floating: [number, number, number, number][] = coverRight > 0 ? [[size.width - coverRight, 0, coverRight, size.height]] : []
    if (words.length > 0 || onScreen <= NAMES_ON_SCREEN) {
      const origin = canvas.getBoundingClientRect()
      const [bar, ...under] = [...(topRef.current?.children ?? [])]
      for (const piece of [...(bar?.children ?? []), ...under, playerRef.current]) {
        const box = piece?.getBoundingClientRect()
        if (box && box.width > 2 && box.height > 2) floating.push([box.left - origin.left, box.top - origin.top, box.width, box.height])
      }
      const [corner, card] = [cornerRef.current, growCardRef.current]
      if (corner) floating.push([corner.offsetLeft, size.height - footing.current.corner - corner.offsetHeight, corner.offsetWidth, corner.offsetHeight])
      if (card) floating.push([card.offsetLeft, card.offsetTop, card.offsetWidth, card.offsetHeight])
      if (clusterBox.current) floating.push(clusterBox.current)
      // The handle's percent tag sits just above it, on whatever the map writes there.
      const [start, end] = [placeOfEnd(grow?.from ?? null), placeOfEnd(grow?.to ?? null)]
      if (grow && !grow.choosing && start && end) {
        const [x, y] = vinePoint(toScreen(view, mapSize, start[0], start[1]), toScreen(view, mapSize, end[0], end[1]), grow.handle)
        floating.push([x - 22, y - 27, 44, 38])
      }
    }
    const regional = !layoutShown || layoutShown.labels.length === 0
    const written = drawWords(context, words, wordsIn * (regional ? regionAlpha(view.scale) * Math.abs(2 * morphed - 1) : 0.9), lettering, regional, floating)
    // Few enough on screen (a small map, a narrow search, or zoomed in) to read: each sound's name beside it in its
    // family's colour, the sound file's own name drawn as canvas text, never markup. Only names with room are written,
    // so a tight cluster stays legible (its sounds are named on hover and in the list above). Each name written is kept
    // with its sound: a click on the name reaches the sound, as one on its point does.
    named.current = []
    if (onScreen <= NAMES_ON_SCREEN) {
      const names = labelled.flatMap(([x, y, index, color]) => {
        const text = mapLabel(model.sounds[points[index].id]?.name ?? '')
        return text ? [{ text, color: glow.shape(color), x, y, rank: nameRank(index), index }] : []
      })
      named.current = drawNames(context, names, reach(morphed) + 4, lettering, [...written, ...floating], size.width - coverRight).map(({ box, at }) => ({ box, index: names[at].index }))
    }
    // Then the layout's axis words inside the map's edges, kept in view.
    if (layoutShown && wordsIn > 0) {
      context.textBaseline = 'middle'
      context.lineJoin = 'round'
      context.strokeStyle = colors.ground
      context.lineWidth = 3
      const family = lettering.family
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
    const rings = (ids: ReadonlySet<string> | undefined, extra: number, later?: ReadonlyMap<string, number>): Path2D | null => {
      if (!ids || ids.size === 0) return null
      const path = new Path2D()
      for (const id of ids) {
        const index = indexOf.get(id)
        if (index === undefined || mask[index] === 2 || later?.has(id)) continue
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
    const pop = popped.current && now - popped.current.at < POP_MS ? popped.current : null
    const inPicks = rings(picked, 1.4, pop?.step)
    if (inPicks) {
      context.strokeStyle = colors.strong
      context.lineWidth = 1.6
      context.stroke(inPicks)
    }
    if (pop && picked) {
      // The rings still arriving: each wave step one path, wide and faint at first, settling onto its sounds.
      const settled = Array.from({ length: POP_STEPS }, (_, step) => clamp01((now - pop.at - (step * (POP_MS - POP_EACH)) / (POP_STEPS - 1)) / POP_EACH))
      const waves = settled.map(() => new Path2D())
      for (const [id, step] of pop.step) {
        const index = indexOf.get(id)
        if (index === undefined || mask[index] === 2 || settled[step] <= 0 || !picked.has(id)) continue
        const [x, y, blend] = placeOf(index)
        if (x < left || y < top || x > right || y > bottom) continue
        const r = (reach(blend) + 1.4) * (1 + 0.9 * (1 - settled[step]) ** 2)
        waves[step].moveTo(x + r, y)
        waves[step].arc(x, y, r, 0, Math.PI * 2)
      }
      context.strokeStyle = colors.strong
      context.lineWidth = 1.6
      waves.forEach((wave, step) => {
        context.globalAlpha = settled[step]
        context.stroke(wave)
      })
      context.globalAlpha = 1
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
      // Its constellation: a thin line in its colour to each of the sounds most like it, the nearest brightest, and a
      // fine ring on each. Zoomed out on a crowded map they sit under it; zoomed in they spread.
      context.strokeStyle = glow.shape(color)
      context.lineWidth = 1
      constellation.forEach((other, rank) => {
        if (other >= count || mask[other] !== 1) return
        const [toX, toY, toBlend] = placeOf(other)
        context.globalAlpha = 0.7 - rank * 0.09
        context.beginPath()
        context.moveTo(x, y)
        context.lineTo(toX, toY)
        context.moveTo(toX + reach(toBlend) + 2, toY)
        context.arc(toX, toY, reach(toBlend) + 2, 0, Math.PI * 2)
        context.stroke()
      })
      context.globalAlpha = 1
      const path = new Path2D()
      shape(path, x, y, blend, 1.45, glyphAt(hovered))
      context.save()
      context.shadowColor = color
      context.shadowBlur = 14
      context.fillStyle = glow.shape(color)
      context.fill(path)
      context.restore()
      // And a fine ring in the text colour, so one sound stands out of a dense cluster of its own colour.
      const ring = new Path2D()
      shape(ring, x, y, blend, 2.3, glyphAt(hovered), true)
      context.strokeStyle = colors.strong
      context.lineWidth = 1.25
      context.stroke(ring)
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
    overlayFollow.current?.()
  }, [model, arranged, arrangementIndex, indexOf, view, size, mask, foundIds, selected, mapSize, painted, clock, marked, picked, tiled, layout, morph, spread, flying, still, hoverAt, constellation, t, topInset, coverRight, grow?.from, grow?.to, grow?.handle, grow?.choosing])

  const at = (event: { clientX: number; clientY: number }): [number, number] => {
    const rect = canvasRef.current?.getBoundingClientRect()
    return rect ? [event.clientX - rect.left, event.clientY - rect.top] : [0, 0]
  }
  // How near the pointer must be to a sound to reach it: a dot's few pixels, or most of a tile.
  const reachPx = (base: number): number => (tiles && layout && view ? Math.max(base, ((mapSize * view.scale) / layout.side) * 0.62) : base)
  // The sound at a canvas point: its point, else the name the map wrote beside it (as last drawn), among the sounds the
  // search and the legend keep; -1 for none.
  const soundAt = (at: MapViewport, x: number, y: number): number => {
    const index = pick(live(), at, mapSize, x, y, reachPx(10), visible)
    if (index >= 0) return index
    const name = named.current.find(({ box: [left, top, width, height], index }) => x >= left && x <= left + width && y >= top - 2 && y <= top + height + 2 && visible(index))
    return name ? name.index : -1
  }

  // A press with the left button starts a box (Shift, Command or Ctrl held: one more box, added to the picks); with the
  // middle button, with Space held or the hand tool on, a pan. A left press that doesn't move picks the sound under it:
  // Command or Ctrl picks or unpicks it, Shift adds it to the picks, and a plain click picks and plays it.
  const press = (event: PointerEvent<HTMLCanvasElement>) => {
    if (!view || (event.button !== 0 && event.button !== 1)) return
    // The middle button scrolls the page on some systems (autoscroll): not over the map.
    if (event.button === 1) event.preventDefault()
    event.currentTarget.setPointerCapture?.(event.pointerId)
    const [x, y] = at(event)
    const boxing = event.button === 0 && gesturesOf(mapState(space.current)).drag === 'pick'
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
      const inside = pointsInBox(live(), view, mapSize, [held.x, held.y, x, y], visible).map(index => model.points[index].id)
      onPicked?.(inside, held.add ? 'add' : 'replace')
      return
    }
    if (held.moved || held.button !== 0) return
    const index = soundAt(view, x, y)
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
      // Naming Grow's ends: the sound is named and heard, and let go of at once, so no card opens at the map's foot,
      // over the very sounds the next click is meant for.
      const end = endOf(id)
      if (end) grow.name(end)
      onPick(id)
      onSelect(null)
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

  const onKey = (event: KeyboardEvent<HTMLDivElement>) => {
    // Escape stops Grow naming its sounds, else lets go of the picks; with neither, it goes on to the map's own
    // handler, which puts Grow's vine away.
    const escape = event.key === 'Escape' && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey ? gesturesOf(mapState(space.current)).escape : undefined
    if (escape === 'cancel') {
      event.preventDefault()
      grow?.choose(false)
      return
    }
    if (escape === 'clear') {
      event.preventDefault()
      onPicked?.([], 'replace')
      setSaid(t('dock.cleared'))
      return
    }
    // Enter, or Space (a checkbox's own key), does on the sound the arrow keys reached what a click does on it: while
    // Grow names its sounds it names this one, else it adds it to the picks or lets it go (a click with Command or
    // Ctrl), so Make is a keyboard's too.
    if ((event.key === 'Enter' || event.key === ' ') && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey && selected !== null && indexOf.has(selected)) {
      event.preventDefault()
      if (grow?.choosing) {
        const end = endOf(selected)
        if (end) grow.name(end)
      } else if (onPicked) {
        const count = pickedIds.length + (pickedIds.includes(selected) ? -1 : 1)
        onPicked([selected], 'toggle')
        setSaid(count > 0 ? t('dock.picked', count) : t('dock.cleared'))
      }
      return
    }
    const key = mapKey(event)
    if (!view || !key) return
    event.preventDefault()
    const from = selected === null ? undefined : indexOf.get(selected)
    const picked = from !== undefined && visible(from) ? from : undefined
    const middle: [number, number] = [size.width / 2, size.height / 2]
    const hit = live()
    if ('fit' in key) {
      moved.current = false
      setView(fit())
    } else if ('zoom' in key) {
      // Around the sound picked, so it stays where it is; else around the middle.
      const [x, y] = picked === undefined ? middle : toScreen(view, mapSize, hit.points[picked].x, hit.points[picked].y)
      moved.current = true
      setView(zoomAt(view, key.zoom, x, y))
    } else {
      const next = moveWith(hit, view, mapSize, picked, key, middle, visible)
      if (next < 0) return
      const point = hit.points[next]
      const [x, y] = toScreen(view, mapSize, point.x, point.y)
      setView(keepInView(view, x, y, size.width, size.height))
      // The keyboard stop now stands for this sound: its name and state are what a screen reader says, once.
      onSelect(point.id)
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
  // The picks' actions show while there are picks, but not while Grow names its sounds: nothing covers the sounds to
  // choose between.
  const picksShown = hasPicks && !grow?.choosing
  // While a box is drawn: how many sounds letting go would leave picked (a Shift-drag's box adds to the picks).
  const inBox = box && view ? pointsInBox(live(), view, mapSize, box, visible) : []
  const boxed = drag.current?.add && picked?.size ? new Set([...picked, ...inBox.map(index => model.points[index].id)]).size : inBox.length
  // The first-open steps still to do here: hearing a sound (with hover-to-play on) and drawing a box are done the
  // moment the map sees them. Not while Grow's prompt has the corner.
  const firstSteps = first && !grow?.choosing ? firstLeft(first.kept, { hover: trail, picks: Boolean(onPicked) }) : []
  const [heard, drew] = [hover !== null && trail, inBox.length > 0]
  useEffect(() => {
    if (heard) first?.onDone(['hover'])
  }, [heard]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (drew) first?.onDone(['box'])
  }, [drew]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    const element = cluster.current
    if (!element) return
    const observer = new ResizeObserver(entries => {
      const rect = entries[0]?.contentRect
      if (rect) setClusterSize([Math.ceil(rect.width) + 12, Math.ceil(rect.height) + 12])
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [picksShown])
  // The player's own height: it grows (a pad grid under Use on pad…), and what sits above it (the corner, the picks'
  // actions, Grow's card) keeps above all of it, never over its name, Play or ×. Not while Grow names its sounds: those
  // clicks open no card, and one already open steps aside, so nothing covers the sounds to choose between.
  const [playerHeight, setPlayerHeight] = useState(0)
  const playerShown = Boolean(selected && model.sounds[selected]) && !grow?.choosing
  useEffect(() => {
    const element = playerRef.current
    if (!playerShown || !element) return setPlayerHeight(0)
    const observer = new ResizeObserver(() => setPlayerHeight(element.offsetHeight))
    observer.observe(element)
    return () => observer.disconnect()
  }, [playerShown])
  // What docks at the map's foot: the player, and the corner above it (at least the player's usual 110 px while it
  // measures). On a short map (less than `ROOM_LEAST` between the top column and the player) the corner folds to its
  // first line, and steps aside while the player is open: the map keeps some of its sounds in view whatever is open.
  // With a find's list open, the corner folds the same way (its first-open steps give way) when the list and the
  // corner as they'd stand would leave the map less than its free room (`FREE_LEAST`, or a third of its height). Both
  // decisions read estimates (`listWants`, `CORNER_WITH_STEPS`), never what they change, so neither flips back and forth.
  const cornerBottom = playerShown ? Math.max(110, playerHeight) + 18 : 10
  const listOpen = matches.length > 0 && closedOf !== matches && !veiled
  const rowsShown = Math.min(matches.length, allOf === matches ? matches.length : MATCHES_LISTED)
  const listWants = LIST_HEAD + rowsShown * LIST_ROW + (matches.length > MATCHES_LISTED ? LIST_MORE : 0)
  const freeLeast = Math.max(FREE_LEAST, size.height / 4)
  const crowded = listOpen && size.height > 0 && size.height - topInset - cornerBottom - (firstSteps.length > 0 ? CORNER_WITH_STEPS : CORNER_ROOM) - listWants < freeLeast
  const short = size.height > 0 && size.height - topInset - cornerBottom < ROOM_LEAST
  const compact = short || crowded
  const cornerShown = !(short && playerShown)
  const cornerRoom = !cornerShown ? 0 : compact && !grow?.choosing ? CORNER_FOLDED : cornerHeight
  const foot = cornerBottom + (cornerRoom > 0 ? cornerRoom + 8 : 0)
  footing.current = { corner: cornerBottom, foot }
  useEffect(() => {
    const element = cornerRef.current
    if (!element) return
    const observer = new ResizeObserver(() => setCornerHeight(element.offsetHeight))
    observer.observe(element)
    return () => observer.disconnect()
  }, [hasPoints, cornerShown])
  // Which floating piece holds the keyboard's focus (the picks' actions, the player, Grow's card): when it goes while it
  // does (its ×, Escape, Clear, Add to palette), the focus goes to the map's keyboard stop rather than falling to the
  // page. Kept by the map's focus events, read here.
  const focusIn = useRef<'grow' | 'picks' | 'player' | null>(null)
  const growShown = Boolean(grow?.from && grow.to)
  useEffect(() => {
    const gone = focusIn.current === 'picks' ? !picksShown : focusIn.current === 'player' ? !playerShown : focusIn.current === 'grow' ? !growShown : false
    if (!gone) return
    focusIn.current = null
    keysRef.current?.focus({ preventScroll: true })
  }, [picksShown, playerShown, growShown])

  // No sounds yet: what builds the map, in the middle of the pane it will fill.
  if (!model.points.length) {
    return (
      <div style={{ ...S.muted, ...TYPE.body, flex: 1, height: '100%', minHeight: 240, display: 'flex', alignItems: 'center', justifyContent: 'center', padding: 24, textAlign: 'center' }}>
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
  // The sound the map's keyboard stop stands for: the one the arrows (or a click) reached, while the search and the
  // legend keep it; its name and kind, where it sits on screen, and the stop's role (a checkbox for picking it; a
  // button while Grow names its ends, or on a map that picks nothing).
  const reachedId = selected !== null && chosenIndex !== undefined && visible(chosenIndex) ? selected : null
  const reachedSound = reachedId ? model.sounds[reachedId] : undefined
  const reachedName = reachedId ? [clean(reachedSound?.name ?? reachedId, 80), clean(reachedSound?.class, 16)].filter(Boolean).join(', ') : ''
  const reachedPoint = reachedId && chosenIndex !== undefined ? live().points[chosenIndex] : undefined
  const reachedAt = reachedPoint && view ? toScreen(view, mapSize, reachedPoint.x, reachedPoint.y) : null
  const keysRole = reachedId && onPicked && !grow?.choosing ? 'checkbox' : 'button'
  const hovered = hover ? model.points[hover.index] : undefined
  const hoveredSound = hovered ? model.sounds[hovered.id] : undefined
  // The sound that plays, when it's one of this map's: the overlay pulses on it, wherever the map last drew it (a
  // sound's place moves with a pan, a zoom and a layout's move).
  const playingIndex = playing === null ? undefined : indexOf.get(playing)
  const playingGroup = playingIndex === undefined ? undefined : model.points[playingIndex].group
  const playingSound =
    playing === null || playingGroup === undefined
      ? null
      : { id: playing, color: model.groups.find(group => group.key === playingGroup)?.color ?? T.accent, glyph: glyphOf(playingGroup), duration: model.sounds[playing]?.duration ?? 0 }
  const placeOnScreen = (id: string): [number, number] | null => {
    const index = indexOf.get(id)
    const [last, at] = [drawn.current, viewRef.current]
    if (index === undefined || !last || !at || last.places.length < 2 * index + 2) return null
    const unit = mapSize * at.scale
    return [last.places[2 * index] * unit + at.dx, last.places[2 * index + 1] * unit + at.dy]
  }
  const countWords =
    shown === model.points.length - leavingCount ? t('map.count', num(shown, 0)) : t('map.countOf', num(shown, 0), num(model.points.length - leavingCount, 0))
  const hiddenCount = (map?.hidden ?? 0) + leavingCount
  // The picks' actions float beside the picks, never over them; with the picks out of view, at the map's foot, above
  // the corner and the player. They, and Grow's card, keep below the whole top column (a find's list in it), never
  // just the legend.
  const columnTop = Math.max(topInset, columnBottom)
  const clusterAt = floatBeside(picksBox, clusterSize[0], clusterSize[1], size.width - coverRight, size.height, columnTop, foot)
  clusterBox.current = picksShown ? [clusterAt.left, clusterAt.top, clusterSize[0], clusterSize[1]] : null
  // A find's list keeps to the room between the legend and the foot, less the picks' actions' (docked at the foot when
  // there's no room beside the picks): its rows scroll inside it. It folds to its head, whose "Show" opens it anyway,
  // when there's too little room for any row, or when its rows would leave the map less than its free room. A find's
  // frame reads the list as drawn, folded or not, so the found sounds land in the room left.
  const listRoom = size.height - topInset - foot - (picksShown ? clusterSize[1] : 0)
  fitRoom.current = { top: columnTop, bottom: foot + (picksShown ? clusterSize[1] : 0) }
  const listFolded = size.height > 0 && allOf !== matches && (listRoom < LIST_LEAST || listRoom - listWants < freeLeast)

  return (
    <div
      onBlur={event => {
        // The focus left the map altogether: no piece of it holds it any more.
        if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget as Node)) focusIn.current = null
      }}
      onFocus={event => {
        const target = event.target as Node
        focusIn.current = cluster.current?.contains(target) ? 'picks' : playerRef.current?.contains(target) ? 'player' : growCardRef.current?.contains(target) ? 'grow' : null
      }}
      onKeyDown={event => {
        // Escape leaves Grow's naming wherever the keyboard is in the map (on the Grow tool just pressed, say), else
        // puts Grow's vine away (from its handle, a bud, its card or the map): only once whatever is nearer the focus
        // and takes Escape (a menu, the player, the canvas's picks) has had it. Past the map, Desktop takes it and stops
        // the chat's turn.
        if (event.key !== 'Escape' || event.defaultPrevented || event.shiftKey || event.altKey || event.ctrlKey || event.metaKey || !grow) return
        if (grow.choosing) {
          event.preventDefault()
          grow.choose(false)
        } else if (grow.from && grow.to) {
          event.preventDefault()
          grow.close()
        }
      }}
      style={{ position: 'relative', height: '100%', minHeight: 240, overflow: 'hidden', color: T.text }}
    >
      {/* What floats over the map's top: the search and the tools, the legend, a search's or a find's sounds by name, and
          the quiet line about sounds that can't play. Only the pieces take the pointer; the map shows through the gaps.
          It comes before the canvas, so the keyboard meets the search and the tools first, as it always has; z-index
          keeps it above the canvas, drawn after it. */}
      <div ref={topRef} style={{ position: 'absolute', top: 8, left: 8, right: 8, zIndex: 1, display: 'flex', flexDirection: 'column', alignItems: 'flex-start', gap: 6, pointerEvents: 'none' }}>
        <div style={{ ...S.row, alignSelf: 'stretch', gap: 8, flexWrap: 'wrap', alignItems: 'flex-start' }}>
          {/* The search alone: how many sounds it keeps is the corner's count ("12 of 900 sounds"), the map's one count. */}
          <div style={{ ...glass, borderRadius: 18, position: 'relative', display: 'flex', alignItems: 'center', flex: '1 1 180px', maxWidth: 420, minWidth: 150, height: 36, pointerEvents: 'auto' }}>
            <input
              aria-label={t('map.search')}
              className={FOCUSABLE}
              onChange={event => setQuery(event.target.value)}
              placeholder={t('map.searchHint')}
              style={{ flex: 1, minWidth: 0, height: '100%', font: 'inherit', ...TYPE.body, padding: '0 12px', borderRadius: 18, border: 'none', background: 'transparent', color: T.text, outlineOffset: -2, textOverflow: 'ellipsis' }}
              value={query}
            />
          </div>
          <span style={{ flex: 1 }} />
          <ToolGroup label={t('map.tools')}>
            {onPicked && <ToolButton icon="pick" label={t('map.pickTool')} onClick={() => setHandTool(false)} pressed={!handTool} title={t('map.pickToolTitle')} />}
            {/* The tool in hand shows its name beside its icon. */}
            {onPicked && !handTool && !grow?.choosing && <ToolName>{t('map.pickTool')}</ToolName>}
            {onPicked && <ToolButton icon="hand" label={t('map.hand')} onClick={() => setHandTool(true)} pressed={handTool} title={t('map.handTitle')} />}
            {onPicked && handTool && !grow?.choosing && <ToolName>{t('map.hand')}</ToolName>}
            {grow && <ToolButton icon="grow" label={t('grow.tool')} onClick={() => grow.choose(!grow.choosing)} pressed={grow.choosing !== null} title={t('grow.toolTitle')} />}
            {grow?.choosing && <ToolName>{t('grow.tool')}</ToolName>}
            {onPicked && <Rule />}
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
        {/* The legend and the cards under it keep left of a side sheet (Make beside the map), which starts below the bar. */}
        <div aria-label={t('map.legend')} ref={legendRef} role="group" style={{ ...glass, borderRadius: 14, ...S.row, flexWrap: 'wrap', gap: 4, padding: 4, pointerEvents: 'auto', maxWidth: `calc(100% - ${coverRight}px)` }}>
          {/* Serve names every class it knows, most of them empty in a small library: the legend lists those with sounds.
              Each chip is a 24 px target. */}
          {model.groups.filter(group => group.count > 0).map(group => {
            const off = hidden.has(group.key)
            return (
              <Tip key={group.key} label={off ? t('map.show', group.name) : t('map.hide', group.name)}>
                <button
                  aria-pressed={!off}
                  className={FOCUSABLE}
                  onClick={() =>
                    setHidden(current => {
                      const next = new Set(current)
                      if (next.has(group.key)) next.delete(group.key)
                      else next.add(group.key)
                      return next
                    })
                  }
                  type="button"
                  style={{ ...S.row, gap: 5, font: 'inherit', ...TYPE.caption, minHeight: 24, padding: '0 8px 0 6px', borderRadius: 12, border: 'none', background: off ? 'transparent' : `color-mix(in srgb, ${group.color} 12%, transparent)`, color: off ? T.faint : T.secondary, cursor: 'pointer', opacity: off ? 0.55 : 1, transition: 'background-color 140ms, opacity 140ms' }}
                >
                  <svg aria-hidden height="9" style={{ flex: 'none', opacity: off ? 0.35 : 1 }} viewBox="-1.5 -1.5 3 3" width="9">
                    <GlyphShape color={group.color} glyph={glyphOf(group.key)} />
                  </svg>
                  {group.name} <span style={{ color: T.faint }}>{group.count}</span>
                </button>
              </Tip>
            )
          })}
        </div>
        {listOpen &&
          (() => {
            // What every listed row would repeat is said once, in the head: their kind when they're all one, the find's
            // reasons they all share, and their closeness when it's all within two points (CC-23's words, "closeness N,
            // relative to this map", never a percentage). A row then says only what's its own.
            const listed = matches.slice(0, allOf === matches ? matches.length : MATCHES_LISTED)
            const kinds = new Set(listed.map(id => model.sounds[id]?.class).filter(Boolean))
            const oneKind = kinds.size === 1 ? clean([...kinds][0], 16) : ''
            const whyOf = (id: string): string[] => (searched ? [] : (found?.matches?.[id]?.why ?? []))
            const sharedWhy = listed.length > 1 ? whyOf(listed[0]).filter(why => listed.every(id => whyOf(id).includes(why))) : []
            const near = nearName !== '' && !searched
            const closeness = listed.map(id => found?.matches?.[id]?.closeness).filter((value): value is number => typeof value === 'number')
            const [low, high] = closeness.length ? [Math.min(...closeness), Math.max(...closeness)] : [0, 0]
            const flat = near && closeness.length === listed.length && closeness.length > 0 && high - low <= 2
            // The kind alone (the head speaks for every row), and a range that never breaks between its two ends.
            const facts = [oneKind, ...sharedWhy.map(why => clean(why, 60)), near && closeness.length ? (flat ? t('map.closeness', low === high ? String(low) : `${low}\u2060–\u2060${high}`) : t('map.closenessShown')) : '']
              .filter(Boolean)
              .join(' · ')
            return (
              // At most the room between the legend and the foot (the corner, the player, the picks' actions docked
              // there): its rows scroll inside it rather than running under them, and with no room for a row it folds
              // to its head (`listFolded`), so the map below still shows.
              <div aria-label={searched ? t('map.matches') : nearName || t('map.found')} ref={matchesRef} role="group" style={{ ...glass, display: 'flex', flexDirection: 'column', gap: 2, padding: 6, width: `min(340px, calc(100% - ${coverRight}px))`, maxHeight: size.height > 0 && !listFolded ? Math.max(120, listRoom) : undefined, pointerEvents: 'auto' }}>
                <div style={{ ...S.row, alignItems: 'flex-start', gap: 6, padding: '0 0 4px 4px' }}>
                  <span style={{ display: 'flex', flexDirection: 'column', gap: 2, flex: 1, minWidth: 0 }}>
                    {/* CC-23: a near search's neighbours, under the seeds' names from this map (the seeds themselves aren't listed). */}
                    <span style={{ ...TYPE.small, ...S.ellipsis, color: T.text }}>{searched ? t('map.matches') : nearName || t('map.found')}</span>
                    {facts && (
                      <Tip label={near ? t('map.closenessTitle') : undefined}>
                        <span style={{ ...TYPE.caption, color: T.faint, overflowWrap: 'anywhere' }}>{facts}</span>
                      </Tip>
                    )}
                  </span>
                  {onPicked && (
                    // Native's `a`: every sound the search keeps, or every sound Kumi found (a "More like these" among them).
                    <Button onClick={() => onPicked(matches, 'replace')} tip={searched !== null ? t('dock.pickAllTitle') : t('dock.pickFoundTitle')}>
                      {t('dock.pickAll', matches.length)}
                    </Button>
                  )}
                  {/* The sounds listed may lie out of view (a search typed while zoomed in): this brings them all into
                      the room the map shows, as a find's frame does, only when asked. */}
                  <Button kind="quiet" onClick={() => frameSounds(new Set(matches))} tip={t('map.frameTitle')}>
                    {t('map.frame')}
                  </Button>
                  <Button kind="quiet" label={t('map.closeList')} onClick={() => setClosedOf(matches)}>
                    ×
                  </Button>
                </div>
                {!listFolded && (
                  <div ref={matchesList} style={{ display: 'flex', flexDirection: 'column', gap: 1, minHeight: 0, overflowY: 'auto' }}>
                    {listed.map(id => {
                      const sound = model.sounds[id]
                      const indexed = indexedValues(sound, t)
                      const own = whyOf(id).filter(why => !sharedWhy.includes(why))
                      const value = found?.matches?.[id]?.closeness
                      return (
                        <button
                          aria-pressed={selected === id}
                          className={FOCUSABLE}
                          key={id}
                          onClick={() => onPick(id)}
                          type="button"
                          style={{ display: 'flex', flexDirection: 'column', alignItems: 'stretch', gap: 0, font: 'inherit', ...TYPE.small, minHeight: 24, padding: '4px 6px', borderRadius: 4, border: 'none', textAlign: 'left', cursor: 'pointer', color: T.text, background: selected === id ? T.card : 'transparent' }}
                        >
                          <span style={{ ...S.row, gap: 8 }}>
                            <span style={{ ...S.ellipsis, flex: 1 }}>{clean(sound?.name ?? id, 80)}</span>
                            {indexed && (
                              <Tip label={t('map.indexedTitle')}>
                                <span style={{ color: T.faint, whiteSpace: 'nowrap' }}>{t('map.indexed', indexed)}</span>
                              </Tip>
                            )}
                            {near && !flat && typeof value === 'number' && <span style={{ color: T.secondary, fontVariantNumeric: 'tabular-nums' }}>{value}</span>}
                            {!oneKind && sound?.class && <span style={{ color: T.faint }}>{clean(sound.class, 16)}</span>}
                          </span>
                          {own.length > 0 && <span style={{ ...TYPE.caption, ...S.ellipsis, color: T.faint }}>{own.map(why => clean(why, 60)).join(' · ')}</span>}
                        </button>
                      )
                    })}
                  </div>
                )}
                {(listFolded || matches.length > MATCHES_LISTED) && (
                  <span style={{ padding: '2px 0 0 2px' }}>
                    <Button kind="quiet" onClick={() => setAllOf(allOf === matches ? null : matches)}>
                      {listFolded ? t('map.showList', matches.length) : allOf === matches ? t('map.showFewer') : t('map.showMore', matches.length - MATCHES_LISTED)}
                    </Button>
                  </span>
                )}
              </div>
            )
          })()}
        {hiddenCount > 0 && (
          // One quiet line for every sound that couldn't play here, never a notice per sound.
          <div role="status" style={{ ...glass, borderRadius: 14, ...S.row, flexWrap: 'wrap', gap: 6, padding: '3px 6px 3px 10px', pointerEvents: 'auto', maxWidth: `calc(100% - ${coverRight}px)` }}>
            <span style={{ ...S.faint, ...TYPE.caption }}>{t('map.hidden', hiddenCount)}</span>
            {hiddenActions}
          </div>
        )}
      </div>
      {/* The map, for the keyboard and a screen reader: a group named for it, holding one keyboard stop for the sound
          reached (`keys`). The canvas only draws and takes the pointer. */}
      <div aria-label={t('map.canvas')} aria-roledescription={t('map.roledescription')} ref={boxRef} role="group" style={{ position: 'absolute', inset: 0 }}>
        <canvas
          ref={canvasRef}
          aria-hidden
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
            const index = pick(live(), view, mapSize, x, y, reachPx(8), visible)
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
          // A press keeps the keyboard on the map: it moves the focus to the map's keyboard stop (a canvas takes none),
          // so the arrows go on from the sound just clicked. The middle button's own press would also start the page's
          // autoscroll on some systems.
          onMouseDown={event => {
            if (event.button !== 0 && event.button !== 1) return
            event.preventDefault()
            keysRef.current?.focus({ preventScroll: true })
          }}
          onDoubleClick={() => {
            moved.current = false
            setView(fit())
          }}
          style={{
            position: 'absolute',
            inset: 0,
            width: '100%',
            height: '100%',
            cursor: grabbing ? 'grabbing' : spaceHeld || handTool || !onPicked ? 'grab' : box ? 'crosshair' : hover ? 'pointer' : 'default',
            touchAction: 'none'
          }}
        />
        <AtlasOverlay audio={audio} follow={overlayFollow} height={size.height} kin={kinRings} place={placeOnScreen} playing={playingSound} still={still} target={canvasRef} trail={trail} width={size.width} />
        {box && Math.abs(box[2] - box[0]) + Math.abs(box[3] - box[1]) > 3 && (
          // The box being drawn, with how many sounds it holds now (with the picks it adds to, on a Shift-drag).
          <PickBox box={box} count={t('dock.picked', boxed)} height={size.height} width={size.width} />
        )}
        <span id={hintId} style={S.hidden}>
          {t('map.keys')}
        </span>
        {/* The map's one keyboard stop, over the sound the arrows reached (the whole map before any is), never in the
            pointer's way. A sound is picked or not, so it's a checkbox named for the sound; while Grow names its ends
            Enter names the sound instead, so it's a button. Standard roles: Hermes Desktop leaves Enter and Space to
            them rather than moving the focus to its chat. (Typed letters still go to the chat: the host's rule.) */}
        <div
          aria-checked={keysRole === 'checkbox' ? Boolean(reachedId && picked?.has(reachedId)) : undefined}
          aria-describedby={reachedId ? undefined : hintId}
          aria-label={reachedName || t('map.canvas')}
          className={FOCUSABLE}
          onKeyDown={onKey}
          ref={keysRef}
          role={keysRole}
          style={reachedAt ? { position: 'absolute', left: reachedAt[0] - 14, top: reachedAt[1] - 14, width: 28, height: 28, borderRadius: '50%', pointerEvents: 'none' } : { position: 'absolute', inset: 0, outlineOffset: -3, pointerEvents: 'none' }}
          tabIndex={0}
        />
        <span aria-live="polite" style={S.hidden}>
          {said}
        </span>
        {hovered && hoveredSound && hover && (
          <HoverPill
            at={[hover.x, hover.y]}
            audio={audio}
            color={model.groups.find(group => group.key === hovered.group)?.color ?? T.accent}
            glyph={glyphOf(hovered.group)}
            id={hovered.id}
            key={hovered.id}
            playing={playing === hovered.id}
            sound={hoveredSound}
            within={[size.width - coverRight, size.height]}
          />
        )}
        {grow && (grow.from || grow.to) && (
          <GrowLayer
            actions={growActions}
            bottom={foot}
            card={growCardRef}
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
            top={columnTop}
            width={size.width - coverRight}
          />
        )}
      </div>
      {picksShown && (
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
          <span style={{ ...S.row, gap: 5, ...TYPE.small, fontWeight: 600, color: T.text }}>
            <Icon name="pick" size={12} />
            {t('dock.picked', pickedIds.length)}
          </span>
          {pickedActions?.(pickedIds)}
        </div>
      )}
      {selected && chosen && playerShown && (
        // The sound picked last, in the player at the map's foot (left of a side sheet, when one is open).
        <div style={{ position: 'absolute', left: 10, right: 10 + coverRight, bottom: 10, zIndex: 2, display: 'flex', justifyContent: 'center', pointerEvents: 'none', transition: still ? 'none' : 'right 260ms cubic-bezier(.2,.8,.2,1)' }}>
          <div ref={playerRef} style={{ width: 'min(640px, 100%)' }}>
            <Player
              actions={actions?.(selected, chosen)}
              audio={audio ?? (() => Promise.reject(new Error('no audio')))}
              color={chosenColor}
              id={selected}
              key={selected}
              onClose={() => {
                // The player is the stop control: closing it while its sound plays stops that sound.
                if (playing === selected) onStop?.()
                onSelect(null)
              }}
              onPlay={() => onPick(selected)}
              onStop={() => onStop?.()}
              playing={playing === selected}
              sound={chosen}
            />
          </div>
        </div>
      )}
      {/* The corner: what the map holds, its layout as a button, the zoom, and what can be done now. Grow's prompt
          stands on its hint line while Grow names its sounds: which one this click names, and a way out. Last here,
          so Tab from the map still goes to the picks' actions and the player first, as the keys' hint says. On a short
          map it folds to its first line (and the prompt), and steps aside while the player is open. */}
      {cornerShown && (
        <Corner
          bottom={cornerBottom}
          box={cornerRef}
          compact={compact}
          cover={coverRight}
          still={still}
          first={first && firstSteps.length > 0 && !compact ? <FirstOpen onSkip={() => first.onDone(FIRST_STEPS)} steps={firstSteps} /> : undefined}
          families={model.groups.filter(group => group.count > 0).length}
          hint={hintOf(mapState(spaceHeld))}
          layout={
            layoutChoices.length > 0 ? (
              // The menu opens upward from the corner, which sits higher while the player is under it.
              <LayoutMenu choices={layoutChoices} current={arrangementIndex >= 0 ? arrangementId : ''} label={t('arrange.label')} maxHeight={Math.max(120, size.height - (playerShown ? 260 : 150))} onChoose={chooseArrangement} words={name => t('corner.layout', name)} />
            ) : undefined
          }
          prompt={
            grow?.choosing ? (
              <div className="kumi-pop" role="status" style={{ ...S.row, gap: 8, flexWrap: 'wrap' }}>
                <Icon name="grow" size={13} />
                <span style={{ ...TYPE.small, color: T.text }}>{grow.choosing === 'from' ? t('grow.from') : t('grow.to', clean(grow.from?.name ?? '', 60))}</span>
                <Button kind="quiet" onClick={() => grow.choose(false)}>
                  {t('grow.cancel')}
                </Button>
              </div>
            ) : undefined
          }
          sounds={countWords}
          // The zoom is the line's least needed word: a narrow map drops it rather than give it a line of its own.
          zoom={size.width - coverRight >= CORNER_ZOOM_ROOM ? (view?.scale ?? 1) : undefined}
        />
      )}
    </div>
  )
}

/** A group's glyph as SVG, on the unit circle (the legend's chips: the shape the map draws that group's sounds in). */
function GlyphShape({ glyph, color }: { glyph: number; color: string }) {
  const shape = GLYPH_SHAPES[glyph]
  return shape ? <polygon fill={color} points={shape.map(([x, y]) => `${x},${y}`).join(' ')} /> : <circle cx="0" cy="0" fill={color} r="1" />
}
