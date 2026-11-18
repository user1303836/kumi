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

test("on a short map what docks at the foot is never covered: the picks' actions keep above the corner, and the map fits above it", () => {
  // A 600 × 300 map whose top column reaches 230 px (a find's list) and whose foot takes 110 px (the corner over the
  // player): no room between them for a 40 px card. It keeps above the foot, over the column's lower edge, never
  // under the corner where it can't be seen or pressed.
  const [card, top, foot] = [[320, 40], 230, 110]
  for (const box of [null, [40, 100, 120, 140], [20, 60, 580, 280]]) {
    const at = map.floatBeside(box, ...card, 600, 300, top, foot)
    assert.ok(at.top + card[1] <= 300 - foot && at.top >= 8, `${JSON.stringify(at)} for ${box}`)
  }
  // Fit puts the whole map between the top and the corner's room at the foot, and a find's frame above all that docks
  // there (the player too), on the same map size.
  const [width, height, bar, corner] = [600, 400, 80, 76]
  const side = map.mapSide(width, height, bar, corner)
  const fitted = map.fitView(width, height, bar, 0, corner)
  assert.ok(map.toScreen(fitted, side, 0, 0)[1] >= bar && map.toScreen(fitted, side, 1, 1)[1] <= height - corner, JSON.stringify(fitted))
  const model = map.buildModel({ revision: 'r1', points: [['a', 0.2, 0.9, 0], ['b', 0.3, 0.95, 0], ['c', 0.5, 0.1, 0]], groups: {}, sounds: {} })
  const framed = map.frame(model, new Set(['a', 'b']), width, height, bar, 0, 24, { bottom: 220 }, corner)
  for (const id of ['a', 'b']) {
    const point = model.points.find(each => each.id === id)
    const [, y] = map.toScreen(framed, side, point.x, point.y)
    assert.ok(y >= bar && y <= height - 220, `${id} framed above the foot at ${y}`)
  }
})

test("a near search's frame keeps every found sound in the room the map shows, at 466 × 750, and keeps them there as the map's top settles", () => {
  // Hermes's default split at 1440 × 900: the map column 466 × 750, its top column (search, tools, a two-row legend) to
  // 195 px, a find's list folded to its head under it (to 300 px), the corner folded at the foot (10 + 40 + 8 px).
  let seed = 7
  const next = () => ((seed = (seed * 16807) % 2147483647) / 2147483647)
  const gauss = () => Math.sqrt(-2 * Math.log(next())) * Math.cos(2 * Math.PI * next())
  const points = [
    ...Array.from({ length: 540 }, (_, n) => [`k-${n}`, 0.18 + gauss() * 0.06, 0.72 + gauss() * 0.06, 0]),
    ...Array.from({ length: 360 }, (_, n) => [`b-${n}`, 0.24 + gauss() * 0.05, 0.4 + gauss() * 0.05, 1])
  ]
  const model = map.buildModel({ revision: 'r1', points, groups: {}, sounds: {} })
  const [width, height, top, corner, listBottom, foot] = [466, 750, 195, 76, 300, 58]
  const inRoom = (view, side, ids) =>
    model.points
      .filter(point => ids.has(point.id))
      .map(point => map.toScreen(view, side, point.x, point.y))
      .every(([x, y]) => x >= 0 && x <= width && y >= listBottom && y <= height - foot)
  // A tight near search (the 15 sounds nearest one kick, closeness 99 to 100) frames zoomed right in; a loose one
  // (kicks all over) near the whole map.
  const seedKick = model.points.find(point => point.id === 'k-0')
  const nearest = [...model.points].sort((a, b) => Math.hypot(a.x - seedKick.x, a.y - seedKick.y) - Math.hypot(b.x - seedKick.x, b.y - seedKick.y)).slice(0, 15)
  for (const ids of [new Set(nearest.map(point => point.id)), new Set(Array.from({ length: 15 }, (_, n) => `k-${n * 7}`))]) {
    const side = map.mapSide(width, height, top, corner)
    const framed = map.frame(model, ids, width, height, top, 0, 48, { top: listBottom, bottom: foot }, corner)
    assert.ok(inRoom(framed, side, ids), JSON.stringify(framed))
    // The legend then wraps to a third row: the map's side shrinks, and the framed sounds stay where they were.
    const settled = map.mapSide(width, height, top + 30, corner)
    assert.ok(inRoom(map.keepPlace(framed, side, settled), settled, ids))
  }
})

test('mid-move, the pointer meets a sound where it is drawn, not where its new layout is taking it', () => {
  const model = map.buildModel({ revision: 'r1', points: [['a', 0.2, 0.2, 0], ['b', 0.8, 0.8, 0]], groups: {}, sounds: {} })
  const view = map.fitView(400, 400)
  const side = map.mapSide(400, 400)
  // Half-way through a move: `a` drawn at the middle on its way from (0.2, 0.2); `b` where it was.
  const drawn = map.placedModel(model, Float32Array.from([0.5, 0.5, 0.8, 0.8]))
  assert.equal(map.pick(drawn, view, side, ...map.toScreen(view, side, 0.5, 0.5)), 0)
  assert.equal(map.pick(drawn, view, side, ...map.toScreen(view, side, 0.2, 0.2)), -1)
  assert.deepEqual(map.pointsInBox(drawn, view, side, [...map.toScreen(view, side, 0.4, 0.4), ...map.toScreen(view, side, 0.6, 0.6)]), [0])
})

test('Fit shows the whole map in the free room: below the top column, above all that docks at the foot as measured, left of a side sheet; and zooms in smoothly from there', () => {
  // 600 × 500, the legend to 80 px and the corner's fixed room (76 px) setting the map's side; a find's card to 230 px,
  // the player, the corner and the picks' actions taking the foot's 180 px, a side sheet over the right 200 px.
  const [width, height, top, corner, sheet, room] = [600, 500, 80, 76, 200, { top: 230, bottom: 180 }]
  const side = map.mapSide(width, height, top, corner)
  const fitted = map.fitView(width, height, top, sheet, corner, room)
  const [[x0, y0], [x1, y1]] = [map.toScreen(fitted, side, 0, 0), map.toScreen(fitted, side, 1, 1)]
  assert.ok(x0 >= 0 && x1 <= width - sheet && y0 >= room.top && y1 <= height - room.bottom, JSON.stringify(fitted))
  assert.ok(fitted.scale < 1)
  // With nothing more than the legend and the corner, the fit is the whole map at its own side, as ever.
  assert.equal(map.fitView(width, height, top, 0, corner).scale, 1)
  // From a fit below 1×, a zoom in goes on from there (no jump to 1×), and a zoom out stays put.
  assert.ok(Math.abs(map.zoomAt(fitted, 1.1, 100, 300).scale - fitted.scale * 1.1) < 1e-9)
  assert.equal(map.zoomAt(fitted, 0.5, 100, 300).scale, fitted.scale)
})
