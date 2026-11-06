// CC-19's pane part. While a script's approval waits in the chat in front, the pane shows the whole of it beside
// Hermes's approval: what Hermes says it does (its claim; the code is the truth), its language, size and SHA-256, the
// approval's own warning, and every line. A device (CC-22) shows its controls (name, range, default) beside its GenExpr
// code, all from the one spec its hash names. A script's HISTORY row can show the code it ran, after a restart too.
// The code comes from the file the plugin saved when it asked, re-hashed by `/script`; nothing here runs anything.
import { T } from '@kumi/theme'
import { useEffect, useState } from 'react'

import { CodeBlock } from '../shared/Code.tsx'
import { clean, dawName } from '../shared/format.ts'
import type { Translate } from '../shared/i18n.ts'
import type { KumiStore } from '../shared/store.ts'
import type { DeviceSpec, HistoryItem, PendingScript, ScriptCode, ScriptLanguage } from '../shared/types.ts'
import { S, TextButton, useT } from '../shared/ui.tsx'

const LANGUAGE: Record<ScriptLanguage, string> = { python: 'script.python', lua: 'script.lua', device: 'script.device' }

/** A device's saved spec, parsed: the code and controls the approval named, or null for anything else. */
export function deviceSpecOf(text: string): DeviceSpec | null {
  try {
    const value: unknown = JSON.parse(text)
    if (!value || typeof value !== 'object' || typeof (value as DeviceSpec).code !== 'string') return null
    const controls = (value as DeviceSpec).controls
    return controls === undefined || Array.isArray(controls) ? (value as DeviceSpec) : null
  } catch {
    return null
  }
}

type Control = NonNullable<DeviceSpec['controls']>[number]

function unitOf(control: Control): string {
  return typeof control.unit === 'string' && control.unit ? ` ${clean(control.unit, 12)}` : ''
}

/** A control's range as the approval writes it (`0–1 Hz`, `Soft/Hard`, `on/off`), Kumi's own words translated. */
function range(control: Control, t: Translate): string {
  if (control.type === 'switch') return `${t('script.on')}/${t('script.off')}`
  if (control.type === 'choice' && Array.isArray(control.options)) return control.options.map(option => clean(String(option), 20)).join('/')
  return typeof control.min === 'number' && typeof control.max === 'number' ? `${control.min}–${control.max}${unitOf(control)}` : '—'
}

/** A control's default as the approval writes it: a switch's on or off, a choice as written, a number with its unit. */
function defaultOf(control: Control, t: Translate): string {
  if (control.default === undefined) return '—'
  if (control.type === 'switch' && typeof control.default === 'boolean') return t(control.default ? 'script.on' : 'script.off')
  return `${clean(String(control.default), 20)}${control.type === 'choice' || control.type === 'switch' ? '' : unitOf(control)}`
}

/** The device's controls, each with its range and default, as the approval named them. */
function Controls({ spec }: { spec: DeviceSpec }) {
  const t = useT()
  const controls = (spec.controls ?? []).slice(0, 16)
  if (!controls.length) return <span style={S.faint}>{t('script.noControls')}</span>
  const cell = { padding: '2px 6px', textAlign: 'left', whiteSpace: 'nowrap' } as const
  return (
    <table aria-label={t('script.controls')} style={{ borderCollapse: 'collapse', fontSize: 12, lineHeight: 1.45, color: T.text, margin: 0 }}>
      <thead>
        <tr style={{ color: T.muted, fontSize: 11 }}>
          <th style={{ ...cell, fontWeight: 400 }}>{t('script.control')}</th>
          <th style={{ ...cell, fontWeight: 400 }}>{t('script.range')}</th>
          <th style={{ ...cell, fontWeight: 400 }}>{t('script.default')}</th>
        </tr>
      </thead>
      <tbody>
        {controls.map((control, index) => (
          <tr key={index} style={{ borderTop: `1px solid ${T.border}` }}>
            <td style={cell}>{clean(String(control.name ?? ''), 32)}</td>
            <td style={{ ...cell, color: T.muted }}>{range(control, t)}</td>
            <td style={{ ...cell, color: T.muted }}>{defaultOf(control, t)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

export function PendingScriptView({ script, daw }: { script: { pending: PendingScript; code: ScriptCode | null | false }; daw: unknown }) {
  const t = useT()
  const { pending, code } = script
  const where = pending.daw ?? daw
  const facts = [
    t('script.in', t(LANGUAGE[pending.language]), dawName(where, t)),
    typeof pending.lines === 'number' ? t('script.lines', pending.lines) : '',
    t('script.sha', pending.sha256.slice(0, 12))
  ].filter(Boolean)
  const heading = t(pending.language === 'device' ? 'script.deviceWaiting' : 'script.waiting')
  return (
    <section aria-label={heading} style={{ ...S.section, borderLeft: `2px solid ${T.warn}` }}>
      <span style={S.heading}>{heading}</span>
      {pending.purpose && <span style={S.text}>{t('script.says', clean(pending.purpose, 200))}</span>}
      {pending.file && <span style={S.muted}>{t('script.file', clean(pending.file, 200))}</span>}
      <span style={S.faint} title={t('script.shaTitle', pending.sha256)}>
        {facts.join(' · ')}
      </span>
      {pending.warning && <span style={{ ...S.muted, color: T.warn }}>{clean(pending.warning, 300)}</span>}
      {code ? (
        pending.language === 'device' ? (
          (() => {
            const spec = deviceSpecOf(code.code)
            return spec ? (
              <>
                {spec.name && <span style={{ ...S.text, fontWeight: 600 }}>{clean(spec.name, 32)}</span>}
                <Controls spec={spec} />
                <CodeBlock code={spec.code} label={t('script.deviceCode')} />
              </>
            ) : (
              <span style={S.muted}>{t('script.unreadable')}</span>
            )
          })()
        ) : (
          <CodeBlock code={code.code} label={t('script.code')} />
        )
      ) : code === false ? (
        <span style={S.muted}>{t('script.unreadable')}</span>
      ) : (
        <span style={S.faint}>{t('script.reading')}</span>
      )}
    </section>
  )
}

/** "Show code" under a script's HISTORY row: the code it ran, read by the row's SHA-256 when asked. */
export function SavedScript({ item, store }: { item: HistoryItem; store: KumiStore }) {
  const t = useT()
  const sha = item.script?.sha256
  const language = item.script?.language === 'python' || item.script?.language === 'lua' ? item.script.language : null
  const [open, setOpen] = useState(false)
  const [code, setCode] = useState<ScriptCode | null | false>(null)
  useEffect(() => {
    if (!open || code !== null || !sha || !language) return
    let current = true
    void store.readScript(sha, language).then(read => {
      if (current) setCode(read ?? false)
    })
    return () => {
      current = false
    }
  }, [open, code, sha, language, store])
  if (!sha || !language) return null
  return (
    <span style={{ display: 'flex', flexDirection: 'column', gap: 4, marginTop: 2 }}>
      <span>
        <TextButton onClick={() => setOpen(!open)} pressed={open} title={t('script.showTitle')}>
          {open ? t('script.hide') : t('script.show')}
        </TextButton>
      </span>
      {open &&
        (code ? (
          <CodeBlock code={code.code} label={t('script.code')} />
        ) : code === false ? (
          <span style={S.muted}>{t('script.unreadableSaved')}</span>
        ) : (
          <span style={S.faint}>{t('script.reading')}</span>
        ))}
    </span>
  )
}
