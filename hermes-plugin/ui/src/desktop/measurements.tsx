// The Measurements tab: the latest `kumi_listen` result, drawn as the transcript's card would draw it, for when the
// transcript shows no tool parts (tool progress off, focus view) and a `listen.done` event brought it (CC-9).
import { T } from '@kumi/theme'

import { ago } from '../shared/format.ts'
import { type Audition, useKumi } from '../shared/hooks.ts'
import type { KumiStore } from '../shared/store.ts'
import { S } from '../shared/ui.tsx'
import { ListenResultCard } from './card.tsx'

export function MeasurementsWorkspace({ store, audition }: { store: KumiStore; audition: Audition }) {
  const { measured } = useKumi(store)
  const note = (text: string) => <span style={S.muted}>{text}</span>
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 10, padding: 16, color: T.text, overflowY: 'auto', height: '100%' }}>
      {measured === null
        ? note('Nothing measured yet. When Kumi measures audio in your chat, its numbers show here as well.')
        : measured.result === null
          ? note("Kumi measured it, but there's too much to show here: the numbers are in its reply in the chat.")
          : (
              <>
                <span style={S.faint}>{`Measured ${ago(measured.at)}`}</span>
                <ListenResultCard audition={audition} fallback={note('Kumi measured it: the numbers are in its reply in the chat.')} result={measured.result} />
              </>
            )}
    </div>
  )
}
