// What the map does with the palette, the same in both UI halves: the picks (a box, Command or Ctrl with a click,
// Pick all), gathering them into the palette, the palette's marks, hover-to-play, a sound that can't play leaving the
// map quietly, and the Make panel's sounds (the picks or the palette) and what it keeps.
import { T } from '@kumi/theme'
import { useEffect, useMemo, useState } from 'react'

import { lineageOf, parentsOf, type Share } from './atlas/lineage.ts'
import { type DesignResult, type DesignSource, sourcesOf } from './design.ts'
import { type Audition, playFailure, PREVIEW_SECONDS, useHoverPlay } from './hooks.ts'
import type { Translate } from './i18n.ts'
import { groupColor } from './mapModel.ts'
import { gatherIds, marksOf, type PaletteView } from './palette.ts'
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
  /** The sounds of the last gather, flying from the map into the dock (the map's `flying`). */
  flying: { ids: string[]; at: number } | null
  /** The sounds the open Make panel makes from; null while it's closed. */
  making: DesignSource[] | null
  /** Open the Make panel on the picks (in the map's order) or on the palette's playable sounds. */
  makeFromPicks: (ids: string[]) => void
  makeFromPalette: (view: PaletteView) => void
  /** Make from these sounds instead (results chosen in the panel, or the ones it had before). */
  makeFrom: (sources: DesignSource[]) => void
  closeMake: () => void
  /** A made sound into the current palette; true when it went in. */
  keep: (result: DesignResult) => Promise<boolean>
  /** The made sounds the current palette holds. */
  kept: ReadonlySet<string>
  /** A sound's colour on the map (its group's); a made sound's is the accent. */
  colorOf: (source: DesignSource) => string
  /** The families a sound made from `sources` comes from, as colours with their shares (`at`: a grown sound's place
   *  between its two); and the same for a made sound by its id, when this window knows what it was made from. */
  lineage: (sources: readonly DesignSource[], at?: number) => Share[]
  madeLineage: (artifactId: string) => Share[] | null
  /** The sounds to ring on the map: the parents of the made sound the pointer or the keyboard is on. `showKin` sets
   *  them from a result's sources (null: none), `showKinOf` from a made sound's id: none for a sound this window
   *  doesn't know the sources of (it knows what it made, and what Make… read on opening; a hover reads nothing). */
  kin: ReadonlySet<string> | null
  showKin: (sources: readonly DesignSource[] | null) => void
  showKinOf: (artifactId: string | null) => void
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
  const [flying, setFlying] = useState<{ ids: string[]; at: number } | null>(null)
  const [making, setMaking] = useState<DesignSource[] | null>(null)
  const kept = useMemo(() => new Set((state.palette?.current?.items ?? []).flatMap(item => (item.artifact_id ? [item.artifact_id] : []))), [state.palette])
  const groupOf = useMemo(() => new Map((state.map?.points ?? []).map(([id, , , group]) => [id, group])), [state.map])
  // What this window knows of each made sound: the engine's answers, by id (made here, or read from its history).
  const designed = useMemo(() => new Map((state.designs ?? []).map(result => [result.artifact_id, result])), [state.designs])
  const parentage = {
    sound: (id: string) => {
      const group = groupOf.get(id)
      return group === undefined ? null : groupColor(state.map?.groups[String(group)]?.color, group)
    },
    made: (id: string) => designed.get(id) ?? null
  }
  // The made sound the pointer or the keyboard is on: its sources, or its id (its sources are `designed`'s, once read).
  const [kinOf, setKinOf] = useState<readonly DesignSource[] | string | null>(null)
  const kin = useMemo(() => {
    if (kinOf === null) return null
    return new Set(parentsOf(typeof kinOf === 'string' ? (designed.get(kinOf)?.sources ?? []) : kinOf, id => designed.get(id) ?? null))
  }, [kinOf, designed])
  // Its sounds' ids belong to the map they were picked on: a new map closes the panel (the results stay, for later).
  useEffect(() => setMaking(current => (current?.some(source => source.sound_id && source.map_revision !== revision) ? null : current)), [revision])
  const open = (sources: DesignSource[]) => {
    setMaking(sources)
    // The engine's earlier results, once a window: a variation history that survives a reload, kept after what this
    // window made already (a grow renders before the panel ever opens).
    void store.readDesignsOnce()
  }

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
    flying,
    making,
    kept,
    makeFromPicks: ids => open(sourcesOf(ids.map(id => ({ sound_id: id, name: state.map?.sounds[id]?.name ?? null })), revision)),
    makeFromPalette: view => open(sourcesOf(view.items, state.palette?.map_revision ?? null)),
    closeMake: () => setMaking(null),
    makeFrom: open,
    keep: result => store.paletteDo({ action: 'gather', artifact_ids: [result.artifact_id] }),
    colorOf: source => {
      const group = source.sound_id ? groupOf.get(source.sound_id) : undefined
      return group === undefined ? T.accent : groupColor(state.map?.groups[String(group)]?.color, group)
    },
    lineage: (sources, at) => lineageOf(sources, parentage, at),
    madeLineage: id => {
      const result = designed.get(id)
      return result ? lineageOf(result.sources, parentage, result.at) : null
    },
    kin,
    showKin: setKinOf,
    showKinOf: setKinOf,
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
      // The sounds take off at once; they land as the palette's answer comes back.
      setFlying({ ids: sent.filter(id => !marked.has(id)), at: performance.now() })
      const done = await store.paletteDo({ action: 'gather', sound_ids: sent }, dropped > 0 ? t('dock.firstOnly', sent.length + dropped) : undefined)
      if (done) setPicks({ revision, ids: new Set() })
    }
  }
}
