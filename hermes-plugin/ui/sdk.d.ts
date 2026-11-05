// The parts of `@hermes/plugin-sdk` Kumi's Desktop half uses, for type checking only: Hermes Desktop supplies the real
// module at runtime. Shapes follow Hermes v0.22.7 (`apps/desktop/src/sdk`, `contrib/plugin.ts`). Anything newer than
// the oldest Desktop Kumi supports is optional here and feature-detected where it's used.
declare module '@hermes/plugin-sdk' {
  import type { ComponentProps, CSSProperties, ReactNode } from 'react'

  export interface ReadableAtom<T> {
    get: () => T
    subscribe: (fn: (value: T) => void) => () => void
    listen?: (fn: (value: T) => void) => () => void
  }

  export interface GatewayEvent {
    type: string
    payload?: unknown
    session_id?: string
  }

  export interface PluginRestOptions {
    method?: string
    body?: unknown
    timeoutMs?: number
  }

  export interface PluginContribution {
    id: string
    area: string
    title?: string
    order?: number
    when?: () => boolean
    render?: () => ReactNode
    data?: unknown
  }

  export interface PluginSettingsSubpage {
    id: string
    title: string
    render: () => ReactNode
  }

  export interface PluginSettingsPage {
    id: string
    title: string
    icon?: string
    order?: number
    render: () => ReactNode
    children?: PluginSettingsSubpage[]
  }

  export interface PluginStorage {
    get<T>(key: string, fallback: T): T
    set(key: string, value: unknown): void
    remove(key: string): void
  }

  export interface PluginOs {
    writeClipboard?: (text: string) => Promise<boolean>
  }

  /** Desktop's plugin i18n (`ctx.i18n`): bundles per locale, resolved in the app's language with English, then the key,
   *  as fallbacks. Older Desktops have none. */
  export interface PluginI18n {
    register: (bundles: Record<string, unknown>) => () => void
    t: (key: string, ...args: unknown[]) => string
    onLocaleChange: (listener: () => void) => () => void
  }

  export interface PluginContext {
    readonly source: string
    register: (contribution: PluginContribution) => () => void
    registerMany: (contributions: PluginContribution[]) => () => void
    registerSettingsPage?: (page: PluginSettingsPage) => () => void
    onDispose: (fn: () => void) => void
    onEvent: (type: string, listener: (event: GatewayEvent) => void) => () => void
    setTimeout: (fn: () => void, ms: number) => () => void
    setInterval: (fn: () => void, ms: number) => () => void
    addEventListener: (target: EventTarget, type: string, listener: EventListener, options?: AddEventListenerOptions | boolean) => () => void
    rest: <T>(path: string, options?: PluginRestOptions) => Promise<T>
    storage: PluginStorage
    os?: PluginOs
    i18n?: PluginI18n
    /** A line in the core pet's speech bubble, labelled with the plugin's name. Older Desktops have none. */
    pet?: PluginPet
  }

  export interface PluginPet {
    say: (text: string, options?: { id?: string; tone?: 'error' | 'info' | 'wait'; ttlMs?: number }) => () => void
    clear: (id?: string) => void
    /** Whether a pet is on screen: none adopted, or turned off, and nothing shows. */
    visible: { get: () => boolean }
  }

  export interface HermesPlugin {
    id: string
    name?: string
    description?: string
    defaultEnabled?: boolean
    register: (ctx: PluginContext) => void
  }

  export interface PaletteContribution {
    id: string
    label: string
    keywords?: string[]
    run: () => void
  }

  export interface ToolMediaFile {
    path: string
    name: string
    kind: 'audio' | 'file' | 'image' | 'video'
    mime: string
    resolveUrl: () => Promise<string>
  }

  export interface ToolMediaRendererProps {
    file: ToolMediaFile
    toolName: string
    toolCallId: string
    args: Readonly<Record<string, unknown>>
    result: unknown
    sessionId: null | string
    fallback: ReactNode
  }

  export interface HostComposer {
    insertText: (sessionId: null | string, text: string, options?: { mode?: 'block' | 'inline' | 'prefix' }) => Promise<boolean>
    submit: (sessionId: null | string, text: string) => boolean
    focus?: (sessionId: null | string) => void
  }

  export interface Host {
    notify: (input: { kind: 'error' | 'info' | 'success' | 'warning'; message: string; title?: string; detail?: string }) => unknown
    navigate: (path: string) => void
    onEvent: (type: string, fn: (event: GatewayEvent) => void) => () => void
    composer?: HostComposer
    /** `dock` places a new tab: `{pane: 'workspace', pos: 'center'}` (the default) is a tab over the chat; `right`
     *  splits the main area so the chat stays in view. Re-opening an open id fronts it where it is. */
    openWorkspace?: (
      id: string,
      options: {
        title?: string
        minWidth?: string
        render: () => ReactNode
        onClose?: () => void
        dock?: { pane: string; pos: 'bottom' | 'center' | 'left' | 'right' | 'top'; before?: string | null }
      }
    ) => () => void
    openSession?: (id: string, options?: { intent?: 'in-place' | 'stack' | 'tab' | 'window'; profile?: string }) => unknown
    paneVisibility?: (paneId: string) => ReadableAtom<boolean>
    state: {
      profile?: ReadableAtom<string>
      /** The profile of the chat in front (Desktop's `normalizeProfileKey` form). */
      focusedSessionProfile?: ReadableAtom<string>
      connectionId?: ReadableAtom<null | string>
      focusedSessionId?: ReadableAtom<null | string>
      /** The durable id of the chat whose composer is focused (the id `owner.session_id` holds). */
      focusedStoredSessionId?: ReadableAtom<null | string>
    }
  }

  export const host: Host
  export const PANES_AREA: string
  export const PALETTE_AREA: string
  export const STATUSBAR_AREAS: { left: string; right: string }
  export const TOOL_MEDIA_RENDERER_AREA: string | undefined
  export const COMPOSER_AREAS: { top?: string; bottom?: string; underside?: string } | undefined
  export function useValue<T>(atom: ReadableAtom<T>): T
  export function pluginSettingsHref(pluginId: string, subpage?: string): string
  /** A React translator for this plugin's bundles, re-rendering on a language switch. Older Desktops have none. */
  export const usePluginI18n: ((pluginId: string) => (key: string, ...args: unknown[]) => string) | undefined

  type Variant = 'default' | 'destructive' | 'ghost' | 'link' | 'outline' | 'secondary' | 'text'
  type Size = 'default' | 'icon-sm' | 'icon-xs' | 'inline' | 'lg' | 'micro' | 'sm' | 'xs'
  export function Button(props: ComponentProps<'button'> & { variant?: Variant; size?: Size }): ReactNode
  export function Codicon(props: { name: string; size?: number | string; spinning?: boolean; className?: string; style?: CSSProperties; title?: string }): ReactNode
  export function StatusDot(props: { tone: 'bad' | 'good' | 'muted' | 'warn'; className?: string }): ReactNode
  export function DropdownMenu(props: { children?: ReactNode; open?: boolean; onOpenChange?: (open: boolean) => void }): ReactNode
  export function DropdownMenuTrigger(props: { children?: ReactNode; asChild?: boolean }): ReactNode
  export function DropdownMenuContent(props: { children?: ReactNode; align?: 'center' | 'end' | 'start'; side?: 'bottom' | 'left' | 'right' | 'top' }): ReactNode
  export function DropdownMenuItem(props: { children?: ReactNode; disabled?: boolean; onSelect?: () => void }): ReactNode
  export function DropdownMenuSeparator(props: Record<string, never>): ReactNode
}
