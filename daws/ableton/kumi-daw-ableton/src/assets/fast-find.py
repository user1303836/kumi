def why(error):
    if isinstance(error, KeyError): return "Live's references changed since Kumi read them; discover again"
    if isinstance(error, (TypeError, RuntimeError)): return "that device isn't in Live any more; discover it again"
    return type(error).__name__ + ': ' + str(error)[:200]
def grid(p):
    lo, hi = float(p.min), float(p.max)
    return [[lo + (hi - lo) * i / 128, str(p.str_for_value(lo + (hi - lo) * i / 128))] for i in range(129)]
out = []
for t in ARGS:
    try:
        index = None
        if t.get('ref'): p = bridge.refs.get(t['ref'])
        else:
            ps = list(bridge.refs.get(t['device']).parameters)
            wanted = t['parameter'].strip().lower()
            names = [str(q.name).lower() for q in ps]
            index = next((i for i, n in enumerate(names) if n == wanted), None)
            if index is None: index = next((i for i, n in enumerate(names) if n.startswith(wanted)), None)
            if index is None:
                out.append({'missing': [str(q.name) for q in ps][:400]})
                continue
            p = ps[index]
        row = {'name': str(p.name), 'min': float(p.min), 'max': float(p.max)}
        if index is not None: row['index'] = index
        if t.get('map'):
            row['items'] = [str(v) for v in p.value_items] if getattr(p, 'is_quantized', False) else []
            row['grid'] = grid(p)
        out.append(row)
    except Exception as error:
        out.append({'error': why(error)})
result = out