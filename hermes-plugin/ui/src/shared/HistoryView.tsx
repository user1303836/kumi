// FOCUS (what the producer is looking at, and the transport) and HISTORY (Kumi's changes, newest first), as both UI
// halves show them. Nothing here acts: what a row can do comes from the half that draws it.
import { Badge } from '@kumi/controls'
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'
import { useState } from 'react'

import { ago, bpm, catchUpLines, changesOf, clean, cleanupWords, colorOf, dawName, focusParameter, focusPath, focusPlace, nameOf, undoRowOf } from './format.ts'
import { english, type Translate } from './i18n.ts'
import type { Focus, History, HistoryItem } from './types.ts'
import { Clamp, Dot, FOCUSABLE, Pending, S, TYPE, useT } from './ui.tsx'

// Each state's mark, tone, and word (a catalog key). `kept` is muted, not a success: the producer changed it since,
// so Kumi left it as it is.
const STATE: Record<string, { mark: string; tone: 'bad' | 'good' | 'muted' | 'warn'; word: string }> = {
  applied: { mark: '✓', tone: 'good', word: 'history.applied' },
  undone: { mark: '↺', tone: 'muted', word: 'history.undone' },
  kept: { mark: '✓', tone: 'muted', word: 'history.kept' },
  expired: { mark: '·', tone: 'muted', word: 'history.expired' },
  partial: { mark: '◐', tone: 'warn', word: 'history.partial' },
  unsure: { mark: '?', tone: 'warn', word: 'history.unsure' },
  refused: { mark: '✕', tone: 'muted', word: 'history.refused' },
  failed: { mark: '✕', tone: 'bad', word: 'history.failed' }
}

/** A row Kumi restarted since, so it can't undo it safely: `expired`, or an earlier runtime's `applied` row (no undo
 *  while a live runtime answers). A stale snapshot sets every row's undo false, so only a live one says so. A row
 *  with the runtime's own note keeps its note, and a script's has no undo from the start (Kumi never undoes one). */
export function restartedSince(item: HistoryItem, live: boolean): boolean {
  if (item.note || item.family === 'script') return false
  return item.state === 'expired' || (live && item.state === 'applied' && item.undo_available === false)
}

export function stateOf(item: HistoryItem, t: Translate = english, live = false): { mark: string; tone: 'bad' | 'good' | 'muted' | 'warn'; word: string } {
  const known = STATE[restartedSince(item, live) ? 'expired' : (item.state ?? '')]
  if (known) return { ...known, word: t(known.word) }
  return { mark: '·', tone: 'muted', word: item.state ? clean(item.state, 20) : t('history.recorded') }
}

const TONE: Record<'bad' | 'good' | 'muted' | 'warn', string> = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint }

/** FOCUS. `loading`: its first read is still on its way (null focus then says nothing yet, not "nothing in focus").
 *  `project`: whether to show the project's chip, which only says something when the status line above doesn't name
 *  it (the DAW closed, say). Whether Kumi stopped or isn't answering is the status line's to say: FOCUS only dims. */
export function FocusView({ focus, now, loading = false, project = true }: { focus: Focus | null; now?: number; loading?: boolean; project?: boolean }) {
  const t = useT()
  const catchUp = catchUpLines(focus)
  if (loading && !focus) return <Pending label={t('pane.reading')} />
  // With the DAW closed, focus.json keeps the project and what changed: show those, not "nothing yet".
  if (!focus || (!focus.focus && !focus.project && !catchUp.lines.length)) {
    return <span style={{ ...TYPE.small, color: T.muted }}>{t('focus.nothing')}</span>
  }
  const transport = focus.transport
  const path = focusPath(focus)
  const parameter = focusParameter(focus)
  const busy = nameOf(focus.focus?.busy ?? undefined)
  const meter = nameOf(transport?.meter ?? undefined)
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8, opacity: focus.stale ? 0.6 : 1 }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
        <div style={{ ...S.row, gap: 8 }}>
          <Dot color={colorOf(focus.focus?.track) ?? T.faint} size={8} />
          <span style={{ ...TYPE.body, ...S.ellipsis, color: T.text, fontWeight: 600 }}>{path.length ? path.join(' › ') : t('focus.nothingSelected')}</span>
        </div>
        {parameter && <span style={{ ...TYPE.small, ...S.ellipsis, color: T.muted, paddingLeft: 16 }}>{parameter}</span>}
      </div>
      <div style={{ ...S.row, gap: 4, flexWrap: 'wrap' }}>
        {project && focus.project?.name && <Badge tip={t('focus.project')}>{clean(focus.project.name, 40)}</Badge>}
        {focusPlace(focus).map((place, index) => (
          <Badge key={index}>{place}</Badge>
        ))}
        {typeof transport?.playing === 'boolean' && <Badge tone={transport.playing ? 'good' : 'fact'}>{transport.playing ? t('focus.playing') : t('focus.stopped')}</Badge>}
        {bpm(transport?.tempo) && <Badge>{bpm(transport?.tempo)}</Badge>}
        {meter && <Badge>{clean(meter, 12)}</Badge>}
        {busy && (
          <Badge tip={t('focus.busy')} tone="warn">
            {clean(busy, 40)}
          </Badge>
        )}
      </div>
      {catchUp.lines.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
          <span style={{ ...TYPE.caption, color: T.faint }}>{t('focus.since')}</span>
          {catchUp.lines.map((line, index) => (
            <span key={index} style={{ ...TYPE.small, ...S.ellipsis, color: T.muted }}>
              {line}
            </span>
          ))}
          {catchUp.more > 0 && <span style={{ ...TYPE.caption, color: T.faint }}>{t('focus.more', catchUp.more)}</span>}
        </div>
      )}
      {focus.stale && typeof focus.heard_at === 'number' && <span style={{ ...TYPE.caption, color: T.faint }}>{t('focus.lastHeard', ago(focus.heard_at, now, t))}</span>}
    </div>
  )
}

/** The DAW's undo key on the machine Kumi runs on: `/setup`'s target (an `apple-darwin` build is a Mac), since a
 *  Webapp can be open on another machine than the DAW's (CC-20f; serve picks its words by its own OS). The browser's
 *  own OS only when setup hasn't said. */
export function undoKeys(target: unknown): string {
  const on = typeof target === 'string' && target ? target : globalThis.navigator?.userAgent ?? ''
  return /apple-darwin|Mac/.test(on) ? 'Cmd+Z' : 'Ctrl+Z'
}

/** Where a row only the DAW's own undo can reverse (CC-20) stands, as a line under its note: next in the DAW's undo
 *  while serve knows of nothing above it, or by hand once a later Kumi change sits above it there (the DAW's undo would
 *  take that back first). How many presses take it back is the note's to say (a pad load is two Live steps, a plan of
 *  add-chains one), so these words never count them. Words, never a request: Kumi would refuse it. */
function DawUndo({ daw, how, keys }: { daw: unknown; how: 'next' | 'buried'; keys: string }) {
  const t = useT()
  const app = dawName(daw, t)
  if (how === 'buried') {
    return (
      <span title={t('history.byHandTitle', app)} style={{ ...S.muted, color: T.warn, whiteSpace: 'normal' }}>
        {t('history.byHand', app)}
      </span>
    )
  }
  return (
    <span title={t('history.dawUndoTitle', app, keys)} style={{ ...S.muted, whiteSpace: 'normal' }}>
      {t('history.dawUndo', app)}
    </span>
  )
}

/** A row's words: its title in two lines (the rest on a click), what the meta line adds to it, and any note the row
 *  carries. The meta line drops what the title already says: the track when the title names it, the kind of change
 *  when the title is that kind ("Tempo 120 → 124 BPM" over "tempo"), and never the change's machine label. */
function RowWords({ item, live, daw, target, now, detail, restartSaid }: { item: HistoryItem; live: boolean; daw: unknown; target?: string; now?: number; detail?: (item: HistoryItem) => ReactNode; restartSaid: boolean }) {
  const t = useT()
  const title = clean(item.title ?? item.label ?? item.id, 300)
  const said = title.toLowerCase()
  const track = nameOf(item.track)
  const showTrack = track && !said.includes(track.toLowerCase())
  const family = item.family && !said.includes(item.family.toLowerCase()) ? clean(item.family, 16) : ''
  const dawUndo = item.state === 'applied' && (item.daw_undo === 'next' || item.daw_undo === 'buried') ? item.daw_undo : null
  const note = { ...TYPE.small, color: T.muted, whiteSpace: 'normal', overflowWrap: 'anywhere' } as const
  return (
    <>
      {/* Two lines, then the rest on a click or Enter: the part of a title that differs is often its end ("…→ Dusty
          Snare 04.wav"), which a one-line cut hid. */}
      <Clamp style={{ ...TYPE.body, color: item.state === 'undone' ? T.muted : T.text, textDecoration: item.state === 'undone' ? 'line-through' : undefined }} text={title} />
      <span style={{ ...S.row, gap: 6 }}>
        {showTrack && <Dot color={colorOf(item.track) ?? T.faint} size={6} />}
        <span style={{ ...TYPE.small, ...S.ellipsis, color: T.faint }}>{[showTrack && clean(track, 24), family, ago(item.at, now, t)].filter(Boolean).join(' · ')}</span>
      </span>
      {item.state === 'unsure' || item.state === 'partial' ? (
        <span style={{ ...note, color: T.warn }}>
          {[clean(item.reason ?? item.note ?? t('history.unconfirmed'), 200), cleanupWords(item.cleanup, t), t('history.lookFirst')].filter(Boolean).join(' ')}
        </span>
      ) : item.note ? (
        // The runtime's own words for any other row (why an undo didn't happen, say), why a script's can't (Kumi never
        // undoes one, CC-19); a restart's is said once, above the rows it covers.
        <span style={note}>{clean(item.note, 400)}</span>
      ) : item.family === 'script' ? (
        <span style={note}>{t('history.scriptUndo', dawName(daw, t))}</span>
      ) : (
        !restartSaid && restartedSince(item, live) && <span style={note}>{t('history.restartedSince', dawName(daw, t))}</span>
      )}
      {item.copy?.file ? <span style={note}>{t('history.copyKept', clean(item.copy.file.split(/[/\\]/).pop() ?? '', 120))}</span> : null}
      {dawUndo && <DawUndo daw={daw} how={dawUndo} keys={undoKeys(target)} />}
      {detail?.(item)}
    </>
  )
}

function Mark({ item, live }: { item: HistoryItem; live: boolean }) {
  const t = useT()
  const state = stateOf(item, t, live)
  return (
    <span aria-label={state.word} title={state.word} style={{ ...TYPE.body, width: 14, textAlign: 'center', color: TONE[state.tone], flex: 'none' }}>
      {state.mark}
    </span>
  )
}

/** One change: its newest step as the head (the one its undo starts from), with the change's action beside it, and
 *  its other steps folded under "N more steps". */
function Change({ rows, live, daw, target, now, action, detail, restartSaid }: { rows: HistoryItem[]; live: boolean; daw: unknown; target?: string; now?: number; action?: (head: HistoryItem, rows: HistoryItem[]) => ReactNode; detail?: (item: HistoryItem) => ReactNode; restartSaid: boolean }) {
  const t = useT()
  const [open, setOpen] = useState(false)
  const head = undoRowOf(rows) ?? rows[0]
  const others = rows.filter(row => row !== head)
  const dawUndo = head.state === 'applied' && (head.daw_undo === 'next' || head.daw_undo === 'buried')
  const words = { live, daw, target, now, detail, restartSaid }
  return (
    <li style={{ display: 'flex', alignItems: 'flex-start', gap: 8, padding: '8px 0', borderTop: `1px solid ${T.hairline}` }}>
      <Mark item={head} live={live} />
      <div style={{ display: 'flex', flexDirection: 'column', gap: 2, minWidth: 0, flex: 1 }}>
        <RowWords item={head} {...words} />
        {others.length > 0 && (
          <>
            <button
              aria-expanded={open}
              className={FOCUSABLE}
              onClick={() => setOpen(!open)}
              type="button"
              style={{ ...TYPE.small, alignSelf: 'flex-start', display: 'inline-flex', alignItems: 'center', gap: 4, minHeight: 24, padding: 0, border: 'none', background: 'transparent', color: T.muted, cursor: 'pointer', fontFamily: 'inherit' }}
            >
              <span aria-hidden style={{ display: 'inline-block', width: 10, transform: open ? 'rotate(90deg)' : 'none', transition: 'transform 120ms' }}>
                ▸
              </span>
              {t('history.moreSteps', others.length)}
            </button>
            {open && (
              <ol style={{ listStyle: 'none', margin: 0, padding: 0, display: 'flex', flexDirection: 'column' }}>
                {others.map(row => (
                  <li key={row.id} style={{ display: 'flex', alignItems: 'flex-start', gap: 8, padding: '6px 0', borderTop: `1px solid ${T.hairline}` }}>
                    <Mark item={row} live={live} />
                    <div style={{ display: 'flex', flexDirection: 'column', gap: 2, minWidth: 0, flex: 1 }}>
                      <RowWords item={row} {...words} />
                    </div>
                  </li>
                ))}
              </ol>
            )}
          </>
        )}
      </div>
      {dawUndo ? null : action?.(head, rows)}
    </li>
  )
}

/** HISTORY: Kumi's changes, newest first, each under one head with its action (CC-3 A2: one undo per change). Changes
 *  from before Kumi last restarted come after a divider that says so once, instead of a paragraph under each of them.
 *  `live`: the runtime answers now (connected, not stale), so a row without undo was a restart's; `daw` names where to
 *  undo it then; `target` is Kumi's build target (`/setup`), for the DAW's undo key. `detail` adds a surface's own part
 *  under a row's words (a script's code). `loading`: the first read is still on its way. */
export function HistoryView({
  history,
  now,
  action,
  detail,
  live = false,
  daw,
  target,
  loading = false
}: {
  history: History | null
  now?: number
  action?: (head: HistoryItem, rows: HistoryItem[]) => ReactNode
  detail?: (item: HistoryItem) => ReactNode
  live?: boolean
  daw?: unknown
  target?: string
  loading?: boolean
}) {
  const t = useT()
  const items = history?.items ?? []
  if (loading && !history) return <Pending label={t('pane.reading')} />
  if (!items.length) {
    return <span style={{ ...TYPE.small, color: T.muted }}>{t('history.empty')}</span>
  }
  const changes = changesOf(items)
  const restarted = (rows: HistoryItem[]) => restartedSince(undoRowOf(rows) ?? rows[0], live)
  const recent = changes.filter(rows => !restarted(rows))
  const before = changes.filter(restarted)
  const list = (group: HistoryItem[][], restartSaid: boolean) => (
    <ol style={{ listStyle: 'none', margin: 0, padding: 0, display: 'flex', flexDirection: 'column' }}>
      {group.map(rows => (
        <Change action={action} daw={daw} detail={detail} key={rows[rows.length - 1].id} live={live} now={now} restartSaid={restartSaid} rows={rows} target={target} />
      ))}
    </ol>
  )
  return (
    <div style={{ display: 'flex', flexDirection: 'column' }}>
      {recent.length > 0 && list(recent, false)}
      {before.length > 0 && (
        <div aria-label={t('history.beforeRestart')} role="group" style={{ display: 'flex', flexDirection: 'column', gap: 4, marginTop: recent.length ? 8 : 0 }}>
          <span style={{ ...S.row, gap: 8, ...TYPE.caption, color: T.faint, textTransform: 'uppercase', letterSpacing: '0.06em' }}>
            {t('history.beforeRestart')}
            <span aria-hidden style={{ flex: 1, height: 1, background: T.hairline }} />
          </span>
          <span style={{ ...TYPE.small, color: T.muted, whiteSpace: 'normal', overflowWrap: 'anywhere' }}>{t('history.restartedSinceAll', dawName(daw, t))}</span>
          {list(before, true)}
        </div>
      )}
    </div>
  )
}
