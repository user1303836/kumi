// How loud a playing sound is right now, as plain numbers (no DOM here: tested in ui/tests): read from the waveform
// `peaks.ts` decodes, and eased so the map's pulse swells with a hit and settles after it instead of flickering.

/** The waveform's size the map's pulse and hover label read: the player's, so one decode serves all three. */
export const WAVE_BARS = 160

/** The sound's level (0 to 1) `progress` of the way through it (0 to 1), between the two bars either side; 0 past
 *  either end, or with no waveform. */
export function levelAt(levels: readonly number[] | null, progress: number): number {
  if (!levels || levels.length === 0 || !(progress >= 0) || progress > 1) return 0
  const at = Math.min(levels.length - 1, progress * levels.length)
  const before = Math.floor(at)
  const after = Math.min(levels.length - 1, before + 1)
  return levels[before] + (levels[after] - levels[before]) * (at - before)
}

/** The level the pulse shows `elapsed` milliseconds after it showed `shown`, with the sound now at `level`: up at once
 *  with a hit, down over about a fifth of a second after it. */
export function follow(shown: number, level: number, elapsed: number): number {
  return level >= shown ? level : Math.max(level, shown - elapsed / 220)
}

/** How far through a sound `duration` seconds long a play is, `elapsed` milliseconds in: 0 to 1, or null once it's
 *  over (or for a sound of no known length). */
export function progressOf(elapsed: number, duration: number): number | null {
  if (!(duration > 0) || elapsed < 0) return null
  const progress = elapsed / 1000 / duration
  return progress <= 1 ? progress : null
}
