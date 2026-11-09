// node --test: the Make panel's requests and results: what a card sends (its own options, in range, at most the sounds
// it takes), a reroll, the takes results group into, the answers it draws, the sources the picks and the palette
// become, Use's request, and the store's design calls.
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { load } from './load.mjs'

const design = await load('src/shared/design.ts')
const store = await load('src/shared/store.ts')

const card = key => design.CARDS.find(entry => entry.key === key)
const sounds = n => Array.from({ length: n }, (_, index) => ({ sound_id: `s-r7-${index}`, map_revision: 'r7' }))
const answer = (extra = {}) => ({
  artifact_id: 'a-dAAAAAAAAAAAAAAAAAAAAAA',
  op: 'cross',
  mode: 'blend',
  seed: 482113,
  sources: sounds(2),
  duration: 2.134,
  sample_rate: 48000,
  channels: 2,
  peaks: Array.from({ length: 256 }, (_, index) => index / 255),
  recipe: 'Blend of 2: 55% Kick 02, 45% Tom 4; aligned on the attack',
  ...extra
})

test('a card sends only its own options, each inside the engine’s range, and at most the sounds it takes', () => {
  const options = { bpm: 300, bars: 2, depth: 1.5, seconds: 4, stretch: 2, attack_from: 5 }
  assert.deepEqual(design.requestFor(card('loop'), sounds(3), options), { op: 'fuse', mode: 'loop', sources: sounds(3), options: { bpm: 240, bars: 2 } })
  assert.deepEqual(design.requestFor(card('smear'), sounds(3), options).options, { stretch: 4, depth: 1 })
  // attack_from names one of the sounds sent, or it's left to the seed.
  assert.equal(design.requestFor(card('attack'), sounds(3), options).options, undefined)
  assert.deepEqual(design.requestFor(card('attack'), sounds(3), { attack_from: 2 }).options, { attack_from: 2 })
  // Stack takes no mode and at most eight sounds; the rest take sixteen: the first ones go.
  const stacked = design.requestFor(card('stack'), sounds(10), options, 7)
  assert.deepEqual([stacked.mode, stacked.sources.length, stacked.seed, stacked.options], [undefined, 8, 7, undefined])
  assert.deepEqual(design.requestFor(card('blend'), sounds(20)).sources, sounds(16))
})

test('too few sounds make nothing; more than a card takes, the panel says only the first ones go in', () => {
  assert.deepEqual(design.cardNote(card('blend'), 1), { key: 'make.needTwo', least: 2 })
  assert.equal(design.cardNote(card('stack'), 8), null)
  assert.deepEqual(design.cardNote(card('stack'), 9), { key: 'make.firstOnly', most: 8, of: 9 })
  assert.equal(design.cardNote(card('texture'), 16), null)
  assert.deepEqual(design.cardNote(card('texture'), 40), { key: 'make.firstOnly', most: 16, of: 40 })
})

test('a reroll is the same take with a new seed: its options stay, and its result joins the take', () => {
  const first = design.requestFor(card('texture'), sounds(3), { seconds: 12 }, 99)
  assert.deepEqual(design.reroll(first), { op: 'fuse', mode: 'texture', sources: sounds(3), options: { seconds: 12 } })
  const made = design.resultOf(answer({ op: 'fuse', mode: 'texture', sources: sounds(3), seed: 99 }))
  const again = design.resultOf(answer({ op: 'fuse', mode: 'texture', sources: sounds(3), seed: 1234, artifact_id: 'a-dBBBBBBBBBBBBBBBBBBBBBB' }))
  assert.equal(design.takeKey(made), design.takeKey(again))
  assert.notEqual(design.takeKey(made), design.takeKey({ ...made, mode: 'smear' }))
  assert.notEqual(design.takeKey(made), design.takeKey({ ...made, sources: sounds(3).reverse() }), 'the order of the sounds is part of the take')
  // An earlier answer (GET /designs) rerolls with its op, mode and sounds; its options weren't kept.
  assert.deepEqual(design.requestOf(made), { op: 'fuse', mode: 'texture', sources: sounds(3) })
})

test('an answer is drawn only when it names a made sound, its seed and its op; peaks stay between 0 and 1', () => {
  const made = design.resultOf(answer({ peaks: [-1, 0.5, 2, Number.NaN, 'loud'], mode: 'warp' }))
  assert.deepEqual(made.peaks, [0, 0.5, 1, 0, 0])
  assert.equal(made.mode, undefined, 'a mode the panel has no card for is dropped')
  assert.equal(design.resultOf(answer({ artifact_id: '../designs/x.wav' })), null)
  assert.equal(design.resultOf(answer({ op: 'melt' })), null)
  assert.equal(design.resultOf(answer({ seed: 1.5 })), null)
  assert.equal(design.resultOf(null), null)
})

test('the picks and the palette become sounds by id, with names safe to quote; a sound off this map stays out', () => {
  const items = [
    { sound_id: 's-r7-1', artifact_id: null, name: 'Kick “02”\u202e' },
    { sound_id: null, artifact_id: 'a-dAAAAAAAAAAAAAAAAAAAAAA', name: 'Blend of 2' },
    { sound_id: null, artifact_id: null, name: 'Moved away' }
  ]
  assert.deepEqual(design.sourcesOf(items, 'r7'), [
    { sound_id: 's-r7-1', map_revision: 'r7', name: 'Kick 02' },
    { artifact_id: 'a-dAAAAAAAAAAAAAAAAAAAAAA', name: 'Blend of 2' }
  ])
  // Without the map's revision a sound id can't be resolved, so only made sounds go.
  assert.deepEqual(design.sourcesOf(items, null), [{ artifact_id: 'a-dAAAAAAAAAAAAAAAAAAAAAA', name: 'Blend of 2' }])
})

test('Use names the made sound by its artifact id for the chat, and nothing for an id Kumi never made', () => {
  const made = design.resultOf(answer())
  assert.equal(design.useRequest(made), 'Put “Blend of 2, seed 482113” (artifact a-dAAAAAAAAAAAAAAAAAAAAAA) on a pad or track in my project')
  // Named as the palette names it once kept: the recipe's lead up to `:` or `;`, and the seed.
  assert.equal(design.resultName({ ...made, recipe: 'Modulate Vox; read by Kick (PM 24 smp)', seed: 7 }), 'Modulate Vox, seed 7')
  assert.equal(design.resultName({ ...made, recipe: '', mode: 'smear', seed: 3 }), 'Smear, seed 3')
  assert.equal(design.useRequest({ ...made, artifact_id: 'a-d x' }), null)
})

test('the store makes a sound: it goes first in the history once; a refusal says why, with its status', async () => {
  const posts = []
  let reply = answer()
  const kumi = new store.KumiStore(async (path, init) => {
    if (path === '/designs') return { designs: [answer({ artifact_id: 'a-dOLDOLDOLDOLDOLDOLDOLDO' }), { artifact_id: '/etc/passwd', op: 'cross', seed: 1 }] }
    posts.push([path, init?.body])
    if (typeof reply === 'function') return reply()
    return reply
  })
  await kumi.readDesigns()
  assert.deepEqual(
    kumi.get().designs.map(made => made.artifact_id),
    ['a-dOLDOLDOLDOLDOLDOLDOLDO'],
    'an entry that names no made sound is left out'
  )
  const request = design.requestFor(card('blend'), sounds(2))
  const made = await kumi.design(request)
  assert.equal(made.result.artifact_id, 'a-dAAAAAAAAAAAAAAAAAAAAAA')
  assert.deepEqual(posts, [['/design', request]])
  await kumi.design(request)
  assert.deepEqual(
    kumi.get().designs.map(result => result.artifact_id),
    ['a-dAAAAAAAAAAAAAAAAAAAAAA', 'a-dOLDOLDOLDOLDOLDOLDOLDO']
  )
  // Desktop's rest rejects with "<status>: <body>"; the dashboard's with an error that carries `status`.
  reply = () => {
    throw new Error('400: {"detail":"Room Tone is silent."}')
  }
  assert.deepEqual(await kumi.design(request), { error: 'Room Tone is silent.', status: 400 })
  reply = () => {
    throw Object.assign(new Error("Kumi's runtime isn't installed in this profile."), { status: 503 })
  }
  assert.deepEqual(await kumi.design(request), { error: "Kumi's runtime isn't installed in this profile.", status: 503 })
  assert.equal(kumi.get().designs.length, 2, 'a refusal adds nothing')
})
