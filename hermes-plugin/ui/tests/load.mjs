// Loads a TypeScript module from src/ for a test: esbuild bundles it (as the real build does) with the Hermes SDK
// replaced by a stub, then Node imports the result. `react: 'real'` keeps React itself (for rendering components);
// otherwise React is a stub that only lets a plugin register.
import { fileURLToPath } from 'node:url'

import { build } from 'esbuild'

// The UI build's folder (hermes-plugin/ui/), and the plugin's folder that holds the built bundles.
export const root = fileURLToPath(new URL('..', import.meta.url))
export const plugin = fileURLToPath(new URL('../..', import.meta.url))

let loads = 0

/** A stub `@hermes/plugin-sdk`. Every newer export is optional: `{ toolMedia: false, composer: false }` makes an older Desktop. */
export function sdkStub({ toolMedia = true, settingsHref = true, composer = true } = {}) {
  return `
    // Read at use, not at import: each test installs its own host after loading the bundle.
    export const host = new Proxy({}, { get: (_target, key) => globalThis.__kumiTestHost?.[key] })
    export const PANES_AREA = 'panes'
    export const PALETTE_AREA = 'palette'
    export const STATUSBAR_AREAS = { left: 'statusBar.left', right: 'statusBar.right' }
    ${toolMedia ? "export const TOOL_MEDIA_RENDERER_AREA = 'transcript.toolMedia'" : ''}
    ${composer ? "export const COMPOSER_AREAS = { top: 'composer.top', bottom: 'composer.bottom', underside: 'composer.underside' }" : ''}
    ${settingsHref ? "export const pluginSettingsHref = (id, page) => '/settings?tab=plugins&plugin=' + id + (page ? '&ppage=' + page : '')" : ''}
    export const useValue = atom => atom.get()
    const Pass = props => props.children ?? null
    export const Button = Pass, Codicon = () => null, StatusDot = () => null
    export const DropdownMenu = Pass, DropdownMenuTrigger = Pass, DropdownMenuContent = Pass, DropdownMenuItem = Pass, DropdownMenuSeparator = () => null
  `
}

const reactStub = `
  export const useEffect = () => {}, useMemo = f => f(), useCallback = f => f, useRef = v => ({ current: v })
  export const useState = v => [typeof v === 'function' ? v() : v, () => {}]
  export const useSyncExternalStore = (subscribe, get) => get()
  export const Fragment = 'Fragment'
  export const jsx = (type, props) => ({ type, props }), jsxs = jsx
  export default { Fragment }
`

/** Bundle `entry` (a path under src/) and import it. */
export async function load(entry, { sdk = sdkStub(), react = 'stub', theme = 'src/desktop/theme.ts' } = {}) {
  const stubs = {
    name: 'stubs',
    setup(b) {
      b.onResolve({ filter: /^@hermes\/plugin-sdk$/ }, () => ({ path: 'sdk', namespace: 'stub' }))
      if (react === 'stub') b.onResolve({ filter: /^react(\/jsx-runtime)?$/ }, () => ({ path: 'react', namespace: 'stub' }))
      b.onLoad({ filter: /.*/, namespace: 'stub' }, args => ({ contents: args.path === 'sdk' ? sdk : reactStub, loader: 'js' }))
    }
  }
  const out = await build({
    entryPoints: [`${root}${entry}`],
    bundle: true,
    format: 'esm',
    platform: 'node',
    jsx: 'automatic',
    write: false,
    logLevel: 'error',
    alias: { '@kumi/theme': `${root}${theme}` },
    external: react === 'real' ? ['react', 'react-dom', 'react/jsx-runtime', 'react-dom/server'] : [],
    plugins: [stubs],
    absWorkingDir: root,
    nodePaths: [`${root}node_modules`]
  })
  loads += 1
  const file = new URL(`./.load-${process.pid}-${loads}.mjs`, import.meta.url)
  const { writeFile, unlink } = await import('node:fs/promises')
  // A real file beside the tests, so a bundle that keeps React external resolves it from node_modules.
  await writeFile(file, out.outputFiles[0].contents)
  try {
    return await import(file.href)
  } finally {
    await unlink(file)
  }
}
