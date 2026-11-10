// The words on the sound map, as plain numbers (no DOM here: tested in ui/tests): where each family's name sits over
// its sounds, how the names fade as the map zooms in, and which of a crowd of labels have room.

/** Where a family's name sits in a layout, in map units: the middle of its sounds across, at their upper edge. */
export interface Region {
  group: number
  x: number
  y: number
}

/** Sounds on screen at most for any to carry its own name (a few hundred: more, and the names are noise), and how many
 *  names at most. */
export const NAMES_ON_SCREEN = 600
export const NAMES_MOST = 50

// Each layout's regions are worked out once and kept with its points: a sort per family is too much for every frame.
const kept = new WeakMap<object, Region[]>()

/** Each family's region in a layout: across, the middle sound of the family; up, where the sounds of its middle half
 *  begin (the third in two hundred from the top, so a stray sound doesn't lift the name off its cluster). */
export function regionsOf(points: readonly { x: number; y: number; group: number }[]): Region[] {
  const known = kept.get(points)
  if (known) return known
  const families = new Map<number, { x: number; y: number }[]>()
  for (const point of points) {
    const family = families.get(point.group)
    if (family) family.push(point)
    else families.set(point.group, [point])
  }
  const regions: Region[] = []
  for (const [group, family] of families) {
    const across = family.map(point => point.x).sort((a, b) => a - b)
    const [from, middle, to] = [across[Math.floor(across.length * 0.25)], across[Math.floor(across.length * 0.5)], across[Math.min(across.length - 1, Math.ceil(across.length * 0.75))]]
    const tops = family.filter(point => point.x >= from && point.x <= to).map(point => point.y)
    tops.sort((a, b) => a - b)
    regions.push({ group, x: middle, y: tops[Math.floor(tops.length * 0.015)] })
  }
  kept.set(points, regions)
  return regions
}

/** How strongly the families' names show at a zoom: whole while the map is in view, gone by a little past twice that,
 *  where the sounds' own names begin. */
export function regionAlpha(zoom: number): number {
  return Math.min(1, Math.max(0, (2.3 - zoom) / 0.9))
}

type Box = readonly [left: number, top: number, width: number, height: number]

/** Of labels in order, the ones with room: a box within `padX` and `padY` of one already kept, or of one in `taken`,
 *  is left out; `most` at most. Their indexes, in order. */
export function fitting(boxes: readonly Box[], most: number, padX = 0, padY = 0, taken: readonly Box[] = []): number[] {
  const kept: Box[] = [...taken]
  const fits: number[] = []
  for (let index = 0; index < boxes.length && fits.length < most; index++) {
    const [left, top, width, height] = boxes[index]
    if (kept.some(([l, t, w, h]) => left < l + w + padX && l < left + width + padX && top < t + h + padY && t < top + height + padY)) continue
    kept.push(boxes[index])
    fits.push(index)
  }
  return fits
}

/** The order sounds are offered a name in: the same for a sound from frame to frame, and mixed across the map's own
 *  order, so the first family's sounds don't take every name. */
export function nameRank(index: number): number {
  return Math.imul(index + 1, 0x9e3779b1) >>> 0
}

/** Which labels lie over empty map: a box with any sound's place inside it (or within `margin` of it) is not clear, so
 *  a name is never written across another sound. */
export function clearOf(boxes: readonly Box[], places: readonly (readonly [number, number])[], margin = 3): boolean[] {
  const ROW = 16
  const rows = new Map<number, (readonly [number, number])[]>()
  for (const place of places) {
    const row = Math.floor(place[1] / ROW)
    const list = rows.get(row)
    if (list) list.push(place)
    else rows.set(row, [place])
  }
  return boxes.map(([left, top, width, height]) => {
    for (let row = Math.floor((top - margin) / ROW); row <= Math.floor((top + height + margin) / ROW); row++) {
      for (const [x, y] of rows.get(row) ?? []) {
        if (x >= left - margin && x <= left + width + margin && y >= top - margin && y <= top + height + margin) return false
      }
    }
    return true
  })
}
