// Kumi's controls in Hermes Desktop are Hermes's own: its Button, Badge and Tip from the SDK (each exported from v0.22.7
// on), so a button hovers, presses, focuses and disables as the app's own do, and a chip is the app's chip. The
// dashboard build swaps this file for src/dashboard/controls.tsx (the same names). A host without one of them gets
// Kumi's own drawing of it, never a missing control.
import * as sdk from '@hermes/plugin-sdk'
import type { CSSProperties, ReactElement, ReactNode } from 'react'

import { Chip, FOCUSABLE, TextButton, useRequestClick } from '../shared/ui.tsx'

/** One button order everywhere: the one action a view exists for (`primary`), the usual action (`secondary`),
 *  everything else in a row (`quiet`), and an action that loses something, after its confirm (`destructive`). */
export type Kind = 'destructive' | 'primary' | 'quiet' | 'secondary'
const VARIANT = { primary: 'default', secondary: 'secondary', quiet: 'ghost', destructive: 'destructive' } as const

export interface ButtonProps {
  children: ReactNode
  /** An answer that's a promise makes it a request's button: busy until it settles, never sent twice. */
  onClick: () => unknown
  kind?: Kind
  /** `md` (28 px) only for a form's one primary action (Make); everything else is `sm` (24 px, the target size). */
  size?: 'md' | 'sm'
  /** Its accessible name, where the face doesn't say it (a symbol, or a word many buttons share). */
  label?: string
  /** What hovering teaches, in Hermes's Tip; none where the face already says it. */
  tip?: ReactNode
  disabled?: boolean
  pressed?: boolean
}

const BUSY: CSSProperties = { opacity: 0.6, cursor: 'progress' }

export function Button({ children, onClick, kind = 'secondary', size = 'sm', label, tip, disabled, pressed }: ButtonProps) {
  const { busy, click } = useRequestClick(onClick)
  const Host = sdk.Button
  if (typeof Host !== 'function') {
    return (
      <Tip label={tip}>
        <TextButton disabled={disabled} label={label} onClick={onClick} pressed={pressed} strong={kind === 'primary'}>
          {children}
        </TextButton>
      </Tip>
    )
  }
  return (
    <Tip label={tip}>
      <Host
        // Kumi's ring, not the host's: v0.22.7 turns every focus ring in Desktop off (its styles.css), which Kumi's
        // class beats; FocusStyle takes the host's own ring off these buttons, so v0.23 shows one ring, not two.
        className={FOCUSABLE}
        aria-busy={busy || undefined}
        aria-label={label}
        aria-pressed={pressed}
        disabled={disabled}
        onClick={click}
        size={size === 'md' ? 'default' : 'sm'}
        style={busy ? BUSY : undefined}
        type="button"
        variant={VARIANT[kind]}
      >
        {children}
      </Host>
    </Tip>
  )
}

/** A chip's tones: a fact (FOCUS's view, the tempo), Kumi's own, and states. */
export type Tone = 'bad' | 'fact' | 'good' | 'kumi' | 'warn'
const BADGE = { fact: 'muted', kumi: 'default', good: 'success', warn: 'warn', bad: 'destructive' } as const
const CHIP = { fact: undefined, kumi: 'accent', good: 'good', warn: 'warn', bad: 'bad' } as const

export function Badge({ children, tone = 'fact', tip }: { children: ReactNode; tone?: Tone; tip?: ReactNode }) {
  const Host = sdk.Badge
  if (typeof Host !== 'function') return <Chip tone={CHIP[tone]}>{children}</Chip>
  return (
    <Tip label={tip}>
      <Host variant={BADGE[tone]}>{children}</Host>
    </Tip>
  )
}

/** Hermes's tooltip around one element: instant, themed, shown on keyboard focus too. Nothing while `label` is empty. */
export function Tip({ label, children }: { label?: ReactNode; children: ReactElement }) {
  const Host = sdk.Tip
  if (!label || typeof Host !== 'function') return children
  return <Host label={label}>{children}</Host>
}
