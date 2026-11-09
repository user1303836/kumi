// The Make panel: a new sound from the picked sounds or the palette, by Kumi's sound-design engine. A card per way of
// making it (cross: blend, attack, resonate, modulate; fuse: texture, loop, smear; stack), a few options the card
// takes (the rest come from the seed), and the results as playable waveforms: Reroll makes the same take again with
// another seed and keeps every variation, Keep puts one in the palette (its waveform flies into the dock), Use puts a
// request for it in the chat. Making a sound changes no project, so it runs at once (`POST /design`). Both UI halves
// use it; what Use does comes from `use`. Everything it draws is its own: it reads only its own elements and the
// dock's, through refs, never the app's markup (catalog rule 8).
import { T } from '@kumi/theme'
import type { ReactNode, RefObject } from 'react'
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'

import { CARDS, cardNote, type DesignCard, type DesignOptions, type DesignRequest, type DesignResult, type DesignSource, mostFor, RANGES, requestFor, requestOf, reroll, resultName, takeKey } from './design.ts'
import { clean, num, seconds } from './format.ts'
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
  /** A card in the page's flow (the dashboard's, under the map's card; its page scrolls), rather than floating over the
   *  map's lower part (Desktop's pane, where the dock under the map stays in view). */
  inline?: boolean
  /** The palette dock (its `landing`), where a kept sound's waveform flies to. */
  dock?: RefObject<HTMLElement | null>
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

/** Each op's picture on its tab: two sounds becoming one, a long wave, and layers. */
const OP_GLYPH = { cross: 'blend', fuse: 'smear', stack: 'stack' } as const
const OPS = ['cross', 'fuse', 'stack'] as const

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

/** A result's waveform, from the engine's 256 peaks, mirrored about the middle; a line runs across it while it plays. */
function Wave({ peaks, playing, duration }: { peaks: number[]; playing: boolean; duration: number }) {
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
    const accent = getComputedStyle(element).color || '#4a90e2'
    const gradient = context.createLinearGradient(0, 0, width, 0)
    gradient.addColorStop(0, accent)
    gradient.addColorStop(1, `color-mix(in srgb, ${accent} 55%, transparent)`)
    context.fillStyle = gradient
    const bars = peaks.length || 1
    const step = width / bars
    for (let bar = 0; bar < peaks.length; bar++) {
      const tall = Math.max(1, peaks[bar] * (height - 4))
      context.fillRect(bar * step, (height - tall) / 2, Math.max(1, step - 0.4), tall)
    }
  }, [peaks, width])
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

/** A slider for one option, with "Auto" (the seed picks it) until it's moved. */
function OptionSlider({ name, label, value, onChange }: { name: Exclude<keyof DesignOptions, 'split' | 'attack_from'>; label: (value: number) => string; value: number | undefined; onChange: (value: number | undefined) => void }) {
  const t = useT()
  const range = RANGES[name]
  const shown = value ?? (range.min + range.max) / 2
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 2, minWidth: 150, flex: '1 1 150px' }}>
      <span style={{ ...S.row, gap: 6, fontSize: 11.5, color: T.secondary }}>
        <span style={{ flex: 1 }}>{value === undefined ? t(`make.options.${name}`) : label(value)}</span>
        <button
          aria-pressed={value === undefined}
          className={FOCUSABLE}
          onClick={event => {
            event.preventDefault()
            onChange(value === undefined ? shown : undefined)
          }}
          title={t('make.autoTitle')}
          type="button"
          style={{ font: 'inherit', fontSize: 10.5, padding: '0 6px', borderRadius: 8, border: `1px solid ${value === undefined ? T.accent : T.border}`, background: 'transparent', color: value === undefined ? T.accent : T.faint, cursor: 'pointer' }}
        >
          {t('make.auto')}
        </button>
      </span>
      <input
        aria-label={t(`make.options.${name}`)}
        className={FOCUSABLE}
        max={range.max}
        min={range.min}
        onChange={event => onChange(Number(event.target.value))}
        step={range.step}
        style={{ accentColor: T.accent, opacity: value === undefined ? 0.45 : 1 }}
        type="range"
        value={shown}
      />
    </label>
  )
}

interface Take {
  key: string
  /** Its variations, oldest first. */
  results: DesignResult[]
  /** The request that made it here (options included), for Reroll; from history, its op, mode and sounds only. */
  request: DesignRequest
}

export function MakePanel({ sources, colorOf, tempo, results, onMake, onClose, playing, onPlay, onStop, onKeep, kept, use, inline = false, dock }: MakePanelProps) {
  const t = useT()
  const [cardKey, setCardKey] = useState('blend')
  // The mode each op had last, so going back to Cross finds Attack where it was left.
  const [lastMode, setLastMode] = useState<Record<(typeof OPS)[number], string>>({ cross: 'blend', fuse: 'texture', stack: 'stack' })
  const [options, setOptions] = useState<DesignOptions>(() => (typeof tempo === 'number' && tempo >= 40 && tempo <= 240 ? { bpm: Math.round(tempo) } : {}))
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
  useEffect(() => {
    if (!arrived) return
    cards.current.get(arrived)?.scrollIntoView({ block: 'nearest', behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' })
    setArrived(null)
  }, [arrived, results])
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
  const note = cardNote(card, sources.length)
  const refused = note?.key === 'make.needTwo'
  const used = sources.slice(0, mostFor(card))

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

  const optionRow = (entry: DesignCard): ReactNode[] =>
    entry.options.map(name => {
      if (name === 'attack_from') {
        return (
          <label key={name} style={{ display: 'flex', flexDirection: 'column', gap: 2, minWidth: 150, flex: '1 1 150px', fontSize: 11.5, color: T.secondary }}>
            {t('make.options.attack_from')}
            <select
              className={FOCUSABLE}
              onChange={event => setOptions(current => ({ ...current, attack_from: event.target.value === '' ? undefined : Number(event.target.value) }))}
              style={{ font: 'inherit', fontSize: 12, padding: '1px 4px', borderRadius: 3, border: `1px solid ${T.border}`, background: 'transparent', color: T.text }}
              value={options.attack_from ?? ''}
            >
              <option value="">{t('make.auto')}</option>
              {used.map((source, index) => (
                <option key={`${index}`} value={index}>
                  {clean(source.name ?? source.sound_id ?? source.artifact_id ?? '', 40)}
                </option>
              ))}
            </select>
          </label>
        )
      }
      if (name === 'split') return null
      const labels: Record<string, (value: number) => string> = {
        seconds: value => t('make.optionValues.seconds', seconds(value)),
        bpm: value => t('make.optionValues.bpm', num(value, 0)),
        bars: value => t('make.optionValues.bars', value),
        depth: value => t('make.optionValues.depth', Math.round(value * 100)),
        stretch: value => t('make.optionValues.stretch', value)
      }
      return <OptionSlider key={name} label={labels[name]} name={name} onChange={next => setOptions(current => ({ ...current, [name]: next }))} value={options[name]} />
    })

  // The way to make it, in two steps that take little room beside the chat: the op as three tabs, then its modes as
  // pills, and a line saying what the chosen one does.
  const tint = `color-mix(in srgb, ${T.accent} 16%, transparent)`
  const chooser = (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 7 }}>
      <div aria-label={t('make.way')} role="radiogroup" style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', border: `1px solid ${T.border}`, borderRadius: 8, overflow: 'hidden' }}>
        {OPS.map((op, index) => {
          const chosen = card.op === op
          return (
            <button
              aria-checked={chosen}
              className={FOCUSABLE}
              disabled={refused}
              key={op}
              onClick={() => setCardKey(lastMode[op])}
              role="radio"
              title={t(`make.opHints.${op}`)}
              type="button"
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                gap: 6,
                minWidth: 0,
                font: 'inherit',
                fontSize: 12.5,
                fontWeight: chosen ? 650 : 500,
                padding: '6px 4px',
                border: 'none',
                borderLeft: index > 0 ? `1px solid ${T.border}` : 'none',
                background: chosen ? tint : 'transparent',
                color: refused ? T.faint : chosen ? T.accent : T.secondary,
                cursor: refused ? 'default' : 'pointer',
                transition: 'background-color 140ms, color 140ms'
              }}
            >
              <Glyph card={OP_GLYPH[op]} size={14} />
              <span style={S.ellipsis}>{t(`make.ops.${op}`)}</span>
            </button>
          )
        })}
      </div>
      {card.op !== 'stack' && (
        <div aria-label={t(`make.ops.${card.op}`)} role="radiogroup" style={{ ...S.row, flexWrap: 'wrap', gap: 6 }}>
          {CARDS.filter(entry => entry.op === card.op).map(entry => {
            const chosen = entry.key === card.key
            return (
              <button
                aria-checked={chosen}
                className={FOCUSABLE}
                disabled={refused}
                key={entry.key}
                onClick={() => {
                  setCardKey(entry.key)
                  setLastMode(current => ({ ...current, [entry.op]: entry.key }))
                }}
                role="radio"
                title={t(`make.cards.${entry.key}.hint`)}
                type="button"
                style={{
                  ...S.row,
                  gap: 5,
                  font: 'inherit',
                  fontSize: 12,
                  fontWeight: chosen ? 600 : 500,
                  padding: '3px 10px 3px 7px',
                  borderRadius: 14,
                  border: `1px solid ${chosen ? T.accent : T.border}`,
                  background: chosen ? tint : 'transparent',
                  boxShadow: chosen ? `0 4px 14px -8px ${T.accent}` : 'none',
                  color: refused ? T.faint : chosen ? T.accent : T.secondary,
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
        ...(inline ? {} : { position: 'absolute', left: 8, right: 8, bottom: 8, maxHeight: '82%' }),
        display: 'flex',
        flexDirection: 'column',
        borderRadius: 12,
        border: `1px solid ${T.border}`,
        background: T.raised,
        boxShadow: '0 18px 48px -18px rgba(0,0,0,.55)',
        color: T.text,
        transform: open ? 'none' : 'translateY(24px)',
        opacity: open ? 1 : 0,
        transition: 'transform 260ms cubic-bezier(.2,.8,.2,1), opacity 200ms',
        zIndex: inline ? undefined : 2
      }}
    >
      <div style={{ ...S.row, gap: 8, padding: '10px 12px 6px', borderBottom: `1px solid ${T.border}` }}>
        <span style={{ fontSize: 13.5, fontWeight: 650 }}>{t('make.title')}</span>
        <span style={S.faint}>{t('make.from', sources.length)}</span>
        <span style={{ flex: 1 }} />
        <TextButton label={t('make.close')} onClick={onClose} title={t('make.close')}>
          ×
        </TextButton>
      </div>
      <div style={{ overflowY: 'auto', padding: '8px 12px 12px', display: 'flex', flexDirection: 'column', gap: 10 }}>
        {/* The sounds that go in, on one line that scrolls; how many more were picked than the card takes. */}
        <div aria-label={t('make.sources')} role="list" style={{ ...S.row, flexWrap: 'nowrap', gap: 4, overflowX: 'auto', paddingBottom: 2, scrollbarWidth: 'thin' }}>
          {used.map((source, index) => (
            <span key={`${index}-${source.sound_id ?? source.artifact_id}`} role="listitem" style={{ ...S.row, gap: 4, flex: 'none', fontSize: 11, padding: '1px 7px 1px 5px', borderRadius: 10, border: `1px solid ${T.border}`, color: T.secondary, maxWidth: 160 }}>
              <span style={{ width: 7, height: 7, borderRadius: '50%', background: colorOf(source), flex: 'none' }} />
              <span style={S.ellipsis}>{clean(source.name ?? source.sound_id ?? source.artifact_id ?? '', 40)}</span>
            </span>
          ))}
          {sources.length > used.length && <span style={{ ...S.faint, flex: 'none' }}>+{sources.length - used.length}</span>}
        </div>
        {chooser}
        {card.options.length > 0 && <div style={{ ...S.row, flexWrap: 'wrap', gap: 10, alignItems: 'flex-end' }}>{optionRow(card)}</div>}
        <div style={{ ...S.row, gap: 8, flexWrap: 'wrap' }}>
          <button
            className={FOCUSABLE}
            disabled={refused || busy !== null}
            onClick={() => void make(requestFor(card, sources, options), 'make')}
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
          {note && <span style={S.faint}>{note.key === 'make.needTwo' ? t(note.key, note.least) : t(note.key, note.most, note.of)}</span>}
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
              return (
                <div
                  className={fresh.has(result.artifact_id) ? 'kumi-pop' : undefined}
                  ref={element => {
                    if (element) cards.current.set(result.artifact_id, element)
                    else cards.current.delete(result.artifact_id)
                  }}
                  key={take.key}
                  role="listitem"
                  style={{ display: 'flex', flexDirection: 'column', gap: 5, padding: '8px 10px', borderRadius: 10, border: `1px solid ${isPlaying ? T.accent : T.border}`, background: isPlaying ? `color-mix(in srgb, ${T.accent} 6%, transparent)` : 'transparent', transition: 'border-color 160ms, background-color 160ms' }}
                >
                  {/* Two lines of the recipe; all of it on hover. */}
                  <span style={{ ...S.text, fontSize: 12, lineHeight: 1.35, display: '-webkit-box', WebkitLineClamp: 2, WebkitBoxOrient: 'vertical', overflow: 'hidden' }} title={result.recipe}>
                    {clean(result.recipe, 220) || resultName(result)}
                  </span>
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
                      <Wave duration={result.duration} peaks={result.peaks} playing={isPlaying} />
                    </div>
                  </div>
                  <div style={{ ...S.row, gap: 6, flexWrap: 'wrap' }}>
                    <span style={S.faint}>
                      {t(`make.ops.${result.op}`)}
                      {result.mode ? ` · ${t(`make.cards.${result.mode}.title`)}` : ''} · {seconds(result.duration)}
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
      {flights.map(flight => (
        <Flying flight={flight} key={flight.key} onDone={() => setFlights(current => current.filter(other => other.key !== flight.key))} />
      ))}
    </section>
  )
}
