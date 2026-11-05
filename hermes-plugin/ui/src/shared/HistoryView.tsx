// FOCUS (what the producer is looking at, and the transport) and HISTORY (Kumi's changes, newest first), as both UI
// halves show them. Nothing here acts: what a row can do comes from the half that draws it.
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'

import { ago, bpm, clean, cleanupWords, colorOf, nameOf } from './format.ts'
import type { Focus, History, HistoryItem } from './types.ts'
import { Chip, Dot, S } from './ui.tsx'

const STATE: Record<string, { mark: string; tone: 'bad' | 'good' | 'muted' | 'warn'; word: string }> = {
  applied: { mark: '✓', tone: 'good', word: 'applied' },
  undone: { mark: '↺', tone: 'muted', word: 'undone' },
  kept: { mark: '✓', tone: 'good', word: 'kept' },
  partial: { mark: '◐', tone: 'warn', word: 'partly applied' },
  unsure: { mark: '?', tone: 'warn', word: 'unsure' },
  refused: { mark: '✕', tone: 'muted', word: 'refused' },
  failed: { mark: '✕', tone: 'bad', word: 'failed' }
}

export function stateOf(item: HistoryItem): { mark: string; tone: 'bad' | 'good' | 'muted' | 'warn'; word: string } {
  return STATE[item.state ?? ''] ?? { mark: '·', tone: 'muted', word: clean(item.state ?? 'recorded', 20) }
}

const TONE: Record<'bad' | 'good' | 'muted' | 'warn', string> = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint }

export function FocusView({ focus, now }: { focus: Focus | null; now?: number }) {
  if (!focus || (!focus.focus && !focus.project)) {
    return <span style={S.muted}>Nothing in focus yet. Kumi shows what you select in your DAW once it's connected.</span>
  }
  const track = nameOf(focus.focus?.track)
  const device = nameOf(focus.focus?.device)
  const clip = nameOf(focus.focus?.clip)
  const view = typeof focus.focus?.view === 'string' ? focus.focus.view : undefined
  const transport = focus.transport
  const path = [track, device ?? clip].filter(Boolean).map(part => clean(part, 40))
  const catchUp = Array.isArray(focus.catch_up) ? focus.catch_up.filter((line): line is string => typeof line === 'string') : []
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6, opacity: focus.stale ? 0.6 : 1 }}>
      <div style={{ ...S.row, gap: 7 }}>
        <Dot color={colorOf(focus.focus?.track) ?? T.faint} size={9} />
        <span style={{ ...S.text, ...S.ellipsis, fontWeight: 600 }}>{path.length ? path.join(' › ') : 'Nothing selected'}</span>
      </div>
      <div style={{ ...S.row, gap: 4, flexWrap: 'wrap' }}>
        {focus.project?.name && <Chip title="The open project">{clean(focus.project.name, 40)}</Chip>}
        {view && <Chip>{clean(view, 20)}</Chip>}
        {transport?.playing !== undefined && <Chip tone={transport.playing ? 'good' : undefined}>{transport.playing ? '▶ playing' : '■ stopped'}</Chip>}
        {bpm(transport?.tempo) && <Chip>{bpm(transport?.tempo)}</Chip>}
        {typeof transport?.position === 'string' && <Chip>{clean(transport.position, 16)}</Chip>}
        {focus.stale && <Chip tone="warn">{typeof focus.at === 'number' ? `not answering · seen ${ago(focus.at, now)}` : 'not answering'}</Chip>}
      </div>
      {catchUp.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
          <span style={S.faint}>Since Kumi last looked</span>
          {catchUp.slice(0, 4).map((line, index) => (
            <span key={index} style={{ ...S.muted, ...S.ellipsis }}>
              {clean(line, 120)}
            </span>
          ))}
        </div>
      )}
      {now !== undefined && typeof focus.age_ms === 'number' && focus.stale && <span style={S.faint}>Last heard {ago(now - focus.age_ms, now)}.</span>}
    </div>
  )
}

export function HistoryView({ history, now, action }: { history: History | null; now?: number; action?: (item: HistoryItem) => ReactNode }) {
  const items = history?.items ?? []
  if (!items.length) {
    return <span style={S.muted}>No changes yet. Each change Kumi makes in your DAW shows here, with its undo.</span>
  }
  return (
    <ol style={{ listStyle: 'none', margin: 0, padding: 0, display: 'flex', flexDirection: 'column', gap: 2 }}>
      {items.map(item => {
        const state = stateOf(item)
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
                  {[track && clean(track, 24), item.family && clean(item.family, 16), item.label && clean(item.label, 12), ago(item.at, now)].filter(Boolean).join(' · ')}
                </span>
              </span>
              {(item.state === 'unsure' || item.state === 'partial') && (
                <span style={{ ...S.muted, color: T.warn, whiteSpace: 'normal' }}>
                  {[clean(item.reason ?? 'Kumi couldn’t confirm what happened.', 200), cleanupWords(item.cleanup), 'Ask Kumi to look again before another change.']
                    .filter(Boolean)
                    .join(' ')}
                </span>
              )}
            </span>
            {action?.(item)}
          </li>
        )
      })}
    </ol>
  )
}
