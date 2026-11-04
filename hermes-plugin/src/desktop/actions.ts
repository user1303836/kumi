// Every button in Kumi's Desktop half that would change something only puts a request in the chat (contract C5):
// the model reads it, calls the tool, and Hermes asks for approval like for any other change. Nothing here writes.
import type { PluginContext } from '@hermes/plugin-sdk'
import { host } from '@hermes/plugin-sdk'

export interface Asked {
  /** Where the request went: `sent` to Kumi's chat, `drafted` into an input, `copied` to the clipboard, or nowhere. */
  outcome: 'copied' | 'drafted' | 'none' | 'sent'
}

/**
 * Put `text` in the chat that owns Kumi's runtime (`session`, from the runtime's owner record), or in the chat in
 * front of the user when no runtime owns one yet. `send` submits it as if typed; otherwise it waits in the input.
 *
 * Fails closed: a request meant for Kumi's chat is never sent from another chat. When that chat can't take it, Kumi
 * opens it and tries once more; failing that, the words wait in the input in front of the user, unsent, with a note.
 */
export async function ask(ctx: PluginContext, text: string, options: { session?: null | string; send: boolean }): Promise<Asked> {
  const composer = host.composer
  const session = options.session ?? null
  if (!composer) {
    const copied = (await ctx.os?.writeClipboard?.(text)) ?? false
    host.notify({
      kind: copied ? 'info' : 'warning',
      message: copied ? 'Copied the request: paste it into the chat to send it.' : `Ask in the chat: ${text}`
    })
    return { outcome: copied ? 'copied' : 'none' }
  }
  const deliver = async (): Promise<boolean> => (options.send ? composer.submit(session, text) : composer.insertText(session, text))
  if (await deliver()) return { outcome: options.send ? 'sent' : 'drafted' }
  if (session && typeof host.openSession === 'function') {
    host.openSession(session)
    const opened = Promise.withResolvers<void>()
    ctx.setTimeout(opened.resolve, 600)
    await opened.promise
    if (await deliver()) return { outcome: options.send ? 'sent' : 'drafted' }
  }
  if (session && (await composer.insertText(null, text))) {
    host.notify({
      kind: 'warning',
      message: "Kumi's chat isn't open here, so the request is in your input, unsent. Send it from the chat Kumi is connected to."
    })
    return { outcome: 'drafted' }
  }
  host.notify({ kind: 'warning', message: 'Open a chat first, then try again.' })
  return { outcome: 'none' }
}
