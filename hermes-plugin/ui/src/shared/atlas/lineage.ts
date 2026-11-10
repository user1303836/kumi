// A made sound's lineage, as plain data (no DOM here: tested in ui/tests): which families the sounds it was made from
// belong to, as their colours, each with its share. A sound Kumi made wears these wherever it shows (its waveform left
// to right, the rule on its palette card), so what it is made of reads at a glance.
import type { DesignSource } from '../design.ts'

/** One family's part in a made sound: its colour (null where no family is known: the accent stands in) and its share
 *  of the whole, 0 to 1. */
export interface Share {
  color: string | null
  share: number
}

/** What a window knows of the sounds something was made from. */
export interface Parentage {
  /** A map sound's colour (its family's); null for a sound that isn't on the map in hand. */
  sound: (id: string) => string | null
  /** What a made sound was itself made from, when this window knows (it was made here, or the engine's history names
   *  it): its sources, and for a grown sound how far along it is. Null otherwise. */
  made: (artifactId: string) => { sources: readonly DesignSource[]; at?: number } | null
}

/** How many families a lineage names at most: the largest, in their own order. */
const MOST_FAMILIES = 6
/** How many generations back a made sound's own parents are followed. */
const GENERATIONS = 3

/** The lineage of a sound made from `sources`: the families in the sources' own order (left to right), each with its
 *  share. Every source counts the same, except in a sound grown `at` of the way between two, which is that much its
 *  second. A source Kumi made stands for its own lineage when that is known, else for the accent. */
export function lineageOf(sources: readonly DesignSource[], parentage: Parentage, at?: number, generations = GENERATIONS): Share[] {
  const shares = new Map<string | null, number>()
  const grown = sources.length === 2 && typeof at === 'number' && at >= 0 && at <= 1
  sources.forEach((source, index) => {
    const weight = grown ? (index === 0 ? 1 - at : at) : 1 / sources.length
    const parents = source.sound_id || !source.artifact_id || generations <= 0 ? null : parentage.made(source.artifact_id)
    const parts: Share[] = parents ? lineageOf(parents.sources, parentage, parents.at, generations - 1) : [{ color: source.sound_id ? parentage.sound(source.sound_id) : null, share: 1 }]
    for (const part of parts) shares.set(part.color, (shares.get(part.color) ?? 0) + part.share * weight)
  })
  let lineage = [...shares].map(([color, share]) => ({ color, share })).filter(part => part.share > 0)
  if (lineage.length > MOST_FAMILIES) {
    const least = [...lineage].sort((a, b) => b.share - a.share)[MOST_FAMILIES - 1].share
    lineage = lineage.filter(part => part.share >= least).slice(0, MOST_FAMILIES)
  }
  const whole = lineage.reduce((sum, part) => sum + part.share, 0)
  return whole > 0 ? lineage.map(part => ({ color: part.color, share: part.share / whole })) : []
}

/** The map sounds a sound made from `sources` comes from: each source that is a map sound, and, for a source Kumi made,
 *  the sounds that one came from when the window knows them. Each id once, in the sources' order. */
export function parentsOf(sources: readonly DesignSource[], made: Parentage['made'], generations = GENERATIONS): string[] {
  const parents = new Set<string>()
  for (const source of sources) {
    if (source.sound_id) parents.add(source.sound_id)
    else if (source.artifact_id && generations > 0) for (const id of parentsOf(made(source.artifact_id)?.sources ?? [], made, generations - 1)) parents.add(id)
  }
  return [...parents]
}
