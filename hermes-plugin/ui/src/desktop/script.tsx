// CC-19's pane part. While a script's approval waits in the chat in front, the pane shows the whole of it beside
// Hermes's approval: what Hermes says it does (its claim; the code is the truth), its language, size and SHA-256, the
// approval's own warning, and every line. A script's HISTORY row can show the code it ran, after a restart too. The
// code comes from the file the plugin saved when it asked, re-hashed by `/script`; nothing here runs anything.
import { T } from '@kumi/theme'
import { useEffect, useState } from 'react'

import { CodeBlock } from '../shared/Code.tsx'
import { clean, dawName } from '../shared/format.ts'
import type { KumiStore } from '../shared/store.ts'
import type { HistoryItem, PendingScript, ScriptCode, ScriptLanguage } from '../shared/types.ts'
import { S, TextButton, useT } from '../shared/ui.tsx'

const LANGUAGE: Record<ScriptLanguage, string> = { python: 'script.python', lua: 'script.lua' }

export function PendingScriptView({ script, daw }: { script: { pending: PendingScript; code: ScriptCode | null | false }; daw: unknown }) {
  const t = useT()
  const { pending, code } = script
  const where = pending.daw ?? daw
  const facts = [
    t('script.in', t(LANGUAGE[pending.language]), dawName(where, t)),
    typeof pending.lines === 'number' ? t('script.lines', pending.lines) : '',
    t('script.sha', pending.sha256.slice(0, 12))
  ].filter(Boolean)
  return (
    <section aria-label={t('script.waiting')} style={{ ...S.section, borderLeft: `2px solid ${T.warn}` }}>
      <span style={S.heading}>{t('script.waiting')}</span>
      {pending.purpose && <span style={S.text}>{t('script.says', clean(pending.purpose, 200))}</span>}
      <span style={S.faint} title={t('script.shaTitle', pending.sha256)}>
        {facts.join(' · ')}
      </span>
      {pending.warning && <span style={{ ...S.muted, color: T.warn }}>{clean(pending.warning, 300)}</span>}
      {code ? (
        <CodeBlock code={code.code} label={t('script.code')} />
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
