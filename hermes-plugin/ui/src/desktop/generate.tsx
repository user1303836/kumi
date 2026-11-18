// The inline card for what Kumi had made through Mirelo (`kumi_job`, once Desktop ties its `MEDIA:` line to the call):
// what was asked, the file, a waveform for audio, what it cost against what was reserved, the producer's cap, and
// one request to finish in the chat (CC-13). Nothing here spends, places or retries anything: the paid step was
// Hermes's approval, and placing is a change the producer approves.
import type { PluginContext, ToolMediaRendererProps } from '@hermes/plugin-sdk'
import { Button, Tip } from '@kumi/controls'
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'

import { capWords, costWords, generationRequest, generationTitle, lengthWords, outputKind, parseGeneration } from '../shared/generate.ts'
import type { Audition } from '../shared/hooks.ts'
import type { KumiStore } from '../shared/store.ts'
import { Chip, S, useT } from '../shared/ui.tsx'
import { ask } from './actions.ts'
import { Wave } from './card.tsx'
import { ownerSession } from './pane.tsx'

const OUTPUTS = new Set(['midi', 'notes', 'musicxml'])
const ENDED = new Set(['failed', 'cancelled', 'interrupted'])

export function GenerateCard({ media, audition, ctx, store }: { media: ToolMediaRendererProps; audition: Audition; ctx: PluginContext; store: KumiStore }): ReactNode {
  const t = useT()
  const made = parseGeneration(media.result)
  // Anything this card can't read (a scan's answer, say) keeps Desktop's own player or chip.
  if (!made) return media.fallback
  const title = generationTitle(made, t)
  const kind = outputKind(made)
  const cost = costWords(made, t)
  const left = capWords(made, t)
  const length = lengthWords(made, t)
  const offer = generationRequest(made, t)
  return (
    <div
      data-kumi-card="generate"
      style={{ display: 'flex', flexDirection: 'column', gap: 8, maxWidth: 560, padding: '10px 12px', borderRadius: 6, border: `1px solid ${T.hairline}`, background: T.card, color: T.text }}
    >
      <div style={{ ...S.row, justifyContent: 'space-between', alignItems: 'flex-start', gap: 8 }}>
        <span style={{ ...S.text, fontWeight: 600, minWidth: 0, whiteSpace: 'normal' }}>
          <span style={{ color: T.accent }}>{kind === 'notes' ? '♫' : '♪'}</span> {made.prompt ? `“${title}”` : title}
        </span>
        <span style={{ ...S.row, gap: 4, flexShrink: 0 }}>
          {made.outputType && OUTPUTS.has(made.outputType) && <Chip>{t(`generate.outputs.${made.outputType}`)}</Chip>}
          {made.noteCount !== null && <Chip>{t('generate.notes', made.noteCount, String(made.noteCount))}</Chip>}
          {length && <Chip>{length}</Chip>}
          {ENDED.has(made.state) ? (
            <Chip tone="bad">{t(`generate.states.${made.state}`)}</Chip>
          ) : made.state === 'running' ? (
            <Chip>{t('generate.states.running')}</Chip>
          ) : (
            // A span for Hermes's Tip to hold: a chip passes no props on.
            <Tip label={t('generate.generatedTitle')}>
              <span>
                <Chip tone="accent">{t('generate.generated')}</Chip>
              </span>
            </Tip>
          )}
        </span>
      </div>
      {made.file && <span style={{ ...S.faint, ...S.ellipsis }}>{made.file}</span>}
      {kind === 'audio' && made.artifactId && media.file?.kind === 'audio' ? (
        // In a row, as in the listen card: the waveform's flex basis is a width there, not a height.
        <div style={{ display: 'flex' }}>
          <Wave audition={audition} caption="" id={made.artifactId} label={title} />
        </div>
      ) : (
        // Notes, MIDI and MusicXML: Desktop's own file chip, to open or save it.
        media.fallback
      )}
      {ENDED.has(made.state) && made.error?.reason && <span style={{ ...S.muted, whiteSpace: 'normal' }}>{made.error.reason}</span>}
      {(cost || left) && (
        <span style={{ ...S.row, gap: 6, flexWrap: 'wrap' }}>
          {cost && (
            <Tip label={t('generate.creditsTitle')}>
              <span>
                <Chip>{cost}</Chip>
              </span>
            </Tip>
          )}
          {left && (
            <Tip label={t('generate.capTitle')}>
              <span style={S.faint}>{left}</span>
            </Tip>
          )}
        </span>
      )}
      {made.ledger?.frozen && <span style={{ ...S.text, color: T.warn, whiteSpace: 'normal' }}>{t('generate.frozen')}</span>}
      {offer && (
        <span>
          <Button kind="primary" onClick={() => ask(ctx, offer.request, { send: false, session: ownerSession(store.get()) })} tip={offer.title}>
            {offer.label}
          </Button>
        </span>
      )}
    </div>
  )
}
