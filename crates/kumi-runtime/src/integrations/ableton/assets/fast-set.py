def why(error):
    if isinstance(error, KeyError): return "Live's references changed since Kumi read them; discover again"
    if isinstance(error, (TypeError, RuntimeError)): return "that device isn't in Live any more; discover it again"
    return type(error).__name__ + ': ' + str(error)[:200]
def find(t):
    try:
        p = bridge.refs.get(t['ref']) if t.get('ref') else list(bridge.refs.get(t['device']).parameters)[t['index']]
        name = str(p.name)
    except IndexError: raise LookupError('the device changed: it has fewer parameters now')
    except Exception as error: raise LookupError(why(error))
    if not t.get('ref') and name != t['name']: raise ValueError('the device changed: its parameter ' + str(t['index']) + ' is now ' + name)
    return p
def fit(p, v):
    lo, hi = float(p.min), float(p.max)
    v = min(hi, max(lo, float(v)))
    return min(hi, lo + round(v - lo)) if getattr(p, 'is_quantized', False) else v
def read(what, otherwise):
    # A read once Live has changed: it may not fail, or the change would read as refused.
    try: return what()
    except Exception: return otherwise
found = []
for t in ARGS:
    p = find(t)
    if not getattr(p, 'is_enabled', True): raise ValueError(str(p.name) + ' is greyed out in Live right now')
    found.append((p, fit(p, t['value'])))
rows, done = [], []
try:
    for p, v in found:
        prior = float(p.value)
        rows.append({'name': str(p.name), 'prior': prior, 'priorDisplay': str(p.str_for_value(prior)), 'min': float(p.min), 'max': float(p.max)})
        p.value = v
        # What this target's own set left: a later target on the same parameter moves it again, and the undo, last
        # target first, checks each against this.
        rows[-1]['applied'] = read(lambda: float(p.value), float(v))
        done.append((p, prior))
except Exception:
    for p, prior in reversed(done):
        try: p.value = prior
        except Exception: pass
    raise
# Live has changed by now: each read falls back to what is known (what each set left; no display, device or track).
for (p, v), row in zip(found, rows):
    row['value'] = read(lambda: float(p.value), row['applied'])
    shown = read(lambda: str(p.str_for_value(row['value'])), None)
    if shown is not None: row['display'] = shown
device = read(lambda: found[0][0].canonical_parent, None)
track = device
while track is not None and type(track).__name__ != 'Track': track = read(lambda: getattr(track, 'canonical_parent', None), None)
result = {'device': read(lambda: str(getattr(device, 'name', '')), ''), 'track': track, 'items': rows}