// The first-open hints as the map's corner shows them: the steps left, one line each under its number, and Skip.
import { Button } from '@kumi/controls'
import { T } from '@kumi/theme'

import { S, TYPE, useT } from '../ui.tsx'
import type { FirstStep } from './first.ts'

export function FirstOpen({ steps, onSkip }: { steps: readonly FirstStep[]; onSkip: () => void }) {
  const t = useT()
  if (steps.length === 0) return null
  return (
    <div aria-label={t('first.label')} role="group" style={{ display: 'flex', flexDirection: 'column', gap: 5, paddingBottom: 7, borderBottom: `1px solid ${T.hairline}` }}>
      {steps.map((step, index) => (
        <div key={step} style={{ ...S.row, gap: 8, ...TYPE.small, color: T.text }}>
          <span
            aria-hidden
            style={{
              flex: 'none',
              width: 16,
              height: 16,
              borderRadius: '50%',
              display: 'grid',
              placeItems: 'center',
              background: `color-mix(in srgb, ${T.accent} 24%, transparent)`,
              color: T.accent,
              ...TYPE.caption,
              fontWeight: 600,
              fontVariantNumeric: 'tabular-nums'
            }}
          >
            {index + 1}
          </span>
          <span style={{ flex: 1 }}>{t(`first.${step}`)}</span>
          {index === 0 && (
            <Button kind="quiet" onClick={onSkip} tip={t('first.skipTitle')}>
              {t('first.skip')}
            </Button>
          )}
        </div>
      ))}
    </div>
  )
}
