// React glue over the store, and the local audition player both halves share.
import { useCallback, useEffect, useRef, useSyncExternalStore } from 'react'

import type { AudioReply } from './types.ts'
import { statusOf, type KumiState, type KumiStore, type Part } from './store.ts'

/** The store's state, re-rendered on every change. */
export function useKumi(store: KumiStore): KumiState {
  return useSyncExternalStore(store.subscribe, store.get, store.get)
}

/** Ask the store for `part` while this component is mounted (the map is read only while something shows it). */
export function useWant(store: KumiStore, part: Part): void {
  useEffect(() => store.want(part), [store, part])
}

/** How much audio the window keeps for replays, counted in data-URL characters (about bytes). */
export const AUDITION_CACHE_CHARS = 48 * 1024 * 1024
/** A hover's preview, as native Kumi's map plays one: the first two seconds, cut with a 0.2 s fade. */
export const PREVIEW_SECONDS = 2
export const PREVIEW_FADE_SECONDS = 0.2
/** How long the pointer rests on a sound before its preview loads: a sweep across the map loads nothing. */
export const HOVER_REST_MS = 70

/**
 * Plays a sound or artifact Kumi published, in the window only: an audition is a read (the bytes come from the
 * backend's `/audio` route), never a change in the DAW. One sound at a time; recent ones are kept for replays, up to
 * `AUDITION_CACHE_CHARS`, and forgotten when the profile changes (ids repeat across profiles).
 */
export class Audition {
  private audio: HTMLAudioElement | null = null
  private cache = new Map<string, string>()
  private cached = 0
  private loading = new Map<string, Promise<string>>()
  private generation = 0
  /** Moves on with every play and stop: a play whose turn has passed never starts its sound. */
  private turn = 0
  /** Whether what plays is a hover's preview: leaving the map stops that, never a sound the producer clicked. */
  private previewing = false
  private listeners = new Set<() => void>()
  playing: string | null = null

  constructor(private readonly load: (id: string) => Promise<AudioReply>, private readonly limit = AUDITION_CACHE_CHARS) {}

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  private changed(playing: string | null): void {
    this.playing = playing
    for (const fn of [...this.listeners]) fn()
  }

  /** The data URL for `id`: from the cache, the load already under way, or the backend. */
  url(id: string): Promise<string> {
    const cached = this.cache.get(id)
    if (cached) {
      // Most recently used last, so the oldest goes first.
      this.cache.delete(id)
      this.cache.set(id, cached)
      return Promise.resolve(cached)
    }
    const loading = this.loading.get(id)
    if (loading) return loading
    const generation = this.generation
    const promise = this.load(id)
      .then(reply => {
        if (typeof reply?.data_url !== 'string' || !reply.data_url.startsWith('data:')) throw new Error('Kumi sent no audio for that sound.')
        if (generation === this.generation) this.keep(id, reply.data_url)
        return reply.data_url
      })
      .finally(() => {
        if (this.loading.get(id) === promise) this.loading.delete(id)
      })
    this.loading.set(id, promise)
    return promise
  }

  private keep(id: string, url: string): void {
    if (url.length > this.limit) return
    this.cache.set(id, url)
    this.cached += url.length
    for (const [oldest, kept] of this.cache) {
      if (this.cached <= this.limit) break
      this.cache.delete(oldest)
      this.cached -= kept.length
    }
  }

  /** Another profile or connection: stop, and forget every sound of the last one. */
  reset(): void {
    this.stop()
    this.generation += 1
    this.cache.clear()
    this.cached = 0
    this.loading.clear()
  }

  /** Play `id` (stopping whatever plays). A later `play` or `stop` takes over even while this one still loads, so
   *  there's never more than one sound playing, and never one that can't be stopped. A play taken over that way ends
   *  quietly, whatever happened to it: only the sound still wanted can fail. `preview` plays a hover's preview: the
   *  first `preview` seconds, faded out. */
  async play(id: string, options: { preview?: number } = {}): Promise<void> {
    this.stop()
    const turn = this.turn
    this.changed(id)
    try {
      const url = await this.url(id)
      if (turn !== this.turn) return
      const audio = new Audio(url)
      this.audio = audio
      this.previewing = options.preview !== undefined
      audio.onended = () => {
        if (this.audio === audio) this.stop()
      }
      if (options.preview !== undefined) this.cut(audio, turn, options.preview)
      await audio.play()
    } catch (error) {
      if (turn !== this.turn) return
      this.stop()
      throw error
    }
  }

  /** A preview's end: past `seconds` less the fade, the volume falls to nothing over the fade, then it stops. */
  private cut(audio: HTMLAudioElement, turn: number, seconds: number): void {
    const fade = PREVIEW_FADE_SECONDS * 1000
    setTimeout(() => {
      const start = performance.now()
      const step = () => {
        if (turn !== this.turn || this.audio !== audio) return
        const left = 1 - (performance.now() - start) / fade
        if (left <= 0) return this.stop()
        audio.volume = left
        setTimeout(step, 16)
      }
      step()
    }, Math.max(0, seconds * 1000 - fade))
  }

  stop(): void {
    this.turn += 1
    this.previewing = false
    if (this.audio) {
      this.audio.pause()
      this.audio.onended = null
      this.audio = null
    }
    if (this.playing !== null) this.changed(null)
  }

  /** The pointer left the map: a hover's preview stops; a sound the producer clicked plays on. */
  stopPreview(): void {
    if (this.previewing) this.stop()
  }

  dispose(): void {
    this.reset()
    this.listeners.clear()
  }
}

export function usePlaying(audition: Audition): string | null {
  return useSyncExternalStore(audition.subscribe, () => audition.playing, () => audition.playing)
}

/** Sounds play as the pointer rests on them (native Kumi's map, `h`): `onHover` from the map plays each one's preview
 *  through `play` once the pointer has rested `HOVER_REST_MS` on it, and stops the preview when it leaves the map.
 *  Nothing happens while `enabled` is off. */
export function useHoverPlay(audition: Audition, enabled: boolean, play: (id: string) => void): (id: string | null) => void {
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const played = useRef(play)
  played.current = play
  useEffect(() => () => clearTimeout(timer.current), [])
  useEffect(() => {
    if (!enabled) audition.stopPreview()
  }, [audition, enabled])
  return useCallback(
    (id: string | null) => {
      clearTimeout(timer.current)
      if (!enabled) return
      if (id === null) {
        audition.stopPreview()
        return
      }
      timer.current = setTimeout(() => played.current(id), HOVER_REST_MS)
    },
    [audition, enabled]
  )
}

/** Why a play failed, for what to do about it: `gone`, the backend has no audio for the id (the file went, or left the
 *  approved folders); `unplayable`, the audio came but this window can't decode it; `blocked`, the window may not play
 *  sound until the producer clicks or presses a key in it; `other` (Kumi's backend didn't answer, a timeout), worth
 *  saying. The first two take the sound off the map quietly, never with a notice per sound. */
export type PlayFailure = 'blocked' | 'gone' | 'other' | 'unplayable'

export function playFailure(error: unknown): PlayFailure {
  const name = error instanceof Error ? error.name : ''
  if (name === 'NotSupportedError') return 'unplayable'
  if (name === 'NotAllowedError') return 'blocked'
  // The dashboard's `fetchJSON` puts the status on its error and says why in the message; Desktop's `ctx.rest` puts
  // it at the start of the message.
  const status = error && typeof error === 'object' && 'status' in error ? error.status : statusOf(error)
  return status === 404 ? 'gone' : 'other'
}
