// Builds Kumi's two UI bundles from src/ into the plugin's folder: the Desktop half (desktop/plugin.js, an ES module
// that imports only the SDK and React, which Hermes Desktop supplies) and the web dashboard's tab
// (dashboard/dist/index.js, a classic script that takes React and the SDK from `window.__HERMES_PLUGIN_SDK__`). Both
// are committed; `--check` rebuilds them in memory and fails when either differs from the committed file, so what's
// reviewed is what runs. This build lives in ui/, never the plugin's root: a package.json there would make Hermes
// install it as the plugin's Node dependencies.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { build } from 'esbuild'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const plugin = join(root, '..')
const check = process.argv.includes('--check')
const banner = '// Built from hermes-plugin/ui/src/ by `npm run build` in hermes-plugin/ui/. Edit ui/src/, not this file.'

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
      alias: { '@kumi/theme': join(root, 'src/desktop/theme.ts'), '@kumi/controls': join(root, 'src/desktop/controls.tsx') }
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
        '@kumi/theme': join(root, 'src/dashboard/theme.ts'),
        '@kumi/controls': join(root, 'src/dashboard/controls.tsx')
      }
    }
  }
]

// An invisible or bidi control character, or a tag character, as the bundle carries it: a `\u…` escape in a string
// reaches the bundle as the character itself (`charset: 'utf8'`), and Hermes's install scan flags each one as a high
// finding, which makes its verdict a caution and blocks a git install (Nov 18: U+2060 in the find card's range).
const INVISIBLE = /[\u061c\u180e\u200b-\u200f\u202a-\u202e\u2060-\u2064\u2066-\u2069\ufeff]|[\u{e0000}-\u{e007f}]/gu

let stale = 0
let hidden = 0
for (const { out, options } of bundles) {
  const result = await build(options)
  const text = result.outputFiles[0].text
  const file = join(plugin, out)
  for (const [index, line] of text.split('\n').entries()) {
    for (const match of line.matchAll(INVISIBLE)) {
      hidden += 1
      const code = match[0].codePointAt(0).toString(16).toUpperCase().padStart(4, '0')
      console.error(`${out}:${index + 1}:${match.index + 1} carries U+${code}: write it so no such character reaches the bundle (in ui/src/).`)
    }
  }
  if (check) {
    let committed = ''
    try {
      committed = readFileSync(file, 'utf8')
    } catch {}
    if (committed !== text) {
      stale += 1
      console.error(`${out} is not what ui/src/ builds: run \`npm run build\` in hermes-plugin/ui/ and commit it.`)
    }
  } else {
    mkdirSync(dirname(file), { recursive: true })
    writeFileSync(file, text)
    console.log(`built ${out} (${Math.round(text.length / 1024)} KB)`)
  }
}
process.exitCode = stale || hidden ? 1 : 0
