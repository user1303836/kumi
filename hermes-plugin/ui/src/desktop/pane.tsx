// The Kumi pane beside the chat: is Kumi connected, what the producer is looking at (FOCUS), and what Kumi changed
// (HISTORY), each change with its undo. Undo, setup and connect only put a request in Kumi's chat.
import type { PluginContext } from '@hermes/plugin-sdk'
import { useEffect, useState } from 'react'

import { canUndoChange, changesOf, chipWords, clean, cleanupWords, LOOK_AGAIN_REQUEST, undoRequest, undoRowOf } from '../shared/format.ts'
import { FocusView, HistoryView } from '../shared/HistoryView.tsx'
import { useKumi } from '../shared/hooks.ts'
import type { KumiState, KumiStore } from '../shared/store.ts'
import { nextSteps, setupWords } from '../shared/steps.ts'
import { Dot, S, Section, TextButton, useT } from '../shared/ui.tsx'
import { T } from '@kumi/theme'
import { ask } from './actions.ts'
import { JobsView } from './jobs.tsx'
import { PendingScriptView, SavedScript } from './script.tsx'

/** The time, re-read every `every` ms while mounted, for "4 min ago" labels. */
export function useNow(every = 30_000): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), every)
    return () => clearInterval(timer)
  }, [every])
  return now
}

export function ownerSession(state: KumiState): null | string {
  const session = state.status?.owner?.session_id
  return typeof session === 'string' && session ? session : null
}

const TONE = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint } as const

export function KumiPane({ ctx, store, onOpenMap }: { ctx: PluginContext; store: KumiStore; onOpenMap: () => void }) {
  const t = useT()
  const state = useKumi(store)
  const now = useNow()
  if (state.available === false) {
    return (
      <div style={{ padding: 14, display: 'flex', flexDirection: 'column', gap: 8 }}>
        <span style={S.text}>{t('pane.agentOff')}</span>
        <span style={S.muted}>{t('pane.agentOffHow')}</span>
      </div>
    )
  }
  if (state.available === null) {
    return <div style={{ ...S.muted, padding: 14 }}>{t('pane.reading')}</div>
  }
  const words = chipWords(state.status, state.focus?.transport?.tempo, t)
  const steps = nextSteps(state, t)
  const session = ownerSession(state)
  const connected = Boolean(state.status?.connected)
  const setupMessage = setupWords(state)
  // One Undo per change (CC-3 A2): on the row it starts from (its newest not yet undone), for all its rows left,
  // counted as one.
  const changes = changesOf(state.history?.items ?? [])
  const changeOf = new Map(changes.flatMap(rows => rows.map(row => [row, rows] as const)))
  const undoable = changes.filter(rows => canUndoChange(state.status, rows)).length
  return (
    <div style={{ height: '100%', overflowY: 'auto', overflowX: 'hidden', color: T.text }}>
      {/* A script waiting for its approval comes first, whole (CC-19): it's what the producer is deciding on now. */}
      {state.script && <PendingScriptView daw={state.status?.daw} script={state.script} />}
      <div style={{ ...S.section, gap: 8 }}>
        <div style={{ ...S.row, gap: 8 }}>
          <Dot color={TONE[words.tone]} size={8} />
          <span style={{ ...S.text, ...S.ellipsis, fontWeight: 600, flex: 1 }}>{words.text}</span>
          {state.status?.runtime?.version && <span style={S.faint}>Kumi {state.status.runtime.version}</span>}
        </div>
        {setupMessage && <span style={S.muted}>{setupMessage}</span>}
        {state.error && <span style={{ ...S.muted, color: T.warn }}>{t('pane.backendError', state.error)}</span>}
        {state.status?.unsure && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            <span style={{ ...S.muted, color: T.warn }}>
              {[
                t('pane.unsure', clean(state.status.unsure.op ?? t('pane.unsureOp'), 40), state.status.unsure.reason ? clean(state.status.unsure.reason, 160) : ''),
                cleanupWords(state.status.unsure.cleanup, t),
                t('pane.needsLook')
              ]
                .filter(Boolean)
                .join(' ')}
            </span>
            <div style={S.row}>
              <TextButton onClick={() => void ask(ctx, LOOK_AGAIN_REQUEST, { send: true, session })}>{t('pane.lookAgain')}</TextButton>
            </div>
          </div>
        )}
        {steps.length > 0 && (
          <div style={{ ...S.row, gap: 6, flexWrap: 'wrap' }}>
            {steps.map(step => (
              <TextButton key={step.request} onClick={() => void ask(ctx, step.request, { send: true, session: connected ? session : null })} strong={steps.length === 1}>
                {step.label}
              </TextButton>
            ))}
          </div>
        )}
      </div>
      <Section title={t('pane.focus')}>
        <FocusView focus={state.focus} now={now} />
      </Section>
      {Object.keys(state.jobs).length > 0 && (
        <Section title={t('pane.jobs')}>
          <JobsView jobs={state.jobs} onRequest={request => void ask(ctx, request, { send: false, session })} />
        </Section>
      )}
      <Section
        title={
          <>
            {t('pane.history')} <span style={{ textTransform: 'none', letterSpacing: 0 }}>{undoable ? t('pane.undoable', undoable) : ''}</span>
          </>
        }
      >
        <HistoryView
          daw={state.status?.daw}
          history={state.history}
          live={Boolean(state.status?.connected) && state.status?.stale !== true}
          now={now}
          target={state.setup?.target}
          detail={item => (item.script ? <SavedScript item={item} store={store} /> : null)}
          action={item => {
            const rows = changeOf.get(item)
            if (!rows || undoRowOf(rows) !== item || !canUndoChange(state.status, rows)) return null
            const left = rows.filter(row => row.state !== 'undone').length
            return (
              <TextButton
                label={t('pane.undoLabel', clean(item.title ?? item.label ?? item.id, 120))}
                onClick={() => void ask(ctx, undoRequest(item), { send: true, session })}
                title={left > 1 ? t('pane.undoChangeTitle', left) : t('pane.undoTitle')}
              >
                {t('pane.undo')}
              </TextButton>
            )
          }}
        />
        {/* Rows are kept while Kumi is away, but undoing one needs it back: said under them, and only when there are some. */}
        {!connected && state.status?.owner && (state.history?.items?.length ?? 0) > 0 && <span style={S.faint}>{t('pane.undoNeedsConnection')}</span>}
      </Section>
      <div style={{ ...S.row, padding: '10px 12px', gap: 8 }}>
        <TextButton onClick={onOpenMap}>{t('pane.openMap')}</TextButton>
      </div>
    </div>
  )
}
