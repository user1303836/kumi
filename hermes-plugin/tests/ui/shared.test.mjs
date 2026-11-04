// node --test: what both UI halves share: the requests and words they show, the listen card's numbers, the map's
// geometry and the store that reads Kumi's backend.
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { load } from './load.mjs'

const format = await load('src/shared/format.ts')
const listen = await load('src/shared/listen.ts')
const map = await load('src/shared/mapModel.ts')
const store = await load('src/shared/store.ts')

test('requests name the ids shown, and the words from the backend stay on one line', () => {
  assert.equal(
    format.undoRequest({ id: 'h-p6-g2-c14', title: 'Drums volume "loud"\nnow', label: 'c14' }),
    'Undo Kumi change h-p6-g2-c14 (“Drums volume ”loud” now”)'
  )
  assert.equal(format.undoRequest({ id: 'h-1' }), 'Undo Kumi change h-1')
  assert.equal(format.useSoundRequest('s-r7-0091', 'Dusty Snare 04'), 'Use sound s-r7-0091 (“Dusty Snare 04”) on the selected track')
  assert.equal(format.setupRequest('ableton'), 'Set up Kumi for Live')
  assert.equal(format.connectRequest('renoise'), 'Connect Kumi to Renoise')
  assert.equal(format.clean('x'.repeat(100), 10), `${'x'.repeat(9)}…`)
})

test('numbers carry a true minus and the chip says what is connected, stale or missing', () => {
  assert.equal(format.num(-1140, 0), '−1,140')
  assert.equal(format.signed(1.25, 1), '+1.3')
  assert.equal(format.signed(-0.04, 1), '0')
  assert.equal(format.hz(2380), '2,380 Hz')
  assert.equal(format.hz(12_500), '12.5 kHz')
  const live = { connected: true, daw: 'ableton', project: { name: 'Night Drive' } }
  assert.deepEqual(format.chipWords(live, 124), { tone: 'good', text: 'Live · Night Drive · 124 BPM' })
  assert.deepEqual(format.chipWords({ ...live, connected: false, stale: true, owner: { pid: 1 } }), { tone: 'warn', text: 'Live · Night Drive · not answering' })
  assert.deepEqual(format.chipWords({ connected: false, stale: true, daw: 'reaper' }), { tone: 'muted', text: 'Kumi · REAPER not connected' })
  assert.equal(format.ago(Date.now() - 4 * 60_000), '4 min ago')
})

const analysis = (lufs, peak, centroid, file) => ({
  file,
  seconds: 0.6,
  loudness: { integratedLufs: lufs, rangeLu: null, truePeakDbtp: peak },
  balance: { centroidHz: centroid, bands: [{ name: 'sub', hz: '20–60', db: -30 }, { name: 'bass', hz: '60–120', db: -22 }] },
  dynamics: { crestDb: 14.2 },
  tempo: null,
  key: null
})

const compared = {
  metrics: analysis(-14.2, -0.8, 2380, '/s/Dusty Snare 04.wav'),
  artifact_id: 'a-7f32',
  verdict_text: 'Dusty Snare 04 is darker.',
  media_tag: 'MEDIA:/tmp/a.wav',
  compare: {
    artifact_id: 'a-7f31',
    metrics: analysis(-12.9, -0.3, 3520, '/s/Snare 01.wav'),
    differences: { headlines: ['1.3 LU quieter'], loudness: { differenceLu: -1.3 }, balance: [{ band: 'sub', hz: '20–60', difference: 2 }] }
  }
}

test('the listen card shows measured numbers beside the reference, with differences, and never invents a missing one', () => {
  const view = listen.listenView(listen.parseListen(JSON.stringify(compared)))
  assert.equal(view.title, 'Dusty Snare 04.wav')
  assert.equal(view.referenceTitle, 'Snare 01.wav')
  assert.deepEqual([view.mineId, view.referenceId], ['a-7f32', 'a-7f31'])
  const rows = Object.fromEntries(view.rows.map(row => [row.label, row]))
  assert.deepEqual(rows.Loudness, { label: 'Loudness', mine: '−14.2 LUFS', reference: '−12.9 LUFS', delta: '−1.3 LU', direction: -1 })
  assert.equal(rows['Brightness (centroid)'].delta, '−1,140 Hz')
  // No loudness range was measured: no row, rather than a zero.
  assert.equal(rows['Loudness range'], undefined)
  assert.equal(view.bandsAre, 'difference')
  assert.deepEqual(view.headlines, ['1.3 LU quieter'])
  assert.equal(view.interpretation, undefined)
})

test("a model's reading shows only when given, labelled as an interpretation of what it was shown", () => {
  const withImage = listen.listenView({ ...compared, interpretation: { text: 'Less 2–6 kHz energy.', inputs: ['metrics', 'spectrogram'], model: 'm-1' } })
  assert.equal(withImage.interpretation.label, 'Model interpretation of the measurements and spectrogram image (m-1)')
  const numbersOnly = listen.listenView({ ...compared, interpretation: { text: 'Quieter.', inputs: ['metrics'] } })
  assert.equal(numbersOnly.interpretation.label, 'Model interpretation of the measurements')
  assert.match(JSON.stringify(withImage), /^(?!.*\bheard\b)/s)
  assert.equal(listen.listenView({ ...compared, interpretation_error: 'the model timed out' }).interpretationError, 'the model timed out')
  assert.equal(listen.parseListen('not json'), null)
  assert.equal(listen.parseListen({ error: 'no file' }), null)
})

const sample = {
  revision: 'r7',
  points: [
    ['s-1', 0.1, 0.1, 0],
    ['s-2', 0.12, 0.1, 0],
    ['s-3', 0.9, 0.9, 1],
    ['bad', 'x', 0, 0]
  ],
  groups: { 0: { name: 'snare', color: [245, 166, 35], count: 2 }, 1: { name: 'pad', color: 'not a colour', count: 1 } },
  sounds: { 's-1': { name: 'Dusty Snare 04', class: 'snare', folder: 'Drums/Snares' }, 's-2': { name: 'Tape Snare 01', class: 'snare' }, 's-3': { name: 'Warm Pad', class: 'pad' } }
}

test('the map keeps valid points, colours each group, and picks the nearest point under the pointer', () => {
  const model = map.buildModel(sample)
  assert.equal(model.points.length, 3)
  // A group whose colour isn't one gets the fallback palette's colour for its number.
  assert.deepEqual(model.groups.map(group => [group.name, group.color]), [['snare', 'rgb(245, 166, 35)'], ['pad', '#f5a623']])
  const view = map.fitView(400, 400)
  const size = 400 - 32
  const [x, y] = map.toScreen(view, size, 0.12, 0.1)
  assert.equal(model.points[map.pick(model, view, size, x + 2, y, 8)].id, 's-2')
  assert.equal(map.pick(model, view, size, x + 40, y + 40, 8), -1)
  // A point the search or legend hides can't be picked.
  assert.equal(model.points[map.pick(model, view, size, x, y, 8, index => model.points[index].id !== 's-2')].id, 's-1')
})

test('zoom keeps the point under the pointer still, search needs every word, and framing shows what was found', () => {
  const model = map.buildModel(sample)
  const view = map.fitView(400, 300)
  const size = 300 - 32
  const before = map.toScreen(view, size, 0.5, 0.5)
  const zoomed = map.zoomAt(view, 4, before[0], before[1])
  assert.deepEqual(map.toScreen(zoomed, size, 0.5, 0.5).map(Math.round), before.map(Math.round))
  assert.equal(map.zoomAt(view, 1000, 0, 0).scale, 40)
  assert.deepEqual([...map.searchSounds(model, 'snare drums')], ['s-1'])
  assert.equal(map.searchSounds(model, '   '), null)
  const framed = map.frame(model, new Set(['s-1', 's-2']), 400, 300)
  for (const id of ['s-1', 's-2']) {
    const point = model.points.find(p => p.id === id)
    const [px, py] = map.toScreen(framed, size, point.x, point.y)
    assert.ok(px >= 0 && px <= 400 && py >= 0 && py <= 300, `${id} framed at ${px},${py}`)
  }
  assert.ok(framed.scale > 1)
})

test('the store reads what moved: FOCUS and HISTORY again only when their revision changes', async () => {
  const reads = []
  let revisions = { focus: 1, history: 1, map: 'r1' }
  const kumi = new store.KumiStore(async path => {
    reads.push(path)
    if (path === '/status') return { connected: true, revisions }
    if (path === '/setup') return { state: 'ready' }
    return { items: [] }
  })
  await kumi.refreshStatus()
  assert.deepEqual(reads.sort(), ['/focus', '/history?limit=50', '/setup', '/status'])
  reads.length = 0
  await kumi.refreshStatus()
  assert.deepEqual(reads, ['/status'])
  reads.length = 0
  revisions = { ...revisions, history: 2 }
  await kumi.refreshStatus()
  assert.deepEqual(reads, ['/status', '/history?limit=50'])
  assert.equal(kumi.get().available, true)
})

test('a backend that is not mounted reads as "off", never as an error, and a reset forgets the last profile', async () => {
  const kumi = new store.KumiStore(async () => {
    throw new Error('404: {"detail":"Not Found"}')
  })
  await kumi.refreshStatus()
  assert.deepEqual([kumi.get().available, kumi.get().error], [false, null])
  const flaky = new store.KumiStore(async () => {
    throw new Error('timed out')
  })
  await flaky.refreshStatus()
  assert.deepEqual([flaky.get().available, flaky.get().error], [null, 'timed out'])
  flaky.reset()
  assert.equal(flaky.get().error, null)
  assert.deepEqual(store.foundSounds('{"map_revision":"r7","items":[{"sound_id":"s-1"},{"name":"x"}]}'), { revision: 'r7', ids: ['s-1'] })
  assert.equal(store.foundSounds('nope'), null)
})
