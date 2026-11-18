// The Sample folders control, the same in Desktop's map and the dashboard's: the folders Kumi may read for the map (one
// gone from the disk says so), and adding one the producer knows by its path (typed, pasted or, where the host has one,
// picked in the app's folder dialog). The plugin's own folder guard answers for the path (`POST /library/folder`), and
// its refusal shows here in its words. A folder it takes is asked for in the chat (Desktop sends the request, the
// dashboard copies it), where the tool's approval names it: nothing is added or read from here.
import { Button } from '@kumi/controls'
import { T } from '@kumi/theme'
import { useCallback, useEffect, useState } from 'react'

import type { Translate } from './i18n.ts'
import type { FolderCheck, KumiStore, SampleFolder } from './store.ts'
import { FOCUSABLE, S, TYPE, useT } from './ui.tsx'

/** Where the request to add a folder went: sent to Kumi's chat, left in an input, copied, or nowhere. */
export type FolderAsked = 'copied' | 'drafted' | 'none' | 'sent'

/** The host's folder dialog: whether there is one, asked without opening anything, and opening it. */
export interface FolderDialog {
  /** True where the host really has a folder dialog (Desktop's app; Hermes in a browser has none). */
  present: () => Promise<boolean>
  /** The path picked; null when it's cancelled or refused (the host says why itself); false: there's no dialog. */
  pick: () => Promise<false | null | string>
}

/** Whether the host has a folder dialog: asked once a window (the host doesn't change under it), and known at once to
 *  every Sample folders control drawn after, so Choose… never flickers in or out. */
let hostDialog: Promise<boolean> | undefined
let hostDialogKnown: boolean | undefined
export function dialogPresent(dialog: FolderDialog): Promise<boolean> {
  // A question that fails tells nothing: the button shows, and a "no dialog" answer to it hides it then.
  hostDialog ??= dialog
    .present()
    .catch(() => true)
    .then(yes => (hostDialogKnown = yes))
  return hostDialog
}

/** The folders Kumi may read now, read again when the map changes: undefined until read, null when they can't be (an
 *  older backend half). */
export function useSampleFolders(store: KumiStore, revision: null | string | undefined): null | SampleFolder[] | undefined {
  const [folders, setFolders] = useState<null | SampleFolder[] | undefined>(undefined)
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
  dialog,
  copies,
  onClose
}: {
  folders: null | SampleFolder[] | undefined
  /** A path as someone would type it on Kumi's computer, for the field's hint. */
  example: string
  check: (typed: string) => Promise<FolderCheck>
  /** Ask in the chat for `folder` (the guard's answer) to be added and scanned; `known`: it's one already, so a scan. */
  ask: (folder: string, known: boolean) => Promise<FolderAsked>
  /** The host's folder dialog. Absent where there is none for sure (the dashboard, an older Desktop); Choose… shows
   *  only once it says it has one. */
  dialog?: FolderDialog
  /** The dashboard: the request is copied, not sent, and the button says so. */
  copies?: boolean
  onClose?: () => void
}) {
  const t = useT()
  const [typed, setTyped] = useState('')
  const [busy, setBusy] = useState(false)
  const [said, setSaid] = useState<{ refused: boolean; text: string } | null>(null)
  const [hasDialog, setHasDialog] = useState(hostDialogKnown === true)
  // Said once, in place, when the dialog answers it isn't there after all; never a toast at each click.
  const [noDialog, setNoDialog] = useState(false)
  useEffect(() => {
    if (!dialog) return
    let live = true
    void dialogPresent(dialog).then(yes => live && setHasDialog(yes))
    return () => {
      live = false
    }
  }, [dialog])
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
    <div aria-label={t('folders.title')} role="group" style={{ display: 'grid', gap: 8, textAlign: 'left', ...TYPE.small, color: T.text }}>
      <div style={{ ...S.row, justifyContent: 'space-between', gap: 8 }}>
        <span style={{ ...TYPE.caption, fontWeight: 600, letterSpacing: '0.06em', textTransform: 'uppercase', color: T.muted }}>{t('folders.title')}</span>
        {onClose && (
          <Button kind="quiet" label={t('folders.close')} onClick={onClose}>
            ×
          </Button>
        )}
      </div>
      {folders === undefined ? (
        <span style={{ ...TYPE.small, color: T.faint }}>{t('folders.reading')}</span>
      ) : folders === null ? null : folders.length === 0 ? (
        <span style={{ ...TYPE.small, color: T.faint }}>{t('folders.none')}</span>
      ) : (
        <ul style={{ margin: 0, padding: 0, listStyle: 'none', display: 'grid', gap: 2, maxHeight: 120, overflowY: 'auto' }}>
          {folders.map(folder => (
            <li key={folder.path} style={{ ...TYPE.caption, overflowWrap: 'anywhere', color: folder.missing ? T.faint : T.text }}>
              <span style={{ fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace', textDecoration: folder.missing ? 'line-through' : undefined }}>{folder.path}</span>
              {folder.missing && <span style={{ color: T.warn }}>{` · ${t('folders.missing')}`}</span>}
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
          style={{ flex: '1 1 13rem', minWidth: 0, height: 24, boxSizing: 'border-box', font: 'inherit', ...TYPE.small, padding: '0 7px', borderRadius: 4, border: `1px solid ${T.border}`, background: 'transparent', color: T.text }}
          value={typed}
        />
        {dialog && hasDialog && (
          <Button
            disabled={busy}
            onClick={() =>
              void dialog.pick().then(path => {
                if (path === false) {
                  hostDialogKnown = false
                  hostDialog = Promise.resolve(false)
                  setHasDialog(false)
                  setNoDialog(true)
                  return
                }
                if (!path) return
                setTyped(path)
                setSaid(null)
              })
            }
            tip={t('folders.chooseTitle')}
          >
            {t('folders.choose')}
          </Button>
        )}
        <Button disabled={busy || !typed.trim()} kind="primary" onClick={() => void submit()} tip={t(copies ? 'folders.copyTitle' : 'folders.addTitle')}>
          {busy ? t('folders.checking') : t(copies ? 'folders.copy' : 'folders.add')}
        </Button>
      </form>
      {said && (
        <div role={said.refused ? 'alert' : 'status'} style={{ color: said.refused ? T.warn : T.secondary, overflowWrap: 'anywhere' }}>
          {said.text}
        </div>
      )}
      {noDialog && <div style={{ ...TYPE.small, color: T.secondary }}>{t('folders.noDialog')}</div>}
      <div style={{ ...TYPE.small, color: T.faint }}>{t('folders.note')}</div>
    </div>
  )
}
