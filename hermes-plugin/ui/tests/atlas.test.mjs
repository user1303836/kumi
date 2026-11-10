// node --test: the atlas look's own logic (the glow under the sound map, the names on it), as numbers: no canvas here.
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { load } from './load.mjs'

const nebula = await load('src/shared/atlas/nebula.ts')
const names = await load('src/shared/atlas/names.ts')
const pulse = await load('src/shared/atlas/pulse.ts')
const trail = await load('src/shared/atlas/trail.ts')

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
  assert.ok(core[3] > 180 && core[3] < 225, `the core's glow ${core[3]}`)
  const few = at(all, 0.75, 0.5)
  assert.deepEqual(few.slice(0, 3), blue)
  assert.ok(few[3] > 8 && few[3] < core[3] / 2, `sixty sounds glow less than two thousand: ${few[3]}`)
  assert.equal(at(all, 0.5, 0.05)[3], 0, 'no sounds, no glow')
  assert.ok(Math.max(...all.filter((_, index) => index % 4 === 3)) <= 217, 'nowhere glows past the cap')

  const hidden = nebula.nebulaField(points, index => points[index].group === 0, group => (group === 0 ? red : blue))
  assert.equal(at(hidden, 0.75, 0.5)[3], 0, 'the hidden family leaves no glow')
  assert.deepEqual(at(hidden, 0.25, 0.5), core)

  // A library of three sounds: a faint light under each, not the glow a crowd gets.
  const lone = nebula.nebulaField([{ x: 0.5, y: 0.5, group: 0 }, { x: 0.2, y: 0.2, group: 0 }, { x: 0.8, y: 0.8, group: 0 }], () => true, () => red)
  assert.ok(at(lone, 0.5, 0.5)[3] > 0 && at(lone, 0.5, 0.5)[3] < 70, `one sound's glow ${at(lone, 0.5, 0.5)[3]}`)
})

test('halos weaken as the map zooms in and on a crowded map, so a core adds up to its colour and never to white', () => {
  const sparse = nebula.haloOf(2.2, 1, 2, 833)
  const crowded = nebula.haloOf(2.2, 1, 22, 833)
  assert.ok(sparse.alpha <= 0.34 && crowded.alpha < sparse.alpha && crowded.alpha >= 0.16)
  assert.ok(nebula.haloOf(3.5, 4, 2, 833 * 4).alpha < sparse.alpha)
  assert.ok(nebula.haloOf(3.5, 4, 2, 833 * 4).radius > sparse.radius)
  assert.equal(nebula.nebulaAlpha(1), 1)
  assert.ok(nebula.nebulaAlpha(2) < 0.5)
  assert.equal(nebula.nebulaAlpha(20), 0, 'far in, the nebula is not drawn at all')
})

test("a family's name sits over the middle of its sounds at their upper edge, fades out by twice the whole map, and a stray sound doesn't move it", () => {
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
  assert.ok(names.regionAlpha(1.9) > 0 && names.regionAlpha(1.9) < 1)
  assert.equal(names.regionAlpha(2.4), 0)
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
  // The level shown jumps up to a hit and falls back over about a fifth of a second, never below the sound's own.
  assert.equal(pulse.follow(0.2, 0.9, 16), 0.9)
  assert.ok(pulse.follow(0.9, 0.1, 16) > 0.8)
  assert.equal(pulse.follow(0.9, 0.1, 400), 0.1)
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
