# Scratch copies of a device's track, for a search heard side by side. Actions: "before" (Live's identities for the
# tracks there now, and the copied track's name), "make" (count), "set" (values) and "drop". A copy is the track a
# duplicate adds, found by comparing the tracks before and after (never by position). Only Kumi hears a copy: its fader
# and sends go all the way down (Kumi hears it before them), so the producer hears the Set, not every copy on top of
# it. A make stops itself well inside its time limit, and a make that fails takes back everything it added, a copy not
# yet named among them. A drop removes the tracks that weren't there before ("before") and carry the prefix. A search's
# own drop ("by_name") also removes one with the copied track's name ("source": a copy the bridge's deadline stopped
# before its rename), while Live's identities still hold: only within one run of Live, so only when the copied track
# itself is still found among them. A later sweep goes by the prefix alone and names a same-named newcomer instead: it
# may be the producer's.
args = ARGS
identity = bridge._capture_object_identity
tracks = list(song.tracks)
if args['action'] in ('before', 'make'):
    device = obj
    track = device.canonical_parent
    if track not in tracks:
        raise ValueError('Kumi hears copies of the device\'s own track, so the device has to be on a track itself (not in a rack, a return or Main)')
    if getattr(track, 'is_foldable', False):
        raise ValueError('Kumi copies one track, and this one is a group (its copy would take its tracks along): pick a device on a track inside it')
if args['action'] == 'before':
    result = {'before': [identity(t) for t in tracks], 'source': str(track.name)}
elif args['action'] == 'make':
    import time
    started = time.perf_counter()
    position = list(track.devices).index(device)
    initial = list(song.tracks)
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
            mixer = added[0].mixer_device
            mixer.volume.value = mixer.volume.min
            for send in mixer.sends:
                send.value = send.min
    except Exception:
        now = list(song.tracks)
        for index in reversed(range(len(now))):
            if now[index] not in initial:
                song.delete_track(index)
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
    holds = source is not None and any(identity(t) in before and str(t.name) == source for t in tracks)
    by_name = bool(args.get('by_name')) and holds
    def new(t):
        return not (before and identity(t) in before)
    def ours(t):
        name = str(t.name)
        return new(t) and (name.startswith(args['prefix'] + ' ') or (by_name and name == source))
    gone = 0
    for index in reversed(range(len(tracks))):
        if ours(tracks[index]):
            song.delete_track(index)
            gone += 1
    newcomers = [] if by_name or not holds else [str(t.name) for t in song.tracks if new(t) and str(t.name) == source]
    result = {'gone': gone, 'left': len([t for t in song.tracks if ours(t)]), 'newcomers': newcomers}
