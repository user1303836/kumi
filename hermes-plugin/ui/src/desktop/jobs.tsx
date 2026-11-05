// Kumi's background jobs in the pane: a library scan while it runs, with how far it got, then for a minute how it
// ended. A job is something Kumi was asked to do in the chat; this only shows it, and changes nothing.
import { T } from '@kumi/theme'

import { jobWords } from '../shared/format.ts'
import type { TrackedJob } from '../shared/store.ts'
import { S, useT } from '../shared/ui.tsx'

const TONE = { good: T.good, warn: T.warn, bad: T.bad, muted: T.faint } as const

export function JobsView({ jobs }: { jobs: Record<string, TrackedJob> }) {
  const t = useT()
  const shown = Object.entries(jobs)
  if (!shown.length) return null
  return (
    <ul style={{ listStyle: 'none', margin: 0, padding: 0, display: 'flex', flexDirection: 'column', gap: 6 }}>
      {shown.map(([id, job]) => {
        const words = jobWords(job, t)
        return (
          <li key={id} style={{ display: 'flex', flexDirection: 'column', gap: 3 }}>
            <span style={{ ...S.row, gap: 6 }}>
              <span style={{ ...S.text, color: job.ended_at === null ? T.text : TONE[words.tone] }}>{words.title}</span>
              {words.detail && <span style={{ ...S.faint, ...S.ellipsis }}>{words.detail}</span>}
            </span>
            {job.ended_at === null && (
              <span
                aria-label={words.fraction === null ? t('job.underWay', words.title) : t('job.percent', words.title, Math.round(words.fraction * 100))}
                role="progressbar"
                style={{ display: 'block', height: 3, borderRadius: 2, background: T.border, overflow: 'hidden' }}
              >
                <span
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
          </li>
        )
      })}
    </ul>
  )
}
