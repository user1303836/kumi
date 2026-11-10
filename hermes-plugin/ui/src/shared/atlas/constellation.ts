// A sound's constellation: the few sounds nearest it on the map, joined to the sound under the pointer by thin lines
// in its colour. In the similarity layout they are its "more like this"; in another, its nearest in that layout's terms.

/** How many sounds a constellation joins to its own. */
export const STARS = 5

/** The `most` sounds nearest sound `index` among `points` (their places in a layout), nearest first; `shown` leaves
 *  out the ones the search or the legend hides. A map of fewer sounds gives fewer. */
export function nearestTo(points: readonly { x: number; y: number }[], index: number, most: number, shown?: (index: number) => boolean): number[] {
  const own = points[index]
  if (!own || most <= 0) return []
  // The nearest so far, as [distance squared, index], nearest first: a map is thousands of sounds and `most` a handful.
  const best: [number, number][] = []
  for (let other = 0; other < points.length; other++) {
    if (other === index || (shown && !shown(other))) continue
    const distance = (points[other].x - own.x) ** 2 + (points[other].y - own.y) ** 2
    if (best.length === most && distance >= best[most - 1][0]) continue
    let at = best.length
    while (at > 0 && best[at - 1][0] > distance) at--
    best.splice(at, 0, [distance, other])
    if (best.length > most) best.pop()
  }
  return best.map(([, other]) => other)
}
