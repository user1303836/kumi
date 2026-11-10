// node --test: the atlas look's own logic (the glow under the sound map, the names on it, what moves over it), as
// numbers: the one canvas here is a stand-in that takes any call.
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { load } from './load.mjs'

const nebula = await load('src/shared/atlas/nebula.ts')
const names = await load('src/shared/atlas/names.ts')
const pulse = await load('src/shared/atlas/pulse.ts')
const trail = await load('src/shared/atlas/trail.ts')
const lineage = await load('src/shared/atlas/lineage.ts')
const gestures = await load('src/shared/atlas/gestures.ts')
const constellation = await load('src/shared/atlas/constellation.ts')
const first = await load('src/shared/atlas/first.ts')

// A seeded cluster of `count` sounds of `group` around (`cx`, `cy`).
function cluster(count, group, cx, cy, spread = 0.04) {
  let seed = (group + 1) * 7919
  const next = () => ((seed = (seed * 16807) % 2147483647) / 2147483647)
  return Array.from({ length: count }, () => {
    const [radius, turn] = [Math.sqrt(-2 * Math.log(1 - next())) * spread, 2 * Math.PI * next()]
    return { x: cx + radius * Math.cos(turn), y: cy + radius * Math.sin(turn), group }
  })
}

test('the nebula glows in each family colour where its sounds are, a crowded core keeps its colour, and a family the legend hides leaves none', () => {
  const [red, blue] = [
    [232, 93, 74],
    [74, 144, 226]
  ]
  const points = [...cluster(2000, 0, 0.25, 0.5), ...cluster(60, 1, 0.75, 0.5)]
  const side = nebula.NEBULA_SIDE
  const at = (pixels, x, y) => {
    const cell = Math.floor(((y + nebula.NEBULA_PAD) / (1 + 2 * nebula.NEBULA_PAD)) * side) * side + Math.floor(((x + nebula.NEBULA_PAD) / (1 + 2 * nebula.NEBULA_PAD)) * side)
    return [...pixels.slice(4 * cell, 4 * cell + 4)]
  }
  const all = nebula.nebulaField(points, () => true, group => (group === 0 ? red : blue))
  const core = at(all, 0.25, 0.5)
  // Two thousand sounds on one spot: still the family's colour, and short of opaque, so the sounds read over it.
  assert.deepEqual(core.slice(0, 3), red)
  assert.ok(core[3] > 0 && core[3] < 255, `the core's glow ${core[3]}`)
  const few = at(all, 0.75, 0.5)
  assert.deepEqual(few.slice(0, 3), blue)
  assert.ok(few[3] > 0 && few[3] < core[3], `sixty sounds glow less than two thousand: ${few[3]}`)
  assert.equal(at(all, 0.5, 0.05)[3], 0, 'no sounds, no glow')
  assert.ok(Math.max(...all.filter((_, index) => index % 4 === 3)) < 255, 'nowhere is the glow opaque')

  const hidden = nebula.nebulaField(points, index => points[index].group === 0, group => (group === 0 ? red : blue))
  assert.equal(at(hidden, 0.75, 0.5)[3], 0, 'the hidden family leaves no glow')
  assert.deepEqual(at(hidden, 0.25, 0.5), core)

  // A library of three sounds: a light under each, fainter than the glow a crowd gets.
  const lone = nebula.nebulaField([{ x: 0.5, y: 0.5, group: 0 }, { x: 0.2, y: 0.2, group: 0 }, { x: 0.8, y: 0.8, group: 0 }], () => true, () => red)
  assert.ok(at(lone, 0.5, 0.5)[3] > 0 && at(lone, 0.5, 0.5)[3] < core[3], `one sound's glow ${at(lone, 0.5, 0.5)[3]}`)
})

test('halos weaken on a crowded map and as the map zooms in, a sound far in still has one, and the nebula is gone by then', () => {
  const sparse = nebula.haloOf(2.2, 1, 2, 833)
  const crowded = nebula.haloOf(2.2, 1, 22, 833)
  // A crowded core adds up to its colour, not to white: each halo there is weaker.
  assert.ok(crowded.alpha > 0 && crowded.alpha < sparse.alpha)
  assert.ok(nebula.haloOf(3.5, 4, 2, 833 * 4).alpha < sparse.alpha)
  // Far in, a sound alone on dark map still glows, and its halo reaches further past its shape.
  const far = nebula.haloOf(5.2, 30, 2, 833 * 30)
  assert.ok(far.alpha > 0 && far.radius / 5.2 > sparse.radius / 2.2)
  assert.equal(nebula.nebulaAlpha(1), 1)
  assert.equal(nebula.nebulaAlpha(20), 0, 'far in, the nebula is not drawn at all')
})

test("a family's name sits over the middle of its sounds at their upper edge, is gone once the map is zoomed in, and a stray sound doesn't move it", () => {
  const kicks = cluster(600, 0, 0.3, 0.6)
  const hats = cluster(400, 1, 0.7, 0.3)
  const [kick, hat] = names.regionsOf([...kicks, ...hats])
  assert.deepEqual([kick.group, hat.group], [0, 1])
  assert.ok(Math.abs(kick.x - 0.3) < 0.01 && Math.abs(hat.x - 0.7) < 0.01, 'over the middle of each cluster')
  // The cluster's spread is 0.04: its upper edge is two to three of those above its middle.
  assert.ok(kick.y < 0.6 - 0.07 && kick.y > 0.6 - 0.13, `the kicks' upper edge ${kick.y}`)
  const strayed = names.regionsOf([...kicks, { x: 0.3, y: 0.02, group: 0 }, { x: 0.95, y: 0.95, group: 0 }])[0]
  assert.ok(Math.abs(strayed.y - kick.y) < 0.01 && Math.abs(strayed.x - kick.x) < 0.01)
  assert.equal(names.regionAlpha(1), 1)
  assert.equal(names.regionAlpha(8), 0)
})

test('sounds carry their names only where there is room: clear of each other, never across another sound, and not one family first', () => {
  // Three labels in a row, the second touching the first: it is left out, and the third still fits.
  const boxes = [
    [10, 10, 60, 14],
    [50, 12, 60, 14],
    [140, 10, 60, 14]
  ]
  assert.deepEqual(names.fitting(boxes, 50), [0, 2])
  assert.deepEqual(names.fitting(boxes, 50, 80), [0], 'kept this far apart, only the first has room')
  assert.deepEqual(names.fitting(boxes, 1), [0], 'no more than asked for')
  assert.deepEqual(names.fitting(boxes, 50, 0, 0, [[0, 0, 30, 30]]), [1, 2], "clear of what's already written")
  // A label lying across another sound's place is not clear; one over empty map is.
  assert.deepEqual(names.clearOf([[20, 3, 60, 14], [20, 43, 60, 14]], [[10, 10], [50, 12], [10, 50]]), [false, true])
  // The map lists its sounds a family at a time: the first fifty offered a name come from all along that order.
  const offered = Array.from({ length: 600 }, (_, index) => index).sort((a, b) => names.nameRank(a) - names.nameRank(b)).slice(0, 50)
  for (const third of [0, 1, 2]) assert.ok(offered.filter(index => Math.floor(index / 200) === third).length >= 8, `third ${third}`)
})

test("the pulse follows the playing sound's level: up at once with a hit, settling after it, and plain before the waveform arrives", () => {
  // A hit, then a tail: loud at the start, quiet by the middle, silent at the end.
  const levels = [1, 0.6, 0.3, 0.1, 0.05, 0, 0, 0]
  assert.equal(pulse.levelAt(levels, 0), 1)
  assert.ok(Math.abs(pulse.levelAt(levels, 1 / 16) - 0.8) < 1e-9, 'between two bars, between their levels')
  assert.equal(pulse.levelAt(levels, 1), 0)
  assert.equal(pulse.levelAt(null, 0.5), 0, 'no waveform yet: no level (the plain pulse)')
  assert.equal(pulse.levelAt(levels, 1.2), 0, 'past the end')
  // The level shown jumps up to a hit at once, and falls back after it rather than dropping, never below the sound's own.
  assert.equal(pulse.follow(0.2, 0.9, 16), 0.9)
  assert.ok(pulse.follow(0.9, 0.1, 1) > 0.1)
  assert.equal(pulse.follow(0.9, 0.1, 60_000), 0.1)
  // How far through the sound a play is: null once it's over, or for a sound of no known length.
  assert.equal(pulse.progressOf(250, 0.5), 0.5)
  assert.equal(pulse.progressOf(600, 0.5), null)
  assert.equal(pulse.progressOf(100, 0), null)
})

test("the pointer's trail runs out when the pointer rests, and a new line never joins the old one", () => {
  let path = []
  for (let step = 0; step < 10; step++) path = trail.stepTo(path, 100 + step * 10, 200, 1000 + step * 16, 'rgb(80, 200, 180)', step === 0)
  assert.equal(path.length, 10)
  assert.deepEqual(path.map(step => step.joined), [false, ...Array(9).fill(true)], 'the first step starts the line')
  // Resting: a tremble of a pixel adds nothing, and the steps fade to nothing in a little over half a second.
  const resting = trail.stepTo(path, 191, 200, 1200, 'rgb(80, 200, 180)', false)
  assert.equal(resting.length, 10)
  assert.ok(trail.strengthOf(path[9], 1144) === 1 && trail.strengthOf(path[9], 1444) > 0.4)
  assert.equal(trail.strengthOf(path[9], 1144 + trail.TRAIL_MS), 0)
  assert.equal(trail.stepTo(path, 191, 200, 3000, 'rgb(80, 200, 180)', false).length, 0, 'long after, nothing is left')
  // The pointer left and came back elsewhere, onto another family: a new line, in that sound's colour.
  const back = trail.stepTo(path, 600, 500, 1300, 'rgb(232, 93, 74)', true)
  assert.deepEqual([back.at(-1).joined, back.at(-1).color], [false, 'rgb(232, 93, 74)'])
})

test("a made sound's lineage names the families it came from, in the sources' order, each with its share", () => {
  const colors = { 's-1': 'red', 's-2': 'red', 's-3': 'teal', 's-4': 'green' }
  const made = { 'a-blend': { sources: [{ sound_id: 's-1' }, { sound_id: 's-3' }] }, 'a-grown': { sources: [{ sound_id: 's-1' }, { sound_id: 's-4' }], at: 0.25 } }
  const parentage = { sound: id => colors[id] ?? null, made: id => made[id] ?? null }
  // Four sounds of three families: the first family named has half.
  assert.deepEqual(lineage.lineageOf([{ sound_id: 's-1' }, { sound_id: 's-3' }, { sound_id: 's-2' }, { sound_id: 's-4' }], parentage), [
    { color: 'red', share: 0.5 },
    { color: 'teal', share: 0.25 },
    { color: 'green', share: 0.25 }
  ])
  // A sound grown a quarter of the way from a kick toward a perc is three quarters kick.
  assert.deepEqual(lineage.lineageOf(made['a-grown'].sources, parentage, 0.25), [
    { color: 'red', share: 0.75 },
    { color: 'green', share: 0.25 }
  ])
  // A source Kumi made stands for its own families when the window knows them; one it doesn't know, or a sound off
  // this map, for the accent (no colour).
  assert.deepEqual(lineage.lineageOf([{ artifact_id: 'a-blend' }, { sound_id: 's-4' }], parentage), [
    { color: 'red', share: 0.25 },
    { color: 'teal', share: 0.25 },
    { color: 'green', share: 0.5 }
  ])
  assert.deepEqual(lineage.lineageOf([{ artifact_id: 'a-unknown' }, { sound_id: 's-gone' }], parentage), [{ color: null, share: 1 }])
  assert.deepEqual(lineage.lineageOf([], parentage), [])
  // The sounds to ring on the map for it: its parents on the map, and a made parent's own, each once.
  assert.deepEqual(lineage.parentsOf([{ artifact_id: 'a-blend' }, { sound_id: 's-4' }, { sound_id: 's-1' }, { artifact_id: 'a-unknown' }], parentage.made), ['s-1', 's-3', 's-4'])
})

test('the hint line says what each gesture does in the state the map is in, and nothing the map does not do', () => {
  const idle = { picks: true, hand: false, naming: false, picked: 0, hover: true, scrollPans: null }
  const line = state => gestures.hintOf(state).map(([gesture, deed]) => `${gesture} ${deed}`)
  assert.deepEqual(line(idle), ['hover hear', 'drag pick', 'shiftDrag add', 'scroll zoom', 'middleDrag pan'])
  // Hover-to-play off: a click is what plays. A trackpad's scroll was seen moving the map: it says so, and a pinch zooms.
  assert.deepEqual(line({ ...idle, hover: false, scrollPans: true }), ['click hear', 'drag pick', 'shiftDrag add', 'scroll pan', 'pinch zoom'])
  // With sounds picked, what the picks take.
  assert.deepEqual(line({ ...idle, picked: 60 }), ['shiftDrag addMore', 'escape clear'])
  // The hand tool has its own line: a drag moves the map and picks nothing, so Shift has nothing to add to; picks made
  // before it can still be let go.
  assert.deepEqual(line({ ...idle, hand: true }), ['hover hear', 'drag pan', 'scroll zoom'])
  assert.deepEqual(line({ ...idle, hand: true, picked: 3 }), ['hover hear', 'drag pan', 'scroll zoom', 'escape clear'])
  // A map that picks nothing (no palette to pick for): a drag moves it.
  assert.deepEqual(line({ ...idle, picks: false }), ['hover hear', 'drag pan', 'scroll zoom', 'middleDrag pan'])
  // While Grow names its sounds, its own prompt stands on the line; a click names a sound and Escape cancels.
  const naming = { ...idle, naming: true, picked: 4 }
  assert.deepEqual(line(naming), [])
  assert.deepEqual([gestures.gesturesOf(naming).click, gestures.gesturesOf(naming).drag, gestures.gesturesOf(naming).escape], ['name', 'pan', 'cancel'])
  // What the handlers ask: a drag boxes only on a map that picks, with the pick tool, outside Grow's naming; Escape
  // has something to clear only with picks.
  assert.equal(gestures.gesturesOf(idle).drag, 'pick')
  assert.equal(gestures.gesturesOf(idle).escape, undefined)
  assert.equal(gestures.gesturesOf({ ...idle, picked: 2 }).escape, 'clear')
  assert.equal(gestures.gesturesOf({ ...idle, picks: false, picked: 2 }).escape, undefined)
})

test("a sound's constellation is the sounds nearest it, nearest first, without the ones the search hides", () => {
  // Sound 0 at the origin; the others at growing distances, out of order.
  const points = [
    { x: 0, y: 0 },
    { x: 0.5, y: 0 },
    { x: 0, y: 0.1 },
    { x: 0.3, y: 0 },
    { x: 0, y: -0.2 },
    { x: 0.9, y: 0.9 },
    { x: -0.4, y: 0 }
  ]
  assert.deepEqual(constellation.nearestTo(points, 0, 5), [2, 4, 3, 6, 1])
  assert.deepEqual(constellation.nearestTo(points, 0, 3), [2, 4, 3])
  // The search hides 2 and 3: the next nearest take their places.
  assert.deepEqual(
    constellation.nearestTo(points, 0, 3, index => index !== 2 && index !== 3),
    [4, 6, 1]
  )
  // A map of three sounds joins the two others; a sound the map doesn't have, none.
  assert.deepEqual(constellation.nearestTo(points.slice(0, 3), 1, 5), [0, 2])
  assert.deepEqual(constellation.nearestTo(points, 99, 5), [])
})

test('a first-open step shows until it has been done once, on a map that offers it', () => {
  const all = { hover: true, picks: true }
  assert.deepEqual(first.firstLeft('', all), ['hover', 'box', 'make'])
  assert.deepEqual(first.firstLeft(null, all), ['hover', 'box', 'make'])
  // Done in any order, each stays done; what the host keeps reads back the same.
  const kept = first.firstKept(first.firstKept('', ['make']), ['hover'])
  assert.equal(kept, 'hover,make')
  assert.deepEqual(first.firstLeft(kept, all), ['box'])
  assert.equal(first.firstKept(kept, ['hover']), kept)
  // Skip does them all.
  assert.deepEqual(first.firstLeft(first.firstKept('', first.FIRST_STEPS), all), [])
  // With hover-to-play off there is nothing to hear by moving; a map that picks nothing has no box and no Make….
  assert.deepEqual(first.firstLeft('', { hover: false, picks: true }), ['box', 'make'])
  assert.deepEqual(first.firstLeft('', { hover: true, picks: false }), ['hover'])
})

test('the overlay asks for a frame only while something on it moves, and never for a map with no size', async () => {
  const { Overlay } = await load('src/shared/atlas/overlay.ts')
  // A canvas that takes any drawing call, frames that run when the test lets time pass, and a clock the test sets.
  const anything = new Proxy(function () {}, { get: () => anything, set: () => true, apply: () => anything, construct: () => anything })
  const asked = new Map()
  let [ids, now] = [0, 1000]
  const kept = { clock: performance.now, frame: globalThis.requestAnimationFrame, cancel: globalThis.cancelAnimationFrame, style: globalThis.getComputedStyle, path: globalThis.Path2D }
  performance.now = () => now
  globalThis.requestAnimationFrame = run => (asked.set(++ids, run), ids)
  globalThis.cancelAnimationFrame = id => asked.delete(id)
  globalThis.getComputedStyle = () => ({ color: 'rgb(255, 255, 255)' })
  globalThis.Path2D = anything
  // Lets `ms` pass and runs the frames asked for; answers how many are asked for after them.
  const pass = ms => {
    now += ms
    const due = [...asked.values()]
    asked.clear()
    for (const run of due) run(now)
    return asked.size
  }
  const sound = { place: () => [40, 40], color: '#e85d4a', glyph: 0, duration: 2, levels: null }
  try {
    const overlay = new Overlay({ width: 0, height: 0, getContext: () => anything })
    overlay.resize(400, 300, 1)
    assert.equal(asked.size, 0, 'a still map asks for nothing')
    // A sound plays: a frame after each. It stops: its pulse fades, then nothing more is asked for.
    overlay.play(sound)
    assert.equal(pass(16), 1)
    overlay.play(null)
    assert.equal(pass(16), 1)
    assert.equal(pass(1000), 0)
    // The pointer's trail: frames while it shows, none once it has run out.
    overlay.trail(10, 10)
    overlay.trail(60, 60)
    assert.equal(pass(16), 1)
    assert.equal(pass(5000), 0)
    // The map loses its size (its tab is hidden) with a sound playing and a trail showing: the frames stop, and
    // the sound stopping there starts none.
    overlay.play(sound)
    overlay.trail(80, 80)
    overlay.trail(120, 120)
    assert.equal(pass(16), 1)
    overlay.resize(0, 0, 1)
    assert.equal(pass(16), 0)
    overlay.play(null)
    assert.equal(asked.size, 0)
    // Back with a size, long after: the fade and the trail are over, so still nothing.
    now += 5000
    overlay.resize(400, 300, 1)
    assert.equal(asked.size, 0)
    // A sound that began while the map had no size pulses once it has one; and nothing outlives the overlay.
    overlay.resize(0, 0, 1)
    overlay.play(sound)
    assert.equal(asked.size, 0)
    overlay.resize(400, 300, 1)
    assert.equal(asked.size, 1)
    overlay.dispose()
    assert.equal(asked.size, 0)
  } finally {
    performance.now = kept.clock
    Object.assign(globalThis, { requestAnimationFrame: kept.frame, cancelAnimationFrame: kept.cancel, getComputedStyle: kept.style, Path2D: kept.path })
  }
})
