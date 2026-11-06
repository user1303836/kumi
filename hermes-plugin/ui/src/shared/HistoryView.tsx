// FOCUS (what the producer is looking at, and the transport) and HISTORY (Kumi's changes, newest first), as both UI
// halves show them. Nothing here acts: what a row can do comes from the half that draws it.
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'

import { ago, bpm, catchUpLines, clean, cleanupWords, colorOf, dawName, focusParameter, focusPath, focusPlace, nameOf } from './format.ts'
import { english, type Translate } from './i18n.ts'
import type { Focus, History, HistoryItem } from './types.ts'
import { Chip, Dot, S, useT } from './ui.tsx'

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

export function FocusView({ focus, now }: { focus: Focus | null; now?: number }) {
  const t = useT()
  const catchUp = catchUpLines(focus)
  // With the DAW closed, focus.json keeps the project and what changed: show those, not "nothing yet".
  if (!focus || (!focus.focus && !focus.project && !catchUp.lines.length)) {
    return <span style={S.muted}>{t('focus.nothing')}</span>
  }
  const transport = focus.transport
  const path = focusPath(focus)
  const parameter = focusParameter(focus)
  const busy = nameOf(focus.focus?.busy ?? undefined)
  const meter = nameOf(transport?.meter ?? undefined)
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6, opacity: focus.stale ? 0.6 : 1 }}>
      <div style={{ ...S.row, gap: 7 }}>
        <Dot color={colorOf(focus.focus?.track) ?? T.faint} size={9} />
        <span style={{ ...S.text, ...S.ellipsis, fontWeight: 600 }}>{path.length ? path.join(' › ') : t('focus.nothingSelected')}</span>
      </div>
      {parameter && <span style={{ ...S.muted, ...S.ellipsis }}>{parameter}</span>}
      <div style={{ ...S.row, gap: 4, flexWrap: 'wrap' }}>
        {focus.project?.name && <Chip title={t('focus.project')}>{clean(focus.project.name, 40)}</Chip>}
        {focusPlace(focus).map((place, index) => (
          <Chip key={index}>{place}</Chip>
        ))}
        {typeof transport?.playing === 'boolean' && <Chip tone={transport.playing ? 'good' : undefined}>{transport.playing ? t('focus.playing') : t('focus.stopped')}</Chip>}
        {bpm(transport?.tempo) && <Chip>{bpm(transport?.tempo)}</Chip>}
        {meter && <Chip>{clean(meter, 12)}</Chip>}
        {busy && (
          <Chip tone="warn" title={t('focus.busy')}>
            {clean(busy, 40)}
          </Chip>
        )}
        {focus.stale && (focus.running === false ? <Chip>{t('focus.kumiStopped')}</Chip> : <Chip tone="warn">{t('focus.notAnswering')}</Chip>)}
      </div>
      {catchUp.lines.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
          <span style={S.faint}>{t('focus.since')}</span>
          {catchUp.lines.map((line, index) => (
            <span key={index} style={{ ...S.muted, ...S.ellipsis }}>
              {line}
            </span>
          ))}
          {catchUp.more > 0 && <span style={S.faint}>{t('focus.more', catchUp.more)}</span>}
        </div>
      )}
      {focus.stale && typeof focus.heard_at === 'number' && <span style={S.faint}>{t('focus.lastHeard', ago(focus.heard_at, now, t))}</span>}
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

/** HISTORY's rows. `live`: the runtime answers now (connected, not stale), so a row without undo was a restart's;
 *  `daw` names where to undo it then; `target` is Kumi's build target (`/setup`), for the DAW's undo key. `detail` adds
 *  a surface's own part under a row's words (a script's code). */
export function HistoryView({
  history,
  now,
  action,
  detail,
  live = false,
  daw,
  target
}: {
  history: History | null
  now?: number
  action?: (item: HistoryItem) => ReactNode
  detail?: (item: HistoryItem) => ReactNode
  live?: boolean
  daw?: unknown
  target?: string
}) {
  const t = useT()
  const items = history?.items ?? []
  if (!items.length) {
    return <span style={S.muted}>{t('history.empty')}</span>
  }
  return (
    <ol style={{ listStyle: 'none', margin: 0, padding: 0, display: 'flex', flexDirection: 'column', gap: 2 }}>
      {items.map(item => {
        const state = stateOf(item, t, live)
        const track = nameOf(item.track)
        const dawUndo = item.state === 'applied' && (item.daw_undo === 'next' || item.daw_undo === 'buried') ? item.daw_undo : null
        return (
          <li key={item.id} style={{ ...S.row, gap: 7, padding: '4px 0', borderTop: `1px solid ${T.border}`, opacity: item.state === 'undone' ? 0.7 : 1 }}>
            <span aria-label={state.word} title={state.word} style={{ width: 14, textAlign: 'center', color: TONE[state.tone], flex: 'none' }}>
              {state.mark}
            </span>
            <span style={{ display: 'flex', flexDirection: 'column', minWidth: 0, flex: 1 }}>
              <span style={{ ...S.text, ...S.ellipsis, textDecoration: item.state === 'undone' ? 'line-through' : undefined }} title={clean(item.title ?? '', 300)}>
                {clean(item.title ?? item.label ?? item.id, 120)}
              </span>
              <span style={{ ...S.row, gap: 5 }}>
                {track && <Dot color={colorOf(item.track) ?? T.faint} size={6} />}
                <span style={{ ...S.faint, ...S.ellipsis }}>
                  {[track && clean(track, 24), item.family && clean(item.family, 16), item.label && clean(item.label, 12), ago(item.at, now, t)].filter(Boolean).join(' · ')}
                </span>
              </span>
              {item.state === 'unsure' || item.state === 'partial' ? (
                <span style={{ ...S.muted, color: T.warn, whiteSpace: 'normal' }}>
                  {[clean(item.reason ?? item.note ?? t('history.unconfirmed'), 200), cleanupWords(item.cleanup, t), t('history.lookFirst')]
                    .filter(Boolean)
                    .join(' ')}
                </span>
              ) : (
                // The runtime's own words for any other row (why an undo didn't happen, say), why a script's can't (Kumi
                // never undoes one, CC-19), or why a restart's can't.
                item.note ? (
                  <span style={{ ...S.muted, whiteSpace: 'normal' }}>{clean(item.note, 400)}</span>
                ) : item.family === 'script' ? (
                  <span style={{ ...S.muted, whiteSpace: 'normal' }}>{t('history.scriptUndo', dawName(daw, t))}</span>
                ) : (
                  restartedSince(item, live) && <span style={{ ...S.muted, whiteSpace: 'normal' }}>{t('history.restartedSince', dawName(daw, t))}</span>
                )
              )}
              {item.copy?.file ? (
                <span style={{ ...S.muted, whiteSpace: 'normal' }}>{t('history.copyKept', clean(item.copy.file, 120))}</span>
              ) : item.copy?.failed ? (
                <span style={{ ...S.muted, color: T.warn, whiteSpace: 'normal' }}>{t('history.copyFailed', clean(item.copy.failed, 200))}</span>
              ) : null}
              {dawUndo && <DawUndo daw={daw} how={dawUndo} keys={undoKeys(target)} />}
              {detail?.(item)}
            </span>
            {dawUndo ? null : action?.(item)}
          </li>
        )
      })}
    </ol>
  )
}
