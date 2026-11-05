// Kumi's entry under Settings → Plugins: where setup stands (the runtime, the DAW part, what's next), the connection,
// and this window's own preference. Kumi's agent settings (the DAW, sample folders, FOCUS for the agent) are the
// form Hermes draws from the plugin's settings schema, one link away.
import type { PluginContext } from '@hermes/plugin-sdk'
import { host } from '@hermes/plugin-sdk'
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'
import { useState } from 'react'

import { chipWords, clean, dawName } from '../shared/format.ts'
import { useKumi, useWant } from '../shared/hooks.ts'
import type { KumiStore } from '../shared/store.ts'
import { Chip, S, TextButton } from '../shared/ui.tsx'
import { ask } from './actions.ts'
import { nextStep, ownerSession } from './pane.tsx'

export const OPEN_MAP_ON_FIND = 'openMapOnFind'

const STATE_WORDS: Record<string, string> = {
  ready: 'Ready',
  needs_runtime: 'Not set up yet',
  outdated_runtime: 'Needs the pinned version',
  damaged_runtime: 'Needs reinstalling',
  needs_daw_component: 'Needs its DAW part',
  no_pin: 'No pinned release in this plugin',
  unsupported_platform: 'Not available on this system',
  unknown: 'Unknown'
}

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
  useWant(store, 'setup')
  const state = useKumi(store)
  const [openOnFind, setOpenOnFind] = useState(() => ctx.storage.get(OPEN_MAP_ON_FIND, true))
  const setup = state.setup
  const step = nextStep(state)
  const daw = state.status?.daw ?? setup?.daw
  const runtime = setup?.runtime
  const component = setup?.daw_component
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 18, maxWidth: 640, color: T.text }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        <span style={{ fontSize: 15, fontWeight: 600 }}>Kumi</span>
        <span style={S.muted}>
          Kumi reads and changes your {dawName(daw)} project through typed changes you approve, keeps HISTORY with undo, measures audio on this computer, and maps
          the sample folders you allow.
        </span>
      </div>
      {state.available === false ? (
        <span style={S.text}>
          Kumi's agent half is off in this profile. Turn on the kumi plugin's Agent switch in Capabilities → Plugins and restart the gateway; its settings form
          appears under Agent settings.
        </span>
      ) : (
        <>
          <div>
            <span style={{ ...S.heading, display: 'block', marginBottom: 4 }}>Setup</span>
            <Line label="State">{STATE_WORDS[setup?.state ?? 'unknown'] ?? clean(setup?.state ?? '', 40)}</Line>
            {setup?.message && <Line label="Next">{clean(setup.message, 300)}</Line>}
            {runtime && (
              <Line label="Kumi runtime">
                {runtime.installed ? `Kumi ${clean(runtime.version ?? '', 20)}` : 'Not installed'}
                {setup?.pinned_version && <span style={S.faint}> · pinned {clean(setup.pinned_version, 20)}</span>}
                {setup?.target && <span style={S.faint}> · {clean(setup.target, 40)}</span>}
              </Line>
            )}
            {runtime && !runtime.installed && megabytes(setup?.download_bytes) && (
              <Line label="Download">
                {megabytes(setup?.download_bytes)}
                {megabytes(setup?.unpacked_bytes) && <span style={S.faint}> ({megabytes(setup?.unpacked_bytes)} unpacked)</span>}
              </Line>
            )}
            {component && (
              <Line label={`${dawName(daw)} part`}>
                {clean(component.name ?? 'DAW component', 40)}: {clean(component.state ?? 'unknown', 20).replace('_', ' ')}
                {component.path && <div style={{ ...S.faint, marginTop: 2 }}>{clean(component.path, 200)}</div>}
              </Line>
            )}
            {setup?.existing_kumi?.path && (
              <Line label="Standalone Kumi">
                {clean(setup.existing_kumi.path, 120)}
                <span style={S.faint}>{setup.existing_kumi.adopted ? ' · adopted' : ' · left as it is'}</span>
              </Line>
            )}
            {setup?.compressed_audio?.message && (
              <Line label="Audio formats">
                <span style={setup.compressed_audio.state === 'available' ? undefined : S.muted}>{clean(setup.compressed_audio.message, 400)}</span>
              </Line>
            )}
          </div>
          <div>
            <span style={{ ...S.heading, display: 'block', marginBottom: 4 }}>Connection</span>
            <Line label="Now">
              {(() => {
                const words = chipWords(state.status, state.focus?.transport?.tempo)
                return <Chip tone={words.tone === 'muted' ? undefined : words.tone}>{words.text}</Chip>
              })()}
            </Line>
            {state.status?.owner?.session_id && <Line label="Kumi's chat">{clean(state.status.owner.session_id, 80)}</Line>}
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
            <TextButton onClick={onOpenMap}>Open the sound map</TextButton>
            <TextButton onClick={() => host.navigate('/settings?tab=plugins&agent=kumi')}>Agent settings</TextButton>
          </div>
        </>
      )}
      <div>
        <span style={{ ...S.heading, display: 'block', marginBottom: 4 }}>This window</span>
        <label style={{ ...S.row, gap: 8, padding: '5px 0', borderTop: `1px solid ${T.border}`, cursor: 'pointer' }}>
          <input
            checked={openOnFind}
            onChange={event => {
              setOpenOnFind(event.target.checked)
              ctx.storage.set(OPEN_MAP_ON_FIND, event.target.checked)
            }}
            type="checkbox"
          />
          <span style={S.text}>Open the sound map when Kumi finds sounds</span>
        </label>
      </div>
    </div>
  )
}
