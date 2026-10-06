"""A Live for snapshots.py (BODY, given before this file): Sets of tracks with Arrangement clips and Session slots,
MIDI notes with every field, audio clips with warp markers. Each scenario runs the script as Live's Python would (ARGS
in front, song, bridge and Live in its namespace, its result read back) and says what went wrong, if anything; the
printed list is what the test checks."""
import json, os, tempfile, types

POINTERS = [1000]

def pointer():
    POINTERS[0] += 1
    return POINTERS[0]

class MidiNoteSpecification:
    def __init__(self, pitch, start_time, duration, velocity, mute=False, probability=1.0, velocity_deviation=0.0, release_velocity=64.0):
        self.pitch, self.start_time, self.duration, self.velocity = pitch, start_time, duration, velocity
        self.mute, self.probability, self.velocity_deviation, self.release_velocity = mute, probability, velocity_deviation, release_velocity

class WarpMarker:
    # Live's order: sample time first.
    def __init__(self, sample_time, beat_time):
        self.sample_time, self.beat_time = sample_time, beat_time

Live = types.SimpleNamespace(Clip=types.SimpleNamespace(MidiNoteSpecification=MidiNoteSpecification, WarpMarker=WarpMarker))

class Clip:
    def __init__(self, parent, arrangement, audio, length, start=0.0, file_path=None):
        self._live_ptr = pointer(); self.canonical_parent = parent
        self.is_arrangement_clip, self.is_audio_clip, self.is_midi_clip = arrangement, audio, not audio
        self.name, self.color, self.muted, self.looping = "", 0x3BA8FF, False, not audio
        self.loop_start, self.loop_end, self.start_marker, self.end_marker = 0.0, length, 0.0, length
        self.signature_numerator, self.signature_denominator = 4, 4
        self.start_time, self.end_time = start, start + length
        self.launch_mode, self.launch_quantization, self.legato, self.velocity_amount = 0, 14, False, 0.0
        self.notes = []
        if audio:
            self.file_path, self.warping, self.warp_mode, self.gain = file_path, True, 0, 0.4
            self.pitch_coarse, self.pitch_fine, self.ram_mode = 0, 0.0, False
            self.warp_markers = [WarpMarker(0.0, 0.0), WarpMarker(16.0, 32.0)]
            self.fade_in_length, self.fade_out_length = 0.0, 0.0
    @property
    def length(self):
        if self.is_arrangement_clip:
            return self.end_time - self.start_time
        return (self.loop_end - self.loop_start) if self.looping else (self.end_marker - self.start_marker)
    def get_all_notes_extended(self):
        return list(self.notes)
    def add_new_notes(self, specifications):
        if not isinstance(specifications, tuple):
            raise TypeError("add_new_notes takes a tuple of MidiNoteSpecification")
        for spec in specifications:
            self.notes.append(types.SimpleNamespace(**vars(spec)))
    def add_warp_marker(self, marker):
        if not isinstance(marker, WarpMarker):
            raise TypeError("No registered converter was able to produce a C++ rvalue of type NApiHelpers::TWarpMarker")
        self.warp_markers.append(marker); self.warp_markers.sort(key=lambda m: m.beat_time)
    def remove_warp_marker(self, beat):
        self.warp_markers = [m for m in self.warp_markers if m.beat_time != beat]

class Slot:
    def __init__(self, track):
        self._live_ptr = pointer(); self.canonical_parent = track; self.clip = None
    @property
    def has_clip(self):
        return self.clip is not None
    def create_clip(self, length):
        self.clip = Clip(self, False, False, length)
    def create_audio_clip(self, path):
        if not os.path.exists(path): raise RuntimeError("Invalid path")
        self.clip = Clip(self, False, True, 32.0, file_path=path)

class Track:
    def __init__(self, name, midi=True, slots=4):
        self._live_ptr = pointer(); self.name = name
        self.has_midi_input, self.has_audio_input = midi, not midi
        self.arrangement_clips = []; self.clip_slots = [Slot(self) for _ in range(slots)]
    def _sorted(self):
        self.arrangement_clips.sort(key=lambda clip: clip.start_time)
    def create_midi_clip(self, start, length):
        if not self.has_midi_input: raise RuntimeError("MIDI clips can only be created on MIDI tracks")
        clip = Clip(self, True, False, length, start); self.arrangement_clips.append(clip); self._sorted(); return clip
    def create_audio_clip(self, path, start):
        if self.has_midi_input: raise RuntimeError("Audio clips can only be created on audio tracks")
        if not os.path.exists(path): raise RuntimeError("Invalid path")
        clip = Clip(self, True, True, 32.0, start, path); self.arrangement_clips.append(clip); self._sorted()
    def delete_clip(self, clip):
        self.arrangement_clips.remove(clip)

def note(pitch, start, duration=0.5, velocity=100.0, mute=False, probability=1.0, deviation=0.0, release=64.0):
    return types.SimpleNamespace(pitch=pitch, start_time=start, duration=duration, velocity=velocity, mute=mute,
                                 probability=probability, velocity_deviation=deviation, release_velocity=release)

def midi_clip(track, name, start, length, notes, **settings):
    clip = track.create_midi_clip(start, length); clip.name = name; clip.notes = list(notes)
    for key, value in settings.items():
        setattr(clip, key, value)
    return clip

class Bridge:
    def __init__(self, refs): self.refs = types.SimpleNamespace(get=lambda ref: refs[ref])

def run(song, refs, args):
    space = {"song": song, "bridge": Bridge(refs), "Live": Live, "result": None}
    exec(compile("import json\nARGS = json.loads(%r)\n" % json.dumps(args) + BODY, "<python.run>", "exec"), space, space)
    return json.loads(json.dumps(space["result"]))

def layout(track):
    return [(clip.name, clip.start_time, clip.end_time, sorted([n.pitch, n.start_time] for n in clip.notes)) for clip in track.arrangement_clips]

RICH = [note(60, 0.0), note(62, 1.25, 0.333333, 87.5, mute=True), note(64, 2.0, 1.0, 64.0, probability=0.5, deviation=-20.0, release=30.0), note(67, 7.75, 0.25, 127.0, probability=0.25, release=127.0)]

def scenario_deleted_clips_come_back_exactly(fail):
    song = types.SimpleNamespace(tracks=[Track("Bass"), Track("Vox", midi=False)]); bass, vox = song.tracks
    looped = midi_clip(bass, "Loop", 0.0, 4.0, RICH, loop_end=8.0, end_marker=8.0, color=0xFF3636, signature_numerator=3)
    plain = midi_clip(bass, "Plain", 16.0, 6.0, RICH[:2], looping=False, start_marker=0.5, end_marker=6.5)
    session = bass.clip_slots[1]; session.create_clip(8.0); session.clip.name = "Hook"; session.clip.notes = list(RICH); session.clip.launch_mode = 2
    sample = tempfile.NamedTemporaryFile(suffix=".wav", delete=False).name
    vox.create_audio_clip(sample, 32.0); audio = vox.arrangement_clips[0]; audio.name = "Take"; audio.gain = 0.6; audio.pitch_coarse = 3
    audio.warp_markers.insert(1, WarpMarker(1.5, 4.0)); audio.warping, audio.warp_mode = True, 4
    refs = {"r:loop": looped, "r:plain": plain, "r:hook": session.clip, "r:take": audio}
    kept = run(song, refs, {"op": "capture", "clips": list(refs)})["clips"]
    if [row["where"] for row in kept] != [{"start": 0.0, "end": 4.0}, {"start": 16.0, "end": 22.0}, {"slot": 1}, {"start": 32.0, "end": 64.0}]:
        fail("places: %r" % [row["where"] for row in kept])
    if kept[0]["leaf"]["notes"][2] != [64, 2.0, 1.0, 64.0, False, 0.5, -20.0, 30.0]:
        fail("every note field: %r" % kept[0]["leaf"]["notes"][2])
    bass.delete_clip(looped); bass.delete_clip(plain); session.clip = None; vox.delete_clip(audio)
    for row in kept:
        done = run(song, refs, {"op": "restore", "track": row["track"], "remnants": [], "clips": [{"where": row["where"], "leaf": row["leaf"]}]})
        if done["partial"]:
            fail("%s came back partly: %r" % (row["leaf"]["name"], done["partial"]))
    again = run(song, {"r:loop": bass.arrangement_clips[0], "r:plain": bass.arrangement_clips[1], "r:hook": session.clip, "r:take": vox.arrangement_clips[0]},
                {"op": "capture", "clips": ["r:loop", "r:plain", "r:hook", "r:take"]})["clips"]
    for before, after in zip(kept, again):
        if before["leaf"] != after["leaf"] or before["where"] != after["where"]:
            fail("%s isn't as it was: %r" % (before["leaf"]["name"], {k: (before["leaf"][k], after["leaf"].get(k)) for k in before["leaf"] if before["leaf"][k] != after["leaf"].get(k)}))
    os.unlink(sample)

def scenario_a_split_clip_goes_back_whole_after_its_remnants(fail):
    song = types.SimpleNamespace(tracks=[Track("Keys")]); keys = song.tracks[0]
    whole = midi_clip(keys, "Verse", 0.0, 16.0, [note(48 + i, float(i)) for i in range(16)])
    kept = run(song, {"r:keys": keys}, {"op": "capture", "track": "r:keys", "from": 4.0, "to": 8.0})
    # Live lays a new clip over its middle: the left part keeps the clip, the right part is a new one.
    whole.end_time = 4.0
    right = midi_clip(keys, "Verse", 8.0, 8.0, whole.notes, start_marker=8.0, loop_start=0.0, loop_end=16.0, end_marker=16.0)
    laid = midi_clip(keys, "New", 4.0, 4.0, [])
    remnants = run(song, {"r:keys": keys}, {"op": "capture", "track": "r:keys", "from": 0.0, "to": 16.0, "except": ["live:%d" % laid._live_ptr]})["clips"]
    keys.delete_clip(laid)
    args = {"op": "restore", "track": kept["track"], "remnants": [{"identity": r["identity"], "hash": r["hash"], "name": r["leaf"]["name"]} for r in remnants],
            "clips": [{"where": row["where"], "leaf": row["leaf"]} for row in kept["clips"]]}
    if run(song, {}, dict(args, check=True)) != {"checked": True}:
        fail("the check")
    if layout(keys) != [("Verse", 0.0, 4.0, layout(keys)[0][3]), ("Verse", 8.0, 16.0, layout(keys)[1][3])]:
        fail("a check changed something: %r" % layout(keys))
    done = run(song, {}, args)
    if [(name, start, end) for name, start, end, _ in layout(keys)] != [("Verse", 0.0, 16.0)] or len(keys.arrangement_clips[0].notes) != 16 or done["partial"]:
        fail("not whole again: %r %r" % (layout(keys), done))

def scenario_a_clip_laid_over_another_is_taken_back_and_that_one_made_whole(fail):
    # Kumi reads what the new clip will cut, Live lays it over the middle of "Verse", and Kumi reads what Live left
    # (found by the new clip's ref, and leaving it out). The undo checks first, with the new clip still there: it's
    # leaving. The change's own undo deletes the new clip, then "Verse" comes back whole.
    song = types.SimpleNamespace(tracks=[Track("Keys")]); keys = song.tracks[0]
    verse = midi_clip(keys, "Verse", 0.0, 8.0, [note(60 + i, float(i)) for i in range(8)])
    kept = run(song, {"r:keys": keys}, {"op": "capture", "track": "r:keys", "from": 2.0, "to": 4.0})
    verse.end_time = 2.0
    midi_clip(keys, "Verse", 4.0, 4.0, verse.notes, start_marker=4.0, loop_end=8.0, end_marker=8.0)
    laid = midi_clip(keys, "New", 2.0, 2.0, [])
    left = run(song, {"r:new": laid}, {"op": "capture", "track": "r:new", "from": 0.0, "to": 8.0, "except": ["live:%d" % laid._live_ptr]})["clips"]
    if [row["where"] for row in left] != [{"start": 0.0, "end": 2.0}, {"start": 4.0, "end": 8.0}]:
        fail("remnants: %r" % [row["where"] for row in left])
    args = {"op": "restore", "track": kept["track"], "leaving": ["live:%d" % laid._live_ptr],
            "remnants": [{"identity": row["identity"], "hash": row["hash"], "name": "Verse"} for row in left],
            "clips": [{"where": row["where"], "leaf": row["leaf"]} for row in kept["clips"]]}
    if run(song, {}, dict(args, check=True)) != {"checked": True}:
        fail("the check, with the new clip leaving")
    keys.delete_clip(laid)
    done = run(song, {}, args)
    if [(name, start, end) for name, start, end, _ in layout(keys)] != [("Verse", 0.0, 8.0)] or len(keys.arrangement_clips[0].notes) != 8 or done["partial"]:
        fail("not whole again: %r %r" % (layout(keys), done))

def scenario_nothing_changes_when_a_check_fails(fail):
    song = types.SimpleNamespace(tracks=[Track("Keys"), Track("Vox", midi=False)]); keys, vox = song.tracks
    whole = midi_clip(keys, "Verse", 0.0, 8.0, [note(60, 0.0)])
    kept = run(song, {"r:verse": whole}, {"op": "capture", "clips": ["r:verse"]})["clips"][0]
    whole.end_time = 4.0
    remnant = run(song, {"r:verse": whole}, {"op": "capture", "clips": ["r:verse"]})["clips"][0]
    args = {"op": "restore", "track": kept["track"], "remnants": [{"identity": remnant["identity"], "hash": remnant["hash"], "name": "Verse"}],
            "clips": [{"where": kept["where"], "leaf": kept["leaf"]}]}
    # The remnant edited since: refused, and it's still there as it is.
    whole.notes.append(note(72, 1.0))
    try:
        run(song, {}, args); fail("an edited remnant was taken")
    except ValueError as error:
        if "“Verse” changed in Live since" not in str(error): fail("edited: %s" % error)
    if len(keys.arrangement_clips) != 1 or len(whole.notes) != 2: fail("something changed: %r" % layout(keys))
    whole.notes.pop()
    # Another clip in its place now.
    midi_clip(keys, "Fill", 5.0, 2.0, [])
    try:
        run(song, {}, args); fail("an occupied place was taken")
    except ValueError as error:
        if "“Fill” is in “Verse”'s place now" not in str(error): fail("occupied: %s" % error)
    if [clip.name for clip in keys.arrangement_clips] != ["Verse", "Fill"]: fail("something changed: %r" % layout(keys))
    # An audio clip whose file has gone, and a clip of the other kind than its track.
    sample = tempfile.NamedTemporaryFile(suffix=".wav", delete=False).name
    vox.create_audio_clip(sample, 0.0); take = vox.arrangement_clips[0]; take.name = "Take"
    kept_take = run(song, {"r:take": take}, {"op": "capture", "clips": ["r:take"]})["clips"][0]
    vox.delete_clip(take); os.unlink(sample)
    for clips, words in (([{"where": kept_take["where"], "leaf": kept_take["leaf"]}], "“Take”'s audio file isn't there any more"),
                         ([{"where": {"start": 40.0, "end": 44.0}, "leaf": kept["leaf"]}], "“Verse” is a MIDI clip, and its track takes audio now")):
        try:
            run(song, {}, {"op": "restore", "track": kept_take["track"], "remnants": [], "clips": clips}); fail("refusal missing: " + words)
        except ValueError as error:
            if words not in str(error): fail("%s: %s" % (words, error))
    if vox.arrangement_clips: fail("audio track changed")

def scenario_a_large_clip_takes_its_notes_in_more_calls(fail):
    song = types.SimpleNamespace(tracks=[Track("Drums")]); drums = song.tracks[0]
    big = midi_clip(drums, "Hats", 0.0, 64.0, [note(42, i * 0.0625) for i in range(1024)])
    kept = run(song, {"r:hats": big}, {"op": "capture", "clips": ["r:hats"]})["clips"][0]
    drums.delete_clip(big)
    first, rest = kept["leaf"]["notes"][:300], kept["leaf"]["notes"][300:]
    done = run(song, {}, {"op": "restore", "track": kept["track"], "remnants": [], "clips": [{"where": kept["where"], "leaf": dict(kept["leaf"], notes=first), "notesToCome": True}]})
    if done["partial"]: fail("partial before the rest: %r" % done["partial"])
    added = run(song, {}, {"op": "notes", "track": kept["track"], "clip": done["made"][0]["identity"], "notes": rest})
    clip = drums.arrangement_clips[0]
    if added != {"added": 724} or sorted([n.pitch, n.start_time] for n in clip.notes) != sorted([n[0], n[1]] for n in kept["leaf"]["notes"]):
        fail("notes: %r, %d" % (added, len(clip.notes)))

failures = []
for name, scenario in sorted((name, value) for name, value in list(globals().items()) if name.startswith("scenario_")):
    problems = []
    try:
        scenario(problems.append)
    except Exception as error:
        problems.append("raised %s: %s" % (type(error).__name__, error))
    failures.append({"scenario": name[len("scenario_"):], "problems": problems})
print(json.dumps(failures))
