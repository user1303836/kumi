"""Exact-build rack zone read/write methods for Python and Max for Live."""
import json
import os
import sys
import Live
try:
    import _ctypes as ct
except ImportError:
    try:
        import ctypes as ct
    except ImportError:
        ct = None  # Windows Live has no ctypes; WindowsNative uses an extension module.

if ct is not None:
    class Pointer(ct._SimpleCData): _type_ = 'P'
    class Integer(ct._SimpleCData): _type_ = 'i'
    class String(ct._SimpleCData): _type_ = 'z'
    class Bool(ct._SimpleCData): _type_ = '?'
    class Values(ct.Array):
        _type_ = Integer
        _length_ = 6

    def bind(handle, name, args, result=String):
        class Function(ct.CFuncPtr):
            _flags_ = ct.FUNCFLAG_CDECL | ct.FUNCFLAG_PYTHONAPI
            _argtypes_ = tuple(args)
            _restype_ = result
        return Function(ct.dlsym(handle, name))

KINDS = {'selector': 0, 'key': 1, 'velocity': 2}
FIELDS = ('minimum', 'maximum', 'fadeMinimum', 'fadeMaximum')

def checked(error):
    if error:
        raise RuntimeError(error.decode('utf-8', 'replace'))

def target(chain, kind):
    if not chain:
        raise ValueError('Live chain has been deleted')
    if not isinstance(kind, str) or kind not in KINDS:
        raise ValueError('Zone kind must be selector, key, or velocity')
    rack = chain.canonical_parent
    allowed = ('AudioEffectGroupDevice', 'InstrumentGroupDevice', 'MidiEffectGroupDevice')
    if not rack or getattr(rack, 'class_name', None) not in allowed or chain not in rack.chains:
        raise ValueError('Rack zones require a regular Audio, Instrument, or MIDI Effect Rack chain')
    if kind != 'selector' and rack.class_name == 'AudioEffectGroupDevice':
        raise ValueError('Audio Effect Racks only have selector zones')
    return chain._live_ptr, KINDS[kind]

class Native:
    def __init__(self, library):
        from WillingtonRuntime import verify
        verify(library)
        self.path = library
        self.handle = ct.dlopen(library, os.RTLD_NOW | os.RTLD_LOCAL)
        self.check = bind(self.handle, 'willington_zone_check', [])
        self._enable = bind(self.handle, 'willington_zone_enable', [Bool], None)
        self.read = bind(self.handle, 'willington_zone_read', [Pointer, Integer, Pointer])
        self.write = bind(self.handle, 'willington_zone_set', [Pointer, Integer, Pointer])
        checked(self.check())
        self.enable(False)
        self.patches = []
        self.additions = ()

    def enable(self, enabled):
        self._enable(bool(enabled))
        self.writes_enabled = bool(enabled)

    def read_zone(self, pointer, index):
        values = Values()
        checked(self.read(pointer, index, ct.addressof(values)))
        return list(values)

    def write_zone(self, pointer, index, endpoints):
        values = Values(*endpoints, 0, 0)
        checked(self.write(pointer, index, ct.addressof(values)))

    def uninstall(self):
        self.enable(False)
        from _MxDCore import LomTypes
        cls = Live.Chain.Chain
        for name, method in reversed(self.patches):
            if getattr(cls, name, None) is method:
                delattr(cls, name)
        props = LomTypes.AVAILABLE_TYPE_PROPERTIES[cls]
        LomTypes.AVAILABLE_TYPE_PROPERTIES[cls] = tuple(
            prop for prop in props if not any(prop is added for added in self.additions))

class WindowsNative(Native):
    """Same contract, backed by the verified willington_zones extension module."""
    def __init__(self, library):
        from WillingtonRuntime import verify, load_extension
        verify(library)
        self.path = library
        self.module = load_extension(library)
        self.module.check()
        self.enable(False)
        self.patches = []
        self.additions = ()

    def enable(self, enabled):
        self.module.enable(bool(enabled))
        self.writes_enabled = bool(enabled)

    def read_zone(self, pointer, index):
        return list(self.module.read(pointer, index))

    def write_zone(self, pointer, index, endpoints):
        self.module.set(pointer, index, tuple(endpoints))

def install(library=None):
    """Install Chain.get_zone/set_zone with writes disabled; retain the handle."""
    if library is None:
        from WillingtonRuntime import resolve
        library, _manifest = resolve('WillingtonRackZones', os.path.dirname(__file__))
    for old in getattr(Live, '_willington_zone_libraries', ()):
        old.uninstall()
    cls = Live.Chain.Chain
    from _MxDCore import LomTypes
    if cls not in LomTypes.AVAILABLE_TYPE_PROPERTIES:
        raise RuntimeError('Expected Max Chain registration is missing')
    for name in ('get_zone', 'set_zone'):
        if hasattr(cls, name):
            raise RuntimeError('Refusing to replace existing API: ' + name)
    native = (WindowsNative if sys.platform == 'win32' else Native)(library)

    def get_zone(self, kind):
        """Return JSON containing all four endpoints and the zone's bounds."""
        pointer, index = target(self, kind)
        values = native.read_zone(pointer, index)
        state = dict(zip(FIELDS, values[:4]))
        state.update(lowerBound=values[4], upperBound=values[5])
        return json.dumps(state, sort_keys=True, separators=(',', ':'))

    def set_zone(self, kind, minimum, maximum, fade_minimum, fade_maximum):
        """Set a complete integer zone state, including both fade endpoints."""
        pointer, index = target(self, kind)
        endpoints = (minimum, maximum, fade_minimum, fade_maximum)
        if any(type(value) is not int for value in endpoints):
            raise ValueError('Zone endpoints must be integers')
        lower = 1 if kind == 'velocity' else 0
        if not lower <= minimum <= fade_minimum <= fade_maximum <= maximum <= 127:
            raise ValueError('Zone endpoints must be ordered and within the zone bounds')
        native.write_zone(pointer, index, endpoints)

    try:
        for name, method in (('get_zone', get_zone), ('set_zone', set_zone)):
            setattr(cls, name, method)
            native.patches.append((name, method))
        known = {prop.name for prop in LomTypes.AVAILABLE_TYPE_PROPERTIES[cls]}
        native.additions = tuple(LomTypes.MFLProperty(name) for name in ('get_zone', 'set_zone') if name not in known)
        LomTypes.AVAILABLE_TYPE_PROPERTIES[cls] = tuple(LomTypes.AVAILABLE_TYPE_PROPERTIES[cls]) + native.additions
    except Exception:
        native.uninstall()
        raise
    if not hasattr(Live, '_willington_zone_libraries'):
        Live._willington_zone_libraries = []
    Live._willington_zone_libraries.append(native)
    return native
