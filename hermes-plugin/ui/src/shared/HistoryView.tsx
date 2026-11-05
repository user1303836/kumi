// FOCUS (what the producer is looking at, and the transport) and HISTORY (Kumi's changes, newest first), as both UI
// halves show them. Nothing here acts: what a row can do comes from the half that draws it.
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'

import { ago, bpm, catchUpLines, clean, cleanupWords, colorOf, focusParameter, focusPath, focusPlace, nameOf } from './format.ts'
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
  const transport = focus.transport
  const path = focusPath(focus)
  const parameter = focusParameter(focus)
  const busy = nameOf(focus.focus?.busy ?? undefined)
  const meter = nameOf(transport?.meter ?? undefined)
  const catchUp = catchUpLines(focus)
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6, opacity: focus.stale ? 0.6 : 1 }}>
      <div style={{ ...S.row, gap: 7 }}>
        <Dot color={colorOf(focus.focus?.track) ?? T.faint} size={9} />
        <span style={{ ...S.text, ...S.ellipsis, fontWeight: 600 }}>{path.length ? path.join(' › ') : 'Nothing selected'}</span>
      </div>
      {parameter && <span style={{ ...S.muted, ...S.ellipsis }}>{parameter}</span>}
      <div style={{ ...S.row, gap: 4, flexWrap: 'wrap' }}>
        {focus.project?.name && <Chip title="The open project">{clean(focus.project.name, 40)}</Chip>}
        {focusPlace(focus).map(place => (
          <Chip key={place}>{place}</Chip>
        ))}
        {typeof transport?.playing === 'boolean' && <Chip tone={transport.playing ? 'good' : undefined}>{transport.playing ? '▶ playing' : '■ stopped'}</Chip>}
        {bpm(transport?.tempo) && <Chip>{bpm(transport?.tempo)}</Chip>}
        {meter && <Chip>{clean(meter, 12)}</Chip>}
        {busy && (
          <Chip tone="warn" title="The DAW answers late while this lasts, so what's shown may be behind">
            {clean(busy, 40)}
          </Chip>
        )}
        {focus.stale && (focus.running === false ? <Chip>Kumi stopped</Chip> : <Chip tone="warn">not answering</Chip>)}
      </div>
      {catchUp.lines.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
          <span style={S.faint}>Since Kumi last looked</span>
          {catchUp.lines.map((line, index) => (
            <span key={index} style={{ ...S.muted, ...S.ellipsis }}>
              {line}
            </span>
          ))}
          {catchUp.more > 0 && <span style={S.faint}>{`and ${catchUp.more} more`}</span>}
        </div>
      )}
      {focus.stale && typeof focus.heard_at === 'number' && <span style={S.faint}>{`Last heard ${ago(focus.heard_at, now)}.`}</span>}
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
