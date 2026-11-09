// The palette dock under the sound map: the sounds the producer gathered to work with, as chips with a waveform in
// their group's colour. A chip plays on a click; × takes it out; a drag (or Alt with ← or →) moves it. The header
// names the palette, switches, renames, makes or forgets one, sets it strict or loose, and offers it as a kit or as
// the sounds to make a new one from. Every edit changes no project, so it runs at once (`POST /palette`); the agent's
// `kumi_palette` reads and edits the same palette. Both UI halves use it: what "Use as kit" does comes from `kit`, and
// "Make…" from `make`.
import { T } from '@kumi/theme'
import type { DragEvent, KeyboardEvent, ReactNode } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'

import { clean, num } from './format.ts'
import { groupColor } from './mapModel.ts'
import { LOOSE, moved, paletteName, type PaletteAction, type PaletteItem, type PaletteReply, type PaletteView, peaksOf, playableId } from './palette.ts'
import type { MapGroup } from './types.ts'
import { FOCUSABLE, S, TextButton, useT } from './ui.tsx'

/** Chips drawn at once: a palette can hold thousands (a gather takes 10,000), and the dock is a strip, not a list. */
export const CHIPS_SHOWN = 240
/** Thumbnail bars per chip, and the largest audio (in data-URL characters) a thumbnail decodes. */
const BARS = 28
const THUMB_MOST = 12 * 1024 * 1024

export interface PaletteDockProps {
  palette: PaletteReply | null
  /** The map's groups, for each chip's colour. */
  groups: Record<string, MapGroup> | undefined
  /** What plays now (a sound id or an artifact id). */
  playing: string | null
  onPlay: (id: string) => void
  onStop: () => void
  /** Run an edit (the store's `paletteDo`); true when it was made. */
  onDo: (action: PaletteAction) => Promise<boolean>
  /** What the last edit said, or why it was refused (the store's `paletteSaid`): shown a few seconds. */
  said: { text: string; failed: boolean; at: number } | null
  /** The audio behind an id, as a data URL (the audition's cache), for the thumbnails. */
  audio: (id: string) => Promise<string>
  /** "Use as kit": Desktop drafts a chat request, the dashboard copies it. */
  kit?: (palette: PaletteView) => ReactNode
  /** "Make…": opens the Make panel on the palette's sounds. */
  make?: (palette: PaletteView) => void
}

// Thumbnails by id, shared by every dock in the window: decoded once, one at a time, kept for the session.
const peaksKept = new Map<string, number[] | null>()
const peaksLoading = new Map<string, Promise<number[] | null>>()
let decodeQueue: Promise<unknown> = Promise.resolve()

function decodePeaks(id: string, audio: (id: string) => Promise<string>): Promise<number[] | null> {
  const kept = peaksKept.get(id)
  if (kept !== undefined) return Promise.resolve(kept)
  const loading = peaksLoading.get(id)
  if (loading) return loading
  const run = async (): Promise<number[] | null> => {
    const url = await audio(id)
    const comma = url.indexOf(',')
    if (comma < 0 || url.length > THUMB_MOST || typeof OfflineAudioContext !== 'function') return null
    const binary = atob(url.slice(comma + 1))
    const bytes = new Uint8Array(binary.length)
    for (let at = 0; at < binary.length; at++) bytes[at] = binary.charCodeAt(at)
    const buffer = await new OfflineAudioContext(1, 1, 44_100).decodeAudioData(bytes.buffer)
    const mixed = new Float32Array(buffer.length)
    for (let channel = 0; channel < buffer.numberOfChannels; channel++) {
      const data = buffer.getChannelData(channel)
      for (let at = 0; at < data.length; at++) mixed[at] += data[at]
    }
    return peaksOf(mixed, BARS)
  }
  // One decode at a time: a palette of hundreds mustn't decode them all at once.
  const promise = decodeQueue
    .then(run, run)
    .catch(() => null)
    .then(peaks => {
      peaksKept.set(id, peaks)
      peaksLoading.delete(id)
      return peaks
    })
  decodeQueue = promise
  peaksLoading.set(id, promise)
  return promise
}

function Waveform({ id, color, audio }: { id: string | null; color: string; audio: (id: string) => Promise<string> }) {
  const canvas = useRef<HTMLCanvasElement | null>(null)
  const [peaks, setPeaks] = useState<number[] | null>(() => (id ? (peaksKept.get(id) ?? null) : null))
  // Decoded once it's on screen: a chip scrolled out of view costs nothing.
  useEffect(() => {
    const element = canvas.current
    if (!id || !element || peaks) return
    let live = true
    const observer = new IntersectionObserver(entries => {
      if (!entries.some(entry => entry.isIntersecting)) return
      observer.disconnect()
      void decodePeaks(id, audio).then(found => live && setPeaks(found))
    })
    observer.observe(element)
    return () => {
      live = false
      observer.disconnect()
    }
  }, [id, audio, peaks])
  useEffect(() => {
    const element = canvas.current
    const context = element?.getContext('2d')
    if (!element || !context) return
    const ratio = window.devicePixelRatio || 1
    const [width, height] = [44, 16]
    element.width = width * ratio
    element.height = height * ratio
    context.setTransform(ratio, 0, 0, ratio, 0, 0)
    context.clearRect(0, 0, width, height)
    context.fillStyle = color
    const bars = peaks ?? new Array<number>(BARS).fill(0.08)
    const step = width / bars.length
    bars.forEach((peak, bar) => {
      const tall = Math.max(1, peak * (height - 2))
      context.globalAlpha = peaks ? 0.95 : 0.35
      context.fillRect(bar * step, (height - tall) / 2, Math.max(1, step - 0.6), tall)
    })
  }, [peaks, color])
  return <canvas aria-hidden ref={canvas} style={{ width: 44, height: 16, flex: 'none' }} />
}

function Chip({
  item,
  index,
  color,
  playing,
  audio,
  onPlay,
  onStop,
  onRemove,
  onMove,
  onDropHere,
  dragging,
  onDrag,
  arriving
}: {
  item: PaletteItem
  index: number
  color: string
  playing: boolean
  audio: (id: string) => Promise<string>
  onPlay: () => void
  onStop: () => void
  onRemove: () => void
  onMove: (to: number) => void
  /** Another chip was dropped on this one: it takes this one's place. */
  onDropHere: () => void
  dragging: string | null
  onDrag: (key: string | null) => void
  /** Just gathered: it pops in, after the flight from the map lands (its place in the batch sets the delay). */
  arriving: number | null
}) {
  const t = useT()
  const id = playableId(item)
  const name = clean(item.name, 60) || item.key
  const [over, setOver] = useState(false)
  const keys = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.altKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
      event.preventDefault()
      onMove(index + (event.key === 'ArrowLeft' ? -1 : 1))
    } else if (event.key === 'Delete') {
      event.preventDefault()
      onRemove()
    }
  }
  return (
    <div
      draggable
      onDragEnd={() => onDrag(null)}
      onDragLeave={() => setOver(false)}
      onDragOver={(event: DragEvent) => {
        if (!dragging || dragging === item.key) return
        event.preventDefault()
        setOver(true)
      }}
      onDragStart={(event: DragEvent) => {
        event.dataTransfer.effectAllowed = 'move'
        event.dataTransfer.setData('text/plain', name)
        onDrag(item.key)
      }}
      onDrop={(event: DragEvent) => {
        event.preventDefault()
        setOver(false)
        if (dragging && dragging !== item.key) onDropHere()
      }}
      className={arriving === null ? undefined : 'kumi-pop'}
      role="listitem"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 2,
        maxWidth: 220,
        padding: '2px 2px 2px 4px',
        borderRadius: 6,
        border: `1px solid ${playing ? color : over ? T.accent : T.border}`,
        boxShadow: playing ? `0 0 0 1px ${color}, 0 0 10px -2px ${color}` : 'none',
        background: playing ? T.chip : 'transparent',
        opacity: dragging === item.key ? 0.4 : id ? 1 : 0.55,
        transition: 'border-color 120ms, box-shadow 160ms, opacity 120ms',
        cursor: 'grab',
        animationDelay: arriving === null ? undefined : `${380 + Math.min(arriving, 24) * 22}ms`
      }}
    >
      <button
        aria-label={playing ? t('dock.stop', name) : t('dock.play', name)}
        aria-pressed={playing}
        className={FOCUSABLE}
        disabled={!id}
        onClick={() => (playing ? onStop() : onPlay())}
        onKeyDown={keys}
        title={id ? `${name}${item.class ? ` · ${clean(item.class, 16)}` : ''}\n${t('dock.reorderHint')}` : t('dock.offMap')}
        type="button"
        style={{ ...S.row, gap: 5, minWidth: 0, font: 'inherit', fontSize: 11.5, padding: '1px 3px', border: 'none', background: 'transparent', color: T.text, cursor: id ? 'pointer' : 'default' }}
      >
        <Waveform audio={audio} color={color} id={id} />
        <span style={S.ellipsis}>{name}</span>
      </button>
      <button
        aria-label={t('dock.remove', name)}
        className={FOCUSABLE}
        onClick={onRemove}
        title={t('dock.remove', name)}
        type="button"
        style={{ font: 'inherit', fontSize: 12, lineHeight: '14px', width: 16, height: 16, padding: 0, borderRadius: 3, border: 'none', background: 'transparent', color: T.faint, cursor: 'pointer', flex: 'none' }}
      >
        ×
      </button>
    </div>
  )
}

/** A one-line text field that commits on Enter or when it loses focus, and gives up on Escape. */
function NameField({ initial, label, onDone }: { initial: string; label: string; onDone: (name: string | null) => void }) {
  const [text, setText] = useState(initial)
  const field = useRef<HTMLInputElement | null>(null)
  const done = useRef(false)
  useEffect(() => {
    field.current?.focus()
    field.current?.select()
  }, [])
  const finish = (name: string | null) => {
    if (done.current) return
    done.current = true
    onDone(name)
  }
  return (
    <input
      aria-label={label}
      className={FOCUSABLE}
      maxLength={120}
      onBlur={() => finish(paletteName(text))}
      onChange={event => setText(event.target.value)}
      onKeyDown={event => {
        if (event.key === 'Enter') {
          event.preventDefault()
          finish(paletteName(text))
        } else if (event.key === 'Escape') {
          event.preventDefault()
          event.stopPropagation()
          finish(null)
        }
      }}
      placeholder={label}
      ref={field}
      style={{ font: 'inherit', fontSize: 12, fontWeight: 600, width: '12rem', padding: '1px 6px', borderRadius: 3, border: `1px solid ${T.accent}`, background: 'transparent', color: T.text }}
      value={text}
    />
  )
}

export function PaletteDock({ palette, groups, playing, onPlay, onStop, onDo, said, audio, kit, make }: PaletteDockProps) {
  const t = useT()
  const current = palette?.current ?? null
  const [editing, setEditing] = useState<'new' | 'rename' | null>(null)
  const [forgetting, setForgetting] = useState(false)
  const [open, setOpen] = useState(true)
  const [dragging, setDragging] = useState<string | null>(null)
  // A move shows at once; the backend's answer then replaces it.
  const [order, setOrder] = useState<PaletteItem[] | null>(null)
  useEffect(() => setOrder(null), [palette])
  // Which chips were there before the last answer: one that wasn't is new (a gather), and pops in. The first answer
  // shows the palette as it is, with nothing popping.
  const known = useRef<{ id: string | null; keys: Set<string> } | null>(null)
  const arrived = useMemo(() => {
    const keys = new Set((current?.items ?? []).map(item => item.key))
    const before = known.current
    const fresh = before && before.id === current?.id ? (current?.items ?? []).filter(item => !before.keys.has(item.key)).map(item => item.key) : []
    known.current = { id: current?.id ?? null, keys }
    return new Map(fresh.map((key, order) => [key, order]))
  }, [current])
  // The last sentence fades after a while: it says what just happened, not what is.
  const [now, setNow] = useState(() => Date.now())
  const shownFor = said?.failed ? 8000 : 4000
  useEffect(() => {
    if (!said) return
    setNow(Date.now())
    const timer = setTimeout(() => setNow(Date.now()), Math.max(0, said.at + shownFor - Date.now()))
    return () => clearTimeout(timer)
  }, [said, shownFor])
  const saying = said && now - said.at < shownFor ? said : null

  const run = (action: PaletteAction) =>
    onDo(action).then(done => {
      if (!done) setOrder(null)
    })
  const items = order ?? current?.items ?? []
  const colorOf = (group: number | null) => (group === null ? T.faint : groupColor(groups?.[String(group)]?.color, group))
  const move = (key: string, to: number) => {
    if (to < 0 || to >= items.length) return
    setOrder(moved(items, key, to))
    void run({ action: 'move', key, to })
  }

  const header = (
    <div style={{ ...S.row, flexWrap: 'wrap', gap: 6 }}>
      <button
        aria-expanded={open}
        className={FOCUSABLE}
        onClick={() => setOpen(!open)}
        title={open ? t('dock.collapse') : t('dock.expand')}
        type="button"
        style={{ ...S.heading, font: 'inherit', fontSize: 11, padding: 0, border: 'none', background: 'transparent', cursor: 'pointer' }}
      >
        <span aria-hidden style={{ display: 'inline-block', transform: open ? 'rotate(90deg)' : 'none', transition: 'transform 120ms' }}>
          ▸
        </span>
        {t('dock.title')}
      </button>
      {editing ? (
        <NameField
          initial={editing === 'rename' ? (current?.name ?? '') : ''}
          label={editing === 'rename' ? t('dock.renameTitle') : t('dock.newName')}
          onDone={name => {
            const action = editing
            setEditing(null)
            if (name && action === 'rename' && name !== current?.name) void run({ action: 'rename', name })
            if (name && action === 'new') void run({ action: 'new', name })
          }}
        />
      ) : (
        <>
          {palette && palette.palettes.length > 0 && (
            <select
              aria-label={t('dock.switch')}
              className={FOCUSABLE}
              onChange={event => {
                const value = event.target.value
                if (value === '\u0000new') setEditing('new')
                else if (value !== current?.id) void run({ action: 'use', id: value })
              }}
              style={{ font: 'inherit', fontSize: 12, fontWeight: 600, maxWidth: '14rem', padding: '0 4px', borderRadius: 3, border: `1px solid ${T.border}`, background: 'transparent', color: T.text }}
              value={current?.id ?? ''}
            >
              {!current && <option value="">{t('dock.choose')}</option>}
              {palette.palettes.map(entry => (
                <option key={entry.id} value={entry.id}>
                  {`${clean(entry.name, 40)} (${num(entry.items, 0)})`}
                </option>
              ))}
              <option value={'\u0000new'}>{t('dock.newPalette')}</option>
            </select>
          )}
          {(!palette || palette.palettes.length === 0) && <span style={S.faint}>{t('dock.none')}</span>}
          {current && (
            <TextButton onClick={() => setEditing('rename')} title={t('dock.renameTitle')}>
              {t('dock.rename')}
            </TextButton>
          )}
          {palette && palette.palettes.length === 0 && (
            <TextButton onClick={() => setEditing('new')} title={t('dock.newName')}>
              {t('dock.newPalette')}
            </TextButton>
          )}
        </>
      )}
      {current && (
        <span role="group" aria-label={t('dock.mode')} style={{ ...S.row, gap: 0 }}>
          <TextButton onClick={() => current.reach !== 0 && void run({ action: 'reach', reach: 0 })} pressed={current.reach === 0} title={t('dock.strictTitle')}>
            {t('dock.strict')}
          </TextButton>
          <TextButton onClick={() => current.reach === 0 && void run({ action: 'reach', reach: LOOSE })} pressed={current.reach > 0} title={t('dock.looseTitle', current.reach || LOOSE)}>
            {current.reach > 0 ? t('dock.looseN', current.reach) : t('dock.loose')}
          </TextButton>
        </span>
      )}
      {current && (
        <span style={S.faint}>
          {t('dock.count', current.items.length)}
          {current.excluded > 0 ? ` · ${t('dock.excluded', current.excluded)}` : ''}
        </span>
      )}
      <span style={{ flex: 1 }} />
      {current && current.items.length > 1 && make && (
        <TextButton onClick={() => make(current)} title={t('make.openTitle')}>
          {t('make.open')}
        </TextButton>
      )}
      {current && current.items.length > 0 && kit?.(current)}
      {current && !forgetting && (
        <TextButton onClick={() => setForgetting(true)} title={t('dock.forgetTitle', clean(current.name, 40))}>
          {t('dock.forget')}
        </TextButton>
      )}
    </div>
  )

  return (
    <section aria-label={t('dock.title')} style={{ display: 'flex', flexDirection: 'column', gap: 6, padding: '8px 10px', borderTop: `1px solid ${T.border}`, color: T.text }}>
      {header}
      {current && forgetting && (
        <div role="alert" style={{ ...S.row, flexWrap: 'wrap', gap: 6 }}>
          <span style={S.muted}>{t('dock.forgetAsk', clean(current.name, 40), current.items.length)}</span>
          <TextButton
            onClick={() => {
              setForgetting(false)
              void run({ action: 'forget', id: current.id })
            }}
            strong
          >
            {t('dock.forgetYes')}
          </TextButton>
          <TextButton onClick={() => setForgetting(false)}>{t('dock.keep')}</TextButton>
        </div>
      )}
      {open && current && items.length > 0 && (
        <div aria-label={t('dock.sounds')} role="list" style={{ display: 'flex', flexWrap: 'wrap', gap: 5, maxHeight: 116, overflowY: 'auto', paddingBottom: 1 }}>
          {items.slice(0, CHIPS_SHOWN).map((item, index) => (
            <Chip
              audio={audio}
              color={colorOf(item.group)}
              dragging={dragging}
              index={index}
              item={item}
              key={item.key}
              arriving={arrived.get(item.key) ?? null}
              onDrag={setDragging}
              onDropHere={() => dragging && move(dragging, index)}
              onMove={to => move(item.key, to)}
              onPlay={() => {
                const id = playableId(item)
                if (id) onPlay(id)
              }}
              onRemove={() => {
                setOrder(items.filter(other => other.key !== item.key))
                void run({ action: 'remove', keys: [item.key] })
              }}
              onStop={onStop}
              playing={playing !== null && playing === playableId(item)}
            />
          ))}
          {items.length > CHIPS_SHOWN && <span style={{ ...S.faint, alignSelf: 'center' }}>{t('dock.more', items.length - CHIPS_SHOWN)}</span>}
        </div>
      )}
      {open && (!current || items.length === 0) && <span style={S.faint}>{t('dock.empty')}</span>}
      <span aria-live="polite" style={saying ? { ...S.faint, color: saying.failed ? T.warn : T.muted } : S.hidden}>
        {saying ? (saying.failed ? t('dock.failed', saying.text) : saying.text) : ''}
      </span>
    </section>
  )
}
