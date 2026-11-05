import re

obj_mixer = obj.mixer_device
# Labels as Live writes them, and nothing else: a level ("-6.0 dB", "-inf dB"), or a pan ("C", "25L", "50R").
LEVEL = re.compile(r'(-?[0-9]+(\.[0-9]+)?|-inf) ?dB')
PAN = re.compile(r'C|([1-9][0-9]?)([LR])')


def plain(text):
    return str(text).replace(u'\xa0', ' ').strip()


def order(text):
    # Where a label sits in its parameter's own order: a level in dB (-inf lowest), or a pan position (L below C below
    # R). Any other text is no label: refused, never read as the nearest number.
    t = plain(text)
    level = LEVEL.fullmatch(t)
    if level:
        return float('-inf') if level.group(1) == '-inf' else float(level.group(1))
    pan = PAN.fullmatch(t)
    if pan and pan.group(0) == 'C':
        return 0.0
    if pan:
        return -float(pan.group(1)) if pan.group(2) == 'L' else float(pan.group(1))
    raise ValueError('not a label: %r' % t)


def exact(p, text):
    lo, hi = float(p.min), float(p.max)

    def show(v):
        return plain(p.str_for_value(v))

    def at(v):
        # Where Live's own label for v sits: "2 dB" and "2.0 dB" are one level.
        return order(show(v))

    try:
        key = order(plain(text))
    except Exception:
        return {'near': [show(lo), show(hi)]}
    if key < at(lo) or key > at(hi):
        return {'near': [show(lo), show(hi)]}
    if at(lo) == key:
        first = lo
    else:
        # The least value Live shows at or past the wanted level, by bisection on Live's own labels.
        a, b = lo, hi
        for _ in range(64):
            mid = (a + b) / 2.0
            if at(mid) < key:
                a = mid
            else:
                b = mid
        first = b
        if at(first) != key:
            return {'near': [show(a), show(first)]}
    if at(hi) == key:
        top = hi
    else:
        # The greatest value Live still shows at that level: the value set is the middle of that run.
        c, d = first, hi
        for _ in range(64):
            mid = (c + d) / 2.0
            if at(mid) == key:
                c = mid
            else:
                d = mid
        top = c
    value = lo if first == lo and key == float('-inf') else (first + top) / 2.0
    return {'value': value, 'shown': show(value)}


def resolved(p, text):
    if text is None:
        return None
    if p is None:
        return {'missing': True}
    try:
        return exact(p, text)
    except Exception:
        return {'unread': True}


sends = list(getattr(obj_mixer, 'sends', []) or [])
result = {
    'volume': resolved(getattr(obj_mixer, 'volume', None), ARGS.get('volume')),
    'pan': resolved(getattr(obj_mixer, 'panning', None), ARGS.get('pan')),
    'sends': [resolved(sends[i] if i < len(sends) else None, text) for i, text in enumerate(ARGS.get('sends') or [])],
}
