/**
 * Kumi for Hermes Desktop.
 *
 * - A Kumi pane beside the chat: the connection, FOCUS (what the producer is looking at in the DAW) and HISTORY (the
 *   changes Kumi made), each with its undo.
 * - A status-bar chip: `● Live · Night Drive · 124 BPM`, with a menu.
 * - The sound map, as a workspace tab: the approved sample folders on a map, where a sound plays in the window as the
 *   pointer rests on it or on a click, and the palette under it, which the agent's `kumi_palette` shares.
 * - An inline card for the audio `kumi_listen` measured, in place of the plain player.
 * - Palette commands and a settings page.
 *
 * Everything reads Kumi's backend (`dashboard/plugin_api.py`) through `ctx.rest`. Nothing here changes the DAW or
 * anything else: a button that would puts a request in Kumi's chat, which goes to the model, its tool and Hermes's
 * approval like any other request (contract C5). The backend's only writes are edits that change no project: the
 * sounds that can't play here, hidden from the map, and the palette.
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
import { Clock, KumiPane, ownerSession } from './pane.tsx'
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
/** While only the chip shows Kumi, every this many polls (30 s): the chip's dot is kept by events, the poll is a net. */
export const IDLE_POLLS = 6
/** How often a running job is read while the window is in view (none is read when nothing runs). */
export const JOB_POLL_MS = 2000
/** The narrowest window with room for a tab of Kumi's beside both the chat and Kumi's pane: Hermes's sessions sidebar
 *  (237 px) + the chat (320) + the tab (320) + Kumi's pane at its width (320). Hermes splits what's left after the
 *  sidebar and the pane evenly between the chat and the tab, so at this width each gets about 320 px. Narrower, while
 *  Kumi's pane is in view, a find or a measurement holds its tab back and the pane offers it at its top
 *  (`KumiState.waiting`); with the pane out of view the tab opens as it always has. */
export const ROOM_BESIDE_LEAST = 237 + 320 + 320 + 320

/** Whether this window has room for a tab of Kumi's beside the chat and Kumi's pane (`ROOM_BESIDE_LEAST`). It reads
 *  the window Kumi's own UI is drawn in, never Hermes's markup. */
export function roomBeside(): boolean {
  const width = globalThis.innerWidth
  return typeof width !== 'number' || width >= ROOM_BESIDE_LEAST
}

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

// The map and the Measurements tab open beside the chat rather than over it: a search or a measurement arrives mid-turn,
// and Use… leaves its request in the chat's input, which a tab over the chat hid (research-1's first-hour run and D3).
// The second of them to open stacks on the first, so the chat keeps its share of the window. `plugin-workspace:<id>` is
// the pane Desktop opens for workspace `<id>`; were that ever another, Desktop places the tab by its own rule.
// Docking a tab into Kumi's own column instead (`{ pane: 'kumi:pane', pos: 'center' }`) was tried for narrow windows
// (E, Nov 18, v0.23.0): Desktop put it in the zone it keeps beside its Files zone, and that column then ran 94 to 186 px
// past the window's edge at 1100 to 760 px, and off the window under 640. So a narrow window holds the tab back.
const besideTheChat = (open: string | null) =>
  open ? { pane: `plugin-workspace:${open}`, pos: 'center' as const } : { pane: 'workspace', pos: 'right' as const }

const plugin: HermesPlugin = {
  id: PLUGIN_ID,
  name: 'Kumi',
  description: "Your DAW beside the chat: FOCUS, HISTORY with undo, a sound map of your sample folders, and Kumi's audio measurements.",
  register(ctx) {
    // English, Japanese and Simplified Chinese; Desktop drops them when the plugin unloads.
    ctx.i18n?.register(MESSAGES)
    const store = new KumiStore((path, init) => ctx.rest(path, { timeoutMs: 15_000, ...init }))
    const clock = new Clock()
    const audition = new Audition(id => ctx.rest(`/audio?id=${encodeURIComponent(id)}`, { timeoutMs: 60_000 }))
    let closeMap: (() => void) | null = null
    let mapPane: (() => void) | null = null
    let closeMeasurements: (() => void) | null = null
    let measurementsPane: (() => void) | null = null
    const mapView = () => (
      <WithT>
        <MapWorkspace audition={audition} ctx={ctx} store={store} />
      </WithT>
    )

    // Whether Kumi's pane is in view now, as the pane itself answers (`KumiPane`'s `onAsk`; out of view while it's not
    // mounted). With neither it nor a tab of Kumi's in view the status poll slows (X9). Desktop's own
    // `paneVisibility('kumi:pane')` reads false while the pane shows, since Kumi's pane is stacked with Desktop's hidden
    // Review pane, whose id stays the group's active one (`isPaneVisible`).
    let askPane: (() => boolean) | null = null
    const notePane = (inView: (() => boolean) | null) => {
      askPane = inView
    }
    const paneInView = () => askPane?.() ?? false
    // A tab is held back only while the pane is in view and the window has no room beside it (P8): the pane then takes
    // its column, and the held tab is on screen at the pane's top. Anywhere else the tab opens as it always has. The
    // pane is asked when a find or a measurement arrives, and again at each turn of the status poll, where a held tab
    // whose pane has left view since (its column closed, Files in front, a window under 640 px) opens.
    const holdBack = () => paneInView() && !roomBeside()
    const lookAtPane = () => {
      const inView = paneInView()
      const held = inView ? null : store.get().waiting
      if (held?.tab === 'map') openMap()
      else if (held?.tab === 'measurements') openMeasurements()
      return inView
    }
    // 20rem at the least: at a 1024 px window Hermes's sidebar (237 px), the chat (225 px) and its right column (160 px,
    // where Kumi's pane is) leave about 400 px, and a wider minimum squeezed the pane to nothing (H4).
    const openMap = () => {
      if (store.get().waiting?.tab === 'map') store.wait(null)
      if (typeof host.openWorkspace === 'function') {
        closeMap = host.openWorkspace(MAP_WORKSPACE, {
          title: wordsOf(ctx)('map.tab'),
          minWidth: '20rem',
          dock: besideTheChat(closeMeasurements ? MEASUREMENTS_WORKSPACE : null),
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
    const openMeasurements = () => {
      if (store.get().waiting?.tab === 'measurements') store.wait(null)
      const render = () => (
        <WithT>
          <MeasurementsWorkspace audition={audition} store={store} />
        </WithT>
      )
      const title = wordsOf(ctx)('measurements.tab')
      if (typeof host.openWorkspace === 'function') {
        closeMeasurements = host.openWorkspace(MEASUREMENTS_WORKSPACE, {
          title,
          minWidth: '20rem',
          dock: besideTheChat(closeMap ? MAP_WORKSPACE : null),
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

    // Read /status now, every few seconds while a Kumi view is in view (the pane, the map or measurements, the settings
    // page) and every 30 s while only the chip shows Kumi (X9), and on every event that says something moved. An event
    // about another profile's Kumi (#966's `profile`) moves nothing this window shows. The same turn moves the pane's
    // "4 min ago" clock on: every 30 s while the pane shows, and as it comes back into view.
    const refresh = () => void store.refreshStatus()
    refresh()
    const viewing = (paneShows: boolean) => paneShows || closeMap !== null || closeMeasurements !== null || mapPane !== null || measurementsPane !== null || store.wants('setup')
    let idle = 0
    let turns = 0
    let paneShowed = false
    ctx.setInterval(() => {
      if (document.visibilityState === 'hidden') return
      const paneShows = lookAtPane()
      if (paneShows && (!paneShowed || ++turns % IDLE_POLLS === 0)) clock.tick()
      paneShowed = paneShows
      if (!viewing(paneShows) && ++idle < IDLE_POLLS) return
      idle = 0
      refresh()
    }, POLL_MS)
    const ours = (act: (payload: Record<string, unknown>) => void) => (event: { payload?: unknown }) => {
      const payload = payloadOf(event)
      if (!forAnotherProfile(payload)) act(payload)
    }
    ctx.onEvent(`plugin.${PLUGIN_ID}.status.changed`, ours(refresh))
    ctx.onEvent(`plugin.${PLUGIN_ID}.focus.changed`, ours(() => void store.refresh('focus')))
    ctx.onEvent(`plugin.${PLUGIN_ID}.history.changed`, ours(() => void store.refresh('history')))
    ctx.onEvent(`plugin.${PLUGIN_ID}.map.changed`, ours(() => void store.refresh('map')))
    // The agent's `kumi_palette` changed the palette the map's dock shows.
    ctx.onEvent(`plugin.${PLUGIN_ID}.palette.changed`, ours(() => void store.refresh('palette')))
    ctx.setInterval(() => {
      if (document.visibilityState !== 'hidden') void store.refreshJobs()
    }, JOB_POLL_MS)
    ctx.onEvent(`plugin.${PLUGIN_ID}.job.progress`, ours(payload => store.trackJob(payload.job_id, payload.state === 'running')))
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
    const showFound = (id: string | null, revision: string | null, ids: string[], said: { near?: unknown; matches?: unknown } = {}) => {
      if (id) {
        if (calls.has(id)) return
        calls.add(id)
      }
      if (!ids.length) return
      store.found(revision, ids, said)
      if (!ctx.storage.get(OPEN_MAP_ON_FIND, true)) return
      // Held at the top of Kumi's pane while it shows in a window with no room beside it; anywhere else, opened (P8).
      if (holdBack()) store.wait({ tab: 'map', count: ids.length })
      else openMap()
    }
    ctx.onEvent(`plugin.${PLUGIN_ID}.sounds.found`, event => {
      const payload = payloadOf(event)
      const id = callId(payload.tool_call_id)
      if (!id || !forChatInFront(payload)) return
      const ids = Array.isArray(payload.sound_ids) ? payload.sound_ids.filter((value): value is string => typeof value === 'string') : []
      // CC-23: per hit `{sound_id, why, closeness?}` under `matches`, and a near search's seed; never names or paths.
      showFound(id, typeof payload.map_revision === 'string' ? payload.map_revision : null, ids.slice(0, 200), { near: payload.near, matches: payload.matches })
    })
    ctx.onEvent(`plugin.${PLUGIN_ID}.listen.done`, event => {
      const payload = payloadOf(event)
      const id = callId(payload.tool_call_id)
      if (!id || !forChatInFront(payload)) return
      // A comparison counts toward its reference's rounds however it's shown (the store counts one audio once).
      store.noteListen(payload.result)
      if (calls.has(id) || waiting.has(id)) return
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
          if (holdBack()) store.wait({ tab: 'measurements' })
          else openMeasurements()
        }, LISTEN_FALLBACK_MS)
      )
    })
    // CC-19: a script's approval waits in the chat in front. The pane shows its whole code, read by its SHA-256 from
    // the file the plugin saved, until the approval settles (any outcome) or the call completes.
    ctx.onEvent(`plugin.${PLUGIN_ID}.script.pending`, event => {
      const payload = payloadOf(event)
      if (forChatInFront(payload)) store.scriptPending(payload)
    })
    ctx.onEvent(`plugin.${PLUGIN_ID}.script.settled`, event => {
      const payload = payloadOf(event)
      if (forChatInFront(payload)) store.scriptSettled(payload.tool_call_id)
    })
    ctx.onEvent('tool.complete', event => {
      const payload = payloadOf(event)
      const name = typeof payload.name === 'string' ? payload.name : ''
      if (!name.startsWith('kumi_')) return
      refresh()
      // A scan (or any call that starts a job) answers with its job's id: follow it, as news even if it has already
      // ended. `kumi_job` only reads a job that exists, which may have ended long ago.
      store.trackJob(jobOf(payload.result), name !== 'kumi_job')
      const id = callId(payload.tool_id)
      if (name === 'kumi_script') store.scriptSettled(id)
      if (name === 'kumi_listen') store.noteListen(payload.result)
      if (name === 'kumi_listen' && id) {
        // Its tool part shows, card and all: no tab.
        calls.add(id)
        waiting.get(id)?.()
        waiting.delete(id)
        return
      }
      if (name !== 'kumi_sounds') return
      const found = foundSounds(payload.result)
      if (found) showFound(id, found.revision, found.ids, found)
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
        // As Hermes's own side panes are: never squeezed below 15rem (P8: at 1000 px its words wrapped a letter at a
        // time while the map kept its 20rem), and on a narrow window an edge overlay rather than a column.
        data: { placement: 'right', width: '320px', minWidth: '15rem', collapsible: true },
        render: () => (
          <WithT>
            <KumiPane clock={clock} ctx={ctx} onAsk={notePane} onOpenMap={openMap} onOpenMeasurements={openMeasurements} store={store} />
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
              <ListenCard audition={audition} media={props} store={store} />
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
