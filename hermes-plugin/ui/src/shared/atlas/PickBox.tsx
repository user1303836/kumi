// The box a drag draws over the map: a faint wash with a marching dashed edge (`kumi-ants`, in FocusStyle), its four
// corners ticked like a viewfinder's, and, at the corner the pointer holds, how many sounds letting go would pick.
import { T } from '@kumi/theme'

/** How long a corner's tick is, at most, and how far from the pointer's corner the count sits. */
const TICK = 12
const GAP = 10

export function PickBox({
  box,
  count,
  width,
  height
}: {
  /** From where the drag began to where the pointer is, in the map's pixels. */
  box: readonly [number, number, number, number]
  /** The count, in words ("12 picked"). */
  count: string
  /** The map's size: the count stays inside it. */
  width: number
  height: number
}) {
  const [w, h] = [Math.abs(box[2] - box[0]), Math.abs(box[3] - box[1])]
  const tick = Math.min(TICK, w / 2, h / 2)
  const [x1, y1] = [Math.max(0.5, w - 0.5), Math.max(0.5, h - 0.5)]
  // The count sits outside the pointer's corner, on the side the drag is going, and never off the map.
  const [right, down] = [box[2] >= box[0], box[3] >= box[1]]
  const x = right ? Math.min(box[2] + GAP, width - 96) : Math.max(box[2] - GAP, 96)
  const y = down ? Math.min(box[3] + GAP, height - 30) : Math.max(box[3] - GAP, 30)
  return (
    <>
      <svg aria-hidden height={h} style={{ position: 'absolute', left: Math.min(box[0], box[2]), top: Math.min(box[1], box[3]), overflow: 'visible', pointerEvents: 'none' }} width={w}>
        <rect
          className="kumi-ants"
          height={Math.max(0, h - 1)}
          rx={2}
          style={{ fill: `color-mix(in srgb, ${T.accent} 9%, transparent)`, stroke: T.accent, strokeOpacity: 0.6, strokeWidth: 1, strokeDasharray: '5 4' }}
          width={Math.max(0, w - 1)}
          x={0.5}
          y={0.5}
        />
        <path
          d={`M0.5 ${tick}V0.5H${tick}M${x1 - tick} 0.5H${x1}V${tick}M${x1} ${y1 - tick}V${y1}H${x1 - tick}M${tick} ${y1}H0.5V${y1 - tick}`}
          style={{ fill: 'none', stroke: `color-mix(in srgb, ${T.accent} 55%, ${T.text})`, strokeWidth: 2, strokeLinecap: 'square' }}
        />
      </svg>
      <span
        aria-hidden
        style={{
          position: 'absolute',
          left: x,
          top: y,
          transform: `translate(${right ? '0' : '-100%'}, ${down ? '0' : '-100%'})`,
          padding: '3px 9px',
          border: `1px solid color-mix(in srgb, ${T.accent} 60%, transparent)`,
          borderRadius: 999,
          background: `color-mix(in srgb, ${T.accent} 24%, ${T.raised})`,
          color: T.text,
          fontSize: 11.5,
          fontWeight: 600,
          fontVariantNumeric: 'tabular-nums',
          lineHeight: '16px',
          whiteSpace: 'nowrap',
          pointerEvents: 'none'
        }}
      >
        {count}
      </span>
    </>
  )
}
