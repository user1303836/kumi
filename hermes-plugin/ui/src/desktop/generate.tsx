// The inline card for a sound Kumi had made (`kumi_generate`, then `kumi_job` with its `MEDIA:` line): the file and
// a waveform to audition, the words it was made from, what it cost against its estimate, what's left under the
// producer's cap, and Place…, which puts a request in the chat. Nothing here spends, places or changes anything: the
// paid step was Hermes's approval, and placing is a `kumi_change` the producer approves.
import type { PluginContext, ToolMediaRendererProps } from '@hermes/plugin-sdk'
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'

import { placeRequest, seconds } from '../shared/format.ts'
import { capWords, costWords, parseGenerated } from '../shared/generate.ts'
import type { Audition } from '../shared/hooks.ts'
import type { KumiStore } from '../shared/store.ts'
import { Chip, S, TextButton, useT } from '../shared/ui.tsx'
import { ask } from './actions.ts'
import { Wave } from './card.tsx'
import { ownerSession } from './pane.tsx'

export function GenerateCard({ media, audition, ctx, store }: { media: ToolMediaRendererProps; audition: Audition; ctx: PluginContext; store: KumiStore }): ReactNode {
  const t = useT()
  const made = parseGenerated(media.result)
  // Anything this card can't read keeps Desktop's own player or file chip.
  if (!made) return media.fallback
  const request = made.artifactId ? placeRequest(made.artifactId, made.file) : null
  const cost = costWords(made, t)
  const left = capWords(made, t)
  const audio = media.file?.kind === 'audio'
  return (
    <div
      data-kumi-card="generate"
      style={{ display: 'flex', flexDirection: 'column', gap: 8, maxWidth: 560, padding: '10px 12px', borderRadius: 6, border: `1px solid ${T.border}`, background: T.card, color: T.text }}
    >
      <div style={{ ...S.row, justifyContent: 'space-between', alignItems: 'flex-start', gap: 8 }}>
        <span style={{ ...S.text, fontWeight: 600, minWidth: 0 }}>
          <span style={{ color: T.accent }}>{audio ? '♪' : '▤'}</span> {made.file}
        </span>
        <span style={{ ...S.row, gap: 4 }}>
          {made.kind === 'audio_to_midi' && <Chip>{t('generate.midi')}</Chip>}
          {made.seconds !== null && <Chip>{seconds(made.seconds)}</Chip>}
          <Chip title={t('generate.generatedTitle', made.provider)} tone="accent">
            {t('generate.generated')}
          </Chip>
        </span>
      </div>
      {made.prompt && <span style={{ ...S.muted, whiteSpace: 'normal' }}>“{made.prompt}”</span>}
      {audio && made.artifactId ? (
        // In a row, as in the listen card: the waveform's flex basis is a width there, not a height.
        <div style={{ display: 'flex' }}>
          <Wave audition={audition} caption="" id={made.artifactId} label={made.file} />
        </div>
      ) : (
        media.fallback
      )}
      {(cost || left) && (
        <span style={{ ...S.row, gap: 6, flexWrap: 'wrap' }}>
          {cost && <Chip>{cost}</Chip>}
          {left && (
            <span style={S.faint} title={t('generate.capTitle')}>
              {left}
            </span>
          )}
        </span>
      )}
      {request && (
        <span>
          <TextButton onClick={() => void ask(ctx, request, { send: false, session: ownerSession(store.get()) })} strong title={t('generate.placeTitle')}>
            {t('generate.place')}
          </TextButton>
        </span>
      )}
    </div>
  )
}
