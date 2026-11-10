// The Make panel: a new sound from the picked sounds or the palette, by Kumi's sound-design engine. Tabs for the way of
// making it (Cross: blend, attack, resonate, modulate; Fuse: texture, smear; Loop: a beat or a melody; Stack), the
// mode's own choices (the rest come from the seed), the steer every way takes (kept per mode), and the results as
// playable waveforms: Reroll makes the same take again with another seed and keeps every variation, Keep puts one in
// the palette (its waveform flies into the dock), Use puts a request for it in the chat, and chosen results become the
// sounds to make from next (generation after generation; Back returns to the last ones). Making a sound changes no
// project, so it runs at once (`POST /design`). Both UI halves use it; what Use does comes from `use`. Everything it
// draws is its own: it reads only its own elements and the dock's, through refs, never the app's markup (catalog rule 8).
import { T } from '@kumi/theme'
import type { ReactNode, RefObject } from 'react'
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'

import { rgbOf } from './atlas/color.ts'
import type { Share } from './atlas/lineage.ts'
import { CARDS, cardNote, DEFAULT_BPM, type DesignOptions, type DesignRequest, type DesignResult, type DesignSource, type DesignTab, requestFor, requestOf, reroll, resultName, sourceOfResult, stagesOf, type Steer, TABS, takeKey } from './design.ts'
import { clean, seconds } from './format.ts'
import { CardOptions, LoopSection, SteerSection } from './MakeOptions.tsx'
import type { Designed } from './store.ts'
import { FOCUSABLE, S, TextButton, useT } from './ui.tsx'

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
/** The sources drawn as chips: every source goes in, but a pick of thousands draws a few dozen, then says how many
 *  more. */
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
 *  made from, left to right (`colors`; null, or none, is the accent); a line runs across it while it plays. */
function Wave({ peaks, playing, duration, colors }: { peaks: number[]; playing: boolean; duration: number; colors?: readonly (string | null)[] }) {
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
        ref={canvas}
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
            <span aria-hidden style={{ color: T.faint, fontSize: 11 }}>
              →
            </span>
          )}
          <span style={{ fontSize: 11, padding: '1px 7px', borderRadius: 8, background: `color-mix(in srgb, ${T.accent} 10%, transparent)`, border: `1px solid color-mix(in srgb, ${T.accent} 30%, transparent)`, color: T.secondary }}>
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

export function MakePanel({ sources, colorOf, tempo, results, onMake, onClose, playing, onPlay, onStop, onKeep, kept, use, placement = 'bottom', dock, onSources, lineage }: MakePanelProps) {
  const inline = placement === 'inline'
  const t = useT()
  const [cardKey, setCardKey] = useState('blend')
  // The mode each tab had last, so going back to Cross finds Attack where it was left.
  const [lastMode, setLastMode] = useState<Record<DesignTab, string>>({ cross: 'blend', fuse: 'texture', loop: 'loop', stack: 'stack' })
  const [options, setOptions] = useState<DesignOptions>({})
  // The steer, kept per mode: Blend's stays Blend's when the producer tries Resonate and comes back.
  const [steers, setSteers] = useState<Record<string, Steer>>({})
  const [steerOpen, setSteerOpen] = useState(false)
  // Results chosen to make from next, in the order they were chosen; and the sounds made from before, for Back.
  const [chosen, setChosen] = useState<string[]>([])
  const [trail, setTrail] = useState<DesignSource[][]>([])
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  // The requests this window sent, by take: a reroll repeats them exactly (their options too).
  const requests = useRef(new Map<string, DesignRequest>())
  // Which variation of each take shows (its index), and which results arrived while the panel was open.
  const [shown, setShown] = useState<Record<string, number>>({})
  const [fresh, setFresh] = useState<ReadonlySet<string>>(() => new Set())
  // The sound just made, brought into view once it's drawn (the cards above it can fill a narrow pane).
  const [arrived, setArrived] = useState<string | null>(null)
  // Each shown result's card, by its sound's id: where a new one scrolls to, and where a kept one flies from.
  const cards = useRef(new Map<string, HTMLDivElement>())
  // The panel's own scrolling list. A card comes into view by scrolling it alone: `scrollIntoView` would scroll every
  // ancestor too, the pane the map sits in among them.
  const list = useRef<HTMLDivElement | null>(null)
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
  const [flights, setFlights] = useState<Flight[]>([])
  const flightKey = useRef(0)
  /** Keep's flight from the card's waveform into the dock; none for a system that asks for less motion, or with no
   *  dock on screen. */
  const fly = (id: string) => {
    const canvas = cards.current.get(id)?.querySelector('canvas')
    const to = dock?.current?.getBoundingClientRect()
    if (!canvas || !to || to.width === 0 || window.matchMedia('(prefers-reduced-motion: reduce)').matches) return
    const key = ++flightKey.current
    setFlights(current => [...current, { key, from: canvas.getBoundingClientRect(), to, image: canvas.toDataURL() }])
  }
  const [open, setOpen] = useState(false)
  useEffect(() => {
    const frame = requestAnimationFrame(() => setOpen(true))
    return () => cancelAnimationFrame(frame)
  }, [])
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
      setArrived(answer.result.artifact_id)
      // A new sound plays as it arrives, as in Kumi's TUI.
      onPlay(answer.result.artifact_id)
      return
    }
    // The backend says why in a sentence of its own: a refusal (400: a sound picked on an older map, one it can't
    // read), two renders already running or the runtime not set up (503), or a render past two minutes (504). Only an
    // older backend with no sound design (404) and a failure with no words get the panel's own.
    setError(answer.status === 404 ? t('make.missing') : [400, 503, 504].includes(answer.status) && answer.error ? answer.error : t('make.failed', answer.error || String(answer.status)))
  }
  /** The panel's sounds become `next`; the ones it had wait for Back. */
  const makeFrom = (next: DesignSource[]) => {
    if (!onSources || next.length === 0) return
    setTrail(current => [...current, sources].slice(-8))
    setChosen([])
    onSources(next)
    reveal(list.current, 'start')
  }

  // The way to make it, in two steps that take little room beside the chat: the tab, then its modes as pills, and a
  // line saying what the chosen one does.
  const tint = `color-mix(in srgb, ${T.accent} 16%, transparent)`
  const modes = CARDS.filter(entry => entry.tab === card.tab)
  const chooser = (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 7 }}>
      <div aria-label={t('make.way')} role="radiogroup" style={{ display: 'grid', gridTemplateColumns: `repeat(${TABS.length}, 1fr)`, border: `1px solid ${T.border}`, borderRadius: 8, overflow: 'hidden' }}>
        {TABS.map((tab, index) => {
          const picked = card.tab === tab
          return (
            <button
              aria-checked={picked}
              className={FOCUSABLE}
              disabled={refused}
              key={tab}
              onClick={() => setCardKey(lastMode[tab])}
              role="radio"
              title={t(`make.opHints.${tab}`)}
              type="button"
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                gap: 6,
                minWidth: 0,
                font: 'inherit',
                fontSize: 12.5,
                fontWeight: picked ? 650 : 500,
                padding: '6px 4px',
                border: 'none',
                borderLeft: index > 0 ? `1px solid ${T.border}` : 'none',
                background: picked ? tint : 'transparent',
                color: refused ? T.faint : picked ? T.accent : T.secondary,
                cursor: refused ? 'default' : 'pointer',
                transition: 'background-color 140ms, color 140ms'
              }}
            >
              <Glyph card={TAB_GLYPH[tab]} size={14} />
              <span style={S.ellipsis}>{t(`make.ops.${tab}`)}</span>
            </button>
          )
        })}
      </div>
      {modes.length > 1 && (
        <div aria-label={t(`make.ops.${card.tab}`)} role="radiogroup" style={{ ...S.row, flexWrap: 'wrap', gap: 6 }}>
          {modes.map(entry => {
            const picked = entry.key === card.key
            return (
              <button
                aria-checked={picked}
                className={FOCUSABLE}
                disabled={refused}
                key={entry.key}
                onClick={() => {
                  setCardKey(entry.key)
                  setLastMode(current => ({ ...current, [entry.tab]: entry.key }))
                }}
                role="radio"
                title={t(`make.cards.${entry.key}.hint`)}
                type="button"
                style={{
                  ...S.row,
                  gap: 5,
                  font: 'inherit',
                  fontSize: 12,
                  fontWeight: picked ? 600 : 500,
                  padding: '3px 10px 3px 7px',
                  borderRadius: 14,
                  border: `1px solid ${picked ? T.accent : T.border}`,
                  background: picked ? tint : 'transparent',
                  boxShadow: picked ? `0 4px 14px -8px ${T.accent}` : 'none',
                  color: refused ? T.faint : picked ? T.accent : T.secondary,
                  cursor: refused ? 'default' : 'pointer',
                  transition: 'border-color 140ms, background-color 140ms, color 140ms, box-shadow 200ms'
                }}
              >
                <Glyph card={entry.key} size={13} />
                {t(`make.cards.${entry.key}.title`)}
              </button>
            )
          })}
        </div>
      )}
      <span style={{ ...S.muted, fontSize: 11.5 }}>{t(`make.cards.${card.key}.hint`)}</span>
    </div>
  )

  return (
    <section
      aria-label={t('make.title')}
      style={{
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
      <div style={{ ...S.row, gap: 8, padding: '10px 12px 6px', borderBottom: `1px solid ${T.border}` }}>
        <span style={{ fontSize: 13.5, fontWeight: 650 }}>{t('make.title')}</span>
        <span style={S.faint}>{t('make.from', sources.length)}</span>
        <span style={{ flex: 1 }} />
        {trail.length > 0 && onSources && (
          <TextButton
            onClick={() => {
              const back = trail[trail.length - 1]
              setTrail(current => current.slice(0, -1))
              setChosen([])
              onSources(back)
            }}
            title={t('make.backTitle', trail[trail.length - 1].length)}
          >
            <span aria-hidden>‹ </span>
            {t('make.back')}
          </TextButton>
        )}
        <TextButton label={t('make.close')} onClick={onClose} title={t('make.close')}>
          ×
        </TextButton>
      </div>
      {/* Positioned, so the screen-reader-only words inside (absolutely placed) scroll and clip with the list instead
          of hanging below the pane, which gave the pane room to scroll. */}
      <div ref={list} style={{ position: 'relative', overflowY: 'auto', padding: '8px 12px 12px', display: 'flex', flexDirection: 'column', gap: 10 }}>
        {/* The sounds that go in (all of them), on one line that scrolls: the first ones as chips (a sound Kumi made
            marked as one), then how many more. A row that scrolls may shrink to nothing in this column once results
            fill it, so it keeps its own height. */}
        <div aria-label={t('make.sources')} role="list" style={{ ...S.row, flex: 'none', flexWrap: 'nowrap', gap: 4, overflowX: 'auto', paddingBottom: 2, scrollbarWidth: 'thin' }}>
          {sources.slice(0, SOURCE_CHIPS).map((source, index) => (
            <span key={`${index}-${source.sound_id ?? source.artifact_id}`} role="listitem" style={{ ...S.row, gap: 4, flex: 'none', fontSize: 11, padding: '1px 7px 1px 5px', borderRadius: 10, border: `1px solid ${source.artifact_id ? `color-mix(in srgb, ${T.accent} 45%, transparent)` : T.border}`, color: T.secondary, maxWidth: 180 }}>
              {source.artifact_id ? (
                <span aria-label={t('make.made')} role="img" style={{ color: T.accent, fontSize: 10, flex: 'none' }}>
                  ✦
                </span>
              ) : (
                <span style={{ width: 7, height: 7, borderRadius: '50%', background: colorOf(source), flex: 'none' }} />
              )}
              <span style={S.ellipsis}>{clean(source.name ?? source.sound_id ?? source.artifact_id ?? '', 40)}</span>
            </span>
          ))}
          {sources.length > SOURCE_CHIPS && <span style={{ ...S.faint, flex: 'none' }}>{t('make.moreSources', sources.length - SOURCE_CHIPS)}</span>}
        </div>
        {chooser}
        {card.key === 'loop' ? <LoopSection onChange={setOptions} options={options} tempo={dawTempo} /> : <CardOptions card={card} onChange={setOptions} options={options} sources={sources} />}
        <SteerSection onChange={next => setSteers(current => ({ ...current, [card.key]: next }))} onToggle={() => setSteerOpen(!steerOpen)} open={steerOpen} steer={steer} />
        <div style={{ ...S.row, gap: 8, flexWrap: 'wrap' }}>
          <button
            className={FOCUSABLE}
            disabled={refused || busy !== null}
            onClick={() => void make(requestFor(card, sources, { ...options, ...(card.key === 'loop' ? { bpm: options.bpm ?? dawTempo } : {}), steer }), 'make')}
            title={t('make.makeTitle', t(`make.cards.${card.key}.title`))}
            type="button"
            style={{
              font: 'inherit',
              fontSize: 13,
              fontWeight: 650,
              padding: '6px 16px',
              borderRadius: 8,
              border: `1px solid ${T.accent}`,
              // The accent tinted, with the accent's own text: neither theme names a colour that reads on a solid accent.
              background: refused ? 'transparent' : `color-mix(in srgb, ${T.accent} 24%, transparent)`,
              color: refused ? T.faint : T.accent,
              cursor: refused || busy ? 'default' : 'pointer',
              opacity: busy === 'make' ? 0.75 : 1
            }}
          >
            {busy === 'make' ? t('make.making') : `${t('make.make')} · ${t(`make.cards.${card.key}.title`)}`}
          </button>
          {busy === 'make' && <span aria-hidden className="kumi-spin" style={{ width: 14, height: 14, borderRadius: '50%', border: `2px solid ${T.accent}`, borderRightColor: 'transparent' }} />}
          {note && <span style={S.faint}>{t(note.key, note.least)}</span>}
          <span aria-live="polite" style={error ? { ...S.faint, color: T.warn, flex: '1 1 14rem' } : S.hidden}>
            {error ?? ''}
          </span>
        </div>
        {takes.length > 0 && (
          <div aria-label={t('make.results')} role="list" style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {takes.map(take => {
              const at = Math.min(take.results.length - 1, shown[take.key] ?? take.results.length - 1)
              const result = take.results[at]
              const isPlaying = playing === result.artifact_id
              const inPalette = kept.has(result.artifact_id)
              const isChosen = chosen.includes(result.artifact_id)
              const stages = result.mode === 'modulate' ? stagesOf(result.recipe) : []
              return (
                <div
                  className={fresh.has(result.artifact_id) ? 'kumi-pop' : undefined}
                  ref={element => {
                    if (element) cards.current.set(result.artifact_id, element)
                    else cards.current.delete(result.artifact_id)
                  }}
                  key={take.key}
                  role="listitem"
                  style={{
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 5,
                    padding: '8px 10px',
                    borderRadius: 10,
                    border: `1px solid ${isChosen || isPlaying ? T.accent : T.border}`,
                    background: isChosen ? `color-mix(in srgb, ${T.accent} 10%, transparent)` : isPlaying ? `color-mix(in srgb, ${T.accent} 6%, transparent)` : 'transparent',
                    boxShadow: isChosen ? `0 0 0 1px ${T.accent}` : 'none',
                    transition: 'border-color 160ms, background-color 160ms, box-shadow 160ms'
                  }}
                >
                  <div style={{ display: 'flex', alignItems: 'flex-start', gap: 8 }}>
                    {/* Two lines of the recipe, all of it on hover; a modulate result names its stages in full below. */}
                    <span style={{ ...S.text, flex: 1, fontSize: 12, lineHeight: 1.35, ...(stages.length ? {} : { display: '-webkit-box', WebkitLineClamp: 2, WebkitBoxOrient: 'vertical', overflow: 'hidden' }) }} title={result.recipe}>
                      {stages.length ? clean(result.recipe.split(':', 1)[0], 80) : clean(result.recipe, 220) || resultName(result)}
                    </span>
                    {onSources && (
                      <button
                        aria-label={t('make.choose')}
                        aria-pressed={isChosen}
                        className={FOCUSABLE}
                        onClick={() => setChosen(current => (current.includes(result.artifact_id) ? current.filter(id => id !== result.artifact_id) : [...current, result.artifact_id]))}
                        title={t('make.chooseTitle')}
                        type="button"
                        style={{
                          flex: 'none',
                          width: 20,
                          height: 20,
                          display: 'flex',
                          alignItems: 'center',
                          justifyContent: 'center',
                          padding: 0,
                          borderRadius: '50%',
                          border: `1.5px solid ${isChosen ? T.accent : T.faint}`,
                          background: isChosen ? T.accent : 'transparent',
                          color: isChosen ? T.raised : 'transparent',
                          fontSize: 11,
                          fontWeight: 700,
                          cursor: 'pointer',
                          transform: isChosen ? 'scale(1.08)' : 'none',
                          transition: 'background-color 140ms, border-color 140ms, transform 160ms'
                        }}
                      >
                        ✓
                      </button>
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
                        fontSize: 11,
                        cursor: 'pointer',
                        transition: 'background-color 140ms'
                      }}
                    >
                      {isPlaying ? '■' : '▶'}
                    </button>
                    <div aria-hidden onClick={() => (isPlaying ? onStop() : onPlay(result.artifact_id))} style={{ flex: 1, minWidth: 0, cursor: 'pointer' }}>
                      <Wave colors={lineage?.(result).map(part => part.color)} duration={result.duration} peaks={result.peaks} playing={isPlaying} />
                    </div>
                  </div>
                  <div style={{ ...S.row, gap: 6, flexWrap: 'wrap' }}>
                    <span style={S.faint}>
                      {t(`make.ops.${result.mode === 'loop' ? 'loop' : result.op}`)}
                      {result.mode && result.mode !== 'loop' ? ` · ${t(`make.cards.${result.mode}.title`)}` : ''} · {seconds(result.duration)}
                    </span>
                    <span style={{ ...S.faint, fontSize: 10.5, fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace' }} title={t('make.seedTitle')}>
                      #{result.seed}
                    </span>
                    <span style={{ flex: 1 }} />
                    {take.results.length > 1 && (
                      <span role="group" aria-label={t('make.variations')} style={{ ...S.row, gap: 2 }}>
                        <TextButton disabled={at === 0} label={t('make.previous')} onClick={() => setShown(current => ({ ...current, [take.key]: at - 1 }))}>
                          ‹
                        </TextButton>
                        <span style={{ ...S.faint, fontVariantNumeric: 'tabular-nums' }}>{t('make.variation', at + 1, take.results.length)}</span>
                        <TextButton disabled={at === take.results.length - 1} label={t('make.next')} onClick={() => setShown(current => ({ ...current, [take.key]: at + 1 }))}>
                          ›
                        </TextButton>
                      </span>
                    )}
                  </div>
                  <div style={{ ...S.row, gap: 6, flexWrap: 'wrap' }}>
                    <TextButton disabled={busy !== null} onClick={() => void make(reroll(take.request), take.key)} title={t('make.rerollTitle')}>
                      {busy === take.key ? (
                        t('make.rerolling')
                      ) : (
                        <>
                          <span aria-hidden>↻ </span>
                          {t('make.reroll')}
                        </>
                      )}
                    </TextButton>
                    <TextButton
                      disabled={inPalette}
                      onClick={() => {
                        fly(result.artifact_id)
                        void onKeep(result)
                      }}
                      pressed={inPalette}
                      title={t('make.keepTitle')}
                    >
                      <span aria-hidden>{inPalette ? '✓ ' : '+ '}</span>
                      {inPalette ? t('make.kept') : t('make.keep')}
                    </TextButton>
                    {use?.(result)}
                  </div>
                </div>
              )
            })}
          </div>
        )}
        {takes.length === 0 && results !== null && <span style={S.faint}>{t('make.none')}</span>}
      </div>
      {/* The results chosen to make from next, and what to do with them: under the list, in view while it scrolls. */}
      {onSources && chosenResults.length > 0 && (
        <div
          aria-label={t('make.chosen', chosenResults.length)}
          className="kumi-pop"
          role="group"
          style={{ ...S.row, flexWrap: 'wrap', gap: 6, padding: '8px 12px', borderTop: `1px solid ${T.border}`, background: `color-mix(in srgb, ${T.accent} 7%, ${T.raised})`, borderRadius: '0 0 12px 12px' }}
        >
          <span style={{ ...S.text, fontWeight: 600, flex: '1 1 6rem' }}>{t('make.chosen', chosenResults.length)}</span>
          <TextButton onClick={() => makeFrom(chosenResults.map(sourceOfResult))} strong title={t('make.fromTheseTitle')}>
            {t('make.fromThese')}
          </TextButton>
          <TextButton
            onClick={() => makeFrom([...sources, ...chosenResults.filter(result => !sources.some(source => source.artifact_id === result.artifact_id)).map(sourceOfResult)])}
            title={t('make.addToSoundsTitle')}
          >
            {t('make.addToSounds')}
          </TextButton>
          <TextButton onClick={() => setChosen([])} title={t('make.clearChosenTitle')}>
            {t('make.clearChosen')}
          </TextButton>
        </div>
      )}
      {flights.map(flight => (
        <Flying flight={flight} key={flight.key} onDone={() => setFlights(current => current.filter(other => other.key !== flight.key))} />
      ))}
    </section>
  )
}
