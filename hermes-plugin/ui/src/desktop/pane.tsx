// The Kumi pane beside the chat: is Kumi connected, what the producer is looking at (FOCUS), and what Kumi changed
// (HISTORY), each change with its undo. Undo, setup and connect only put a request in Kumi's chat.
import type { PluginContext } from '@hermes/plugin-sdk'
import { useEffect, useState } from 'react'

import { canUndo, chipWords, clean, cleanupWords, connectRequest, dawName, LOOK_AGAIN_REQUEST, setupRequest, undoRequest } from '../shared/format.ts'
import { FocusView, HistoryView } from '../shared/HistoryView.tsx'
import { useKumi } from '../shared/hooks.ts'
import { english, type Translate } from '../shared/i18n.ts'
import { dawPartWorks, type KumiState, type KumiStore } from '../shared/store.ts'
import { Dot, S, Section, TextButton, useT } from '../shared/ui.tsx'
import { T } from '@kumi/theme'
import { ask } from './actions.ts'
import { JobsView } from './jobs.tsx'

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

// Setup states (W5's `setup.status()`) that a setup request fixes. `no_pin` and `unsupported_platform` it can't, and
// `choose_control_surface` is the producer's step in the DAW, which setup's message explains.
const NEEDS_SETUP: Record<string, true> = { needs_runtime: true, outdated_runtime: true, damaged_runtime: true, needs_daw_component: true }

/** Setup's next step, when there is one, as the button that asks for it (its label in `t`'s words; the request stays
 *  English, C5). */
export function nextStep(state: KumiState, t: Translate = english): { label: string; request: string } | null {
  const setup = state.setup
  const daw = state.status?.daw ?? setup?.daw
  if (setup && !dawPartWorks(state) && (NEEDS_SETUP[setup.state ?? ''] || setup.next_step === 'install_runtime' || setup.next_step === 'install_daw_component')) {
    return { label: t('step.setup', dawName(daw, t)), request: setupRequest(daw) }
  }
  if (setup?.state === 'ready' && !state.status?.connected) return { label: t('step.connect', dawName(daw, t)), request: connectRequest(daw) }
  return null
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
  const step = nextStep(state, t)
  const session = ownerSession(state)
  const connected = Boolean(state.status?.connected)
  // Setup's word only when there's something to do; an unknown state is the settings page's to explain.
  const setupState = state.setup?.state
  const setupMessage = typeof state.setup?.message === 'string' && setupState && setupState !== 'ready' && setupState !== 'unknown' && !dawPartWorks(state) ? state.setup.message : null
  const undoable = (state.history?.items ?? []).filter(item => canUndo(state.status, item)).length
  return (
    <div style={{ height: '100%', overflowY: 'auto', overflowX: 'hidden', color: T.text }}>
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
        {step && (
          <div style={S.row}>
            <TextButton onClick={() => void ask(ctx, step.request, { send: true, session: connected ? session : null })} strong>
              {step.label}
            </TextButton>
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
          action={item =>
            canUndo(state.status, item) ? (
              <TextButton
                label={t('pane.undoLabel', clean(item.title ?? item.label ?? item.id, 120))}
                onClick={() => void ask(ctx, undoRequest(item), { send: true, session })}
                title={t('pane.undoTitle')}
              >
                {t('pane.undo')}
              </TextButton>
            ) : null
          }
        />
      </Section>
      <div style={{ ...S.row, padding: '10px 12px', gap: 8 }}>
        <TextButton onClick={onOpenMap}>{t('pane.openMap')}</TextButton>
        {!connected && state.status?.owner && <span style={S.faint}>{t('pane.undoNeedsConnection')}</span>}
      </div>
    </div>
  )
}
