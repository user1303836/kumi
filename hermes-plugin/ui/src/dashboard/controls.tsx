// Kumi's controls in the web dashboard: the same names as src/desktop/controls.tsx, drawn by Kumi on the dashboard's
// own tokens at the same sizes (a 24 px button, 12 px words, the button order), with the hover and press the
// `kumi-btn` class gives (FocusStyle). The dashboard's tooltip is the browser's `title`: it has no Tip of its own.
import { T } from '@kumi/theme'
import type { CSSProperties, ReactElement, ReactNode } from 'react'

import { Chip, FOCUSABLE, useRequestClick } from '../shared/ui.tsx'

export type Kind = 'destructive' | 'primary' | 'quiet' | 'secondary'

export interface ButtonProps {
  children: ReactNode
  onClick: () => unknown
  kind?: Kind
  size?: 'md' | 'sm'
  label?: string
  tip?: ReactNode
  disabled?: boolean
  pressed?: boolean
  expanded?: boolean
}

const KIND: Record<Kind, CSSProperties> = {
  primary: { background: T.accent, color: 'var(--color-primary-foreground, #fff)' },
  secondary: { background: T.chip, color: T.text },
  quiet: { background: 'transparent', color: T.secondary },
  destructive: { background: `color-mix(in srgb, ${T.bad} 14%, transparent)`, color: T.bad }
}

export function Button({ children, onClick, kind = 'secondary', size = 'sm', label, tip, disabled, pressed, expanded }: ButtonProps) {
  const { busy, click } = useRequestClick(onClick)
  return (
    <button
      aria-busy={busy || undefined}
      aria-expanded={expanded}
      aria-label={label}
      aria-pressed={pressed}
      className={`${FOCUSABLE} kumi-btn`}
      disabled={disabled}
      onClick={click}
      title={typeof tip === 'string' ? tip : undefined}
      type="button"
      style={{
        ...KIND[kind],
        ...(pressed ? { background: `color-mix(in srgb, ${T.accent} 16%, transparent)`, color: T.accent } : {}),
        display: 'inline-flex',
        alignItems: 'center',
        gap: 6,
        flex: 'none',
        font: 'inherit',
        fontSize: 12,
        lineHeight: '16px',
        fontWeight: 500,
        padding: size === 'md' ? '6px 12px' : '4px 10px',
        border: 'none',
        borderRadius: 3,
        whiteSpace: 'nowrap',
        cursor: disabled ? 'default' : busy ? 'progress' : 'pointer',
        opacity: disabled ? 0.5 : busy ? 0.6 : 1
      }}
    >
      {children}
    </button>
  )
}

export type Tone = 'bad' | 'fact' | 'good' | 'kumi' | 'warn'
const CHIP = { fact: undefined, kumi: 'accent', good: 'good', warn: 'warn', bad: 'bad' } as const

export function Badge({ children, tone = 'fact', tip }: { children: ReactNode; tone?: Tone; tip?: ReactNode }) {
  return (
    <Chip title={typeof tip === 'string' ? tip : undefined} tone={CHIP[tone]}>
      {children}
    </Chip>
  )
}

/** The browser's `title` on a box that draws nothing of its own (`display: contents`), around the one element. */
export function Tip({ label, children }: { label?: ReactNode; children: ReactElement }) {
  if (typeof label !== 'string' || !label) return children
  return (
    <span style={{ display: 'contents' }} title={label}>
      {children}
    </span>
  )
}
