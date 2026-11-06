// node --test: the listen card, rendered with React to static markup: what it says for a comparison, how it labels a
// model's reading, and that anything it can't draw keeps Desktop's own player.
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { load } from './load.mjs'

const { ListenCard } = await load('src/desktop/card.tsx', { react: 'real' })
const { Audition } = await load('src/shared/hooks.ts', { react: 'real' })
const { KumiStore } = await load('src/shared/store.ts')
// The catalog's English, so a test checks what a surface says by its meaning, not by one wording of it.
const { english } = await load('src/shared/i18n.ts')

const analysis = (lufs, peak, centroid, file) => ({
  file,
  seconds: 0.6,
  loudness: { integratedLufs: lufs, rangeLu: null, truePeakDbtp: peak },
  balance: { centroidHz: centroid, bands: [] },
  dynamics: { crestDb: 14.2 },
  tempo: null,
  key: null
})

const result = {
  metrics: analysis(-14.2, -0.8, 2380, '/s/Dusty Snare 04.wav'),
  artifact_id: 'a-7f32',
  verdict_text: 'Dusty Snare 04 is darker than Snare 01.',
  media_tag: 'MEDIA:/tmp/a.wav',
  compare: {
    artifact_id: 'a-7f31',
    metrics: analysis(-12.9, -0.3, 3520, '/s/Snare 01.wav'),
    differences: { headlines: ['Darker: less presence'], loudness: { differenceLu: -1.3 }, balance: [{ band: 'presence', hz: '2k–4k', difference: -5 }] }
  }
}

function render(props, store = new KumiStore(async () => ({}))) {
  const audition = new Audition(async () => ({ data_url: 'data:audio/wav;base64,' }))
  const media = {
    file: { path: '/tmp/a.wav', name: 'a.wav', kind: 'audio', mime: 'audio/wav', resolveUrl: async () => '' },
    toolName: 'kumi_listen',
    toolCallId: 'call-1',
    args: {},
    sessionId: 's-1',
    fallback: createElement('span', null, 'BUILT-IN PLAYER'),
    ...props
  }
  return renderToStaticMarkup(createElement(ListenCard, { media, audition, store }))
}

test('a comparison shows both files, the measured numbers side by side and the differences', () => {
  const html = render({ result: JSON.stringify(result) })
  for (const text of ['Dusty Snare 04.wav', 'vs Snare 01.wav', '−14.2 LUFS', '−12.9 LUFS', '−1.3 LU', '−1,140 Hz', 'Darker: less presence', 'measured', 'Dusty Snare 04 is darker']) {
    assert.ok(html.includes(text), `${text} in ${html}`)
  }
  assert.ok(!html.includes('BUILT-IN PLAYER'))
  assert.ok(!html.includes('Interpretation'))
  assert.ok(!/\bheard\b/i.test(html))
})

test('a mastering round says how far each comparison so far was from the reference, over both spectra', () => {
  const bands = db => [{ name: 'sub', hz: '20–60', db }, { name: 'bass', hz: '60–120', db: db + 4 }]
  const pass = (lufs, artifact) => ({
    ...result,
    artifact_id: artifact,
    metrics: { ...analysis(lufs, -1, 2000, '/mix/Mix.wav'), balance: { centroidHz: 2000, tiltDbPerOctave: -3.9, bands: bands(-12) } },
    compare: {
      artifact_id: `r-${artifact}`,
      metrics: { ...analysis(-8.6, -0.2, 2600, '/refs/Ref.wav'), balance: { centroidHz: 2600, tiltDbPerOctave: -3.1, bands: bands(-10) } },
      differences: { loudness: { differenceLu: Math.round((lufs + 8.6) * 10) / 10 } }
    }
  })
  const store = new KumiStore(async () => ({}))
  store.noteListen(pass(-12.7, 'a-1'))
  store.noteListen(pass(-10.2, 'a-2'))
  const html = render({ result: pass(-10.2, 'a-2') }, store)
  assert.ok(html.includes(english('listen.round', 2)), html)
  assert.ok(html.includes('−4.1 → −1.6'), html)
  assert.ok(html.includes(english('listen.overlay')))
  // A comparison this window didn't see arrive (a chat opened later) isn't numbered, and the first one has no trend.
  assert.ok(!render({ result: pass(-9.0, 'a-9') }, store).includes(english('listen.round', 3)))
  assert.ok(!render({ result: pass(-12.7, 'a-1') }, store).includes(english('listen.round', 1)))
  // From round 2 the skill compares against the copy Kumi kept (`compare_to: {"artifact_id"}`), measured under the
  // copy's own name: still this reference's next round, shown under the reference's name.
  const third = pass(-9.4, 'a-3')
  const againstCopy = { ...third, compare: { ...third.compare, artifact_id: 'r-a-1', metrics: { ...third.compare.metrics, file: 'r-a-1.wav' } } }
  store.noteListen(againstCopy)
  const html3 = render({ result: againstCopy }, store)
  assert.ok(html3.includes(english('listen.round', 3)), html3)
  assert.ok(html3.includes('vs Ref.wav') && !html3.includes('r-a-1.wav'), html3)
})

test("a model's reading sits under a plain heading with its model, its own opening line said once", () => {
  const opening = 'Model interpretation of measurements (no raw audio heard)'
  const read = render({ result: { ...result, interpretation: { text: `${opening}\nLess upper-mid energy.`, model: 'm-1' } } })
  assert.ok(read.includes('Interpretation · m-1'))
  assert.equal(read.split(opening).length - 1, 1)
  assert.ok(read.includes('Less upper-mid energy.'))
  const failed = render({ result: { ...result, interpretation_error: 'the model timed out' } })
  assert.ok(failed.includes('No model interpretation: the model timed out'))
})

test("what the card can't draw keeps Desktop's own player", () => {
  assert.equal(render({ result: 'not json' }), '<span>BUILT-IN PLAYER</span>')
  assert.equal(render({ result: { error: 'Kumi refused that file' } }), '<span>BUILT-IN PLAYER</span>')
  assert.equal(render({ result, file: { kind: 'image', path: '/x.png', name: 'x.png', mime: 'image/png', resolveUrl: async () => '' } }), '<span>BUILT-IN PLAYER</span>')
})

test('a HISTORY row Kumi is unsure of says why, what it put back, and offers no undo', async () => {
  const { HistoryView } = await load('src/shared/HistoryView.tsx', { react: 'real' })
  const item = { id: 'h-1', label: 'c15', title: 'Listen to Drums, beats 0–8', state: 'unsure', undo_available: false, reason: 'Live stopped answering during the capture', cleanup: 'unsure' }
  const html = renderToStaticMarkup(createElement(HistoryView, { history: { items: [item] }, action: row => (row.undo_available ? createElement('button', null, 'Undo') : null) }))
  for (const text of ['Listen to Drums, beats 0–8', 'Live stopped answering during the capture', 'isn’t sure it put everything back', 'look again before another change']) {
    assert.ok(html.includes(text), `${text} in ${html}`)
  }
  assert.ok(!html.includes('Undo'))
})

test("a change only Live's own Undo can take back (CC-20) says how, with the DAW machine's key or by hand, and offers no Undo", async () => {
  const { HistoryView } = await load('src/shared/HistoryView.tsx', { react: 'real' })
  // A pad load is two Live steps: the note counts them, and the chip leaves the count to it.
  const note = "Kumi can't undo this change. Live's own Undo (Cmd+Z) takes it back in 2 steps while they're Live's latest: press Cmd+Z twice, right away."
  const esc = text => text.replaceAll("'", '&#x27;')
  const shown = esc(note)
  const row = how => ({ id: 'h-1', title: 'Chain added to Drum Rack', family: 'device', state: 'applied', undo_available: false, reason: 'daw_undo', note, daw_undo: how })
  const draw = (how, target) => renderToStaticMarkup(createElement(HistoryView, { history: { items: [row(how)] }, daw: 'ableton', live: true, target, action: () => createElement('button', null, 'Undo') }))
  // The key is the one on the machine Kumi and Live run on, whatever machine the page is open on.
  const next = draw('next', 'aarch64-apple-darwin')
  assert.ok(next.includes(shown), next)
  assert.ok(next.includes(esc(english('history.dawUndo', 'Live'))) && next.includes(esc(english('history.dawUndoTitle', 'Live', 'Cmd+Z'))), next)
  assert.ok(draw('next', 'x86_64-pc-windows-msvc').includes(esc(english('history.dawUndoTitle', 'Live', 'Ctrl+Z'))))
  assert.ok(!next.includes('<button'), 'words, never a request Kumi would refuse')
  // A later Kumi change sits above it: Live's undo would take that back first, so no key is offered.
  const buried = draw('buried', 'aarch64-apple-darwin')
  assert.ok(buried.includes(english('history.byHand', 'Live')) && !/\+Z/.test(buried.replace(shown, '')), buried)
  // Serve doesn't know (after a restart): the note alone, still no Undo.
  const unknown = draw(undefined, 'aarch64-apple-darwin')
  assert.ok(unknown.includes(shown) && !unknown.includes(esc(english('history.dawUndo', 'Live'))))
  // A row without daw_undo that Kumi can undo keeps the surface's own action.
  assert.ok(renderToStaticMarkup(createElement(HistoryView, { history: { items: [{ ...row(undefined), undo_available: true }] }, action: () => createElement('button', null, 'Undo') })).includes('<button'))
})

test("the pane warns while Kumi isn't sure of the project and offers no Undo, until a completed observe clears it", async () => {
  const { KumiPane } = await load('src/desktop/pane.tsx', { react: 'real' })
  const { KumiStore } = await load('src/shared/store.ts', { react: 'real' })
  const unsure = { at: Date.now(), op: 'listen_track', reason: 'Live stopped answering during the capture', cleanup: 'restored' }
  const render = async status => {
    const store = new KumiStore(async path => {
      if (path.startsWith('/status')) return { connected: true, stale: false, daw: 'ableton', owner: { session_id: 's1' }, revisions: { history: 1 }, ...status }
      if (path.startsWith('/history')) return { seq: 1, items: [{ id: 'h-1', title: 'Drums volume', state: 'applied', undo_available: true }] }
      throw Object.assign(new Error('not here'), { status: 404 })
    })
    await store.refreshStatus()
    return renderToStaticMarkup(createElement(KumiPane, { ctx: {}, store, onOpenMap() {} }))
  }
  const warned = await render({ unsure })
  for (const text of ['listen_track', 'Live stopped answering during the capture', 'Kumi put back what it changed meanwhile.', english('pane.lookAgain')]) {
    assert.ok(warned.includes(text), `${text} in the pane`)
  }
  assert.ok(!warned.includes('>Undo<'))
  const cleared = await render({ unsure: null })
  assert.ok(!cleared.includes(english('pane.lookAgain')))
  assert.ok(cleared.includes('>Undo<'))
})

test("while Kumi is connected through a current DAW part setup didn't install, the pane doesn't ask to install it; an older one still asks", async () => {
  const { KumiPane } = await load('src/desktop/pane.tsx', { react: 'real' })
  const { KumiStore } = await load('src/shared/store.ts', { react: 'real' })
  const setup = { state: 'needs_daw_component', daw: 'ableton', message: "Kumi's runtime is ready; next, install Kumi's part in Live." }
  const render = async (status, answer = setup) => {
    const store = new KumiStore(async path => {
      if (path.startsWith('/status')) return { stale: false, daw: 'ableton', revisions: {}, ...status }
      if (path.startsWith('/setup')) return answer
      throw Object.assign(new Error('not here'), { status: 404 })
    })
    await store.refreshStatus()
    return renderToStaticMarkup(createElement(KumiPane, { ctx: {}, store, onOpenMap() {} }))
  }
  const asks = html => html.includes('Set up Kumi for Live') || html.includes("install Kumi's part")
  const part = (installed, version) => ({ setup: { component: { installed, version, bundled: '1.0.104' }, daw_open: true } })
  // Connected through the part this Kumi bundles (an adopted Kumi's, or one put in place by hand): it evidently works.
  assert.equal(asks(await render({ connected: true, owner: { session_id: 's1' }, ...part(true, '1.0.104') })), false)
  // A runtime that doesn't report its part: nothing says it's out of date.
  assert.equal(asks(await render({ connected: true })), false)
  // Connected through an older part (just after a bridge bump), or none the runtime can see: the update is next.
  assert.equal(asks(await render({ connected: true, ...part(true, '1.0.103') })), true)
  assert.equal(asks(await render({ connected: true, ...part(false, null) })), true)
  // Not connected, or missing the runtime itself, setup is still the next step.
  assert.equal(asks(await render({ connected: false, ...part(true, '1.0.104') })), true)
  assert.equal(asks(await render({ connected: true, ...part(true, '1.0.104') }, { ...setup, state: 'needs_runtime' })), true)
})

test('"Last heard" counts from when the runtime was last heard, so it keeps moving while nothing new is read', async () => {
  const { FocusView } = await load('src/shared/HistoryView.tsx', { react: 'real' })
  const heardAt = 1_000_000_000
  const focus = { stale: true, heard_at: heardAt, focus: { track: { name: 'Drums' } } }
  const at = now => renderToStaticMarkup(createElement(FocusView, { focus, now }))
  assert.ok(at(heardAt + 60_000).includes('Last heard 1 min ago.'))
  assert.ok(at(heardAt + 3 * 3_600_000).includes('Last heard 3 h ago.'))
})

test("under the composer, the FOCUS hint shows what Kumi sees only in Kumi's own chat", async () => {
  const { FocusHint } = await load('src/desktop/composer.tsx', { react: 'real' })
  const { KumiStore } = await load('src/shared/store.ts', { react: 'real' })
  const focus = { focus: { track: { name: 'Drums', color: '#f59e0b' }, device: 'Drum Rack' }, transport: { tempo: 124, playing: true } }
  const store = new KumiStore(async path => {
    if (path.startsWith('/status')) return { connected: true, daw: 'ableton', owner: { session_id: 's1' }, revisions: { focus: 3 } }
    if (path.startsWith('/focus')) return focus
    throw Object.assign(new Error('not here'), { status: 404 })
  })
  store.want('focus')
  await store.refreshStatus()
  await new Promise(resolve => setTimeout(resolve, 20))
  const hint = chat => {
    globalThis.__kumiTestHost = { state: { focusedStoredSessionId: { get: () => chat, subscribe: () => () => {}, listen: () => () => {} } } }
    return renderToStaticMarkup(createElement(FocusHint, { store }))
  }
  const shown = hint('s1')
  for (const text of ['Kumi sees ', 'Drums › Drum Rack', '124 BPM']) assert.ok(shown.includes(text), `${text} in ${shown}`)
  assert.equal(hint('another-chat'), '')
  assert.equal(hint(null), '')
})

test("while Kumi isn't sure of the project, the hint says what it last saw, dimmed", async () => {
  const { FocusHint } = await load('src/desktop/composer.tsx', { react: 'real' })
  const { KumiStore } = await load('src/shared/store.ts', { react: 'real' })
  const unsure = { op: 'listen_track', reason: 'Live stopped answering', cleanup: 'unsure' }
  const store = new KumiStore(async path => {
    if (path.startsWith('/status')) return { connected: true, daw: 'ableton', owner: { session_id: 's1' }, revisions: { focus: 3 }, unsure }
    if (path.startsWith('/focus')) return { focus: { track: { name: 'Drums' }, device: 'Drum Rack' } }
    throw Object.assign(new Error('not here'), { status: 404 })
  })
  store.want('focus')
  await store.refreshStatus()
  await new Promise(resolve => setTimeout(resolve, 20))
  globalThis.__kumiTestHost = { state: { focusedStoredSessionId: { get: () => 's1', subscribe: () => () => {}, listen: () => () => {} } } }
  const html = renderToStaticMarkup(createElement(FocusHint, { store }))
  assert.ok(html.includes('Kumi last saw Drums › Drum Rack'), html)
  assert.ok(html.includes('opacity:0.5'), html)
})

test('FOCUS reads Kumi\'s own shape: Live\'s fields and the neutral ones, the parameter, a busy DAW and every catch-up', async () => {
  const { FocusView } = await load('src/shared/HistoryView.tsx', { react: 'real' })
  const live = {
    project: { name: 'Night Drive', key: 'p-1', epoch: 3 },
    focus: { track: { name: 'Drums', color: '#f59e0b', kind: 'midi' }, trackRef: 't-3', device: 'Drum Rack', view: 'Session', scene: 'Verse', parameter: { name: 'Filter Freq', value: '1.20 kHz', owner: 'Auto Filter' } },
    transport: { playing: false, tempo: 124, position: 66, beatsPerBar: 4 },
    catch_up: [
      { set: 'Night Drive', lastSeenAt: 1, lines: ['Drums: pad D1 changed', 'Tempo 120 → 124 BPM'], more: 3 },
      { set: 'Night Drive', lastSeenAt: 2, lines: ['Bass muted'], more: 0, afterReconnect: true }
    ]
  }
  const html = renderToStaticMarkup(createElement(FocusView, { focus: live, now: 0 }))
  for (const text of ['Drums › Drum Rack', 'Filter Freq 1.20 kHz', 'Night Drive', 'Session', 'Verse', '■ stopped', '124 BPM', 'Drums: pad D1 changed', 'Bass muted', 'and 3 more']) {
    assert.ok(html.includes(text), `${text} in ${html}`)
  }
  assert.ok(!html.includes('66'), 'the position a snapshot was written at is never shown')
  const reaper = {
    focus: { track: { name: 'Vox' }, mainView: { kind: 'timeline', label: 'Arrange' }, item: { kind: 'item', name: 'Take 3' }, section: { kind: 'marker', name: 'Chorus' }, busy: 'a dialog is open' },
    transport: { playing: true, tempo: 98, meter: '7/8', line: 12 }
  }
  const neutral = renderToStaticMarkup(createElement(FocusView, { focus: reaper, now: 0 }))
  for (const text of ['Vox › Take 3', 'Arrange', 'Chorus', '▶ playing', '98 BPM', '7/8', 'a dialog is open']) {
    assert.ok(neutral.includes(text), `${text} in ${neutral}`)
  }
})

test('FOCUS says Kumi stopped (not "not answering") once it stopped, and keeps the catch-up when the DAW reports no focus', async () => {
  const { FocusView } = await load('src/shared/HistoryView.tsx', { react: 'real' })
  const stopped = renderToStaticMarkup(createElement(FocusView, { focus: { stale: true, running: false, focus: { track: { name: 'Drums' } } }, now: 0 }))
  assert.ok(stopped.includes('Kumi stopped') && !stopped.includes('not answering'), stopped)
  const closed = { project: { name: 'Night Drive' }, focus: null, catch_up: [{ set: 'Night Drive', lines: ['Bass muted'], more: 0 }] }
  const html = renderToStaticMarkup(createElement(FocusView, { focus: closed, now: 0 }))
  for (const text of ['Nothing selected', 'Night Drive', 'Since Kumi last looked', 'Bass muted']) assert.ok(html.includes(text), `${text} in ${html}`)
  const reaper = { focus: { track: { name: 'Vox' }, mainView: { kind: 'timeline', label: 'Arrange' }, section: { kind: 'marker', name: 'Arrange' } }, transport: { playing: true, line: 12 } }
  const neutral = renderToStaticMarkup(createElement(FocusView, { focus: reaper, now: 0 }))
  assert.ok(!/>[^<]*\b12\b[^<]*</.test(neutral), 'the line a snapshot was written at is never shown')
  assert.equal(neutral.split('Arrange').length - 1, 2, 'a view and a section of the same name are both shown')
})

test('the pane shows a running scan with how far it got, and how a finished one ended', async () => {
  const { JobsView } = await load('src/desktop/jobs.tsx', { react: 'real' })
  const jobs = {
    'j-1': { job_id: 'j-1', kind: 'scan', state: 'running', progress: { done: 412, total: 1200, message: 'Drums' }, ended_at: null },
    'j-2': { job_id: 'j-2', kind: 'scan', state: 'failed', error: { reason: 'the folder went away' }, ended_at: 1 },
    'j-3': { job_id: 'j-3', kind: 'scan', state: 'done', result: { sounds: 637, measured: 637, failed: 3, map_revision: 'r8' }, ended_at: 1 }
  }
  const html = renderToStaticMarkup(createElement(JobsView, { jobs }))
  for (const text of ['Scanning', 'Drums · 412 of 1,200', 'aria-label="Scanning: 34%"', 'width:34%', 'Scan failed', 'the folder went away', 'Scan done', "637 sounds · 3 couldn&#x27;t be read"]) {
    assert.ok(html.includes(text), `${text} in ${html}`)
  }
  assert.equal(renderToStaticMarkup(createElement(JobsView, { jobs: {} })), '')
})

test("the pad picker lays out Live's first sixteen pads as Live's Drum Rack does, and another DAW's by note", async () => {
  const { PadPicker } = await load('src/desktop/map.tsx', { react: 'real' })
  const labels = daw => [...renderToStaticMarkup(createElement(PadPicker, { daw, onPick() {}, onClose() {} })).matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map(match => match[1])
  // Top row first, as on screen: D#2 top right, C1 bottom left.
  assert.deepEqual(labels('ableton'), ['C2', 'C#2', 'D2', 'D#2', 'G#1', 'A1', 'A#1', 'B1', 'E1', 'F1', 'F#1', 'G1', 'C1', 'C#1', 'D1', 'D#1'])
  assert.deepEqual(labels('renoise').slice(-4), ['36', '37', '38', '39'])
})

test("a pending script (CC-19) shows Hermes's claim as one, its facts, the approval's warning and every numbered line", async () => {
  const { PendingScriptView } = await load('src/desktop/script.tsx', { react: 'real' })
  const sha = 'f'.repeat(64)
  const pending = { tool_call_id: 'call-7', language: 'lua', sha256: sha, purpose: 'Rename every pattern', lines: 3, warning: "Runs with the DAW's access to your computer; Kumi can't undo it" }
  // A right-to-left override (Trojan Source) that got past serve is drawn where it is, never applied.
  const code = 'for i = 1, #song.patterns do\n  song.patterns[i].name = "Part " .. i -- \u202E edit\nend\n'
  const html = renderToStaticMarkup(createElement(PendingScriptView, { daw: 'renoise', script: { pending, code: { sha256: sha, language: 'lua', code, lines: 3 } } }))
  for (const text of [english('script.says', 'Rename every pattern'), 'Lua in Renoise', '3 lines', 'SHA-256 ffffffffffff', 'Kumi can&#x27;t undo it', 'for i = 1, #song.patterns do', '>3<', 'U+202E']) {
    assert.ok(html.includes(text), `${text} in ${html}`)
  }
  assert.ok(!html.includes('\u202E'), 'the override character itself never reaches the page')
  // While its code is read, and when it can't be, the section says so rather than showing anything else.
  const reading = renderToStaticMarkup(createElement(PendingScriptView, { daw: 'renoise', script: { pending, code: null } }))
  assert.ok(reading.includes(english('script.reading')) && !reading.includes('<pre'))
  const unreadable = renderToStaticMarkup(createElement(PendingScriptView, { daw: 'renoise', script: { pending, code: false } }))
  assert.ok(unreadable.includes(english('script.unreadable').replaceAll("'", '&#x27;')) && !unreadable.includes('<pre'))
})

test('a pending device (CC-22) shows its file and its controls, each with its range and default as the approval writes them, beside its GenExpr code', async () => {
  const { PendingScriptView } = await load('src/desktop/script.tsx', { react: 'real' })
  const { pendingScriptOf } = await load('src/shared/store.ts', { react: 'real' })
  const spec = {
    name: 'Soft Clip',
    about: 'Gentle saturation',
    controls: [
      { name: 'Tone', type: 'number', min: 200, max: 8000, default: 1000, unit: 'Hz' },
      { name: 'Shape', type: 'choice', options: ['Soft', 'Hard'], default: 'Soft' },
      { name: 'Bypass', type: 'switch', default: false }
    ],
    code: 'out1 = tanh(in1 * (1 + Drive * 4));\nout2 = tanh(in2 * (1 + Drive * 4));\n'
  }
  const sha = 'd'.repeat(64)
  const file = 'User Library/Kumi/Audio Effects/Soft Clip.amxd'
  // The event as the plugin sends it: the pane keeps the file the approval names.
  const pending = pendingScriptOf({ tool_call_id: 'call-9', language: 'device', sha256: sha, purpose: 'Gentle saturation', warning: 'Writes a new Max for Live device into your User Library (Kumi folder)', file })
  const html = renderToStaticMarkup(createElement(PendingScriptView, { daw: 'ableton', script: { pending, code: { sha256: sha, language: 'device', code: JSON.stringify(spec), lines: 2 } } }))
  const switched = `${english('script.on')}/${english('script.off')}`
  for (const text of ['Soft Clip', english('script.device'), english('script.file', file), '200–8000 Hz', '>1000 Hz<', '>Soft/Hard<', '>Soft<', `>${switched}<`, `>${english('script.off')}<`, 'out1 = tanh(in1 * (1 + Drive * 4));', '>2<', 'User Library (Kumi folder)']) {
    assert.ok(html.includes(text), `${text} in ${html}`)
  }
  // A file with a hidden character (here a right-to-left override) isn't shown.
  assert.equal(pendingScriptOf({ tool_call_id: 'call-9', language: 'device', sha256: sha, file: 'Soft\u202eClip.amxd' }).file, undefined)
  // A file that isn't a device's spec (no code) shows no code, only that it can't be read.
  const broken = renderToStaticMarkup(createElement(PendingScriptView, { daw: 'ableton', script: { pending, code: { sha256: sha, language: 'device', code: '{"name":"x"}', lines: 0 } } }))
  assert.ok(broken.includes(english('script.unreadable').replaceAll("'", '&#x27;')) && !broken.includes('<pre'))
})

test('the Measurements tab draws the latest measurement an event brought, or says where the numbers are', async () => {
  const { MeasurementsWorkspace } = await load('src/desktop/measurements.tsx', { react: 'real' })
  const { KumiStore } = await load('src/shared/store.ts', { react: 'real' })
  const store = new KumiStore(async () => ({}))
  const audition = new Audition(async () => ({ data_url: 'data:audio/wav;base64,' }))
  const draw = () => renderToStaticMarkup(createElement(MeasurementsWorkspace, { store, audition }))
  assert.ok(draw().includes('Nothing measured yet'))
  store.measure('call-4', JSON.stringify(result))
  const drawn = draw()
  assert.ok(drawn.includes('Dusty Snare 04.wav') && drawn.includes('−14.2 LUFS'), drawn)
  store.measure('call-5', null)
  assert.ok(draw().includes('the numbers are in its reply in the chat'))
})

test('Settings says whether Kumi can read compressed audio, in setup\'s own words (CC-10), and nothing when setup says nothing', async () => {
  const { SettingsPage } = await load('src/desktop/settings.tsx', { react: 'real' })
  const { KumiStore } = await load('src/shared/store.ts', { react: 'real' })
  const page = async setup => {
    const store = new KumiStore(async path => {
      if (path.startsWith('/setup')) return setup
      throw Object.assign(new Error('not here'), { status: 404 })
    })
    await store.refresh('setup')
    const ctx = { storage: { get: (_key, fallback) => fallback, set: () => {} } }
    return renderToStaticMarkup(createElement(SettingsPage, { ctx, store, onOpenMap: () => {} }))
  }
  const message = 'Compressed audio (MP3, FLAC…): no. Kumi reads WAV and AIFF only; to let Kumi use the ffmpeg at /opt/homebrew/bin/ffmpeg, run hermes kumi setup (or ask Hermes to set up Kumi).'
  const html = await page({ state: 'ready', compressed_audio: { state: 'not_allowed', ffmpeg: null, found: '/opt/homebrew/bin/ffmpeg', message } })
  assert.ok(html.includes('Audio formats'), html)
  assert.ok(html.includes('Kumi reads WAV and AIFF only; to let Kumi use the ffmpeg at /opt/homebrew/bin/ffmpeg'), html)
  assert.ok(!(await page({ state: 'ready' })).includes('Audio formats'))
})

test("Settings doesn't tell a producer connected through a current Live part to install it, and still does for an older one", async () => {
  const { SettingsPage } = await load('src/desktop/settings.tsx', { react: 'real' })
  const { KumiStore } = await load('src/shared/store.ts', { react: 'real' })
  // An adopted Kumi's part: setup didn't install it, so setup says it's missing.
  const setup = {
    state: 'needs_daw_component',
    next_step: 'install_daw_component',
    message: "Kumi's runtime is ready; next, install Kumi's part in Live.",
    daw: 'ableton',
    daw_component: { name: 'DAW component', state: 'not_installed' }
  }
  const page = async component => {
    const store = new KumiStore(async path => {
      if (path.startsWith('/status')) return { connected: true, daw: 'ableton', revisions: {}, setup: { component } }
      if (path.startsWith('/setup')) return setup
      throw Object.assign(new Error('not here'), { status: 404 })
    })
    await store.refreshStatus()
    await store.refresh('setup')
    const ctx = { storage: { get: (_key, fallback) => fallback, set: () => {} } }
    // The words as a producer reads them: markup escapes the apostrophe in "Kumi's".
    return renderToStaticMarkup(createElement(SettingsPage, { ctx, store, onOpenMap: () => {} })).replaceAll('&#x27;', "'")
  }
  const current = await page({ installed: true, version: '1.0.104', bundled: '1.0.104' })
  assert.ok(current.includes(english('settings.states.ready')) && current.includes(english('settings.partInUse')), current)
  assert.ok(!current.includes(english('settings.states.needs_daw_component')) && !current.includes("install Kumi's part"), current)
  // Right after a bridge bump the part Kumi is connected through is older than the one it ships: installing it is next.
  const older = await page({ installed: true, version: '1.0.99', bundled: '1.0.104' })
  for (const text of [english('settings.states.needs_daw_component'), "install Kumi's part in Live", english('settings.componentStates.not_installed')]) {
    assert.ok(older.includes(text), `${text} in ${older}`)
  }
})

test('what the agent found, or a search, is also listed by name above the map, for the keyboard', async () => {
  const { MapCanvas } = await load('src/shared/MapCanvas.tsx', { react: 'real' })
  const ids = Array.from({ length: 8 }, (_, index) => `s-r7-00${10 + index}`)
  const map = {
    revision: 'r7',
    points: [...ids.map((id, index) => [id, index / 10, index / 10, 0]), ['s-r7-0099', 0.9, 0.9, 1]],
    groups: { 0: { name: 'snare', color: '#f59e0b', count: 8 }, 1: { name: 'kick', color: '#ef4444', count: 1 } },
    sounds: { ...Object.fromEntries(ids.map((id, index) => [id, { name: `Dusty Snare ${index + 1}.wav`, class: 'snare' }])), 's-r7-0099': { name: 'Kick 1.wav', class: 'kick' } }
  }
  const draw = (found, selected = null) =>
    renderToStaticMarkup(createElement(MapCanvas, { map, found, playing: null, selected, onPick: () => {}, onSelect: () => {}, empty: 'none' }))
  // The listed rows: after the list's label, before the canvas (the legend's toggles are pressed buttons too).
  const rows = html => html.slice(html.indexOf('role="group"'), html.indexOf('<canvas'))
  const html = draw({ revision: 'r7', ids: ids.slice(0, 2) }, ids[1])
  assert.ok(html.includes('aria-label="Sounds Kumi found"'), html)
  assert.ok(html.includes('Dusty Snare 1.wav') && html.includes('Dusty Snare 2.wav') && !html.includes('Kick 1.wav'), html)
  assert.equal(rows(html).split('aria-pressed="true"').length - 1, 1, 'the picked sound is the pressed row')
  const many = draw({ revision: 'r7', ids })
  assert.equal(rows(many).split('<button aria-pressed').length - 1, 6)
  assert.ok(many.includes('and 2 more on the map'), many)
  // A find from another map revision names sounds this map may not have: no list.
  assert.ok(!draw({ revision: 'r6', ids }).includes('Sounds Kumi found'))
  assert.ok(!draw(null).includes('Sounds Kumi found'))
})

test("HISTORY shows the runtime's own note: why an unsure row is unsure, and why an undo didn't happen", async () => {
  const { HistoryView } = await load('src/shared/HistoryView.tsx', { react: 'real' })
  // #965's rows: `track` a plain name, `note` the runtime's words, an earlier runtime's rows with no undo.
  const history = {
    items: [
      { id: 'h-g2-u1', label: '', title: 'Raise the Drums fader', family: 'unsure', state: 'unsure', undo_available: false, note: 'Live stopped answering mid-change.', runtime_generation: 'g2' },
      { id: 'h-g2-c3', label: 'c3', title: 'Bass muted', family: 'mixer', state: 'kept', undo_available: false, track: 'Bass', note: 'Bass was changed by hand since, so Kumi kept it.', runtime_generation: 'g2' },
      { id: 'h-g2-c2', label: 'c2', title: 'Script: Name the tracks', family: 'script', state: 'applied', undo_available: false, script: { sha256: 'a'.repeat(64), language: 'python' }, runtime_generation: 'g2' },
      { id: 'h-g1-c9', label: 'c9', title: 'Tempo 120 → 124', family: 'tempo', state: 'applied', undo_available: false, runtime_generation: 'g1' }
    ]
  }
  const html = renderToStaticMarkup(createElement(HistoryView, { history, now: 0, action: () => null }))
  for (const text of ['Live stopped answering mid-change.', english('history.lookFirst'), 'Bass was changed by hand since, so Kumi kept it.', 'Bass · mixer · c3', 'Tempo 120 → 124']) {
    assert.ok(html.includes(text), `${text} in ${html}`)
  }
  // An earlier runtime's applied row, while Kumi answers: it can't be undone because Kumi restarted since, and says
  // where to undo it. A stale snapshot clears every row's undo, so there it says nothing of a restart.
  const restarted = english('history.restartedSince', 'Live').replaceAll("'", '&#x27;')
  const live = renderToStaticMarkup(createElement(HistoryView, { history, now: 0, live: true, daw: 'ableton' }))
  assert.ok(live.includes(restarted) && live.includes(english('history.expired').replaceAll("'", '&#x27;')), live)
  assert.ok(!html.includes(restarted))
  // A script's row has no undo from the start (Kumi never undoes one, CC-19): applied, saying so, never "restarted".
  assert.equal(live.split(restarted).length - 1, 1, live)
  assert.ok(live.includes(english('history.scriptUndo', 'Live').replaceAll("'", '&#x27;')), live)
  assert.ok(live.includes(`aria-label="${english('history.applied')}"`), live)
})

test('a Japanese window reads FOCUS and HISTORY in Japanese; the rows keep their own names', async () => {
  const { FocusView, HistoryView, I18n, translator } = await load('tests/i18n.entry.ts', { react: 'real' })
  const inJapanese = element => renderToStaticMarkup(createElement(I18n.Provider, { value: translator('ja') }, element))
  const history = { items: [{ id: 'h-1', label: 'c1', title: 'Drums volume −6.0 → −4.0 dB', family: 'mixer', state: 'undone', undo_available: false, at: 0, track: 'Drums' }] }
  const rows = inJapanese(createElement(HistoryView, { history, now: 0 }))
  for (const text of ['aria-label="取り消し済み"', 'Drums volume −6.0 → −4.0 dB', 'Drums · mixer · c1 · たった今']) assert.ok(rows.includes(text), `${text} in ${rows}`)
  assert.ok(inJapanese(createElement(HistoryView, { history: { items: [] } })).includes('まだ変更はありません'))
  const focus = inJapanese(createElement(FocusView, { focus: { stale: true, running: false, focus: { track: { name: 'Drums' } }, transport: { playing: false } }, now: 0 }))
  for (const text of ['Drums', '■ 停止中', 'Kumi は停止中']) assert.ok(focus.includes(text), `${text} in ${focus}`)
})

test("a track listen's capture note shows as plain text under the capture line, and nothing shows for an empty one", () => {
  const capture = { method: 'ears', from_beat: 17, beats: 8, seconds: 6.1, cleanup: 'restored' }
  const card = note => renderToStaticMarkup(createElement(ListenCard, {
    media: { file: { kind: 'audio' }, result: { metrics: { file: 'Drums (beats 17–25).wav', loudness: { integratedLufs: -12.1 } }, capture: { ...capture, note } }, fallback: null },
    audition: new Audition(async () => ({ data_url: 'data:audio/wav;base64,' })),
    store: new KumiStore(async () => ({}))
  }))
  const html = card('Captured 6.1 s of the 7.9 s window; <b>Live</b> was put back.')
  assert.ok(html.includes('Captured 6.1 s of the 7.9 s window; &lt;b&gt;Live&lt;/b&gt; was put back.'), 'the note is text, never markup')
  assert.equal(card('  '), card(undefined), 'an empty note adds nothing')
})

test('the generate card shows what was asked, the file, the waveform, the cost and the cap, and Place… for audio only', async () => {
  const { GenerateCard } = await load('src/desktop/generate.tsx', { react: 'real' })
  const fixtures = await import('./generation.fixtures.mjs')
  const media = (result, kind = 'audio') => ({ file: { kind }, result, fallback: createElement('i', null, 'player') })
  const audition = new Audition(async () => ({ data_url: 'data:audio/wav;base64,' }))
  const card = (result, kind) => renderToStaticMarkup(createElement(GenerateCard, { media: media(result, kind), audition, ctx: {}, store: { get: () => ({ status: null }) } }))
  const audio = card(fixtures.DONE)
  for (const text of ['“a short metallic impact”', 'a-primary.wav', 'aria-label="Waveform of a short metallic impact"', '12 credits (est. 15)', '68 of 100 credits left under your cap', 'Place…']) {
    assert.ok(audio.includes(text), `${text} in ${audio}`)
  }
  // Notes from audio: Desktop's own file chip, the note count and Plan notes…; no waveform and no Place step.
  const notes = card(fixtures.MIDI_DONE, 'file')
  for (const text of ['<i>player</i>', 'MIDI file', '128 notes', 'Plan notes…']) assert.ok(notes.includes(text), `${text} in ${notes}`)
  assert.ok(!notes.includes('Place…') && !notes.includes('Waveform'))
  assert.ok(card(fixtures.FROZEN).includes('New paid generations are paused'))
  // A scan's answer, or an upload preview (CC-13 amendment A), keeps Desktop's own player, unchanged.
  assert.equal(card({ job_id: 'j-scan-1', kind: 'scan', state: 'done', result: { sounds: 3 } }), '<i>player</i>')
  assert.equal(card(fixtures.PREVIEW), '<i>player</i>')
  // CC-15: the window, never the billed seconds as the file's length; with no prompt, the kind's own title.
  const extended = card(fixtures.EXTEND.DONE)
  for (const text of ['Extended audio', 'Extended by 3 s (source 12 s)', 'a-extend.wav', 'Place…']) assert.ok(extended.includes(text), `${text} in ${extended}`)
  assert.ok(!extended.includes('>3 s<'), 'no bare seconds chip')
})
