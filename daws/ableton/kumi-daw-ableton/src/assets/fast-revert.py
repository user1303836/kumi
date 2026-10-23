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
def same(a, b): return abs(a - b) <= 1e-6 * max(1.0, abs(a), abs(b))
back, moved, gone = 0, [], []
for t in reversed(ARGS):
    # A modulator's slot Kumi mapped (it was empty before): emptied again while it still holds what Kumi mapped there.
    if t.get('kind') == 'modulation':
        label = t.get('name') or 'a modulator'
        try:
            m = bridge.refs.get(t['device'])
            current = m.get_modulation_target(t['slot'])
        except Exception:
            gone.append(label)
            continue
        same_target = current is not None and (getattr(current, '_live_ptr', None) == t['applied'] if t.get('applied') is not None else str(current.name) == t.get('appliedName'))
        if not same_target:
            moved.append(label)
            continue
        m.map_modulation(t['slot'], None)
        back += 1
        continue
    try: p = find(t)
    except Exception:
        gone.append(t.get('name') or 'a parameter')
        continue
    if not same(float(p.value), float(t['applied'])):
        moved.append(str(p.name))
        continue
    p.value = t['prior']
    back += 1
result = {'back': back, 'moved': moved, 'gone': gone}