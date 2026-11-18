// Kumi's background jobs in the pane: a library scan or a generation while it runs, with how far it got, then for a
// minute how it ended. A job is something Kumi was asked to do in the chat; this only shows it. A finished generation
// offers its one request (Place… or Plan notes…), left unsent in the chat, as its transcript card does (CC-13): with
// tool progress off, this row is where it shows.
import { Button, Tip } from '@kumi/controls'
import { T } from '@kumi/theme'

import { jobWords } from '../shared/format.ts'
import { generationJobWords, generationRequest, parseGeneration } from '../shared/generate.ts'
import type { TrackedJob } from '../shared/store.ts'
import { S, TYPE, useT } from '../shared/ui.tsx'

const TONE = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint } as const

export function JobsView({ jobs, onRequest }: { jobs: Record<string, TrackedJob>; onRequest?: (request: string) => unknown }) {
  const t = useT()
  const shown = Object.entries(jobs)
  if (!shown.length) return null
  return (
    <ul style={{ listStyle: 'none', margin: 0, padding: 0, display: 'flex', flexDirection: 'column', gap: 8 }}>
      {shown.map(([id, job]) => {
        const generation = job.kind === 'generate' ? parseGeneration(job) : null
        const words = generation ? generationJobWords(job, t) : jobWords(job, t)
        const offer = generation && onRequest ? generationRequest(generation, t) : null
        return (
          <li key={id} style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            <span style={{ ...S.row, gap: 6 }}>
              <span style={{ ...TYPE.body, color: job.ended_at === null ? T.text : TONE[words.tone] }}>{words.title}</span>
              {words.detail && (
                // A generation's detail names credits: the tooltip says they're Mirelo API credits, not money (13d).
                <Tip label={generation ? t('generate.creditsTitle') : undefined}>
                  <span style={{ ...TYPE.small, ...S.ellipsis, color: T.faint }}>{words.detail}</span>
                </Tip>
              )}
            </span>
            {job.ended_at === null && (
              <span
                aria-label={words.fraction === null ? t('job.underWay', words.title) : t('job.percent', words.title, Math.round(words.fraction * 100))}
                // No value while the job can't say how far along it is: an indeterminate bar.
                aria-valuemax={100}
                aria-valuemin={0}
                aria-valuenow={words.fraction === null ? undefined : Math.round(words.fraction * 100)}
                role="progressbar"
                style={{ display: 'block', height: 3, borderRadius: 2, background: T.border, overflow: 'hidden' }}
              >
                <span
                  // While the job can't say how far it is, a segment slides along the track, so the bar never reads as stuck
                  // at a number (a system asking for less motion sees it still, beside the words).
                  className={words.fraction === null ? 'kumi-slide' : undefined}
                  style={{
                    display: 'block',
                    height: '100%',
                    width: words.fraction === null ? '30%' : `${Math.max(2, Math.round(words.fraction * 100))}%`,
                    background: T.accent,
                    opacity: words.fraction === null ? 0.5 : 1
                  }}
                />
              </span>
            )}
            {offer && (
              <span>
                <Button onClick={() => onRequest?.(offer.request)} tip={offer.title}>
                  {offer.label}
                </Button>
              </span>
            )}
          </li>
        )
      })}
    </ul>
  )
}
