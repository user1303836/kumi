// CC-9: Kumi's sound search and listen results also arrive as plugin events (`plugin.kumi.sounds.found`,
// `plugin.kumi.listen.done`), sent by the plugin's post_tool_call hook, so the map and the measurements still show
// when Hermes sends no `tool.complete` (tool progress off, focus view). A plugin event reaches every client of the
// Hermes process, so one is acted on only for this window's profile (9a), then only for the chat in front, and once
// per tool call, whether the event or the call's `tool.complete` comes first (9d).
import { host } from '@hermes/plugin-sdk'

/** A profile's name as Desktop compares it (`normalizeProfileKey`): trimmed, and `default` when empty. */
export function profileKey(value: string): string {
  return value.trim() || 'default'
}

/** The profile of the chat in front, as Desktop knows it; null when it can't say. */
function frontProfile(): string | null {
  const focused = host.state.focusedSessionProfile?.get()
  if (typeof focused === 'string') return profileKey(focused)
  const profile = host.state.profile?.get()
  return typeof profile === 'string' ? profileKey(profile) : null
}

/** Whether an event names this window's profile (9a, checked first) and the chat in front. Fails closed: an event
 *  without a profile or a chat, or a window that can't say which chat is in front, is for no one here. */
export function forChatInFront(payload: Record<string, unknown>): boolean {
  if (typeof payload.profile !== 'string' || typeof payload.session_id !== 'string' || !payload.session_id) return false
  const profile = frontProfile()
  if (profile === null || profileKey(payload.profile) !== profile) return false
  return host.state.focusedStoredSessionId?.get() === payload.session_id
}

/** A tool call's id as an event or a `tool.complete` carries it, or null. */
export function callId(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 && value.length <= 200 ? value : null
}

/** The tool calls already acted on, newest kept: a call's event and its `tool.complete` act once between them. */
export class Calls {
  private seen: string[] = []

  constructor(private readonly most = 200) {}

  has(id: string): boolean {
    return this.seen.includes(id)
  }

  add(id: string): void {
    if (this.has(id)) return
    this.seen.push(id)
    if (this.seen.length > this.most) this.seen.shift()
  }
}

/** An event's payload as an object, whatever came. */
export function payloadOf(event: { payload?: unknown }): Record<string, unknown> {
  return event.payload && typeof event.payload === 'object' ? (event.payload as Record<string, unknown>) : {}
}
