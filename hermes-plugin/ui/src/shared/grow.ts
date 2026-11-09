// Grow (A2): the sounds part of the way from one sound toward another, as a vine on the map. The producer names two
// sounds (or grows between two picks); a vine bows from one to the other and buds open along it as the engine makes
// them, one seed for the whole path (a new seed, Reroll, is a new path). A handle scrubs along the vine: where it
// stops is grown and played. The backend renders at most two sounds at once, so the buds wait their turn, and the
// handle's point goes first: what the producer is listening for comes before what's still opening.
import { useEffect, useRef, useState } from 'react'

import { type DesignResult, type DesignSource, growRequest, sourcesOf } from './design.ts'
import { clean } from './format.ts'
import type { Designed } from './store.ts'

/** Where the buds open along a vine: from an eighth of the way to seven eighths. */
export const BUDS = [0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875] as const
/** How many sounds the backend renders at once (more are refused, 503). */
export const RENDERS = 2
/** How far the handle moves before a drag grows its new point (a release always grows where it stops). */
const SCRUB_STEP = 0.04

/** One end of a vine: its sound (a map sound, or one Kumi made), and for a made sound where it sits on the map (where
 *  the bud it was grew); a map sound sits wherever the layout shown puts it. */
export interface GrowEnd {
  source: DesignSource
  /** The map sound's id, to find it in the layout shown; null for a made sound. */
  id: string | null
  /** A made sound's place on the map (map units). */
  place: readonly [number, number] | null
  name: string
}

/** A map sound as a vine's end: its design source (with the map it's from); null without a map revision. */
export function soundEnd(id: string, name: string | null | undefined, revision: string | null): GrowEnd | null {
  const [source] = sourcesOf([{ sound_id: id, name: name ?? id }], revision)
  return source ? { source, id, place: null, name: clean(name ?? id, 80) } : null
}

/** A point along the vine: waiting for a render, growing, grown (its sound), or not (why). */
export type Bud = { at: number; state: 'growing' | 'waiting' } | { at: number; state: 'ready'; result: DesignResult } | { at: number; state: 'failed'; error: string }

/** The point `t` (0…1) along a vine from `a` to `b`: a quadratic curve bowing to the left of the line between them by
 *  `bow` of its length. It is the same curve in map units and on screen (the view only scales and shifts). */
export function vinePoint(a: readonly [number, number], b: readonly [number, number], t: number, bow = 0.22): [number, number] {
  const [cx, cy] = [(a[0] + b[0]) / 2 - (b[1] - a[1]) * bow, (a[1] + b[1]) / 2 + (b[0] - a[0]) * bow]
  const u = 1 - t
  return [u * u * a[0] + 2 * u * t * cx + t * t * b[0], u * u * a[1] + 2 * u * t * cy + t * t * b[1]]
}

/** How far along the vine (0…1) its point nearest `p` is: where a drag of the handle puts it. */
export function nearestOnVine(a: readonly [number, number], b: readonly [number, number], p: readonly [number, number], bow = 0.22): number {
  let [best, nearest] = [0, Infinity]
  for (let step = 0; step <= 240; step++) {
    const [x, y] = vinePoint(a, b, step / 240, bow)
    const distance = (x - p[0]) ** 2 + (y - p[1]) ** 2
    if (distance < nearest) [best, nearest] = [step / 240, distance]
  }
  return best
}

/** What to render next, with `running` renders under way: the handle's point first, then the buds in order from the
 *  vine's start; nothing while every render is taken. */
export function nextToGrow(buds: readonly Bud[], scrub: Bud | null, running: number): Bud | null {
  if (running >= RENDERS) return null
  if (scrub?.state === 'waiting') return scrub
  return buds.find(bud => bud.state === 'waiting') ?? null
}

export interface Grow {
  /** Naming the ends: `from` first, then `to`; null while not. */
  choosing: 'from' | 'to' | null
  from: GrowEnd | null
  to: GrowEnd | null
  buds: Bud[]
  /** The handle: where it is (it follows a drag), and the point it last grew. */
  handle: number
  scrub: Bud | null
  /** The bud, or the handle's point, the producer picked (its `at`). */
  picked: number | null
  /** Start or stop naming the ends with clicks on the map. */
  choose: (on: boolean) => void
  /** A click on the map while naming: the first names `from`, the second `to` and grows the vine. */
  name: (end: GrowEnd) => void
  /** Grow a vine between two sounds at once (two picks, or a bud grown again toward the same sound). */
  between: (from: GrowEnd, to: GrowEnd) => void
  /** Move the handle to `at`: a drag grows its point every few steps, a release (`done`) where it stops. */
  scrubTo: (at: number, done: boolean) => void
  pick: (at: number | null) => void
  /** The same two sounds by another path: a new seed, every bud again. */
  reroll: () => void
  close: () => void
}

const newSeed = (): number => Math.floor(Math.random() * 2 ** 31)
const fresh = (): Bud[] => BUDS.map(at => ({ at, state: 'waiting' }))

/** Grow's vine and its renders. `design` makes a sound (the store's); `heard` plays one the producer is listening for
 *  (the handle's point, once grown); a new map `revision` lets the vine go (its sounds' ids were that map's). */
export function useGrow(design: (request: ReturnType<typeof growRequest>) => Promise<Designed>, heard: (result: DesignResult) => void, revision: string | null): Grow {
  const [choosing, setChoosing] = useState<'from' | 'to' | null>(null)
  const [path, setPath] = useState<{ from: GrowEnd; to: GrowEnd; seed: number } | null>(null)
  const [pending, setPending] = useState<GrowEnd | null>(null)
  const [buds, setBuds] = useState<Bud[]>([])
  const [scrub, setScrub] = useState<Bud | null>(null)
  const [handle, setHandle] = useState(0.5)
  const [picked, setPicked] = useState<number | null>(null)
  const [running, setRunning] = useState(0)
  // Each path (and each reroll) is a generation: an answer for an older one is dropped.
  const generation = useRef(0)
  const heardRef = useRef(heard)
  heardRef.current = heard
  // The point the handle last asked for: only its sound plays when it lands, never one the handle has moved on from.
  const scrubAt = useRef<number | null>(null)

  const start = (from: GrowEnd, to: GrowEnd) => {
    generation.current += 1
    setPath({ from, to, seed: newSeed() })
    setBuds(fresh())
    setScrub(null)
    setHandle(0.5)
    setPicked(null)
    setChoosing(null)
    setPending(null)
  }
  const close = () => {
    generation.current += 1
    setPath(null)
    setBuds([])
    setScrub(null)
    setPicked(null)
    setChoosing(null)
    setPending(null)
  }
  useEffect(close, [revision]) // eslint-disable-line react-hooks/exhaustive-deps
  // The map closed: an answer still on its way marks nothing and plays nothing.
  useEffect(
    () => () => {
      generation.current += 1
    },
    []
  )

  // Start the next render whenever one is waiting and a slot is free.
  useEffect(() => {
    if (!path) return
    const next = nextToGrow(buds, scrub, running)
    if (!next) return
    const mine = generation.current
    const isScrub = next === scrub
    const mark = (bud: Bud) => (isScrub ? setScrub(current => (current?.at === next.at ? bud : current)) : setBuds(current => current.map(each => (each.at === next.at ? bud : each))))
    mark({ at: next.at, state: 'growing' })
    setRunning(count => count + 1)
    void design(growRequest(path.from.source, path.to.source, next.at, path.seed)).then(answer => {
      setRunning(count => count - 1)
      if (mine !== generation.current) return
      if ('result' in answer) {
        mark({ at: next.at, state: 'ready', result: answer.result })
        if (isScrub && scrubAt.current === next.at) heardRef.current(answer.result)
      } else {
        mark({ at: next.at, state: 'failed', error: answer.error })
      }
    })
  }, [path, buds, scrub, running]) // eslint-disable-line react-hooks/exhaustive-deps

  return {
    choosing,
    // A new vine's first sound, once named, is the one shown and named in the prompt, even with another vine open.
    from: pending ?? path?.from ?? null,
    to: pending ? null : (path?.to ?? null),
    buds,
    handle,
    scrub,
    picked,
    choose: on => {
      setChoosing(on ? 'from' : null)
      setPending(null)
    },
    name: end => {
      if (choosing === 'from') {
        setPending(end)
        setChoosing('to')
      } else if (choosing === 'to' && pending && (end.id ?? end.source.artifact_id) !== (pending.id ?? pending.source.artifact_id)) {
        start(pending, end)
      }
    },
    between: start,
    scrubTo: (at, done) => {
      setHandle(at)
      if (done) setPicked(at)
      // A point a bud already grew (the arrow keys land on 25%, 50% and 75%) is that bud: heard, never grown again.
      const grown = buds.find(each => each.at === at)
      if (grown?.state === 'ready') {
        scrubAt.current = at
        if (done) heardRef.current(grown.result)
        return
      }
      // Latest wins: a release asks for where the handle stopped; a drag asks once the handle is a step from the point
      // last asked for, and only while nothing it asked for is still on its way (that lands, then the drag asks again).
      const asks = done || !scrub || (scrub.state !== 'growing' && scrub.state !== 'waiting' && Math.abs(scrub.at - at) >= SCRUB_STEP)
      if (!asks) return
      scrubAt.current = at
      setScrub({ at, state: 'waiting' })
    },
    pick: at => {
      setPicked(at)
      const bud = at === null ? null : scrub?.at === at ? scrub : buds.find(each => each.at === at)
      if (bud?.state === 'ready') heardRef.current(bud.result)
    },
    reroll: () => {
      if (path) start(path.from, path.to)
    },
    close
  }
}
