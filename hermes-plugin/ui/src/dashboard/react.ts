// `react` in the dashboard bundle: the dashboard's own React instance, from its plugin SDK. Plugins never bundle React
// (two copies would break hooks), so the dashboard build aliases `react` to this file.
import type * as ReactTypes from 'react'

interface DashboardSdk {
  React: typeof ReactTypes
  fetchJSON: <T>(url: string, init?: RequestInit) => Promise<T>
  /** The dashboard's i18n context (`{locale, setLocale, t}`); older dashboards have none. */
  useI18n?: () => unknown
}

declare global {
  interface Window {
    __HERMES_PLUGIN_SDK__?: DashboardSdk
    __HERMES_PLUGINS__?: { register: (name: string, component: ReactTypes.ComponentType) => void }
  }
}

const sdk = window.__HERMES_PLUGIN_SDK__
if (!sdk) throw new Error('Kumi: the dashboard plugin SDK is missing')

export const SDK: DashboardSdk = sdk
const React = sdk.React
export default React
export const {
  Fragment,
  createContext,
  createElement,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore
} = React
