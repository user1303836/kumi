// What the map does with the palette, the same in both UI halves: the picks (a box, Command or Ctrl with a click,
// Pick all), gathering them into the palette, the palette's marks, hover-to-play, and a sound that can't play leaving
// the map quietly.
import { useEffect, useMemo, useState } from 'react'

import { type Audition, playFailure, PREVIEW_SECONDS, useHoverPlay } from './hooks.ts'
import type { Translate } from './i18n.ts'
import { gatherIds, marksOf } from './palette.ts'
import type { KumiState, KumiStore } from './store.ts'

export interface PaletteMap {
  /** The sounds picked to work with, on the map in hand. */
  picked: ReadonlySet<string>
  onPicked: (ids: string[], how: 'add' | 'replace' | 'toggle') => void
  clearPicks: () => void
  /** The palette's sounds on the map in hand. */
  marked: ReadonlySet<string>
  /** Play a sound (a click; with `preview`, a hover's first seconds). One that can't play here leaves the map. */
  play: (id: string, preview?: boolean) => void
  onHover: (id: string | null) => void
  /** Gather sounds into the current palette (native's Enter on the map), then pick nothing. */
  gather: (ids: string[]) => Promise<void>
}

/** The picks, the marks and the plays for a map. `hover` turns hover-to-play on; `other` hears a play's failure that
 *  isn't the sound's own (Desktop says it, the dashboard doesn't); `left` hears a sound that left the map. */
export function usePaletteMap(
  store: KumiStore,
  state: KumiState,
  audition: Audition,
  t: Translate,
  { hover, other, left }: { hover: boolean; other?: (error: unknown) => void; left?: (id: string) => void }
): PaletteMap {
  const revision = state.map?.revision ?? null
  const [picks, setPicks] = useState<{ revision: string | null; ids: ReadonlySet<string> }>({ revision, ids: new Set() })
  // Ids belong to one map revision: a new map picks nothing.
  const picked = picks.revision === revision ? picks.ids : new Set<string>()
  useEffect(() => setPicks(current => (current.revision === revision ? current : { revision, ids: new Set() })), [revision])
  const marked = useMemo(() => marksOf(state.palette, revision), [state.palette, revision])

  const play = (id: string, preview = false) => {
    audition.play(id, preview ? { preview: PREVIEW_SECONDS } : {}).catch(error => {
      const why = playFailure(error)
      if (why === 'gone' || why === 'unplayable') {
        store.hide(id, why)
        left?.(id)
      } else if (why === 'other') {
        other?.(error)
      }
    })
  }
  const onHover = useHoverPlay(audition, hover, id => play(id, true))

  return {
    picked,
    marked,
    play,
    onHover,
    onPicked: (ids, how) =>
      setPicks(current => {
        const base = current.revision === revision ? current.ids : new Set<string>()
        const next = how === 'replace' ? new Set<string>() : new Set(base)
        for (const id of ids) {
          if (how === 'toggle' && next.has(id)) next.delete(id)
          else next.add(id)
        }
        return { revision, ids: next }
      }),
    clearPicks: () => setPicks({ revision, ids: new Set() }),
    gather: async ids => {
      const { ids: sent, dropped } = gatherIds(ids)
      if (sent.length === 0) return
      const done = await store.paletteDo({ action: 'gather', sound_ids: sent }, dropped > 0 ? t('dock.firstOnly', sent.length + dropped) : undefined)
      if (done) setPicks({ revision, ids: new Set() })
    }
  }
}
