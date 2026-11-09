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
  /** False once the runtime's last word was that it stopped (a stop, not a silence); null when it hasn't said. */
  running?: boolean | null
  owner?: Owner | null
  /** When the runtime was last heard, in this window's clock (ms): the store sets it from `age_ms` on receipt. */
  heard_at?: number | null
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
  project?: Project | null
  runtime?: { version?: string; protocol?: string }
  /** What `kumi serve` sees of its DAW part (W1); where setup stands is `/setup`'s answer instead. */
  setup?: { component?: { installed?: boolean; version?: string | null; bundled?: string | null }; daw_open?: boolean }
  capabilities?: string[]
  /** Each snapshot's revision; `hidden` counts the edits to the sounds hidden from the map. */
  revisions?: { status?: number | null; focus?: number | null; history?: number | null; map?: string | null; hidden?: number | null }
  /** An operation whose outcome Kumi couldn't confirm (CC-2 R6), cleared by the next completed look. */
  unsure?: { at?: number; op?: string; reason?: string; cleanup?: string } | null
}

/** The DAW's transport in `focus.json` (CC-7). `position` (in beats) and `line` are where the playhead was when the
 *  snapshot was written, so they're never shown: a snapshot isn't rewritten as the playhead moves. */
export interface Transport {
  playing?: boolean
  tempo?: number | null
  position?: number | null
  beatsPerBar?: number | null
  /** The DAW's words for its time beside the tempo: `7/8`, `LPB 4`. */
  meter?: string | null
  line?: number | null
}

/** A view as Kumi names it: its kind (`launcher`, `timeline`, `device-chain`…) and the DAW's own word for it. */
export interface FocusViewName {
  kind?: string
  label?: string
}

/** What the producer is looking at, as Kumi's `DawFocus` serializes it (camelCase). Live fills its own fields (`view`,
 *  `scene`, `clip`); REAPER and Renoise the neutral ones (`mainView`, `section`, `item`). Names are data. */
export interface DawFocus {
  track?: Named
  device?: string | null
  clip?: string | null
  scene?: string | null
  view?: string | null
  mainView?: FocusViewName | null
  editorView?: FocusViewName | null
  item?: { kind?: string; name?: string | null } | null
  section?: { kind?: string; name?: string | null } | null
  parameter?: { name?: string; value?: string | null; owner?: string | null } | null
  /** Why the DAW answers late ("a dialog is open"), while it does. */
  busy?: string | null
}

/** What changed in a project while Kumi wasn't looking, as Kumi's `CatchUp` serializes it. */
export interface CatchUp {
  set?: string
  lastSeenAt?: number
  lines?: string[]
  /** Changes not listed. */
  more?: number
  afterReconnect?: boolean | null
}

export interface Focus extends Liveness {
  project?: Project | null
  focus?: DawFocus | null
  transport?: Transport | null
  catch_up?: CatchUp[] | null
  seq?: number | null
}

export interface HistoryItem {
  id: string
  /** The serve-made id every row of one `change` call shares (CC-3 A2); absent on a change of a single row. */
  change_id?: string
  label?: string
  title?: string
  family?: string
  at?: number
  state?: string
  undo_available?: boolean
  track?: Named
  project_key?: string
  /** The runtime that made it: an earlier runtime's rows come back as the record, with `undo_available` false. */
  runtime_generation?: string
  /** Why a row is partial or unsure, and whether Kumi put back what it changed meanwhile (CC-2). */
  reason?: string
  cleanup?: string
  /** What `kumi serve` (#965) writes in plain words: why an undo didn't happen, or what's unsure. */
  note?: string
  /** A row only the DAW's own undo can take back (#1101's device loads, whose undo Kumi refuses): `next` while it's
   *  the DAW's next undo, `buried` once later work sits above it there, so the DAW's undo would take that back first. */
  daw_undo?: 'next' | 'buried'
  /** A script's row (CC-19, `Script: <purpose>`): which saved code it ran, so the pane can show it, after a restart
   *  too, through `/script`. */
  script?: { sha256?: string; language?: string }
  /** The copy of the project Kumi kept beside it before a step that could lose material (Live's third step or a
   *  delete, Renoise's sample load): its file's name and why. Never a path (serve keeps those). */
  copy?: { file?: string; why?: string }
}

/** A pending code-class approval's language: a script's (CC-19), or `device`, a Max for Live device's whole spec as
 *  JSON (CC-22), its GenExpr code and controls together under one hash. */
export type ScriptLanguage = 'python' | 'lua' | 'device'

/** A device's spec as the plugin saved it (CC-22 22b): what its approval named, controls and code together. */
export interface DeviceSpec {
  name?: string
  about?: string
  controls?: { name?: string; type?: string; min?: number; max?: number; default?: number | string | boolean; unit?: string; options?: string[] }[]
  code: string
}

/** `plugin.kumi.script.pending` (CC-19): a script whose approval waits. The code never travels in an event: the pane
 *  reads it through `/script` by its SHA-256, from the file the plugin saved when it asked. */
export interface PendingScript {
  tool_call_id: string
  language: ScriptLanguage
  sha256: string
  /** What Hermes's model says the script does: its claim, shown as one. The code is the truth. */
  purpose?: string
  daw?: string
  lines?: number
  /** The approval's own warning, word for word. */
  warning?: string
  /** A device's new file in the User Library, as the approval names it (CC-22). */
  file?: string
}

/** `/script`'s answer: the saved code, re-hashed against its name before it was sent. */
export interface ScriptCode {
  sha256: string
  language: ScriptLanguage
  code: string
  lines: number
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
  /** CC-23c: the tempo, key and root note Kumi indexed at the folder's last scan (from the audio, or a file's or
   *  folder's name); null where the index has none. */
  bpm?: number | null
  key?: string | null
  note?: string | null
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
  /** How many of this revision's sounds are hidden (their audio couldn't be played here), left out of `points`. */
  hidden?: number
}

export interface AudioReply {
  id: string
  mime: string
  bytes: number
  data_url: string
}

/** A background job as `/job` answers it (#947's `jobs/<id>.json`): a library scan, say. */
export interface Job {
  job_id?: string
  /** What it does: `scan`. */
  kind?: string
  /** `running`, then `done`, `failed`, `cancelled`, or `interrupted` when the runtime stopped first. */
  state?: string
  /** How far it got; `message` names what it's on (MCP progress's word). */
  progress?: { done?: number; total?: number; message?: string } | null
  result?: unknown
  error?: { code?: string; reason?: string } | null
}

export interface SetupStatus {
  state?: string
  target?: string
  pinned_version?: string
  download_bytes?: number
  unpacked_bytes?: number
  runtime?: { installed?: boolean; version?: string; path?: string }
  daw?: string
  /** Whether a DAW is chosen in Kumi's settings (`/setup` adds it). Unset, setup reads as its default DAW. */
  daw_chosen?: boolean
  daw_component?: { state?: string; name?: string; path?: string | null; version?: string | null }
  existing_kumi?: { path?: string; adopted?: boolean } | null
  /** CC-10: whether Kumi's runtime can decode MP3, FLAC and other compressed audio, with the line to show. */
  compressed_audio?: { state?: 'available' | 'changed' | 'gone' | 'not_allowed'; message?: string } | null
  next_step?: string | null
  message?: string
}

/** `kumi_media::audio::Analysis`, as `kumi_listen` serializes it (camelCase; nulls where nothing was measured). */
export interface Analysis {
  file?: string
  seconds?: number
  analyzed?: { from?: string; to?: string; focus?: string }
  loudness?: { integratedLufs?: number | null; shortTermMaxLufs?: number | null; rangeLu?: number | null; truePeakDbtp?: number | null; samplePeakDbfs?: number | null }
  /** Each band's `db` is its share of the whole file's energy, so two files' bands compare level-matched. `width` is the
   *  band's side share (0 is mono). */
  balance?: { bands?: { name?: string; hz?: string; db?: number; width?: number }[]; tiltDbPerOctave?: number; centroidHz?: number }
  /** Absent for a mono file. `width` is the side share of the whole file (0 is mono). */
  stereo?: { correlation?: number; width?: number; lowEndMono?: boolean } | null
  dynamics?: { crestDb?: number; peakToLoudnessDb?: number | null }
  tempo?: { bpm?: number; confidence?: number } | null
  key?: { name?: string; confidence?: number } | null
  /** Set when the audio is one sound (a sample, a one-shot): its pitch, envelope and movement. The runtime's own
   *  summary of such a sound names no key, and neither does the card. */
  sound?: object | null
}

/** `kumi_media::audio::Comparison`: current minus reference; paired values are `[current, reference]`. */
export interface Comparison {
  headlines?: string[]
  loudness?: { mineLufs?: number | null; referenceLufs?: number | null; differenceLu?: number | null; truePeak?: [number, number] }
  balance?: { band?: string; hz?: string; difference?: number }[]
  dynamics?: { crestDb?: [number, number] }
}

/** Exact decoded-frame provenance; Analysis.seconds can still describe the original whole file. */
export interface SampleWindow {
  original_total_frames: number | null
  original_seconds: number | null
  original_duration_exact: boolean
  requested: { from: number; seconds: number } | null
  start_frame: number
  frame_count: number
  sample_rate: number
  channels: number
  replay_from: number
}

export interface ReferenceProvenance {
  name?: string
  identity?: { kind?: string; file_identity?: unknown; sound_id?: string; map_revision?: string; artifact_id?: string }
  window?: SampleWindow | null
}

export interface ListenReference extends ReferenceProvenance {
  artifact_id?: string
  metrics?: Analysis
  origin?: ReferenceProvenance
  /** Real Heard/Profile member values; null includes a reason, including incomplete Nyquist coverage. */
  profile_quantities?: { quantity: string; unit: string; basis: string; value: number | null; reason: string | null; reason_code?: string | null; nyquist_hz?: number | null }[]
  availability?: {
    energy_zero: boolean
    profile_reason: string | null
    loud_frame_count: number
    fixed_region_tolerance: boolean
    unavailable: { quantity: string; reason: string }[]
  } | null
}

export interface ProfileQuantity {
  quantity: string
  unit: string
  basis: string
  source: number | null
  source_reason: string | null
  source_reason_detail?: string | null
  source_nyquist_hz?: number | null
  target: { mid: number; low: number; high: number } | null
  target_reason: string | null
  contributors: number
  references: number
  deviation: number | null
  inside_spread: boolean | null
}

export interface ListenResult {
  metrics?: Analysis
  artifact_id?: string
  verdict_text?: string
  media_tag?: string
  compare?: ListenReference & { differences?: Comparison }
  references?: ListenReference[]
  reference_profile?: {
    basis: string
    references: number
    usable_references: number
    source_window: SampleWindow | null
    quantities: ProfileQuantity[]
    range_note: string
    peak_note: string
  }
  /** A model's reading of the measurements (CC-1). Its text opens with the runtime's own line saying what it was given. */
  interpretation?: { text?: string; model?: string }
  interpretation_error?: string
  error?: string
  /** `refused`, `cancelled` or `unsure` for a track listen that didn't complete (CC-2); those carry no measurements. */
  state?: string
  /** How a track or the mix was captured (CC-2): the beat window, the measured seconds, whether all was put back, and
   *  the runtime's one-line note (a restored capture's or a staging cleanup's warning). */
  capture?: { method?: string; from_beat?: number; beats?: number; seconds?: number; cleanup?: string; note?: string }
}
