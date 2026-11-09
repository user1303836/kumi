// The Make panel's controls, drawn by Kumi so they look and behave alike in Desktop and the dashboard: a slider (one
// value or a `[low, high]` range; a bipolar one fills from its middle and names its ends), choice pills, toggle chips,
// and a group that folds. A control left unset rests: an option is "Auto" (the seed draws it, so Reroll varies it), a
// steer control is at its neutral (it changes nothing). A double click, Delete or Backspace puts a slider back to rest.
import { T } from '@kumi/theme'
import type { KeyboardEvent, PointerEvent, ReactNode } from 'react'
import { useRef, useState } from 'react'

import type { Span } from './design.ts'
import { FOCUSABLE, S } from './ui.tsx'

export type SliderValue = number | [number, number]

const clamp = (span: Span, value: number): number => Math.min(span.max, Math.max(span.min, value))
const snap = (span: Span, value: number): number => {
  const steps = Math.round((clamp(span, value) - span.min) / span.step)
  // Rounded to the step's own decimals, so 0.1 + 0.2 never shows as 0.30000000000000004.
  const decimals = Math.max(0, -Math.floor(Math.log10(span.step)))
  return Number((span.min + steps * span.step).toFixed(decimals + 1))
}
const fraction = (span: Span, value: number): number => (clamp(span, value) - span.min) / (span.max - span.min)

/** A slider in Kumi's look: a thin track filled in the accent, round thumbs, its label and value above, and for a
 *  bipolar control the words at each end below. `range` takes `[low, high]` with a thumb at each end. */
export function Slider({
  label,
  span,
  value,
  onChange,
  format,
  rest,
  restAt,
  ends,
  range = false,
  title
}: {
  label: string
  span: Span
  value: SliderValue | undefined
  onChange: (value: SliderValue | undefined) => void
  /** The value as words ("−5…+3 st", "40%"). */
  format: (value: SliderValue) => string
  /** What shows while it rests: "Auto", or nothing for a steer control at its neutral. */
  rest: string
  /** Where the thumb rests while unset (the loop's tempo: the DAW's); the neutral, or the middle, otherwise. */
  restAt?: number
  /** The words at the track's two ends, for a control that runs between two characters ("Dark", "Bright"). */
  ends?: [string, string]
  range?: boolean
  title?: string
}) {
  const track = useRef<HTMLDivElement | null>(null)
  const [dragging, setDragging] = useState<number | null>(null)
  const middle = span.neutral ?? (range ? span.min : (span.min + span.max) / 2)
  const resting = restAt ?? middle
  const values: number[] = value === undefined ? (range ? [resting, resting] : [resting]) : Array.isArray(value) ? value : [value]
  const set = value !== undefined
  const from = (clientX: number): number => {
    const box = track.current?.getBoundingClientRect()
    if (!box || box.width === 0) return values[0]
    return snap(span, span.min + ((clientX - box.left) / box.width) * (span.max - span.min))
  }
  const commit = (thumb: number, next: number) => {
    if (!range) return onChange(next)
    const pair: [number, number] = thumb === 0 ? [Math.min(next, values[1]), values[1]] : [values[0], Math.max(next, values[0])]
    onChange(pair)
  }
  const press = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    const next = from(event.clientX)
    // A range moves the thumb nearer the pointer (the high one when they sit together and the press is past them).
    const thumb = range ? (Math.abs(next - values[0]) < Math.abs(next - values[1]) || (values[0] === values[1] && next < values[0]) ? 0 : 1) : 0
    event.currentTarget.setPointerCapture?.(event.pointerId)
    setDragging(thumb)
    commit(thumb, next)
  }
  const keys = (thumb: number) => (event: KeyboardEvent<HTMLDivElement>) => {
    const big = event.shiftKey ? 10 : 1
    const at = values[thumb]
    const moves: Record<string, number> = {
      ArrowRight: at + span.step * big,
      ArrowUp: at + span.step * big,
      ArrowLeft: at - span.step * big,
      ArrowDown: at - span.step * big,
      PageUp: at + span.step * 10,
      PageDown: at - span.step * 10,
      Home: span.min,
      End: span.max
    }
    if (event.key === 'Delete' || event.key === 'Backspace') {
      event.preventDefault()
      onChange(undefined)
    } else if (event.key in moves) {
      event.preventDefault()
      commit(thumb, snap(span, moves[event.key]))
    }
  }
  // The filled stretch: from the neutral (or the start) to the value, or between a range's thumbs.
  const [a, b] = range ? [fraction(span, values[0]), fraction(span, values[1])] : [fraction(span, Math.min(middle, values[0])), fraction(span, Math.max(middle, values[0]))]
  const fillFrom = range || span.neutral !== undefined ? a : 0
  const fillTo = range || span.neutral !== undefined ? b : fraction(span, values[0])
  const shown = set ? format(value) : rest
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 3, minWidth: 0 }} title={title}>
      <span style={{ ...S.row, gap: 6, fontSize: 11.5, color: set ? T.secondary : T.muted }}>
        <span style={{ ...S.ellipsis, flex: 1 }}>{label}</span>
        <button
          aria-label={set ? `${label}: ${shown}` : `${label}: ${rest || '—'}`}
          className={FOCUSABLE}
          disabled={!set}
          onClick={() => onChange(undefined)}
          type="button"
          style={{
            font: 'inherit',
            fontSize: 10.5,
            fontVariantNumeric: 'tabular-nums',
            padding: '0 6px',
            borderRadius: 8,
            border: `1px solid ${set ? `color-mix(in srgb, ${T.accent} 55%, transparent)` : 'transparent'}`,
            background: set ? `color-mix(in srgb, ${T.accent} 12%, transparent)` : 'transparent',
            color: set ? T.accent : T.faint,
            cursor: set ? 'pointer' : 'default',
            whiteSpace: 'nowrap',
            transition: 'background-color 140ms, border-color 140ms, color 140ms'
          }}
        >
          {shown || '·'}
        </button>
      </span>
      <div
        onDoubleClick={() => onChange(undefined)}
        onPointerCancel={() => setDragging(null)}
        onPointerDown={press}
        onPointerMove={event => {
          if (dragging !== null) commit(dragging, from(event.clientX))
        }}
        onPointerUp={() => setDragging(null)}
        ref={track}
        style={{ position: 'relative', height: 16, cursor: 'pointer', touchAction: 'none' }}
      >
        <span aria-hidden style={{ position: 'absolute', left: 0, right: 0, top: 6, height: 4, borderRadius: 2, background: T.border, opacity: set ? 0.9 : 0.55 }} />
        {span.neutral !== undefined && span.neutral > span.min && (
          <span aria-hidden style={{ position: 'absolute', left: `${fraction(span, span.neutral) * 100}%`, top: 3, width: 1, height: 10, background: T.faint, transform: 'translateX(-0.5px)' }} />
        )}
        {set && (
          <span
            aria-hidden
            style={{
              position: 'absolute',
              left: `${fillFrom * 100}%`,
              width: `${Math.max(0, fillTo - fillFrom) * 100}%`,
              top: 6,
              height: 4,
              borderRadius: 2,
              background: T.accent,
              boxShadow: `0 0 8px -2px ${T.accent}`,
              transition: dragging === null ? 'left 120ms, width 120ms' : 'none'
            }}
          />
        )}
        {values.map((at, thumb) => (
          <div
            aria-label={range ? `${label} ${thumb === 0 ? '↓' : '↑'}` : label}
            aria-valuemax={span.max}
            aria-valuemin={span.min}
            aria-valuenow={at}
            aria-valuetext={set ? shown : rest}
            className={FOCUSABLE}
            key={thumb}
            onKeyDown={keys(thumb)}
            role="slider"
            tabIndex={0}
            style={{
              position: 'absolute',
              left: `${fraction(span, at) * 100}%`,
              top: 2,
              width: 12,
              height: 12,
              borderRadius: '50%',
              transform: `translateX(-6px) scale(${dragging === thumb ? 1.25 : 1})`,
              background: T.raised,
              border: `2px solid ${set ? T.accent : T.faint}`,
              opacity: set ? 1 : 0.7,
              boxShadow: set ? `0 2px 6px -2px ${T.accent}` : 'none',
              transition: dragging === null ? 'left 120ms, transform 120ms, border-color 140ms' : 'transform 120ms'
            }}
          />
        ))}
      </div>
      {ends && (
        <span aria-hidden style={{ ...S.row, justifyContent: 'space-between', fontSize: 10, color: T.faint, marginTop: -2 }}>
          <span>{ends[0]}</span>
          <span>{ends[1]}</span>
        </span>
      )}
    </div>
  )
}

/** One choice of several, as pills: the first pill is the rest ("Auto"), unless `rest` is null (a choice that's always
 *  made, like a loop's style). */
export function Pills<V extends number | string>({
  label,
  options,
  value,
  onChange,
  rest
}: {
  label: string
  options: { value: V; label: string; title?: string; glyph?: ReactNode }[]
  value: V | undefined
  onChange: (value: V | undefined) => void
  rest: string | null
}) {
  const all: { value: V | undefined; label: string; title?: string; glyph?: ReactNode }[] = rest === null ? options : [{ value: undefined, label: rest }, ...options]
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4, minWidth: 0 }}>
      <span style={{ fontSize: 11.5, color: value === undefined ? T.muted : T.secondary }}>{label}</span>
      <div aria-label={label} role="radiogroup" style={{ ...S.row, flexWrap: 'wrap', gap: 4 }}>
        {all.map(option => {
          const chosen = option.value === value
          return (
            <button
              aria-checked={chosen}
              className={FOCUSABLE}
              key={String(option.value ?? '')}
              onClick={() => onChange(option.value)}
              role="radio"
              title={option.title}
              type="button"
              style={{
                ...S.row,
                gap: 4,
                font: 'inherit',
                fontSize: 11.5,
                fontWeight: chosen ? 600 : 500,
                padding: '2px 9px',
                borderRadius: 12,
                border: `1px solid ${chosen ? T.accent : T.border}`,
                background: chosen ? `color-mix(in srgb, ${T.accent} 16%, transparent)` : 'transparent',
                color: chosen ? T.accent : option.value === undefined ? T.muted : T.secondary,
                cursor: 'pointer',
                transition: 'border-color 140ms, background-color 140ms, color 140ms'
              }}
            >
              {option.glyph}
              {option.label}
            </button>
          )
        })}
      </div>
    </div>
  )
}

/** A long list of choices as a menu: the rest first. */
export function Choice<V extends string>({ label, options, value, onChange, rest }: { label: string; options: { value: V; label: string }[]; value: V | undefined; onChange: (value: V | undefined) => void; rest: string }) {
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 4, minWidth: 0, fontSize: 11.5, color: value === undefined ? T.muted : T.secondary }}>
      {label}
      <select
        className={FOCUSABLE}
        onChange={event => onChange(event.target.value === '' ? undefined : (event.target.value as V))}
        style={{ font: 'inherit', fontSize: 12, padding: '2px 6px', borderRadius: 6, border: `1px solid ${value === undefined ? T.border : T.accent}`, background: 'transparent', color: value === undefined ? T.secondary : T.accent }}
        value={value ?? ''}
      >
        <option value="">{rest}</option>
        {options.map(option => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
    </label>
  )
}

/** Any of several, as chips: none chosen rests (`rest`, "All, at the engine's odds"). */
export function Toggles<V extends string>({ label, options, value, onChange, rest }: { label: string; options: { value: V; label: string; title?: string }[]; value: V[] | undefined; onChange: (value: V[] | undefined) => void; rest: string }) {
  const chosen = value ?? []
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4, minWidth: 0 }}>
      <span style={{ ...S.row, gap: 6, fontSize: 11.5, color: chosen.length ? T.secondary : T.muted }}>
        {label}
        {chosen.length === 0 && <span style={{ color: T.faint, fontSize: 10.5 }}>{rest}</span>}
      </span>
      <div aria-label={label} role="group" style={{ ...S.row, flexWrap: 'wrap', gap: 4 }}>
        {options.map(option => {
          const on = chosen.includes(option.value)
          return (
            <button
              aria-pressed={on}
              className={FOCUSABLE}
              key={option.value}
              onClick={() => {
                const next = on ? chosen.filter(other => other !== option.value) : [...chosen, option.value]
                onChange(next.length ? next : undefined)
              }}
              title={option.title}
              type="button"
              style={{
                font: 'inherit',
                fontSize: 11.5,
                padding: '2px 9px',
                borderRadius: 6,
                border: `1px solid ${on ? T.accent : T.border}`,
                background: on ? `color-mix(in srgb, ${T.accent} 16%, transparent)` : 'transparent',
                color: on ? T.accent : T.secondary,
                cursor: 'pointer',
                transition: 'border-color 140ms, background-color 140ms, color 140ms'
              }}
            >
              {on ? '✓ ' : ''}
              {option.label}
            </button>
          )
        })}
      </div>
    </div>
  )
}

/** A section that folds: its title, how many of its controls are set, its own buttons, and its controls in columns as
 *  wide as the panel allows. */
export function Group({ title, count, summary, open, onToggle, actions, children, hint }: { title: string; count?: string; summary?: string; open: boolean; onToggle: () => void; actions?: ReactNode; children: ReactNode; hint?: string }) {
  return (
    <section style={{ display: 'flex', flexDirection: 'column', gap: 8, padding: '8px 10px', borderRadius: 10, border: `1px solid ${open ? `color-mix(in srgb, ${T.border} 100%, transparent)` : T.border}`, background: open ? `color-mix(in srgb, ${T.card} 60%, transparent)` : 'transparent', transition: 'background-color 160ms' }}>
      <div style={{ ...S.row, gap: 6 }}>
        <button
          aria-expanded={open}
          className={FOCUSABLE}
          onClick={onToggle}
          type="button"
          style={{ ...S.row, gap: 6, flex: 1, font: 'inherit', fontSize: 12, fontWeight: 650, padding: 0, border: 'none', background: 'transparent', color: T.text, cursor: 'pointer', textAlign: 'left' }}
        >
          <span aria-hidden style={{ display: 'inline-block', color: T.muted, transform: open ? 'rotate(90deg)' : 'none', transition: 'transform 160ms' }}>
            ▸
          </span>
          {title}
          {count && <span style={{ fontSize: 10.5, fontWeight: 500, padding: '0 6px', borderRadius: 8, background: `color-mix(in srgb, ${T.accent} 14%, transparent)`, color: T.accent }}>{count}</span>}
          {!open && summary && <span style={{ ...S.ellipsis, flex: 1, fontSize: 11, fontWeight: 400, color: T.muted }}>{summary}</span>}
        </button>
        {actions}
      </div>
      {open && hint && <span style={{ ...S.faint, fontSize: 11, marginTop: -4 }}>{hint}</span>}
      {open && <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(150px, 1fr))', gap: '10px 16px', alignItems: 'start' }}>{children}</div>}
    </section>
  )
}
