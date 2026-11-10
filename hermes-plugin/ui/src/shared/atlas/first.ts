// The map's first-open hints: three steps (hear a sound, pick some, make one from them), each shown until it has been
// done once, or all are skipped. The host keeps which are done, as one string, where it keeps the drawer's fold.
import { useRef, useState } from 'react'

export const FIRST_STEPS = ['hover', 'box', 'make'] as const
export type FirstStep = (typeof FIRST_STEPS)[number]

const keptSteps = (kept: string | null | undefined): string[] => (kept ?? '').split(',')

/** The steps still to show, in order: the ones not done yet that this map offers (`hover`: it plays what the pointer
 *  is on; `picks`: sounds can be picked on it, and made from). */
export function firstLeft(kept: string | null | undefined, offers: { hover: boolean; picks: boolean }): FirstStep[] {
  const done = keptSteps(kept)
  return FIRST_STEPS.filter(step => !done.includes(step) && (step === 'hover' ? offers.hover : offers.picks))
}

/** What the host keeps once `steps` are done as well. */
export function firstKept(kept: string | null | undefined, steps: readonly FirstStep[]): string {
  const done = keptSteps(kept)
  return FIRST_STEPS.filter(step => done.includes(step) || steps.includes(step)).join(',')
}

/** The steps done, read once from the host's storage and written back as each is done. */
export function useFirstOpen(read: () => string | null | undefined, write: (kept: string) => void): { kept: string; done: (steps: readonly FirstStep[]) => void } {
  const [kept, setKept] = useState(() => read() ?? '')
  const latest = useRef(kept)
  return {
    kept,
    done: steps => {
      const next = firstKept(latest.current, steps)
      if (next === latest.current) return
      latest.current = next
      write(next)
      setKept(next)
    }
  }
}
