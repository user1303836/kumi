// The sound-map workspace: Kumi's map of the approved sample folders. A click auditions a sound here, in the window
// (a read, nothing reaches the DAW); "Use" puts a request naming that sound into the chat for the producer to finish.
import type { PluginContext } from '@hermes/plugin-sdk'
import { host } from '@hermes/plugin-sdk'
import { useState } from 'react'

import { soundRequest } from '../shared/format.ts'
import { type Audition, useKumi, usePlaying, useWant } from '../shared/hooks.ts'
import { MapCanvas } from '../shared/MapCanvas.tsx'
import type { KumiStore } from '../shared/store.ts'
import { TextButton } from '../shared/ui.tsx'
import { ask } from './actions.ts'
import { ownerSession } from './pane.tsx'

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
  useWant(store, 'map')
  const state = useKumi(store)
  const playing = usePlaying(audition)
  const [selected, setSelected] = useState<null | string>(null)
  const play = (id: string) => {
    audition.play(id).catch(error => host.notify({ kind: 'warning', message: `Kumi couldn't play that sound: ${errorText(error)}` }))
  }
  const map = state.map
  const empty =
    state.available === false
      ? "Kumi's agent half is off in this profile, so there's no map to show."
      : map === null
        ? 'Reading the sound map…'
        : "No sound map yet. Add a sample folder in Kumi's settings and ask Kumi to scan it; the map is made from what it finds there."
  return (
    <MapCanvas
      actions={(id, sound) => {
        const request = soundRequest(id, sound.name)
        return (
          <>
            <TextButton onClick={() => (playing === id ? audition.stop() : play(id))} title="Play it here (nothing changes in your DAW)">
              {playing === id ? '■ Stop' : '▶ Play'}
            </TextButton>
            {request && (
              <TextButton
                onClick={() => void ask(ctx, request, { send: false, session: ownerSession(state) })}
                strong
                title="Put a request to use this sound in the chat, to finish and send"
              >
                Use…
              </TextButton>
            )}
          </>
        )
      }}
      empty={empty}
      found={state.found}
      map={map}
      onPick={id => {
        setSelected(id)
        play(id)
      }}
      playing={playing}
      selected={selected}
    />
  )
}
