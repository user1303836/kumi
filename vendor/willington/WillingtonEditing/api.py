"""Native editing methods; default resolution accepts validated profiles only."""
import json
import hashlib
import hmac
import math
import os
import Live
try:
    import _ctypes as ct
except ImportError:
    try:
        import ctypes as ct
    except ImportError:
        ct = None

if ct is not None:
    class Pointer(ct._SimpleCData): _type_ = 'P'
    class NoteID(ct._SimpleCData): _type_ = 'Q'
    class Integer(ct._SimpleCData): _type_ = 'i'
    class Double(ct._SimpleCData): _type_ = 'd'
    class Bool(ct._SimpleCData): _type_ = '?'
    class String(ct._SimpleCData): _type_ = 'z'
    class Values(ct.Array):
        _type_ = Double
        _length_ = 10

FIELDS = ('enabled', 'action_a', 'action_b', 'chance_a', 'chance_b', 'jump_a', 'jump_b', 'time', 'linked', 'loop_count')

def checked(error):
    if error: raise RuntimeError(error.decode('utf-8', 'replace'))

def undo_call(song, operation, *args):
    # Native scopes capture model requests; the Python boundary prevents Live
    # coalescing consecutive Max calls into a single Song-owned undo entry.
    song.begin_undo_step()
    try:
        checked(operation(*args))
    finally:
        song.end_undo_step()


def bind(handle, name, args, result=String if ct is not None else None):
    class Function(ct.CFuncPtr):
        _flags_ = ct.FUNCFLAG_CDECL | ct.FUNCFLAG_PYTHONAPI
        _argtypes_ = tuple(args)
        _restype_ = result
    return Function(ct.dlsym(handle, name))

def validate(field, value):
    if field not in FIELDS:
        raise ValueError('Unknown scene Follow Action field')
    if field in ('enabled', 'linked'):
        if type(value) is not bool and not (type(value) is int and value in (0, 1)):
            raise ValueError('enabled must be a boolean or 0/1')
        return
    if type(value) not in (int, float) or not math.isfinite(value):
        raise ValueError('Follow Action value must be a finite number')
    if field == 'time':
        if value < 0.25: raise ValueError('Follow Action time must be at least 0.25 quarter-note beats')
        return
    low, high = (1, 1073741823) if field == 'loop_count' else (0, 8388608) if field.startswith('jump') else (0, 100) if field.startswith('chance') else (0, 9)
    if not low <= value <= high or int(value) != value:
        raise ValueError('Follow Action value is outside its whole-number range')

def owner(obj, cls):
    if not isinstance(obj, cls) or not obj:
        raise ValueError('Invalid or deleted Live object')
    if cls is Live.Scene.Scene:
        parent = obj.canonical_parent
        if not isinstance(parent, Live.Song.Song) or obj not in parent.scenes:
            raise ValueError('Scene must belong to the current Song')
    return obj._live_ptr

class Native:
    def __init__(self, library, *, defer_enable=False):
        from WillingtonRuntime import verify
        self.profile_id = verify(library)['profile_id']
        self.snapshot_key = os.urandom(32)
        self._snapshot_owners = {}
        self.retired = False
        self.path = library
        self.handle = ct.dlopen(library, os.RTLD_NOW | os.RTLD_LOCAL)
        self._check = bind(self.handle, 'willington_editing_check', [])
        self._enable = bind(self.handle, 'willington_editing_enable', [Bool], None)
        self._scene_read = bind(self.handle, 'willington_scene_follow_read', [Pointer, Pointer])
        self._scene_set = bind(self.handle, 'willington_scene_follow_set', [Pointer, Pointer, Integer, Double])
        self._global_read = bind(self.handle, 'willington_global_follow_read', [Pointer, Pointer])
        self._global_set = bind(self.handle, 'willington_global_follow_set', [Pointer, Bool])
        self._group_tracks = bind(self.handle, 'willington_group_tracks', [Pointer, Pointer, Integer])
        self._ungroup_track = bind(self.handle, 'willington_ungroup_track', [Pointer, Pointer])
        self._expression_read = bind(self.handle, 'willington_expression_read', [Pointer, NoteID, Integer, Pointer, Integer, Pointer, Pointer])
        self._expression_replace = bind(self.handle, 'willington_expression_replace', [Pointer, Pointer, NoteID, Integer, Pointer, Integer, Bool])
        self._arrangement_read = bind(self.handle, 'willington_arrangement_read', [Pointer, Pointer, Double, Double, Pointer, Integer, Pointer, Pointer])
        self._arrangement_insert = bind(self.handle, 'willington_arrangement_insert', [Pointer, Pointer, Pointer, Pointer])
        self._arrangement_delete = bind(self.handle, 'willington_arrangement_delete', [Pointer, Pointer, Pointer, Double, Double])
        self._arrangement_snapshot = bind(self.handle, 'willington_arrangement_snapshot', [Pointer, Pointer, Pointer, Integer, Pointer, Pointer])
        self._arrangement_restore = bind(self.handle, 'willington_arrangement_restore', [Pointer, Pointer, Pointer, Pointer, Integer, Bool])
        checked(self._check())
        self.patches = []
        self.additions = []
        self.writes_enabled = False
        if not defer_enable: self.enable(False)

    def _assert_active(self):
        if getattr(self, 'retired', False): raise RuntimeError('Native editing adapter has been retired')

    def enable(self, value):
        self._assert_active()
        if type(value) is not bool: raise ValueError('enable requires a boolean')
        self._enable(value)
        self.writes_enabled = value

    def scene_read(self, scene):
        self._assert_active()
        values = Values()
        checked(self._scene_read(owner(scene, Live.Scene.Scene), ct.addressof(values)))
        result = dict(zip(FIELDS, values))
        result['enabled'] = bool(result['enabled'])
        result['linked'] = bool(result['linked'])
        return result

    def scene_set(self, scene, field, value):
        self._assert_active()
        validate(field, value)
        pointer = owner(scene, Live.Scene.Scene)
        if scene.canonical_parent.is_playing:
            raise RuntimeError('Stop transport before editing candidate Follow Actions')
        undo_call(scene.canonical_parent, self._scene_set, scene.canonical_parent._live_ptr, pointer, FIELDS.index(field), value)

    def global_read(self, song):
        self._assert_active()
        value = Bool()
        checked(self._global_read(owner(song, Live.Song.Song), ct.addressof(value)))
        return value.value

    def global_set(self, song, value):
        self._assert_active()
        validate('enabled', value)
        pointer = owner(song, Live.Song.Song)
        if song.is_playing:
            raise RuntimeError('Stop transport before editing candidate Follow Actions')
        checked(self._global_set(pointer, value))

    def group_tracks(self, song, tracks):
        self._assert_active()
        owner(song, Live.Song.Song)
        if song.is_playing:
            raise RuntimeError('Stop transport before grouping candidate tracks')
        before = tuple(song.tracks)
        tracks = tuple(tracks)
        if not 1 <= len(tracks) <= 128:
            raise ValueError('Group requires 1..128 tracks')
        indices = []
        for track in tracks:
            owner(track, Live.Track.Track)
            if track not in before or track.is_grouped or track.is_foldable:
                raise ValueError('Candidate grouping requires top-level audio/MIDI tracks')
            indices.append(before.index(track))
        if indices != list(range(indices[0], indices[0] + len(indices))):
            raise ValueError('Group tracks must be distinct, contiguous, and in Song order')
        class Pointers(ct.Array):
            _type_ = Pointer
            _length_ = len(tracks)
        pointers = Pointers(*(track._live_ptr for track in tracks))
        undo_call(song, self._group_tracks, song._live_ptr, ct.addressof(pointers), len(tracks))
        after = tuple(song.tracks)
        created = [track for track in after if track not in before]
        if len(created) != 1 or len(after) != len(before) + 1:
            raise RuntimeError('Group creation topology readback failed; inspect Live undo')
        group = created[0]
        if not group.is_foldable or any(track.group_track != group for track in tracks):
            raise RuntimeError('Group membership readback failed; inspect Live undo')
        if tuple(track for track in after if track != group) != before:
            raise RuntimeError('Group creation changed track order; inspect Live undo')
        return group

    def ungroup_track(self, song, group):
        self._assert_active()
        owner(song, Live.Song.Song)
        owner(group, Live.Track.Track)
        before = tuple(song.tracks)
        if song.is_playing or group not in before or not group.is_foldable or group.is_grouped:
            raise ValueError('Candidate ungroup requires a stopped top-level group')
        if tuple(group.devices):
            raise ValueError('Candidate ungroup refuses groups containing devices')
        members = tuple(track for track in before if track.is_grouped and track.group_track == group)
        if not members or any(track.is_foldable for track in members):
            raise ValueError('Candidate ungroup requires audio/MIDI members without nested groups')
        expected = tuple(track for track in before if track != group)
        undo_call(song, self._ungroup_track, song._live_ptr, group._live_ptr)
        if tuple(song.tracks) != expected or any(track.is_grouped for track in members):
            raise RuntimeError('Ungroup topology readback failed; inspect Live undo')

    def expression_owner(self, clip, note_id, dimension):
        self._assert_active()
        owner(clip, Live.Clip.Clip)
        if not clip.is_midi_clip:
            raise ValueError('Per-note expressions require a MIDI clip')
        if type(note_id) is not int or not 0 < note_id < 2**64:
            raise ValueError('note_id must be a positive integer')
        if dimension not in ('pitch', 'slide', 'pressure'):
            raise ValueError('Unknown expression dimension')
        notes = tuple(clip.get_notes_by_id((note_id,)))
        if len(notes) != 1:
            raise ValueError('Note ID is absent or stale')
        return {'pitch': -2, 'slide': 74, 'pressure': -1}[dimension], notes[0]

    def expression_read(self, clip, note_id, dimension):
        dim, note = self.expression_owner(clip, note_id, dimension)
        count, exists = Integer(), Bool()
        checked(self._expression_read(clip._live_ptr, note_id, dim, None, 0, ct.addressof(count), ct.addressof(exists)))
        class Rows(ct.Array):
            _type_ = Double
            _length_ = count.value * 6
        rows = Rows()
        checked(self._expression_read(clip._live_ptr, note_id, dim, ct.addressof(rows), count.value, ct.addressof(count), ct.addressof(exists)))
        return {'exists': exists.value, 'dimension': dimension,
                'unit': 'cents' if dimension == 'pitch' else 'midi',
                'time_origin': 'note_start', 'events': [list(rows[i:i+6]) for i in range(0,len(rows),6)]}

    def expression_replace(self, clip, note_id, dimension, state):
        dim, note = self.expression_owner(clip, note_id, dimension)
        if type(state) is not dict or type(state.get('exists')) is not bool or type(state.get('events')) is not list:
            raise ValueError('Expression state requires exists and events')
        events = state['events']
        if len(events) > 65536:
            raise ValueError('Too many expression events')
        for row in events:
            if type(row) not in (list, tuple) or len(row) != 6 or any(type(v) not in (int,float) or not math.isfinite(v) for v in row):
                raise ValueError('Each expression event requires six finite numbers')
        parent = clip.canonical_parent
        while parent and not isinstance(parent, Live.Song.Song):
            parent = parent.canonical_parent
        if not parent or parent.is_playing:
            raise RuntimeError('Stop transport before editing candidate expressions')
        class Rows(ct.Array):
            _type_ = Double
            _length_ = len(events)*6
        rows = Rows(*(v for row in events for v in row))
        undo_call(parent, self._expression_replace, parent._live_ptr, clip._live_ptr, note_id, dim, ct.addressof(rows), len(events), state['exists'])

    def arrangement_owner(self, track, parameter, writing=False):
        self._assert_active()
        owner(track, Live.Track.Track)
        owner(parameter, Live.DeviceParameter.DeviceParameter)
        song = track.canonical_parent
        if not isinstance(song, Live.Song.Song) or track not in tuple(song.tracks) + tuple(song.return_tracks) + (song.master_track,):
            raise ValueError('Track must belong to the current Song')
        if parameter.is_quantized:
            raise ValueError('Candidate Arrangement editing supports continuous parameters')
        if writing and song.is_playing:
            raise RuntimeError('Stop transport before editing candidate Arrangement automation')
        return song

    def arrangement_read(self, track, parameter, start, end):
        self.arrangement_owner(track, parameter)
        if any(type(v) not in (int,float) or not math.isfinite(v) for v in (start,end)) or not 0 <= start <= end <= 1576800:
            raise ValueError('Invalid Arrangement interval')
        count, exists = Integer(), Bool()
        checked(self._arrangement_read(track._live_ptr, parameter._live_ptr, start, end, None, 0, ct.addressof(count), ct.addressof(exists)))
        class Rows(ct.Array):
            _type_ = Double
            _length_ = count.value*6
        rows = Rows()
        checked(self._arrangement_read(track._live_ptr, parameter._live_ptr, start, end, ct.addressof(rows), count.value, ct.addressof(count), ct.addressof(exists)))
        return {'exists': exists.value, 'start': start, 'end': end, 'time_origin': 'song_start', 'value_domain': 'parameter',
                'events': [list(rows[i:i+6]) for i in range(0,len(rows),6)]}

    def arrangement_insert(self, track, parameter, event):
        song = self.arrangement_owner(track, parameter, True)
        if type(event) not in (list,tuple) or len(event)!=6 or any(type(v) not in (int,float) or not math.isfinite(v) for v in event):
            raise ValueError('Event requires six finite numbers')
        if not parameter.min <= event[1] <= parameter.max:
            raise ValueError('Event value is outside the parameter bounds')
        class Row(ct.Array):
            _type_ = Double
            _length_ = 6
        row = Row(*event)
        undo_call(song, self._arrangement_insert, song._live_ptr,track._live_ptr,parameter._live_ptr,ct.addressof(row))

    def arrangement_delete(self, track, parameter, start, end):
        song = self.arrangement_owner(track, parameter, True)
        if any(type(v) not in (int,float) or not math.isfinite(v) for v in (start,end)) or not 0 <= start <= end <= 1576800:
            raise ValueError('Invalid Arrangement interval')
        undo_call(song, self._arrangement_delete, song._live_ptr,track._live_ptr,parameter._live_ptr,start,end)

    @staticmethod
    def _snapshot_same_owner(entry, song, track, parameter):
        try:
            return (bool(entry['song']) and bool(entry['track']) and bool(entry['parameter'])
                    and entry['song'] == song and entry['track'] == track and entry['parameter'] == parameter)
        except Exception:
            return False

    def _snapshot_binding(self, song, track, parameter):
        key = str(parameter._live_ptr)
        entry = self._snapshot_owners.get(key)
        if entry is None or not self._snapshot_same_owner(entry, song, track, parameter):
            if key not in self._snapshot_owners and len(self._snapshot_owners) >= 128:
                del self._snapshot_owners[next(iter(self._snapshot_owners))]
            entry = dict(song=song, track=track, parameter=parameter, token=os.urandom(16).hex())
            self._snapshot_owners[key] = entry
        return entry['token']

    def arrangement_snapshot(self, track, parameter):
        song = self.arrangement_owner(track, parameter)
        count, exists = Integer(), Bool()
        checked(self._arrangement_snapshot(track._live_ptr, parameter._live_ptr, None, 0, ct.addressof(count), ct.addressof(exists)))
        class Rows(ct.Array):
            _type_ = Double
            _length_ = count.value*6
        rows = Rows()
        checked(self._arrangement_snapshot(track._live_ptr, parameter._live_ptr, ct.addressof(rows), count.value, ct.addressof(count), ct.addressof(exists)))
        state = {'schema': 1, 'profile': self.profile_id,
                'parameter_handle': str(parameter._live_ptr), 'value_domain': 'native',
                'owner_token': self._snapshot_binding(song, track, parameter),
                'exists': exists.value, 'events': [list(rows[i:i+6]) for i in range(0,len(rows),6)]}
        state['signature'] = self.snapshot_signature(state)
        return state

    def snapshot_signature(self, state):
        payload = {k:v for k,v in state.items() if k != 'signature'}
        # Max JSON.stringify emits 16 for Python's 16.0. Canonicalize numeric
        # event fields so an unchanged JSON roundtrip retains its signature.
        payload['events'] = [[0.0 if float(v) == 0 else float(v) for v in row] for row in state['events']]
        return hmac.new(self.snapshot_key, json.dumps(payload,sort_keys=True,separators=(',', ':'),allow_nan=False).encode(), hashlib.sha256).hexdigest()

    def arrangement_restore(self, track, parameter, state):
        song = self.arrangement_owner(track, parameter, True)
        if (type(state) is not dict or type(state.get('schema')) is not int or state['schema'] != 1
                or state.get('profile') != self.profile_id
                or state.get('value_domain') != 'native'
                or state.get('parameter_handle') != str(parameter._live_ptr)
                or type(state.get('exists')) is not bool or type(state.get('events')) is not list):
            raise ValueError('Snapshot must originate from this parameter in this Live session and exact profile')
        entry = self._snapshot_owners.get(str(parameter._live_ptr))
        if (entry is None or state.get('owner_token') != entry['token']
                or not self._snapshot_same_owner(entry, song, track, parameter)):
            raise ValueError('Snapshot owner was deleted, replaced, or evicted from this adapter')
        events = state['events']
        if len(events) > 65536 or (not state['exists'] and events) or (state['exists'] and not events):
            raise ValueError('Invalid snapshot event count')
        for row in events:
            if type(row) not in (list,tuple) or len(row)!=6 or any(type(v) not in (int,float) or not math.isfinite(v) for v in row):
                raise ValueError('Snapshot events require six finite numbers')
        if (type(state.get('signature')) is not str
                or not hmac.compare_digest(state['signature'], self.snapshot_signature(state))):
            raise ValueError('Snapshot was changed or belongs to a different adapter session')
        class Rows(ct.Array):
            _type_ = Double
            _length_ = len(events)*6
        rows = Rows(*(v for row in events for v in row))
        undo_call(song, self._arrangement_restore, song._live_ptr, track._live_ptr, parameter._live_ptr,
                  ct.addressof(rows), len(events), state['exists'])
        actual = self.arrangement_snapshot(track, parameter)
        if actual['exists'] != state['exists'] or actual['events'] != events:
            raise RuntimeError('Snapshot restoration readback differs; inspect Live undo')

    def uninstall(self):
        if getattr(self, 'retired', False): return
        self.enable(False)
        from _MxDCore import LomTypes
        for cls, name, method in reversed(self.patches):
            if getattr(cls, name, None) is method: delattr(cls, name)
        for cls, added in self.additions:
            LomTypes.AVAILABLE_TYPE_PROPERTIES[cls] = tuple(
                prop for prop in LomTypes.AVAILABLE_TYPE_PROPERTIES[cls] if prop is not added)
        self.patches.clear()
        self.additions.clear()
        self.retired = True
        getattr(self, '_snapshot_owners', {}).clear()


def install(library=None):
    if library is None:
        from WillingtonRuntime import resolve
        library, _manifest = resolve('WillingtonEditing', os.path.dirname(__file__))
    if ct is None:
        raise RuntimeError('Native editing requires the macOS ctypes runtime')
    from _MxDCore import LomTypes
    # Verification/loading must not alter the current shared-library write flag.
    native = Native(library, defer_enable=True)
    previous = [(old, list(old.patches), list(old.additions), old.writes_enabled)
                for old in getattr(Live, '_willington_editing_libraries', ())
                if not getattr(old, 'retired', False) and (old.patches or old.additions)]
    previous_owners = {id(old): dict(getattr(old, '_snapshot_owners', {})) for old, _, _, _ in previous}
    owned = {(cls, name): method for _, patches, _, _ in previous for cls, name, method in patches}
    owned_properties = [prop for _, _, additions, _ in previous for _, prop in additions]
    def get_follow_actions(scene):
        return json.dumps(native.scene_read(scene), sort_keys=True, separators=(',', ':'))
    def set_follow_action(scene, field, value):
        native.scene_set(scene, field, value)
    def get_follow_actions_enabled(song):
        return native.global_read(song)
    def set_follow_actions_enabled(song, value):
        native.global_set(song, value)
    def group_tracks(song, *tracks):
        return native.group_tracks(song, tracks)
    def ungroup_track(song, group):
        return native.ungroup_track(song, group)
    def get_note_expression(clip, note_id, dimension):
        return json.dumps(native.expression_read(clip, note_id, dimension), sort_keys=True, separators=(',', ':'))
    def replace_note_expression(clip, note_id, dimension, state_json):
        native.expression_replace(clip, note_id, dimension, json.loads(state_json))
    def get_arrangement_automation(track, parameter, start, end):
        return json.dumps(native.arrangement_read(track, parameter, start, end), sort_keys=True, separators=(',', ':'))
    def insert_arrangement_event(track, parameter, event_json):
        native.arrangement_insert(track, parameter, json.loads(event_json))
    def delete_arrangement_events(track, parameter, start, end):
        native.arrangement_delete(track, parameter, start, end)
    def get_arrangement_snapshot(track, parameter):
        return json.dumps(native.arrangement_snapshot(track, parameter), sort_keys=True, separators=(',', ':'))
    def restore_arrangement_snapshot(track, parameter, state_json):
        native.arrangement_restore(track, parameter, json.loads(state_json))
    patches = [(Live.Track.Track, 'get_arrangement_snapshot', get_arrangement_snapshot),
               (Live.Track.Track, 'restore_arrangement_snapshot', restore_arrangement_snapshot),
               (Live.Track.Track, 'get_arrangement_automation', get_arrangement_automation),
               (Live.Track.Track, 'insert_arrangement_event', insert_arrangement_event),
               (Live.Track.Track, 'delete_arrangement_events', delete_arrangement_events),
               (Live.Clip.Clip, 'get_note_expression', get_note_expression),
               (Live.Clip.Clip, 'replace_note_expression', replace_note_expression),
               (Live.Song.Song, 'group_tracks', group_tracks),
               (Live.Song.Song, 'ungroup_track', ungroup_track),
               (Live.Scene.Scene, 'get_follow_actions', get_follow_actions),
               (Live.Scene.Scene, 'set_follow_action', set_follow_action),
               (Live.Song.Song, 'get_follow_actions_enabled', get_follow_actions_enabled),
               (Live.Song.Song, 'set_follow_actions_enabled', set_follow_actions_enabled)]
    # Preflight collisions before retiring a working installation.
    for cls, name, method in patches:
        if cls not in LomTypes.AVAILABLE_TYPE_PROPERTIES:
            raise RuntimeError('Missing Max class: ' + name)
        if hasattr(cls, name) and getattr(cls, name) is not owned.get((cls, name)):
            raise RuntimeError('Existing API collision: ' + name)
        if any(prop.name == name and all(prop is not own for own in owned_properties)
               for prop in LomTypes.AVAILABLE_TYPE_PROPERTIES[cls]):
            raise RuntimeError('Existing Max API collision: ' + name)
    original_properties = {cls: LomTypes.AVAILABLE_TYPE_PROPERTIES[cls] for cls, _, _ in patches}
    try:
        for old, _, _, _ in previous: old.uninstall()
        native.enable(False)
        for cls, name, method in patches:
            setattr(cls, name, method)
            native.patches.append((cls, name, method))
            prop = LomTypes.MFLProperty(name)
            LomTypes.AVAILABLE_TYPE_PROPERTIES[cls] = tuple(LomTypes.AVAILABLE_TYPE_PROPERTIES[cls]) + (prop,)
            native.additions.append((cls, prop))
    except Exception:
        try:
            native.uninstall()
        finally:
            for cls, properties in original_properties.items():
                LomTypes.AVAILABLE_TYPE_PROPERTIES[cls] = properties
            for old, old_patches, old_additions, enabled in previous:
                old.retired = False
                old._snapshot_owners = previous_owners[id(old)]
                old.patches[:] = old_patches
                old.additions[:] = old_additions
                for cls, name, method in old_patches: setattr(cls, name, method)
                old.enable(enabled)
        raise
    if not hasattr(Live, '_willington_editing_libraries'):
        Live._willington_editing_libraries = []
    Live._willington_editing_libraries.append(native)
    return native
