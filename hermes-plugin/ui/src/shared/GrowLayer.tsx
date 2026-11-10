// Grow's vine over the map: a curve from one sound to the other in their colours, drawing itself in; its buds, each a
// point part of the way, breathing while the engine grows it and opening once grown (a click plays it); the handle,
// dragged along the vine to grow and hear any point; and a card for the point picked, with what can be done with it
// (the parent's Keep, Use… and Make from; Grow from here, another path, close).
import { T } from '@kumi/theme'
import type { KeyboardEvent, PointerEvent, ReactNode } from 'react'
import { useRef } from 'react'

import { usePlayhead, Wave } from './atlas/Wave.tsx'
import { type DesignResult, sourceOfResult } from './design.ts'
import { clean, seconds } from './format.ts'
import { type Bud, type Grow, type GrowEnd, nearestOnVine, vinePoint } from './grow.ts'
import { floatBeside } from './mapModel.ts'
import { glass, Icon } from './mapUi.tsx'
import { FOCUSABLE, S, TextButton, useT } from './ui.tsx'

type Point = readonly [number, number]

const percent = (at: number): number => Math.round(at * 100)

/** A bud's colour: its ends' colours mixed by how far along it is. */
const mixed = (from: string, to: string, at: number): string => `color-mix(in srgb, ${to} ${percent(at)}%, ${from})`

/** A grown point's waveform in its two ends' colours, the first's on the left, with a playhead while it plays. */
function GrownWave({ result, colors, playing }: { result: DesignResult; colors: readonly string[]; playing: boolean }) {
  return <Wave colors={colors} height={26} peaks={result.peaks} progress={usePlayhead(playing, result.duration)} width={278} />
}

export function GrowLayer({
  grow,
  from,
  to,
  fromMap,
  toMap,
  fromColor,
  toColor,
  width,
  height,
  top,
  bottom,
  playing,
  onPlay,
  onStop,
  actions
}: {
  grow: Grow
  /** The ends on screen (px), and in map units (a bud grown again keeps its place); null for one not on the map. */
  from: Point | null
  to: Point | null
  fromMap: Point | null
  toMap: Point | null
  fromColor: string
  toColor: string
  /** The map's size, and what floats over its top and its foot (the player), for the card's place. */
  width: number
  height: number
  top: number
  bottom: number
  playing: string | null
  onPlay: (result: DesignResult) => void
  onStop: () => void
  /** What the parent offers for a grown point (Keep, Use…, Make from). */
  actions?: (result: DesignResult) => ReactNode
}) {
  const t = useT()
  // A press on the handle: where it began, and whether it has moved enough to be a drag. One that never moves is a
  // click, on the bud under the handle when there is one (the handle sits on the vine, often right over a bud).
  const press = useRef<{ x: number; y: number; moved: boolean } | null>(null)
  if (!from) return null
  // The first sound named, while the second is still to come: it breathes where it is.
  if (!to || !grow.to) {
    return (
      <svg aria-hidden height={height} style={{ position: 'absolute', inset: 0, overflow: 'visible', pointerEvents: 'none' }} width={width}>
        <circle className="kumi-breathe" cx={from[0]} cy={from[1]} fill="none" r={11} stroke={fromColor} strokeWidth={2.5} />
      </svg>
    )
  }
  const at = (bud: number): Point => vinePoint(from, to, bud)
  const [cx, cy] = [(from[0] + to[0]) / 2 - (to[1] - from[1]) * 0.22, (from[1] + to[1]) / 2 + (to[0] - from[0]) * 0.22]
  const curve = `M ${from[0]} ${from[1]} Q ${cx} ${cy} ${to[0]} ${to[1]}`
  const gradient = `kumi-vine-${Math.round(from[0])}-${Math.round(to[0])}`
  const handle = at(grow.handle)
  const pointOf = (event: PointerEvent<SVGElement>): Point => {
    const box = (event.currentTarget.ownerSVGElement ?? (event.currentTarget as SVGSVGElement)).getBoundingClientRect()
    return [event.clientX - box.left, event.clientY - box.top]
  }
  const nudge = (event: KeyboardEvent<SVGElement>) => {
    const step = event.key === 'ArrowRight' || event.key === 'ArrowUp' ? 0.05 : event.key === 'ArrowLeft' || event.key === 'ArrowDown' ? -0.05 : 0
    if (!step) return
    event.preventDefault()
    grow.scrubTo(Math.min(1, Math.max(0, Math.round((grow.handle + step) * 20) / 20)), true)
  }
  // The point picked: a bud, or the handle's point.
  const picked = grow.picked === null ? null : grow.scrub?.at === grow.picked ? grow.scrub : (grow.buds.find(bud => bud.at === grow.picked) ?? null)
  const pickedAt = picked ? at(picked.at) : null
  // Nothing grew and nothing is on its way: the first failure says why, without a click.
  const failed = grow.buds.some(bud => bud.state !== 'failed') ? undefined : grow.buds.find((bud): bud is Extract<Bud, { state: 'failed' }> => bud.state === 'failed')
  // Another path: the same two sounds on a new seed, offered with a grown point and with a failure.
  const another = (
    <TextButton onClick={grow.reroll} title={t('grow.rerollTitle')}>
      {t('grow.reroll')}
    </TextButton>
  )
  // The card floats beside the point picked, else beside the whole vine (its ends and its bow), never over them.
  const xs = [from[0], to[0], cx]
  const ys = [from[1], to[1], cy]
  const anchor: [number, number, number, number] = pickedAt ? [pickedAt[0] - 14, pickedAt[1] - 14, pickedAt[0] + 14, pickedAt[1] + 14] : [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)]
  const card = floatBeside(anchor, 300, picked?.state === 'ready' ? 164 : failed || picked?.state === 'failed' ? 96 : 70, width, height, top, bottom)
  const budView = (bud: Bud, scrubbed = false) => {
    const [x, y] = at(bud.at)
    const color = mixed(fromColor, toColor, bud.at)
    const sounding = bud.state === 'ready' && playing === bud.result.artifact_id
    const isPicked = grow.picked === bud.at
    return (
      <g
        aria-label={t('grow.bud', percent(bud.at))}
        className={FOCUSABLE}
        key={`${scrubbed ? 's' : 'b'}${bud.at}`}
        onClick={() => grow.pick(bud.at)}
        onKeyDown={event => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault()
            grow.pick(bud.at)
          }
        }}
        role="button"
        style={{ cursor: 'pointer', pointerEvents: 'auto' }}
        tabIndex={0}
      >
        <circle cx={x} cy={y} fill="transparent" r={12} />
        {isPicked && <circle cx={x} cy={y} fill="none" r={11} stroke={T.text} strokeWidth={1.25} />}
        {sounding && <circle cx={x} cy={y} fill="none" r={14} stroke={color} strokeOpacity={0.5} strokeWidth={2} />}
        {bud.state === 'ready' ? (
          <circle className="kumi-open" cx={x} cy={y} fill={color} r={6.5} stroke={T.raised} strokeWidth={1.5} />
        ) : bud.state === 'failed' ? (
          <path d={`M ${x - 4} ${y - 4} L ${x + 4} ${y + 4} M ${x + 4} ${y - 4} L ${x - 4} ${y + 4}`} stroke={T.bad} strokeLinecap="round" strokeWidth={2} />
        ) : (
          <circle className={bud.state === 'growing' ? 'kumi-breathe' : undefined} cx={x} cy={y} fill={T.raised} r={5} stroke={color} strokeDasharray={bud.state === 'waiting' ? '2 2' : undefined} strokeWidth={1.75} />
        )}
      </g>
    )
  }
  return (
    <>
      <svg aria-label={t('grow.title', clean(grow.from?.name ?? '', 60), clean(grow.to.name, 60))} height={height} role="group" style={{ position: 'absolute', inset: 0, overflow: 'visible', pointerEvents: 'none' }} width={width}>
        <defs>
          <linearGradient gradientUnits="userSpaceOnUse" id={gradient} x1={from[0]} x2={to[0]} y1={from[1]} y2={to[1]}>
            <stop offset="0" stopColor={fromColor} />
            <stop offset="1" stopColor={toColor} />
          </linearGradient>
        </defs>
        <path d={curve} fill="none" opacity={0.22} stroke={`url(#${gradient})`} strokeLinecap="round" strokeWidth={10} />
        <path className="kumi-vine" d={curve} fill="none" pathLength={1} stroke={`url(#${gradient})`} strokeLinecap="round" strokeWidth={3} />
        <circle cx={from[0]} cy={from[1]} fill="none" r={9} stroke={fromColor} strokeWidth={2} />
        <circle cx={to[0]} cy={to[1]} fill="none" r={9} stroke={toColor} strokeDasharray="3 2" strokeWidth={2} />
        {grow.buds.map(bud => budView(bud))}
        {grow.scrub && !grow.buds.some(bud => bud.at === grow.scrub?.at) && budView(grow.scrub, true)}
        <g
          aria-label={t('grow.handle')}
          aria-valuemax={100}
          aria-valuemin={0}
          aria-valuenow={percent(grow.handle)}
          aria-valuetext={t('grow.bud', percent(grow.handle))}
          className={FOCUSABLE}
          onKeyDown={nudge}
          onPointerDown={event => {
            event.stopPropagation()
            event.currentTarget.setPointerCapture(event.pointerId)
            const [x, y] = pointOf(event)
            press.current = { x, y, moved: false }
          }}
          onPointerMove={event => {
            const held = press.current
            if (!held) return
            const at = pointOf(event)
            if (!held.moved && Math.hypot(at[0] - held.x, at[1] - held.y) > 3) held.moved = true
            if (held.moved) grow.scrubTo(nearestOnVine(from, to, at), false)
          }}
          onPointerUp={event => {
            const held = press.current
            press.current = null
            if (!held) return
            const at = pointOf(event)
            if (held.moved) {
              grow.scrubTo(nearestOnVine(from, to, at), true)
              return
            }
            const under = [...grow.buds, ...(grow.scrub ? [grow.scrub] : [])].find(bud => {
              const [x, y] = vinePoint(from, to, bud.at)
              return Math.hypot(x - at[0], y - at[1]) <= 12
            })
            if (under) grow.pick(under.at)
            else grow.scrubTo(nearestOnVine(from, to, at), true)
          }}
          onPointerCancel={() => {
            press.current = null
          }}
          role="slider"
          style={{ cursor: 'grab', pointerEvents: 'auto' }}
          tabIndex={0}
        >
          <circle cx={handle[0]} cy={handle[1]} fill="transparent" r={16} />
          <circle cx={handle[0]} cy={handle[1]} fill={T.raised} r={8.5} stroke={T.accent} strokeWidth={2.5} />
          <circle cx={handle[0]} cy={handle[1]} fill={T.accent} r={3} />
          <text fill={T.text} fontSize={10.5} fontWeight={600} paintOrder="stroke" stroke={T.raised} strokeWidth={3} textAnchor="middle" x={handle[0]} y={handle[1] - 15}>
            {`${percent(grow.handle)}%`}
          </text>
        </g>
      </svg>
      <div
        className="kumi-pop"
        role="group"
        aria-label={t('grow.title', clean(grow.from?.name ?? '', 60), clean(grow.to.name, 60))}
        style={{ ...glass, position: 'absolute', zIndex: 1, left: card.left, top: card.top, width: 300, padding: '8px 10px', display: 'flex', flexDirection: 'column', gap: 6 }}
      >
        <div style={{ ...S.row, gap: 6 }}>
          <Icon name="grow" size={14} />
          <span style={{ ...S.ellipsis, flex: 1, fontSize: 12, fontWeight: 650 }}>{t('grow.title', clean(grow.from?.name ?? '', 40), clean(grow.to.name, 40))}</span>
          <button aria-label={t('grow.close')} className={FOCUSABLE} onClick={grow.close} title={t('grow.close')} type="button" style={{ display: 'flex', padding: 3, border: 'none', borderRadius: 10, background: 'transparent', color: T.faint, cursor: 'pointer' }}>
            <Icon name="close" size={11} />
          </button>
        </div>
        {!picked ? (
          failed ? (
            // Nothing grew and nothing is on its way (a sound's file is gone, the map moved on): why, at once.
            <>
              <span style={{ fontSize: 11, color: T.bad }}>{t('grow.failed', clean(failed.error, 160))}</span>
              <div style={S.row}>{another}</div>
            </>
          ) : (
            <span style={{ ...S.faint, fontSize: 11 }}>{t('grow.hint')}</span>
          )
        ) : picked.state === 'ready' ? (
          <>
            <span style={{ fontSize: 11, color: T.secondary }} title={picked.result.recipe}>
              <span style={{ ...S.ellipsis, display: 'block' }}>{clean(picked.result.recipe, 160)}</span>
              <span style={{ color: T.faint }}>{seconds(picked.result.duration)}</span>
            </span>
            {/* Keyed by the sound: another bud played while this one sounds starts its playhead anew. */}
            <GrownWave colors={[fromColor, toColor]} key={picked.result.artifact_id} playing={playing === picked.result.artifact_id} result={picked.result} />
            <div style={{ ...S.row, gap: 6, flexWrap: 'wrap' }}>
              <TextButton onClick={() => (playing === picked.result.artifact_id ? onStop() : onPlay(picked.result))}>{playing === picked.result.artifact_id ? t('map.stop') : t('map.play')}</TextButton>
              {actions?.(picked.result)}
              {fromMap && toMap && (
                <TextButton
                  onClick={() => {
                    const place = vinePoint(fromMap, toMap, picked.at)
                    const end: GrowEnd = { source: sourceOfResult(picked.result), id: null, place, name: t('grow.bud', percent(picked.at)) }
                    if (grow.to) grow.between(end, grow.to)
                  }}
                  title={t('grow.againTitle')}
                >
                  {t('grow.again')}
                </TextButton>
              )}
              {another}
            </div>
          </>
        ) : picked.state === 'failed' ? (
          <>
            <span style={{ fontSize: 11, color: T.bad }}>{t('grow.failed', clean(picked.error, 160))}</span>
            <div style={S.row}>{another}</div>
          </>
        ) : (
          <span style={{ ...S.faint, fontSize: 11 }}>{t('grow.growing', percent(picked.at))}</span>
        )}
      </div>
    </>
  )
}
