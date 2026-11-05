// FOCUS (what the producer is looking at, and the transport) and HISTORY (Kumi's changes, newest first), as both UI
// halves show them. Nothing here acts: what a row can do comes from the half that draws it.
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'

import { ago, bpm, catchUpLines, clean, cleanupWords, colorOf, focusParameter, focusPath, focusPlace, nameOf } from './format.ts'
import { english, type Translate } from './i18n.ts'
import type { Focus, History, HistoryItem } from './types.ts'
import { Chip, Dot, S, useT } from './ui.tsx'

// Each state's mark, tone, and word (a catalog key).
const STATE: Record<string, { mark: string; tone: 'bad' | 'good' | 'muted' | 'warn'; word: string }> = {
  applied: { mark: '✓', tone: 'good', word: 'history.applied' },
  undone: { mark: '↺', tone: 'muted', word: 'history.undone' },
  kept: { mark: '✓', tone: 'good', word: 'history.kept' },
  partial: { mark: '◐', tone: 'warn', word: 'history.partial' },
  unsure: { mark: '?', tone: 'warn', word: 'history.unsure' },
  refused: { mark: '✕', tone: 'muted', word: 'history.refused' },
  failed: { mark: '✕', tone: 'bad', word: 'history.failed' }
}

export function stateOf(item: HistoryItem, t: Translate = english): { mark: string; tone: 'bad' | 'good' | 'muted' | 'warn'; word: string } {
  const known = STATE[item.state ?? '']
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

export function HistoryView({ history, now, action }: { history: History | null; now?: number; action?: (item: HistoryItem) => ReactNode }) {
  const t = useT()
  const items = history?.items ?? []
  if (!items.length) {
    return <span style={S.muted}>{t('history.empty')}</span>
  }
  return (
    <ol style={{ listStyle: 'none', margin: 0, padding: 0, display: 'flex', flexDirection: 'column', gap: 2 }}>
      {items.map(item => {
        const state = stateOf(item, t)
        const track = nameOf(item.track)
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
                // The runtime's own words for any other row: why an undo didn't happen, say.
                item.note && <span style={{ ...S.muted, whiteSpace: 'normal' }}>{clean(item.note, 200)}</span>
              )}
            </span>
            {action?.(item)}
          </li>
        )
      })}
    </ol>
  )
}
