// The map's corner, bottom left: what the map is and what can be done on it now, in two lines of glass. Above, how many
// sounds in how many families (the map's one count: a search's "12 of 900 sounds" too), the layout as a button anyone
// can find (the map's one layout menu), and the zoom. Below, the hint line: the gestures that do something now, each a
// keycap and a word, written from the same table the map's handlers read (`gestures.ts`); Grow's prompt while it names
// its sounds. The picks' count is the picks' card's, not the corner's. On a host's first maps, the first-open steps
// stand over both (`FirstOpen.tsx`).
import { Tip } from '@kumi/controls'
import { T } from '@kumi/theme'
import type { ReactNode, RefObject } from 'react'

import { glass } from '../mapUi.tsx'
import { S, TYPE, useT } from '../ui.tsx'
import type { Deed, Gesture } from './gestures.ts'

const caps = { ...TYPE.caption, fontWeight: 600, letterSpacing: '0.14em', textTransform: 'uppercase', color: T.faint, fontVariantNumeric: 'tabular-nums', whiteSpace: 'nowrap' } as const

/** The name of the tool in hand, beside its icon in the toolbar (the icon's button already says it to a screen reader). */
export function ToolName({ children }: { children: ReactNode }) {
  return (
    <span aria-hidden style={{ ...TYPE.small, color: T.accent, fontWeight: 600, margin: '0 7px 0 1px', whiteSpace: 'nowrap' }}>
      {children}
    </span>
  )
}

/** A key, or a gesture, as a keycap. */
function Keycap({ children }: { children: ReactNode }) {
  return (
    <kbd
      style={{
        display: 'inline-block',
        padding: '0 6px',
        border: `1px solid color-mix(in srgb, ${T.muted} 45%, transparent)`,
        borderBottomWidth: 2,
        borderRadius: 5,
        font: '600 11px ui-monospace, SFMono-Regular, Menlo, monospace',
        lineHeight: '15px',
        color: T.text,
        background: `color-mix(in srgb, ${T.text} 5%, transparent)`,
        whiteSpace: 'nowrap'
      }}
    >
      {children}
    </kbd>
  )
}

export function Corner({
  sounds,
  families,
  layout,
  zoom,
  hint,
  prompt,
  bottom,
  box,
  first,
  cover = 0,
  compact = false,
  still = false
}: {
  /** The count in words ("8,960 sounds"), and how many families have sounds. */
  sounds: string
  families: number
  /** The layout's button (it opens the layout menu), when the map has layouts. */
  layout?: ReactNode
  /** The view's scale: 1 is the whole map. Left out where the line has no room for it. */
  zoom?: number
  /** The hint line's gestures, each with what it does (`hintOf`). */
  hint: readonly (readonly [Gesture, Deed])[]
  /** What stands on the hint line instead: Grow's prompt while it names its sounds. */
  prompt?: ReactNode
  /** How far above the map's foot it sits (the player takes the foot when a sound is picked). */
  bottom: number
  /** Its own box, for the map to keep its words from under it. */
  box?: RefObject<HTMLDivElement | null>
  /** A short map: only the first line (and a prompt), so the map keeps room for its sounds. */
  compact?: boolean
  /** The first-open steps still to do, while there are some: they stand over the corner's own lines. */
  first?: ReactNode
  /** How much of the map's right a side sheet covers: the corner keeps to the room left of it. */
  cover?: number
  /** The system asks for less motion: the corner doesn't slide when the player opens under it. */
  still?: boolean
}) {
  const t = useT()
  return (
    <div
      aria-label={t('corner.label')}
      ref={box}
      role="group"
      style={{
        ...glass,
        position: 'absolute',
        zIndex: 1,
        left: 10,
        bottom,
        maxWidth: `calc(100% - ${20 + cover}px)`,
        padding: '7px 10px',
        display: 'flex',
        flexDirection: 'column',
        gap: 6,
        transition: still ? 'none' : 'bottom 220ms cubic-bezier(.2,.8,.2,1)'
      }}
    >
      {first}
      <div style={{ ...S.row, gap: 10, flexWrap: 'wrap' }}>
        <span style={caps}>
          {sounds} · {t('corner.families', families)}
        </span>
        {layout}
        {zoom !== undefined && (
          <Tip label={t('corner.zoom')}>
            <span style={caps}>{Math.round(zoom * 100)}%</span>
          </Tip>
        )}
      </div>
      {prompt ?? (compact ? null : (
        <div style={{ ...S.row, gap: 6, flexWrap: 'wrap', ...TYPE.small, color: T.muted }}>
          {hint.map(([gesture, deed], index) => (
            <span key={gesture} style={{ ...S.row, gap: 5, whiteSpace: 'nowrap' }}>
              {index > 0 && (
                <span aria-hidden style={{ opacity: 0.4 }}>
                  ·
                </span>
              )}
              <Keycap>{t(`corner.keys.${gesture}`)}</Keycap>
              {t(`corner.does.${deed}`)}
            </span>
          ))}
        </div>
      ))}
    </div>
  )
}
