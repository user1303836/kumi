// The Make panel: a new sound from the picked sounds or the palette, by Kumi's sound-design engine. Tabs for the way of
// making it (Cross: blend, attack, resonate, modulate; Fuse: texture, smear; Loop: a beat or a melody; Stack), the
// mode's own choices (the rest come from the seed), the steer every way takes (kept per mode), and the results as
// playable waveforms: Reroll makes the same take again with another seed and keeps every variation, Keep puts one in
// the palette (its waveform flies into the dock), Use puts a request for it in the chat, and chosen results become the
// sounds to make from next (generation after generation; Back returns to the last ones). Making a sound changes no
// project, so it runs at once (`POST /design`). Both UI halves use it; what Use does comes from `use`. Everything it
// draws is its own: it reads only its own elements and the dock's, through refs, never the app's markup (catalog rule 8).
import { Button, Tip } from '@kumi/controls'
import { T } from '@kumi/theme'
import type { ReactNode, RefObject } from 'react'
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'

import { rgbOf } from './atlas/color.ts'
import type { Share } from './atlas/lineage.ts'
import { LineageBar } from './atlas/LineageBar.tsx'
import { CARDS, cardNote, DEFAULT_BPM, type DesignOptions, type DesignRequest, type DesignResult, type DesignSource, type DesignTab, loopTiming, madeOf, requestFor, requestOf, reroll, resultName, sourceOfResult, stagesOf, type Steer, TABS, takeKey } from './design.ts'
import { clean, seconds } from './format.ts'
import { CardOptions, LoopSection, SteerSection } from './MakeOptions.tsx'
import type { Designed } from './store.ts'
import { FOCUSABLE, radioKeys, S, SEGMENT_TRACK, segmentStyle, TYPE, useEdgeFade, useT } from './ui.tsx'

export interface MakePanelProps {
  /** The sounds to make from, in order (the picks, or the palette's), with their names. */
  sources: DesignSource[]
  /** Each source's colour on the map (its group's), for its chip. */
  colorOf: (source: DesignSource) => string
  /** The project's tempo: a loop starts there (Auto lets the roll pick). */
  tempo?: number | null
  results: DesignResult[] | null
  onMake: (request: DesignRequest) => Promise<Designed>
  onClose: () => void
  playing: string | null
  onPlay: (id: string) => void
  onStop: () => void
  /** Put a result in the palette; true when it went in. */
  onKeep: (result: DesignResult) => Promise<boolean>
  /** Results the palette already holds. */
  kept: ReadonlySet<string>
  /** "Use…": Desktop drafts a chat request for the result, the dashboard copies it. */
  use?: (result: DesignResult) => ReactNode
  /** Where it sits: `inline`, a card in the page's flow (the dashboard's, under the map's card; its page scrolls);
   *  `side`, a sheet floating at the map's right, below its top bar (a wide pane: the map stays in view beside it);
   *  `bottom`, floating over the map's lower part (a narrow pane, where the palette under the map stays in view). */
  placement?: 'bottom' | 'inline' | 'side'
  /** The palette dock (its `landing`), where a kept sound's waveform flies to. */
  dock?: RefObject<HTMLElement | null>
  /** Make from other sounds (chosen results, or back to the last ones): the panel's `sources` become these. */
  onSources?: (sources: DesignSource[]) => void
  /** The families a result's sounds come from, as colours with their shares: its waveform wears them, left to right.
   *  Without it (or for a result of no known family), the accent. */
  lineage?: (result: DesignResult) => Share[]
  /** The pointer or the keyboard came onto a result (or left: null): its parents are ringed on the map. */
  onKin?: (result: DesignResult | null) => void
}

/** A kept sound's flight: from its waveform's box to the dock's, as the viewport saw them when Keep was pressed, and
 *  the waveform as drawn then. */
interface Flight {
  key: number
  from: DOMRect
  to: DOMRect
  image: string | null
}

// Each card's little picture of what it does, drawn in the card's colour: two sounds overlapping, a transient on a
// body, rings of resonance, a wave whose loudness another moves, grains, a loop, a long smear, and layers.
const GLYPHS: Record<string, ReactNode> = {
  blend: (
    <>
      <circle cx="11" cy="10" r="6.5" fill="currentColor" fillOpacity="0.35" />
      <circle cx="17" cy="10" r="6.5" fill="currentColor" fillOpacity="0.35" />
    </>
  ),
  attack: <path d="M2 17 L7 17 L8.5 2 L10 12 Q14 16 26 17" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinejoin="round" />,
  resonate: (
    <>
      <circle cx="9" cy="10" r="2" fill="currentColor" />
      <path d="M13 5 Q16 10 13 15 M17 3 Q21.5 10 17 17 M21 1.5 Q27 10 21 18.5" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
    </>
  ),
  modulate: <path d="M1 10 Q3 2 5 10 T9 10 Q11 5 13 10 T17 10 Q19 8 21 10 T25 10 Q26 9.5 27 10" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />,
  texture: (
    <>
      {[
        [3, 6],
        [8, 13],
        [12, 4],
        [16, 11],
        [20, 6],
        [24, 14],
        [6, 17],
        [18, 17],
        [25, 3]
      ].map(([x, y], index) => (
        <circle cx={x} cy={y} fill="currentColor" fillOpacity={0.45 + (index % 3) * 0.2} key={index} r={1.4 + (index % 2) * 0.6} />
      ))}
    </>
  ),
  loop: <path d="M8 6 H19 A4 4 0 0 1 19 14 H9 M11 11 L8 14 L11 17 M8 6 A4 4 0 0 0 6.5 13" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />,
  smear: <path d="M1 10 Q3 3 5 10 Q7 15 10 10 Q14 6 18 10 Q22 12.5 27 10" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />,
  stack: (
    <>
      <rect fill="currentColor" fillOpacity="0.85" height="3" rx="1.5" width="20" x="4" y="4" />
      <rect fill="currentColor" fillOpacity="0.6" height="3" rx="1.5" width="20" x="4" y="9" />
      <rect fill="currentColor" fillOpacity="0.35" height="3" rx="1.5" width="20" x="4" y="14" />
    </>
  )
}

function Glyph({ card, size = 20 }: { card: string; size?: number }) {
  return (
    <svg aria-hidden height={size} style={{ flex: 'none' }} viewBox="0 0 28 20" width={(size * 28) / 20}>
      {GLYPHS[card]}
    </svg>
  )
}

/** Each tab's picture: two sounds becoming one, a long wave, a loop, and layers. */
const TAB_GLYPH: Record<DesignTab, string> = { cross: 'blend', fuse: 'smear', loop: 'loop', stack: 'stack' }
/** The sources drawn as chips: every source goes in, but the chips wrap and the first few show (`SOURCES_FOLDED`), with
 *  the rest on request; a pick of thousands draws a few dozen at most (`SOURCE_CHIPS`), then says how many more. */
const SOURCES_FOLDED = 8
const SOURCE_CHIPS = 48

/** A kept sound flies into the palette: its waveform, as drawn, arcs from its card into the dock's row of chips,
 *  shrinking as it goes, while the palette's answer brings the chip in (it pops in as it lands). The panel draws it,
 *  fixed to the viewport; a fixed box sits against a transformed ancestor instead, so it measures where its (0, 0) fell
 *  and starts from the card's box from there. */
function Flying({ flight, onDone }: { flight: Flight; onDone: () => void }) {
  const ghost = useRef<HTMLDivElement | null>(null)
  // The panel re-renders while the sound plays and the palette answers; the flight mustn't start over each time.
  const done = useRef(onDone)
  done.current = onDone
  const [origin, setOrigin] = useState<{ left: number; top: number } | null>(null)
  useLayoutEffect(() => {
    const box = ghost.current?.getBoundingClientRect()
    setOrigin(box ? { left: box.left, top: box.top } : null)
  }, [])
  useEffect(() => {
    const element = ghost.current
    if (!element || !origin) return
    const { from, to } = flight
    // The chips' row is at the dock's foot: land a little in from its left, in the middle of that row.
    const dx = to.left + Math.min(64, to.width / 2) - (from.left + from.width / 2)
    const dy = to.bottom - Math.min(22, to.height / 2) - (from.top + from.height / 2)
    const animation = element.animate(
      [
        { transform: 'translate(0, 0) scale(1)', opacity: 1 },
        { transform: `translate(${dx * 0.45}px, ${dy * 0.45 - 36}px) scale(0.5)`, opacity: 0.95, offset: 0.5 },
        { transform: `translate(${dx}px, ${dy}px) scale(0.12)`, opacity: 0.3 }
      ],
      { duration: 640, easing: 'cubic-bezier(.35,.6,.4,1)', fill: 'forwards' }
    )
    animation.onfinish = () => done.current()
    return () => animation.cancel()
  }, [origin, flight])
  return (
    <div
      aria-hidden
      ref={ghost}
      style={{
        position: 'fixed',
        left: origin ? flight.from.left - origin.left : 0,
        top: origin ? flight.from.top - origin.top : 0,
        width: flight.from.width,
        height: flight.from.height,
        borderRadius: 8,
        pointerEvents: 'none',
        zIndex: 20,
        visibility: origin ? 'visible' : 'hidden',
        background: flight.image ? `center / 100% 100% no-repeat url(${flight.image})` : T.accent,
        boxShadow: `0 10px 28px -10px ${T.accent}`
      }}
    />
  )
}

/** A result's waveform, from the engine's 256 peaks, mirrored about the middle, in the colours of the families it was
 *  made from, left to right (`colors`; null, or none, is the accent); a line runs across it while it plays. `onCanvas`
 *  hears its canvas element, for Keep's flight to start from. */
function Wave({ peaks, playing, duration, colors, onCanvas }: { peaks: number[]; playing: boolean; duration: number; colors?: readonly (string | null)[]; onCanvas?: (element: HTMLCanvasElement | null) => void }) {
  const canvas = useRef<HTMLCanvasElement | null>(null)
  const box = useRef<HTMLDivElement | null>(null)
  const [width, setWidth] = useState(0)
  const [drawn, setDrawn] = useState(false)
  const [startedAt, setStartedAt] = useState<number | null>(null)
  const [now, setNow] = useState(0)
  useEffect(() => {
    const element = box.current
    if (!element) return
    const observer = new ResizeObserver(entries => setWidth(Math.floor(entries[0]?.contentRect.width ?? 0)))
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  // It draws itself in once, left to right.
  useEffect(() => {
    const frame = requestAnimationFrame(() => setDrawn(true))
    return () => cancelAnimationFrame(frame)
  }, [])
  useEffect(() => setStartedAt(playing ? performance.now() : null), [playing])
  useEffect(() => {
    if (startedAt === null) return
    const frame = requestAnimationFrame(setNow)
    return () => cancelAnimationFrame(frame)
  }, [startedAt, now])
  useEffect(() => {
    const element = canvas.current
    const context = element?.getContext('2d')
    if (!element || !context || width === 0) return
    const ratio = window.devicePixelRatio || 1
    const height = 56
    element.width = width * ratio
    element.height = height * ratio
    context.setTransform(ratio, 0, 0, ratio, 0, 0)
    context.clearRect(0, 0, width, height)
    // A canvas takes no CSS variable: a family's colour comes resolved, and the accent is the canvas's own CSS colour.
    const accent = getComputedStyle(element).color || '#4a90e2'
    const tints = (colors ?? []).map(color => (color && rgbOf(color) ? color : accent))
    const gradient = context.createLinearGradient(0, 0, width, 0)
    if (tints.length > 1) tints.forEach((tint, index) => gradient.addColorStop(index / (tints.length - 1), tint))
    else {
      // One colour: it thins toward the end, as it always has.
      gradient.addColorStop(0, tints[0] ?? accent)
      gradient.addColorStop(1, `color-mix(in srgb, ${tints[0] ?? accent} 55%, transparent)`)
    }
    context.fillStyle = gradient
    const bars = peaks.length || 1
    const step = width / bars
    for (let bar = 0; bar < peaks.length; bar++) {
      const tall = Math.max(1, peaks[bar] * (height - 4))
      context.fillRect(bar * step, (height - tall) / 2, Math.max(1, step - 0.4), tall)
    }
  }, [peaks, width, colors?.join()]) // eslint-disable-line react-hooks/exhaustive-deps
  const progress = startedAt !== null && duration > 0 ? Math.min(1, (now - startedAt) / 1000 / duration) : null
  return (
    <div ref={box} style={{ position: 'relative', height: 56, color: T.accent }}>
      <canvas
        aria-hidden
        ref={element => {
          canvas.current = element
          onCanvas?.(element)
        }}
        style={{ width: '100%', height: 56, display: 'block', clipPath: drawn ? 'inset(0 0 0 0)' : 'inset(0 100% 0 0)', transition: 'clip-path 520ms cubic-bezier(.2,.8,.2,1)' }}
      />
      {progress !== null && progress > 0 && progress < 1 && (
        <span aria-hidden style={{ position: 'absolute', top: 0, bottom: 0, left: `${progress * 100}%`, width: 2, borderRadius: 1, background: T.text, opacity: 0.8 }} />
      )}
    </div>
  )
}

/** A modulate result's stages, as its recipe names them, in a chain: how the sounds were modulated, always in view. */
function Stages({ stages }: { stages: string[] }) {
  const t = useT()
  return (
    <span aria-label={t('make.stages')} role="list" style={{ ...S.row, flexWrap: 'wrap', gap: 4 }}>
      {stages.map((stage, index) => (
        <span key={index} role="listitem" style={{ ...S.row, gap: 4 }}>
          {index > 0 && (
            <span aria-hidden style={{ ...TYPE.caption, color: T.faint }}>
              →
            </span>
          )}
          <span style={{ ...TYPE.caption, padding: '1px 7px', borderRadius: 8, background: `color-mix(in srgb, ${T.accent} 10%, transparent)`, border: `1px solid color-mix(in srgb, ${T.accent} 30%, transparent)`, color: T.secondary }}>
            {clean(stage, 80)}
          </span>
        </span>
      ))}
    </span>
  )
}

interface Take {
  key: string
  /** Its variations, oldest first. */
  results: DesignResult[]
  /** The request that made it here (options included), for Reroll; from history, its op, mode and sounds only. */
  request: DesignRequest
}

/** The side sheet's width in a map `area` pixels wide, as its style's `min(430px, 46%)` sets it. */
export const sideSheetWidth = (area: number): number => Math.min(430, area * 0.46)

/** The buttons inside one of the panel's own elements, in order: where the keyboard goes next (Hermes's Button takes
 *  no ref, so its element is found under the panel's own). */
const buttonsOf = (root: Element | null): HTMLButtonElement[] => (root ? [...root.children].flatMap(child => (child instanceof HTMLButtonElement ? [child] : buttonsOf(child))) : [])
const focus = (element: HTMLElement | null | undefined) => element?.focus({ preventScroll: true })

export function MakePanel({ sources, colorOf, tempo, results, onMake, onClose, playing, onPlay, onStop, onKeep, kept, use, placement = 'bottom', dock, onSources, lineage, onKin }: MakePanelProps) {
  const inline = placement === 'inline'
  const t = useT()
  // The panel's results go with it, and so do the map's rings for the one the pointer or the keyboard was on.
  const tellKin = useRef(onKin)
  tellKin.current = onKin
  useEffect(() => () => tellKin.current?.(null), [])
  const [cardKey, setCardKey] = useState('blend')
  // The mode each tab had last, so going back to Cross finds Attack where it was left.
  const [lastMode, setLastMode] = useState<Record<DesignTab, string>>({ cross: 'blend', fuse: 'texture', loop: 'loop', stack: 'stack' })
  const [options, setOptions] = useState<DesignOptions>({})
  // The steer, kept per mode: Blend's stays Blend's when the producer tries Resonate and comes back.
  const [steers, setSteers] = useState<Record<string, Steer>>({})
  const [steerOpen, setSteerOpen] = useState(false)
  // Whether every source shows as a chip, or the first few with "+N more".
  const [allSources, setAllSources] = useState(false)
  // The first chip "+N more" brings in: the keyboard goes there, as the button that showed it now folds them again.
  const firstMore = useRef<HTMLSpanElement | null>(null)
  const toMore = useRef(false)
  useEffect(() => {
    if (!allSources || !toMore.current) return
    toMore.current = false
    focus(firstMore.current)
    reveal(firstMore.current, 'nearest')
  }, [allSources]) // eslint-disable-line react-hooks/exhaustive-deps
  // Results chosen to make from next, in the order they were chosen; and the sounds made from before, for Back.
  const [chosen, setChosen] = useState<string[]>([])
  const [trail, setTrail] = useState<DesignSource[][]>([])
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  // The requests this window sent, by take: a reroll repeats them exactly (their options too).
  const requests = useRef(new Map<string, DesignRequest>())
  // Which variation of each take shows (its index), and which results arrived while the panel was open.
  const [shown, setShown] = useState<Record<string, number>>({})
  // The results whose names read in full: a click or Enter on a name opens it in place (two lines otherwise), and folds it again.
  const [openNames, setOpenNames] = useState<ReadonlySet<string>>(() => new Set())
  const [fresh, setFresh] = useState<ReadonlySet<string>>(() => new Set())
  // The sound just made, brought into view once it's drawn (the cards above it can fill a narrow pane).
  const [arrived, setArrived] = useState<string | null>(null)
  // Each shown result's card, by its sound's id: where a new one scrolls to; and its waveform's canvas, where a kept
  // one flies from.
  const cards = useRef(new Map<string, HTMLDivElement>())
  const canvases = useRef(new Map<string, HTMLCanvasElement>())
  // Each result's Choose, by its sound's id: where the keyboard goes back to when the choice is cleared.
  const chooses = useRef(new Map<string, HTMLButtonElement>())
  // The panel's own scrolling list. A card comes into view by scrolling it alone: `scrollIntoView` would scroll every
  // ancestor too, the pane the map sits in among them.
  const list = useRef<HTMLDivElement | null>(null)
  useEdgeFade(list)
  const reveal = (element: HTMLElement | null | undefined, where: 'nearest' | 'start') => {
    const box = list.current
    if (!box || !element) return
    const [outer, inner] = [box.getBoundingClientRect(), element.getBoundingClientRect()]
    let top = box.scrollTop
    if (where === 'start') top = 0
    else if (inner.top < outer.top) top += inner.top - outer.top - 8
    else if (inner.bottom > outer.bottom) top += inner.bottom - outer.bottom + 8
    else return
    box.scrollTo({ top, behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' })
  }
  useEffect(() => {
    if (!arrived) return
    reveal(cards.current.get(arrived), 'nearest')
    setArrived(null)
  }, [arrived, results]) // eslint-disable-line react-hooks/exhaustive-deps
  // The result being worked on (kept, made, or holding the keyboard) stays whole in view while the room around it
  // changes: the list's own box (the foot grows with chosen results; the first Keep's dock takes room from the map, so
  // from the panel) and the results in it (a kept card's row rewraps as Keep becomes "In palette"). Taller than the
  // list, it shows its bottom, where its actions are. It lets go when the producer scrolls the list, or takes the
  // keyboard elsewhere in it.
  const held = useRef<string | null>(null)
  // Measured by layout (the list is the cards' offset parent), so a card still popping in, scaled, measures whole.
  const hold = () => {
    const box = list.current
    const card = held.current ? cards.current.get(held.current) : undefined
    if (!box || !card) return
    const [top, bottom] = [card.offsetTop, card.offsetTop + card.offsetHeight]
    if (bottom > box.scrollTop + box.clientHeight - 8 || card.offsetHeight > box.clientHeight - 16) box.scrollTop = bottom - box.clientHeight + 8
    else if (top < box.scrollTop + 8) box.scrollTop = top - 8
  }
  // One observer for the list and the results, made at the first of them; a stable callback, so a render observes
  // nothing again.
  const watcher = useRef<ResizeObserver | null>(null)
  const watch = useRef((element: Element | null) => {
    if (!element) return
    watcher.current ??= new ResizeObserver(() => hold())
    watcher.current.observe(element)
  }).current
  useEffect(() => {
    watch(list.current)
    return () => watcher.current?.disconnect()
  }, []) // eslint-disable-line react-hooks/exhaustive-deps
  // Where the keyboard goes after an action that takes its control away, once the panel has drawn what follows: Make
  // after the sounds change, the chosen results' action after Back, a Choose after Clear.
  const makeRow = useRef<HTMLDivElement | null>(null)
  const chosenBar = useRef<HTMLDivElement | null>(null)
  const focusNext = useRef<(() => void) | null>(null)
  useEffect(() => {
    const next = focusNext.current
    focusNext.current = null
    next?.()
  })
  const [flights, setFlights] = useState<Flight[]>([])
  const flightKey = useRef(0)
  /** Keep's flight from the card's waveform into the dock; none for a system that asks for less motion, or with no
   *  dock on screen. */
  const fly = (id: string) => {
    const canvas = canvases.current.get(id)
    const to = dock?.current?.getBoundingClientRect()
    if (!canvas || !to || to.width === 0 || window.matchMedia('(prefers-reduced-motion: reduce)').matches) return
    const key = ++flightKey.current
    setFlights(current => [...current, { key, from: canvas.getBoundingClientRect(), to, image: canvas.toDataURL() }])
  }
  const [open, setOpen] = useState(false)
  // Make opens only from a click or a key on something of the map's ("Make…" on the picks or the palette): the keyboard
  // comes with it, to the panel itself, and goes back to what opened it when it closes (if that's still there).
  const panel = useRef<HTMLElement | null>(null)
  const opener = useRef<HTMLElement | null>(null)
  useEffect(() => {
    panel.current?.focus({ preventScroll: true })
    const frame = requestAnimationFrame(() => setOpen(true))
    return () => cancelAnimationFrame(frame)
  }, [])
  const close = () => {
    const back = opener.current
    onClose()
    if (back?.isConnected) back.focus({ preventScroll: true })
  }
  const card = CARDS.find(entry => entry.key === cardKey) ?? CARDS[0]
  const note = cardNote(sources.length)
  const refused = note !== null
  // A loop starts at the DAW's tempo (A6), the engine's 120 with none.
  const dawTempo = typeof tempo === 'number' && tempo >= 40 && tempo <= 240 ? Math.round(tempo) : DEFAULT_BPM
  const steer = steers[card.key] ?? {}

  // The results as takes, newest take first: a reroll joins its take.
  const takes = useMemo(() => {
    const byKey = new Map<string, Take>()
    for (const result of [...(results ?? [])].reverse()) {
      const key = takeKey(result)
      const take = byKey.get(key)
      if (take) take.results.push(result)
      else byKey.set(key, { key, results: [result], request: requests.current.get(key) ?? requestOf(result) })
    }
    return [...byKey.values()].reverse()
  }, [results])
  const byId = useMemo(() => new Map((results ?? []).map(result => [result.artifact_id, result])), [results])
  const chosenResults = chosen.flatMap(id => (byId.has(id) ? [byId.get(id) as DesignResult] : []))
  // What the chosen results can do: be the sounds alone (two or more), or join the sounds there are (any new to them).
  const chosenAlone = cardNote(chosenResults.length) === null
  const chosenNew = chosenResults.filter(result => !sources.some(source => source.artifact_id === result.artifact_id))
  const chosenJoin = chosenNew.length > 0 && cardNote(sources.length + chosenNew.length) === null
  /** A result's name on its card, in the producer's words: how it was made and from what ("Blend of Kick 02 + Tom 4"),
   *  a grown one by how far it went between its two. Its recipe is in the name's tip. */
  const nameOf = (result: DesignResult): string => {
    if (result.op === 'grow') {
      const [from, to] = result.sources.map(source => clean(source.name ?? '', 40))
      return typeof result.at === 'number' && from && to ? t('make.grownName', Math.round(result.at * 100), from, to) : resultName(result)
    }
    const { way, named, more } = madeOf(result)
    const word = t(`make.ways.${way}`)
    return named.length > 0 ? t('make.madeName', word, clean(named[0], 40), named[1] ? clean(named[1], 40) : '', more) : t('make.madeCount', word, result.sources.length)
  }

  const make = async (request: DesignRequest, label: string) => {
    setBusy(label)
    setError(null)
    const answer = await onMake(request)
    setBusy(null)
    if ('result' in answer) {
      const key = takeKey(answer.result)
      requests.current.set(key, request)
      setShown(current => ({ ...current, [key]: Number.MAX_SAFE_INTEGER }))
      setFresh(current => new Set([...current, answer.result.artifact_id]))
      // It comes into view, and is the one held there from now on.
      setArrived(answer.result.artifact_id)
      held.current = answer.result.artifact_id
      // A new sound plays as it arrives, as in Kumi's TUI.
      onPlay(answer.result.artifact_id)
      return
    }
    // The backend says why in a sentence of its own: a refusal (400: a sound picked on an older map, one it can't
    // read), two renders already running or the runtime not set up (503), or a render past two minutes (504). Only an
    // older backend with no sound design (404) and a failure with no words get the panel's own.
    setError(answer.status === 404 ? t('make.missing') : [400, 503, 504].includes(answer.status) && answer.error ? answer.error : t('make.failed', answer.error || String(answer.status)))
  }
  /** The panel's sounds become `next`; the ones it had wait for Back. The keyboard goes to Make. */
  const makeFrom = (next: DesignSource[]) => {
    if (!onSources || next.length === 0) return
    setTrail(current => [...current, sources].slice(-8))
    setChosen([])
    onSources(next)
    reveal(list.current, 'start')
    focusNext.current = () => focus(buttonsOf(makeRow.current)[0])
  }
  /** Back to the sounds before these: the results that made these are chosen again (Back undoes "Make from these"),
   *  and the keyboard goes back to the chosen results' action; with none, to the panel. */
  const back = () => {
    if (!onSources || trail.length === 0) return
    const previous = trail[trail.length - 1]
    const again = sources.flatMap(source => (source.artifact_id && byId.has(source.artifact_id) && !previous.some(other => other.artifact_id === source.artifact_id) ? [source.artifact_id] : []))
    setTrail(current => current.slice(0, -1))
    setChosen(again)
    onSources(previous)
    focusNext.current = () => focus(buttonsOf(chosenBar.current).at(-1) ?? panel.current)
  }

  // The way to make it, in two steps that take little room beside the chat: the tab, then its modes, and a line saying
  // what the chosen one does. Both in Hermes's segmented look.
  const track = SEGMENT_TRACK
  const segment = (picked: boolean) => segmentStyle(picked, refused)
  const modes = CARDS.filter(entry => entry.tab === card.tab)
  const chooser = (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <div aria-label={t('make.way')} role="radiogroup" style={{ ...track, display: 'grid', gridTemplateColumns: `repeat(${TABS.length}, 1fr)` }}>
        {TABS.map((tab, index) => {
          const picked = card.tab === tab
          return (
            <Tip key={tab} label={t(`make.opHints.${tab}`)}>
              <button
                aria-checked={picked}
                className={FOCUSABLE}
                disabled={refused}
                onClick={() => setCardKey(lastMode[tab])}
                {...radioKeys(index, TABS.length, TABS.indexOf(card.tab), next => setCardKey(lastMode[TABS[next]]))}
                role="radio"
                type="button"
                style={segment(picked)}
              >
                <Glyph card={TAB_GLYPH[tab]} size={14} />
                <span style={S.ellipsis}>{t(`make.ops.${tab}`)}</span>
              </button>
            </Tip>
          )
        })}
      </div>
      {modes.length > 1 && (
        <div aria-label={t(`make.ops.${card.tab}`)} role="radiogroup" style={{ ...track, alignSelf: 'flex-start' }}>
          {modes.map((entry, index) => {
            const picked = entry.key === card.key
            const choose = (chosen: (typeof modes)[number]) => {
              setCardKey(chosen.key)
              setLastMode(current => ({ ...current, [chosen.tab]: chosen.key }))
            }
            return (
              <Tip key={entry.key} label={t(`make.cards.${entry.key}.hint`)}>
                <button
                  aria-checked={picked}
                  className={FOCUSABLE}
                  disabled={refused}
                  onClick={() => choose(entry)}
                  {...radioKeys(index, modes.length, modes.findIndex(each => each.key === card.key), next => choose(modes[next]))}
                  role="radio"
                  type="button"
                  style={segment(picked)}
                >
                  <Glyph card={entry.key} size={13} />
                  {t(`make.cards.${entry.key}.title`)}
                </button>
              </Tip>
            )
          })}
        </div>
      )}
      <span style={{ ...TYPE.small, color: T.muted }}>{t(`make.cards.${card.key}.hint`)}</span>
    </div>
  )

  return (
    <section
      aria-label={t('make.title')}
      onFocus={event => {
        // The first focus is the one Make took as it opened: what had it before opened Make.
        if (opener.current === null && event.target === event.currentTarget && event.relatedTarget instanceof HTMLElement) opener.current = event.relatedTarget
      }}
      // Escape closes Make from anywhere in it, and only then: it's the map's to take (past the map, Desktop's Escape
      // stops the chat's turn, and stands down for a key already taken).
      onKeyDown={event => {
        if (event.key !== 'Escape' || event.defaultPrevented || event.shiftKey || event.altKey || event.ctrlKey || event.metaKey) return
        event.preventDefault()
        close()
      }}
      ref={panel}
      tabIndex={-1}
      style={{
        // It holds the focus only as it opens (Tab goes on to its controls, which show the ring): no ring round it all.
        outline: 'none',
        ...(placement === 'side'
          ? { position: 'absolute', right: 8, top: 52, bottom: 8, width: 'min(430px, 46%)' }
          : placement === 'bottom'
            ? { position: 'absolute', left: 8, right: 8, bottom: 8, maxHeight: '82%' }
            : {}),
        display: 'flex',
        flexDirection: 'column',
        borderRadius: 12,
        border: `1px solid ${T.border}`,
        background: T.raised,
        boxShadow: '0 18px 48px -18px rgba(0,0,0,.55)',
        color: T.text,
        transform: open ? 'none' : placement === 'side' ? 'translateX(24px)' : 'translateY(24px)',
        opacity: open ? 1 : 0,
        transition: 'transform 260ms cubic-bezier(.2,.8,.2,1), opacity 200ms',
        zIndex: inline ? undefined : 2
      }}
    >
      <div style={{ ...S.row, gap: 8, padding: '8px 8px 8px 12px', borderBottom: `1px solid ${T.hairline}` }}>
        <span style={{ ...TYPE.body, fontWeight: 600 }}>{t('make.title')}</span>
        <span style={{ ...TYPE.small, color: T.faint }}>{t('make.from', sources.length)}</span>
        <span style={{ flex: 1 }} />
        {trail.length > 0 && onSources && (
          <Button kind="quiet" onClick={back} tip={t('make.backTitle', trail[trail.length - 1].length)}>
            <span aria-hidden>‹</span>
            {t('make.back')}
          </Button>
        )}
        <Button kind="quiet" label={t('make.close')} onClick={close}>
          ×
        </Button>
      </div>
      {/* The options and the results scroll; Make stays in the foot below them, in view however far they run. Positioned,
          so the screen-reader-only words inside (absolutely placed) scroll and clip with the list instead of hanging
          below the pane, which gave the pane room to scroll. Scrolling it by hand (a wheel, a touch, its scroll bar) lets
          go of the result held in view, and so does the keyboard going to anything in it outside that result. */}
      <div
        onFocus={event => {
          if (!(held.current && cards.current.get(held.current)?.contains(event.target))) held.current = null
        }}
        onPointerDown={event => {
          if (event.target === event.currentTarget) held.current = null
        }}
        onTouchMove={() => (held.current = null)}
        onWheel={() => (held.current = null)}
        ref={list}
        style={{ position: 'relative', flex: '1 1 auto', minHeight: 0, overflowY: 'auto', padding: '8px 12px 12px', display: 'flex', flexDirection: 'column', gap: 10 }}
      >
        {/* The sounds that go in (all of them): the first few as chips that wrap, whole (a sound Kumi made marked as one),
            then the rest on request. Nothing is cut at the panel's edge. */}
        <div aria-label={t('make.sources')} role="list" style={{ ...S.row, flex: 'none', flexWrap: 'wrap', gap: 4 }}>
          {sources.slice(0, allSources ? SOURCE_CHIPS : SOURCES_FOLDED).map((source, index) => (
            <span
              className={index === SOURCES_FOLDED ? FOCUSABLE : undefined}
              key={`${index}-${source.sound_id ?? source.artifact_id}`}
              ref={index === SOURCES_FOLDED ? firstMore : undefined}
              role="listitem"
              style={{ ...S.row, gap: 4, ...TYPE.caption, padding: '2px 6px', borderRadius: 3, background: T.chip, boxShadow: source.artifact_id ? `inset 0 0 0 1px color-mix(in srgb, ${T.accent} 45%, transparent)` : undefined, color: T.secondary, maxWidth: '100%' }}
              tabIndex={index === SOURCES_FOLDED ? -1 : undefined}
            >
              {source.artifact_id ? (
                <span aria-label={t('make.made')} role="img" style={{ ...TYPE.caption, color: T.accent, flex: 'none' }}>
                  ✦
                </span>
              ) : (
                <span style={{ width: 7, height: 7, borderRadius: '50%', background: colorOf(source), flex: 'none' }} />
              )}
              <span style={{ minWidth: 0, overflowWrap: 'anywhere' }}>{clean(source.name ?? source.sound_id ?? source.artifact_id ?? '', 60)}</span>
            </span>
          ))}
          {allSources && sources.length > SOURCE_CHIPS && <span style={{ ...TYPE.caption, color: T.faint, flex: 'none' }}>{t('make.moreSources', sources.length - SOURCE_CHIPS)}</span>}
          {/* One button that shows the rest and folds them again, so the keyboard never loses it. */}
          {sources.length > SOURCES_FOLDED && (
            <Button
              expanded={allSources}
              kind="quiet"
              onClick={() => {
                toMore.current = !allSources
                setAllSources(!allSources)
              }}
            >
              {allSources ? t('make.fewerSources') : t('make.moreSources', sources.length - SOURCES_FOLDED)}
            </Button>
          )}
        </div>
        {chooser}
        {card.key === 'loop' ? <LoopSection onChange={setOptions} options={options} tempo={dawTempo} /> : <CardOptions card={card} onChange={setOptions} options={options} sources={sources} />}
        <SteerSection onChange={next => setSteers(current => ({ ...current, [card.key]: next }))} onToggle={() => setSteerOpen(!steerOpen)} open={steerOpen} steer={steer} />
        {takes.length > 0 && (
          <div aria-label={t('make.results')} ref={watch} role="list" style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {takes.map(take => {
              const at = Math.min(take.results.length - 1, shown[take.key] ?? take.results.length - 1)
              const result = take.results[at]
              const isPlaying = playing === result.artifact_id
              const inPalette = kept.has(result.artifact_id)
              const isChosen = chosen.includes(result.artifact_id)
              const stages = result.mode === 'modulate' ? stagesOf(result.recipe) : []
              const timing = loopTiming(result)
              const name = nameOf(result)
              return (
                <div
                  className={fresh.has(result.artifact_id) ? 'kumi-pop' : undefined}
                  ref={element => {
                    if (element) cards.current.set(result.artifact_id, element)
                    else cards.current.delete(result.artifact_id)
                  }}
                  key={take.key}
                  onBlur={() => onKin?.(null)}
                  onFocus={() => {
                    held.current = result.artifact_id
                    onKin?.(result)
                  }}
                  onPointerEnter={() => onKin?.(result)}
                  onPointerLeave={() => onKin?.(null)}
                  role="listitem"
                  style={{
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 5,
                    padding: '8px 10px',
                    borderRadius: 10,
                    border: `1px solid ${isChosen || isPlaying ? T.accent : T.hairline}`,
                    background: isChosen ? `color-mix(in srgb, ${T.accent} 10%, transparent)` : isPlaying ? `color-mix(in srgb, ${T.accent} 6%, transparent)` : 'transparent',
                    boxShadow: isChosen ? `0 0 0 1px ${T.accent}` : 'none',
                    transition: 'border-color 160ms, background-color 160ms, box-shadow 160ms'
                  }}
                >
                  {/* Its lineage along its top edge: the families it came from, each as wide as its share. */}
                  {lineage && <LineageBar shares={lineage(result)} />}
                  <div style={{ display: 'flex', alignItems: 'flex-start', gap: 8 }}>
                    {/* What it is, in a producer's words, two lines at most until a click or Enter opens it whole (and
                        folds it again); how the engine made it (its recipe) in the tip, which the keyboard reaches too. A
                        modulate result names its stages below. */}
                    <Tip label={clean(result.recipe, 400)}>
                      <button
                        aria-expanded={openNames.has(result.artifact_id)}
                        className={FOCUSABLE}
                        onClick={() =>
                          setOpenNames(current => {
                            const next = new Set(current)
                            if (!next.delete(result.artifact_id)) next.add(result.artifact_id)
                            return next
                          })
                        }
                        type="button"
                        style={{
                          font: 'inherit',
                          ...TYPE.body,
                          flex: 1,
                          minWidth: 0,
                          minHeight: 24,
                          padding: 0,
                          border: 'none',
                          background: 'transparent',
                          color: T.text,
                          textAlign: 'left',
                          cursor: 'pointer',
                          overflowWrap: 'anywhere',
                          ...(openNames.has(result.artifact_id) ? {} : { display: '-webkit-box', WebkitLineClamp: 2, WebkitBoxOrient: 'vertical', overflow: 'hidden' })
                        }}
                      >
                        {name}
                      </button>
                    </Tip>
                    {/* Choose: a 24 px target round a 20 px circle. */}
                    {onSources && (
                      <Tip label={t('make.chooseTitle')}>
                        <button
                          aria-label={t('make.choose', name)}
                          aria-pressed={isChosen}
                          className={FOCUSABLE}
                          onClick={() => setChosen(current => (current.includes(result.artifact_id) ? current.filter(id => id !== result.artifact_id) : [...current, result.artifact_id]))}
                          ref={element => {
                            if (element) chooses.current.set(result.artifact_id, element)
                            else chooses.current.delete(result.artifact_id)
                          }}
                          type="button"
                          style={{ flex: 'none', width: 24, height: 24, margin: -2, display: 'flex', alignItems: 'center', justifyContent: 'center', padding: 0, border: 'none', borderRadius: '50%', background: 'transparent', cursor: 'pointer' }}
                        >
                          <span
                            aria-hidden
                            style={{
                              width: 20,
                              height: 20,
                              boxSizing: 'border-box',
                              display: 'flex',
                              alignItems: 'center',
                              justifyContent: 'center',
                              borderRadius: '50%',
                              border: `1.5px solid ${isChosen ? T.accent : T.faint}`,
                              background: isChosen ? T.accent : 'transparent',
                              color: isChosen ? T.raised : 'transparent',
                              ...TYPE.caption,
                              fontWeight: 600,
                              transform: isChosen ? 'scale(1.08)' : 'none',
                              transition: 'background-color 140ms, border-color 140ms, transform 160ms'
                            }}
                          >
                            ✓
                          </span>
                        </button>
                      </Tip>
                    )}
                  </div>
                  {stages.length > 0 && <Stages stages={stages} />}
                  {/* A round play button by the waveform; the waveform plays on a click too. */}
                  <div style={{ ...S.row, gap: 8 }}>
                    <button
                      aria-label={isPlaying ? t('make.stop') : t('make.play')}
                      className={FOCUSABLE}
                      onClick={() => (isPlaying ? onStop() : onPlay(result.artifact_id))}
                      type="button"
                      style={{
                        flex: 'none',
                        width: 30,
                        height: 30,
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: 'center',
                        padding: 0,
                        borderRadius: '50%',
                        border: `1px solid ${T.accent}`,
                        background: isPlaying ? `color-mix(in srgb, ${T.accent} 30%, transparent)` : `color-mix(in srgb, ${T.accent} 12%, transparent)`,
                        color: T.accent,
                        ...TYPE.caption,
                        cursor: 'pointer',
                        transition: 'background-color 140ms'
                      }}
                    >
                      {isPlaying ? '■' : '▶'}
                    </button>
                    <div aria-hidden onClick={() => (isPlaying ? onStop() : onPlay(result.artifact_id))} style={{ flex: 1, minWidth: 0, cursor: 'pointer' }}>
                      <Wave
                        colors={lineage?.(result).map(part => part.color)}
                        duration={result.duration}
                        onCanvas={element => {
                          if (element) canvases.current.set(result.artifact_id, element)
                          else canvases.current.delete(result.artifact_id)
                        }}
                        peaks={result.peaks}
                        playing={isPlaying}
                      />
                    </div>
                  </div>
                  <div style={{ ...S.row, gap: 6, flexWrap: 'wrap' }}>
                    {/* The take's facts; its seed is in their tip, for anyone who looks, not on the face (K2). A loop
                        also says its bars and tempo: its name says only how and from what, and in time with what is
                        what a producer needs of a loop. */}
                    <Tip label={`#${result.seed} · ${t('make.seedTitle')}`}>
                      <span style={{ ...TYPE.small, color: T.faint }}>
                        {[
                          t(`make.ops.${result.mode === 'loop' ? 'loop' : result.op}`),
                          ...(timing ? [t('make.optionValues.bars', timing.bars), timing.meter, t('make.optionValues.bpm', timing.bpm)] : []),
                          seconds(result.duration)
                        ]
                          .filter(Boolean)
                          .join(' · ')}
                      </span>
                    </Tip>
                    <span style={{ flex: 1 }} />
                    {take.results.length > 1 && (
                      <span role="group" aria-label={t('make.variations')} style={{ ...S.row, gap: 2 }}>
                        <Button disabled={at === 0} kind="quiet" label={t('make.previous')} onClick={() => setShown(current => ({ ...current, [take.key]: at - 1 }))}>
                          ‹
                        </Button>
                        <span style={{ ...TYPE.small, color: T.faint, fontVariantNumeric: 'tabular-nums' }}>{t('make.variation', at + 1, take.results.length)}</span>
                        <Button disabled={at === take.results.length - 1} kind="quiet" label={t('make.next')} onClick={() => setShown(current => ({ ...current, [take.key]: at + 1 }))}>
                          ›
                        </Button>
                      </span>
                    )}
                  </div>
                  {/* One order for a result's actions: the quiet one first, the usual ones at the right edge. Each stays
                      where it is as it works, so the keyboard stays on it: Reroll's new variation shows in this card, and
                      Keep becomes "In palette" in place. */}
                  <div style={{ ...S.row, gap: 6, flexWrap: 'wrap' }}>
                    <Button disabled={busy !== null && busy !== take.key} kind="quiet" onClick={() => make(reroll(take.request), take.key)} tip={t('make.rerollTitle')}>
                      {busy === take.key ? (
                        t('make.rerolling')
                      ) : (
                        <>
                          <span aria-hidden>↻</span>
                          {t('make.reroll')}
                        </>
                      )}
                    </Button>
                    <span style={{ flex: 1 }} />
                    <Button
                      onClick={() => {
                        if (inPalette) return
                        held.current = result.artifact_id
                        fly(result.artifact_id)
                        return onKeep(result)
                      }}
                      pressed={inPalette}
                      // Always a tip, so the button keeps its place in the tree as Keep becomes "In palette" (a tip
                      // that comes and goes wraps it anew, and the keyboard would lose it).
                      tip={inPalette ? t('make.keptTitle') : t('make.keepTitle')}
                    >
                      <span aria-hidden>{inPalette ? '✓' : '+'}</span>
                      {inPalette ? t('make.kept') : t('make.keep')}
                    </Button>
                    {use?.(result)}
                  </div>
                </div>
              )
            })}
          </div>
        )}
        {takes.length === 0 && results !== null && <span style={S.faint}>{t('make.none')}</span>}
      </div>
      {/* The foot, under the list and in view however far it scrolls: the results chosen to make from next and what they
          can do, then Make. */}
      <div style={{ flex: 'none', borderTop: `1px solid ${T.hairline}`, background: T.raised, borderRadius: '0 0 12px 12px', ...(inline ? { position: 'sticky', bottom: 0 } : {}) }}>
        {onSources && chosenResults.length > 0 && (
          <div
            aria-label={t('make.chosen', chosenResults.length)}
            className="kumi-pop"
            ref={chosenBar}
            role="group"
            style={{ ...S.row, flexWrap: 'wrap', gap: 6, padding: '8px 12px', borderBottom: `1px solid ${T.hairline}`, background: `color-mix(in srgb, ${T.accent} 7%, ${T.raised})` }}
          >
            <span style={{ ...TYPE.body, fontWeight: 600 }}>{t('make.chosen', chosenResults.length)}</span>
            {/* One alone can't be made from: it says what makes it so, and offers only what it can do. */}
            <span style={{ ...TYPE.small, color: T.muted, flex: '1 1 6rem' }}>{chosenAlone ? '' : t('make.oneChosen')}</span>
            <Button
              kind="quiet"
              onClick={() => {
                const first = chosenResults[0]?.artifact_id
                setChosen([])
                focusNext.current = () => focus(first ? chooses.current.get(first) : undefined)
              }}
              tip={t('make.clearChosenTitle')}
            >
              {t('make.clearChosen')}
            </Button>
            {chosenJoin && (
              <Button kind={chosenAlone ? 'secondary' : 'primary'} onClick={() => makeFrom([...sources, ...chosenNew.map(result => sourceOfResult(result))])} tip={t('make.addToSoundsTitle')}>
                {t('make.addToSounds')}
              </Button>
            )}
            {chosenAlone && (
              <Button kind="primary" onClick={() => makeFrom(chosenResults.map(result => sourceOfResult(result)))} tip={t('make.fromTheseTitle')}>
                {t('make.fromThese')}
              </Button>
            )}
          </div>
        )}
        <div ref={makeRow} style={{ ...S.row, gap: 8, flexWrap: 'wrap', padding: '8px 12px' }}>
          {/* The one primary action of the panel, a size up from the rest (Hermes's 28 px). It stays enabled while it
              makes (a request's button sends once), so the keyboard stays on it. */}
          <Button
            disabled={refused || (busy !== null && busy !== 'make')}
            kind="primary"
            onClick={() => make(requestFor(card, sources, { ...options, ...(card.key === 'loop' ? { bpm: options.bpm ?? dawTempo } : {}), steer }), 'make')}
            size="md"
            tip={t('make.makeTitle', t(`make.cards.${card.key}.title`))}
          >
            {busy === 'make' ? t('make.making') : `${t('make.make')} · ${t(`make.cards.${card.key}.title`)}`}
          </Button>
          {busy === 'make' && <span aria-hidden className="kumi-spin" style={{ width: 14, height: 14, borderRadius: '50%', border: `2px solid ${T.accent}`, borderRightColor: 'transparent' }} />}
          {note && <span style={S.faint}>{t(note.key, note.least)}</span>}
          <span aria-live="polite" style={error ? { ...S.faint, color: T.warn, flex: '1 1 14rem' } : S.hidden}>
            {error ?? ''}
          </span>
        </div>
      </div>
      {flights.map(flight => (
        <Flying flight={flight} key={flight.key} onDone={() => setFlights(current => current.filter(other => other.key !== flight.key))} />
      ))}
    </section>
  )
}
