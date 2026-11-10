// node --test: the atlas look's own logic (the glow under the sound map, the names on it), as numbers: no canvas here.
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { load } from './load.mjs'

const nebula = await load('src/shared/atlas/nebula.ts')
const names = await load('src/shared/atlas/names.ts')

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
