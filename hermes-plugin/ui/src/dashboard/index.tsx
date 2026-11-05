// Kumi's tab in the Hermes web dashboard: the connection, FOCUS, HISTORY and the sound map, read from the same
// backend routes as the Desktop half. The dashboard has no chat input to put a request in, so it changes nothing:
// a row's request can be copied, to paste into a chat.
import { T } from '@kumi/theme'
import { useEffect, useMemo, useState } from 'react'

import { canUndoChange, changesOf, chipWords, clean, cleanupWords, LOOK_AGAIN_REQUEST, soundRequest, undoRequest, undoRowOf } from '../shared/format.ts'
import { Audition, useKumi, usePlaying, useWant } from '../shared/hooks.ts'
import { FocusView, HistoryView } from '../shared/HistoryView.tsx'
import { translator } from '../shared/i18n.ts'
import { MapCanvas } from '../shared/MapCanvas.tsx'
import { dawPartWorks, KumiStore } from '../shared/store.ts'
import type { AudioReply } from '../shared/types.ts'
import { Dot, FocusStyle, I18n, S, Section, TextButton, useT } from '../shared/ui.tsx'
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
  const t = useT()
  const [done, setDone] = useState(false)
  return (
    <TextButton
      onClick={() => {
        void navigator.clipboard?.writeText(text).then(() => {
          setDone(true)
          setTimeout(() => setDone(false), 1500)
        })
      }}
      title={t('dashboard.copyTitle', text)}
    >
      {done ? t('dashboard.copied') : label}
    </TextButton>
  )
}

/** The dashboard's language (its own i18n context), for Kumi's words. */
function KumiDashboard() {
  const locale = (SDK.useI18n?.() as { locale?: unknown } | undefined)?.locale
  const t = useMemo(() => translator(locale), [locale])
  return (
    <I18n.Provider value={t}>
      <FocusStyle />
      <Dashboard />
    </I18n.Provider>
  )
}

function Dashboard() {
  const t = useT()
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

  const words = chipWords(state.status, state.focus?.transport?.tempo, t)
  const tone = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint }[words.tone]
  const connected = Boolean(state.status?.connected)

  if (state.available === false) {
    return (
      <div style={{ padding: 24, color: T.text }}>
        <p style={S.text}>{t('dashboard.agentOff')}</p>
        <p style={S.muted}>{t('dashboard.agentOffHow')}</p>
      </div>
    )
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12, color: T.text, minHeight: 'calc(100vh - 160px)' }}>
      <div style={{ ...S.row, gap: 10, padding: '4px 2px' }}>
        <Dot color={tone} size={9} />
        <span style={{ fontSize: 15, fontWeight: 600 }}>{words.text}</span>
        {state.status?.runtime?.version && <span style={S.faint}>Kumi {state.status.runtime.version}</span>}
        {state.setup?.message && state.setup.state && state.setup.state !== 'ready' && state.setup.state !== 'unknown' && !dawPartWorks(state) && (
          <span style={S.muted}>{state.setup.message}</span>
        )}
      </div>
      {state.status?.unsure && (
        <div style={{ ...S.row, gap: 10, padding: '0 2px', alignItems: 'flex-start' }}>
          <span style={{ ...S.muted, color: T.warn, flex: 1 }}>
            {[
              t('pane.unsure', clean(state.status.unsure.op ?? t('pane.unsureOp'), 40), state.status.unsure.reason ? clean(state.status.unsure.reason, 160) : ''),
              cleanupWords(state.status.unsure.cleanup, t),
              t('dashboard.askLook')
            ]
              .filter(Boolean)
              .join(' ')}
          </span>
          <CopyButton label={t('dashboard.copyRequest')} text={LOOK_AGAIN_REQUEST} />
        </div>
      )}
      <div style={{ display: 'grid', gridTemplateColumns: 'minmax(260px, 1fr) minmax(320px, 2fr)', gap: 12, alignItems: 'stretch' }}>
        <div style={{ border: `1px solid ${T.border}`, borderRadius: 6, background: T.card, overflow: 'hidden', alignSelf: 'start' }}>
          <Section title={t('pane.focus')}>
            <FocusView focus={state.focus} now={now} />
          </Section>
          <Section title={t('pane.history')}>
            <HistoryView
              action={item => {
                // One Copy undo per change, on the row its undo starts from, as in Desktop's pane (CC-3 A2).
                const rows = changesOf(state.history?.items ?? []).find(change => change.includes(item))
                return rows && undoRowOf(rows) === item && canUndoChange(state.status, rows) ? <CopyButton label={t('dashboard.copyUndo')} text={undoRequest(item)} /> : null
              }}
              daw={state.status?.daw}
              history={state.history}
              live={Boolean(state.status?.connected) && state.status?.stale !== true}
              now={now}
              target={state.setup?.target}
            />
          </Section>
        </div>
        {/* Tall enough that a search's list (six rows) and a picked sound's row still leave the map most of the card:
            at 420 the canvas shrank to its 160 px floor. The page scrolls, so the height costs nothing. */}
        <div style={{ border: `1px solid ${T.border}`, borderRadius: 6, background: T.card, overflow: 'hidden', minHeight: 640, display: 'flex', flexDirection: 'column' }}>
          <MapCanvas
            actions={(id, sound) => {
              const request = soundRequest(id, sound.name, state.map?.revision)
              return (
                <>
                  <TextButton onClick={() => (playing === id ? audition.stop() : void audition.play(id).catch(() => undefined))}>{playing === id ? t('map.stop') : t('map.play')}</TextButton>
                  {request && <CopyButton label={t('dashboard.copyRequest')} text={request} />}
                </>
              )
            }}
            empty={state.map === null ? t('map.reading') : t('dashboard.mapEmpty')}
            found={state.found}
            map={state.map}
            onPick={id => {
              setSelected(id)
              void audition.play(id).catch(() => undefined)
            }}
            onSelect={setSelected}
            playing={playing}
            selected={selected}
          />
        </div>
      </div>
    </div>
  )
}

window.__HERMES_PLUGINS__?.register('kumi', KumiDashboard)
