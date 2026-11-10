// The words the sound map's canvas writes: a family's name over its sounds like a region on an atlas (upper-case,
// tracked, in its colour, its count under it), a layout's own data words in the same hand, and, zoomed in, each sound's
// name beside it. Canvas text, never markup: a sound file's name is drawn, not parsed.
import { clearOf, fitting, NAMES_MOST } from './names.ts'

type Context = CanvasRenderingContext2D
type Box = readonly [left: number, top: number, width: number, height: number]

/** The canvas's font family, and the colours words are haloed in and counts written in. */
export interface Lettering {
  family: string
  ground: string
  muted: string
}

/** A word at a screen place (its middle), in its colour; a region also has its count, written under it. */
export interface Word {
  text: string
  count?: string
  color: string
  x: number
  y: number
}

// Tracking is the look; a canvas without `letterSpacing` writes the same words untracked.
function track(context: Context, spacing: string): void {
  if ('letterSpacing' in context) context.letterSpacing = spacing
}

/** Write `words` in the atlas's hand at `alpha`, in order, none of them under what floats over the map (`avoid`); with
 *  `room`, a word that would touch one already written is left out too. The boxes of the ones written, for what's
 *  drawn after to keep clear of. */
export function drawWords(context: Context, words: readonly Word[], alpha: number, look: Lettering, room: boolean, avoid: readonly Box[] = []): Box[] {
  if (alpha <= 0 || words.length === 0) return []
  context.save()
  context.textAlign = 'center'
  context.textBaseline = 'middle'
  context.lineJoin = 'round'
  context.strokeStyle = look.ground
  const name = `700 12px ${look.family}`
  context.font = name
  track(context, '0.2em')
  const boxes = words.map((word): Box => {
    const width = context.measureText(word.text).width
    return [word.x - width / 2, word.y - 8, width, word.count ? 30 : 16]
  })
  const written = room ? fitting(boxes, words.length, 10, 2, avoid) : boxes.flatMap((box, index) => (fitting([box], 1, 4, 2, avoid).length > 0 ? [index] : []))
  for (const index of written) {
    const word = words[index]
    context.font = name
    track(context, '0.2em')
    context.lineWidth = 5
    context.globalAlpha = 0.85 * alpha
    context.strokeText(word.text, word.x, word.y)
    context.globalAlpha = alpha
    context.fillStyle = word.color
    context.fillText(word.text, word.x, word.y)
    if (!word.count) continue
    context.font = `500 10px ${look.family}`
    track(context, '0.06em')
    context.lineWidth = 3
    context.globalAlpha = 0.8 * alpha
    context.strokeText(word.count, word.x, word.y + 14)
    context.globalAlpha = 0.85 * alpha
    context.fillStyle = look.muted
    context.fillText(word.count, word.x, word.y + 14)
  }
  context.restore()
  return written.map(index => boxes[index])
}

// A name's width in the names' font, kept: a few hundred sounds are measured on every frame of a pan.
const widths = new Map<string, number>()

/** Each sound's name beside it (`gap` pixels to its right), in its colour, offered in `rank` order: only a name with
 *  empty map to lie on, clear of the other names and of `taken`, is written; `NAMES_MOST` at most. */
export function drawNames(context: Context, names: readonly (Word & { rank: number })[], gap: number, look: Lettering, taken: readonly Box[]): void {
  if (names.length === 0) return
  context.save()
  const font = `500 10.5px ${look.family}`
  context.font = font
  context.textBaseline = 'middle'
  context.lineJoin = 'round'
  context.lineWidth = 3
  context.strokeStyle = look.ground
  if (widths.size > 6000) widths.clear()
  const offered = [...names].sort((a, b) => a.rank - b.rank)
  const boxes = offered.map((word): Box => {
    const key = `${font}|${word.text}`
    let width = widths.get(key)
    if (width === undefined) {
      width = context.measureText(word.text).width
      widths.set(key, width)
    }
    return [word.x + gap, word.y - 7, width, 14]
  })
  const clear = clearOf(
    boxes,
    offered.map(word => [word.x, word.y])
  )
  const open = boxes.flatMap((_, index) => (clear[index] ? [index] : []))
  for (const pick of fitting(open.map(index => boxes[index]), NAMES_MOST, 16, 5, taken)) {
    const index = open[pick]
    const word = offered[index]
    context.globalAlpha = 0.75
    context.strokeText(word.text, boxes[index][0], word.y)
    context.globalAlpha = 0.95
    context.fillStyle = word.color
    context.fillText(word.text, boxes[index][0], word.y)
  }
  context.restore()
}
