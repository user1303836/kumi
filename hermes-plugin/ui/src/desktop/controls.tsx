// Kumi's controls in Hermes Desktop are Hermes's own: its Button, Badge, Tip and DropdownMenu from the SDK (each
// exported from v0.22.7 on), so a button hovers, presses, focuses and disables as the app's own do, a chip is the
// app's chip and a menu opens, moves and closes as the app's menus do. The dashboard build swaps this file for
// src/dashboard/controls.tsx (the same names). A host without one of them gets Kumi's own drawing of it, never a
// missing control.
import * as sdk from '@hermes/plugin-sdk'
import { T } from '@kumi/theme'
import type { CSSProperties, ReactElement, ReactNode } from 'react'

import { Chip, FOCUSABLE, HOST_CONTROL, MENU_BUTTON, TextButton, useRequestClick } from '../shared/ui.tsx'

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
  /** The button of something it shows and hides (the palette's drawer): open or not. */
  expanded?: boolean
}

const BUSY: CSSProperties = { opacity: 0.6, cursor: 'progress' }

export function Button({ children, onClick, kind = 'secondary', size = 'sm', label, tip, disabled, pressed, expanded }: ButtonProps) {
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
        // Kumi's ring on the host's own: v0.22.7 turns every focus ring in Desktop off (its styles.css), which Kumi's
        // class beats; on v0.23 it sits right on the host's ring (FocusStyle), so either version shows one ring.
        className={`${FOCUSABLE} ${HOST_CONTROL}`}
        aria-busy={busy || undefined}
        aria-expanded={expanded}
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

/** One thing a menu does. */
export interface MenuAction {
  id: string
  name: string
  /** What hovering it teaches, where the menu shows hints (the dashboard's; Hermes's menus show none on an item). */
  title?: string
  onChoose: () => void
  /** It loses something (after its own confirm): its name in the bad tone. */
  danger?: boolean
}

/** A few actions behind a quiet "⋯" button (a palette's Rename… and Forget…): Hermes's DropdownMenu, as the composer
 *  chip uses it, so it opens, moves under the arrows, closes and gives the focus back as the app's own menus do. `up`
 *  opens it above the button, for a bar at the window's foot. */
export function Menu({ label, actions, up = false }: { label: string; actions: MenuAction[]; up?: boolean }) {
  return (
    <sdk.DropdownMenu>
      <Tip label={label}>
        <sdk.DropdownMenuTrigger asChild>
          <button aria-label={label} className={`${FOCUSABLE} kumi-btn`} type="button" style={MENU_BUTTON}>
            ⋯
          </button>
        </sdk.DropdownMenuTrigger>
      </Tip>
      <sdk.DropdownMenuContent align="start" side={up ? 'top' : 'bottom'}>
        {actions.map(action => (
          <sdk.DropdownMenuItem key={action.id} onSelect={action.onChoose}>
            {action.danger ? <span style={{ color: T.bad }}>{action.name}</span> : action.name}
          </sdk.DropdownMenuItem>
        ))}
      </sdk.DropdownMenuContent>
    </sdk.DropdownMenu>
  )
}
