// A family's glyph as a small SVG, in its colour: the shape the map draws that family's sounds in, wherever a sound
// is named off the map (the hover label, a palette card).
import { GLYPH_SHAPES } from '../mapModel.ts'

export function Glyph({ glyph, color, size = 9 }: { glyph: number; color: string; size?: number }) {
  const shape = GLYPH_SHAPES[glyph]
  return (
    <svg aria-hidden height={size} style={{ flex: 'none' }} viewBox="-1.5 -1.5 3 3" width={size}>
      {shape ? <polygon fill={color} points={shape.map(([x, y]) => `${x},${y}`).join(' ')} /> : <circle cx="0" cy="0" fill={color} r="1" />}
    </svg>
  )
}
