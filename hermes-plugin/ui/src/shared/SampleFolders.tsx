// The Sample folders control, the same in Desktop's map and the dashboard's: the folders Kumi may read for the map, and
// adding one the producer knows by its path (typed, pasted or, in Desktop, picked in the app's folder dialog). The
// plugin's own folder guard answers for the path (`POST /library/folder`), and its refusal shows here in its words. A
// folder it takes is asked for in the chat (Desktop sends the request, the dashboard copies it), where the tool's
// approval names it: nothing is added or read from here.
import { T } from '@kumi/theme'
import { useCallback, useEffect, useState } from 'react'

import type { Translate } from './i18n.ts'
import type { FolderCheck, KumiStore } from './store.ts'
import { FOCUSABLE, S, TextButton, useT } from './ui.tsx'

/** Where the request to add a folder went: sent to Kumi's chat, left in an input, copied, or nowhere. */
export type FolderAsked = 'copied' | 'drafted' | 'none' | 'sent'

/** The folders Kumi may read now, read again when the map changes: undefined until read, null when they can't be (an
 *  older backend half). */
export function useSampleFolders(store: KumiStore, revision: null | string | undefined): null | string[] | undefined {
  const [folders, setFolders] = useState<null | string[] | undefined>(undefined)
  const read = useCallback(() => void store.libraryFolders().then(setFolders), [store])
  useEffect(read, [read, revision])
  return folders
}

/** What the button does with what's typed: the folder guard's answer first, and only a folder it takes is asked for
 *  in the chat, as the guard keeps it. Answers what to say under the field (a refusal in the guard's own words), or
 *  null when the request went nowhere and the host said why itself. */
export async function addTyped(
  typed: string,
  check: (typed: string) => Promise<FolderCheck>,
  ask: (folder: string, known: boolean) => Promise<FolderAsked>,
  t: Translate
): Promise<{ refused: boolean; text: string } | null> {
  const answer = await check(typed)
  if (answer.folder === undefined) return { refused: true, text: answer.refusal ? t('folders.refused', answer.refusal) : t('folders.failed', answer.error ?? '') }
  const asked = await ask(answer.folder, answer.known === true)
  if (asked === 'none') return null
  const where = t(asked === 'sent' ? 'folders.sent' : asked === 'drafted' ? 'folders.drafted' : 'folders.copied')
  return { refused: false, text: answer.known ? `${t('folders.known')} ${where}` : where }
}

export function SampleFolders({
  folders,
  example,
  check,
  ask,
  pick,
  copies,
  onClose
}: {
  folders: null | string[] | undefined
  /** A path as someone would type it on Kumi's computer, for the field's hint. */
  example: string
  check: (typed: string) => Promise<FolderCheck>
  /** Ask in the chat for `folder` (the guard's answer) to be added and scanned; `known`: it's one already, so a scan. */
  ask: (folder: string, known: boolean) => Promise<FolderAsked>
  /** Desktop's folder dialog: the path picked, or null. Absent where there is none (the dashboard, an older Desktop). */
  pick?: () => Promise<null | string>
  /** The dashboard: the request is copied, not sent, and the button says so. */
  copies?: boolean
  onClose?: () => void
}) {
  const t = useT()
  const [typed, setTyped] = useState('')
  const [busy, setBusy] = useState(false)
  const [said, setSaid] = useState<{ refused: boolean; text: string } | null>(null)
  const submit = async () => {
    const folder = typed.trim()
    if (!folder || busy) return
    setBusy(true)
    setSaid(null)
    try {
      const answer = await addTyped(folder, check, ask, t)
      setSaid(answer)
      if (answer && !answer.refused) setTyped('')
    } finally {
      setBusy(false)
    }
  }
  return (
    <div aria-label={t('folders.title')} role="group" style={{ display: 'grid', gap: 7, textAlign: 'left', fontSize: 12, color: T.text }}>
      <div style={{ ...S.row, justifyContent: 'space-between', gap: 8 }}>
        <span style={{ fontSize: 10.5, fontWeight: 600, letterSpacing: '.09em', textTransform: 'uppercase', color: T.secondary }}>{t('folders.title')}</span>
        {onClose && (
          <TextButton label={t('folders.close')} onClick={onClose}>
            ×
          </TextButton>
        )}
      </div>
      {folders === undefined ? (
        <span style={S.faint}>{t('folders.reading')}</span>
      ) : folders === null ? null : folders.length === 0 ? (
        <span style={S.faint}>{t('folders.none')}</span>
      ) : (
        <ul style={{ margin: 0, padding: 0, listStyle: 'none', display: 'grid', gap: 2, maxHeight: 120, overflowY: 'auto' }}>
          {folders.map(folder => (
            <li key={folder} style={{ fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace', fontSize: 11.5, overflowWrap: 'anywhere' }}>
              {folder}
            </li>
          ))}
        </ul>
      )}
      <form
        onSubmit={event => {
          event.preventDefault()
          void submit()
        }}
        style={{ ...S.row, flexWrap: 'wrap', gap: 6 }}
      >
        <input
          aria-label={t('folders.path')}
          autoComplete="off"
          className={FOCUSABLE}
          disabled={busy}
          maxLength={4096}
          onChange={event => {
            setTyped(event.target.value)
            setSaid(null)
          }}
          placeholder={t('folders.pathHint', example)}
          spellCheck={false}
          style={{ flex: '1 1 13rem', minWidth: 0, font: 'inherit', fontSize: 12, lineHeight: '18px', padding: '2px 7px', borderRadius: 4, border: `1px solid ${T.border}`, background: 'transparent', color: T.text }}
          value={typed}
        />
        {pick && (
          <TextButton
            disabled={busy}
            onClick={() =>
              void pick().then(path => {
                if (!path) return
                setTyped(path)
                setSaid(null)
              })
            }
            title={t('folders.chooseTitle')}
          >
            {t('folders.choose')}
          </TextButton>
        )}
        <TextButton disabled={busy || !typed.trim()} onClick={() => void submit()} strong title={t(copies ? 'folders.copyTitle' : 'folders.addTitle')}>
          {busy ? t('folders.checking') : t(copies ? 'folders.copy' : 'folders.add')}
        </TextButton>
      </form>
      {said && (
        <div role={said.refused ? 'alert' : 'status'} style={{ color: said.refused ? T.warn : T.secondary, overflowWrap: 'anywhere' }}>
          {said.text}
        </div>
      )}
      <div style={S.faint}>{t('folders.note')}</div>
    </div>
  )
}
