// Kumi's controls in the web dashboard: the same names as src/desktop/controls.tsx, drawn by Kumi on the dashboard's
// own tokens at the same sizes (a 24 px button, 12 px words, the button order), with the hover and press the
// `kumi-btn` class gives (FocusStyle). The dashboard's tooltip is the browser's `title`: it has no Tip of its own.
import { T } from '@kumi/theme'
import type { CSSProperties, KeyboardEvent, ReactElement, ReactNode } from 'react'
import { cloneElement, useEffect, useRef, useState } from 'react'

import { Chip, FOCUSABLE, MENU_BUTTON, TYPE, useRequestClick } from '../shared/ui.tsx'

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
        ...TYPE.small,
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

/** The browser's `title`, set on the one element itself: no wrapper, so the element keeps its place among its
 *  siblings, as Hermes's Tip (`asChild`) keeps it in Desktop. A wrapper broke Make's radio groups: their arrow keys
 *  move the focus to the next sibling (`radioKeys`), and with each radio in a box of its own there was none. */
export function Tip({ label, children }: { label?: ReactNode; children: ReactElement }) {
  if (typeof label !== 'string' || !label) return children
  return cloneElement(children as ReactElement<{ title?: string }>, { title: label })
}

/** One thing a menu does. */
export interface MenuAction {
  id: string
  name: string
  /** What hovering it teaches (the browser's `title` here). */
  title?: string
  onChoose: () => void
  /** It loses something (after its own confirm): its name in the bad tone. */
  danger?: boolean
}

/** A few actions behind a quiet "⋯" button (a palette's Rename… and Forget…), drawn by Kumi: the dashboard has no
 *  menu of its own. A choice, Escape or a press anywhere else closes it, and the focus goes back to its button; the
 *  arrow keys move between its items. `up` opens it above the button, for a bar at the window's foot. */
export function Menu({ label, actions, up = false }: { label: string; actions: MenuAction[]; up?: boolean }) {
  const [open, setOpen] = useState(false)
  const box = useRef<HTMLDivElement | null>(null)
  const button = useRef<HTMLButtonElement | null>(null)
  const items = useRef<(HTMLButtonElement | null)[]>([])
  useEffect(() => {
    if (!open) return
    const away = (event: PointerEvent) => {
      if (!box.current?.contains(event.target as Node)) setOpen(false)
    }
    window.addEventListener('pointerdown', away, true)
    return () => window.removeEventListener('pointerdown', away, true)
  }, [open])
  useEffect(() => {
    if (open) items.current[0]?.focus()
  }, [open])
  const close = () => {
    setOpen(false)
    button.current?.focus()
  }
  const keys = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!open) return
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      close()
      return
    }
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
    event.preventDefault()
    const at = items.current.findIndex(item => item === event.target)
    items.current[(at + (event.key === 'ArrowDown' ? 1 : actions.length - 1)) % actions.length]?.focus()
  }
  return (
    <div onKeyDown={keys} ref={box} style={{ position: 'relative', display: 'flex' }}>
      <button
        aria-expanded={open}
        aria-haspopup="menu"
        aria-label={label}
        className={`${FOCUSABLE} kumi-btn`}
        onClick={() => setOpen(!open)}
        ref={button}
        title={label}
        type="button"
        style={{ ...MENU_BUTTON, background: open ? T.chip : 'transparent' }}
      >
        ⋯
      </button>
      {open && (
        <div
          aria-label={label}
          role="menu"
          style={{ position: 'absolute', ...(up ? { bottom: 28 } : { top: 28 }), left: 0, zIndex: 3, minWidth: 160, width: 'max-content', maxWidth: 280, padding: 4, display: 'flex', flexDirection: 'column', gap: 1, borderRadius: 6, border: `1px solid ${T.border}`, background: T.raised, boxShadow: '0 8px 24px -8px rgba(0,0,0,.45)' }}
        >
          {actions.map((action, index) => (
            <button
              className={`${FOCUSABLE} kumi-btn`}
              key={action.id}
              onClick={() => {
                close()
                action.onChoose()
              }}
              ref={element => {
                items.current[index] = element
              }}
              role="menuitem"
              title={action.title}
              type="button"
              style={{ font: 'inherit', ...TYPE.small, textAlign: 'left', padding: '5px 10px', borderRadius: 4, border: 'none', background: 'transparent', color: action.danger ? T.bad : T.text, cursor: 'pointer', whiteSpace: 'nowrap' }}
            >
              {action.name}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
