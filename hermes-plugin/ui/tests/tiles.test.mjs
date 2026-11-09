// node --test: the square-tile view's layout: every sound on its own tile, near where the map put it, clusters kept
// where they are, and the tile under the pointer naming its sound.
import assert from 'node:assert/strict'
import { test } from 'node:test'

import { load } from './load.mjs'

const map = await load('src/shared/mapModel.ts')

/** A map of `count` sounds in clusters, from a seeded generator (deterministic). */
function clustered(clusters, perCluster, spread = 0.04, seed = 7) {
  let state = seed
  const random = () => ((state = (state * 1103515245 + 12345) % 2 ** 31) / 2 ** 31)
  const gauss = () => Math.sqrt(-2 * Math.log(random() || 1e-9)) * Math.cos(2 * Math.PI * random())
  const points = []
  clusters.forEach(([cx, cy], group) => {
    for (let n = 0; n < perCluster; n++) points.push([`s-${group}-${n}`, cx + gauss() * spread, cy + gauss() * spread, group])
  })
  return map.buildModel({ revision: 'r1', points, groups: {}, sounds: {} })
}

test('every sound gets a tile of its own, and each tile names the sound on it', () => {
  const model = clustered([[0.3, 0.3], [0.7, 0.4], [0.5, 0.75]], 400)
  const layout = map.tileLayout(model)
  assert.ok(layout.side * layout.side >= model.points.length)
  const taken = new Set()
  model.points.forEach((_, index) => {
    const cell = layout.row[index] * layout.side + layout.col[index]
    assert.ok(layout.col[index] >= 0 && layout.col[index] < layout.side && layout.row[index] >= 0 && layout.row[index] < layout.side)
    assert.ok(!taken.has(cell), `tile ${cell} taken twice`)
    taken.add(cell)
    assert.equal(layout.cells[cell], index)
  })
  assert.equal([...layout.cells].filter(index => index >= 0).length, model.points.length, 'no tile names a sound that is elsewhere')
})

test('tiles keep the map: a sound alone keeps its place, a crowd spills nearby, clusters stay apart and in order', () => {
  const model = clustered([[0.25, 0.5], [0.75, 0.5]], 500, 0.05)
  const lonely = map.buildModel({ revision: 'r1', points: [...model.points.map(point => [point.id, point.x, point.y, point.group]), ['s-alone', 0.5, 0.05, 2]], groups: {}, sounds: {} })
  const layout = map.tileLayout(lonely)
  const tiled = map.tileModel(lonely, layout)
  const alone = lonely.points.length - 1
  assert.ok(Math.abs(tiled.points[alone].x - 0.5) <= 1 / layout.side && Math.abs(tiled.points[alone].y - 0.05) <= 1 / layout.side, 'the lone sound is on the tile at its place')
  const moved = lonely.points.map((point, index) => Math.hypot(tiled.points[index].x - point.x, tiled.points[index].y - point.y))
  const mean = moved.reduce((sum, distance) => sum + distance, 0) / moved.length
  assert.ok(mean < 0.06, `sounds moved ${mean.toFixed(3)} of the map on average`)
  const rightmostLeft = Math.max(...tiled.points.filter(point => point.group === 0).map(point => point.x))
  const leftmostRight = Math.min(...tiled.points.filter(point => point.group === 1).map(point => point.x))
  assert.ok(rightmostLeft < leftmostRight, 'the left cluster stays left of the right one')
})

test('the tile under the pointer is its sound, and a box over tiles picks the sounds on them', () => {
  const model = clustered([[0.5, 0.5]], 300)
  const layout = map.tileLayout(model)
  const tiled = map.tileModel(model, layout)
  const view = { scale: 1, dx: 0, dy: 0 }
  const size = 600
  const index = 123
  const [x, y] = map.toScreen(view, size, tiled.points[index].x, tiled.points[index].y)
  const tile = size / layout.side
  assert.equal(map.pick(tiled, view, size, x + tile * 0.3, y - tile * 0.3, tile * 0.62), index, 'anywhere on the tile')
  const inside = map.pointsInBox(tiled, view, size, [x - tile * 0.4, y - tile * 0.4, x + tile * 0.4, y + tile * 0.4])
  assert.deepEqual(inside, [index])
})

test('a map of one sound, or none, lays out without fault', () => {
  const one = map.buildModel({ revision: 'r1', points: [['s-1', 0.2, 0.8, 0]], groups: {}, sounds: {} })
  const layout = map.tileLayout(one)
  assert.equal(layout.cells.filter(index => index === 0).length, 1)
  const none = map.tileLayout(map.buildModel(null))
  assert.equal(none.col.length, 0)
})
