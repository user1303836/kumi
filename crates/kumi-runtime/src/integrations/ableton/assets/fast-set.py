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
        rows[-1]['applied'] = float(p.value)
        done.append((p, prior))
except Exception:
    for p, prior in reversed(done):
        try: p.value = prior
        except Exception: pass
    raise
for (p, v), row in zip(found, rows):
    row['value'] = float(p.value)
    row['display'] = str(p.str_for_value(p.value))
device = found[0][0].canonical_parent
track = device
while track is not None and type(track).__name__ != 'Track': track = getattr(track, 'canonical_parent', None)
result = {'device': str(getattr(device, 'name', '')), 'track': track, 'items': rows}