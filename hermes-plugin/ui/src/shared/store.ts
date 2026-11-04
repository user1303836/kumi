// One store per UI half over Kumi's read-only routes (contract C4), shared by every view in it. It never writes: it
// reads `/status` on a poll and on events, and reads FOCUS, HISTORY and the map again only when their revision moved.
import type { Focus, History, SetupStatus, SoundMap, Status } from './types.ts'

export type Fetch = <T>(path: string) => Promise<T>

export type Part = 'focus' | 'history' | 'map' | 'setup'

export interface KumiState {
  /** null until the first answer; false when the backend half isn't mounted (the agent plugin is off). */
  available: boolean | null
  /** The last read's failure, other than "not mounted". */
  error: string | null
  status: Status | null
  focus: Focus | null
  history: History | null
  map: SoundMap | null
  setup: SetupStatus | null
  /** Sounds the agent just found (`kumi_sounds`), to light up on the map. */
  found: { revision: string | null; ids: string[]; at: number } | null
}

const EMPTY: KumiState = { available: null, error: null, status: null, focus: null, history: null, map: null, setup: null, found: null }

/** The HTTP status a failed read carries: Desktop's `ctx.rest` rejects with "<status>: <body>". */
export function statusOf(error: unknown): number {
  const text = error instanceof Error ? error.message : String(error ?? '')
  const match = /^(\d{3})\b/.exec(text.trim()) ?? /\b(\d{3})\b/.exec(text)
  return match ? Number(match[1]) : 0
}

export class KumiStore {
  private state: KumiState = EMPTY
  private listeners = new Set<() => void>()
  private inflight = new Map<string, Promise<void>>()
  private wanted = new Map<Part, number>()
  private generation = 0

  constructor(private readonly fetch: Fetch, private readonly historyLimit = 50) {}

  get = (): KumiState => this.state

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  private set(patch: Partial<KumiState>): void {
    this.state = { ...this.state, ...patch }
    for (const fn of [...this.listeners]) fn()
  }

  /** A view that shows `part` asks for it while mounted: the map isn't read until something shows it. */
  want(part: Part): () => void {
    this.wanted.set(part, (this.wanted.get(part) ?? 0) + 1)
    if (!this.state[part]) void this.refresh(part)
    return () => this.wanted.set(part, Math.max(0, (this.wanted.get(part) ?? 1) - 1))
  }

  /** Forget everything (another profile or connection): what's shown must be that profile's. */
  reset(): void {
    this.generation += 1
    this.inflight.clear()
    this.state = EMPTY
    for (const fn of [...this.listeners]) fn()
  }

  private once(key: string, run: () => Promise<void>): Promise<void> {
    const running = this.inflight.get(key)
    if (running) return running
    const generation = this.generation
    const promise = run()
      .catch(() => undefined)
      .finally(() => {
        if (this.generation === generation) this.inflight.delete(key)
      })
    this.inflight.set(key, promise)
    return promise
  }

  /** Read /status, then whatever its revisions say moved. */
  refreshStatus(): Promise<void> {
    return this.once('status', async () => {
      const generation = this.generation
      let status: Status
      try {
        status = await this.fetch<Status>('/status')
      } catch (error) {
        if (generation !== this.generation) return
        if (statusOf(error) === 404) this.set({ available: false, error: null })
        else this.set({ error: error instanceof Error ? error.message : String(error) })
        return
      }
      if (generation !== this.generation) return
      const before = this.state.status?.revisions
      const after = status.revisions ?? {}
      const first = this.state.available !== true
      this.set({ available: true, error: null, status })
      const moved = (part: 'focus' | 'history' | 'map'): boolean => first || !before || before[part] !== after[part]
      const reads: Promise<void>[] = []
      if (moved('focus') || this.state.focus === null) reads.push(this.refresh('focus'))
      if (moved('history') || this.state.history === null) reads.push(this.refresh('history'))
      if ((this.wanted.get('map') ?? 0) > 0 && (moved('map') || this.state.map === null)) reads.push(this.refresh('map'))
      if (first || (this.wanted.get('setup') ?? 0) > 0) reads.push(this.refresh('setup'))
      await Promise.all(reads)
    })
  }

  refresh(part: Part): Promise<void> {
    return this.once(part, async () => {
      const generation = this.generation
      const path = part === 'history' ? `/history?limit=${this.historyLimit}` : `/${part}`
      try {
        const value = await this.fetch<unknown>(path)
        if (generation === this.generation) this.set({ [part]: value } as Partial<KumiState>)
      } catch (error) {
        if (generation !== this.generation) return
        if (statusOf(error) === 404 && part !== 'setup') this.set({ available: false })
      }
    })
  }

  /** The sounds a `kumi_sounds` call found, to light up on the map of the same revision. */
  found(revision: string | null, ids: string[]): void {
    this.set({ found: { revision, ids: ids.slice(0, 200), at: Date.now() } })
  }
}

/** Sound ids out of a `kumi_sounds` result (`{map_revision, items: [{sound_id}]}`), however the event carried it. */
export function foundSounds(result: unknown): { revision: string | null; ids: string[] } | null {
  let value = result
  if (typeof value === 'string') {
    try {
      value = JSON.parse(value)
    } catch {
      return null
    }
  }
  if (!value || typeof value !== 'object' || !('items' in value) || !Array.isArray(value.items)) return null
  const revision = 'map_revision' in value && typeof value.map_revision === 'string' ? value.map_revision : null
  const ids = value.items.flatMap((item: unknown) =>
    item && typeof item === 'object' && 'sound_id' in item && typeof item.sound_id === 'string' ? [item.sound_id] : []
  )
  return { revision, ids }
}
