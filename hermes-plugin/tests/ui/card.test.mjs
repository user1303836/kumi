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

test("the pane warns while Kumi isn't sure of the project, until a completed observe clears it", async () => {
  const { KumiPane } = await load('src/desktop/pane.tsx', { react: 'real' })
  const { KumiStore } = await load('src/shared/store.ts', { react: 'real' })
  const unsure = { at: Date.now(), op: 'listen_track', reason: 'Live stopped answering during the capture', cleanup: 'restored' }
  const render = async status => {
    const store = new KumiStore(async path => {
      if (path.startsWith('/status')) return { connected: true, daw: 'ableton', owner: { session_id: 's1' }, revisions: {}, ...status }
      throw Object.assign(new Error('not here'), { status: 404 })
    })
    await store.refreshStatus()
    return renderToStaticMarkup(createElement(KumiPane, { ctx: {}, store, onOpenMap() {} }))
  }
  const warned = await render({ unsure })
  for (const text of ['listen_track', 'Live stopped answering during the capture', 'Kumi put back what it changed meanwhile.', 'Ask Kumi to look again']) {
    assert.ok(warned.includes(text), `${text} in the pane`)
  }
  assert.ok(!(await render({ unsure: null })).includes('Ask Kumi to look again'))
})
