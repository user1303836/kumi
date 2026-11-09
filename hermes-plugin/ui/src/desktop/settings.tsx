// Kumi's entry under Settings → Plugins: where setup stands (the runtime, the DAW part, what's next), the connection,
// and this window's own preferences (open the map on a find; the pet's word on a finished change or job). Kumi's agent
// settings (the DAW, sample folders, FOCUS for the agent) are the form Hermes draws from the plugin's settings schema,
// one link away.
import type { PluginContext } from '@hermes/plugin-sdk'
import { host } from '@hermes/plugin-sdk'
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'
import { useState } from 'react'

import { chipWords, clean, dawName, knownDaw } from '../shared/format.ts'
import { useKumi, useWant } from '../shared/hooks.ts'
import { dawPartWorks, type KumiStore } from '../shared/store.ts'
import { dawChosen, nextSteps } from '../shared/steps.ts'
import { Chip, FOCUSABLE, S, TextButton, useT } from '../shared/ui.tsx'
import { ask } from './actions.ts'
import { ownerSession } from './pane.tsx'
import { PET_LINES } from './pet.ts'

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

// Decimal megabytes, as setup's own messages and the docs give them (setup.py `_mb`): 27.6 MB, not 26.
function megabytes(bytes: unknown): string | null {
  return typeof bytes === 'number' && bytes > 0 ? `${(bytes / 1_000_000).toFixed(1)} MB` : null
}

// The release targets Kumi ships, which the catalogs name as a producer would (`settings.platforms.*`).
const PLATFORMS = new Set([
  'aarch64-apple-darwin',
  'x86_64-apple-darwin',
  'aarch64-pc-windows-msvc',
  'x86_64-pc-windows-msvc',
  'aarch64-unknown-linux-gnu',
  'x86_64-unknown-linux-gnu'
])

export function SettingsPage({ ctx, store, onOpenMap }: { ctx: PluginContext; store: KumiStore; onOpenMap: () => void }) {
  const t = useT()
  useWant(store, 'setup')
  const state = useKumi(store)
  const [openOnFind, setOpenOnFind] = useState(() => ctx.storage.get(OPEN_MAP_ON_FIND, true))
  const [petLines, setPetLines] = useState(() => ctx.storage.get(PET_LINES, false))
  const setup = state.setup
  const steps = nextSteps(state, t)
  const runtime = setup?.runtime
  const component = setup?.daw_component
  // Connected through a current part setup didn't install (an adopted Kumi's): setup's "needs its DAW part" and its
  // "install Kumi's part" aren't the state of things, so the page says ready and the part is in use (#1019's rule).
  const partWorks = dawPartWorks(state)
  const setupState = partWorks ? 'ready' : (setup?.state ?? 'unknown')
  const componentState = component?.state ?? 'unknown'
  // The DAW the page speaks of: none until one is chosen in Kumi's settings or set up (setup reads an unset DAW as
  // Live, its default, and a REAPER user would be told about Live). Then each DAW's setup is offered below.
  const named = dawChosen(state) || partWorks || componentState === 'installed' || Boolean(state.status?.connected)
  const daw = named ? (state.status?.daw ?? setup?.daw) : null
  const next = setup?.message && !partWorks && (named || setupState !== 'needs_daw_component') ? setup.message : null
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
            {next && <Line label={t('settings.next')}>{clean(next, 300)}</Line>}
            {runtime && (
              <Line label={t('settings.runtime')}>
                {runtime.installed ? t('settings.installed', clean(runtime.version ?? '', 20)) : t('settings.notInstalled')}
                {/* The pin shows only where the runtime isn't it yet: that's when it's a step. */}
                {setup?.pinned_version && setup.pinned_version !== runtime.version && (
                  <span style={S.faint}> · {t('settings.needs', clean(setup.pinned_version, 20))}</span>
                )}
                {setup?.target && (
                  <span style={S.faint}> · {PLATFORMS.has(setup.target) ? t(`settings.platforms.${setup.target}`) : clean(setup.target, 40)}</span>
                )}
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
                {(() => {
                  const word = partWorks
                    ? t('settings.partInUse')
                    : COMPONENT_STATES.has(componentState)
                      ? t(`settings.componentStates.${componentState}`)
                      : clean(componentState, 20).replace('_', ' ')
                  // The line's label already names Kumi's part: its own name shows only when setup knows a specific one.
                  return component.name ? t('settings.componentLine', clean(component.name, 40), word) : word
                })()}
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
            {/* Which chat Kumi answers in, as the producer sees chats: this one, or another (its id means nothing to them). */}
            {state.status?.owner?.session_id && (
              <Line label={t('settings.chat')}>
                {host.state.focusedStoredSessionId?.get() === state.status.owner.session_id ? t('settings.chatHere') : t('settings.chatElsewhere')}
              </Line>
            )}
          </div>
          <div style={{ ...S.row, gap: 8, flexWrap: 'wrap' }}>
            {steps.map(step => (
              <TextButton
                key={step.request}
                onClick={() => void ask(ctx, step.request, { send: true, session: state.status?.connected ? ownerSession(state) : null })}
                strong={steps.length === 1}
              >
                {step.label}
              </TextButton>
            ))}
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
        {/* Only where Desktop has a pet to speak (ctx.pet); off until the producer turns it on. */}
        {ctx.pet && (
          <label style={{ ...S.row, gap: 8, padding: '5px 0', borderTop: `1px solid ${T.border}`, cursor: 'pointer' }}>
            <input
              checked={petLines}
              className={FOCUSABLE}
              onChange={event => {
                setPetLines(event.target.checked)
                ctx.storage.set(PET_LINES, event.target.checked)
              }}
              type="checkbox"
            />
            <span style={S.text}>{t('settings.petLines')}</span>
          </label>
        )}
      </div>
    </div>
  )
}
