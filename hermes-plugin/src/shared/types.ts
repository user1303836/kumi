// What Kumi's backend routes answer (contract C4 over the runtime's C3 snapshots), and what `kumi_listen` returns.
// Every field is optional on purpose: the UI reads replies defensively and shows what's there.

export interface Owner {
  profile?: string
  session_id?: string
  pid?: number
}

export interface Liveness {
  connected?: boolean
  stale?: boolean
  age_ms?: number | null
  owner?: Owner | null
}

export interface Project {
  name?: string
  key?: string
  epoch?: number
}

/** A track, device or clip as FOCUS names it: a name, or an object with one. */
export type Named = string | { name?: string; color?: string; kind?: string } | null | undefined

export interface Status extends Liveness {
  daw?: string
  project?: Project
  runtime?: { version?: string; protocol?: string }
  /** What `kumi serve` sees of its DAW part (W1); where setup stands is `/setup`'s answer instead. */
  setup?: { component?: { installed?: boolean; version?: string | null; bundled?: string | null }; daw_open?: boolean }
  capabilities?: string[]
  revisions?: { status?: number | null; focus?: number | null; history?: number | null; map?: string | null }
  /** An operation whose outcome Kumi couldn't confirm (CC-2 R6), cleared by the next completed look. */
  unsure?: { at?: number; op?: string; reason?: string; cleanup?: string } | null
}

export interface Transport {
  playing?: boolean
  tempo?: number
  position?: string
}

export interface Focus extends Liveness {
  project?: Project
  focus?: { track?: Named; device?: Named; clip?: Named; view?: string | null }
  transport?: Transport
  catch_up?: unknown[]
}

export interface HistoryItem {
  id: string
  label?: string
  title?: string
  family?: string
  at?: number
  state?: string
  undo_available?: boolean
  track?: Named
  project_key?: string
  /** Why a row is partial or unsure, and whether Kumi put back what it changed meanwhile (CC-2). */
  reason?: string
  cleanup?: string
}

export interface History extends Liveness {
  items: HistoryItem[]
  seq?: number | null
}

export interface MapGroup {
  name?: string
  color?: [number, number, number] | string
  count?: number
}

export interface MapSound {
  name?: string
  kind?: string
  class?: string
  folder?: string
  duration?: number
}

/** `[sound_id, x, y, group]`, x and y in [0, 1]. */
export type MapPoint = [string, number, number, number]

export interface SoundMap extends Liveness {
  revision: string | null
  current?: string | null
  made?: number
  points: MapPoint[]
  groups: Record<string, MapGroup>
  sounds: Record<string, MapSound>
}

export interface AudioReply {
  id: string
  mime: string
  bytes: number
  data_url: string
}

export interface SetupStatus {
  state?: string
  target?: string
  pinned_version?: string
  download_bytes?: number
  unpacked_bytes?: number
  runtime?: { installed?: boolean; version?: string; path?: string }
  daw?: string
  daw_component?: { state?: string; name?: string; path?: string | null; version?: string | null }
  existing_kumi?: { path?: string; adopted?: boolean } | null
  next_step?: string | null
  message?: string
}

/** `kumi_media::audio::Analysis`, as `kumi_listen` serializes it (camelCase; nulls where nothing was measured). */
export interface Analysis {
  file?: string
  seconds?: number
  analyzed?: { from?: string; to?: string; focus?: string }
  loudness?: { integratedLufs?: number | null; rangeLu?: number | null; truePeakDbtp?: number | null; samplePeakDbfs?: number | null }
  balance?: { bands?: { name?: string; hz?: string; db?: number }[]; tiltDbPerOctave?: number; centroidHz?: number }
  dynamics?: { crestDb?: number }
  tempo?: { bpm?: number; confidence?: number } | null
  key?: { name?: string; confidence?: number } | null
}

/** `kumi_media::audio::Comparison`: current minus reference; paired values are `[current, reference]`. */
export interface Comparison {
  headlines?: string[]
  loudness?: { mineLufs?: number | null; referenceLufs?: number | null; differenceLu?: number | null; truePeak?: [number, number] }
  balance?: { band?: string; hz?: string; difference?: number }[]
  dynamics?: { crestDb?: [number, number] }
}

export interface ListenResult {
  metrics?: Analysis
  artifact_id?: string
  verdict_text?: string
  media_tag?: string
  compare?: { artifact_id?: string; metrics?: Analysis; differences?: Comparison }
  interpretation?: { text?: string; inputs?: string[]; model?: string }
  interpretation_error?: string
  error?: string
}
