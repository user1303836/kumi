// React glue over the store, and the local audition player both halves share.
import { useEffect, useSyncExternalStore } from 'react'

import type { AudioReply } from './types.ts'
import type { KumiState, KumiStore, Part } from './store.ts'

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
   *  there's never more than one sound playing, and never one that can't be stopped. */
  async play(id: string): Promise<void> {
    this.stop()
    const turn = this.turn
    this.changed(id)
    try {
      const url = await this.url(id)
      if (turn !== this.turn) return
      const audio = new Audio(url)
      this.audio = audio
      audio.onended = () => {
        if (this.audio === audio) this.stop()
      }
      await audio.play()
    } catch (error) {
      if (turn === this.turn) this.stop()
      throw error
    }
  }

  stop(): void {
    this.turn += 1
    if (this.audio) {
      this.audio.pause()
      this.audio.onended = null
      this.audio = null
    }
    if (this.playing !== null) this.changed(null)
  }

  dispose(): void {
    this.reset()
    this.listeners.clear()
  }
}

export function usePlaying(audition: Audition): string | null {
  return useSyncExternalStore(audition.subscribe, () => audition.playing, () => audition.playing)
}
