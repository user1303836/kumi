// What each gesture does on the sound map, in the state the map is in: one table, as plain data (no DOM here: tested
// in ui/tests). The map's pointer and key handlers ask it what a drag or Escape does, and the hint line in the map's
// corner is written from it, so the line can't say what the map doesn't do.

/** The map's state, as far as the pointer and the keys go. */
export interface MapState {
  /** The map picks sounds (its parent takes the picks): without it a drag only moves the map. */
  picks: boolean
  /** The hand tool is on, or Space is held: the left button moves the map. */
  hand: boolean
  /** Grow is naming its two sounds: a click names one. */
  naming: boolean
  /** How many sounds are picked. */
  picked: number
  /** Hover-to-play is on. */
  hover: boolean
  /** What a scroll last did on this map: true, it moved the map (a trackpad's two fingers); false, it zoomed (a
   *  mouse's wheel); null, the map has seen none yet. */
  scrollPans: boolean | null
}

/** The gestures the map takes, and what one can do. */
export type Gesture = 'click' | 'drag' | 'escape' | 'hover' | 'middleDrag' | 'pinch' | 'scroll' | 'shiftDrag'
export type Deed = 'add' | 'addMore' | 'cancel' | 'clear' | 'hear' | 'name' | 'pan' | 'pick' | 'zoom'

/** What every gesture does in `state`; a gesture that does nothing there is left out. */
export function gesturesOf(state: MapState): Partial<Record<Gesture, Deed>> {
  const boxes = state.picks && !state.hand && !state.naming
  return {
    click: state.naming ? 'name' : 'hear',
    drag: boxes ? 'pick' : 'pan',
    ...(boxes ? { shiftDrag: state.picked > 0 ? 'addMore' : 'add' } : {}),
    ...(state.hover ? { hover: 'hear' } : {}),
    scroll: state.scrollPans ? 'pan' : 'zoom',
    pinch: 'zoom',
    middleDrag: 'pan',
    ...(state.naming ? { escape: 'cancel' } : state.picks && state.picked > 0 ? { escape: 'clear' } : {})
  }
}

/** The hint line for `state`: the gestures worth saying now, in reading order, each with what it does. While Grow
 *  names its sounds its own prompt stands there instead (no gestures). With sounds picked, what the picks take. With
 *  the hand tool on, its own line: a drag moves the map, so Shift and the middle button aren't worth a word. */
export function hintOf(state: MapState): [Gesture, Deed][] {
  const does = gesturesOf(state)
  // The pointer first (what it plays, then what a drag does), then the wheel. A trackpad scrolls to move and pinches
  // to zoom, and has no middle button; a mouse has one.
  const pointer: Gesture[] = [state.hover ? 'hover' : 'click', 'drag', 'shiftDrag', 'scroll', ...(state.scrollPans ? (['pinch'] as const) : state.hand ? [] : (['middleDrag'] as const))]
  const said: Gesture[] = state.naming ? [] : state.hand ? [...pointer, 'escape'] : state.picked > 0 && does.escape ? ['shiftDrag', 'escape'] : pointer
  return said.flatMap((gesture): [Gesture, Deed][] => (does[gesture] ? [[gesture, does[gesture]]] : []))
}
