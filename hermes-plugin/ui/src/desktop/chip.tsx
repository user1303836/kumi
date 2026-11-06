// The status-bar chip: `● Live · Night Drive · 124 BPM` while Kumi is connected, what's wrong when it isn't. Its menu
// opens the sound map or Kumi's settings, and asks for setup or a connection in the chat when one is needed.
import type { PluginContext } from '@hermes/plugin-sdk'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, StatusDot } from '@hermes/plugin-sdk'

import { chipWords } from '../shared/format.ts'
import { useKumi } from '../shared/hooks.ts'
import type { KumiStore } from '../shared/store.ts'
import { FOCUSABLE, useT } from '../shared/ui.tsx'
import { ask } from './actions.ts'
import { nextSteps, ownerSession } from './pane.tsx'

export function StatusChip({
  ctx,
  store,
  onOpenMap,
  onOpenSettings
}: {
  ctx: PluginContext
  store: KumiStore
  onOpenMap: () => void
  onOpenSettings: () => void
}) {
  const t = useT()
  const state = useKumi(store)
  // Nothing in the bar until Kumi's backend answers: no dead control when the agent half is off.
  if (state.available !== true) return null
  const words = chipWords(state.status, state.focus?.transport?.tempo, t)
  const steps = nextSteps(state, t)
  const connected = Boolean(state.status?.connected)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          aria-label={t('chip.label', words.text)}
          className={FOCUSABLE}
          title="Kumi"
          type="button"
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 5,
            height: '100%',
            padding: '0 6px',
            border: 0,
            background: 'transparent',
            color: 'var(--ui-text-tertiary)',
            font: 'inherit',
            fontSize: '0.6875rem',
            fontVariantNumeric: 'tabular-nums',
            cursor: 'pointer',
            whiteSpace: 'nowrap'
          }}
        >
          <StatusDot tone={words.tone} />
          {words.text}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" side="top">
        <DropdownMenuItem onSelect={onOpenMap}>{t('chip.openMap')}</DropdownMenuItem>
        {steps.map(step => (
          <DropdownMenuItem key={step.request} onSelect={() => void ask(ctx, step.request, { send: true, session: connected ? ownerSession(state) : null })}>
            {step.label}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={onOpenSettings}>{t('chip.settings')}</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
