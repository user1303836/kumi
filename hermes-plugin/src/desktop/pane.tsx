// The Kumi pane beside the chat: is Kumi connected, what the producer is looking at (FOCUS), and what Kumi changed
// (HISTORY), each change with its undo. Undo, setup and connect only put a request in Kumi's chat.
import type { PluginContext } from '@hermes/plugin-sdk'
import { useEffect, useState } from 'react'

import { chipWords, clean, cleanupWords, connectRequest, dawName, LOOK_AGAIN_REQUEST, setupRequest, undoRequest } from '../shared/format.ts'
import { FocusView, HistoryView } from '../shared/HistoryView.tsx'
import { useKumi } from '../shared/hooks.ts'
import type { KumiState, KumiStore } from '../shared/store.ts'
import { Dot, S, Section, TextButton } from '../shared/ui.tsx'
import { T } from '@kumi/theme'
import { ask } from './actions.ts'

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

/** Setup's next step, when there is one, as the button that asks for it. */
export function nextStep(state: KumiState): { label: string; request: string } | null {
  const setup = state.setup
  const daw = state.status?.daw ?? setup?.daw
  if (setup && (NEEDS_SETUP[setup.state ?? ''] || setup.next_step === 'install_runtime' || setup.next_step === 'install_daw_component')) {
    return { label: `Set up Kumi for ${dawName(daw)}`, request: setupRequest(daw) }
  }
  const ready = setup?.state === 'ready' || setup?.installed === true
  if (ready && !state.status?.connected) return { label: `Connect to ${dawName(daw)}`, request: connectRequest(daw) }
  return null
}

const TONE = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint } as const

export function KumiPane({ ctx, store, onOpenMap }: { ctx: PluginContext; store: KumiStore; onOpenMap: () => void }) {
  const state = useKumi(store)
  const now = useNow()
  if (state.available === false) {
    return (
      <div style={{ padding: 14, display: 'flex', flexDirection: 'column', gap: 8 }}>
        <span style={S.text}>Kumi's agent half is off in this profile.</span>
        <span style={S.muted}>
          Turn on the kumi plugin's Agent switch in Capabilities → Plugins, then restart the gateway. This pane shows your DAW's FOCUS and Kumi's HISTORY once
          it's on.
        </span>
      </div>
    )
  }
  if (state.available === null) {
    return <div style={{ ...S.muted, padding: 14 }}>Reading Kumi's state…</div>
  }
  const words = chipWords(state.status, state.focus?.transport?.tempo)
  const step = nextStep(state)
  const session = ownerSession(state)
  const connected = Boolean(state.status?.connected)
  const setupMessage = typeof state.setup?.message === 'string' && state.setup.state !== 'ready' ? state.setup.message : null
  const undoable = (state.history?.items ?? []).filter(item => item.undo_available).length
  return (
    <div style={{ height: '100%', overflowY: 'auto', color: T.text }}>
      <div style={{ ...S.section, gap: 8 }}>
        <div style={{ ...S.row, gap: 8 }}>
          <Dot color={TONE[words.tone]} size={8} />
          <span style={{ ...S.text, ...S.ellipsis, fontWeight: 600, flex: 1 }}>{words.text}</span>
          {state.status?.runtime?.version && <span style={S.faint}>Kumi {state.status.runtime.version}</span>}
        </div>
        {setupMessage && <span style={S.muted}>{setupMessage}</span>}
        {state.error && <span style={{ ...S.muted, color: T.warn }}>Kumi's backend didn't answer: {state.error}</span>}
        {state.status?.unsure && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            <span style={{ ...S.muted, color: T.warn }}>
              Kumi isn't sure your project is as it was after {clean(state.status.unsure.op ?? 'its last action', 40)}
              {state.status.unsure.reason ? `: ${clean(state.status.unsure.reason, 160)}` : ''}. {cleanupWords(state.status.unsure.cleanup)} It needs a
              fresh look before another change.
            </span>
            <div style={S.row}>
              <TextButton onClick={() => void ask(ctx, LOOK_AGAIN_REQUEST, { send: true, session })}>Ask Kumi to look again</TextButton>
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
      <Section title="Focus">
        <FocusView focus={state.focus} now={now} />
      </Section>
      <Section
        title={
          <>
            History <span style={{ textTransform: 'none', letterSpacing: 0 }}>{undoable ? `· ${undoable} can be undone` : ''}</span>
          </>
        }
      >
        <HistoryView
          history={state.history}
          now={now}
          action={item =>
            item.undo_available && connected ? (
              <TextButton onClick={() => void ask(ctx, undoRequest(item), { send: true, session })} title={`Ask Kumi to undo ${item.label ?? 'this change'}`}>
                Undo
              </TextButton>
            ) : null
          }
        />
      </Section>
      <div style={{ ...S.row, padding: '10px 12px', gap: 8 }}>
        <TextButton onClick={onOpenMap}>Open the sound map</TextButton>
        {!connected && state.status?.owner && <span style={S.faint}>Undo needs a connected Kumi.</span>}
      </div>
    </div>
  )
}
