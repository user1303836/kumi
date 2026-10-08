# Scratch copies of a device's track, for a search heard side by side. Actions: "before" (Live's identities for the
# tracks there now, and the copied track's name), "make" (count), "set" (values) and "drop". A copy is the track a
# duplicate adds, found by comparing the tracks before and after (never by position). A make stops itself well inside
# its time limit, so a make cut off partway still takes back the copies it made. A drop removes the tracks that
# weren't there before ("before") and carry the prefix or the copied track's name ("source": a copy never renamed).
args = ARGS
identity = bridge._capture_object_identity
tracks = list(song.tracks)
if args['action'] in ('before', 'make'):
    device = obj
    track = device.canonical_parent
    if track not in tracks:
        raise ValueError('search tunes a device on a track itself (not in a rack, a return or Main)')
    if getattr(track, 'is_foldable', False):
        raise ValueError('search copies one track, and this one is a group (its copy would take its tracks along): tune a device on a track inside it')
if args['action'] == 'before':
    result = {'before': [identity(t) for t in tracks], 'source': str(track.name)}
elif args['action'] == 'make':
    import time
    started = time.perf_counter()
    position = list(track.devices).index(device)
    made = []
    try:
        for k in range(args['count']):
            if time.perf_counter() - started > args['budget']:
                raise ValueError('Live took too long making the copies (%d of %d made)' % (len(made), args['count']))
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
    before = set(args.get('before') or [])
    source = args.get('source')
    def ours(t):
        name = str(t.name)
        if before and identity(t) in before:
            return False
        return name.startswith(args['prefix'] + ' ') or (bool(before) and source is not None and name == source)
    gone = 0
    for index in reversed(range(len(tracks))):
        if ours(tracks[index]):
            song.delete_track(index)
            gone += 1
    result = {'gone': gone, 'left': len([t for t in song.tracks if ours(t)])}
