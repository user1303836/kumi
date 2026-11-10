// The pointer's trail over the sound map while hover-to-play is on, as plain numbers (no DOM here: tested in
// ui/tests): where it has just been, each step in the colour of what was playing as it passed, fading over a little
// more than half a second. When the pointer rests or leaves, nothing new is added and the trail runs out.

/** How long a step of the trail shows. */
export const TRAIL_MS = 600
/** How many steps are kept: a fast sweep's half second. */
const MOST_STEPS = 72

/** One place the pointer was: when, in which colour, and whether it joins the step before it (the first step after the
 *  pointer came back, or pressed, starts a new line). */
export interface Step {
  x: number
  y: number
  at: number
  color: string
  joined: boolean
}

/** The path with the pointer's move to (`x`, `y`) at `now` added, in `color`; `lifted` starts a new line. A move of
 *  under two pixels adds nothing: a pointer that only trembles is resting. Steps past their time are dropped. */
export function stepTo(path: readonly Step[], x: number, y: number, now: number, color: string, lifted: boolean): Step[] {
  const last = path[path.length - 1]
  const live = path.filter(step => now - step.at < TRAIL_MS)
  if (last && !lifted && Math.hypot(x - last.x, y - last.y) < 2) return live
  return [...live, { x, y, at: now, color, joined: !lifted && live.length > 0 && live[live.length - 1] === last }].slice(-MOST_STEPS)
}

/** How strongly a step shows at `now`: 1 as it's made, 0 once its time is up. */
export function strengthOf(step: Step, now: number): number {
  return Math.min(1, Math.max(0, 1 - (now - step.at) / TRAIL_MS))
}
