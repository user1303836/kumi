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
# Each parameter as Live has it now, found as fast-revert finds it, and what Live shows for it: None for one that's gone.
# Reads only: nothing is set.
values = []
for t in ARGS:
    try:
        p = find(t)
        value = float(p.value)
        values.append({'value': value, 'display': str(p.str_for_value(value))})
    except Exception:
        values.append(None)
result = values
