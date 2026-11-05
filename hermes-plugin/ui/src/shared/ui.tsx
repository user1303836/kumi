// Small pieces both UI halves draw with: they take every colour from the host's theme (`@kumi/theme`), so the same
// component reads as native in Hermes Desktop and in the web dashboard. Their words come from the translator a UI
// root provides (`I18n`): Desktop's for its language, the dashboard's for its locale, English outside any.
import { T } from '@kumi/theme'
import type { CSSProperties, ReactNode } from 'react'
import { createContext, useContext } from 'react'

import { english, type Translate } from './i18n.ts'

export const I18n = createContext<Translate>(english)

export function useT(): Translate {
  return useContext(I18n)
}

export const S = {
  section: { display: 'flex', flexDirection: 'column', gap: 6, padding: '10px 12px', borderBottom: `1px solid ${T.border}` },
  heading: { display: 'flex', alignItems: 'center', gap: 6, fontSize: 11, letterSpacing: '0.06em', textTransform: 'uppercase', color: T.muted },
  row: { display: 'flex', alignItems: 'center', gap: 6, minWidth: 0 },
  // A message can hold a URL or a path with no place to break: it wraps anywhere rather than widen the pane.
  text: { fontSize: 12.5, color: T.text, lineHeight: 1.45, overflowWrap: 'anywhere' },
  muted: { fontSize: 12, color: T.muted, lineHeight: 1.45, overflowWrap: 'anywhere' },
  faint: { fontSize: 11.5, color: T.faint },
  ellipsis: { overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', minWidth: 0 },
  // Read by a screen reader, not drawn: hints and live announcements.
  hidden: { position: 'absolute', width: 1, height: 1, margin: -1, padding: 0, border: 0, overflow: 'hidden', clip: 'rect(0 0 0 0)', whiteSpace: 'nowrap' }
} satisfies Record<string, CSSProperties>

/** The class every Kumi control carries, so the keyboard's focus shows on it. */
export const FOCUSABLE = 'kumi-focusable'

/** The focus ring for Kumi's controls, in the host theme's colour. Desktop turns focus rings off for every element
 *  (`*:focus-visible { outline: none }`); this rule's class beats that one, and only a keyboard focus shows it. Each UI
 *  root renders it once. */
export function FocusStyle() {
  return <style>{`.${FOCUSABLE}:focus-visible{outline:2px solid ${T.focus};outline-offset:2px;border-radius:3px}`}</style>
}

export function Chip({ children, title, tone }: { children: ReactNode; title?: string; tone?: 'accent' | 'bad' | 'good' | 'warn' }) {
  const color = tone === 'accent' ? T.accent : tone === 'good' ? T.good : tone === 'warn' ? T.warn : tone === 'bad' ? T.bad : T.secondary
  return (
    <span title={title} style={{ fontSize: 11, lineHeight: '16px', padding: '0 6px', borderRadius: 3, background: T.chip, color, whiteSpace: 'nowrap' }}>
      {children}
    </span>
  )
}

export function Dot({ color, size = 7, title }: { color: string; size?: number; title?: string }) {
  return <span aria-hidden={!title} title={title} style={{ display: 'inline-block', width: size, height: size, borderRadius: '50%', background: color, flex: 'none' }} />
}

/** A plain text button in the theme's colours: the shared views' only control. `label` names it for a screen reader
 *  when its face is a symbol (▶, ■). */
export function TextButton({
  children,
  onClick,
  title,
  label,
  pressed,
  disabled,
  strong
}: {
  children: ReactNode
  onClick: () => void
  title?: string
  label?: string
  pressed?: boolean
  disabled?: boolean
  strong?: boolean
}) {
  return (
    <button
      aria-label={label}
      aria-pressed={pressed}
      className={FOCUSABLE}
      disabled={disabled}
      onClick={onClick}
      title={title}
      type="button"
      style={{
        font: 'inherit',
        fontSize: 11.5,
        lineHeight: '18px',
        padding: '0 7px',
        borderRadius: 3,
        border: `1px solid ${strong ? T.accent : T.border}`,
        background: 'transparent',
        color: disabled ? T.faint : strong ? T.accent : T.secondary,
        cursor: disabled ? 'default' : 'pointer',
        whiteSpace: 'nowrap',
        flex: 'none'
      }}
    >
      {children}
    </button>
  )
}

export function Section({ title, right, children }: { title: ReactNode; right?: ReactNode; children: ReactNode }) {
  return (
    <section style={S.section}>
      <div style={{ ...S.heading, justifyContent: 'space-between' }}>
        {/* A heading, so a screen reader can jump between FOCUS, NOW and HISTORY; drawn as the small caps line. */}
        <h3 style={{ ...S.row, gap: 6, margin: 0, fontSize: 'inherit', fontWeight: 'inherit' }}>{title}</h3>
        {right}
      </div>
      {children}
    </section>
  )
}
