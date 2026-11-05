// The core pet's speech bubble (`ctx.pet`, newer Desktops): one line when Kumi finishes a change or a job. With the pet
// popped out over the DAW, a producer sees the change land without looking at the chat. Off unless the producer turns it
// on in Kumi's settings. The host does the rest: it strips control and bidi characters, keeps the line to one row of 120
// characters, labels it "Kumi", and shows nothing while there's no pet.
import { clean, jobWords } from '../shared/format.ts'
import { generationJobWords } from '../shared/generate.ts'
import type { Translate } from '../shared/i18n.ts'
import type { KumiState } from '../shared/store.ts'

/** The setting (this window's storage): whether the pet says Kumi's finished changes and jobs. Off unless turned on. */
export const PET_LINES = 'petLines'

export interface PetLine {
  /** The bubble line's id: a newer line of the same kind replaces the one before it. */
  id: 'change' | 'job'
  text: string
  tone: 'error' | 'info'
}

/** What the pet says as the state goes from `before` to `after`: a change that just landed (its HISTORY title), an undo
 *  that just landed, else a job that just finished. Null for anything else, and for the first read of HISTORY, which
 *  holds nothing new. */
export function petLine(before: KumiState, after: KumiState, t: Translate): PetLine | null {
  const was = before.history?.items
  const items = after.history?.items ?? []
  if (was && items.length) {
    const title = (item: (typeof items)[number]) => clean(item.title ?? item.label ?? item.id, 110)
    const top = items[0]
    if (!was.some(item => item.id === top.id) && (top.state === 'applied' || top.state === undefined)) return { id: 'change', text: title(top), tone: 'info' }
    const undone = items.find(item => item.state === 'undone' && was.some(old => old.id === item.id && old.state !== 'undone'))
    if (undone) return { id: 'change', text: t('pet.undone', title(undone)), tone: 'info' }
  }
  for (const [id, job] of Object.entries(after.jobs)) {
    if (job.ended_at === null || before.jobs[id]?.ended_at !== null) continue
    const words = job.kind === 'generate' ? generationJobWords(job, t) : jobWords(job, t)
    return { id: 'job', text: [words.title, words.detail].filter(Boolean).join(' · '), tone: words.tone === 'bad' ? 'error' : 'info' }
  }
  return null
}
