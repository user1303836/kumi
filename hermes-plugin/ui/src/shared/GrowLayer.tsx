// Grow's vine over the map: a curve from one sound to the other in their colours, drawing itself in; its buds, each a
// point part of the way, breathing while the engine grows it and opening once grown (a click plays it); the handle,
// dragged along the vine to grow and hear any point; and a card for the point picked, with what can be done with it
// (the parent's Keep, Use… and Make from; Grow from here, another path, close).
import { Button } from '@kumi/controls'
import { T } from '@kumi/theme'
import type { KeyboardEvent, PointerEvent, ReactNode, RefObject } from 'react'
import { useEffect, useRef, useState } from 'react'

import { usePlayhead, Wave } from './atlas/Wave.tsx'
import { type DesignResult, sourceOfResult } from './design.ts'
import { clean, seconds } from './format.ts'
import { alongPath, type Bud, type Grow, type GrowEnd, nearestOnVine, vinePoint } from './grow.ts'
import { floatBeside } from './mapModel.ts'
import { glass, Icon } from './mapUi.tsx'
import { FOCUSABLE, S, TYPE, useT } from './ui.tsx'

type Point = readonly [number, number]
const percent = (at: number): number => Math.round(at * 100)
/** The card's width: its waveform (278 px) and padding. */
const CARD_WIDTH = 300

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
  actions,
  card: cardBox
}: {
  grow: Grow
  /** The ends on screen (px), and in map units (a bud grown again keeps its place); null for one not on the map. */
  from: Point | null
  to: Point | null
  fromMap: Point | null
  toMap: Point | null
  fromColor: string
  toColor: string
  /** The map's size, and what floats over its top (the whole top column: a find's list too) and its foot (the corner
   *  and the player), for the card's place. */
  width: number
  height: number
  top: number
  bottom: number
  playing: string | null
  onPlay: (result: DesignResult) => void
  onStop: () => void
  /** What the parent offers for a grown point (Keep, Use…, Make from). */
  actions?: (result: DesignResult) => ReactNode
  /** The card's own box, for the map to keep its words from under it. */
  card?: RefObject<HTMLDivElement | null>
}) {
  const t = useT()
  // A press on the handle: where it began, and whether it has moved enough to be a drag. One that never moves is a
  // click, on the bud under the handle when there is one (the handle sits on the vine, often right over a bud).
  const press = useRef<{ x: number; y: number; moved: boolean } | null>(null)
  // The card's own height, as laid out: its words wrap, so it's only known once drawn (an estimate until then).
  const own = useRef<HTMLDivElement | null>(null)
  const cardRef = cardBox ?? own
  const [cardHeight, setCardHeight] = useState(0)
  const hasCard = Boolean(from && to && grow.to)
  useEffect(() => {
    const element = cardRef.current
    if (!hasCard || !element) return
    const observer = new ResizeObserver(() => setCardHeight(element.offsetHeight))
    observer.observe(element)
    return () => observer.disconnect()
  }, [hasCard]) // eslint-disable-line react-hooks/exhaustive-deps
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
  // How far a point of this vine is on the whole path from its first sound (a vine grown from a grown point is the
  // rest of that path), in percent: what the buds, the handle and the card say, so every name is on one scale.
  const onPath = (bud: number): number => percent(alongPath(grow.from, bud))
  const origin = clean(grow.from?.origin ?? '', 60)
  const toName = clean(grow.to.name, 60)
  // Nothing grew and nothing is on its way: the first failure says why, without a click.
  const failed = grow.buds.some(bud => bud.state !== 'failed') ? undefined : grow.buds.find((bud): bud is Extract<Bud, { state: 'failed' }> => bud.state === 'failed')
  // Another path: the same two sounds on a new seed, offered with a grown point and with a failure.
  const another = (
    <Button kind="quiet" onClick={grow.reroll} tip={t('grow.rerollTitle')}>
      {t('grow.reroll')}
    </Button>
  )
  // The card floats beside the point picked, else beside the whole vine (its ends and its bow), never over them; at
  // its height as laid out (its words wrap), estimated until it has been.
  const xs = [from[0], to[0], cx]
  const ys = [from[1], to[1], cy]
  const anchor: [number, number, number, number] = pickedAt ? [pickedAt[0] - 14, pickedAt[1] - 14, pickedAt[0] + 14, pickedAt[1] + 14] : [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)]
  const card = floatBeside(anchor, CARD_WIDTH, cardHeight || (picked?.state === 'ready' ? 180 : failed || picked?.state === 'failed' ? 96 : 70), width, height, top, bottom)
  const budView = (bud: Bud, scrubbed = false) => {
    const [x, y] = at(bud.at)
    const color = mixed(fromColor, toColor, bud.at)
    const sounding = bud.state === 'ready' && playing === bud.result.artifact_id
    const isPicked = grow.picked === bud.at
    return (
      <g
        aria-label={t('grow.bud', onPath(bud.at))}
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
      <svg aria-label={t('grow.title', origin, toName)} height={height} role="group" style={{ position: 'absolute', inset: 0, overflow: 'visible', pointerEvents: 'none' }} width={width}>
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
          aria-valuemin={onPath(0)}
          aria-valuenow={onPath(grow.handle)}
          aria-valuetext={t('grow.bud', onPath(grow.handle))}
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
          <text fill={T.text} fontSize={11} fontWeight={600} paintOrder="stroke" stroke={T.raised} strokeWidth={3} textAnchor="middle" x={handle[0]} y={handle[1] - 15}>
            {`${onPath(grow.handle)}%`}
          </text>
        </g>
      </svg>
      <div
        className="kumi-pop"
        role="group"
        aria-label={t('grow.title', origin, toName)}
        ref={cardRef}
        style={{ ...glass, position: 'absolute', zIndex: 1, left: card.left, top: card.top, width: CARD_WIDTH, padding: '8px 10px', display: 'flex', flexDirection: 'column', gap: 6 }}
      >
        <div style={{ ...S.row, gap: 6, alignItems: 'flex-start' }}>
          <Icon name="grow" size={14} />
          <span style={{ flex: 1, minWidth: 0, ...TYPE.small, fontWeight: 600, overflowWrap: 'anywhere' }}>{t('grow.title', clean(grow.from?.origin ?? '', 40), clean(grow.to.name, 40))}</span>
          {/* The × says what it does: no tip. A 24 px target, drawn into the card's padding. */}
          <button aria-label={t('grow.close')} className={FOCUSABLE} onClick={grow.close} type="button" style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', width: 24, height: 24, margin: '-4px -6px -4px 0', padding: 0, border: 'none', borderRadius: 12, background: 'transparent', color: T.faint, cursor: 'pointer', flex: 'none' }}>
            <Icon name="close" size={11} />
          </button>
        </div>
        {!picked ? (
          failed ? (
            // Nothing grew and nothing is on its way (a sound's file is gone, the map moved on): why, at once.
            <>
              <span style={{ ...TYPE.caption, color: T.bad }}>{t('grow.failed', clean(failed.error, 160))}</span>
              <div style={S.row}>{another}</div>
            </>
          ) : (
            <span style={{ ...S.faint, ...TYPE.caption }}>{t('grow.hint')}</span>
          )
        ) : picked.state === 'ready' ? (
          <>
            {/* The point by its place on the path the head names, in words that wrap: never the engine's recipe, which
                names a grown sound by the recipe and seed of the one it grew from. */}
            <span style={{ ...TYPE.caption, color: T.secondary, overflowWrap: 'anywhere' }}>
              {t('grow.bud', onPath(picked.at))} <span style={{ color: T.faint }}>· {seconds(picked.result.duration)}</span>
            </span>
            {/* Keyed by the sound: another bud played while this one sounds starts its playhead anew. */}
            <GrownWave colors={[fromColor, toColor]} key={picked.result.artifact_id} playing={playing === picked.result.artifact_id} result={picked.result} />
            {/* One order: the sound's own play, the quiet ones, then what's made of it (Keep and Use… at the right edge). */}
            <div style={{ ...S.row, gap: 6, flexWrap: 'wrap' }}>
              <Button kind="quiet" onClick={() => (playing === picked.result.artifact_id ? onStop() : onPlay(picked.result))}>
                <span aria-hidden="true">{playing === picked.result.artifact_id ? '■' : '▶'}</span>
                {playing === picked.result.artifact_id ? t('map.stop') : t('map.play')}
              </Button>
              {fromMap && toMap && (
                <Button
                  kind="quiet"
                  onClick={() => {
                    // A new vine from this point toward the same sound: the rest of the path, named on its scale.
                    const place = vinePoint(fromMap, toMap, picked.at)
                    const name = t('grow.grown', onPath(picked.at), origin, toName)
                    const end: GrowEnd = { source: { ...sourceOfResult(picked.result), name }, id: null, place, name, origin: grow.from?.origin ?? '', along: alongPath(grow.from, picked.at) }
                    if (grow.to) grow.between(end, grow.to)
                  }}
                  tip={t('grow.againTitle')}
                >
                  {t('grow.again')}
                </Button>
              )}
              {another}
              {actions?.(picked.result)}
            </div>
          </>
        ) : picked.state === 'failed' ? (
          <>
            <span style={{ ...TYPE.caption, color: T.bad }}>{t('grow.failed', clean(picked.error, 160))}</span>
            <div style={S.row}>{another}</div>
          </>
        ) : (
          <span style={{ ...S.faint, ...TYPE.caption }}>{t('grow.growing', onPath(picked.at))}</span>
        )}
      </div>
    </>
  )
}
