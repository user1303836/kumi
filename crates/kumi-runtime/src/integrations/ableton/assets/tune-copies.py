# Scratch copies of a device's track, for a search heard side by side: action "make" (count), "set" (values), "drop".
# A copy is the track a duplicate adds (found by comparing the tracks before and after, never by position); a make cut
# off partway takes back the copies it made.
args = ARGS
device = obj
track = device.canonical_parent
tracks = list(song.tracks)
if args['action'] == 'make':
    if track not in tracks:
        raise ValueError('search tunes a device on a track itself (not in a rack, a return or Main)')
    if getattr(track, 'is_foldable', False):
        raise ValueError('search copies one track, and this one is a group (its copy would take its tracks along): tune a device on a track inside it')
    position = list(track.devices).index(device)
    made = []
    try:
        for k in range(args['count']):
            before = list(song.tracks)
            song.duplicate_track(before.index(track))
            added = [t for t in song.tracks if t not in before]
            if len(added) != 1:
                raise ValueError('a copy of the track came out as %d tracks' % len(added))
            made.append(added[0])
            added[0].name = '%s %d' % (args['prefix'], k + 1)
    except Exception:
        for copy in made:
            now = list(song.tracks)
            if copy in now:
                song.delete_track(now.index(copy))
        raise
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
