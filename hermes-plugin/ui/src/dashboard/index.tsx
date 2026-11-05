// Kumi's tab in the Hermes web dashboard: the connection, FOCUS, HISTORY and the sound map, read from the same
// backend routes as the Desktop half. The dashboard has no chat input to put a request in, so it changes nothing:
// a row's request can be copied, to paste into a chat.
import { T } from '@kumi/theme'
import { useEffect, useMemo, useState } from 'react'

import { canUndo, chipWords, clean, cleanupWords, LOOK_AGAIN_REQUEST, soundRequest, undoRequest } from '../shared/format.ts'
import { Audition, useKumi, usePlaying, useWant } from '../shared/hooks.ts'
import { FocusView, HistoryView } from '../shared/HistoryView.tsx'
import { MapCanvas } from '../shared/MapCanvas.tsx'
import { KumiStore } from '../shared/store.ts'
import type { AudioReply } from '../shared/types.ts'
import { Dot, S, Section, TextButton } from '../shared/ui.tsx'
import { SDK } from './react.ts'

const BASE = '/api/plugins/kumi'
const POLL_MS = 5000

function useNow(every = 30_000): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), every)
    return () => clearInterval(timer)
  }, [every])
  return now
}

function CopyButton({ text, label }: { text: string; label: string }) {
  const [done, setDone] = useState(false)
  return (
    <TextButton
      onClick={() => {
        void navigator.clipboard?.writeText(text).then(() => {
          setDone(true)
          setTimeout(() => setDone(false), 1500)
        })
      }}
      title={`Copy “${text}” to paste into a chat`}
    >
      {done ? 'Copied' : label}
    </TextButton>
  )
}

function KumiDashboard() {
  const store = useMemo(() => new KumiStore(path => SDK.fetchJSON(`${BASE}${path}`)), [])
  const audition = useMemo(() => new Audition(id => SDK.fetchJSON<AudioReply>(`${BASE}/audio?id=${encodeURIComponent(id)}`)), [])
  useWant(store, 'map')
  const state = useKumi(store)
  const playing = usePlaying(audition)
  const now = useNow()
  const [selected, setSelected] = useState<null | string>(null)

  useEffect(() => {
    void store.refreshStatus()
    const timer = setInterval(() => {
      if (document.visibilityState !== 'hidden') void store.refreshStatus()
    }, POLL_MS)
    return () => {
      clearInterval(timer)
      audition.dispose()
    }
  }, [store, audition])

  const words = chipWords(state.status, state.focus?.transport?.tempo)
  const tone = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint }[words.tone]
  const connected = Boolean(state.status?.connected)

  if (state.available === false) {
    return (
      <div style={{ padding: 24, color: T.text }}>
        <p style={S.text}>Kumi's agent plugin is off in this profile.</p>
        <p style={S.muted}>Turn it on (Hermes's plugins settings, or `hermes plugins enable kumi`) and restart the dashboard to see Kumi here.</p>
      </div>
    )
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12, color: T.text, minHeight: 'calc(100vh - 160px)' }}>
      <div style={{ ...S.row, gap: 10, padding: '4px 2px' }}>
        <Dot color={tone} size={9} />
        <span style={{ fontSize: 15, fontWeight: 600 }}>{words.text}</span>
        {state.status?.runtime?.version && <span style={S.faint}>Kumi {state.status.runtime.version}</span>}
        {state.setup?.message && state.setup.state && state.setup.state !== 'ready' && state.setup.state !== 'unknown' && (
          <span style={S.muted}>{state.setup.message}</span>
        )}
      </div>
      {state.status?.unsure && (
        <div style={{ ...S.row, gap: 10, padding: '0 2px', alignItems: 'flex-start' }}>
          <span style={{ ...S.muted, color: T.warn, flex: 1 }}>
            Kumi isn't sure your project is as it was after {clean(state.status.unsure.op ?? 'its last action', 40)}
            {state.status.unsure.reason ? `: ${clean(state.status.unsure.reason, 160)}` : ''}. {cleanupWords(state.status.unsure.cleanup)} Ask Kumi in
            your chat to look again before another change.
          </span>
          <CopyButton label="Copy request" text={LOOK_AGAIN_REQUEST} />
        </div>
      )}
      <div style={{ display: 'grid', gridTemplateColumns: 'minmax(260px, 1fr) minmax(320px, 2fr)', gap: 12, alignItems: 'stretch' }}>
        <div style={{ border: `1px solid ${T.border}`, borderRadius: 6, background: T.card, overflow: 'hidden', alignSelf: 'start' }}>
          <Section title="Focus">
            <FocusView focus={state.focus} now={now} />
          </Section>
          <Section title="History">
            <HistoryView
              action={item => (canUndo(state.status, item) ? <CopyButton label="Copy undo" text={undoRequest(item)} /> : null)}
              history={state.history}
              now={now}
            />
          </Section>
        </div>
        <div style={{ border: `1px solid ${T.border}`, borderRadius: 6, background: T.card, overflow: 'hidden', minHeight: 420, display: 'flex', flexDirection: 'column' }}>
          <MapCanvas
            actions={(id, sound) => {
              const request = soundRequest(id, sound.name, state.map?.revision)
              return (
                <>
                  <TextButton onClick={() => (playing === id ? audition.stop() : void audition.play(id).catch(() => undefined))}>{playing === id ? '■ Stop' : '▶ Play'}</TextButton>
                  {request && <CopyButton label="Copy request" text={request} />}
                </>
              )
            }}
            empty={state.map === null ? 'Reading the sound map…' : "No sound map yet: add a sample folder in Kumi's settings and ask Kumi to scan it."}
            found={state.found}
            map={state.map}
            onPick={id => {
              setSelected(id)
              void audition.play(id).catch(() => undefined)
            }}
            playing={playing}
            selected={selected}
          />
        </div>
      </div>
    </div>
  )
}

window.__HERMES_PLUGINS__?.register('kumi', KumiDashboard)
