/**
 * Fast changes: a device's parameters set in one trip into Live, instead of a preview and an apply
 * (a status check, a read, a read again, the write and a read back: five of Live's display ticks,
 * about half a second, for one knob). Kumi's own Python runs on Live's main thread through the
 * bridge's python.run: it sets each parameter within its range (on a step, when it has steps) and
 * reads back what Live shows. Nothing else happens in Live while it runs, so nothing can change
 * between its read and its write. It says where each parameter was, so HISTORY can put it back;
 * putting back leaves alone any the producer has moved since, as an undo of Kumi's should.
 *
 * A parameter named rather than referenced ("Drive"), or a value given as Live shows it ("2 dB"),
 * takes one trip more first, for all of a change's parameters at once.
 */

/** A parameter as the scripts address it: by its reference, or by its place on a device and its name. */
export type FastTarget = { ref: string } | { device: string; index: number; name: string };

/** What the finding trip says of a parameter: where it is, its range, and (asked for) Live's text across it. */
export interface FastFound { index?: number; name: string; min: number; max: number; items?: string[]; grid?: [number, string][] }

/** What the setting trip says of a parameter: where it was and is, with Live's text for both. */
export interface FastSet { name: string; prior: number; value: number; priorDisplay: string; display: string; min: number; max: number }

/** What putting back needs: the parameter, the value it had, and the value Kumi left it at. */
export type FastRevert = FastTarget & { prior: number; applied: number };

/** Arguments go in as a JSON string literal: a JSON string is also a Python string literal. */
const withArgs = (marker: string, args: unknown, body: readonly string[]) =>
  [`# kumi:${marker}`, "import json", `ARGS = json.loads(${JSON.stringify(JSON.stringify(args))})`, ...body].join("\n");

/** The Python that finds a target's parameter, and checks a name still matches its place. */
const FIND = [
  "def find(t):",
  "    if t.get('ref'): return bridge.refs.get(t['ref'])",
  "    p = list(bridge.refs.get(t['device']).parameters)[t['index']]",
  "    if str(p.name) != t['name']: raise ValueError('the device changed: its parameter ' + str(t['index']) + ' is now ' + str(p.name))",
  "    return p",
];

/**
 * Finds parameters by name on their devices (exactly, then by how the name starts), and reads the
 * text Live shows across a parameter's range where a value was given as text.
 */
export function findScript(items: readonly ({ device: string; parameter: string; map: boolean } | { ref: string; map: boolean })[]): string {
  return withArgs("fast-find", items, [
    "def grid(p):",
    "    lo, hi = float(p.min), float(p.max)",
    "    return [[lo + (hi - lo) * i / 128, str(p.str_for_value(lo + (hi - lo) * i / 128))] for i in range(129)]",
    "out = []",
    "for t in ARGS:",
    "    try:",
    "        index = None",
    "        if t.get('ref'): p = bridge.refs.get(t['ref'])",
    "        else:",
    "            ps = list(bridge.refs.get(t['device']).parameters)",
    "            wanted = t['parameter'].strip().lower()",
    "            names = [str(q.name).lower() for q in ps]",
    "            index = next((i for i, n in enumerate(names) if n == wanted), None)",
    "            if index is None: index = next((i for i, n in enumerate(names) if n.startswith(wanted)), None)",
    "            if index is None:",
    "                out.append({'missing': [str(q.name) for q in ps][:24]})",
    "                continue",
    "            p = ps[index]",
    "        row = {'name': str(p.name), 'min': float(p.min), 'max': float(p.max)}",
    "        if index is not None: row['index'] = index",
    "        if t.get('map'):",
    "            row['items'] = [str(v) for v in p.value_items] if getattr(p, 'is_quantized', False) else []",
    "            row['grid'] = grid(p)",
    "        out.append(row)",
    "    except Exception as error:",
    "        out.append({'error': type(error).__name__ + ': ' + str(error)[:200]})",
    "result = out",
  ]);
}

/**
 * Sets each parameter, all or none: every one is found and checked before the first is set. Says
 * where each was and is, the device's name and its track.
 */
export function setScript(items: readonly (FastTarget & { value: number })[]): string {
  return withArgs("fast-set", items, [
    ...FIND,
    // Within its range, and on a step (Live's stepped parameters step by 1 from their minimum), as the bridge does.
    "def fit(p, v):",
    "    lo, hi = float(p.min), float(p.max)",
    "    v = min(hi, max(lo, float(v)))",
    "    return min(hi, lo + round(v - lo)) if getattr(p, 'is_quantized', False) else v",
    "found = []",
    "for t in ARGS:",
    "    p = find(t)",
    "    if not getattr(p, 'is_enabled', True): raise ValueError(str(p.name) + ' is greyed out in Live right now')",
    "    found.append((p, fit(p, t['value'])))",
    "rows, done = [], []",
    "try:",
    "    for p, v in found:",
    "        prior = float(p.value)",
    "        rows.append({'name': str(p.name), 'prior': prior, 'priorDisplay': str(p.str_for_value(prior)), 'min': float(p.min), 'max': float(p.max)})",
    "        p.value = v",
    "        done.append((p, prior))",
    // One Live refused: the ones already set go back, so the change is all or nothing.
    "except Exception:",
    "    for p, prior in reversed(done):",
    "        try: p.value = prior",
    "        except Exception: pass",
    "    raise",
    "for (p, v), row in zip(found, rows):",
    "    row['value'] = float(p.value)",
    "    row['display'] = str(p.str_for_value(p.value))",
    "device = found[0][0].canonical_parent",
    "track = device",
    "while track is not None and type(track).__name__ != 'Track': track = getattr(track, 'canonical_parent', None)",
    "result = {'device': str(getattr(device, 'name', '')), 'track': track, 'items': rows}",
  ]);
}

/**
 * Puts parameters back where they were, latest first, each only if it's still where Kumi left it:
 * one the producer (or anything else) has moved since is left as it is, and named.
 */
export function revertScript(items: readonly FastRevert[]): string {
  return withArgs("fast-revert", items, [
    ...FIND,
    "def same(a, b): return abs(a - b) <= 1e-6 * max(1.0, abs(a), abs(b))",
    "back, moved, gone = 0, [], []",
    "for t in reversed(ARGS):",
    "    try: p = find(t)",
    "    except Exception:",
    "        gone.append(t.get('name') or 'a parameter')",
    "        continue",
    "    if not same(float(p.value), float(t['applied'])):",
    "        moved.append(str(p.name))",
    "        continue",
    "    p.value = t['prior']",
    "    back += 1",
    "result = {'back': back, 'moved': moved, 'gone': gone}",
  ]);
}
