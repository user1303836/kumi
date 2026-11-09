// The Make panel: a new sound from the picked sounds or the palette, by Kumi's sound-design engine. A card per way of
// making it (cross: blend, attack, resonate, modulate; fuse: texture, loop, smear; stack), a few options the card
// takes (the rest come from the seed), and the results as playable waveforms: Reroll makes the same take again with
// another seed and keeps every variation, Keep puts one in the palette, Use puts a request for it in the chat. Making
// a sound changes no project, so it runs at once (`POST /design`). Both UI halves use it; what Use does comes from
// `use`.
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'

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

function Glyph({ card }: { card: string }) {
  return (
    <svg aria-hidden height="20" style={{ flex: 'none' }} viewBox="0 0 28 20" width="28">
      {GLYPHS[card]}
    </svg>
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

export function MakePanel({ sources, colorOf, tempo, results, onMake, onClose, playing, onPlay, onStop, onKeep, kept, use, inline = false }: MakePanelProps) {
  const t = useT()
  const [cardKey, setCardKey] = useState('blend')
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
  const list = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    if (!arrived) return
    const take = list.current?.querySelector<HTMLElement>(`[data-made="${arrived}"]`)
    take?.scrollIntoView({ block: 'nearest', behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' })
    setArrived(null)
  }, [arrived, results])
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

  const group = (op: 'cross' | 'fuse' | 'stack') => (
    <div key={op} style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      <span style={{ ...S.heading, fontSize: 10.5 }}>
        {t(`make.ops.${op}`)} <span style={{ textTransform: 'none', letterSpacing: 0, color: T.faint }}>{t(`make.opHints.${op}`)}</span>
      </span>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(124px, 1fr))', gap: 6 }}>
        {CARDS.filter(entry => entry.op === op).map(entry => {
          const chosen = entry.key === card.key
          const blocked = refused
          return (
            <button
              aria-pressed={chosen}
              className={FOCUSABLE}
              disabled={Boolean(blocked)}
              key={entry.key}
              onClick={() => setCardKey(entry.key)}
              title={t(`make.cards.${entry.key}.hint`)}
              type="button"
              style={{
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'flex-start',
                gap: 3,
                textAlign: 'left',
                font: 'inherit',
                padding: '7px 8px',
                borderRadius: 8,
                border: `1px solid ${chosen ? T.accent : T.border}`,
                background: chosen ? `color-mix(in srgb, ${T.accent} 13%, transparent)` : 'transparent',
                boxShadow: chosen ? `0 0 0 1px ${T.accent}, 0 6px 18px -10px ${T.accent}` : 'none',
                color: blocked ? T.faint : T.text,
                cursor: blocked ? 'default' : 'pointer',
                opacity: blocked ? 0.55 : 1,
                transition: 'border-color 140ms, background-color 140ms, box-shadow 200ms, transform 140ms',
                transform: chosen ? 'translateY(-1px)' : 'none'
              }}
            >
              <span style={{ ...S.row, gap: 6, color: chosen ? T.accent : T.secondary }}>
                <Glyph card={entry.key} />
                <span style={{ fontSize: 12.5, fontWeight: 600, color: blocked ? T.faint : T.text }}>{t(`make.cards.${entry.key}.title`)}</span>
              </span>
              <span style={{ fontSize: 11, lineHeight: 1.35, color: T.muted }}>{t(`make.cards.${entry.key}.hint`)}</span>
            </button>
          )
        })}
      </div>
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
        {group('cross')}
        {group('fuse')}
        {group('stack')}
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
          <div aria-label={t('make.results')} ref={list} role="list" style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {takes.map(take => {
              const at = Math.min(take.results.length - 1, shown[take.key] ?? take.results.length - 1)
              const result = take.results[at]
              const isPlaying = playing === result.artifact_id
              const inPalette = kept.has(result.artifact_id)
              return (
                <div
                  className={fresh.has(result.artifact_id) ? 'kumi-pop' : undefined}
                  data-made={result.artifact_id}
                  key={take.key}
                  role="listitem"
                  style={{ display: 'flex', flexDirection: 'column', gap: 4, padding: '8px 10px', borderRadius: 10, border: `1px solid ${isPlaying ? T.accent : T.border}`, background: 'transparent' }}
                >
                  <span style={{ ...S.text, fontSize: 12, lineHeight: 1.35 }} title={result.recipe}>
                    {clean(result.recipe, 220) || resultName(result)}
                  </span>
                  <Wave duration={result.duration} peaks={result.peaks} playing={isPlaying} />
                  <div style={{ ...S.row, gap: 6, flexWrap: 'wrap' }}>
                    <span style={S.faint}>
                      {t(`make.ops.${result.op}`)}
                      {result.mode ? ` · ${t(`make.cards.${result.mode}.title`)}` : ''} · {seconds(result.duration)}
                    </span>
                    <span style={{ ...S.faint, fontVariantNumeric: 'tabular-nums' }} title={t('make.seedTitle')}>
                      #{result.seed}
                    </span>
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
                    <span style={{ flex: 1 }} />
                    <TextButton label={isPlaying ? t('make.stop') : t('make.play')} onClick={() => (isPlaying ? onStop() : onPlay(result.artifact_id))}>
                      {isPlaying ? '■' : '▶'}
                    </TextButton>
                    <TextButton disabled={busy !== null} onClick={() => void make(reroll(take.request), take.key)} title={t('make.rerollTitle')}>
                      {busy === take.key ? t('make.rerolling') : t('make.reroll')}
                    </TextButton>
                    <TextButton disabled={inPalette} onClick={() => void onKeep(result)} pressed={inPalette} title={t('make.keepTitle')}>
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
    </section>
  )
}
