// node --test: the listen card, rendered with React to static markup: what it says for a comparison, how it labels a
// model's reading, and that anything it can't draw keeps Desktop's own player.
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { load } from './load.mjs'

const { ListenCard } = await load('src/desktop/card.tsx', { react: 'real' })
const { Audition } = await load('src/shared/hooks.ts', { react: 'real' })

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

function render(props) {
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
  return renderToStaticMarkup(createElement(ListenCard, { media, audition }))
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
  for (const text of ['listen_track', 'Live stopped answering during the capture', 'Kumi put back what it changed meanwhile.', 'Ask Kumi to look again']) {
    assert.ok(warned.includes(text), `${text} in the pane`)
  }
  assert.ok(!warned.includes('>Undo<'))
  const cleared = await render({ unsure: null })
  assert.ok(!cleared.includes('Ask Kumi to look again'))
  assert.ok(cleared.includes('>Undo<'))
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
    renderToStaticMarkup(createElement(MapCanvas, { map, found, playing: null, selected, onPick: () => {}, empty: 'none' }))
  const html = draw({ revision: 'r7', ids: ids.slice(0, 2) }, ids[1])
  assert.ok(html.includes('aria-label="Sounds Kumi found"'), html)
  assert.ok(html.includes('Dusty Snare 1.wav') && html.includes('Dusty Snare 2.wav') && !html.includes('Kick 1.wav'), html)
  assert.equal(html.split('aria-pressed="true"').length - 1, 1, 'the picked sound is the pressed row')
  const many = draw({ revision: 'r7', ids })
  assert.equal(many.split('<button aria-pressed').length - 1, 6)
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
      { id: 'h-g1-c9', label: 'c9', title: 'Tempo 120 → 124', family: 'tempo', state: 'applied', undo_available: false, runtime_generation: 'g1' }
    ]
  }
  const html = renderToStaticMarkup(createElement(HistoryView, { history, now: 0, action: () => null }))
  for (const text of ['Live stopped answering mid-change.', 'Ask Kumi to look again before another change.', 'Bass was changed by hand since, so Kumi kept it.', 'Bass · mixer · c3', 'Tempo 120 → 124']) {
    assert.ok(html.includes(text), `${text} in ${html}`)
  }
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
    audition: new Audition(async () => ({ data_url: 'data:audio/wav;base64,' }))
  }))
  const html = card('Captured 6.1 s of the 7.9 s window; <b>Live</b> was put back.')
  assert.ok(html.includes('Captured 6.1 s of the 7.9 s window; &lt;b&gt;Live&lt;/b&gt; was put back.'), 'the note is text, never markup')
  assert.equal(card('  '), card(undefined), 'an empty note adds nothing')
})

test('the generate card shows the file, its words, the waveform, the cost and the cap, and Place… drafts a request', async () => {
  const { GenerateCard } = await load('src/desktop/generate.tsx', { react: 'real' })
  const media = (result, kind = 'audio') => ({ file: { kind, name: 'Riser 01.wav' }, result, fallback: createElement('i', null, 'player') })
  const audition = new Audition(async () => ({ data_url: 'data:audio/wav;base64,' }))
  const store = { get: () => ({ status: null }) }
  const result = { job_id: 'j-gen-1', kind: 'generate', state: 'done', result: { artifact_id: 'a-12', file: 'Riser 01.wav', kind: 'sfx', prompt: 'a rising white-noise sweep', seconds: 4, provider: 'mirelo', credits: { estimate: 15, actual: 12 }, ledger: { cap: 100, left: 48 } } }
  const html = renderToStaticMarkup(createElement(GenerateCard, { media: media(result), audition, ctx: {}, store }))
  for (const text of ['Riser 01.wav', '“a rising white-noise sweep”', 'aria-label="Waveform of Riser 01.wav"', '12 credits (est. 15)', '48 of 100 credits left under your cap', 'Place…', 'generated']) {
    assert.ok(html.includes(text), `${text} in ${html}`)
  }
  // Anything else keeps Desktop's own player.
  assert.equal(renderToStaticMarkup(createElement(GenerateCard, { media: media({ job_id: 'j-scan-1', kind: 'scan', state: 'done', result: { sounds: 3 } }), audition, ctx: {}, store })), '<i>player</i>')
})
