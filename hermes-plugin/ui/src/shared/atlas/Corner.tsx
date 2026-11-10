// The map's corner, bottom left: what the map is and what can be done on it now, in two lines of glass. Above, how many
// sounds in how many families, the layout as a button anyone can find (it opens the layout menu), and the zoom. Below,
// the hint line: the gestures that do something now, each a keycap and a word, written from the same table the map's
// handlers read (`gestures.ts`); the count of the picks while there are some; Grow's prompt while it names its sounds.
// On a host's first maps, the first-open steps stand over both (`FirstOpen.tsx`).
import { T } from '@kumi/theme'
import type { ReactNode, RefObject } from 'react'

import { glass } from '../mapUi.tsx'
import { S, useT } from '../ui.tsx'
import type { Deed, Gesture } from './gestures.ts'

const caps = { fontSize: 10.5, fontWeight: 600, letterSpacing: '0.14em', textTransform: 'uppercase', color: T.faint, fontVariantNumeric: 'tabular-nums', whiteSpace: 'nowrap' } as const

/** The name of the tool in hand, beside its icon in the toolbar (the icon's button already says it to a screen reader). */
export function ToolName({ children }: { children: ReactNode }) {
  return (
    <span aria-hidden style={{ color: T.accent, fontSize: 11.5, fontWeight: 600, margin: '0 7px 0 1px', whiteSpace: 'nowrap' }}>
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
        font: '600 10.5px ui-monospace, SFMono-Regular, Menlo, monospace',
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
  picked,
  hint,
  prompt,
  bottom,
  box,
  first
}: {
  /** The count in words ("8,960 sounds"), and how many families have sounds. */
  sounds: string
  families: number
  /** The layout's button (it opens the layout menu), when the map has layouts. */
  layout?: ReactNode
  /** The view's scale: 1 is the whole map. */
  zoom: number
  /** The picks' count in words, while there are some: it leads the hint line. */
  picked?: string
  /** The hint line's gestures, each with what it does (`hintOf`). */
  hint: readonly (readonly [Gesture, Deed])[]
  /** What stands on the hint line instead: Grow's prompt while it names its sounds. */
  prompt?: ReactNode
  /** How far above the map's foot it sits (the player takes the foot when a sound is picked). */
  bottom: number
  /** Its own box, for the map to keep its words from under it. */
  box?: RefObject<HTMLDivElement | null>
  /** The first-open steps still to do, while there are some: they stand over the corner's own lines. */
  first?: ReactNode
}) {
  const t = useT()
  return (
    <div
      aria-label={t('corner.label')}
      ref={box}
      role="group"
      style={{ ...glass, position: 'absolute', zIndex: 1, left: 10, bottom, maxWidth: 'calc(100% - 20px)', padding: '7px 10px', display: 'flex', flexDirection: 'column', gap: 6, transition: 'bottom 220ms cubic-bezier(.2,.8,.2,1)' }}
    >
      {first}
      <div style={{ ...S.row, gap: 10, flexWrap: 'wrap' }}>
        <span style={caps}>
          {sounds} · {t('corner.families', families)}
        </span>
        {layout}
        <span style={caps} title={t('corner.zoom')}>
          {Math.round(zoom * 100)}%
        </span>
      </div>
      {prompt ?? (
        <div style={{ ...S.row, gap: 6, flexWrap: 'wrap', fontSize: 11.5, color: T.muted }}>
          {picked && <span style={{ color: T.text, fontWeight: 650, fontVariantNumeric: 'tabular-nums' }}>{picked}</span>}
          {hint.map(([gesture, deed], index) => (
            <span key={gesture} style={{ ...S.row, gap: 5, whiteSpace: 'nowrap' }}>
              {(index > 0 || picked) && (
                <span aria-hidden style={{ opacity: 0.4 }}>
                  ·
                </span>
              )}
              <Keycap>{t(`corner.keys.${gesture}`)}</Keycap>
              {t(`corner.does.${deed}`)}
            </span>
          ))}
        </div>
      )}
    </div>
  )
}
