
# Kumi's snapshots of clips, read and put back in Live (version 1). ARGS["op"]:
# - "capture": the clips a change will cut or delete, by ref ({"clips": [...]}), or as the main-lane Arrangement clips
#   of a track overlapping a span ({"track": the track's ref or a clip's on it, "from", "to", "except": identities left
#   out}). Each comes with its identity, its track's (with the track's name and Kumi's id for it), its place (with its
#   scene's identity, for a Session clip), what it is (its leaf: settings and every note field, or its file, warping
#   and markers; its groove; a Session clip's automation and follow actions) and a hash of that, which a restore checks
#   before deleting anything, and of its notes. overFile: a looping audio clip drawn out past its file, which Live can't
#   make again.
# - "restore": {"track", "trackName", "trackId", "remnants": [{"identity", "hash", "name"}], "leaving": identities,
#   "clips": [{"where", "leaf", "notesToCome"}], "check"}: every check first (nothing changes when one fails); then
#   Arrangement audio clips are made aside at their length (one Live can't make that long refuses here); then the
#   remnants (the pieces Live left) go, and each clip is made again whole. Leaving: clips the change's own undo takes
#   away first, so not in a clip's way. With check, only the checks. What Live won't set back exactly is named in
#   partial. A failure once something changed is answered with how far it got ("error", "removed", "made").
# - "notes": {"track", "trackName", "trackId", "clip", "notes", "notesHash"}: more of a restored clip's notes (a big
#   clip's take several calls); with notesHash, the last, which checks the clip holds every note it had.
# - "state": {"track", "trackName", "trackId", "from", "scenes"}: what the track holds from a time on and in some
#   scenes' slots, to tell whether Live's undo put things back as they were.
import hashlib, os

NOTE = ("pitch", "start_time", "duration", "velocity", "mute", "probability", "velocity_deviation", "release_velocity")
FOLLOW = ("enabled", "linked", "a", "b", "chance_a", "chance_b", "loop_count", "time", "jump_a", "jump_b")
WORDS = {"name": "its name", "color": "its color", "muted": "its mute", "looping": "its looping", "loop": "its loop",
         "markers": "its start and end", "signature": "its time signature", "span": "its length", "length": "its length",
         "launch": "its launch settings", "notes": "its notes", "file": "its file", "warping": "its warping",
         "warpMode": "its warp mode", "gain": "its gain", "pitch": "its pitch", "ram": "its RAM mode",
         "warpMarkers": "its warp markers", "fades": "its fades", "envelopes": "its automation", "groove": "its groove",
         "follow": "its follow actions"}

def identity(item):
    return "live:%d" % item._live_ptr

def readable(item, name):
    try:
        return getattr(item, name)
    except Exception:
        return None

def offers(item, name):
    """Whether this Live has the property at all: a Python attribute set on one object isn't Live's."""
    return hasattr(type(item), name)

def kumi_id(track):
    try:
        value = track.get_data("kumi.track", None)
    except Exception:
        return None
    return value if isinstance(value, str) and value else None

def quoted(name):
    return "“%s”" % name

def note_row(note):
    """A note as eight numbers, Live's own fields in NOTE's order (the old note calls would lose the last four)."""
    return [int(note.pitch)] + [float(getattr(note, field)) for field in NOTE[1:4]] + [bool(note.mute)] + [float(getattr(note, field)) for field in NOTE[5:]]

def envelopes(clip):
    """A Session clip's automation, each envelope with its parameter and events: time, Live's own value, the value in
    the parameter's terms (what a new event takes; a step's two are read either side of it) and the curve. Whole when
    that's all of it: an envelope Live lists with no parameter (a MIDI controller's), or a Live that doesn't list
    them while the clip has some, isn't."""
    has = readable(clip, "has_envelopes")
    if has is False:
        return [], True
    listed = readable(clip, "automation_envelopes")
    if listed is None:
        return [], False
    rows, whole = [], True
    for envelope in listed:
        parameter = readable(envelope, "parameter")
        if parameter is None:
            whole = False
            continue
        events = list(envelope.events_in_range(0.0, float(clip.length) + 4.0))
        times = [float(event.time) for event in events]
        kept = []
        for index, event in enumerate(events):
            time = float(event.time)
            at = time if times.count(time) == 1 else (time - 1e-6 if index == times.index(time) else time + 1e-6)
            curve = readable(event, "control_coefficients")
            kept.append([time, float(event.value), float(envelope.value_at_time(at)),
                         [float(getattr(curve, name)) for name in ("x1", "y1", "x2", "y2")] if curve is not None else None])
        rows.append({"parameter": identity(parameter), "name": str(readable(parameter, "name") or ""), "events": kept})
    return rows, whole and (has is not True or len(rows) > 0)

def leaf(clip):
    out = {"kind": "audio" if clip.is_audio_clip else "midi", "name": clip.name, "color": int(clip.color),
           "muted": bool(clip.muted), "looping": bool(clip.looping), "loop": [float(clip.loop_start), float(clip.loop_end)],
           "markers": [float(clip.start_marker), float(clip.end_marker)],
           "signature": [int(clip.signature_numerator), int(clip.signature_denominator)]}
    partial = []
    if clip.is_arrangement_clip:
        out["span"] = [float(clip.start_time), float(clip.end_time)]
        # Live doesn't let Kumi write an Arrangement clip's own automation. A Live that can't say counts as some.
        has, listed = readable(clip, "has_envelopes"), readable(clip, "automation_envelopes")
        if has is True or (listed is not None and len(list(listed)) > 0) or (has is None and listed is None):
            partial.append("envelopes")
    else:
        out["length"] = float(clip.length)
        out["launch"] = {"mode": int(clip.launch_mode), "quantization": int(clip.launch_quantization),
                         "legato": bool(clip.legato), "velocity": float(clip.velocity_amount)}
        out["envelopes"], whole = envelopes(clip)
        if not whole:
            partial.append("envelopes")
        if offers(clip, "follow_action_enabled"):
            out["follow"] = [readable(clip, "follow_action_" + name) for name in FOLLOW]
    if offers(clip, "groove"):
        groove = readable(clip, "groove")
        out["groove"] = None if groove is None else {"name": str(readable(groove, "name") or ""), "identity": identity(groove)}
    if clip.is_midi_clip:
        out["notes"] = sorted(note_row(note) for note in clip.get_all_notes_extended())
    else:
        out["file"] = clip.file_path
        out["warping"] = bool(clip.warping)
        out["warpMode"] = int(clip.warp_mode)
        out["gain"] = float(clip.gain)
        out["pitch"] = [int(clip.pitch_coarse), float(clip.pitch_fine)]
        out["ram"] = bool(clip.ram_mode)
        out["warpMarkers"] = [[float(marker.beat_time), float(marker.sample_time)] for marker in clip.warp_markers]
        fades = [readable(clip, "fade_in_length"), readable(clip, "fade_out_length")]
        if all(isinstance(value, (int, float)) and not isinstance(value, bool) for value in fades):
            out["fades"] = [float(value) for value in fades]
        elif clip.is_arrangement_clip:
            partial.append("fades")
    if partial:
        out["partial"] = partial
    return out

def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")).hexdigest()

def owner(clip):
    parent = clip.canonical_parent
    return parent if clip.is_arrangement_clip else parent.canonical_parent

def place(clip):
    if clip.is_arrangement_clip:
        return {"start": float(clip.start_time), "end": float(clip.end_time)}
    slot = clip.canonical_parent
    for index, candidate in enumerate(owner(clip).clip_slots):
        if candidate._live_ptr == slot._live_ptr:
            scene = list(song.scenes)[index]
            return {"slot": index, "scene": identity(scene), "sceneName": str(scene.name)}
    raise ValueError("the clip's slot wasn't found")

def track_of(track):
    out = {"track": identity(track), "trackName": track.name}
    if kumi_id(track):
        out["trackId"] = kumi_id(track)
    return out

def over_file(clip):
    """Whether a looping, warped Arrangement audio clip is drawn out past its file's length in beats (its first warp
    marker to its last): Live can't make one that long again."""
    if not (clip.is_arrangement_clip and clip.is_audio_clip and clip.looping and clip.warping):
        return False
    beats = [float(marker.beat_time) for marker in clip.warp_markers]
    return len(beats) > 1 and clip.end_time - clip.start_time > max(beats) - min(beats) + 1e-6

def row(clip):
    kept = leaf(clip)
    out = dict(track_of(owner(clip)), identity=identity(clip), where=place(clip), leaf=kept, hash=digest(kept))
    if "notes" in kept:
        out["notesHash"] = digest(kept["notes"])
    if over_file(clip):
        out["overFile"] = True
    return out

def capture(args):
    if "clips" in args:
        return {"clips": [row(bridge.refs.get(reference)) for reference in args["clips"]]}
    anchor = bridge.refs.get(args["track"])
    track = owner(anchor) if hasattr(anchor, "is_arrangement_clip") else anchor
    low, high, left_out = float(args["from"]), float(args["to"]), set(args.get("except", []))
    return dict(track_of(track), clips=[row(clip) for clip in track.arrangement_clips
                if identity(clip) not in left_out and clip.start_time < high - 1e-6 and clip.end_time > low + 1e-6])

def find_track(args):
    """The track by its identity, still the track it was (Live can give a new track a deleted one's address): Kumi's
    id for it says so, or, when it had none, its name."""
    for track in song.tracks:
        if identity(track) == args["track"]:
            if args.get("trackId") is not None and kumi_id(track) != args["trackId"]:
                break
            if args.get("trackId") is None and args.get("trackName") not in (None, track.name):
                raise ValueError("%s isn't the track it was (renamed, or another in its place)" % quoted(track.name))
            return track
    raise ValueError("its track isn't in the Set any more")

def slot_of(track, where, name):
    """A Session clip's slot, found by its scene (a scene added above moves it down)."""
    for index, scene in enumerate(song.scenes):
        if identity(scene) == where.get("scene"):
            return list(track.clip_slots)[index]
    raise ValueError("%s's scene isn't in the Set any more" % quoted(name))

def check(track, args):
    """The remnants as the change left them, each clip's place free but for them, its file there, the track of its
    kind and not frozen. Raises before anything changes."""
    clips = list(track.arrangement_clips)
    by_identity = {identity(clip): clip for clip in clips}
    remnants = []
    for kept in args.get("remnants", []):
        clip = by_identity.get(kept["identity"])
        if clip is None or digest(leaf(clip)) != kept["hash"]:
            raise ValueError("%s changed in Live since" % quoted(kept.get("name", "")))
        remnants.append(clip)
    if args["clips"] and readable(track, "is_frozen") is True:
        raise ValueError("%s is frozen" % quoted(track.name))
    going = {identity(clip) for clip in remnants} | set(args.get("leaving", []))
    for item in args["clips"]:
        where, kept = item["where"], item["leaf"]
        if kept["kind"] == "midi" and not track.has_midi_input:
            raise ValueError("%s is a MIDI clip, and its track takes audio now" % quoted(kept["name"]))
        if kept["kind"] == "audio":
            if track.has_midi_input:
                raise ValueError("%s is an audio clip, and its track takes MIDI now" % quoted(kept["name"]))
            if not os.path.exists(kept["file"]):
                raise ValueError("%s's audio file isn't there any more (%s)" % (quoted(kept["name"]), kept["file"]))
        if "slot" in where:
            if slot_of(track, where, kept["name"]).has_clip:
                raise ValueError("another clip is in %s's slot now" % quoted(kept["name"]))
            continue
        for other in clips:
            if identity(other) not in going and other.start_time < where["end"] - 1e-6 and other.end_time > where["start"] + 1e-6:
                raise ValueError("%s is in %s's place now" % (quoted(other.name), quoted(kept["name"])))
    return remnants

def made_by(track, step, name):
    """The one clip `step` adds to the track's Arrangement."""
    before = {identity(clip) for clip in track.arrangement_clips}
    step()
    made = [clip for clip in track.arrangement_clips if identity(clip) not in before]
    if len(made) != 1:
        raise ValueError("Live didn't make %s again" % quoted(name))
    return made[0]

def make_aside(track, where, kept, aside):
    """An Arrangement audio clip, made past the track's last clip and cut to its length, to be put in its place later.
    Live makes one at its file's length, laid over whatever is there, and setting its markers doesn't shorten it: so a
    throwaway clip laid over its tail cuts it, and is deleted. One Live can't make that long (a loop drawn out past its
    file, a warp stretched past Live's own), or a stand-in the throwaway splits rather than cuts (one that grew when it
    settled, unwarped say), raises with everything it made deleted. Each one made is added to `aside`."""
    length = float(where["end"]) - float(where["start"])
    far = max([float(clip.end_time) for clip in track.arrangement_clips] + [float(where["end"])]) + 4.0
    before = {identity(clip) for clip in track.arrangement_clips}
    def made_since():
        return [clip for clip in track.arrangement_clips if identity(clip) not in before]
    try:
        stand = made_by(track, lambda: track.create_audio_clip(kept["file"], far), kept["name"])
        settle(stand, kept)
        if stand.end_time - stand.start_time > length + 1e-6:
            track.create_audio_clip(kept["file"], far + length)
            extra = [clip for clip in made_since() if identity(clip) != identity(stand)]
            for clip in extra:
                track.delete_clip(clip)
            if len(extra) != 1:
                raise ValueError("Live can't make %s again at its length" % quoted(kept["name"]))
            settle(stand, kept)
        if abs(stand.end_time - stand.start_time - length) > 1e-6:
            raise ValueError("Live can't make %s again at its length: the clip is longer than its file" % quoted(kept["name"]))
    except Exception:
        for clip in made_since():
            attempt(lambda clip=clip: track.delete_clip(clip))
        raise
    aside.append(stand)
    return stand

def put_in_place(track, stand, where, kept):
    """A clip made aside, copied into its place (which the check found free), and the stand-in deleted."""
    try:
        return made_by(track, lambda: track.duplicate_clip_to_arrangement(stand, float(where["start"])), kept["name"])
    finally:
        track.delete_clip(stand)

def create(track, where, kept):
    if "slot" in where:
        slot = slot_of(track, where, kept["name"])
        if kept["kind"] == "midi":
            slot.create_clip(float(kept["length"]))
        else:
            slot.create_audio_clip(kept["file"])
        return slot.clip
    return made_by(track, lambda: track.create_midi_clip(float(where["start"]), float(where["end"] - where["start"])), kept["name"])

def attempt(step):
    try:
        step()
        return True
    except Exception:
        return False

def pair(clip, low_name, high_name, low, high):
    """Set a pair (a loop, or the markers) in an order Live takes: the far end first when moving past it."""
    if low >= getattr(clip, high_name):
        setattr(clip, high_name, high)
        setattr(clip, low_name, low)
    else:
        setattr(clip, low_name, low)
        setattr(clip, high_name, high)

def settle(clip, kept):
    audio = kept["kind"] == "audio"
    for name, key in (("name", "name"), ("color", "color"), ("muted", "muted")):
        attempt(lambda: setattr(clip, name, kept[key]))
    if audio:
        if kept["warping"]:
            attempt(lambda: setattr(clip, "warping", True))
        # Live takes a warp mode while the clip is unwarped too.
        attempt(lambda: setattr(clip, "warp_mode", kept["warpMode"]))
    attempt(lambda: setattr(clip, "looping", kept["looping"]))
    if kept["looping"]:
        attempt(lambda: pair(clip, "loop_start", "loop_end", *kept["loop"]))
    attempt(lambda: pair(clip, "start_marker", "end_marker", *kept["markers"]))
    if audio and not kept["warping"]:
        # After the markers: an unwarped clip's would be held to its file's seconds.
        attempt(lambda: setattr(clip, "warping", False))
        attempt(lambda: pair(clip, "start_marker", "end_marker", *kept["markers"]))
    attempt(lambda: setattr(clip, "signature_numerator", kept["signature"][0]))
    attempt(lambda: setattr(clip, "signature_denominator", kept["signature"][1]))
    if audio:
        attempt(lambda: setattr(clip, "gain", kept["gain"]))
        attempt(lambda: setattr(clip, "pitch_coarse", kept["pitch"][0]))
        attempt(lambda: setattr(clip, "pitch_fine", kept["pitch"][1]))
        attempt(lambda: setattr(clip, "ram_mode", kept["ram"]))
        if "fades" in kept:
            attempt(lambda: setattr(clip, "fade_in_length", kept["fades"][0]))
            attempt(lambda: setattr(clip, "fade_out_length", kept["fades"][1]))
        wanted = [list(marker) for marker in kept["warpMarkers"]]
        for beat, sample in [[float(marker.beat_time), float(marker.sample_time)] for marker in clip.warp_markers]:
            if [beat, sample] not in wanted:
                attempt(lambda: clip.remove_warp_marker(beat))
        have = [[float(marker.beat_time), float(marker.sample_time)] for marker in clip.warp_markers]
        for beat, sample in wanted:
            if [beat, sample] not in have:
                attempt(lambda: clip.add_warp_marker(Live.Clip.WarpMarker(beat_time=beat, sample_time=sample)))
    if "launch" in kept:
        launch = kept["launch"]
        attempt(lambda: setattr(clip, "launch_mode", launch["mode"]))
        attempt(lambda: setattr(clip, "launch_quantization", launch["quantization"]))
        attempt(lambda: setattr(clip, "legato", launch["legato"]))
        attempt(lambda: setattr(clip, "velocity_amount", launch["velocity"]))

def parameters(track):
    """Every parameter a Session clip's envelope can be on: the track's mixer's and its devices', in racks too."""
    found, mixer = [], readable(track, "mixer_device")
    if mixer is not None:
        found += [parameter for parameter in (readable(mixer, "volume"), readable(mixer, "panning"), readable(mixer, "track_activator")) if parameter is not None]
        found += list(readable(mixer, "sends") or [])
    def walk(devices):
        for device in devices or []:
            found.extend(readable(device, "parameters") or [])
            for chain in readable(device, "chains") or []:
                walk(readable(chain, "devices"))
    walk(readable(track, "devices"))
    return found

def put_back(clip, track, kept):
    """What a made clip holds besides its settings: its notes, groove, automation and follow actions. What couldn't be
    put back, by key."""
    failed = []
    if kept["kind"] == "midi":
        add_notes(clip, kept["notes"])
    groove = kept.get("groove")
    if groove is not None:
        pool = list(readable(readable(song, "groove_pool"), "grooves") or [])
        found = [candidate for candidate in pool if identity(candidate) == groove["identity"]] or \
                [candidate for candidate in pool if str(readable(candidate, "name") or "") == groove["name"]]
        if not found or not attempt(lambda: setattr(clip, "groove", found[0])):
            failed.append("groove")
    if kept.get("envelopes"):
        by_identity = {identity(parameter): parameter for parameter in parameters(track)}
        event = Live.Envelope.EnvelopeEvent
        curve_of = Live.Envelope.EnvelopeEventControlCoefficients
        for envelope in kept["envelopes"]:
            parameter = by_identity.get(envelope["parameter"])
            def write():
                made = clip.create_automation_envelope(parameter)
                made = made if made is not None else clip.automation_envelope(parameter)
                for time, _, value, curve in envelope["events"]:
                    made.create_event(event(time, value, curve_of(*curve)) if curve is not None else event(time, value))
            if parameter is None or not attempt(write):
                failed.append("envelopes")
    if "follow" in kept:
        def follow():
            clip.follow_action_enabled = False
            for name, value in zip(FOLLOW, kept["follow"]):
                if name not in ("enabled", "chance_b"):
                    setattr(clip, "follow_action_" + name, value)
            clip.follow_action_enabled = kept["follow"][0]
        if not attempt(follow):
            failed.append("follow")
    return failed

def add_notes(clip, notes):
    if notes:
        clip.add_new_notes(tuple(Live.Clip.MidiNoteSpecification(pitch=note[0], start_time=note[1], duration=note[2], velocity=note[3],
                                                                 mute=note[4], probability=note[5], velocity_deviation=note[6],
                                                                 release_velocity=note[7]) for note in notes))

def close(a, b):
    return abs(a - b) <= 1e-6 * max(1.0, abs(a), abs(b))

def same(key, now, kept):
    """A made clip's value as it was kept. Automation by its events' times, Live's own values and curves (the values
    an event was written with are read back as Live's own)."""
    if key != "envelopes":
        return now == kept
    def events(rows):
        return sorted((row["parameter"], [(event[0], event[1], event[3]) for event in row["events"]]) for row in rows or [])
    a, b = events(now), events(kept)
    return len(a) == len(b) and all(x[0] == y[0] and len(x[1]) == len(y[1]) and all(
        close(p[0], q[0]) and close(p[1], q[1]) and (p[2] == q[2] or (p[2] is not None and q[2] is not None and all(close(m, n) for m, n in zip(p[2], q[2]))))
        for p, q in zip(x[1], y[1])) for x, y in zip(a, b))

def missing(clip, kept, notes_to_come, failed):
    """What of a made clip isn't as it was kept, in words (its notes only once all of them are in)."""
    now, out = leaf(clip), list(kept.get("partial", []))
    out += [key for key in failed if key not in out]
    for key in kept:
        if key in ("partial", "kind") or key in out or (key == "notes" and notes_to_come):
            continue
        if not same(key, now.get(key), kept[key]):
            out.append(key)
    return [WORDS.get(key, key) for key in out]

def restore(args):
    track = find_track(args)
    remnants = check(track, args)
    if args.get("check"):
        return {"checked": True}
    # Arrangement audio clips are made aside first, so one Live can't make at its length refuses before anything in the
    # Set changes (Live keeps a step of Kumi's that changed nothing).
    aside, stands = [], {}
    try:
        for index, item in enumerate(args["clips"]):
            if item["leaf"]["kind"] == "audio" and "slot" not in item["where"]:
                stands[index] = make_aside(track, item["where"], item["leaf"], aside)
    except Exception:
        for stand in aside:
            attempt(lambda stand=stand: track.delete_clip(stand))
        raise
    done = {"removed": [], "made": [], "partial": []}
    try:
        for clip in remnants:
            name = clip.name
            track.delete_clip(clip)
            done["removed"].append(name)
        for index, item in enumerate(args["clips"]):
            kept = item["leaf"]
            clip = put_in_place(track, stands.pop(index), item["where"], kept) if index in stands else create(track, item["where"], kept)
            done["made"].append({"identity": identity(clip), "where": place(clip), "name": kept["name"]})
            settle(clip, kept)
            gaps = missing(clip, kept, item.get("notesToCome", False), put_back(clip, track, kept))
            if gaps:
                done["partial"].append({"name": kept["name"], "missing": gaps})
    except Exception as error:
        for stand in stands.values():
            attempt(lambda stand=stand: track.delete_clip(stand))
        done["error"] = "%s: %s" % (type(error).__name__, error)
    return done

def more_notes(args):
    track = find_track(args)
    for clip in list(track.arrangement_clips) + [slot.clip for slot in track.clip_slots if slot.has_clip]:
        if identity(clip) == args["clip"]:
            add_notes(clip, args["notes"])
            out = {"added": len(args["notes"])}
            if args.get("notesHash"):
                out["exact"] = digest(sorted(note_row(note) for note in clip.get_all_notes_extended())) == args["notesHash"]
            return out
    raise ValueError("the clip Kumi made again isn't there any more")

def state(args):
    track = find_track(args)
    low = float(args["from"])
    clips = sorted([clip.name, round(float(clip.start_time), 6), round(float(clip.end_time), 6), digest(leaf(clip))]
                   for clip in track.arrangement_clips if clip.end_time > low + 1e-6)
    slots = []
    for wanted in args.get("scenes", []):
        index = [identity(scene) for scene in song.scenes].index(wanted) if wanted in [identity(scene) for scene in song.scenes] else None
        slot = list(track.clip_slots)[index] if index is not None else None
        slots.append(digest(leaf(slot.clip)) if slot is not None and slot.has_clip else None)
    return {"clips": clips, "slots": slots}

result = {"capture": capture, "restore": restore, "notes": more_notes, "state": state}[ARGS["op"]](ARGS)
