// Where the label of the sound under the pointer sits, as plain numbers (no DOM here: tested in ui/tests).

/** The label at its widest and tallest, and how far from its sound it sits. */
export const LABEL_WIDEST = 300
export const LABEL_TALLEST = 44
export const LABEL_AWAY = 24

/** The label's place for a sound at `at`, on a map with `room` clear (its width left of a side sheet, and its height):
 *  to the sound's right, level with it; to its left when the right hasn't the room and the left has more. `x` is the
 *  label's left edge when it sits to the right, and its right edge when it sits to the left, both from the map's left
 *  edge; `most` is the width it has there. */
export function labelPlace(at: readonly [number, number], room: readonly [number, number]): { side: 'left' | 'right'; x: number; top: number; most: number } {
  const [right, left] = [room[0] - at[0] - LABEL_AWAY, at[0] - LABEL_AWAY]
  const top = Math.min(Math.max(0, at[1] - LABEL_TALLEST / 2), Math.max(0, room[1] - LABEL_TALLEST))
  return right >= LABEL_WIDEST || right >= left
    ? { side: 'right', x: at[0] + LABEL_AWAY, top, most: Math.min(LABEL_WIDEST, Math.max(0, right)) }
    : { side: 'left', x: Math.max(0, left), top, most: Math.min(LABEL_WIDEST, Math.max(0, left)) }
}
