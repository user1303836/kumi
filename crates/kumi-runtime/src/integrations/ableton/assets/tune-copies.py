# Scratch copies of a device's track, for a search heard side by side: action "make" (count), "set" (values), "drop".
args = ARGS
device = obj
track = device.canonical_parent
tracks = list(song.tracks)
if args['action'] == 'make':
    if track not in tracks:
        raise ValueError('search tunes a device on a track itself (not in a rack, a return or Main)')
    index = tracks.index(track)
    position = list(track.devices).index(device)
    names = []
    for k in range(args['count']):
        song.duplicate_track(index)
        copy = list(song.tracks)[index + 1]
        copy.name = '%s %d' % (args['prefix'], args['count'] - k)
    result = {'position': position, 'names': ['%s %d' % (args['prefix'], k + 1) for k in range(args['count'])]}
elif args['action'] == 'set':
    by_name = dict((str(t.name), t) for t in tracks)
    for name, values in args['values'].items():
        copy = by_name.get(name)
        if copy is None:
            continue
        target = list(copy.devices)[args['position']]
        knobs = dict((str(p.name), p) for p in target.parameters)
        for knob, value in values.items():
            p = knobs.get(knob)
            if p is not None:
                p.value = max(float(p.min), min(float(p.max), float(value)))
    result = {'ok': True}
else:
    gone = 0
    for index in reversed(range(len(tracks))):
        if str(tracks[index].name).startswith(args['prefix'] + ' '):
            song.delete_track(index)
            gone += 1
    result = {'gone': gone}
