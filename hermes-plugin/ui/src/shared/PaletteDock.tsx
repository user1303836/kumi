// The palette, a drawer under the sound map: the sounds the producer gathered to work with. Folded, it's one row of
// chips with their waveforms and a "Show all" pill; open, a roomy grid of cards, each with its waveform in its group's
// colour, its name and kind, Use…, and ×. A card or chip plays on a click; a drag (or Alt with ← or →) moves it; the
// grip on top folds the drawer, or resizes it with a drag. The header names the palette, switches, renames, makes or
// forgets one, sets it strict or loose, and offers it as a kit or as the sounds to make a new one from. Every edit
// changes no project, so it runs at once (`POST /palette`); the agent's `kumi_palette` reads and edits the same
// palette. Both UI halves use it: what "Use as kit", a card's "Use…" and "Make…" do comes from the parent.
import { T } from '@kumi/theme'
import type { DragEvent, KeyboardEvent, PointerEvent, ReactNode, RefObject } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'

import { rgbOf } from './atlas/color.ts'
import { Glyph } from './atlas/Glyph.tsx'
import type { Share } from './atlas/lineage.ts'
import { clean, num } from './format.ts'
import { glyphOf, groupColor } from './mapModel.ts'
import { Icon } from './mapUi.tsx'
import { LOOSE, moved, paletteName, type PaletteAction, type PaletteItem, type PaletteReply, type PaletteView, playableId } from './palette.ts'
import { peaksFor, peaksKept } from './peaks.ts'
import type { MapGroup } from './types.ts'
import { FOCUSABLE, S, TextButton, useT } from './ui.tsx'

/** Sounds drawn at once: a palette can hold thousands (a gather takes 10,000). */
export const CHIPS_SHOWN = 240
/** The open drawer's height: where it starts, and how far a drag on its grip takes it. */
const OPEN_HEIGHT = 260
const LOWEST = 150
const HIGHEST = 620

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

/** What a chip and a card share: dragged to move, Alt with ← or → to move by keys, Delete to take it out. */
function useItemKeys(index: number, onMove: (to: number) => void, onRemove: () => void) {
  return (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.altKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
      event.preventDefault()
      onMove(index + (event.key === 'ArrowLeft' ? -1 : 1))
    } else if (event.key === 'Delete') {
      event.preventDefault()
      onRemove()
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
  onRemove: () => void
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
  const { item, index, color, playing, audio, onPlay, onStop, onRemove, onMove, dragging, arriving, tints, onKin } = props
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
        onClick={onRemove}
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
  const { item, index, color, playing, audio, onPlay, onStop, onRemove, onMove, dragging, arriving, use, glyph, tints, onKin } = props
  const t = useT()
  const id = playableId(item)
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
        <span style={{ ...S.ellipsis, flex: 1, fontSize: 11.5, color: T.text }} title={name}>
          {name}
        </span>
        <button
          aria-label={t('dock.remove', name)}
          className={FOCUSABLE}
          onClick={onRemove}
          title={t('dock.remove', name)}
          type="button"
          style={{ font: 'inherit', fontSize: 12, lineHeight: '14px', width: 16, height: 16, padding: 0, borderRadius: 3, border: 'none', background: 'transparent', color: T.faint, cursor: 'pointer', flex: 'none' }}
        >
          ×
        </button>
      </div>
      <div style={{ ...S.row, gap: 5, minHeight: 18 }}>
        <span style={{ ...S.faint, ...S.ellipsis, flex: 1, fontSize: 10.5 }}>{item.source === 'artifact' ? t('dock.made') : clean(item.class ?? '', 16)}</span>
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

export function PaletteDock({ palette, groups, playing, onPlay, onStop, onDo, said, audio, kit, use, make, landing, open, onOpen, lineage, onKin }: PaletteDockProps) {
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
  // shows the palette as it is, with nothing popping.
  const known = useRef<{ id: string | null; keys: Set<string> } | null>(null)
  const arrived = useMemo(() => {
    const keys = new Set((current?.items ?? []).map(item => item.key))
    const before = known.current
    const fresh = before && before.id === current?.id ? (current?.items ?? []).filter(item => !before.keys.has(item.key)).map(item => item.key) : []
    known.current = { id: current?.id ?? null, keys }
    return new Map(fresh.map((key, order) => [key, order]))
  }, [current])
  // New sounds go at the end: the row or grid scrolls to them, so a gather or a kept sound lands in view.
  const strip = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    const element = strip.current
    if (!element || arrived.size === 0) return
    const smooth = window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth'
    element.scrollTo(open ? { top: element.scrollHeight, behavior: smooth } : { left: element.scrollWidth, behavior: smooth })
  }, [arrived, open])
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
    if (held.moved) setHeight(Math.min(HIGHEST, Math.max(LOWEST, held.height + delta)))
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
    arriving: arrived.get(item.key) ?? null,
    onDrag: setDragging,
    onDropHere: () => dragging && move(dragging, index),
    onMove: to => move(item.key, to),
    onPlay: () => {
      const id = playableId(item)
      if (id) onPlay(id)
    },
    onRemove: () => {
      setOrder(items.filter(other => other.key !== item.key))
      void run({ action: 'remove', keys: [item.key] })
    },
    onStop,
    playing: playing !== null && playing === playableId(item),
    use: use?.(item)
  })

  const header = (
    <div style={{ ...S.row, flexWrap: 'wrap', gap: 6 }}>
      <span style={{ ...S.heading, fontSize: 11, color: T.secondary }}>{t('dock.title')}</span>
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
              style={{ font: 'inherit', fontSize: 12, fontWeight: 600, maxWidth: '14rem', padding: '0 4px', borderRadius: 4, border: `1px solid ${T.border}`, background: 'transparent', color: T.text }}
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
          {(!palette || palette.palettes.length === 0) && <span style={S.faint}>{t('dock.none')}</span>}
          {current && (
            <TextButton onClick={() => setEditing('rename')} title={t('dock.renameTitle')}>
              {t('dock.rename')}
            </TextButton>
          )}
          {palette && palette.palettes.length === 0 && (
            <TextButton onClick={() => setEditing('new')} title={t('dock.newName')}>
              {t('dock.newPalette')}
            </TextButton>
          )}
        </>
      )}
      {current && (
        <span role="group" aria-label={t('dock.mode')} style={{ ...S.row, gap: 0 }}>
          <TextButton onClick={() => current.reach !== 0 && void run({ action: 'reach', reach: 0 })} pressed={current.reach === 0} title={t('dock.strictTitle')}>
            {t('dock.strict')}
          </TextButton>
          <TextButton onClick={() => current.reach === 0 && void run({ action: 'reach', reach: LOOSE })} pressed={current.reach > 0} title={t('dock.looseTitle', current.reach || LOOSE)}>
            {current.reach > 0 ? t('dock.looseN', current.reach) : t('dock.loose')}
          </TextButton>
        </span>
      )}
      {current && current.excluded > 0 && <span style={S.faint}>{t('dock.excluded', current.excluded)}</span>}
      <span style={{ flex: 1 }} />
      {current && current.items.length > 1 && make && (
        <TextButton onClick={() => make(current)} title={t('make.openTitle')}>
          {t('make.open')}
        </TextButton>
      )}
      {current && current.items.length > 0 && kit?.(current)}
      {current && !forgetting && (
        <TextButton onClick={() => setForgetting(true)} title={t('dock.forgetTitle', clean(current.name, 40))}>
          {t('dock.forget')}
        </TextButton>
      )}
      {current && items.length > 0 && (
        <button
          aria-expanded={open}
          className={FOCUSABLE}
          onClick={() => onOpen(!open)}
          title={open ? t('dock.collapse') : t('dock.expand')}
          type="button"
          style={{
            ...S.row,
            gap: 4,
            font: 'inherit',
            fontSize: 11.5,
            fontWeight: 600,
            padding: '2px 10px',
            borderRadius: 12,
            border: `1px solid ${T.accent}`,
            background: `color-mix(in srgb, ${T.accent} ${open ? 10 : 18}%, transparent)`,
            color: T.accent,
            cursor: 'pointer',
            flex: 'none'
          }}
        >
          {open ? t('dock.fold') : t('dock.showAll', items.length)}
          <span aria-hidden style={{ display: 'inline-block', transform: open ? 'rotate(180deg)' : 'none', transition: 'transform 200ms' }}>
            ▴
          </span>
        </button>
      )}
    </div>
  )

  return (
    <section
      aria-label={t('dock.title')}
      ref={landing}
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
        height: open && items.length > 0 ? height : undefined,
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
        title={open ? t('dock.gripTitle') : t('dock.expand')}
        type="button"
        style={{ alignSelf: 'stretch', height: 12, padding: 0, border: 'none', background: 'transparent', cursor: open ? 'ns-resize' : 'pointer', display: 'flex', alignItems: 'center', justifyContent: 'center', flex: 'none' }}
      >
        <span aria-hidden style={{ width: 36, height: 4, borderRadius: 2, background: T.border, transition: 'background-color 140ms' }} />
      </button>
      {header}
      {current && forgetting && (
        <div role="alert" style={{ ...S.row, flexWrap: 'wrap', gap: 6 }}>
          <span style={S.muted}>{t('dock.forgetAsk', clean(current.name, 40), current.items.length)}</span>
          <TextButton
            onClick={() => {
              setForgetting(false)
              void run({ action: 'forget', id: current.id })
            }}
            strong
          >
            {t('dock.forgetYes')}
          </TextButton>
          <TextButton onClick={() => setForgetting(false)}>{t('dock.keep')}</TextButton>
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
        <div aria-label={t('dock.sounds')} ref={strip} role="list" style={{ display: 'flex', flexWrap: 'nowrap', gap: 5, overflowX: 'auto', paddingBottom: 2, scrollbarWidth: 'thin' }}>
          {items.slice(0, CHIPS_SHOWN).map((item, index) => (
            <Chip key={item.key} {...itemProps(item, index)} />
          ))}
          {items.length > CHIPS_SHOWN && <span style={{ ...S.faint, alignSelf: 'center', flex: 'none' }}>{t('dock.more', items.length - CHIPS_SHOWN)}</span>}
        </div>
      )}
      {(!current || items.length === 0) && (
        <span style={{ ...S.row, gap: 6, ...S.faint }}>
          <Icon name="plus" size={12} />
          {t('dock.empty')}
        </span>
      )}
      <span aria-live="polite" style={saying ? { ...S.faint, color: saying.failed ? T.warn : T.muted } : S.hidden}>
        {saying ? (saying.failed ? t('dock.failed', saying.text) : saying.text) : ''}
      </span>
    </section>
  )
}
