// The sound-map workspace: Kumi's map of the approved sample folders, with the palette under it. A sound plays here,
// in the window, as the pointer comes onto it or on a click (a read, nothing reaches the DAW); a box picks sounds
// (Shift adds another), and Add to palette gathers them into the palette the agent shares. "Use" puts a request naming
// a sound into the chat for the producer to finish, "Use on pad…" one naming a drum pad too, "More like this" (or "More
// like these" for the picks) one asking for the sounds nearest them, and the palette's "Use as kit…" one to load it as a
// kit. "Make…" (the picks' or the palette's) opens the Make panel, which makes new sounds from them. A sound that can't
// play here leaves the map quietly (never a notice per sound), and one line under the map counts them.
import type { PluginContext } from '@hermes/plugin-sdk'
import { host } from '@hermes/plugin-sdk'
import { T } from '@kumi/theme'
import { useEffect, useRef, useState } from 'react'

import { jobWords, liveNote, MOST_NEAR, nearRequest, padRequest, RESCAN_REQUEST, SCAN_REQUEST, soundRequest } from '../shared/format.ts'
import { sourcesOf, useRequest } from '../shared/design.ts'
import { soundEnd, useGrow } from '../shared/grow.ts'
import { type Audition, useKumi, usePlaying, useWant } from '../shared/hooks.ts'
import { MakePanel, sideSheetWidth } from '../shared/MakePanel.tsx'
import { MapCanvas } from '../shared/MapCanvas.tsx'
import { ToolButton } from '../shared/mapUi.tsx'
import { itemReference, kitRequest } from '../shared/palette.ts'
import { PaletteDock } from '../shared/PaletteDock.tsx'
import { usePaletteMap } from '../shared/paletteMap.ts'
import { errorText, type KumiStore } from '../shared/store.ts'
import { FOCUSABLE, TextButton, useT } from '../shared/ui.tsx'
import { ask } from './actions.ts'
import { nextSteps, ownerSession } from './pane.tsx'

/** Desktop's own storage key for hover-to-play (on unless the producer turned it off, as native Kumi's `h`). */
export const HOVER_PLAY = 'mapHover'
/** Desktop's own storage key for the square-tile view (off until the producer turns it on). */
export const TILES = 'mapTiles'
/** Desktop's own storage key for whether the palette drawer is open (open until the producer folds it). */
export const PALETTE_OPEN = 'paletteOpen'
/** Desktop's own storage key for the map's layout (similarity, '', until the producer picks another). */
export const LAYOUT = 'mapLayout'
/** How wide the map must be for the Make panel to float beside it rather than over its lower part. */
const SIDE_SHEET = 820
/** Setup's states with no usable runtime: nothing builds the map until setup runs. */
const RUNTIME_MISSING: Record<string, true> = { needs_runtime: true, outdated_runtime: true, damaged_runtime: true }

// The first sixteen pads as Live's Drum Rack shows them: C1 (note 36) bottom left, D#2 (51) top right.
const PAD_ROWS = [48, 44, 40, 36].map(start => [start, start + 1, start + 2, start + 3])

/** Sixteen drum pads to choose from, on a line of their own under the sound's actions, so a narrow map beside the chat
 *  shows them whole. Picking one drafts the request; Escape, or the button again, closes them. Live's pads carry Live's
 *  names; another DAW's, their notes. */
export function PadPicker({ daw, onPick, onClose }: { daw: unknown; onPick: (note: number) => void; onClose: () => void }) {
  const t = useT()
  const first = useRef<HTMLButtonElement>(null)
  useEffect(() => first.current?.focus(), [])
  return (
    <div
      aria-label={t('map.pads')}
      onKeyDown={event => {
        if (event.key !== 'Escape') return
        event.stopPropagation()
        onClose()
      }}
      role="group"
      style={{ flexBasis: '100%', display: 'grid', gridTemplateColumns: 'repeat(4, 2.8rem)', justifyContent: 'end', gap: 4, paddingTop: 6 }}
    >
      {PAD_ROWS.flat().map(note => {
        const name = daw === 'ableton' ? liveNote(note) : String(note)
        return (
          <button
            aria-label={t('map.pad', name, note)}
            className={FOCUSABLE}
            key={note}
            onClick={() => onPick(note)}
            ref={note === 36 ? first : undefined}
            style={{ font: 'inherit', fontSize: 11, height: 28, borderRadius: 4, border: `1px solid ${T.border}`, background: 'transparent', color: T.text, cursor: 'pointer' }}
            title={t('map.pad', name, note)}
            type="button"
          >
            {name}
          </button>
        )
      })}
    </div>
  )
}

export function MapWorkspace({ ctx, store, audition }: { ctx: PluginContext; store: KumiStore; audition: Audition }) {
  const t = useT()
  useWant(store, 'map')
  useWant(store, 'palette')
  const state = useKumi(store)
  const playing = usePlaying(audition)
  const [selected, setSelected] = useState<null | string>(null)
  // The sound whose pad picker is open: one at a time, and it closes when another sound is picked.
  const [padsFor, setPadsFor] = useState<null | string>(null)
  const [hover, setHover] = useState(() => ctx.storage.get<boolean>(HOVER_PLAY, true) !== false)
  const [tiles, setTiles] = useState(() => ctx.storage.get<boolean>(TILES, false) === true)
  const [layout, setLayout] = useState(() => ctx.storage.get<string>(LAYOUT, '') ?? '')
  const palette = usePaletteMap(store, state, audition, t, {
    hover,
    other: error => host.notify({ kind: 'warning', message: t('map.playFailed', errorText(error)) }),
    left: id => setSelected(current => (current === id ? null : current))
  })
  // Grow: its renders go through the store; the handle's point plays as soon as it's grown.
  const playMade = (id: string) => void audition.play(id).catch(error => host.notify({ kind: 'warning', message: t('map.playFailed', errorText(error)) }))
  const grow = useGrow(request => store.design(request), result => playMade(result.artifact_id), state.map?.revision ?? null)
  // The dock's box: the Make panel flies a kept sound into it.
  const dock = useRef<HTMLElement | null>(null)
  const [paletteOpen, setPaletteOpen] = useState(() => ctx.storage.get<boolean>(PALETTE_OPEN, true) !== false)
  // How wide the map is: wide enough, and the Make panel floats beside it, the map moving into the room it leaves.
  const area = useRef<HTMLDivElement | null>(null)
  const [areaWidth, setAreaWidth] = useState(0)
  useEffect(() => {
    const element = area.current
    if (!element) return
    const observer = new ResizeObserver(entries => setAreaWidth(entries[0]?.contentRect.width ?? 0))
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  const beside = areaWidth >= SIDE_SHEET
  const session = ownerSession(state)
  const map = state.map
  // An empty map says what builds it and offers that step: setup when Kumi's runtime is missing (the pane's own step,
  // sent as the pane sends it), the scan's progress while one runs, else a request to scan (Kumi asks which folder
  // when none is set up).
  const setupFirst = RUNTIME_MISSING[state.setup?.state ?? ''] ? (nextSteps(state, t)[0] ?? null) : null
  const scanning = Object.values(state.jobs).find(job => job.kind === 'scan' && (job.state === 'running' || job.state === undefined))
  const empty =
    state.available === false ? (
      t('map.agentOff')
    ) : map === null ? (
      t('map.reading')
    ) : scanning ? (
      <>
        {t('map.scanningNow')}
        {jobWords(scanning, t).detail && <div style={{ marginTop: 6, color: T.faint }}>{jobWords(scanning, t).detail}</div>}
      </>
    ) : (
      <>
        {setupFirst ? t('map.setupFirst') : t('map.empty')}
        <div style={{ marginTop: 10 }}>
          {setupFirst ? (
            <TextButton onClick={() => void ask(ctx, setupFirst.request, { send: true, session: null })} strong>
              {setupFirst.label}
            </TextButton>
          ) : (
            <TextButton onClick={() => void ask(ctx, SCAN_REQUEST, { send: false, session })} strong title={t('map.scanTitle')}>
              {t('map.scan')}
            </TextButton>
          )}
        </div>
      </>
    )
  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0 }}>
      {/* The Make panel floats over the map, not the palette: a kept sound lands in the palette in view. */}
      <div ref={area} style={{ flex: 1, minHeight: 0, position: 'relative' }}>
        <MapCanvas
          audio={id => audition.url(id)}
          coverRight={palette.making && beside ? sideSheetWidth(areaWidth) + 16 : 0}
          onStop={() => audition.stop()}
          actions={(id, sound) => {
            const request = soundRequest(id, sound.name, map?.revision)
            const near = nearRequest([{ id, name: sound.name }], map?.revision)
            return (
              <>
                <TextButton disabled={palette.marked.has(id)} onClick={() => void palette.gather([id])} title={t('dock.addTitle')}>
                  {palette.marked.has(id) ? t('dock.inPalette') : t('dock.add')}
                </TextButton>
                {near && (
                  <TextButton onClick={() => void ask(ctx, near, { send: false, session })} title={t('map.moreLikeTitle')}>
                    {t('map.moreLike')}
                  </TextButton>
                )}
                {request && (
                  <TextButton onClick={() => setPadsFor(padsFor === id ? null : id)} pressed={padsFor === id} title={t('map.useOnPadTitle')}>
                    {t('map.useOnPad')}
                  </TextButton>
                )}
                {request && (
                  <TextButton onClick={() => void ask(ctx, request, { send: false, session })} strong title={t('map.useTitle')}>
                    {t('map.use')}
                  </TextButton>
                )}
                {request && padsFor === id && (
                  <PadPicker
                    daw={state.status?.daw}
                    onClose={() => setPadsFor(null)}
                    onPick={note => {
                      setPadsFor(null)
                      const onPad = padRequest(id, note, state.status?.daw, sound.name, map?.revision)
                      if (onPad) void ask(ctx, onPad, { send: false, session })
                    }}
                  />
                )}
              </>
            )
          }}
          empty={empty}
          found={state.found}
          hiddenActions={
            <>
              <TextButton onClick={() => void store.showHidden()} title={t('map.showHiddenTitle')}>
                {t('map.showHidden')}
              </TextButton>
              <TextButton onClick={() => void ask(ctx, RESCAN_REQUEST, { send: false, session })} title={t('map.rescanTitle')}>
                {t('map.rescan')}
              </TextButton>
            </>
          }
          leaving={state.leaving}
          map={map}
          marked={palette.marked}
          onHover={palette.onHover}
          onPick={id => {
            setSelected(id)
            palette.play(id)
          }}
          onPicked={palette.onPicked}
          grow={grow}
          onPlayMade={result => playMade(result.artifact_id)}
          growActions={result => {
            const request = useRequest(result)
            const kept = palette.kept.has(result.artifact_id)
            return (
              <>
                <TextButton disabled={kept} onClick={() => void palette.keep(result)} title={t('make.keepTitle')}>
                  {kept ? t('make.kept') : t('make.keep')}
                </TextButton>
                {request && (
                  <TextButton onClick={() => void ask(ctx, request, { send: false, session })} title={t('make.useTitle')}>
                    {t('make.use')}
                  </TextButton>
                )}
                <TextButton onClick={() => palette.makeFrom(sourcesOf([{ artifact_id: result.artifact_id, name: result.recipe }], null))} title={t('make.openTitle')}>
                  {t('make.open')}
                </TextButton>
              </>
            )
          }}
          onSelect={id => {
            setSelected(id)
            if (id !== padsFor) setPadsFor(null)
          }}
          picked={palette.picked}
          pickedActions={ids => {
            const near = ids.length > 1 ? nearRequest(ids.map(id => ({ id, name: map?.sounds[id]?.name })), map?.revision) : null
            return (
              <>
                <TextButton onClick={() => void palette.gather(ids)} strong title={t('dock.addTitle')}>
                  {t('dock.addPicked', ids.length)}
                </TextButton>
                {ids.length > 1 && (
                  <TextButton onClick={() => palette.makeFromPicks(ids)} title={t('make.openTitle')}>
                    {t('make.open')}
                  </TextButton>
                )}
                {ids.length === 2 &&
                  (() => {
                    const [from, to] = ids.map(id => soundEnd(id, map?.sounds[id]?.name, map?.revision ?? null))
                    return from && to ? (
                      <TextButton onClick={() => grow.between(from, to)} title={t('grow.betweenTitle')}>
                        {t('grow.between')}
                      </TextButton>
                    ) : null
                  })()}
                {near && (
                  <TextButton
                    onClick={() => void ask(ctx, near, { send: false, session })}
                    title={ids.length > MOST_NEAR ? t('map.moreLikeTheseFirst', MOST_NEAR, ids.length) : t('map.moreLikeTheseTitle')}
                  >
                    {t('map.moreLikeThese', Math.min(ids.length, MOST_NEAR))}
                  </TextButton>
                )}
                <TextButton onClick={palette.clearPicks} title={t('dock.clearPicksTitle')}>
                  {t('dock.clearPicks', ids.length)}
                </TextButton>
              </>
            )
          }}
          playing={playing}
          selected={selected}
          flying={palette.flying}
          onTiles={on => {
            ctx.storage.set(TILES, on)
            setTiles(on)
          }}
          tiles={tiles}
          arrangement={layout}
          onArrangement={id => {
            ctx.storage.set(LAYOUT, id)
            setLayout(id)
          }}
          tools={
            <ToolButton
              icon="hover"
              label={t('dock.hover')}
              onClick={() => {
                ctx.storage.set(HOVER_PLAY, !hover)
                setHover(!hover)
              }}
              pressed={hover}
              title={t('dock.hoverTitle')}
            />
          }
        />
        {palette.making && (
          <MakePanel
            colorOf={palette.colorOf}
            dock={dock}
            kept={palette.kept}
            onClose={palette.closeMake}
            onKeep={palette.keep}
            onMake={request => store.design(request)}
            onSources={palette.makeFrom}
            onPlay={id => void audition.play(id).catch(error => host.notify({ kind: 'warning', message: t('map.playFailed', errorText(error)) }))}
            onStop={() => audition.stop()}
            placement={beside ? 'side' : 'bottom'}
            playing={playing}
            results={state.designs}
            sources={palette.making}
            tempo={state.focus?.transport?.tempo}
            use={result => {
              const request = useRequest(result)
              return request ? (
                <TextButton onClick={() => void ask(ctx, request, { send: false, session })} strong title={t('make.useTitle')}>
                  {t('make.use')}
                </TextButton>
              ) : null
            }}
          />
        )}
      </div>
      {/* The palette's strip waits for a map: with no sounds yet, the empty map's next step has the pane to itself. */}
      {(map?.points?.length ?? 0) > 0 && (
        <PaletteDock
          audio={id => audition.url(id)}
          groups={map?.groups}
          kit={current => (
            <TextButton onClick={() => void ask(ctx, kitRequest(current.name), { send: false, session })} title={t('dock.kitTitle')}>
              {t('dock.kit')}
            </TextButton>
          )}
          landing={dock}
          make={palette.makeFromPalette}
          onDo={action => store.paletteDo(action)}
          onOpen={open => {
            ctx.storage.set(PALETTE_OPEN, open)
            setPaletteOpen(open)
          }}
          onPlay={id => palette.play(id)}
          onStop={() => audition.stop()}
          open={paletteOpen}
          palette={state.palette}
          playing={playing}
          said={state.paletteSaid}
          use={item => {
            const request = itemReference(item, state.palette?.map_revision ?? null)
            return request ? (
              <TextButton onClick={() => void ask(ctx, request, { send: false, session })} title={t('map.useTitle')}>
                {t('map.use')}
              </TextButton>
            ) : null
          }}
        />
      )}
    </div>
  )
}
