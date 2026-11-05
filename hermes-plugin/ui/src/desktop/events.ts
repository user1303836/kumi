// Kumi's plugin events reach every client of the Hermes process, each naming the profile it's for (CC-9's 9a; #966
// adds it to C5's other events). The sound search and listen results (`plugin.kumi.sounds.found`,
// `plugin.kumi.listen.done`) come from the plugin's post_tool_call hook, so the map and the measurements still show
// when Hermes sends no `tool.complete` (tool progress off, focus view). One is acted on only for the profile this
// window's Kumi reads from (9a), then only for the chat in front, and once per tool call, whether the event or the
// call's `tool.complete` comes first (9d).
import { host } from '@hermes/plugin-sdk'

/** A profile's name as Desktop compares it (`normalizeProfileKey`): trimmed, and `default` when empty. */
export function profileKey(value: string): string {
  return value.trim() || 'default'
}

/** Whether an event names the profile this window's Kumi reads from: `host.state.profile`, the profile its routes,
 *  `/audio` and its reset follow (Bot Mode can put another profile's chat in front without moving them). Fails
 *  closed: an event without a profile, or a window that can't say its own, is for no one here. */
export function forThisProfile(payload: Record<string, unknown>): boolean {
  const own = host.state.profile?.get()
  return typeof payload.profile === 'string' && typeof own === 'string' && profileKey(payload.profile) === profileKey(own)
}

/** Whether an event is known to be about another profile's Kumi: it names a profile, this window knows its own, and
 *  they differ. Events that only make this window read its own routes again are gated by this alone, so an event
 *  naming no profile (a runtime before #966) still counts. */
export function forAnotherProfile(payload: Record<string, unknown>): boolean {
  const own = host.state.profile?.get()
  return typeof payload.profile === 'string' && typeof own === 'string' && profileKey(payload.profile) !== profileKey(own)
}

/** Whether an event is for this profile (9a, checked first) and the chat in front. Fails closed: an event without a
 *  chat, or a window that can't say which chat is in front, is for no one here. */
export function forChatInFront(payload: Record<string, unknown>): boolean {
  if (!forThisProfile(payload) || typeof payload.session_id !== 'string' || !payload.session_id) return false
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

  clear(): void {
    this.seen = []
  }
}

/** An event's payload as an object, whatever came. */
export function payloadOf(event: { payload?: unknown }): Record<string, unknown> {
  return event.payload && typeof event.payload === 'object' ? (event.payload as Record<string, unknown>) : {}
}
