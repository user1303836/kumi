"""Exact-build device extensions, callable by Python Remote Scripts and Max."""
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

HERE = os.path.dirname(__file__)

if ct is not None:
    class Pointer(ct._SimpleCData):
        _type_ = 'P'
    class Integer(ct._SimpleCData):
        _type_ = 'i'
    class String(ct._SimpleCData):
        _type_ = 'z'
    class Float(ct._SimpleCData):
        _type_ = 'f'
    class Bool(ct._SimpleCData):
        _type_ = '?'

    def bind(handle, name, args, result=String):
        class Function(ct.CFuncPtr):
            _flags_ = ct.FUNCFLAG_CDECL | ct.FUNCFLAG_PYTHONAPI
            _argtypes_ = tuple(args)
            _restype_ = result
        return Function(ct.dlsym(handle, name))


class Native:
    def __init__(self, library):
        if ct is None:
            from WillingtonRuntime import ComponentUnavailableError
            raise ComponentUnavailableError('Willington Device Tools has no implementation for this Live (no ctypes)')
        manifest = os.path.join(os.path.dirname(os.path.abspath(library)), 'build.json')
        if not os.path.isfile(manifest):
            manifest = os.path.join(HERE, 'build.json')
        from WillingtonRuntime import verify
        verify(library, manifest)
        self.path = library
        self.handle = ct.dlopen(library, os.RTLD_NOW | os.RTLD_LOCAL)
        self.check = bind(self.handle, 'willington_device_check', [])
        self._enable = bind(self.handle, 'willington_device_enable', [Bool], None)
        self.rename_macro = bind(self.handle, 'willington_rename_macro', [Pointer, Integer, String])
        self.rename_variation = bind(self.handle, 'willington_rename_variation', [Pointer, String])
        self.replace_sample = bind(self.handle, 'willington_replace_drum_sample', [Pointer, String])
        self.overwrite_variation = bind(self.handle, 'willington_overwrite_variation', [Pointer])
        self.validate_modulation = bind(self.handle, 'willington_validate_modulation', [Pointer,Bool])
        self.max_object_name = bind(self.handle, 'willington_max_object_name', [Pointer,Integer,Pointer,Integer])
        self.map_macro = bind(self.handle, 'willington_map_macro', [Pointer,Integer,Pointer])
        self.unmap_macro = bind(self.handle, 'willington_unmap_macro', [Pointer,Pointer])
        self.macro_range = bind(self.handle, 'willington_macro_range', [Pointer,Pointer,Float,Float])
        self.macro_switch_range = bind(self.handle, 'willington_macro_switch_range', [Pointer,Pointer,Float,Float])
        self.mapping_read = bind(self.handle, 'willington_macro_mapping', [Pointer,Pointer,Pointer,Pointer,Pointer,Pointer])
        self.variation_name = bind(self.handle, 'willington_variation_name', [Pointer,Pointer,Integer])
        checked(self.check())
        self.enable(False)

    writes_enabled = False

    def enable(self, enabled):
        self._enable(bool(enabled))
        self.writes_enabled = bool(enabled)

    def object_name(self, device, object_id):
        class Byte(ct._SimpleCData):
            _type_ = 'c'
        class Buffer(ct.Array):
            _type_ = Byte
            _length_ = 2048
        buffer = Buffer()
        checked(self.max_object_name(live_pointer(device), object_id, ct.addressof(buffer), 2048))
        return buffer.value.decode('utf-8')

    def read_mapping(self, rack, parameter):
        """Return the native (index, minimum, maximum, kind) mapping readback."""
        index = Integer(); low = Float(); high = Float(); kind = Integer()
        checked(self.mapping_read(rack, parameter, ct.addressof(index), ct.addressof(low),
                                  ct.addressof(high), ct.addressof(kind)))
        return index.value, low.value, high.value, kind.value

    def read_variation_name(self, rack):
        class CodeUnit(ct._SimpleCData): _type_ = 'H'
        class Buffer(ct.Array):
            _type_ = CodeUnit
            _length_ = 1024
        buffer = Buffer()
        checked(self.variation_name(rack, ct.addressof(buffer), 1024))
        units = []
        for unit in buffer:
            if unit == 0: break
            units.append(unit)
        return b''.join(unit.to_bytes(2, 'little') for unit in units).decode('utf-16-le')

    def uninstall(self):
        """Disable writes and remove only this instance's Python/Max additions."""
        self.enable(False)
        from _MxDCore import LomTypes
        for cls, name, method in reversed(getattr(self, 'patches', ())):
            if getattr(cls, name, None) is method:
                delattr(cls, name)
        for cls, additions in getattr(self, 'properties', ()):
            props = LomTypes.AVAILABLE_TYPE_PROPERTIES[cls]
            LomTypes.AVAILABLE_TYPE_PROPERTIES[cls] = tuple(
                prop for prop in props if not any(prop is added for added in additions))


class WindowsNative(Native):
    """Same contract, backed by the verified willington_devices extension module.

    Module functions raise on failure and return None otherwise, so checked()
    passes; validate_modulation returns its refusal like the macOS entry point.
    """
    def __init__(self, library):
        from WillingtonRuntime import verify, load_extension
        verify(library)
        self.path = library
        self.module = load_extension(library)
        for name in ('check', 'rename_macro', 'rename_variation', 'overwrite_variation',
                     'validate_modulation', 'map_macro', 'unmap_macro', 'macro_range',
                     'macro_switch_range'):
            setattr(self, name, getattr(self.module, name))
        self.check()
        self.enable(False)

    def enable(self, enabled):
        self.module.enable(bool(enabled))
        self.writes_enabled = bool(enabled)

    def replace_sample(self, device, path):
        # Live on Windows takes UTF-16 paths; install() passes UTF-8 bytes.
        return self.module.replace_sample(device, name_bytes(path.decode('utf-8')))

    def object_name(self, device, object_id):
        return self.module.max_object_name(live_pointer(device), object_id)

    def read_mapping(self, rack, parameter):
        return self.module.macro_mapping(rack, parameter)

    def read_variation_name(self, rack):
        return self.module.variation_name(rack).decode('utf-16-le')


def checked(error):
    if error:
        raise RuntimeError(error.decode('utf-8', 'replace'))


def live_pointer(obj):
    if not obj:
        raise ValueError('Live object has been deleted')
    return obj._live_ptr


def name_bytes(name):
    if not isinstance(name, str) or '\0' in name:
        raise ValueError('Name must be text without NUL characters')
    return name.encode('utf-16-le') + b'\0\0'


def install(library=None):
    """Register the extension with writes disabled; return its lifecycle handle.

    Call from a Live main-thread Remote Script callback. Keep the returned
    handle, call enable(True) deliberately, and uninstall() on disconnect.
    """
    if library is None:
        from WillingtonRuntime import resolve
        library, _manifest = resolve('WillingtonDeviceTools', HERE)
    for old in getattr(Live, '_willington_device_libraries', ()):
        if hasattr(old, 'uninstall'):
            old.uninstall()
        else:
            old.enable(False)
    native = (WindowsNative if sys.platform == 'win32' else Native)(library)
    from _MxDCore import LomTypes
    # Refuse collisions before changing any class, including future native APIs.
    declarations = (
        (Live.RackDevice.RackDevice, ('rename_macro', 'rename_selected_variation',
          'overwrite_selected_variation', 'map_macro', 'unmap_macro', 'set_macro_mapping_range',
          'set_macro_switch_range', 'get_macro_mapping', 'get_selected_variation_name')),
        (Live.DrumCellDevice.DrumCellDevice, ('replace_sample',)),
        (Live.MaxDevice.MaxDevice, ('map_modulation', 'get_modulation_target')),
    )
    for cls, names in declarations:
        if cls not in LomTypes.AVAILABLE_TYPE_PROPERTIES:
            raise RuntimeError('Expected Max class registration is missing')
        for name in names:
            if hasattr(cls, name):
                raise RuntimeError('Refusing to replace existing API: ' + name)
    native.patches = []
    native.properties = []

    def patch(cls, name, method):
        setattr(cls, name, method)
        native.patches.append((cls, name, method))

    def rename_macro(self, index, name):
        """Rename a macro by zero-based index (0..15)."""
        if type(index) is not int or not 0 <= index < 16:
            raise ValueError('Macro index must be 0..15')
        checked(native.rename_macro(live_pointer(self), index, name_bytes(name)))
    def rename_selected_variation(self, name):
        """Rename the currently selected Macro Variation."""
        checked(native.rename_variation(live_pointer(self), name_bytes(name)))
    def overwrite_selected_variation(self):
        """Replace the selected variation with current mapped macro values."""
        checked(native.overwrite_variation(live_pointer(self)))
    def replace_sample(self, path):
        """Replace a Drum Sampler's sample with an existing audio file."""
        if not isinstance(path, str) or '\0' in path:
            raise ValueError('Sample path must be text without NUL characters')
        path = os.path.realpath(os.path.expanduser(path))
        if not os.path.isfile(path):
            raise ValueError('Sample file does not exist')
        checked(native.replace_sample(live_pointer(self), path.encode('utf-8')))
    cls = Live.RackDevice.RackDevice
    def map_macro(self, index, parameter):
        """Map a parameter belonging to this rack to macro index 0..15."""
        if type(index) is not int or not 0 <= index < 16:
            raise ValueError('Macro index must be 0..15')
        if not isinstance(parameter, Live.DeviceParameter.DeviceParameter):
            raise TypeError('Expected a Live DeviceParameter')
        checked(native.map_macro(live_pointer(self),index,live_pointer(parameter)))
    def unmap_macro(self, parameter):
        """Remove this rack's macro mapping from the parameter."""
        if not isinstance(parameter, Live.DeviceParameter.DeviceParameter):
            raise TypeError('Expected a Live DeviceParameter')
        checked(native.unmap_macro(live_pointer(self),live_pointer(parameter)))
    def set_macro_mapping_range(self, parameter, minimum, maximum):
        """Set continuous/enum endpoints in parameter units; inversion is allowed."""
        if not isinstance(parameter, Live.DeviceParameter.DeviceParameter):
            raise TypeError('Expected a Live DeviceParameter')
        checked(native.macro_range(live_pointer(self),live_pointer(parameter),float(minimum),float(maximum)))
    def set_macro_switch_range(self, parameter, minimum, maximum):
        """Set a boolean mapping's inclusive on-interval in macro units 0..127."""
        if not isinstance(parameter, Live.DeviceParameter.DeviceParameter):
            raise TypeError('Expected a Live DeviceParameter')
        checked(native.macro_switch_range(live_pointer(self),live_pointer(parameter),float(minimum),float(maximum)))
    from . import modulators
    def get_macro_mapping(self, parameter):
        """Return JSON mapping index/range/type, or null for an unmapped target."""
        index, low, high, kind = native.read_mapping(live_pointer(self), live_pointer(parameter))
        if index < 0: return 'null'
        return json.dumps({'index':index,'minimum':low,'maximum':high,
                           'kind':('continuous','enum','boolean')[kind]})
    def get_selected_variation_name(self):
        return native.read_variation_name(live_pointer(self))
    def map_modulation(self, slot, parameter):
        """Map a bundled modulator slot (zero-based); pass None to clear it."""
        modulators.map_parameter(native, self, slot, parameter)
    def get_modulation_target(self, slot):
        """Return the slot's target DeviceParameter, or None."""
        return modulators.mapped_parameter(native, self, slot)
    methods = locals()
    try:
        for cls, names in declarations:
            for name in names:
                patch(cls, name, methods[name])
            props = LomTypes.AVAILABLE_TYPE_PROPERTIES[cls]
            known = {p.name for p in props}
            additions = tuple(LomTypes.MFLProperty(n) for n in names if n not in known)
            native.properties.append((cls, additions))
            LomTypes.AVAILABLE_TYPE_PROPERTIES[cls] = tuple(props) + additions
    except Exception:
        native.uninstall()
        raise
    # Keep the loaded image and its ctypes callables alive.
    if not hasattr(Live, '_willington_device_libraries'):
        Live._willington_device_libraries = []
    Live._willington_device_libraries.append(native)
    return native
