// A made sound's lineage as a bar: the colours of the families it came from, each as wide as its share. It sits along
// the top edge of the sound's card, so what a sound is made of reads before its words do.
import { T } from '@kumi/theme'

import type { Share } from './lineage.ts'

export function LineageBar({ shares }: { shares: readonly Share[] }) {
  if (shares.length === 0) return null
  return (
    <span aria-hidden style={{ display: 'flex', height: 4, borderRadius: 2, overflow: 'hidden', flex: 'none' }}>
      {shares.map((part, index) => (
        <span key={index} style={{ flex: `${part.share} 1 0`, minWidth: 2, background: part.color ?? T.accent }} />
      ))}
    </span>
  )
}
