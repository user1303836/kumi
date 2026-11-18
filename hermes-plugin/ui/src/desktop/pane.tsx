// The Kumi pane beside the chat: is Kumi connected, what the producer is looking at (FOCUS), and what Kumi changed
// (HISTORY), each change with its undo. Undo, setup and connect only put a request in Kumi's chat.
import type { PluginContext } from '@hermes/plugin-sdk'
import { Button } from '@kumi/controls'
import { T } from '@kumi/theme'
import { useEffect, useRef, useState, useSyncExternalStore } from 'react'

import { beatSeconds, canUndoChange, changesOf, chipWords, clean, cleanupWords, LOOK_AGAIN_REQUEST, STARTER_REQUESTS, statusNamesProject, undoRequest } from '../shared/format.ts'
import { FocusView, HistoryView } from '../shared/HistoryView.tsx'
import { useKumi } from '../shared/hooks.ts'
import type { KumiState, KumiStore } from '../shared/store.ts'
import { nextSteps, setupWords } from '../shared/steps.ts'
import { Dot, S, Section, TYPE, useT } from '../shared/ui.tsx'
import { ask } from './actions.ts'
import { JobsView } from './jobs.tsx'
import { PendingScriptView, SavedScript } from './script.tsx'

// The starters' labels, in the producer's language (their requests stay English, as every request does).
const STARTER_LABELS = { look: 'pane.starterLook', change: 'pane.starterChange', find: 'pane.starterFind' } as const

/** The time for "4 min ago" labels. Kumi's status poll moves it on (every 30 s while the pane is in view, and as it
 *  comes back into view): the pane keeps no timer of its own. */
export class Clock {
  private now = Date.now()
  private readonly listeners = new Set<() => void>()
  readonly get = (): number => this.now
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }
  tick(): void {
    this.now = Date.now()
    for (const listener of this.listeners) listener()
  }
}

export function ownerSession(state: KumiState): null | string {
  const session = state.status?.owner?.session_id
  return typeof session === 'string' && session ? session : null
}

const TONE = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint } as const

interface PaneProps {
  ctx: PluginContext
  store: KumiStore
  clock: Clock
  onOpenMap: () => void
  onOpenMeasurements: () => void
}

/** The least of the pane worth laying its content out in when Desktop runs Kumi's column past the window's edge (145
 *  px of 240 at a 900 px window with the map open beside the chat): under 8rem on screen nothing reads, and the content
 *  keeps the pane's width. */
const LEAST_ON_SCREEN = 128
/** Every hundredth of the pane coming into or going out of view: the observer then sees the window's edge move. */
const STEPS = Array.from({ length: 101 }, (_, step) => step / 100)

/** Whether nothing hides the element: `display`, `visibility` or `opacity` on it or a parent. */
const unhidden = (element: Element) => typeof element.checkVisibility !== 'function' || element.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })

/** Kumi's pane, handing `onAsk` a function that says whether it's in view now (null once it's gone). Desktop's own
 *  `paneVisibility('kumi:pane')` reads false while the pane shows (it's stacked with Desktop's hidden Review pane, whose
 *  id stays the group's active one), so Kumi asks its own root instead: its observer knows whether its box is on
 *  screen, and `checkVisibility` whether Desktop hides it behind another tab of its column (Files in front keeps the
 *  pane's box and hides it with `visibility` and `opacity` on a parent, which no observer reports). It's asked, never
 *  watched: no timer of the pane's own. The same observer keeps the pane's content in the part of it that's on screen:
 *  in a narrow window Desktop can run Kumi's column past the window's edge (P8: 95 px at 900 px with the map open
 *  beside the chat), and the right of every row, Undo with it, would be cut off. */
export function KumiPane({ onAsk, ...props }: PaneProps & { onAsk?: (inView: (() => boolean) | null) => void }) {
  const root = useRef<HTMLDivElement | null>(null)
  const [cut, setCut] = useState({ left: 0, right: 0 })
  useEffect(() => {
    const element = root.current
    if (!element) return
    let intersecting = false
    const seen = new IntersectionObserver(
      entries => {
        const entry = entries[entries.length - 1]
        if (!entry) return
        intersecting = entry.isIntersecting
        const box = entry.boundingClientRect
        const shown = entry.intersectionRect
        const left = entry.isIntersecting ? Math.max(0, Math.round(shown.left - box.left)) : 0
        const right = entry.isIntersecting ? Math.max(0, Math.round(box.right - shown.right)) : 0
        const next = box.width - left - right < LEAST_ON_SCREEN ? { left: 0, right: 0 } : { left, right }
        setCut(current => (current.left === next.left && current.right === next.right ? current : next))
      },
      { threshold: STEPS }
    )
    seen.observe(element)
    onAsk?.(() => intersecting && unhidden(element))
    return () => {
      seen.disconnect()
      onAsk?.(null)
    }
  }, [onAsk])
  return (
    <div ref={root} style={{ height: '100%' }}>
      <div style={{ height: '100%', marginLeft: cut.left, marginRight: cut.right }}>
        <PaneBody {...props} />
      </div>
    </div>
  )
}

function PaneBody({ ctx, store, clock, onOpenMap, onOpenMeasurements }: PaneProps) {
  const t = useT()
  const state = useKumi(store)
  const now = useSyncExternalStore(clock.subscribe, clock.get, clock.get)
  if (state.available === false) {
    return (
      <div style={{ padding: 12, display: 'flex', flexDirection: 'column', gap: 8 }}>
        <span style={{ ...TYPE.body, color: T.text }}>{t('pane.agentOff')}</span>
        <span style={{ ...TYPE.small, color: T.muted }}>{t('pane.agentOffHow')}</span>
      </div>
    )
  }
  if (state.available === null) {
    return <div style={{ ...TYPE.small, color: T.muted, padding: 12 }}>{t('pane.reading')}</div>
  }
  // One home per fact (P6): the status line names the DAW and the project; FOCUS keeps the view and the transport, so
  // the tempo is FOCUS's, and the project's chip shows there only while the status line doesn't name it.
  const words = chipWords(state.status, undefined, t)
  const steps = nextSteps(state, t)
  const session = ownerSession(state)
  const connected = Boolean(state.status?.connected)
  const setupMessage = setupWords(state)
  const undoable = changesOf(state.history?.items ?? []).filter(rows => canUndoChange(state.status, rows)).length
  const note = { ...TYPE.small, color: T.muted, overflowWrap: 'anywhere' } as const
  return (
    <div style={{ height: '100%', overflowY: 'auto', overflowX: 'hidden', color: T.text }}>
      {/* A script waiting for its approval comes first, whole (CC-19): it's what the producer is deciding on now. */}
      {state.script && <PendingScriptView daw={state.status?.daw} script={state.script} />}
      <div style={{ ...S.section, gap: 8 }}>
        <div style={{ ...S.row, gap: 8 }}>
          {/* On the DAW's beat while it plays; steady when it's stopped, or not connected. */}
          <Dot beat={connected ? beatSeconds(state.focus?.transport) : null} color={TONE[words.tone]} size={8} />
          <span style={{ ...TYPE.body, ...S.ellipsis, fontWeight: 600, flex: '1 1 auto' }}>{words.text}</span>
          {/* The version gives way first when the pane is narrow: the project's name is what's read. */}
          {state.status?.runtime?.version && <span style={{ ...TYPE.caption, ...S.ellipsis, color: T.faint, flex: '0 100 auto' }}>Kumi {state.status.runtime.version}</span>}
        </div>
        {setupMessage && <span style={note}>{setupMessage}</span>}
        {state.error && <span style={{ ...note, color: T.warn }}>{t('pane.backendError', state.error)}</span>}
        {/* A find or a measurement whose tab a narrow window held back (P8): the one thing to do, at the top, until it's
            opened or another takes its place. Under twenty HISTORY rows it was lost, while the chat said the sounds were
            lit up on the map. */}
        {state.waiting && (
          <div role="status" style={{ ...S.row, gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
            <span style={{ ...TYPE.small, color: T.text, flex: '1 1 auto' }}>{state.waiting.tab === 'map' ? t('pane.waitingFound', state.waiting.count) : t('pane.waitingMeasured')}</span>
            <Button kind="primary" onClick={state.waiting.tab === 'map' ? onOpenMap : onOpenMeasurements}>
              {state.waiting.tab === 'map' ? t('pane.showOnMap') : t('pane.showMeasurement')}
            </Button>
          </div>
        )}
        {state.status?.unsure && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 6, alignItems: 'flex-start' }}>
            <span style={{ ...note, color: T.warn }}>
              {[
                t('pane.unsure', clean(state.status.unsure.op ?? t('pane.unsureOp'), 40), state.status.unsure.reason ? clean(state.status.unsure.reason, 160) : ''),
                cleanupWords(state.status.unsure.cleanup, t),
                t('pane.needsLook')
              ]
                .filter(Boolean)
                .join(' ')}
            </span>
            <Button onClick={() => ask(ctx, LOOK_AGAIN_REQUEST, { send: true, session })}>{t('pane.lookAgain')}</Button>
          </div>
        )}
        {steps.length > 0 && (
          <div style={{ ...S.row, gap: 8, flexWrap: 'wrap' }}>
            {steps.map(step => (
              <Button key={step.request} kind={steps.length === 1 ? 'primary' : 'secondary'} onClick={() => ask(ctx, step.request, { send: true, session: connected ? session : null })}>
                {step.label}
              </Button>
            ))}
          </div>
        )}
      </div>
      <Section title={t('pane.focus')}>
        <FocusView focus={state.focus} loading={state.focus === null && !state.failed.focus} now={now} project={!statusNamesProject(state.status)} />
      </Section>
      {Object.keys(state.jobs).length > 0 && (
        <Section title={t('pane.jobs')}>
          <JobsView jobs={state.jobs} onRequest={request => ask(ctx, request, { send: false, session })} />
        </Section>
      )}
      <Section
        title={
          <>
            {t('pane.history')} <span style={{ textTransform: 'none', letterSpacing: 0, fontWeight: 400 }}>{undoable ? t('pane.undoable', undoable) : ''}</span>
          </>
        }
      >
        <HistoryView
          daw={state.status?.daw}
          history={state.history}
          loading={state.history === null && !state.failed.history}
          live={Boolean(state.status?.connected) && state.status?.stale !== true}
          now={now}
          target={state.setup?.target}
          detail={item => (item.script ? <SavedScript item={item} store={store} /> : null)}
          action={(head, rows) => {
            // One Undo per change (CC-3 A2), on its head: it undoes every step left, and says so when there are several.
            if (!canUndoChange(state.status, rows)) return null
            const left = rows.filter(row => row.state !== 'undone').length
            const change = clean(head.title ?? head.label ?? head.id, 120)
            // Its accessible name starts with the words it shows, so voice control and a screen reader say the same (P7A-29).
            return (
              <Button label={left > 1 ? t('pane.undoAllLabel', left, change) : t('pane.undoLabel', change)} onClick={() => ask(ctx, undoRequest(head), { send: true, session })} tip={left > 1 ? t('pane.undoChangeTitle', left) : undefined}>
                {left > 1 ? t('pane.undoAll', left) : t('pane.undo')}
              </Button>
            )
          }}
        />
        {/* Rows are kept while Kumi is away, but undoing one needs it back: said under them, and only when there are some. */}
        {!connected && state.status?.owner && (state.history?.items?.length ?? 0) > 0 && <span style={{ ...TYPE.small, color: T.faint }}>{t('pane.undoNeedsConnection')}</span>}
        {/* Nothing in HISTORY yet, with Kumi connected: three requests to try, each only put in the chat to send. */}
        {connected && state.history !== null && (state.history.items?.length ?? 0) === 0 && (
          <div style={{ ...S.row, gap: 4, flexWrap: 'wrap', marginTop: 4 }}>
            <span style={{ ...TYPE.small, color: T.faint, marginRight: 4 }}>{t('pane.starters')}</span>
            {STARTER_REQUESTS.map(starter => (
              <Button key={starter.key} kind="quiet" onClick={() => ask(ctx, starter.request, { send: false, session })} tip={t('pane.starterTitle')}>
                {t(STARTER_LABELS[starter.key])}
              </Button>
            ))}
          </div>
        )}
      </Section>
      <div style={{ ...S.row, flexWrap: 'wrap', padding: 12, gap: 8 }}>
        <Button onClick={onOpenMap}>{t('pane.openMap')}</Button>
        {/* A measurement with no tool part to draw its card (Hermes's tool progress off): a narrow window doesn't open
            its tab by itself, so it's here for the producer to open, as the map is. */}
        {state.measured && (
          <Button kind="quiet" onClick={onOpenMeasurements}>
            {t('pane.openMeasurements')}
          </Button>
        )}
      </div>
    </div>
  )
}
