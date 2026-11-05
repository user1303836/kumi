// What the generate card shows for a sound Kumi had made (Mirelo, through `kumi_generate`; RESEARCH C1, C7): the file,
// the words it was made from, what it cost against the estimate, and how much of the producer's cap is left. The
// result's shape is the one agreed with W3 (agent-m) until its contract change names it; anything else isn't ours.
import { clean, num, plainId } from './format.ts'
import { english, type Translate } from './i18n.ts'

/** A `kumi_job` (or `kumi_generate`) result for a finished generation, as far as the card reads it. */
export interface Generated {
  artifactId: string | null
  file: string
  kind: string
  prompt: string
  seconds: number | null
  provider: string
  /** Credits the job used, and the approved estimate it was held to (C7). */
  actual: number | null
  estimate: number | null
  /** The producer's cap and what's left under it (the plugin's own budget, not the provider's bill). */
  cap: number | null
  left: number | null
}

const count = (value: unknown): number | null => (typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null)

function objectOf(value: unknown): Record<string, unknown> | null {
  let found = value
  if (typeof found === 'string') {
    try {
      found = JSON.parse(found)
    } catch {
      return null
    }
  }
  return found && typeof found === 'object' && !Array.isArray(found) ? (found as Record<string, unknown>) : null
}

/** A finished generation from a tool's result: a `kumi_job` answer `{kind: 'generate', state: 'done', result}`, or
 *  the result itself. Null for anything else (a scan, a quote, a failure): the card isn't drawn for those. */
export function parseGenerated(value: unknown): Generated | null {
  const outer = objectOf(value)
  if (!outer) return null
  // A job's answer carries its own kind ('generate', 'scan') and, once done, the result; a bare result has neither.
  let done = outer
  if ('job_id' in outer) {
    const result = objectOf(outer.result)
    if (outer.kind !== 'generate' || outer.state !== 'done' || !result) return null
    done = result
  }
  const credits = objectOf(done.credits) ?? {}
  const ledger = objectOf(done.ledger) ?? {}
  const artifactId = plainId(done.artifact_id)
  const file = typeof done.file === 'string' ? clean(done.file.split(/[\\/]/).pop() ?? '', 80) : ''
  // A generation names what it made and from what: without either, this is some other tool's result.
  if (!file || (!artifactId && typeof done.prompt !== 'string')) return null
  return {
    artifactId,
    file,
    kind: typeof done.kind === 'string' ? clean(done.kind, 24) : '',
    prompt: typeof done.prompt === 'string' ? clean(done.prompt, 300) : '',
    seconds: count(done.seconds),
    provider: typeof done.provider === 'string' ? clean(done.provider, 24) : '',
    actual: count(credits.actual),
    estimate: count(credits.estimate),
    cap: count(ledger.cap),
    left: count(ledger.left)
  }
}

/** The cost in words: `12 credits (est. 15)`, or nothing when the result doesn't say. */
export function costWords(made: Generated, t: Translate = english): string {
  if (made.actual === null) return made.estimate === null ? '' : t('generate.estimateOnly', num(made.estimate, 0))
  const spent = t('generate.credits', made.actual, num(made.actual, 0))
  return made.estimate === null || made.estimate === made.actual ? spent : t('generate.cost', spent, num(made.estimate, 0))
}

/** What's left under the producer's cap: `48 of 100 credits left under your cap`, or nothing. */
export function capWords(made: Generated, t: Translate = english): string {
  return made.cap === null || made.left === null ? '' : t('generate.left', num(made.left, 0), num(made.cap, 0))
}
