// node --test: the Make panel's requests and results: what a card sends (its own options, in range, and every sound
// picked), a reroll, the takes results group into, the answers it draws, the sources the picks and the palette become,
// Use's request, and the store's design calls.
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

test('a card sends only its own options, each inside the engine’s range, and every sound picked', () => {
  const options = { bpm: 300, bars: 2, depth: 1.5, seconds: 4, stretch: 2, attack_from: 5 }
  assert.deepEqual(design.requestFor(card('loop'), sounds(3), options), { op: 'fuse', mode: 'loop', sources: sounds(3), options: { bpm: 240, bars: 2, loop: { style: 'beat' } } })
  assert.deepEqual(design.requestFor(card('smear'), sounds(3), options).options, { stretch: 4, depth: 1 })
  // attack_from names one of the sounds sent, or it's left to the seed.
  assert.equal(design.requestFor(card('attack'), sounds(3), options).options, undefined)
  assert.deepEqual(design.requestFor(card('attack'), sounds(3), { attack_from: 2 }).options, { attack_from: 2 })
  // Stack takes no mode; no card has an upper limit (the engine reads each sound shorter when there are many).
  const stacked = design.requestFor(card('stack'), sounds(10), options, 7)
  assert.deepEqual([stacked.mode, stacked.sources.length, stacked.seed, stacked.options], [undefined, 10, 7, undefined])
  assert.deepEqual(design.requestFor(card('blend'), sounds(500)).sources, sounds(500))
})

test('fewer than two sounds make nothing; any number from two makes one', () => {
  assert.deepEqual(design.cardNote(1), { key: 'make.needTwo', least: 2 })
  assert.equal(design.cardNote(2), null)
  assert.equal(design.cardNote(5000), null)
})

test('a request carries only the choices set, each one the engine takes; an untouched steer is no steer at all', () => {
  // Resonate and modulate: their own choices only, known values, in range; the kinds in the engine's order.
  const resonate = design.requestFor(card('resonate'), sounds(4), { resonate: { pairing: 'bright-dark', ring: 'loud', decay: 3 }, modulate: { stages: 3 } })
  assert.deepEqual(resonate.options, { resonate: { pairing: 'bright-dark', decay: 1 } })
  const modulate = design.requestFor(card('modulate'), sounds(4), { modulate: { carrier: 'darkest', kinds: ['vocode', 'am', 'nope'], stages: 9.6, retrigger: 0.25 } })
  assert.deepEqual(modulate.options, { modulate: { carrier: 'darkest', kinds: ['am', 'vocode'], stages: 8, retrigger: 0.25 } })
  // A loop names its style; the other style's controls stay out (a beat's voices with a melody, a key with a beat).
  const loop = { style: 'melody', key: 'F#', scale: 'dorian', kick: 0.9, lean: -0.5, swing: 2, meter: [7, 8], grid: 3 }
  assert.deepEqual(design.requestFor(card('loop'), sounds(4), { loop }).options.loop, { style: 'melody', meter: [7, 8], grid: 3, key: 'F#', scale: 'dorian', swing: 0.75 })
  assert.deepEqual(design.requestFor(card('loop'), sounds(4), { loop: { ...loop, style: 'beat', meter: [7, 5] } }).options.loop, { style: 'beat', grid: 3, lean: -0.5, kick: 0.9, swing: 0.75 })
  // Steer: what changes nothing is left out (an empty steer renders the same bytes as none); ranges come low to high.
  assert.equal(design.requestFor(card('blend'), sounds(2), { steer: {} }).options, undefined)
  assert.equal(design.requestFor(card('stack'), sounds(2), { steer: { bright: 0, pitch: [0, 0], reverse: 0 } }).options, undefined)
  assert.deepEqual(design.requestFor(card('stack'), sounds(2), { steer: { bright: -0.4, pitch: [7, -30], delay: [0, 120], level: 0 } }).options, { steer: { bright: -0.4, pitch: [-24, 7], delay: [0, 120] } })
  assert.equal(design.steerCount({ bright: -0.4, pitch: [0, 0], space: 0.3 }), 2)
  // Grow: exactly the two sounds, a point inside 0–1, the path's seed kept for every point along it.
  const [from, to] = sounds(2)
  assert.deepEqual(design.growRequest(from, to, 1.4, 77, { punch: 0.5 }), { op: 'grow', sources: [from, to], seed: 77, options: { at: 1, steer: { punch: 0.5 } } })
})

test('a surprise moves three to five steer controls a good way, inside their ranges; a modulate recipe names its stages', () => {
  let state = 7
  const random = () => ((state = (state * 1103515245 + 12345) % 2147483648) / 2147483648)
  for (let round = 0; round < 50; round++) {
    const rolled = design.rollSteer(random)
    const kept = design.steerOf(rolled) ?? {}
    assert.ok(Object.keys(rolled).length >= 3 && Object.keys(rolled).length <= 5, JSON.stringify(rolled))
    // Everything it moved is something the engine takes as is.
    assert.deepEqual(kept, Object.fromEntries(Object.entries(rolled).filter(([key]) => key in kept)), JSON.stringify(rolled))
  }
  assert.deepEqual(design.stagesOf('Modulate of 3: ring by Kick 02 → low-pass filter follows Snare 4 → FM by Vox; retrigger 0.6'), ['ring by Kick 02', 'low-pass filter follows Snare 4', 'FM by Vox'])
  assert.deepEqual(design.stagesOf('Blend of 2: 55% Kick, 45% Tom'), [])
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

test("Use is the made sound's reference, its name and artifact id, and nothing for an id Kumi never made", () => {
  const made = design.resultOf(answer())
  assert.equal(design.useRequest(made), '“Blend of 2, seed 482113” (artifact a-dAAAAAAAAAAAAAAAAAAAAAA)')
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

test("a sound made before the Make panel's first open keeps the earlier results: the history follows it, each once, read once", async () => {
  let reads = 0
  const kumi = new store.KumiStore(async path => {
    if (path !== '/designs') return answer()
    reads += 1
    // The backend's history already holds the sound this window made.
    return { designs: [answer(), answer({ artifact_id: 'a-dOLDOLDOLDOLDOLDOLDOLDO' })] }
  })
  // A grow's bud renders before Make… was ever opened.
  await kumi.design(design.requestFor(card('blend'), sounds(2)))
  await kumi.readDesignsOnce()
  await kumi.readDesignsOnce()
  assert.deepEqual(
    kumi.get().designs.map(made => made.artifact_id),
    ['a-dAAAAAAAAAAAAAAAAAAAAAA', 'a-dOLDOLDOLDOLDOLDOLDOLDO']
  )
  assert.equal(reads, 1, 'a second open reads nothing more')
})

test("grow's vine runs from one sound to the other, a drag finds its point, and renders go handle first, then buds in order, two at a time", async () => {
  const grow = await load('src/shared/grow.ts')
  const [a, b] = [
    [0.1, 0.5],
    [0.9, 0.5]
  ]
  assert.deepEqual(grow.vinePoint(a, b, 0), a)
  assert.deepEqual(grow.vinePoint(a, b, 1), b)
  // It bows off the straight line between them, and is the same curve on screen as on the map (the view only scales
  // and shifts), so the buds drawn on screen sit where their map places are.
  assert.ok(Math.abs(grow.vinePoint(a, b, 0.5)[1] - 0.5) > 0.05)
  const onScreen = ([x, y]) => [x * 400 + 30, y * 400 + 12]
  for (const t of [0.2, 0.5, 0.8]) {
    const drawn = grow.vinePoint(onScreen(a), onScreen(b), t)
    const placed = onScreen(grow.vinePoint(a, b, t))
    assert.ok(Math.hypot(drawn[0] - placed[0], drawn[1] - placed[1]) < 1e-9, `t ${t}`)
    // A drag at a point of the vine puts the handle there.
    assert.ok(Math.abs(grow.nearestOnVine(a, b, grow.vinePoint(a, b, t)) - t) < 0.01, `t ${t}`)
  }
  // The backend renders two at once (a third is refused): the handle's point goes first, then the buds from the start.
  const buds = grow.BUDS.map(at => ({ at, state: 'waiting' }))
  assert.equal(grow.nextToGrow(buds, { at: 0.4, state: 'waiting' }, 0).at, 0.4)
  assert.equal(grow.nextToGrow(buds, { at: 0.4, state: 'growing' }, 1).at, 0.125)
  assert.equal(grow.nextToGrow([{ at: 0.125, state: 'ready', result: answer() }, ...buds.slice(1)], null, 1).at, 0.25)
  assert.equal(grow.nextToGrow(buds, { at: 0.4, state: 'waiting' }, grow.RENDERS), null)
  assert.equal(grow.nextToGrow(buds.map(bud => ({ ...bud, state: 'failed', error: 'no' })), null, 0), null)
  // A map sound is an end by its id on the map it's from; without a map, there's none.
  assert.deepEqual(grow.soundEnd('s-r7-0009', 'Dusty Snare 09.wav', 'r7')?.source, { sound_id: 's-r7-0009', map_revision: 'r7', name: 'Dusty Snare 09.wav' })
  assert.equal(grow.soundEnd('s-r7-0009', 'Dusty Snare 09.wav', null), null)
})
