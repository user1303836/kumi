// node --test: the real Desktop half (src/desktop/plugin.tsx, bundled by esbuild) registered against a stub SDK, on a
// Desktop with every surface Kumi uses and on an older one without the tool-media seam or settings-page links.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

import { load, root, sdkStub } from './load.mjs'

const SESSION = '20261105_101500_k7d2a1'
const ROUTES = {
  '/status': {
    connected: true,
    stale: false,
    daw: 'ableton',
    owner: { profile: 'default', session_id: SESSION, pid: 4242 },
    project: { name: 'Night Drive', key: 'p-6f1c9e', epoch: 3 },
    runtime: { version: '1.24.70', protocol: 'kumi-serve/1' },
    revisions: { status: 12, focus: 88, history: 9, map: 'r7' }
  },
  '/focus': { connected: true, focus: { track: { name: 'Drums' }, device: { name: 'Drum Rack' } }, transport: { playing: true, tempo: 124 } },
  '/history?limit=50': {
    connected: true,
    items: [
      { id: 'h-p6f1c9e-g2-c14', label: 'c14', title: 'Drums volume −6.0 → −4.0 dB', state: 'applied', undo_available: true },
      { id: 'h-p6f1c9e-g2-c13', label: 'c13', title: 'Drum Rack pad D1: Snare 01.wav → Dusty Snare 04.wav', state: 'applied', undo_available: true }
    ]
  },
  '/setup': { state: 'ready', daw: 'ableton' }
}

function atom(value) {
  const listeners = new Set()
  return { get: () => value, subscribe: fn => (fn(value), listeners.add(fn), () => listeners.delete(fn)), listen: fn => (listeners.add(fn), () => listeners.delete(fn)) }
}

function harness({ submit = () => true, routes = ROUTES } = {}) {
  const calls = { submit: [], insert: [], notify: [], workspaces: [], sessions: [], rest: [] }
  globalThis.__kumiTestHost = {
    notify: input => calls.notify.push(input),
    navigate: () => {},
    onEvent: () => () => {},
    composer: {
      submit: (session, text) => (calls.submit.push([session, text]), submit(session, text)),
      insertText: async (session, text) => (calls.insert.push([session, text]), true)
    },
    openWorkspace: (id, options) => (calls.workspaces.push([id, options.title]), () => {}),
    openSession: id => calls.sessions.push(id),
    state: { profile: atom('default'), connectionId: atom('local') }
  }
  const registered = []
  const settings = []
  const events = new Map()
  const intervals = []
  const ctx = {
    source: 'plugin:kumi',
    register: contribution => (registered.push(contribution), () => {}),
    registerMany: contributions => (registered.push(...contributions), () => {}),
    registerSettingsPage: page => (settings.push(page), () => {}),
    onDispose: () => {},
    onEvent: (type, fn) => (events.set(type, fn), () => {}),
    setTimeout: (fn, ms) => {
      const timer = setTimeout(fn, ms)
      return () => clearTimeout(timer)
    },
    setInterval: (_fn, ms) => (intervals.push(ms), () => {}),
    addEventListener: () => () => {},
    rest: async path => {
      calls.rest.push(path)
      if (path in routes) return structuredClone(routes[path])
      throw new Error(`404: {"detail":"no ${path}"}`)
    },
    storage: { get: (_key, fallback) => fallback, set() {}, remove() {} },
    os: { writeClipboard: async () => true }
  }
  return { ctx, calls, registered, settings, events, intervals }
}

const settle = () => new Promise(resolve => setTimeout(resolve, 20))

test('it registers the pane, the chip, its palette commands, the listen card and a settings page', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const h = harness()
  plugin.register(h.ctx)
  assert.equal(plugin.id, 'kumi')
  const byArea = Object.groupBy(h.registered, contribution => contribution.area)
  assert.deepEqual(byArea.panes.map(pane => [pane.id, pane.title, pane.data.placement]), [['pane', 'Kumi', 'right']])
  assert.equal(byArea['statusBar.right'].length, 1)
  assert.deepEqual(byArea.palette.map(entry => entry.data.id), ['kumi.map', 'kumi.undo-last', 'kumi.settings'])
  assert.deepEqual(byArea['transcript.toolMedia'].map(card => card.data.tools), [['kumi_listen']])
  assert.deepEqual(h.settings.map(page => page.id), ['kumi'])
})

test('on a Desktop without the tool-media seam it registers everything else and no card', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx', { sdk: sdkStub({ toolMedia: false, settingsHref: false }) })
  const h = harness()
  delete h.ctx.registerSettingsPage
  plugin.register(h.ctx)
  const areas = h.registered.map(contribution => contribution.area)
  assert.ok(areas.includes('panes') && areas.includes('statusBar.right'))
  assert.ok(!areas.includes('transcript.toolMedia'))
})

test('it reads /status at once, polls it every 5 s, and listens for what Kumi and its tools say moved', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const h = harness()
  plugin.register(h.ctx)
  await settle()
  assert.equal(h.calls.rest[0], '/status')
  assert.ok(h.calls.rest.includes('/focus') && h.calls.rest.includes('/history?limit=50'))
  // The map is read only when something shows it.
  assert.ok(!h.calls.rest.includes('/map'))
  assert.deepEqual(h.intervals, [5000])
  for (const event of ['status', 'focus', 'history', 'map', 'settings'].map(name => `plugin.kumi.${name}.changed`).concat('plugin.kumi.job.progress', 'tool.complete')) {
    assert.ok(h.events.has(event), event)
  }
  // Nothing a read did went anywhere but Kumi's own backend routes.
  assert.ok(h.calls.rest.every(path => path.startsWith('/')))
  assert.equal(h.calls.submit.length + h.calls.insert.length, 0)
})

test("undo puts the newest undoable change's request, with its id, in Kumi's own chat", async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const h = harness()
  plugin.register(h.ctx)
  await settle()
  h.registered.find(contribution => contribution.data?.id === 'kumi.undo-last').data.run()
  await settle()
  assert.deepEqual(h.calls.submit, [[SESSION, 'Undo Kumi change h-p6f1c9e-g2-c14 (“Drums volume −6.0 → −4.0 dB”)']])
})

test("when Kumi's chat can't take a request, it opens that chat, then leaves the words unsent in the input", async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const h = harness({ submit: () => false })
  plugin.register(h.ctx)
  await settle()
  h.registered.find(contribution => contribution.data?.id === 'kumi.undo-last').data.run()
  await new Promise(resolve => setTimeout(resolve, 800))
  assert.deepEqual(h.calls.sessions, [SESSION])
  assert.equal(h.calls.submit.length, 2)
  assert.deepEqual(h.calls.insert, [[null, 'Undo Kumi change h-p6f1c9e-g2-c14 (“Drums volume −6.0 → −4.0 dB”)']])
  assert.equal(h.calls.notify.at(-1).kind, 'warning')
})

test('nothing is undone when Kumi is not connected', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const h = harness({ routes: { ...ROUTES, '/status': { ...ROUTES['/status'], connected: false, stale: true } } })
  plugin.register(h.ctx)
  await settle()
  h.registered.find(contribution => contribution.data?.id === 'kumi.undo-last').data.run()
  await settle()
  assert.equal(h.calls.submit.length, 0)
  assert.equal(h.calls.notify.at(-1).kind, 'info')
})

test('a kumi_sounds result opens the map on what it found; other tools only refresh', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const h = harness()
  plugin.register(h.ctx)
  await settle()
  const complete = h.events.get('tool.complete')
  complete({ payload: { name: 'terminal', result: '{}' } })
  complete({ payload: { name: 'kumi_change', result: { state: 'applied' } } })
  assert.equal(h.calls.workspaces.length, 0)
  complete({ payload: { name: 'kumi_sounds', result: JSON.stringify({ map_revision: 'r7', items: [{ sound_id: 's-r7-0091' }, { sound_id: 's-r7-0092' }] }) } })
  assert.deepEqual(h.calls.workspaces, [['kumi-sound-map', 'Sound map']])
})

test('the built Desktop file uses only the SDK and React, and none of the doors rule 8 closes', () => {
  const bundle = readFileSync(`${root}desktop/plugin.js`, 'utf8')
  const imports = [...bundle.matchAll(/^import\b[^;]*?from\s*["']([^"']+)["']/gm)].map(match => match[1])
  assert.deepEqual([...new Set(imports)].sort(), ['@hermes/plugin-sdk', 'react', 'react/jsx-runtime'])
  for (const [pattern, why] of [
    [/\beval\s*\(/, 'eval'],
    [/new\s+Function\s*\(/, 'new Function'],
    [/\bimport\s*\(/, 'a dynamic import'],
    [/hermesDesktop/, 'the raw Desktop bridge'],
    [/document\s*\.\s*(querySelector|getElementBy|body)/, "the app's markup"],
    [/MutationObserver/, 'a MutationObserver'],
    [/\.prototype\s*\.\s*\w+\s*=/, 'prototype patching'],
    [/createElement\(\s*["']script/, 'a script tag'],
    [/<iframe|<webview|["']iframe["']|["']webview["']/, 'a raw frame']
  ]) {
    assert.ok(!pattern.test(bundle), `desktop/plugin.js contains ${why}`)
  }
})
