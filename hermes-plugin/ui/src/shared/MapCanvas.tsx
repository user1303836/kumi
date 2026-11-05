// The sound map, drawn on a canvas: each sound of the library where Kumi's layout put it, coloured by its group.
// Wheel zooms, a drag pans, a click picks the sound under the pointer. Search and the legend dim what they leave out;
// sounds the agent just found get a ring. What a search or a find picks out is also listed by name, for the keyboard.
// Both UI halves use it; what a picked sound can do comes from `actions`.
import { CANVAS, T } from '@kumi/theme'
import type { ReactNode } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'

import { clean, num, seconds } from './format.ts'
import { buildModel, fitView, frame, type MapViewport, pick, searchSounds, toScreen, zoomAt } from './mapModel.ts'
import type { KumiState } from './store.ts'
import type { MapSound, SoundMap } from './types.ts'
import { Chip, S, useT } from './ui.tsx'

export interface MapCanvasProps {
  map: SoundMap | null
  found: KumiState['found']
  playing: string | null
  selected: string | null
  onPick: (id: string) => void
  actions?: (id: string, sound: MapSound) => ReactNode
  empty: ReactNode
}

/** How many searched or found sounds are listed by name above the map; the rest stay rings on it. */
const MATCHES_LISTED = 6

interface Size {
  width: number
  height: number
}

function resolve(canvas: HTMLCanvasElement): Record<keyof typeof CANVAS, string> {
  const style = getComputedStyle(canvas)
  const read = (name: string, fallback: string): string => style.getPropertyValue(name).trim() || fallback
  return {
    text: read(CANVAS.text, '#999'),
    faint: read(CANVAS.faint, '#666'),
    border: read(CANVAS.border, '#444'),
    accent: read(CANVAS.accent, '#4a90e2'),
    strong: read(CANVAS.strong, '#ddd')
  }
}

export function MapCanvas({ map, found, playing, selected, onPick, actions, empty }: MapCanvasProps) {
  const t = useT()
  const model = useMemo(() => buildModel(map), [map])
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const boxRef = useRef<HTMLDivElement | null>(null)
  const [size, setSize] = useState<Size>({ width: 0, height: 0 })
  const [view, setView] = useState<MapViewport | null>(null)
  const [query, setQuery] = useState('')
  const [hidden, setHidden] = useState<Set<number>>(() => new Set())
  const [hover, setHover] = useState<{ index: number; x: number; y: number } | null>(null)
  const drag = useRef<{ x: number; y: number; moved: boolean; view: MapViewport } | null>(null)
  const mapSize = Math.max(1, Math.min(size.width, size.height) - 32)
  // The canvas exists only once the map has points: the effects that attach to it wait for that.
  const hasPoints = model.points.length > 0

  const searched = useMemo(() => searchSounds(model, query), [model, query])
  const indexOf = useMemo(() => new Map(model.points.map((point, index) => [point.id, index])), [model])
  const foundIds = useMemo(
    () => (found && (found.revision === null || found.revision === model.revision) ? new Set(found.ids) : null),
    [found, model.revision]
  )
  const visible = useMemo(() => {
    const groupHidden = hidden.size > 0
    return (index: number): boolean => {
      const point = model.points[index]
      if (groupHidden && hidden.has(point.group)) return false
      return !searched || searched.has(point.id)
    }
  }, [model, hidden, searched])
  // What a search, or else the agent's find, picked out, also as named rows: a keyboard or a screen reader can pick a
  // sound the canvas only draws.
  const matches = useMemo(() => {
    const ids = searched ?? foundIds
    return ids ? [...ids].filter(id => indexOf.has(id) && visible(indexOf.get(id) as number)) : []
  }, [searched, foundIds, indexOf, visible])

  // Follow the box's size (the pane or tab is resized all the time).
  useEffect(() => {
    const box = boxRef.current
    if (!box) return
    const observer = new ResizeObserver(entries => {
      const rect = entries[0]?.contentRect
      if (rect) setSize({ width: Math.floor(rect.width), height: Math.floor(rect.height) })
    })
    observer.observe(box)
    return () => observer.disconnect()
  }, [hasPoints])

  // A new map, or a new size before the user moved the view: fit the whole map.
  useEffect(() => {
    if (size.width > 0 && size.height > 0) setView(current => current ?? fitView(size.width, size.height))
  }, [size.width, size.height])
  useEffect(() => setView(size.width > 0 ? fitView(size.width, size.height) : null), [model.revision]) // eslint-disable-line react-hooks/exhaustive-deps

  // What the agent found: frame it.
  useEffect(() => {
    if (!foundIds || size.width === 0) return
    const framed = frame(model, foundIds, size.width, size.height)
    if (framed) setView(framed)
  }, [foundIds, model, size.width, size.height])

  // Wheel zoom (a native listener: React's wheel handler can't stop the page from scrolling).
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const wheel = (event: WheelEvent) => {
      event.preventDefault()
      const rect = canvas.getBoundingClientRect()
      const factor = Math.exp(-event.deltaY * (event.ctrlKey ? 0.01 : 0.0025))
      setView(current => (current ? zoomAt(current, factor, event.clientX - rect.left, event.clientY - rect.top) : current))
    }
    canvas.addEventListener('wheel', wheel, { passive: false })
    return () => canvas.removeEventListener('wheel', wheel)
  }, [hasPoints])

  // Draw whenever anything shown changes.
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || !view || size.width === 0) return
    const ratio = window.devicePixelRatio || 1
    canvas.width = Math.round(size.width * ratio)
    canvas.height = Math.round(size.height * ratio)
    const context = canvas.getContext('2d')
    if (!context) return
    const colors = resolve(canvas)
    context.setTransform(ratio, 0, 0, ratio, 0, 0)
    context.clearRect(0, 0, size.width, size.height)
    // A small map (a fresh folder of a few sounds) draws bigger points than a library of thousands.
    const count = model.points.length
    const base = count <= 40 ? 5 : count <= 300 ? 3.2 : 2.2
    const radius = Math.max(1.5, Math.min(base + 3, base + Math.log2(view.scale) * 0.8))
    const groupColor = new Map(model.groups.map(group => [group.key, group.color]))
    const labelled: [number, number, string][] = []
    // Dimmed points first, then the ones the search and legend keep, so those sit on top.
    for (const pass of [false, true]) {
      context.globalAlpha = pass ? 0.92 : 0.16
      model.points.forEach((point, index) => {
        if (visible(index) !== pass) return
        const [x, y] = toScreen(view, mapSize, point.x, point.y)
        if (x < -8 || y < -8 || x > size.width + 8 || y > size.height + 8) return
        context.fillStyle = groupColor.get(point.group) ?? colors.faint
        context.beginPath()
        context.arc(x, y, radius, 0, Math.PI * 2)
        context.fill()
        if (pass) labelled.push([x, y, model.sounds[point.id]?.name ?? ''])
      })
    }
    // Few enough on screen (a small map, or zoomed in) to read: each one's name beside it, the sound file's own
    // name drawn as canvas text, never markup.
    if (labelled.length <= 40) {
      context.globalAlpha = 0.85
      context.fillStyle = colors.text
      context.font = `11px ${getComputedStyle(canvas).fontFamily || 'sans-serif'}`
      context.textBaseline = 'middle'
      for (const [x, y, name] of labelled) {
        if (name) context.fillText(name.length > 28 ? `${name.slice(0, 27)}…` : name, x + radius + 4, y)
      }
    }
    context.globalAlpha = 1
    const ring = (id: string, color: string, extra: number, width: number) => {
      const index = indexOf.get(id)
      if (index === undefined) return
      const [x, y] = toScreen(view, mapSize, model.points[index].x, model.points[index].y)
      context.strokeStyle = color
      context.lineWidth = width
      context.beginPath()
      context.arc(x, y, radius + extra, 0, Math.PI * 2)
      context.stroke()
    }
    foundIds?.forEach(id => ring(id, colors.accent, 3, 1.5))
    if (playing) ring(playing, colors.accent, 6, 2)
    if (selected) ring(selected, colors.strong, 4, 2)
  }, [model, indexOf, view, size, visible, foundIds, selected, playing, mapSize])

  const at = (event: { clientX: number; clientY: number }): [number, number] => {
    const rect = canvasRef.current?.getBoundingClientRect()
    return rect ? [event.clientX - rect.left, event.clientY - rect.top] : [0, 0]
  }

  if (!model.points.length) {
    return <div style={{ ...S.muted, padding: 16 }}>{empty}</div>
  }

  const chosen = selected ? model.sounds[selected] : undefined
  const hovered = hover ? model.points[hover.index] : undefined
  const hoveredSound = hovered ? model.sounds[hovered.id] : undefined
  const shown = model.points.reduce((count, _, index) => count + (visible(index) ? 1 : 0), 0)

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 240, color: T.text }}>
      <div style={{ ...S.row, flexWrap: 'wrap', padding: '8px 10px', gap: 6, borderBottom: `1px solid ${T.border}` }}>
        <input
          aria-label={t('map.search')}
          onChange={event => setQuery(event.target.value)}
          placeholder={t('map.searchHint')}
          style={{ flex: '1 1 180px', minWidth: 140, font: 'inherit', fontSize: 12, padding: '3px 8px', borderRadius: 3, border: `1px solid ${T.border}`, background: 'transparent', color: T.text }}
          value={query}
        />
        {/* The revision is an id (`r` and 32 hex digits): there for anyone who looks, not in the line itself. */}
        <span style={S.faint} title={model.revision ? t('map.revision', model.revision) : undefined}>
          {shown === model.points.length ? t('map.count', num(model.points.length, 0)) : t('map.countOf', num(shown, 0), num(model.points.length, 0))}
        </span>
      </div>
      <div style={{ ...S.row, flexWrap: 'wrap', gap: 4, padding: '6px 10px' }}>
        {model.groups.map(group => {
          const off = hidden.has(group.key)
          return (
            <button
              key={group.key}
              onClick={() =>
                setHidden(current => {
                  const next = new Set(current)
                  if (next.has(group.key)) next.delete(group.key)
                  else next.add(group.key)
                  return next
                })
              }
              title={off ? t('map.show', group.name) : t('map.hide', group.name)}
              type="button"
              style={{ ...S.row, gap: 4, font: 'inherit', fontSize: 11, padding: '1px 6px', borderRadius: 10, border: `1px solid ${T.border}`, background: 'transparent', color: off ? T.faint : T.secondary, cursor: 'pointer', opacity: off ? 0.6 : 1 }}
            >
              <span style={{ width: 8, height: 8, borderRadius: '50%', background: group.color, opacity: off ? 0.3 : 1 }} />
              {group.name} <span style={{ color: T.faint }}>{group.count}</span>
            </button>
          )
        })}
      </div>
      {matches.length > 0 && (
        <div aria-label={searched ? t('map.matches') : t('map.found')} role="group" style={{ display: 'flex', flexDirection: 'column', gap: 1, padding: '0 10px 6px' }}>
          {matches.slice(0, MATCHES_LISTED).map(id => {
            const sound = model.sounds[id]
            return (
              <button
                aria-pressed={selected === id}
                key={id}
                onClick={() => onPick(id)}
                type="button"
                style={{ ...S.row, gap: 6, font: 'inherit', fontSize: 11.5, padding: '1px 6px', borderRadius: 3, border: 'none', textAlign: 'left', cursor: 'pointer', color: T.text, background: selected === id ? T.card : 'transparent' }}
              >
                <span style={{ ...S.ellipsis, flex: 1 }}>{clean(sound?.name ?? id, 80)}</span>
                {sound?.class && <span style={{ color: T.faint }}>{clean(sound.class, 16)}</span>}
              </button>
            )
          })}
          {matches.length > MATCHES_LISTED && <span style={{ ...S.faint, padding: '0 6px' }}>{t('map.more', matches.length - MATCHES_LISTED)}</span>}
        </div>
      )}
      <div ref={boxRef} style={{ position: 'relative', flex: 1, minHeight: 160, overflow: 'hidden' }}>
        <canvas
          ref={canvasRef}
          aria-label={t('map.canvas')}
          onPointerDown={event => {
            if (!view) return
            event.currentTarget.setPointerCapture?.(event.pointerId)
            const [x, y] = at(event)
            drag.current = { x, y, moved: false, view }
          }}
          onPointerMove={event => {
            const [x, y] = at(event)
            const held = drag.current
            if (held) {
              if (Math.abs(x - held.x) + Math.abs(y - held.y) > 3) held.moved = true
              if (held.moved) setView({ ...held.view, dx: held.view.dx + x - held.x, dy: held.view.dy + y - held.y })
              return
            }
            if (!view) return
            const index = pick(model, view, mapSize, x, y, 8, visible)
            setHover(index >= 0 ? { index, x, y } : null)
          }}
          onPointerUp={event => {
            const held = drag.current
            drag.current = null
            if (!held || held.moved || !view) return
            const [x, y] = at(event)
            const index = pick(model, view, mapSize, x, y, 10, visible)
            if (index >= 0) onPick(model.points[index].id)
          }}
          onPointerLeave={() => setHover(null)}
          onDoubleClick={() => setView(fitView(size.width, size.height))}
          style={{ position: 'absolute', inset: 0, width: '100%', height: '100%', cursor: hover ? 'pointer' : drag.current?.moved ? 'grabbing' : 'grab', touchAction: 'none' }}
        />
        {hovered && hoveredSound && hover && (
          <div
            style={{ position: 'absolute', left: Math.min(hover.x + 12, Math.max(0, size.width - 220)), top: Math.max(0, hover.y - 34), maxWidth: 220, pointerEvents: 'none', padding: '3px 7px', borderRadius: 3, background: T.card, border: `1px solid ${T.border}`, fontSize: 11.5, color: T.text, ...S.ellipsis }}
          >
            {clean(hoveredSound.name, 48)} <span style={{ color: T.muted }}>· {clean(hoveredSound.class ?? '', 16)}</span>
          </div>
        )}
      </div>
      {selected && chosen && (
        <div style={{ ...S.row, justifyContent: 'space-between', gap: 8, padding: '8px 10px', borderTop: `1px solid ${T.border}` }}>
          <div style={{ display: 'flex', flexDirection: 'column', minWidth: 0 }}>
            <span style={{ ...S.text, ...S.ellipsis, fontWeight: 600 }}>{clean(chosen.name, 80)}</span>
            <span style={{ ...S.row, gap: 4, marginTop: 2 }}>
              {chosen.class && <Chip>{clean(chosen.class, 16)}</Chip>}
              {chosen.kind && <Chip>{clean(chosen.kind, 16)}</Chip>}
              {typeof chosen.duration === 'number' && <Chip>{seconds(chosen.duration)}</Chip>}
              {chosen.folder && <span style={{ ...S.faint, ...S.ellipsis }}>{clean(chosen.folder, 60)}</span>}
            </span>
          </div>
          <div style={{ ...S.row, gap: 6 }}>{actions?.(selected, chosen)}</div>
        </div>
      )}
    </div>
  )
}
