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
 *  root renders it once. With it come the few animations Kumi's views draw in CSS (a box's marching edge, a palette
 *  chip or a new sound popping in, the spinner while one is made), which a system asking for less motion doesn't get
 *  (the spinner turns slowly instead, so it still shows that something is happening). */
export function FocusStyle() {
  return (
    <style>
      {`.${FOCUSABLE}:focus-visible{outline:2px solid ${T.focus};outline-offset:2px;border-radius:3px}` +
        '@keyframes kumi-ants{to{stroke-dashoffset:-18}}.kumi-ants{animation:kumi-ants .7s linear infinite}' +
        '@keyframes kumi-pop{0%{transform:scale(.55) translateY(6px);opacity:0}70%{transform:scale(1.06);opacity:1}100%{transform:none;opacity:1}}' +
        '.kumi-pop{animation:kumi-pop .34s cubic-bezier(.2,.8,.3,1) both}' +
        '@keyframes kumi-spin{to{transform:rotate(360deg)}}.kumi-spin{display:inline-block;animation:kumi-spin .8s linear infinite}' +
        // Grow's vine draws itself from its first sound to its last; a bud breathes while it grows and opens once grown.
        '@keyframes kumi-vine{from{stroke-dashoffset:1}to{stroke-dashoffset:0}}.kumi-vine{stroke-dasharray:1;animation:kumi-vine .9s cubic-bezier(.3,.6,.2,1) both}' +
        '@keyframes kumi-breathe{50%{opacity:.35}}.kumi-breathe{animation:kumi-breathe 1.1s ease-in-out infinite}' +
        '@keyframes kumi-open{0%{transform:scale(0)}70%{transform:scale(1.25)}100%{transform:scale(1)}}.kumi-open{transform-box:fill-box;transform-origin:center;animation:kumi-open .42s cubic-bezier(.2,.8,.3,1) both}' +
        // The connection dot at the DAW's tempo (the beat's length is set where the dot is drawn): it swells once a
        // beat and settles before the next. Kumi's own dots also throw a ring of their colour (`kumi-beat`); a dot
        // that isn't Kumi's only swells (`kumi-swell`), and nothing of its own style is touched.
        '@keyframes kumi-beat{0%{transform:scale(1.45);box-shadow:0 0 0 1px color-mix(in srgb,currentColor 55%,transparent)}50%{transform:scale(1);box-shadow:0 0 0 5px transparent}100%{transform:scale(1);box-shadow:0 0 0 5px transparent}}' +
        '@keyframes kumi-swell{0%{transform:scale(1.45)}50%{transform:scale(1)}100%{transform:scale(1)}}' +
        '.kumi-beat{border-radius:50%;animation:kumi-beat .5s cubic-bezier(.2,.7,.3,1) infinite}.kumi-swell{animation:kumi-swell .5s cubic-bezier(.2,.7,.3,1) infinite}' +
        '@media (prefers-reduced-motion:reduce){.kumi-ants,.kumi-pop,.kumi-vine,.kumi-open,.kumi-beat,.kumi-swell{animation:none}.kumi-breathe{animation-duration:3s}.kumi-spin{animation-duration:2.4s}}'}
    </style>
  )
}

export function Chip({ children, title, tone }: { children: ReactNode; title?: string; tone?: 'accent' | 'bad' | 'good' | 'warn' }) {
  const color = tone === 'accent' ? T.accent : tone === 'good' ? T.good : tone === 'warn' ? T.warn : tone === 'bad' ? T.bad : T.secondary
  return (
    <span title={title} style={{ fontSize: 11, lineHeight: '16px', padding: '0 6px', borderRadius: 3, background: T.chip, color, whiteSpace: 'nowrap' }}>
      {children}
    </span>
  )
}

/** How a dot beats: the class, and the beat's length (`seconds`; null: it sits steady). With `color` (the dot's own)
 *  it throws a ring of it; without, it only swells, and its colours are left as they are. */
export const beating = (seconds: number | null | undefined, color?: string): { className?: string; style: CSSProperties } => {
  if (!seconds) return { style: {} }
  const animationDuration = `${seconds.toFixed(3)}s`
  return color ? { className: 'kumi-beat', style: { color, animationDuration } } : { className: 'kumi-swell', style: { animationDuration } }
}

/** A dot of `color`. With `beat` (a beat's length in seconds) it pulses in time: the connection's dot, while the DAW
 *  plays. */
export function Dot({ color, size = 7, title, beat }: { color: string; size?: number; title?: string; beat?: number | null }) {
  const pulse = beating(beat, color)
  return <span aria-hidden={!title} className={pulse.className} title={title} style={{ display: 'inline-block', width: size, height: size, borderRadius: '50%', background: color, flex: 'none', ...pulse.style }} />
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
        // A toggle that's on reads as on, not only to a screen reader (hover to play, Select, Strict or Loose).
        border: `1px solid ${strong || pressed ? T.accent : T.border}`,
        background: pressed ? `color-mix(in srgb, ${T.accent} 14%, transparent)` : 'transparent',
        color: disabled ? T.faint : strong || pressed ? T.accent : T.secondary,
        cursor: disabled ? 'default' : 'pointer',
        whiteSpace: 'nowrap',
        flex: 'none',
        transition: 'background-color 120ms, border-color 120ms, color 120ms'
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
