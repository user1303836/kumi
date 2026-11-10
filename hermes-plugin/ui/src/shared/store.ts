// One store per UI half over Kumi's backend routes (contract C4), shared by every view in it. It reads `/status` on a
// poll and on events, and reads FOCUS, HISTORY, the map and the palette again when their revision isn't the one it
// holds, or when the runtime's liveness changed (the routes say per read whether anything can be undone). Its only
// writes are the contract's edits that change no project: the sounds that can't play here, hidden from the map, and
// the palette.
import { clean, plainId } from './format.ts'
import { parseListen, roundKey, roundOf, type Round } from './listen.ts'
import { type DesignRequest, type DesignResult, resultOf } from './design.ts'
import type { PaletteAction, PaletteReply } from './palette.ts'
import type { Focus, History, Job, PendingScript, ScriptCode, ScriptLanguage, SetupStatus, SoundMap, Status } from './types.ts'

/** A read (`GET`), or with `init` one of the backend's few writes, a JSON `POST`. */
export type Fetch = <T>(path: string, init?: { method: 'POST'; body: unknown }) => Promise<T>

export type Part = 'focus' | 'history' | 'map' | 'palette' | 'setup'

/** The folder guard's answer for a folder the producer typed or picked (`POST /library/folder`): the folder as Kumi
 *  would keep it and whether it's one already, or the guard's refusal in its own words; `error` when it couldn't ask. */
export interface FolderCheck {
  folder?: string
  known?: boolean
  refusal?: string
  error?: string
}

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
  found: Found | null
  /** Background jobs this window was told of (a library scan), by id: read while they run, kept a minute after. */
  jobs: Record<string, TrackedJob>
  /** The latest `kumi_listen` result a `listen.done` event brought when the transcript showed none (CC-9); `result`
   *  is null when the event left it out for its size. */
  measured: { tool_call_id: string; result: unknown; at: number } | null
  /** Comparisons against each reference this window saw arrive (a mastering pass's rounds), oldest first, by
   *  `roundKey`. Only what arrived while the window was open, so a round is never numbered from a partial past. */
  rounds: Record<string, Round[]>
  /** The script whose approval waits in the chat in front (CC-19), and its saved code once read: null while it's being
   *  read, `false` when it couldn't be (missing, or no longer what the approval named). One at a time: approvals are. */
  script: { pending: PendingScript; code: ScriptCode | null | false } | null
  /** Sounds of the map in hand that couldn't play here: off the map at once (they fade out), and reported, until a map
   *  read no longer has them. */
  leaving: ReadonlySet<string>
  /** The palette the map and the agent share (`/palette`): read while something shows it. */
  palette: PaletteReply | null
  /** What the last palette edit from this window said ("Added 3 sounds to Drums."), or why it was refused. */
  paletteSaid: { text: string; failed: boolean; at: number } | null
  /** The sound-design engine's results (`GET /designs`, then each new one first): null until the Make panel reads them. */
  designs: DesignResult[] | null
}

/** How a design request ended: its result, or why not, in the backend's words, with the HTTP status (400: refused, a
 *  sound picked on an older map among the reasons; 404: this plugin's backend has no sound design yet; 503: two renders
 *  already running, or the runtime isn't set up; 504: it took too long). */
export type Designed = { result: DesignResult } | { error: string; status: number }

export interface TrackedJob extends Job {
  /** When this window saw it finish (ms, this window's clock); null while it runs. */
  ended_at: number | null
  /** A generation's latest `generate.updated` card (CC-13): its reconciled ledger, which the raw `/job` snapshot lacks. */
  card?: unknown
}

/** CC-23: what a find said of one hit: why it matched (at most four short reasons, path-free as Kumi bounds them), and
 *  on a near search how close it is, 0 to 100, relative to the sounds on this map (never a percentage or a confidence). */
export interface FoundMatch {
  why: string[]
  closeness?: number
}

/** A find, in the order Kumi ranked it. `near` is a near search's seeds (CC-23; one sound, or up to sixteen): the hits
 *  are their neighbours, and the seeds themselves aren't among them. `matches` holds only hits that are in `ids`. */
export interface Found {
  revision: string | null
  ids: string[]
  at: number
  near: string[]
  matches: Record<string, FoundMatch>
}

const MOST_REASONS = 4
const REASON_CHARACTERS = 60

/** CC-23's per-hit reasons and closeness, from a `sounds.found` event's `matches` or a `kumi_sounds` result's `items`
 *  (the same fields): only for the found ids, each reason cleaned and bounded, closeness only as an integer 0–100. */
export function foundMatches(entries: unknown, ids: ReadonlySet<string>): Record<string, FoundMatch> {
  // No prototype: a sound id is the runtime's text, and `__proto__` or `constructor` must be a key like any other.
  const out: Record<string, FoundMatch> = Object.create(null)
  if (!Array.isArray(entries)) return out
  for (const entry of entries) {
    if (!entry || typeof entry !== 'object') continue
    const value = entry as Record<string, unknown>
    const id = typeof value.sound_id === 'string' ? value.sound_id : ''
    if (!ids.has(id) || Object.hasOwn(out, id)) continue
    const why = (Array.isArray(value.why) ? value.why : []).map(line => clean(line, REASON_CHARACTERS)).filter(Boolean).slice(0, MOST_REASONS)
    const closeness = value.closeness
    out[id] = { why, ...(typeof closeness === 'number' && Number.isInteger(closeness) && closeness >= 0 && closeness <= 100 ? { closeness } : {}) }
  }
  return out
}

/** How long a finished job stays in view, and how many jobs are followed at once (ids come from events). */
export const JOB_KEPT_MS = 60_000
export const MOST_JOBS = 20
/** Comparisons kept per reference, and references kept: a mastering pass is three rounds at most. */
const MOST_ROUNDS = 6
const MOST_REFERENCES = 8

const EMPTY: KumiState = { available: null, error: null, status: null, focus: null, history: null, map: null, setup: null, found: null, jobs: {}, measured: null, rounds: {}, script: null, leaving: new Set(), palette: null, paletteSaid: null, designs: null }

/** How long failed plays gather before they're reported as one batch, and the most ids one report carries. */
export const HIDE_BATCH_MS = 1200
export const HIDE_MOST = 500

const SHA256 = /^[0-9a-f]{64}$/

/** A `script.pending` payload as the pane can use it, or null: a plain call id, an exact SHA-256 and a known language. */
export function pendingScriptOf(payload: unknown): PendingScript | null {
  if (!payload || typeof payload !== 'object') return null
  const value = payload as Record<string, unknown>
  const id = plainId(value.tool_call_id)
  const language = value.language === 'python' || value.language === 'lua' || value.language === 'device' ? value.language : null
  if (!id || !language || typeof value.sha256 !== 'string' || !SHA256.test(value.sha256)) return null
  const text = (key: string) => (typeof value[key] === 'string' ? (value[key] as string) : undefined)
  const lines = typeof value.lines === 'number' && Number.isInteger(value.lines) && value.lines >= 0 ? value.lines : undefined
  // A device's file is a path the approval names; one printable line, or nothing.
  const file = language === 'device' && typeof value.file === 'string' && /^[^\p{C}]{1,300}$/u.test(value.file) ? value.file : undefined
  return { tool_call_id: id, language, sha256: value.sha256, purpose: text('purpose'), daw: text('daw'), lines, warning: text('warning'), file }
}

function finished(job: Job): boolean {
  return typeof job.state === 'string' && job.state !== 'running'
}

/** The HTTP status a failed read carries: Desktop's `ctx.rest` rejects with "<status>: <body>". */
export function statusOf(error: unknown): number {
  const text = error instanceof Error ? error.message : String(error ?? '')
  const match = /^(\d{3})\b/.exec(text.trim()) ?? /\b(\d{3})\b/.exec(text)
  return match ? Number(match[1]) : 0
}

/** Why a request failed, in the backend's words: Desktop's `ctx.rest` rejects with "<status>: <body>", and FastAPI's
 *  body says why in `detail`; the dashboard's error already is that sentence. */
export function errorText(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error ?? '')
  const json = text.indexOf('{')
  if (json >= 0) {
    try {
      const body: unknown = JSON.parse(text.slice(json))
      if (body && typeof body === 'object' && 'detail' in body && typeof body.detail === 'string') return body.detail
    } catch {
      // not JSON: the text as it came
    }
  }
  return text || 'no answer'
}

/** What's shown once Kumi stops answering: the last known state, nothing live in it and nothing to undo. */
function notLive(state: KumiState): Partial<KumiState> {
  return {
    status: state.status && { ...state.status, connected: false, stale: true },
    focus: state.focus && { ...state.focus, connected: false, stale: true },
    history: state.history && {
      ...state.history,
      connected: false,
      stale: true,
      items: state.history.items.map(item => ({ ...item, undo_available: false }))
    }
  }
}

/** A reply with `heard_at` added: when the runtime was last heard, in this window's clock (the backend's `age_ms`
 *  counted back from now), so "last heard" keeps moving while nothing new is read, whatever the backend's clock. */
function heard<T>(reply: T): T {
  if (!reply || typeof reply !== 'object' || !('age_ms' in reply)) return reply
  const age = (reply as { age_ms?: unknown }).age_ms
  return { ...reply, heard_at: typeof age === 'number' && Number.isFinite(age) ? Date.now() - age : null }
}

export class KumiStore {
  private state: KumiState = EMPTY
  private listeners = new Set<() => void>()
  private reads = new Map<string, { promise: Promise<void>; again: boolean }>()
  private wanted = new Map<Part, number>()
  private generation = 0
  /** Whether this window asked for the engine's earlier results (`readDesignsOnce`): once, however many results this
   *  window made before (a grow renders before the Make panel ever opens). */
  private designsRead = false
  /** `generate.updated` cards for jobs not read yet, by job id; kept on the job once it's read. */
  private cards = new Map<string, unknown>()
  /** Jobs a call started, or an event reported running, not read yet: news even when the first read finds them
   *  finished (a scan of folders already indexed ends within a second, before the pane's read). */
  private started = new Set<string>()
  /** Failed plays not reported yet, by sound id, and the timer that reports them. */
  private toHide = new Map<string, 'gone' | 'unplayable'>()
  private hideTimer: ReturnType<typeof setTimeout> | null = null
  /** `/status`'s count of hidden-sound edits at the last map read: another window's report reads the map again. */
  private hiddenSeq: number | null = null

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
    this.designsRead = false
    this.reads.clear()
    this.cards.clear()
    this.started.clear()
    this.toHide.clear()
    clearTimeout(this.hideTimer ?? undefined)
    this.hideTimer = null
    this.hiddenSeq = null
    this.state = EMPTY
    for (const fn of [...this.listeners]) fn()
  }

  /** Run read `key`, or, while one is running, run it once more after it: a read never answers with what an
   *  older one, begun before the change that asked for it, saw. */
  private once(key: string, run: () => Promise<void>): Promise<void> {
    const running = this.reads.get(key)
    if (running) {
      running.again = true
      return running.promise
    }
    const generation = this.generation
    const entry = { again: false, promise: Promise.resolve() }
    entry.promise = (async () => {
      do {
        entry.again = false
        await run().catch(() => undefined)
      } while (entry.again && generation === this.generation)
    })().finally(() => {
      if (this.reads.get(key) === entry) this.reads.delete(key)
    })
    this.reads.set(key, entry)
    return entry.promise
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
        else this.set({ ...notLive(this.state), error: error instanceof Error ? error.message : String(error) })
        return
      }
      if (generation !== this.generation) return
      const before = this.state.status
      const after = status.revisions ?? {}
      const first = this.state.available !== true
      // Any change of liveness (answering, quiet, or stopped) re-reads what the routes judge by it.
      const liveness = !before || before.connected !== status.connected || before.stale !== status.stale || before.running !== status.running
      this.set({ available: true, error: null, status: heard(status) })
      const held = { focus: this.state.focus?.seq ?? null, history: this.state.history?.seq ?? null, map: this.state.map?.revision ?? null, palette: this.state.palette?.seq ?? null }
      const moved = (part: 'focus' | 'history' | 'map' | 'palette'): boolean => this.state[part] === null || (after[part] ?? null) !== held[part]
      const hidden = typeof after.hidden === 'number' ? after.hidden : 0
      const hiddenMoved = this.hiddenSeq !== null && hidden !== this.hiddenSeq
      this.hiddenSeq = hidden
      const reads: Promise<void>[] = []
      if (liveness || moved('focus')) reads.push(this.refresh('focus'))
      if (liveness || moved('history')) reads.push(this.refresh('history'))
      if ((this.wanted.get('map') ?? 0) > 0 && (moved('map') || hiddenMoved)) reads.push(this.refresh('map'))
      // The palette names its sounds by the current map's ids: a new map revision reads it again too.
      if ((this.wanted.get('palette') ?? 0) > 0 && (moved('palette') || (this.state.map !== null && moved('map')))) reads.push(this.refresh('palette'))
      // Setup moves while Kumi isn't connected (a step a chat just installed, the DAW restarted to load Kumi's part),
      // and the pane's next step is read from it: read it on every poll until Kumi connects, and when liveness changes.
      if (first || liveness || !status.connected || (this.wanted.get('setup') ?? 0) > 0) reads.push(this.refresh('setup'))
      await Promise.all(reads)
    })
  }

  refresh(part: Part): Promise<void> {
    return this.once(part, async () => {
      const generation = this.generation
      const path = part === 'history' ? `/history?limit=${this.historyLimit}` : `/${part}`
      try {
        const value = await this.fetch<unknown>(path)
        if (generation !== this.generation) return
        this.set({ [part]: heard(value) } as Partial<KumiState>)
        if (part === 'map') this.mapRead()
      } catch (error) {
        if (generation !== this.generation) return
        if (statusOf(error) === 404 && part !== 'setup') this.set({ available: false })
      }
    })
  }

  /** A map read: sounds it left out (hidden, or gone in a new revision) are done leaving. One still on it waits there,
   *  off the map, while its report is pending or on its way. */
  private mapRead(): void {
    if (this.state.leaving.size === 0) return
    const on = new Set((this.state.map?.points ?? []).map(point => point[0]))
    const leaving = new Set([...this.state.leaving].filter(id => on.has(id) && (this.toHide.has(id) || this.hideTimer !== null)))
    if (leaving.size !== this.state.leaving.size) this.set({ leaving })
  }

  /** A sound of the map in hand couldn't play here (`playFailure`): off the map now, and reported with the others that
   *  fail within `HIDE_BATCH_MS`, so the next map read leaves them out and counts them. Never a notice per sound. */
  hide(id: string, why: 'gone' | 'unplayable'): void {
    const map = this.state.map
    if (!map?.revision || this.state.leaving.has(id) || !map.points.some(point => point[0] === id)) return
    this.toHide.set(id, why)
    this.set({ leaving: new Set([...this.state.leaving, id]) })
    if (this.hideTimer === null) this.hideTimer = setTimeout(() => void this.reportHidden(), HIDE_BATCH_MS)
  }

  private async reportHidden(): Promise<void> {
    this.hideTimer = null
    const revision = this.state.map?.revision
    const batch = [...this.toHide].map(([id, why]) => ({ id, why }))
    this.toHide.clear()
    if (!revision || batch.length === 0) return
    const generation = this.generation
    for (let at = 0; at < batch.length; at += HIDE_MOST) {
      try {
        await this.fetch('/map/hidden', { method: 'POST', body: { revision, ids: batch.slice(at, at + HIDE_MOST) } })
      } catch {
        // Not reported: the sounds stay off the map in this window until it closes, and fail again quietly elsewhere.
      }
      if (generation !== this.generation) return
    }
    await this.refresh('map')
  }

  /** "Show them": every hidden sound goes back on the map. */
  async showHidden(): Promise<void> {
    const generation = this.generation
    this.toHide.clear()
    clearTimeout(this.hideTimer ?? undefined)
    this.hideTimer = null
    try {
      await this.fetch('/map/hidden', { method: 'POST', body: { action: 'clear' } })
    } catch {
      return
    }
    if (generation !== this.generation) return
    this.set({ leaving: new Set() })
    await this.refresh('map')
  }

  /** The sample folders Kumi may read (`GET /library/folders`), for the Sample folders control; null when the read
   *  fails (an older backend half, or the agent plugin off). */
  async libraryFolders(): Promise<null | string[]> {
    try {
      const answer = await this.fetch<{ folders?: unknown }>('/library/folders')
      return Array.isArray(answer?.folders) ? answer.folders.filter((folder): folder is string => typeof folder === 'string') : null
    } catch {
      return null
    }
  }

  /** What the plugin's folder guard says of a typed folder. Nothing is added: the request to add it goes to the chat. */
  async checkFolder(folder: string): Promise<FolderCheck> {
    try {
      const answer = await this.fetch<FolderCheck>('/library/folder', { method: 'POST', body: { folder } })
      if (typeof answer?.folder === 'string') return { folder: answer.folder, known: answer.known === true }
      return { refusal: clean(typeof answer?.refusal === 'string' ? answer.refusal : '', 400) }
    } catch (error) {
      return { error: clean(errorText(error), 300) }
    }
  }

  /** One palette edit (`POST /palette`): the palette as the backend now has it, at once, and its sentence (with `also`
   *  after it) as `paletteSaid`; a refusal's reason there instead. True when the edit was made. */
  async paletteDo(action: PaletteAction, also?: string): Promise<boolean> {
    const generation = this.generation
    try {
      const answer = await this.fetch<{ said?: unknown; palette?: PaletteReply }>('/palette', { method: 'POST', body: action })
      if (generation !== this.generation) return false
      const said = [typeof answer?.said === 'string' ? clean(answer.said, 200) : '', also ?? ''].filter(Boolean).join(' ')
      this.set({ ...(answer?.palette ? { palette: answer.palette } : {}), paletteSaid: said ? { text: said, failed: false, at: Date.now() } : null })
      return true
    } catch (error) {
      if (generation === this.generation) this.set({ paletteSaid: { text: clean(errorText(error), 200), failed: true, at: Date.now() } })
      return false
    }
  }

  /** The engine's recent results, newest first (`GET /designs`), for the Make panel's history, after the ones this
   *  window made already (a grow's buds, a make before the read answered), each once. None when the backend has no
   *  sound design (an older plugin) or can't read them. */
  async readDesigns(): Promise<void> {
    const generation = this.generation
    try {
      const answer = await this.fetch<{ designs?: unknown }>('/designs')
      if (generation !== this.generation) return
      const history = (Array.isArray(answer?.designs) ? answer.designs : []).map(resultOf).filter((result): result is DesignResult => result !== null)
      const made = this.state.designs ?? []
      const have = new Set(made.map(result => result.artifact_id))
      this.set({ designs: [...made, ...history.filter(result => !have.has(result.artifact_id))] })
    } catch {
      if (generation === this.generation && this.state.designs === null) this.set({ designs: [] })
    }
  }

  /** `readDesigns`, the first time this window asks (the Make panel opening); later opens read nothing more. */
  readDesignsOnce(): Promise<void> {
    if (this.designsRead) return Promise.resolve()
    this.designsRead = true
    return this.readDesigns()
  }

  /** Make a sound (`POST /design`): the result goes first in `designs`. Renders are quick (well under a second for
   *  one-shots, a few for a long smear), so this waits for the answer. */
  async design(request: DesignRequest): Promise<Designed> {
    const generation = this.generation
    try {
      const answer = await this.fetch<unknown>('/design', { method: 'POST', body: request })
      const result = resultOf(answer)
      if (!result) return { error: 'Kumi sent no sound back.', status: 0 }
      if (generation === this.generation) this.set({ designs: [result, ...(this.state.designs ?? []).filter(other => other.artifact_id !== result.artifact_id)] })
      return { result }
    } catch (error) {
      const status = error && typeof error === 'object' && 'status' in error && typeof error.status === 'number' ? error.status : statusOf(error)
      return { error: clean(errorText(error), 300), status }
    }
  }

  /** The sounds a `kumi_sounds` call found, to light up on the map of the same revision, with what it said of each
   *  (CC-23: an event's `matches` or a result's `items`) and a near search's seeds (one id, or a list of them). */
  found(revision: string | null, ids: string[], said: { near?: unknown; matches?: unknown } = {}): void {
    const kept = ids.slice(0, 200)
    const seeds = Array.isArray(said.near) ? said.near : [said.near]
    const near = seeds.filter((seed): seed is string => typeof seed === 'string' && seed !== '').slice(0, 16)
    this.set({ found: { revision, ids: kept, at: Date.now(), near, matches: foundMatches(said.matches, new Set(kept)) } })
  }

  /** A `kumi_listen` result to show in the Measurements tab (CC-9: the transcript showed none). */
  measure(toolCallId: string, result: unknown): void {
    this.set({ measured: { tool_call_id: toolCallId, result: result ?? null, at: Date.now() } })
  }

  /** A `kumi_listen` result as it arrived (its call's `tool.complete` or `listen.done`): one against a reference is
   *  that reference's next round, whether it named the reference's file or the copy Kumi kept of it. The same audio
   *  twice (both signals for one call) counts once. */
  noteListen(result: unknown): void {
    const parsed = parseListen(result)
    const key = parsed ? roundKey(parsed, this.state.rounds) : null
    const id = typeof parsed?.artifact_id === 'string' ? parsed.artifact_id : null
    if (!parsed || !key || !id) return
    const before = this.state.rounds[key] ?? []
    if (before.some(round => round.id === id)) return
    // A few references, each with a few rounds: a pass is three at most, and older ones drop off.
    const rounds = { ...this.state.rounds, [key]: [...before, roundOf(parsed, id, this.state.rounds)].slice(-MOST_ROUNDS) }
    const keys = Object.keys(rounds)
    if (keys.length > MOST_REFERENCES) delete rounds[keys[0]]
    this.set({ rounds })
  }

  /** A `script.pending` event for the chat in front (CC-19): show it, then read its saved code. An event that names
   *  no plain call id, exact SHA-256 or known language shows nothing. */
  scriptPending(payload: unknown): void {
    const pending = pendingScriptOf(payload)
    if (!pending) return
    this.set({ script: { pending, code: null } })
    const generation = this.generation
    void this.readScript(pending.sha256, pending.language).then(code => {
      // Only for the same pending script: it may have settled, or another taken its place, meanwhile.
      if (generation !== this.generation || this.state.script?.pending !== pending) return
      this.set({ script: { pending, code: code ?? false } })
    })
  }

  /** The pending script's approval settled (`script.settled`, or its call's `tool.complete`): it isn't waiting now. */
  scriptSettled(toolCallId: unknown): void {
    const id = plainId(toolCallId)
    if (id && this.state.script?.pending.tool_call_id === id) this.set({ script: null })
  }

  /** A saved script's code by its SHA-256 (`/script`, which re-hashes the file): for the pending script, and for a
   *  script row's "Show code". Null when it can't be read, or when what came back isn't that code. */
  async readScript(sha256: string, language: ScriptLanguage): Promise<ScriptCode | null> {
    if (!SHA256.test(sha256)) return null
    try {
      const read = await this.fetch<ScriptCode>(`/script?sha=${sha256}&language=${language}`)
      return read && read.sha256 === sha256 && typeof read.code === 'string' ? read : null
    } catch {
      return null
    }
  }

  /** Follow job `id` (from a tool's result or a `job.progress` event): read it now, then while it runs. `started` when
   *  the source says it was running: a call that started it, or an event that reported it running. */
  trackJob(id: unknown, started = false): void {
    const jobId = plainId(id)
    // Shown only once this profile's backend answers for it (another profile's job 404s and never shows, even from an
    // event that names no profile), and at most MOST_JOBS at a time.
    if (!jobId || this.state.jobs[jobId] || Object.keys(this.state.jobs).length >= MOST_JOBS) return
    if (started) {
      this.started.add(jobId)
      if (this.started.size > MOST_JOBS) this.started.delete(this.started.values().next().value as string)
    }
    void this.refreshJob(jobId)
  }

  /** A `generate.updated` card (CC-13): keep it on its job, since its ledger is reconciled and the raw `/job` snapshot
   *  has none, and read the job as news. Unlike a job first met finished elsewhere, a generation the event reports
   *  shows even when it has already ended: with tool progress off, this row is where it shows. */
  noteGeneration(card: unknown): void {
    const id = jobOf(card)
    if (!id) return
    const before = this.state.jobs[id]
    if (before) this.set({ jobs: { ...this.state.jobs, [id]: { ...before, card } } })
    else if (Object.keys(this.state.jobs).length >= MOST_JOBS) return
    else {
      this.cards.delete(id)
      this.cards.set(id, card)
      // Bounded like the jobs themselves: the oldest card waiting for its first read goes first.
      if (this.cards.size > MOST_JOBS) this.cards.delete(this.cards.keys().next().value as string)
    }
    void this.refreshJob(id)
  }

  /** Read every job still running and forget the finished ones kept long enough. While the runtime is quiet nothing
   *  moves, so nothing's read; once it has stopped, each job is read a last time, and one still "running" then ended
   *  with it. */
  refreshJobs(now = Date.now()): Promise<void> {
    const kept = Object.entries(this.state.jobs).filter(([, job]) => job.ended_at === null || now - job.ended_at < JOB_KEPT_MS)
    if (kept.length !== Object.keys(this.state.jobs).length) this.set({ jobs: Object.fromEntries(kept) })
    const stopped = this.state.status?.stale === true && this.state.status.running === false
    if (this.state.status?.stale && !stopped) return Promise.resolve()
    return Promise.all(kept.filter(([, job]) => job.ended_at === null).map(([id]) => this.refreshJob(id, stopped))).then(() => undefined)
  }

  private refreshJob(id: string, stopped = false): Promise<void> {
    return this.once(`job:${id}`, async () => {
      const generation = this.generation
      try {
        const read = await this.fetch<Job>(`/job?id=${encodeURIComponent(id)}`)
        if (generation !== this.generation) return
        const before = this.state.jobs[id]
        // A job first seen finished isn't news: it isn't shown as if it had just ended. One a call started or an event
        // reported running is, and so is one a generation event reported (its card waits here until the first read),
        // whichever read gets there first.
        const news = this.started.delete(id) || this.cards.has(id)
        if (!before && finished(read) && !news) return
        const card = before?.card ?? this.cards.get(id)
        this.cards.delete(id)
        const job: Job = stopped && !finished(read) ? { ...read, state: 'interrupted' } : read
        const ended_at = finished(job) ? (before?.ended_at ?? Date.now()) : null
        this.set({ jobs: { ...this.state.jobs, [id]: { ...job, job_id: id, ended_at, ...(card === undefined ? {} : { card }) } } })
      } catch (error) {
        if (generation !== this.generation || statusOf(error) !== 404) return
        this.cards.delete(id)
        if (!this.state.jobs[id]) return
        // No such job in this profile (long gone): nothing to follow.
        const { [id]: _gone, ...rest } = this.state.jobs
        this.set({ jobs: rest })
      }
    })
  }
}

/** Whether setup only lacks the DAW's part while Kumi is connected through one that's current. An install setup didn't
 *  make (an adopted Kumi's, say) evidently works, so no surface asks to install it then, or shows setup's "install
 *  Kumi's part" as the word of the moment. The runtime's own look at the part (`status.setup.component`) decides: the
 *  bundled version installed, or no report at all (an older runtime), hides the step. An older part, or none
 *  installed, still asks: right after a bridge bump, that update is the next step (rgzm, #1019). The settings page
 *  says ready and shows the part as in use then, with setup's own facts (its path, the runtime) as they are. */
export function dawPartWorks(state: KumiState): boolean {
  const setup = state.setup
  const onlyDawPart = setup?.state === 'needs_daw_component' || (setup?.state === 'ready' && setup.next_step === 'install_daw_component')
  if (!onlyDawPart || !state.status?.connected) return false
  const component = state.status.setup?.component
  if (!component) return true
  return component.installed === true && typeof component.version === 'string' && component.version === component.bundled
}

/** The job a tool's result started or reports (`{job_id}`), however the event carried the result. */
export function jobOf(result: unknown): string | null {
  let value = result
  if (typeof value === 'string') {
    try {
      value = JSON.parse(value)
    } catch {
      return null
    }
  }
  return value && typeof value === 'object' && 'job_id' in value ? plainId(value.job_id) : null
}

/** Sound ids out of a `kumi_sounds` result (`{map_revision, items: [{sound_id}]}`), however the event carried it, with
 *  the items themselves (their CC-23 `why` and `closeness`) and a near search's seed. */
export function foundSounds(result: unknown): { revision: string | null; ids: string[]; near: unknown; matches: unknown[] } | null {
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
  return { revision, ids, near: 'near' in value ? value.near : null, matches: value.items }
}
