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

/**
 * Plays a sound or artifact Kumi published, in the window only: an audition is a read (the bytes come from the
 * backend's `/audio` route), never a change in the DAW. One sound at a time; recent ones are kept for replays.
 */
export class Audition {
  private audio: HTMLAudioElement | null = null
  private cache = new Map<string, string>()
  private listeners = new Set<() => void>()
  playing: string | null = null

  constructor(private readonly load: (id: string) => Promise<AudioReply>) {}

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  private changed(playing: string | null): void {
    this.playing = playing
    for (const fn of [...this.listeners]) fn()
  }

  /** The data URL for `id`, from the cache or the backend. */
  async url(id: string): Promise<string> {
    const cached = this.cache.get(id)
    if (cached) return cached
    const reply = await this.load(id)
    if (typeof reply?.data_url !== 'string' || !reply.data_url.startsWith('data:')) throw new Error('Kumi sent no audio for that sound.')
    this.cache.set(id, reply.data_url)
    // A dozen recent sounds at most: data URLs of whole files add up.
    while (this.cache.size > 12) this.cache.delete(this.cache.keys().next().value as string)
    return reply.data_url
  }

  async play(id: string): Promise<void> {
    this.stop()
    this.changed(id)
    try {
      const audio = new Audio(await this.url(id))
      if (this.playing !== id) return
      this.audio = audio
      audio.onended = () => {
        if (this.audio === audio) this.stop()
      }
      await audio.play()
    } catch (error) {
      if (this.playing === id) this.stop()
      throw error
    }
  }

  stop(): void {
    if (this.audio) {
      this.audio.pause()
      this.audio.onended = null
      this.audio = null
    }
    if (this.playing !== null) this.changed(null)
  }

  dispose(): void {
    this.stop()
    this.cache.clear()
    this.listeners.clear()
  }
}

export function usePlaying(audition: Audition): string | null {
  return useSyncExternalStore(audition.subscribe, () => audition.playing, () => audition.playing)
}
