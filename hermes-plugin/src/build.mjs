// Builds Kumi's two UI bundles from src/: the Desktop half (desktop/plugin.js, an ES module that imports only the SDK
// and React, which Hermes Desktop supplies) and the web dashboard's tab (dashboard/dist/index.js, a classic script
// that takes React and the SDK from `window.__HERMES_PLUGIN_SDK__`). Both are committed; `--check` rebuilds them in
// memory and fails when either differs from the committed file, so what's reviewed is what runs.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { build } from 'esbuild'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const check = process.argv.includes('--check')
const banner = '// Built from hermes-plugin/src/ by `npm run build`. Edit src/, not this file.'

const shared = {
  bundle: true,
  write: false,
  minify: false,
  legalComments: 'none',
  charset: 'utf8',
  jsx: 'automatic',
  target: 'es2022',
  platform: 'browser',
  logLevel: 'error',
  banner: { js: banner }
}

const bundles = [
  {
    out: 'desktop/plugin.js',
    options: {
      ...shared,
      entryPoints: [join(root, 'src/desktop/plugin.tsx')],
      format: 'esm',
      external: ['@hermes/plugin-sdk', 'react', 'react/jsx-runtime'],
      alias: { '@kumi/theme': join(root, 'src/desktop/theme.ts') }
    }
  },
  {
    out: 'dashboard/dist/index.js',
    options: {
      ...shared,
      entryPoints: [join(root, 'src/dashboard/index.tsx')],
      format: 'iife',
      alias: {
        react: join(root, 'src/dashboard/react.ts'),
        'react/jsx-runtime': join(root, 'src/dashboard/jsx-runtime.ts'),
        '@kumi/theme': join(root, 'src/dashboard/theme.ts')
      }
    }
  }
]

let stale = 0
for (const { out, options } of bundles) {
  const result = await build(options)
  const text = result.outputFiles[0].text
  const file = join(root, out)
  if (check) {
    let committed = ''
    try {
      committed = readFileSync(file, 'utf8')
    } catch {}
    if (committed !== text) {
      stale += 1
      console.error(`${out} is not what src/ builds: run \`npm run build\` in hermes-plugin/ and commit it.`)
    }
  } else {
    mkdirSync(dirname(file), { recursive: true })
    writeFileSync(file, text)
    console.log(`built ${out} (${Math.round(text.length / 1024)} KB)`)
  }
}
process.exitCode = stale ? 1 : 0
