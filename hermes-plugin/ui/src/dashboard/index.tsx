// Kumi's tab in the Hermes web dashboard: the connection, FOCUS, HISTORY, the sound map and the palette, read from the
// same backend routes as the Desktop half. The dashboard has no chat input to put a request in, so it changes nothing
// in the DAW: a row's request can be copied, to paste into a chat. It does only what changes no project, as Desktop
// does: edits the palette and the sounds hidden from the map, and makes new sounds (the Make panel).
import { Button } from '@kumi/controls'
import { T } from '@kumi/theme'
import { useEffect, useMemo, useRef, useState } from 'react'

import { addFolderRequest, beatSeconds, canUndoChange, chipWords, clean, cleanupWords, folderExample, LOOK_AGAIN_REQUEST, RESCAN_REQUEST, SCAN_REQUEST, soundRequest, statusNamesProject, undoRequest } from '../shared/format.ts'
import { useFirstOpen } from '../shared/atlas/first.ts'
import { type DesignResult, madeNameIn, sourceOfResult, useRequest } from '../shared/design.ts'
import { soundEnd, useGrow } from '../shared/grow.ts'
import { Audition, useKumi, usePlaying, useWant } from '../shared/hooks.ts'
import { FocusView, HistoryView } from '../shared/HistoryView.tsx'
import { translator } from '../shared/i18n.ts'
import { MakePanel } from '../shared/MakePanel.tsx'
import { MapCanvas } from '../shared/MapCanvas.tsx'
import { glass, ToolButton } from '../shared/mapUi.tsx'
import { itemReference, kitRequest } from '../shared/palette.ts'
import { PaletteDock } from '../shared/PaletteDock.tsx'
import { usePaletteMap } from '../shared/paletteMap.ts'
import { SampleFolders, useSampleFolders } from '../shared/SampleFolders.tsx'
import { KumiStore } from '../shared/store.ts'
import { nextSteps, setupWords } from '../shared/steps.ts'
import type { AudioReply } from '../shared/types.ts'
import { Dot, FocusStyle, I18n, S, Section, TYPE, useT } from '../shared/ui.tsx'
import { SDK } from './react.ts'

const BASE = '/api/plugins/kumi'
const POLL_MS = 5000
/** The browser's own key for hover-to-play: on unless the producer turned it off, as native Kumi's `h`. */
const HOVER_PLAY = 'kumi.mapHover'
/** The browser's own key for the square-tile view: off until the producer turns it on. */
const TILES = 'kumi.mapTiles'
/** The browser's own key for whether the palette drawer is open: open until the producer folds it. */
const PALETTE_OPEN = 'kumi.paletteOpen'
/** The browser's own key for the map's layout: similarity ('') until the producer picks another. */
const LAYOUT = 'kumi.mapLayout'
/** The browser's own key for the map's first-open steps the producer has done or skipped: none at first. */
const FIRST_OPEN = 'kumi.mapFirst'

function useNow(every = 30_000): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), every)
    return () => clearInterval(timer)
  }, [every])
  return now
}

function CopyButton({ text, label, kind = 'secondary' }: { text: string; label: string; kind?: 'primary' | 'quiet' | 'secondary' }) {
  const t = useT()
  const [done, setDone] = useState(false)
  return (
    <Button
      kind={kind}
      onClick={() => {
        void navigator.clipboard?.writeText(text).then(() => {
          setDone(true)
          setTimeout(() => setDone(false), 1500)
        })
      }}
      tip={t('dashboard.copyTitle', text)}
    >
      {done ? t('dashboard.copied') : label}
    </Button>
  )
}

/** The dashboard's language (its own i18n context), for Kumi's words. */
function KumiDashboard() {
  const locale = (SDK.useI18n?.() as { locale?: unknown } | undefined)?.locale
  const t = useMemo(() => translator(locale), [locale])
  return (
    <I18n.Provider value={t}>
      <FocusStyle />
      <Dashboard />
    </I18n.Provider>
  )
}

function Dashboard() {
  const t = useT()
  // Reads, and the backend's few writes (the palette, the sounds hidden from the map) as JSON POSTs.
  const store = useMemo(
    () =>
      new KumiStore((path, init) =>
        SDK.fetchJSON(`${BASE}${path}`, init && { method: init.method, headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(init.body) })
      ),
    []
  )
  const audition = useMemo(() => new Audition(id => SDK.fetchJSON<AudioReply>(`${BASE}/audio?id=${encodeURIComponent(id)}`)), [])
  useWant(store, 'map')
  useWant(store, 'palette')
  const state = useKumi(store)
  // A made sound by the name its palette gives it (a later variation of one recipe numbered), for Use and as a source.
  const madeName = (result: DesignResult) => madeNameIn(state.palette?.current?.items ?? [], result)
  const playing = usePlaying(audition)
  const now = useNow()
  const [selected, setSelected] = useState<null | string>(null)
  const [hover, setHover] = useState(() => globalThis.localStorage?.getItem(HOVER_PLAY) !== 'off')
  const [tiles, setTiles] = useState(() => globalThis.localStorage?.getItem(TILES) === 'on')
  const [paletteOpen, setPaletteOpen] = useState(() => globalThis.localStorage?.getItem(PALETTE_OPEN) !== 'off')
  const [layout, setLayout] = useState(() => globalThis.localStorage?.getItem(LAYOUT) ?? '')
  // A sound that can't play here leaves the map quietly; nothing else a play meets is shown here either.
  const palette = usePaletteMap(store, state, audition, t, { hover, left: id => setSelected(current => (current === id ? null : current)) })
  // The map's first-open steps, kept where the drawer's fold is. Make… pressed is seen here; the map sees the others.
  const first = useFirstOpen(
    () => globalThis.localStorage?.getItem(FIRST_OPEN),
    kept => globalThis.localStorage?.setItem(FIRST_OPEN, kept)
  )
  const makeOpen = palette.making !== null
  useEffect(() => {
    if (makeOpen) first.done(['make'])
  }, [makeOpen]) // eslint-disable-line react-hooks/exhaustive-deps
  // Grow: its renders go through the store; the handle's point plays as soon as it's grown.
  const grow = useGrow(request => store.design(request), result => void audition.play(result.artifact_id).catch(() => {}), state.map?.revision ?? null)
  // The dock's box: the Make panel flies a kept sound into it.
  const dock = useRef<HTMLElement | null>(null)
  // Sample folders: the folders now, and adding one by its path. The dashboard can't send a request, so it copies it.
  const folders = useSampleFolders(store, state.map?.revision)
  const [foldersOpen, setFoldersOpen] = useState(false)
  const folderControl = (onClose?: () => void) => (
    <SampleFolders
      ask={(folder, known) =>
        navigator.clipboard?.writeText(known ? RESCAN_REQUEST : addFolderRequest(folder)).then(
          () => 'copied' as const,
          () => 'none' as const
        ) ?? Promise.resolve('none' as const)
      }
      check={typed => store.checkFolder(typed)}
      copies
      example={folderExample(state.setup?.target)}
      folders={folders}
      onClose={onClose}
    />
  )

  useEffect(() => {
    void store.refreshStatus()
    const timer = setInterval(() => {
      if (document.visibilityState !== 'hidden') void store.refreshStatus()
    }, POLL_MS)
    return () => {
      clearInterval(timer)
      audition.dispose()
    }
  }, [store, audition])

  // One home per fact, as in Desktop's pane: the tempo is FOCUS's; the project is the status line's while it names it.
  const words = chipWords(state.status, undefined, t)
  const tone = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint }[words.tone]
  const connected = Boolean(state.status?.connected)

  if (state.available === false) {
    return (
      <div style={{ padding: 24, color: T.text }}>
        <p style={S.text}>{t('dashboard.agentOff')}</p>
        <p style={S.muted}>{t('dashboard.agentOffHow')}</p>
      </div>
    )
  }

  // Setup's words and next steps, as the pane has them: one step per DAW until one is chosen, none that setup can't
  // take (no pin, an unsupported system), and no words that name a DAW nobody chose.
  const setupMessage = setupWords(state)
  const steps = nextSteps(state, t)
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12, color: T.text, minHeight: 'calc(100vh - 160px)' }}>
      <div style={{ ...S.row, gap: 10, padding: '4px 2px' }}>
        <Dot beat={connected ? beatSeconds(state.focus?.transport) : null} color={tone} size={9} />
        <span style={{ ...TYPE.body, fontWeight: 600 }}>{words.text}</span>
        {state.status?.runtime?.version && <span style={S.faint}>Kumi {state.status.runtime.version}</span>}
        {setupMessage && <span style={S.muted}>{setupMessage}</span>}
        {/* Setup's next step, to paste into a chat: the dashboard can't send one itself. */}
        {steps.map(step => (
          <CopyButton key={step.request} label={step.label} text={step.request} />
        ))}
      </div>
      {state.status?.unsure && (
        <div style={{ ...S.row, gap: 10, padding: '0 2px', alignItems: 'flex-start' }}>
          <span style={{ ...S.muted, color: T.warn, flex: 1 }}>
            {[
              t('pane.unsure', clean(state.status.unsure.op ?? t('pane.unsureOp'), 40), state.status.unsure.reason ? clean(state.status.unsure.reason, 160) : ''),
              cleanupWords(state.status.unsure.cleanup, t),
              t('dashboard.askLook')
            ]
              .filter(Boolean)
              .join(' ')}
          </span>
          <CopyButton label={t('dashboard.copyRequest')} text={LOOK_AGAIN_REQUEST} />
        </div>
      )}
      <div style={{ display: 'grid', gridTemplateColumns: 'minmax(260px, 1fr) minmax(320px, 2fr)', gap: 12, alignItems: 'stretch' }}>
        <div style={{ border: `1px solid ${T.hairline}`, borderRadius: 6, background: T.card, overflow: 'hidden', alignSelf: 'start' }}>
          <Section title={t('pane.focus')}>
            <FocusView focus={state.focus} loading={state.focus === null && !state.failed.focus} now={now} project={!statusNamesProject(state.status)} />
          </Section>
          <Section title={t('pane.history')}>
            <HistoryView
              action={(head, rows) =>
                // One Copy undo per change, on its head, as in Desktop's pane (CC-3 A2).
                canUndoChange(state.status, rows) ? <CopyButton label={t('dashboard.copyUndo')} text={undoRequest(head)} /> : null
              }
              daw={state.status?.daw}
              history={state.history}
              loading={state.history === null && !state.failed.history}
              live={Boolean(state.status?.connected) && state.status?.stale !== true}
              now={now}
              target={state.setup?.target}
            />
          </Section>
        </div>
        {/* Tall enough that the palette drawer, open, still leaves the map most of the card. The page scrolls, so the
            height costs nothing. */}
        <div style={{ border: `1px solid ${T.hairline}`, borderRadius: 6, background: T.card, overflow: 'hidden', minHeight: 780, display: 'flex', flexDirection: 'column', position: 'relative' }}>
          {/* Sample folders, over the map's top right corner once there's a map (an empty map shows it in place). */}
          {foldersOpen && (state.map?.points?.length ?? 0) > 0 && (
            <div
              onKeyDown={event => {
                if (event.key !== 'Escape') return
                event.stopPropagation()
                setFoldersOpen(false)
              }}
              style={{ ...glass, position: 'absolute', top: 56, right: 12, width: 'min(380px, calc(100% - 24px))', padding: 12, zIndex: 6 }}
            >
              {folderControl(() => setFoldersOpen(false))}
            </div>
          )}
          <MapCanvas
            audio={id => audition.url(id)}
            onStop={() => audition.stop()}
            actions={(id, sound) => {
              const request = soundRequest(id, sound.name, state.map?.revision)
              return (
                <>
                  <Button disabled={palette.marked.has(id)} onClick={() => palette.gather([id])} tip={t('dock.addTitle')}>
                    {palette.marked.has(id) ? t('dock.inPalette') : t('dock.add')}
                  </Button>
                  {request && <CopyButton label={t('dashboard.copyUse')} text={request} />}
                </>
              )
            }}
            empty={
              state.map === null ? (
                state.failed.map ? (
                  <>
                    {t('map.failed', state.failed.map)}
                    <div style={{ marginTop: 10, display: 'flex', justifyContent: 'center' }}>
                      <Button onClick={() => store.refresh('map')}>{t('map.retry')}</Button>
                    </div>
                  </>
                ) : (
                  t('map.reading')
                )
              ) : ['needs_runtime', 'outdated_runtime', 'damaged_runtime'].includes(state.setup?.state ?? '') ? (
                // No runtime yet: setup comes first (the dashboard's header says how).
                t('map.setupFirst')
              ) : (
                <>
                  {t('dashboard.mapEmpty')}
                  <div style={{ ...glass, marginTop: 12, padding: 12 }}>{folderControl()}</div>
                  <div style={{ marginTop: 10 }}>
                    <CopyButton kind="primary" label={t('map.scanCopy')} text={SCAN_REQUEST} />
                  </div>
                </>
              )
            }
            found={state.found}
            hiddenActions={
              <>
                <Button kind="quiet" onClick={() => store.showHidden()} tip={t('map.showHiddenTitle')}>
                  {t('map.showHidden')}
                </Button>
                <CopyButton kind="quiet" label={t('map.rescan')} text={RESCAN_REQUEST} />
              </>
            }
            leaving={state.leaving}
            map={state.map}
            marked={palette.marked}
            onHover={palette.onHover}
            trail={hover}
            kin={palette.kin}
            first={{ kept: first.kept, onDone: first.done }}
            onPick={id => {
              setSelected(id)
              palette.play(id)
            }}
            onPicked={palette.onPicked}
            grow={grow}
            onPlayMade={result => void audition.play(result.artifact_id).catch(() => {})}
            growActions={result => {
              const request = useRequest(result, madeName(result))
              const kept = palette.kept.has(result.artifact_id)
              return (
                <>
                  {/* From the grown point and the vine's far end: two sounds, so every way of making one is open. */}
                  <Button kind="quiet" onClick={() => palette.makeFrom([sourceOfResult(result, madeName(result)), ...(grow.to ? [grow.to.source] : [])])} tip={t('make.openTitle')}>
                    {t('make.open')}
                  </Button>
                  <span style={{ flex: 1 }} />
                  <Button disabled={kept} onClick={() => palette.keep(result)} tip={kept ? undefined : t('make.keepTitle')}>
                    {kept ? t('make.kept') : t('make.keep')}
                  </Button>
                  {request && <CopyButton label={t('dashboard.copyUse')} text={request} />}
                </>
              )
            }}
            onSelect={setSelected}
            picked={palette.picked}
            pickedActions={ids => (
              <>
                <Button kind="primary" onClick={() => palette.gather(ids)} tip={t('dock.addTitle')}>
                  {t('dock.addPicked', ids.length)}
                </Button>
                {ids.length > 1 && (
                  <Button onClick={() => palette.makeFromPicks(ids)} tip={t('make.openTitle')}>
                    {t('make.open')}
                  </Button>
                )}
                {ids.length === 2 &&
                  (() => {
                    const [from, to] = ids.map(id => soundEnd(id, state.map?.sounds[id]?.name, state.map?.revision ?? null))
                    return from && to ? (
                      // The vine takes over from the picks: their actions would float over its card.
                      <Button
                        onClick={() => {
                          grow.between(from, to)
                          palette.clearPicks()
                        }}
                        tip={t('grow.betweenTitle')}
                      >
                        {t('grow.between')}
                      </Button>
                    ) : null
                  })()}
                <Button kind="quiet" onClick={palette.clearPicks} tip={t('dock.clearPicksTitle')}>
                  {t('dock.clearPicks', ids.length)}
                </Button>
              </>
            )}
            playing={playing}
            selected={selected}
            flying={palette.flying}
            onTiles={on => {
              globalThis.localStorage?.setItem(TILES, on ? 'on' : 'off')
              setTiles(on)
            }}
            tiles={tiles}
            arrangement={layout}
            onArrangement={id => {
              globalThis.localStorage?.setItem(LAYOUT, id)
              setLayout(id)
            }}
            tools={
              <>
                <ToolButton
                  icon="hover"
                  label={t('dock.hover')}
                  onClick={() => {
                    globalThis.localStorage?.setItem(HOVER_PLAY, hover ? 'off' : 'on')
                    setHover(!hover)
                  }}
                  pressed={hover}
                  title={t('dock.hoverTitle')}
                />
                <ToolButton icon="folder" label={t('folders.title')} onClick={() => setFoldersOpen(!foldersOpen)} pressed={foldersOpen} title={t('folders.openTitle')} />
              </>
            }
          />
          {/* The palette's strip waits for a map, as in Desktop. */}
          {(state.map?.points?.length ?? 0) > 0 && (
            <PaletteDock
              audio={id => audition.url(id)}
              groups={state.map?.groups}
              kit={current => <CopyButton kind="quiet" label={t('dock.kitCopy')} text={kitRequest(current.name)} />}
              landing={dock}
              lineage={palette.madeLineage}
              onKin={palette.showKinOf}
              make={palette.makeFromPalette}
              onDo={action => store.paletteDo(action)}
              onOpen={open => {
                globalThis.localStorage?.setItem(PALETTE_OPEN, open ? 'on' : 'off')
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
                return request ? <CopyButton kind="quiet" label={t('dashboard.copyUse')} text={request} /> : null
              }}
            />
          )}
        </div>
        {/* Under the map card, in the grid's own column (the page scrolls): the map keeps its whole height, and the
            dock sits between them, so a kept sound lands in view. */}
        {palette.making && (
          <div style={{ gridColumn: 2, minWidth: 0 }}>
            <MakePanel
              colorOf={palette.colorOf}
              lineage={result => palette.lineage(result.sources, result.at)}
              onKin={result => palette.showKin(result ? result.sources : null)}
              dock={dock}
              placement="inline"
              kept={palette.kept}
              onClose={palette.closeMake}
              onKeep={palette.keep}
              onMake={request => store.design(request)}
              onSources={palette.makeFrom}
              onPlay={id => void audition.play(id).catch(() => {})}
              onStop={() => audition.stop()}
              playing={playing}
              results={state.designs}
              sources={palette.making}
              tempo={state.focus?.transport?.tempo}
              use={result => {
                const request = useRequest(result, madeName(result))
                return request ? <CopyButton label={t('dashboard.copyUse')} text={request} /> : null
              }}
            />
          </div>
        )}
      </div>
    </div>
  )
}

window.__HERMES_PLUGINS__?.register('kumi', KumiDashboard)
