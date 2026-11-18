// The palette, a drawer under the sound map: the sounds the producer gathered to work with. Folded, it's one row of
// chips with their waveforms and a "Show all" pill; open, a roomy grid of cards, each with its waveform in its group's
// colour, its name and kind, Use…, and ×. A card or chip plays on a click; a drag (or Alt with ← or →) moves it; the
// grip on top folds the drawer, or resizes it with a drag. The header names the palette, switches, renames, makes or
// forgets one, sets it strict or loose, and offers it as a kit or as the sounds to make a new one from. Every edit
// changes no project, so it runs at once (`POST /palette`); the agent's `kumi_palette` reads and edits the same
// palette. Both UI halves use it: what "Use as kit", a card's "Use…" and "Make…" do comes from the parent.
import { Button } from '@kumi/controls'
import { T } from '@kumi/theme'
import type { DragEvent, KeyboardEvent, PointerEvent, ReactNode, RefObject } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'

import { rgbOf } from './atlas/color.ts'
import { Glyph } from './atlas/Glyph.tsx'
import type { Share } from './atlas/lineage.ts'
import { clean, num } from './format.ts'
import { glyphOf, groupColor } from './mapModel.ts'
import { ActionMenu, Icon } from './mapUi.tsx'
import { LOOSE, moved, paletteName, type PaletteAction, type PaletteItem, type PaletteReply, type PaletteView, playableId } from './palette.ts'
import { peaksFor, peaksKept } from './peaks.ts'
import type { MapGroup } from './types.ts'
import { FOCUSABLE, Pending, S, SEGMENT_TRACK, segmentStyle, TYPE, useT } from './ui.tsx'

/** Sounds drawn at once: a palette can hold thousands (a gather takes 10,000). */
export const CHIPS_SHOWN = 240
/** The open drawer's height: where it starts, and how far a drag on its grip takes it; never so far that the map above
 *  keeps less than `MAP_SHARE` of the room (`MAP_LEAST` at the least): the find's list, a sound's card and the corner
 *  float over the map, and at 1024×700 a drawer that took all but 240 px left no sound in view (M14b). */
const OPEN_HEIGHT = 260
const LOWEST = 150
const HIGHEST = 620
const MAP_LEAST = 240
const MAP_SHARE = 0.62
/** How long sounds just gathered pop in where they land: long enough for the last of a batch (its delay included). */
const POP_FOR_MS = 1500
/** A card's name and kind: wrapped to two lines, cut only past them (its title says it whole). */
const TWO_LINES = { display: '-webkit-box', WebkitLineClamp: 2, WebkitBoxOrient: 'vertical', overflow: 'hidden', overflowWrap: 'anywhere', minWidth: 0 } as const

/** The button a box holds, down its first children: Hermes's Button and the ⋯ menu's take no ref of their own. */
function firstButton(box: Element | null): HTMLButtonElement | null {
  let at = box?.firstElementChild ?? null
  while (at && !(at instanceof HTMLButtonElement)) at = at.firstElementChild
  return at
}

export interface PaletteDockProps {
  palette: PaletteReply | null
  /** The map's groups, for each sound's colour. */
  groups: Record<string, MapGroup> | undefined
  /** What plays now (a sound id or an artifact id). */
  playing: string | null
  onPlay: (id: string) => void
  onStop: () => void
  /** Run an edit (the store's `paletteDo`); true when it was made. */
  onDo: (action: PaletteAction) => Promise<boolean>
  /** What the last edit said, or why it was refused (the store's `paletteSaid`): shown a few seconds. */
  said: { text: string; failed: boolean; at: number } | null
  /** While Make is open over the map: the sentence is heard, not shown (Make says "✓ In palette" in place, and the
   *  sentence would float over Make's foot, P7A-38). */
  hush?: boolean
  /** The audio behind an id, as a data URL (the audition's cache), for the waveforms. */
  audio: (id: string) => Promise<string>
  /** "Use as kit": Desktop drafts a chat request, the dashboard copies it. */
  kit?: (palette: PaletteView) => ReactNode
  /** A card's "Use…": Desktop drafts the sound's reference in the chat, the dashboard copies it. */
  use?: (item: PaletteItem) => ReactNode
  /** "Make…": opens the Make panel on the palette's sounds. */
  make?: (palette: PaletteView) => void
  /** The drawer's own box, for the Make panel to fly a kept sound into. */
  landing?: RefObject<HTMLElement | null>
  /** Whether the drawer is open, kept by the parent (each UI half remembers it), and how to change it. */
  open: boolean
  onOpen: (open: boolean) => void
  /** A made sound's lineage, by its id, when the window knows what it was made from (`atlas/lineage.ts`): its card
   *  wears its families' colours. */
  lineage?: (artifactId: string) => Share[] | null
  /** The pointer or the keyboard came onto a made sound's card (its id) or left it (null): the map rings its parents. */
  onKin?: (artifactId: string | null) => void
}

/** A waveform drawn from the sound's own audio, decoded once it's on screen (a card scrolled out of view costs
 *  nothing), in `color`; or, for a sound Kumi made, in its families' colours left to right (`tints`: null is the
 *  accent, the canvas's own colour). */
function Waveform({ id, color, audio, width, height, bars, tints }: { id: string | null; color: string; audio: (id: string) => Promise<string>; width: number | '100%'; height: number; bars: number; tints?: readonly (string | null)[] | null }) {
  const canvas = useRef<HTMLCanvasElement | null>(null)
  const [peaks, setPeaks] = useState<number[] | null>(() => (id ? peaksKept(id, bars) : null))
  const [drawnWidth, setDrawnWidth] = useState(typeof width === 'number' ? width : 0)
  useEffect(() => {
    const element = canvas.current
    if (!id || !element || peaks) return
    let live = true
    const observer = new IntersectionObserver(entries => {
      if (!entries.some(entry => entry.isIntersecting)) return
      observer.disconnect()
      void peaksFor(id, bars, audio).then(found => live && setPeaks(found))
    })
    observer.observe(element)
    return () => {
      live = false
      observer.disconnect()
    }
  }, [id, audio, peaks, bars])
  useEffect(() => {
    if (typeof width === 'number') return
    const element = canvas.current
    if (!element) return
    const observer = new ResizeObserver(entries => setDrawnWidth(Math.floor(entries[0]?.contentRect.width ?? 0)))
    observer.observe(element)
    return () => observer.disconnect()
  }, [width])
  useEffect(() => {
    const element = canvas.current
    const context = element?.getContext('2d')
    if (!element || !context || drawnWidth === 0) return
    const ratio = window.devicePixelRatio || 1
    element.width = drawnWidth * ratio
    element.height = height * ratio
    context.setTransform(ratio, 0, 0, ratio, 0, 0)
    context.clearRect(0, 0, drawnWidth, height)
    // A canvas takes no CSS variables, and a theme colour is one (`var(--ui-accent)`): the canvas wears the colour as
    // its own CSS colour, and the computed value is what it draws with.
    const own = getComputedStyle(element).color
    if (tints && tints.length > 1) {
      const across = context.createLinearGradient(0, 0, drawnWidth, 0)
      tints.forEach((tint, index) => across.addColorStop(index / (tints.length - 1), tint && rgbOf(tint) ? tint : own))
      context.fillStyle = across
    } else context.fillStyle = own
    const shown = peaks ?? new Array<number>(bars).fill(0.08)
    const step = drawnWidth / shown.length
    context.globalAlpha = peaks ? 0.95 : 0.35
    shown.forEach((peak, bar) => {
      const tall = Math.max(1, peak * (height - 2))
      context.fillRect(bar * step, (height - tall) / 2, Math.max(1, step - 0.6), tall)
    })
  }, [peaks, color, drawnWidth, height, bars, tints?.join()]) // eslint-disable-line react-hooks/exhaustive-deps
  return <canvas aria-hidden ref={canvas} style={{ width, height, flex: 'none', display: 'block', color }} />
}

/** What a chip and a card share: dragged to move, Alt with ← or → to move by keys, Delete to take it out (from its
 *  play button: the focus goes on to the next sound's). */
function useItemKeys(index: number, onMove: (to: number) => void, onRemove: (from: 'play' | 'remove') => void) {
  return (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.altKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
      event.preventDefault()
      onMove(index + (event.key === 'ArrowLeft' ? -1 : 1))
    } else if (event.key === 'Delete') {
      event.preventDefault()
      onRemove('play')
    }
  }
}

interface ItemProps {
  item: PaletteItem
  index: number
  color: string
  /** Its family's glyph on the map; null for a sound of no family here (one Kumi made, or one off this map). */
  glyph: number | null
  /** A made sound's families' colours, left to right, when they're known (null in one: the accent). */
  tints: readonly (string | null)[] | null
  /** The pointer or the keyboard came onto a made sound's card (true) or left it: its parents are ringed on the map. */
  onKin?: (on: boolean) => void
  playing: boolean
  audio: (id: string) => Promise<string>
  onPlay: () => void
  onStop: () => void
  /** Take it out; `from` is the control it was taken out with, where the focus goes on the next sound. */
  onRemove: (from: 'play' | 'remove') => void
  /** Its play button and its ×, for the focus to land on when the sound before it is taken out. */
  playRef: (element: HTMLButtonElement | null) => void
  removeRef: (element: HTMLButtonElement | null) => void
  onMove: (to: number) => void
  /** Another sound was dropped on this one: it takes this one's place. */
  onDropHere: () => void
  dragging: string | null
  onDrag: (key: string | null) => void
  /** Just gathered: it pops in, after the flight from the map lands (its place in the batch sets the delay). */
  arriving: number | null
  use?: ReactNode
}

/** The drag-and-drop a chip and a card share, and how one looks while it's dragged or something hovers over it. */
function useDrop({ item, dragging, onDrag, onDropHere }: Pick<ItemProps, 'item' | 'dragging' | 'onDrag' | 'onDropHere'>, name: string) {
  const [over, setOver] = useState(false)
  return {
    over,
    handlers: {
      draggable: true,
      onDragEnd: () => onDrag(null),
      onDragLeave: () => setOver(false),
      onDragOver: (event: DragEvent) => {
        if (!dragging || dragging === item.key) return
        event.preventDefault()
        setOver(true)
      },
      onDragStart: (event: DragEvent) => {
        event.dataTransfer.effectAllowed = 'move'
        event.dataTransfer.setData('text/plain', name)
        onDrag(item.key)
      },
      onDrop: (event: DragEvent) => {
        event.preventDefault()
        setOver(false)
        if (dragging && dragging !== item.key) onDropHere()
      }
    }
  }
}

/** A made sound's chip and card tell the map while the pointer or the keyboard is on them (it rings the sound's
 *  parents), and when they go: a sound taken out from under the pointer leaves no ring behind. */
function useKin(onKin: ItemProps['onKin']) {
  const tell = useRef(onKin)
  tell.current = onKin
  useEffect(() => () => tell.current?.(false), [])
  return { onBlur: () => onKin?.(false), onFocus: () => onKin?.(true), onPointerEnter: () => onKin?.(true), onPointerLeave: () => onKin?.(false) }
}

/** One sound in the folded drawer's row: its waveform and name. */
function Chip(props: ItemProps) {
  const { item, index, color, playing, audio, onPlay, onStop, onRemove, onMove, dragging, arriving, tints, onKin, playRef, removeRef } = props
  const t = useT()
  const id = playableId(item)
  const name = clean(item.name, 60) || item.key
  const { over, handlers } = useDrop(props, name)
  const keys = useItemKeys(index, onMove, onRemove)
  const kin = useKin(onKin)
  return (
    <div
      {...handlers}
      {...kin}
      className={arriving === null ? undefined : 'kumi-pop'}
      role="listitem"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 2,
        flex: 'none',
        maxWidth: 220,
        padding: '2px 2px 2px 4px',
        borderRadius: 7,
        border: `1px solid ${playing ? color : over ? T.accent : T.border}`,
        boxShadow: playing ? `0 0 0 1px ${color}, 0 0 10px -2px ${color}` : 'none',
        background: playing ? T.chip : 'transparent',
        opacity: dragging === item.key ? 0.4 : id ? 1 : 0.55,
        transition: 'border-color 120ms, box-shadow 160ms, opacity 120ms',
        cursor: 'grab',
        animationDelay: arriving === null ? undefined : `${380 + Math.min(arriving, 24) * 22}ms`
      }}
    >
      <button
        aria-label={playing ? t('dock.stop', name) : t('dock.play', name)}
        aria-pressed={playing}
        className={FOCUSABLE}
        disabled={!id}
        onClick={() => (playing ? onStop() : onPlay())}
        onKeyDown={keys}
        ref={playRef}
        title={id ? `${name}${item.class ? ` · ${clean(item.class, 16)}` : ''}\n${t('dock.reorderHint')}` : t('dock.offMap')}
        type="button"
        style={{ ...S.row, gap: 5, minWidth: 0, font: 'inherit', fontSize: 11.5, padding: '1px 3px', border: 'none', background: 'transparent', color: T.text, cursor: id ? 'pointer' : 'default' }}
      >
        <Waveform audio={audio} bars={28} color={color} height={16} id={id} tints={tints} width={44} />
        <span style={S.ellipsis}>{name}</span>
      </button>
      <button
        aria-label={t('dock.remove', name)}
        className={FOCUSABLE}
        onClick={() => onRemove('remove')}
        ref={removeRef}
        title={t('dock.remove', name)}
        type="button"
        style={{ font: 'inherit', fontSize: 12, lineHeight: '14px', width: 16, height: 16, padding: 0, borderRadius: 3, border: 'none', background: 'transparent', color: T.faint, cursor: 'pointer', flex: 'none' }}
      >
        ×
      </button>
    </div>
  )
}

/** One sound in the open drawer: a card with its waveform across the top, its name, kind, Use… and ×. */
function Card(props: ItemProps) {
  const { item, index, color, playing, audio, onPlay, onStop, onRemove, onMove, dragging, arriving, use, glyph, tints, onKin, playRef, removeRef } = props
  const t = useT()
  const id = playableId(item)
  const kind = item.source === 'artifact' ? t('dock.made') : clean(item.class ?? '', 16)
  const name = clean(item.name, 80) || item.key
  const { over, handlers } = useDrop(props, name)
  const keys = useItemKeys(index, onMove, onRemove)
  const kin = useKin(onKin)
  return (
    <div
      {...handlers}
      {...kin}
      className={arriving === null ? undefined : 'kumi-pop'}
      role="listitem"
      style={{
        position: 'relative',
        display: 'flex',
        flexDirection: 'column',
        gap: 4,
        minWidth: 0,
        padding: '7px 7px 7px 10px',
        borderRadius: 10,
        border: `1px solid ${playing ? color : over ? T.accent : T.border}`,
        background: playing ? `color-mix(in srgb, ${color} 12%, transparent)` : `color-mix(in srgb, ${T.card} 55%, transparent)`,
        boxShadow: playing ? `0 0 0 1px ${color}, 0 6px 18px -8px ${color}` : 'none',
        opacity: dragging === item.key ? 0.4 : id ? 1 : 0.55,
        transition: 'border-color 120ms, box-shadow 180ms, background-color 160ms, opacity 120ms, transform 160ms',
        cursor: 'grab',
        animationDelay: arriving === null ? undefined : `${380 + Math.min(arriving, 24) * 22}ms`
      }}
    >
      {/* The rule down its edge: its family's colour, or the colours of the families a made sound comes from. It sits
          in a clipped layer of its own, so the card's corners round it and the card itself clips nothing (a grid's
          rows shrink to nothing under items that clip). */}
      <span aria-hidden style={{ position: 'absolute', inset: 0, borderRadius: 'inherit', overflow: 'hidden', pointerEvents: 'none' }}>
        <span style={{ position: 'absolute', left: 0, top: 0, bottom: 0, width: 3, background: tints && tints.length > 1 ? `linear-gradient(${tints.map(tint => tint ?? T.accent).join(', ')})` : (tints?.[0] ?? color) }} />
      </span>
      <button
        aria-label={playing ? t('dock.stop', name) : t('dock.play', name)}
        aria-pressed={playing}
        className={FOCUSABLE}
        disabled={!id}
        onClick={() => (playing ? onStop() : onPlay())}
        onKeyDown={keys}
        ref={playRef}
        title={id ? t('dock.reorderHint') : t('dock.offMap')}
        type="button"
        style={{ display: 'block', width: '100%', padding: 0, border: 'none', background: 'transparent', cursor: id ? 'pointer' : 'default' }}
      >
        <Waveform audio={audio} bars={64} color={tints?.length === 1 && tints[0] ? tints[0] : color} height={30} id={id} tints={tints} width="100%" />
      </button>
      <div style={{ ...S.row, gap: 5 }}>
        {/* The shape the map draws its family in; a made sound has none, and wears a spark. */}
        {glyph === null ? (
          <span aria-hidden style={{ color, fontSize: 9, lineHeight: 1, flex: 'none' }}>
            {item.source === 'artifact' ? '✦' : '●'}
          </span>
        ) : (
          <Glyph color={color} glyph={glyph} />
        )}
        {/* Two lines before it's cut: four cards a row leave a name ~130 px, and names differ at their end. */}
        <span style={{ ...TWO_LINES, flex: 1, fontSize: 11.5, lineHeight: '14px', color: T.text }} title={name}>
          {name}
        </span>
        <button
          aria-label={t('dock.remove', name)}
          className={FOCUSABLE}
          onClick={() => onRemove('remove')}
          ref={removeRef}
          title={t('dock.remove', name)}
          type="button"
          style={{ font: 'inherit', fontSize: 12, lineHeight: '14px', width: 16, height: 16, padding: 0, borderRadius: 3, border: 'none', background: 'transparent', color: T.faint, cursor: 'pointer', flex: 'none' }}
        >
          ×
        </button>
      </div>
      <div style={{ ...S.row, gap: 5, minHeight: 18 }}>
        <span style={{ ...S.faint, ...TWO_LINES, flex: 1, fontSize: 10.5, lineHeight: '13px' }} title={kind || undefined}>
          {kind}
        </span>
        {use}
      </div>
    </div>
  )
}

/** A one-line text field that commits on Enter or when it loses focus, and gives up on Escape. */
function NameField({ initial, label, onDone }: { initial: string; label: string; onDone: (name: string | null) => void }) {
  const [text, setText] = useState(initial)
  const field = useRef<HTMLInputElement | null>(null)
  const done = useRef(false)
  useEffect(() => {
    field.current?.focus()
    field.current?.select()
  }, [])
  const finish = (name: string | null) => {
    if (done.current) return
    done.current = true
    onDone(name)
  }
  return (
    <input
      aria-label={label}
      className={FOCUSABLE}
      maxLength={120}
      onBlur={() => finish(paletteName(text))}
      onChange={event => setText(event.target.value)}
      onKeyDown={event => {
        if (event.key === 'Enter') {
          event.preventDefault()
          finish(paletteName(text))
        } else if (event.key === 'Escape') {
          event.preventDefault()
          event.stopPropagation()
          finish(null)
        }
      }}
      placeholder={label}
      ref={field}
      style={{ font: 'inherit', fontSize: 12, fontWeight: 600, width: '12rem', padding: '1px 6px', borderRadius: 3, border: `1px solid ${T.accent}`, background: 'transparent', color: T.text }}
      value={text}
    />
  )
}

export function PaletteDock({ palette, groups, playing, onPlay, onStop, onDo, said, hush = false, audio, kit, use, make, landing, open, onOpen, lineage, onKin }: PaletteDockProps) {
  const t = useT()
  const current = palette?.current ?? null
  const [editing, setEditing] = useState<'new' | 'rename' | null>(null)
  const [forgetting, setForgetting] = useState(false)
  const [dragging, setDragging] = useState<string | null>(null)
  const [height, setHeight] = useState(OPEN_HEIGHT)
  // A move shows at once; the backend's answer then replaces it.
  const [order, setOrder] = useState<PaletteItem[] | null>(null)
  useEffect(() => setOrder(null), [palette])
  // Which sounds were there before the last answer: one that wasn't is new (a gather), and pops in. The first answer
  // shows the palette as it is, with nothing popping. A pop plays where the sounds land, once: a fold or an unfold
  // later draws the same sounds without it (`POP_FOR_MS`).
  const known = useRef<{ id: string | null; keys: Set<string> } | null>(null)
  const arrivedAt = useRef(0)
  const arrived = useMemo(() => {
    const keys = new Set((current?.items ?? []).map(item => item.key))
    const before = known.current
    const fresh = before && before.id === current?.id ? (current?.items ?? []).filter(item => !before.keys.has(item.key)).map(item => item.key) : []
    known.current = { id: current?.id ?? null, keys }
    if (fresh.length > 0) arrivedAt.current = Date.now()
    return new Map(fresh.map((key, order) => [key, order]))
  }, [current])
  // New sounds go at the end: the row or grid scrolls to them, so a gather or a kept sound lands in view. Once per
  // gather: a fold or an unfold later opens at the start (the sounds just gathered stay `arrived` until the next answer).
  const strip = useRef<HTMLDivElement | null>(null)
  const scrolledFor = useRef<Map<string, number> | null>(null)
  useEffect(() => {
    const element = strip.current
    if (scrolledFor.current === arrived) return
    scrolledFor.current = arrived
    if (!element || arrived.size === 0) return
    const smooth = window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth'
    element.scrollTo(open ? { top: element.scrollHeight, behavior: smooth } : { left: element.scrollWidth, behavior: smooth })
  }, [arrived, open])
  // A sound taken out hands the focus on: to the same control of the sound after it (before it, at the end), or to the
  // grip once the palette is empty, never to the page.
  const plays = useRef(new Map<string, HTMLButtonElement>())
  const removes = useRef(new Map<string, HTMLButtonElement>())
  const gripButton = useRef<HTMLButtonElement | null>(null)
  const refocus = useRef<{ key: string | null; from: 'play' | 'remove' } | null>(null)
  useEffect(() => {
    const next = refocus.current
    if (!next) return
    refocus.current = null
    if (next.key === null) return gripButton.current?.focus()
    const play = plays.current.get(next.key)
    ;(next.from === 'play' && play && !play.disabled ? play : removes.current.get(next.key))?.focus()
  })
  const keepRef = (kept: Map<string, HTMLButtonElement>, key: string) => (element: HTMLButtonElement | null) => {
    if (element) kept.set(key, element)
    else kept.delete(key)
  }
  // Forget…'s ask takes the focus to Keep, the safe choice, and Keep gives it back to the palette's ⋯ menu. Both are
  // drawn by components that take no ref, so each sits in a box that draws nothing (`firstButton`).
  const keepBox = useRef<HTMLSpanElement | null>(null)
  const menuBox = useRef<HTMLSpanElement | null>(null)
  useEffect(() => {
    if (forgetting) firstButton(keepBox.current)?.focus()
  }, [forgetting])
  // The folded row scrolls sideways: whichever end has more sounds past it fades out, so a chip at the edge reads as
  // "more this way", not as cut off; and a mouse's wheel turns it (nothing else here scrolls up or down).
  const [edges, setEdges] = useState({ start: false, end: false })
  useEffect(() => {
    const element = strip.current
    if (open || !element) return
    const measure = () => {
      const next = { start: element.scrollLeft > 1, end: element.scrollLeft + element.clientWidth < element.scrollWidth - 1 }
      setEdges(before => (before.start === next.start && before.end === next.end ? before : next))
    }
    const wheel = (event: WheelEvent) => {
      if (Math.abs(event.deltaY) <= Math.abs(event.deltaX) || element.scrollWidth <= element.clientWidth) return
      event.preventDefault()
      element.scrollLeft += event.deltaY
    }
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    element.addEventListener('scroll', measure, { passive: true })
    element.addEventListener('wheel', wheel, { passive: false })
    return () => {
      observer.disconnect()
      element.removeEventListener('scroll', measure)
      element.removeEventListener('wheel', wheel)
    }
  }, [open, current?.items.length])
  // The open drawer never leaves the map less than its least height: the room is its workspace's, as it is now.
  const section = useRef<HTMLElement | null>(null)
  const [room, setRoom] = useState(Number.POSITIVE_INFINITY)
  useEffect(() => {
    const parent = section.current?.parentElement
    if (!parent) return
    const observer = new ResizeObserver(() => setRoom(parent.clientHeight))
    observer.observe(parent)
    return () => observer.disconnect()
  }, [])
  const highest = Math.max(LOWEST, Math.min(HIGHEST, room - Math.max(MAP_LEAST, Math.round(room * MAP_SHARE))))
  const rowMask = `linear-gradient(to right, ${edges.start ? 'transparent' : '#000'}, #000 28px, #000 calc(100% - 28px), ${edges.end ? 'transparent' : '#000'})`
  // The last sentence fades after a while: it says what just happened, not what is.
  const [now, setNow] = useState(() => Date.now())
  const shownFor = said?.failed ? 8000 : 4000
  useEffect(() => {
    if (!said) return
    setNow(Date.now())
    const timer = setTimeout(() => setNow(Date.now()), Math.max(0, said.at + shownFor - Date.now()))
    return () => clearTimeout(timer)
  }, [said, shownFor])
  const saying = said && now - said.at < shownFor ? said : null
  const sayingText = saying ? (saying.failed ? t('dock.failed', saying.text) : saying.text) : ''

  // The grip: a click folds or opens the drawer; a drag on an open one resizes it.
  const grip = useRef<{ y: number; height: number; moved: boolean } | null>(null)
  const gripDown = (event: PointerEvent<HTMLButtonElement>) => {
    if (!open || event.button !== 0) return
    event.currentTarget.setPointerCapture?.(event.pointerId)
    grip.current = { y: event.clientY, height, moved: false }
  }
  const gripMove = (event: PointerEvent<HTMLButtonElement>) => {
    const held = grip.current
    if (!held) return
    const delta = held.y - event.clientY
    if (Math.abs(delta) > 3) held.moved = true
    if (held.moved) setHeight(Math.min(highest, Math.max(LOWEST, held.height + delta)))
  }

  const run = (action: PaletteAction) =>
    onDo(action).then(done => {
      if (!done) setOrder(null)
    })
  const items = order ?? current?.items ?? []
  const colorOf = (item: PaletteItem) => (item.source === 'artifact' ? T.accent : item.group === null ? T.faint : groupColor(groups?.[String(item.group)]?.color, item.group))
  const move = (key: string, to: number) => {
    if (to < 0 || to >= items.length) return
    setOrder(moved(items, key, to))
    void run({ action: 'move', key, to })
  }
  const itemProps = (item: PaletteItem, index: number): ItemProps => ({
    audio,
    color: colorOf(item),
    glyph: item.source === 'artifact' || item.group === null ? null : glyphOf(item.group),
    tints: item.source === 'artifact' && item.artifact_id ? (lineage?.(item.artifact_id)?.map(part => part.color) ?? null) : null,
    onKin: item.source === 'artifact' && item.artifact_id && onKin ? on => onKin(on ? item.artifact_id : null) : undefined,
    dragging,
    index,
    item,
    arriving: Date.now() - arrivedAt.current < POP_FOR_MS ? (arrived.get(item.key) ?? null) : null,
    onDrag: setDragging,
    onDropHere: () => dragging && move(dragging, index),
    onMove: to => move(item.key, to),
    onPlay: () => {
      const id = playableId(item)
      if (id) onPlay(id)
    },
    onRemove: from => {
      const at = items.findIndex(other => other.key === item.key)
      refocus.current = { key: (items[at + 1] ?? items[at - 1])?.key ?? null, from }
      setOrder(items.filter(other => other.key !== item.key))
      void run({ action: 'remove', keys: [item.key] })
    },
    playRef: keepRef(plays.current, item.key),
    removeRef: keepRef(removes.current, item.key),
    onStop,
    playing: playing !== null && playing === playableId(item),
    use: use?.(item)
  })

  // The header (D6): the palette's name with its own menu (Rename…, Forget…), Strict | Loose, then what's done with its
  // sounds (Make… and Use as kit…, quiet) and the drawer's button at the right edge. It wraps at the narrowest widths.
  const header = (
    <div style={{ ...S.row, flexWrap: 'wrap', gap: 6 }}>
      <span style={{ ...TYPE.caption, fontWeight: 600, letterSpacing: '0.06em', textTransform: 'uppercase', color: T.muted }}>{t('dock.title')}</span>
      {editing ? (
        <NameField
          initial={editing === 'rename' ? (current?.name ?? '') : ''}
          label={editing === 'rename' ? t('dock.renameTitle') : t('dock.newName')}
          onDone={name => {
            const action = editing
            setEditing(null)
            if (name && action === 'rename' && name !== current?.name) void run({ action: 'rename', name })
            if (name && action === 'new') void run({ action: 'new', name })
          }}
        />
      ) : (
        <>
          {palette && palette.palettes.length > 0 && (
            <select
              aria-label={t('dock.switch')}
              className={FOCUSABLE}
              onChange={event => {
                const value = event.target.value
                if (value === '\u0000new') setEditing('new')
                else if (value !== current?.id) void run({ action: 'use', id: value })
              }}
              style={{ font: 'inherit', ...TYPE.small, fontWeight: 600, maxWidth: '14rem', height: 24, padding: '0 4px', borderRadius: 4, border: `1px solid ${T.border}`, background: 'transparent', color: T.text }}
              value={current?.id ?? ''}
            >
              {!current && <option value="">{t('dock.choose')}</option>}
              {palette.palettes.map(entry => (
                <option key={entry.id} value={entry.id}>
                  {`${clean(entry.name, 40)} (${num(entry.items, 0)})`}
                </option>
              ))}
              <option value={'\u0000new'}>{t('dock.newPalette')}</option>
            </select>
          )}
          {palette === null ? <Pending label={t('dock.reading')} width="8rem" /> : palette.palettes.length === 0 && <span style={S.faint}>{t('dock.none')}</span>}
          {palette && palette.palettes.length === 0 && (
            <Button kind="quiet" onClick={() => setEditing('new')} tip={t('dock.newName')}>
              {t('dock.newPalette')}
            </Button>
          )}
          {current && (
            <span ref={menuBox} style={{ display: 'contents' }}>
              <ActionMenu
                actions={[
                  { id: 'rename', name: t('dock.rename'), title: t('dock.renameTitle'), onChoose: () => setEditing('rename') },
                  { id: 'forget', name: t('dock.forget'), title: t('dock.forgetTitle', clean(current.name, 40)), onChoose: () => setForgetting(true), danger: true }
                ]}
                label={t('dock.menu', clean(current.name, 40))}
                up
              />
            </span>
          )}
        </>
      )}
      {current && (
        <span aria-label={t('dock.mode')} role="group" style={{ ...SEGMENT_TRACK, flexWrap: 'nowrap' }}>
          <button aria-pressed={current.reach === 0} className={FOCUSABLE} onClick={() => current.reach !== 0 && void run({ action: 'reach', reach: 0 })} title={t('dock.strictTitle')} type="button" style={segmentStyle(current.reach === 0)}>
            {t('dock.strict')}
          </button>
          <button aria-pressed={current.reach > 0} className={FOCUSABLE} onClick={() => current.reach === 0 && void run({ action: 'reach', reach: LOOSE })} title={t('dock.looseTitle', current.reach || LOOSE)} type="button" style={segmentStyle(current.reach > 0)}>
            {current.reach > 0 ? t('dock.looseN', current.reach) : t('dock.loose')}
          </button>
        </span>
      )}
      {current && current.excluded > 0 && <span style={{ ...TYPE.caption, color: T.faint }}>{t('dock.excluded', current.excluded)}</span>}
      <span style={{ flex: 1 }} />
      {current && current.items.length > 1 && make && (
        <Button kind="quiet" onClick={() => make(current)} tip={t('make.openTitle')}>
          {t('make.open')}
        </Button>
      )}
      {current && current.items.length > 0 && kit?.(current)}
      {current && items.length > 0 && (
        <Button expanded={open} onClick={() => onOpen(!open)} tip={open ? t('dock.collapse') : t('dock.expand')}>
          {open ? t('dock.fold') : t('dock.showAll', items.length)}
          <span aria-hidden style={{ display: 'inline-block', marginLeft: 4, transform: open ? 'rotate(180deg)' : 'none', transition: 'transform 200ms' }}>
            ▴
          </span>
        </Button>
      )}
    </div>
  )

  return (
    <section
      aria-label={t('dock.title')}
      ref={element => {
        section.current = element
        if (landing) landing.current = element
      }}
      style={{
        position: 'relative',
        display: 'flex',
        flexDirection: 'column',
        gap: 6,
        padding: '0 10px 8px',
        borderTop: `1px solid ${T.border}`,
        background: `color-mix(in srgb, ${T.card} 40%, transparent)`,
        color: T.text,
        flex: 'none',
        height: open && items.length > 0 ? Math.min(height, highest) : undefined,
        transition: grip.current ? 'none' : 'height 260ms cubic-bezier(.2,.8,.2,1)'
      }}
    >
      {/* The grip: a handle across the top, so the drawer reads as one that opens. */}
      <button
        aria-expanded={open}
        aria-label={open ? t('dock.collapse') : t('dock.expand')}
        className={FOCUSABLE}
        onClick={() => {
          if (grip.current?.moved) return
          onOpen(!open)
        }}
        onPointerCancel={() => (grip.current = null)}
        onPointerDown={gripDown}
        onPointerMove={gripMove}
        onPointerUp={() => setTimeout(() => (grip.current = null), 0)}
        ref={gripButton}
        title={open ? t('dock.gripTitle') : t('dock.expand')}
        type="button"
        style={{ alignSelf: 'stretch', height: 12, padding: 0, border: 'none', background: 'transparent', cursor: open ? 'ns-resize' : 'pointer', display: 'flex', alignItems: 'center', justifyContent: 'center', flex: 'none' }}
      >
        <span aria-hidden style={{ width: 36, height: 4, borderRadius: 2, background: T.border, transition: 'background-color 140ms' }} />
      </button>
      {header}
      {/* What the last edit said, for a few seconds, floating just above the dock's top edge: it never adds a line (a line
          that came and went resized the map, and an unmoved map refit), and however crowded the header, it's never cut. */}
      <span
        aria-live="polite"
        style={{
          position: 'absolute',
          right: 10,
          bottom: 'calc(100% + 6px)',
          maxWidth: 'calc(100% - 20px)',
          zIndex: 3,
          padding: '3px 10px',
          borderRadius: 12,
          border: `1px solid ${T.border}`,
          background: T.raised,
          boxShadow: '0 6px 18px -10px rgba(0,0,0,.5)',
          ...TYPE.caption,
          color: saying?.failed ? T.warn : T.muted,
          pointerEvents: 'none',
          opacity: saying && !hush ? 1 : 0,
          transition: 'opacity 160ms'
        }}
      >
        {sayingText}
      </span>
      {current && forgetting && (
        <div role="alert" style={{ ...S.row, flexWrap: 'wrap', gap: 6 }}>
          <span style={S.muted}>{t('dock.forgetAsk', clean(current.name, 40), current.items.length)}</span>
          <span style={{ flex: 1 }} />
          <span ref={keepBox} style={{ display: 'contents' }}>
            <Button
              kind="quiet"
              onClick={() => {
                setForgetting(false)
                firstButton(menuBox.current)?.focus()
              }}
            >
              {t('dock.keep')}
            </Button>
          </span>
          <Button
            kind="destructive"
            onClick={() => {
              setForgetting(false)
              void run({ action: 'forget', id: current.id })
            }}
          >
            {t('dock.forgetYes')}
          </Button>
        </div>
      )}
      {current && items.length > 0 && open && (
        <div aria-label={t('dock.sounds')} ref={strip} role="list" style={{ flex: 1, minHeight: 0, overflowY: 'auto', display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(150px, 1fr))', gap: 8, alignContent: 'start', padding: '2px 1px' }}>
          {items.slice(0, CHIPS_SHOWN).map((item, index) => (
            <Card key={item.key} {...itemProps(item, index)} />
          ))}
          {items.length > CHIPS_SHOWN && <span style={{ ...S.faint, alignSelf: 'center' }}>{t('dock.more', items.length - CHIPS_SHOWN)}</span>}
        </div>
      )}
      {current && items.length > 0 && !open && (
        <div
          aria-label={t('dock.sounds')}
          ref={strip}
          role="list"
          style={{
            display: 'flex',
            flexWrap: 'nowrap',
            gap: 5,
            overflowX: 'auto',
            paddingBottom: 2,
            scrollbarWidth: 'thin',
            maskImage: rowMask,
            WebkitMaskImage: rowMask
          }}
        >
          {items.slice(0, CHIPS_SHOWN).map((item, index) => (
            <Chip key={item.key} {...itemProps(item, index)} />
          ))}
          {items.length > CHIPS_SHOWN && <span style={{ ...S.faint, alignSelf: 'center', flex: 'none' }}>{t('dock.more', items.length - CHIPS_SHOWN)}</span>}
        </div>
      )}
      {palette !== null && (!current || items.length === 0) && (
        <span style={{ ...S.row, gap: 6, ...S.faint }}>
          <Icon name="plus" size={12} />
          {t('dock.empty')}
        </span>
      )}
    </section>
  )
}
