// The sound map's floating pieces, drawn the same in Desktop and the dashboard: a glass surface for what floats over
// the map (in the theme's own colours, a little see-through, with a soft shadow), the icons the map's tools wear, and
// the round buttons and pill groups the toolbar is made of. Every icon is drawn here in SVG, in `currentColor`, so it
// follows the theme and needs no font.
import { T } from '@kumi/theme'
import type { CSSProperties, KeyboardEvent, ReactNode } from 'react'
import { useEffect, useRef, useState } from 'react'

import { FOCUSABLE, S } from './ui.tsx'

/** What floats over the map: the raised surface, slightly see-through and blurred behind, with a soft shadow. */
export const glass: CSSProperties = {
  background: `color-mix(in srgb, ${T.raised} 90%, transparent)`,
  backdropFilter: 'blur(12px) saturate(1.15)',
  WebkitBackdropFilter: 'blur(12px) saturate(1.15)',
  border: `1px solid color-mix(in srgb, ${T.border} 85%, transparent)`,
  boxShadow: '0 10px 30px -12px rgba(0,0,0,.55), 0 1px 0 rgba(255,255,255,.03) inset',
  borderRadius: 12
}

export type IconName = 'arrange' | 'close' | 'fit' | 'grow' | 'hand' | 'hover' | 'layers' | 'make' | 'near' | 'pick' | 'play' | 'plus' | 'reset' | 'stop' | 'tiles'

const PATHS: Record<IconName, ReactNode> = {
  pick: <path d="M4 2.5 L4 13 L6.8 10.4 L8.7 14.5 L10.4 13.7 L8.6 9.7 L12.4 9.5 Z" fill="currentColor" strokeLinejoin="round" />,
  hand: (
    <path
      d="M5.2 8.5 V3.8 a1 1 0 0 1 2 0 V7.6 V2.9 a1 1 0 0 1 2 0 V7.6 V3.6 a1 1 0 0 1 2 0 V8 V5.4 a1 1 0 0 1 2 0 V9.6 c0 2.6 -1.8 4.6 -4.4 4.6 c-1.7 0 -2.7 -.7 -3.6 -1.9 L2.6 9.6 a1 1 0 0 1 1.5 -1.3 Z"
      fill="none"
      stroke="currentColor"
      strokeLinejoin="round"
      strokeWidth="1.2"
    />
  ),
  tiles: (
    <>
      <rect fill="currentColor" height="4.6" rx="1.2" width="4.6" x="2" y="2" />
      <rect fill="currentColor" fillOpacity=".7" height="4.6" rx="1.2" width="4.6" x="9.4" y="2" />
      <rect fill="currentColor" fillOpacity=".7" height="4.6" rx="1.2" width="4.6" x="2" y="9.4" />
      <rect fill="currentColor" fillOpacity=".45" height="4.6" rx="1.2" width="4.6" x="9.4" y="9.4" />
    </>
  ),
  hover: (
    <>
      <path d="M3 9.5 V8 a5 5 0 0 1 10 0 V9.5" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="1.3" />
      <rect fill="currentColor" height="4.6" rx="1.2" width="2.8" x="2.2" y="9" />
      <rect fill="currentColor" height="4.6" rx="1.2" width="2.8" x="11" y="9" />
    </>
  ),
  fit: <path d="M2.5 6 V2.5 H6 M10 2.5 H13.5 V6 M13.5 10 V13.5 H10 M6 13.5 H2.5 V10" fill="none" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.4" />,
  grow: (
    <>
      <path d="M2.5 13.5 C5 13 6 10.5 7.5 8.5 S11 4 13.5 2.5" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="1.3" />
      <circle cx="2.6" cy="13.4" fill="currentColor" r="1.6" />
      <circle cx="13.4" cy="2.6" fill="currentColor" r="1.6" />
      <path d="M6.6 9.6 c-1.6 .2 -2.4 -.6 -2.6 -2 c1.4 -.1 2.4 .5 2.6 2 Z M9.4 5.8 c.3 -1.6 1.3 -2.2 2.6 -2 c-.1 1.4 -1 2.2 -2.6 2 Z" fill="currentColor" fillOpacity=".8" />
    </>
  ),
  arrange: (
    <>
      <circle cx="5" cy="5" fill="currentColor" r="2" />
      <path d="M8.5 8.5 L13.5 13.5 M13.5 9.6 V13.5 H9.6" fill="none" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.4" />
    </>
  ),
  layers: (
    <>
      <path d="M8 2 L14 5.2 L8 8.4 L2 5.2 Z" fill="currentColor" fillOpacity=".9" />
      <path d="M2.4 8 L8 11 L13.6 8 M2.4 10.8 L8 13.8 L13.6 10.8" fill="none" stroke="currentColor" strokeLinejoin="round" strokeWidth="1.2" />
    </>
  ),
  close: <path d="M4 4 L12 12 M12 4 L4 12" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="1.6" />,
  play: <path d="M5 3 L13 8 L5 13 Z" fill="currentColor" strokeLinejoin="round" />,
  stop: <rect fill="currentColor" height="9" rx="1.5" width="9" x="3.5" y="3.5" />,
  plus: <path d="M8 3 V13 M3 8 H13" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="1.6" />,
  make: (
    <>
      <circle cx="6" cy="8" fill="currentColor" fillOpacity=".55" r="4" />
      <circle cx="10" cy="8" fill="currentColor" fillOpacity=".55" r="4" />
    </>
  ),
  near: (
    <>
      <circle cx="8" cy="8" fill="currentColor" r="1.8" />
      <circle cx="8" cy="8" fill="none" r="4.4" stroke="currentColor" strokeDasharray="2 1.6" strokeWidth="1.2" />
      <circle cx="13.2" cy="4" fill="currentColor" r="1.1" />
      <circle cx="3.2" cy="12.4" fill="currentColor" r="1.1" />
    </>
  ),
  reset: <path d="M3.5 8 a4.5 4.5 0 1 0 1.4 -3.3 M3.5 2.8 V5.6 H6.3" fill="none" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.4" />
}

export function Icon({ name, size = 16 }: { name: IconName; size?: number }) {
  return (
    <svg aria-hidden height={size} style={{ flex: 'none', display: 'block' }} viewBox="0 0 16 16" width={size}>
      {PATHS[name]}
    </svg>
  )
}

/** A round tool button: an icon, its name for a screen reader and a tooltip, pressed while its tool or toggle is on;
 *  with `expanded`, the button of a menu (open or not) instead of a toggle. */
export function ToolButton({ icon, label, title, pressed, onClick, children, disabled, expanded }: { icon: IconName; label: string; title?: string; pressed?: boolean; onClick: () => void; children?: ReactNode; disabled?: boolean; expanded?: boolean }) {
  return (
    <button
      aria-expanded={expanded}
      aria-haspopup={expanded === undefined ? undefined : 'menu'}
      aria-label={children ? undefined : label}
      aria-pressed={expanded === undefined ? pressed : undefined}
      className={FOCUSABLE}
      disabled={disabled}
      onClick={onClick}
      title={title ?? label}
      type="button"
      style={{
        ...S.row,
        justifyContent: 'center',
        gap: 5,
        minWidth: 30,
        height: 30,
        padding: children ? '0 10px 0 8px' : 0,
        font: 'inherit',
        fontSize: 12,
        fontWeight: pressed ? 600 : 500,
        borderRadius: 15,
        border: 'none',
        background: pressed ? `color-mix(in srgb, ${T.accent} 22%, transparent)` : 'transparent',
        color: disabled ? T.faint : pressed ? T.accent : T.secondary,
        cursor: disabled ? 'default' : 'pointer',
        boxShadow: pressed ? `0 0 0 1px color-mix(in srgb, ${T.accent} 45%, transparent) inset` : 'none',
        transition: 'background-color 140ms, color 140ms, box-shadow 160ms, transform 120ms',
        flex: 'none'
      }}
    >
      <Icon name={icon} />
      {children}
    </button>
  )
}

/** A pill of tool buttons, with a hairline between groups (`Rule`). It stacks above the legend and cards under it
 *  (its blur makes it a layer of its own), so a menu opening from it lands on top of them. */
export function ToolGroup({ children, label }: { children: ReactNode; label: string }) {
  return (
    <div aria-label={label} role="toolbar" style={{ ...glass, ...S.row, position: 'relative', zIndex: 2, gap: 2, padding: 3, borderRadius: 18, pointerEvents: 'auto' }}>
      {children}
    </div>
  )
}

export function Rule() {
  return <span aria-hidden style={{ width: 1, height: 18, margin: '0 3px', background: T.border, flex: 'none' }} />
}

/** One way to lay the map out, as the layout menu lists it. */
export interface LayoutChoice {
  id: string
  name: string
  title: string
}

/** The layout menu: a tool button (naming the layout shown, unless it's the first), opening a menu of `choices`. A
 *  choice closes it, as do Escape and a press anywhere else; the arrow keys move between choices. */
export function LayoutMenu({ label, choices, current, onChoose }: { label: string; choices: LayoutChoice[]; current: string; onChoose: (id: string) => void }) {
  const [open, setOpen] = useState(false)
  const box = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    if (!open) return
    const away = (event: PointerEvent) => {
      if (!box.current?.contains(event.target as Node)) setOpen(false)
    }
    window.addEventListener('pointerdown', away, true)
    return () => window.removeEventListener('pointerdown', away, true)
  }, [open])
  const shown = choices.find(choice => choice.id === current) ?? choices[0]
  const keys = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!open) return
    if (event.key === 'Escape') {
      event.stopPropagation()
      setOpen(false)
      return
    }
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
    event.preventDefault()
    const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')]
    const at = items.findIndex(item => item === event.target)
    items[(at + (event.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length]?.focus()
  }
  return (
    <div onKeyDown={keys} ref={box} style={{ position: 'relative', display: 'flex' }}>
      <ToolButton expanded={open} icon="layers" label={label} onClick={() => setOpen(!open)} pressed={open || shown !== choices[0]} title={`${label}: ${shown?.name ?? ''}`}>
        {shown !== choices[0] ? shown?.name : undefined}
      </ToolButton>
      {open && (
        <div aria-label={label} className="kumi-pop" role="menu" style={{ ...glass, position: 'absolute', top: 38, right: -4, width: 'max-content', maxWidth: 300, padding: 4, display: 'flex', flexDirection: 'column', gap: 1, zIndex: 3 }}>
          {choices.map(choice => (
            <button
              aria-checked={choice.id === shown?.id}
              className={FOCUSABLE}
              key={choice.id}
              onClick={() => {
                onChoose(choice.id)
                setOpen(false)
              }}
              ref={choice.id === shown?.id ? element => element?.focus() : undefined}
              role="menuitemradio"
              type="button"
              style={{
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'flex-start',
                gap: 1,
                font: 'inherit',
                textAlign: 'left',
                padding: '6px 10px',
                borderRadius: 8,
                border: 'none',
                cursor: 'pointer',
                color: T.text,
                background: choice.id === shown?.id ? `color-mix(in srgb, ${T.accent} 16%, transparent)` : 'transparent'
              }}
            >
              <span style={{ fontSize: 12.5, fontWeight: 600, color: choice.id === shown?.id ? T.accent : T.text }}>{choice.name}</span>
              <span style={{ fontSize: 11, color: T.muted }}>{choice.title}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
