// Small pieces both UI halves draw with: they take every colour from the host's theme (`@kumi/theme`), so the same
// component reads as native in Hermes Desktop and in the web dashboard. Their words come from the translator a UI
// root provides (`I18n`): Desktop's for its language, the dashboard's for its locale, English outside any.
import { T } from '@kumi/theme'
import type { CSSProperties, KeyboardEvent, MouseEvent, ReactNode, RefObject } from 'react'
import { createContext, useContext, useLayoutEffect, useRef, useState } from 'react'

import { english, type Translate } from './i18n.ts'

export const I18n = createContext<Translate>(english)

export function useT(): Translate {
  return useContext(I18n)
}

/** The type scale the design pass draws with (T2-DESIGN): body for what a row says, small for meta lines, hints and
 *  buttons (Hermes's own button text is 12 px), caption for section headings, times and chips; weights 400 and 600. */
export const TYPE = {
  body: { fontSize: 13, lineHeight: '18px' },
  small: { fontSize: 12, lineHeight: '16px' },
  caption: { fontSize: 11, lineHeight: '14px' }
} as const

export const S = {
  section: { display: 'flex', flexDirection: 'column', gap: 8, padding: 12, borderBottom: `1px solid ${T.hairline}` },
  heading: { display: 'flex', alignItems: 'center', gap: 6, ...TYPE.caption, fontWeight: 600, letterSpacing: '0.06em', textTransform: 'uppercase', color: T.muted },
  row: { display: 'flex', alignItems: 'center', gap: 6, minWidth: 0 },
  // A message can hold a URL or a path with no place to break: it wraps anywhere rather than widen the pane.
  text: { ...TYPE.body, color: T.text, overflowWrap: 'anywhere' },
  muted: { ...TYPE.small, color: T.muted, overflowWrap: 'anywhere' },
  faint: { ...TYPE.caption, color: T.faint },
  ellipsis: { overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', minWidth: 0 },
  // Read by a screen reader, not drawn: hints and live announcements.
  hidden: { position: 'absolute', width: 1, height: 1, margin: -1, padding: 0, border: 0, overflow: 'hidden', clip: 'rect(0 0 0 0)', whiteSpace: 'nowrap' }
} satisfies Record<string, CSSProperties>

/** The class every Kumi control carries, so the keyboard's focus shows on it. */
export const FOCUSABLE = 'kumi-focusable'
/** The class Kumi puts on the host's own buttons it draws (`@kumi/controls`), so its ring can sit on the host's. */
export const HOST_CONTROL = 'kumi-host'

/** The focus ring for Kumi's controls, in the host theme's colour. Desktop turns focus rings off for every element
 *  (`*:focus-visible { outline: none }`); this rule's class beats that one, and only a keyboard focus shows it. Each UI
 *  root renders it once. With it come the few animations Kumi's views draw in CSS (a box's marching edge, a palette
 *  chip or a new sound popping in, the spinner while one is made), which a system asking for less motion doesn't get
 *  (the spinner turns slowly instead, so it still shows that something is happening). */
export function FocusStyle() {
  return (
    <style>
      {`.${FOCUSABLE}:focus-visible{outline:2px solid ${T.focus};outline-offset:2px;border-radius:3px}` +
        // On Hermes's own buttons (`HOST_CONTROL`, Kumi's class) the ring sits right on the host's own, so v0.23 shows one
        // ring, and v0.22.7, which turns every ring off, still shows Kumi's. Nothing of Hermes's is restyled.
        `.${FOCUSABLE}.${HOST_CONTROL}:focus-visible{outline-offset:0}` +
        // Where Kumi draws a button itself (the dashboard's), hover and press tint whatever its fill is, at once.
        '.kumi-btn:hover:not(:disabled){background-image:linear-gradient(rgba(127,127,127,.14),rgba(127,127,127,.14))}' +
        '.kumi-btn:active:not(:disabled){background-image:linear-gradient(rgba(127,127,127,.24),rgba(127,127,127,.24))}' +
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
        // A job that can't say how far it is: a segment slides along its track.
        '@keyframes kumi-slide{from{transform:translateX(-100%)}to{transform:translateX(340%)}}.kumi-slide{animation:kumi-slide 1.6s ease-in-out infinite}' +
        '@media (prefers-reduced-motion:reduce){.kumi-ants,.kumi-pop,.kumi-vine,.kumi-open,.kumi-beat,.kumi-swell,.kumi-slide{animation:none}.kumi-breathe{animation-duration:3s}.kumi-spin{animation-duration:2.4s}}'}
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

/** Hermes's segmented look, for a choice among a few (Make's ways and modes, Strict | Loose): a tinted track, the
 *  chosen one raised in the theme's own colours. */
export const SEGMENT_TRACK = { display: 'flex', flexWrap: 'wrap', gap: 2, padding: 2, borderRadius: 5, background: T.chip } as const
export function segmentStyle(picked: boolean, disabled = false): CSSProperties {
  return {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 6,
    minWidth: 0,
    font: 'inherit',
    ...TYPE.small,
    fontWeight: picked ? 600 : 400,
    padding: '4px 8px',
    border: 'none',
    borderRadius: 3,
    background: picked ? T.raised : 'transparent',
    boxShadow: picked ? '0 1px 2px rgba(0,0,0,.18)' : 'none',
    color: disabled ? T.faint : picked ? T.text : T.muted,
    cursor: disabled ? 'default' : 'pointer',
    transition: 'background-color 100ms, color 100ms'
  }
}

/** Fade `element`'s top and bottom edges while there's more to scroll that way, so a row cut at an edge reads as
 *  scrolling, not as a fault. It sets the element's own mask (no re-render): on scroll, when it or one of its rows
 *  changes size, and at each `apply` (`useEdgeFade` calls it after every render of the scroller's owner). Never a
 *  MutationObserver: rule 8 keeps the Desktop file free of them. */
function fadeEdges(element: HTMLElement): { apply: () => void; stop: () => void } {
  const rows = new Set<Element>()
  const resized = new ResizeObserver(() => apply())
  const apply = () => {
    // Each row is watched once (observing one again would report it anew and loop), and let go once it's gone.
    const now = new Set(element.children)
    for (const row of rows) {
      if (now.has(row)) continue
      resized.unobserve(row)
      rows.delete(row)
    }
    for (const row of now) {
      if (rows.has(row)) continue
      rows.add(row)
      resized.observe(row)
    }
    const top = element.scrollTop > 1
    const bottom = element.scrollTop + element.clientHeight < element.scrollHeight - 1
    const mask = top || bottom ? `linear-gradient(to bottom, ${top ? 'transparent' : '#000'}, #000 ${top ? 16 : 0}px, #000 calc(100% - ${bottom ? 16 : 0}px), ${bottom ? 'transparent' : '#000'})` : ''
    element.style.setProperty('mask-image', mask)
    element.style.setProperty('-webkit-mask-image', mask)
  }
  apply()
  element.addEventListener('scroll', apply, { passive: true })
  resized.observe(element)
  return {
    apply,
    stop: () => {
      element.removeEventListener('scroll', apply)
      resized.disconnect()
      element.style.removeProperty('mask-image')
      element.style.removeProperty('-webkit-mask-image')
    }
  }
}

/** Every scroller of Kumi's fades at the edge it can still scroll to (`fadeEdges`), while `on`. After each render of
 *  its owner it binds to the scroller shown, or measures the same one again (its rows may have changed). */
export function useEdgeFade(ref: RefObject<HTMLElement | null>, on = true): void {
  const bound = useRef<{ element: HTMLElement; apply: () => void; stop: () => void } | null>(null)
  useLayoutEffect(() => {
    const element = on ? ref.current : null
    if (bound.current && bound.current.element === element) {
      bound.current.apply()
      return
    }
    bound.current?.stop()
    bound.current = element ? { element, ...fadeEdges(element) } : null
  })
  useLayoutEffect(
    () => () => {
      bound.current?.stop()
      bound.current = null
    },
    []
  )
}

/** A click that may put a request in the chat: an `onClick` that answers with a promise makes the button a request's.
 *  It holds, busy, until the promise settles, and a click meanwhile, or the second click of a double click, sends
 *  nothing more. Kumi's own button and Hermes's (through `@kumi/controls`) both click through it. */
export function useRequestClick(onClick: () => unknown): { busy: boolean; click: (event: MouseEvent<HTMLElement>) => void } {
  const [busy, setBusy] = useState(false)
  // Whether this button's clicks have been requests: from then on a double click's second click is the same request.
  const asks = useRef(false)
  const click = (event: MouseEvent<HTMLElement>) => {
    if (busy || (asks.current && event.detail > 1)) return
    const answer = onClick()
    if (!answer || typeof (answer as PromiseLike<unknown>).then !== 'function') return
    asks.current = true
    setBusy(true)
    void Promise.resolve(answer)
      .catch(() => undefined)
      .finally(() => setBusy(false))
  }
  return { busy, click }
}

/** A plain text button in the theme's colours. `label` names it for a screen reader when its face is a symbol (▶, ■).
 *  A request's button holds while it goes and never sends twice (`useRequestClick`). */
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
  onClick: () => unknown
  title?: string
  label?: string
  pressed?: boolean
  disabled?: boolean
  strong?: boolean
}) {
  const { busy, click } = useRequestClick(onClick)
  return (
    <button
      aria-busy={busy || undefined}
      aria-label={label}
      aria-pressed={pressed}
      className={FOCUSABLE}
      disabled={disabled}
      onClick={click}
      title={title}
      type="button"
      style={{
        // The face is a row: an icon sits beside its word whatever the host's CSS does with an `svg` (Hermes's base
        // styles make every `svg` a block, which stacked "Surprise"'s die above its word).
        display: 'inline-flex',
        alignItems: 'center',
        gap: 4,
        font: 'inherit',
        fontSize: 11.5,
        lineHeight: '18px',
        padding: '0 7px',
        borderRadius: 3,
        // A toggle that's on reads as on, not only to a screen reader (hover to play, Select, Strict or Loose).
        border: `1px solid ${strong || pressed ? T.accent : T.border}`,
        background: pressed ? `color-mix(in srgb, ${T.accent} 14%, transparent)` : 'transparent',
        color: disabled ? T.faint : strong || pressed ? T.accent : T.secondary,
        cursor: disabled ? 'default' : busy ? 'progress' : 'pointer',
        opacity: busy ? 0.6 : 1,
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

/** What a view shows while its first read is on its way: a quiet line where its words will be, never words that say
 *  there's nothing ("No changes yet" before HISTORY has been read is untrue). `label` is what a screen reader hears. */
export function Pending({ label, width = '60%' }: { label: string; width?: number | string }) {
  return <span aria-busy="true" aria-label={label} role="status" style={{ display: 'block', width, maxWidth: '100%', height: 10, margin: '4px 0', borderRadius: 3, background: T.chip, opacity: 0.7 }} />
}

/** Text that wraps to at most `lines` lines and, when that cuts it, shows the rest on a click or Enter (and folds it
 *  again the same way): nothing is cut without a way to read it, and a pointer is never the only way. Whether it's cut
 *  is measured, so a short text is plain text. `title` stays for the full words on hover. */
export function Clamp({ text, lines = 2, style, title }: { text: string; lines?: number; style?: CSSProperties; title?: string }) {
  const box = useRef<HTMLSpanElement | null>(null)
  const [open, setOpen] = useState(false)
  const [cut, setCut] = useState(false)
  useLayoutEffect(() => {
    const element = box.current
    if (!element || open) return
    const measure = () => setCut(element.scrollHeight > element.clientHeight + 1)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    return () => observer.disconnect()
  }, [text, open, lines])
  const folded: CSSProperties = open ? {} : { display: '-webkit-box', WebkitLineClamp: lines, WebkitBoxOrient: 'vertical', overflow: 'hidden' }
  if (!cut && !open) {
    return (
      <span ref={box} style={{ ...style, ...folded }} title={title}>
        {text}
      </span>
    )
  }
  return (
    <span
      aria-expanded={open}
      className={FOCUSABLE}
      onClick={(event: MouseEvent) => {
        event.stopPropagation()
        setOpen(!open)
      }}
      onKeyDown={(event: KeyboardEvent) => {
        if (event.key !== 'Enter' && event.key !== ' ') return
        event.preventDefault()
        setOpen(!open)
      }}
      ref={box}
      role="button"
      style={{ ...style, ...folded, cursor: 'pointer' }}
      tabIndex={0}
      title={title}
    >
      {text}
    </span>
  )
}

/** A radio group's keyboard, as a native one has (WAI-ARIA's radio group): the group is one Tab stop, its chosen radio
 *  (the first while none is), and the arrow keys move to the next or previous radio and choose it, wrapping at the
 *  ends. Spread on radio `index` of `count`, `chosen` being the chosen one's index (-1: none). Focus moves through the
 *  group's own children, which must be its radios in order. */
export function radioKeys(index: number, count: number, chosen: number, choose: (index: number) => void): { tabIndex: number; onKeyDown: (event: KeyboardEvent<HTMLElement>) => void } {
  return {
    tabIndex: index === Math.max(0, chosen) ? 0 : -1,
    onKeyDown: event => {
      const step = event.key === 'ArrowRight' || event.key === 'ArrowDown' ? 1 : event.key === 'ArrowLeft' || event.key === 'ArrowUp' ? -1 : 0
      if (!step || count < 2 || event.altKey || event.ctrlKey || event.metaKey) return
      event.preventDefault()
      const next = (index + step + count) % count
      choose(next)
      const radio = event.currentTarget.parentElement?.children[next]
      if (radio instanceof HTMLElement) radio.focus()
    }
  }
}
