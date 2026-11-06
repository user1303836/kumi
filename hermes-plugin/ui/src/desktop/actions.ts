// Every button in Kumi's Desktop half that would change something only puts a request in the chat (contract C5):
// the model reads it, calls the tool, and Hermes asks for approval like for any other change. Nothing here writes.
import type { HostComposer, PluginContext } from '@hermes/plugin-sdk'
import { host } from '@hermes/plugin-sdk'

import { english, type Translate } from '../shared/i18n.ts'

/** Words for this plugin in Desktop's language, read when they're needed; English on a Desktop without plugin i18n. */
export function wordsOf(ctx: PluginContext): Translate {
  return ctx.i18n?.t ?? english
}

export interface Asked {
  /** Where the request went: `sent` to Kumi's chat, `drafted` into an input, `copied` to the clipboard, or nowhere. */
  outcome: 'copied' | 'drafted' | 'none' | 'sent'
}

/** How long Kumi's chat (or a new one) gets to mount before the request falls back. */
const OPEN_WAIT_MS = 1000
const OPEN_POLL_MS = 100
/** Desktop's route for a new chat (its `NEW_CHAT_ROUTE`). */
const NEW_CHAT = '/'

/**
 * Put `text` in the chat that owns Kumi's runtime (`session`, from the runtime's owner record). `send` submits it as
 * if typed; otherwise it waits in the input.
 *
 * Fails closed: a request meant for Kumi's chat is never sent from another chat. When that chat can't take it, Kumi
 * opens it and tries again for a moment; failing that, the words wait in the input in front of the user, unsent, with
 * a note. With no chat owning the runtime yet, nothing is sent at all: the words wait in the input in front, so the
 * producer chooses the chat, or in a new chat's input when nothing in front has one (Settings, a fresh profile).
 */
export async function ask(ctx: PluginContext, text: string, options: { session?: null | string; send: boolean }): Promise<Asked> {
  const t = wordsOf(ctx)
  const composer = host.composer
  const session = options.session ?? null
  if (!composer) {
    const copied = (await ctx.os?.writeClipboard?.(text)) ?? false
    host.notify({
      kind: copied ? 'info' : 'warning',
      message: copied ? t('notify.copied') : t('notify.askInChat', text)
    })
    return { outcome: copied ? 'copied' : 'none' }
  }
  if (!session) {
    if ((await composer.insertText(null, text)) || (await inNewChat(ctx, composer, text))) return { outcome: 'drafted' }
    host.notify({ kind: 'warning', message: t('notify.openChat') })
    return { outcome: 'none' }
  }
  const deliver = async (): Promise<boolean> => (options.send ? composer.submit(session, text) : composer.insertText(session, text))
  if (await deliver()) return { outcome: options.send ? 'sent' : 'drafted' }
  if (typeof host.openSession === 'function') {
    try {
      await host.openSession(session)
      for (let waited = 0; waited <= OPEN_WAIT_MS; waited += OPEN_POLL_MS) {
        if (await deliver()) return { outcome: options.send ? 'sent' : 'drafted' }
        const tick = Promise.withResolvers<void>()
        ctx.setTimeout(tick.resolve, OPEN_POLL_MS)
        await tick.promise
      }
    } catch {
      // The chat couldn't be opened here: the words go to the input in front, below.
    }
  }
  if (await composer.insertText(null, text)) {
    host.notify({ kind: 'warning', message: t('notify.notOpenHere') })
    return { outcome: 'drafted' }
  }
  host.notify({ kind: 'warning', message: t('notify.openChat') })
  return { outcome: 'none' }
}

/** Open a new chat and leave `text` in its input, unsent: for a request no chat owns yet, made where there's no input
 *  (Settings, Capabilities, a profile with no chat). False when no input took the words in time. */
async function inNewChat(ctx: PluginContext, composer: HostComposer, text: string): Promise<boolean> {
  try {
    host.navigate(NEW_CHAT)
  } catch {
    return false
  }
  for (let waited = 0; waited < OPEN_WAIT_MS; waited += OPEN_POLL_MS) {
    const tick = Promise.withResolvers<void>()
    ctx.setTimeout(tick.resolve, OPEN_POLL_MS)
    await tick.promise
    if (await composer.insertText(null, text)) return true
  }
  return false
}
