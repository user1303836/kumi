p = obj
lo, hi = float(p.min), float(p.max)
items = [str(item) for item in p.value_items] if getattr(p, 'is_quantized', False) else []
count = 129
grid = [[lo + (hi - lo) * i / (count - 1), str(p.str_for_value(lo + (hi - lo) * i / (count - 1)))] for i in range(count)]
result = {'min': lo, 'max': hi, 'items': items, 'grid': grid}