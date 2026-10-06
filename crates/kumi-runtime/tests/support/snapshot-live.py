"""A Live for snapshots.py (BODY, given before this file), with what probes of Live 12.4.15b5 found:
- a new Arrangement clip (made, or a duplicate) is laid over the clips it lands on and cuts them: covered ones go, an
  edge is trimmed, a middle splits (the left part keeps the clip, the right part is a new one);
- an audio clip is made at its file's length, looped; setting its markers doesn't change its Arrangement span, and a
  duplicate copies its whole span;
- no clip has fade members;
- envelope events read back in Live's own terms (Track Volume's are a curve of the fader's), while a new event and
  value_at_time take the parameter's; each event has a curve (control coefficients);
- a Session clip's slot sits in a scene; a track can be frozen, and carries Kumi's id in its data; grooves come from
  the pool; follow actions only on a Live that has them.
Each scenario runs the script as Live's Python would (ARGS in front, song, bridge and Live in its namespace, its
result read back) and says what went wrong, if anything; the printed list is what the test checks."""
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

class EnvelopeEventControlCoefficients:
    def __init__(self, x1=0.5, y1=0.5, x2=0.5, y2=0.5):
        self.x1, self.y1, self.x2, self.y2 = x1, y1, x2, y2

class EnvelopeEvent:
    def __init__(self, time, value, control_coefficients=None):
        self.time, self.value = time, value
        self.control_coefficients = control_coefficients or EnvelopeEventControlCoefficients()

Live = types.SimpleNamespace(Clip=types.SimpleNamespace(MidiNoteSpecification=MidiNoteSpecification, WarpMarker=WarpMarker),
                             Envelope=types.SimpleNamespace(EnvelopeEvent=EnvelopeEvent, EnvelopeEventControlCoefficients=EnvelopeEventControlCoefficients))

class Parameter:
    def __init__(self, name, own=lambda value: value):
        self._live_ptr = pointer(); self.name = name; self.own = own

class Device:
    def __init__(self, name, parameters, chains=()):
        self._live_ptr = pointer(); self.name = name
        self.parameters = [Parameter(parameter) for parameter in parameters]
        self.chains = [types.SimpleNamespace(devices=list(devices)) for devices in chains]

class Envelope:
    def __init__(self, clip, parameter):
        self._live_ptr = pointer(); self.canonical_parent = clip; self.parameter = parameter; self.events = []
    def create_event(self, event):
        if not isinstance(event, EnvelopeEvent): raise TypeError("create_event takes an EnvelopeEvent")
        self.events.append((float(event.time), float(event.value), event.control_coefficients))
        self.events.sort(key=lambda row: row[0])
    def events_in_range(self, start, end):
        return [types.SimpleNamespace(time=time, value=self.parameter.own(value), control_coefficients=curve)
                for time, value, curve in self.events if start <= time <= end]
    def value_at_time(self, time):
        before = [row for row in self.events if row[0] <= time]
        if not before: return self.events[0][1]
        after = [row for row in self.events if row[0] > time]
        (t0, v0, _), = before[-1:]
        if not after: return v0
        t1, v1, _ = after[0]
        return v0 + (v1 - v0) * (time - t0) / (t1 - t0)
    def insert_step(self, time, length, value):
        held = self.value_at_time(time)
        for row in ((time, held), (time, value), (time + length, value), (time + length, held)):
            self.events.append((row[0], row[1], EnvelopeEventControlCoefficients()))
        self.events.sort(key=lambda row: row[0])

class Groove:
    def __init__(self, name):
        self._live_ptr = pointer(); self.name = name

FILE_BEATS = {}

class Clip:
    groove = None
    def __init__(self, parent, arrangement, audio, length, start=0.0, file_path=None):
        self._live_ptr = pointer(); self.canonical_parent = parent
        self.is_arrangement_clip, self.is_audio_clip, self.is_midi_clip = arrangement, audio, not audio
        self.name, self.color, self.muted, self.looping = "", 0x3BA8FF, False, True
        self.loop_start, self.loop_end, self.start_marker, self.end_marker = 0.0, length, 0.0, length
        self.signature_numerator, self.signature_denominator = 4, 4
        self.start_time, self.end_time = start, start + length
        self.launch_mode, self.launch_quantization, self.legato, self.velocity_amount = 0, 14, False, 0.0
        self.notes, self.envelopes, self.unlisted, self.drops = [], [], 0, None
        if audio:
            self.file_path, self.warping, self.warp_mode, self.gain = file_path, True, 0, 0.4
            self.pitch_coarse, self.pitch_fine, self.ram_mode = 0, 0.0, False
            self.warp_markers = [WarpMarker(0.0, 0.0), WarpMarker(16.0, 32.0)]
    @property
    def length(self):
        if self.is_arrangement_clip:
            return self.end_time - self.start_time
        return (self.loop_end - self.loop_start) if self.looping else (self.end_marker - self.start_marker)
    @property
    def has_envelopes(self):
        return bool(self.envelopes) or self.unlisted > 0
    @property
    def automation_envelopes(self):
        # An envelope Live lists without a parameter: a MIDI controller's.
        return list(self.envelopes) + [types.SimpleNamespace(parameter=None) for _ in range(self.unlisted)]
    def automation_envelope(self, parameter):
        if self.is_arrangement_clip: return None
        return next((envelope for envelope in self.envelopes if envelope.parameter is parameter), None)
    def create_automation_envelope(self, parameter):
        if self.is_arrangement_clip: raise RuntimeError("Automation envelopes can only be created for Session clips")
        envelope = self.automation_envelope(parameter)
        if envelope is None:
            envelope = Envelope(self, parameter); self.envelopes.append(envelope)
        return envelope
    def get_all_notes_extended(self):
        return list(self.notes)
    def add_new_notes(self, specifications):
        if not isinstance(specifications, tuple):
            raise TypeError("add_new_notes takes a tuple of MidiNoteSpecification")
        for spec in specifications:
            if spec.pitch != self.drops:
                self.notes.append(types.SimpleNamespace(**vars(spec)))
    def add_warp_marker(self, marker):
        if not isinstance(marker, WarpMarker):
            raise TypeError("No registered converter was able to produce a C++ rvalue of type NApiHelpers::TWarpMarker")
        self.warp_markers.append(marker); self.warp_markers.sort(key=lambda m: m.beat_time)
    def remove_warp_marker(self, beat):
        self.warp_markers = [m for m in self.warp_markers if m.beat_time != beat]
    def copy(self, parent, start, end):
        """Live's duplicate: everything the clip is, at another place."""
        clip = type(self).__new__(type(self))
        clip.__dict__.update({key: value for key, value in self.__dict__.items()})
        clip._live_ptr, clip.canonical_parent, clip.start_time, clip.end_time = pointer(), parent, start, end
        clip.notes = [types.SimpleNamespace(**vars(note)) for note in self.notes]
        clip.envelopes = []
        if self.is_audio_clip:
            clip.warp_markers = [WarpMarker(m.sample_time, m.beat_time) for m in self.warp_markers]
        return clip

class FollowClip(Clip):
    """A clip on a Live that has follow actions in Python. Their chances add up to 100."""
    follow_action_enabled, follow_action_linked, follow_action_a, follow_action_b = False, True, 0, 0
    follow_action_chance_a, follow_action_loop_count, follow_action_time = 100, 1, 4.0
    follow_action_jump_a, follow_action_jump_b = 0, 0
    @property
    def follow_action_chance_b(self):
        return 100 - self.follow_action_chance_a
    @follow_action_chance_b.setter
    def follow_action_chance_b(self, value):
        self.follow_action_chance_a = 100 - value

class Scene:
    def __init__(self, name=""):
        self._live_ptr = pointer(); self.name = name

class Slot:
    def __init__(self, track):
        self._live_ptr = pointer(); self.canonical_parent = track; self.clip = None
    @property
    def has_clip(self):
        return self.clip is not None
    def create_clip(self, length):
        if self.canonical_parent.is_frozen: raise RuntimeError("Clips cannot be created on frozen tracks")
        self.clip = self.canonical_parent.kind(self, False, False, length)
    def create_audio_clip(self, path):
        if not os.path.exists(path): raise RuntimeError("Invalid path")
        self.clip = self.canonical_parent.kind(self, False, True, FILE_BEATS.get(path, 32.0), file_path=path)

class Track:
    def __init__(self, name, midi=True, kind=Clip):
        self._live_ptr = pointer(); self.name = name; self.kind = kind
        self.has_midi_input, self.has_audio_input = midi, not midi
        self.is_frozen, self.data, self.devices, self.made = False, {}, [], 0
        self.broken_after = None
        self.mixer_device = types.SimpleNamespace(volume=Parameter("Track Volume", own=lambda value: value * value * value),
                                                  panning=Parameter("Track Panning"), track_activator=Parameter("Speaker On"), sends=[])
        self.arrangement_clips = []; self.clip_slots = []
    def get_data(self, key, default):
        return self.data.get(key, default)
    def _made(self):
        if self.is_frozen: raise RuntimeError("Clips cannot be created on frozen tracks")
        self.made += 1
        if self.broken_after is not None and self.made > self.broken_after: raise RuntimeError("Live refused")
    def _lay(self, new):
        """Live lays a new clip over what's there: covered clips go, an edge is trimmed, a middle splits."""
        for other in list(self.arrangement_clips):
            start, end = other.start_time, other.end_time
            if end <= new.start_time + 1e-9 or start >= new.end_time - 1e-9:
                continue
            if start >= new.start_time - 1e-9 and end <= new.end_time + 1e-9:
                self.arrangement_clips.remove(other)
            elif start < new.start_time and end > new.end_time:
                right = other.copy(self, new.end_time, end); right.start_marker = other.start_marker + (new.end_time - start)
                other.end_time = new.start_time; self.arrangement_clips.append(right)
            elif start < new.start_time:
                other.end_time = new.start_time
                other.loop_end = min(other.loop_end, other.loop_start + (other.end_time - start))
            else:
                other.start_marker += new.end_time - start; other.start_time = new.end_time
        self.arrangement_clips.append(new)
        self.arrangement_clips.sort(key=lambda clip: clip.start_time)
        return new
    def create_midi_clip(self, start, length):
        if not self.has_midi_input: raise RuntimeError("MIDI clips can only be created on MIDI tracks")
        self._made()
        return self._lay(self.kind(self, True, False, length, start))
    def create_audio_clip(self, path, start):
        if self.has_midi_input: raise RuntimeError("Audio clips can only be created on audio tracks")
        if not os.path.exists(path): raise RuntimeError("Invalid path")
        self._made()
        self._lay(self.kind(self, True, True, FILE_BEATS.get(path, 32.0), start, path))
    def duplicate_clip_to_arrangement(self, clip, time):
        self._made()
        self._lay(clip.copy(self, time, time + (clip.end_time - clip.start_time)))
    def delete_clip(self, clip):
        self.arrangement_clips.remove(clip)

def make_song(*tracks, scenes=4):
    song = types.SimpleNamespace(tracks=list(tracks), scenes=[Scene("Scene %d" % (n + 1)) for n in range(scenes)],
                                 groove_pool=types.SimpleNamespace(grooves=[Groove("Swing 16ths 66"), Groove("MPC 16 Swing-62")]))
    for track in tracks:
        track.clip_slots = [Slot(track) for _ in range(scenes)]
    return song

def note(pitch, start, duration=0.5, velocity=100.0, mute=False, probability=1.0, deviation=0.0, release=64.0):
    return types.SimpleNamespace(pitch=pitch, start_time=start, duration=duration, velocity=velocity, mute=mute,
                                 probability=probability, velocity_deviation=deviation, release_velocity=release)

def midi_clip(track, name, start, length, notes, **settings):
    clip = track.create_midi_clip(start, length); clip.name = name; clip.notes = list(notes)
    for key, value in settings.items():
        setattr(clip, key, value)
    return clip

def audio_file(beats=32.0):
    path = tempfile.NamedTemporaryFile(suffix=".wav", delete=False).name
    FILE_BEATS[path] = beats
    return path

def audio_clip(track, name, path, start, **settings):
    before = list(track.arrangement_clips)
    track.create_audio_clip(path, start)
    clip = [c for c in track.arrangement_clips if c not in before][0]; clip.name = name
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

def spans(track):
    return [(clip.name, clip.start_time, clip.end_time) for clip in track.arrangement_clips]

def restore_args(rows, remnants=(), leaving=()):
    first = rows[0]
    args = {"op": "restore", "track": first["track"], "trackName": first["trackName"], "remnants": list(remnants), "leaving": list(leaving),
            "clips": [{"where": row["where"], "leaf": row["leaf"]} for row in rows]}
    if "trackId" in first:
        args["trackId"] = first["trackId"]
    return args

def differences(before, after):
    return {k: (before["leaf"][k], after["leaf"].get(k)) for k in before["leaf"] if before["leaf"][k] != after["leaf"].get(k)}

RICH = [note(60, 0.0), note(62, 1.25, 0.333333, 87.5, mute=True), note(64, 2.0, 1.0, 64.0, probability=0.5, deviation=-20.0, release=30.0), note(67, 7.75, 0.25, 127.0, probability=0.25, release=127.0)]

def scenario_deleted_clips_come_back_exactly(fail):
    bass, vox = Track("Bass"), Track("Vox", midi=False); song = make_song(bass, vox)
    bass.data["kumi.track"] = "01J9BASS"
    looped = midi_clip(bass, "Loop", 0.0, 4.0, RICH, loop_end=8.0, end_marker=8.0, color=0xFF3636, signature_numerator=3)
    plain = midi_clip(bass, "Plain", 16.0, 6.0, RICH[:2], looping=False, start_marker=0.5, end_marker=6.5)
    session = bass.clip_slots[1]; session.create_clip(8.0); session.clip.name = "Hook"; session.clip.notes = list(RICH); session.clip.launch_mode = 2
    session.clip.groove = song.groove_pool.grooves[1]
    sample = audio_file()
    audio = audio_clip(vox, "Take", sample, 32.0, gain=0.6, pitch_coarse=3, warp_mode=4)
    audio.warp_markers.insert(1, WarpMarker(1.5, 4.0))
    refs = {"r:loop": looped, "r:plain": plain, "r:hook": session.clip, "r:take": audio}
    kept = run(song, refs, {"op": "capture", "clips": list(refs)})["clips"]
    if [row["where"] for row in kept] != [{"start": 0.0, "end": 4.0}, {"start": 16.0, "end": 22.0},
                                          {"slot": 1, "scene": "live:%d" % song.scenes[1]._live_ptr, "sceneName": "Scene 2"}, {"start": 32.0, "end": 64.0}]:
        fail("places: %r" % [row["where"] for row in kept])
    if kept[0]["leaf"]["notes"][2] != [64, 2.0, 1.0, 64.0, False, 0.5, -20.0, 30.0]:
        fail("every note field: %r" % kept[0]["leaf"]["notes"][2])
    if (kept[0].get("trackId"), kept[3].get("trackId"), kept[3]["trackName"]) != ("01J9BASS", None, "Vox"):
        fail("tracks: %r" % [(row.get("trackId"), row["trackName"]) for row in kept])
    bass.delete_clip(looped); bass.delete_clip(plain); session.clip = None; vox.delete_clip(audio)
    for row in kept:
        done = run(song, refs, restore_args([row]))
        # Live doesn't give an Arrangement audio clip's fades: that one is named, the rest are whole.
        want = [{"name": "Take", "missing": ["its fades"]}] if row["leaf"]["name"] == "Take" else []
        if done["partial"] != want or "error" in done:
            fail("%s came back: %r" % (row["leaf"]["name"], done))
    again = run(song, {"r:loop": bass.arrangement_clips[0], "r:plain": bass.arrangement_clips[1], "r:hook": session.clip, "r:take": vox.arrangement_clips[0]},
                {"op": "capture", "clips": ["r:loop", "r:plain", "r:hook", "r:take"]})["clips"]
    for before, after in zip(kept, again):
        if before["leaf"] != after["leaf"] or before["where"] != after["where"]:
            fail("%s isn't as it was: %r" % (before["leaf"]["name"], differences(before, after)))
    os.unlink(sample)

def scenario_an_audio_clip_comes_back_without_cutting_its_neighbours(fail):
    # Live makes an audio clip at its file's length: made where it was, "Take" (cut to 4 beats by "Take 2") would
    # land on 32 to 64 and cut "Take 2" away.
    vox = Track("Vox", midi=False); song = make_song(vox)
    sample = audio_file(32.0)
    take = audio_clip(vox, "Take", sample, 32.0, looping=False, gain=0.6, pitch_coarse=3)
    audio_clip(vox, "Take 2", sample, 36.0)
    loop = audio_clip(vox, "Loop", sample, 100.0, loop_end=2.0)
    audio_clip(vox, "After", sample, 108.0)
    if spans(vox) != [("Take", 32.0, 36.0), ("Take 2", 36.0, 68.0), ("Loop", 100.0, 108.0), ("After", 108.0, 140.0)]:
        fail("the set-up: %r" % spans(vox))
    kept = run(song, {"r:take": take, "r:loop": loop}, {"op": "capture", "clips": ["r:take", "r:loop"]})["clips"]
    vox.delete_clip(take); vox.delete_clip(loop)
    done = run(song, {}, restore_args(kept))
    if "error" in done or [row["name"] for row in done["made"]] != ["Take", "Loop"]:
        fail("restore: %r" % done)
    if spans(vox) != [("Take", 32.0, 36.0), ("Take 2", 36.0, 68.0), ("Loop", 100.0, 108.0), ("After", 108.0, 140.0)]:
        fail("not as it was, or a neighbour cut: %r" % spans(vox))
    again = run(song, {"r:take": vox.arrangement_clips[0], "r:loop": vox.arrangement_clips[2]}, {"op": "capture", "clips": ["r:take", "r:loop"]})["clips"]
    for before, after in zip(kept, again):
        if differences(before, after):
            fail("%s isn't as it was: %r" % (before["leaf"]["name"], differences(before, after)))
    # A clip longer than Live can make it (its loop played past its file's length) isn't made; nothing is cut.
    long = audio_clip(vox, "Long", sample, 200.0, loop_end=2.0)
    long.end_time = 264.0
    kept = run(song, {"r:long": long}, {"op": "capture", "clips": ["r:long"]})["clips"]
    vox.delete_clip(long)
    done = run(song, {}, restore_args(kept))
    if "at its length" not in done.get("error", "") or done["made"] or [name for name, _, _ in spans(vox)] != ["Take", "Take 2", "Loop", "After"]:
        fail("a clip too long to make: %r %r" % (done, spans(vox)))
    os.unlink(sample)

def scenario_a_session_clips_automation_groove_and_follow_actions_come_back(fail):
    keys = Track("Keys", kind=FollowClip); song = make_song(keys)
    keys.devices = [Device("Rack", [], chains=[[Device("Filter", ["Frequency", "Resonance"])]])]
    frequency = keys.devices[0].chains[0].devices[0].parameters[0]
    slot = keys.clip_slots[0]; slot.create_clip(4.0); hook = slot.clip; hook.name = "Hook"; hook.notes = list(RICH)
    volume = hook.create_automation_envelope(keys.mixer_device.volume)
    for time, value, curve in ((0.0, 0.2, None), (1.0, 0.85, EnvelopeEventControlCoefficients(0.2, 0.9, 0.6, 0.1)), (2.5, 0.4, None)):
        volume.create_event(EnvelopeEvent(time, value, curve))
    volume.insert_step(3.0, 0.5, 0.1)
    hook.create_automation_envelope(frequency).create_event(EnvelopeEvent(1.0, 0.3))
    hook.groove = song.groove_pool.grooves[0]
    hook.follow_action_enabled, hook.follow_action_a, hook.follow_action_b, hook.follow_action_chance_a, hook.follow_action_chance_b = True, 3, 4, 70, 30
    kept = run(song, {"r:hook": hook}, {"op": "capture", "clips": ["r:hook"]})["clips"][0]
    if len(kept["leaf"]["envelopes"]) != 2 or kept["leaf"].get("partial") or kept["leaf"]["groove"]["name"] != "Swing 16ths 66":
        fail("capture: %r" % {k: kept["leaf"].get(k) for k in ("envelopes", "partial", "groove", "follow")})
    slot.clip = None
    done = run(song, {}, restore_args([kept]))
    if done["partial"] or "error" in done:
        fail("came back partly: %r" % done)
    again = run(song, {"r:hook": slot.clip}, {"op": "capture", "clips": ["r:hook"]})["clips"][0]
    for key in ("groove", "follow", "notes", "launch"):
        if again["leaf"][key] != kept["leaf"][key]:
            fail("%s: %r" % (key, (kept["leaf"][key], again["leaf"][key])))
    if [[event[:2] + [event[3]] for event in row["events"]] for row in again["leaf"]["envelopes"]] != \
       [[event[:2] + [event[3]] for event in row["events"]] for row in kept["leaf"]["envelopes"]]:
        fail("automation: %r" % again["leaf"]["envelopes"])
    # The filter gone since: that automation can't come back, and the undo says so.
    slot.clip = None; keys.devices = []
    done = run(song, {}, restore_args([kept]))
    if done["partial"] != [{"name": "Hook", "missing": ["its automation"]}]:
        fail("a parameter gone: %r" % done)
    # A MIDI controller's envelope (Live lists it with no parameter) is named before anything happens.
    slot.clip.unlisted = 1
    kept = run(song, {"r:hook": slot.clip}, {"op": "capture", "clips": ["r:hook"]})["clips"][0]
    if kept["leaf"].get("partial") != ["envelopes"]:
        fail("a controller's envelope: %r" % kept["leaf"].get("partial"))

def scenario_what_live_wont_put_back_is_named(fail):
    keys, vox = Track("Keys"), Track("Vox", midi=False); song = make_song(keys, vox)
    ride = midi_clip(keys, "Ride", 0.0, 4.0, RICH)
    ride.envelopes = [Envelope(ride, keys.mixer_device.volume)]
    grooved = midi_clip(keys, "Swung", 8.0, 4.0, RICH); grooved.groove = song.groove_pool.grooves[1]
    sample = audio_file()
    take = audio_clip(vox, "Take", sample, 0.0)
    kept = run(song, {"r:ride": ride, "r:swung": grooved, "r:take": take}, {"op": "capture", "clips": ["r:ride", "r:swung", "r:take"]})["clips"]
    if [row["leaf"].get("partial") for row in kept] != [["envelopes"], None, ["fades"]]:
        fail("named at capture: %r" % [row["leaf"].get("partial") for row in kept])
    keys.delete_clip(ride); keys.delete_clip(grooved); vox.delete_clip(take)
    song.groove_pool.grooves.pop()
    done = [run(song, {}, restore_args([row])) for row in kept]
    if [d["partial"] for d in done] != [[{"name": "Ride", "missing": ["its automation"]}], [{"name": "Swung", "missing": ["its groove"]}],
                                        [{"name": "Take", "missing": ["its fades"]}]]:
        fail("named after: %r" % [d["partial"] for d in done])
    os.unlink(sample)

def scenario_a_split_clip_goes_back_whole_after_its_remnants(fail):
    keys = Track("Keys"); song = make_song(keys)
    whole = midi_clip(keys, "Verse", 0.0, 16.0, [note(48 + i, float(i)) for i in range(16)])
    kept = run(song, {"r:keys": keys}, {"op": "capture", "track": "r:keys", "from": 4.0, "to": 8.0})
    # Live lays a new clip over its middle: the left part keeps the clip, the right part is a new one.
    laid = midi_clip(keys, "New", 4.0, 4.0, [])
    if spans(keys) != [("Verse", 0.0, 4.0), ("New", 4.0, 8.0), ("Verse", 8.0, 16.0)]:
        fail("Live's laying over: %r" % spans(keys))
    remnants = run(song, {"r:keys": keys}, {"op": "capture", "track": "r:keys", "from": 0.0, "to": 16.0, "except": ["live:%d" % laid._live_ptr]})["clips"]
    keys.delete_clip(laid)
    args = restore_args(kept["clips"], remnants=[{"identity": r["identity"], "hash": r["hash"], "name": r["leaf"]["name"]} for r in remnants])
    if run(song, {}, dict(args, check=True)) != {"checked": True}:
        fail("the check")
    if spans(keys) != [("Verse", 0.0, 4.0), ("Verse", 8.0, 16.0)]:
        fail("a check changed something: %r" % layout(keys))
    done = run(song, {}, args)
    if spans(keys) != [("Verse", 0.0, 16.0)] or len(keys.arrangement_clips[0].notes) != 16 or done["partial"] or done["removed"] != ["Verse", "Verse"]:
        fail("not whole again: %r %r" % (layout(keys), done))

def scenario_a_clip_laid_over_another_is_taken_back_and_that_one_made_whole(fail):
    # Kumi reads what the new clip will cut, Live lays it over the middle of "Verse", and Kumi reads what Live left
    # (found by the new clip's ref, and leaving it out). The undo checks first, with the new clip still there: it's
    # leaving. The change's own undo deletes the new clip, then "Verse" comes back whole.
    keys = Track("Keys"); song = make_song(keys)
    midi_clip(keys, "Verse", 0.0, 8.0, [note(60 + i, float(i)) for i in range(8)])
    kept = run(song, {"r:keys": keys}, {"op": "capture", "track": "r:keys", "from": 2.0, "to": 4.0})
    laid = midi_clip(keys, "New", 2.0, 2.0, [])
    left = run(song, {"r:new": laid}, {"op": "capture", "track": "r:new", "from": 0.0, "to": 8.0, "except": ["live:%d" % laid._live_ptr]})["clips"]
    if [row["where"] for row in left] != [{"start": 0.0, "end": 2.0}, {"start": 4.0, "end": 8.0}]:
        fail("remnants: %r" % [row["where"] for row in left])
    args = restore_args(kept["clips"], remnants=[{"identity": row["identity"], "hash": row["hash"], "name": "Verse"} for row in left],
                        leaving=["live:%d" % laid._live_ptr])
    if run(song, {}, dict(args, check=True)) != {"checked": True}:
        fail("the check, with the new clip leaving")
    keys.delete_clip(laid)
    done = run(song, {}, args)
    if spans(keys) != [("Verse", 0.0, 8.0)] or len(keys.arrangement_clips[0].notes) != 8 or done["partial"]:
        fail("not whole again: %r %r" % (layout(keys), done))

def scenario_nothing_changes_when_a_check_fails(fail):
    keys, vox = Track("Keys"), Track("Vox", midi=False); song = make_song(keys, vox)
    whole = midi_clip(keys, "Verse", 0.0, 8.0, [note(60, 0.0)])
    kept = run(song, {"r:verse": whole}, {"op": "capture", "clips": ["r:verse"]})["clips"][0]
    whole.end_time = 4.0
    remnant = run(song, {"r:verse": whole}, {"op": "capture", "clips": ["r:verse"]})["clips"][0]
    args = restore_args([kept], remnants=[{"identity": remnant["identity"], "hash": remnant["hash"], "name": "Verse"}])
    def refused(args, words, what):
        try:
            run(song, {}, args); fail("%s wasn't refused" % what)
        except ValueError as error:
            if words not in str(error): fail("%s: %s" % (what, error))
    # The remnant edited since: refused, and it's still there as it is.
    whole.notes.append(note(72, 1.0))
    refused(args, "“Verse” changed in Live since", "an edited remnant")
    if len(keys.arrangement_clips) != 1 or len(whole.notes) != 2: fail("something changed: %r" % layout(keys))
    whole.notes.pop()
    # Another clip in its place now.
    fill = midi_clip(keys, "Fill", 5.0, 2.0, [])
    refused(args, "“Fill” is in “Verse”'s place now", "an occupied place")
    keys.delete_clip(fill)
    # The track frozen, or another track at the address Live gave the deleted one.
    keys.is_frozen = True
    refused(args, "“Keys” is frozen", "a frozen track")
    keys.is_frozen = False
    keys.name = "Pad"
    refused(args, "its track isn't in the Set any more", "another track")
    keys.name = "Keys"
    if [clip.name for clip in keys.arrangement_clips] != ["Verse"]: fail("something changed: %r" % layout(keys))
    # An audio clip whose file has gone, and a clip of the other kind than its track.
    sample = audio_file()
    take = audio_clip(vox, "Take", sample, 0.0)
    kept_take = run(song, {"r:take": take}, {"op": "capture", "clips": ["r:take"]})["clips"][0]
    vox.delete_clip(take); os.unlink(sample)
    refused(restore_args([kept_take]), "“Take”'s audio file isn't there any more", "a missing file")
    refused(dict(restore_args([kept_take]), clips=[{"where": {"start": 40.0, "end": 44.0}, "leaf": kept["leaf"]}]),
            "“Verse” is a MIDI clip, and its track takes audio now", "the other kind")
    if vox.arrangement_clips: fail("audio track changed")

def scenario_a_session_clip_goes_back_to_its_scene(fail):
    keys = Track("Keys"); song = make_song(keys)
    keys.clip_slots[1].create_clip(4.0); hook = keys.clip_slots[1].clip; hook.name = "Hook"; hook.notes = list(RICH)
    kept = run(song, {"r:hook": hook}, {"op": "capture", "clips": ["r:hook"]})["clips"][0]
    keys.clip_slots[1].clip = None
    # A scene added above: the clip goes back to its own scene, one slot down.
    song.scenes.insert(0, Scene("Intro")); keys.clip_slots.insert(0, Slot(keys))
    done = run(song, {}, restore_args([kept]))
    if not keys.clip_slots[2].has_clip or keys.clip_slots[1].has_clip or done["partial"]:
        fail("not in its scene: %r" % ([slot.has_clip for slot in keys.clip_slots], done))
    # Its slot taken, or its scene gone: refused.
    keys.clip_slots[2].clip = None; keys.clip_slots[2].create_clip(2.0)
    try:
        run(song, {}, restore_args([kept])); fail("a taken slot was used")
    except ValueError as error:
        if "another clip is in “Hook”'s slot now" not in str(error): fail("taken: %s" % error)
    del song.scenes[2]; del keys.clip_slots[2]
    try:
        run(song, {}, restore_args([kept])); fail("a scene that's gone was used")
    except ValueError as error:
        if "“Hook”'s scene isn't in the Set any more" not in str(error): fail("gone: %s" % error)

def scenario_a_failure_after_the_checks_says_how_far_it_got(fail):
    keys = Track("Keys"); song = make_song(keys)
    a = midi_clip(keys, "A", 0.0, 4.0, [note(60, 0.0)]); b = midi_clip(keys, "B", 8.0, 4.0, [note(62, 0.0)])
    kept = run(song, {"r:a": a, "r:b": b}, {"op": "capture", "clips": ["r:a", "r:b"]})["clips"]
    keys.delete_clip(a); keys.delete_clip(b)
    keys.broken_after = keys.made + 1
    done = run(song, {}, restore_args(kept))
    if [row["name"] for row in done["made"]] != ["A"] or "Live refused" not in done.get("error", ""):
        fail("how far it got: %r" % done)

def scenario_a_large_clip_takes_its_notes_in_more_calls_and_the_last_checks_them(fail):
    drums = Track("Drums"); song = make_song(drums)
    big = midi_clip(drums, "Hats", 0.0, 64.0, [note(42, i * 0.0625) for i in range(1024)] + [note(127, 3.0)])
    kept = run(song, {"r:hats": big}, {"op": "capture", "clips": ["r:hats"]})["clips"][0]
    drums.delete_clip(big)
    notes = kept["leaf"]["notes"]
    first, rest = notes[:300], notes[300:]
    args = restore_args([kept]); args["clips"][0]["leaf"] = dict(kept["leaf"], notes=first); args["clips"][0]["notesToCome"] = True
    done = run(song, {}, args)
    if done["partial"]: fail("partial before the rest: %r" % done["partial"])
    track = {k: kept[k] for k in ("track", "trackName")}
    added = run(song, {}, dict(track, op="notes", clip=done["made"][0]["identity"], notes=rest, notesHash=kept["notesHash"]))
    clip = drums.arrangement_clips[0]
    if added != {"added": 725, "exact": True} or sorted([n.pitch, n.start_time] for n in clip.notes) != sorted([n[0], n[1]] for n in notes):
        fail("notes: %r, %d" % (added, len(clip.notes)))
    # A note Live drops on the way is found by the last call.
    drums.delete_clip(clip)
    done = run(song, {}, args)
    drums.arrangement_clips[0].drops = 127
    added = run(song, {}, dict(track, op="notes", clip=done["made"][0]["identity"], notes=rest, notesHash=kept["notesHash"]))
    if added.get("exact") is not False:
        fail("a dropped note went unsaid: %r" % added)

def scenario_state_says_what_the_track_holds(fail):
    keys = Track("Keys"); song = make_song(keys)
    midi_clip(keys, "Verse", 0.0, 8.0, [note(60, 0.0)]); midi_clip(keys, "Far", 400.0, 4.0, [])
    keys.clip_slots[2].create_clip(4.0)
    ask = {"op": "state", "track": "live:%d" % keys._live_ptr, "trackName": "Keys", "from": 4.0, "scenes": ["live:%d" % song.scenes[2]._live_ptr, "live:%d" % song.scenes[3]._live_ptr]}
    before = run(song, {}, ask)
    if [row[:3] for row in before["clips"]] != [["Far", 400.0, 404.0], ["Verse", 0.0, 8.0]] or before["slots"][1] is not None or before["slots"][0] is None:
        fail("state: %r" % before)
    keys.arrangement_clips[0].notes.append(note(61, 1.0))
    if run(song, {}, ask) == before:
        fail("an edit didn't change the state")

failures = []
for name, scenario in sorted((name, value) for name, value in list(globals().items()) if name.startswith("scenario_")):
    problems = []
    try:
        scenario(problems.append)
    except Exception as error:
        problems.append("raised %s: %s" % (type(error).__name__, error))
    failures.append({"scenario": name[len("scenario_"):], "problems": problems})
print(json.dumps(failures))
