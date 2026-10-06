
# Kumi's snapshots of clips, read and put back in Live (version 1). ARGS["op"]:
# - "capture": the clips a change will cut or delete, by ref ({"clips": [...]}), or as the main-lane Arrangement clips
#   of a track overlapping a span ({"track": the track's ref or a clip's on it, "from", "to", "except": identities left
#   out}). Each comes with its
#   identity, its track's, its place, what it is (its leaf: settings and every note field, or its file, warping and
#   markers) and a hash of that, which a restore checks before deleting anything.
# - "restore": {"track", "remnants": [{"identity", "hash", "name"}], "leaving": identities, "clips": [{"where", "leaf"}],
#   "check"}: every check first (nothing changes when one fails); then the remnants (the pieces Live left) go, and each
#   clip is made again whole. Leaving: clips the change's own undo takes away first, so not in a clip's way. With
#   check, only the checks. What Live won't set back exactly is named in partial.
# - "notes": {"track", "clip", "notes"}: more of a restored clip's notes (a large clip's take several calls).
import hashlib, os

NOTE = ("pitch", "start_time", "duration", "velocity", "mute", "probability", "velocity_deviation", "release_velocity")
WORDS = {"name": "its name", "color": "its color", "muted": "its mute", "looping": "its looping", "loop": "its loop",
         "markers": "its start and end", "signature": "its time signature", "span": "its length", "length": "its length",
         "launch": "its launch settings", "notes": "its notes", "file": "its file", "warping": "its warping",
         "warpMode": "its warp mode", "gain": "its gain", "pitch": "its pitch", "ram": "its RAM mode",
         "warpMarkers": "its warp markers", "fades": "its fades", "envelopes": "its automation"}

def identity(item):
    return "live:%d" % item._live_ptr

def readable(item, name):
    try:
        return getattr(item, name)
    except Exception:
        return None

def note_row(note):
    """A note as eight numbers, Live's own fields in NOTE's order (the old note calls would lose the last four)."""
    return [int(note.pitch)] + [float(getattr(note, field)) for field in NOTE[1:4]] + [bool(note.mute)] + [float(getattr(note, field)) for field in NOTE[5:]]

def leaf(clip):
    out = {"kind": "audio" if clip.is_audio_clip else "midi", "name": clip.name, "color": int(clip.color),
           "muted": bool(clip.muted), "looping": bool(clip.looping), "loop": [float(clip.loop_start), float(clip.loop_end)],
           "markers": [float(clip.start_marker), float(clip.end_marker)],
           "signature": [int(clip.signature_numerator), int(clip.signature_denominator)]}
    partial = []
    if clip.is_arrangement_clip:
        out["span"] = [float(clip.start_time), float(clip.end_time)]
        envelopes = readable(clip, "automation_envelopes")
        if envelopes is not None and len(list(envelopes)) > 0:
            partial.append("envelopes")
    else:
        out["length"] = float(clip.length)
        out["launch"] = {"mode": int(clip.launch_mode), "quantization": int(clip.launch_quantization),
                         "legato": bool(clip.legato), "velocity": float(clip.velocity_amount)}
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
            return {"slot": index}
    raise ValueError("the clip's slot wasn't found")

def row(clip):
    kept = leaf(clip)
    return {"identity": identity(clip), "track": identity(owner(clip)), "where": place(clip), "leaf": kept, "hash": digest(kept)}

def capture(args):
    if "clips" in args:
        return {"clips": [row(bridge.refs.get(reference)) for reference in args["clips"]]}
    anchor = bridge.refs.get(args["track"])
    track = owner(anchor) if hasattr(anchor, "is_arrangement_clip") else anchor
    low, high, left_out = float(args["from"]), float(args["to"]), set(args.get("except", []))
    return {"track": identity(track), "clips": [row(clip) for clip in track.arrangement_clips
            if identity(clip) not in left_out and clip.start_time < high - 1e-6 and clip.end_time > low + 1e-6]}

def find_track(wanted):
    for track in song.tracks:
        if identity(track) == wanted:
            return track
    raise ValueError("its track isn't in the Set any more")

def quoted(name):
    return "“%s”" % name

def check(track, args):
    """The remnants as the change left them, each clip's place free but for them, its file there, the track of its
    kind. Raises before anything changes."""
    clips = list(track.arrangement_clips)
    by_identity = {identity(clip): clip for clip in clips}
    remnants = []
    for kept in args.get("remnants", []):
        clip = by_identity.get(kept["identity"])
        if clip is None or digest(leaf(clip)) != kept["hash"]:
            raise ValueError("%s changed in Live since" % quoted(kept.get("name", "")))
        remnants.append(clip)
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
            slots = list(track.clip_slots)
            if where["slot"] >= len(slots):
                raise ValueError("%s's scene isn't in the Set any more" % quoted(kept["name"]))
            if slots[where["slot"]].has_clip:
                raise ValueError("another clip is in %s's slot now" % quoted(kept["name"]))
            continue
        for other in clips:
            if identity(other) not in going and other.start_time < where["end"] - 1e-6 and other.end_time > where["start"] + 1e-6:
                raise ValueError("%s is in %s's place now" % (quoted(other.name), quoted(kept["name"])))
    return remnants

def create(track, where, kept):
    if "slot" in where:
        slot = list(track.clip_slots)[where["slot"]]
        if kept["kind"] == "midi":
            slot.create_clip(float(kept["length"]))
        else:
            slot.create_audio_clip(kept["file"])
        return slot.clip
    before = {identity(clip) for clip in track.arrangement_clips}
    if kept["kind"] == "midi":
        track.create_midi_clip(float(where["start"]), float(where["end"] - where["start"]))
    else:
        track.create_audio_clip(kept["file"], float(where["start"]))
    made = [clip for clip in track.arrangement_clips if identity(clip) not in before]
    if len(made) != 1:
        raise ValueError("Live didn't make %s again" % quoted(kept["name"]))
    return made[0]

def attempt(step):
    try:
        step()
    except Exception:
        pass

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
    if audio and kept["warping"]:
        attempt(lambda: setattr(clip, "warping", True))
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

def add_notes(clip, notes):
    if notes:
        clip.add_new_notes(tuple(Live.Clip.MidiNoteSpecification(pitch=note[0], start_time=note[1], duration=note[2], velocity=note[3],
                                                                 mute=note[4], probability=note[5], velocity_deviation=note[6],
                                                                 release_velocity=note[7]) for note in notes))

def missing(clip, kept, notes_to_come):
    """What of a made clip isn't as it was kept, in words (its notes only once all of them are in)."""
    now, out = leaf(clip), list(kept.get("partial", []))
    for key in kept:
        if key in ("partial", "kind") or (key == "notes" and notes_to_come):
            continue
        if now.get(key) != kept[key] and key not in out:
            out.append(key)
    return [WORDS.get(key, key) for key in out]

def restore(args):
    track = find_track(args["track"])
    remnants = check(track, args)
    if args.get("check"):
        return {"checked": True}
    for clip in remnants:
        track.delete_clip(clip)
    made, partial = [], []
    for item in args["clips"]:
        kept = item["leaf"]
        clip = create(track, item["where"], kept)
        settle(clip, kept)
        if kept["kind"] == "midi":
            add_notes(clip, kept["notes"])
        gaps = missing(clip, kept, item.get("notesToCome", False))
        made.append({"identity": identity(clip), "where": place(clip), "name": kept["name"]})
        if gaps:
            partial.append({"name": kept["name"], "missing": gaps})
    return {"made": made, "partial": partial}

def more_notes(args):
    track = find_track(args["track"])
    for clip in list(track.arrangement_clips) + [slot.clip for slot in track.clip_slots if slot.has_clip]:
        if identity(clip) == args["clip"]:
            add_notes(clip, args["notes"])
            return {"added": len(args["notes"])}
    raise ValueError("the clip Kumi made again isn't there any more")

result = {"capture": capture, "restore": restore, "notes": more_notes}[ARGS["op"]](ARGS)
