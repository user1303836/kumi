// Kumi's entry under Settings → Plugins: where setup stands (the runtime, the DAW part, what's next), the connection,
// and this window's own preference. Kumi's agent settings (the DAW, sample folders, FOCUS for the agent) are the
// form Hermes draws from the plugin's settings schema, one link away.
import type { PluginContext } from '@hermes/plugin-sdk'
import { host } from '@hermes/plugin-sdk'
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'
import { useState } from 'react'

import { chipWords, clean, dawName, knownDaw } from '../shared/format.ts'
import { useKumi, useWant } from '../shared/hooks.ts'
import type { KumiStore } from '../shared/store.ts'
import { Chip, FOCUSABLE, S, TextButton, useT } from '../shared/ui.tsx'
import { ask } from './actions.ts'
import { nextStep, ownerSession } from './pane.tsx'

export const OPEN_MAP_ON_FIND = 'openMapOnFind'

// Setup states and DAW-part states the catalogs word (`settings.states.*`, `settings.componentStates.*`).
const STATES = new Set(['ready', 'needs_runtime', 'outdated_runtime', 'damaged_runtime', 'needs_daw_component', 'no_pin', 'unsupported_platform', 'unknown'])
const COMPONENT_STATES = new Set(['not_installed', 'installed', 'outdated', 'unknown'])
// A group's title: a heading for a screen reader, drawn as the small caps line.
const HEADING = { ...S.heading, display: 'block', margin: '0 0 4px', fontWeight: 'inherit' } as const

function Line({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div style={{ display: 'grid', gridTemplateColumns: '9rem 1fr', gap: 8, alignItems: 'baseline', padding: '5px 0', borderTop: `1px solid ${T.border}` }}>
      <span style={S.muted}>{label}</span>
      <span style={{ ...S.text, minWidth: 0 }}>{children}</span>
    </div>
  )
}

function megabytes(bytes: unknown): string | null {
  return typeof bytes === 'number' && bytes > 0 ? `${(bytes / 1_048_576).toFixed(0)} MB` : null
}

export function SettingsPage({ ctx, store, onOpenMap }: { ctx: PluginContext; store: KumiStore; onOpenMap: () => void }) {
  const t = useT()
  useWant(store, 'setup')
  const state = useKumi(store)
  const [openOnFind, setOpenOnFind] = useState(() => ctx.storage.get(OPEN_MAP_ON_FIND, true))
  const setup = state.setup
  const step = nextStep(state, t)
  const daw = state.status?.daw ?? setup?.daw
  const runtime = setup?.runtime
  const component = setup?.daw_component
  const setupState = setup?.state ?? 'unknown'
  const componentState = component?.state ?? 'unknown'
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 18, maxWidth: 640, color: T.text }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        <span style={{ fontSize: 15, fontWeight: 600 }}>Kumi</span>
        <span style={S.muted}>{t('settings.intro', knownDaw(daw) ?? '')}</span>
      </div>
      {state.available === false ? (
        <span style={S.text}>{t('settings.agentOff')}</span>
      ) : (
        <>
          <div>
            <h3 style={HEADING}>{t('settings.setup')}</h3>
            <Line label={t('settings.state')}>{STATES.has(setupState) ? t(`settings.states.${setupState}`) : clean(setupState, 40)}</Line>
            {setup?.message && <Line label={t('settings.next')}>{clean(setup.message, 300)}</Line>}
            {runtime && (
              <Line label={t('settings.runtime')}>
                {runtime.installed ? t('settings.installed', clean(runtime.version ?? '', 20)) : t('settings.notInstalled')}
                {setup?.pinned_version && <span style={S.faint}> · {t('settings.pinned', clean(setup.pinned_version, 20))}</span>}
                {setup?.target && <span style={S.faint}> · {clean(setup.target, 40)}</span>}
              </Line>
            )}
            {runtime && !runtime.installed && megabytes(setup?.download_bytes) && (
              <Line label={t('settings.download')}>
                {megabytes(setup?.download_bytes)}
                {megabytes(setup?.unpacked_bytes) && <span style={S.faint}> {t('settings.unpacked', megabytes(setup?.unpacked_bytes))}</span>}
              </Line>
            )}
            {component && (
              <Line label={t('settings.part', dawName(daw, t))}>
                {t(
                  'settings.componentLine',
                  clean(component.name ?? t('settings.dawComponent'), 40),
                  COMPONENT_STATES.has(componentState) ? t(`settings.componentStates.${componentState}`) : clean(componentState, 20).replace('_', ' ')
                )}
                {component.path && <div style={{ ...S.faint, marginTop: 2 }}>{clean(component.path, 200)}</div>}
              </Line>
            )}
            {setup?.existing_kumi?.path && (
              <Line label={t('settings.standalone')}>
                {clean(setup.existing_kumi.path, 120)}
                <span style={S.faint}> · {setup.existing_kumi.adopted ? t('settings.adopted') : t('settings.leftAsIs')}</span>
              </Line>
            )}
            {setup?.compressed_audio?.message && (
              <Line label={t('settings.audioFormats')}>
                <span style={setup.compressed_audio.state === 'available' ? undefined : S.muted}>{clean(setup.compressed_audio.message, 400)}</span>
              </Line>
            )}
          </div>
          <div>
            <h3 style={HEADING}>{t('settings.connection')}</h3>
            <Line label={t('settings.now')}>
              {(() => {
                const words = chipWords(state.status, state.focus?.transport?.tempo, t)
                return <Chip tone={words.tone === 'muted' ? undefined : words.tone}>{words.text}</Chip>
              })()}
            </Line>
            {state.status?.owner?.session_id && <Line label={t('settings.chat')}>{clean(state.status.owner.session_id, 80)}</Line>}
          </div>
          <div style={{ ...S.row, gap: 8, flexWrap: 'wrap' }}>
            {step && (
              <TextButton
                onClick={() => void ask(ctx, step.request, { send: true, session: state.status?.connected ? ownerSession(state) : null })}
                strong
              >
                {step.label}
              </TextButton>
            )}
            <TextButton onClick={onOpenMap}>{t('settings.openMap')}</TextButton>
            <TextButton onClick={() => host.navigate('/settings?tab=plugins&agent=kumi')}>{t('settings.agentSettings')}</TextButton>
          </div>
        </>
      )}
      <div>
        <h3 style={HEADING}>{t('settings.thisWindow')}</h3>
        <label style={{ ...S.row, gap: 8, padding: '5px 0', borderTop: `1px solid ${T.border}`, cursor: 'pointer' }}>
          <input
            checked={openOnFind}
            className={FOCUSABLE}
            onChange={event => {
              setOpenOnFind(event.target.checked)
              ctx.storage.set(OPEN_MAP_ON_FIND, event.target.checked)
            }}
            type="checkbox"
          />
          <span style={S.text}>{t('settings.openOnFind')}</span>
        </label>
      </div>
    </div>
  )
}
