// The sound map, drawn on a canvas: each sound of the library where Kumi's layout put it, coloured by its group.
// Wheel zooms, a drag pans, a click picks the sound under the pointer. Search and the legend dim what they leave out;
// sounds the agent just found get a ring. What a search or a find picks out is also listed by name, for the keyboard.
// On the keyboard (`mapKey`), the arrows move to the nearest sound that way, Shift+←/→ step through every sound in turn,
// Shift+↑/↓ zoom and Home shows the whole map; each sound reached is named to a screen reader, and Tab goes on to its
// Play and Use….
// Both UI halves use it; what a picked sound can do comes from `actions`.
import { CANVAS, T } from '@kumi/theme'
import type { KeyboardEvent, ReactNode } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'

import { clean, num, seconds } from './format.ts'
import { buildModel, fitView, foundLine, frame, indexedValues, keepInView, mapKey, mapLabel, type MapViewport, moveWith, pick, searchSounds, toScreen, zoomAt } from './mapModel.ts'
import type { KumiState } from './store.ts'
import type { MapSound, SoundMap } from './types.ts'
import { Chip, FOCUSABLE, S, useT } from './ui.tsx'

export interface MapCanvasProps {
  map: SoundMap | null
  found: KumiState['found']
  playing: string | null
  selected: string | null
  /** A click, or a found list's row: pick that sound and play it. */
  onPick: (id: string) => void
  /** The keyboard moved to a sound: picked, not played (its Play is the next tab stop). */
  onSelect: (id: string) => void
  actions?: (id: string, sound: MapSound) => ReactNode
  empty: ReactNode
  /** Sounds that couldn't play here (the store's `leaving`): they fade off the map at once, and can't be picked. */
  leaving?: ReadonlySet<string>
  /** What the quiet line about hidden sounds offers ("Show them", a rescan), when there are any. */
  hiddenActions?: ReactNode
}

/** How many searched or found sounds are listed by name above the map; the rest stay rings on it. */
const MATCHES_LISTED = 6
/** How long a sound that couldn't play takes to fade off the map. */
const FADE_MS = 450

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

// How many interleaved paths each group's points are drawn in: enough that overlapping groups mix about as the map's
// own order would, few enough that a pass stays a few hundred fills.
const ROUNDS = 8

// Each map's own id for its keys hint. Not React's useId: the hosts' React builds differ, and this needs none of it.
let maps = 0

export function MapCanvas({ map, found, playing, selected, onPick, onSelect, actions, empty, leaving, hiddenActions }: MapCanvasProps) {
  const t = useT()
  const model = useMemo(() => buildModel(map), [map])
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const boxRef = useRef<HTMLDivElement | null>(null)
  const [size, setSize] = useState<Size>({ width: 0, height: 0 })
  const [view, setView] = useState<MapViewport | null>(null)
  // What the search box holds, and what the map searches for: the latter a beat behind while the producer types, so a
  // 25,000-sound map redraws once per pause, not on every key.
  const [query, setQuery] = useState('')
  const [searchFor, setSearchFor] = useState('')
  const [hidden, setHidden] = useState<Set<number>>(() => new Set())
  const [hover, setHover] = useState<{ index: number; x: number; y: number } | null>(null)
  const drag = useRef<{ x: number; y: number; moved: boolean; view: MapViewport } | null>(null)
  // A drag or a wheel moves the view at most once per frame: the latest view waits here for the next animation frame.
  const viewRef = useRef<MapViewport | null>(null)
  viewRef.current = view
  const nextView = useRef<MapViewport | null>(null)
  const frameRef = useRef(0)
  const queueView = (next: MapViewport) => {
    nextView.current = next
    if (frameRef.current) return
    frameRef.current = requestAnimationFrame(() => {
      frameRef.current = 0
      const latest = nextView.current
      nextView.current = null
      if (latest) setView(latest)
    })
  }
  useEffect(() => () => cancelAnimationFrame(frameRef.current), [])
  // The canvas's backing store, resized only when its size or the screen's pixel ratio changes: setting a canvas's
  // width reallocates and clears it.
  const backing = useRef('')
  // What a screen reader is told when the keyboard moves to a sound.
  const [said, setSaid] = useState('')
  const [hintId] = useState(() => `kumi-map-keys-${(maps += 1)}`)
  const mapSize = Math.max(1, Math.min(size.width, size.height) - 32)
  // The canvas exists only once the map has points: the effects that attach to it wait for that.
  const hasPoints = model.points.length > 0

  useEffect(() => {
    if (query === searchFor) return
    const timer = setTimeout(() => setSearchFor(query), query ? 150 : 0)
    return () => clearTimeout(timer)
  }, [query, searchFor])
  const searched = useMemo(() => searchSounds(model, searchFor), [model, searchFor])
  const indexOf = useMemo(() => new Map(model.points.map((point, index) => [point.id, index])), [model])
  const foundIds = useMemo(
    () => (found && (found.revision === null || found.revision === model.revision) ? new Set(found.ids) : null),
    [found, model.revision]
  )
  // Which points the legend and the search keep (1), dim (0), or leave off the map (2: they couldn't play here), once
  // per change of any of them, so a frame's draw and a hover read one byte per point instead of a set lookup by id.
  const mask = useMemo(() => {
    const out = new Uint8Array(model.points.length)
    model.points.forEach((point, index) => {
      out[index] = leaving?.has(point.id) ? 2 : (hidden.size > 0 && hidden.has(point.group)) || (searched !== null && !searched.has(point.id)) ? 0 : 1
    })
    return out
  }, [model, hidden, searched, leaving])
  const visible = useMemo(() => (index: number): boolean => mask[index] === 1, [mask])
  const shown = useMemo(() => mask.reduce((count, keep) => count + (keep === 1 ? 1 : 0), 0), [mask])
  const leavingCount = useMemo(() => mask.reduce((count, keep) => count + (keep === 2 ? 1 : 0), 0), [mask])
  // When each leaving sound started fading, and a clock that moves once a frame while one still fades.
  const leftAt = useRef(new Map<string, number>())
  const [clock, setClock] = useState(0)
  useEffect(() => {
    const now = performance.now()
    for (const id of leaving ?? []) if (!leftAt.current.has(id)) leftAt.current.set(id, now)
    for (const id of [...leftAt.current.keys()]) if (!leaving?.has(id)) leftAt.current.delete(id)
  }, [leaving])
  useEffect(() => {
    const now = performance.now()
    if (![...leftAt.current.values()].some(at => now - at < FADE_MS)) return
    const frame = requestAnimationFrame(setClock)
    return () => cancelAnimationFrame(frame)
  }, [clock, leaving])
  // What a search, or else the agent's find, picked out, also as named rows: a keyboard or a screen reader can pick a
  // sound the canvas only draws.
  const matches = useMemo(() => {
    const ids = searched ?? foundIds
    return ids ? [...ids].filter(id => indexOf.has(id) && visible(indexOf.get(id) as number)) : []
  }, [searched, foundIds, indexOf, visible])
  // CC-23: a near search names its seed (the top-level `near`) from this map; one the map doesn't list heads nothing.
  const nearName = !searched && foundIds && found?.near && model.sounds[found.near] ? clean(model.sounds[found.near].name ?? '', 60) : ''

  // The canvas paints the theme's colours itself, and neither host tells a plugin its theme changed (Desktop's
  // useTheme is newer than some Desktops; the dashboard has none): it looks at them once a second, and a light or dark
  // switch redraws it.
  const [painted, setPainted] = useState('')
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const timer = setInterval(() => {
      const now = Object.values(resolve(canvas)).join()
      setPainted(current => (current === now ? current : now))
    }, 1000)
    return () => clearInterval(timer)
  }, [hasPoints])

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
      const base = nextView.current ?? viewRef.current
      if (base) queueView(zoomAt(base, factor, event.clientX - rect.left, event.clientY - rect.top))
    }
    canvas.addEventListener('wheel', wheel, { passive: false })
    return () => canvas.removeEventListener('wheel', wheel)
  }, [hasPoints])

  // Draw whenever anything shown changes. Points are batched into one path per group colour, so a 25,000-sound map is a
  // few hundred fills, not 25,000. Each colour's points are split into `ROUNDS` interleaved paths, drawn round by round:
  // where groups overlap, their colours still mix as the map's own order left them, instead of the last colour covering
  // the rest.
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || !view || size.width === 0) return
    const ratio = window.devicePixelRatio || 1
    const wanted = `${Math.round(size.width * ratio)}x${Math.round(size.height * ratio)}`
    if (backing.current !== wanted) {
      backing.current = wanted
      canvas.width = Math.round(size.width * ratio)
      canvas.height = Math.round(size.height * ratio)
    }
    const context = canvas.getContext('2d')
    if (!context) return
    const colors = resolve(canvas)
    context.setTransform(ratio, 0, 0, ratio, 0, 0)
    context.clearRect(0, 0, size.width, size.height)
    // A small map (a fresh folder of a few sounds) draws bigger points than a library of thousands.
    const points = model.points
    const count = points.length
    const base = count <= 40 ? 5 : count <= 300 ? 3.2 : 2.2
    const radius = Math.max(1.5, Math.min(base + 3, base + Math.log2(view.scale) * 0.8))
    const groupColor = new Map(model.groups.map(group => [group.key, group.color]))
    const unit = mapSize * view.scale
    const [left, top, right, bottom] = [-8, -8, size.width + 8, size.height + 8]
    // The labels need at most 41 on-screen points: past 40 there are none.
    const labelled: [number, number, string][] = []
    let onScreen = 0
    // Dimmed points first, then the ones the search and legend keep, so those sit on top.
    for (const pass of [0, 1]) {
      const rounds = Array.from({ length: ROUNDS }, () => new Map<string, Path2D>())
      for (let index = 0; index < count; index++) {
        if (mask[index] !== pass) continue
        const point = points[index]
        const x = point.x * unit + view.dx
        const y = point.y * unit + view.dy
        if (x < left || y < top || x > right || y > bottom) continue
        const color = groupColor.get(point.group) ?? colors.faint
        const paths = rounds[index % ROUNDS]
        let path = paths.get(color)
        if (!path) {
          path = new Path2D()
          paths.set(color, path)
        }
        path.moveTo(x + radius, y)
        path.arc(x, y, radius, 0, Math.PI * 2)
        if (pass === 1 && ++onScreen <= 41) labelled.push([x, y, model.sounds[point.id]?.name ?? ''])
      }
      context.globalAlpha = pass ? 0.92 : 0.16
      for (const paths of rounds) {
        for (const [color, path] of paths) {
          context.fillStyle = color
          context.fill(path)
        }
      }
    }
    // A sound that couldn't play shrinks and fades off where it was, drawn on its own: there are only ever a few.
    const now = performance.now()
    for (const [id, at] of leftAt.current) {
      const progress = (now - at) / FADE_MS
      const index = indexOf.get(id)
      if (progress >= 1 || index === undefined) continue
      const eased = 1 - (1 - progress) ** 2
      const point = points[index]
      context.globalAlpha = 0.92 * (1 - eased)
      context.fillStyle = groupColor.get(point.group) ?? colors.faint
      context.beginPath()
      context.arc(point.x * unit + view.dx, point.y * unit + view.dy, radius * (1 - eased) + 0.1, 0, Math.PI * 2)
      context.fill()
    }
    // Few enough on screen (a small map, a narrow search, or zoomed in) to read: each one's name beside it, the sound
    // file's own name drawn as canvas text, never markup. A name that would overlap one already drawn is left out, so
    // a tight cluster stays legible (its sounds are named on hover and in the list above).
    if (onScreen <= 40) {
      context.globalAlpha = 0.85
      context.fillStyle = colors.text
      context.font = `11px ${getComputedStyle(canvas).fontFamily || 'sans-serif'}`
      context.textBaseline = 'middle'
      const placed: [number, number, number, number][] = []
      for (const [x, y, name] of labelled) {
        const text = mapLabel(name)
        if (!text) continue
        const box: [number, number, number, number] = [x + radius + 4, y - 7, context.measureText(text).width, 14]
        if (placed.some(([left, top, width, height]) => box[0] < left + width && left < box[0] + box[2] && box[1] < top + height && top < box[1] + box[3])) continue
        placed.push(box)
        context.fillText(text, box[0], y)
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
  }, [model, indexOf, view, size, mask, foundIds, selected, playing, mapSize, painted, clock])

  const at = (event: { clientX: number; clientY: number }): [number, number] => {
    const rect = canvasRef.current?.getBoundingClientRect()
    return rect ? [event.clientX - rect.left, event.clientY - rect.top] : [0, 0]
  }

  const onKey = (event: KeyboardEvent<HTMLCanvasElement>) => {
    const key = mapKey(event)
    if (!view || !key) return
    event.preventDefault()
    const from = selected === null ? undefined : indexOf.get(selected)
    const picked = from !== undefined && visible(from) ? from : undefined
    const middle: [number, number] = [size.width / 2, size.height / 2]
    if ('fit' in key) {
      setView(fitView(size.width, size.height))
    } else if ('zoom' in key) {
      // Around the sound picked, so it stays where it is; else around the middle.
      const [x, y] = picked === undefined ? middle : toScreen(view, mapSize, model.points[picked].x, model.points[picked].y)
      setView(zoomAt(view, key.zoom, x, y))
    } else {
      const next = moveWith(model, view, mapSize, picked, key, middle, visible)
      if (next < 0) return
      const point = model.points[next]
      const [x, y] = toScreen(view, mapSize, point.x, point.y)
      setView(keepInView(view, x, y, size.width, size.height))
      onSelect(point.id)
      const sound = model.sounds[point.id]
      setSaid([clean(sound?.name ?? point.id, 80), clean(sound?.class, 16), clean(sound?.folder, 60)].filter(Boolean).join(', '))
    }
  }

  if (!model.points.length) {
    return (
      <div style={{ ...S.muted, padding: 16 }}>
        {empty}
        {(map?.hidden ?? 0) > 0 && (
          <div role="status" style={{ ...S.row, flexWrap: 'wrap', gap: 6, marginTop: 8 }}>
            <span style={{ ...S.faint, flex: '1 1 14rem' }}>{t('map.hidden', map?.hidden ?? 0)}</span>
            {hiddenActions}
          </div>
        )}
      </div>
    )
  }

  const chosen = selected ? model.sounds[selected] : undefined
  const hovered = hover ? model.points[hover.index] : undefined
  const hoveredSound = hovered ? model.sounds[hovered.id] : undefined

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 240, color: T.text }}>
      <div style={{ ...S.row, flexWrap: 'wrap', padding: '8px 10px', gap: 6, borderBottom: `1px solid ${T.border}` }}>
        <input
          aria-label={t('map.search')}
          className={FOCUSABLE}
          onChange={event => setQuery(event.target.value)}
          placeholder={t('map.searchHint')}
          style={{ flex: '1 1 180px', minWidth: 140, font: 'inherit', fontSize: 12, padding: '3px 8px', borderRadius: 3, border: `1px solid ${T.border}`, background: 'transparent', color: T.text }}
          value={query}
        />
        {/* The revision is an id (`r` and 32 hex digits): there for anyone who looks, not in the line itself. */}
        <span style={S.faint} title={model.revision ? t('map.revision', model.revision) : undefined}>
          {shown === model.points.length - leavingCount
            ? t('map.count', num(shown, 0))
            : t('map.countOf', num(shown, 0), num(model.points.length - leavingCount, 0))}
        </span>
      </div>
      <div style={{ ...S.row, flexWrap: 'wrap', gap: 4, padding: '6px 10px' }}>
        {model.groups.map(group => {
          const off = hidden.has(group.key)
          return (
            <button
              aria-pressed={!off}
              className={FOCUSABLE}
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
        <div aria-label={searched ? t('map.matches') : nearName ? t('map.near', nearName) : t('map.found')} role="group" style={{ display: 'flex', flexDirection: 'column', gap: 1, padding: '0 10px 6px' }}>
          {/* CC-23: a near search's neighbours, under the seed's name from this map (the seed itself isn't listed). */}
          {nearName && <span style={{ ...S.muted, padding: '0 6px' }}>{t('map.near', nearName)}</span>}
          {matches.slice(0, MATCHES_LISTED).map(id => {
            const sound = model.sounds[id]
            const indexed = indexedValues(sound, t)
            // A typed search lists names; the agent's find also says why each matched, as Kumi bounds it (CC-23).
            const said = searched ? '' : foundLine(found?.matches?.[id], nearName !== '', t)
            return (
              <button
                aria-pressed={selected === id}
                className={FOCUSABLE}
                key={id}
                onClick={() => onPick(id)}
                type="button"
                style={{ display: 'flex', flexDirection: 'column', alignItems: 'stretch', gap: 0, font: 'inherit', fontSize: 11.5, padding: '1px 6px', borderRadius: 3, border: 'none', textAlign: 'left', cursor: 'pointer', color: T.text, background: selected === id ? T.card : 'transparent' }}
              >
                <span style={{ ...S.row, gap: 6 }}>
                  <span style={{ ...S.ellipsis, flex: 1 }}>{clean(sound?.name ?? id, 80)}</span>
                  {indexed && (
                    <span style={{ color: T.faint, whiteSpace: 'nowrap' }} title={t('map.indexedTitle')}>
                      {t('map.indexed', indexed)}
                    </span>
                  )}
                  {sound?.class && <span style={{ color: T.faint }}>{clean(sound.class, 16)}</span>}
                </span>
                {said && (
                  <span style={{ ...S.ellipsis, color: T.faint, fontSize: 10.5 }} title={nearName ? t('map.closenessTitle') : undefined}>
                    {said}
                  </span>
                )}
              </button>
            )
          })}
          {matches.length > MATCHES_LISTED && <span style={{ ...S.faint, padding: '0 6px' }}>{t('map.more', matches.length - MATCHES_LISTED)}</span>}
        </div>
      )}
      <div ref={boxRef} style={{ position: 'relative', flex: 1, minHeight: 160, overflow: 'hidden' }}>
        <canvas
          ref={canvasRef}
          aria-describedby={hintId}
          aria-label={t('map.canvas')}
          aria-roledescription={t('map.roledescription')}
          className={FOCUSABLE}
          onKeyDown={onKey}
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
              if (held.moved) queueView({ ...held.view, dx: held.view.dx + x - held.x, dy: held.view.dy + y - held.y })
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
          role="application"
          tabIndex={0}
          // The box clips what's outside it: the focus ring sits inside the canvas's edge.
          style={{ position: 'absolute', inset: 0, width: '100%', height: '100%', outlineOffset: -3, cursor: hover ? 'pointer' : drag.current?.moved ? 'grabbing' : 'grab', touchAction: 'none' }}
        />
        <span id={hintId} style={S.hidden}>
          {t('map.keys')}
        </span>
        <span aria-live="polite" style={S.hidden}>
          {said}
        </span>
        {hovered && hoveredSound && hover && (
          <div
            style={{ position: 'absolute', left: Math.min(hover.x + 12, Math.max(0, size.width - 220)), top: Math.max(0, hover.y - 34), maxWidth: 220, pointerEvents: 'none', padding: '3px 7px', borderRadius: 3, background: T.card, border: `1px solid ${T.border}`, fontSize: 11.5, color: T.text, ...S.ellipsis }}
          >
            {clean(hoveredSound.name, 48)} <span style={{ color: T.muted }}>· {clean(hoveredSound.class ?? '', 16)}</span>
          </div>
        )}
      </div>
      {(map?.hidden ?? 0) + leavingCount > 0 && (
        // One quiet line for every sound that couldn't play here, never a notice per sound.
        <div role="status" style={{ ...S.row, flexWrap: 'wrap', gap: 6, padding: '4px 10px', borderTop: `1px solid ${T.border}` }}>
          <span style={{ ...S.faint, flex: '1 1 14rem' }}>{t('map.hidden', (map?.hidden ?? 0) + leavingCount)}</span>
          {hiddenActions}
        </div>
      )}
      {selected && chosen && (
        // Beside the chat the map can be narrow: the name and the actions wrap rather than push actions out of view.
        <div style={{ ...S.row, justifyContent: 'space-between', flexWrap: 'wrap', gap: 8, padding: '8px 10px', borderTop: `1px solid ${T.border}` }}>
          <div style={{ display: 'flex', flexDirection: 'column', minWidth: 0 }}>
            <span style={{ ...S.text, ...S.ellipsis, fontWeight: 600 }}>{clean(chosen.name, 80)}</span>
            <span style={{ ...S.row, gap: 4, marginTop: 2 }}>
              {chosen.class && <Chip>{clean(chosen.class, 16)}</Chip>}
              {chosen.kind && <Chip>{clean(chosen.kind, 16)}</Chip>}
              {typeof chosen.duration === 'number' && <Chip>{seconds(chosen.duration)}</Chip>}
              {chosen.folder && <span style={{ ...S.faint, ...S.ellipsis }}>{clean(chosen.folder, 60)}</span>}
            </span>
          </div>
          <div style={{ ...S.row, gap: 6, flexWrap: 'wrap', justifyContent: 'flex-end', marginLeft: 'auto' }}>{actions?.(selected, chosen)}</div>
        </div>
      )}
    </div>
  )
}
