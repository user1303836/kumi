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
    openWorkspace?: (id: string, options: { title?: string; minWidth?: string; render: () => ReactNode; onClose?: () => void }) => () => void
    openSession?: (id: string, options?: { intent?: 'in-place' | 'stack' | 'tab' | 'window'; profile?: string }) => unknown
    paneVisibility?: (paneId: string) => ReadableAtom<boolean>
    state: {
      profile: ReadableAtom<string>
      connectionId?: ReadableAtom<null | string>
      focusedSessionId?: ReadableAtom<null | string>
    }
  }

  export const host: Host
  export const PANES_AREA: string
  export const PALETTE_AREA: string
  export const STATUSBAR_AREAS: { left: string; right: string }
  export const TOOL_MEDIA_RENDERER_AREA: string | undefined
  export function useValue<T>(atom: ReadableAtom<T>): T
  export function pluginSettingsHref(pluginId: string, subpage?: string): string

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
