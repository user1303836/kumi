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

import { undoRequest } from '../shared/format.ts'
import { Audition } from '../shared/hooks.ts'
import { foundSounds, KumiStore } from '../shared/store.ts'
import { ask } from './actions.ts'
import { ListenCard } from './card.tsx'
import { StatusChip } from './chip.tsx'
import { MapWorkspace } from './map.tsx'
import { KumiPane, ownerSession } from './pane.tsx'
import { OPEN_MAP_ON_FIND, SettingsPage } from './settings.tsx'

export const PLUGIN_ID = 'kumi'
export const MAP_WORKSPACE = 'kumi-sound-map'
/** How often /status is read while the window is in view: events are faster, but only reach the process that emits. */
export const POLL_MS = 5000

const plugin: HermesPlugin = {
  id: PLUGIN_ID,
  name: 'Kumi',
  description: "Your DAW beside the chat: FOCUS, HISTORY with undo, a sound map of your sample folders, and Kumi's audio measurements.",
  register(ctx) {
    const store = new KumiStore(path => ctx.rest(path, { timeoutMs: 15_000 }))
    const audition = new Audition(id => ctx.rest(`/audio?id=${encodeURIComponent(id)}`, { timeoutMs: 60_000 }))
    let closeMap: (() => void) | null = null
    let mapPane: (() => void) | null = null

    const openMap = () => {
      if (typeof host.openWorkspace === 'function') {
        closeMap = host.openWorkspace(MAP_WORKSPACE, {
          title: 'Sound map',
          minWidth: '26rem',
          render: () => <MapWorkspace audition={audition} ctx={ctx} store={store} />,
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
        title: 'Sound map',
        data: { placement: 'main' },
        render: () => <MapWorkspace audition={audition} ctx={ctx} store={store} />
      })
    }
    const openSettings = () => host.navigate(typeof sdk.pluginSettingsHref === 'function' ? sdk.pluginSettingsHref(PLUGIN_ID) : '/settings?tab=plugins')

    // Read /status now, every few seconds while the window is in view, and on every event that says something moved.
    const refresh = () => void store.refreshStatus()
    refresh()
    ctx.setInterval(() => {
      if (document.visibilityState !== 'hidden') refresh()
    }, POLL_MS)
    ctx.onEvent(`plugin.${PLUGIN_ID}.status.changed`, refresh)
    ctx.onEvent(`plugin.${PLUGIN_ID}.focus.changed`, () => void store.refresh('focus'))
    ctx.onEvent(`plugin.${PLUGIN_ID}.history.changed`, () => void store.refresh('history'))
    ctx.onEvent(`plugin.${PLUGIN_ID}.map.changed`, () => void store.refresh('map'))
    ctx.onEvent(`plugin.${PLUGIN_ID}.job.progress`, refresh)
    ctx.onEvent(`plugin.${PLUGIN_ID}.settings.changed`, () => {
      refresh()
      void store.refresh('setup')
    })
    ctx.onEvent('tool.complete', event => {
      const payload = event.payload && typeof event.payload === 'object' ? event.payload : {}
      const name = 'name' in payload && typeof payload.name === 'string' ? payload.name : ''
      if (!name.startsWith('kumi_')) return
      refresh()
      if (name !== 'kumi_sounds') return
      const found = foundSounds('result' in payload ? payload.result : undefined)
      if (!found?.ids.length) return
      store.found(found.revision, found.ids)
      if (ctx.storage.get(OPEN_MAP_ON_FIND, true)) openMap()
    })
    // Another profile or connection: another backend, another Kumi.
    for (const atom of [host.state.profile, host.state.connectionId]) {
      const stop = atom?.listen?.(() => {
        store.reset()
        audition.stop()
        refresh()
      })
      if (stop) ctx.onDispose(stop)
    }
    ctx.onDispose(() => {
      closeMap?.()
      audition.dispose()
    })

    const contributions: PluginContribution[] = [
      {
        id: 'pane',
        area: PANES_AREA,
        title: 'Kumi',
        data: { placement: 'right', width: '320px' },
        render: () => <KumiPane ctx={ctx} onOpenMap={openMap} store={store} />
      },
      {
        id: 'status',
        area: STATUSBAR_AREAS.right,
        order: 60,
        render: () => <StatusChip ctx={ctx} onOpenMap={openMap} onOpenSettings={openSettings} store={store} />
      },
      {
        id: 'open-map',
        area: PALETTE_AREA,
        data: { id: 'kumi.map', label: 'Kumi: open the sound map', keywords: ['kumi', 'sound', 'samples', 'map', 'library'], run: openMap } satisfies PaletteContribution
      },
      {
        id: 'undo-last',
        area: PALETTE_AREA,
        data: {
          id: 'kumi.undo-last',
          label: 'Kumi: undo the last change',
          keywords: ['kumi', 'undo', 'history', 'revert'],
          run: () => {
            const state = store.get()
            const last = state.status?.connected ? state.history?.items.find(item => item.undo_available) : undefined
            if (!last) {
              host.notify({ kind: 'info', message: 'Kumi has no change it can undo right now.' })
              return
            }
            void ask(ctx, undoRequest(last), { send: true, session: ownerSession(state) })
          }
        } satisfies PaletteContribution
      },
      {
        id: 'settings',
        area: PALETTE_AREA,
        data: { id: 'kumi.settings', label: 'Kumi: settings', keywords: ['kumi', 'setup', 'settings', 'daw'], run: openSettings } satisfies PaletteContribution
      }
    ]
    if (typeof sdk.TOOL_MEDIA_RENDERER_AREA === 'string') {
      contributions.push({
        id: 'listen-card',
        area: sdk.TOOL_MEDIA_RENDERER_AREA,
        data: {
          tools: ['kumi_listen'],
          render: (props: ToolMediaRendererProps) => <ListenCard audition={audition} media={props} />
        }
      })
    }
    ctx.registerMany(contributions)
    ctx.registerSettingsPage?.({
      id: 'kumi',
      title: 'Kumi',
      icon: 'pulse',
      render: () => <SettingsPage ctx={ctx} onOpenMap={openMap} store={store} />
    })
  }
}

export default plugin
