// Under the composer of Kumi's own chat: what Kumi sees right now (FOCUS), so "this track" means the same to the
// producer and to Kumi before they send. It shows in no other chat, and it does nothing: it's words, not a control.
import { host } from '@hermes/plugin-sdk'
import { T } from '@kumi/theme'
import { useCallback, useSyncExternalStore } from 'react'

import { bpm, colorOf, focusPath } from '../shared/format.ts'
import { useKumi, useWant } from '../shared/hooks.ts'
import type { KumiStore } from '../shared/store.ts'
import { Dot, S } from '../shared/ui.tsx'
import { ownerSession } from './pane.tsx'

const noChange = () => () => {}

/** The stored id of the chat whose composer is focused; null on a Desktop that doesn't say. */
function useFocusedChat(): null | string {
  const atom = host.state.focusedStoredSessionId
  const subscribe = useCallback(
    (fn: () => void) => (atom ? (atom.listen ? atom.listen(() => fn()) : atom.subscribe(() => fn())) : noChange()),
    [atom]
  )
  const read = () => atom?.get() ?? null
  return useSyncExternalStore(subscribe, read, read)
}

export function FocusHint({ store }: { store: KumiStore }) {
  useWant(store, 'focus')
  const state = useKumi(store)
  const chat = useFocusedChat()
  const owner = ownerSession(state)
  if (!state.status?.connected || !owner || chat !== owner) return null
  const path = focusPath(state.focus)
  if (!path.length) return null
  const tempo = bpm(state.focus?.transport?.tempo)
  return (
    <div
      style={{ ...S.row, gap: 6, padding: '2px 10px', maxWidth: '100%' }}
      title="What Kumi sees selected in your DAW: words like “this track” in this chat mean it."
    >
      <Dot color={colorOf(state.focus?.focus?.track) ?? T.faint} size={7} />
      <span style={{ ...S.faint, ...S.ellipsis }}>
        Kumi sees {path.join(' › ')}
        {tempo ? ` · ${tempo}` : ''}
      </span>
    </div>
  )
}
