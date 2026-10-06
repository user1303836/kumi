"""Experimental, exact-build native Clip Follow Action binding.

Select this Control Surface with no MIDI ports. Commands are local JSON files,
limited to the built-in disposable-fixture test and enabling/disabling writes.
"""
try:
    import _ctypes
except ImportError:
    try:
        import ctypes as _ctypes  # Live registers the native module under this name.
    except ImportError:
        _ctypes = None  # Windows Live has no ctypes; WindowsNativeLibrary uses an extension module.
import hashlib
import json
import os
import sys
import traceback
import Live
from _Framework.ControlSurface import ControlSurface

HERE = os.path.dirname(__file__)
PROPERTIES = ('enabled', 'linked', 'a', 'b', 'loop_count', 'chance_a', 'chance_b',
              'jump_a', 'jump_b', 'time')
PROPERTIES = tuple('follow_action_' + name for name in PROPERTIES)



if _ctypes is not None:
    class _Object(_ctypes._SimpleCData):
        _type_ = 'O'


    class _String(_ctypes._SimpleCData):
        _type_ = 'z'


    class _Bool(_ctypes._SimpleCData):
        _type_ = '?'


    class _Register(_ctypes.CFuncPtr):
        _flags_ = _ctypes.FUNCFLAG_CDECL | _ctypes.FUNCFLAG_PYTHONAPI  # Hold the GIL and check Python errors.
        _argtypes_ = (_Object,)
        _restype_ = _String


    class _Enable(_ctypes.CFuncPtr):
        _flags_ = _ctypes.FUNCFLAG_CDECL | _ctypes.FUNCFLAG_PYTHONAPI
        _argtypes_ = (_Bool,)
        _restype_ = None


    class NativeLibrary:
        def __init__(self, path):
            self.path = path
            # Live's frozen importer exposes raw _ctypes under both module names.
            # Use its ABI primitives without depending on stdlib ctypes wrappers.
            self.handle = _ctypes.dlopen(path, os.RTLD_NOW | os.RTLD_LOCAL)
            self.willington_register_clip = _Register(_ctypes.dlsym(self.handle, 'willington_register_clip'))
            self.willington_enable_writes = _Enable(_ctypes.dlsym(self.handle, 'willington_enable_writes'))


class WindowsNativeLibrary:
    """Same interface, backed by the verified willington_bindings extension module."""
    def __init__(self, path):
        from WillingtonRuntime import load_extension
        self.path = path
        self.module = load_extension(path)
        self.module.check()

    def willington_register_clip(self, cls):
        try:
            self.module.register(cls)
        except RuntimeError as error:
            return str(error).encode('utf-8')
        return None

    def willington_enable_writes(self, enabled):
        self.module.enable(bool(enabled))


def write_report(name, value):
    path = os.path.join(HERE, name)
    with open(path + '.tmp', 'w') as out:
        json.dump(value, out, indent=2)
    os.replace(path + '.tmp', path)


def install():
    from WillingtonRuntime import resolve
    path, _manifest = resolve('WillingtonBindings', HERE)
    if not hasattr(Live, '_willington_native_library'):
        library = (WindowsNativeLibrary if sys.platform == 'win32' else NativeLibrary)(path)
        # Keep the code mapped for as long as Clip's property refers to it.
        Live._willington_native_library = library
    library = Live._willington_native_library
    if library.path != path:
        raise RuntimeError('A different Follow Action library is mapped; restart Live')
    error = library.willington_register_clip(Live.Clip.Clip)
    if error:
        raise RuntimeError(error.decode('utf-8', 'replace'))
    from _MxDCore import LomTypes
    props = LomTypes.AVAILABLE_TYPE_PROPERTIES[Live.Clip.Clip]
    names = {p.name for p in props}
    LomTypes.AVAILABLE_TYPE_PROPERTIES[Live.Clip.Clip] = tuple(props) + tuple(
        LomTypes.MFLProperty(name) for name in PROPERTIES if name not in names)
    return library


def create_instance(c_instance):
    return WillingtonBindings(c_instance)


class WillingtonBindings(ControlSurface):
    def __init__(self, c_instance):
        super(WillingtonBindings, self).__init__(c_instance)
        self._library = None
        self._testing = False
        self.schedule_message(1, self._start)

    def _start(self):
        try:
            self._library = install()
            write_report('status.json', {
                'status': 'registered', 'python': sys.version,
                'properties': [n for n in dir(Live.Clip.Clip) if 'follow_action' in n],
                'writes_enabled': False,
            })
            self._library.willington_enable_writes(False)
        except Exception:
            write_report('status.json', {'status': 'error', 'error': traceback.format_exc()})
        if self._library:
            self.schedule_message(5, self._poll)

    def _poll(self):
        command_path = os.path.join(HERE, 'command.json')
        try:
            if os.path.exists(command_path):
                with open(command_path) as stream:
                    command = json.load(stream)
                os.replace(command_path, command_path + '.processed')
                action = command.get('action')
                if action == 'self_test' and not self._testing:
                    self._test_create()
                elif action == 'enable_writes':
                    with open(os.path.join(HERE, 'self-test.json')) as stream:
                        result = json.load(stream)
                    with open(self._library.path, 'rb') as stream:
                        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
                    if result.get('status') != 'passed' or result.get('library_sha256') != digest:
                        raise RuntimeError('Self-test must pass before enabling writes')
                    self._library.willington_enable_writes(True)
                    write_report('status.json', {'status': 'ready', 'writes_enabled': True})
                elif action == 'disable_writes':
                    self._library.willington_enable_writes(False)
                else:
                    raise ValueError('Unknown command or test already running')
        except Exception:
            write_report('command-error.json', {'error': traceback.format_exc()})
        self.schedule_message(5, self._poll)

    def _snapshot(self):
        return {name: getattr(self._clip, name) for name in PROPERTIES}

    def _test_create(self):
        self._testing = True
        self._listeners = {}
        self._result = {'status': 'running', 'checks': {}, 'cases': [], 'events': {}}
        self._library.willington_enable_writes(False)
        try:
            song = self.song()
            tracks = [t for t in song.tracks if t.name == 'Willington Follow Action Test']
            if len(tracks) > 1:
                raise RuntimeError('Ambiguous fixture tracks')
            if tracks:
                self._test_track = tracks[0]
                self._clip = self._test_track.clip_slots[0].clip
                if not self._clip or self._clip.name != 'Willington native binding fixture':
                    raise RuntimeError('Fixture clip does not match')
            else:
                song.create_midi_track(-1)
                self._test_track = song.tracks[-1]
                self._test_track.name = 'Willington Follow Action Test'
                self._test_track.clip_slots[0].create_clip(4.0)
                self._clip = self._test_track.clip_slots[0].clip
                self._clip.name = 'Willington native binding fixture'
            self._result['initial'] = self._snapshot()
            for name in PROPERTIES:
                def changed(name=name):
                    self._result['events'][name].append(getattr(self._clip, name))
                self._result['events'][name] = []
                self._listeners[name] = changed
                getattr(self._clip, 'add_' + name + '_listener')(changed)
                assert getattr(self._clip, name + '_has_listener')(changed)
            self._result['checks']['listeners_registered'] = True
            self._cases = [('enabled', not self._clip.follow_action_enabled),
                           ('linked', not self._clip.follow_action_linked),
                           ('loop_count', 3), ('chance_a', 37.0), ('chance_b', 23.0),
                           ('jump_a', 2.0), ('jump_b', 3.0), ('time', 7.5)]
            self._cases += [(side, action) for side in ('a', 'b') for action in range(10)
                            if action != getattr(self._clip, 'follow_action_' + side)]
            self._case_index = 0
            write_report('self-test.json', self._result)
            self.schedule_message(2, self._test_write)
        except Exception:
            self._test_fail()

    def _test_write(self):
        try:
            suffix, value = self._cases[self._case_index]
            name = 'follow_action_' + suffix
            self._before = self._snapshot()
            self._result['events'] = {p: [] for p in PROPERTIES}
            self._library.willington_enable_writes(False)
            try:
                setattr(self._clip, name, value)
                raise AssertionError('Disabled write unexpectedly succeeded')
            except RuntimeError:
                assert self._snapshot() == self._before
            self._library.willington_enable_writes(True)
            self.song().begin_undo_step()
            try:
                setattr(self._clip, name, value)
            finally:
                self.song().end_undo_step()
            assert getattr(self._clip, name) == value, (name, getattr(self._clip, name), value)
            if suffix.startswith('chance_'):
                assert self._clip.follow_action_chance_a + self._clip.follow_action_chance_b == 100.0
            self._case = {'property': name, 'value': value, 'readback': True, 'disabled_write_rejected': True}
            self.schedule_message(2, self._test_undo)
        except Exception:
            self._test_fail()

    def _test_undo(self):
        try:
            name, value = self._case['property'], self._case['value']
            assert value in self._result['events'][name], (name, 'missing notification')
            self._case['write_notified'] = True
            self._case['after'] = self._snapshot()
            self.song().undo()
            self.schedule_message(2, self._test_next)
        except Exception:
            self._test_fail()

    def _test_next(self):
        try:
            assert self._snapshot() == self._before, ('undo mismatch', self._snapshot(), self._before)
            name = self._case['property']
            events = self._result['events'][name]
            assert len(events) >= 2 and events[-1] == self._before[name], (name, events)
            self._case.update(undo_restored_all=True, undo_notified=True,
                              events={p: list(v) for p, v in self._result['events'].items() if v})
            self._result['cases'].append(self._case)
            self._case_index += 1
            write_report('self-test.json', self._result)
            if self._case_index < len(self._cases):
                self.schedule_message(2, self._test_write)
            else:
                self._test_finish()
        except Exception:
            self._test_fail()

    def _cleanup_listeners(self):
        for name, callback in list(self._listeners.items()):
            if getattr(self._clip, name + '_has_listener')(callback):
                getattr(self._clip, 'remove_' + name + '_listener')(callback)
            assert not getattr(self._clip, name + '_has_listener')(callback)
            del self._listeners[name]

    def _test_finish(self):
        try:
            invalid = {'a': [-1, 10], 'b': [-1, 10], 'loop_count': [0, -1],
                       'chance_a': [-1.0, 101.0, 0.5, float('nan')],
                       'chance_b': [-1.0, 101.0], 'jump_a': [0.0, 1.5],
                       'jump_b': [-1.0, float('inf')], 'time': [0.0, float('nan'), float('inf')]}
            before = self._snapshot()
            for suffix, values in invalid.items():
                for value in values:
                    try:
                        setattr(self._clip, 'follow_action_' + suffix, value)
                    except (RuntimeError, ValueError):
                        pass
                    else:
                        raise AssertionError(('Invalid value accepted', suffix, value))
                    assert self._snapshot() == before
            self._result['checks']['invalid_values_rejected'] = True
            self._cleanup_listeners()
            self._result['checks']['listeners_removed'] = True
            from _MxDCore import LomTypes
            available = LomTypes.get_available_property_names_for_type(Live.Clip.Clip, (99, 99))
            assert all(name in available for name in PROPERTIES)
            self._result['checks']['mfl_allowlist'] = True
            self._result['status'] = 'passed'
            with open(self._library.path, 'rb') as stream:
                self._result['library_sha256'] = hashlib.file_digest(stream, 'sha256').hexdigest()
            self._library.willington_enable_writes(False)
            write_report('self-test.json', self._result)
            self._testing = False
        except Exception:
            self._test_fail()

    def _test_fail(self):
        error = traceback.format_exc()
        self._library.willington_enable_writes(False)
        try:
            self._cleanup_listeners()
        except Exception:
            error += '\nListener cleanup: ' + traceback.format_exc()
        self._result.update(status='failed', error=error)
        write_report('self-test.json', self._result)
        self._testing = False

    def disconnect(self):
        if self._library:
            self._library.willington_enable_writes(False)
        super(WillingtonBindings, self).disconnect()
