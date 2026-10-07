d = obj
out = []
for p in list(d.parameters):
    lo, hi = float(p.min), float(p.max)
    quantized = bool(getattr(p, 'is_quantized', False))
    items = [str(item) for item in p.value_items] if quantized else []
    grid = []
    if not quantized:
        count = 49
        for i in range(count):
            v = lo + (hi - lo) * i / (count - 1)
            try:
                grid.append([v, str(p.str_for_value(v))])
            except Exception:
                pass
    out.append({'name': str(p.name), 'value': float(p.value), 'min': lo, 'max': hi, 'items': items, 'grid': grid})
result = out
