// node --test: the real Desktop half (src/desktop/plugin.tsx, bundled by esbuild) registered against a stub SDK, on a
// Desktop with every surface Kumi uses and on an older one without the tool-media seam or settings-page links.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

import { load, plugin, sdkStub } from './load.mjs'

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
  '/focus': { connected: true, focus: { track: { name: 'Drums', kind: 'midi' }, trackRef: 't-3', device: 'Drum Rack', view: 'Session' }, transport: { playing: true, tempo: 124, position: 66 } },
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

test('it registers the pane, the chip, its palette commands, the listen card, the FOCUS hint and a settings page', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const h = harness()
  plugin.register(h.ctx)
  assert.equal(plugin.id, 'kumi')
  const byArea = Object.groupBy(h.registered, contribution => contribution.area)
  assert.deepEqual(byArea.panes.map(pane => [pane.id, pane.title, pane.data.placement]), [['pane', 'Kumi', 'right']])
  assert.equal(byArea['statusBar.right'].length, 1)
  assert.deepEqual(byArea.palette.map(entry => entry.data.id), ['kumi.map', 'kumi.undo-last', 'kumi.settings'])
  assert.deepEqual(byArea['transcript.toolMedia'].map(card => card.data.tools), [['kumi_listen'], ['kumi_job']])
  assert.deepEqual(byArea['composer.underside'].map(hint => hint.id), ['focus-hint'])
  assert.deepEqual(h.settings.map(page => page.id), ['kumi'])
})

test('on a Desktop without the tool-media and composer seams it registers everything else, with no card or hint', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx', { sdk: sdkStub({ toolMedia: false, settingsHref: false, composer: false }) })
  const h = harness()
  delete h.ctx.registerSettingsPage
  plugin.register(h.ctx)
  const areas = h.registered.map(contribution => contribution.area)
  assert.ok(areas.includes('panes') && areas.includes('statusBar.right'))
  assert.ok(!areas.includes('transcript.toolMedia') && !areas.includes('composer.underside'))
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
  assert.deepEqual(h.intervals, [5000, 2000])
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
  assert.deepEqual(h.calls.submit, [[SESSION, 'Undo Kumi change h-p6f1c9e-g2-c14']])
})

test("when Kumi's chat can't take a request, it opens that chat, tries a while, then leaves the words unsent in the input", async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  for (const openSession of [undefined, () => Promise.reject(new Error('no such chat here'))]) {
    const h = harness({ submit: () => false })
    if (openSession) globalThis.__kumiTestHost.openSession = id => (h.calls.sessions.push(id), openSession())
    plugin.register(h.ctx)
    await settle()
    h.registered.find(contribution => contribution.data?.id === 'kumi.undo-last').data.run()
    await new Promise(resolve => setTimeout(resolve, 1400))
    assert.deepEqual(h.calls.sessions, [SESSION])
    assert.ok(h.calls.submit.every(([session]) => session === SESSION), 'never sent from another chat')
    assert.deepEqual(h.calls.insert, [[null, 'Undo Kumi change h-p6f1c9e-g2-c14']])
    assert.equal(h.calls.notify.at(-1).kind, 'warning')
  }
})

test('a request with no chat owning Kumi yet waits unsent in the input in front, for the producer to send', async () => {
  const { ask } = await load('src/desktop/actions.ts')
  const h = harness()
  assert.deepEqual(await ask(h.ctx, 'Set up Kumi for Live', { send: true, session: null }), { outcome: 'drafted' })
  assert.deepEqual([h.calls.submit, h.calls.insert], [[], [[null, 'Set up Kumi for Live']]])
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

test("the palette undoes nothing while Kumi isn't sure of the project, and never past a change it isn't sure of", async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const unsure = { at: 1, op: 'listen_track', reason: 'Live stopped answering', cleanup: 'restored' }
  const newestUnsure = { ...ROUTES['/history?limit=50'], items: [{ id: 'h-c15', state: 'unsure', undo_available: false }, ...ROUTES['/history?limit=50'].items] }
  for (const routes of [
    { ...ROUTES, '/status': { ...ROUTES['/status'], unsure } },
    { ...ROUTES, '/history?limit=50': newestUnsure }
  ]) {
    const h = harness({ routes })
    plugin.register(h.ctx)
    await settle()
    h.registered.find(contribution => contribution.data?.id === 'kumi.undo-last').data.run()
    await settle()
    assert.equal(h.calls.submit.length, 0)
    assert.match(h.calls.notify.at(-1).message, /isn't sure of its last change/)
  }
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

test('a call that starts a job, or a job.progress event, has the pane follow that job until it ends', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  let state = 'running'
  const h = harness({
    routes: {
      ...ROUTES,
      '/job?id=j-scan-1': { job_id: 'j-scan-1', kind: 'scan', get state() { return state }, progress: { done: 412, total: 1200, message: 'Drums' } },
      '/job?id=j-scan-2': { job_id: 'j-scan-2', kind: 'scan', state: 'running', progress: { done: 3 } }
    }
  })
  plugin.register(h.ctx)
  await settle()
  h.events.get('tool.complete')({ payload: { name: 'kumi_library', result: JSON.stringify({ job_id: 'j-scan-1', state: 'running' }) } })
  h.events.get('plugin.kumi.job.progress')({ payload: { profile: 'default', job_id: 'j-scan-2', state: 'running', progress: { done: 3 } } })
  h.events.get('plugin.kumi.job.progress')({ payload: { profile: 'work', job_id: 'j-work-1', state: 'running' } })
  h.events.get('plugin.kumi.job.progress')({ payload: { job_id: '../owner' } })
  await settle()
  assert.deepEqual(h.calls.rest.filter(path => path.startsWith('/job')).sort(), ['/job?id=j-scan-1', '/job?id=j-scan-2'], "another profile's job is never read")
})

test('a generate.updated event has the pane follow its generation, never one of another profile, and sends nothing', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const h = harness({ routes: { ...ROUTES, '/job?id=native-job-id': { job_id: 'native-job-id', kind: 'generate', state: 'running', progress: { done: 0 } } } })
  plugin.register(h.ctx)
  await settle()
  const event = h.events.get('plugin.kumi.generate.updated')
  const card = { state: 'running', job_id: 'native-job-id', quote_id: 'q-opaque-id', kind: 'sfx', estimate: { credits: 15, max_credits: 15 } }
  event({ payload: { profile: 'work', session_id: SESSION, tool_call_id: 'call-8', tool: 'kumi_generate', card: { ...card, job_id: 'work-job' } } })
  event({ payload: { profile: 'default', session_id: SESSION, tool_call_id: 'call-9', tool: 'kumi_generate', card } })
  await settle()
  assert.deepEqual(h.calls.rest.filter(path => path.startsWith('/job')), ['/job?id=native-job-id'])
  assert.equal(h.calls.submit.length + h.calls.insert.length, 0, 'an event never places or sends anything')
})

test('the built Desktop file uses only the SDK and React, and none of the doors rule 8 closes', () => {
  const bundle = readFileSync(`${plugin}desktop/plugin.js`, 'utf8')
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

// CC-9: the plugin's own events for the chat in front, when Hermes sends no tool.complete (tool progress off).
// The window's own profile is `host.state.profile` ('default' here); a Desktop without workspace tabs knows no
// focusedSessionProfile, which the gate doesn't need.
function inFront(h, { chat = SESSION, focusedProfile } = {}) {
  globalThis.__kumiTestHost.state = { ...globalThis.__kumiTestHost.state, focusedStoredSessionId: atom(chat) }
  if (focusedProfile !== undefined) globalThis.__kumiTestHost.state.focusedSessionProfile = atom(focusedProfile)
  return h
}

const found = (over = {}) => ({ payload: { profile: 'default', session_id: SESSION, tool_call_id: 'call-1', map_revision: 'r7', sound_ids: ['s-r7-0091'], ...over } })

test('a sounds.found event opens the map for the chat in front, once with its tool.complete, and never for another profile or chat', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const h = inFront(harness())
  plugin.register(h.ctx)
  await settle()
  const event = h.events.get('plugin.kumi.sounds.found')
  event(found({ profile: 'work' }))
  event(found({ session_id: 'another-chat' }))
  event(found({ profile: undefined }))
  event(found({ tool_call_id: 'call-0', sound_ids: [] }))
  assert.equal(h.calls.workspaces.length, 0)
  event(found())
  h.events.get('tool.complete')({ payload: { name: 'kumi_sounds', tool_id: 'call-1', result: { map_revision: 'r7', items: [{ sound_id: 's-r7-0091' }] } } })
  assert.equal(h.calls.workspaces.length, 1, 'the event and the same call\'s tool.complete open the map once')
  h.events.get('tool.complete')({ payload: { name: 'kumi_sounds', tool_id: 'call-2', result: { map_revision: 'r7', items: [{ sound_id: 's-r7-0092' }] } } })
  event(found({ tool_call_id: 'call-2', sound_ids: ['s-r7-0092'] }))
  assert.equal(h.calls.workspaces.length, 2, 'in either order')
})

test('a listen.done event opens Measurements only when its call\'s tool.complete doesn\'t come within 2 s, and only for the chat in front', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const h = inFront(harness())
  const timers = []
  h.ctx.setTimeout = (fn, ms) => {
    const timer = { fn, ms, cancelled: false }
    timers.push(timer)
    return () => (timer.cancelled = true)
  }
  plugin.register(h.ctx)
  await settle()
  const done = h.events.get('plugin.kumi.listen.done')
  const result = { metrics: { file: 'Dusty Snare 04.wav', loudness: { integratedLufs: -14.2 } }, artifact_id: 'a-1' }
  done({ payload: { profile: 'work', session_id: SESSION, tool_call_id: 'call-3', result } })
  done({ payload: { profile: 'default', session_id: 'another-chat', tool_call_id: 'call-3', result } })
  assert.equal(timers.filter(timer => timer.ms === 2000).length, 0)
  done({ payload: { profile: 'default', session_id: SESSION, tool_call_id: 'call-3', result } })
  h.events.get('tool.complete')({ payload: { name: 'kumi_listen', tool_id: 'call-3', result } })
  const waited = timers.filter(timer => timer.ms === 2000)
  assert.deepEqual(waited.map(timer => timer.cancelled), [true])
  waited[0].fn()
  assert.ok(!h.calls.workspaces.some(([id]) => id === 'kumi-measurements'), 'the transcript shows the card: no tab')
  done({ payload: { profile: 'default', session_id: SESSION, tool_call_id: 'call-4', result } })
  timers.filter(timer => timer.ms === 2000 && !timer.cancelled).at(-1).fn()
  assert.deepEqual(h.calls.workspaces.filter(([id]) => id === 'kumi-measurements'), [['kumi-measurements', 'Measurements']])
})

test("events follow the profile this window's Kumi reads from, even when Bot Mode puts another profile's chat in front", async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const h = inFront(harness(), { focusedProfile: 'work' })
  plugin.register(h.ctx)
  await settle()
  const event = h.events.get('plugin.kumi.sounds.found')
  event(found({ profile: 'work' }))
  assert.equal(h.calls.workspaces.length, 0, "the focused chat's profile isn't the one whose map this window reads")
  event(found())
  assert.deepEqual(h.calls.workspaces, [['kumi-sound-map', 'Sound map']])
  // Another profile's change moves nothing here; one that names no profile (a runtime before #966) still re-reads.
  const reads = () => h.calls.rest.filter(path => path === '/focus').length
  const before = reads()
  h.events.get('plugin.kumi.focus.changed')({ payload: { profile: 'work', seq: 9 } })
  await settle()
  assert.equal(reads(), before)
  h.events.get('plugin.kumi.focus.changed')({ payload: { seq: 9 } })
  await settle()
  assert.equal(reads(), before + 1)
})

test('a profile switch drops the measurement waiting on the old one, and a result that is no measurement opens nothing', async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx')
  const listeners = new Set()
  let current = 'default'
  const profile = { get: () => current, subscribe: fn => (fn(current), listeners.add(fn), () => listeners.delete(fn)), listen: fn => (listeners.add(fn), () => listeners.delete(fn)) }
  const h = inFront(harness())
  globalThis.__kumiTestHost.state.profile = profile
  const timers = []
  h.ctx.setTimeout = (fn, ms) => {
    const timer = { fn, ms, cancelled: false }
    timers.push(timer)
    return () => (timer.cancelled = true)
  }
  plugin.register(h.ctx)
  await settle()
  const done = h.events.get('plugin.kumi.listen.done')
  done({ payload: { profile: 'default', session_id: SESSION, tool_call_id: 'call-5', result: 'not a measurement' } })
  assert.equal(timers.filter(timer => timer.ms === 2000).length, 0)
  done({ payload: { profile: 'default', session_id: SESSION, tool_call_id: 'call-6', result: { metrics: { file: 'Kick.wav', loudness: { integratedLufs: -9 } } } } })
  current = 'work'
  for (const fn of listeners) fn(current)
  assert.deepEqual(timers.filter(timer => timer.ms === 2000).map(timer => timer.cancelled), [true])
  // The same call id on the new profile is a new call: it isn't taken for one already acted on.
  done({ payload: { profile: 'work', session_id: SESSION, tool_call_id: 'call-6' } })
  timers.filter(timer => timer.ms === 2000 && !timer.cancelled).at(-1).fn()
  assert.deepEqual(h.calls.workspaces.filter(([id]) => id === 'kumi-measurements'), [['kumi-measurements', 'Measurements']], 'a result too large to carry still opens the tab')
})

test("with Desktop's plugin i18n it registers Kumi's words, and labels the palette in the app's language, again on a switch", async () => {
  const { default: plugin } = await load('src/desktop/plugin.tsx', { sdk: sdkStub({ i18n: true }) })
  const h = harness()
  let words = { 'palette.map': 'Kumi: サンプルマップを開く', 'palette.undo': 'Kumi: 最後の変更を取り消す' }
  let bundles = null
  const switches = []
  const active = new Set()
  h.ctx.registerMany = contributions => {
    for (const contribution of contributions) active.add(contribution)
    return () => contributions.forEach(contribution => active.delete(contribution))
  }
  h.ctx.i18n = { register: given => ((bundles = given), () => {}), t: key => words[key] ?? key, onLocaleChange: fn => (switches.push(fn), () => {}) }
  plugin.register(h.ctx)
  assert.deepEqual(Object.keys(bundles).sort(), ['en', 'ja', 'zh'])
  const palette = () => [...active].filter(contribution => contribution.area === 'palette').map(entry => entry.data.label)
  assert.deepEqual(palette().slice(0, 2), ['Kumi: サンプルマップを開く', 'Kumi: 最後の変更を取り消す'])
  words = { 'palette.map': 'Kumi：打开采样地图' }
  for (const fn of switches) fn()
  assert.equal(palette().length, 3, 'the old labels are gone, not doubled')
  assert.equal(palette()[0], 'Kumi：打开采样地图')
})
