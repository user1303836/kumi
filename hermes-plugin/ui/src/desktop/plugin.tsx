/**
 * Kumi for Hermes Desktop.
 *
 * - A Kumi pane beside the chat: the connection, FOCUS (what the producer is looking at in the DAW) and HISTORY (the
 *   changes Kumi made), each with its undo.
 * - A status-bar chip: `● Live · Night Drive · 124 BPM`, with a menu.
 * - The sound map, as a workspace tab: the approved sample folders on a map; a click auditions in the window.
 * - An inline card for the audio `kumi_listen` measured, in place of the plain player.
 * - Palette commands and a settings page.
 *
 * Everything reads Kumi's backend (`dashboard/plugin_api.py`, GET only) through `ctx.rest`. Nothing here changes the
 * DAW or anything else: a button that would puts a request in Kumi's chat, which goes to the model, its tool and
 * Hermes's approval like any other request (contract C5).
 */
import type { HermesPlugin, PaletteContribution, PluginContribution, ToolMediaRendererProps } from '@hermes/plugin-sdk'
// A namespace import too: on a Desktop without an export (the tool-media seam), a named import would stop the module.
import * as sdk from '@hermes/plugin-sdk'
import { host, PALETTE_AREA, PANES_AREA, STATUSBAR_AREAS } from '@hermes/plugin-sdk'
import type { ReactNode } from 'react'

import { canUndoChange, changesOf, undoRequest, undoRowOf } from '../shared/format.ts'
import { Audition } from '../shared/hooks.ts'
import { english, MESSAGES, type Translate } from '../shared/i18n.ts'
import { parseListen } from '../shared/listen.ts'
import { foundSounds, jobOf, KumiStore } from '../shared/store.ts'
import { FocusStyle, I18n } from '../shared/ui.tsx'
import { ask, wordsOf } from './actions.ts'
import { ListenCard } from './card.tsx'
import { GenerateCard } from './generate.tsx'
import { FocusHint } from './composer.tsx'
import { StatusChip } from './chip.tsx'
import { callId, Calls, forAnotherProfile, forChatInFront, forThisProfile, payloadOf } from './events.ts'
import { MapWorkspace } from './map.tsx'
import { MeasurementsWorkspace } from './measurements.tsx'
import { KumiPane, ownerSession } from './pane.tsx'
import { PET_LINES, petLine } from './pet.ts'
import { OPEN_MAP_ON_FIND, SettingsPage } from './settings.tsx'
export const PLUGIN_ID = 'kumi'
export const MAP_WORKSPACE = 'kumi-sound-map'
export const MEASUREMENTS_WORKSPACE = 'kumi-measurements'
/** How long a `listen.done` event waits for its call's `tool.complete` (whose tool part draws the card) before the
 *  Measurements tab opens instead (CC-9). */
export const LISTEN_FALLBACK_MS = 2000
/** How often /status is read while the window is in view: events are faster, but only reach the process that emits. */
export const POLL_MS = 5000
/** How often a running job is read while the window is in view (none is read when nothing runs). */
export const JOB_POLL_MS = 2000

// Desktop's translator for this plugin's catalogs, reactive to a language switch; English on a Desktop without it.
const usePluginWords: ((pluginId: string) => Translate) | null = typeof sdk.usePluginI18n === 'function' ? sdk.usePluginI18n : null

/** Every Desktop root draws inside this: its components' `useT()` gets Desktop's words for the app's language. */
function WithT({ children }: { children: ReactNode }) {
  const t = usePluginWords ? usePluginWords(PLUGIN_ID) : english
  return (
    <I18n.Provider value={t}>
      <FocusStyle />
      {children}
    </I18n.Provider>
  )
}

const plugin: HermesPlugin = {
  id: PLUGIN_ID,
  name: 'Kumi',
  description: "Your DAW beside the chat: FOCUS, HISTORY with undo, a sound map of your sample folders, and Kumi's audio measurements.",
  register(ctx) {
    // English, Japanese and Simplified Chinese; Desktop drops them when the plugin unloads.
    ctx.i18n?.register(MESSAGES)
    const store = new KumiStore(path => ctx.rest(path, { timeoutMs: 15_000 }))
    const audition = new Audition(id => ctx.rest(`/audio?id=${encodeURIComponent(id)}`, { timeoutMs: 60_000 }))
    let closeMap: (() => void) | null = null
    let mapPane: (() => void) | null = null
    const mapView = () => (
      <WithT>
        <MapWorkspace audition={audition} ctx={ctx} store={store} />
      </WithT>
    )

    const openMap = () => {
      if (typeof host.openWorkspace === 'function') {
        closeMap = host.openWorkspace(MAP_WORKSPACE, {
          title: wordsOf(ctx)('map.tab'),
          minWidth: '26rem',
          render: mapView,
          onClose: () => {
            closeMap = null
          }
        })
        return
      }
      // Desktops without workspace tabs: the map as a pane of its own.
      mapPane ??= ctx.register({
        id: 'map-pane',
        area: PANES_AREA,
        title: wordsOf(ctx)('map.tab'),
        data: { placement: 'main' },
        render: mapView
      })
    }
    let closeMeasurements: (() => void) | null = null
    let measurementsPane: (() => void) | null = null
    const openMeasurements = () => {
      const render = () => (
        <WithT>
          <MeasurementsWorkspace audition={audition} store={store} />
        </WithT>
      )
      const title = wordsOf(ctx)('measurements.tab')
      if (typeof host.openWorkspace === 'function') {
        closeMeasurements = host.openWorkspace(MEASUREMENTS_WORKSPACE, {
          title,
          minWidth: '24rem',
          render,
          onClose: () => {
            closeMeasurements = null
          }
        })
        return
      }
      measurementsPane ??= ctx.register({ id: 'measurements-pane', area: PANES_AREA, title, data: { placement: 'main' }, render })
    }
    const openSettings = () => host.navigate(typeof sdk.pluginSettingsHref === 'function' ? sdk.pluginSettingsHref(PLUGIN_ID) : '/settings?tab=plugins')

    // Read /status now, every few seconds while the window is in view, and on every event that says something moved.
    // An event about another profile's Kumi (#966's `profile`) moves nothing this window shows.
    const refresh = () => void store.refreshStatus()
    refresh()
    ctx.setInterval(() => {
      if (document.visibilityState !== 'hidden') refresh()
    }, POLL_MS)
    const ours = (act: (payload: Record<string, unknown>) => void) => (event: { payload?: unknown }) => {
      const payload = payloadOf(event)
      if (!forAnotherProfile(payload)) act(payload)
    }
    ctx.onEvent(`plugin.${PLUGIN_ID}.status.changed`, ours(refresh))
    ctx.onEvent(`plugin.${PLUGIN_ID}.focus.changed`, ours(() => void store.refresh('focus')))
    ctx.onEvent(`plugin.${PLUGIN_ID}.history.changed`, ours(() => void store.refresh('history')))
    ctx.onEvent(`plugin.${PLUGIN_ID}.map.changed`, ours(() => void store.refresh('map')))
    ctx.setInterval(() => {
      if (document.visibilityState !== 'hidden') void store.refreshJobs()
    }, JOB_POLL_MS)
    ctx.onEvent(`plugin.${PLUGIN_ID}.job.progress`, ours(payload => store.trackJob(payload.job_id)))
    // CC-13: a generation's card, sent whatever Hermes's tool progress setting is. The Jobs row follows its job and
    // keeps the card for its reconciled ledger; it creates, places or inserts nothing. The event always names its
    // profile, so one naming none is for no one here (fails closed, like CC-9's). One over 256 KiB comes without a
    // card, and carries nothing to follow.
    ctx.onEvent(`plugin.${PLUGIN_ID}.generate.updated`, event => {
      const payload = payloadOf(event)
      if (forThisProfile(payload)) store.noteGeneration(payload.card)
    })
    ctx.onEvent(
      `plugin.${PLUGIN_ID}.settings.changed`,
      ours(() => {
        refresh()
        void store.refresh('setup')
      })
    )

    // A search's finds and a measurement reach this window twice: as the call's `tool.complete` (not sent while
    // Hermes's tool progress is off, or in focus view) and as a plugin event (CC-9). Each call is acted on once.
    const calls = new Calls()
    const waiting = new Map<string, () => void>()
    const showFound = (id: string | null, revision: string | null, ids: string[]) => {
      if (id) {
        if (calls.has(id)) return
        calls.add(id)
      }
      if (!ids.length) return
      store.found(revision, ids)
      if (ctx.storage.get(OPEN_MAP_ON_FIND, true)) openMap()
    }
    ctx.onEvent(`plugin.${PLUGIN_ID}.sounds.found`, event => {
      const payload = payloadOf(event)
      const id = callId(payload.tool_call_id)
      if (!id || !forChatInFront(payload)) return
      const ids = Array.isArray(payload.sound_ids) ? payload.sound_ids.filter((value): value is string => typeof value === 'string') : []
      showFound(id, typeof payload.map_revision === 'string' ? payload.map_revision : null, ids.slice(0, 200))
    })
    ctx.onEvent(`plugin.${PLUGIN_ID}.listen.done`, event => {
      const payload = payloadOf(event)
      const id = callId(payload.tool_call_id)
      if (!id || !forChatInFront(payload) || calls.has(id) || waiting.has(id)) return
      // A result too large to carry comes as none (the tab then points to Kumi's reply); one that isn't a measurement
      // opens nothing.
      const result = payload.result ?? null
      if (result !== null && !parseListen(result)) return
      // The transcript draws the card in the call's tool part: the tab opens only when no tool.complete comes for it.
      waiting.set(
        id,
        ctx.setTimeout(() => {
          waiting.delete(id)
          if (calls.has(id)) return
          calls.add(id)
          store.measure(id, result)
          openMeasurements()
        }, LISTEN_FALLBACK_MS)
      )
    })
    ctx.onEvent('tool.complete', event => {
      const payload = payloadOf(event)
      const name = typeof payload.name === 'string' ? payload.name : ''
      if (!name.startsWith('kumi_')) return
      refresh()
      // A scan (or any call that starts a job) answers with its job's id: follow it.
      store.trackJob(jobOf(payload.result))
      const id = callId(payload.tool_id)
      if (name === 'kumi_listen' && id) {
        // Its tool part shows, card and all: no tab.
        calls.add(id)
        waiting.get(id)?.()
        waiting.delete(id)
        return
      }
      if (name !== 'kumi_sounds') return
      const found = foundSounds(payload.result)
      if (found) showFound(id, found.revision, found.ids)
    })
    // Another profile or connection: another backend, another Kumi. What waited on the old one is dropped with it.
    for (const atom of [host.state.profile, host.state.connectionId]) {
      const stop = atom?.listen?.(() => {
        for (const cancel of waiting.values()) cancel()
        waiting.clear()
        calls.clear()
        store.reset()
        audition.reset()
        refresh()
      })
      if (stop) ctx.onDispose(stop)
    }
    ctx.onDispose(() => {
      for (const cancel of waiting.values()) cancel()
      closeMap?.()
      closeMeasurements?.()
      audition.dispose()
    })

    // The pet's bubble (newer Desktops): a finished change or job, once the producer turned it on in Kumi's settings.
    // A store reset (another profile or connection) reads HISTORY afresh, which holds nothing new to say.
    const pet = ctx.pet
    if (pet && typeof pet.say === 'function') {
      let last = store.get()
      ctx.onDispose(
        store.subscribe(() => {
          const now = store.get()
          const line = ctx.storage.get(PET_LINES, false) && pet.visible.get() ? petLine(last, now, wordsOf(ctx)) : null
          last = now
          if (line) pet.say(line.text, { id: line.id, tone: line.tone, ttlMs: 8000 })
        })
      )
    }

    const contributions: PluginContribution[] = [
      {
        id: 'pane',
        area: PANES_AREA,
        title: 'Kumi',
        data: { placement: 'right', width: '320px' },
        render: () => (
          <WithT>
            <KumiPane ctx={ctx} onOpenMap={openMap} store={store} />
          </WithT>
        )
      },
      {
        id: 'status',
        area: STATUSBAR_AREAS.right,
        order: 60,
        render: () => (
          <WithT>
            <StatusChip ctx={ctx} onOpenMap={openMap} onOpenSettings={openSettings} store={store} />
          </WithT>
        )
      }
    ]
    if (typeof sdk.TOOL_MEDIA_RENDERER_AREA === 'string') {
      contributions.push({
        id: 'listen-card',
        area: sdk.TOOL_MEDIA_RENDERER_AREA,
        data: {
          tools: ['kumi_listen'],
          render: (props: ToolMediaRendererProps) => (
            <WithT>
              <ListenCard audition={audition} media={props} />
            </WithT>
          )
        }
      })
      // What Kumi had made through Mirelo: `kumi_job`'s answer for a generation, once Desktop ties its `MEDIA:` line to
      // the call (CC-13). `kumi_generate` never answers with a file. Any other result keeps Desktop's player or chip; a
      // scan's answer has no `MEDIA:` line at all.
      contributions.push({
        id: 'generate-card',
        area: sdk.TOOL_MEDIA_RENDERER_AREA,
        data: {
          tools: ['kumi_job'],
          render: (props: ToolMediaRendererProps) => (
            <WithT>
              <GenerateCard audition={audition} ctx={ctx} media={props} store={store} />
            </WithT>
          )
        }
      })
    }
    if (typeof sdk.COMPOSER_AREAS?.underside === 'string') {
      contributions.push({
        id: 'focus-hint',
        area: sdk.COMPOSER_AREAS.underside,
        render: () => (
          <WithT>
            <FocusHint store={store} />
          </WithT>
        )
      })
    }
    ctx.registerMany(contributions)

    // The palette's labels are plain strings: registered in the app's language, and again when it changes.
    const palette = (t: Translate): PluginContribution[] => [
      {
        id: 'open-map',
        area: PALETTE_AREA,
        data: { id: 'kumi.map', label: t('palette.map'), keywords: ['kumi', 'sound', 'samples', 'map', 'library'], run: openMap } satisfies PaletteContribution
      },
      {
        id: 'undo-last',
        area: PALETTE_AREA,
        data: {
          id: 'kumi.undo-last',
          label: t('palette.undo'),
          keywords: ['kumi', 'undo', 'history', 'revert'],
          run: () => {
            const state = store.get()
            // The last change is the newest one with a row not already undone: never an older one past a change Kumi
            // isn't sure of. Its request names the row its undo starts from, for the whole change (CC-3 A2).
            const last = changesOf(state.history?.items ?? []).find(rows => undoRowOf(rows) !== undefined)
            const next = last && undoRowOf(last)
            if (!last || !next || !canUndoChange(state.status, last)) {
              const unsure = Boolean(state.status?.unsure) || next?.state === 'unsure' || next?.state === 'partial'
              host.notify({ kind: 'info', message: wordsOf(ctx)(unsure ? 'notify.undoUnsure' : 'notify.nothingToUndo') })
              return
            }
            void ask(ctx, undoRequest(next), { send: true, session: ownerSession(state) })
          }
        } satisfies PaletteContribution
      },
      {
        id: 'settings',
        area: PALETTE_AREA,
        data: { id: 'kumi.settings', label: t('palette.settings'), keywords: ['kumi', 'setup', 'settings', 'daw'], run: openSettings } satisfies PaletteContribution
      }
    ]
    let unregisterPalette = ctx.registerMany(palette(wordsOf(ctx)))
    ctx.i18n?.onLocaleChange(() => {
      unregisterPalette()
      unregisterPalette = ctx.registerMany(palette(wordsOf(ctx)))
    })
    ctx.registerSettingsPage?.({
      id: 'kumi',
      title: 'Kumi',
      icon: 'pulse',
      render: () => (
        <WithT>
          <SettingsPage ctx={ctx} onOpenMap={openMap} store={store} />
        </WithT>
      )
    })
  }
}

export default plugin
