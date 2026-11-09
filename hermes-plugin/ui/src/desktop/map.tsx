// The sound-map workspace: Kumi's map of the approved sample folders. A click auditions a sound here, in the window
// (a read, nothing reaches the DAW); "Use" puts a request naming that sound into the chat for the producer to finish,
// and "Use on pad…" one naming a drum pad too. A sound that can't play here leaves the map quietly (never a notice per
// sound), and one line under the map counts them.
import type { PluginContext } from '@hermes/plugin-sdk'
import { host } from '@hermes/plugin-sdk'
import { T } from '@kumi/theme'
import { useEffect, useRef, useState } from 'react'

import { liveNote, nearRequest, padRequest, RESCAN_REQUEST, soundRequest } from '../shared/format.ts'
import { type Audition, playFailure, useKumi, usePlaying, useWant } from '../shared/hooks.ts'
import { MapCanvas } from '../shared/MapCanvas.tsx'
import type { KumiStore } from '../shared/store.ts'
import { FOCUSABLE, TextButton, useT } from '../shared/ui.tsx'
import { ask } from './actions.ts'
import { ownerSession } from './pane.tsx'

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

export function errorText(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error ?? '')
  // Desktop's ctx.rest rejects with "<status>: <body>"; FastAPI's body says why in `detail`.
  const json = text.indexOf('{')
  if (json >= 0) {
    try {
      const body: unknown = JSON.parse(text.slice(json))
      if (body && typeof body === 'object' && 'detail' in body && typeof body.detail === 'string') return body.detail
    } catch {
      // not JSON: the text as it came
    }
  }
  return text || 'no answer'
}

export function MapWorkspace({ ctx, store, audition }: { ctx: PluginContext; store: KumiStore; audition: Audition }) {
  const t = useT()
  useWant(store, 'map')
  const state = useKumi(store)
  const playing = usePlaying(audition)
  const [selected, setSelected] = useState<null | string>(null)
  // The sound whose pad picker is open: one at a time, and it closes when another sound is picked.
  const [padsFor, setPadsFor] = useState<null | string>(null)
  const play = (id: string) => {
    audition.play(id).catch(error => {
      const why = playFailure(error)
      if (why === 'gone' || why === 'unplayable') {
        store.hide(id, why)
        setSelected(current => (current === id ? null : current))
      } else if (why === 'other') {
        host.notify({ kind: 'warning', message: t('map.playFailed', errorText(error)) })
      }
    })
  }
  const map = state.map
  const empty = state.available === false ? t('map.agentOff') : map === null ? t('map.reading') : t('map.empty')
  return (
    <MapCanvas
      actions={(id, sound) => {
        const request = soundRequest(id, sound.name, map?.revision)
        const near = nearRequest(id, sound.name, map?.revision)
        return (
          <>
            <TextButton onClick={() => (playing === id ? audition.stop() : play(id))} title={t('map.playTitle')}>
              {playing === id ? t('map.stop') : t('map.play')}
            </TextButton>
            {near && (
              <TextButton onClick={() => void ask(ctx, near, { send: false, session: ownerSession(state) })} title={t('map.moreLikeTitle')}>
                {t('map.moreLike')}
              </TextButton>
            )}
            {request && (
              <TextButton onClick={() => setPadsFor(padsFor === id ? null : id)} pressed={padsFor === id} title={t('map.useOnPadTitle')}>
                {t('map.useOnPad')}
              </TextButton>
            )}
            {request && (
              <TextButton
                onClick={() => void ask(ctx, request, { send: false, session: ownerSession(state) })}
                strong
                title={t('map.useTitle')}
              >
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
                  if (onPad) void ask(ctx, onPad, { send: false, session: ownerSession(state) })
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
          <TextButton onClick={() => void ask(ctx, RESCAN_REQUEST, { send: false, session: ownerSession(state) })} title={t('map.rescanTitle')}>
            {t('map.rescan')}
          </TextButton>
        </>
      }
      leaving={state.leaving}
      map={map}
      onPick={id => {
        setSelected(id)
        play(id)
      }}
      onSelect={id => {
        setSelected(id)
        if (id !== padsFor) setPadsFor(null)
      }}
      playing={playing}
      selected={selected}
    />
  )
}
